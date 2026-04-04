use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
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

struct PatternBindingPlan {
    name: SymbolId,
    ty: TypeId,
    is_mut: bool,
    init: MastExpr,
}

type MatchVariantPayloadInfo = (usize, TypeId, MonoId);

struct MatchLowerContext<'a> {
    arms: &'a [ast::MatchArm],
    target_var_expr: &'a MastExpr,
    target_ty: TypeId,
    subst_map: &'a HashMap<SymbolId, TypeId>,
    exp_ty: TypeId,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_optional_stmt_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> Option<MastStmt> {
        if matches!(expr.kind, ExprKind::Assign { .. }) && self.is_pure_dead_assignment(expr.id) {
            return None;
        }

        let lowered = self.lower_expr(expr, subst_map, None);
        if matches!(expr.kind, ExprKind::Static { .. }) {
            return None;
        }

        Some(MastStmt::Expr(lowered))
    }

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
            if let Some(forward_scope) = self.local_forwardings.last_mut() {
                // A concrete local binding must shadow any forwarded alias from an outer scope.
                forward_scope.insert(name, name);
            }
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
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });
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
            MastExpr::new(tag_ty, target_var_expr.kind.clone(), span)
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

    fn build_payload_extract_expr(
        &mut self,
        span: Span,
        target_expr: &MastExpr,
        mono_id: MonoId,
        field_idx: usize,
        payload_ty: TypeId,
    ) -> Option<MastExpr> {
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

        Some(MastExpr::new(
            payload_ty,
            MastExprKind::FieldAccess {
                lhs: Box::new(union_access),
                struct_id: target_union_id,
                field_idx,
            },
            span,
        ))
    }

    fn is_ignored_binding(&self, name: SymbolId) -> bool {
        self.ctx.resolve(name) == "_"
    }

    fn resolve_struct_pattern_field(
        &mut self,
        target_ty: TypeId,
        field_name: SymbolId,
        span: Span,
    ) -> Option<(TypeId, MonoId, usize)> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Def(def_id, gen_args) => {
                let Def::Struct(def) = &self.ctx.defs[def_id.0 as usize] else {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): expected a struct definition while lowering a destructuring pattern.",
                    );
                    return None;
                };

                let ast_idx = def
                    .fields
                    .iter()
                    .position(|field| field.name == field_name)?;
                let mut field_ty = self
                    .ctx
                    .node_types
                    .get(&def.fields[ast_idx].type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if !def.generics.is_empty() && !gen_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def.generics.iter().enumerate() {
                        map.insert(param.name, gen_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    field_ty = subst.substitute(field_ty);
                }

                let field_idx = self.get_physical_field_index(target_ty, field_name, span);
                let struct_id = self.instantiate_struct(def_id, &gen_args);
                Some((field_ty, struct_id, field_idx))
            }
            TypeKind::AnonymousStruct(_, fields) => {
                let field_idx = fields.iter().position(|field| field.name == field_name)?;
                let struct_id = self.instantiate_anon_struct(norm_target);
                Some((fields[field_idx].ty, struct_id, field_idx))
            }
            _ => None,
        }
    }

    fn bool_expr(&self, span: Span, value: bool) -> MastExpr {
        MastExpr::new(TypeId::BOOL, MastExprKind::Bool(value), span)
    }

    fn and_expr(&self, span: Span, lhs: MastExpr, rhs: MastExpr) -> MastExpr {
        if matches!(lhs.kind, MastExprKind::Bool(true)) {
            return rhs;
        }
        if matches!(rhs.kind, MastExprKind::Bool(true)) {
            return lhs;
        }

        MastExpr::new(
            TypeId::BOOL,
            MastExprKind::If {
                cond: Box::new(lhs),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(rhs)),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(self.bool_expr(span, false))),
                    defers: vec![],
                }),
            },
            span,
        )
    }

    fn build_enum_variant_condition(
        &mut self,
        span: Span,
        target_expr: &MastExpr,
        target_ty: TypeId,
        variant_name: SymbolId,
    ) -> Option<(MastExpr, Option<MatchVariantPayloadInfo>)> {
        let adt_info = self.resolve_match_adt(target_ty, span)?;
        let (variant_idx, tag_value) = self.resolve_match_variant(&adt_info, variant_name, span)?;
        let tag_expr = self.build_match_tag_expr(target_expr, &Some(adt_info.clone()), span);
        let tag_cond = MastExpr::new(
            TypeId::BOOL,
            MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(tag_expr.clone()),
                rhs: Box::new(MastExpr::new(
                    tag_expr.ty,
                    MastExprKind::Integer(tag_value),
                    span,
                )),
            },
            span,
        );

        let payload_info = match adt_info {
            MatchAdtInfo::Named {
                mono_id,
                gen_args,
                def,
                is_pure,
                ..
            } => {
                if is_pure {
                    None
                } else {
                    def.variants[variant_idx]
                        .payload_type
                        .as_ref()
                        .map(|payload_ast| {
                            let mut payload_ty = self
                                .ctx
                                .node_types
                                .get(&payload_ast.id)
                                .copied()
                                .unwrap_or(TypeId::ERROR);
                            if !def.generics.is_empty() && !gen_args.is_empty() {
                                let mut map = HashMap::new();
                                for (i, param) in def.generics.iter().enumerate() {
                                    map.insert(param.name, gen_args[i]);
                                }
                                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                                payload_ty = subst.substitute(payload_ty);
                            }
                            (variant_idx, payload_ty, mono_id)
                        })
                }
            }
            MatchAdtInfo::Anonymous {
                mono_id,
                def,
                is_pure,
                ..
            } => {
                if is_pure {
                    None
                } else {
                    def.variants[variant_idx]
                        .payload_ty
                        .map(|payload_ty| (variant_idx, payload_ty, mono_id))
                }
            }
        };

        Some((tag_cond, payload_info))
    }

    fn collect_pattern_plan(
        &mut self,
        span: Span,
        pattern: &ast::Pattern,
        target_expr: &MastExpr,
        target_ty: TypeId,
        bindings: &mut Vec<PatternBindingPlan>,
    ) -> MastExpr {
        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                if !self.is_ignored_binding(binding.name) {
                    bindings.push(PatternBindingPlan {
                        name: binding.name,
                        ty: target_ty,
                        is_mut: binding.is_mut,
                        init: target_expr.clone(),
                    });
                }
                self.bool_expr(span, true)
            }
            ast::PatternKind::Ignore => self.bool_expr(span, true),
            ast::PatternKind::Variant(variant) => self
                .build_enum_variant_condition(span, target_expr, target_ty, variant.variant_name)
                .map(|(cond, _)| cond)
                .unwrap_or_else(|| self.bool_expr(span, false)),
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.ctx.type_registry.normalize(target_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    let Some(field) = destructure.fields.first() else {
                        return self.bool_expr(span, true);
                    };
                    let Some((tag_cond, payload_info)) =
                        self.build_enum_variant_condition(span, target_expr, target_ty, field.name)
                    else {
                        return self.bool_expr(span, false);
                    };
                    let Some((variant_idx, payload_ty, mono_id)) = payload_info else {
                        return tag_cond;
                    };
                    let Some(payload_expr) = self.build_payload_extract_expr(
                        span,
                        target_expr,
                        mono_id,
                        variant_idx,
                        payload_ty,
                    ) else {
                        return self.bool_expr(span, false);
                    };
                    let inner = self.collect_pattern_plan(
                        span,
                        &field.pattern,
                        &payload_expr,
                        payload_ty,
                        bindings,
                    );
                    self.and_expr(span, tag_cond, inner)
                } else {
                    let mut cond = self.bool_expr(span, true);
                    for field in &destructure.fields {
                        let Some((field_ty, struct_id, field_idx)) =
                            self.resolve_struct_pattern_field(target_ty, field.name, field.span)
                        else {
                            return self.bool_expr(span, false);
                        };
                        let field_expr = MastExpr::new(
                            field_ty,
                            MastExprKind::FieldAccess {
                                lhs: Box::new(target_expr.clone()),
                                struct_id,
                                field_idx,
                            },
                            field.span,
                        );
                        let inner = self.collect_pattern_plan(
                            field.span,
                            &field.pattern,
                            &field_expr,
                            field_ty,
                            bindings,
                        );
                        cond = self.and_expr(field.span, cond, inner);
                    }
                    cond
                }
            }
        }
    }

    fn lower_match_pattern_body(
        &mut self,
        arm_body: &Expr,
        bindings: Vec<PatternBindingPlan>,
        subst_map: &HashMap<SymbolId, TypeId>,
        exp_ty: TypeId,
    ) -> MastBlock {
        self.local_types.push(HashMap::new());
        self.local_forwardings.push(HashMap::new());
        self.local_value_forwardings.push(HashMap::new());
        let mut prefix = Vec::new();
        for binding in bindings {
            self.bind_local_type(
                arm_body.span,
                binding.name,
                binding.ty,
                binding.is_mut,
                "match pattern binding",
            );
            prefix.push(MastStmt::Let {
                name: binding.name,
                ty: binding.ty,
                is_mut: binding.is_mut,
                init: binding.init,
            });
        }

        let mut block = self.lower_block_as_body(arm_body, subst_map, exp_ty);
        prefix.append(&mut block.stmts);
        block.stmts = prefix;
        self.local_types.pop();
        self.local_forwardings.pop();
        self.local_value_forwardings.pop();
        block
    }

    pub(crate) fn lower_let_stmts(
        &mut self,
        expr: &Expr,
        pattern: &ast::LetPattern,
        init: &Expr,
        else_branch: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> Vec<MastStmt> {
        if else_branch.is_none() {
            match &pattern.pattern.kind {
                ast::PatternKind::Binding(binding) => {
                    if self.is_ignored_binding(binding.name) {
                        if self.is_pure_dead_initializer(expr.id) {
                            return Vec::new();
                        }

                        return self
                            .lower_optional_stmt_expr(init, subst_map)
                            .into_iter()
                            .collect();
                    }

                    let target_ty = {
                        let raw_ty = self.resolve_expr_type(init);
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
                        subst.substitute(raw_ty)
                    };

                    if !binding.is_mut && self.is_elidable_binding(expr.id) {
                        return Vec::new();
                    }

                    self.bind_local_type(
                        expr.span,
                        binding.name,
                        target_ty,
                        binding.is_mut,
                        "let pattern binding",
                    );

                    if !binding.is_mut && self.is_forwardable_value_binding(expr.id) {
                        let init = self.lower_expr(init, subst_map, Some(target_ty));
                        self.record_local_value_forwarding(
                            expr.span,
                            binding.name,
                            init,
                            "recording forwardable pure value binding",
                        );
                        return Vec::new();
                    }

                    if !binding.is_mut
                        && let Some(source_name) = self.forwardable_binding_source(expr.id)
                    {
                        self.record_local_forwarding(
                            expr.span,
                            binding.name,
                            source_name,
                            "recording forwardable immutable alias binding",
                        );
                        return Vec::new();
                    }

                    let init = if self.is_pure_dead_initializer(expr.id) {
                        MastExpr::new(target_ty, MastExprKind::Undef, expr.span)
                    } else {
                        self.lower_expr(init, subst_map, Some(target_ty))
                    };

                    return vec![MastStmt::Let {
                        name: binding.name,
                        ty: target_ty,
                        is_mut: binding.is_mut,
                        init,
                    }];
                }
                ast::PatternKind::Ignore => {
                    if self.is_pure_dead_initializer(expr.id) {
                        return Vec::new();
                    }

                    return self
                        .lower_optional_stmt_expr(init, subst_map)
                        .into_iter()
                        .collect();
                }
                _ => {}
            }
        }

        let lowered_init = self.lower_expr(init, subst_map, None);
        let target_ty = lowered_init.ty;
        let (target_let, target_var_expr) =
            self.build_match_target_binding(target_ty, lowered_init, init.span);

        let mut bindings = Vec::new();
        let condition = self.collect_pattern_plan(
            expr.span,
            &pattern.pattern,
            &target_var_expr,
            target_ty,
            &mut bindings,
        );

        if let Some(else_expr) = else_branch {
            let mut outer_stmts = Vec::new();
            let mut success_stmts = Vec::new();

            for binding in bindings {
                self.bind_local_type(
                    expr.span,
                    binding.name,
                    binding.ty,
                    binding.is_mut,
                    "let pattern binding",
                );
                outer_stmts.push(MastStmt::Let {
                    name: binding.name,
                    ty: binding.ty,
                    is_mut: binding.is_mut,
                    init: MastExpr::new(binding.ty, MastExprKind::Undef, expr.span),
                });
                success_stmts.push(MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Assign {
                        op: ast::AssignmentOperator::Assign,
                        lhs: Box::new(MastExpr::new(
                            binding.ty,
                            MastExprKind::Var(binding.name),
                            expr.span,
                        )),
                        rhs: Box::new(binding.init),
                    },
                    expr.span,
                )));
            }

            let if_expr = MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(condition),
                    then_branch: MastBlock {
                        stmts: success_stmts,
                        result: None,
                        defers: vec![],
                    },
                    else_branch: Some(self.lower_block_as_body(else_expr, subst_map, TypeId::VOID)),
                },
                expr.span,
            );

            outer_stmts.push(MastStmt::Expr(MastExpr::new(
                TypeId::VOID,
                MastExprKind::Block(MastBlock {
                    stmts: vec![target_let],
                    result: Some(Box::new(if_expr)),
                    defers: vec![],
                }),
                expr.span,
            )));

            outer_stmts
        } else {
            let mut stmts = vec![target_let];
            for binding in bindings {
                self.bind_local_type(
                    expr.span,
                    binding.name,
                    binding.ty,
                    binding.is_mut,
                    "let pattern binding",
                );
                stmts.push(MastStmt::Let {
                    name: binding.name,
                    ty: binding.ty,
                    is_mut: binding.is_mut,
                    init: binding.init,
                });
            }
            stmts
        }
    }

    pub(crate) fn lower_block_as_body(
        &mut self,
        block_expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        expected_ty: TypeId,
    ) -> MastBlock {
        self.defer_stack.push(Vec::new());
        self.local_types.push(HashMap::new());
        self.local_forwardings.push(HashMap::new());
        self.local_value_forwardings.push(HashMap::new());
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
                        } else if let ExprKind::Let {
                            pattern,
                            init,
                            else_branch,
                        } = &e.kind
                        {
                            stmts.extend(self.lower_let_stmts(
                                e,
                                pattern,
                                init,
                                else_branch.as_deref(),
                                subst_map,
                            ));
                        } else {
                            if let Some(stmt) = self.lower_optional_stmt_expr(e, subst_map) {
                                stmts.push(stmt);
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
            defers.push(d); // Preserve LIFO order in a dedicated array.
        }

        self.local_types.pop();
        self.local_forwardings.pop();
        self.local_value_forwardings.pop();
        self.local_statics.pop();
        MastBlock {
            stmts,
            result,
            defers,
        } // Pass defers to the backend separately.
    }

    pub(super) fn lower_closure_expr(&mut self, spec: ClosureLowerSpec<'_>) -> MastExprKind {
        let struct_id = self.new_mono_id();
        let func_id = self.new_mono_id();
        self.closure_fn_map.insert(spec.node_id, func_id);

        // 0. ================= Detect decay context =================
        let is_decay = spec.captures.is_empty()
            && matches!(
                self.ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(spec.exp_ty))
                    .clone(),
                TypeKind::Function { .. } | TypeKind::FnDef(..)
            );

        // 1. ================= Build the capture-state struct =================
        let (env_struct_fields, cap_exprs) =
            self.lower_closure_captures(spec.captures, spec.subst_map);
        self.register_closure_state_struct(struct_id, env_struct_fields.clone());

        // 2. ================= Build the closure entry function =================
        let env_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: spec.concrete_ty,
        });

        let mut mast_params = Vec::new();
        let env_sym = self.ctx.intern("__env");

        // Decayed closures become plain C ABI functions with no hidden context pointer.
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
        let saved_local_forwardings = std::mem::take(&mut self.local_forwardings);
        let saved_local_value_forwardings = std::mem::take(&mut self.local_value_forwardings);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        self.local_types.push(HashMap::new());
        self.local_forwardings.push(HashMap::new());
        self.local_value_forwardings.push(HashMap::new());
        for p in &mast_params {
            self.bind_local_type(spec.body.span, p.name, p.ty, p.is_mut, "closure parameter");
        }

        // Restore captured values when the closure was not decayed away.
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
        self.local_forwardings.pop();
        self.local_value_forwardings.pop();

        self.local_types = saved_local_types;
        self.local_forwardings = saved_local_forwardings;
        self.local_value_forwardings = saved_local_value_forwardings;
        self.defer_stack = saved_defer_stack;
        self.loop_frames = saved_loop_frames;
        self.local_statics = saved_local_statics;

        self.module.functions.push(MastFunction {
            id: func_id,
            name: format!("__closure_fn_{}", func_id.0),
            linkage: MastLinkage::Internal,
            params: mast_params,
            ret_ty,
            body: Some(body_block),
            is_extern: false,
            is_variadic: false,
            attributes: vec![],
        });

        // 3. ================= Assemble the expression at the current site =================
        let struct_init = MastExpr::new(
            spec.concrete_ty,
            MastExprKind::StructInit {
                struct_id,
                fields: cap_exprs,
            },
            Span::default(),
        );

        // 4. ================= Apply BNC and decay rules =================
        if is_decay {
            // Return the generated static C function pointer directly.
            return MastExprKind::FuncRef(func_id);
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
        let has_init_scope = init.is_some();
        if has_init_scope {
            self.local_types.push(HashMap::new());
            self.local_forwardings.push(HashMap::new());
            self.local_value_forwardings.push(HashMap::new());
        }

        let mut outer_stmts = Vec::new();
        if let Some(i) = init {
            match &i.kind {
                ExprKind::Let {
                    pattern,
                    init,
                    else_branch,
                } => outer_stmts.extend(self.lower_let_stmts(
                    i,
                    pattern,
                    init,
                    else_branch.as_deref(),
                    subst_map,
                )),
                _ => {
                    if let Some(stmt) = self.lower_optional_stmt_expr(i, subst_map) {
                        outer_stmts.push(stmt);
                    }
                }
            }
        }

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

        // Record the defer-stack height before entering the loop body.
        self.loop_frames.push(self.defer_stack.len());
        // Lower the loop body without the post expression.
        loop_stmts.push(MastStmt::Expr(self.lower_expr(body, subst_map, None)));

        let body_block = MastBlock {
            stmts: loop_stmts,
            result: None,
            defers: vec![],
        };

        // Lower the post statement separately as the latch block.
        let latch_block = post.map(|p| MastBlock {
            stmts: self
                .lower_optional_stmt_expr(p, subst_map)
                .into_iter()
                .collect(),
            result: None,
            defers: vec![],
        });

        // Leave the loop body and pop its control-flow boundary.
        self.loop_frames.pop();

        let loop_expr = MastExpr::new(
            TypeId::VOID,
            // Handle the newer AST representation.
            MastExprKind::Loop {
                body: body_block,
                latch: latch_block,
            },
            span,
        );

        if has_init_scope {
            outer_stmts.push(MastStmt::Expr(loop_expr));
            let block = MastExprKind::Block(MastBlock {
                stmts: outer_stmts,
                result: None,
                defers: vec![],
            });
            self.local_types.pop();
            self.local_forwardings.pop();
            self.local_value_forwardings.pop();
            block
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
        let (let_stmt, target_var_expr) =
            self.build_match_target_binding(target_ty, lowered_target, target.span);
        let match_context = MatchLowerContext {
            arms,
            target_var_expr: &target_var_expr,
            target_ty,
            subst_map,
            exp_ty,
        };
        let match_expr = self.lower_match_arm_chain(&match_context, 0);

        MastExprKind::Block(MastBlock {
            stmts: vec![let_stmt],
            result: Some(Box::new(match_expr)),
            defers: vec![],
        })
    }

    fn lower_match_arm_chain(
        &mut self,
        match_context: &MatchLowerContext<'_>,
        arm_index: usize,
    ) -> MastExpr {
        if arm_index >= match_context.arms.len() {
            return MastExpr::new(
                match_context.exp_ty,
                MastExprKind::Trap,
                match_context.target_var_expr.span,
            );
        }

        let arm = &match_context.arms[arm_index];
        self.lower_match_pattern_chain(match_context, &arm.patterns, 0, arm, arm_index)
    }

    fn lower_match_pattern_chain(
        &mut self,
        match_context: &MatchLowerContext<'_>,
        patterns: &[ast::MatchPattern],
        pattern_index: usize,
        arm: &ast::MatchArm,
        arm_index: usize,
    ) -> MastExpr {
        if pattern_index >= patterns.len() {
            return self.lower_match_arm_chain(match_context, arm_index + 1);
        }

        let pattern = &patterns[pattern_index];
        let (cond, bindings) = match &pattern.kind {
            ast::MatchPatternKind::Value(value) => {
                let cond = if let ExprKind::EnumLiteral { variant, .. } = value.kind {
                    self.build_enum_variant_condition(
                        pattern.span,
                        match_context.target_var_expr,
                        match_context.target_ty,
                        variant,
                    )
                    .map(|(cond, _)| cond)
                    .unwrap_or_else(|| self.bool_expr(pattern.span, false))
                } else {
                    let value_expr = self.lower_expr(
                        value,
                        match_context.subst_map,
                        Some(match_context.target_ty),
                    );
                    MastExpr::new(
                        TypeId::BOOL,
                        MastExprKind::Binary {
                            op: ast::BinaryOperator::Equal,
                            lhs: Box::new(match_context.target_var_expr.clone()),
                            rhs: Box::new(value_expr),
                        },
                        pattern.span,
                    )
                };
                (cond, Vec::new())
            }
            ast::MatchPatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_expr = self.lower_expr(
                    start,
                    match_context.subst_map,
                    Some(match_context.target_ty),
                );
                let end_expr =
                    self.lower_expr(end, match_context.subst_map, Some(match_context.target_ty));
                let lower = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: ast::BinaryOperator::LessOrEqual,
                        lhs: Box::new(start_expr),
                        rhs: Box::new(match_context.target_var_expr.clone()),
                    },
                    pattern.span,
                );
                let upper_op = if *inclusive {
                    ast::BinaryOperator::LessOrEqual
                } else {
                    ast::BinaryOperator::LessThan
                };
                let upper = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: upper_op,
                        lhs: Box::new(match_context.target_var_expr.clone()),
                        rhs: Box::new(end_expr),
                    },
                    pattern.span,
                );
                (self.and_expr(pattern.span, lower, upper), Vec::new())
            }
            ast::MatchPatternKind::Pattern(inner) => {
                let mut bindings = Vec::new();
                let cond = self.collect_pattern_plan(
                    pattern.span,
                    inner,
                    match_context.target_var_expr,
                    match_context.target_ty,
                    &mut bindings,
                );
                (cond, bindings)
            }
        };

        let then_branch = self.lower_match_pattern_body(
            &arm.body,
            bindings,
            match_context.subst_map,
            match_context.exp_ty,
        );
        let fallback = self.lower_match_pattern_chain(
            match_context,
            patterns,
            pattern_index + 1,
            arm,
            arm_index,
        );

        MastExpr::new(
            match_context.exp_ty,
            MastExprKind::If {
                cond: Box::new(cond),
                then_branch,
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(fallback)),
                    defers: vec![],
                }),
            },
            arm.span,
        )
    }

    /// Helper: synthesize a temporary `let` binding to isolate scope effects.
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

    /// Helper: recover ADT metadata for a target type.
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

    pub(crate) fn lower_return(
        &mut self,
        val: Option<&Expr>,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let v = val.map(|e| self.lower_expr(e, subst_map, None));
        let mut defer_stmts = Vec::new();

        // Expand all defers in the current scope stack in reverse order.
        for stack in self.defer_stack.iter().rev() {
            for d in stack.iter().rev() {
                defer_stmts.push(MastStmt::Expr(d.clone()));
            }
        }

        if defer_stmts.is_empty() {
            MastExprKind::Return(v.map(Box::new))
        } else {
            match v {
                Some(ret_expr) if ret_expr.ty != TypeId::VOID && ret_expr.ty != TypeId::ERROR => {
                    let temp_name = self.ctx.intern(&format!("__ret_tmp_{}", self.next_mono_id));
                    self.next_mono_id += 1;
                    let temp_ty = ret_expr.ty;

                    defer_stmts.insert(
                        0,
                        MastStmt::Let {
                            name: temp_name,
                            ty: temp_ty,
                            is_mut: false,
                            init: ret_expr,
                        },
                    );
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(Some(Box::new(MastExpr::new(
                            temp_ty,
                            MastExprKind::Var(temp_name),
                            span,
                        )))),
                        span,
                    )));
                }
                Some(ret_expr) => {
                    defer_stmts.insert(0, MastStmt::Expr(ret_expr));
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(None),
                        span,
                    )));
                }
                None => {
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(None),
                        span,
                    )));
                }
            }
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }

    /// Expand defers specifically for `break` and `continue`.
    pub(crate) fn lower_jump(&mut self, jump_kind: MastExprKind, span: Span) -> MastExprKind {
        let mut defer_stmts = Vec::new();

        // Find the defer-stack depth at the start of the current loop.
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

        // Walk backward through the defer stack until the loop boundary is reached.
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
            // Emit the real jump only after all cleanup work has run.
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
