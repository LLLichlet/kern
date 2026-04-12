use super::Lowerer;
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::checker::Substituter;
use kernc_sema::def::{Def, DefId, ImplDef, TraitDef};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};
use std::collections::{HashMap, HashSet};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn collect_transitive_supertraits(&mut self, trait_ty: TypeId) -> Vec<TypeId> {
        let mut supertraits = Vec::new();
        let mut visited = HashSet::new();
        self.collect_transitive_supertraits_inner(
            self.ctx.type_registry.normalize(trait_ty),
            &mut visited,
            &mut supertraits,
        );
        supertraits
    }

    fn collect_transitive_supertraits_inner(
        &mut self,
        trait_ty: TypeId,
        visited: &mut HashSet<TypeId>,
        supertraits: &mut Vec<TypeId>,
    ) {
        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, trait_args) =
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

        let trait_arg_map: HashMap<SymbolId, TypeId> = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();

        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_ty);
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
        let target_norm = self.ctx.type_registry.normalize(target_trait_ty);
        self.collect_transitive_supertraits(trait_ty)
            .iter()
            .position(|&super_ty| super_ty == target_norm)
    }

    pub(crate) fn is_trait_object_upcast(
        &mut self,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.ctx.type_registry.normalize(source_trait_ty);
        let target_norm = self.ctx.type_registry.normalize(target_trait_ty);
        source_norm == target_norm
            || self
                .vtable_supertrait_slot(source_norm, target_norm)
                .is_some()
    }

    pub(crate) fn direct_trait_method_slot(
        &mut self,
        trait_ty: TypeId,
        method_name: SymbolId,
    ) -> Option<usize> {
        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(trait_def_id, _) = self.ctx.type_registry.get(trait_norm).clone()
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
            .position(|method| method.name == method_name)?;
        Some(self.collect_transitive_supertraits(trait_norm).len() + direct_idx)
    }

    pub(crate) fn get_or_create_vtable(&mut self, source_ty: TypeId, trait_ty: TypeId) -> MonoId {
        let norm_source = self.ctx.type_registry.normalize(source_ty);
        let norm_trait = self.ctx.type_registry.normalize(trait_ty);
        let key = (norm_source, norm_trait);
        if let Some(&id) = self.vtable_cache.get(&key) {
            return id;
        }
        self.measure_phase("  lower_create_vtable", |this| {
            let trait_def_id = match this.ctx.type_registry.get(norm_trait) {
                TypeKind::TraitObject(id, _) => *id,
                other => {
                    return this.build_invalid_vtable(
                        key,
                        source_ty,
                        trait_ty,
                        format!(
                            "Kern ICE (Lowering): Target must be a TraitObject, found: {:?}",
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
                    source_ty,
                    trait_ty,
                    format!(
                        "Kern ICE (Lowering): DefId {} is not a Trait!",
                        trait_def_id.0
                    ),
                );
            };

            let (base_source_ty, source_args) = this.resolve_vtable_source_base(source_ty);

            let impl_def = match this.find_matching_impl_block(base_source_ty, trait_def_id) {
                Some(def) => def,
                None => {
                    let src_name = this.ctx.ty_to_string(base_source_ty);
                    let trait_name = this.ctx.resolve(trait_def.name);
                    return this.build_invalid_vtable(
                        key,
                        source_ty,
                        trait_ty,
                        format!(
                            "Kern ICE (Lowering): Impl block missing for cast `{} as {}`. Sema failed to enforce Trait bounding contract.",
                            src_name, trait_name
                        ),
                    );
                }
            };

            let vtable_id = this.new_mono_id();
            this.vtable_cache.insert(key, vtable_id);

            this.build_and_inject_vtable_global(
                vtable_id,
                source_ty,
                norm_trait,
                &trait_def,
                &impl_def,
                &source_args,
            );

            vtable_id
        })
    }

    pub(crate) fn resolve_vtable_source_base(&self, source_ty: TypeId) -> (TypeId, Vec<TypeId>) {
        let mut base_ty = source_ty;
        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                }
                _ => {
                    base_ty = norm;
                    break;
                }
            }
        }

        let source_args = match self.ctx.type_registry.get(base_ty) {
            TypeKind::Def(_, args) | TypeKind::Enum(_, args) => args.clone(),
            _ => Vec::new(),
        };

        (base_ty, source_args)
    }

    pub(crate) fn find_matching_impl_block(
        &self,
        base_source_ty: TypeId,
        target_trait_id: DefId,
    ) -> Option<ImplDef> {
        let get_base_def_id = |ty: TypeId| -> Option<DefId> {
            let norm = self.ctx.type_registry.normalize(ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Def(id, _) | TypeKind::Enum(id, _) => Some(*id),
                _ => None,
            }
        };

        let src_base_id = get_base_def_id(base_source_ty);
        let norm_src_base = self.ctx.type_registry.normalize(base_source_ty);

        for &impl_id in &self.ctx.global_impls {
            if let Def::Impl(impl_def) = &self.ctx.defs[impl_id.0 as usize]
                && let Some(impl_trait_node) = &impl_def.trait_type
            {
                let i_trait_ty = self
                    .ctx
                    .node_types
                    .get(&impl_trait_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if let TypeKind::TraitObject(i_trait_id, _) = self.ctx.type_registry.get(i_trait_ty)
                    && *i_trait_id == target_trait_id
                {
                    let i_target_ty = self
                        .ctx
                        .node_types
                        .get(&impl_def.target_type.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let (i_target_base, _) = self.resolve_vtable_source_base(i_target_ty);

                    if let (Some(target_id), Some(src_id)) =
                        (get_base_def_id(i_target_base), src_base_id)
                    {
                        if target_id == src_id {
                            return Some(impl_def.clone());
                        }
                    } else if self.ctx.type_registry.normalize(i_target_base) == norm_src_base {
                        return Some(impl_def.clone());
                    }
                }
            }
        }
        None
    }

    fn build_invalid_vtable(
        &mut self,
        key: (TypeId, TypeId),
        source_ty: TypeId,
        trait_ty: TypeId,
        message: String,
    ) -> MonoId {
        self.ctx.emit_ice(Span::default(), message);

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
            is_mut: false,
            elem: void_ptr_ty,
            len: 0,
        });

        self.module.globals.push(MastGlobal {
            id,
            name: format!("__vtable_invalid_{}_{}", source_ty.0, trait_ty.0),
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

    pub(crate) fn build_and_inject_vtable_global(
        &mut self,
        vtable_id: MonoId,
        source_ty: TypeId,
        actual_trait_ty: TypeId,
        trait_def: &TraitDef,
        impl_def: &ImplDef,
        source_args: &[TypeId],
    ) {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let mut vtable_entries = Vec::new();

        for super_trait_ty in self.collect_transitive_supertraits(actual_trait_ty) {
            let super_vtable_id = self.get_or_create_vtable(source_ty, super_trait_ty);
            match self.vtable_global_void_ptr_expr(super_vtable_id, Span::default()) {
                Some(expr) => vtable_entries.push(expr),
                None => vtable_entries.push(MastExpr::new(
                    void_ptr_ty,
                    MastExprKind::Integer(0),
                    Span::default(),
                )),
            }
        }

        for method in &trait_def.methods {
            let mut method_mono_id = None;

            for &m_id in &impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize]
                    && f.name == method.name
                {
                    method_mono_id = Some(self.instantiate_function(m_id, source_args));
                    break;
                }
            }

            let m_id = match method_mono_id {
                Some(id) => id,
                None => {
                    let method_name = self.ctx.resolve(method.name);
                    self.ctx.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Lowering): Missing implementation for trait method `{}`. Sema failed to check trait completeness.",
                            method_name
                        ),
                    );
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
            is_mut: false,
            elem: void_ptr_ty,
            len: vtable_len,
        });

        let vtable_init = MastExpr::new(
            vtable_array_ty,
            MastExprKind::ArrayInit(vtable_entries),
            Span::default(),
        );

        self.module.globals.push(MastGlobal {
            id: vtable_id,
            name: format!("__vtable_{}_{}", source_ty.0, actual_trait_ty.0),
            linkage: MastLinkage::Internal,
            ty: vtable_array_ty,
            is_mut: false,
            init: Some(vtable_init),
            is_extern: false,
            attributes: vec![],
        });
    }
}
