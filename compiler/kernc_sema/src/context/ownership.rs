use super::*;

#[derive(Clone, Default)]
pub(crate) struct ModuleOwnershipState {
    pub(crate) alias_roots: FastHashMap<SymbolId, DefId>,
    pub(crate) root_module: Option<DefId>,
    pub(crate) root_module_package_names: FastHashMap<DefId, SymbolId>,
    pub(crate) module_defs_by_scope: FastHashMap<ScopeId, DefId>,
    pub(crate) parent_modules_by_def: FastHashMap<DefId, DefId>,
    pub(crate) defs_without_parent_module: FastHashSet<DefId>,
    pub(crate) owner_scopes_by_def: FastHashMap<DefId, ScopeId>,
}

impl<'a> SemaContext<'a> {
    pub fn root_module(&self) -> Option<DefId> {
        self.resolution.module_ownership.root_module
    }

    pub fn set_root_module(&mut self, root_module: Option<DefId>) {
        self.resolution.module_ownership.root_module = root_module;
    }

    pub fn alias_root(&self, alias: SymbolId) -> Option<DefId> {
        self.resolution
            .module_ownership
            .alias_roots
            .get(&alias)
            .copied()
    }

    pub fn register_alias_root(&mut self, alias: SymbolId, module_id: DefId) {
        self.resolution
            .module_ownership
            .alias_roots
            .insert(alias, module_id);
    }

    pub fn register_root_module_package(&mut self, module_id: DefId, package_name: SymbolId) {
        self.resolution
            .module_ownership
            .root_module_package_names
            .insert(module_id, package_name);
    }

    pub fn module_parent(&self, module_id: DefId) -> Option<DefId> {
        match self.defs.get(module_id.0 as usize) {
            Some(Def::Module(module)) => module.parent,
            _ => None,
        }
    }

    pub fn module_is_same_or_descendant_of(
        &self,
        module_id: DefId,
        ancestor_module_id: DefId,
    ) -> bool {
        let mut current = Some(module_id);
        while let Some(module_id) = current {
            if module_id == ancestor_module_id {
                return true;
            }
            current = self.module_parent(module_id);
        }
        false
    }

    pub fn module_root(&self, module_id: DefId) -> DefId {
        let mut current = module_id;
        while let Some(parent) = self.module_parent(current) {
            current = parent;
        }
        current
    }

    pub fn root_module_package_name(&self, module_id: DefId) -> Option<SymbolId> {
        let root = self.module_root(module_id);
        self.resolution
            .module_ownership
            .root_module_package_names
            .get(&root)
            .copied()
    }

    pub fn visibility_allows_access(
        &self,
        vis: Visibility,
        owner_module: DefId,
        current_module: Option<DefId>,
    ) -> bool {
        match vis {
            Visibility::Public => true,
            Visibility::Private => current_module == Some(owner_module),
            Visibility::Super => {
                let Some(current_module) = current_module else {
                    return false;
                };
                let Some(parent_module) = self.module_parent(owner_module) else {
                    return false;
                };
                self.module_is_same_or_descendant_of(current_module, parent_module)
            }
            Visibility::Package => {
                let Some(current_module) = current_module else {
                    return false;
                };
                let current_root = self.module_root(current_module);
                let owner_root = self.module_root(owner_module);
                match (
                    self.resolution
                        .module_ownership
                        .root_module_package_names
                        .get(&current_root),
                    self.resolution
                        .module_ownership
                        .root_module_package_names
                        .get(&owner_root),
                ) {
                    (Some(current_package), Some(owner_package)) => {
                        current_package == owner_package
                    }
                    _ => current_root == owner_root,
                }
            }
        }
    }

    pub fn register_module_scope(&mut self, module_id: DefId, scope_id: ScopeId) {
        self.resolution
            .module_ownership
            .module_defs_by_scope
            .insert(scope_id, module_id);
    }

    pub fn register_def_owner(
        &mut self,
        def_id: DefId,
        parent_module: Option<DefId>,
        owner_scope: Option<ScopeId>,
    ) {
        if let Some(module_id) = parent_module {
            self.resolution
                .module_ownership
                .parent_modules_by_def
                .insert(def_id, module_id);
            self.resolution
                .module_ownership
                .defs_without_parent_module
                .remove(&def_id);
        }
        if let Some(scope_id) = owner_scope {
            self.resolution
                .module_ownership
                .owner_scopes_by_def
                .insert(def_id, scope_id);
        }
    }

    pub fn module_for_scope(&self, scope_id: ScopeId) -> Option<DefId> {
        let mut current = Some(scope_id);
        while let Some(scope_id) = current {
            if let Some(&module_id) = self
                .resolution
                .module_ownership
                .module_defs_by_scope
                .get(&scope_id)
            {
                return Some(module_id);
            }
            current = self.scopes.parent_scope(scope_id);
        }

        self.defs
            .iter()
            .filter_map(|def| {
                let Def::Module(module) = def else {
                    return None;
                };
                self.scopes
                    .distance_to_ancestor(scope_id, module.scope_id)
                    .map(|distance| (module.id, distance))
            })
            .min_by_key(|(_, distance)| *distance)
            .map(|(module_id, _)| module_id)
    }

    pub fn def_parent_module(&self, def_id: DefId) -> Option<DefId> {
        let parent = match self.defs.get(def_id.0 as usize) {
            Some(Def::Module(module)) => module.parent,
            Some(Def::Function(function)) => match function.parent {
                Some(parent_id) => match self.defs.get(parent_id.0 as usize) {
                    Some(Def::Module(_)) => Some(parent_id),
                    Some(Def::Impl(impl_def)) => impl_def.parent_module,
                    _ => None,
                },
                None => None,
            },
            Some(Def::Struct(def)) => def.parent_module,
            Some(Def::Union(def)) => def.parent_module,
            Some(Def::Enum(_)) | Some(Def::Trait(_)) | Some(Def::TypeAlias(_)) => self
                .resolution
                .module_ownership
                .parent_modules_by_def
                .get(&def_id)
                .copied(),
            Some(Def::AssociatedType(def)) => {
                if let Some(parent_impl) = def.parent_impl {
                    match self.defs.get(parent_impl.0 as usize) {
                        Some(Def::Impl(impl_def)) => impl_def.parent_module,
                        _ => None,
                    }
                } else if let Some(parent_trait) = def.parent_trait {
                    self.resolution
                        .module_ownership
                        .parent_modules_by_def
                        .get(&parent_trait)
                        .copied()
                } else {
                    None
                }
            }
            Some(Def::Impl(def)) => def.parent_module,
            Some(Def::Global(global)) => match global.parent {
                Some(parent_id) => match self.defs.get(parent_id.0 as usize) {
                    Some(Def::Module(_)) => Some(parent_id),
                    Some(Def::Impl(impl_def)) => impl_def.parent_module,
                    _ => None,
                },
                None => None,
            },
            None => None,
        };

        if parent.is_some() {
            return parent;
        }
        if self
            .resolution
            .module_ownership
            .defs_without_parent_module
            .contains(&def_id)
        {
            return None;
        }

        parent.or_else(|| {
            self.defs.iter().find_map(|def| match def {
                Def::Module(module) if module.items.contains(&def_id) => Some(module.id),
                _ => None,
            })
        })
    }

    pub fn def_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.resolution
            .module_ownership
            .owner_scopes_by_def
            .get(&def_id)
            .copied()
            .or_else(|| match self.defs.get(def_id.0 as usize) {
                Some(Def::Function(function)) => {
                    let mut current_parent = function.parent;
                    while let Some(parent_id) = current_parent {
                        match self.defs.get(parent_id.0 as usize) {
                            Some(Def::Module(module)) => return Some(module.scope_id),
                            Some(Def::Impl(impl_def)) => current_parent = impl_def.parent_module,
                            _ => return None,
                        }
                    }
                    None
                }
                Some(Def::Global(_))
                | Some(Def::Struct(_))
                | Some(Def::Union(_))
                | Some(Def::Enum(_))
                | Some(Def::Trait(_))
                | Some(Def::AssociatedType(_))
                | Some(Def::TypeAlias(_))
                | Some(Def::Impl(_)) => self.def_parent_module(def_id).and_then(|module_id| {
                    match self.defs.get(module_id.0 as usize) {
                        Some(Def::Module(module)) => Some(module.scope_id),
                        _ => None,
                    }
                }),
                _ => None,
            })
    }
}
