use crate::context::SemaContext;
use crate::passes::TypeResolver;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use std::time::Instant;

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

    /// Main entry point for expression type checking.
    pub(crate) fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            // === 1. Primitive literals ===
            ExprKind::Integer(_) => self.check_integer(expr, expected_ty),
            ExprKind::Float(_) => self.check_float(expr, expected_ty),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32,
            ExprKind::ByteChar(_) => TypeId::U8,
            ExprKind::String(_) => self.ctx.type_registry.intern(TypeKind::Slice {
                is_mut: false,
                elem: TypeId::U8,
            }),

            // === 2. Identifiers and variables ===
            ExprKind::Identifier(name) => {
                let started = Instant::now();
                let ty = self.check_identifier(*name, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.access += elapsed;
                self.ctx.expr_timing_stats.access_identifier += elapsed;
                ty
            }
            ExprKind::SelfValue => self.check_self_value(expr.span),

            // === 3. Declarations and bindings ===
            ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                let started = Instant::now();
                let ty = self.check_let(
                    expr.id,
                    pattern,
                    init,
                    else_pattern.as_ref(),
                    else_branch.as_deref(),
                    expected_ty,
                    expr.span,
                );
                self.ctx.expr_timing_stats.bindings += started.elapsed();
                ty
            }
            ExprKind::Static { pattern, init, .. } => {
                let started = Instant::now();
                let ty = self.check_static(expr.id, pattern, init, expected_ty, expr.span);
                self.ctx.expr_timing_stats.bindings += started.elapsed();
                ty
            }

            // === 4. Operators and assignment ===
            ExprKind::Binary { lhs, op, rhs } => {
                let started = Instant::now();
                let ty = self.check_binary(lhs, *op, rhs, expected_ty);
                self.ctx.expr_timing_stats.ops += started.elapsed();
                ty
            }
            ExprKind::Unary { op, operand } => {
                let started = Instant::now();
                let ty = self.check_unary(*op, operand, expr.span, expected_ty);
                self.ctx.expr_timing_stats.ops += started.elapsed();
                ty
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                let started = Instant::now();
                let ty = self.check_assign(lhs, rhs);
                self.ctx.expr_timing_stats.ops += started.elapsed();
                ty
            }

            // === 5. Casts and coercions ===
            ExprKind::As { lhs, target } => {
                let started = Instant::now();
                let actual_target_ty = self.evaluate_dynamic_typeof(target);
                let ty = self.check_as_expr(lhs, actual_target_ty);
                self.ctx.expr_timing_stats.ops += started.elapsed();
                ty
            }

            // === 6. Memory access ===
            ExprKind::IndexAccess { lhs, index, is_mut } => {
                let started = Instant::now();
                let ty = self.check_index_access(lhs, index, *is_mut, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.access += elapsed;
                self.ctx.expr_timing_stats.access_index += elapsed;
                ty
            }
            ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let started = Instant::now();
                let ty = self.check_field_access(expr.id, lhs, *field, *field_span, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.access += elapsed;
                self.ctx.expr_timing_stats.access_field += elapsed;
                ty
            }
            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                is_mut,
            } => {
                let started = Instant::now();
                let ty = self.check_slice_op(
                    lhs,
                    start.as_deref(),
                    end.as_deref(),
                    *is_inclusive,
                    *is_mut,
                    expr.span,
                );
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.access += elapsed;
                self.ctx.expr_timing_stats.access_slice += elapsed;
                ty
            }

            // === 7. Calls and macros ===
            ExprKind::Call { callee, args } => {
                let started = Instant::now();
                let ty = self.check_call(callee, args, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.call += elapsed;
                self.ctx.expr_timing_stats.call_plain += elapsed;
                ty
            }
            ExprKind::GenericInstantiation { target, types } => {
                let started = Instant::now();
                for ty_node in types {
                    self.evaluate_dynamic_typeof(ty_node);
                }
                let ty = self.check_generic_instantiation(target, types, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.call += elapsed;
                self.ctx.expr_timing_stats.call_generic_instantiation += elapsed;
                ty
            }
            ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                let started = Instant::now();
                let ty = self.check_closure(expr.id, captures, params, ret_type, body, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.call += elapsed;
                self.ctx.expr_timing_stats.call_closure += elapsed;
                ty
            }

            // === 8. Aggregate literals ===
            ExprKind::DataInit { type_node, literal } => {
                let started = Instant::now();
                if let Some(t_node) = type_node {
                    self.evaluate_dynamic_typeof(t_node);
                }
                let ty =
                    self.check_data_init_expr(type_node.as_deref(), literal, expected_ty, expr.span);
                self.ctx.expr_timing_stats.aggregate += started.elapsed();
                ty
            }
            ExprKind::EnumLiteral {
                variant,
                variant_span,
            } => {
                let started = Instant::now();
                let ty = self.check_enum_literal(*variant, *variant_span, expected_ty, expr.span);
                self.ctx.expr_timing_stats.aggregate += started.elapsed();
                ty
            }
            ExprKind::Undef => {
                let started = Instant::now();
                let ty = self.check_undef(expected_ty, expr.span);
                self.ctx.expr_timing_stats.aggregate += started.elapsed();
                ty
            }

            // === 9. Control flow ===
            ExprKind::Block { stmts, result } => {
                let started = Instant::now();
                let ty = self.check_block(stmts, result.as_deref(), expected_ty);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_block += elapsed;
                ty
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let started = Instant::now();
                let ty = self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_if += elapsed;
                ty
            }
            ExprKind::Match { target, arms } => {
                let started = Instant::now();
                let ty = self.check_match_expr(target, arms, expected_ty, expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_match += elapsed;
                ty
            }
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let started = Instant::now();
                let ty = self.check_for(init.as_deref(), cond.as_deref(), post.as_deref(), body);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_for += elapsed;
                ty
            }
            ExprKind::Defer { expr: defer_expr } => {
                let started = Instant::now();
                let ty = self.check_defer(defer_expr);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_defer += elapsed;
                ty
            }
            ExprKind::Break | ExprKind::Continue => TypeId::NEVER,
            ExprKind::Return(val) => {
                let started = Instant::now();
                self.check_return(val.as_deref(), expr.span);
                let elapsed = started.elapsed();
                self.ctx.expr_timing_stats.control += elapsed;
                self.ctx.expr_timing_stats.control_return += elapsed;
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

    /// Recursively scan AST type nodes, resolve every `@typeOf`, and rebuild the final type bottom-up.
    pub(crate) fn evaluate_dynamic_typeof(&mut self, ty_node: &kernc_ast::TypeNode) -> TypeId {
        let started = Instant::now();
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
            // Plain static types such as `Path` or `SelfType` cannot contain nested `@typeOf`.
            // Delegate them directly to the type resolver.
            _ => {
                let mut resolver = TypeResolver::new(self.ctx);
                let scope = resolver.current_scope_id().unwrap();
                resolver.resolve_type(ty_node, scope)
            }
        };

        // Overwrite the cached node type with the freshly resolved result.
        self.ctx.node_types.insert(ty_node.id, ty_id);
        self.ctx.expr_timing_stats.dynamic_typeof += started.elapsed();
        ty_id
    }
}
