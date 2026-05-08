use super::*;
use crate::ty::{BuiltinAnonymousEnumKind, ConstGeneric, GenericArg};

fn const_binary_op(op: BinaryOperator) -> Option<ConstBinaryOp> {
    Some(match op {
        BinaryOperator::Add => ConstBinaryOp::Add,
        BinaryOperator::Subtract => ConstBinaryOp::Subtract,
        BinaryOperator::Multiply => ConstBinaryOp::Multiply,
        BinaryOperator::Divide => ConstBinaryOp::Divide,
        BinaryOperator::Modulo => ConstBinaryOp::Modulo,
        BinaryOperator::ShiftLeft => ConstBinaryOp::ShiftLeft,
        BinaryOperator::ShiftRight => ConstBinaryOp::ShiftRight,
        BinaryOperator::BitwiseAnd => ConstBinaryOp::BitwiseAnd,
        BinaryOperator::BitwiseOr => ConstBinaryOp::BitwiseOr,
        BinaryOperator::BitwiseXor => ConstBinaryOp::BitwiseXor,
        BinaryOperator::LogicalAnd => ConstBinaryOp::LogicalAnd,
        BinaryOperator::LogicalOr => ConstBinaryOp::LogicalOr,
        BinaryOperator::Equal => ConstBinaryOp::Equal,
        BinaryOperator::NotEqual => ConstBinaryOp::NotEqual,
        BinaryOperator::LessThan => ConstBinaryOp::LessThan,
        BinaryOperator::LessOrEqual => ConstBinaryOp::LessOrEqual,
        BinaryOperator::GreaterThan => ConstBinaryOp::GreaterThan,
        BinaryOperator::GreaterOrEqual => ConstBinaryOp::GreaterOrEqual,
    })
}

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
        self.host.ctx
            .struct_error(
                expr.span,
                format!(
                    "integer literal {} is out of bounds for type `{}`",
                    rendered_value,
                    self.host.ty_to_string(ty)
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

        let bit_width = self.layout_size(norm)? * 8;
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
                self.host.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", rendered_value, self.host.ty_to_string(ty)))
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

    pub(super) fn expr_uses_unsigned_integer_semantics(&mut self, expr: &Expr) -> bool {
        let ty = self.expr_type(expr);
        let norm = self.host.normalize_type(ty);
        matches!(
            self.host.type_kind(norm),
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

    fn check_pattern_recursion_depth(
        &mut self,
        depth: usize,
        span: kernc_utils::Span,
    ) -> ConstEvalResult<()> {
        if depth > 100 {
            self.host.ctx
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
                let norm_target = self.host.normalize_type(target_ty);
                if matches!(
                    self.host.type_kind(norm_target),
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
                if self.host.resolve_symbol(binding.name) != "_" {
                    self.define_local_type(binding.name, target_ty);
                    self.define_local_mutability(binding.name, binding.is_mut);
                }
            }
            ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
            ast::PatternKind::Destructure(destructure) => {
                let norm_target = self.host.normalize_type(target_ty);
                if matches!(
                    self.host.type_kind(norm_target),
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
            self.host.ctx
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
            ExprKind::Grouped { expr: inner } => self.eval_inner(inner, depth + 1),

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
                    let bit_width = self.layout_size(target_ty)? * 8;
                    let mask = if bit_width >= 128 {
                        u128::MAX
                    } else {
                        (1 << bit_width) - 1
                    };
                    let u_val = (v as u128) & mask;

                    Ok(ConstValue::Int(u_val as i128))
                } else {
                    self.host.ctx
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
                let self_name = self.host.ctx.intern("self");
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
                else_clause,
            } => {
                let value = self.eval_inner(init, depth + 1)?;
                let init_ty = self.expr_type(init);

                let is_irrefutable =
                    self.pattern_is_irrefutable(&pattern.pattern, init_ty, depth + 1)?;
                if is_irrefutable && else_clause.is_some() {
                    self.host.ctx
                        .struct_error(expr.span, "irrefutable `let` patterns cannot use `else`")
                        .with_code(kernc_utils::DiagnosticCode::IrrefutableLetElse)
                        .emit();
                    return Err(ConstEvalError);
                }
                if !is_irrefutable && else_clause.is_none() {
                    self.host.ctx
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
                    let Some(else_clause) = else_clause else {
                        self.host.ctx
                            .struct_error(
                                expr.span,
                                "refutable `let` patterns require an `else` branch",
                            )
                            .emit();
                        return Err(ConstEvalError);
                    };

                    match else_clause {
                        kernc_ast::LetElseClause::Expr(else_expr) => {
                            let _ = self.eval_inner(else_expr, depth + 1)?;
                            return Ok(ConstValue::Void);
                        }
                        kernc_ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                let Some(else_bindings) = self.match_inner_pattern(
                                    &arm.pattern,
                                    &value,
                                    init_ty,
                                    depth + 1,
                                )?
                                else {
                                    continue;
                                };

                                self.with_local_scope(|this| {
                                    for (name, value) in else_bindings {
                                        this.define_local(name, value);
                                    }
                                    this.install_pattern_binding_types(
                                        &arm.pattern,
                                        init_ty,
                                        depth + 1,
                                    )?;
                                    let _ = this.eval_inner(&arm.body, depth + 1)?;
                                    Ok(())
                                })?;
                                return Ok(ConstValue::Void);
                            }

                            self.host.ctx
                                .struct_error(
                                    else_clause.span(),
                                    "`let ... else` arms did not match the failing value during constant evaluation",
                                )
                                .emit();
                            return Err(ConstEvalError);
                        }
                    }
                };

                for (name, value) in bindings {
                    self.define_local(name, value);
                }
                self.install_pattern_binding_types(&pattern.pattern, init_ty, depth + 1)?;

                Ok(ConstValue::Void)
            }
            ExprKind::Block { stmts, result } => {
                self.eval_const_block(stmts, result.as_deref(), depth)
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.eval_const_if(cond, then_branch, else_branch.as_deref(), depth, expr.span),
            ExprKind::Match { target, arms } => self.eval_match(target, arms, depth, expr.span),
            ExprKind::While { cond, body } => self.eval_const_while(cond, body, depth, expr.span),
            ExprKind::Assign { lhs, op, rhs } => {
                self.eval_const_assign(lhs, *op, rhs, depth, expr.span)
            }
            ExprKind::Break => self.eval_const_break(expr.span),
            ExprKind::Continue => self.eval_const_continue(expr.span),
            ExprKind::Return(value) => self.eval_const_return(value.as_deref(), depth, expr.span),

            // === 8. Constant aggregate projection ===
            ExprKind::FieldAccess { lhs, field, .. } => {
                let norm_lhs = self.expr_type(lhs);

                if self.expr_is_type_namespace(lhs)
                    && matches!(
                        self.host.type_kind(norm_lhs),
                        TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
                    )
                {
                    self.eval_enum_literal_in_type(norm_lhs, *field, depth, expr.span)
                } else if let TypeKind::Module(mod_def_id) =
                    self.host.type_kind(norm_lhs).clone()
                {
                    let Some(module) = self.module_def(mod_def_id) else {
                        self.host.ctx.emit_ice(
                            expr.span,
                            format!(
                                "Kern ICE (ConstEval): Expected module definition for DefId {} during constant field access.",
                                mod_def_id.0
                            ),
                        );
                        return Err(ConstEvalError);
                    };
                    let mod_scope = module.scope_id;
                    if let Some(info) = self.host.ctx.scopes.resolve_value_in(mod_scope, *field).cloned()
                    {
                        if info.kind == SymbolKind::Const {
                            if let Some(def_id) = info.def_id {
                                self.eval_const_def(def_id, depth)
                            } else {
                                Err(ConstEvalError)
                            }
                        } else {
                            let field_str = self.host.resolve_symbol(*field);
                            self.host.ctx
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
                        let field_str = self.host.resolve_symbol(*field);
                        self.host.ctx
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
                self.host.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated directly as a value",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            ExprKind::TypeNode(type_node) => {
                let resolved_ty = self.resolve_explicit_type_node(type_node);
                let resolved_builtin = match self.host.type_kind(resolved_ty).clone() {
                    TypeKind::AnonymousEnum(enum_def) => enum_def.builtin,
                    _ => None,
                };
                match (&type_node.kind, resolved_builtin) {
                    (ast::TypeKind::Optional { .. }, _)
                    | (_, Some(BuiltinAnonymousEnumKind::Optional)) => {
                        self.host.ctx
                            .struct_error(
                                expr.span,
                                "optional types cannot be evaluated as value expressions",
                            )
                            .with_hint(
                                "optional types are ordinary enum families, not null-pointer syntax",
                            )
                            .with_hint(
                                "if you meant the empty optional constructor, write `?T.None`",
                            )
                            .emit();
                    }
                    (ast::TypeKind::Result { .. }, _)
                    | (_, Some(BuiltinAnonymousEnumKind::Result)) => {
                        self.host.ctx
                            .struct_error(
                                expr.span,
                                "result types cannot be evaluated as value expressions",
                            )
                            .with_hint(
                                "results are types; construct values with `T!E.{ Ok: ... }` or `T!E.{ Err: ... }`",
                            )
                            .emit();
                    }
                    _ => {
                        let message = if resolved_ty == TypeId::ERROR {
                            "type expressions cannot be evaluated as values".to_string()
                        } else {
                            format!(
                                "type `{}` cannot be evaluated as a value expression",
                                self.host.ty_to_string(resolved_ty)
                            )
                        };
                        self.host.ctx
                            .struct_error(expr.span, message)
                            .with_hint(
                                "construct a value with `Type.{...}`, access a constructor like `Type.Variant`, or move the type back into a type position",
                            )
                            .emit();
                    }
                }
                Err(ConstEvalError)
            }
            ExprKind::Static { .. } | ExprKind::Defer { .. } | ExprKind::Closure { .. } => {
                self.host.ctx
                    .struct_error(
                        expr.span,
                        "this construct is not supported in constant evaluation",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => {
                self.host.ctx
                    .struct_error(expr.span, "expected a valid constant expression")
                    .emit();
                Err(ConstEvalError)
            }
        };

        self.apply_post_eval_checks(expr, eval_result?)
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
        let op = match const_binary_op(op) {
            Some(op) => op,
            None => return Err(self.emit_arithmetic_error(ConstArithmeticError::TypeMismatch, span)),
        };

        kernc_consteval::eval_binary_values(left, op, right, lhs_is_unsigned)
            .map_err(|error| self.emit_arithmetic_error(error, span))
    }

    fn with_const_local_scope<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> ConstEvalResult<T>,
    ) -> ConstEvalResult<T> {
        self.core.push_local_scope();
        let result = f(self);
        self.core.pop_local_scope();
        result
    }

    fn eval_const_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        self.with_const_local_scope(|this| {
            for stmt in stmts {
                let stmt_expr = match &stmt.kind {
                    ast::StmtKind::Use(_) => continue,
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => expr,
                };
                let _ = this.eval_inner(stmt_expr, depth + 1)?;
                if this.core.has_return_value() || this.core.loop_control().is_some() {
                    return Ok(ConstValue::Void);
                }
            }

            if let Some(result_expr) = result {
                this.eval_inner(result_expr, depth + 1)
            } else {
                Ok(ConstValue::Void)
            }
        })
    }

    fn eval_const_while(
        &mut self,
        cond: &Expr,
        body: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        const MAX_CONST_LOOP_ITERATIONS: usize = 100_000;

        self.core.push_local_scope();
        self.core.enter_loop();

        let mut iterations = 0usize;
        loop {
            if iterations >= MAX_CONST_LOOP_ITERATIONS {
                self.core.leave_loop();
                self.core.pop_local_scope();
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "constant evaluation exceeded the maximum loop iteration count",
                    )
                    .with_hint(
                        "check for a non-terminating `while` loop in a `const fn` or constant expression",
                    )
                    .emit();
                return Err(ConstEvalError);
            }

            match self.eval_inner(cond, depth + 1)? {
                ConstValue::Bool(true) => {}
                ConstValue::Bool(false) => break,
                _ => {
                    self.core.leave_loop();
                    self.core.pop_local_scope();
                    self.host
                        .ctx
                        .struct_error(
                            cond.span,
                            "while condition must evaluate to a boolean constant",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }
            }
            if self.core.has_return_value() {
                break;
            }

            let _ = self.eval_inner(body, depth + 1)?;
            if self.core.has_return_value() {
                break;
            }

            match self.core.take_loop_control() {
                Some(LoopControl::Break) => break,
                Some(LoopControl::Continue) | None => {}
            }

            iterations += 1;
        }

        self.core.leave_loop();
        self.core.pop_local_scope();
        Ok(ConstValue::Void)
    }

    fn eval_const_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match self.eval_inner(cond, depth + 1)? {
            ConstValue::Bool(true) => self.eval_inner(then_branch, depth + 1),
            ConstValue::Bool(false) => {
                if let Some(else_branch) = else_branch {
                    self.eval_inner(else_branch, depth + 1)
                } else {
                    Ok(ConstValue::Void)
                }
            }
            _ => {
                self.host
                    .ctx
                    .struct_error(span, "if condition must evaluate to a boolean constant")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_const_return(
        &mut self,
        value: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if !self.core.in_function() {
            self.host
                .ctx
                .struct_error(span, "`return` is only valid inside a `const fn` body")
                .emit();
            return Err(ConstEvalError);
        }

        let value = if let Some(expr) = value {
            if let Some(return_ty) = self.core.current_return_type() {
                self.core.push_expected_type(return_ty);
                let value = self.eval_inner(expr, depth + 1);
                self.core.pop_expected_type();
                value?
            } else {
                self.eval_inner(expr, depth + 1)?
            }
        } else {
            ConstValue::Void
        };
        self.core.set_return_value(value);
        Ok(ConstValue::Void)
    }

    fn eval_const_break(&mut self, span: Span) -> ConstEvalResult<ConstValue> {
        if !self.core.in_loop() {
            self.host
                .ctx
                .struct_error(span, "`break` is only valid inside a `const fn` loop")
                .emit();
            return Err(ConstEvalError);
        }
        self.core.set_loop_control(LoopControl::Break);
        Ok(ConstValue::Void)
    }

    fn eval_const_continue(&mut self, span: Span) -> ConstEvalResult<ConstValue> {
        if !self.core.in_loop() {
            self.host
                .ctx
                .struct_error(span, "`continue` is only valid inside a `const fn` loop")
                .emit();
            return Err(ConstEvalError);
        }
        self.core.set_loop_control(LoopControl::Continue);
        Ok(ConstValue::Void)
    }

    fn eval_const_assign(
        &mut self,
        lhs: &Expr,
        op: AssignmentOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if matches!(lhs.kind, ExprKind::Infer) {
            if op != AssignmentOperator::Assign {
                self.host
                    .ctx
                    .struct_error(lhs.span, "discard assignment only supports `=`")
                    .with_hint("use `_ = ...;` to explicitly discard a value")
                    .emit();
                let _ = self.eval_inner(rhs, depth + 1);
                return Err(ConstEvalError);
            }
            let _ = self.eval_inner(rhs, depth + 1)?;
            return Ok(ConstValue::Void);
        }

        let place = self.resolve_reference_place(lhs, depth + 1, true)?;

        if place.require_root_mutability {
            let Some(is_mut) = self
                .core
                .lookup_local_mutability_at(place.root_scope, place.root_name)
            else {
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "constant evaluation can only assign to local bindings declared in the current const context",
                    )
                    .emit();
                return Err(ConstEvalError);
            };
            if !is_mut {
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "cannot assign to an immutable local binding in constant evaluation",
                    )
                    .emit();
                return Err(ConstEvalError);
            }
        }

        let Some(mut root_value) = self.core.lookup_local_at(place.root_scope, place.root_name)
        else {
            self.host
                .ctx
                .struct_error(span, "failed to read local binding during constant assignment")
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

            if !self
                .core
                .assign_local_at(place.root_scope, place.root_name, next_value)
            {
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "failed to update local binding during constant evaluation",
                    )
                    .emit();
                return Err(ConstEvalError);
            }

            return Ok(ConstValue::Void);
        }

        let target = match root_value.project_mut(&place.path) {
            Ok(value) => value,
            Err(error) => {
                self.emit_place_error(error, span, true);
                return Err(ConstEvalError);
            }
        };
        let next_value = if op == AssignmentOperator::Assign {
            rhs_value
        } else {
            self.apply_assignment_operator(target.clone(), op, rhs_value, span)?
        };
        *target = next_value;

        if !self
            .core
            .assign_local_at(place.root_scope, place.root_name, root_value)
        {
            self.host
                .ctx
                .struct_error(
                    span,
                    "failed to update local binding during constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        }

        Ok(ConstValue::Void)
    }

    pub(super) fn eval_const_int_division(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        kernc_consteval::eval_const_int_division(lhs, rhs)
            .map(ConstValue::Int)
            .map_err(|error| self.emit_arithmetic_error(error, span))
    }

    pub(super) fn eval_const_int_modulo(
        &mut self,
        lhs: i128,
        rhs: i128,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        kernc_consteval::eval_const_int_modulo(lhs, rhs)
            .map(ConstValue::Int)
            .map_err(|error| self.emit_arithmetic_error(error, span))
    }

    pub(super) fn eval_const_int_shift(
        &mut self,
        lhs: i128,
        rhs: i128,
        is_left: bool,
        unsigned_lhs: bool,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        kernc_consteval::eval_const_int_shift(lhs, rhs, is_left, unsigned_lhs)
            .map(ConstValue::Int)
            .map_err(|error| self.emit_arithmetic_error(error, span))
    }

    fn emit_arithmetic_error(&mut self, error: ConstArithmeticError, span: Span) -> ConstEvalError {
        match error {
            ConstArithmeticError::DivisionByZero => {
                self.host
                    .ctx
                    .struct_error(span, "division by zero in constant expression")
                    .emit();
            }
            ConstArithmeticError::ModuloByZero => {
                self.host
                    .ctx
                    .struct_error(span, "modulo by zero in constant expression")
                    .emit();
            }
            ConstArithmeticError::DivisionOverflow => {
                self.host
                    .ctx
                    .struct_error(span, "division overflow in constant expression")
                    .with_hint("this division cannot be represented in Kern's constant evaluator")
                    .emit();
            }
            ConstArithmeticError::ModuloOverflow => {
                self.host
                    .ctx
                    .struct_error(span, "modulo overflow in constant expression")
                    .with_hint("this remainder cannot be represented in Kern's constant evaluator")
                    .emit();
            }
            ConstArithmeticError::NegativeShift => {
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "shift amount in constant expression must be non-negative",
                    )
                    .emit();
            }
            ConstArithmeticError::ShiftTooLarge => {
                self.host
                    .ctx
                    .struct_error(span, "shift amount in constant expression is too large")
                    .with_hint("constant integer shifts are evaluated on 128-bit values")
                    .emit();
            }
            ConstArithmeticError::UnsupportedIntegerOperator => {
                self.host
                    .ctx
                    .struct_error(span, "unsupported operator for constant integers")
                    .emit();
            }
            ConstArithmeticError::UnsupportedFloatOperator => {
                self.host
                    .ctx
                    .struct_error(span, "unsupported operator for constant floats")
                    .emit();
            }
            ConstArithmeticError::UnsupportedBoolOperator => {
                self.host
                    .ctx
                    .struct_error(span, "unsupported operator for constant booleans")
                    .emit();
            }
            ConstArithmeticError::UnsupportedEnumOperator => {
                self.host
                    .ctx
                    .struct_error(span, "unsupported operator for constant enum values")
                    .emit();
            }
            ConstArithmeticError::TypeMismatch => {
                self.host
                    .ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant binary expression",
                    )
                    .emit();
            }
        }
        ConstEvalError
    }

    fn apply_post_eval_checks(
        &mut self,
        expr: &Expr,
        value: ConstValue,
    ) -> ConstEvalResult<ConstValue> {
        let mut val = value;

        if let ConstValue::Int(mut v) = val {
            let ty = self.node_type(expr.id);
            let norm = self.host.normalize_type(ty);

            if let TypeKind::Primitive(p) = self.host.type_kind(norm).clone() {
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
                    if is_unsigned {
                        let bit_width = self.layout_size(norm)? * 8;
                        if bit_width < 128 {
                            let mask = (1i128 << bit_width) - 1;
                            v &= mask;
                            if v < 0 {
                                self.host.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", v, self.host.ty_to_string(ty)))
                                    .with_hint("if you need a bit-pattern of all 1s, use explicit bitwise negation (e.g., `~0`) or `as` cast")
                                    .emit();
                                return Err(ConstEvalError);
                            }
                        }
                    }

                    if (is_signed || is_unsigned)
                        && p != PrimitiveType::I128
                        && p != PrimitiveType::U128
                    {
                        let bit_width = self.layout_size(norm)? * 8;

                        let (min, max) = if is_signed {
                            let max = (1i128 << (bit_width - 1)) - 1;
                            let min = -(1i128 << (bit_width - 1));
                            (min, max)
                        } else {
                            let max = ((1u128 << bit_width) - 1) as i128;
                            (0, max)
                        };

                        if v < min || v > max {
                            self.host
                                .ctx
                                .struct_error(
                                    expr.span,
                                    format!(
                                        "integer literal {} is out of bounds for type `{}`",
                                        v,
                                        self.host.ty_to_string(ty)
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

        if op == UnaryOperator::MetaOf {
            let norm_ty = self.node_type(operand.id);
            return match self.host.type_kind(norm_ty).clone() {
                TypeKind::Array { len, .. } => {
                    let len = self.resolved_const_generic(len);
                    let ConstGeneric::Value(value) = len else {
                        self.host.ctx
                            .struct_error(span, "array length is not a concrete constant")
                            .emit();
                        return Err(ConstEvalError);
                    };
                    let Some(len) = value.as_int() else {
                        self.host.ctx
                            .struct_error(span, "array length is not an integer constant")
                            .emit();
                        return Err(ConstEvalError);
                    };
                    Ok(ConstValue::Int(len))
                }
                TypeKind::Slice { .. } => {
                    let value = self.eval_inner(operand, depth + 1)?;
                    match value {
                        ConstValue::String(s) => Ok(ConstValue::Int(s.len() as i128)),
                        ConstValue::Array(items) => Ok(ConstValue::Int(items.len() as i128)),
                        _ => {
                            self.host.ctx
                                .struct_error(span, "cannot evaluate slice length at compile time")
                                .emit();
                            Err(ConstEvalError)
                        }
                    }
                }
                _ => {
                    self.host.ctx
                        .struct_error(span, "invalid unary operator for the given constant type")
                        .emit();
                    Err(ConstEvalError)
                }
            };
        }

        let val = self.eval_inner(operand, depth + 1)?;

        let norm_ty = self.node_type(operand.id);
        let is_unsigned = if let TypeKind::Primitive(p) = self.host.type_kind(norm_ty) {
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
                    self.host.ctx.struct_error(span, "cannot apply unary minus `-` to an unsigned type")
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
                self.host.ctx
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

        if let Some(value) = self.const_param_value(name) {
            return Ok(value);
        }

        let sym_info = if let Some(&scope_id) = self.host.const_scopes.last() {
            self.host.ctx
                .scopes
                .resolve_from_namespace(scope_id, name, crate::scope::SymbolNamespace::Value)
                .cloned()
        } else {
            self.host.ctx.scopes.resolve_value_symbol(name).cloned()
        };

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    return self.eval_const_def(def_id, depth);
                }
            } else if info.kind == SymbolKind::ConstParam {
                if let Some(value) = self.const_param_value(name) {
                    return Ok(value);
                }

                let name_str = self.host.resolve_symbol(name).to_string();
                self.host.ctx
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
                let name_str = self.host.resolve_symbol(name).to_string();
                self.host.ctx
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
        self.host.ctx
            .struct_error(span, "use of undeclared identifier in constant expression")
            .emit();
        Err(ConstEvalError)
    }

    fn const_param_value(&mut self, name: SymbolId) -> Option<ConstValue> {
        let values = self
            .core
            .type_substs()
            .iter()
            .rev()
            .filter_map(|subst_map| match subst_map.get(&name).copied() {
                Some(GenericArg::Const(value)) => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>();

        for value in values {
            let value = self.resolved_const_generic(value);
            let ConstGeneric::Value(value) = value else {
                continue;
            };

            return match value.kind {
                kernc_ty::ConstGenericValueKind::Int(value) => Some(ConstValue::Int(value)),
                kernc_ty::ConstGenericValueKind::Bool(value) => Some(ConstValue::Bool(value)),
            };
        }

        None
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

            return self.with_local_scope(|this| {
                for (name, value) in bindings {
                    this.define_local(name, value);
                }
                if let Some(pattern) = matched_pattern
                    && let ast::MatchPatternKind::Pattern(inner) = &pattern.kind
                {
                    this.install_pattern_binding_types(inner, target_ty, depth + 1)?;
                }
                this.eval_inner(&arm.body, depth + 1)
            });
        }

        self.host.ctx
            .struct_error(span, "match expression did not resolve to any constant arm")
            .emit();
        Err(ConstEvalError)
    }

}
