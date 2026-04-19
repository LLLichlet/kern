use super::*;

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    fn integer_literal_magnitude(expr: &Expr) -> Option<(bool, u128)> {
        match &expr.kind {
            ExprKind::Integer(value) => Some((false, *value)),
            ExprKind::Unary {
                op: UnaryOperator::Negate,
                operand,
            } => match &operand.kind {
                ExprKind::Integer(value) => Some((true, *value)),
                _ => None,
            },
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Scalar(inner),
                ..
            } => Self::integer_literal_magnitude(inner),
            _ => None,
        }
    }

    fn range_error_for_integer_literal(
        &mut self,
        expr: &Expr,
        ty: TypeId,
        rendered_value: &str,
        min: &str,
        max: &str,
    ) -> ConstEvalResult<i128> {
        self.ctx
            .struct_error(
                expr.span,
                format!(
                    "integer literal {} is out of bounds for type `{}`",
                    rendered_value,
                    self.ctx.ty_to_string(ty)
                ),
            )
            .with_hint(format!("the valid range is {} to {}", min, max))
            .emit();
        Err(ConstEvalError)
    }

    fn bind_integer_literal_to_type(
        &mut self,
        expr: &Expr,
        ty: TypeId,
        norm: TypeId,
        primitive: PrimitiveType,
    ) -> ConstEvalResult<Option<i128>> {
        let Some((is_negative, magnitude)) = Self::integer_literal_magnitude(expr) else {
            return Ok(None);
        };

        let bit_width = crate::LayoutEngine::new(self.ctx).compute_type_size(norm) * 8;
        let rendered_value = if is_negative {
            format!("-{}", magnitude)
        } else {
            magnitude.to_string()
        };

        let is_signed = matches!(
            primitive,
            PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::I128
                | PrimitiveType::ISize
        );
        let is_unsigned = matches!(
            primitive,
            PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::U128
                | PrimitiveType::USize
        );

        if !is_signed && !is_unsigned {
            return Ok(None);
        }

        if is_unsigned {
            let max = if bit_width >= 128 {
                u128::MAX
            } else {
                (1u128 << bit_width) - 1
            };
            if is_negative {
                self.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", rendered_value, self.ctx.ty_to_string(ty)))
                    .with_hint("if you need a bit-pattern of all 1s, use explicit bitwise negation (e.g., `~0`) or `as` cast")
                    .emit();
                return Err(ConstEvalError);
            }
            if magnitude > max {
                return self
                    .range_error_for_integer_literal(
                        expr,
                        ty,
                        &rendered_value,
                        "0",
                        &max.to_string(),
                    )
                    .map(Some);
            }
            return Ok(Some(magnitude as i128));
        }

        let max = if bit_width >= 128 {
            i128::MAX as u128
        } else {
            (1u128 << (bit_width - 1)) - 1
        };
        let min_magnitude = 1u128 << (bit_width - 1);
        let min = if bit_width >= 128 {
            i128::MIN.to_string()
        } else {
            format!("-{}", min_magnitude)
        };
        let max_str = max.to_string();

        if is_negative {
            if magnitude > min_magnitude {
                return self
                    .range_error_for_integer_literal(expr, ty, &rendered_value, &min, &max_str)
                    .map(Some);
            }
            if bit_width >= 128 && magnitude == min_magnitude {
                return Ok(Some(i128::MIN));
            }
            return Ok(Some(-(magnitude as i128)));
        }

        if magnitude > max {
            return self
                .range_error_for_integer_literal(expr, ty, &rendered_value, &min, &max_str)
                .map(Some);
        }

        Ok(Some(magnitude as i128))
    }

    fn expr_uses_unsigned_integer_semantics(&mut self, expr: &Expr) -> bool {
        let ty = self.expr_type(expr);
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(
            self.ctx.type_registry.get(norm),
            TypeKind::Primitive(
                PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            )
        )
    }

    fn eval_const_uint_division(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if rhs == 0 {
            self.ctx
                .struct_error(span, "division by zero in constant expression")
                .emit();
            return Err(ConstEvalError);
        }

        Ok(ConstValue::Int(((lhs as u128) / (rhs as u128)) as i128))
    }

    fn eval_const_uint_modulo(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if rhs == 0 {
            self.ctx
                .struct_error(span, "modulo by zero in constant expression")
                .emit();
            return Err(ConstEvalError);
        }

        Ok(ConstValue::Int(((lhs as u128) % (rhs as u128)) as i128))
    }

    fn check_pattern_recursion_depth(
        &mut self,
        depth: usize,
        span: kernc_utils::Span,
    ) -> ConstEvalResult<()> {
        if depth > 100 {
            self.ctx
                .struct_error(
                    span,
                    "constant pattern evaluation exceeded maximum recursion depth",
                )
                .with_hint("check for excessively nested destructuring in constant evaluation")
                .emit();
            return Err(ConstEvalError);
        }

        Ok(())
    }

    fn pattern_is_irrefutable(
        &mut self,
        pattern: &ast::Pattern,
        target_ty: TypeId,
        depth: usize,
    ) -> ConstEvalResult<bool> {
        self.check_pattern_recursion_depth(depth, pattern.span)?;

        match &pattern.kind {
            ast::PatternKind::Binding(_) | ast::PatternKind::Ignore => Ok(true),
            ast::PatternKind::Variant(_) => Ok(false),
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.ctx.type_registry.normalize(target_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    return Ok(false);
                }

                for field in &destructure.fields {
                    let Some(field_ty) =
                        self.struct_pattern_field_ty(target_ty, field.name, field.span)?
                    else {
                        return Ok(false);
                    };

                    if !self.pattern_is_irrefutable(&field.pattern, field_ty, depth + 1)? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }
        }
    }

    fn install_pattern_binding_types(
        &mut self,
        pattern: &ast::Pattern,
        target_ty: TypeId,
        depth: usize,
    ) -> ConstEvalResult<()> {
        self.check_pattern_recursion_depth(depth, pattern.span)?;

        match &pattern.kind {
            ast::PatternKind::Binding(binding) => {
                if self.ctx.resolve(binding.name) != "_" {
                    self.define_local_type(binding.name, target_ty);
                    self.define_local_mutability(binding.name, binding.is_mut);
                }
            }
            ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.ctx.type_registry.normalize(target_ty);
                if matches!(
                    self.ctx.type_registry.get(norm_target),
                    TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                ) {
                    if let Some(field) = destructure.fields.first()
                        && let Some(payload_ty) =
                            self.variant_payload_ty(target_ty, field.name, depth, field.span)?
                    {
                        self.install_pattern_binding_types(&field.pattern, payload_ty, depth + 1)?;
                    }
                } else {
                    for field in &destructure.fields {
                        if let Some(field_ty) =
                            self.struct_pattern_field_ty(target_ty, field.name, field.span)?
                        {
                            self.install_pattern_binding_types(
                                &field.pattern,
                                field_ty,
                                depth + 1,
                            )?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn eval_inner(&mut self, expr: &Expr, depth: usize) -> ConstEvalResult<ConstValue> {
        if depth > 100 {
            self.ctx
                .struct_error(
                    expr.span,
                    "constant evaluation exceeded maximum recursion depth",
                )
                .with_hint("check for circular references in your `const` declarations")
                .emit();
            return Err(ConstEvalError);
        }

        let eval_result = match &expr.kind {
            // === 1. Basic literals ===
            ExprKind::Integer(val) => Ok(ConstValue::Int(*val as i128)),
            ExprKind::Float(val) => Ok(ConstValue::Float(*val)),
            ExprKind::Bool(b) => Ok(ConstValue::Bool(*b)),
            ExprKind::Char(c) => Ok(ConstValue::Int(*c as u32 as i128)),
            ExprKind::ByteChar(c) => Ok(ConstValue::Int(*c as i128)),
            ExprKind::String(s) => Ok(ConstValue::String(s.clone())),
            ExprKind::Undef => Ok(ConstValue::Undef),

            // === 2. Arithmetic and logical operators ===
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(lhs, *op, rhs, depth, expr.span),
            ExprKind::Unary { op, operand } => {
                // Fold `-123` directly so it is treated as a signed literal.
                if *op == UnaryOperator::Negate {
                    if let ExprKind::Integer(val) = &operand.kind {
                        Ok(ConstValue::Int((*val as i128).wrapping_neg()))
                    } else if let ExprKind::Float(val) = &operand.kind {
                        Ok(ConstValue::Float(-*val))
                    } else {
                        self.eval_unary(*op, operand, depth, expr.span)
                    }
                } else {
                    self.eval_unary(*op, operand, depth, expr.span)
                }
            }

            ExprKind::As { lhs, .. } => {
                let val = self.eval_inner(lhs, depth + 1)?;
                let target_ty = self.node_type(expr.id);

                if let ConstValue::Int(v) = val {
                    let mut layout = LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(target_ty) * 8;
                    let mask = if bit_width >= 128 {
                        u128::MAX
                    } else {
                        (1 << bit_width) - 1
                    };
                    let u_val = (v as u128) & mask;

                    Ok(ConstValue::Int(u_val as i128))
                } else {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "only integer casts are supported in const context currently",
                        )
                        .emit();
                    Err(ConstEvalError)
                }
            }

            // === 3. Resolve constant identifiers ===
            ExprKind::Identifier(name) => self.eval_identifier(*name, depth, expr.span),
            ExprKind::SelfValue => {
                let self_name = self.ctx.intern("self");
                self.eval_identifier(self_name, depth, expr.span)
            }

            // === 4. Constant function calls ===
            ExprKind::Call { callee, args } => self.eval_call(callee, args, depth, expr.span),

            // === 5. Enum literals ===
            ExprKind::EnumLiteral { variant, .. } => {
                self.eval_enum_literal(expr.id, *variant, depth, expr.span)
            }

            // === 6. Aggregate initialization ===
            ExprKind::DataInit { literal, .. } => self.eval_data_init(expr, literal, depth),

            // === 7. Local control flow ===
            ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                let value = self.eval_inner(init, depth + 1)?;
                let init_ty = self.expr_type(init);

                let is_irrefutable =
                    self.pattern_is_irrefutable(&pattern.pattern, init_ty, depth + 1)?;
                if is_irrefutable && else_branch.is_some() {
                    self.ctx
                        .struct_error(expr.span, "irrefutable `let` patterns cannot use `else`")
                        .with_code(kernc_utils::DiagnosticCode::IrrefutableLetElse)
                        .emit();
                    return Err(ConstEvalError);
                }
                if !is_irrefutable && else_branch.is_none() {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "refutable `let` patterns require an `else` branch",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                let Some(bindings) =
                    self.match_inner_pattern(&pattern.pattern, &value, init_ty, depth + 1)?
                else {
                    let Some(else_expr) = else_branch else {
                        self.ctx
                            .struct_error(
                                expr.span,
                                "refutable `let` patterns require an `else` branch",
                            )
                            .emit();
                        return Err(ConstEvalError);
                    };

                    if let Some(else_pattern) = else_pattern {
                        let Some(else_bindings) =
                            self.match_inner_pattern(else_pattern, &value, init_ty, depth + 1)?
                        else {
                            self.ctx
                                .struct_error(
                                    else_pattern.span,
                                    "explicit `else` pattern did not match the failing `let` value",
                                )
                                .emit();
                            return Err(ConstEvalError);
                        };

                        self.push_local_scope();
                        for (name, value) in else_bindings {
                            self.define_local(name, value);
                        }
                        self.install_pattern_binding_types(else_pattern, init_ty, depth + 1)?;
                        let _ = self.eval_inner(else_expr, depth + 1)?;
                        self.pop_local_scope();
                        return Ok(ConstValue::Void);
                    }

                    let _ = self.eval_inner(else_expr, depth + 1)?;
                    return Ok(ConstValue::Void);
                };

                for (name, value) in bindings {
                    self.define_local(name, value);
                }
                self.install_pattern_binding_types(&pattern.pattern, init_ty, depth + 1)?;

                Ok(ConstValue::Void)
            }
            ExprKind::Block { stmts, result } => self.eval_block(stmts, result.as_deref(), depth),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.eval_if(cond, then_branch, else_branch.as_deref(), depth, expr.span),
            ExprKind::Match { target, arms } => self.eval_match(target, arms, depth, expr.span),
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.eval_for(
                init.as_deref(),
                cond.as_deref(),
                post.as_deref(),
                body,
                depth,
                expr.span,
            ),
            ExprKind::Assign { lhs, op, rhs } => self.eval_assign(lhs, *op, rhs, depth, expr.span),
            ExprKind::Break => self.eval_break(expr.span),
            ExprKind::Continue => self.eval_continue(expr.span),
            ExprKind::Return(value) => self.eval_return(value.as_deref(), depth, expr.span),

            // === 8. Constant aggregate projection ===
            ExprKind::FieldAccess { lhs, field, .. } => {
                let norm_lhs = self.expr_type(lhs);

                if self.expr_is_type_namespace(lhs)
                    && matches!(
                        self.ctx.type_registry.get(norm_lhs),
                        TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                    )
                {
                    self.eval_enum_literal(expr.id, *field, depth, expr.span)
                } else if let TypeKind::Module(mod_def_id) =
                    self.ctx.type_registry.get(norm_lhs).clone()
                {
                    let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        m.scope_id
                    } else {
                        self.ctx.emit_ice(
                            expr.span,
                            format!(
                                "Kern ICE (ConstEval): Expected module definition for DefId {} during constant field access.",
                                mod_def_id.0
                            ),
                        );
                        return Err(ConstEvalError);
                    };
                    if let Some(info) = self.ctx.scopes.resolve_in(mod_scope, *field).cloned() {
                        if info.kind == SymbolKind::Const {
                            if let Some(def_id) = info.def_id {
                                self.eval_const_def(def_id, depth)
                            } else {
                                Err(ConstEvalError)
                            }
                        } else {
                            let field_str = self.ctx.resolve(*field);
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!(
                                        "`{}` is a {}, not a compile-time constant",
                                        field_str,
                                        self.kind_to_string(info.kind)
                                    ),
                                )
                                .emit();
                            Err(ConstEvalError)
                        }
                    } else {
                        let field_str = self.ctx.resolve(*field);
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!("constant `{}` not found in module", field_str),
                            )
                            .emit();
                        Err(ConstEvalError)
                    }
                } else {
                    let base = self.eval_inner(lhs, depth + 1)?;
                    match base {
                        ConstValue::Pointer {
                            root_scope,
                            root_name,
                            mut path,
                            ..
                        } => {
                            path.push(PlaceSegment::Field(*field));
                            self.read_place_value(root_scope, root_name, &path, expr.span)
                        }
                        other => self.project_const_value(
                            &other,
                            &[PlaceSegment::Field(*field)],
                            expr.span,
                        ),
                    }
                }
            }

            ExprKind::IndexAccess { lhs, index, .. } => {
                let base = self.eval_inner(lhs, depth + 1)?;
                let idx = self.eval_usize(index)?;
                match base {
                    ConstValue::Pointer {
                        root_scope,
                        root_name,
                        mut path,
                        ..
                    } => {
                        path.push(PlaceSegment::Index(idx as usize));
                        self.read_place_value(root_scope, root_name, &path, expr.span)
                    }
                    other => self.project_const_value(
                        &other,
                        &[PlaceSegment::Index(idx as usize)],
                        expr.span,
                    ),
                }
            }

            ExprKind::GenericInstantiation { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated directly as a value",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            ExprKind::Static { .. } | ExprKind::Defer { .. } | ExprKind::Closure { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "this construct is not supported in constant evaluation",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => {
                self.ctx
                    .struct_error(expr.span, "expected a valid constant expression")
                    .emit();
                Err(ConstEvalError)
            }
        };

        // Start from the freshly evaluated value.
        let mut val = eval_result?;

        // Apply integer range and signedness checks based on the expression type.
        if let ConstValue::Int(mut v) = val {
            let ty = self.node_type(expr.id);
            let norm = self.ctx.type_registry.normalize(ty);

            if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm).clone() {
                let literal_bound = self.bind_integer_literal_to_type(expr, ty, norm, p)?;
                let is_signed = matches!(
                    p,
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                );
                let is_unsigned = matches!(
                    p,
                    PrimitiveType::U8
                        | PrimitiveType::U16
                        | PrimitiveType::U32
                        | PrimitiveType::U64
                        | PrimitiveType::U128
                        | PrimitiveType::USize
                );

                if let Some(bound_v) = literal_bound {
                    v = bound_v;
                } else {
                    // Reinterpret wrapped unsigned bit-patterns such as `!0`.
                    if is_unsigned {
                        let mut layout = crate::LayoutEngine::new(self.ctx);
                        let bit_width = layout.compute_type_size(norm) * 8;
                        if bit_width < 128 {
                            let mask = (1i128 << bit_width) - 1;
                            v &= mask; // Truncate wrapped values like `-1` to the target bit-pattern.
                            if v < 0 {
                                self.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", v, self.ctx.ty_to_string(ty)))
                                    .with_hint("if you need a bit-pattern of all 1s, use explicit bitwise negation (e.g., `~0`) or `as` cast")
                                    .emit();
                                return Err(ConstEvalError);
                            }
                        }
                    }

                    // Check that the value fits within the destination bit width.
                    if (is_signed || is_unsigned)
                        && p != PrimitiveType::I128
                        && p != PrimitiveType::U128
                    {
                        let mut layout = crate::LayoutEngine::new(self.ctx);
                        let bit_width = layout.compute_type_size(norm) * 8;

                        let (min, max) = if is_signed {
                            let max = (1i128 << (bit_width - 1)) - 1;
                            let min = -(1i128 << (bit_width - 1));
                            (min, max)
                        } else {
                            let max = ((1u128 << bit_width) - 1) as i128;
                            (0, max)
                        };

                        if v < min || v > max {
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!(
                                        "integer literal {} is out of bounds for type `{}`",
                                        v,
                                        self.ctx.ty_to_string(ty)
                                    ),
                                )
                                .with_hint(format!("the valid range is {} to {}", min, max))
                                .emit();
                            return Err(ConstEvalError);
                        }
                    }
                }
            }
            val = ConstValue::Int(v);
        }

        Ok(val)
    }

    // ==========================================
    //            Const Eval Helpers
    // ==========================================

    fn eval_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let left = self.eval_inner(lhs, depth + 1)?;
        let right = self.eval_inner(rhs, depth + 1)?;
        let lhs_is_unsigned = self.expr_uses_unsigned_integer_semantics(lhs);

        match (left, right) {
            (ConstValue::Int(l), ConstValue::Int(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Int(l.wrapping_add(r))),
                    Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
                    Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
                    Divide => {
                        if lhs_is_unsigned {
                            self.eval_const_uint_division(l, r, span)
                        } else {
                            if r == 0 {
                                self.ctx
                                    .struct_error(span, "division by zero in constant expression")
                                    .emit();
                                Err(ConstEvalError)
                            } else {
                                self.eval_const_int_division(l, r, span)
                            }
                        }
                    }
                    Modulo => {
                        if lhs_is_unsigned {
                            self.eval_const_uint_modulo(l, r, span)
                        } else {
                            if r == 0 {
                                self.ctx
                                    .struct_error(span, "modulo by zero in constant expression")
                                    .emit();
                                Err(ConstEvalError)
                            } else {
                                self.eval_const_int_modulo(l, r, span)
                            }
                        }
                    }
                    ShiftLeft => self.eval_const_int_shift(l, r, true, lhs_is_unsigned, span),
                    ShiftRight => self.eval_const_int_shift(l, r, false, lhs_is_unsigned, span),
                    BitwiseAnd => Ok(ConstValue::Int(l & r)),
                    BitwiseOr => Ok(ConstValue::Int(l | r)),
                    BitwiseXor => Ok(ConstValue::Int(l ^ r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(if lhs_is_unsigned {
                        (l as u128) < (r as u128)
                    } else {
                        l < r
                    })),
                    LessOrEqual => Ok(ConstValue::Bool(if lhs_is_unsigned {
                        (l as u128) <= (r as u128)
                    } else {
                        l <= r
                    })),
                    GreaterThan => Ok(ConstValue::Bool(if lhs_is_unsigned {
                        (l as u128) > (r as u128)
                    } else {
                        l > r
                    })),
                    GreaterOrEqual => Ok(ConstValue::Bool(if lhs_is_unsigned {
                        (l as u128) >= (r as u128)
                    } else {
                        l >= r
                    })),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant integers")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            (ConstValue::Float(l), ConstValue::Float(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Float(l + r)),
                    Subtract => Ok(ConstValue::Float(l - r)),
                    Multiply => Ok(ConstValue::Float(l * r)),
                    Divide => Ok(ConstValue::Float(l / r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant floats")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            (ConstValue::Bool(l), ConstValue::Bool(r)) => {
                use BinaryOperator::*;
                match op {
                    LogicalAnd => Ok(ConstValue::Bool(l && r)),
                    LogicalOr => Ok(ConstValue::Bool(l || r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant booleans")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            (
                ConstValue::Enum {
                    tag: l_tag,
                    payload: l_payload,
                },
                ConstValue::Enum {
                    tag: r_tag,
                    payload: r_payload,
                },
            ) => {
                use BinaryOperator::*;
                match op {
                    Equal => Ok(ConstValue::Bool(l_tag == r_tag && l_payload == r_payload)),
                    NotEqual => Ok(ConstValue::Bool(l_tag != r_tag || l_payload != r_payload)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant enum values")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant binary expression",
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn eval_const_int_division(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match lhs.checked_div(rhs) {
            Some(value) => Ok(ConstValue::Int(value)),
            None => {
                self.ctx
                    .struct_error(span, "division overflow in constant expression")
                    .with_hint("this division cannot be represented in Kern's constant evaluator")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn eval_const_int_modulo(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match lhs.checked_rem(rhs) {
            Some(value) => Ok(ConstValue::Int(value)),
            None => {
                self.ctx
                    .struct_error(span, "modulo overflow in constant expression")
                    .with_hint("this remainder cannot be represented in Kern's constant evaluator")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn eval_const_int_shift(
        &mut self,
        lhs: i128,
        rhs: i128,
        is_left: bool,
        unsigned_lhs: bool,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if rhs < 0 {
            self.ctx
                .struct_error(
                    span,
                    "shift amount in constant expression must be non-negative",
                )
                .emit();
            return Err(ConstEvalError);
        }

        let Ok(shift) = u32::try_from(rhs) else {
            self.ctx
                .struct_error(span, "shift amount in constant expression is too large")
                .with_hint("constant integer shifts are evaluated on 128-bit values")
                .emit();
            return Err(ConstEvalError);
        };

        let value = if is_left {
            (lhs as u128).checked_shl(shift).map(|value| value as i128)
        } else if unsigned_lhs {
            (lhs as u128).checked_shr(shift).map(|value| value as i128)
        } else {
            lhs.checked_shr(shift)
        };

        match value {
            Some(value) => Ok(ConstValue::Int(value)),
            None => {
                self.ctx
                    .struct_error(span, "shift amount in constant expression is too large")
                    .with_hint("constant integer shifts are evaluated on 128-bit values")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if matches!(op, UnaryOperator::AddressOf | UnaryOperator::MutAddressOf) {
            let place = self.resolve_reference_place(
                operand,
                depth + 1,
                op == UnaryOperator::MutAddressOf,
            )?;
            return Ok(ConstValue::Pointer {
                root_scope: place.root_scope,
                root_name: place.root_name,
                path: place.path,
                is_mut: op == UnaryOperator::MutAddressOf,
            });
        }

        if op == UnaryOperator::PointerDeRef {
            let pointer = self.eval_inner(operand, depth + 1)?;
            let place = self.resolve_pointer_target(&pointer, false, span)?;
            return self.read_place_value(place.root_scope, place.root_name, &place.path, span);
        }

        let val = self.eval_inner(operand, depth + 1)?;

        let norm_ty = self.node_type(operand.id);
        let is_unsigned = if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm_ty) {
            matches!(
                p,
                PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            )
        } else {
            false
        };

        match (op, val) {
            (UnaryOperator::Negate, ConstValue::Int(v)) => {
                if is_unsigned {
                    self.ctx.struct_error(span, "cannot apply unary minus `-` to an unsigned type")
                        .with_hint("unsigned types cannot be negative. use `~` or bitwise operations if you intend to manipulate bits")
                        .emit();
                    return Err(ConstEvalError);
                }
                Ok(ConstValue::Int(v.wrapping_neg()))
            }
            (UnaryOperator::Negate, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
            (UnaryOperator::BitwiseNot, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)),
            (UnaryOperator::LogicalNot, ConstValue::Bool(v)) => Ok(ConstValue::Bool(!v)),
            _ => {
                self.ctx
                    .struct_error(span, "invalid unary operator for the given constant type")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_identifier(
        &mut self,
        name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(value) = self.lookup_local(name) {
            return Ok(value);
        }

        let sym_info = if let Some(&scope_id) = self.const_scopes.last() {
            self.ctx.scopes.resolve_from(scope_id, name).cloned()
        } else {
            self.ctx.scopes.resolve(name).cloned()
        };

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    return self.eval_const_def(def_id, depth);
                }
            } else if info.kind == SymbolKind::ConstParam {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a const generic parameter, not a standalone constant expression",
                            name_str
                        ),
                    )
                    .with_hint(
                        "direct const-generic parameter references are only supported in dedicated const-generic positions for now",
                    )
                    .emit();
                return Err(ConstEvalError);
            } else {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a {}, not a compile-time constant",
                            name_str,
                            self.kind_to_string(info.kind)
                        ),
                    )
                    .with_hint("only `const` variables can be used in constant expressions")
                    .emit();
                return Err(ConstEvalError);
            }
        }
        self.ctx
            .struct_error(span, "use of undeclared identifier in constant expression")
            .emit();
        Err(ConstEvalError)
    }

    fn eval_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        self.push_local_scope();

        for stmt in stmts {
            let stmt_expr = match &stmt.kind {
                StmtKind::Use(_) => continue,
                StmtKind::ExprStmt(expr) | StmtKind::ExprValue(expr) => expr,
            };
            let _ = self.eval_inner(stmt_expr, depth + 1)?;
            if self.return_value.is_some() || self.loop_control.is_some() {
                self.pop_local_scope();
                return Ok(ConstValue::Void);
            }
        }

        let value = if let Some(result_expr) = result {
            self.eval_inner(result_expr, depth + 1)?
        } else {
            ConstValue::Void
        };

        self.pop_local_scope();
        Ok(value)
    }

    fn eval_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        const MAX_CONST_LOOP_ITERATIONS: usize = 100_000;

        self.push_local_scope();

        if let Some(init) = init {
            let _ = self.eval_inner(init, depth + 1)?;
            if self.return_value.is_some() || self.loop_control.is_some() {
                self.pop_local_scope();
                return Ok(ConstValue::Void);
            }
        }

        self.loop_depth += 1;
        let mut iterations = 0usize;
        loop {
            if iterations >= MAX_CONST_LOOP_ITERATIONS {
                self.loop_depth -= 1;
                self.pop_local_scope();
                self.ctx
                    .struct_error(
                        span,
                        "constant evaluation exceeded the maximum loop iteration count",
                    )
                    .with_hint(
                        "check for a non-terminating `for` loop in a `const fn` or constant expression",
                    )
                    .emit();
                return Err(ConstEvalError);
            }

            if let Some(cond) = cond {
                match self.eval_inner(cond, depth + 1)? {
                    ConstValue::Bool(true) => {}
                    ConstValue::Bool(false) => break,
                    _ => {
                        self.loop_depth -= 1;
                        self.pop_local_scope();
                        self.ctx
                            .struct_error(
                                cond.span,
                                "for condition must evaluate to a boolean constant",
                            )
                            .emit();
                        return Err(ConstEvalError);
                    }
                }
                if self.return_value.is_some() {
                    break;
                }
            }

            let _ = self.eval_inner(body, depth + 1)?;
            if self.return_value.is_some() {
                break;
            }

            match self.loop_control.take() {
                Some(LoopControl::Break) => break,
                Some(LoopControl::Continue) | None => {}
            }

            if let Some(post) = post {
                let _ = self.eval_inner(post, depth + 1)?;
                if self.return_value.is_some() {
                    break;
                }
                match self.loop_control.take() {
                    Some(LoopControl::Break) => break,
                    Some(LoopControl::Continue) | None => {}
                }
            }

            iterations += 1;
        }

        self.loop_depth -= 1;
        self.pop_local_scope();
        Ok(ConstValue::Void)
    }

    fn eval_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let cond_val = self.eval_inner(cond, depth + 1)?;
        match cond_val {
            ConstValue::Bool(true) => self.eval_inner(then_branch, depth + 1),
            ConstValue::Bool(false) => {
                if let Some(else_branch) = else_branch {
                    self.eval_inner(else_branch, depth + 1)
                } else {
                    Ok(ConstValue::Void)
                }
            }
            _ => {
                self.ctx
                    .struct_error(span, "if condition must evaluate to a boolean constant")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let target_value = self.eval_inner(target, depth + 1)?;
        let target_ty = self.expr_type(target);

        for arm in arms {
            let mut bindings = None;
            let mut matched_pattern = None;

            for pattern in &arm.patterns {
                if let Some(found) =
                    self.match_pattern(pattern, &target_value, target_ty, depth + 1)?
                {
                    bindings = Some(found);
                    matched_pattern = Some(pattern);
                    break;
                }
            }

            let Some(bindings) = bindings else {
                continue;
            };

            self.push_local_scope();
            for (name, value) in bindings {
                self.define_local(name, value);
            }
            if let Some(pattern) = matched_pattern
                && let ast::MatchPatternKind::Pattern(inner) = &pattern.kind
            {
                self.install_pattern_binding_types(inner, target_ty, depth + 1)?;
            }
            let body_value = self.eval_inner(&arm.body, depth + 1);
            self.pop_local_scope();
            return body_value;
        }

        self.ctx
            .struct_error(span, "match expression did not resolve to any constant arm")
            .emit();
        Err(ConstEvalError)
    }

    fn eval_return(
        &mut self,
        value: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if self.function_depth == 0 {
            self.ctx
                .struct_error(span, "`return` is only valid inside a `const fn` body")
                .emit();
            return Err(ConstEvalError);
        }

        let value = if let Some(expr) = value {
            self.eval_inner(expr, depth + 1)?
        } else {
            ConstValue::Void
        };
        self.return_value = Some(value);
        Ok(ConstValue::Void)
    }

    fn eval_break(&mut self, span: Span) -> ConstEvalResult<ConstValue> {
        if self.loop_depth == 0 {
            self.ctx
                .struct_error(span, "`break` is only valid inside a `const fn` loop")
                .emit();
            return Err(ConstEvalError);
        }
        self.loop_control = Some(LoopControl::Break);
        Ok(ConstValue::Void)
    }

    fn eval_continue(&mut self, span: Span) -> ConstEvalResult<ConstValue> {
        if self.loop_depth == 0 {
            self.ctx
                .struct_error(span, "`continue` is only valid inside a `const fn` loop")
                .emit();
            return Err(ConstEvalError);
        }
        self.loop_control = Some(LoopControl::Continue);
        Ok(ConstValue::Void)
    }

    fn eval_assign(
        &mut self,
        lhs: &Expr,
        op: AssignmentOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let place = self.resolve_assignment_place(lhs, depth)?;

        if place.require_root_mutability {
            let Some(is_mut) = self.lookup_local_mutability_at(place.root_scope, place.root_name)
            else {
                self.ctx
                    .struct_error(
                        span,
                        "constant evaluation can only assign to local bindings declared in the current const context",
                    )
                    .emit();
                return Err(ConstEvalError);
            };
            if !is_mut {
                self.ctx
                    .struct_error(
                        span,
                        "cannot assign to an immutable local binding in constant evaluation",
                    )
                    .emit();
                return Err(ConstEvalError);
            }
        }

        let Some(mut root_value) = self.lookup_local_at(place.root_scope, place.root_name) else {
            self.ctx
                .struct_error(
                    span,
                    "failed to read local binding during constant assignment",
                )
                .emit();
            return Err(ConstEvalError);
        };
        let rhs_value = self.eval_inner(rhs, depth + 1)?;

        if place.path.is_empty() {
            let next_value = if op == AssignmentOperator::Assign {
                rhs_value
            } else {
                self.apply_assignment_operator(root_value, op, rhs_value, span)?
            };

            if !self.assign_local_at(place.root_scope, place.root_name, next_value) {
                self.ctx
                    .struct_error(
                        span,
                        "failed to update local binding during constant evaluation",
                    )
                    .emit();
                return Err(ConstEvalError);
            }

            return Ok(ConstValue::Void);
        }

        let target = self.place_value_mut(&mut root_value, &place.path, span)?;
        let next_value = if op == AssignmentOperator::Assign {
            rhs_value
        } else {
            self.apply_assignment_operator(target.clone(), op, rhs_value, span)?
        };
        *target = next_value;

        if !self.assign_local_at(place.root_scope, place.root_name, root_value) {
            self.ctx
                .struct_error(
                    span,
                    "failed to update local binding during constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        }

        Ok(ConstValue::Void)
    }

    fn resolve_assignment_place(
        &mut self,
        expr: &Expr,
        depth: usize,
    ) -> ConstEvalResult<ResolvedPlace> {
        self.resolve_reference_place(expr, depth + 1, true)
    }

    fn place_value_mut<'b>(
        &mut self,
        value: &'b mut ConstValue,
        path: &[PlaceSegment],
        span: Span,
    ) -> ConstEvalResult<&'b mut ConstValue> {
        if path.is_empty() {
            return Ok(value);
        }

        match path[0] {
            PlaceSegment::Field(field) => match value {
                ConstValue::Struct(map) => {
                    let Some(next) = map.get_mut(&field) else {
                        let field_str = self.ctx.resolve(field);
                        self.ctx
                            .struct_error(
                                span,
                                format!("field `{}` not found in constant struct", field_str),
                            )
                            .emit();
                        return Err(ConstEvalError);
                    };
                    self.place_value_mut(next, &path[1..], span)
                }
                _ => {
                    self.ctx
                        .struct_error(span, "attempted field assignment on a non-struct constant")
                        .emit();
                    Err(ConstEvalError)
                }
            },
            PlaceSegment::Index(index) => match value {
                ConstValue::Array(items) => {
                    let Some(next) = items.get_mut(index) else {
                        self.ctx
                            .struct_error(span, "constant array index out of bounds")
                            .emit();
                        return Err(ConstEvalError);
                    };
                    self.place_value_mut(next, &path[1..], span)
                }
                _ => {
                    self.ctx
                        .struct_error(
                            span,
                            "attempted indexing assignment into a non-array constant",
                        )
                        .emit();
                    Err(ConstEvalError)
                }
            },
        }
    }
}
