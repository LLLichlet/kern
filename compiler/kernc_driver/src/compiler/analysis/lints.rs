use super::*;

impl CompilerDriver {
    pub(super) fn emit_unused_private_item_warnings(
        &self,
        ctx: &mut SemaContext<'_>,
        references: &[(Span, Span)],
        flow: &FlowModel,
    ) {
        for item in self.collect_unused_private_items(ctx, references, flow) {
            ctx.struct_warning(item.definition_span, unused_item_message(&item))
                .with_code(kernc_utils::DiagnosticCode::UnusedPrivateItem)
                .with_tag(kernc_utils::DiagnosticTag::Unnecessary)
                .with_hint("remove it, make it public, or reference it from reachable code")
                .emit();
        }
    }

    pub(super) fn collect_unused_private_items(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
        flow: &FlowModel,
    ) -> Vec<AnalysisUnusedItem> {
        let reachability = self.compute_module_item_reachability(ctx, references, flow);
        if reachability.nodes.is_empty() {
            return Vec::new();
        }

        let mut unused = reachability
            .nodes
            .values()
            .filter(|node| node.is_warnable && !reachability.reachable.contains(&node.def_id))
            .filter_map(|node| self.unused_item_from_node(ctx, node))
            .collect::<Vec<_>>();
        unused.sort_by_key(|item| item.definition_span);
        unused
    }

    pub(in crate::compiler) fn compute_module_item_reachability(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
        flow: &FlowModel,
    ) -> ModuleItemReachability {
        let nodes = self.collect_module_item_reachability_nodes(ctx);
        if nodes.is_empty() {
            return ModuleItemReachability {
                nodes,
                reachable: std::collections::HashSet::new(),
            };
        }

        let item_definition_spans = nodes
            .values()
            .map(|node| (node.def_id, node.name_span))
            .collect::<std::collections::HashMap<_, _>>();
        let definition_span_to_def_id = item_definition_spans
            .iter()
            .map(|(&def_id, &span)| (span, def_id))
            .collect::<std::collections::HashMap<_, _>>();
        let mut reachable = nodes
            .values()
            .filter(|node| node.is_root)
            .map(|node| node.def_id)
            .collect::<std::collections::HashSet<_>>();
        let mut edges = std::collections::HashMap::<DefId, std::collections::HashSet<DefId>>::new();

        for (reference_span, definition_span) in references {
            let Some(&callee) = definition_span_to_def_id.get(definition_span) else {
                continue;
            };

            if let Some(caller) = flow.owner_def_id(*reference_span) {
                edges.entry(caller).or_default().insert(callee);
            } else {
                reachable.insert(callee);
            }
        }

        for (caller, callee) in flow.referenced_item_edges() {
            edges.entry(caller).or_default().insert(callee);
        }

        let mut worklist = reachable.iter().copied().collect::<Vec<_>>();
        let mut cursor = 0;
        while let Some(def_id) = worklist.get(cursor).copied() {
            cursor += 1;
            let Some(callees) = edges.get(&def_id) else {
                continue;
            };
            for callee in callees {
                if reachable.insert(*callee) {
                    worklist.push(*callee);
                }
            }
        }

        ModuleItemReachability { nodes, reachable }
    }

    pub(super) fn emit_unused_binding_warnings(&self, ctx: &mut SemaContext<'_>, flow: &FlowModel) {
        for binding in self.collect_unused_bindings(ctx, flow) {
            ctx.struct_warning(binding.definition_span, unused_binding_message(&binding))
                .with_code(kernc_utils::DiagnosticCode::UnusedBinding)
                .with_tag(kernc_utils::DiagnosticTag::Unnecessary)
                .with_hint("remove it, rename it to `_`, or reference it from reachable code")
                .emit();
        }
    }

    pub(super) fn emit_dead_store_warnings(
        &self,
        ctx: &mut SemaContext<'_>,
        references: &[(Span, Span)],
        flow: &FlowModel,
    ) {
        for store in self.collect_dead_stores(ctx, references, flow) {
            ctx.struct_warning(store.span, dead_store_message(&store))
                .with_code(kernc_utils::DiagnosticCode::DeadStore)
                .with_tag(kernc_utils::DiagnosticTag::Unnecessary)
                .with_hint("remove the assignment or use the value before it is overwritten")
                .emit();
        }
    }

    pub(super) fn collect_unused_bindings(
        &self,
        ctx: &SemaContext<'_>,
        flow: &FlowModel,
    ) -> Vec<AnalysisUnusedBinding> {
        let mut unused = flow
            .public_owners()
            .into_iter()
            .flat_map(|owner| owner.bindings.into_iter())
            .filter(|binding| binding.reference_spans.is_empty())
            .filter_map(|binding| {
                let kind = match binding.kind {
                    crate::compiler::AnalysisFlowBindingKind::Variable => {
                        AnalysisUnusedBindingKind::Variable
                    }
                    crate::compiler::AnalysisFlowBindingKind::Parameter => {
                        AnalysisUnusedBindingKind::Parameter
                    }
                    crate::compiler::AnalysisFlowBindingKind::Static => return None,
                };
                let name = ctx
                    .sess
                    .source_manager
                    .slice_source(binding.definition_span)
                    .trim()
                    .to_string();
                if name.is_empty() || name == "_" {
                    return None;
                }

                Some(AnalysisUnusedBinding {
                    definition_span: binding.definition_span,
                    kind,
                    name,
                })
            })
            .collect::<Vec<_>>();
        unused.sort_by_key(|binding| binding.definition_span);
        unused
    }

    pub(super) fn collect_dead_stores(
        &self,
        ctx: &SemaContext<'_>,
        references: &[(Span, Span)],
        flow: &FlowModel,
    ) -> Vec<crate::compiler::AnalysisDeadStore> {
        let mut dead_stores = flow.collect_dead_stores(ctx, references);
        dead_stores.sort_by_key(|store| store.span);
        dead_stores
    }

    fn collect_module_item_reachability_nodes(
        &self,
        ctx: &SemaContext<'_>,
    ) -> std::collections::HashMap<DefId, ReachabilityItemNode> {
        let mut nodes = std::collections::HashMap::new();
        let scope_spans = self.module_item_definition_spans(ctx);

        for def in &ctx.defs {
            match def {
                kernc_sema::def::Def::Function(function) => {
                    if !self.is_lintable_free_function(ctx, function) {
                        continue;
                    }

                    if function.body.is_none() {
                        continue;
                    }
                    let is_root = function.vis == Visibility::Public
                        || function.is_extern
                        || self.item_has_export_name(&function.attributes, ctx)
                        || self.item_is_publicly_exported(function.id, function.name_span, ctx)
                        || ctx.resolve(function.name) == "main";

                    nodes.insert(
                        function.id,
                        ReachabilityItemNode {
                            def_id: function.id,
                            name_span: function.name_span,
                            kind: ReachabilityItemKind::Function,
                            is_root,
                            is_warnable: function.vis == Visibility::Private,
                        },
                    );
                }
                kernc_sema::def::Def::Global(global) => {
                    if global.is_imported {
                        continue;
                    }
                    let Some(&name_span) = scope_spans.get(&global.id) else {
                        continue;
                    };
                    let is_root = global.vis == Visibility::Public
                        || global.is_extern
                        || self.item_has_export_name(&global.attributes, ctx)
                        || self.item_is_publicly_exported(global.id, name_span, ctx);

                    nodes.insert(
                        global.id,
                        ReachabilityItemNode {
                            def_id: global.id,
                            name_span,
                            kind: if global.is_static {
                                ReachabilityItemKind::Static
                            } else {
                                ReachabilityItemKind::Constant
                            },
                            is_root,
                            is_warnable: global.vis == Visibility::Private,
                        },
                    );
                }
                _ => {}
            }
        }

        nodes
    }

    fn is_lintable_free_function(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
    ) -> bool {
        if function.is_imported || function.is_intrinsic {
            return false;
        }

        let Some(parent) = function.parent else {
            return false;
        };

        matches!(
            ctx.defs.get(parent.0 as usize),
            Some(kernc_sema::def::Def::Module(_))
        )
    }

    pub(in crate::compiler) fn module_item_definition_spans(
        &self,
        ctx: &SemaContext<'_>,
    ) -> std::collections::HashMap<DefId, Span> {
        ctx.scopes
            .all_symbols()
            .filter_map(|(_name, info)| info.def_id.map(|def_id| (def_id, info.span)))
            .collect()
    }

    fn item_has_export_name(&self, attributes: &[ast::Attribute], ctx: &SemaContext<'_>) -> bool {
        attributes.iter().any(|attribute| {
            let ast::AttributeKind::Meta(items) = &attribute.kind else {
                return false;
            };

            items.iter().any(|item| match item {
                ast::MetaItem::Call(name, _) | ast::MetaItem::Marker(name) => {
                    ctx.resolve(*name) == "export_name"
                }
            })
        })
    }

    fn item_is_publicly_exported(
        &self,
        def_id: DefId,
        definition_span: Span,
        ctx: &SemaContext<'_>,
    ) -> bool {
        ctx.scopes.all_symbols().any(|(_name, info)| {
            info.def_id == Some(def_id) && info.is_pub && info.span != definition_span
        })
    }

    fn unused_item_from_node(
        &self,
        ctx: &SemaContext<'_>,
        node: &ReachabilityItemNode,
    ) -> Option<AnalysisUnusedItem> {
        match (&ctx.defs[node.def_id.0 as usize], node.kind) {
            (kernc_sema::def::Def::Function(function), ReachabilityItemKind::Function) => {
                Some(AnalysisUnusedItem {
                    definition_span: node.name_span,
                    kind: AnalysisUnusedItemKind::Function,
                    name: ctx.resolve(function.name).to_string(),
                })
            }
            (kernc_sema::def::Def::Global(global), ReachabilityItemKind::Constant) => {
                Some(AnalysisUnusedItem {
                    definition_span: node.name_span,
                    kind: AnalysisUnusedItemKind::Constant,
                    name: ctx.resolve(global.name).to_string(),
                })
            }
            (kernc_sema::def::Def::Global(global), ReachabilityItemKind::Static) => {
                Some(AnalysisUnusedItem {
                    definition_span: node.name_span,
                    kind: AnalysisUnusedItemKind::Static,
                    name: ctx.resolve(global.name).to_string(),
                })
            }
            _ => None,
        }
    }
}

fn unused_item_message(item: &AnalysisUnusedItem) -> String {
    match item.kind {
        AnalysisUnusedItemKind::Function => {
            format!("private function `{}` is never used", item.name)
        }
        AnalysisUnusedItemKind::Constant => {
            format!("private constant `{}` is never used", item.name)
        }
        AnalysisUnusedItemKind::Static => {
            format!("private static `{}` is never used", item.name)
        }
    }
}

fn unused_binding_message(binding: &AnalysisUnusedBinding) -> String {
    match binding.kind {
        AnalysisUnusedBindingKind::Variable => {
            format!("local variable `{}` is never used", binding.name)
        }
        AnalysisUnusedBindingKind::Parameter => {
            format!("parameter `{}` is never used", binding.name)
        }
    }
}

fn dead_store_message(store: &crate::compiler::AnalysisDeadStore) -> String {
    match store.kind {
        crate::compiler::AnalysisDeadStoreKind::Initializer => {
            format!("initial value assigned to `{}` is never read", store.name)
        }
        crate::compiler::AnalysisDeadStoreKind::Assignment => {
            format!("value assigned to `{}` is never read", store.name)
        }
    }
}
