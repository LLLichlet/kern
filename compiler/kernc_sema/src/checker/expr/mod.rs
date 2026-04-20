use crate::context::SemaContext;
use crate::def::DefId;
use crate::passes::TypeResolver;
use crate::scope::ScopeId;
use crate::ty::{
    AnonymousEnum, AnonymousVariant, BuiltinAnonymousEnumKind, ConstExprKind, ConstGeneric,
    GenericArg, TypeId, TypeKind,
};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{FastHashMap, Span, SymbolId};
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::time::{Duration, Instant};

mod access;
mod call;
mod cast;
mod coercion;
mod control;
mod literal;
mod ops;

use access::LetElseClause;

pub(crate) struct ExprChecker<'a, 'ctx> {
    pub(crate) ctx: &'a mut SemaContext<'ctx>,
    pub(crate) current_return_type: Option<TypeId>,
    pub(crate) has_returned: bool,
    pub(crate) type_vars: Vec<Option<TypeId>>,
    pub(crate) trait_obligation_stack: Vec<(TypeId, TypeId)>,
    pub(crate) projection_normalization_stack: Vec<TypeId>,
    pub(crate) current_module_cache: Option<(ScopeId, Option<DefId>)>,
    pub(crate) allow_uninstantiated_generic_function_items: bool,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn new(ctx: &'a mut SemaContext<'ctx>, current_return_type: Option<TypeId>) -> Self {
        Self {
            ctx,
            current_return_type,
            has_returned: false,
            type_vars: Vec::new(),
            trait_obligation_stack: Vec::new(),
            projection_normalization_stack: Vec::new(),
            current_module_cache: None,
            allow_uninstantiated_generic_function_items: false,
        }
    }

    pub(crate) fn with_uninstantiated_generic_function_items_allowed<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let prev = self.allow_uninstantiated_generic_function_items;
        self.allow_uninstantiated_generic_function_items = true;
        let result = f(self);
        self.allow_uninstantiated_generic_function_items = prev;
        result
    }

    fn reject_uninstantiated_generic_function_item(&mut self, expr: &Expr, ty: TypeId) -> TypeId {
        let norm_ty = self.resolve_tv(ty);
        let TypeKind::FnDef(def_id, generic_args) = self.ctx.type_registry.get(norm_ty).clone()
        else {
            return ty;
        };

        let Some(function) = self
            .ctx
            .defs
            .get(def_id.0 as usize)
            .and_then(|def| match def {
                crate::def::Def::Function(function) => Some(function),
                _ => None,
            })
        else {
            return ty;
        };

        if function.generics.is_empty() || generic_args.len() >= function.generics.len() {
            return ty;
        }

        let fn_name = self.ctx.resolve(function.name).to_string();
        self.ctx
            .struct_error(
                expr.span,
                format!(
                    "generic function `{}` cannot be used as a value without explicit instantiation",
                    fn_name
                ),
            )
            .with_hint(format!(
                "use `{}[...]` with concrete generic arguments, for example `{}[i32]`",
                fn_name, fn_name
            ))
            .with_hint("bare generic function items are only allowed in direct call position")
            .emit();
        TypeId::ERROR
    }

    fn timing_start(&self) -> Option<Instant> {
        self.ctx.collects_timings().then(Instant::now)
    }

    fn record_expr_timing(
        &mut self,
        started: Option<Instant>,
        record: impl FnOnce(&mut crate::context::ExprTimingStats, Duration),
    ) {
        if let Some(started) = started {
            record(&mut self.ctx.expr_timing_stats, started.elapsed());
        }
    }

    pub(crate) fn generic_param_occurs_in_type_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        ty: TypeId,
        map: &HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, &mut Vec::new())
    }

    fn generic_param_occurs_in_type_with_map_inner<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        ty: TypeId,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_) => false,
            TypeKind::Alias(..) => unreachable!("aliases are removed by resolve_tv"),
            TypeKind::Param(name) => {
                if name == needle {
                    return true;
                }
                if param_stack.contains(&name) {
                    return false;
                }
                let Some(&mapped_ty) = map.get(&name) else {
                    return false;
                };
                param_stack.push(name);
                let occurs =
                    self.generic_param_occurs_in_type_with_map_inner(needle, mapped_ty, map, param_stack);
                param_stack.pop();
                occurs
            }
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::ArrayInfer { elem, .. }
            | TypeKind::AnonymousEnumPayload(elem)
            | TypeKind::Simd { elem, .. } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, elem, map, param_stack)
            }
            TypeKind::Array { elem, len, .. } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, elem, map, param_stack)
                    || self.generic_param_occurs_in_const_generic_with_map(
                        needle,
                        len,
                        map,
                        param_stack,
                    )
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::FnDef(_, args)
            | TypeKind::Associated(_, args) => args.into_iter().any(|arg| {
                self.generic_param_occurs_in_generic_arg_with_map(needle, arg, map, param_stack)
            }),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter().any(|arg| {
                    self.generic_param_occurs_in_generic_arg_with_map(needle, arg, map, param_stack)
                }) || assoc_bindings.into_iter().any(|(_, assoc_ty)| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        assoc_ty,
                        map,
                        param_stack,
                    )
                })
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.generic_param_occurs_in_type_with_map_inner(needle, target, map, param_stack)
                    || trait_args.into_iter().any(|arg| {
                        self.generic_param_occurs_in_generic_arg_with_map(
                            needle,
                            arg,
                            map,
                            param_stack,
                        )
                    })
                    || assoc_args.into_iter().any(|arg| {
                        self.generic_param_occurs_in_generic_arg_with_map(
                            needle,
                            arg,
                            map,
                            param_stack,
                        )
                    })
            }
            TypeKind::ClosureInterface { params, ret } | TypeKind::Function { params, ret, .. } => {
                params.into_iter().any(|param_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        param_ty,
                        map,
                        param_stack,
                    )
                }) || self.generic_param_occurs_in_type_with_map_inner(
                    needle,
                    ret,
                    map,
                    param_stack,
                )
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures.into_iter().any(|capture_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        capture_ty,
                        map,
                        param_stack,
                    )
                }) || params.into_iter().any(|param_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        param_ty,
                        map,
                        param_stack,
                    )
                }) || self.generic_param_occurs_in_type_with_map_inner(
                    needle,
                    ret,
                    map,
                    param_stack,
                )
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        field.ty,
                        map,
                        param_stack,
                    )
                }),
            TypeKind::AnonymousEnum(enum_def) => {
                enum_def.backing_ty.is_some_and(|backing_ty| {
                    self.generic_param_occurs_in_type_with_map_inner(
                        needle,
                        backing_ty,
                        map,
                        param_stack,
                    )
                }) || enum_def.variants.iter().any(|variant| {
                    variant.payload_ty.is_some_and(|payload_ty| {
                        self.generic_param_occurs_in_type_with_map_inner(
                            needle,
                            payload_ty,
                            map,
                            param_stack,
                        )
                    })
                })
            }
        }
    }

    fn generic_param_occurs_in_generic_arg_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        arg: GenericArg,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match arg {
            GenericArg::Type(ty) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, param_stack)
            }
            GenericArg::Const(value) => {
                self.generic_param_occurs_in_const_generic_with_map(needle, value, map, param_stack)
            }
        }
    }

    fn generic_param_occurs_in_const_generic_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, TypeId, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match value {
            ConstGeneric::Value(value) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, value.ty, map, param_stack)
            }
            ConstGeneric::Param(_, ty) => {
                self.generic_param_occurs_in_type_with_map_inner(needle, ty, map, param_stack)
            }
            ConstGeneric::Expr(expr_id) => match *self.ctx.type_registry.const_expr(expr_id) {
                ConstExprKind::Unary { expr, ty, .. } | ConstExprKind::Cast { expr, ty } => {
                    self.generic_param_occurs_in_const_generic_with_map(needle, expr, map, param_stack)
                        || self.generic_param_occurs_in_type_with_map_inner(
                            needle,
                            ty,
                            map,
                            param_stack,
                        )
                }
                ConstExprKind::Binary { lhs, rhs, ty, .. } => {
                    self.generic_param_occurs_in_const_generic_with_map(needle, lhs, map, param_stack)
                        || self.generic_param_occurs_in_const_generic_with_map(
                            needle,
                            rhs,
                            map,
                            param_stack,
                        )
                        || self.generic_param_occurs_in_type_with_map_inner(
                            needle,
                            ty,
                            map,
                            param_stack,
                        )
                }
            },
            ConstGeneric::Error => false,
        }
    }

    pub(crate) fn const_param_occurs_in_const_generic_with_map<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, ConstGeneric, S>,
    ) -> bool {
        self.const_param_occurs_in_const_generic_with_map_inner(needle, value, map, &mut Vec::new())
    }

    fn const_param_occurs_in_const_generic_with_map_inner<S: BuildHasher>(
        &mut self,
        needle: SymbolId,
        value: ConstGeneric,
        map: &HashMap<SymbolId, ConstGeneric, S>,
        param_stack: &mut Vec<SymbolId>,
    ) -> bool {
        match value {
            ConstGeneric::Value(_) | ConstGeneric::Error => false,
            ConstGeneric::Param(name, _) => {
                if name == needle {
                    return true;
                }
                if param_stack.contains(&name) {
                    return false;
                }
                let Some(&mapped_value) = map.get(&name) else {
                    return false;
                };
                param_stack.push(name);
                let occurs = self.const_param_occurs_in_const_generic_with_map_inner(
                    needle,
                    mapped_value,
                    map,
                    param_stack,
                );
                param_stack.pop();
                occurs
            }
            ConstGeneric::Expr(expr_id) => match *self.ctx.type_registry.const_expr(expr_id) {
                ConstExprKind::Unary { expr, .. } | ConstExprKind::Cast { expr, .. } => self
                    .const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        expr,
                        map,
                        param_stack,
                    ),
                ConstExprKind::Binary { lhs, rhs, .. } => {
                    self.const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        lhs,
                        map,
                        param_stack,
                    ) || self.const_param_occurs_in_const_generic_with_map_inner(
                        needle,
                        rhs,
                        map,
                        param_stack,
                    )
                }
            },
        }
    }

    pub(crate) fn build_generic_subst_map<TS: BuildHasher, CS: BuildHasher>(
        &self,
        type_map: &HashMap<SymbolId, TypeId, TS>,
        const_map: &HashMap<SymbolId, ConstGeneric, CS>,
    ) -> FastHashMap<SymbolId, GenericArg> {
        let mut subst_map = FastHashMap::default();
        for (&name, &ty) in type_map {
            subst_map.insert(name, GenericArg::Type(ty));
        }
        for (&name, &value) in const_map {
            subst_map.insert(name, GenericArg::Const(value));
        }
        subst_map
    }

    pub(crate) fn substitute_type_with_unification_maps<TS: BuildHasher, CS: BuildHasher>(
        &mut self,
        ty: TypeId,
        type_map: &HashMap<SymbolId, TypeId, TS>,
        const_map: &HashMap<SymbolId, ConstGeneric, CS>,
    ) -> TypeId {
        if type_map.is_empty() && const_map.is_empty() {
            return ty;
        }
        let subst_map = self.build_generic_subst_map(type_map, const_map);
        let mut subst = crate::checker::Substituter::new(&mut self.ctx.type_registry, &subst_map);
        subst.substitute(ty)
    }

    fn type_arg_is_direct_const_param_ref(&mut self, ty_node: &kernc_ast::TypeNode) -> bool {
        let ast::TypeKind::Path {
            anchor: None,
            segments,
        } = &ty_node.kind
        else {
            return false;
        };
        let [segment] = segments.as_slice() else {
            return false;
        };
        if !segment.args.is_empty() {
            return false;
        }

        self.ctx
            .scopes
            .resolve(segment.name)
            .is_some_and(|info| info.kind == crate::scope::SymbolKind::ConstParam)
    }

    fn type_arg_is_payloadless_enum_value_ref(
        &mut self,
        ty_node: &kernc_ast::TypeNode,
        span: Span,
    ) -> bool {
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return false;
        };
        if segments.len() < 2 || segments.iter().any(|segment| !segment.args.is_empty()) {
            return false;
        }

        let last_segment = segments.last().unwrap();
        let mut current_scope = match anchor {
            Some(anchor) => {
                let Some((_, scope)) = self.anchored_start_scope(*anchor, span) else {
                    return false;
                };
                scope
            }
            None => match self.ctx.scopes.current_scope_id() {
                Some(scope) => scope,
                None => return false,
            },
        };

        for (index, segment) in segments[..segments.len() - 1].iter().enumerate() {
            let symbol = if index == 0 && anchor.is_none() {
                self.ctx.scopes.resolve_from(current_scope, segment.name)
            } else {
                self.ctx.scopes.resolve_in(current_scope, segment.name)
            };
            let Some(symbol) = symbol.cloned() else {
                return false;
            };

            match symbol.kind {
                crate::scope::SymbolKind::Module => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(crate::def::Def::Module(module)) =
                        self.ctx.defs.get(def_id.0 as usize)
                    else {
                        return false;
                    };
                    current_scope = module.scope_id;
                }
                crate::scope::SymbolKind::Enum if index == segments.len() - 2 => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(crate::def::Def::Enum(enum_def)) =
                        self.ctx.defs.get(def_id.0 as usize)
                    else {
                        return false;
                    };
                    return enum_def.variants.iter().any(|variant| {
                        variant.name == last_segment.name && variant.payload_type.is_none()
                    });
                }
                crate::scope::SymbolKind::TypeAlias if index == segments.len() - 2 => {
                    let alias_ty = self.ctx.type_registry.normalize(symbol.type_id);
                    return match self.ctx.type_registry.get(alias_ty) {
                        TypeKind::Enum(def_id, _) => self
                            .ctx
                            .defs
                            .get(def_id.0 as usize)
                            .and_then(|def| match def {
                                crate::def::Def::Enum(enum_def) => Some(enum_def),
                                _ => None,
                            })
                            .is_some_and(|enum_def| {
                                enum_def.variants.iter().any(|variant| {
                                    variant.name == last_segment.name
                                        && variant.payload_type.is_none()
                                })
                            }),
                        TypeKind::AnonymousEnum(enum_def) => {
                            enum_def.variants.iter().any(|variant| {
                                variant.name == last_segment.name && variant.payload_ty.is_none()
                            })
                        }
                        _ => false,
                    };
                }
                _ => return false,
            }
        }

        false
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
                let started = self.timing_start();
                let ty = self.check_identifier(*name, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_identifier += elapsed;
                });
                ty
            }
            ExprKind::AnchoredPath { anchor, name, .. } => {
                let started = self.timing_start();
                let ty = self.check_anchored_identifier(*anchor, *name, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_identifier += elapsed;
                });
                ty
            }
            ExprKind::TypeNode(type_node) => self.evaluate_dynamic_typeof(type_node),
            ExprKind::SelfValue => self.check_self_value(expr.span),

            // === 3. Declarations and bindings ===
            ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                let started = self.timing_start();
                let ty = self.check_let(
                    expr.id,
                    pattern,
                    init,
                    else_branch.as_deref().map(|branch| LetElseClause {
                        pattern: else_pattern.as_ref(),
                        branch,
                    }),
                    expected_ty,
                    expr.span,
                );
                self.record_expr_timing(started, |stats, elapsed| stats.bindings += elapsed);
                ty
            }
            ExprKind::Static { pattern, init, .. } => {
                let started = self.timing_start();
                let ty = self.check_static(expr.id, pattern, init, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.bindings += elapsed);
                ty
            }

            // === 4. Operators and assignment ===
            ExprKind::Binary { lhs, op, rhs } => {
                let started = self.timing_start();
                let ty = self.check_binary(lhs, *op, rhs, expected_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Unary { op, operand } => {
                let started = self.timing_start();
                let ty = self.check_unary(*op, operand, expr.span, expected_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Assign { lhs, rhs, .. } => {
                let started = self.timing_start();
                let ty = self.check_assign(lhs, rhs);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }

            // === 5. Casts and coercions ===
            ExprKind::As { lhs, target } => {
                let started = self.timing_start();
                let actual_target_ty = self.evaluate_dynamic_typeof(target);
                let ty = self.check_as_expr(lhs, actual_target_ty);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }
            ExprKind::Propagate { operand, kind } => {
                let started = self.timing_start();
                let ty = self.check_propagate(operand, *kind, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.ops += elapsed);
                ty
            }

            // === 6. Memory access ===
            ExprKind::IndexAccess { lhs, index, is_mut } => {
                let started = self.timing_start();
                let ty = self.check_index_access(lhs, index, *is_mut, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_index += elapsed;
                });
                ty
            }
            ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                let started = self.timing_start();
                let ty = self.check_field_access(expr.id, lhs, *field, *field_span, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_field += elapsed;
                });
                ty
            }
            ExprKind::SliceOp {
                lhs,
                start,
                end,
                is_inclusive,
                is_mut,
            } => {
                let started = self.timing_start();
                let ty = self.check_slice_op(
                    lhs,
                    start.as_deref(),
                    end.as_deref(),
                    *is_inclusive,
                    *is_mut,
                    expr.span,
                );
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.access += elapsed;
                    stats.access_slice += elapsed;
                });
                ty
            }

            // === 7. Calls and macros ===
            ExprKind::Call { callee, args } => {
                let started = self.timing_start();
                let ty = self.check_call(callee, args, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_plain += elapsed;
                });
                ty
            }
            ExprKind::GenericInstantiation { target, args } => {
                let started = self.timing_start();
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty_node)
                        | ast::GenericArg::AssocBinding { value: ty_node, .. } => {
                            if matches!(arg, ast::GenericArg::Type(_))
                                && (self.type_arg_is_direct_const_param_ref(ty_node)
                                    || self
                                        .type_arg_is_payloadless_enum_value_ref(ty_node, expr.span))
                            {
                                continue;
                            }
                            self.evaluate_dynamic_typeof(ty_node);
                        }
                        ast::GenericArg::ConstExpr(expr) => {
                            self.check_expr(expr, None);
                        }
                    }
                }
                let ty = self.check_generic_instantiation(target, args, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_generic_instantiation += elapsed;
                });
                ty
            }
            ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                let started = self.timing_start();
                let ty = self.check_closure(expr.id, captures, params, ret_type, body, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.call += elapsed;
                    stats.call_closure += elapsed;
                });
                ty
            }

            // === 8. Aggregate literals ===
            ExprKind::DataInit { type_node, literal } => {
                let started = self.timing_start();
                let target_ty = if let Some(t_node) = type_node {
                    self.evaluate_dynamic_typeof(t_node)
                } else {
                    self.resolve_data_init_target_type(None, expected_ty, expr.span)
                };
                let ty =
                    self.check_data_init_expr(target_ty, literal, type_node.is_none(), expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }
            ExprKind::EnumLiteral {
                variant,
                variant_span,
            } => {
                let started = self.timing_start();
                let ty = self.check_enum_literal(*variant, *variant_span, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }
            ExprKind::Undef => {
                let started = self.timing_start();
                let ty = self.check_undef(expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| stats.aggregate += elapsed);
                ty
            }

            // === 9. Control flow ===
            ExprKind::Block { stmts, result } => {
                let started = self.timing_start();
                let ty = self.check_block(stmts, result.as_deref(), expected_ty);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_block += elapsed;
                });
                ty
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let started = self.timing_start();
                let ty = self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_if += elapsed;
                });
                ty
            }
            ExprKind::Match { target, arms } => {
                let started = self.timing_start();
                let ty = self.check_match_expr(target, arms, expected_ty, expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_match += elapsed;
                });
                ty
            }
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let started = self.timing_start();
                let ty = self.check_for(init.as_deref(), cond.as_deref(), post.as_deref(), body);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_for += elapsed;
                });
                ty
            }
            ExprKind::Defer { expr: defer_expr } => {
                let started = self.timing_start();
                let ty = self.check_defer(defer_expr);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_defer += elapsed;
                });
                ty
            }
            ExprKind::Break | ExprKind::Continue => TypeId::NEVER,
            ExprKind::Return(val) => {
                let started = self.timing_start();
                self.check_return(val.as_deref(), expr.span);
                self.record_expr_timing(started, |stats, elapsed| {
                    stats.control += elapsed;
                    stats.control_return += elapsed;
                });
                TypeId::NEVER
            }

            ExprKind::Infer => {
                self.ctx.struct_error(expr.span, "type placeholder `_` cannot be evaluated as an expression")
                    .with_hint("in Kern, `_` is only used as a discard binding (`let _ =`) or in array length inference (`[_]T`)")
                    .emit();
                TypeId::ERROR
            }
        };

        let ty = if self.allow_uninstantiated_generic_function_items {
            ty
        } else {
            self.reject_uninstantiated_generic_function_item(expr, ty)
        };

        self.ctx.node_types.insert(expr.id, ty);
        ty
    }

    /// Recursively scan AST type nodes, resolve every `@typeOf`, and rebuild the final type bottom-up.
    pub(crate) fn evaluate_dynamic_typeof(&mut self, ty_node: &kernc_ast::TypeNode) -> TypeId {
        let started = self.timing_start();
        let ty_id = match &ty_node.kind {
            ast::TypeKind::TypeOf(inner_expr) => self.check_expr(inner_expr, None),
            ast::TypeKind::Optional { inner } => {
                let inner_ty = self.evaluate_dynamic_typeof(inner);
                let some = self.ctx.intern("Some");
                let none = self.ctx.intern("None");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: None,
                        builtin: Some(BuiltinAnonymousEnumKind::Optional),
                        variants: vec![
                            crate::ty::AnonymousVariant {
                                name: some,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(inner_ty),
                                explicit_value: None,
                            },
                            crate::ty::AnonymousVariant {
                                name: none,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: None,
                                explicit_value: None,
                            },
                        ],
                    }))
            }
            ast::TypeKind::Result { ok, err } => {
                let ok_ty = self.evaluate_dynamic_typeof(ok);
                let err_ty = self.evaluate_dynamic_typeof(err);
                let ok_name = self.ctx.intern("Ok");
                let err_name = self.ctx.intern("Err");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: None,
                        builtin: Some(BuiltinAnonymousEnumKind::Result),
                        variants: vec![
                            crate::ty::AnonymousVariant {
                                name: ok_name,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(ok_ty),
                                explicit_value: None,
                            },
                            crate::ty::AnonymousVariant {
                                name: err_name,
                                name_span: kernc_utils::Span::default(),
                                payload_ty: Some(err_ty),
                                explicit_value: None,
                            },
                        ],
                    }))
            }
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
                    len: crate::ty::ConstGeneric::Value(crate::ty::ConstGenericValue {
                        ty: TypeId::USIZE,
                        kind: crate::ty::ConstGenericValueKind::Int(length as i128),
                    }),
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
        self.record_expr_timing(started, |stats, elapsed| stats.dynamic_typeof += elapsed);
        ty_id
    }

    fn check_propagate(
        &mut self,
        operand: &Expr,
        kind: ast::PropagateKind,
        span: kernc_utils::Span,
    ) -> TypeId {
        let Some(current_return_ty) = self.current_return_type else {
            self.ctx
                .struct_error(
                    span,
                    "propagation is only valid inside functions with a return type",
                )
                .emit();
            return TypeId::ERROR;
        };
        let norm_return = self.resolve_tv(current_return_ty);

        let TypeKind::AnonymousEnum(return_enum) = self.ctx.type_registry.get(norm_return).clone()
        else {
            let ret_str = self.ctx.ty_to_string(current_return_ty);
            self.ctx
                .struct_error(
                    span,
                    format!("propagation target function must return a builtin optional/result, found `{}`", ret_str),
                )
                .emit();
            return TypeId::ERROR;
        };

        let operand_expected = match kind {
            ast::PropagateKind::Option => Some(current_return_ty),
            ast::PropagateKind::Result => {
                let Some((_, ret_err_ty)) = return_enum.builtin_result_types() else {
                    let ret_str = self.ctx.ty_to_string(current_return_ty);
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.!` requires the enclosing function to return a builtin result, found `{}`",
                                ret_str
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                };

                let ok = self.fresh_type_var();
                let ok_name = self.ctx.intern("Ok");
                let err_name = self.ctx.intern("Err");
                Some(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                            backing_ty: None,
                            builtin: Some(BuiltinAnonymousEnumKind::Result),
                            variants: vec![
                                AnonymousVariant {
                                    name: ok_name,
                                    name_span: Span::default(),
                                    payload_ty: Some(ok),
                                    explicit_value: None,
                                },
                                AnonymousVariant {
                                    name: err_name,
                                    name_span: Span::default(),
                                    payload_ty: Some(ret_err_ty),
                                    explicit_value: None,
                                },
                            ],
                        })),
                )
            }
        };

        let operand_ty = self.check_expr(operand, operand_expected);
        let norm_operand = self.resolve_tv(operand_ty);

        let TypeKind::AnonymousEnum(operand_enum) =
            self.ctx.type_registry.get(norm_operand).clone()
        else {
            let op = match kind {
                ast::PropagateKind::Option => ".?",
                ast::PropagateKind::Result => ".!",
            };
            let found = self.ctx.ty_to_string(operand_ty);
            self.ctx
                .struct_error(
                    span,
                    format!("`{}` requires a builtin optional or result value", op),
                )
                .with_hint(format!("found `{}`", found))
                .emit();
            return TypeId::ERROR;
        };

        match kind {
            ast::PropagateKind::Option => {
                let Some(inner_ty) = operand_enum.builtin_optional_payload() else {
                    self.ctx
                        .struct_error(span, "`.?` requires a builtin optional value")
                        .emit();
                    return TypeId::ERROR;
                };
                if return_enum.builtin != Some(BuiltinAnonymousEnumKind::Optional) {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.?` requires the enclosing function to return a builtin optional, found `{}`",
                                self.ctx.ty_to_string(current_return_ty)
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                inner_ty
            }
            ast::PropagateKind::Result => {
                let Some((ok_ty, err_ty)) = operand_enum.builtin_result_types() else {
                    self.ctx
                        .struct_error(span, "`.!` requires a builtin result value")
                        .emit();
                    return TypeId::ERROR;
                };
                let Some((_, ret_err_ty)) = return_enum.builtin_result_types() else {
                    let ret_str = self.ctx.ty_to_string(current_return_ty);
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`.!` requires the enclosing function to return a builtin result, found `{}`",
                                ret_str
                            ),
                        )
                        .emit();
                    return TypeId::ERROR;
                };
                if err_ty != ret_err_ty && err_ty != TypeId::ERROR && ret_err_ty != TypeId::ERROR {
                    self.emit_mismatch_error(span, err_ty, ret_err_ty);
                    return TypeId::ERROR;
                }
                ok_ty
            }
        }
    }
}
