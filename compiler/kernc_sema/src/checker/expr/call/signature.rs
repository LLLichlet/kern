use super::{ExprChecker, SignatureDeductionInput};
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{FastHashMap, Span};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn generic_target_identity(
        &mut self,
        target_norm: TypeId,
        span: Span,
    ) -> Option<(DefId, Vec<TypeId>)> {
        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(id, args)
            | TypeKind::Def(id, args)
            | TypeKind::Enum(id, args)
            | TypeKind::TraitObject(id, args, _) => Some((*id, args.clone())),
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "this expression does not support generic instantiation",
                    )
                    .emit();
                None
            }
        }
    }

    fn resolve_generic_instantiation_types(
        &mut self,
        types: &[ast::TypeNode],
        span: Span,
    ) -> Option<Vec<TypeId>> {
        let scope = self.resolve_current_scope_for_types(span, "generic instantiation")?;
        let mut resolver = TypeResolver::new(self.ctx);

        let mut arg_tys = Vec::with_capacity(types.len());
        for ty_node in types {
            arg_tys.push(resolver.resolve_type(ty_node, scope));
        }
        Some(arg_tys)
    }

    fn instantiate_call_signature(
        &mut self,
        callee_ty: TypeId,
        raw_sig: TypeId,
        generics: &[ast::GenericParam],
        generic_args: &[TypeId],
    ) -> TypeId {
        if generics.is_empty() || generic_args.is_empty() {
            return raw_sig;
        }

        if let Some(&cached_sig) = self.ctx.call_signature_instantiation_cache.get(&callee_ty) {
            return cached_sig;
        }

        let mut map = FastHashMap::default();
        for (param, generic_arg) in generics.iter().zip(generic_args.iter()) {
            map.insert(param.name, *generic_arg);
        }

        let sig_ty = if map.is_empty() {
            raw_sig
        } else {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            subst.substitute(raw_sig)
        };
        self.ctx
            .call_signature_instantiation_cache
            .insert(callee_ty, sig_ty);
        sig_ty
    }

    pub(super) fn deduce_and_resolve_signature(
        &mut self,
        norm_callee: TypeId,
        input: SignatureDeductionInput<'_>,
    ) -> (TypeId, Option<TypeId>, Option<Vec<Option<TypeId>>>) {
        let SignatureDeductionInput {
            args,
            is_method,
            receiver_ty,
            expected_ty,
            span,
            has_user_explicit_generics,
        } = input;
        if let TypeKind::FnDef(def_id, explicit_args) = self.ctx.type_registry.get(norm_callee) {
            let def_id = *def_id;
            let explicit_args_ptr = std::ptr::from_ref(explicit_args.as_slice());
            let explicit_args_len = explicit_args.len();
            let explicit_args = unsafe { &*explicit_args_ptr };
            let Some(function_ptr) =
                self.ctx
                    .defs
                    .get(def_id.0 as usize)
                    .and_then(|def| match def {
                        Def::Function(func) => Some(std::ptr::from_ref(func)),
                        _ => None,
                    })
            else {
                let other = &self.ctx.defs[def_id.0 as usize];
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: expected function Def for callee, found `{:?}`.",
                        other
                    ),
                );
                return (TypeId::ERROR, None, None);
            };
            let function = unsafe { &*function_ptr };
            let Some(raw_sig) = function.resolved_sig else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: function `{}` has no resolved signature during call checking.",
                        self.ctx.resolve(function.name)
                    ),
                );
                return (TypeId::ERROR, None, None);
            };
            let fn_name_id = function.name;
            let skip_expected_return_inference = matches!(
                self.ctx.resolve(fn_name_id),
                "@simdReduceAdd"
                    | "@simdReduceMul"
                    | "@simdReduceAnd"
                    | "@simdReduceOr"
                    | "@simdReduceXor"
                    | "@simdReduceMin"
                    | "@simdReduceMax"
            );
            let generics = function.generics.as_slice();
            let generics_count = generics.len();

            if generics_count == 0 {
                return (raw_sig, None, None);
            }

            if explicit_args_len > generics_count {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: function `{}` carried {} generic arguments, but only {} generic parameters exist.",
                        name_str,
                        explicit_args_len,
                        generics_count
                    ),
                );
                return (TypeId::ERROR, None, None);
            }

            if explicit_args.len() == generics_count {
                return (
                    self.instantiate_call_signature(norm_callee, raw_sig, generics, explicit_args),
                    None,
                    None,
                );
            }

            if has_user_explicit_generics && !explicit_args.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx.struct_error(span, format!("function `{}` requires exactly {} generic arguments, but {} were provided", name_str, generics_count, explicit_args.len()))
                    .with_hint("either provide all generic arguments or omit them entirely to let the compiler infer them")
                    .emit();
                return (TypeId::ERROR, None, None);
            }

            let mut map = FastHashMap::default();
            for (param, explicit_arg) in generics.iter().zip(explicit_args.iter()) {
                map.insert(param.name, *explicit_arg);
            }
            let (raw_params_ptr, raw_ret) = match self.ctx.type_registry.get(raw_sig) {
                TypeKind::Function { params, ret, .. } => {
                    (std::ptr::from_ref(params.as_slice()), *ret)
                }
                other => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Compiler ICE: expected function signature type during call checking, found `{:?}`.",
                            other
                        ),
                    );
                    return (TypeId::ERROR, None, None);
                }
            };
            let raw_params = unsafe { &*raw_params_ptr };
            let raw_param_count = raw_params.len();
            if raw_param_count == 0 && is_method {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: method call `{}` resolved to a signature without receiver parameter.",
                        self.ctx.resolve(fn_name_id)
                    ),
                );
                return (TypeId::ERROR, None, None);
            }
            let mut inferred_arg_tys = vec![None; args.len()];

            let param_offset = if is_method { 1 } else { 0 };

            if is_method {
                let mut stripped_recv = self.resolve_tv(receiver_ty);
                let expected_recv =
                    self.resolve_tv(raw_params.first().copied().unwrap_or(TypeId::ERROR));
                if let TypeKind::Pointer { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                {
                    if let TypeKind::Pointer { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                    {
                        stripped_recv = self.ctx.type_registry.intern(TypeKind::Pointer {
                            is_mut: false,
                            elem,
                        });
                    }
                } else if let TypeKind::VolatilePtr { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                    && let TypeKind::VolatilePtr { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                {
                    stripped_recv = self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut: false,
                        elem,
                    });
                }

                self.unify(expected_recv, stripped_recv, &mut map);
            }

            if let Some(expected_ty) = expected_ty
                && !skip_expected_return_inference
            {
                let expected_norm = self.resolve_tv(expected_ty);
                if expected_norm != TypeId::ERROR {
                    self.unify(raw_ret, expected_ty, &mut map);
                }
            }

            for (i, arg) in args.iter().enumerate() {
                let sig_idx = i + param_offset;
                let expected_param = raw_params.get(sig_idx).copied();
                if let Some(expected_param) = expected_param {
                    let substituted_expected = {
                        let mut substituter = Substituter::new(&mut self.ctx.type_registry, &map);
                        substituter.substitute(expected_param)
                    };
                    let arg_expected = if self.type_contains_unresolved_params(substituted_expected)
                    {
                        None
                    } else {
                        Some(substituted_expected)
                    };
                    let arg_ty = self.check_expr(arg, arg_expected);
                    inferred_arg_tys[i] = Some(arg_ty);
                    let arg_norm = self.resolve_tv(arg_ty);
                    if arg_norm != TypeId::ERROR {
                        self.unify(expected_param, arg_norm, &mut map);
                    }
                }
            }

            let mut missing_generics = Vec::new();
            let mut resolved_args = Vec::new();
            for param in generics {
                if let Some(&inferred_ty) = map.get(&param.name) {
                    resolved_args.push(inferred_ty);
                } else {
                    missing_generics.push(self.ctx.resolve(param.name).to_string());
                }
            }

            if !missing_generics.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "cannot infer generic type(s) `{}` for function `{}`",
                            missing_generics.join(", "),
                            name_str
                        ),
                    )
                    .with_hint("the compiler needs these generic types to be explicitly specified")
                    .emit();
                return (TypeId::ERROR, None, Some(inferred_arg_tys));
            }

            self.check_generic_bounds(span, def_id, generics, &resolved_args);

            let inferred_callee_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, resolved_args));
            let inferred_args_ptr = match self.ctx.type_registry.get(inferred_callee_ty) {
                TypeKind::FnDef(_, args) => std::ptr::from_ref(args.as_slice()),
                _ => unreachable!("just interned FnDef must remain a FnDef"),
            };
            let inferred_args = unsafe { &*inferred_args_ptr };
            return (
                self.instantiate_call_signature(
                    inferred_callee_ty,
                    raw_sig,
                    generics,
                    inferred_args,
                ),
                Some(inferred_callee_ty),
                Some(inferred_arg_tys),
            );
        }

        (norm_callee, None, None)
    }

    pub(crate) fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
            let callee_node_ty = self
                .ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let lhs_node_ty = self
                .ctx
                .node_types
                .get(&lhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let norm_lhs = self.ctx.type_registry.normalize(lhs_node_ty);
            if matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(..)) {
                return (false, TypeId::ERROR);
            }

            let norm_node_ty = self.ctx.type_registry.normalize(callee_node_ty);

            if matches!(
                self.ctx.type_registry.get(norm_node_ty),
                TypeKind::FnDef(..) | TypeKind::Function { .. }
            ) {
                return (true, lhs_node_ty);
            }
        }
        (false, TypeId::ERROR)
    }

    pub(crate) fn check_call_arity(
        &mut self,
        arg_count: usize,
        param_count: usize,
        is_method: bool,
        is_variadic: bool,
        span: Span,
    ) {
        let expected_arg_count = if is_method {
            param_count.saturating_sub(1)
        } else {
            param_count
        };

        if is_variadic {
            if arg_count < expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects at least {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        } else if arg_count != expected_arg_count {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "function expects exactly {} arguments, but {} were provided",
                        expected_arg_count, arg_count
                    ),
                )
                .emit();
        }
    }

    pub(super) fn check_method_receiver(
        &mut self,
        expected_self: TypeId,
        receiver_ty: TypeId,
        expr: &Expr,
    ) {
        let norm_expected = self.resolve_tv(expected_self);

        if !self.check_coercion(expr, expected_self, receiver_ty) {
            let is_exp_ptr = matches!(
                self.ctx.type_registry.get(norm_expected),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            if is_exp_ptr {
                self.ctx.struct_error(expr.span, "method receiver type mismatch")
                    .with_hint("the method expects a pointer receiver")
                    .with_hint("Kern does not implicitly take addresses for method calls. Try using `(&obj).method()` or `obj.&.method()`")
                    .emit();
            }
        }
    }

    pub(super) fn check_call_arguments(
        &mut self,
        args: &[Expr],
        params: &[TypeId],
        is_method: bool,
        _is_variadic: bool,
        inferred_arg_tys: Option<&[Option<TypeId>]>,
    ) {
        let param_offset = if is_method { 1 } else { 0 };

        for (i, arg) in args.iter().enumerate() {
            let sig_param_idx = i + param_offset;

            if sig_param_idx < params.len() {
                let expected = params[sig_param_idx];
                let arg_ty = inferred_arg_tys
                    .and_then(|tys| tys.get(i))
                    .and_then(|ty| *ty)
                    .unwrap_or_else(|| self.check_expr(arg, Some(expected)));
                self.check_coercion(arg, expected, arg_ty);
            } else {
                let arg_ty = inferred_arg_tys
                    .and_then(|tys| tys.get(i))
                    .and_then(|ty| *ty)
                    .unwrap_or_else(|| self.check_expr(arg, None));
                let norm_arg = self.resolve_tv(arg_ty);

                if norm_arg == TypeId::ERROR {
                    continue;
                }

                let is_small_int = matches!(
                    norm_arg,
                    TypeId::I8 | TypeId::I16 | TypeId::U8 | TypeId::U16
                );

                if is_small_int {
                    self.ctx.struct_error(arg.span, "C ABI requires integer arguments passed to `...` to be at least 32-bit")
                        .with_hint("please cast it explicitly (e.g., `as i32` or `as u32`)")
                        .emit();
                } else if norm_arg == TypeId::F32 {
                    self.ctx
                        .struct_error(
                            arg.span,
                            "C ABI requires float arguments passed to `...` to be 64-bit",
                        )
                        .with_hint("please cast it explicitly (e.g., `as f64`)")
                        .emit();
                }
            }
        }
    }

    pub(crate) fn check_generic_instantiation(
        &mut self,
        target: &Expr,
        types: &[ast::TypeNode],
        span: Span,
    ) -> TypeId {
        let target_ty = self.with_uninstantiated_generic_function_items_allowed(|this| {
            this.check_expr(target, None)
        });
        let target_norm = self.resolve_tv(target_ty);

        if target_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let Some(resolved_arg_tys) = self.resolve_generic_instantiation_types(types, span) else {
            return TypeId::ERROR;
        };
        let arg_tys = resolved_arg_tys;

        let Some((def_id, _)) = self.generic_target_identity(target_norm, span) else {
            return TypeId::ERROR;
        };

        let generics = {
            let def = &self.ctx.defs[def_id.0 as usize];
            match def {
                Def::Function(f) => f.generics.clone(),
                Def::Struct(s) => s.generics.clone(),
                Def::Union(u) => u.generics.clone(),
                Def::TypeAlias(t) => t.generics.clone(),
                Def::Enum(e) => e.generics.clone(),
                Def::Trait(t) => t.generics.clone(),
                other => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Compiler ICE: generic instantiation resolved to unsupported def `{:?}`.",
                            other
                        ),
                    );
                    return TypeId::ERROR;
                }
            }
        };

        if generics.len() != arg_tys.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "expected {} generic arguments, but {} were provided",
                        generics.len(),
                        arg_tys.len()
                    ),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.check_generic_bounds(span, def_id, &generics, &arg_tys);

        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, arg_tys)),
            TypeKind::Enum(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::Enum(def_id, arg_tys)),
            TypeKind::TraitObject(..) => {
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, arg_tys, Vec::new()))
            }
            _ => self
                .ctx
                .type_registry
                .intern(TypeKind::Def(def_id, arg_tys)),
        }
    }

    fn check_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        generics: &[ast::GenericParam],
        arg_tys: &[TypeId],
    ) {
        let has_where_clauses = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => !f.where_clauses.is_empty(),
            Def::Struct(s) => !s.where_clauses.is_empty(),
            Def::Union(u) => !u.where_clauses.is_empty(),
            Def::TypeAlias(t) => !t.where_clauses.is_empty(),
            Def::Impl(i) => !i.where_clauses.is_empty(),
            Def::Enum(e) => !e.where_clauses.is_empty(),
            Def::Trait(t) => !t.where_clauses.is_empty(),
            _ => false,
        };
        if !has_where_clauses {
            return;
        }

        let where_clauses_ptr = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => std::ptr::from_ref(f.where_clauses.as_slice()),
            Def::Struct(s) => std::ptr::from_ref(s.where_clauses.as_slice()),
            Def::Union(u) => std::ptr::from_ref(u.where_clauses.as_slice()),
            Def::TypeAlias(t) => std::ptr::from_ref(t.where_clauses.as_slice()),
            Def::Impl(i) => std::ptr::from_ref(i.where_clauses.as_slice()),
            Def::Enum(e) => std::ptr::from_ref(e.where_clauses.as_slice()),
            Def::Trait(t) => std::ptr::from_ref(t.where_clauses.as_slice()),
            _ => return,
        };
        let where_clauses = unsafe { &*where_clauses_ptr };

        let mut map = FastHashMap::default();
        for (i, param) in generics.iter().enumerate() {
            if i < arg_tys.len() {
                map.insert(param.name, arg_tys[i]);
            }
        }

        for clause in where_clauses {
            let original_target = self
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let sub_target = {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                subst.substitute(original_target)
            };

            for bound_ast in &clause.bounds {
                let original_bound = self
                    .ctx
                    .node_types
                    .get(&bound_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_bound = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(original_bound)
                };

                if sub_target != TypeId::ERROR
                    && sub_bound != TypeId::ERROR
                    && !self.check_trait_impl(sub_target, sub_bound)
                {
                    let req_str = self.ctx.ty_to_string(sub_bound);
                    let act_str = self.ctx.ty_to_string(sub_target);
                    self.ctx
                        .struct_error(span, "type does not satisfy trait bounds")
                        .with_hint(format!("required bound: `{}: {}`", act_str, req_str))
                        .emit();
                }
            }
        }
    }
}
