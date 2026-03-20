use super::Lowerer;
use kernc_ast::{Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::checker::Substituter;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::SymbolId;
use std::collections::HashMap;

mod access;
mod call;
mod cast;
mod control;
mod literal;
mod ops;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_expr(
        &mut self,
        expr: &Expr,
        subst_map: &HashMap<SymbolId, TypeId>,
        expected_ty: Option<TypeId>,
    ) -> MastExpr {
        let raw_ty = self.resolve_expr_type(expr);

        let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
        let concrete_ty = subst.substitute(raw_ty);
        let exp_ty = expected_ty.unwrap_or(concrete_ty);

        if exp_ty == TypeId::ERROR {
            self.ctx
                .emit_ice(expr.span, "Lowering encountered an unresolved ERROR type.");
            // 立即打印并终止降级，防止带着污染数据进入 LLVM 导致玄学 Panic
            self.ctx.sess.print_diagnostics();
            std::process::exit(1);
        }

        let mast_kind = match &expr.kind {
            ExprKind::Integer(val) => MastExprKind::Integer(*val),
            ExprKind::Float(val) => MastExprKind::Float(*val),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::Char(c) => MastExprKind::Integer(*c as u32 as u128),
            ExprKind::ByteChar(b) => MastExprKind::Integer(*b as u128),
            ExprKind::String(s) => self.lower_string_literal(s, expr.span),
            ExprKind::Identifier(name) => {
                let expr_ty = self
                    .ctx
                    .node_types
                    .get(&expr.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_ty = self.ctx.type_registry.normalize(expr_ty);

                if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(norm_ty).clone()
                {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    MastExprKind::FuncRef(mono_id)
                } else {
                    let kind = self.lower_identifier(*name);

                    // 检测是否属于非法闭包捕获
                    if let MastExprKind::Var(v) = kind {
                        let mut found = false;
                        for scope in self.local_types.iter().rev() {
                            if scope.contains_key(&v) {
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            let var_str = self.ctx.resolve(v).to_string();
                            self.ctx.struct_error(expr.span, "closures cannot capture environmental variables in Kern")
                                .with_hint(format!("variable `{}` belongs to an outer scope", var_str))
                                .with_hint("Kern anonymous functions compile directly to static C function pointers")
                                .emit();
                        }
                    }
                    kind
                }
            }

            ExprKind::Let { init, .. } => {
                return self.lower_expr(init, subst_map, Some(concrete_ty));
            }
            ExprKind::Static { pattern, init } => {
                self.lower_static_decl(pattern.name, init, subst_map, concrete_ty, pattern.is_mut)
            }

            ExprKind::Binary { lhs, op, rhs } => {
                self.lower_binary(lhs, *op, rhs, subst_map, expr.span)
            }
            ExprKind::Unary { op, operand } => self.lower_unary(*op, operand, subst_map),

            ExprKind::Call { callee, args } => self.lower_call(callee, args, subst_map, expr.span),
            ExprKind::FieldAccess { lhs, field } => {
                // 通过 AST 节点的推导类型直接判断是否是静态函数
                // 解决跨模块调用时，Scope 丢失导致的误判问题
                let expr_ty = self
                    .ctx
                    .node_types
                    .get(&expr.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_ty = self.ctx.type_registry.normalize(expr_ty);

                if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(norm_ty).clone()
                {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    return MastExpr::new(exp_ty, MastExprKind::FuncRef(mono_id), expr.span);
                }

                // 老老实实走普通的结构体/联合体字段访问
                self.lower_field_access(lhs, *field, subst_map, expr.span)
            }
            ExprKind::IndexAccess { lhs, index, .. } => {
                self.lower_index_access(lhs, index, subst_map)
            }

            ExprKind::DataInit { literal, .. } => {
                self.lower_data_init(literal, subst_map, concrete_ty, expr.span)
            }
            ExprKind::EnumLiteral(variant_name) => {
                self.lower_enum_literal(*variant_name, concrete_ty)
            }

            ExprKind::As { lhs, target } => {
                return self.lower_as_expr(lhs, target, concrete_ty, subst_map, expr.span);
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.lower_if(cond, then_branch, else_branch.as_deref(), subst_map, exp_ty),
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.lower_for(
                init.as_deref(),
                cond.as_deref(),
                post.as_deref(),
                body,
                subst_map,
                expr.span,
            ),
            ExprKind::Match { target, arms } => self.lower_match(target, arms, subst_map, exp_ty),
            ExprKind::Lambda {
                params,
                ret_type: _,
                body,
            } => self.lower_lambda_expr(params, body, concrete_ty, subst_map),
            ExprKind::Block { .. } => {
                MastExprKind::Block(self.lower_block_as_body(expr, subst_map, exp_ty))
            }

            ExprKind::Return(val) => self.lower_return(val.as_deref(), subst_map, expr.span),
            ExprKind::Assign { lhs, op, rhs } => self.lower_assign(lhs, *op, rhs, subst_map),
            ExprKind::GenericInstantiation { .. } => self.lower_generic_instantiation(concrete_ty),

            ExprKind::SelfValue => MastExprKind::Var(self.ctx.intern("self")),
            ExprKind::Break => self.lower_jump(MastExprKind::Break, expr.span),
            ExprKind::Continue => self.lower_jump(MastExprKind::Continue, expr.span),
            ExprKind::Undef => MastExprKind::Undef,

            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                ..
            } => {
                let mast_lhs = self.lower_expr(lhs, subst_map, None);
                let mast_start = start
                    .as_ref()
                    .map(|e| Box::new(self.lower_expr(e, subst_map, Some(TypeId::USIZE))));
                let mast_end = end
                    .as_ref()
                    .map(|e| Box::new(self.lower_expr(e, subst_map, Some(TypeId::USIZE))));

                MastExprKind::SliceOp {
                    lhs: Box::new(mast_lhs),
                    start: mast_start,
                    end: mast_end,
                    is_inclusive: *is_inclusive,
                }
            }
            _ => unreachable!("Unhandled ExprKind in lowering: {:?}", expr.kind),
        };

        self.apply_implicit_cast(mast_kind, concrete_ty, exp_ty, expr.span)
    }

    pub(crate) fn resolve_expr_type(&self, expr: &Expr) -> TypeId {
        let raw_ty = self
            .ctx
            .node_types
            .get(&expr.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        if raw_ty == TypeId::ERROR {
            if let ExprKind::Identifier(name) = &expr.kind {
                for scope in self.local_types.iter().rev() {
                    if let Some(&(local_ty, _)) = scope.get(name) {
                        return local_ty;
                    }
                }
            }
        }
        raw_ty
    }
}
