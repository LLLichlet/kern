use kernc_utils::AtomicOrdering;
use kernc_utils::config::RuntimeEntry;
use kernc_utils::{
    DiagnosticBuilder, DiagnosticLevel, FastHashMap, FastHashSet, FileId, NodeId, Session, Span,
    SymbolId,
};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use crate::checker::{ExprChecker, Substituter};
use crate::def::{Def, DefId, ImplDef};
use crate::passes::TypeResolver;
use crate::scope::{ScopeId, SymbolTable};
use crate::semantic::{SemanticDefinition, SemanticSymbolKind};
use crate::ty::{GenericArg, TypeFormatter, TypeId, TypeKind, TypeRegistry};
use kernc_ast::Visibility;

mod impl_requirements;
mod ownership;
mod projection_normalization;
mod semantic_index;
mod symbol_mangling;

use ownership::ModuleOwnershipState;
use semantic_index::SemanticIndexState;

type NamedFieldQueryKey = (Option<DefId>, DefId, Vec<GenericArg>, SymbolId);
type NamedFieldQueryValue = Option<crate::query::MemberCandidate>;
type MemberResolutionQueryKey = (Option<DefId>, TypeId, SymbolId);

#[derive(Clone, Default)]
pub(crate) struct SemaQueryCacheState {
    // These caches only store facts derivable from the current semantic graph. Any structural
    // rollback must invalidate them so later passes do not observe stale trait or member results.
    pub(crate) call_signature_instantiation_cache: FastHashMap<TypeId, TypeId>,
    pub(crate) field_type_subst_cache: FastHashMap<(NodeId, Vec<GenericArg>), TypeId>,
    pub(crate) trait_method_query_cache:
        FastHashMap<(TypeId, SymbolId, TypeId), crate::query::MemberResolution>,
    pub(crate) impl_method_query_cache:
        FastHashMap<(TypeId, SymbolId), Option<crate::query::MemberCandidate>>,
    pub(crate) bound_trait_match_cache: FastHashMap<TypeId, Vec<TypeId>>,
    pub(crate) impl_applicability_cache:
        FastHashMap<(TypeId, DefId), Option<Vec<crate::ty::GenericArg>>>,
    pub(crate) impl_requirement_cycle_cache: FastHashMap<DefId, Option<ImplRequirementCycle>>,
    pub(crate) impl_paterson_boundedness_cache:
        FastHashMap<DefId, Option<NonDecreasingImplRequirement>>,
    pub(crate) named_field_query_cache: FastHashMap<NamedFieldQueryKey, NamedFieldQueryValue>,
    pub(crate) member_resolution_query_cache:
        FastHashMap<MemberResolutionQueryKey, crate::query::MemberResolution>,
}

impl SemaQueryCacheState {
    fn clear_all(&mut self) {
        self.call_signature_instantiation_cache.clear();
        self.field_type_subst_cache.clear();
        self.trait_method_query_cache.clear();
        self.impl_method_query_cache.clear();
        self.bound_trait_match_cache.clear();
        self.impl_applicability_cache.clear();
        self.impl_requirement_cycle_cache.clear();
        self.impl_paterson_boundedness_cache.clear();
        self.named_field_query_cache.clear();
        self.member_resolution_query_cache.clear();
    }

    fn clear_active_bound_caches(&mut self) {
        self.bound_trait_match_cache.clear();
        self.impl_applicability_cache.clear();
        self.impl_method_query_cache.clear();
        self.member_resolution_query_cache.clear();
    }
}

#[derive(Clone, Default)]
pub(crate) struct RecursiveReportState {
    pub(crate) reported_recursive_layout_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_types: FastHashSet<TypeId>,
    pub(crate) reported_recursive_projection_assoc_defs: FastHashSet<DefId>,
}

#[derive(Clone, Default)]
pub struct SemaAnalysisState {
    pub active_bounds: Vec<(TypeId, Vec<TypeId>)>,
    pub(crate) expr_timing_stats: ExprTimingStats,
    pub(crate) query_caches: SemaQueryCacheState,
    pub(crate) recursive_reports: RecursiveReportState,
    pub(crate) semantic_index: SemanticIndexState,
}

#[derive(Clone, Default)]
pub struct SemaImplIndexState {
    pub global_impls: Vec<DefId>,
    pub trait_impls: Vec<DefId>,
    pub impl_methods_by_name: FastHashMap<SymbolId, Vec<DefId>>,
}

#[derive(Clone, Default)]
pub struct SemaResolutionState {
    pub builtin_defs: FastHashMap<SymbolId, DefId>,
    pub current_package_name: Option<SymbolId>,
    pub module_aliases: HashMap<String, String>,
    pub module_interface_aliases: HashMap<String, String>,
    pub(crate) module_ownership: ModuleOwnershipState,
}

#[derive(Clone, Default)]
pub struct SemaNodeFactsState {
    pub node_types: FastHashMap<NodeId, TypeId>,
    pub atomic_orderings: FastHashMap<NodeId, AtomicOrdering>,
    pub trait_method_owners: FastHashMap<NodeId, TypeId>,
}

#[derive(Clone)]
pub struct SemaStructureSnapshot {
    pub type_registry: TypeRegistry,
    pub facts: SemaNodeFactsState,
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,
    pub resolution: SemaResolutionState,
    pub impl_index: SemaImplIndexState,
    pub(crate) recursive_reports: RecursiveReportState,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExprTimingStats {
    pub bindings: Duration,
    pub ops: Duration,
    pub access: Duration,
    pub access_identifier: Duration,
    pub access_field: Duration,
    pub access_field_module: Duration,
    pub access_field_enum_variant: Duration,
    pub access_field_member_query: Duration,
    pub access_field_query_trait_object: Duration,
    pub access_field_query_named_type: Duration,
    pub access_field_query_bound: Duration,
    pub access_field_query_impl: Duration,
    pub access_field_miss: Duration,
    pub access_index: Duration,
    pub access_slice: Duration,
    pub call: Duration,
    pub call_plain: Duration,
    pub call_signature: Duration,
    pub call_intrinsic: Duration,
    pub call_arguments: Duration,
    pub call_generic_instantiation: Duration,
    pub call_closure: Duration,
    pub aggregate: Duration,
    pub control: Duration,
    pub control_block: Duration,
    pub control_if: Duration,
    pub control_match: Duration,
    pub control_match_patterns: Duration,
    pub control_match_bodies: Duration,
    pub control_match_exhaustiveness: Duration,
    pub control_for: Duration,
    pub control_return: Duration,
    pub control_defer: Duration,
    pub dynamic_typeof: Duration,
}

pub struct SemaContext<'a> {
    // 1. Shared compiler services.
    pub sess: &'a mut Session,

    // 2. Type-system state.
    pub type_registry: TypeRegistry,
    // Final inferred type for each AST node and other per-node semantic facts.
    pub facts: SemaNodeFactsState,

    // 3. Symbol and scope state.
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,
    pub impl_index: SemaImplIndexState,

    // 4. Module and package resolution state.
    pub resolution: SemaResolutionState,
    // 5. Analysis-time caches, timings, and semantic indexes.
    pub(crate) analysis: SemaAnalysisState,
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
            facts: SemaNodeFactsState::default(),
            defs: Vec::new(),
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
        let id = DefId(self.defs.len() as u32);
        self.defs.push(def);
        self.resolution
            .module_ownership
            .defs_without_parent_module
            .insert(id);
        id
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
        self.analysis.recursive_reports = snapshot.recursive_reports;
        self.analysis.expr_timing_stats = ExprTimingStats::default();
        self.analysis.query_caches.clear_all();
        self.clear_active_bound_caches();
        self.analysis.semantic_index.clear();
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

    pub fn atomic_ordering(&self, node_id: NodeId) -> Option<AtomicOrdering> {
        self.facts.atomic_orderings.get(&node_id).copied()
    }

    pub fn set_atomic_ordering(&mut self, node_id: NodeId, ordering: AtomicOrdering) {
        self.facts.atomic_orderings.insert(node_id, ordering);
    }

    pub fn trait_method_owner(&self, node_id: NodeId) -> Option<TypeId> {
        self.facts.trait_method_owners.get(&node_id).copied()
    }

    pub fn set_trait_method_owner(&mut self, node_id: NodeId, owner_trait_ty: TypeId) {
        self.facts
            .trait_method_owners
            .insert(node_id, owner_trait_ty);
    }

    pub fn trait_impl_ids(&self) -> &[DefId] {
        &self.impl_index.trait_impls
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

    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        self.sess.report(span, level, msg);
    }

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

    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.sess.load_file(path)
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

    pub fn configured_runtime_entry(&self) -> RuntimeEntry {
        self.sess.runtime_entry
    }

    pub fn program_entry_enabled(&self) -> bool {
        !matches!(self.configured_runtime_entry(), RuntimeEntry::None)
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
        let id = DefId(ctx.defs.len() as u32);
        let scope_id = ScopeId(id.0 as usize);
        let name = ctx.intern(name);
        let def_id = ctx.add_def(Def::Module(ModuleDef {
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
        }));
        ctx.register_module_scope(def_id, scope_id);
        def_id
    }

    fn add_struct(ctx: &mut SemaContext<'_>, name: &str, parent_module: Option<DefId>) -> DefId {
        let id = DefId(ctx.defs.len() as u32);
        let name = ctx.intern(name);
        let def_id = ctx.add_def(Def::Struct(StructDef {
            id,
            name,
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
        }));
        ctx.register_def_owner(def_id, parent_module, None);
        def_id
    }

    fn add_function(ctx: &mut SemaContext<'_>, name: &str, parent: Option<DefId>) -> DefId {
        let id = DefId(ctx.defs.len() as u32);
        let name = ctx.intern(name);
        let type_node = TypeNode {
            id: ctx.next_node_id(),
            span: Span::default(),
            kind: kernc_ast::TypeKind::Infer,
        };
        let def_id = ctx.add_def(Def::Function(FunctionDef {
            id,
            name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent,
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
        }));
        ctx.register_def_owner(def_id, None, None);
        def_id
    }
}
