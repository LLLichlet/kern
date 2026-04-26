use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(crate) fn resolve_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        if expected_ty == TypeId::ERROR {
            return ConstGeneric::Error;
        }

        let value = self.build_const_generic_expr(expr, expected_ty, env_scope, context);
        self.ctx.type_registry.fold_const_generic(value)
    }

    fn build_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        if !self.expr_references_const_param(expr, env_scope) {
            return self.resolve_closed_const_generic_expr(expr, expected_ty, env_scope, context);
        }

        self.ctx.scopes.set_current_scope(env_scope);
        match &expr.kind {
            ast::ExprKind::Identifier(name) => {
                let Some(info) = self.ctx.scopes.resolve(*name).cloned() else {
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!("{} must reference a known const generic parameter", context),
                        )
                        .emit();
                    return ConstGeneric::Error;
                };

                if info.kind != SymbolKind::ConstParam {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                }

                if info.type_id != expected_ty {
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!(
                                "const generic parameter `{}` has type `{}`, but `{}` requires `{}`",
                                self.ctx.resolve(*name),
                                self.ctx.ty_to_string(info.type_id),
                                context,
                                self.ctx.ty_to_string(expected_ty)
                            ),
                        )
                        .with_hint(
                            "use an explicit `as` cast if you want to convert the const parameter to another integer type",
                        )
                        .emit();
                    return ConstGeneric::Error;
                }

                ConstGeneric::Param(*name, info.type_id)
            }
            ast::ExprKind::Unary { op, operand } => {
                let Some(op) = self.const_expr_unary_op(*op) else {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                };
                let operand =
                    self.build_const_generic_expr(operand, expected_ty, env_scope, context);
                if matches!(operand, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                ConstGeneric::Expr(
                    self.ctx
                        .type_registry
                        .intern_const_expr(ConstExprKind::Unary {
                            op,
                            expr: operand,
                            ty: expected_ty,
                        }),
                )
            }
            ast::ExprKind::Binary { lhs, op, rhs } => {
                let Some(op) = self.const_expr_binary_op(*op) else {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                };
                let lhs = self.build_const_generic_expr(lhs, expected_ty, env_scope, context);
                let rhs = self.build_const_generic_expr(rhs, expected_ty, env_scope, context);
                if matches!(lhs, ConstGeneric::Error) || matches!(rhs, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                    ConstExprKind::Binary {
                        op,
                        lhs,
                        rhs,
                        ty: expected_ty,
                    },
                ))
            }
            ast::ExprKind::As { lhs, target } => {
                let target_ty =
                    self.resolve_const_generic_param_type(target, env_scope, target.span);
                let lhs = self.build_const_generic_expr(lhs, target_ty, env_scope, context);
                if matches!(lhs, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                let cast_expr = ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                    ConstExprKind::Cast {
                        expr: lhs,
                        ty: target_ty,
                    },
                ));
                if target_ty == expected_ty {
                    cast_expr
                } else {
                    ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                        ConstExprKind::Cast {
                            expr: cast_expr,
                            ty: expected_ty,
                        },
                    ))
                }
            }
            _ => {
                self.unsupported_parametric_const_generic_expr(expr.span, context);
                ConstGeneric::Error
            }
        }
    }

    fn resolve_closed_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        self.ctx.scopes.set_current_scope(env_scope);
        let checked_ty = {
            let mut checker = ExprChecker::new(self.ctx, None);
            checker.check_expr(expr, Some(expected_ty))
        };
        if checked_ty == TypeId::ERROR {
            return ConstGeneric::Error;
        }
        let mut evaluator = ConstEvaluator::new(self.ctx);
        let Ok(mut value) = evaluator.eval_const_value(expr) else {
            return ConstGeneric::Error;
        };
        let expected_norm = self.ctx.type_registry.normalize(expected_ty);
        let checked_norm = self.ctx.type_registry.normalize(checked_ty);
        if expected_norm == checked_norm
            && matches!(
                self.ctx.type_registry.get(expected_norm),
                TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
            )
            && let ConstValue::Int(tag) = value
        {
            value = ConstValue::Enum { tag, payload: None };
        }
        let Some(value) = self.coerce_const_generic_value(value, expected_ty, expr.span, context)
        else {
            return ConstGeneric::Error;
        };
        ConstGeneric::Value(value)
    }

    fn unsupported_parametric_const_generic_expr(&mut self, span: Span, context: &str) {
        self.ctx
            .struct_error(
                span,
                format!(
                    "{} can only use symbolic computed expressions for integer const parameters",
                    context
                ),
            )
            .with_hint(
                "supported symbolic forms are direct const parameters, literals / const items, unary `-` or `~`, integer arithmetic / bitwise operators, and explicit `as` casts",
            )
            .with_hint(
                "non-integer const parameters such as `bool` may still be passed directly as literals, const items, or direct parameter references",
            )
            .emit();
    }

    fn const_expr_unary_op(&self, op: UnaryOperator) -> Option<ConstExprUnaryOp> {
        match op {
            UnaryOperator::Negate => Some(ConstExprUnaryOp::Negate),
            UnaryOperator::BitwiseNot => Some(ConstExprUnaryOp::BitwiseNot),
            UnaryOperator::LogicalNot
            | UnaryOperator::AddressOf
            | UnaryOperator::MutAddressOf
            | UnaryOperator::MetaOf
            | UnaryOperator::PointerDeRef => None,
        }
    }

    fn const_expr_binary_op(&self, op: BinaryOperator) -> Option<ConstExprBinaryOp> {
        match op {
            BinaryOperator::Add => Some(ConstExprBinaryOp::Add),
            BinaryOperator::Subtract => Some(ConstExprBinaryOp::Subtract),
            BinaryOperator::Multiply => Some(ConstExprBinaryOp::Multiply),
            BinaryOperator::Divide => Some(ConstExprBinaryOp::Divide),
            BinaryOperator::Modulo => Some(ConstExprBinaryOp::Modulo),
            BinaryOperator::BitwiseAnd => Some(ConstExprBinaryOp::BitwiseAnd),
            BinaryOperator::BitwiseOr => Some(ConstExprBinaryOp::BitwiseOr),
            BinaryOperator::BitwiseXor => Some(ConstExprBinaryOp::BitwiseXor),
            BinaryOperator::ShiftLeft => Some(ConstExprBinaryOp::ShiftLeft),
            BinaryOperator::ShiftRight => Some(ConstExprBinaryOp::ShiftRight),
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual
            | BinaryOperator::LogicalAnd
            | BinaryOperator::LogicalOr => None,
        }
    }

    fn coerce_const_generic_value(
        &mut self,
        value: ConstValue,
        expected_ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<ConstGenericValue> {
        let norm = self.ctx.type_registry.normalize(expected_ty);
        let ty_name = self.ctx.ty_to_string(expected_ty);
        let norm_kind = self.ctx.type_registry.get(norm).clone();

        match self.coerce_payloadless_enum_const_generic_value(
            &value, norm, &norm_kind, span, context, &ty_name,
        ) {
            Ok(Some(tag)) => {
                return Some(ConstGenericValue {
                    ty: norm,
                    kind: ConstGenericValueKind::Int(tag),
                });
            }
            Ok(None) => {}
            Err(()) => return None,
        }

        let TypeKind::Primitive(primitive) = norm_kind else {
            self.ctx
                .struct_error(
                    span,
                    format!("{} must use a scalar const-generic type", context),
                )
                .emit();
            return None;
        };

        if primitive == PrimitiveType::Bool {
            let value = match value {
                ConstValue::Bool(value) => value,
                _ => {
                    self.ctx
                        .struct_error(span, format!("{} must evaluate to `bool`", context))
                        .with_hint(format!("this const generic expects `{}`", ty_name))
                        .emit();
                    return None;
                }
            };
            return Some(ConstGenericValue {
                ty: norm,
                kind: ConstGenericValueKind::Bool(value),
            });
        }

        let ConstValue::Int(value) = value else {
            self.ctx
                .struct_error(
                    span,
                    format!("{} must evaluate to an integer constant", context),
                )
                .with_hint(format!("this const generic expects `{}`", ty_name))
                .emit();
            return None;
        };

        let bit_width = LayoutEngine::new(self.ctx).compute_type_size(norm) * 8;

        let coerced = match primitive {
            PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize => {
                if value < 0 {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} cannot be negative for `{}`", context, ty_name),
                        )
                        .emit();
                    return None;
                }
                let max = if bit_width >= 128 {
                    u128::MAX
                } else {
                    (1u128 << bit_width) - 1
                };
                if (value as u128) > max {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} is out of range for `{}`", context, ty_name),
                        )
                        .with_hint(format!("maximum value here is {}", max))
                        .emit();
                    return None;
                }
                value
            }
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize => {
                let (min, max) = if bit_width >= 128 {
                    (i128::MIN, i128::MAX)
                } else {
                    let max = (1i128 << (bit_width - 1)) - 1;
                    let min = -(1i128 << (bit_width - 1));
                    (min, max)
                };
                if value < min || value > max {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} is out of range for `{}`", context, ty_name),
                        )
                        .with_hint(format!("valid range here is {} to {}", min, max))
                        .emit();
                    return None;
                }
                value
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        format!("{} must currently use an integer or `bool` type", context),
                    )
                    .emit();
                return None;
            }
        };

        Some(ConstGenericValue {
            ty: norm,
            kind: ConstGenericValueKind::Int(coerced),
        })
    }

    fn coerce_payloadless_enum_const_generic_value(
        &mut self,
        value: &ConstValue,
        norm: TypeId,
        norm_kind: &TypeKind,
        span: Span,
        context: &str,
        ty_name: &str,
    ) -> Result<Option<i128>, ()> {
        let tag = match value {
            ConstValue::Enum { tag, payload } if payload.is_none() => *tag,
            ConstValue::Int(_) => {
                if matches!(norm_kind, TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)) {
                    let example = self.enum_const_generic_example(norm, norm_kind);
                    let mut diagnostic = self.ctx.struct_error(
                        span,
                        format!(
                            "{} must evaluate to a value of enum type `{}`",
                            context, ty_name
                        ),
                    );
                    if let Some(example) = example {
                        diagnostic = diagnostic.with_hint(format!(
                            "write an explicit enum value such as `{}`",
                            example
                        ));
                    } else {
                        diagnostic = diagnostic.with_hint(
                            "write an explicit payload-less enum variant instead of a raw integer",
                        );
                    }
                    diagnostic.emit();
                    return Err(());
                }
                return Ok(None);
            }
            _ => return Ok(None),
        };

        let is_valid = match norm_kind {
            TypeKind::Enum(def_id, _) => match &self.ctx.defs[def_id.0 as usize] {
                Def::Enum(def) => {
                    let variants = def.variants.clone();
                    if variants
                        .iter()
                        .any(|variant| variant.payload_type.is_some())
                    {
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "{} cannot use enum `{}` as a const generic type because it has payload-carrying variants",
                                    context, ty_name
                                ),
                            )
                            .with_hint(
                                "only payload-less enums are currently supported as const generic value types",
                            )
                            .emit();
                        return Err(());
                    }

                    let mut current_tag = 0i128;
                    let mut matched = false;
                    for variant in &variants {
                        if let Some(value_expr) = &variant.value {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            if let Ok(ConstValue::Int(value)) =
                                evaluator.eval_const_value(value_expr)
                            {
                                current_tag = value;
                            }
                        }
                        if current_tag == tag {
                            matched = true;
                            break;
                        }
                        current_tag += 1;
                    }
                    matched
                }
                _ => false,
            },
            TypeKind::AnonymousEnum(enum_def) => {
                if enum_def
                    .variants
                    .iter()
                    .any(|variant| variant.payload_ty.is_some())
                {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "{} cannot use enum `{}` as a const generic type because it has payload-carrying variants",
                                context, ty_name
                            ),
                        )
                        .with_hint(
                            "only payload-less enums are currently supported as const generic value types",
                        )
                        .emit();
                    return Err(());
                }

                let mut current_tag = 0i128;
                let mut matched = false;
                for variant in &enum_def.variants {
                    if let Some(value) = variant.explicit_value {
                        current_tag = value;
                    }
                    if current_tag == tag {
                        matched = true;
                        break;
                    }
                    current_tag += 1;
                }
                matched
            }
            _ => return Ok(None),
        };

        if !is_valid {
            self.ctx
                .struct_error(
                    span,
                    format!("{} is not a valid value for `{}`", context, ty_name),
                )
                .with_hint("use one of the declared payload-less enum variants")
                .emit();
            return Err(());
        }

        Ok(Some(tag))
    }

    fn enum_const_generic_example(&self, norm: TypeId, norm_kind: &TypeKind) -> Option<String> {
        match norm_kind {
            TypeKind::Enum(def_id, _) => {
                let def = self.ctx.defs.get(def_id.0 as usize)?;
                let Def::Enum(enum_def) = def else {
                    return None;
                };
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.payload_type.is_none())?;
                Some(format!(
                    "{}.{}",
                    self.ctx.ty_to_string(norm),
                    self.ctx.resolve(variant.name)
                ))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.payload_ty.is_none())?;
                Some(format!(".{}", self.ctx.resolve(variant.name)))
            }
            _ => None,
        }
    }

    pub(crate) fn expr_references_const_param(
        &mut self,
        expr: &ast::Expr,
        env_scope: ScopeId,
    ) -> bool {
        self.ctx.scopes.set_current_scope(env_scope);
        match &expr.kind {
            ast::ExprKind::Identifier(name) => self
                .ctx
                .scopes
                .resolve(*name)
                .is_some_and(|info| info.kind == SymbolKind::ConstParam),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(rhs, env_scope)
            }
            ast::ExprKind::Unary { operand, .. }
            | ast::ExprKind::Grouped { expr: operand }
            | ast::ExprKind::FieldAccess { lhs: operand, .. }
            | ast::ExprKind::Return(Some(operand))
            | ast::ExprKind::Defer { expr: operand }
            | ast::ExprKind::As { lhs: operand, .. }
            | ast::ExprKind::Propagate { operand, .. } => {
                self.expr_references_const_param(operand, env_scope)
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(index, env_scope)
            }
            ast::ExprKind::Call { callee, args } => {
                self.expr_references_const_param(callee, env_scope)
                    || args
                        .iter()
                        .any(|arg| self.expr_references_const_param(arg, env_scope))
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => fields
                    .iter()
                    .any(|field| self.expr_references_const_param(&field.value, env_scope)),
                ast::DataLiteralKind::Array(items) => items
                    .iter()
                    .any(|item| self.expr_references_const_param(item, env_scope)),
                ast::DataLiteralKind::Repeat { value, count } => {
                    self.expr_references_const_param(value, env_scope)
                        || self.expr_references_const_param(count, env_scope)
                }
                ast::DataLiteralKind::Scalar(inner) => {
                    self.expr_references_const_param(inner, env_scope)
                }
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr_references_const_param(cond, env_scope)
                    || self.expr_references_const_param(then_branch, env_scope)
                    || else_branch
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::Match { target, arms } => {
                self.expr_references_const_param(target, env_scope)
                    || arms
                        .iter()
                        .any(|arm| self.expr_references_const_param(&arm.body, env_scope))
            }
            ast::ExprKind::Block { stmts, result } => {
                stmts.iter().any(|stmt| match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        self.expr_references_const_param(expr, env_scope)
                    }
                    ast::StmtKind::Use(_) => false,
                }) || result
                    .as_deref()
                    .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::While { cond, body } => {
                self.expr_references_const_param(cond, env_scope)
                    || self.expr_references_const_param(body, env_scope)
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.expr_references_const_param(lhs, env_scope)
                    || start
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
                    || end
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(rhs, env_scope)
            }
            ast::ExprKind::Let {
                init, else_clause, ..
            } => {
                self.expr_references_const_param(init, env_scope)
                    || else_clause.as_ref().is_some_and(|clause| match clause {
                        ast::LetElseClause::Expr(expr) => {
                            self.expr_references_const_param(expr, env_scope)
                        }
                        ast::LetElseClause::Arms(arms) => arms
                            .iter()
                            .any(|arm| self.expr_references_const_param(&arm.body, env_scope)),
                    })
            }
            ast::ExprKind::Static { init, .. } => self.expr_references_const_param(init, env_scope),
            ast::ExprKind::GenericInstantiation { target, args } => {
                self.expr_references_const_param(target, env_scope)
                    || args.iter().any(|arg| match arg {
                        ast::GenericArg::Type(ty) => self
                            .reinterpret_type_arg_as_const_expr(ty)
                            .is_some_and(|expr| self.expr_references_const_param(&expr, env_scope)),
                        ast::GenericArg::AssocBinding { .. } => false,
                        ast::GenericArg::ConstExpr(expr) => {
                            self.expr_references_const_param(expr, env_scope)
                        }
                    })
            }
            ast::ExprKind::Closure { body, .. } => {
                self.expr_references_const_param(body, env_scope)
            }
            ast::ExprKind::AnchoredPath { .. }
            | ast::ExprKind::TypeNode(_)
            | ast::ExprKind::Integer(_)
            | ast::ExprKind::Float(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::Char(_)
            | ast::ExprKind::ByteChar(_)
            | ast::ExprKind::String(_)
            | ast::ExprKind::EnumLiteral { .. }
            | ast::ExprKind::Break
            | ast::ExprKind::Continue
            | ast::ExprKind::Return(None)
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer
            | ast::ExprKind::SelfValue => false,
        }
    }
}
