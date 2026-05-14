#![doc = include_str!("../README.md")]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use kernc_ast as ast;
use kernc_flow::{FlowLoweringHints, FlowLoweringOwnerHints};
use kernc_mast::*;
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, EnumDef, FunctionDef, Visibility};
use kernc_sema::scope::ScopeId;
use kernc_sema::ty::{
    ConstExprKind, ConstGeneric, ConstGenericValue, ConstGenericValueKind, GenericArg,
    Substituter, TypeId, TypeKind,
};
use kernc_utils::{NodeId, Span, SymbolId};

pub(crate) mod expr;
mod inline;
pub(crate) mod mono;
pub(crate) mod vtable;

#[derive(Clone, Copy)]
enum LowerRootAction {
    InstantiateFunction(DefId),
    LowerGlobal(DefId),
}

#[derive(Debug, Clone)]
struct ActiveFunctionInstantiation {
    def_id: DefId,
    args: Vec<GenericArg>,
    request_span: Span,
}

#[derive(Debug, Clone)]
struct PendingFunctionInstantiation {
    def_id: DefId,
    args: Vec<GenericArg>,
    id: MonoId,
    request_span: Span,
    lineage: Vec<ActiveFunctionInstantiation>,
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

type BoundImplMethodKey = (TypeId, TypeId, SymbolId);
type BoundImplMethodTarget = (DefId, Option<TypeId>, Vec<GenericArg>);

pub struct Lowerer<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    module: MastModule,

    pub(crate) mono_cache: HashMap<(DefId, Vec<GenericArg>), MonoId>,
    pub(crate) pure_enum_tag_map: HashMap<(DefId, Vec<GenericArg>), TypeId>,
    pub(crate) next_mono_id: u32,
    pub(crate) pending_function_instantiations: Vec<PendingFunctionInstantiation>,
    pub(crate) next_pending_function_instantiation: usize,
    pub(crate) active_function_instantiations: Vec<ActiveFunctionInstantiation>,
    pub(crate) defer_stack: Vec<Vec<MastExpr>>,
    pub(crate) global_map: HashMap<DefId, MonoId>,
    pub(crate) global_symbol_map: HashMap<SymbolId, MonoId>,
    pub(crate) vtable_cache: HashMap<(TypeId, TypeId, TypeId), MonoId>,
    pub(crate) vtable_method_adapter_cache: HashMap<(MonoId, TypeId, TypeId), MonoId>,
    pub(crate) local_types: Vec<HashMap<SymbolId, (TypeId, bool)>>,
    pub(crate) local_forwardings: Vec<HashMap<SymbolId, SymbolId>>,
    pub(crate) local_value_forwardings: Vec<HashMap<SymbolId, MastExpr>>,
    pub(crate) local_statics: Vec<HashMap<SymbolId, MonoId>>,
    pub(crate) loop_frames: Vec<usize>,
    pub(crate) field_index_cache: HashMap<(TypeId, SymbolId), usize>,
    pub(crate) bound_impl_method_cache: HashMap<BoundImplMethodKey, Option<BoundImplMethodTarget>>,
    pub(crate) callee_expected_params_cache: HashMap<TypeId, Vec<TypeId>>,
    pub(crate) callable_signature_cache: HashMap<TypeId, (Vec<TypeId>, TypeId)>,
    pub(crate) named_struct_layout_cache: HashMap<NamedStructLayoutKey, StructLayoutMapping>,
    pub(crate) anon_struct_layout_cache: HashMap<TypeId, (Vec<usize>, Vec<usize>)>,
    pub(crate) adt_union_map: HashMap<MonoId, MonoId>,
    pub(crate) range_cache: HashMap<TypeId, MonoId>,
    pub(crate) closure_fn_map: HashMap<NodeId, MonoId>,
    pub(crate) fn_closure_adapter_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_struct_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_union_cache: HashMap<TypeId, MonoId>,
    pub(crate) anon_enum_cache: HashMap<TypeId, MonoId>,
    pub(crate) repr_tracked_types: HashSet<TypeId>,
    pub(crate) reachable_module_items: Option<HashSet<DefId>>,
    pub(crate) flow_lowering_hints: FlowLoweringHints,
    pub(crate) forwarding_symbol_cache: HashMap<String, SymbolId>,
    pub(crate) current_owner_def_id: Option<DefId>,
    pub(crate) current_return_types: Vec<TypeId>,
    pub(crate) next_synth_symbol: u32,
    collect_phase_timings: bool,
    phase_totals: HashMap<&'static str, Duration>,
    cache_stats: LowerCacheStats,
}

type StructLayoutMapping = (Vec<usize>, Vec<usize>);
type NamedStructLayoutKey = (DefId, Vec<GenericArg>);

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn normalize_concrete_type(&mut self, ty: TypeId) -> TypeId {
        self.ctx.normalize_concrete_type(ty)
    }

    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        let collect_phase_timings = ctx.sess.report_timings;
        let module_name = ctx
            .root_module()
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
                mono: MonoModuleMetadata::default(),
            },
            mono_cache: HashMap::new(),
            pure_enum_tag_map: HashMap::new(),
            next_mono_id: 1,
            pending_function_instantiations: Vec::new(),
            next_pending_function_instantiation: 0,
            active_function_instantiations: Vec::new(),
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            global_symbol_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            vtable_method_adapter_cache: HashMap::new(),
            local_types: Vec::new(),
            local_forwardings: Vec::new(),
            local_value_forwardings: Vec::new(),
            local_statics: Vec::new(),
            loop_frames: Vec::new(),
            field_index_cache: HashMap::new(),
            bound_impl_method_cache: HashMap::new(),
            callee_expected_params_cache: HashMap::new(),
            callable_signature_cache: HashMap::new(),
            named_struct_layout_cache: HashMap::new(),
            anon_struct_layout_cache: HashMap::new(),
            adt_union_map: HashMap::new(),
            range_cache: HashMap::new(),
            closure_fn_map: HashMap::new(),
            fn_closure_adapter_cache: HashMap::new(),
            anon_struct_cache: HashMap::new(),
            anon_union_cache: HashMap::new(),
            anon_enum_cache: HashMap::new(),
            repr_tracked_types: HashSet::new(),
            reachable_module_items: None,
            flow_lowering_hints: FlowLoweringHints::default(),
            forwarding_symbol_cache: HashMap::new(),
            current_owner_def_id: None,
            current_return_types: Vec::new(),
            next_synth_symbol: 0,
            collect_phase_timings,
            phase_totals: HashMap::new(),
            cache_stats: LowerCacheStats::default(),
        }
    }

    fn cached_forwarding_symbol(&mut self, name: &str) -> SymbolId {
        if let Some(&symbol) = self.forwarding_symbol_cache.get(name) {
            return symbol;
        }
        let symbol = self.ctx.intern(name);
        self.forwarding_symbol_cache
            .insert(name.to_string(), symbol);
        symbol
    }

    pub(crate) fn fresh_synth_symbol(&mut self, prefix: &str) -> SymbolId {
        let symbol = self
            .ctx
            .intern(&format!("__{}_{}", prefix, self.next_synth_symbol));
        self.next_synth_symbol += 1;
        symbol
    }

    pub(crate) fn measure_phase<T, F>(&mut self, name: &'static str, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
    {
        if !self.collect_phase_timings {
            return f(self);
        }
        let started = Instant::now();
        let value = f(self);
        *self.phase_totals.entry(name).or_default() += started.elapsed();
        value
    }

    pub(crate) fn substitute_type_with_map(
        &mut self,
        ty: TypeId,
        subst_map: &HashMap<SymbolId, GenericArg>,
    ) -> TypeId {
        if ty == TypeId::ERROR {
            ty
        } else {
            let substituted = if subst_map.is_empty() {
                ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
                subst.substitute(ty)
            };
            self.normalize_concrete_type(substituted)
        }
    }

    pub(crate) fn usize_const_generic(&self, value: u64) -> ConstGeneric {
        ConstGeneric::Value(ConstGenericValue {
            ty: TypeId::USIZE,
            kind: ConstGenericValueKind::Int(value as i128),
        })
    }

    pub(crate) fn const_generic_usize(&mut self, value: ConstGeneric, span: Span) -> Option<u64> {
        match value {
            ConstGeneric::Value(value) if value.ty == TypeId::USIZE => {
                u64::try_from(value.as_int()?).ok()
            }
            ConstGeneric::Value(_) | ConstGeneric::Error => None,
            ConstGeneric::Param(symbol, _) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): unresolved const generic `{}` reached lowering.",
                        self.ctx.resolve(symbol)
                    ),
                );
                None
            }
            ConstGeneric::Expr(expr_id) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): unresolved const expression `{:?}` reached lowering.",
                        self.ctx.type_registry.const_expr(expr_id)
                    ),
                );
                None
            }
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
        Some(self.cached_forwarding_symbol(&name))
    }

    pub(crate) fn forwardable_binding_source(&mut self, expr_id: NodeId) -> Option<SymbolId> {
        let name = self
            .current_owner_flow_hints()
            .and_then(|hints| hints.forwarding.forwardable_binding_sources.get(&expr_id))
            .cloned()?;
        Some(self.cached_forwarding_symbol(&name))
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
        let max_hops = self
            .local_forwardings
            .iter()
            .map(HashMap::len)
            .sum::<usize>()
            .saturating_add(1);

        for _ in 0..max_hops {
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

    pub(crate) fn local_binding(&self, name: SymbolId) -> Option<(TypeId, bool)> {
        self.local_types
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    pub(crate) fn cached_named_struct_mapping(
        &mut self,
        def_id: DefId,
        gen_args: &[GenericArg],
    ) -> (Vec<usize>, Vec<usize>) {
        let key = (def_id, gen_args.to_vec());
        if let Some(mapping) = self.named_struct_layout_cache.get(&key) {
            return mapping.clone();
        }

        let mapping = {
            let mut layout = kernc_sema::LayoutEngine::new(self.ctx);
            layout.get_struct_mapping(def_id, gen_args, 0)
        };
        self.named_struct_layout_cache.insert(key, mapping.clone());
        mapping
    }

    pub(crate) fn cached_anon_struct_mapping(
        &mut self,
        norm_ty: TypeId,
        is_extern: bool,
        fields: &[kernc_sema::ty::AnonymousField],
    ) -> (Vec<usize>, Vec<usize>) {
        if let Some(mapping) = self.anon_struct_layout_cache.get(&norm_ty) {
            return mapping.clone();
        }

        let mapping = {
            let mut layout = kernc_sema::LayoutEngine::new(self.ctx);
            layout.get_anon_struct_mapping(is_extern, fields, 0)
        };
        self.anon_struct_layout_cache
            .insert(norm_ty, mapping.clone());
        mapping
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
            Def::Trait(_) => {
                let module_id = self.ctx.def_parent_module(parent_id)?;
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
        let def_ids: Vec<_> = self.ctx.defs.ids().collect();

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
                        if g.is_static
                            && !g.is_imported
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
                        let request_span = match &this.ctx.defs[id.0 as usize] {
                            Def::Function(function) => function.name_span,
                            _ => Span::default(),
                        };
                        this.instantiate_function_at(id, &[], request_span);
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

        self.drain_pending_function_instantiations();
        self.measure_phase("  lower_inline", |this| {
            this.run_inline_pass();
        });

        self.module.mono = MonoModuleMetadata {
            def_mono_map: self.mono_cache.clone(),
            pure_enum_tag_map: self.pure_enum_tag_map.clone(),
            adt_union_map: self.adt_union_map.clone(),
            range_map: self.range_cache.clone(),
            anon_struct_map: self.anon_struct_cache.clone(),
            anon_union_map: self.anon_union_cache.clone(),
            anon_enum_map: self.anon_enum_cache.clone(),
        };

        let module = MastModule {
            name: std::mem::take(&mut self.module.name),
            structs: std::mem::take(&mut self.module.structs),
            globals: std::mem::take(&mut self.module.globals),
            functions: std::mem::take(&mut self.module.functions),
            mono: std::mem::take(&mut self.module.mono),
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

    pub(crate) fn has_meta_attr(&self, attrs: &[ast::Attribute], expected: &str) -> bool {
        attrs.iter().any(|attr| {
            let ast::AttributeKind::Meta(items) = &attr.kind else {
                return false;
            };
            items.iter().any(|item| {
                matches!(
                    item,
                    ast::MetaItem::Call(name, _) | ast::MetaItem::Marker(name)
                        if self.ctx.resolve(*name) == expected
                )
            })
        })
    }

    pub(crate) fn lowered_inline_hint(&self, attrs: &[ast::Attribute]) -> MastInlineHint {
        for attr in attrs {
            let ast::AttributeKind::Meta(items) = &attr.kind else {
                continue;
            };
            for item in items {
                match item {
                    ast::MetaItem::Marker(name) => match self.ctx.resolve(*name) {
                        "inline" => return MastInlineHint::Inline,
                        "noinline" => return MastInlineHint::NoInline,
                        _ => {}
                    },
                    ast::MetaItem::Call(_, _) => {}
                }
            }
        }
        MastInlineHint::None
    }

    pub(crate) fn lowered_function_linkage(
        &self,
        vis: Visibility,
        is_extern: bool,
        attrs: &[ast::Attribute],
        uses_odr_linkage: bool,
    ) -> MastLinkage {
        if uses_odr_linkage {
            return MastLinkage::LinkOnceOdr;
        }
        if is_extern || !vis.is_private() || self.has_meta_attr(attrs, "export_name") {
            MastLinkage::External
        } else {
            MastLinkage::Internal
        }
    }

    pub(crate) fn lowered_global_linkage(
        &self,
        vis: Visibility,
        is_extern: bool,
        attrs: &[ast::Attribute],
    ) -> MastLinkage {
        if is_extern || !vis.is_private() || self.has_meta_attr(attrs, "export_name") {
            MastLinkage::External
        } else {
            MastLinkage::Internal
        }
    }

    /// Detect pure-data enums whose payload-free layout is equivalent to an integer.
    pub(crate) fn is_pure_enum(&self, def: &EnumDef) -> bool {
        def.variants.iter().all(|v| v.payload_type.is_none())
    }

    pub(crate) fn record_pure_enum_tag_ty(&mut self, def_id: DefId, args: &[GenericArg]) -> TypeId {
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
            self.ctx.node_type(backing_ty.id).unwrap_or(TypeId::U32)
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
        if !self.repr_tracked_types.insert(norm_ty) {
            return;
        }
        match self.ctx.type_registry.get(norm_ty).clone() {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Array { elem, .. }
            | TypeKind::ArrayInfer { elem, .. } => self.track_pure_enum_repr_in_type(elem),
            TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.track_pure_enum_repr_in_type(start);
                }
                if let Some(end) = end {
                    self.track_pure_enum_repr_in_type(end);
                }
                self.instantiate_range_struct(norm_ty);
            }
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
            TypeKind::AnonymousStruct(..) => {
                self.instantiate_anon_struct(norm_ty);
            }
            TypeKind::AnonymousUnion(..) => {
                self.instantiate_anon_union(norm_ty);
            }
            TypeKind::AnonymousEnum(_) => {
                self.instantiate_anon_enum(norm_ty);
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let enum_ty = self.ctx.type_registry.normalize(enum_ty);
                self.instantiate_anon_enum(enum_ty);
            }
            _ => {}
        }
    }

    pub(crate) fn track_pure_enum_repr_in_const_generic(&mut self, value: ConstGeneric) {
        match value {
            ConstGeneric::Value(value) => self.track_pure_enum_repr_in_type(value.ty),
            ConstGeneric::Param(_, ty) => self.track_pure_enum_repr_in_type(ty),
            ConstGeneric::Expr(expr_id) => {
                let expr = *self.ctx.type_registry.const_expr(expr_id);
                match expr {
                    ConstExprKind::Unary { expr, ty, .. }
                    | ConstExprKind::Cast { expr, ty } => {
                        self.track_pure_enum_repr_in_const_generic(expr);
                        self.track_pure_enum_repr_in_type(ty);
                    }
                    ConstExprKind::Binary { lhs, rhs, ty, .. } => {
                        self.track_pure_enum_repr_in_const_generic(lhs);
                        self.track_pure_enum_repr_in_const_generic(rhs);
                        self.track_pure_enum_repr_in_type(ty);
                    }
                }
            }
            ConstGeneric::Error => {}
        }
    }

    pub(crate) fn track_pure_enum_repr_in_const_generic_arg(&mut self, arg: GenericArg) {
        if let GenericArg::Const(value) = arg {
            self.track_pure_enum_repr_in_const_generic(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use kernc_ast::{GenericParam, GenericParamKind};
    use kernc_utils::{DiagnosticLevel, Session};

    #[test]
    fn invalid_vtable_target_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let mut lowerer = Lowerer::new(&mut ctx);

        let vtable_id = lowerer.get_or_create_vtable(TypeId::U8, TypeId::U8, TypeId::U8);

        assert_eq!(lowerer.ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].level,
            DiagnosticLevel::Error
        );
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].message,
            "cannot build a vtable for non-trait-object type `Primitive(U8)`"
        );
        assert!(
            lowerer
                .module
                .globals
                .iter()
                .any(|global| global.id == vtable_id)
        );
    }

    #[test]
    fn generic_subst_map_mismatch_emits_error_not_ice() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let param = GenericParam {
            name: ctx.intern("T"),
            span: Span::default(),
            kind: GenericParamKind::Type,
        };
        let mut lowerer = Lowerer::new(&mut ctx);

        let subst = lowerer.build_generic_subst_map("function", "demo", &[param], &[]);

        assert_eq!(subst, None);
        assert_eq!(lowerer.ctx.sess.diagnostics.len(), 1);
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].level,
            DiagnosticLevel::Error
        );
        assert_eq!(
            lowerer.ctx.sess.diagnostics[0].message,
            "generic argument count mismatch for function `demo`: expected 1, got 0"
        );
    }
}
