use crate::SemaContext;
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use kernc_ast::Visibility;
use kernc_ast::{UsePathKind, UseTarget, UseTree};
use kernc_utils::{Span, SymbolId};

pub struct ImportResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

#[derive(Clone)]
struct FlatImport {
    path: Vec<SymbolId>,
    alias: Option<SymbolId>,
    span: Span,
    binding_span: Span,
}

impl<'a, 'ctx> ImportResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    /// Resolve all imports, repeating until the graph reaches a fixed point.
    pub fn resolve_all(&mut self) {
        let module_ids: Vec<DefId> = self
            .ctx
            .defs
            .iter()
            .filter_map(|def| {
                if let Def::Module(m) = def {
                    Some(m.id)
                } else {
                    None
                }
            })
            .collect();

        let mut pending_imports: Vec<(DefId, ImportDef)> = Vec::new();
        for mod_id in module_ids {
            if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
                for imp in &m.imports {
                    pending_imports.push((mod_id, imp.clone()));
                }
            }
        }

        // Fixed-point iteration handles imports whose dependencies resolve later.
        let mut progress = true;
        while progress && !pending_imports.is_empty() {
            progress = false;
            let mut unresolved = Vec::new();

            for (mod_id, import) in pending_imports {
                // Stay quiet during speculative iterations.
                if self.resolve_single_import(mod_id, &import, false) {
                    progress = true;
                } else {
                    unresolved.push((mod_id, import)); // Defer unresolved imports to the next round.
                }
            }
            pending_imports = unresolved;
        }

        // Run one final pass with diagnostics enabled for anything still unresolved.
        for (mod_id, failed_import) in pending_imports {
            self.resolve_single_import(mod_id, &failed_import, true);
        }
    }

    pub fn binding_names(import: &ImportDef) -> Vec<SymbolId> {
        match &import.target {
            UseTarget::Module(alias) => import
                .path
                .last()
                .copied()
                .map(|last| vec![alias.unwrap_or(last)])
                .unwrap_or_default(),
            UseTarget::Tree(items) => Self::flatten_use_trees(&import.path, items)
                .into_iter()
                .filter_map(|flat| {
                    flat.path
                        .last()
                        .copied()
                        .map(|last| flat.alias.unwrap_or(last))
                })
                .collect(),
        }
    }

    pub fn resolve_import_into_scope(
        &mut self,
        current_mod_id: DefId,
        target_scope: ScopeId,
        import: &ImportDef,
        emit_errors: bool,
    ) -> bool {
        match &import.target {
            UseTarget::Module(alias) => self.resolve_flat_import(
                current_mod_id,
                target_scope,
                import.path_kind,
                import.vis,
                FlatImport {
                    path: import.path.clone(),
                    alias: *alias,
                    span: import.span,
                    binding_span: import.binding_span,
                },
                emit_errors,
            ),
            UseTarget::Tree(items) => {
                let flat_imports = Self::flatten_use_trees(&import.path, items);
                let mut all_resolved = true;
                for flat in flat_imports {
                    if !self.resolve_flat_import(
                        current_mod_id,
                        target_scope,
                        import.path_kind,
                        import.vis,
                        flat,
                        emit_errors,
                    ) {
                        all_resolved = false;
                    }
                }
                all_resolved
            }
        }
    }

    fn resolve_single_import(
        &mut self,
        current_mod_id: DefId,
        import: &ImportDef,
        emit_errors: bool,
    ) -> bool {
        let current_scope = self.get_module_scope(current_mod_id);
        self.resolve_import_into_scope(current_mod_id, current_scope, import, emit_errors)
    }

    fn flatten_use_trees(base_path: &[SymbolId], items: &[UseTree]) -> Vec<FlatImport> {
        let mut flat = Vec::new();
        for item in items {
            Self::flatten_use_tree_item(base_path, item, &mut flat);
        }
        flat
    }

    fn flatten_use_tree_item(base_path: &[SymbolId], item: &UseTree, flat: &mut Vec<FlatImport>) {
        match item {
            UseTree::SelfModule {
                alias,
                span,
                binding_span,
            } => flat.push(FlatImport {
                path: base_path.to_vec(),
                alias: *alias,
                span: *span,
                binding_span: *binding_span,
            }),
            UseTree::Path {
                path,
                alias,
                nested,
                span,
                binding_span,
            } => {
                let mut full_path = base_path.to_vec();
                full_path.extend(path.iter().copied());

                if alias.is_some() || nested.is_none() {
                    flat.push(FlatImport {
                        path: full_path.clone(),
                        alias: *alias,
                        span: *span,
                        binding_span: *binding_span,
                    });
                }

                if let Some(nested) = nested {
                    for child in nested {
                        Self::flatten_use_tree_item(&full_path, child, flat);
                    }
                }
            }
        }
    }

    fn resolve_flat_import(
        &mut self,
        current_mod_id: DefId,
        current_scope: ScopeId,
        kind: UsePathKind,
        vis: Visibility,
        flat: FlatImport,
        emit_errors: bool,
    ) -> bool {
        if flat.path.is_empty() {
            if emit_errors {
                self.ctx
                    .struct_error(flat.span, "imports cannot target an empty path")
                    .emit();
            }
            return false;
        }

        if let Some((mod_id, mod_scope)) =
            self.resolve_path(current_mod_id, kind, &flat.path, flat.span, false)
        {
            let target_name = flat.alias.unwrap_or(*flat.path.last().unwrap());
            let symbol_info = if matches!(kind, UsePathKind::External) && flat.path.len() == 1 {
                self.ctx
                    .scopes
                    .resolve_in(ScopeId(0), flat.path[0])
                    .cloned()
            } else {
                let (parent_path, last_segment) = flat.path.split_at(flat.path.len() - 1);
                let (parent_mod_id, parent_scope) = match self.resolve_path(
                    current_mod_id,
                    kind,
                    parent_path,
                    flat.span,
                    emit_errors,
                ) {
                    Some(res) => res,
                    None => return false,
                };
                let Some(symbol_info) = self
                    .ctx
                    .scopes
                    .resolve_in(parent_scope, last_segment[0])
                    .cloned()
                else {
                    if emit_errors {
                        let name_str = self.ctx.resolve(last_segment[0]).to_string();
                        self.ctx
                            .struct_error(
                                flat.span,
                                format!("Cannot find module or symbol `{}`", name_str),
                            )
                            .emit();
                    }
                    return false;
                };

                if !self.check_visibility(&symbol_info, current_mod_id, parent_mod_id) {
                    if emit_errors {
                        let name_str = self.ctx.resolve(last_segment[0]).to_string();
                        self.ctx
                            .struct_error(
                                flat.span,
                                format!("Symbol `{}` is not visible from this module", name_str),
                            )
                            .emit();
                    }
                    return false;
                }

                Some(symbol_info)
            };

            if let Some(symbol_info) = symbol_info
                && symbol_info.kind == SymbolKind::Module
                && symbol_info.def_id == Some(mod_id)
            {
                self.define_import(
                    current_scope,
                    target_name,
                    symbol_info,
                    vis,
                    flat.binding_span,
                    emit_errors,
                );
                return true;
            }

            let _ = mod_scope;
        }

        let (parent_path, last_segment) = flat.path.split_at(flat.path.len() - 1);
        let target_name = last_segment[0];

        let (parent_mod_id, parent_scope) =
            match self.resolve_path(current_mod_id, kind, parent_path, flat.span, emit_errors) {
                Some(res) => res,
                None => return false,
            };

        if let Some(symbol_info) = self.ctx.scopes.resolve_in(parent_scope, target_name) {
            if !self.check_visibility(symbol_info, current_mod_id, parent_mod_id) {
                if emit_errors {
                    let name_str = self.ctx.resolve(target_name).to_string();
                    self.ctx
                        .struct_error(
                            flat.span,
                            format!("Symbol `{}` is not visible from this module", name_str),
                        )
                        .emit();
                }
                return false;
            }

            let name_to_bind = flat.alias.unwrap_or(target_name);
            self.define_import(
                current_scope,
                name_to_bind,
                symbol_info.clone(),
                vis,
                flat.binding_span,
                emit_errors,
            );
            true
        } else {
            if emit_errors {
                let name_str = self.ctx.resolve(target_name).to_string();
                self.ctx
                    .struct_error(
                        flat.span,
                        format!("Cannot find module or symbol `{}`", name_str),
                    )
                    .emit();
            }
            false
        }
    }

    // ==========================================
    //               Core Resolution
    // ==========================================

    /// Resolve an import path to its target module definition and scope.
    fn resolve_path(
        &mut self,
        current_mod_id: DefId,
        kind: UsePathKind,
        path: &[SymbolId],
        span: Span,
        emit_errors: bool, // Silence speculative failures during fixed-point iteration.
    ) -> Option<(DefId, ScopeId)> {
        let mut actual_path = path;
        let (mut curr_mod_id, mut curr_scope) = match kind {
            UsePathKind::External => {
                if let Some(&first_seg) = actual_path.first() {
                    if let Some(alias_root_id) = self.ctx.alias_root(first_seg) {
                        actual_path = &actual_path[1..];
                        (alias_root_id, self.get_module_scope(alias_root_id))
                    } else {
                        if emit_errors {
                            let name = self.ctx.resolve(first_seg).to_string();
                            self.ctx
                                .struct_error(
                                    span,
                                    format!("Unresolved external import root `{}`", name),
                                )
                                .with_hint("use `/name` for the current package root, or add the package through module mappings")
                                .emit();
                        }
                        return None;
                    }
                } else {
                    if emit_errors {
                        self.ctx
                            .struct_error(span, "external imports cannot be empty")
                            .emit();
                    }
                    return None;
                }
            }
            UsePathKind::Package => {
                let root_id = self.root_module_id(current_mod_id)?;
                (root_id, self.get_module_scope(root_id))
            }
            UsePathKind::Current => {
                // Start from the current module.
                (current_mod_id, self.get_module_scope(current_mod_id))
            }
            UsePathKind::Parent => {
                // Climb directly to the parent module.
                if let Some(module) = self.module_def(current_mod_id).cloned() {
                    if let Some(pid) = module.parent {
                        (pid, self.get_module_scope(pid))
                    } else {
                        if emit_errors {
                            self.ctx
                                .struct_error(span, "Cannot use `..` (Parent) from the root module")
                                .emit();
                        }
                        return None;
                    }
                } else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Imports): DefId {} is not a module while resolving a parent import path.",
                            current_mod_id.0
                        ),
                    );
                    return None;
                }
            }
        };

        // An empty path means the start module itself is the target.
        if actual_path.is_empty() {
            return Some((curr_mod_id, curr_scope));
        }

        // Resolve the remaining path segments normally.
        for &segment in actual_path {
            if let Some(symbol) = self.ctx.scopes.resolve_in(curr_scope, segment) {
                if symbol.kind == SymbolKind::Module
                    && let Some(target_def_id) = symbol.def_id
                {
                    curr_mod_id = target_def_id;
                    curr_scope = self.get_module_scope(target_def_id);
                    continue;
                }

                if emit_errors {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx.struct_error(span, format!("`{}` is not a module", name))
                        .with_hint("only modules can be used in the intermediate segments of an import path")
                        .emit();
                }
                return None;
            } else {
                if emit_errors {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx
                        .struct_error(
                            span,
                            format!("Unresolved import: cannot find module `{}`", name),
                        )
                        .emit();
                }
                return None;
            }
        }

        Some((curr_mod_id, curr_scope))
    }

    /// Check whether a resolved symbol is visible from the importing module.
    fn check_visibility(
        &self,
        symbol_info: &SymbolInfo,
        current_mod: DefId,
        target_mod: DefId,
    ) -> bool {
        // 1. Imports from the same module family are always visible.
        if current_mod == target_mod {
            return true;
        }

        // 2. Respect the symbol's immediate visibility flag in the target scope.
        if !self
            .ctx
            .visibility_allows_access(symbol_info.vis, target_mod, Some(current_mod))
        {
            return false;
        }

        // 3. Cross-module reexports establish a new visibility boundary.
        // If the scope entry resolves to a definition owned by another module,
        // the current scope's binding visibility is the authority.
        let def_id = match symbol_info.def_id {
            Some(id) => id,
            None => return true,
        };
        let owner_module = self.ctx.def_parent_module(def_id).unwrap_or(target_mod);
        if owner_module != target_mod {
            return true;
        }

        // 4. Direct definitions, and same-module aliases of them, still obey
        // the underlying item's declared visibility.
        let def = &self.ctx.defs[def_id.0 as usize];
        let vis = match def {
            Def::Function(d) => d.vis,
            Def::Struct(d) => d.vis,
            Def::Union(d) => d.vis,
            Def::Enum(d) => d.vis,
            Def::Trait(d) => d.vis,
            Def::Global(d) => d.vis,
            Def::TypeAlias(d) => d.vis,
            Def::AssociatedType(_) => return true,
            // Module visibility has already been handled by the scope entry itself.
            Def::Module(_) => Visibility::Public,
            Def::Impl(_) => return true,
        };
        self.ctx
            .visibility_allows_access(vis, owner_module, Some(current_mod))
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn module_def(&self, mod_id: DefId) -> Option<&ModuleDef> {
        match self.ctx.defs.get(mod_id.0 as usize) {
            Some(Def::Module(module)) => Some(module),
            _ => None,
        }
    }

    fn root_module_id(&mut self, start_mod_id: DefId) -> Option<DefId> {
        let mut root_id = start_mod_id;
        loop {
            let Some(module) = self.module_def(root_id).cloned() else {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Imports): DefId {} is not a module while searching for the root module.",
                        root_id.0
                    ),
                );
                return None;
            };

            if let Some(parent) = module.parent {
                root_id = parent;
            } else {
                return Some(root_id);
            }
        }
    }

    fn get_module_scope(&self, mod_id: DefId) -> ScopeId {
        if let Some(module) = self.module_def(mod_id) {
            module.scope_id
        } else {
            ScopeId(0)
        }
    }

    /// Inject a resolved import into the current module scope.
    fn define_import(
        &mut self,
        target_scope: ScopeId,
        name: SymbolId,
        mut info: SymbolInfo,
        vis: Visibility,
        span: Span,
        emit_errors: bool,
    ) {
        info.vis = vis;
        info.span = span;

        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(target_scope);

        if let Err(old_info) = self.ctx.scopes.define(name, info.clone())
            && emit_errors
            && old_info.span != span
        {
            // Allow no-op duplicate imports of the exact same target.
            if old_info.def_id == info.def_id && old_info.kind == info.kind {
                // Nothing to do: the same symbol is already present.
            } else {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("the name `{}` is defined multiple times", name_str),
                    )
                    .with_hint(format!(
                        "`{}` was already imported or defined in this module",
                        name_str
                    ))
                    .with_span_label(old_info.span, "previous definition was here")
                    .emit();
            }
        }

        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
    }
}
