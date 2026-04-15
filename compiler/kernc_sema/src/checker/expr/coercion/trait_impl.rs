use super::*;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let mut visited = FastHashSet::default();
        if self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited) {
            return true;
        }

        if self.check_builtin_auto_trait_impl(source_ty, target_trait_ty) {
            return true;
        }

        // If a mutable pointer or slice lacks a direct impl, try its immutable form.
        let source_norm = self.resolve_tv(source_ty);
        let downgraded = match self.ctx.type_registry.get(source_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::Slice { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }))
            }
            _ => None,
        };

        if let Some(down_ty) = downgraded {
            let mut visited = FastHashSet::default(); // Restart the search with a fresh visited set.
            return self.check_trait_impl_inner(down_ty, target_trait_ty, &mut visited);
        }

        false
    }

    fn check_builtin_auto_trait_impl(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.resolve_tv(source_ty);
        let target_norm = self.resolve_tv(target_trait_ty);

        let TypeKind::TraitObject(trait_def_id, trait_args, _) =
            self.ctx.type_registry.get(target_norm).clone()
        else {
            return false;
        };

        let Some(eq_def_id) = self.ctx.builtin_def("Eq") else {
            return false;
        };
        if trait_def_id == eq_def_id && trait_args.len() == 1 && trait_args[0] == source_norm {
            return match self.ctx.type_registry.get(source_norm).clone() {
                TypeKind::Enum(def_id, _) => {
                    let Def::Enum(def) = &self.ctx.defs[def_id.0 as usize] else {
                        return false;
                    };
                    def.variants
                        .iter()
                        .all(|variant| variant.payload_type.is_none())
                }
                TypeKind::AnonymousEnum(anon) => anon
                    .variants
                    .iter()
                    .all(|variant| variant.payload_ty.is_none()),
                _ => false,
            };
        }

        let simd_elem = self
            .ctx
            .type_registry
            .simd_info(source_norm)
            .map(|(elem, _)| self.resolve_tv(elem));

        let Some(integer_def_id) = self.ctx.builtin_def("Integer") else {
            return false;
        };
        if trait_def_id == integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(elem) if self.ctx.type_registry.is_integer(elem)
            );
        }

        let Some(signed_integer_def_id) = self.ctx.builtin_def("SignedInteger") else {
            return false;
        };
        if trait_def_id == signed_integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(
                    TypeId::I8
                        | TypeId::I16
                        | TypeId::I32
                        | TypeId::I64
                        | TypeId::I128
                        | TypeId::ISIZE
                )
            );
        }

        let Some(unsigned_integer_def_id) = self.ctx.builtin_def("UnsignedInteger") else {
            return false;
        };
        if trait_def_id == unsigned_integer_def_id && trait_args.is_empty() {
            return matches!(
                simd_elem,
                Some(
                    TypeId::U8
                        | TypeId::U16
                        | TypeId::U32
                        | TypeId::U64
                        | TypeId::U128
                        | TypeId::USIZE
                )
            );
        }

        false
    }

    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut FastHashSet<DefId>,
    ) -> bool {
        // === 1. Check active where-bounds from the current environment first ===
        if self.check_trait_impl_in_env_bounds(source_ty, target_trait_ty, visited) {
            return true;
        }

        // === 2. Fall back to globally collected impl blocks ===
        if self.check_trait_impl_in_global_impls(source_ty, target_trait_ty, visited) {
            return true;
        }

        false
    }

    /// Helper 1: check constraints supplied by the current `active_bounds` context.
    fn check_trait_impl_in_env_bounds(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        _visited: &mut FastHashSet<DefId>,
    ) -> bool {
        if self.ctx.active_bounds.is_empty() {
            return false;
        }

        let active_bounds_ptr = std::ptr::from_ref(self.ctx.active_bounds.as_slice());
        let target_norm = self.resolve_tv(target_trait_ty);
        let source_norm = self.resolve_tv(source_ty);
        let mut map = FastHashMap::default();
        // Safety: this helper only reads `active_bounds`; it never resizes or replaces the vec.
        for (env_target, env_bounds) in unsafe { &*active_bounds_ptr } {
            map.clear();

            // If the queried source type matches the contextual target type, inspect its bounds.
            let matched = if *env_target == source_norm {
                true
            } else {
                self.unify(*env_target, source_ty, &mut map)
            };
            if matched {
                if map.is_empty() {
                    for inst_env_bound in env_bounds.iter().copied() {
                        let inst_norm = self.resolve_tv(inst_env_bound);
                        let mut trait_map = FastHashMap::default();

                        if inst_norm == target_norm
                            || inst_env_bound == target_trait_ty
                            || self.unify(target_trait_ty, inst_env_bound, &mut trait_map)
                        {
                            return true;
                        }

                        if matches!(
                            (
                                self.ctx.type_registry.get(inst_norm),
                                self.ctx.type_registry.get(target_norm)
                            ),
                            (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
                        ) && self.is_trait_object_upcast(inst_env_bound, target_trait_ty)
                        {
                            return true;
                        }
                    }
                    continue;
                }

                for bound in env_bounds.iter().copied() {
                    let inst_env_bound = {
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                        subst.substitute(bound)
                    };
                    let inst_norm = self.resolve_tv(inst_env_bound);
                    let mut trait_map = FastHashMap::default();

                    if inst_norm == target_norm
                        || inst_env_bound == target_trait_ty
                        || self.unify(target_trait_ty, inst_env_bound, &mut trait_map)
                    {
                        return true;
                    }

                    if matches!(
                        (
                            self.ctx.type_registry.get(inst_norm),
                            self.ctx.type_registry.get(target_norm)
                        ),
                        (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
                    ) && self.is_trait_object_upcast(inst_env_bound, target_trait_ty)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Helper 2: scan globally registered impl blocks.
    fn check_trait_impl_in_global_impls(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        _visited: &mut FastHashSet<DefId>,
    ) -> bool {
        let target_norm = self.resolve_tv(target_trait_ty);
        let trait_impl_ids_ptr = std::ptr::from_ref(self.ctx.trait_impls.as_slice());
        // Safety: this helper only reads the collected impl id list; it never mutates the vec.
        for impl_id in unsafe { &*trait_impl_ids_ptr }.iter().copied() {
            let Some(impl_ptr) = self
                .ctx
                .defs
                .get(impl_id.0 as usize)
                .and_then(|def| match def {
                    Def::Impl(impl_def) => Some(std::ptr::from_ref(impl_def)),
                    _ => None,
                })
            else {
                continue;
            };

            {
                let mut resolver = TypeResolver::new(self.ctx);
                resolver.ensure_impl_signature_types_resolved(impl_id);
            }

            // Safety: semantic definitions are stable during type queries; use a raw pointer
            // to avoid cloning each impl block on every trait-impl check.
            let impl_def = unsafe { &*impl_ptr };
            let Some(trait_ast) = &impl_def.trait_type else {
                continue;
            };

            let impl_target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let impl_trait_ty = self
                .ctx
                .node_types
                .get(&trait_ast.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                continue;
            }

            let mut map = FastHashMap::default();

            if self.unify(impl_target_ty, source_ty, &mut map) {
                let instantiated_trait_ty = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(impl_trait_ty)
                };

                let inst_norm = self.resolve_tv(instantiated_trait_ty);
                let mut trait_map = FastHashMap::default();

                if inst_norm == target_norm
                    || instantiated_trait_ty == target_trait_ty
                    || self.unify(target_trait_ty, instantiated_trait_ty, &mut trait_map)
                {
                    return true;
                }

                if matches!(
                    (
                        self.ctx.type_registry.get(inst_norm),
                        self.ctx.type_registry.get(target_norm)
                    ),
                    (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
                ) && self.is_trait_object_upcast(instantiated_trait_ty, target_trait_ty)
                {
                    return true;
                }
            }
        }
        false
    }
}
