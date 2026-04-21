use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(super) fn maybe_lower_asm_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> Option<MastExprKind> {
        let ExprKind::Identifier(sym) = &callee.kind else {
            return None;
        };

        if self.ctx.resolve(*sym) == "@asm" {
            Some(self.lower_asm_call(args, subst_map, span))
        } else {
            None
        }
    }

    pub(super) fn detect_method_call(
        &mut self,
        callee: &Expr,
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
    ) -> Option<(NodeId, SymbolId, MastExpr)> {
        let ExprKind::FieldAccess { lhs, field, .. } = &callee.kind else {
            return None;
        };

        let lhs_ty = self
            .ctx
            .node_types
            .get(&lhs.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);
        if matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(_)) {
            return None;
        }

        let callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_callee = self.ctx.type_registry.normalize(callee_ty);
        if !matches!(
            self.ctx.type_registry.get(norm_callee),
            TypeKind::FnDef(..) | TypeKind::Function { .. }
        ) {
            return None;
        }

        Some((callee.id, *field, self.lower_expr(lhs, subst_map, None)))
    }

    pub(super) fn asm_config_fields<'b>(
        &mut self,
        args: &'b [Expr],
        span: Span,
    ) -> Option<&'b [ast::StructFieldInit]> {
        let Some(config_arg) = args.first() else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): `@asm` lowering expected one configuration argument.",
            );
            return None;
        };

        if let ExprKind::DataInit {
            literal: ast::DataLiteralKind::Struct(fields),
            ..
        } = &config_arg.kind
        {
            Some(fields)
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): `@asm` macro argument must be a structural data literal (e.g. `.{ ... }`). Sema failed to validate this.",
            );
            None
        }
    }

    pub(super) fn lower_asm_template(&mut self, value: &Expr) -> Option<String> {
        match &value.kind {
            ExprKind::String(s) => Some(s.clone()),
            _ => {
                self.ctx.emit_ice(
                    value.span,
                    "Kern ICE (Lowering): invalid format for `asm` field in `@asm` macro.",
                );
                None
            }
        }
    }

    pub(super) fn lower_asm_volatile_flag(&mut self, value: &Expr) -> Option<bool> {
        match &value.kind {
            ExprKind::Bool(flag) => Some(*flag),
            _ => {
                let mut evaluator = ConstEvaluator::new(self.ctx);
                match evaluator.eval_const_value(value) {
                    Ok(ConstValue::Bool(flag)) => Some(flag),
                    Ok(other) => {
                        self.ctx.emit_ice(
                            value.span,
                            format!(
                                "Kern ICE (Lowering): `@asm` `volatile` flag must reduce to a compile-time boolean, found `{:?}`.",
                                other
                            ),
                        );
                        None
                    }
                    Err(_) => {
                        self.ctx.emit_ice(
                            value.span,
                            "Kern ICE (Lowering): `@asm` `volatile` flag was not reduced to a compile-time boolean.",
                        );
                        None
                    }
                }
            }
        }
    }

    pub(super) fn asm_output_value_type(
        &mut self,
        ptr_expr: &MastExpr,
        span: Span,
    ) -> Option<TypeId> {
        match self.ctx.type_registry.get_elem_type(ptr_expr.ty) {
            Some(ty) => Some(ty),
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): `@asm` output operand must lower to a pointer value.",
                );
                None
            }
        }
    }

    pub(crate) fn lower_asm_call(
        &mut self,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, kernc_sema::ty::GenericArg>,
        span: Span,
    ) -> MastExprKind {
        let Some(fields) = self.asm_config_fields(args, span) else {
            return MastExprKind::Trap;
        };

        let mut asm_template = String::new();
        let mut is_volatile = false;

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        let mut clobbers = Vec::new();

        for field in fields {
            let field_name = self.ctx.resolve(field.name);
            match field_name {
                "asm" => {
                    let Some(template) = self.lower_asm_template(&field.value) else {
                        return MastExprKind::Trap;
                    };
                    asm_template = template;
                }
                "volatile" => {
                    let Some(flag) = self.lower_asm_volatile_flag(&field.value) else {
                        return MastExprKind::Trap;
                    };
                    is_volatile = flag;
                }
                "outputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM constraint mapping: `reg -> "=r"`, `freg -> "=f"`, `eax -> "={eax}"`.
                            let constraint = if reg_name == "reg" {
                                "=r".to_string()
                            } else if reg_name == "freg" {
                                "=f".to_string()
                            } else {
                                format!("={{{}}}", reg_name)
                            };

                            let ptr_expr = self.lower_expr(&reg.value, subst_map, None);
                            let Some(val_ty) =
                                self.asm_output_value_type(&ptr_expr, reg.value.span)
                            else {
                                return MastExprKind::Trap;
                            };
                            outputs.push((constraint, ptr_expr, val_ty));
                        }
                    }
                }
                "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM constraint mapping: `reg -> "r"`, `freg -> "f"`, `eax -> "{eax}"`.
                            let constraint = if reg_name == "reg" {
                                "r".to_string()
                            } else if reg_name == "freg" {
                                "f".to_string()
                            } else {
                                format!("{{{}}}", reg_name)
                            };

                            let val_expr = self.lower_expr(&reg.value, subst_map, None);
                            inputs.push((constraint, val_expr));
                        }
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(elems),
                        ..
                    } = &field.value.kind
                    {
                        for e in elems {
                            if let ExprKind::String(s) = &e.kind {
                                clobbers.push(format!("~{{{}}}", s));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Build the final LLVM constraint string in output/input/clobber order.
        let mut all_constraints = Vec::new();
        let mut output_ptrs = Vec::new();
        let mut output_tys = Vec::new();
        for (c, ptr, ty) in outputs {
            all_constraints.push(c);
            output_ptrs.push(ptr);
            output_tys.push(ty);
        }

        let mut input_args = Vec::new();
        for (c, expr) in inputs {
            all_constraints.push(c);
            input_args.push(expr);
        }

        for c in clobbers {
            all_constraints.push(c);
        }

        MastExprKind::Asm(MastAsmBlock {
            asm_template,
            constraints: all_constraints.join(","),
            input_args,
            output_ptrs,
            output_tys,
            is_volatile,
        })
    }
}
