use super::{ExprChecker, SignatureDeductionInput};
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::ty::{ConstGeneric, GenericArg, TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::{FastHashMap, FastHashSet, Span, SymbolId};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn generic_bounds_success_cache_key(
        &mut self,
        def_id: DefId,
        arg_values: &[GenericArg],
    ) -> Option<(DefId, Vec<GenericArg>)> {
        if !self.ctx.analysis.active_bounds.is_empty() {
            return None;
        }

        // Bound satisfaction can depend on the caller's active where-bounds. Only cache fully
        // ground instantiations so we do not reuse an env-dependent success in a weaker context.
        let mut canonical_args = Vec::with_capacity(arg_values.len());
        for arg in arg_values.iter().copied() {
            match arg {
                GenericArg::Type(ty) => {
                    let ty = self.resolve_tv(ty);
                    if self.type_contains_unresolved_params(ty) {
                        return None;
                    }
                    canonical_args.push(GenericArg::Type(ty));
                }
                GenericArg::Const(value) => {
                    let value = self.ctx.type_registry.fold_const_generic(value);
                    if self.ctx.type_registry.const_generic_contains_params(value) {
                        return None;
                    }
                    canonical_args.push(GenericArg::Const(value));
                }
            }
        }
        Some((def_id, canonical_args))
    }

    fn substitute_known_const_generic(
        &mut self,
        value: ConstGeneric,
        const_map: &FastHashMap<SymbolId, ConstGeneric>,
    ) -> ConstGeneric {
        if const_map.is_empty() {
            return self.ctx.type_registry.fold_const_generic(value);
        }

        let subst_map = const_map
            .iter()
            .map(|(name, value)| (*name, GenericArg::Const(*value)))
            .collect::<FastHashMap<_, _>>();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        subst.substitute_const_generic(value)
    }

    fn infer_const_generic_direct(
        &mut self,
        generic: ConstGeneric,
        concrete: ConstGeneric,
        map: &mut FastHashMap<SymbolId, ConstGeneric>,
    ) -> bool {
        let generic = self.substitute_known_const_generic(generic, map);
        let generic_ty = self.ctx.type_registry.const_generic_ty(generic);
        let concrete_ty = self.ctx.type_registry.const_generic_ty(concrete);
        if generic_ty != concrete_ty {
            return false;
        }

        match generic {
            ConstGeneric::Param(name, _) => {
                if let Some(&existing) = map.get(&name) {
                    existing == concrete
                } else {
                    map.insert(name, concrete);
                    true
                }
            }
            ConstGeneric::Expr(_) => {
                if generic == concrete {
                    return true;
                }
                if self
                    .ctx
                    .type_registry
                    .const_generic_contains_params(generic)
                {
                    return false;
                }
                generic == concrete
            }
            ConstGeneric::Value(_) | ConstGeneric::Error => generic == concrete,
        }
    }

    fn infer_generic_arg_direct(
        &mut self,
        generic: GenericArg,
        concrete: GenericArg,
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, ConstGeneric>,
    ) -> bool {
        match (generic, concrete) {
            (GenericArg::Type(generic_ty), GenericArg::Type(concrete_ty)) => {
                self.infer_generic_args_from_types(generic_ty, concrete_ty, type_map, const_map)
            }
            (GenericArg::Const(generic), GenericArg::Const(concrete)) => {
                self.infer_const_generic_direct(generic, concrete, const_map)
            }
            _ => false,
        }
    }

    fn infer_generic_args_from_types(
        &mut self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, ConstGeneric>,
    ) -> bool {
        let generic_ty = self.resolve_tv(generic_ty);
        let concrete_ty = self.resolve_tv(concrete_ty);

        let generic_kind = self.ctx.type_registry.get(generic_ty).clone();
        let concrete_kind = self.ctx.type_registry.get(concrete_ty).clone();

        match (generic_kind, concrete_kind) {
            (TypeKind::Param(name), _) => {
                if let Some(&existing) = type_map.get(&name) {
                    existing == concrete_ty
                } else if matches!(self.ctx.type_registry.get(concrete_ty), TypeKind::Param(other) if *other == name)
                {
                    type_map.insert(name, concrete_ty);
                    true
                } else if self.generic_param_occurs_in_type_with_map(name, concrete_ty, type_map) {
                    false
                } else {
                    type_map.insert(name, concrete_ty);
                    true
                }
            }
            (
                TypeKind::Pointer {
                    is_mut: generic_mut,
                    elem: generic_elem,
                },
                TypeKind::Pointer {
                    is_mut: concrete_mut,
                    elem: concrete_elem,
                },
            ) => {
                generic_mut == concrete_mut
                    && self.infer_generic_args_from_types(
                        generic_elem,
                        concrete_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::VolatilePtr {
                    is_mut: generic_mut,
                    elem: generic_elem,
                },
                TypeKind::VolatilePtr {
                    is_mut: concrete_mut,
                    elem: concrete_elem,
                },
            ) => {
                generic_mut == concrete_mut
                    && self.infer_generic_args_from_types(
                        generic_elem,
                        concrete_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Slice {
                    is_mut: generic_mut,
                    elem: generic_elem,
                },
                TypeKind::Slice {
                    is_mut: concrete_mut,
                    elem: concrete_elem,
                },
            ) => {
                generic_mut == concrete_mut
                    && self.infer_generic_args_from_types(
                        generic_elem,
                        concrete_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::Array {
                    elem: generic_elem,
                    len: generic_len,
                },
                TypeKind::Array {
                    elem: concrete_elem,
                    len: concrete_len,
                },
            ) => {
                self.infer_const_generic_direct(generic_len, concrete_len, const_map)
                    && self.infer_generic_args_from_types(
                        generic_elem,
                        concrete_elem,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::ArrayInfer { elem: generic_elem },
                TypeKind::ArrayInfer {
                    elem: concrete_elem,
                },
            ) => {
                self.infer_generic_args_from_types(generic_elem, concrete_elem, type_map, const_map)
            }
            (
                TypeKind::Def(generic_def, generic_args),
                TypeKind::Def(concrete_def, concrete_args),
            )
            | (
                TypeKind::Enum(generic_def, generic_args),
                TypeKind::Enum(concrete_def, concrete_args),
            )
            | (
                TypeKind::Associated(generic_def, generic_args),
                TypeKind::Associated(concrete_def, concrete_args),
            )
            | (
                TypeKind::FnDef(generic_def, generic_args),
                TypeKind::FnDef(concrete_def, concrete_args),
            ) => {
                generic_def == concrete_def
                    && generic_args.len() == concrete_args.len()
                    && generic_args
                        .into_iter()
                        .zip(concrete_args)
                        .all(|(generic, concrete)| {
                            self.infer_generic_arg_direct(generic, concrete, type_map, const_map)
                        })
            }
            (
                TypeKind::TraitObject(generic_def, generic_args, generic_assoc),
                TypeKind::TraitObject(_, _, _),
            ) => self.infer_generic_args_from_trait_object_candidates(
                generic_def,
                &generic_args,
                &generic_assoc,
                concrete_ty,
                type_map,
                const_map,
            ),
            (
                TypeKind::Projection {
                    target: generic_target,
                    trait_def_id: generic_trait,
                    trait_args: generic_trait_args,
                    assoc_def_id: generic_assoc,
                    assoc_args: generic_assoc_args,
                },
                TypeKind::Projection {
                    target: concrete_target,
                    trait_def_id: concrete_trait,
                    trait_args: concrete_trait_args,
                    assoc_def_id: concrete_assoc,
                    assoc_args: concrete_assoc_args,
                },
            ) => {
                generic_trait == concrete_trait
                    && generic_assoc == concrete_assoc
                    && generic_trait_args.len() == concrete_trait_args.len()
                    && generic_assoc_args.len() == concrete_assoc_args.len()
                    && self.infer_generic_args_from_types(
                        generic_target,
                        concrete_target,
                        type_map,
                        const_map,
                    )
                    && generic_trait_args.into_iter().zip(concrete_trait_args).all(
                        |(generic, concrete)| {
                            self.infer_generic_arg_direct(generic, concrete, type_map, const_map)
                        },
                    )
                    && generic_assoc_args.into_iter().zip(concrete_assoc_args).all(
                        |(generic, concrete)| {
                            self.infer_generic_arg_direct(generic, concrete, type_map, const_map)
                        },
                    )
            }
            (
                TypeKind::Function {
                    params: generic_params,
                    ret: generic_ret,
                    is_variadic: generic_variadic,
                },
                TypeKind::Function {
                    params: concrete_params,
                    ret: concrete_ret,
                    is_variadic: concrete_variadic,
                },
            ) => {
                generic_variadic == concrete_variadic
                    && generic_params.len() == concrete_params.len()
                    && generic_params
                        .into_iter()
                        .zip(concrete_params)
                        .all(|(generic, concrete)| {
                            self.infer_generic_args_from_types(
                                generic, concrete, type_map, const_map,
                            )
                        })
                    && self.infer_generic_args_from_types(
                        generic_ret,
                        concrete_ret,
                        type_map,
                        const_map,
                    )
            }
            (
                TypeKind::ClosureInterface {
                    params: generic_params,
                    ret: generic_ret,
                },
                TypeKind::ClosureInterface {
                    params: concrete_params,
                    ret: concrete_ret,
                },
            ) => {
                generic_params.len() == concrete_params.len()
                    && generic_params
                        .into_iter()
                        .zip(concrete_params)
                        .all(|(generic, concrete)| {
                            self.infer_generic_args_from_types(
                                generic, concrete, type_map, const_map,
                            )
                        })
                    && self.infer_generic_args_from_types(
                        generic_ret,
                        concrete_ret,
                        type_map,
                        const_map,
                    )
            }
            _ => generic_ty == concrete_ty,
        }
    }

    fn infer_generic_args_from_trait_object_candidates(
        &mut self,
        generic_def: DefId,
        generic_args: &[GenericArg],
        generic_assoc: &[(DefId, TypeId)],
        concrete_ty: TypeId,
        type_map: &mut FastHashMap<SymbolId, TypeId>,
        const_map: &mut FastHashMap<SymbolId, ConstGeneric>,
    ) -> bool {
        let mut candidates = Vec::new();
        let mut visited = FastHashSet::default();
        self.collect_trait_object_hierarchy_candidates(
            concrete_ty,
            generic_def,
            &mut visited,
            &mut candidates,
        );

        for concrete_view in candidates {
            let TypeKind::TraitObject(_, concrete_args, concrete_assoc) =
                self.ctx.type_registry.get(concrete_view).clone()
            else {
                continue;
            };
            if generic_args.len() != concrete_args.len() {
                continue;
            }

            let mut local_type_map = type_map.clone();
            let mut local_const_map = const_map.clone();
            let args_match = generic_args
                .iter()
                .copied()
                .zip(concrete_args.iter().copied())
                .all(|(generic, concrete)| {
                    self.infer_generic_arg_direct(
                        generic,
                        concrete,
                        &mut local_type_map,
                        &mut local_const_map,
                    )
                });
            if !args_match {
                continue;
            }

            let assoc_match = generic_assoc
                .iter()
                .all(|(assoc_def_id, generic_assoc_ty)| {
                    concrete_assoc
                        .iter()
                        .find(|(candidate_def_id, _)| *candidate_def_id == *assoc_def_id)
                        .is_some_and(|(_, concrete_assoc_ty)| {
                            self.infer_generic_args_from_types(
                                *generic_assoc_ty,
                                *concrete_assoc_ty,
                                &mut local_type_map,
                                &mut local_const_map,
                            )
                        })
                });
            if !assoc_match {
                continue;
            }

            *type_map = local_type_map;
            *const_map = local_const_map;
            return true;
        }

        false
    }

    fn collect_trait_object_hierarchy_candidates(
        &mut self,
        trait_ty: TypeId,
        target_trait_def_id: DefId,
        visited: &mut FastHashSet<TypeId>,
        out: &mut Vec<TypeId>,
    ) {
        let trait_ty = self.resolve_tv(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.ctx.type_registry.get(trait_ty).clone()
        else {
            return;
        };
        if !visited.insert(trait_ty) {
            return;
        }

        if trait_def_id == target_trait_def_id {
            out.push(trait_ty);
        }

        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            return;
        };
        let trait_arg_map = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect::<FastHashMap<_, _>>();
        let assoc_binding_map = assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();

        for super_ty in trait_def.resolved_supertraits {
            let substituted = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let substituted = crate::checker::substitute_associated_types(
                &mut self.ctx.type_registry,
                &self.ctx.defs,
                substituted,
                &assoc_binding_map,
            );
            let enriched = crate::query::augment_trait_object_assoc_bindings_from_map(
                self.ctx,
                substituted,
                &assoc_binding_map,
            );
            self.collect_trait_object_hierarchy_candidates(
                enriched,
                target_trait_def_id,
                visited,
                out,
            );
        }
    }

    fn generic_target_identity(&mut self, target_norm: TypeId, span: Span) -> Option<DefId> {
        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(id, args)
            | TypeKind::Def(id, args)
            | TypeKind::Enum(id, args)
            | TypeKind::TraitObject(id, args, _) => {
                let _ = args;
                Some(*id)
            }
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

    fn resolve_generic_instantiation_args(
        &mut self,
        def_id: DefId,
        args: &[ast::GenericArg],
        span: Span,
    ) -> Option<Vec<GenericArg>> {
        let scope = self.resolve_current_scope_for_types(span, "generic instantiation")?;
        let generics = match &self.ctx.defs[def_id.0 as usize] {
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
                return None;
            }
        };
        let mut resolver = TypeResolver::new(self.ctx);
        let (resolved_args, assoc_bindings) =
            resolver.resolve_generic_args_for_params(&generics, args, scope, span);
        if !assoc_bindings.is_empty() {
            self.ctx
                .struct_error(
                    span,
                    "generic expression instantiation does not accept associated type bindings",
                )
                .emit();
            return None;
        }
        Some(resolved_args)
    }

    fn instantiate_call_signature(
        &mut self,
        callee_ty: TypeId,
        raw_sig: TypeId,
        generics: &[ast::GenericParam],
        generic_args: &[GenericArg],
    ) -> TypeId {
        if generics.is_empty() || generic_args.is_empty() {
            return raw_sig;
        }

        if let Some(&cached_sig) = self
            .ctx
            .analysis
            .query_caches
            .call_signature_instantiation_cache
            .get(&callee_ty)
        {
            return cached_sig;
        }

        let mut map: FastHashMap<kernc_utils::SymbolId, GenericArg> = FastHashMap::default();
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
            .analysis
            .query_caches
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

            let mut map: FastHashMap<kernc_utils::SymbolId, TypeId> = FastHashMap::default();
            let mut const_map: FastHashMap<kernc_utils::SymbolId, ConstGeneric> =
                FastHashMap::default();
            for (param, explicit_arg) in generics.iter().zip(explicit_args.iter()) {
                match (&param.kind, explicit_arg) {
                    (ast::GenericParamKind::Type, GenericArg::Type(ty)) => {
                        map.insert(param.name, *ty);
                    }
                    (ast::GenericParamKind::Const { .. }, GenericArg::Const(value)) => {
                        const_map.insert(param.name, *value);
                    }
                    _ => {}
                }
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
                self.infer_generic_args_from_types(
                    expected_recv,
                    stripped_recv,
                    &mut map,
                    &mut const_map,
                );
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
                        self.infer_generic_args_from_types(
                            expected_param,
                            arg_norm,
                            &mut map,
                            &mut const_map,
                        );
                    }
                }
            }

            let mut missing_generics = Vec::new();
            let mut resolved_args = Vec::new();
            for param in generics {
                match &param.kind {
                    ast::GenericParamKind::Type => {
                        if let Some(&inferred_ty) = map.get(&param.name) {
                            resolved_args.push(GenericArg::Type(inferred_ty));
                        } else {
                            missing_generics.push(self.ctx.resolve(param.name).to_string());
                        }
                    }
                    ast::GenericParamKind::Const { .. } => {
                        if let Some(&value) = const_map.get(&param.name) {
                            resolved_args.push(GenericArg::Const(value));
                        } else {
                            missing_generics.push(self.ctx.resolve(param.name).to_string());
                        }
                    }
                }
            }

            if !missing_generics.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "cannot infer generic argument(s) `{}` for function `{}`",
                            missing_generics.join(", "),
                            name_str
                        ),
                    )
                    .with_hint(
                        "type generics are inferred from direct type matches; const generics are inferred only from direct structural matches such as `[N]T`",
                    )
                    .with_hint(
                        "Kern does not reverse-solve const expressions like `[N + 1]T`; write those arguments explicitly",
                    )
                    .emit();
                return (TypeId::ERROR, None, Some(inferred_arg_tys));
            }

            self.check_generic_bounds(span, def_id, generics, &resolved_args);

            let inferred_callee_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, resolved_args.clone()));
            return (
                self.instantiate_call_signature(
                    inferred_callee_ty,
                    raw_sig,
                    generics,
                    &resolved_args,
                ),
                Some(inferred_callee_ty),
                Some(inferred_arg_tys),
            );
        }

        (norm_callee, None, None)
    }

    pub(super) fn method_callee_field_access<'b>(&self, callee: &'b Expr) -> Option<&'b Expr> {
        match &callee.kind {
            ExprKind::FieldAccess { .. } => Some(callee),
            ExprKind::GenericInstantiation { target, .. } => match &target.kind {
                ExprKind::FieldAccess { .. } => Some(target),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let Some(method_target) = self.method_callee_field_access(callee)
            && let ExprKind::FieldAccess { lhs, .. } = &method_target.kind
        {
            let callee_node_ty = self
                .ctx
                .facts
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let lhs_node_ty = self
                .ctx
                .facts
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
        args: &[ast::GenericArg],
        span: Span,
    ) -> TypeId {
        let target_ty = self.with_uninstantiated_generic_function_items_allowed(|this| {
            this.resolve_type_namespace_expr(target)
                .unwrap_or_else(|| this.check_expr(target, None))
        });
        let target_norm = self.resolve_tv(target_ty);

        if target_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let Some(def_id) = self.generic_target_identity(target_norm, span) else {
            return TypeId::ERROR;
        };
        let Some(arg_values) = self.resolve_generic_instantiation_args(def_id, args, span) else {
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

        if generics.len() != arg_values.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "expected {} generic arguments, but {} were provided",
                        generics.len(),
                        arg_values.len()
                    ),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.check_generic_bounds(span, def_id, &generics, &arg_values);

        match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, arg_values)),
            TypeKind::Enum(..) => self
                .ctx
                .type_registry
                .intern(TypeKind::Enum(def_id, arg_values)),
            TypeKind::TraitObject(..) => {
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, arg_values, Vec::new()))
            }
            _ => self
                .ctx
                .type_registry
                .intern(TypeKind::Def(def_id, arg_values)),
        }
    }

    fn check_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        generics: &[ast::GenericParam],
        arg_values: &[GenericArg],
    ) {
        let cache_key = self.generic_bounds_success_cache_key(def_id, arg_values);
        if let Some(key) = cache_key.as_ref()
            && self
                .ctx
                .analysis
                .query_caches
                .generic_bounds_success_cache
                .contains(key)
        {
            return;
        }

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
            if i < arg_values.len() {
                map.insert(param.name, arg_values[i]);
            }
        }

        let mut all_bounds_satisfied = true;
        for clause in where_clauses {
            let original_target = self
                .ctx
                .facts
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
                    .facts
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
                    all_bounds_satisfied = false;
                    let req_str = self.ctx.ty_to_string(sub_bound);
                    let act_str = self.ctx.ty_to_string(sub_target);
                    self.ctx
                        .struct_error(span, "type does not satisfy trait bounds")
                        .with_hint(format!("required bound: `{}: {}`", act_str, req_str))
                        .emit();
                }
            }
        }

        if all_bounds_satisfied && let Some(key) = cache_key {
            self.ctx
                .analysis
                .query_caches
                .generic_bounds_success_cache
                .insert(key);
        }
    }
}
