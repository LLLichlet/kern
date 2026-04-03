use super::ExprChecker;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{BinaryOperator, Expr, UnaryOperator};
use kernc_utils::Span;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
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

                // Normal numeric arithmetic.
                if !self.check_coercion(rhs, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
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
                if !self.check_coercion(rhs, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
            Equal | NotEqual => {
                // Allow `void == void`; constexpr will fold it to `true`.
                if !self.check_coercion(rhs, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                TypeId::BOOL
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
                if !self.check_coercion(rhs, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                TypeId::BOOL
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs, TypeId::BOOL, l_norm);
                self.check_coercion(rhs, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            _ => {
                // Bitwise Ops
                if !self.ctx.type_registry.is_integer(l_norm) {
                    self.ctx
                        .struct_error(lhs.span, "bitwise operations require integer types")
                        .emit();
                }
                if !self.check_coercion(rhs, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
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
                if !self.ctx.type_registry.is_integer(op_ty_id)
                    && !self.ctx.type_registry.is_float(op_ty_id)
                {
                    self.ctx
                        .struct_error(span, "negation requires a numeric type")
                        .emit();
                }
                op_ty
            }
            UnaryOperator::LogicalNot => {
                self.check_coercion(operand, TypeId::BOOL, op_ty);
                TypeId::BOOL
            }
            UnaryOperator::BitwiseNot => {
                let op_ty_id = self.resolve_tv(op_ty);
                if !self.ctx.type_registry.is_integer(op_ty_id) {
                    self.ctx
                        .struct_error(span, "bitwise NOT requires an integer type")
                        .emit();
                }
                op_ty
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
