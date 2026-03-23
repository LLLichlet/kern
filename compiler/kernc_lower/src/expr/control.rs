use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::checker::ConstEvaluator;
use kernc_sema::checker::Substituter;
use kernc_sema::def::Def;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
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
                            self.defer_stack.last_mut().unwrap().push(lowered);
                        } else {
                            if let ExprKind::Let { pattern, init } = &e.kind {
                                let init_mast = self.lower_expr(init, subst_map, None);
                                // 如果是忽略绑定，转换为单纯的 ExprStmt 执行副作用
                                if self.ctx.resolve(pattern.name) == "_" {
                                    stmts.push(MastStmt::Expr(init_mast));
                                } else {
                                    let var_ty = init_mast.ty;
                                    let is_mut = pattern.is_mut;

                                    self.local_types
                                        .last_mut()
                                        .unwrap()
                                        .insert(pattern.name, (var_ty, is_mut));
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

        let popped_defers = self.defer_stack.pop().unwrap();
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

    pub(crate) fn lower_lambda_expr(
        &mut self,
        params: &[ast::FuncParam],
        body: &Expr,
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let mono_id = self.new_mono_id();

        // 1. 生成独一无二的内部函数名，直接映射到 C ABI
        let lambda_name = format!("__kern_lambda_{}", mono_id.0);

        // 2. 从 TypeId 提取确切的签名
        let (param_tys, ret_ty) = if let TypeKind::Function { params, ret, .. } =
            self.ctx.type_registry.get(concrete_ty)
        {
            (params.clone(), *ret)
        } else {
            self.ctx.emit_ice(
                body.span,
                "Kern ICE (Lowering): Lambda expression does not have a Function type.",
            );
            unreachable!()
        };

        let mut mast_params = Vec::new();
        for (i, p) in params.iter().enumerate() {
            mast_params.push(MastParam {
                name: p.pattern.name,
                ty: param_tys[i],
                is_mut: p.pattern.is_mut,
            });
        }

        // ==========================================
        // 全面保护并清空外部上下文 (Context Isolation)
        // ==========================================
        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        // 为 Lambda 开启自己独有的局部作用域
        self.local_types.push(HashMap::new());
        for p in &mast_params {
            self.local_types
                .last_mut()
                .unwrap()
                .insert(p.name, (p.ty, p.is_mut));
        }

        // 3. 降级 Lambda 的函数体 (Block)
        let body_block = self.lower_block_as_body(body, subst_map, ret_ty);

        self.local_types.pop();

        // ==========================================
        // 恢复外部的作用域
        // ==========================================
        self.local_types = saved_local_types;
        self.defer_stack = saved_defer_stack;
        self.loop_frames = saved_loop_frames;
        self.local_statics = saved_local_statics;

        // 4. 将提取出的逻辑打包成一个全局的 MastFunction
        let mast_fn = MastFunction {
            id: mono_id,
            name: lambda_name,
            params: mast_params,
            ret_ty,
            body: Some(body_block),
            is_extern: false,
            is_variadic: false,
            attributes: vec![],
        };

        // 强势插入到模块的顶层函数列表中
        self.module.functions.push(mast_fn);

        // 5. 在原地返回一个指向该新函数的指针引用
        MastExprKind::FuncRef(mono_id)
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
        let t = self.lower_expr(target, subst_map, None);
        let norm_target_ty = self.ctx.type_registry.normalize(t.ty);

        // 1. 判断要匹配的 Target 到底是不是一个 Enum (ADT)
        let is_adt = matches!(
            self.ctx.type_registry.get(norm_target_ty),
            TypeKind::Enum(..)
        );

        // 提取 ADT 专用的元数据
        let (mono_id, gen_args, adt_def, is_pure) = if is_adt {
            let (def_id, args) = if let TypeKind::Enum(id, args) =
                self.ctx.type_registry.get(norm_target_ty).clone()
            {
                (id, args)
            } else {
                self.ctx.emit_ice(target.span, "Kern ICE (Lowering): Target type is an Enum but failed to destruct TypeKind::Enum.");
                unreachable!()
            };

            let def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
                d.clone()
            } else {
                self.ctx.emit_ice(
                    target.span,
                    format!(
                        "Kern ICE (Lowering): DefId {} is not an Enum Definition.",
                        def_id.0
                    ),
                );
                unreachable!()
            };

            let pure = self.is_pure_enum(&def);
            let m_id = if !pure {
                self.instantiate_data(def_id, &args)
            } else {
                MonoId(0)
            };
            (Some(m_id), args, Some(def), pure)
        } else {
            (None, vec![], None, true) // 对于普通整数视作 None
        };

        // 2. 确定给 LLVM switch 指令用的 target 值 (u128/u32)
        let tag_access = if is_adt && !is_pure {
            MastExpr::new(
                TypeId::U32,
                MastExprKind::FieldAccess {
                    lhs: Box::new(t.clone()),
                    struct_id: mono_id.unwrap(),
                    field_idx: 0, // __tag 字段
                },
                target.span,
            )
        } else {
            t.clone() // Pure Enum 或是普通整数，直接就是值本身
        };

        let mut mast_cases = Vec::new();
        let mut def_case = None;

        // 3. 遍历解析所有分支
        for arm in arms {
            let mut case_vals = Vec::new();
            let mut has_catch_all = false;
            let mut bound_variant = None; // 用于记录当前分支是否有需要解包的负载

            // 处理形如 `1, 2, 3 =>` 或 `.Ok, .Err =>` 的多模式组合
            for pat in &arm.patterns {
                match &pat.kind {
                    ast::MatchPatternKind::Value(val_expr) => {
                        if is_adt {
                            // 对于 ADT，普通值只能是 EnumLiteral，需要转化为对应的 Tag Index
                            if let ExprKind::EnumLiteral(variant_name) = &val_expr.kind {
                                let tag_idx = match adt_def
                                    .as_ref()
                                    .unwrap()
                                    .variants
                                    .iter()
                                    .position(|v| v.name == *variant_name)
                                {
                                    Some(idx) => idx,
                                    None => {
                                        self.ctx.emit_ice(val_expr.span, format!("Kern ICE (Lowering): Variant `{}` not found in enum.", self.ctx.resolve(*variant_name)));
                                        unreachable!()
                                    }
                                };
                                case_vals.push(tag_idx as u128);
                            }
                        } else {
                            // 对于普通整数/字符匹配，在编译期求出具体的值
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
                        // 展开 Range: 1..=3 变成 LLVM switch case 里的 1, 2, 3
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
                        let tag_idx = adt_def
                            .as_ref()
                            .unwrap()
                            .variants
                            .iter()
                            .position(|v| v.name == *variant_name)
                            .unwrap();
                        case_vals.push(tag_idx as u128);
                        bound_variant = Some((tag_idx, variant_name, binding));
                    }
                    ast::MatchPatternKind::CatchAll => {
                        has_catch_all = true;
                    }
                }
            }

            if has_catch_all {
                def_case = Some(self.lower_block_as_body(&arm.body, subst_map, exp_ty));
            } else {
                // 降级代码块。如果是带有载荷的 Variant 匹配，必须生成 payload 提取逻辑
                let body_block = if let Some((tag_idx, _v_name, binding)) = bound_variant {
                    let variant_def = &adt_def.as_ref().unwrap().variants[tag_idx];
                    self.lower_match_variant_arm(
                        arm,
                        binding.as_ref(),
                        &t,
                        mono_id.unwrap(),
                        tag_idx,
                        variant_def,
                        &adt_def.as_ref().unwrap().generics,
                        &gen_args,
                        subst_map,
                        exp_ty,
                        is_pure,
                    )
                } else {
                    self.lower_block_as_body(&arm.body, subst_map, exp_ty)
                };

                mast_cases.push(MastSwitchCase {
                    values: case_vals,
                    body: body_block,
                });
            }
        }

        MastExprKind::Switch {
            target: Box::new(tag_access),
            cases: mast_cases,
            default_case: def_case,
        }
    }

    pub(crate) fn lower_match_variant_arm(
        &mut self,
        arm: &ast::MatchArm,
        binding: Option<&ast::BindingPattern>,
        target_expr: &MastExpr,
        mono_id: MonoId,
        tag_idx: usize,
        variant_def: &ast::EnumVariant,
        def_generics: &[ast::GenericParam],
        gen_args: &[TypeId],
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
        is_pure: bool,
    ) -> MastBlock {
        self.local_types.push(HashMap::new());
        let mut arm_stmts = Vec::new();

        // 如果不是 Pure 且用户要求绑定变量，提取负载
        if !is_pure {
            if let Some(bind_pattern) = binding {
                let payload_type_id = match &variant_def.payload_type {
                    Some(ast) => ast.id,
                    None => {
                        self.ctx.emit_ice(arm.span, format!("Kern ICE (Lowering): Attempted to bind payload to a variant `{}` without a payload.", self.ctx.resolve(variant_def.name)));
                        unreachable!()
                    }
                };

                let mut payload_ty = self
                    .ctx
                    .node_types
                    .get(&payload_type_id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if !def_generics.is_empty() && !gen_args.is_empty() {
                    let mut var_map = HashMap::new();
                    for (i, param) in def_generics.iter().enumerate() {
                        var_map.insert(param.name, gen_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &var_map);
                    payload_ty = subst.substitute(payload_ty);
                }

                let union_access = MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(target_expr.clone()),
                        struct_id: mono_id,
                        field_idx: 1, // __payload
                    },
                    arm.span,
                );

                let target_union_id = match self.adt_union_map.get(&mono_id) {
                    Some(&id) => id,
                    None => {
                        self.ctx.emit_ice(arm.span, "Kern ICE (Lowering): Missing Enum Union payload mapping in adt_union_map.");
                        unreachable!()
                    }
                };

                let payload_extract = MastExpr::new(
                    payload_ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(union_access),
                        struct_id: target_union_id,
                        field_idx: tag_idx,
                    },
                    arm.span,
                );

                self.local_types
                    .last_mut()
                    .unwrap()
                    .insert(bind_pattern.name, (payload_ty, bind_pattern.is_mut));
                arm_stmts.push(MastStmt::Let {
                    name: bind_pattern.name,
                    ty: payload_ty,
                    is_mut: bind_pattern.is_mut,
                    init: payload_extract,
                });
            }
        }

        let mut block = self.lower_block_as_body(&arm.body, subst_map, exp_ty);
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
        let boundary = match self.loop_frames.last() {
            Some(&b) => b,
            None => {
                self.ctx.emit_ice(span, "Kern ICE (Lowering): `break` or `continue` found outside of any loop frame! Sema missed this context check.");
                unreachable!()
            }
        };

        // 从当前栈顶一路向下倒序提取 defer，直到达到循环的边界
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
