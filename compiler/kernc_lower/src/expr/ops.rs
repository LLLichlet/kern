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

            if self.ctx.type_registry.is_void(l_norm) {
                if op == ast::BinaryOperator::Equal {
                    return MastExprKind::Bool(true);
                } else if op == ast::BinaryOperator::NotEqual {
                    return MastExprKind::Bool(false);
                }
                // TODO: unreachable() + 报错 ICE
            }

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
            ast::UnaryOperator::MetaOf => {
                let op_norm = self.ctx.type_registry.normalize(op_mast.ty);
                let op_kind = self.ctx.type_registry.get(op_norm).clone();

                match op_kind {
                    // 1. 切片类型 (Slice)：胖指针，元数据是长度 (len)
                    TypeKind::Slice { .. } => {
                        return MastExprKind::ExtractFatPtrMeta(Box::new(op_mast));
                    }

                    // 2. 定长数组 (Array) 或 推导数组 (ArrayInfer)：
                    // 它们在内存中不是胖指针，而是一个连续的内存块，长度在编译期已知。
                    // 所以这里的 # 操作符直接被编译器折叠为一个整数常量。
                    TypeKind::Array { len, .. } => {
                        return MastExprKind::Integer(len as u128);
                    }
                    TypeKind::ArrayInfer { .. } => {
                        // 如果到了 Lowering 阶段 ArrayInfer 还没有被定长（通常在 constexpr 阶段就被处理了），
                        // 这里作为兜底，发出一个 ICE 错误。
                        self.ctx.emit_ice(operand.span, "Kern ICE (Lowering): Array length still inferred during MetaOf lowering.");
                        return MastExprKind::Trap;
                    }

                    // 3. 闭包和 Trait 胖指针：提取底层的匿名状态指针 (Data)
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                        let elem_norm = self.ctx.type_registry.normalize(elem);
                        let inner_kind = self.ctx.type_registry.get(elem_norm);

                        if matches!(
                            inner_kind,
                            TypeKind::ClosureInterface { .. } | TypeKind::TraitObject(..)
                        ) {
                            // 闭包和 Trait 胖指针调用 `#` 是为了拿回堆上的内存地址以便 free，所以提取 Data
                            return MastExprKind::ExtractFatPtrData(Box::new(op_mast));
                        }
                    }

                    _ => {}
                }

                MastExprKind::Unary {
                    op,
                    operand: Box::new(op_mast),
                }
            }

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
