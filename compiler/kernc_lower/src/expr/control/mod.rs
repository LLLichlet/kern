use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::def::Def;
use kernc_sema::ty::{BuiltinAnonymousEnumKind, GenericArg, TypeId, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

mod closure;
mod flow;
mod match_adt;
mod pattern;

#[derive(Clone)]
enum MatchAdtInfo {
    Named {
        mono_id: MonoId,
        gen_args: Vec<GenericArg>,
        def: kernc_sema::def::EnumDef,
        is_pure: bool,
        tag_ty: TypeId,
    },
    Anonymous {
        mono_id: MonoId,
        def: kernc_sema::ty::AnonymousEnum,
        is_pure: bool,
        tag_ty: TypeId,
    },
}

pub(crate) struct ClosureLowerSpec<'a> {
    pub node_id: NodeId,
    pub captures: &'a [ast::CapturePattern],
    pub params: &'a [ast::FuncParam],
    pub body: &'a Expr,
    pub concrete_ty: TypeId,
    pub subst_map: &'a HashMap<SymbolId, GenericArg>,
    pub exp_ty: TypeId,
}

struct PatternBindingPlan {
    name: SymbolId,
    ty: TypeId,
    is_mut: bool,
    init: MastExpr,
}

type MatchVariantPayloadInfo = (usize, TypeId, MonoId);

struct MatchLowerContext<'a> {
    arms: &'a [ast::MatchArm],
    target_var_expr: &'a MastExpr,
    target_ty: TypeId,
    subst_map: &'a HashMap<SymbolId, GenericArg>,
    exp_ty: TypeId,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_block_stmt(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
        lowered_stmts: &mut Vec<MastStmt>,
    ) {
        if let ExprKind::Defer { expr: def_expr } = &expr.kind {
            let lowered = self.measure_phase("      lower_stmt_defer", |this| {
                this.lower_expr(def_expr, subst_map, None)
            });
            self.push_defer_in_current_scope(expr.span, lowered);
        } else if let ExprKind::Let {
            pattern,
            init,
            else_clause,
        } = &expr.kind
        {
            let mut stmts = self.measure_phase("      lower_stmt_let", |this| {
                this.lower_let_stmts(expr, pattern, init, else_clause.as_ref(), subst_map)
            });
            lowered_stmts.append(&mut stmts);
        } else if let Some(stmt) = self.measure_phase("      lower_stmt_expr", |this| {
            this.lower_optional_stmt_expr(expr, subst_map)
        }) {
            lowered_stmts.push(stmt);
        }
    }

    fn lower_optional_stmt_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> Option<MastStmt> {
        if self.measure_phase("        lower_stmt_expr_elide", |this| {
            matches!(expr.kind, ExprKind::Assign { .. }) && this.is_pure_dead_assignment(expr.id)
        }) {
            return None;
        }

        let lowered = self.measure_phase("        lower_stmt_expr_lower", |this| {
            this.lower_expr(expr, subst_map, None)
        });
        if self.measure_phase("        lower_stmt_expr_drop_static", |_| {
            matches!(expr.kind, ExprKind::Static { .. })
        }) {
            return None;
        }

        self.measure_phase("        lower_stmt_expr_wrap", |_| {
            Some(MastStmt::Expr(lowered))
        })
    }

    fn push_defer_in_current_scope(&mut self, span: Span, deferred: MastExpr) {
        if let Some(scope) = self.defer_stack.last_mut() {
            scope.push(deferred);
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): attempted to register `defer` without an active block scope.",
            );
        }
    }

    fn bind_local_type(
        &mut self,
        span: Span,
        name: SymbolId,
        ty: TypeId,
        is_mut: bool,
        context: &str,
    ) -> bool {
        self.track_pure_enum_repr_in_type(ty);
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, is_mut));
            if let Some(forward_scope) = self.local_forwardings.last_mut() {
                // A concrete local binding must shadow any forwarded alias from an outer scope.
                forward_scope.insert(name, name);
            }
            true
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): missing local type scope while binding `{}` in {}.",
                    self.ctx.resolve(name),
                    context
                ),
            );
            false
        }
    }

    fn pop_defer_scope(&mut self, span: Span) -> Vec<MastExpr> {
        match self.defer_stack.pop() {
            Some(scope) => scope,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Kern ICE (Lowering): attempted to exit a block with an empty defer stack.",
                );
                Vec::new()
            }
        }
    }

    fn bool_expr(&self, span: Span, value: bool) -> MastExpr {
        MastExpr::new(TypeId::BOOL, MastExprKind::Bool(value), span)
    }

    fn current_return_type(&mut self, span: Span) -> Option<TypeId> {
        if let Some(ty) = self.current_return_types.last().copied() {
            Some(ty)
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): missing active return type while lowering propagation.",
            );
            None
        }
    }

    fn lower_return_lowered_value(&mut self, value: Option<MastExpr>, span: Span) -> MastExprKind {
        let mut defer_stmts = self.measure_phase("            lower_return_defers", |this| {
            let capacity = this.defer_stack.iter().map(Vec::len).sum();
            let mut defer_stmts = Vec::with_capacity(capacity);

            for stack in this.defer_stack.iter().rev() {
                for d in stack.iter().rev() {
                    defer_stmts.push(MastStmt::Expr(d.clone()));
                }
            }

            defer_stmts
        });

        if defer_stmts.is_empty() {
            MastExprKind::Return(value.map(Box::new))
        } else {
            match value {
                Some(ret_expr) if ret_expr.ty != TypeId::VOID && ret_expr.ty != TypeId::ERROR => {
                    let temp_name = self.ctx.intern(&format!("__ret_tmp_{}", self.next_mono_id));
                    self.next_mono_id += 1;
                    let temp_ty = ret_expr.ty;

                    defer_stmts.insert(
                        0,
                        MastStmt::Let {
                            name: temp_name,
                            ty: temp_ty,
                            is_mut: false,
                            init: ret_expr,
                        },
                    );
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(Some(Box::new(MastExpr::new(
                            temp_ty,
                            MastExprKind::Var(temp_name),
                            span,
                        )))),
                        span,
                    )));
                }
                Some(ret_expr) => {
                    defer_stmts.insert(0, MastStmt::Expr(ret_expr));
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(None),
                        span,
                    )));
                }
                None => {
                    defer_stmts.push(MastStmt::Expr(MastExpr::new(
                        TypeId::NEVER,
                        MastExprKind::Return(None),
                        span,
                    )));
                }
            }
            MastExprKind::Block(MastBlock {
                stmts: defer_stmts,
                result: None,
                defers: vec![],
            })
        }
    }
}
