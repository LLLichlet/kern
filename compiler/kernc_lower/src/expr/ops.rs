use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr};
use kernc_mast::*;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_binary(
        &mut self,
        lhs: &Expr,
        op: ast::BinaryOperator,
        rhs: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
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
            let l = self.lower_expr(lhs, subst_map, None);

            let l_norm = self.ctx.type_registry.normalize(l.ty);
            let is_l_ptr = matches!(
                self.ctx.type_registry.get(l_norm),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            // 获取 Sema 阶段缓存的真实右侧类型
            let r_sema_ty = self
                .ctx
                .node_types
                .get(&rhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let r_norm = self.ctx.type_registry.normalize(r_sema_ty);
            let is_r_ptr = matches!(
                self.ctx.type_registry.get(r_norm),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            // 核心修改：如果是由于指针算术 (ptr + int, int + ptr, ptr - ptr) 导致的两侧类型不对等，
            // 就不强行用左侧的类型去约束右侧，直接放行 (None) 交给节点原类型去解析。
            let expected_r = if is_l_ptr || is_r_ptr {
                None
            } else {
                Some(l.ty)
            };

            let r = self.lower_expr(rhs, subst_map, expected_r);

            MastExprKind::Binary {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            }
        }
    }

    pub(crate) fn lower_unary(
        &mut self,
        op: ast::UnaryOperator,
        operand: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let op_mast = self.lower_expr(operand, subst_map, None);
        match op {
            ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf => {
                MastExprKind::AddressOf(Box::new(op_mast))
            }
            ast::UnaryOperator::PointerDeRef => MastExprKind::Deref(Box::new(op_mast)),
            _ => MastExprKind::Unary {
                op,
                operand: Box::new(op_mast),
            },
        }
    }

    pub(crate) fn lower_assign(
        &mut self,
        lhs: &Expr,
        op: ast::AssignmentOperator,
        rhs: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let l = self.lower_expr(lhs, subst_map, None);
        let r = self.lower_expr(rhs, subst_map, Some(l.ty));
        MastExprKind::Assign {
            op,
            lhs: Box::new(l),
            rhs: Box::new(r),
        }
    }
}
