use crate::context::SemaContext;
use crate::passes::TypeResolver;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};

mod access;
mod call;
mod cast;
mod coercion;
mod control;
mod literal;
mod ops;

pub(crate) struct ExprChecker<'a, 'ctx> {
    pub(crate) ctx: &'a mut SemaContext<'ctx>,
    pub(crate) current_return_type: Option<TypeId>,
    pub(crate) has_returned: bool,
    pub(crate) type_vars: Vec<Option<TypeId>>,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn new(ctx: &'a mut SemaContext<'ctx>, current_return_type: Option<TypeId>) -> Self {
        Self {
            ctx,
            current_return_type,
            has_returned: false,
            type_vars: Vec::new(),
        }
    }

    /// 核心入口：检查表达式类型
    pub(crate) fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(_) => self.check_integer(expr, expected_ty),
            ExprKind::Float(_) => self.check_float(expr, expected_ty),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32,
            ExprKind::ByteChar(_) => TypeId::U8,
            ExprKind::String(_) => self.ctx.type_registry.intern(TypeKind::Slice {
                is_mut: false,
                elem: TypeId::U8,
            }),

            // === 2. 标识符与变量 ===
            ExprKind::Identifier(name) => self.check_identifier(*name, expr.span),
            ExprKind::SelfValue => self.check_self_value(expr.span),

            // === 3. 声明与绑定 ===
            ExprKind::Let { pattern, init, .. } => {
                self.check_let_or_static(expr.id, pattern, init, expected_ty, false, expr.span)
            }
            ExprKind::Static { pattern, init, .. } => {
                self.check_let_or_static(expr.id, pattern, init, expected_ty, true, expr.span)
            }

            // === 4. 运算与赋值 ===
            ExprKind::Binary { lhs, op, rhs } => self.check_binary(lhs, *op, rhs, expected_ty),
            ExprKind::Unary { op, operand } => {
                self.check_unary(*op, operand, expr.span, expected_ty)
            }
            ExprKind::Assign { lhs, rhs, .. } => self.check_assign(lhs, rhs),

            // === 5. 转换 ===
            ExprKind::As { lhs, target } => {
                let actual_target_ty = self.evaluate_dynamic_typeof(target);
                self.check_as_expr(lhs, actual_target_ty)
            }

            // === 6. 内存访问 ===
            ExprKind::IndexAccess { lhs, index, is_mut } => {
                self.check_index_access(lhs, index, *is_mut, expr.span)
            }
            ExprKind::FieldAccess { lhs, field } => self.check_field_access(lhs, *field, expr.span),
            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                is_mut,
            } => self.check_slice_op(
                lhs,
                start.as_deref(),
                end.as_deref(),
                *is_inclusive,
                *is_mut,
                expr.span,
            ),

            // === 7. 函数/宏调用 ===
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),
            ExprKind::GenericInstantiation { target, types } => {
                for ty_node in types {
                    self.evaluate_dynamic_typeof(ty_node);
                }
                self.check_generic_instantiation(target, types, expr.span)
            }
            ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => self.check_closure(expr.id, captures, params, ret_type, body, expr.span),

            // === 8. 复杂字面量 ===
            ExprKind::DataInit { type_node, literal } => {
                if let Some(t_node) = type_node {
                    self.evaluate_dynamic_typeof(t_node);
                }
                self.check_data_init_expr(type_node.as_deref(), literal, expected_ty, expr.span)
            }
            ExprKind::EnumLiteral(variant_name) => {
                self.check_enum_literal(*variant_name, expected_ty, expr.span)
            }
            ExprKind::Undef => self.check_undef(expected_ty, expr.span),

            // === 9. 控制流 ===
            ExprKind::Block { stmts, result } => {
                self.check_block(stmts, result.as_deref(), expected_ty)
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty),
            ExprKind::Match { target, arms } => {
                self.check_match_expr(target, arms, expected_ty, expr.span)
            }
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.check_for(init.as_deref(), cond.as_deref(), post.as_deref(), body),
            ExprKind::Defer { expr: defer_expr } => self.check_defer(defer_expr),
            ExprKind::Break | ExprKind::Continue => TypeId::NEVER,
            ExprKind::Return(val) => {
                self.check_return(val.as_deref(), expr.span);
                TypeId::NEVER
            }

            ExprKind::Infer => {
                self.ctx.struct_error(expr.span, "type placeholder `_` cannot be evaluated as an expression")
                    .with_hint("in Kern, `_` is only used as a discard binding (`let _ =`) or in array length inference (`[_]T`)")
                    .emit();
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(expr.id, ty);
        ty
    }

    /// 递归深度扫描 AST 类型节点，推导所有 @typeOf，并自下而上重组真实类型
    pub(crate) fn evaluate_dynamic_typeof(&mut self, ty_node: &kernc_ast::TypeNode) -> TypeId {
        let ty_id = match &ty_node.kind {
            ast::TypeKind::TypeOf(inner_expr) => self.check_expr(inner_expr, None),
            ast::TypeKind::Pointer { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::ArrayInfer { is_mut, elem } => {
                let base = self.evaluate_dynamic_typeof(elem);
                self.ctx.type_registry.intern(TypeKind::ArrayInfer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Array { is_mut, elem, len } => {
                let base = self.evaluate_dynamic_typeof(elem);
                let Ok(length) = crate::checker::ConstEvaluator::new(self.ctx).eval_usize(len)
                else {
                    return TypeId::ERROR;
                };
                if length > u32::MAX as u64 {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                length,
                                u32::MAX
                            ),
                        )
                        .with_hint(
                            "LLVM array types are emitted with a 32-bit element count; split the object or allocate dynamically instead",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                self.ctx.type_registry.intern(TypeKind::Array {
                    is_mut: *is_mut,
                    elem: base,
                    len: length,
                })
            }
            ast::TypeKind::ClosureInterface { params, ret } => {
                let mut param_tys = Vec::new();
                for p in params {
                    param_tys.push(self.evaluate_dynamic_typeof(p));
                }
                let ret_ty = if let Some(r) = ret {
                    self.evaluate_dynamic_typeof(r)
                } else {
                    TypeId::VOID
                };
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: param_tys,
                    ret: ret_ty,
                })
            }
            // 普通的静态类型（如 Path, SelfType 等）内部不可能包含 @typeOf
            // 直接借用 TypeResolver 的能力即可
            _ => {
                let mut resolver = TypeResolver::new(self.ctx);
                let scope = resolver.current_scope_id().unwrap();
                resolver.resolve_type(ty_node, scope)
            }
        };

        // 强行覆盖缓存
        self.ctx.node_types.insert(ty_node.id, ty_id);
        ty_id
    }
}
