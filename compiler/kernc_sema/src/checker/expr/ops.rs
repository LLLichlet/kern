use super::ExprChecker;
use crate::def::Def;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{BinaryOperator, Expr, UnaryOperator};
use kernc_utils::{DiagnosticCode, Span};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn fresh_type_var(&mut self) -> TypeId {
        let vid = self.type_vars.len() as u32;
        self.type_vars.push(None);
        self.ctx.type_registry.intern(TypeKind::TypeVar(vid))
    }

    fn has_builtin_binary_fast_path(
        &mut self,
        op: BinaryOperator,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
    ) -> bool {
        let l_norm = self.resolve_tv(lhs_ty);
        let r_norm = self.resolve_tv(rhs_ty);
        let is_l_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );
        let is_r_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        match op {
            BinaryOperator::Add | BinaryOperator::Subtract => {
                if is_l_ptr || is_r_ptr {
                    return true;
                }

                (self.ctx.type_registry.is_integer(l_norm)
                    && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
            }
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => {
                (self.ctx.type_registry.is_integer(l_norm)
                    && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    return true;
                }
                if l_norm == r_norm && self.is_pure_enum_type(l_norm) {
                    return true;
                }
                is_l_ptr
                    || is_r_ptr
                    || (self.ctx.type_registry.is_integer(l_norm)
                        && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
                    || (l_norm == TypeId::BOOL && r_norm == TypeId::BOOL)
            }
            BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual => {
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    return false;
                }
                is_l_ptr
                    || is_r_ptr
                    || (self.ctx.type_registry.is_integer(l_norm)
                        && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
                    || (l_norm == TypeId::BOOL && r_norm == TypeId::BOOL)
            }
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => true,
            BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight => {
                self.ctx.type_registry.is_integer(l_norm)
                    && self.ctx.type_registry.is_integer(r_norm)
            }
        }
    }

    fn is_pure_enum_type(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Enum(def_id, _) => {
                let Def::Enum(def) = &self.ctx.defs[def_id.0 as usize] else {
                    return false;
                };
                def.variants
                    .iter()
                    .all(|variant| variant.payload_type.is_none())
            }
            TypeKind::AnonymousEnum(anon) => anon
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none()),
            _ => false,
        }
    }

    fn builtin_binary_trait_name(&self, op: BinaryOperator) -> Option<(&'static str, bool)> {
        match op {
            BinaryOperator::Add => Some(("Add", false)),
            BinaryOperator::Subtract => Some(("Sub", false)),
            BinaryOperator::Multiply => Some(("Mul", false)),
            BinaryOperator::Divide => Some(("Div", false)),
            BinaryOperator::Modulo => Some(("Rem", false)),
            BinaryOperator::Equal | BinaryOperator::NotEqual => Some(("Eq", true)),
            BinaryOperator::LessThan => Some(("Lt", true)),
            BinaryOperator::LessOrEqual => Some(("Le", true)),
            BinaryOperator::GreaterThan => Some(("Gt", true)),
            BinaryOperator::GreaterOrEqual => Some(("Ge", true)),
            BinaryOperator::BitwiseAnd => Some(("BitAnd", false)),
            BinaryOperator::BitwiseOr => Some(("BitOr", false)),
            BinaryOperator::BitwiseXor => Some(("BitXor", false)),
            BinaryOperator::ShiftLeft => Some(("Shl", false)),
            BinaryOperator::ShiftRight => Some(("Shr", false)),
            // `and` / `or` are builtin bool-only short-circuit operators.
            // Treating them as ordinary trait dispatch would either eagerly evaluate
            // the RHS or force a separate lazy/thunk protocol into operator syntax.
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => None,
        }
    }

    fn builtin_unary_trait_name(&self, op: UnaryOperator) -> Option<(&'static str, bool)> {
        match op {
            UnaryOperator::Negate => Some(("Neg", false)),
            UnaryOperator::LogicalNot => Some(("Not", false)),
            UnaryOperator::BitwiseNot => Some(("BitNot", false)),
            // Address-of, metadata access, and dereference carry memory/control-flow
            // semantics and remain language-owned instead of overloadable traits.
            UnaryOperator::AddressOf
            | UnaryOperator::MutAddressOf
            | UnaryOperator::MetaOf
            | UnaryOperator::PointerDeRef => None,
        }
    }

    fn require_builtin_binary_trait(
        &mut self,
        lhs: &Expr,
        _rhs: &Expr,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
        op: BinaryOperator,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let Some((trait_name, returns_bool)) = self.builtin_binary_trait_name(op) else {
            self.ctx.emit_ice(
                lhs.span,
                "missing builtin trait mapping for binary operator",
            );
            return TypeId::ERROR;
        };

        let out_ty = if returns_bool {
            TypeId::BOOL
        } else {
            expected_ty.unwrap_or_else(|| self.fresh_type_var())
        };

        let trait_args = if returns_bool {
            vec![rhs_ty]
        } else {
            vec![rhs_ty, out_ty]
        };
        let Some(target_trait_ty) = self.ctx.builtin_trait_ty(trait_name, trait_args) else {
            self.ctx.emit_ice(
                lhs.span,
                format!("missing builtin operator trait `{}`", trait_name),
            );
            return TypeId::ERROR;
        };

        if self.check_trait_impl(lhs_ty, target_trait_ty) {
            out_ty
        } else {
            let bound_hint = self.ctx.ty_to_string(target_trait_ty);
            self.ctx
                .struct_error(
                    lhs.span,
                    format!(
                        "operator `{}` is not available for `{}` and `{}`",
                        match op {
                            BinaryOperator::Add => "+",
                            BinaryOperator::Subtract => "-",
                            BinaryOperator::Multiply => "*",
                            BinaryOperator::Divide => "/",
                            BinaryOperator::Modulo => "%",
                            BinaryOperator::Equal => "==",
                            BinaryOperator::NotEqual => "!=",
                            BinaryOperator::LessThan => "<",
                            BinaryOperator::GreaterThan => ">",
                            BinaryOperator::LessOrEqual => "<=",
                            BinaryOperator::GreaterOrEqual => ">=",
                            BinaryOperator::LogicalAnd => "and",
                            BinaryOperator::LogicalOr => "or",
                            BinaryOperator::BitwiseAnd => "&",
                            BinaryOperator::BitwiseOr => "|",
                            BinaryOperator::BitwiseXor => "^",
                            BinaryOperator::ShiftLeft => "<<",
                            BinaryOperator::ShiftRight => ">>",
                        },
                        self.ctx.ty_to_string(lhs_ty),
                        self.ctx.ty_to_string(rhs_ty)
                    ),
                )
                .with_hint(format!(
                    "add a builtin trait bound such as `{}` or implement it for the left-hand type",
                    bound_hint
                ))
                .emit();
            TypeId::ERROR
        }
    }

    fn require_builtin_unary_trait(
        &mut self,
        operand: &Expr,
        operand_ty: TypeId,
        op: UnaryOperator,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let Some((trait_name, _)) = self.builtin_unary_trait_name(op) else {
            self.ctx.emit_ice(
                operand.span,
                "missing builtin trait mapping for unary operator",
            );
            return TypeId::ERROR;
        };

        let out_ty = expected_ty.unwrap_or_else(|| self.fresh_type_var());
        let Some(target_trait_ty) = self.ctx.builtin_trait_ty(trait_name, vec![out_ty]) else {
            self.ctx.emit_ice(
                operand.span,
                format!("missing builtin operator trait `{}`", trait_name),
            );
            return TypeId::ERROR;
        };

        if self.check_trait_impl(operand_ty, target_trait_ty) {
            out_ty
        } else {
            let bound_hint = self.ctx.ty_to_string(target_trait_ty);
            self.ctx
                .struct_error(
                    operand.span,
                    format!(
                        "operator is not available for `{}`",
                        self.ctx.ty_to_string(operand_ty)
                    ),
                )
                .with_hint(format!(
                    "add a builtin trait bound such as `{}` or implement it for the operand type",
                    bound_hint
                ))
                .emit();
            TypeId::ERROR
        }
    }

    pub fn check_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        // 1. Check the left operand first and recover its concrete type.
        let lhs_ty = self.check_expr(lhs, expected_ty);
        let l_norm = self.resolve_tv(lhs_ty);

        // 2. Detect pointer arithmetic up front.
        let is_l_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        // 3. Derive the expected type for the right operand.
        // Pointer addition and subtraction must not force integer literals to become pointers.
        let rhs_expected =
            if is_l_ptr && (op == BinaryOperator::Add || op == BinaryOperator::Subtract) {
                None // Let the right-hand side infer naturally as an integer offset.
            } else {
                Some(lhs_ty) // Other operators still benefit from pointer-aware context.
            };

        // 4. Check the right operand with the repaired expectation.
        let rhs_ty = self.check_expr(rhs, rhs_expected);
        let r_norm = self.resolve_tv(rhs_ty);

        // 5. Propagate earlier errors.
        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 6. Detect whether the right operand is also a pointer.
        let is_r_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        use BinaryOperator::*;
        match op {
            Add | Subtract => {
                if is_l_ptr || is_r_ptr {
                    if op == Add {
                        // `ptr + int` or `int + ptr`.
                        if is_l_ptr && (r_norm == TypeId::USIZE || r_norm == TypeId::ISIZE) {
                            return l_norm;
                        }
                        if is_r_ptr && (l_norm == TypeId::USIZE || l_norm == TypeId::ISIZE) {
                            return r_norm;
                        }
                    } else if op == Subtract {
                        // ptr - int
                        if is_l_ptr && (r_norm == TypeId::USIZE || r_norm == TypeId::ISIZE) {
                            return l_norm;
                        }
                        // `ptr - ptr` yields an offset.
                        if is_l_ptr && is_r_ptr {
                            if l_norm == r_norm {
                                return TypeId::ISIZE; // Pointer subtraction yields a signed offset.
                            } else {
                                self.ctx
                                    .struct_error(
                                        lhs.span,
                                        "cannot subtract pointers of different types",
                                    )
                                    .with_hint("both pointers must point to the exact same type")
                                    .emit();
                                return TypeId::ERROR;
                            }
                        }
                    }

                    // Reject invalid pointer arithmetic combinations.
                    self.ctx.struct_error(lhs.span, "invalid pointer arithmetic")
                        .with_hint("pointer arithmetic requires `usize` or `isize` offsets, or subtraction between identical pointer types")
                        .emit();
                    return TypeId::ERROR;
                }

                // Reject arithmetic on `void`.
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    self.ctx
                        .struct_error(
                            lhs.span,
                            "arithmetic operations cannot be applied to `void`",
                        )
                        .with_hint("`void` is a zero-sized type and carries no scalar value")
                        .emit();
                    return TypeId::ERROR;
                }

                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return l_norm;
                }

                self.require_builtin_binary_trait(lhs, rhs, l_norm, r_norm, op, expected_ty)
            }
            Multiply | Divide | Modulo => {
                if is_l_ptr || is_r_ptr {
                    self.ctx
                        .struct_error(
                            lhs.span,
                            "multiplication, division, or modulo cannot be applied to pointers",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    self.ctx
                        .struct_error(
                            lhs.span,
                            "arithmetic operations cannot be applied to `void`",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return l_norm;
                }

                self.require_builtin_binary_trait(lhs, rhs, l_norm, r_norm, op, expected_ty)
            }
            Equal | NotEqual => {
                // Allow `void == void`; constexpr will fold it to `true`.
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return TypeId::BOOL;
                }

                self.require_builtin_binary_trait(lhs, rhs, l_norm, r_norm, op, expected_ty)
            }
            LessThan | GreaterThan | LessOrEqual | GreaterOrEqual => {
                // Ordering comparisons on `void` are never valid.
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    self.ctx
                        .struct_error(
                            lhs.span,
                            "relational comparisons cannot be applied to `void`",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return TypeId::BOOL;
                }

                self.require_builtin_binary_trait(lhs, rhs, l_norm, r_norm, op, expected_ty)
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs, TypeId::BOOL, l_norm);
                self.check_coercion(rhs, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            BitwiseAnd | BitwiseOr | BitwiseXor | ShiftLeft | ShiftRight => {
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.ctx.type_registry.is_integer(l_norm) {
                        self.ctx
                            .struct_error(lhs.span, "bitwise operations require integer types")
                            .emit();
                    }
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return l_norm;
                }

                self.require_builtin_binary_trait(lhs, rhs, l_norm, r_norm, op, expected_ty)
            }
        }
    }

    pub fn check_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        span: Span,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let inner_expected = match op {
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => expected_ty,
            // Support both immutable and mutable address-of forms.
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                if let Some(exp) = expected_ty {
                    let norm = self.resolve_tv(exp);
                    match self.ctx.type_registry.get(norm) {
                        TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                            Some(*elem)
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        let op_ty = self.check_expr(operand, inner_expected);
        if op_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        match op {
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                let is_mut = op == UnaryOperator::MutAddressOf;

                // Mutable address-of requires a mutable lvalue.
                if is_mut && !self.is_lvalue_mutable(operand) {
                    self.ctx
                        .struct_error(
                            span,
                            "cannot take mutable address `..&` of immutable memory",
                        )
                        .with_hint(
                            "declare the variable with `let mut` or ensure the target is mutable",
                        )
                        .emit();
                }

                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: op_ty,
                })
            }
            UnaryOperator::PointerDeRef => {
                let norm = self.resolve_tv(op_ty);
                match self.ctx.type_registry.get(norm).clone() {
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => elem,
                    _ => {
                        let ty_str = self.ctx.ty_to_string(op_ty);
                        self.ctx
                            .struct_error(span, "cannot dereference a non-pointer type")
                            .with_hint(format!("type is `{}`", ty_str))
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::MetaOf => {
                let norm = self.resolve_tv(op_ty);
                let kind = self.ctx.type_registry.get(norm).clone(); // Clone to simplify matching.

                match kind {
                    // 1. Slices and arrays produce their logical length.
                    TypeKind::Array { .. }
                    | TypeKind::Slice { .. }
                    | TypeKind::ArrayInfer { .. } => TypeId::USIZE,

                    // 2. Closure and trait fat pointers expose their underlying data pointer.
                    TypeKind::Pointer { is_mut, elem } | TypeKind::VolatilePtr { is_mut, elem } => {
                        let elem_norm = self.resolve_tv(elem);
                        let inner_kind = self.ctx.type_registry.get(elem_norm);

                        // Both closure interfaces and trait objects use `{ data_ptr, meta_ptr }`.
                        if matches!(
                            inner_kind,
                            TypeKind::ClosureInterface { .. } | TypeKind::TraitObject(..)
                        ) {
                            // Return a raw data pointer for explicit frees or low-level unsafe casts.
                            self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut,
                                elem: TypeId::VOID,
                            })
                        } else {
                            self.ctx
                                .struct_error(
                                    span,
                                    "operator `#` cannot be applied to a standard thin pointer",
                                )
                                .with_hint("it can only extract metadata or state from fat pointers (e.g., slices `[]T`, closures `*Fn`, or trait objects `*Trait`)")
                                .emit();
                            TypeId::ERROR
                        }
                    }

                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "operator `#` can only be applied to arrays, slices, or fat pointers",
                            )
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::Negate => {
                let op_ty_id = self.resolve_tv(op_ty);
                if self.ctx.type_registry.is_integer(op_ty_id)
                    || self.ctx.type_registry.is_float(op_ty_id)
                {
                    return op_ty;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
            UnaryOperator::LogicalNot => {
                if self.resolve_tv(op_ty) == TypeId::BOOL {
                    return TypeId::BOOL;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
            UnaryOperator::BitwiseNot => {
                let op_ty_id = self.resolve_tv(op_ty);
                if self.ctx.type_registry.is_integer(op_ty_id) {
                    return op_ty;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
        }
    }

    pub fn check_assign(&mut self, lhs: &Expr, rhs: &Expr) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        // Defer to the inherited-mutability analysis.
        if !self.is_lvalue_mutable(lhs) && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    lhs.span,
                    "cannot assign to an immutable variable or location",
                )
                .with_code(DiagnosticCode::RequiresLetMut)
                .with_hint("if this is a variable, declare it with `let mut`")
                .with_hint(
                    "if this is a pointer dereference, ensure it is a mutable pointer (`*mut T`)",
                )
                .emit();
        }

        let l_norm = self.resolve_tv(lhs_ty);
        let rhs_ty = self.check_expr(rhs, Some(l_norm));

        if lhs_ty == TypeId::ERROR || rhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let rhs_ty_id = self.resolve_tv(rhs_ty);
        self.check_coercion(rhs, l_norm, rhs_ty_id);
        TypeId::VOID
    }
}
