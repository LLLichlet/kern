use super::{ExprChecker, NumericInferenceKind};
use crate::def::Def;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{AssignmentOperator, BinaryOperator, Expr, ExprKind, UnaryOperator};
use kernc_utils::{DiagnosticCode, NodeId, Span};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn alloc_type_var(&mut self, kind: Option<NumericInferenceKind>) -> TypeId {
        let vid = self.type_vars.len() as u32;
        self.type_vars.push(None);
        self.numeric_type_vars
            .push(kind.map(Self::numeric_state_for_kind));
        self.ctx.type_registry.intern(TypeKind::TypeVar(vid))
    }

    fn builtin_rhs_expectation_for_lhs(
        &mut self,
        op: BinaryOperator,
        lhs_ty: TypeId,
        lhs_norm: TypeId,
    ) -> Option<TypeId> {
        match op {
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => Some(TypeId::BOOL),
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo
            | BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight => {
                if self.type_is_integer_like(lhs_ty)
                    || self.type_is_float_like(lhs_ty)
                    || lhs_norm == TypeId::BOOL
                    || self.ctx.type_registry.is_simd(lhs_norm)
                    || self.is_pure_enum_type(lhs_norm)
                {
                    Some(lhs_ty)
                } else {
                    None
                }
            }
        }
    }

    fn simd_compare_type(&mut self, ty: TypeId) -> TypeId {
        let Some((_, lanes)) = self.ctx.type_registry.simd_info(ty) else {
            return TypeId::ERROR;
        };
        self.ctx.type_registry.intern(TypeKind::Simd {
            elem: TypeId::BOOL,
            lanes,
        })
    }

    fn has_builtin_simd_binary_fast_path(
        &mut self,
        op: BinaryOperator,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
    ) -> bool {
        let Some((l_elem, l_lanes)) = self.ctx.type_registry.simd_info(lhs_ty) else {
            return false;
        };
        let Some((r_elem, r_lanes)) = self.ctx.type_registry.simd_info(rhs_ty) else {
            return false;
        };

        if l_elem != r_elem || l_lanes != r_lanes {
            return false;
        }

        let elem_is_int = self.ctx.type_registry.is_integer(l_elem);
        let elem_is_float = self.ctx.type_registry.is_float(l_elem);
        let elem_is_bool = l_elem == TypeId::BOOL;

        match op {
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Modulo => elem_is_int || elem_is_float,
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                elem_is_int || elem_is_float || elem_is_bool
            }
            BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual => elem_is_int || elem_is_float,
            BinaryOperator::BitwiseAnd | BinaryOperator::BitwiseOr | BinaryOperator::BitwiseXor => {
                elem_is_int || elem_is_bool
            }
            BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight => elem_is_int,
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => false,
        }
    }

    pub(crate) fn fresh_type_var(&mut self) -> TypeId {
        self.alloc_type_var(None)
    }

    pub(crate) fn fresh_numeric_type_var(&mut self, kind: NumericInferenceKind) -> TypeId {
        self.alloc_type_var(Some(kind))
    }

    fn constrain_pointer_offset_type(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.resolve_tv(ty);
        let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(resolved).clone() else {
            return resolved;
        };

        if self.numeric_inference_kind(vid).is_none() {
            return resolved;
        }

        if self.constrain_numeric_type_var(vid, Self::NUMERIC_CAND_POINTER_OFFSETS) {
            self.resolve_tv(ty)
        } else {
            resolved
        }
    }

    fn type_is_pointer_offset_like(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        if norm == TypeId::USIZE || norm == TypeId::ISIZE {
            return true;
        }

        self.type_numeric_candidates(norm)
            .is_some_and(|candidates| {
                candidates != 0 && (candidates & !Self::NUMERIC_CAND_POINTER_OFFSETS) == 0
            })
    }

    fn constrain_integer_type(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.resolve_tv(ty);
        let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(resolved).clone() else {
            return resolved;
        };

        if self.numeric_inference_kind(vid).is_none() {
            return resolved;
        }

        if self.constrain_numeric_type_var(vid, Self::NUMERIC_CAND_ALL_INTS) {
            self.resolve_tv(ty)
        } else {
            resolved
        }
    }

    fn constrain_signed_numeric_type(&mut self, ty: TypeId) -> TypeId {
        let resolved = self.resolve_tv(ty);
        let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(resolved).clone() else {
            return resolved;
        };

        if self.numeric_inference_kind(vid).is_none() {
            return resolved;
        }

        let signed_candidates = Self::NUMERIC_CAND_I8
            | Self::NUMERIC_CAND_I16
            | Self::NUMERIC_CAND_I32
            | Self::NUMERIC_CAND_I64
            | Self::NUMERIC_CAND_I128
            | Self::NUMERIC_CAND_ISIZE
            | Self::NUMERIC_CAND_ALL_FLOATS;
        if self.constrain_numeric_type_var(vid, signed_candidates) {
            self.resolve_tv(ty)
        } else {
            resolved
        }
    }

    fn has_builtin_binary_fast_path(
        &mut self,
        op: BinaryOperator,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
    ) -> bool {
        let l_norm = self.resolve_tv(lhs_ty);
        let r_norm = self.resolve_tv(rhs_ty);
        if self.has_builtin_simd_binary_fast_path(op, l_norm, r_norm) {
            return true;
        }
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
                (self.type_is_integer_like(lhs_ty) && self.type_is_integer_like(rhs_ty))
                    || (self.type_is_numeric_like(lhs_ty) && self.type_is_numeric_like(rhs_ty))
            }
            BinaryOperator::Multiply | BinaryOperator::Divide | BinaryOperator::Modulo => {
                (self.type_is_integer_like(lhs_ty) && self.type_is_integer_like(rhs_ty))
                    || (self.type_is_numeric_like(lhs_ty) && self.type_is_numeric_like(rhs_ty))
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual => {
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    return true;
                }
                if l_norm == r_norm && self.is_pure_enum_type(l_norm) {
                    return true;
                }
                (is_l_ptr && is_r_ptr)
                    || (self.type_is_numeric_like(lhs_ty) && self.type_is_numeric_like(rhs_ty))
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
                (is_l_ptr && is_r_ptr)
                    || (self.type_is_numeric_like(lhs_ty) && self.type_is_numeric_like(rhs_ty))
                    || (l_norm == TypeId::BOOL && r_norm == TypeId::BOOL)
            }
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => true,
            BinaryOperator::BitwiseAnd
            | BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight => {
                self.type_is_integer_like(lhs_ty) && self.type_is_integer_like(rhs_ty)
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
        binary_expr_id: NodeId,
        lhs: &Expr,
        rhs: &Expr,
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

        let mut lhs_trait_self_candidates = vec![lhs_ty];
        if let Some(slice_ty) = self.immutable_slice_type_for_array(lhs_ty) {
            lhs_trait_self_candidates.push(slice_ty);
        }

        let mut rhs_trait_arg_candidates = vec![rhs_ty];
        if let Some(slice_ty) = self.immutable_slice_type_for_array(rhs_ty) {
            rhs_trait_arg_candidates.push(slice_ty);
        }

        let mut first_target_trait_ty = None;
        for lhs_trait_self_ty in lhs_trait_self_candidates {
            for rhs_trait_arg_ty in rhs_trait_arg_candidates.iter().copied() {
                let target_trait_ty = if returns_bool {
                    self.ctx
                        .builtin_trait_ty(trait_name, vec![rhs_trait_arg_ty])
                } else {
                    self.ctx.builtin_trait_ty_with_assoc(
                        trait_name,
                        vec![rhs_trait_arg_ty],
                        vec![("Out", out_ty)],
                    )
                };
                let Some(target_trait_ty) = target_trait_ty else {
                    self.ctx.emit_ice(
                        lhs.span,
                        format!("missing builtin operator trait `{}`", trait_name),
                    );
                    return TypeId::ERROR;
                };
                first_target_trait_ty.get_or_insert(target_trait_ty);

                if self.check_trait_impl(lhs_trait_self_ty, target_trait_ty) {
                    if lhs_trait_self_ty != lhs_ty {
                        if !self.check_coercion(lhs, lhs_trait_self_ty, lhs_ty) {
                            return TypeId::ERROR;
                        }
                        self.ctx.set_binary_operator_lhs_trait_self_ty(
                            binary_expr_id,
                            lhs_trait_self_ty,
                        );
                    }
                    if rhs_trait_arg_ty != rhs_ty {
                        if !self.check_coercion(rhs, rhs_trait_arg_ty, rhs_ty) {
                            return TypeId::ERROR;
                        }
                        self.ctx
                            .set_binary_operator_rhs_trait_arg_ty(binary_expr_id, rhs_trait_arg_ty);
                    }
                    return self.resolve_tv(out_ty);
                }
            }
        }

        {
            let bound_hint = first_target_trait_ty
                .map(|ty| self.ctx.ty_to_string(ty))
                .unwrap_or_else(|| format!("{}[{}]", trait_name, self.ctx.ty_to_string(rhs_ty)));
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

    fn immutable_slice_type_for_array(&mut self, ty: TypeId) -> Option<TypeId> {
        let norm = self.resolve_tv(ty);
        let (TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem }) =
            self.ctx.type_registry.get(norm).clone()
        else {
            return None;
        };
        Some(self.ctx.type_registry.intern(TypeKind::Slice {
            is_mut: false,
            elem,
        }))
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
        let Some(target_trait_ty) =
            self.ctx
                .builtin_trait_ty_with_assoc(trait_name, vec![], vec![("Out", out_ty)])
        else {
            self.ctx.emit_ice(
                operand.span,
                format!("missing builtin operator trait `{}`", trait_name),
            );
            return TypeId::ERROR;
        };

        if self.check_trait_impl(operand_ty, target_trait_ty) {
            self.resolve_tv(out_ty)
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
        binary_expr_id: NodeId,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        // 1. Check the left operand first and recover its concrete type.
        let lhs_expected = match op {
            BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr => Some(TypeId::BOOL),
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual => None,
            _ => expected_ty,
        };
        let lhs_ty = self.check_expr(lhs, lhs_expected);
        let l_norm = self.resolve_tv(lhs_ty);

        // 2. Detect pointer arithmetic up front.
        let is_l_obj_ptr = matches!(self.ctx.type_registry.get(l_norm), TypeKind::Pointer { .. });
        let is_l_addr_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::VolatilePtr { .. }
        );
        let is_l_ptr = is_l_obj_ptr || is_l_addr_ptr;

        // 3. Derive the expected type for the right operand.
        // Pointer addition and subtraction must not force integer literals to become pointers.
        let rhs_expected =
            if is_l_ptr && (op == BinaryOperator::Add || op == BinaryOperator::Subtract) {
                None
            } else {
                self.builtin_rhs_expectation_for_lhs(op, lhs_ty, l_norm)
            };

        // 4. Check the right operand with the repaired expectation.
        let rhs_ty = self.check_expr(rhs, rhs_expected);
        let r_norm = self.resolve_tv(rhs_ty);

        // 5. Propagate earlier errors.
        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 6. Detect whether the right operand is also a pointer.
        let is_r_obj_ptr = matches!(self.ctx.type_registry.get(r_norm), TypeKind::Pointer { .. });
        let is_r_addr_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::VolatilePtr { .. }
        );
        let is_r_ptr = is_r_obj_ptr || is_r_addr_ptr;

        use BinaryOperator::*;
        match op {
            Add | Subtract => {
                if is_l_ptr || is_r_ptr {
                    let l_norm = if is_r_ptr {
                        self.constrain_pointer_offset_type(lhs_ty)
                    } else {
                        l_norm
                    };
                    let r_norm = if is_l_ptr {
                        self.constrain_pointer_offset_type(rhs_ty)
                    } else {
                        r_norm
                    };
                    let rhs_is_offset = self.type_is_pointer_offset_like(r_norm);
                    let lhs_is_offset = self.type_is_pointer_offset_like(l_norm);

                    if op == Add {
                        if is_l_ptr && rhs_is_offset {
                            return l_norm;
                        }
                        if is_r_ptr && lhs_is_offset {
                            return r_norm;
                        }
                    } else {
                        if is_l_ptr && rhs_is_offset {
                            return l_norm;
                        }
                        if is_l_ptr && is_r_ptr {
                            if l_norm == r_norm {
                                return TypeId::ISIZE;
                            }

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

                    self.ctx
                        .struct_error(lhs.span, "invalid pointer arithmetic")
                        .with_hint("builtin pointer arithmetic only supports `ptr +/- usize`, `ptr +/- isize`, and `ptr - ptr` for identical pointer types")
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

                self.require_builtin_binary_trait(
                    binary_expr_id,
                    lhs,
                    rhs,
                    l_norm,
                    r_norm,
                    op,
                    expected_ty,
                )
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

                self.require_builtin_binary_trait(
                    binary_expr_id,
                    lhs,
                    rhs,
                    l_norm,
                    r_norm,
                    op,
                    expected_ty,
                )
            }
            Equal | NotEqual => {
                // Allow `void == void`; constexpr will fold it to `true`.
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    if self.ctx.type_registry.is_simd(l_norm) {
                        return self.simd_compare_type(l_norm);
                    }
                    return TypeId::BOOL;
                }

                self.require_builtin_binary_trait(
                    binary_expr_id,
                    lhs,
                    rhs,
                    l_norm,
                    r_norm,
                    op,
                    expected_ty,
                )
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
                    if self.ctx.type_registry.is_simd(l_norm) {
                        return self.simd_compare_type(l_norm);
                    }
                    return TypeId::BOOL;
                }

                self.require_builtin_binary_trait(
                    binary_expr_id,
                    lhs,
                    rhs,
                    l_norm,
                    r_norm,
                    op,
                    expected_ty,
                )
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs, TypeId::BOOL, l_norm);
                self.check_coercion(rhs, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            BitwiseAnd | BitwiseOr | BitwiseXor | ShiftLeft | ShiftRight => {
                if self.has_builtin_binary_fast_path(op, l_norm, r_norm) {
                    let l_norm = self.constrain_integer_type(lhs_ty);
                    let r_norm = self.constrain_integer_type(rhs_ty);
                    let bitwise_ok =
                        if let Some((elem, _)) = self.ctx.type_registry.simd_info(l_norm) {
                            self.ctx.type_registry.is_integer(elem) || elem == TypeId::BOOL
                        } else {
                            self.type_is_integer_like(lhs_ty)
                        };
                    if !bitwise_ok {
                        self.ctx
                            .struct_error(lhs.span, "bitwise operations require integer types")
                            .emit();
                    }
                    if !self.check_coercion(rhs, l_norm, r_norm) {
                        return TypeId::ERROR;
                    }
                    return l_norm;
                }

                self.require_builtin_binary_trait(
                    binary_expr_id,
                    lhs,
                    rhs,
                    l_norm,
                    r_norm,
                    op,
                    expected_ty,
                )
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

        let norm_op = self.resolve_tv(op_ty);

        match op {
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                let is_mut = op == UnaryOperator::MutAddressOf;

                if let ExprKind::IndexAccess { lhs, .. } = &operand.kind {
                    let lhs_ty = self.ctx.node_type_or_error(lhs.id);
                    if self.ctx.type_registry.is_simd(lhs_ty) {
                        self.ctx
                            .struct_error(span, "cannot take the address of a SIMD lane")
                            .with_hint("read or write SIMD lanes through `.[]` directly")
                            .emit();
                        return TypeId::ERROR;
                    }
                }

                // `expr..&` is allowed on mutable places and on value expressions that
                // explicitly materialize a stack temporary in the current scope.
                if is_mut && !self.can_take_mut_address_of(operand) {
                    let mut diag = self.ctx.struct_error(
                        span,
                        "cannot take mutable address `..&` of immutable memory",
                    );
                    diag = diag.with_hint(
                        "declare the variable with `let mut`, or use a value expression that can be materialized as a stack temporary",
                    );
                    diag.emit();
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
                                .with_hint("it can only extract metadata or state from fat pointers (e.g., slices `&[T]`, closures `&Fn`, or trait objects `&Trait`)")
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
                let op_ty_id = self.constrain_signed_numeric_type(op_ty);
                if let Some((elem, _)) = self.ctx.type_registry.simd_info(op_ty_id)
                    && (matches!(
                        elem,
                        TypeId::I8
                            | TypeId::I16
                            | TypeId::I32
                            | TypeId::I64
                            | TypeId::I128
                            | TypeId::ISIZE
                    ) || self.ctx.type_registry.is_float(elem))
                {
                    return op_ty;
                }
                if matches!(
                    op_ty_id,
                    TypeId::I8
                        | TypeId::I16
                        | TypeId::I32
                        | TypeId::I64
                        | TypeId::I128
                        | TypeId::ISIZE
                ) || self.ctx.type_registry.is_float(op_ty_id)
                    || self
                        .type_numeric_candidates(op_ty_id)
                        .is_some_and(|candidates| {
                            candidates
                                & (Self::NUMERIC_CAND_I8
                                    | Self::NUMERIC_CAND_I16
                                    | Self::NUMERIC_CAND_I32
                                    | Self::NUMERIC_CAND_I64
                                    | Self::NUMERIC_CAND_I128
                                    | Self::NUMERIC_CAND_ISIZE
                                    | Self::NUMERIC_CAND_ALL_FLOATS)
                                != 0
                        })
                {
                    return op_ty;
                }
                if self.ctx.type_registry.is_integer(op_ty_id)
                    && !matches!(
                        op_ty_id,
                        TypeId::I8
                            | TypeId::I16
                            | TypeId::I32
                            | TypeId::I64
                            | TypeId::I128
                            | TypeId::ISIZE
                    )
                {
                    self.ctx
                        .struct_error(
                            span,
                            "unary `-` cannot be applied to an unsigned integer type",
                        )
                        .with_hint("cast to a signed type first if a negative value is intended")
                        .emit();
                    return TypeId::ERROR;
                }
                if let Some((elem, _)) = self.ctx.type_registry.simd_info(op_ty_id)
                    && (self.ctx.type_registry.is_integer(elem)
                        || self.ctx.type_registry.is_float(elem))
                {
                    self.ctx
                        .struct_error(
                            span,
                            "unary `-` cannot be applied to a SIMD value with unsigned integer lanes",
                        )
                        .with_hint("cast the lanes to a signed type first if negative values are intended")
                        .emit();
                    return TypeId::ERROR;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
            UnaryOperator::LogicalNot => {
                if norm_op == TypeId::BOOL || self.ctx.type_registry.is_simd_mask(norm_op) {
                    return op_ty;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
            UnaryOperator::BitwiseNot => {
                let op_ty_id = self.constrain_integer_type(op_ty);
                if let Some((elem, _)) = self.ctx.type_registry.simd_info(op_ty_id)
                    && self.ctx.type_registry.is_integer(elem)
                {
                    return op_ty;
                }
                if self.ctx.type_registry.is_integer(op_ty_id) {
                    return op_ty;
                }
                if self
                    .type_numeric_candidates(op_ty_id)
                    .is_some_and(Self::numeric_candidates_have_integers)
                {
                    return op_ty;
                }
                self.require_builtin_unary_trait(operand, op_ty, op, expected_ty)
            }
        }
    }

    pub fn check_assign(&mut self, lhs: &Expr, op: AssignmentOperator, rhs: &Expr) -> TypeId {
        if matches!(lhs.kind, ExprKind::Infer) {
            if op != AssignmentOperator::Assign {
                self.ctx
                    .struct_error(lhs.span, "discard assignment only supports `=`")
                    .with_hint("use `_ = ...;` to explicitly discard a value")
                    .emit();
                let _ = self.check_expr(rhs, None);
                return TypeId::ERROR;
            }
            let _ = self.check_expr(rhs, None);
            return TypeId::VOID;
        }

        let lhs_ty = self.check_expr(lhs, None);
        if self.assignment_may_store_long_lived_pointer(lhs, op) {
            self.reject_temporary_address_escape(rhs, "static storage");
        }

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
                    "if this is a pointer dereference, ensure it is a mutable pointer (`&mut T`)",
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
