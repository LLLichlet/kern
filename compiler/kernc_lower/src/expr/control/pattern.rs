//! Pattern lowering.
//!
//! Checked patterns lower to binding plans and runtime tests against MAST
//! expressions. This module handles destructuring, ignored bindings, field
//! projection ordering, and user-pattern lowering for match/let constructs.

use super::*;

struct UserPatternPlanInput<'a> {
    span: Span,
    value: &'a Expr,
    target_expr: &'a MastExpr,
    target_ty: TypeId,
    bind_ty: TypeId,
    subst_map: &'a HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    bindings: &'a mut Vec<PatternBindingPlan>,
}

pub(super) struct PatternPlan {
    prelude: Vec<MastStmt>,
    cond: MastExpr,
}

enum StructValuePatternField {
    Written(ast::StructFieldInit),
    Default {
        name: SymbolId,
        value: Expr,
        span: Span,
        owner_scope: Option<kernc_sema::scope::ScopeId>,
        subst_map: HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    },
}

impl StructValuePatternField {
    fn name(&self) -> SymbolId {
        match self {
            Self::Written(field) => field.name,
            Self::Default { name, .. } => *name,
        }
    }

    fn span(&self) -> Span {
        match self {
            Self::Written(field) => field.span,
            Self::Default { span, .. } => *span,
        }
    }

    fn value(&self) -> &Expr {
        match self {
            Self::Written(field) => &field.value,
            Self::Default { value, .. } => value,
        }
    }
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(super) fn is_ignored_binding(&self, name: SymbolId) -> bool {
        self.ctx.resolve(name) == "_"
    }

    pub(crate) fn resolve_struct_pattern_field(
        &mut self,
        target_ty: TypeId,
        field_name: SymbolId,
        span: Span,
    ) -> Option<(TypeId, MonoId, usize)> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Def(def_id, gen_args) => {
                let Def::Struct(def) = &self.ctx.defs[def_id.0 as usize] else {
                    self.ctx
                        .struct_error(span, "destructuring pattern expected a struct type")
                        .emit();
                    return None;
                };

                let ast_idx = def
                    .fields
                    .iter()
                    .position(|field| field.name == field_name)?;
                let mut field_ty = self
                    .ctx
                    .node_type(def.fields[ast_idx].type_node.id)
                    .unwrap_or(TypeId::ERROR);

                if !def.generics.is_empty() && !gen_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def.generics.iter().enumerate() {
                        map.insert(param.name, gen_args[i]);
                    }
                    field_ty = self.substitute_type_with_map(field_ty, &map);
                }

                let field_idx = self.get_physical_field_index(target_ty, field_name, span)?;
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

    pub(super) fn and_expr(&self, span: Span, lhs: MastExpr, rhs: MastExpr) -> MastExpr {
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

    fn collect_value_pattern_plan(
        &mut self,
        span: Span,
        value: &Expr,
        target_expr: &MastExpr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> Option<MastExpr> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        match &value.kind {
            ExprKind::Grouped { expr, .. } => {
                self.collect_value_pattern_plan(span, expr, target_expr, target_ty, subst_map)
            }
            ExprKind::Range {
                start: Some(start),
                end: Some(end),
                is_inclusive,
            } => {
                let start_expr = self.lower_expr(start, subst_map, Some(target_ty));
                let end_expr = self.lower_expr(end, subst_map, Some(target_ty));
                let lower = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: ast::BinaryOperator::LessOrEqual,
                        lhs: Box::new(start_expr),
                        rhs: Box::new(target_expr.clone()),
                    },
                    span,
                );
                let upper_op = if *is_inclusive {
                    ast::BinaryOperator::LessOrEqual
                } else {
                    ast::BinaryOperator::LessThan
                };
                let upper = MastExpr::new(
                    TypeId::BOOL,
                    MastExprKind::Binary {
                        op: upper_op,
                        lhs: Box::new(target_expr.clone()),
                        rhs: Box::new(end_expr),
                    },
                    span,
                );
                Some(self.and_expr(span, lower, upper))
            }
            ExprKind::Bool(expected) if norm_target == TypeId::BOOL => Some(MastExpr::new(
                TypeId::BOOL,
                MastExprKind::Binary {
                    op: ast::BinaryOperator::Equal,
                    lhs: Box::new(target_expr.clone()),
                    rhs: Box::new(self.bool_expr(span, *expected)),
                },
                span,
            )),
            ExprKind::Integer { .. }
            | ExprKind::Float { .. }
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Identifier(_)
            | ExprKind::Unary {
                op: ast::UnaryOperator::Negate,
                ..
            } => self.collect_scalar_const_value_pattern_plan(
                span,
                value,
                target_expr,
                target_ty,
                subst_map,
            ),
            ExprKind::EnumLiteral { variant, .. } => self
                .build_enum_variant_condition(span, target_expr, target_ty, *variant)
                .map(|(cond, _)| cond),
            ExprKind::FieldAccess { field, .. }
                if self.value_pattern_is_qualified_enum_variant(value, target_ty, subst_map) =>
            {
                self.build_enum_variant_condition(span, target_expr, target_ty, *field)
                    .map(|(cond, _)| cond)
            }
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Struct(fields),
                ..
            } => {
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    let [field] = fields.as_slice() else {
                        return None;
                    };
                    let (tag_cond, payload_info) = self.build_enum_variant_condition(
                        span,
                        target_expr,
                        target_ty,
                        field.name,
                    )?;
                    let Some((variant_idx, payload_ty, mono_id)) = payload_info else {
                        return Some(tag_cond);
                    };
                    let payload_expr = self.build_payload_extract_expr(
                        span,
                        target_expr,
                        mono_id,
                        variant_idx,
                        payload_ty,
                    )?;
                    let inner = self.collect_value_pattern_plan(
                        field.span,
                        &field.value,
                        &payload_expr,
                        payload_ty,
                        subst_map,
                    )?;
                    Some(self.and_expr(span, tag_cond, inner))
                } else {
                    let expanded_fields =
                        self.expand_struct_value_pattern_fields(span, target_ty, fields)?;
                    let mut cond = self.bool_expr(span, true);
                    for field in &expanded_fields {
                        let field_name = field.name();
                        let field_span = field.span();
                        let (field_ty, struct_id, field_idx) =
                            self.resolve_struct_pattern_field(target_ty, field_name, field_span)?;
                        let field_expr = MastExpr::new(
                            field_ty,
                            MastExprKind::FieldAccess {
                                lhs: Box::new(target_expr.clone()),
                                struct_id,
                                field_idx,
                            },
                            field_span,
                        );
                        let inner = match field {
                            StructValuePatternField::Written(_) => self
                                .collect_value_pattern_plan(
                                    field_span,
                                    field.value(),
                                    &field_expr,
                                    field_ty,
                                    subst_map,
                                )?,
                            StructValuePatternField::Default {
                                owner_scope,
                                subst_map,
                                ..
                            } => {
                                let prev_scope = self.ctx.scopes.current_scope_id();
                                if let Some(owner_scope) = *owner_scope {
                                    self.ctx.scopes.set_current_scope(owner_scope);
                                }
                                let inner = self.collect_value_pattern_plan(
                                    field_span,
                                    field.value(),
                                    &field_expr,
                                    field_ty,
                                    subst_map,
                                );
                                if let Some(prev_scope) = prev_scope {
                                    self.ctx.scopes.set_current_scope(prev_scope);
                                }
                                inner?
                            }
                        };
                        cond = self.and_expr(field_span, cond, inner);
                    }
                    Some(cond)
                }
            }
            _ => None,
        }
    }

    fn collect_scalar_const_value_pattern_plan(
        &mut self,
        span: Span,
        value: &Expr,
        target_expr: &MastExpr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> Option<MastExpr> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        let rhs = if matches!(value.kind, ExprKind::Identifier(_)) {
            let const_value = ConstEvaluator::new(self.ctx)
                .with_type_substs(subst_map)
                .eval_inner(value, 0)
                .ok()?;
            match const_value {
                ConstValue::Int(raw) if self.ctx.type_registry.is_integer(norm_target) => {
                    MastExpr::new(target_ty, MastExprKind::Integer(raw as u128), value.span)
                }
                ConstValue::Float(raw) if self.ctx.type_registry.is_float(norm_target) => {
                    MastExpr::new(target_ty, MastExprKind::Float(raw), value.span)
                }
                ConstValue::Bool(raw) if norm_target == TypeId::BOOL => {
                    MastExpr::new(TypeId::BOOL, MastExprKind::Bool(raw), value.span)
                }
                ConstValue::Enum { tag, .. } => {
                    MastExpr::new(target_ty, MastExprKind::Integer(tag as u128), value.span)
                }
                _ => return None,
            }
        } else {
            self.lower_expr(value, subst_map, Some(target_ty))
        };

        Some(MastExpr::new(
            TypeId::BOOL,
            MastExprKind::Binary {
                op: ast::BinaryOperator::Equal,
                lhs: Box::new(target_expr.clone()),
                rhs: Box::new(rhs),
            },
            span,
        ))
    }

    fn collect_nested_value_pattern_plan(
        &mut self,
        span: Span,
        value: &Expr,
        target_expr: &MastExpr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        bindings: &mut Vec<PatternBindingPlan>,
    ) -> PatternPlan {
        if let Some(bind_ty) = self.ctx.match_value_pattern_bind_ty(value.id) {
            let (prelude, cond) = self.collect_user_pattern_plan(UserPatternPlanInput {
                span,
                value,
                target_expr,
                target_ty,
                bind_ty,
                subst_map,
                bindings,
            });
            return PatternPlan { prelude, cond };
        }

        let cond = self
            .collect_value_pattern_plan(span, value, target_expr, target_ty, subst_map)
            .unwrap_or_else(|| {
                self.lower_error_expr(
                    TypeId::BOOL,
                    span,
                    "cannot lower invalid match value pattern",
                )
            });
        PatternPlan {
            prelude: Vec::new(),
            cond,
        }
    }

    fn optional_bind_ty(&mut self, bind_ty: TypeId) -> TypeId {
        let some = self.ctx.intern("Some");
        let none = self.ctx.intern("None");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(kernc_sema::ty::AnonymousEnum {
                backing_ty: None,
                builtin: Some(kernc_sema::ty::BuiltinAnonymousEnumKind::Optional),
                variants: vec![
                    kernc_sema::ty::AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: Some(bind_ty),
                        explicit_value: None,
                    },
                    kernc_sema::ty::AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn lower_user_pattern_apply(
        &mut self,
        span: Span,
        value: &Expr,
        target_expr: &MastExpr,
        target_ty: TypeId,
        bind_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> MastExpr {
        let pattern_expr = self.lower_expr(value, subst_map, None);
        let target_ty = self.substitute_type_with_map(target_ty, subst_map);
        let bind_ty = self.substitute_type_with_map(bind_ty, subst_map);
        let Some(owner_trait_ty) = self.ctx.builtin_trait_ty_with_assoc(
            "Pattern",
            vec![target_ty],
            vec![("Bind", bind_ty)],
        ) else {
            return self.lower_error_expr(bind_ty, span, "missing builtin trait `Pattern`");
        };
        let ret_ty = self.optional_bind_ty(bind_ty);
        let callee_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: vec![pattern_expr.ty, target_ty],
            ret: ret_ty,
            is_variadic: false,
        });
        let apply = self.ctx.intern("apply");
        self.lower_resolved_trait_method_call(
            pattern_expr,
            vec![target_expr.clone()],
            owner_trait_ty,
            super::super::call::MethodCallSite {
                field: apply,
                norm_callee: callee_ty,
                expected_self_ty: None,
                default_ret_ty: ret_ty,
                span,
            },
        )
    }

    fn bind_struct_fields_from_payload(
        &mut self,
        span: Span,
        payload_expr: &MastExpr,
        bind_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        bindings: &mut Vec<PatternBindingPlan>,
    ) {
        let bind_ty = self.substitute_type_with_map(bind_ty, subst_map);
        let norm_bind_ty = self.ctx.type_registry.normalize(bind_ty);
        let TypeKind::AnonymousStruct(is_extern, fields) =
            self.ctx.type_registry.get(norm_bind_ty).clone()
        else {
            return;
        };
        let struct_id = self.instantiate_anon_struct(norm_bind_ty);
        let (ast_to_physical, _) =
            self.cached_anon_struct_mapping(norm_bind_ty, is_extern, &fields);
        for (ast_idx, field) in fields.iter().enumerate() {
            let Some(&field_idx) = ast_to_physical.get(ast_idx) else {
                continue;
            };
            bindings.push(PatternBindingPlan {
                name: field.name,
                ty: field.ty,
                is_mut: false,
                init: MastExpr::new(
                    field.ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(payload_expr.clone()),
                        struct_id,
                        field_idx,
                    },
                    span,
                ),
            });
        }
    }

    fn collect_user_pattern_plan(
        &mut self,
        input: UserPatternPlanInput<'_>,
    ) -> (Vec<MastStmt>, MastExpr) {
        let UserPatternPlanInput {
            span,
            value,
            target_expr,
            target_ty,
            bind_ty,
            subst_map,
            bindings,
        } = input;
        let apply_result =
            self.lower_user_pattern_apply(span, value, target_expr, target_ty, bind_ty, subst_map);
        let (matched_let, matched_expr) =
            self.build_match_target_binding(apply_result.ty, apply_result, span);

        let some = self.ctx.intern("Some");
        let (cond, payload_info) =
            match self.build_enum_variant_condition(span, &matched_expr, matched_expr.ty, some) {
                Some(value) => value,
                None => {
                    return (
                        Vec::new(),
                        self.lower_error_expr(
                            TypeId::BOOL,
                            span,
                            "cannot lower `Pattern.apply` result as a builtin optional",
                        ),
                    );
                }
            };

        if let Some((variant_idx, payload_ty, mono_id)) = payload_info
            && bind_ty != TypeId::VOID
            && let Some(payload_expr) = self.build_payload_extract_expr(
                span,
                &matched_expr,
                mono_id,
                variant_idx,
                payload_ty,
            )
        {
            self.bind_struct_fields_from_payload(span, &payload_expr, bind_ty, subst_map, bindings);
        }

        (vec![matched_let], cond)
    }

    fn expand_struct_value_pattern_fields(
        &mut self,
        span: Span,
        target_ty: TypeId,
        fields: &[ast::StructFieldInit],
    ) -> Option<Vec<StructValuePatternField>> {
        let norm_target = self.ctx.type_registry.normalize(target_ty);
        let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm_target).clone()
        else {
            return Some(
                fields
                    .iter()
                    .cloned()
                    .map(StructValuePatternField::Written)
                    .collect(),
            );
        };
        let Def::Struct(def) = self.ctx.defs.get(def_id.0 as usize)? else {
            return Some(
                fields
                    .iter()
                    .cloned()
                    .map(StructValuePatternField::Written)
                    .collect(),
            );
        };
        if fields.len() >= def.fields.len() {
            return Some(
                fields
                    .iter()
                    .cloned()
                    .map(StructValuePatternField::Written)
                    .collect(),
            );
        }

        let mut expanded = Vec::with_capacity(def.fields.len());
        let mut struct_subst_map = HashMap::new();
        for (index, param) in def.generics.iter().enumerate() {
            if let Some(arg) = gen_args.get(index).copied() {
                struct_subst_map.insert(param.name, arg);
            }
        }

        for field_def in &def.fields {
            if let Some(field) = fields.iter().find(|field| field.name == field_def.name) {
                expanded.push(StructValuePatternField::Written(field.clone()));
                continue;
            }

            let default_value = field_def.default_value.as_deref()?;
            let owner_scope = self.ctx.def_owner_scope(def_id);
            expanded.push(StructValuePatternField::Default {
                name: field_def.name,
                value: default_value.clone(),
                span,
                owner_scope,
                subst_map: struct_subst_map.clone(),
            });
        }

        Some(expanded)
    }

    fn value_pattern_is_qualified_enum_variant(
        &mut self,
        value: &Expr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> bool {
        let ExprKind::FieldAccess { lhs, .. } = &value.kind else {
            return false;
        };
        let Some(lhs_ty) = self.ctx.node_type(lhs.id) else {
            return false;
        };
        let lhs_ty = self.substitute_type_with_map(lhs_ty, subst_map);
        let lhs_norm = self.normalize_concrete_type(lhs_ty);
        let lhs_norm = self.ctx.type_registry.normalize(lhs_norm);
        let target_norm = self.normalize_concrete_type(target_ty);
        let target_norm = self.ctx.type_registry.normalize(target_norm);

        if lhs_norm != target_norm {
            return false;
        }
        matches!(
            self.ctx.type_registry.get(target_norm),
            TypeKind::Enum(..) | TypeKind::AnonymousEnum(_)
        )
    }

    pub(super) fn collect_pattern_plan(
        &mut self,
        span: Span,
        pattern: &ast::Pattern,
        target_expr: &MastExpr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        bindings: &mut Vec<PatternBindingPlan>,
    ) -> PatternPlan {
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
                PatternPlan {
                    prelude: Vec::new(),
                    cond: self.bool_expr(span, true),
                }
            }
            ast::PatternKind::Ignore => PatternPlan {
                prelude: Vec::new(),
                cond: self.bool_expr(span, true),
            },
            ast::PatternKind::Value(value) => self.collect_nested_value_pattern_plan(
                span,
                value,
                target_expr,
                target_ty,
                subst_map,
                bindings,
            ),
            ast::PatternKind::Variant(variant) => PatternPlan {
                prelude: Vec::new(),
                cond: self
                    .build_enum_variant_condition(
                        span,
                        target_expr,
                        target_ty,
                        variant.variant_name,
                    )
                    .map(|(cond, _)| cond)
                    .unwrap_or_else(|| self.bool_expr(span, false)),
            },
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.ctx.type_registry.normalize(target_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    let Some(field) = destructure.fields.first() else {
                        return PatternPlan {
                            prelude: Vec::new(),
                            cond: self.bool_expr(span, true),
                        };
                    };
                    let Some((tag_cond, payload_info)) =
                        self.build_enum_variant_condition(span, target_expr, target_ty, field.name)
                    else {
                        return PatternPlan {
                            prelude: Vec::new(),
                            cond: self.bool_expr(span, false),
                        };
                    };
                    let Some((variant_idx, payload_ty, mono_id)) = payload_info else {
                        return PatternPlan {
                            prelude: Vec::new(),
                            cond: tag_cond,
                        };
                    };
                    let Some(payload_expr) = self.build_payload_extract_expr(
                        span,
                        target_expr,
                        mono_id,
                        variant_idx,
                        payload_ty,
                    ) else {
                        return PatternPlan {
                            prelude: Vec::new(),
                            cond: self.bool_expr(span, false),
                        };
                    };
                    let inner = self.collect_pattern_plan(
                        span,
                        &field.pattern,
                        &payload_expr,
                        payload_ty,
                        subst_map,
                        bindings,
                    );
                    PatternPlan {
                        prelude: inner.prelude,
                        cond: self.and_expr(span, tag_cond, inner.cond),
                    }
                } else {
                    let mut prelude = Vec::new();
                    let mut cond = self.bool_expr(span, true);
                    for field in &destructure.fields {
                        let Some((field_ty, struct_id, field_idx)) =
                            self.resolve_struct_pattern_field(target_ty, field.name, field.span)
                        else {
                            return PatternPlan {
                                prelude,
                                cond: self.bool_expr(span, false),
                            };
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
                            subst_map,
                            bindings,
                        );
                        prelude.extend(inner.prelude);
                        cond = self.and_expr(field.span, cond, inner.cond);
                    }
                    PatternPlan { prelude, cond }
                }
            }
        }
    }

    pub(super) fn lower_match_pattern_body(
        &mut self,
        arm_body: &Expr,
        bindings: Vec<PatternBindingPlan>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
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
        else_clause: Option<&ast::LetElseClause>,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> Vec<MastStmt> {
        if else_clause.is_none() {
            match &pattern.pattern.kind {
                ast::PatternKind::Binding(binding) => {
                    if self.measure_phase("        lower_let_binding_ignored", |this| {
                        this.is_ignored_binding(binding.name)
                    }) {
                        if self.is_pure_dead_initializer(expr.id) {
                            return Vec::new();
                        }

                        return self
                            .lower_optional_stmt_expr(init, subst_map)
                            .into_iter()
                            .collect();
                    }

                    let target_ty = self.measure_phase("        lower_let_binding_type", |this| {
                        this.substitute_type_with_map(this.resolve_expr_type(init), subst_map)
                    });

                    if self.measure_phase("        lower_let_binding_elide", |this| {
                        !binding.is_mut && this.is_elidable_binding(expr.id)
                    }) {
                        return Vec::new();
                    }

                    self.measure_phase("        lower_let_binding_bind_local", |this| {
                        this.bind_local_type(
                            expr.span,
                            binding.name,
                            target_ty,
                            binding.is_mut,
                            "let pattern binding",
                        );
                    });

                    if self.measure_phase("        lower_let_binding_forward_value", |this| {
                        !binding.is_mut && this.is_forwardable_value_binding(expr.id)
                    }) {
                        let init = self.measure_phase(
                            "        lower_let_binding_forward_value_init",
                            |this| this.lower_expr(init, subst_map, Some(target_ty)),
                        );
                        if !self.types_match_for_forwarding(init.ty, target_ty) {
                            return self.measure_phase("        lower_let_binding_emit", |_| {
                                vec![MastStmt::Let {
                                    name: binding.name,
                                    ty: target_ty,
                                    is_mut: binding.is_mut,
                                    init,
                                }]
                            });
                        }
                        self.measure_phase(
                            "        lower_let_binding_forward_value_record",
                            |this| {
                                this.record_local_value_forwarding(
                                    expr.span,
                                    binding.name,
                                    init,
                                    "recording forwardable pure value binding",
                                );
                            },
                        );
                        return Vec::new();
                    }

                    if !binding.is_mut
                        && let Some(source_name) = self
                            .measure_phase("        lower_let_binding_forward_alias", |this| {
                                this.forwardable_binding_source(expr.id)
                            })
                    {
                        self.measure_phase(
                            "        lower_let_binding_forward_alias_record",
                            |this| {
                                this.record_local_forwarding(
                                    expr.span,
                                    binding.name,
                                    source_name,
                                    "recording forwardable immutable alias binding",
                                );
                            },
                        );
                        return Vec::new();
                    }

                    let init = if self
                        .measure_phase("        lower_let_binding_dead_init", |this| {
                            this.is_pure_dead_initializer(expr.id)
                        }) {
                        MastExpr::new(target_ty, MastExprKind::Undef, expr.span)
                    } else {
                        self.measure_phase("        lower_let_binding_init", |this| {
                            this.lower_expr(init, subst_map, Some(target_ty))
                        })
                    };

                    return self.measure_phase("        lower_let_binding_emit", |_| {
                        vec![MastStmt::Let {
                            name: binding.name,
                            ty: target_ty,
                            is_mut: binding.is_mut,
                            init,
                        }]
                    });
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

        let lowered_init = self.measure_phase("        lower_let_pattern_init", |this| {
            this.lower_expr(init, subst_map, None)
        });
        let target_ty = lowered_init.ty;
        let (target_let, target_var_expr) = self
            .measure_phase("        lower_let_pattern_target", |this| {
                this.build_match_target_binding(target_ty, lowered_init, init.span)
            });

        let mut bindings = Vec::new();
        let pattern_plan = self.measure_phase("        lower_let_pattern_plan", |this| {
            this.collect_pattern_plan(
                expr.span,
                &pattern.pattern,
                &target_var_expr,
                target_ty,
                subst_map,
                &mut bindings,
            )
        });
        let PatternPlan {
            prelude: condition_prelude,
            cond: condition,
        } = pattern_plan;

        if let Some(else_clause) = else_clause {
            let mut outer_stmts = Vec::new();
            let mut success_stmts = Vec::new();
            let mut finalized_bindings = Vec::new();

            self.measure_phase("        lower_let_else_bindings", |this| {
                for binding in bindings {
                    let temp_id = this.new_mono_id();
                    let temp_name = this
                        .ctx
                        .intern(&format!("__let_else_binding_{}", temp_id.0));

                    // Keep success values in hidden temps until the control-flow block
                    // completes so the initializer still resolves any shadowed outer name.
                    outer_stmts.push(MastStmt::Let {
                        name: temp_name,
                        ty: binding.ty,
                        is_mut: false,
                        init: MastExpr::new(binding.ty, MastExprKind::Undef, expr.span),
                    });
                    success_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::VOID,
                        MastExprKind::Assign {
                            op: ast::AssignmentOperator::Assign,
                            lhs: Box::new(MastExpr::new(
                                binding.ty,
                                MastExprKind::Var(temp_name),
                                expr.span,
                            )),
                            rhs: Box::new(binding.init),
                        },
                        expr.span,
                    )));

                    this.bind_local_type(
                        expr.span,
                        binding.name,
                        binding.ty,
                        binding.is_mut,
                        "let pattern binding",
                    );
                    finalized_bindings.push((binding.name, binding.ty, binding.is_mut, temp_name));
                }
            });

            let lowered_else = match else_clause {
                ast::LetElseClause::Expr(else_expr) => self
                    .measure_phase("        lower_let_else_block", |this| {
                        this.lower_block_as_body(else_expr, subst_map, TypeId::VOID)
                    }),
                ast::LetElseClause::Arms(arms) => MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(self.measure_phase(
                        "        lower_let_else_arm_chain",
                        |this| {
                            this.lower_let_else_arm_chain(
                                arms,
                                &target_var_expr,
                                target_ty,
                                subst_map,
                                0,
                            )
                        },
                    ))),
                    defers: vec![],
                },
            };

            let if_expr = MastExpr::new(
                TypeId::VOID,
                MastExprKind::If {
                    cond: Box::new(condition),
                    then_branch: MastBlock {
                        stmts: success_stmts,
                        result: None,
                        defers: vec![],
                    },
                    else_branch: Some(lowered_else),
                },
                expr.span,
            );

            self.measure_phase("        lower_let_else_emit", |_| {
                let mut block_stmts = vec![target_let];
                block_stmts.extend(condition_prelude);
                outer_stmts.push(MastStmt::Expr(MastExpr::new(
                    TypeId::VOID,
                    MastExprKind::Block(MastBlock {
                        stmts: block_stmts,
                        result: Some(Box::new(if_expr)),
                        defers: vec![],
                    }),
                    expr.span,
                )));

                for (name, ty, is_mut, temp_name) in finalized_bindings {
                    outer_stmts.push(MastStmt::Let {
                        name,
                        ty,
                        is_mut,
                        init: MastExpr::new(ty, MastExprKind::Var(temp_name), expr.span),
                    });
                }
            });

            outer_stmts
        } else {
            let mut stmts = vec![target_let];
            stmts.extend(condition_prelude);
            self.measure_phase("        lower_let_pattern_bindings", |this| {
                for binding in bindings {
                    this.bind_local_type(
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
            });
            stmts
        }
    }

    fn lower_let_else_arm_chain(
        &mut self,
        arms: &[ast::LetElseArm],
        target_var_expr: &MastExpr,
        target_ty: TypeId,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        arm_index: usize,
    ) -> MastExpr {
        if arm_index >= arms.len() {
            return MastExpr::new(TypeId::NEVER, MastExprKind::Trap, target_var_expr.span);
        }

        let arm = &arms[arm_index];
        let mut bindings = Vec::new();
        let pattern_plan = self.measure_phase("          lower_let_else_arm_plan", |this| {
            this.collect_pattern_plan(
                arm.span,
                &arm.pattern,
                target_var_expr,
                target_ty,
                subst_map,
                &mut bindings,
            )
        });
        let PatternPlan {
            prelude: cond_prelude,
            cond,
        } = pattern_plan;
        let then_branch = self.measure_phase("          lower_let_else_arm_body", |this| {
            this.lower_match_pattern_body(&arm.body, bindings, subst_map, TypeId::VOID)
        });
        let fallback = self.measure_phase("          lower_let_else_arm_fallback", |this| {
            this.lower_let_else_arm_chain(
                arms,
                target_var_expr,
                target_ty,
                subst_map,
                arm_index + 1,
            )
        });

        let if_expr = MastExpr::new(
            TypeId::VOID,
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
        );

        if cond_prelude.is_empty() {
            if_expr
        } else {
            MastExpr::new(
                TypeId::VOID,
                MastExprKind::Block(MastBlock {
                    stmts: cond_prelude,
                    result: Some(Box::new(if_expr)),
                    defers: vec![],
                }),
                arm.span,
            )
        }
    }

    pub(crate) fn lower_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        exp_ty: TypeId,
    ) -> MastExprKind {
        let lowered_target = self.measure_phase("            lower_match_target", |this| {
            this.lower_expr(target, subst_map, None)
        });
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
        let match_expr = self.measure_phase("            lower_match_arms", |this| {
            this.lower_match_arm_chain(&match_context, 0)
        });

        MastExprKind::Block(MastBlock {
            stmts: vec![let_stmt],
            result: Some(Box::new(match_expr)),
            defers: vec![],
        })
    }

    pub(super) fn lower_match_arm_chain(
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

    pub(super) fn lower_match_pattern_chain(
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
        let (prelude, cond, bindings) = match &pattern.kind {
            ast::MatchPatternKind::Value(value) => {
                self.measure_phase("              lower_match_pattern_value", |this| {
                    if let Some(bind_ty) = this.ctx.match_value_pattern_bind_ty(value.id) {
                        let mut bindings = Vec::new();
                        let (prelude, cond) =
                            this.collect_user_pattern_plan(UserPatternPlanInput {
                                span: pattern.span,
                                value,
                                target_expr: match_context.target_var_expr,
                                target_ty: match_context.target_ty,
                                bind_ty,
                                subst_map: match_context.subst_map,
                                bindings: &mut bindings,
                            });
                        return (prelude, cond, bindings);
                    }

                    let cond = if let Some(cond) = this.collect_value_pattern_plan(
                        pattern.span,
                        value,
                        match_context.target_var_expr,
                        match_context.target_ty,
                        match_context.subst_map,
                    ) {
                        cond
                    } else {
                        this.lower_error_expr(
                            TypeId::BOOL,
                            pattern.span,
                            "cannot lower invalid match value pattern",
                        )
                    };
                    (Vec::new(), cond, Vec::new())
                })
            }
            ast::MatchPatternKind::Pattern(inner) => {
                self.measure_phase("              lower_match_pattern_plan", |this| {
                    let mut bindings = Vec::new();
                    let cond = this.collect_pattern_plan(
                        pattern.span,
                        inner,
                        match_context.target_var_expr,
                        match_context.target_ty,
                        match_context.subst_map,
                        &mut bindings,
                    );
                    (cond.prelude, cond.cond, bindings)
                })
            }
        };

        let then_branch = self.measure_phase("              lower_match_pattern_body", |this| {
            this.lower_match_pattern_body(
                &arm.body,
                bindings,
                match_context.subst_map,
                match_context.exp_ty,
            )
        });
        let fallback = self.measure_phase("              lower_match_pattern_fallback", |this| {
            this.lower_match_pattern_chain(
                match_context,
                patterns,
                pattern_index + 1,
                arm,
                arm_index,
            )
        });

        let if_expr = MastExpr::new(
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
        );

        if prelude.is_empty() {
            if_expr
        } else {
            MastExpr::new(
                match_context.exp_ty,
                MastExprKind::Block(MastBlock {
                    stmts: prelude,
                    result: Some(Box::new(if_expr)),
                    defers: vec![],
                }),
                arm.span,
            )
        }
    }
}
