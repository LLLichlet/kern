#![doc = include_str!("../README.md")]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use kernc_ast as ast;
use kernc_mast::*;
use kernc_sema::SemaContext;
use kernc_sema::checker::Substituter;
use kernc_sema::def::{Def, DefId, EnumDef, FunctionDef};
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{NodeId, Span, SymbolId};

pub(crate) mod expr;
pub(crate) mod mono;
pub(crate) mod vtable;

#[derive(Clone, Copy)]
enum LowerRootAction {
    InstantiateFunction(DefId),
    LowerGlobal(DefId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LowerTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LowerCacheStats {
    pub mono_function_hits: usize,
    pub mono_function_misses: usize,
    pub mono_struct_hits: usize,
    pub mono_struct_misses: usize,
    pub mono_data_hits: usize,
    pub mono_data_misses: usize,
}

impl LowerCacheStats {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }
}

#[derive(Debug, Clone)]
pub struct LowerReport {
    pub module: MastModule,
    pub phase_timings: Vec<LowerTiming>,
    pub cache_stats: LowerCacheStats,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringHints {
    owners: HashMap<DefId, FlowLoweringOwnerHints>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringOwnerHints {
    pub elision: FlowLoweringElisionHints,
    pub forwarding: FlowLoweringForwardingHints,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringElisionHints {
    pub pure_dead_initializer_expr_ids: HashSet<NodeId>,
    pub pure_dead_assignment_expr_ids: HashSet<NodeId>,
    pub elidable_binding_expr_ids: HashSet<NodeId>,
}

#[derive(Debug, Clone, Default)]
pub struct FlowLoweringForwardingHints {
    pub identifier_copy_sources: HashMap<NodeId, String>,
    pub forwardable_binding_sources: HashMap<NodeId, String>,
    pub forwardable_value_expr_ids: HashSet<NodeId>,
}

impl FlowLoweringHints {
    pub fn insert_owner(&mut self, def_id: DefId, hints: FlowLoweringOwnerHints) {
        self.owners.insert(def_id, hints);
    }

    pub fn owner(&self, def_id: DefId) -> Option<&FlowLoweringOwnerHints> {
        self.owners.get(&def_id)
    }
}

pub struct Lowerer<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    module: MastModule,

    pub(crate) mono_cache: HashMap<(DefId, Vec<TypeId>), MonoId>,
    pub(crate) pure_enum_tag_map: HashMap<(DefId, Vec<TypeId>), TypeId>,
    pub(crate) next_mono_id: u32,
    pub(crate) defer_stack: Vec<Vec<MastExpr>>,
    pub(crate) global_map: HashMap<DefId, MonoId>,
    pub(crate) global_symbol_map: HashMap<SymbolId, MonoId>,
    pub(crate) vtable_cache: HashMap<(TypeId, TypeId), MonoId>,
    pub(crate) local_types: Vec<HashMap<SymbolId, (TypeId, bool)>>,
    pub(crate) local_forwardings: Vec<HashMap<SymbolId, SymbolId>>,
    pub(crate) local_value_forwardings: Vec<HashMap<SymbolId, MastExpr>>,
    pub(crate) local_statics: Vec<HashMap<SymbolId, MonoId>>,
    pub(crate) loop_frames: Vec<usize>,
    pub(crate) adt_union_map: HashMap<MonoId, MonoId>,
    pub(crate) closure_fn_map: HashMap<NodeId, MonoId>,
    pub(crate) anon_struct_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_union_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_enum_cache: HashMap<TypeId, MonoId>,
    pub(crate) reachable_module_items: Option<HashSet<DefId>>,
    pub(crate) flow_lowering_hints: FlowLoweringHints,
    pub(crate) current_owner_def_id: Option<DefId>,
    phase_totals: HashMap<&'static str, Duration>,
    cache_stats: LowerCacheStats,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        let module_name = ctx
            .root_module
            .and_then(|root_id| match &ctx.defs[root_id.0 as usize] {
                Def::Module(module) => Some(ctx.resolve(module.name).to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "kern_out".to_string());
        Self {
            ctx,
            module: MastModule {
                name: module_name,
                structs: Vec::new(),
                globals: Vec::new(),
                functions: Vec::new(),
                def_mono_map: HashMap::new(),
                pure_enum_tag_map: HashMap::new(),
                adt_union_map: HashMap::new(),
                anon_struct_map: HashMap::new(),
                anon_union_map: HashMap::new(),
                anon_enum_map: HashMap::new(),
            },
            mono_cache: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            next_mono_id: 1,
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            global_symbol_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            local_types: Vec::new(),
            local_forwardings: Vec::new(),
            local_value_forwardings: Vec::new(),
            local_statics: Vec::new(),
            loop_frames: Vec::new(),
            adt_union_map: HashMap::new(),
            closure_fn_map: HashMap::new(),
            anon_struct_cache: HashMap::new(),
            anon_union_cache: HashMap::new(),
            anon_enum_cache: HashMap::new(),
            reachable_module_items: None,
            flow_lowering_hints: FlowLoweringHints::default(),
            current_owner_def_id: None,
            phase_totals: HashMap::new(),
            cache_stats: LowerCacheStats::default(),
        }
    }

    pub(crate) fn measure_phase<T, F>(&mut self, name: &'static str, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        let started = Instant::now();
        let value = f(self);
        *self.phase_totals.entry(name).or_default() += started.elapsed();
        value
    }

    pub(crate) fn substitute_type_with_map(
        &mut self,
        ty: TypeId,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> TypeId {
        if subst_map.is_empty() || ty == TypeId::ERROR {
            ty
        } else {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
            subst.substitute(ty)
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn set_reachable_module_items(&mut self, reachable: HashSet<DefId>) {
        self.reachable_module_items = Some(reachable);
    }

    pub fn set_flow_lowering_hints(&mut self, hints: FlowLoweringHints) {
        self.flow_lowering_hints = hints;
    }

    pub(crate) fn current_owner_flow_hints(&self) -> Option<&FlowLoweringOwnerHints> {
        self.current_owner_def_id
            .and_then(|def_id| self.flow_lowering_hints.owner(def_id))
    }

    pub(crate) fn is_pure_dead_initializer(&self, expr_id: NodeId) -> bool {
        self.current_owner_flow_hints().is_some_and(|hints| {
            hints
                .elision
                .pure_dead_initializer_expr_ids
                .contains(&expr_id)
        })
    }

    pub(crate) fn is_pure_dead_assignment(&self, expr_id: NodeId) -> bool {
        self.current_owner_flow_hints().is_some_and(|hints| {
            hints
                .elision
                .pure_dead_assignment_expr_ids
                .contains(&expr_id)
        })
    }

    pub(crate) fn identifier_copy_source(&mut self, expr_id: NodeId) -> Option<SymbolId> {
        let name = self
            .current_owner_flow_hints()
            .and_then(|hints| hints.forwarding.identifier_copy_sources.get(&expr_id))
            .cloned()?;
        Some(self.ctx.intern(&name))
    }

    pub(crate) fn forwardable_binding_source(&mut self, expr_id: NodeId) -> Option<SymbolId> {
        let name = self
            .current_owner_flow_hints()
            .and_then(|hints| hints.forwarding.forwardable_binding_sources.get(&expr_id))
            .cloned()?;
        Some(self.ctx.intern(&name))
    }

    pub(crate) fn is_forwardable_value_binding(&self, expr_id: NodeId) -> bool {
        self.current_owner_flow_hints().is_some_and(|hints| {
            hints
                .forwarding
                .forwardable_value_expr_ids
                .contains(&expr_id)
        })
    }

    pub(crate) fn is_elidable_binding(&self, expr_id: NodeId) -> bool {
        self.current_owner_flow_hints()
            .is_some_and(|hints| hints.elision.elidable_binding_expr_ids.contains(&expr_id))
    }

    pub(crate) fn record_local_forwarding(
        &mut self,
        span: Span,
        name: SymbolId,
        forwarded_to: SymbolId,
        context: &str,
    ) -> bool {
        if let Some(scope) = self.local_forwardings.last_mut() {
            scope.insert(name, forwarded_to);
            true
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): missing local forwarding scope while {}.",
                    context
                ),
            );
            false
        }
    }

    pub(crate) fn record_local_value_forwarding(
        &mut self,
        span: Span,
        name: SymbolId,
        value: MastExpr,
        context: &str,
    ) -> bool {
        if let Some(scope) = self.local_value_forwardings.last_mut() {
            scope.insert(name, value);
            true
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Kern ICE (Lowering): missing local value-forwarding scope while {}.",
                    context
                ),
            );
            false
        }
    }

    pub(crate) fn resolve_forwarded_local(&self, name: SymbolId) -> SymbolId {
        let mut current = name;
        let mut visited = HashSet::new();
        while visited.insert(current) {
            let Some(next) = self
                .local_forwardings
                .iter()
                .rev()
                .find_map(|scope| scope.get(&current).copied())
            else {
                break;
            };
            current = next;
        }
        current
    }

    pub(crate) fn forwarded_local_value(&self, name: SymbolId) -> Option<MastExpr> {
        for scope_idx in (0..self.local_value_forwardings.len()).rev() {
            if let Some(value) = self.local_value_forwardings[scope_idx].get(&name).cloned() {
                return Some(value);
            }

            if self
                .local_types
                .get(scope_idx)
                .is_some_and(|scope| scope.contains_key(&name))
            {
                return None;
            }
        }

        None
    }

    pub(crate) fn local_binding(&self, name: SymbolId) -> Option<(TypeId, bool)> {
        self.local_types
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    pub(crate) fn has_local_binding(&self, name: SymbolId) -> bool {
        self.local_binding(name).is_some()
    }

    fn new_mono_id(&mut self) -> MonoId {
        let id = self.next_mono_id;
        self.next_mono_id += 1;
        MonoId(id)
    }

    fn function_requires_runtime_body(&self, f: &kernc_sema::def::FunctionDef) -> bool {
        if !f.is_imported {
            return true;
        }
        if !f.generics.is_empty() {
            return true;
        }
        if let Some(parent_id) = f.parent
            && let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize]
        {
            return !impl_def.generics.is_empty();
        }
        false
    }

    pub(crate) fn function_owner_scope(&self, f: &FunctionDef) -> Option<ScopeId> {
        let parent_id = f.parent?;
        match &self.ctx.defs[parent_id.0 as usize] {
            Def::Module(module) => Some(module.scope_id),
            Def::Impl(impl_def) => {
                let module_id = impl_def.parent_module?;
                match &self.ctx.defs[module_id.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Entry point for lowering: recursively monomorphize every non-generic root item.
    pub fn lower_all(&mut self) -> MastModule {
        self.lower_all_with_report().module
    }

    /// Entry point for lowering with internal timing breakdowns.
    pub fn lower_all_with_report(&mut self) -> LowerReport {
        let def_ids: Vec<_> = (0..self.ctx.defs.len()).map(|i| DefId(i as u32)).collect();

        // Phase 1: preallocate `MonoId`s for globals.
        self.measure_phase("  lower_preallocate_globals", |this| {
            for &id in &def_ids {
                let global_name = if let Def::Global(g) = &this.ctx.defs[id.0 as usize] {
                    Some(g.name)
                } else {
                    None
                };

                if let Some(name) = global_name {
                    let mono_id = this.new_mono_id();
                    this.global_map.insert(id, mono_id);
                    // Pre-register top-level global names.
                    this.global_symbol_map.insert(name, mono_id);
                }
            }
        });

        // Phase 2: lower concrete entities for real.
        let actions = self.measure_phase("  lower_collect_roots", |this| {
            let mut actions = Vec::new();
            for id in def_ids {
                match &this.ctx.defs[id.0 as usize] {
                    Def::Function(f) => {
                        if f.is_imported || f.is_intrinsic {
                            continue;
                        }
                        if this
                            .reachable_module_items
                            .as_ref()
                            .is_some_and(|reachable| !reachable.contains(&id))
                        {
                            continue;
                        }

                        let mut is_generic = !f.generics.is_empty();
                        if let Some(parent_id) = f.parent
                            && let Def::Impl(impl_def) = &this.ctx.defs[parent_id.0 as usize]
                            && !impl_def.generics.is_empty()
                        {
                            is_generic = true;
                        }

                        if !is_generic {
                            actions.push(LowerRootAction::InstantiateFunction(id));
                        }
                    }
                    Def::Global(g) => {
                        if !g.is_imported
                            && this
                                .reachable_module_items
                                .as_ref()
                                .is_none_or(|reachable| reachable.contains(&id))
                        {
                            actions.push(LowerRootAction::LowerGlobal(id));
                        }
                    }
                    _ => {}
                }
            }
            actions
        });

        for action in actions {
            match action {
                LowerRootAction::InstantiateFunction(id) => {
                    self.measure_phase("  lower_root_functions", |this| {
                        this.instantiate_function(id, &[]);
                    })
                }
                LowerRootAction::LowerGlobal(id) => {
                    self.measure_phase("  lower_root_globals", |this| {
                        let Some(global_ptr) =
                            this.ctx.defs.get(id.0 as usize).and_then(|def| match def {
                                Def::Global(global) => Some(std::ptr::from_ref(global)),
                                _ => None,
                            })
                        else {
                            return;
                        };

                        // Safety: lowering does not mutate semantic definition storage.
                        let global = unsafe { &*global_ptr };
                        this.lower_global(global);
                    })
                }
            }
        }

        self.module.def_mono_map = self.mono_cache.clone();
        self.module.pure_enum_tag_map = self.pure_enum_tag_map.clone();
        self.module.adt_union_map = self.adt_union_map.clone();
        self.module.anon_struct_map = self.anon_struct_cache.clone();
        self.module.anon_union_map = self.anon_union_cache.clone();
        self.module.anon_enum_map = self.anon_enum_cache.clone();

        let module = MastModule {
            name: std::mem::take(&mut self.module.name),
            structs: std::mem::take(&mut self.module.structs),
            globals: std::mem::take(&mut self.module.globals),
            functions: std::mem::take(&mut self.module.functions),
            def_mono_map: std::mem::take(&mut self.module.def_mono_map),
            pure_enum_tag_map: std::mem::take(&mut self.module.pure_enum_tag_map),
            adt_union_map: std::mem::take(&mut self.module.adt_union_map),
            anon_struct_map: std::mem::take(&mut self.module.anon_struct_map),
            anon_union_map: std::mem::take(&mut self.module.anon_union_map),
            anon_enum_map: std::mem::take(&mut self.module.anon_enum_map),
        };
        let mut phase_timings = self
            .phase_totals
            .iter()
            .map(|(name, duration)| LowerTiming {
                name,
                duration: *duration,
            })
            .collect::<Vec<_>>();
        phase_timings.sort_by_key(|timing| timing.name);

        LowerReport {
            module,
            phase_timings,
            cache_stats: self.cache_stats,
        }
    }

    pub(crate) fn extract_meta_items(&self, attrs: &[ast::Attribute]) -> Vec<ast::MetaItem> {
        let mut meta = Vec::new();
        for attr in attrs {
            if let ast::AttributeKind::Meta(items) = &attr.kind {
                meta.extend(items.clone());
            }
        }
        meta
    }

    /// Detect pure-data enums whose payload-free layout is equivalent to an integer.
    pub(crate) fn is_pure_enum(&self, def: &EnumDef) -> bool {
        def.variants.iter().all(|v| v.payload_type.is_none())
    }

    pub(crate) fn record_pure_enum_tag_ty(&mut self, def_id: DefId, args: &[TypeId]) -> TypeId {
        let key = (def_id, args.to_vec());
        if let Some(&tag_ty) = self.pure_enum_tag_map.get(&key) {
            return tag_ty;
        }

        let Some(Def::Enum(def)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            self.ctx.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Lowering): DefId {} is not an enum while recording pure enum representation.",
                    def_id.0
                ),
            );
            return TypeId::ERROR;
        };

        let raw_tag_ty = def.backing_type.as_ref().map_or(TypeId::U32, |backing_ty| {
            self.ctx
                .node_types
                .get(&backing_ty.id)
                .copied()
                .unwrap_or(TypeId::U32)
        });

        let tag_ty = if def.generics.is_empty() || args.is_empty() {
            raw_tag_ty
        } else {
            let mut subst_map = HashMap::new();
            for (param, arg) in def.generics.iter().zip(args.iter().copied()) {
                subst_map.insert(param.name, arg);
            }
            self.substitute_type_with_map(raw_tag_ty, &subst_map)
        };

        self.pure_enum_tag_map.insert(key, tag_ty);
        tag_ty
    }

    pub(crate) fn track_pure_enum_repr_in_type(&mut self, ty: TypeId) {
        let norm_ty = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. } => self.track_pure_enum_repr_in_type(elem),
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                for param in params {
                    self.track_pure_enum_repr_in_type(param);
                }
                self.track_pure_enum_repr_in_type(ret);
            }
            TypeKind::Def(def_id, args) => {
                self.instantiate_struct(def_id, &args);
            }
            TypeKind::Enum(def_id, args) => {
                if let Some(Def::Enum(def)) = self.ctx.defs.get(def_id.0 as usize).cloned() {
                    if self.is_pure_enum(&def) {
                        self.record_pure_enum_tag_ty(def_id, &args);
                    } else {
                        self.instantiate_data(def_id, &args);
                    }
                }
            }
            TypeKind::Alias(_, inner) => self.track_pure_enum_repr_in_type(inner),
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                for field in fields {
                    self.track_pure_enum_repr_in_type(field.ty);
                }
            }
            _ => {}
        }
    }

    /// Look up the wrapper function `MonoId` associated with a closure-state AST node.
    pub(crate) fn get_closure_func_mono_id(&mut self, closure_node_id: NodeId) -> MonoId {
        match self.closure_fn_map.get(&closure_node_id) {
            Some(&id) => id,
            None => {
                // Missing entries here indicate an internal lowering-order bug.
                self.ctx.emit_ice(
                    Span::default(),
                    format!("Kern ICE (Lowering): Attempted to resolve a closure function ID before the closure expression (NodeId {}) was lowered.", closure_node_id.0)
                );
                let placeholder = self.new_mono_id();
                self.closure_fn_map.insert(closure_node_id, placeholder);
                placeholder
            }
        }
    }

    pub(crate) fn global_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.ctx.defs.iter().find_map(|def| {
            let Def::Module(module) = def else {
                return None;
            };

            if module.items.contains(&def_id) {
                Some(module.scope_id)
            } else {
                None
            }
        })
    }
}
