//! Operator lowering.
//!
//! Builtin operations become direct MAST unary/binary/assignment nodes, while
//! trait-backed operations lower to regular or method calls after semantic
//! resolution has selected the target function and concrete receiver types.

use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::def::Def;
use kernc_sema::ty::{GenericArg, TypeId, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

pub(crate) struct BinaryLowerInput<'a> {
    pub(crate) binary_expr_id: NodeId,
    pub(crate) lhs: &'a Expr,
    pub(crate) op: ast::BinaryOperator,
    pub(crate) rhs: &'a Expr,
    pub(crate) subst_map: &'a HashMap<SymbolId, GenericArg>,
    pub(crate) result_ty: TypeId,
    pub(crate) span: Span,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn has_builtin_simd_binary_fast_path(
        &mut self,
        op: ast::BinaryOperator,
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
            ast::BinaryOperator::Add
            | ast::BinaryOperator::Subtract
            | ast::BinaryOperator::Multiply
            | ast::BinaryOperator::Divide
            | ast::BinaryOperator::Modulo => elem_is_int || elem_is_float,
            ast::BinaryOperator::Equal | ast::BinaryOperator::NotEqual => {
                elem_is_int || elem_is_float || elem_is_bool
            }
            ast::BinaryOperator::LessThan
            | ast::BinaryOperator::GreaterThan
            | ast::BinaryOperator::LessOrEqual
            | ast::BinaryOperator::GreaterOrEqual => elem_is_int || elem_is_float,
            ast::BinaryOperator::BitwiseAnd
            | ast::BinaryOperator::BitwiseOr
            | ast::BinaryOperator::BitwiseXor => elem_is_int || elem_is_bool,
            ast::BinaryOperator::ShiftLeft | ast::BinaryOperator::ShiftRight => elem_is_int,
            ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr => false,
        }
    }

    pub(super) fn has_builtin_binary_fast_path(
        &mut self,
        op: ast::BinaryOperator,
        lhs_ty: TypeId,
        rhs_ty: TypeId,
    ) -> bool {
        let l_norm = self.ctx.type_registry.normalize(lhs_ty);
        let r_norm = self.ctx.type_registry.normalize(rhs_ty);
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
            ast::BinaryOperator::Add | ast::BinaryOperator::Subtract => {
                if is_l_ptr || is_r_ptr {
                    return true;
                }

                (self.ctx.type_registry.is_integer(l_norm)
                    && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
            }
            ast::BinaryOperator::Multiply
            | ast::BinaryOperator::Divide
            | ast::BinaryOperator::Modulo => {
                (self.ctx.type_registry.is_integer(l_norm)
                    && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
            }
            ast::BinaryOperator::Equal | ast::BinaryOperator::NotEqual => {
                if self.ctx.type_registry.is_void(l_norm) || self.ctx.type_registry.is_void(r_norm)
                {
                    return true;
                }
                if l_norm == r_norm && self.is_pure_enum_type(l_norm) {
                    return true;
                }
                (is_l_ptr && is_r_ptr)
                    || (self.ctx.type_registry.is_integer(l_norm)
                        && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
                    || (l_norm == TypeId::BOOL && r_norm == TypeId::BOOL)
            }
            ast::BinaryOperator::LessThan
            | ast::BinaryOperator::GreaterThan
            | ast::BinaryOperator::LessOrEqual
            | ast::BinaryOperator::GreaterOrEqual => {
                (is_l_ptr && is_r_ptr)
                    || (self.ctx.type_registry.is_integer(l_norm)
                        && self.ctx.type_registry.is_integer(r_norm))
                    || (self.ctx.type_registry.is_float(l_norm)
                        && self.ctx.type_registry.is_float(r_norm))
                    || (l_norm == TypeId::BOOL && r_norm == TypeId::BOOL)
            }
            ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr => true,
            ast::BinaryOperator::BitwiseAnd
            | ast::BinaryOperator::BitwiseOr
            | ast::BinaryOperator::BitwiseXor
            | ast::BinaryOperator::ShiftLeft
            | ast::BinaryOperator::ShiftRight => {
                self.ctx.type_registry.is_integer(l_norm) && l_norm == r_norm
            }
        }
    }

    fn is_pure_enum_type(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
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

    fn has_builtin_unary_fast_path(&mut self, op: ast::UnaryOperator, operand_ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(operand_ty);
        match op {
            ast::UnaryOperator::Negate => {
                if let Some((elem, _)) = self.ctx.type_registry.simd_info(norm) {
                    return self.ctx.type_registry.is_integer(elem)
                        || self.ctx.type_registry.is_float(elem);
                }
                self.ctx.type_registry.is_integer(norm) || self.ctx.type_registry.is_float(norm)
            }
            ast::UnaryOperator::LogicalNot => {
                norm == TypeId::BOOL || self.ctx.type_registry.is_simd_mask(norm)
            }
            ast::UnaryOperator::BitwiseNot => {
                if let Some((elem, _)) = self.ctx.type_registry.simd_info(norm) {
                    return self.ctx.type_registry.is_integer(elem);
                }
                self.ctx.type_registry.is_integer(norm)
            }
            ast::UnaryOperator::AddressOf
            | ast::UnaryOperator::MutAddressOf
            | ast::UnaryOperator::PointerDeRef => true,
        }
    }

    fn binary_operator_trait_name(&self, op: ast::BinaryOperator) -> Option<&'static str> {
        match op {
            ast::BinaryOperator::Add => Some("Add"),
            ast::BinaryOperator::Subtract => Some("Sub"),
            ast::BinaryOperator::Multiply => Some("Mul"),
            ast::BinaryOperator::Divide => Some("Div"),
            ast::BinaryOperator::Modulo => Some("Rem"),
            ast::BinaryOperator::Equal | ast::BinaryOperator::NotEqual => Some("Eq"),
            ast::BinaryOperator::LessThan => Some("Lt"),
            ast::BinaryOperator::LessOrEqual => Some("Le"),
            ast::BinaryOperator::GreaterThan => Some("Gt"),
            ast::BinaryOperator::GreaterOrEqual => Some("Ge"),
            ast::BinaryOperator::BitwiseAnd => Some("BitAnd"),
            ast::BinaryOperator::BitwiseOr => Some("BitOr"),
            ast::BinaryOperator::BitwiseXor => Some("BitXor"),
            ast::BinaryOperator::ShiftLeft => Some("Shl"),
            ast::BinaryOperator::ShiftRight => Some("Shr"),
            // `and` / `or` lower as builtin short-circuit control flow, not trait calls.
            ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr => None,
        }
    }

    fn binary_operator_method_name(&mut self, op: ast::BinaryOperator) -> Option<SymbolId> {
        Some(match op {
            ast::BinaryOperator::Add => self.ctx.intern("add"),
            ast::BinaryOperator::Subtract => self.ctx.intern("sub"),
            ast::BinaryOperator::Multiply => self.ctx.intern("mul"),
            ast::BinaryOperator::Divide => self.ctx.intern("div"),
            ast::BinaryOperator::Modulo => self.ctx.intern("rem"),
            ast::BinaryOperator::Equal | ast::BinaryOperator::NotEqual => self.ctx.intern("eq"),
            ast::BinaryOperator::LessThan => self.ctx.intern("lt"),
            ast::BinaryOperator::LessOrEqual => self.ctx.intern("le"),
            ast::BinaryOperator::GreaterThan => self.ctx.intern("gt"),
            ast::BinaryOperator::GreaterOrEqual => self.ctx.intern("ge"),
            ast::BinaryOperator::BitwiseAnd => self.ctx.intern("bit_and"),
            ast::BinaryOperator::BitwiseOr => self.ctx.intern("bit_or"),
            ast::BinaryOperator::BitwiseXor => self.ctx.intern("bit_xor"),
            ast::BinaryOperator::ShiftLeft => self.ctx.intern("shl"),
            ast::BinaryOperator::ShiftRight => self.ctx.intern("shr"),
            ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr => return None,
        })
    }

    fn unary_operator_trait_name(&self, op: ast::UnaryOperator) -> Option<&'static str> {
        match op {
            ast::UnaryOperator::Negate => Some("Neg"),
            ast::UnaryOperator::LogicalNot => Some("Not"),
            ast::UnaryOperator::BitwiseNot => Some("BitNot"),
            // Keep memory/metadata operators owned by the language.
            ast::UnaryOperator::AddressOf
            | ast::UnaryOperator::MutAddressOf
            | ast::UnaryOperator::PointerDeRef => None,
        }
    }

    fn unary_operator_method_name(&mut self, op: ast::UnaryOperator) -> Option<SymbolId> {
        Some(match op {
            ast::UnaryOperator::Negate => self.ctx.intern("neg"),
            ast::UnaryOperator::LogicalNot => self.ctx.intern("not"),
            ast::UnaryOperator::BitwiseNot => self.ctx.intern("bit_not"),
            ast::UnaryOperator::AddressOf
            | ast::UnaryOperator::MutAddressOf
            | ast::UnaryOperator::PointerDeRef => return None,
        })
    }

    pub(super) fn lower_custom_binary_operator(
        &mut self,
        lhs: MastExpr,
        rhs: MastExpr,
        rhs_trait_arg_ty: TypeId,
        op: ast::BinaryOperator,
        result_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let Some(trait_name) = self.binary_operator_trait_name(op) else {
            self.ctx
                .emit_ice(span, "missing builtin trait for binary operator lowering");
            return MastExprKind::Trap;
        };
        let Some(method_name) = self.binary_operator_method_name(op) else {
            self.ctx.emit_ice(
                span,
                "missing builtin method name for binary operator lowering",
            );
            return MastExprKind::Trap;
        };
        let owner_trait_ty = match op {
            ast::BinaryOperator::Equal
            | ast::BinaryOperator::NotEqual
            | ast::BinaryOperator::LessThan
            | ast::BinaryOperator::LessOrEqual
            | ast::BinaryOperator::GreaterThan
            | ast::BinaryOperator::GreaterOrEqual => {
                // Preserve the trait argument shape chosen by sema. The lowered RHS expression may
                // already be coerced to an expected type, but static devirtualization still has to
                // search for the impl that satisfied the original operator proof.
                self.ctx
                    .builtin_trait_ty(trait_name, vec![rhs_trait_arg_ty])
            }
            _ => self.ctx.builtin_trait_ty_with_assoc(
                trait_name,
                vec![rhs_trait_arg_ty],
                vec![("Out", result_ty)],
            ),
        };
        let Some(owner_trait_ty) = owner_trait_ty else {
            self.ctx.emit_ice(
                span,
                format!("missing builtin trait `{}` during lowering", trait_name),
            );
            return MastExprKind::Trap;
        };
        let callee_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: vec![lhs.ty, rhs.ty],
            ret: result_ty,
            is_variadic: false,
        });
        let expected_self_ty = lhs.ty;
        let call = self.lower_resolved_trait_method_call(
            lhs,
            vec![rhs],
            owner_trait_ty,
            super::call::MethodCallSite {
                field: method_name,
                norm_callee: callee_ty,
                expected_self_ty: Some(expected_self_ty),
                default_ret_ty: result_ty,
                span,
            },
        );

        if op == ast::BinaryOperator::NotEqual {
            return MastExprKind::Unary {
                op: ast::UnaryOperator::LogicalNot,
                operand: Box::new(call),
            };
        }

        call.kind
    }

    fn lower_custom_unary_operator(
        &mut self,
        operand: MastExpr,
        op: ast::UnaryOperator,
        result_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let Some(trait_name) = self.unary_operator_trait_name(op) else {
            self.ctx
                .emit_ice(span, "missing builtin trait for unary operator lowering");
            return MastExprKind::Trap;
        };
        let Some(method_name) = self.unary_operator_method_name(op) else {
            self.ctx.emit_ice(
                span,
                "missing builtin method name for unary operator lowering",
            );
            return MastExprKind::Trap;
        };
        let Some(owner_trait_ty) =
            self.ctx
                .builtin_trait_ty_with_assoc(trait_name, vec![], vec![("Out", result_ty)])
        else {
            self.ctx.emit_ice(
                span,
                format!("missing builtin trait `{}` during lowering", trait_name),
            );
            return MastExprKind::Trap;
        };
        let callee_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: vec![operand.ty],
            ret: result_ty,
            is_variadic: false,
        });
        let expected_self_ty = operand.ty;
        self.lower_resolved_trait_method_call(
            operand,
            vec![],
            owner_trait_ty,
            super::call::MethodCallSite {
                field: method_name,
                norm_callee: callee_ty,
                expected_self_ty: Some(expected_self_ty),
                default_ret_ty: result_ty,
                span,
            },
        )
        .kind
    }

    pub(crate) fn lower_binary(&mut self, input: BinaryLowerInput<'_>) -> MastExprKind {
        let BinaryLowerInput {
            binary_expr_id,
            lhs,
            op,
            rhs,
            subst_map,
            result_ty,
            span,
        } = input;
        if op == ast::BinaryOperator::LogicalAnd {
            let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
            let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
            MastExprKind::If {
                cond: Box::new(l),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(r)),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(MastExpr::new(
                        TypeId::BOOL,
                        MastExprKind::Bool(false),
                        span,
                    ))),
                    defers: vec![],
                }),
            }
        } else if op == ast::BinaryOperator::LogicalOr {
            let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
            let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
            MastExprKind::If {
                cond: Box::new(l),
                then_branch: MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(MastExpr::new(
                        TypeId::BOOL,
                        MastExprKind::Bool(true),
                        span,
                    ))),
                    defers: vec![],
                },
                else_branch: Some(MastBlock {
                    stmts: vec![],
                    result: Some(Box::new(r)),
                    defers: vec![],
                }),
            }
        } else {
            let l_expected = self
                .ctx
                .binary_operator_lhs_trait_self_ty(binary_expr_id)
                .map(|ty| self.substitute_type_with_map(ty, subst_map));
            let l = self.lower_expr(lhs, subst_map, l_expected);

            let l_norm = self.ctx.type_registry.normalize(l.ty);

            if self.ctx.type_registry.is_void(l_norm) {
                if op == ast::BinaryOperator::Equal {
                    return MastExprKind::Bool(true);
                } else if op == ast::BinaryOperator::NotEqual {
                    return MastExprKind::Bool(false);
                }
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): void operand reached non-equality binary operator `{:?}`.",
                        op
                    ),
                );
                return MastExprKind::Trap;
            }

            let is_l_ptr = matches!(
                self.ctx.type_registry.get(l_norm),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            // Read the real right-hand type cached by Sema.
            let r_sema_ty = self
                .ctx
                .binary_operator_rhs_trait_arg_ty(binary_expr_id)
                .or_else(|| self.ctx.node_type(rhs.id))
                .unwrap_or(TypeId::ERROR);
            let r_concrete_ty = self.substitute_type_with_map(r_sema_ty, subst_map);
            let r_norm = self.ctx.type_registry.normalize(r_concrete_ty);
            let is_r_ptr = matches!(
                self.ctx.type_registry.get(r_norm),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            // Reuse sema's finalized RHS type. For overloaded operators this preserves the trait
            // argument shape that actually proved the operation. Builtin pointer arithmetic still
            // needs to keep its mixed pointer/integer RHS uncoerced.
            let expected_r = if (is_l_ptr || is_r_ptr)
                && matches!(op, ast::BinaryOperator::Add | ast::BinaryOperator::Subtract)
            {
                None
            } else {
                Some(r_concrete_ty)
            };

            let r = self.lower_expr(rhs, subst_map, expected_r);

            if self.has_builtin_binary_fast_path(op, l.ty, r.ty) {
                return self.measure_phase("            lower_ops_binary_builtin", |_this| {
                    MastExprKind::Binary {
                        op,
                        lhs: Box::new(l),
                        rhs: Box::new(r),
                    }
                });
            }

            self.measure_phase("            lower_ops_binary_custom", |this| {
                this.lower_custom_binary_operator(l, r, r_concrete_ty, op, result_ty, span)
            })
        }
    }

    pub(crate) fn lower_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        result_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let op_mast = self.lower_expr(operand, subst_map, None);

        match op {
            ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf => self
                .measure_phase("            lower_ops_unary_builtin", |_this| {
                    MastExprKind::AddressOf(Box::new(op_mast))
                }),
            ast::UnaryOperator::PointerDeRef => self
                .measure_phase("            lower_ops_unary_builtin", |_this| {
                    MastExprKind::Deref(Box::new(op_mast))
                }),
            _ if self.has_builtin_unary_fast_path(op, op_mast.ty) => {
                self.measure_phase("            lower_ops_unary_builtin", |_this| {
                    MastExprKind::Unary {
                        op,
                        operand: Box::new(op_mast),
                    }
                })
            }
            _ => self.measure_phase("            lower_ops_unary_custom", |this| {
                this.lower_custom_unary_operator(op_mast, op, result_ty, span)
            }),
        }
    }

    pub(crate) fn lower_assign(
        &mut self,
        lhs: &Expr,
        op: ast::AssignmentOperator,
        rhs: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> MastExprKind {
        if matches!(lhs.kind, ExprKind::Infer) {
            return MastExprKind::Discard(Box::new(self.lower_expr(rhs, subst_map, None)));
        }

        let l = self.lower_expr(lhs, subst_map, None);
        let r = self.lower_expr(rhs, subst_map, Some(l.ty));
        self.measure_phase("            lower_ops_assign_build", |_this| {
            MastExprKind::Assign {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            }
        })
    }
}
