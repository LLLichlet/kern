use kernc_utils::AtomicOrdering;
use kernc_utils::config::RuntimeEntry;
use kernc_utils::{DiagnosticBuilder, FastHashMap, FastHashSet, NodeId, Session, Span, SymbolId};
use std::collections::HashMap;

use crate::checker::ExprChecker;
use crate::def::{Def, DefId, DefTable, ImplDef};
use crate::passes::TypeResolver;
use crate::scope::{ScopeId, SymbolTable};
use crate::semantic::{SemanticDefinition, SemanticSymbolKind};
use crate::ty::{GenericArg, Substituter, TypeFormatter, TypeId, TypeKind, TypeRegistry};
use kernc_ast::Visibility;
use kernc_middle::NodeFacts;

mod impl_requirements;
mod ownership;
mod projection_normalization;
mod semantic_index;
mod snapshot;
mod state;
mod symbol_mangling;

pub use snapshot::SemaStructureSnapshot;
pub(crate) use state::{EscapeSummary, PendingEscapeCheck, RecursiveReportState};
pub use state::{ExprTimingStats, SemaAnalysisState, SemaImplIndexState, SemaResolutionState};

pub struct SemaContext<'a> {
    // 1. Shared compiler services.
    pub sess: &'a mut Session,

    // 2. Type-system state.
    pub type_registry: TypeRegistry,
    // Final inferred type for each AST node and other per-node semantic facts.
    facts: NodeFacts,

    // 3. Symbol and scope state.
    pub defs: DefTable,
    pub scopes: SymbolTable,
    impl_index: SemaImplIndexState,

    // 4. Module and package resolution state.
    pub resolution: SemaResolutionState,
    // 5. Analysis-time caches, timings, and semantic indexes.
    pub(crate) analysis: SemaAnalysisState,
}

#[derive(Debug, Clone)]
pub struct IndexedImplDef {
    pub id: DefId,
    pub def: ImplDef,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexedImplMethod {
    pub method_id: DefId,
    pub impl_id: DefId,
    pub name_span: Span,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SelfReferentialImplRequirement {
    pub bound_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
}

#[derive(Debug, Clone)]
pub(crate) struct ImplRequirementCycle {
    pub start_bound_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
    pub requirements: Vec<ImplRequirementEdge>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ImplRequirementEdge {
    pub impl_id: DefId,
    pub requirement_span: Span,
    pub target_ty: TypeId,
    pub trait_ty: TypeId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PatersonParam {
    Type(SymbolId),
    Const(SymbolId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PatersonBoundednessIssue {
    ConstructorCount {
        head: usize,
        requirement: usize,
    },
    VariableCount {
        param: PatersonParam,
        head: usize,
        requirement: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PatersonMeasure {
    pub constructors: usize,
    pub params: FastHashMap<PatersonParam, usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct NonDecreasingImplRequirement {
    pub bound_span: Span,
    pub head_target_ty: TypeId,
    pub head_trait_ty: TypeId,
    pub requirement_target_ty: TypeId,
    pub requirement_trait_ty: TypeId,
    pub issue: PatersonBoundednessIssue,
}

impl<'a> SemaContext<'a> {
    /// Build semantic-analysis state around an existing session.
    pub fn new(sess: &'a mut Session) -> Self {
        Self {
            sess,
            type_registry: TypeRegistry::new(),
            facts: NodeFacts::default(),
            defs: DefTable::new(),
            scopes: SymbolTable::new(),
            impl_index: SemaImplIndexState::default(),
            resolution: SemaResolutionState::default(),
            analysis: SemaAnalysisState::default(),
        }
    }

    // ==========================================
    // Core operations
    // ==========================================

    pub fn add_def(&mut self, def: Def) -> DefId {
        let id = self.defs.add(def);
        self.resolution
            .module_ownership
            .defs_without_parent_module
            .insert(id);
        id
    }

    pub fn add_def_with(&mut self, build: impl FnOnce(DefId) -> Def) -> DefId {
        let id = self.defs.next_id();
        self.add_def(build(id))
    }

    pub fn collects_timings(&self) -> bool {
        self.sess.report_timings
    }

    pub fn ty_to_string(&self, ty: TypeId) -> String {
        TypeFormatter { ctx: self }.format(ty)
    }

    pub fn structure_snapshot(&self) -> SemaStructureSnapshot {
        SemaStructureSnapshot {
            type_registry: self.type_registry.clone(),
            facts: self.facts.clone(),
            defs: self.defs.clone(),
            scopes: self.scopes.clone(),
            resolution: self.resolution.clone(),
            impl_index: self.impl_index.clone(),
            semantic_index: self.analysis.semantic_index.clone(),
            recursive_reports: self.analysis.recursive_reports.clone(),
        }
    }

    pub fn into_structure_snapshot(self) -> SemaStructureSnapshot {
        SemaStructureSnapshot {
            type_registry: self.type_registry,
            facts: self.facts,
            defs: self.defs,
            scopes: self.scopes,
            resolution: self.resolution,
            impl_index: self.impl_index,
            semantic_index: self.analysis.semantic_index,
            recursive_reports: self.analysis.recursive_reports,
        }
    }

    pub fn restore_structure(&mut self, snapshot: SemaStructureSnapshot) {
        // Restoring a structural snapshot rewinds the semantic graph itself. Any traversal-local
        // state derived from that graph, such as active bounds, query caches, timings, and the
        // semantic index, must be reset alongside the core tables.
        self.type_registry = snapshot.type_registry;
        self.facts = snapshot.facts;
        self.analysis.active_bounds.clear();
        self.clear_active_bound_caches();
        self.defs = snapshot.defs;
        self.scopes = snapshot.scopes;
        self.impl_index = snapshot.impl_index;
        self.resolution = snapshot.resolution;
        self.analysis.semantic_index = snapshot.semantic_index;
        self.analysis.recursive_reports = snapshot.recursive_reports;
        self.analysis.expr_timing_stats = ExprTimingStats::default();
        self.analysis.query_caches.clear_all();
        self.clear_active_bound_caches();
        self.analysis.escape_summaries.clear();
        self.analysis.pending_escape_checks.clear();
    }

    pub fn clear_active_bound_caches(&mut self) {
        // Bound-dependent queries become stale whenever we enter or leave a generic environment.
        self.analysis.query_caches.clear_active_bound_caches();
    }

    // ==========================================
    // Semantic fact access
    // ==========================================

    /// Centralize per-node semantic facts so algorithm-heavy code does not depend on how
    /// `SemaContext` stores those tables internally.
    pub fn node_type(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts.node_types.get(&node_id).copied()
    }

    pub fn node_type_or_error(&self, node_id: NodeId) -> TypeId {
        self.node_type(node_id).unwrap_or(TypeId::ERROR)
    }

    pub fn normalized_node_type_or_error(&self, node_id: NodeId) -> TypeId {
        self.type_registry
            .normalize(self.node_type_or_error(node_id))
    }

    pub fn set_node_type(&mut self, node_id: NodeId, ty: TypeId) {
        self.facts.node_types.insert(node_id, ty);
    }

    pub fn has_node_type(&self, node_id: NodeId) -> bool {
        self.facts.node_types.contains_key(&node_id)
    }

    pub fn remove_node_type(&mut self, node_id: NodeId) {
        self.facts.node_types.remove(&node_id);
    }

    pub(crate) fn node_types_snapshot(&self) -> FastHashMap<NodeId, TypeId> {
        self.facts.node_types.clone()
    }

    pub(crate) fn restore_node_types(&mut self, node_types: FastHashMap<NodeId, TypeId>) {
        self.facts.node_types = node_types;
    }

    pub(crate) fn node_facts_snapshot(&self) -> NodeFacts {
        self.facts.clone()
    }

    pub(crate) fn restore_node_facts(&mut self, facts: NodeFacts) {
        self.facts = facts;
    }

    pub fn atomic_ordering(&self, node_id: NodeId) -> Option<AtomicOrdering> {
        self.facts.atomic_orderings.get(&node_id).copied()
    }

    pub fn set_atomic_ordering(&mut self, node_id: NodeId, ordering: AtomicOrdering) {
        self.facts.atomic_orderings.insert(node_id, ordering);
    }

    pub fn method_owner_ty(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts.method_owner_tys.get(&node_id).copied()
    }

    pub fn set_method_owner_ty(&mut self, node_id: NodeId, owner_ty: TypeId) {
        self.facts.method_owner_tys.insert(node_id, owner_ty);
    }

    pub fn set_call_arg_expected_ty(&mut self, node_id: NodeId, expected_ty: TypeId) {
        self.facts
            .call_arg_expected_tys
            .insert(node_id, expected_ty);
    }

    pub fn call_arg_expected_ty(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts.call_arg_expected_tys.get(&node_id).copied()
    }

    pub fn set_binary_operator_lhs_trait_self_ty(&mut self, node_id: NodeId, self_ty: TypeId) {
        self.facts
            .binary_operator_lhs_trait_self_tys
            .insert(node_id, self_ty);
    }

    pub fn binary_operator_lhs_trait_self_ty(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts
            .binary_operator_lhs_trait_self_tys
            .get(&node_id)
            .copied()
    }

    pub fn set_binary_operator_rhs_trait_arg_ty(&mut self, node_id: NodeId, arg_ty: TypeId) {
        self.facts
            .binary_operator_rhs_trait_arg_tys
            .insert(node_id, arg_ty);
    }

    pub fn binary_operator_rhs_trait_arg_ty(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts
            .binary_operator_rhs_trait_arg_tys
            .get(&node_id)
            .copied()
    }

    pub fn set_match_value_pattern_bind_ty(&mut self, pattern_id: NodeId, bind_ty: TypeId) {
        self.facts
            .match_value_pattern_bind_tys
            .insert(pattern_id, bind_ty);
    }

    pub fn match_value_pattern_bind_ty(&self, pattern_id: NodeId) -> Option<TypeId> {
        self.facts
            .match_value_pattern_bind_tys
            .get(&pattern_id)
            .copied()
    }

    pub fn global_impl_entries(&self) -> Vec<IndexedImplDef> {
        self.impl_index
            .global_impls
            .iter()
            .filter_map(|&id| match self.defs.get(id.0 as usize) {
                Some(Def::Impl(def)) => Some(IndexedImplDef {
                    id,
                    def: def.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    pub fn trait_impl_entries(&self) -> Vec<IndexedImplDef> {
        self.impl_index
            .trait_impls
            .iter()
            .filter_map(|&id| match self.defs.get(id.0 as usize) {
                Some(Def::Impl(def)) => Some(IndexedImplDef {
                    id,
                    def: def.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    pub fn impl_methods_named(&self, name: SymbolId) -> Vec<IndexedImplMethod> {
        let Some(method_ids) = self.impl_index.impl_methods_by_name.get(&name) else {
            return Vec::new();
        };

        method_ids
            .iter()
            .filter_map(|&method_id| match self.defs.get(method_id.0 as usize) {
                Some(Def::Function(function)) => function.parent.map(|impl_id| IndexedImplMethod {
                    method_id,
                    impl_id,
                    name_span: function.name_span,
                }),
                _ => None,
            })
            .collect()
    }

    pub fn register_global_impl(&mut self, impl_id: DefId) {
        self.impl_index.global_impls.push(impl_id);
    }

    pub fn register_trait_impl(&mut self, impl_id: DefId) {
        self.impl_index.trait_impls.push(impl_id);
    }

    pub fn register_impl_method(&mut self, name: SymbolId, method_id: DefId) {
        self.impl_index
            .impl_methods_by_name
            .entry(name)
            .or_default()
            .push(method_id);
    }

    pub(crate) fn set_trait_impl_groups(&mut self, groups: FastHashMap<String, Vec<DefId>>) {
        self.impl_index.trait_impls_by_trait_key = groups;
    }

    pub(crate) fn trait_impl_groups(&self) -> &FastHashMap<String, Vec<DefId>> {
        &self.impl_index.trait_impls_by_trait_key
    }

    pub(crate) fn trait_def_lookup_key(&self, trait_def_id: DefId) -> Option<String> {
        let trait_name = match self.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => self.resolve(trait_def.name).to_string(),
            _ => return None,
        };

        let mut components = vec![trait_name];
        let mut current_module = self.def_parent_module(trait_def_id);

        while let Some(module_id) = current_module {
            let Def::Module(module_def) = &self.defs[module_id.0 as usize] else {
                break;
            };
            if module_def.parent.is_none()
                && let Some(package_name) = self.root_module_package_name(module_id)
            {
                components.push(self.resolve(package_name).to_string());
            } else {
                components.push(self.resolve(module_def.name).to_string());
            }
            current_module = module_def.parent;
        }

        components.reverse();
        Some(components.join("."))
    }

    pub fn trait_impl_ids_for_trait(&self, trait_def_id: DefId) -> Vec<DefId> {
        if self.impl_index.trait_impls_by_trait_key.is_empty() {
            return self.impl_index.trait_impls.clone();
        }

        let Some(trait_key) = self.trait_def_lookup_key(trait_def_id) else {
            return Vec::new();
        };
        if let Some(impl_ids) = self.impl_index.trait_impls_by_trait_key.get(&trait_key) {
            return impl_ids.clone();
        }

        self.impl_index
            .trait_impls
            .iter()
            .copied()
            .filter(|impl_id| {
                let Some(Def::Impl(impl_def)) = self.defs.get(impl_id.0 as usize) else {
                    return false;
                };
                let Some(trait_ty) = impl_def
                    .trait_type
                    .as_ref()
                    .and_then(|trait_ty| self.facts.node_types.get(&trait_ty.id).copied())
                else {
                    return false;
                };
                let TypeKind::TraitObject(candidate_trait_def_id, _, _) = self
                    .type_registry
                    .get(self.type_registry.normalize(trait_ty))
                else {
                    return false;
                };
                self.trait_def_lookup_key(*candidate_trait_def_id)
                    .is_some_and(|candidate_key| candidate_key == trait_key)
            })
            .collect()
    }

    /// Inject CLI-provided module aliases such as `std` into the root scope.
    /// This lets code refer to `std.io` directly without an explicit `use std;`.
    pub fn inject_alias_roots(&mut self) {
        // Save and restore the current scope around the root-scope injection.
        let prev_scope = self.scopes.current_scope_id();

        // Scope 0 is the global builtin scope created by the symbol table.
        let global_scope = ScopeId(0);
        self.scopes.set_current_scope(global_scope);

        // Clone aliases up front to avoid borrow conflicts with `scopes`.
        let aliases: Vec<(SymbolId, DefId)> = self
            .resolution
            .module_ownership
            .alias_roots
            .iter()
            .map(|(&name, &mod_id)| (name, mod_id))
            .collect();

        let node_id = self.next_node_id();
        for (name, mod_id) in aliases {
            let info = crate::scope::SymbolInfo {
                kind: crate::scope::SymbolKind::Module,
                node_id,
                type_id: TypeId::ERROR,
                def_id: Some(mod_id),
                span: kernc_utils::Span::default(),
                vis: Visibility::Public,
                is_mut: false,
            };

            // Ignore duplicates here; later collection reports real conflicts.
            let _ = self.scopes.define(name, info);
        }

        // Restore the caller's scope.
        if let Some(prev) = prev_scope {
            self.scopes.set_current_scope(prev);
        }
    }

    // ==========================================
    // Convenience forwarders
    // ==========================================

    pub fn has_errors(&self) -> bool {
        self.sess.has_errors()
    }

    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_error(span, msg)
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_warning(span, msg)
    }

    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_error(span, msg);
    }

    pub fn emit_warning(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_warning(span, msg.into());
    }

    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_ice(span, msg);
    }

    pub fn next_node_id(&mut self) -> NodeId {
        self.sess.next_node_id()
    }

    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.sess.interner.intern(string)
    }

    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }

    pub fn register_builtin_def(&mut self, name: SymbolId, def_id: DefId) {
        self.resolution.builtin_defs.insert(name, def_id);
    }

    pub fn builtin_def(&mut self, name: &str) -> Option<DefId> {
        let symbol = self.intern(name);
        self.resolution.builtin_defs.get(&symbol).copied()
    }

    pub fn builtin_trait_ty(&mut self, name: &str, args: Vec<TypeId>) -> Option<TypeId> {
        let def_id = self.builtin_def(name)?;
        Some(self.type_registry.intern(crate::ty::TypeKind::TraitObject(
            def_id,
            crate::ty::wrap_type_args(args),
            Vec::new(),
        )))
    }

    pub fn builtin_trait_ty_with_assoc(
        &mut self,
        name: &str,
        generics: Vec<TypeId>,
        assoc_bindings: Vec<(&str, TypeId)>,
    ) -> Option<TypeId> {
        let def_id = self.builtin_def(name)?;
        let resolved_assoc_bindings = match self.defs.get(def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => assoc_bindings
                .into_iter()
                .filter_map(|(assoc_name, ty)| {
                    trait_def.assoc_types.iter().copied().find_map(|assoc_id| {
                        let assoc_def = match self.defs.get(assoc_id.0 as usize) {
                            Some(Def::AssociatedType(assoc_def)) => assoc_def,
                            _ => return None,
                        };
                        if self.resolve(assoc_def.name) == assoc_name {
                            Some((assoc_id, ty))
                        } else {
                            None
                        }
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        Some(self.type_registry.intern(crate::ty::TypeKind::TraitObject(
            def_id,
            crate::ty::wrap_type_args(generics),
            resolved_assoc_bindings,
        )))
    }

    pub fn program_entry_enabled(&self) -> bool {
        !matches!(self.sess.runtime_entry, RuntimeEntry::None)
    }

    pub fn test_mode_enabled(&self) -> bool {
        self.sess.test_mode
    }

    pub fn main_argv_ptr_ty(&mut self) -> TypeId {
        let ptr_u8 = self.type_registry.intern(crate::ty::TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        self.type_registry.intern(crate::ty::TypeKind::Pointer {
            is_mut: false,
            elem: ptr_u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::SemaContext;
    use crate::def::{Def, DefId, FunctionDef, ModuleDef, StructDef, Visibility};
    use crate::scope::ScopeId;
    use crate::ty::TypeKind;
    use kernc_ast::{Attribute, TypeNode};
    use kernc_utils::{FileId, Session, Span};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn mangled_named_types_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_seen = add_struct(&mut ctx, "SeenItem", Some(left_id));
        let right_seen = add_struct(&mut ctx, "SeenItem", Some(right_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::Def(left_seen, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::Def(right_seen, Vec::new()));

        assert_ne!(ctx.mangle_type(left_ty), ctx.mangle_type(right_ty));
    }

    #[test]
    fn generic_export_names_distinguish_same_short_name_types() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_seen = add_struct(&mut ctx, "SeenItem", Some(left_id));
        let right_seen = add_struct(&mut ctx, "SeenItem", Some(right_id));
        let parse_id = add_function(&mut ctx, "parse", Some(root_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::Def(left_seen, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::Def(right_seen, Vec::new()));

        assert_ne!(
            ctx.get_export_name(parse_id, &[left_ty]),
            ctx.get_export_name(parse_id, &[right_ty])
        );
    }

    #[test]
    fn exported_named_types_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_error = add_struct(&mut ctx, "Error", Some(left_id));
        let right_error = add_struct(&mut ctx, "Error", Some(right_id));

        assert_ne!(
            ctx.get_export_name(left_error, &[]),
            ctx.get_export_name(right_error, &[])
        );
    }

    #[test]
    fn mangled_fn_defs_include_module_qualification() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let root_id = add_module(&mut ctx, "root", None);
        let left_id = add_module(&mut ctx, "left", Some(root_id));
        let right_id = add_module(&mut ctx, "right", Some(root_id));
        let left_parse = add_function(&mut ctx, "parse", Some(left_id));
        let right_parse = add_function(&mut ctx, "parse", Some(right_id));

        let left_ty = ctx
            .type_registry
            .intern(TypeKind::FnDef(left_parse, Vec::new()));
        let right_ty = ctx
            .type_registry
            .intern(TypeKind::FnDef(right_parse, Vec::new()));

        assert_ne!(ctx.mangle_type(left_ty), ctx.mangle_type(right_ty));
    }

    #[test]
    fn package_visibility_allows_same_package_across_module_roots() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let app_root = add_module(&mut ctx, "app", None);
        let dep_root = add_module(&mut ctx, "dep", None);
        let dep_inner = add_module(&mut ctx, "inner", Some(dep_root));
        let package = ctx.intern("bed");
        ctx.register_root_module_package(app_root, package);
        ctx.register_root_module_package(dep_root, package);

        assert!(ctx.visibility_allows_access(Visibility::Package, dep_inner, Some(app_root)));
    }

    #[test]
    fn package_visibility_denies_different_packages_across_module_roots() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);

        let app_root = add_module(&mut ctx, "app", None);
        let dep_root = add_module(&mut ctx, "dep", None);
        let dep_inner = add_module(&mut ctx, "inner", Some(dep_root));
        let app_package = ctx.intern("app");
        let dep_package = ctx.intern("dep");
        ctx.register_root_module_package(app_root, app_package);
        ctx.register_root_module_package(dep_root, dep_package);

        assert!(!ctx.visibility_allows_access(Visibility::Package, dep_inner, Some(app_root)));
    }

    fn add_module(ctx: &mut SemaContext<'_>, name: &str, parent: Option<DefId>) -> DefId {
        let scope_id = ScopeId(ctx.defs.next_id().0 as usize);
        let name = ctx.intern(name);
        let def_id = ctx.add_def_with(|id| {
            Def::Module(ModuleDef {
                id,
                name,
                parent,
                is_imported: false,
                scope_id,
                dir_path: PathBuf::new(),
                file_id: FileId(0),
                submodules: HashMap::new(),
                items: Vec::new(),
                imports: Vec::new(),
                is_init: parent.is_none(),
                docs: None,
            })
        });
        ctx.register_module_scope(def_id, scope_id);
        def_id
    }

    fn add_struct(ctx: &mut SemaContext<'_>, name: &str, parent_module: Option<DefId>) -> DefId {
        let name = ctx.intern(name);
        let def_id = ctx.add_def_with(|id| {
            Def::Struct(StructDef {
                id,
                name,
                name_span: Span::default(),
                vis: Visibility::Private,
                parent_module,
                is_imported: false,
                generics: Vec::new(),
                where_clauses: Vec::new(),
                fields: Vec::new(),
                is_extern: false,
                span: Span::default(),
                docs: None,
                attributes: Vec::new(),
            })
        });
        ctx.register_def_owner(def_id, parent_module, None);
        def_id
    }

    fn add_function(ctx: &mut SemaContext<'_>, name: &str, parent: Option<DefId>) -> DefId {
        let name = ctx.intern(name);
        let type_node = TypeNode {
            id: ctx.next_node_id(),
            span: Span::default(),
            kind: kernc_ast::TypeKind::Infer,
        };
        let def_id = ctx.add_def_with(|id| {
            Def::Function(FunctionDef {
                id,
                name,
                name_span: Span::default(),
                vis: Visibility::Private,
                parent,
                default_trait_method: None,
                is_imported: false,
                generics: Vec::new(),
                where_clauses: Vec::new(),
                params: Vec::new(),
                ret_type: type_node,
                body: None,
                is_const: false,
                is_extern: false,
                is_variadic: false,
                is_intrinsic: false,
                span: Span::default(),
                resolved_sig: None,
                docs: None,
                attributes: Vec::<Attribute>::new(),
            })
        });
        ctx.register_def_owner(def_id, None, None);
        def_id
    }
}
