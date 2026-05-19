//! Trait object and vtable lowering.
//!
//! This module constructs vtable globals for concrete impl/trait pairs, resolves
//! inherited trait views and associated-type bindings, and decides when an
//! existing trait object can satisfy another required trait object.

use super::Lowerer;
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::def::{Def, ImplDef, TraitDef};
use kernc_sema::ty::{GenericArg, Substituter, TypeId, TypeKind, substitute_associated_types};
use kernc_utils::{Span, SymbolId};
use std::collections::{HashMap, HashSet};

pub(crate) struct VtableGlobalInput<'a> {
    vtable_id: MonoId,
    data_ptr_ty: TypeId,
    receiver_ty: TypeId,
    impl_receiver_ty: TypeId,
    actual_trait_ty: TypeId,
    trait_def: &'a TraitDef,
    impl_def: &'a ImplDef,
    impl_args: &'a [GenericArg],
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn trait_object_satisfies_required(
        &mut self,
        available_trait_ty: TypeId,
        required_trait_ty: TypeId,
    ) -> bool {
        let available_norm = self.ctx.normalize_concrete_type(available_trait_ty);
        let available_norm = self.ctx.type_registry.normalize(available_norm);
        let required_norm = self.ctx.normalize_concrete_type(required_trait_ty);
        let required_norm = self.ctx.type_registry.normalize(required_norm);

        let (
            TypeKind::TraitObject(available_def_id, available_args, available_assoc_bindings),
            TypeKind::TraitObject(required_def_id, required_args, required_assoc_bindings),
        ) = (
            self.ctx.type_registry.get(available_norm).clone(),
            self.ctx.type_registry.get(required_norm).clone(),
        )
        else {
            return false;
        };

        if available_def_id != required_def_id || available_args != required_args {
            return false;
        }

        if required_assoc_bindings.is_empty() {
            return true;
        }

        let available_assoc_bindings = available_assoc_bindings
            .into_iter()
            .collect::<HashMap<_, _>>();
        required_assoc_bindings
            .into_iter()
            .all(|(assoc_def_id, required_assoc_ty)| {
                available_assoc_bindings
                    .get(&assoc_def_id)
                    .is_some_and(|available_assoc_ty| {
                        self.ctx.type_registry.normalize(*available_assoc_ty)
                            == self.ctx.type_registry.normalize(required_assoc_ty)
                    })
            })
    }

    fn augment_trait_object_assoc_bindings_from_map(
        &mut self,
        trait_ty: TypeId,
        assoc_binding_map: &HashMap<kernc_sema::def::DefId, TypeId>,
    ) -> TypeId {
        let trait_ty = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.ctx.type_registry.get(trait_ty).clone()
        else {
            return trait_ty;
        };

        let mut merged = assoc_bindings.into_iter().collect::<HashMap<_, _>>();
        // Traversal should fill missing inherited bindings, not overwrite a more specific
        // equality already written on the current supertrait edge.
        for (&assoc_id, &assoc_ty) in assoc_binding_map {
            merged.entry(assoc_id).or_insert(assoc_ty);
        }

        let mut merged = merged.into_iter().collect::<Vec<_>>();
        merged.sort_by_key(|(assoc_id, _)| assoc_id.0);
        self.ctx
            .type_registry
            .intern(TypeKind::TraitObject(trait_def_id, trait_args, merged))
    }

    pub(crate) fn collect_transitive_supertraits(&mut self, trait_ty: TypeId) -> Vec<TypeId> {
        let root_trait_ty = self.ctx.normalize_concrete_type(trait_ty);
        let root_trait_ty = self.ctx.type_registry.normalize(root_trait_ty);
        let mut supertraits = Vec::new();
        let mut visited = HashSet::new();
        self.collect_transitive_supertraits_inner(root_trait_ty, &mut visited, &mut supertraits);

        let mut canonical = Vec::with_capacity(supertraits.len());
        for super_ty in supertraits {
            let canonical_super_ty = match self.ctx.type_registry.get(super_ty).clone() {
                TypeKind::TraitObject(super_def_id, super_args, _) => {
                    kernc_sema::query::declared_trait_object_view_from_hierarchy(
                        self.ctx,
                        root_trait_ty,
                        super_def_id,
                        &super_args,
                    )
                    .unwrap_or_else(|| {
                        kernc_sema::query::retain_declared_trait_object_assoc_bindings(
                            self.ctx, super_ty,
                        )
                    })
                }
                _ => super_ty,
            };

            if !canonical.contains(&canonical_super_ty) {
                canonical.push(canonical_super_ty);
            }
        }

        canonical
    }

    fn collect_transitive_supertraits_inner(
        &mut self,
        trait_ty: TypeId,
        visited: &mut HashSet<TypeId>,
        supertraits: &mut Vec<TypeId>,
    ) {
        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args, assoc_bindings) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): Expected TraitObject while collecting supertraits, found {:?}.",
                    self.ctx.type_registry.get(trait_norm)
                ),
            );
            return;
        };

        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not a trait while collecting supertraits.",
                    trait_def_id.0
                ),
            );
            return;
        };

        let trait_arg_map: HashMap<SymbolId, GenericArg> = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();
        let assoc_binding_map = assoc_bindings.into_iter().collect::<HashMap<_, _>>();

        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_ty = substitute_associated_types(
                &mut self.ctx.type_registry,
                &self.ctx.defs,
                inst_super_ty,
                &assoc_binding_map,
            );
            let inst_super_ty = self
                .augment_trait_object_assoc_bindings_from_map(inst_super_ty, &assoc_binding_map);
            let inst_super_norm = self.ctx.normalize_concrete_type(inst_super_ty);
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_norm);
            if visited.insert(inst_super_norm) {
                supertraits.push(inst_super_norm);
                self.collect_transitive_supertraits_inner(inst_super_norm, visited, supertraits);
            }
        }
    }

    pub(crate) fn vtable_supertrait_slot(
        &mut self,
        trait_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> Option<usize> {
        let target_norm = self.ctx.normalize_concrete_type(target_trait_ty);
        let target_norm = self.ctx.type_registry.normalize(target_norm);
        self.collect_transitive_supertraits(trait_ty)
            .iter()
            .position(|&super_ty| self.trait_object_satisfies_required(super_ty, target_norm))
    }

    pub(crate) fn is_trait_object_upcast(
        &mut self,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.ctx.normalize_concrete_type(source_trait_ty);
        let source_norm = self.ctx.type_registry.normalize(source_norm);
        let target_norm = self.ctx.normalize_concrete_type(target_trait_ty);
        let target_norm = self.ctx.type_registry.normalize(target_norm);
        self.trait_object_satisfies_required(source_norm, target_norm)
            || self
                .vtable_supertrait_slot(source_norm, target_norm)
                .is_some()
    }

    pub(crate) fn direct_trait_method_slot(
        &mut self,
        trait_ty: TypeId,
        method_name: SymbolId,
    ) -> Option<usize> {
        let trait_norm = self.ctx.normalize_concrete_type(trait_ty);
        let trait_norm = self.ctx.type_registry.normalize(trait_norm);
        let TypeKind::TraitObject(trait_def_id, _, _) =
            self.ctx.type_registry.get(trait_norm).clone()
        else {
            return None;
        };
        let Some(Def::Trait(trait_def)) = self.ctx.defs.get(trait_def_id.0 as usize).cloned()
        else {
            return None;
        };

        let direct_idx = trait_def
            .methods
            .iter()
            .position(|method| method.signature.name == method_name)?;
        Some(self.collect_transitive_supertraits(trait_norm).len() + direct_idx)
    }

    pub(crate) fn get_or_create_vtable(
        &mut self,
        data_ptr_ty: TypeId,
        receiver_ty: TypeId,
        trait_ty: TypeId,
    ) -> MonoId {
        let norm_data_ptr = self.ctx.type_registry.normalize(data_ptr_ty);
        let norm_receiver = self.ctx.type_registry.normalize(receiver_ty);
        let norm_trait = self.ctx.normalize_concrete_type(trait_ty);
        let norm_trait = self.ctx.type_registry.normalize(norm_trait);
        let key = (norm_data_ptr, norm_receiver, norm_trait);
        if let Some(&id) = self.vtable_cache.get(&key) {
            return id;
        }
        self.measure_phase("  lower_create_vtable", |this| {
            let trait_def_id = match this.ctx.type_registry.get(norm_trait) {
                TypeKind::TraitObject(id, _, _) => *id,
                other => {
                    return this.build_invalid_vtable(
                        key,
                        data_ptr_ty,
                        receiver_ty,
                        trait_ty,
                        format!(
                            "cannot build a vtable for non-trait-object type `{:?}`",
                            other
                        ),
                    );
                }
            };

            let trait_def = if let Def::Trait(t) = &this.ctx.defs[trait_def_id.0 as usize] {
                t.clone()
            } else {
                return this.build_invalid_vtable(
                    key,
                    data_ptr_ty,
                    receiver_ty,
                    trait_ty,
                    format!("cannot build a vtable because def `{}` is not a trait", trait_def_id.0),
                );
            };

            let (impl_def, impl_args, impl_receiver_ty) =
                match this.find_matching_impl_block(norm_receiver, norm_data_ptr, norm_trait) {
                    Some(found) => found,
                    None => {
                        let src_name = this.ctx.ty_to_string(norm_receiver);
                        let trait_name = this.ctx.resolve(trait_def.name);
                        return this.build_invalid_vtable(
                            key,
                            data_ptr_ty,
                            receiver_ty,
                            trait_ty,
                            format!(
                                "cannot build a vtable for cast `{} as {}` because no matching impl was found",
                                src_name, trait_name
                            ),
                        );
                    }
                };

            let vtable_id = this.new_mono_id();
            this.vtable_cache.insert(key, vtable_id);

            this.build_and_inject_vtable_global(VtableGlobalInput {
                vtable_id,
                data_ptr_ty,
                receiver_ty,
                impl_receiver_ty,
                actual_trait_ty: norm_trait,
                trait_def: &trait_def,
                impl_def: &impl_def,
                impl_args: &impl_args,
            });

            vtable_id
        })
    }

    pub(crate) fn find_matching_impl_block(
        &mut self,
        receiver_ty: TypeId,
        data_ptr_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> Option<(ImplDef, Vec<kernc_sema::ty::GenericArg>, TypeId)> {
        let norm_receiver = self.ctx.type_registry.normalize(receiver_ty);
        let norm_data_ptr = self.ctx.type_registry.normalize(data_ptr_ty);
        let target_trait_norm = self.ctx.normalize_concrete_type(target_trait_ty);
        let target_trait_norm = self.ctx.type_registry.normalize(target_trait_norm);
        let target_trait_id = match self.ctx.type_registry.get(target_trait_norm) {
            TypeKind::TraitObject(id, _, _) => *id,
            _ => return None,
        };
        let search_types = self.vtable_impl_search_types(norm_receiver, norm_data_ptr);

        let mut selected: Option<(
            ImplDef,
            Vec<kernc_sema::ty::GenericArg>,
            kernc_sema::def::DefId,
            TypeId,
        )> = None;
        for entry in self.ctx.global_impl_entries() {
            let impl_id = entry.id;
            let impl_def = entry.def;

            let Some(impl_trait_node) = &impl_def.trait_type else {
                continue;
            };

            let impl_trait_ty = self
                .ctx
                .node_type(impl_trait_node.id)
                .unwrap_or(TypeId::ERROR);
            if impl_trait_ty == TypeId::ERROR {
                continue;
            }

            let impl_trait_norm = self.ctx.type_registry.normalize(impl_trait_ty);
            if !matches!(
                self.ctx.type_registry.get(impl_trait_norm),
                TypeKind::TraitObject(i_trait_id, _, _) if *i_trait_id == target_trait_id
            ) {
                continue;
            }

            for search_ty in &search_types {
                let Some(resolved_impl_args) = kernc_sema::query::resolve_trait_impl_obligation(
                    self.ctx,
                    *search_ty,
                    target_trait_norm,
                    impl_id,
                ) else {
                    continue;
                };

                let replace = match selected.as_ref() {
                    None => true,
                    Some((_, _, selected_impl_id, _)) => matches!(
                        kernc_sema::query::compare_impl_specificity(
                            self.ctx,
                            impl_id,
                            *selected_impl_id,
                        ),
                        kernc_sema::query::ImplSpecificity::LeftMoreSpecific
                    ),
                };
                if replace {
                    selected = Some((impl_def.clone(), resolved_impl_args, impl_id, *search_ty));
                }
            }
        }
        selected.map(|(impl_def, resolved_impl_args, _, matched_receiver_ty)| {
            (impl_def, resolved_impl_args, matched_receiver_ty)
        })
    }

    fn vtable_impl_search_types(
        &mut self,
        receiver_ty: TypeId,
        data_ptr_ty: TypeId,
    ) -> Vec<TypeId> {
        let mut search_tys = self.vtable_receiver_search_types(receiver_ty);
        let norm_data_ptr = self.ctx.type_registry.normalize(data_ptr_ty);
        let norm_receiver = self.ctx.type_registry.normalize(receiver_ty);
        if norm_data_ptr != norm_receiver {
            for candidate in self.vtable_receiver_search_types(norm_data_ptr) {
                if !search_tys.contains(&candidate) {
                    search_tys.push(candidate);
                }
            }
        }
        search_tys
    }

    fn vtable_receiver_search_types(&mut self, source_ty: TypeId) -> Vec<TypeId> {
        let mut search_tys = Vec::new();
        let mut current_ty = self.ctx.type_registry.normalize(source_ty);

        loop {
            if !search_tys.contains(&current_ty) {
                search_tys.push(current_ty);
            }

            let Some(next_ty) = self.vtable_downgraded_search_type(current_ty) else {
                break;
            };
            current_ty = self.ctx.type_registry.normalize(next_ty);
        }

        search_tys
    }

    fn vtable_downgraded_search_type(&mut self, source_ty: TypeId) -> Option<TypeId> {
        match self.ctx.type_registry.get(source_ty).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::Pointer { is_mut, elem } => {
                self.vtable_downgraded_search_type(elem).map(|down_elem| {
                    self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut,
                        elem: down_elem,
                    })
                })
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                self.vtable_downgraded_search_type(elem).map(|down_elem| {
                    self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                        is_mut,
                        elem: down_elem,
                    })
                })
            }
            TypeKind::Slice { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }))
            }
            _ => None,
        }
    }

    fn lower_vtable_method_self_arg(
        &mut self,
        data_sym: SymbolId,
        data_ptr_ty: TypeId,
        self_ty: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let data_var = MastExpr::new(void_ptr_ty, MastExprKind::Var(data_sym), span);
        let direct_search_tys = self.vtable_receiver_search_types(data_ptr_ty);
        let self_norm = self.ctx.type_registry.normalize(self_ty);

        if direct_search_tys.contains(&self_norm) {
            return Some(MastExpr::new(
                self_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::Bitcast,
                    operand: Box::new(data_var),
                },
                span,
            ));
        }

        let storage_ptr_ty = match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(data_ptr_ty))
            .clone()
        {
            TypeKind::Pointer { is_mut, .. } => self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut,
                elem: self_ty,
            }),
            TypeKind::VolatilePtr { is_mut, .. } => {
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: self_ty,
                })
            }
            other => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): trait-object data pointer must be pointer-shaped while adapting a vtable method receiver, found {:?}.",
                        other
                    ),
                );
                return None;
            }
        };

        let storage_ptr = MastExpr::new(
            storage_ptr_ty,
            MastExprKind::Cast {
                kind: MastCastKind::Bitcast,
                operand: Box::new(data_var),
            },
            span,
        );
        Some(MastExpr::new(
            self_ty,
            MastExprKind::Deref(Box::new(storage_ptr)),
            span,
        ))
    }

    fn get_or_create_vtable_method_adapter(
        &mut self,
        target_mono_id: MonoId,
        data_ptr_ty: TypeId,
        fn_ty: TypeId,
        span: Span,
    ) -> Option<MonoId> {
        let (params, ret_ty) = self.fn_like_signature(fn_ty, span)?;
        let Some((&self_ty, tail_params)) = params.split_first() else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): trait method signature is missing its receiver while building a vtable adapter.",
            );
            return None;
        };

        let norm_data_ptr = self.ctx.type_registry.normalize(data_ptr_ty);
        let norm_self = self.ctx.type_registry.normalize(self_ty);
        let cache_key = (target_mono_id, norm_data_ptr, norm_self);
        if let Some(&adapter_id) = self.vtable_method_adapter_cache.get(&cache_key) {
            return Some(adapter_id);
        }

        let adapter_id = self.new_mono_id();
        let data_sym = self.fresh_synth_symbol("vtable_data");
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let target_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: params.clone(),
            ret: ret_ty,
            is_variadic: false,
        });

        let mut mast_params = Vec::with_capacity(tail_params.len() + 1);
        mast_params.push(MastParam {
            name: data_sym,
            ty: void_ptr_ty,
            is_mut: false,
        });

        let mut call_args = Vec::with_capacity(params.len());
        call_args.push(self.lower_vtable_method_self_arg(data_sym, data_ptr_ty, self_ty, span)?);

        for &param_ty in tail_params {
            let arg_sym = self.fresh_synth_symbol("vtable_arg");
            mast_params.push(MastParam {
                name: arg_sym,
                ty: param_ty,
                is_mut: false,
            });
            call_args.push(MastExpr::new(param_ty, MastExprKind::Var(arg_sym), span));
        }

        let call_expr = MastExpr::new(
            ret_ty,
            MastExprKind::Call {
                callee: Box::new(MastExpr::new(
                    target_fn_ty,
                    MastExprKind::FuncRef(target_mono_id),
                    span,
                )),
                args: call_args,
            },
            span,
        );

        self.module.functions.push(MastFunction {
            id: adapter_id,
            name: format!("__vtable_method_adapter_{}", adapter_id.0),
            span,
            linkage: MastLinkage::Internal,
            params: mast_params,
            ret_ty,
            body: Some(MastBlock {
                stmts: vec![],
                result: Some(Box::new(call_expr)),
                defers: vec![],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MastInlineHint::None,
            attributes: vec![],
        });

        self.vtable_method_adapter_cache
            .insert(cache_key, adapter_id);
        Some(adapter_id)
    }

    fn build_invalid_vtable(
        &mut self,
        key: (TypeId, TypeId, TypeId),
        data_ptr_ty: TypeId,
        receiver_ty: TypeId,
        trait_ty: TypeId,
        message: String,
    ) -> MonoId {
        self.ctx.struct_error(Span::default(), message).emit();

        if let Some(&existing) = self.vtable_cache.get(&key) {
            return existing;
        }

        let id = self.new_mono_id();
        self.vtable_cache.insert(key, id);

        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let vtable_array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: void_ptr_ty,
            len: self.usize_const_generic(0),
        });

        self.module.globals.push(MastGlobal {
            id,
            name: format!(
                "__vtable_invalid_{}_{}_{}",
                data_ptr_ty.0, receiver_ty.0, trait_ty.0
            ),
            span: Span::default(),
            linkage: MastLinkage::Internal,
            ty: vtable_array_ty,
            is_mut: false,
            init: Some(MastExpr::new(
                vtable_array_ty,
                MastExprKind::ArrayInit(vec![]),
                Span::default(),
            )),
            is_extern: false,
            attributes: vec![],
        });

        id
    }

    fn trait_default_function_args(
        &mut self,
        function_id: kernc_sema::def::DefId,
        actual_trait_ty: TypeId,
        receiver_ty: TypeId,
    ) -> Option<Vec<GenericArg>> {
        let TypeKind::TraitObject(_, trait_args, _) =
            self.ctx.type_registry.get(actual_trait_ty).clone()
        else {
            return None;
        };
        let Def::Function(function) = self.ctx.defs.get(function_id.0 as usize)? else {
            return None;
        };
        let expected_trait_args = function.generics.len().saturating_sub(1);
        let mut args = trait_args;
        args.truncate(expected_trait_args);
        args.push(GenericArg::Type(receiver_ty));
        Some(args)
    }

    pub(crate) fn build_and_inject_vtable_global(&mut self, input: VtableGlobalInput<'_>) {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let mut vtable_entries = Vec::new();

        for super_trait_ty in self.collect_transitive_supertraits(input.actual_trait_ty) {
            let super_vtable_id = self.get_or_create_vtable(
                input.data_ptr_ty,
                input.impl_receiver_ty,
                super_trait_ty,
            );
            match self.vtable_global_void_ptr_expr(super_vtable_id, Span::default()) {
                Some(expr) => vtable_entries.push(expr),
                None => vtable_entries.push(MastExpr::new(
                    void_ptr_ty,
                    MastExprKind::Integer(0),
                    Span::default(),
                )),
            }
        }

        for method in &input.trait_def.methods {
            let mut method_entry = None;

            for &m_id in &input.impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize]
                    && f.name == method.signature.name
                {
                    let method_mono_id =
                        self.instantiate_function_at(m_id, input.impl_args, f.name_span);
                    let method_fn_ty = self
                        .ctx
                        .type_registry
                        .intern(TypeKind::FnDef(m_id, input.impl_args.to_vec()));
                    method_entry = self.get_or_create_vtable_method_adapter(
                        method_mono_id,
                        input.data_ptr_ty,
                        method_fn_ty,
                        Span::default(),
                    );
                    break;
                }
            }

            if method_entry.is_none()
                && let Some(default_id) = method.default_impl
                && let Some(default_args) = self.trait_default_function_args(
                    default_id,
                    input.actual_trait_ty,
                    input.impl_receiver_ty,
                )
            {
                let method_mono_id = self.instantiate_function_at(
                    default_id,
                    &default_args,
                    method.signature.name_span,
                );
                let method_fn_ty = self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(default_id, default_args));
                method_entry = self.get_or_create_vtable_method_adapter(
                    method_mono_id,
                    input.data_ptr_ty,
                    method_fn_ty,
                    Span::default(),
                );
            }

            let m_id = match method_entry {
                Some(id) => id,
                None => {
                    let method_name = self.ctx.resolve(method.signature.name);
                    self.ctx
                        .struct_error(
                            Span::default(),
                            format!(
                                "cannot build a complete vtable because trait method `{}` has no implementation",
                                method_name
                            ),
                        )
                        .emit();
                    vtable_entries.push(MastExpr::new(
                        void_ptr_ty,
                        MastExprKind::Integer(0),
                        Span::default(),
                    ));
                    continue;
                }
            };

            vtable_entries.push(MastExpr::new(
                void_ptr_ty,
                MastExprKind::FuncRef(m_id),
                Span::default(),
            ));
        }

        let vtable_len = vtable_entries.len() as u64;
        let vtable_array_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: void_ptr_ty,
            len: self.usize_const_generic(vtable_len),
        });

        let vtable_init = MastExpr::new(
            vtable_array_ty,
            MastExprKind::ArrayInit(vtable_entries),
            Span::default(),
        );

        self.module.globals.push(MastGlobal {
            id: input.vtable_id,
            name: format!(
                "__vtable_{}_{}_{}",
                input.data_ptr_ty.0, input.receiver_ty.0, input.actual_trait_ty.0
            ),
            span: Span::default(),
            linkage: MastLinkage::Internal,
            ty: vtable_array_ty,
            is_mut: false,
            init: Some(vtable_init),
            is_extern: false,
            attributes: vec![],
        });
    }
}
