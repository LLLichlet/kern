use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::checker::ConstEvaluator;
use kernc_sema::checker::Substituter;
use kernc_sema::def::Def;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

#[derive(Clone)]
enum MatchAdtInfo {
    Named {
        mono_id: MonoId,
        gen_args: Vec<TypeId>,
        def: kernc_sema::def::EnumDef,
        is_pure: bool,
        tag_ty: TypeId,
    },
    Anonymous {
        mono_id: MonoId,
        def: kernc_sema::ty::AnonymousEnum,
        is_pure: bool,
        tag_ty: TypeId,
    },
}

pub(super) struct ClosureLowerSpec<'a> {
    pub node_id: NodeId,
    pub captures: &'a [ast::CapturePattern],
    pub params: &'a [ast::FuncParam],
    pub body: &'a Expr,
    pub concrete_ty: TypeId,
    pub subst_map: &'a HashMap<SymbolId, TypeId>,
    pub exp_ty: TypeId,
}

struct NamedVariantArmSpec<'a> {
    arm: &'a ast::MatchArm,
    binding: Option<&'a ast::BindingPattern>,
    target_expr: &'a MastExpr,
    mono_id: MonoId,
    tag_idx: usize,
    variant_def: &'a ast::EnumVariant,
    def_generics: &'a [ast::GenericParam],
    gen_args: &'a [TypeId],
    subst_map: &'a HashMap<SymbolId, TypeId>,
    exp_ty: TypeId,
    is_pure: bool,
}

struct AnonymousVariantArmSpec<'a> {
    arm: &'a ast::MatchArm,
    binding: Option<&'a ast::BindingPattern>,
    target_expr: &'a MastExpr,
    mono_id: MonoId,
    variant_idx: usize,
    variant_def: &'a kernc_sema::ty::AnonymousVariant,
    subst_map: &'a HashMap<SymbolId, TypeId>,
    exp_ty: TypeId,
    is_pure: bool,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn push_defer_in_current_scope(&mut self, span: Span, deferred: MastExpr) {
        if let Some(scope) = self.defer_stack.last_mut() {
            scope.push(deferred);
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): attempted to register `defer` without an active block scope.",
            );
        }
    }

    fn bind_local_type(
        &mut self,
        span: Span,
        name: SymbolId,
        ty: TypeId,
        is_mut: bool,
        context: &str,
    ) -> bool {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, is_mut));
            true
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): missing local type scope while binding `{}` in {}.",
                    self.ctx.resolve(name),
                    context
                ),
            );
            false
        }
    }

    fn pop_defer_scope(&mut self, span: Span) -> Vec<MastExpr> {
        match self.defer_stack.pop() {
            Some(scope) => scope,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): attempted to exit a block with an empty defer stack.",
                );
                Vec::new()
            }
        }
    }

    fn anonymous_state_signature(
        &mut self,
        concrete_ty: TypeId,
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        match self.ctx.type_registry.get(concrete_ty).clone() {
            TypeKind::AnonymousState { params, ret, .. } => Some((params, ret)),
            other => {
                let concrete_ty_str = self.ctx.ty_to_string(concrete_ty);
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): closure expected `AnonymousState`, found `{}` ({other:?}).",
                        concrete_ty_str
                    ),
                );
                None
            }
        }
    }

    fn lower_closure_captures(
        &mut self,
        captures: &[ast::CapturePattern],
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> (Vec<MastField>, Vec<MastExpr>) {
        let mut env_struct_fields = Vec::new();
        let mut cap_exprs = Vec::new();

        for cap in captures {
            let cap_mast = self.lower_expr(&cap.value, subst_map, None);
            env_struct_fields.push(MastField {
                name: cap.name,
                ty: cap_mast.ty,
            });
            cap_exprs.push(cap_mast);
        }

        (env_struct_fields, cap_exprs)
    }

    fn register_closure_state_struct(&mut self, struct_id: MonoId, fields: Vec<MastField>) {
        self.module.structs.push(MastStruct {
            id: struct_id,
            name: format!("__closure_state_{}", struct_id.0),
            fields,
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            attributes: vec![],
        });
    }

    fn build_closure_fat_pointer(
        &mut self,
        norm_exp: TypeId,
        concrete_ty: TypeId,
        func_id: MonoId,
        struct_init: &MastExpr,
    ) -> Option<MastExprKind> {
        match self.ctx.type_registry.get(norm_exp).clone() {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let inner_norm = self.ctx.type_registry.normalize(elem);
                if !matches!(
                    self.ctx.type_registry.get(inner_norm),
                    TypeKind::ClosureInterface { .. }
                ) {
                    return None;
                }

                let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem: TypeId::VOID,
                });
                let struct_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem: concrete_ty,
                });

                let data_ptr = MastExpr::new(
                    struct_ptr_ty,
                    MastExprKind::AddressOf(Box::new(struct_init.clone())),
                    Span::default(),
                );
                let data_ptr_cast = MastExpr::new(
                    void_ptr_ty,
                    MastExprKind::Cast {
                        kind: MastCastKind::Bitcast,
                        operand: Box::new(data_ptr),
                    },
                    Span::default(),
                );

                let func_ref = MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::FuncRef(func_id),
                    Span::default(),
                );
                let code_ptr_cast = MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Cast {
                        kind: MastCastKind::PtrToInt,
                        operand: Box::new(func_ref),
                    },
                    Span::default(),
                );

                Some(MastExprKind::ConstructFatPointer {
                    data_ptr: Box::new(data_ptr_cast),
                    meta: Box::new(code_ptr_cast),
                })
            }
            _ => None,
        }
    }

    fn resolve_named_match_adt(
        &mut self,
        def_id: kernc_sema::def::DefId,
        args: Vec<TypeId>,
        span: Span,
    ) -> Option<MatchAdtInfo> {
        let Def::Enum(def) = self.ctx.defs[def_id.0 as usize].clone() else {
            self.ctx.emit_ice(
                span,
                format!("Kern ICE (Lowering): DefId {} is not an Enum.", def_id.0),
            );
            return None;
        };

        let pure = self.is_pure_enum(&def);
        let mono_id = if pure {
            MonoId(0)
        } else {
            self.instantiate_data(def_id, &args)
        };
        let tag_ty = def.backing_type.as_ref().map_or(TypeId::U32, |backing_ty| {
            self.ctx
                .node_types
                .get(&backing_ty.id)
                .copied()
                .unwrap_or(TypeId::U32)
        });

        Some(MatchAdtInfo::Named {
            mono_id,
            gen_args: args,
            def,
            is_pure: pure,
            tag_ty,
        })
    }

    fn build_match_tag_expr(
        &mut self,
        target_var_expr: &MastExpr,
        adt_info: &Option<MatchAdtInfo>,
        span: Span,
    ) -> MastExpr {
        let Some(info) = adt_info else {
            return target_var_expr.clone();
        };

        let (mono_id, is_pure, tag_ty) = match info {
            MatchAdtInfo::Named {
                mono_id,
                is_pure,
                tag_ty,
                ..
            }
            | MatchAdtInfo::Anonymous {
                mono_id,
                is_pure,
                tag_ty,
                ..
            } => (*mono_id, *is_pure, *tag_ty),
        };

        if is_pure {
            target_var_expr.clone()
        } else {
            MastExpr::new(
                tag_ty,
                MastExprKind::FieldAccess {
                    lhs: Box::new(target_var_expr.clone()),
                    struct_id: mono_id,
                    field_idx: 0,
                },
                span,
            )
        }
    }

    fn resolve_match_variant(
        &mut self,
        info: &MatchAdtInfo,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<(usize, u128)> {
        match info {
            MatchAdtInfo::Named { def, .. } => {
                self.named_enum_variant_info(def, variant_name, span)
            }
            MatchAdtInfo::Anonymous { def, .. } => {
                let variant = self.anon_enum_variant_info(def, variant_name);
                if variant.is_none() {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): variant `{}` not found in anonymous enum match.",
                            self.ctx.resolve(variant_name)
                        ),
                    );
                }
                variant
            }
        }
    }

    fn payload_union_id(&mut self, mono_id: MonoId, span: Span) -> Option<MonoId> {
        match self.adt_union_map.get(&mono_id).copied() {
            Some(id) => Some(id),
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): missing enum payload union mapping in `adt_union_map`.",
                );
                None
            }
        }
    }

    fn bind_payload_pattern(
        &mut self,
        span: Span,
        binding: &ast::BindingPattern,
        target_expr: &MastExpr,
        mono_id: MonoId,
        field_idx: usize,
        payload_ty: TypeId,
    ) -> Option<MastStmt> {
        let target_union_id = self.payload_union_id(mono_id, span)?;
        let union_access = MastExpr::new(
            TypeId::VOID,
            MastExprKind::FieldAccess {
                lhs: Box::new(target_expr.clone()),
                struct_id: mono_id,
                field_idx: 1,
            },
            span,
        );
        let payload_extract = MastExpr::new(
            payload_ty,
            MastExprKind::FieldAccess {
                lhs: Box::new(union_access),
                struct_id: target_union_id,
                field_idx,
            },
            span,
        );

        self.bind_local_type(
            span,
            binding.name,
            payload_ty,
            binding.is_mut,
            "match arm payload binding",
        );

        Some(MastStmt::Let {
            name: binding.name,
            ty: payload_ty,
            is_mut: binding.is_mut,
            init: payload_extract,
        })
    }

    pub(crate) fn lower_block_as_body(
        &mut self,
        block_expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        expected_ty: TypeId,
    ) -> MastBlock {
        self.defer_stack.push(Vec::new());
        self.local_types.push(HashMap::new());
        self.local_statics.push(HashMap::new());

        let mut stmts = Vec::new();
        let mut result = None;

        if let ExprKind::Block {
            stmts: ast_stmts,
            result: ast_res,
        } = &block_expr.kind
        {
            for stmt in ast_stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                        if let ExprKind::Defer { expr: def_expr } = &e.kind {
                            let lowered = self.lower_expr(def_expr, subst_map, None);
                            self.push_defer_in_current_scope(e.span, lowered);
                        } else if let ExprKind::Let { pattern, init } = &e.kind {
                            let init_mast = self.lower_expr(init, subst_map, None);
                            // 如果是忽略绑定，转换为单纯的 ExprStmt 执行副作用
                            if self.ctx.resolve(pattern.name) == "_" {
                                stmts.push(MastStmt::Expr(init_mast));
                            } else {
                                let var_ty = init_mast.ty;
                                let is_mut = pattern.is_mut;

                                self.bind_local_type(
                                    e.span,
                                    pattern.name,
                                    var_ty,
                                    is_mut,
                                    "block local binding",
                                );
                                stmts.push(MastStmt::Let {
                                    name: pattern.name,
                                    ty: var_ty,
                                    is_mut,
                                    init: init_mast,
                                });
                            }
                        } else {
                            let lowered = self.lower_expr(e, subst_map, None);
                            if !matches!(e.kind, ExprKind::Static { .. }) {
                                stmts.push(MastStmt::Expr(lowered));
                            }
                        }
                    }
                }
            }
            if let Some(res) = ast_res {
                result = Some(Box::new(self.lower_expr(res, subst_map, Some(expected_ty))));
            }
        } else {
            result = Some(Box::new(self.lower_expr(
                block_expr,
                subst_map,
                Some(expected_ty),
            )));
        }

        let popped_defers = self.pop_defer_scope(block_expr.span);
        let mut defers = Vec::new();
        for d in popped_defers.into_iter().rev() {
            defers.push(d); // 保持 LIFO 顺序存入单独的数组
        }

        self.local_types.pop();
        self.local_statics.pop();
        MastBlock {
            stmts,
            result,
            defers,
        } // 将 defers 独立传递给后端
    }

    pub(super) fn lower_closure_expr(&mut self, spec: ClosureLowerSpec<'_>) -> MastExprKind {
        let struct_id = self.new_mono_id();
        let func_id = self.new_mono_id();
        self.closure_fn_map.insert(spec.node_id, func_id);

        // 0. ================= 嗅探 Decay (退化) 上下文 =================
        let norm_exp = self.ctx.type_registry.normalize(spec.exp_ty);
        let is_decay = spec.captures.is_empty()
            && matches!(
                self.ctx.type_registry.get(norm_exp).clone(),
                TypeKind::Function { .. } | TypeKind::FnDef(..)
            );

        // 1. ================= 构建捕获状态结构体 =================
        let (env_struct_fields, cap_exprs) =
            self.lower_closure_captures(spec.captures, spec.subst_map);
        self.register_closure_state_struct(struct_id, env_struct_fields.clone());

        // 2. ================= 构建闭包执行函数 =================
        let env_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: spec.concrete_ty,
        });

        let mut mast_params = Vec::new();
        let env_sym = self.ctx.intern("__env");

        // 如果是 Decay 退化为 C ABI 静态函数，不生成隐藏的上下文指针
        if !is_decay {
            mast_params.push(MastParam {
                name: env_sym,
                ty: env_ptr_ty,
                is_mut: false,
            });
        }

        let Some((param_tys, ret_ty)) =
            self.anonymous_state_signature(spec.concrete_ty, spec.body.span)
        else {
            return MastExprKind::Trap;
        };

        for (i, p) in spec.params.iter().enumerate() {
            mast_params.push(MastParam {
                name: p.pattern.name,
                ty: param_tys[i],
                is_mut: p.pattern.is_mut,
            });
        }

        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        self.local_types.push(HashMap::new());
        for p in &mast_params {
            self.bind_local_type(spec.body.span, p.name, p.ty, p.is_mut, "closure parameter");
        }

        // 还原捕获的值 (只有在非退化时才需要)
        let mut injected_stmts = Vec::new();
        if !is_decay {
            let env_var_expr =
                MastExpr::new(env_ptr_ty, MastExprKind::Var(env_sym), Span::default());

            for (i, cap) in spec.captures.iter().enumerate() {
                let deref_env = MastExpr::new(
                    spec.concrete_ty,
                    MastExprKind::Deref(Box::new(env_var_expr.clone())),
                    Span::default(),
                );
                let field_access = MastExpr::new(
                    env_struct_fields[i].ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(deref_env),
                        struct_id,
                        field_idx: i,
                    },
                    Span::default(),
                );

                injected_stmts.push(MastStmt::Let {
                    name: cap.name,
                    ty: env_struct_fields[i].ty,
                    is_mut: false,
                    init: field_access,
                });
                self.bind_local_type(
                    spec.body.span,
                    cap.name,
                    env_struct_fields[i].ty,
                    false,
                    "closure capture restore",
                );
            }
        }

        let mut body_block = self.lower_block_as_body(spec.body, spec.subst_map, ret_ty);
        injected_stmts.append(&mut body_block.stmts);
        body_block.stmts = injected_stmts;

        self.local_types.pop();

        self.local_types = saved_local_types;
        self.defer_stack = saved_defer_stack;
        self.loop_frames = saved_loop_frames;
        self.local_statics = saved_local_statics;

        self.module.functions.push(MastFunction {
            id: func_id,
            name: format!("__closure_fn_{}", func_id.0),
            params: mast_params,
            ret_ty,
            body: Some(body_block),
            is_extern: false,
            is_variadic: false,
            attributes: vec![],
        });

        // 3. ================= 组装当前位置的表达式 =================
        let struct_init = MastExpr::new(
            spec.concrete_ty,
            MastExprKind::StructInit {
                struct_id,
                fields: cap_exprs,
            },
            Span::default(),
        );

        // 4. ================= 处理 BNC 和 退化规则 =================
        if is_decay {
            // 直接返回生成的静态 C 函数指针
            return MastExprKind::FuncRef(func_id);
        }

        if let Some(fat_ptr) =
            self.build_closure_fat_pointer(norm_exp, spec.concrete_ty, func_id, &struct_init)
        {
            return fat_ptr;
        }

        struct_init.kind
    }

    pub(crate) fn lower_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let c = self.lower_expr(cond, subst_map, Some(TypeId::BOOL));
        let t = self.lower_block_as_body(then_branch, subst_map, exp_ty);
        let e = else_branch.map(|eb| self.lower_block_as_body(eb, subst_map, exp_ty));
        MastExprKind::If {
            cond: Box::new(c),
            then_branch: t,
            else_branch: e,
        }
    }

    pub(crate) fn lower_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let mut loop_stmts = Vec::new();

        if let Some(c) = cond {
            let c_expr = self.lower_expr(c, subst_map, Some(TypeId::BOOL));
            let not_c = MastExpr::new(
                TypeId::BOOL,
                MastExprKind::Unary {
                    op: ast::UnaryOperator::LogicalNot,
                    operand: Box::new(c_expr),
                },
                c.span,
            );

            loop_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(not_c),
                    then_branch: MastBlock {
                        stmts: vec![MastStmt::Expr(MastExpr::new(
                            TypeId::VOID,
                            MastExprKind::Break,
                            c.span,
                        ))],
                        result: None,
                        defers: vec![],
                    },
                    else_branch: None,
                },
                c.span,
            )));
        }

        // 记录进入循环体前 defer_stack 的高度
        self.loop_frames.push(self.defer_stack.len());
        // 仅仅降级循环体，不包含 post
        loop_stmts.push(MastStmt::Expr(self.lower_expr(body, subst_map, None)));

        let body_block = MastBlock {
            stmts: loop_stmts,
            result: None,
            defers: vec![],
        };

        // 独立降级 post 语句，将其作为 Latch 块
        let latch_block = post.map(|p| MastBlock {
            stmts: vec![MastStmt::Expr(self.lower_expr(p, subst_map, None))],
            result: None,
            defers: vec![],
        });

        // 退出循环体，弹出边界
        self.loop_frames.pop();

        let loop_expr = MastExpr::new(
            TypeId::VOID,
            // 采用新的 AST 结构
            MastExprKind::Loop {
                body: body_block,
                latch: latch_block,
            },
            span,
        );

        if let Some(i) = init {
            let mut outer_stmts = Vec::new();
            if let ExprKind::Let {
                pattern,
                init: let_init,
            } = &i.kind
            {
                let lowered_init = self.lower_expr(let_init, subst_map, None);
                outer_stmts.push(MastStmt::Let {
                    name: pattern.name,
                    ty: lowered_init.ty,
                    init: lowered_init,
                    is_mut: pattern.is_mut,
                });
            } else {
                outer_stmts.push(MastStmt::Expr(self.lower_expr(i, subst_map, None)));
            }
            outer_stmts.push(MastStmt::Expr(loop_expr));
            MastExprKind::Block(MastBlock {
                stmts: outer_stmts,
                result: None,
                defers: vec![],
            })
        } else {
            loop_expr.kind
        }
    }

    pub(crate) fn lower_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let lowered_target = self.lower_expr(target, subst_map, None);
        let target_ty = lowered_target.ty;

        // 1. 创建局部绑定 (Let Binding)，防止重复执行副作用
        let (let_stmt, target_var_expr) =
            self.build_match_target_binding(target_ty, lowered_target, target.span);

        // 2. 解析目标类型的枚举/ADT信息
        let adt_info = self.resolve_match_adt(target_ty, target.span);

        // 3. 构建 Switch 的 Tag 提取表达式
        let tag_access = self.build_match_tag_expr(&target_var_expr, &adt_info, target.span);

        // 4. 解析所有的分支 (Arms)
        let (mast_cases, def_case) =
            self.lower_match_arms(arms, &target_var_expr, &adt_info, subst_map, exp_ty);

        // 5. 组装最终的 Block 表达式
        let switch_expr = MastExpr::new(
            exp_ty,
            MastExprKind::Switch {
                target: Box::new(tag_access),
                cases: mast_cases,
                default_case: def_case,
            },
            target.span,
        );

        MastExprKind::Block(MastBlock {
            stmts: vec![let_stmt],
            result: Some(Box::new(switch_expr)),
            defers: vec![],
        })
    }

    /// 辅助方法：生成临时 Let 绑定，隔离作用域
    fn build_match_target_binding(
        &mut self,
        target_ty: TypeId,
        lowered_target: MastExpr,
        span: Span,
    ) -> (MastStmt, MastExpr) {
        let new_mono_id = self.new_mono_id();
        let tmp_sym = self
            .ctx
            .intern(&format!("__match_target_{}", new_mono_id.0));

        self.bind_local_type(span, tmp_sym, target_ty, false, "match target binding");

        let let_stmt = MastStmt::Let {
            name: tmp_sym,
            ty: target_ty,
            is_mut: false,
            init: lowered_target,
        };

        let target_var_expr = MastExpr::new(target_ty, MastExprKind::Var(tmp_sym), span);

        (let_stmt, target_var_expr)
    }

    /// 辅助方法：解析目标类型的 ADT 元数据
    fn resolve_match_adt(&mut self, target_ty: TypeId, span: Span) -> Option<MatchAdtInfo> {
        let norm_target_ty = self.ctx.type_registry.normalize(target_ty);

        match self.ctx.type_registry.get(norm_target_ty).clone() {
            TypeKind::Enum(def_id, args) => self.resolve_named_match_adt(def_id, args, span),
            TypeKind::AnonymousEnum(def) => {
                let pure = def
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none());
                let mono_id = if !pure {
                    self.instantiate_anon_enum(norm_target_ty)
                } else {
                    MonoId(0)
                };
                let tag_ty = def.backing_ty.unwrap_or(TypeId::U32);

                Some(MatchAdtInfo::Anonymous {
                    mono_id,
                    def,
                    is_pure: pure,
                    tag_ty,
                })
            }
            _ => None,
        }
    }

    /// 辅助方法：解析并降级 Match 的所有分支
    fn lower_match_arms(
        &mut self,
        arms: &[ast::MatchArm],
        target_var_expr: &MastExpr,
        adt_info: &Option<MatchAdtInfo>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> (Vec<MastSwitchCase>, Option<MastBlock>) {
        let mut mast_cases = Vec::new();
        let mut def_case = None;

        for arm in arms {
            let mut case_vals = Vec::new();
            let mut has_catch_all = false;
            let mut bound_variant = None;

            for pat in &arm.patterns {
                match &pat.kind {
                    ast::MatchPatternKind::Value(val_expr) => {
                        if let Some(info) = adt_info {
                            if let ExprKind::EnumLiteral(variant_name) = &val_expr.kind
                                && let Some((_, tag_value)) =
                                    self.resolve_match_variant(info, *variant_name, val_expr.span)
                            {
                                case_vals.push(tag_value);
                            }
                        } else {
                            let mut ce = ConstEvaluator::new(self.ctx);
                            if let Ok(val) = ce.eval_math(val_expr) {
                                case_vals.push(val as u128);
                            }
                        }
                    }
                    ast::MatchPatternKind::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let mut ce = ConstEvaluator::new(self.ctx);
                        if let (Ok(s), Ok(e)) = (ce.eval_math(start), ce.eval_math(end)) {
                            let end_bound = if *inclusive { e } else { e - 1 };
                            for v in s..=end_bound {
                                case_vals.push(v as u128);
                            }
                        }
                    }
                    ast::MatchPatternKind::Variant {
                        variant_name,
                        binding,
                        ..
                    } => {
                        let Some(info) = adt_info.as_ref() else {
                            self.ctx.emit_ice(
                                pat.span,
                                "Kern ICE (Lowering): variant match pattern lowered without enum metadata.",
                            );
                            continue;
                        };

                        if let Some((variant_idx, tag_value)) =
                            self.resolve_match_variant(info, *variant_name, pat.span)
                        {
                            case_vals.push(tag_value);
                            bound_variant = Some((variant_idx, variant_name, binding));
                        }
                    }
                    ast::MatchPatternKind::CatchAll => {
                        has_catch_all = true;
                    }
                }
            }

            if has_catch_all {
                def_case = Some(self.lower_block_as_body(&arm.body, subst_map, exp_ty));
            } else {
                let body_block = if let Some((tag_idx, _v_name, binding)) = bound_variant {
                    match adt_info.as_ref() {
                        Some(MatchAdtInfo::Named {
                            mono_id,
                            gen_args,
                            def,
                            is_pure,
                            ..
                        }) => {
                            let variant_def = &def.variants[tag_idx];
                            self.lower_match_variant_arm(NamedVariantArmSpec {
                                arm,
                                binding: binding.as_ref(),
                                target_expr: target_var_expr,
                                mono_id: *mono_id,
                                tag_idx,
                                variant_def,
                                def_generics: &def.generics,
                                gen_args,
                                subst_map,
                                exp_ty,
                                is_pure: *is_pure,
                            })
                        }
                        Some(MatchAdtInfo::Anonymous {
                            mono_id,
                            def,
                            is_pure,
                            ..
                        }) => {
                            let variant_def = &def.variants[tag_idx];
                            self.lower_anon_match_variant_arm(AnonymousVariantArmSpec {
                                arm,
                                binding: binding.as_ref(),
                                target_expr: target_var_expr,
                                mono_id: *mono_id,
                                variant_idx: tag_idx,
                                variant_def,
                                subst_map,
                                exp_ty,
                                is_pure: *is_pure,
                            })
                        }
                        None => {
                            self.ctx.emit_ice(
                                arm.span,
                                "Kern ICE (Lowering): bound match arm lacks enum metadata.",
                            );
                            self.lower_block_as_body(&arm.body, subst_map, exp_ty)
                        }
                    }
                } else {
                    self.lower_block_as_body(&arm.body, subst_map, exp_ty)
                };

                mast_cases.push(MastSwitchCase {
                    values: case_vals,
                    body: body_block,
                });
            }
        }
        (mast_cases, def_case)
    }

    fn lower_match_variant_arm(&mut self, spec: NamedVariantArmSpec<'_>) -> MastBlock {
        self.local_types.push(HashMap::new());
        let mut arm_stmts = Vec::new();

        // 如果不是 Pure 且用户要求绑定变量，提取负载
        if !spec.is_pure
            && let Some(bind_pattern) = spec.binding
        {
            let Some(payload_ast) = &spec.variant_def.payload_type else {
                self.ctx.emit_ice(
                    spec.arm.span,
                    format!(
                        "Kern ICE (Lowering): attempted to bind payload to variant `{}` without payload.",
                        self.ctx.resolve(spec.variant_def.name)
                    ),
                );
                let block = self.lower_block_as_body(&spec.arm.body, spec.subst_map, spec.exp_ty);
                self.local_types.pop();
                return block;
            };

            let mut payload_ty = self
                .ctx
                .node_types
                .get(&payload_ast.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            if !spec.def_generics.is_empty() && !spec.gen_args.is_empty() {
                let mut var_map = HashMap::new();
                for (i, param) in spec.def_generics.iter().enumerate() {
                    var_map.insert(param.name, spec.gen_args[i]);
                }
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &var_map);
                payload_ty = subst.substitute(payload_ty);
            }

            if let Some(payload_stmt) = self.bind_payload_pattern(
                spec.arm.span,
                bind_pattern,
                spec.target_expr,
                spec.mono_id,
                spec.tag_idx,
                payload_ty,
            ) {
                arm_stmts.push(payload_stmt);
            }
        }

        let mut block = self.lower_block_as_body(&spec.arm.body, spec.subst_map, spec.exp_ty);
        arm_stmts.append(&mut block.stmts);
        block.stmts = arm_stmts;

        self.local_types.pop();
        block
    }

    fn lower_anon_match_variant_arm(&mut self, spec: AnonymousVariantArmSpec<'_>) -> MastBlock {
        self.local_types.push(HashMap::new());
        let mut arm_stmts = Vec::new();

        if !spec.is_pure
            && let Some(bind_pattern) = spec.binding
        {
            let Some(payload_ty) = spec.variant_def.payload_ty else {
                self.ctx.emit_ice(
                    spec.arm.span,
                    format!(
                        "Kern ICE (Lowering): attempted to bind payload to variant `{}` without payload.",
                        self.ctx.resolve(spec.variant_def.name)
                    ),
                );
                let block = self.lower_block_as_body(&spec.arm.body, spec.subst_map, spec.exp_ty);
                self.local_types.pop();
                return block;
            };

            if let Some(payload_stmt) = self.bind_payload_pattern(
                spec.arm.span,
                bind_pattern,
                spec.target_expr,
                spec.mono_id,
                spec.variant_idx,
                payload_ty,
            ) {
                arm_stmts.push(payload_stmt);
            }
        }

        let mut block = self.lower_block_as_body(&spec.arm.body, spec.subst_map, spec.exp_ty);
        arm_stmts.append(&mut block.stmts);
        block.stmts = arm_stmts;

        self.local_types.pop();
        block
    }

    pub(crate) fn lower_return(
        &mut self,
        val: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let v = val.map(|e| Box::new(self.lower_expr(e, subst_map, None)));
        let mut defer_stmts = Vec::new();

        // 倒序展开当前作用域栈中所有的 defer
        for stack in self.defer_stack.iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            MastExprKind::Return(v)
        } else {
            defer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::Return(v),
                span,
            )));
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }

    /// 专门处理 Break 和 Continue 的 Defer 展开
    pub(crate) fn lower_jump(&mut self, jump_kind: MastExprKind, span: Span) -> MastExprKind {
        let mut defer_stmts = Vec::new();

        // 获取当前所属循环的起始栈深度
        let boundary = match self.loop_frames.last().copied() {
            Some(b) => b,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): `break` or `continue` found outside any loop frame.",
                );
                return MastExprKind::Trap;
            }
        };

        // 从当前栈顶一路向下倒序提取 defer，直到达到循环的边界
        if boundary > self.defer_stack.len() {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): loop frame boundary exceeds current defer stack depth.",
            );
            return MastExprKind::Trap;
        }

        for stack in self.defer_stack[boundary..].iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            jump_kind
        } else {
            // 将实际的跳转指令放在所有清理工作之后
            defer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::NEVER,
                jump_kind,
                span,
            )));
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }
}
