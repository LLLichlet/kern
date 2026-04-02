use super::*;

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub(super) fn resolve_local_place(
        &mut self,
        name: SymbolId,
        span: Span,
    ) -> ConstEvalResult<ResolvedPlace> {
        if let Some(scope_idx) = self.lookup_local_slot(name) {
            Ok(ResolvedPlace {
                root_scope: scope_idx,
                root_name: name,
                path: Vec::new(),
                require_root_mutability: true,
            })
        } else {
            self.ctx
                .struct_error(
                    span,
                    "constant evaluation can only take references to local bindings in the current const context",
                )
                .emit();
            Err(ConstEvalError)
        }
    }

    pub(super) fn project_const_value(
        &mut self,
        value: &ConstValue,
        path: &[PlaceSegment],
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if path.is_empty() {
            return Ok(value.clone());
        }

        match path[0] {
            PlaceSegment::Field(field) => match value {
                ConstValue::Struct(map) => {
                    let Some(next) = map.get(&field) else {
                        let field_str = self.ctx.resolve(field);
                        self.ctx
                            .struct_error(
                                span,
                                format!("field `{}` not found in constant struct", field_str),
                            )
                            .emit();
                        return Err(ConstEvalError);
                    };
                    self.project_const_value(next, &path[1..], span)
                }
                _ => {
                    self.ctx
                        .struct_error(span, "attempted field access on a non-struct constant")
                        .emit();
                    Err(ConstEvalError)
                }
            },
            PlaceSegment::Index(index) => match value {
                ConstValue::Array(items) => {
                    let Some(next) = items.get(index) else {
                        self.ctx
                            .struct_error(span, "constant array index out of bounds")
                            .emit();
                        return Err(ConstEvalError);
                    };
                    self.project_const_value(next, &path[1..], span)
                }
                _ => {
                    self.ctx
                        .struct_error(span, "attempted indexing into a non-array constant")
                        .emit();
                    Err(ConstEvalError)
                }
            },
        }
    }

    pub(super) fn read_place_value(
        &mut self,
        root_scope: usize,
        root_name: SymbolId,
        path: &[PlaceSegment],
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(root_value) = self.lookup_local_at(root_scope, root_name) else {
            self.ctx
                .struct_error(
                    span,
                    "constant pointer target is no longer available in the current local scope",
                )
                .emit();
            return Err(ConstEvalError);
        };

        self.project_const_value(&root_value, path, span)
    }

    pub(super) fn resolve_pointer_target(
        &mut self,
        value: &ConstValue,
        require_mut: bool,
        span: Span,
    ) -> ConstEvalResult<ResolvedPlace> {
        match value {
            ConstValue::Pointer {
                root_scope,
                root_name,
                path,
                is_mut,
            } => {
                if require_mut && !*is_mut {
                    self.ctx
                        .struct_error(
                            span,
                            "constant evaluation cannot mutate through an immutable pointer",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                Ok(ResolvedPlace {
                    root_scope: *root_scope,
                    root_name: *root_name,
                    path: path.clone(),
                    require_root_mutability: false,
                })
            }
            _ => {
                self.ctx
                    .struct_error(span, "expected a local pointer in constant evaluation")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn resolve_reference_place(
        &mut self,
        expr: &Expr,
        depth: usize,
        require_mut: bool,
    ) -> ConstEvalResult<ResolvedPlace> {
        match &expr.kind {
            ExprKind::Identifier(name) => self.resolve_local_place(*name, expr.span),
            ExprKind::SelfValue => {
                let self_name = self.ctx.intern("self");
                self.resolve_local_place(self_name, expr.span)
            }
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                operand,
            } => {
                let pointer = self.eval_inner(operand, depth + 1)?;
                self.resolve_pointer_target(&pointer, require_mut, expr.span)
            }
            ExprKind::FieldAccess { lhs, field, .. } => {
                let lhs_norm = self.expr_type(lhs);

                let mut place = match self.ctx.type_registry.get(lhs_norm).clone() {
                    TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } => {
                        let pointer = self.eval_inner(lhs, depth + 1)?;
                        self.resolve_pointer_target(&pointer, require_mut, lhs.span)?
                    }
                    _ => self.resolve_reference_place(lhs, depth + 1, require_mut)?,
                };
                place.path.push(PlaceSegment::Field(*field));
                Ok(place)
            }
            ExprKind::IndexAccess { lhs, index, .. } => {
                let mut place = self.resolve_reference_place(lhs, depth + 1, require_mut)?;
                let idx = self.eval_usize(index)? as usize;
                place.path.push(PlaceSegment::Index(idx));
                Ok(place)
            }
            _ => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "constant evaluation currently supports references only to local bindings, explicit pointer dereferences, struct fields, or array elements",
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }
}
