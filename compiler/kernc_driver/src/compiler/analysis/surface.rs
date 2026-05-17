use super::*;

impl CompilerDriver {
    pub(super) fn collect_analysis_symbols(
        &self,
        ctx: &SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> Vec<AnalysisSymbol> {
        let mut symbols = Vec::new();

        for (mod_id, module) in asts {
            let module_name = match &ctx.defs[mod_id.0 as usize] {
                kernc_sema::def::Def::Module(module_def) => {
                    ctx.resolve(module_def.name).to_string()
                }
                _ => continue,
            };

            let children = module
                .decls
                .iter()
                .filter_map(|decl| self.analysis_symbol_from_decl(ctx, decl))
                .collect::<Vec<_>>();

            let module_span = children
                .iter()
                .map(|symbol| symbol.span)
                .reduce(|acc, span| acc.to(span))
                .unwrap_or_default();

            symbols.push(AnalysisSymbol {
                name: module_name,
                kind: AnalysisSymbolKind::Module,
                span: module_span,
                selection_span: module_span,
                detail: Some(module.path.clone()),
                children,
            });
        }

        symbols
    }

    pub(super) fn collect_analysis_hovers(&self, ctx: &SemaContext<'_>) -> Vec<AnalysisHover> {
        let mut by_span = std::collections::BTreeMap::new();
        for (name, info) in ctx.scopes.all_symbols() {
            if !self.is_hoverable_span(ctx, info.span) {
                continue;
            }

            let Some(contents) = self.hover_contents_for_symbol(ctx, name, info) else {
                continue;
            };

            by_span.entry(info.span).or_insert(contents);
        }
        for def in &ctx.defs {
            let kernc_sema::def::Def::Function(function) = def else {
                continue;
            };
            if !self.is_hoverable_span(ctx, function.name_span) {
                continue;
            }
            let Some(contents) = self.hover_contents_for_function(ctx, function) else {
                continue;
            };

            by_span.entry(function.name_span).or_insert(contents);
        }
        self.collect_member_hovers(ctx, &mut by_span);

        by_span
            .into_iter()
            .map(|(span, contents)| AnalysisHover { span, contents })
            .collect()
    }

    pub(super) fn collect_analysis_semantic_entries(
        &self,
        symbols: &[AnalysisSymbol],
        ctx: &SemaContext<'_>,
        references: &[AnalysisReference],
    ) -> Vec<AnalysisSemanticEntry> {
        let mut definitions = std::collections::BTreeMap::new();
        for symbol in symbols {
            self.collect_symbol_semantic_entries(symbol, &mut definitions);
        }
        for definition in ctx.semantic_definitions() {
            definitions
                .entry(definition.span)
                .or_insert_with(|| self.semantic_entry_from_definition(*definition));
        }
        self.collect_named_member_semantic_entries(ctx, &mut definitions);

        let mut entries = definitions.into_values().collect::<Vec<_>>();
        let definition_by_span = entries
            .iter()
            .filter(|entry| entry.role == AnalysisSemanticRole::Definition)
            .map(|entry| (entry.definition_span, entry.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let scoped_definition_by_span = ctx
            .scopes
            .all_symbols()
            .filter_map(|(_name, info)| {
                let def_id = info.def_id?;
                let definition_span = self.canonical_scope_definition_span(def_id, info.span, ctx);
                Some((
                    info.span,
                    AnalysisSemanticEntry {
                        span: info.span,
                        definition_span,
                        kind: self.semantic_kind_from_scope_symbol_kind(info.kind, def_id, ctx),
                        role: AnalysisSemanticRole::Definition,
                        is_mut: info.is_mut,
                        is_pub: info.vis.is_public(),
                    },
                ))
            })
            .collect::<std::collections::BTreeMap<_, _>>();

        for reference in references {
            let Some(definition) = definition_by_span
                .get(&reference.definition_span)
                .or_else(|| scoped_definition_by_span.get(&reference.definition_span))
            else {
                continue;
            };
            entries.push(AnalysisSemanticEntry {
                span: reference.reference_span,
                definition_span: definition.definition_span,
                kind: definition.kind,
                role: AnalysisSemanticRole::Reference,
                is_mut: definition.is_mut,
                is_pub: definition.is_pub,
            });
        }

        entries.sort_by_key(|entry| {
            (
                entry.span.file.0,
                entry.span.start,
                entry.span.end,
                matches!(entry.role, AnalysisSemanticRole::Reference),
            )
        });
        entries
    }

    pub(super) fn collect_analysis_calls(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
        semantic_entries: &[AnalysisSemanticEntry],
        flow_model: &FlowModel,
    ) -> Vec<AnalysisCall> {
        let callable_entries = semantic_entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.role,
                    AnalysisSemanticRole::Definition | AnalysisSemanticRole::Reference
                ) && matches!(
                    entry.kind,
                    AnalysisSemanticKind::Function | AnalysisSemanticKind::Method
                )
            })
            .map(|entry| (entry.span, entry.definition_span))
            .collect::<std::collections::BTreeMap<_, _>>();
        if callable_entries.is_empty() {
            return Vec::new();
        }
        let reference_definition_spans = semantic_entries
            .iter()
            .filter(|entry| entry.role == AnalysisSemanticRole::Reference)
            .map(|entry| (entry.span, entry.definition_span))
            .collect::<std::collections::BTreeMap<_, _>>();

        let mut function_definition_spans = std::collections::HashMap::new();
        let mut function_ids_by_definition_span = std::collections::BTreeMap::new();
        let mut function_param_spans_by_def_id = std::collections::HashMap::new();
        for def in &ctx.defs {
            let kernc_sema::def::Def::Function(function) = def else {
                continue;
            };
            function_definition_spans.insert(function.id, function.name_span);
            function_ids_by_definition_span.insert(function.name_span, function.id);
            function_param_spans_by_def_id.insert(
                function.id,
                function
                    .params
                    .iter()
                    .map(|param| param.pattern.name_span)
                    .collect::<Vec<_>>(),
            );
        }

        let parameter_call_targets = InterproceduralFunctionValueFacts::collect(
            ctx,
            asts,
            &callable_entries,
            &function_ids_by_definition_span,
            &function_param_spans_by_def_id,
            flow_model,
        );

        let mut calls = Vec::new();
        for (_module_id, module) in asts {
            for decl in &module.decls {
                collect_calls_in_decl(
                    ctx,
                    decl,
                    &callable_entries,
                    &reference_definition_spans,
                    flow_model,
                    &function_definition_spans,
                    &parameter_call_targets,
                    &mut calls,
                );
            }
        }

        calls.sort_by_key(|call| {
            (
                call.call_span.file.0,
                call.call_span.start,
                call.call_span.end,
                call.callee_definition_span
                    .map(|span| span.file.0)
                    .unwrap_or(usize::MAX),
                call.callee_definition_span
                    .map(|span| span.start)
                    .unwrap_or(usize::MAX),
            )
        });
        calls.dedup();
        calls
    }

    pub(super) fn collect_analysis_definition_links(
        &self,
        ctx: &SemaContext<'_>,
    ) -> Vec<AnalysisDefinitionLink> {
        let mut links = Vec::new();

        for (_name, info) in ctx.scopes.all_symbols() {
            let Some(def_id) = info.def_id else {
                continue;
            };
            let canonical_span = self.canonical_scope_definition_span(def_id, info.span, ctx);
            if canonical_span == info.span {
                continue;
            }

            links.push(AnalysisDefinitionLink {
                definition_span: info.span,
                linked_definition_span: canonical_span,
            });
            links.push(AnalysisDefinitionLink {
                definition_span: canonical_span,
                linked_definition_span: info.span,
            });
        }

        for def in &ctx.defs {
            let kernc_sema::def::Def::Impl(impl_def) = def else {
                continue;
            };
            let Some(trait_type) = &impl_def.trait_type else {
                continue;
            };
            let Some(trait_ty) = ctx.node_type(trait_type.id) else {
                continue;
            };
            let kernc_sema::ty::TypeKind::TraitObject(trait_def_id, _, _) =
                ctx.type_registry.get(ctx.type_registry.normalize(trait_ty))
            else {
                continue;
            };
            let Some(kernc_sema::def::Def::Trait(trait_def)) =
                ctx.defs.get(trait_def_id.0 as usize)
            else {
                continue;
            };
            for method_id in &impl_def.methods {
                let Some(kernc_sema::def::Def::Function(function)) =
                    ctx.defs.get(method_id.0 as usize)
                else {
                    continue;
                };
                let Some(trait_method_span) = trait_def
                    .methods
                    .iter()
                    .find(|method| method.signature.name == function.name)
                    .map(|method| method.signature.name_span)
                else {
                    continue;
                };

                links.push(AnalysisDefinitionLink {
                    definition_span: trait_method_span,
                    linked_definition_span: function.name_span,
                });
                links.push(AnalysisDefinitionLink {
                    definition_span: function.name_span,
                    linked_definition_span: trait_method_span,
                });
            }
        }

        links.sort_by_key(|link| {
            (
                link.definition_span.file.0,
                link.definition_span.start,
                link.definition_span.end,
                link.linked_definition_span.file.0,
                link.linked_definition_span.start,
                link.linked_definition_span.end,
            )
        });
        links.dedup_by(|left, right| {
            left.definition_span == right.definition_span
                && left.linked_definition_span == right.linked_definition_span
        });
        links
    }

    fn canonical_scope_definition_span(
        &self,
        def_id: DefId,
        fallback_span: kernc_utils::Span,
        ctx: &SemaContext<'_>,
    ) -> kernc_utils::Span {
        match &ctx.defs[def_id.0 as usize] {
            kernc_sema::def::Def::Function(function) => function.name_span,
            _ => fallback_span,
        }
    }

    fn semantic_kind_from_scope_symbol_kind(
        &self,
        kind: kernc_sema::scope::SymbolKind,
        def_id: DefId,
        ctx: &SemaContext<'_>,
    ) -> AnalysisSemanticKind {
        match kind {
            kernc_sema::scope::SymbolKind::Function => {
                if matches!(
                    &ctx.defs[def_id.0 as usize],
                    kernc_sema::def::Def::Function(function) if function.parent.is_some()
                ) {
                    AnalysisSemanticKind::Method
                } else {
                    AnalysisSemanticKind::Function
                }
            }
            kernc_sema::scope::SymbolKind::Const | kernc_sema::scope::SymbolKind::ConstParam => {
                AnalysisSemanticKind::Constant
            }
            kernc_sema::scope::SymbolKind::Static => AnalysisSemanticKind::Static,
            kernc_sema::scope::SymbolKind::Var => AnalysisSemanticKind::Variable,
            kernc_sema::scope::SymbolKind::Struct | kernc_sema::scope::SymbolKind::Union => {
                AnalysisSemanticKind::Struct
            }
            kernc_sema::scope::SymbolKind::Enum => AnalysisSemanticKind::Enum,
            kernc_sema::scope::SymbolKind::Trait => AnalysisSemanticKind::Interface,
            kernc_sema::scope::SymbolKind::Module => AnalysisSemanticKind::Module,
            kernc_sema::scope::SymbolKind::TypeAlias
            | kernc_sema::scope::SymbolKind::AssociatedType => AnalysisSemanticKind::Type,
            kernc_sema::scope::SymbolKind::TypeParam => AnalysisSemanticKind::TypeParameter,
        }
    }

    fn collect_symbol_semantic_entries(
        &self,
        symbol: &AnalysisSymbol,
        definitions: &mut std::collections::BTreeMap<kernc_utils::Span, AnalysisSemanticEntry>,
    ) {
        definitions
            .entry(symbol.selection_span)
            .or_insert_with(|| AnalysisSemanticEntry {
                span: symbol.selection_span,
                definition_span: symbol.selection_span,
                kind: self.semantic_kind_from_symbol_kind(symbol.kind),
                role: AnalysisSemanticRole::Definition,
                is_mut: false,
                is_pub: true,
            });
        for child in &symbol.children {
            self.collect_symbol_semantic_entries(child, definitions);
        }
    }

    fn semantic_entry_from_definition(
        &self,
        definition: SemanticDefinition,
    ) -> AnalysisSemanticEntry {
        AnalysisSemanticEntry {
            span: definition.span,
            definition_span: definition.span,
            kind: self.semantic_kind_from_sema_kind(definition.kind),
            role: AnalysisSemanticRole::Definition,
            is_mut: definition.is_mut,
            is_pub: definition.is_pub,
        }
    }

    fn semantic_kind_from_symbol_kind(&self, kind: AnalysisSymbolKind) -> AnalysisSemanticKind {
        match kind {
            AnalysisSymbolKind::Module => AnalysisSemanticKind::Module,
            AnalysisSymbolKind::Namespace => AnalysisSemanticKind::Namespace,
            AnalysisSymbolKind::Function => AnalysisSemanticKind::Function,
            AnalysisSymbolKind::Method => AnalysisSemanticKind::Method,
            AnalysisSymbolKind::Struct | AnalysisSymbolKind::Union => AnalysisSemanticKind::Struct,
            AnalysisSymbolKind::Enum => AnalysisSemanticKind::Enum,
            AnalysisSymbolKind::Trait => AnalysisSemanticKind::Interface,
            AnalysisSymbolKind::TypeAlias => AnalysisSemanticKind::Type,
            AnalysisSymbolKind::Constant => AnalysisSemanticKind::Constant,
            AnalysisSymbolKind::Static => AnalysisSemanticKind::Static,
        }
    }

    fn semantic_kind_from_sema_kind(&self, kind: SemanticSymbolKind) -> AnalysisSemanticKind {
        match kind {
            SemanticSymbolKind::Module => AnalysisSemanticKind::Module,
            SemanticSymbolKind::Namespace => AnalysisSemanticKind::Namespace,
            SemanticSymbolKind::Struct | SemanticSymbolKind::Union => AnalysisSemanticKind::Struct,
            SemanticSymbolKind::Enum => AnalysisSemanticKind::Enum,
            SemanticSymbolKind::Trait => AnalysisSemanticKind::Interface,
            SemanticSymbolKind::TypeAlias => AnalysisSemanticKind::Type,
            SemanticSymbolKind::TypeParameter => AnalysisSemanticKind::TypeParameter,
            SemanticSymbolKind::Variable => AnalysisSemanticKind::Variable,
            SemanticSymbolKind::Function => AnalysisSemanticKind::Function,
            SemanticSymbolKind::Method => AnalysisSemanticKind::Method,
            SemanticSymbolKind::Constant => AnalysisSemanticKind::Constant,
            SemanticSymbolKind::Static => AnalysisSemanticKind::Static,
            SemanticSymbolKind::Parameter => AnalysisSemanticKind::Parameter,
        }
    }

    fn collect_named_member_semantic_entries(
        &self,
        ctx: &SemaContext<'_>,
        definitions: &mut std::collections::BTreeMap<kernc_utils::Span, AnalysisSemanticEntry>,
    ) {
        for def in &ctx.defs {
            match def {
                kernc_sema::def::Def::Struct(struct_def) => {
                    for field in &struct_def.fields {
                        definitions
                            .entry(field.name_span)
                            .or_insert(AnalysisSemanticEntry {
                                span: field.name_span,
                                definition_span: field.name_span,
                                kind: AnalysisSemanticKind::Property,
                                role: AnalysisSemanticRole::Definition,
                                is_mut: false,
                                is_pub: field.vis.is_public(),
                            });
                    }
                }
                kernc_sema::def::Def::Union(union_def) => {
                    for field in &union_def.fields {
                        definitions
                            .entry(field.name_span)
                            .or_insert(AnalysisSemanticEntry {
                                span: field.name_span,
                                definition_span: field.name_span,
                                kind: AnalysisSemanticKind::Property,
                                role: AnalysisSemanticRole::Definition,
                                is_mut: false,
                                is_pub: field.vis.is_public(),
                            });
                    }
                }
                kernc_sema::def::Def::Trait(trait_def) => {
                    for method in &trait_def.methods {
                        definitions.entry(method.signature.name_span).or_insert(
                            AnalysisSemanticEntry {
                                span: method.signature.name_span,
                                definition_span: method.signature.name_span,
                                kind: AnalysisSemanticKind::Method,
                                role: AnalysisSemanticRole::Definition,
                                is_mut: false,
                                is_pub: true,
                            },
                        );
                    }
                }
                kernc_sema::def::Def::Enum(enum_def) => {
                    for variant in &enum_def.variants {
                        definitions
                            .entry(variant.name_span)
                            .or_insert(AnalysisSemanticEntry {
                                span: variant.name_span,
                                definition_span: variant.name_span,
                                kind: AnalysisSemanticKind::Enum,
                                role: AnalysisSemanticRole::Definition,
                                is_mut: false,
                                is_pub: true,
                            });
                    }
                }
                _ => {}
            }
        }
    }

    fn hover_contents_for_symbol(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        info: &kernc_sema::scope::SymbolInfo,
    ) -> Option<String> {
        let name = ctx.resolve(name);

        let code = match info.kind {
            kernc_sema::scope::SymbolKind::Function => {
                let def_id = info.def_id?;
                let kernc_sema::def::Def::Function(function) = &ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                self.function_decl_signature(ctx, function, name)
            }
            kernc_sema::scope::SymbolKind::Const => {
                format!(
                    "const {}: {}",
                    name,
                    self.hover_type_label(ctx, info.type_id)
                )
            }
            kernc_sema::scope::SymbolKind::ConstParam => {
                format!(
                    "const {}: {}",
                    name,
                    self.hover_type_label(ctx, info.type_id)
                )
            }
            kernc_sema::scope::SymbolKind::Static => {
                let prefix = if info.is_mut { "static mut" } else { "static" };
                format!(
                    "{} {}: {}",
                    prefix,
                    name,
                    self.hover_type_label(ctx, info.type_id)
                )
            }
            kernc_sema::scope::SymbolKind::Var => {
                let prefix = if info.is_mut { "let mut" } else { "let" };
                format!(
                    "{} {}: {}",
                    prefix,
                    name,
                    self.hover_type_label(ctx, info.type_id)
                )
            }
            kernc_sema::scope::SymbolKind::Struct => format!("struct {}", name),
            kernc_sema::scope::SymbolKind::Union => format!("union {}", name),
            kernc_sema::scope::SymbolKind::Enum => format!("enum {}", name),
            kernc_sema::scope::SymbolKind::Trait => format!("trait {}", name),
            kernc_sema::scope::SymbolKind::Module => format!("module {}", name),
            kernc_sema::scope::SymbolKind::TypeAlias
            | kernc_sema::scope::SymbolKind::AssociatedType => {
                let detail = if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                {
                    ctx.node_type(alias.target.id)
                        .map(|target_ty| self.hover_type_label(ctx, target_ty))
                } else if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::AssociatedType(assoc) =
                        &ctx.defs[def_id.0 as usize]
                    && let Some(target) = assoc.target.as_ref()
                {
                    ctx.node_type(target.id)
                        .map(|target_ty| self.hover_type_label(ctx, target_ty))
                } else {
                    Some(self.hover_type_label(ctx, info.type_id))
                };

                if let Some(detail) = detail {
                    format!("type {} = {}", name, detail)
                } else {
                    format!("type {}", name)
                }
            }
            kernc_sema::scope::SymbolKind::TypeParam => format!("type {}", name),
        };

        Some(render_hover_markdown(
            &code,
            info.def_id
                .and_then(|def_id| self.doc_block_for_def(ctx, def_id)),
        ))
    }

    fn hover_contents_for_function(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
    ) -> Option<String> {
        let name = ctx.resolve(function.name);
        let code = self.function_decl_signature(ctx, function, name);
        Some(render_hover_markdown(&code, function.docs.as_ref()))
    }

    fn function_decl_signature(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
        name: &str,
    ) -> String {
        let mut out = String::new();
        if function.is_extern {
            out.push_str("extern ");
        }
        if function.is_const {
            out.push_str("const ");
        }
        out.push_str("fn ");
        out.push_str(name);
        out.push_str(&generic_params_label_local(ctx, &function.generics));
        out.push('(');

        let mut params = Vec::new();
        for param in &function.params {
            params.push(format!(
                "{}: {}",
                ctx.resolve(param.pattern.name),
                self.hover_type_node_label(ctx, &param.type_node)
            ));
        }
        if function.is_variadic {
            params.push("...".to_string());
        }
        out.push_str(&params.join(", "));
        out.push(')');
        out.push(' ');
        out.push_str(&self.hover_type_node_label(ctx, &function.ret_type));
        out
    }

    fn function_type_signature(
        &self,
        ctx: &SemaContext<'_>,
        name: &str,
        type_node: &ast::TypeNode,
    ) -> String {
        let ast::TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = &type_node.kind
        else {
            return format!(
                "fn {}: {}",
                name,
                self.hover_type_node_label(ctx, type_node)
            );
        };

        let mut out = String::new();
        out.push_str("fn ");
        out.push_str(name);
        out.push('(');

        let mut rendered_params = Vec::new();
        for param in params {
            rendered_params.push(self.hover_type_node_label(ctx, param));
        }
        if *is_variadic {
            rendered_params.push("...".to_string());
        }
        out.push_str(&rendered_params.join(", "));
        out.push(')');
        out.push(' ');
        if let Some(ret) = ret {
            out.push_str(&self.hover_type_node_label(ctx, ret));
        } else {
            out.push_str("void");
        }
        out
    }

    fn hover_type_node_label(&self, ctx: &SemaContext<'_>, type_node: &ast::TypeNode) -> String {
        if let Some(ty) = ctx.node_type(type_node.id) {
            return ctx.ty_to_string(ty);
        }
        ctx.sess
            .source_manager
            .slice_source(type_node.span)
            .trim()
            .to_string()
    }

    fn collect_member_hovers(
        &self,
        ctx: &SemaContext<'_>,
        by_span: &mut std::collections::BTreeMap<kernc_utils::Span, String>,
    ) {
        for def in &ctx.defs {
            match def {
                kernc_sema::def::Def::Struct(struct_def) => {
                    for field in &struct_def.fields {
                        if !self.is_hoverable_span(ctx, field.name_span) {
                            continue;
                        }
                        let Some(contents) =
                            self.hover_contents_for_field(ctx, field.name, field.type_node.id)
                        else {
                            continue;
                        };
                        by_span.entry(field.name_span).or_insert(contents);
                    }
                }
                kernc_sema::def::Def::Union(union_def) => {
                    for field in &union_def.fields {
                        if !self.is_hoverable_span(ctx, field.name_span) {
                            continue;
                        }
                        let Some(contents) =
                            self.hover_contents_for_field(ctx, field.name, field.type_node.id)
                        else {
                            continue;
                        };
                        by_span.entry(field.name_span).or_insert(contents);
                    }
                }
                kernc_sema::def::Def::Trait(trait_def) => {
                    for method in &trait_def.methods {
                        if !self.is_hoverable_span(ctx, method.signature.name_span) {
                            continue;
                        }
                        let Some(contents) = self.hover_contents_for_trait_method(
                            ctx,
                            method.signature.name,
                            &method.signature.type_node,
                        ) else {
                            continue;
                        };
                        by_span
                            .entry(method.signature.name_span)
                            .or_insert(contents);
                    }
                }
                kernc_sema::def::Def::Enum(enum_def) => {
                    for variant in &enum_def.variants {
                        if !self.is_hoverable_span(ctx, variant.name_span) {
                            continue;
                        }
                        let Some(contents) = self.hover_contents_for_enum_variant(ctx, variant)
                        else {
                            continue;
                        };
                        by_span.entry(variant.name_span).or_insert(contents);
                    }
                }
                _ => {}
            }
        }
    }

    fn hover_type_label(&self, ctx: &SemaContext<'_>, ty: kernc_sema::ty::TypeId) -> String {
        match ctx
            .type_registry
            .get(ctx.type_registry.normalize(ty))
            .clone()
        {
            kernc_sema::ty::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut out = String::from("fn(");
                let mut rendered_params = params
                    .iter()
                    .map(|param| ctx.ty_to_string(*param))
                    .collect::<Vec<_>>();
                if is_variadic {
                    rendered_params.push("...".to_string());
                }
                out.push_str(&rendered_params.join(", "));
                out.push_str(") ");
                out.push_str(&ctx.ty_to_string(ret));
                out
            }
            kernc_sema::ty::TypeKind::ClosureInterface { params, ret } => {
                let mut out = String::from("Fn(");
                let rendered_params = params
                    .iter()
                    .map(|param| ctx.ty_to_string(*param))
                    .collect::<Vec<_>>();
                out.push_str(&rendered_params.join(", "));
                out.push_str(") ");
                out.push_str(&ctx.ty_to_string(ret));
                out
            }
            _ => ctx.ty_to_string(ty),
        }
    }

    fn hover_contents_for_field(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<String> {
        let ty = ctx.node_type(type_node_id)?;
        Some(render_hover_markdown(
            &format!("field {}: {}", ctx.resolve(name), ctx.ty_to_string(ty)),
            self.field_doc_block(ctx, name, type_node_id),
        ))
    }

    fn hover_contents_for_trait_method(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node: &ast::TypeNode,
    ) -> Option<String> {
        let rendered = self.function_type_signature(ctx, ctx.resolve(name), type_node);
        Some(render_hover_markdown(
            &rendered,
            self.trait_method_doc_block(ctx, name, type_node.id),
        ))
    }

    fn hover_contents_for_enum_variant(
        &self,
        ctx: &SemaContext<'_>,
        variant: &ast::EnumVariant,
    ) -> Option<String> {
        let name = ctx.resolve(variant.name);
        if let Some(payload_type) = &variant.payload_type {
            let ty = ctx.node_type(payload_type.id)?;
            Some(render_hover_markdown(
                &format!("variant {}: {}", name, ctx.ty_to_string(ty)),
                variant.docs.as_ref(),
            ))
        } else {
            Some(render_hover_markdown(
                &format!("variant {}", name),
                variant.docs.as_ref(),
            ))
        }
    }

    fn doc_block_for_def<'a>(
        &self,
        ctx: &'a SemaContext<'_>,
        def_id: DefId,
    ) -> Option<&'a ast::DocBlock> {
        match &ctx.defs[def_id.0 as usize] {
            kernc_sema::def::Def::Module(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Function(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Struct(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Union(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Enum(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Trait(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Global(def) => def.docs.as_ref(),
            kernc_sema::def::Def::AssociatedType(def) => def.docs.as_ref(),
            kernc_sema::def::Def::TypeAlias(def) => def.docs.as_ref(),
            kernc_sema::def::Def::Impl(_) => None,
        }
    }

    fn field_doc_block<'a>(
        &self,
        ctx: &'a SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<&'a ast::DocBlock> {
        for def in &ctx.defs {
            match def {
                kernc_sema::def::Def::Struct(struct_def) => {
                    for field in &struct_def.fields {
                        if field.name == name && field.type_node.id == type_node_id {
                            return field.docs.as_ref();
                        }
                    }
                }
                kernc_sema::def::Def::Union(union_def) => {
                    for field in &union_def.fields {
                        if field.name == name && field.type_node.id == type_node_id {
                            return field.docs.as_ref();
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn trait_method_doc_block<'a>(
        &self,
        ctx: &'a SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<&'a ast::DocBlock> {
        for def in &ctx.defs {
            let kernc_sema::def::Def::Trait(trait_def) = def else {
                continue;
            };
            for method in &trait_def.methods {
                if method.signature.name == name && method.signature.type_node.id == type_node_id {
                    return method.signature.docs.as_ref();
                }
            }
        }
        None
    }

    fn is_hoverable_span(&self, ctx: &SemaContext<'_>, span: kernc_utils::Span) -> bool {
        span.end > span.start && ctx.sess.source_manager.get_file(span.file).is_some()
    }

    fn analysis_symbol_from_decl(
        &self,
        ctx: &SemaContext<'_>,
        decl: &ast::Decl,
    ) -> Option<AnalysisSymbol> {
        let name = ctx.resolve(decl.name).to_string();
        match &decl.kind {
            ast::DeclKind::Function { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Function,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Var { is_static, .. } => Some(AnalysisSymbol {
                name,
                kind: if *is_static {
                    AnalysisSymbolKind::Static
                } else {
                    AnalysisSymbolKind::Constant
                },
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::TypeAlias { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::TypeAlias,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Struct { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Struct,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Union { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Union,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Enum { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Enum,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Trait { .. } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Trait,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::Mod { decls } => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: decls
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|child| self.analysis_symbol_from_decl(ctx, child))
                    .collect(),
            }),
            ast::DeclKind::ExternBlock { decls, .. } => Some(AnalysisSymbol {
                name: "extern".to_string(),
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: decls
                    .iter()
                    .filter_map(|child| self.analysis_symbol_from_decl(ctx, child))
                    .collect(),
            }),
            ast::DeclKind::Impl {
                target_type,
                trait_type,
                decls,
                ..
            } => Some(AnalysisSymbol {
                name: self.describe_impl_symbol(ctx, target_type, trait_type.as_ref()),
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.name_span,
                detail: Some("impl".to_string()),
                children: decls
                    .iter()
                    .filter_map(|child| self.analysis_symbol_from_impl_decl(ctx, child))
                    .collect(),
            }),
            ast::DeclKind::Use { .. } => None,
        }
    }

    fn analysis_symbol_from_impl_decl(
        &self,
        ctx: &SemaContext<'_>,
        decl: &ast::Decl,
    ) -> Option<AnalysisSymbol> {
        let mut symbol = self.analysis_symbol_from_decl(ctx, decl)?;
        if matches!(symbol.kind, AnalysisSymbolKind::Function) {
            symbol.kind = AnalysisSymbolKind::Method;
        }
        Some(symbol)
    }

    fn describe_impl_symbol(
        &self,
        ctx: &SemaContext<'_>,
        target_type: &ast::TypeNode,
        trait_type: Option<&ast::TypeNode>,
    ) -> String {
        let target = self.describe_type_node(ctx, target_type);
        if let Some(trait_type) = trait_type {
            format!(
                "impl {} : {}",
                target,
                self.describe_type_node(ctx, trait_type)
            )
        } else {
            format!("impl {}", target)
        }
    }

    fn describe_type_node(&self, ctx: &SemaContext<'_>, ty: &ast::TypeNode) -> String {
        if let Some(ty_id) = ctx.node_type(ty.id) {
            return ctx.ty_to_string(ty_id);
        }

        ctx.sess
            .source_manager
            .slice_source(ty.span)
            .trim()
            .to_string()
    }

    pub(super) fn collect_trait_impl_stubs(
        &self,
        ctx: &SemaContext<'_>,
    ) -> Vec<crate::AnalysisTraitImplStub> {
        let mut stubs = Vec::new();
        for def in &ctx.defs {
            let kernc_sema::def::Def::Impl(impl_def) = def else {
                continue;
            };
            if impl_def.is_imported {
                continue;
            }
            let Some(trait_ty) = impl_def.resolved_trait_ty else {
                continue;
            };
            let trait_ty = ctx.type_registry.normalize(trait_ty);
            let TypeKind::TraitObject(trait_def_id, _, _) = ctx.type_registry.get(trait_ty).clone()
            else {
                continue;
            };
            let Some(kernc_sema::def::Def::Trait(trait_def)) =
                ctx.defs.get(trait_def_id.0 as usize)
            else {
                continue;
            };

            let implemented_methods = impl_def
                .methods
                .iter()
                .filter_map(|method_id| match ctx.defs.get(method_id.0 as usize) {
                    Some(kernc_sema::def::Def::Function(function)) => Some(function.name),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>();
            let Some(file) = ctx.sess.source_manager.get_file(impl_def.span.file) else {
                continue;
            };
            let Some(insertion_offset) = impl_body_end_insertion_offset(file, impl_def.span) else {
                continue;
            };
            let indent = impl_member_indent(file, insertion_offset);

            for method in &trait_def.methods {
                if method.default_impl.is_some()
                    || implemented_methods.contains(&method.signature.name)
                {
                    continue;
                }
                let method_name = ctx.resolve(method.signature.name).to_string();
                stubs.push(crate::AnalysisTraitImplStub {
                    impl_span: impl_def.span,
                    method_name,
                    insertion_offset,
                    insert_text: trait_impl_stub_text(ctx, method, &indent),
                });
            }
        }

        stubs.sort_by_key(|stub| {
            (
                stub.impl_span.file.0,
                stub.impl_span.start,
                stub.impl_span.end,
                stub.method_name.clone(),
            )
        });
        stubs
    }
}

fn generic_params_label_local(ctx: &SemaContext<'_>, generics: &[ast::GenericParam]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let names = generics
        .iter()
        .map(|param| match &param.kind {
            ast::GenericParamKind::Type => ctx.resolve(param.name).to_string(),
            ast::GenericParamKind::Const { ty } => {
                format!(
                    "{}: {}",
                    ctx.resolve(param.name),
                    ctx.sess.source_manager.slice_source(ty.span).trim()
                )
            }
        })
        .collect::<Vec<_>>();
    format!("[{}]", names.join(", "))
}

fn trait_impl_stub_text(
    ctx: &SemaContext<'_>,
    method: &kernc_sema::def::TraitMethodDef,
    indent: &str,
) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(indent);
    out.push_str("fn ");
    out.push_str(ctx.resolve(method.signature.name));
    out.push('(');

    let mut params = Vec::new();
    for param in &method.params {
        params.push(format!(
            "{}: {}",
            ctx.resolve(param.pattern.name),
            type_node_source(ctx, &param.type_node)
        ));
    }
    if let ast::TypeKind::Function { is_variadic, .. } = &method.signature.type_node.kind
        && *is_variadic
    {
        params.push("...".to_string());
    }
    out.push_str(&params.join(", "));
    out.push(')');
    let ret_type = trait_method_return_type_source(ctx, &method.signature.type_node);
    if ret_type != "void" {
        out.push(' ');
        out.push_str(&ret_type);
    } else {
        out.push_str(" void");
    }
    out.push_str(" {\n");
    out.push_str(indent);
    out.push_str("    @unreachable();\n");
    out.push_str(indent);
    out.push_str("}\n");
    out
}

fn trait_method_return_type_source(ctx: &SemaContext<'_>, type_node: &ast::TypeNode) -> String {
    match &type_node.kind {
        ast::TypeKind::Function { ret: Some(ret), .. } => type_node_source(ctx, ret),
        ast::TypeKind::Function { ret: None, .. } => "void".to_string(),
        _ => type_node_source(ctx, type_node),
    }
}

fn type_node_source(ctx: &SemaContext<'_>, type_node: &ast::TypeNode) -> String {
    let source = ctx.sess.source_manager.slice_source(type_node.span).trim();
    if source.is_empty() {
        return ctx
            .node_type(type_node.id)
            .map(|ty| ctx.ty_to_string(ty))
            .unwrap_or_else(|| "void".to_string());
    }
    source.to_string()
}

fn impl_body_end_insertion_offset(file: &kernc_utils::SourceFile, span: Span) -> Option<usize> {
    if span.end > file.src.len() || span.start >= span.end {
        return None;
    }
    let close_relative = file.src.get(span.start..span.end)?.rfind('}')?;
    Some(span.start + close_relative)
}

fn impl_member_indent(file: &kernc_utils::SourceFile, insertion_offset: usize) -> String {
    let line_start = file.src[..insertion_offset]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let close_indent = file.src[line_start..insertion_offset]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .collect::<String>();
    format!("{}    ", close_indent)
}

fn collect_calls_in_decl(
    ctx: &mut SemaContext<'_>,
    decl: &ast::Decl,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    reference_definition_spans: &std::collections::BTreeMap<Span, Span>,
    flow_model: &FlowModel,
    function_definition_spans: &std::collections::HashMap<DefId, Span>,
    parameter_call_targets: &InterproceduralFunctionValueFacts,
    calls: &mut Vec<AnalysisCall>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            let indirect_call_targets =
                IndirectCallTargetFacts::collect(body, callable_entries, flow_model);
            collect_calls_in_expr(
                ctx,
                body,
                callable_entries,
                &indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::DeclKind::Var {
            value: Some(value), ..
        } => {
            let indirect_call_targets =
                IndirectCallTargetFacts::collect(value, callable_entries, flow_model);
            collect_calls_in_expr(
                ctx,
                value,
                callable_entries,
                &indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                collect_calls_in_decl(
                    ctx,
                    child,
                    callable_entries,
                    reference_definition_spans,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::DeclKind::Mod { decls: Some(decls) } => {
            for child in decls {
                collect_calls_in_decl(
                    ctx,
                    child,
                    callable_entries,
                    reference_definition_spans,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_calls_in_decl(
                    ctx,
                    child,
                    callable_entries,
                    reference_definition_spans,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        _ => {}
    }
}

fn analysis_call_kind(ctx: &SemaContext<'_>, callee: &ast::Expr) -> Option<AnalysisCallKind> {
    let Some(callee_ty) = ctx.node_type(callee.id) else {
        return None;
    };
    let callee_ty = ctx.type_registry.normalize(callee_ty);
    if matches!(
        ctx.type_registry.get(callee_ty),
        TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. }
            if matches!(
                ctx.type_registry.get(ctx.type_registry.normalize(*elem)),
                TypeKind::ClosureInterface { .. } | TypeKind::Function { .. }
            )
    ) {
        return Some(AnalysisCallKind::Indirect);
    }
    if matches!(
        ctx.type_registry.get(callee_ty),
        TypeKind::AnonymousState { .. }
    ) {
        return Some(AnalysisCallKind::Indirect);
    }
    if matches!(ctx.type_registry.get(callee_ty), TypeKind::Function { .. })
        && let Some(owner_ty) = ctx.method_owner_ty(callee.id)
        && matches!(
            ctx.type_registry.get(ctx.type_registry.normalize(owner_ty)),
            TypeKind::TraitObject(..)
        )
    {
        return Some(AnalysisCallKind::DynamicDispatch);
    }

    if matches!(
        ctx.type_registry.get(callee_ty),
        TypeKind::Function { .. } | TypeKind::FnDef(..)
    ) && let Some(owner_ty) = ctx.method_owner_ty(callee.id)
        && matches!(
            ctx.type_registry.get(ctx.type_registry.normalize(owner_ty)),
            TypeKind::TraitObject(..)
        )
    {
        return Some(AnalysisCallKind::Direct);
    }

    if matches!(
        ctx.type_registry.get(callee_ty),
        TypeKind::Function { .. } | TypeKind::FnDef(..)
    ) {
        Some(AnalysisCallKind::Direct)
    } else {
        None
    }
}

fn dynamic_dispatch_targets(ctx: &mut SemaContext<'_>, callee: &ast::Expr) -> Vec<Span> {
    if analysis_call_kind(ctx, callee) != Some(AnalysisCallKind::DynamicDispatch) {
        return Vec::new();
    }

    let Some(method_name) = callee_method_name(callee) else {
        return Vec::new();
    };
    let Some(owner_ty) = ctx.method_owner_ty(callee.id) else {
        return Vec::new();
    };
    let owner_ty = ctx.normalize_concrete_type(owner_ty);
    let owner_ty = ctx.type_registry.normalize(owner_ty);
    let TypeKind::TraitObject(owner_trait_id, owner_trait_args, _) =
        ctx.type_registry.get(owner_ty).clone()
    else {
        return Vec::new();
    };

    let mut targets = Vec::new();
    for entry in ctx.trait_impl_entries() {
        let Some(impl_trait_ty) = entry.def.resolved_trait_ty.or_else(|| {
            entry
                .def
                .trait_type
                .as_ref()
                .and_then(|ty| ctx.node_type(ty.id))
        }) else {
            continue;
        };
        let Some(owner_view) = kernc_sema::query::declared_trait_object_view_from_hierarchy(
            ctx,
            impl_trait_ty,
            owner_trait_id,
            &owner_trait_args,
        ) else {
            continue;
        };
        if !trait_object_satisfies_required(ctx, owner_view, owner_ty) {
            continue;
        }
        let Some(function_span) =
            dispatch_target_span_for_impl(ctx, &entry.def, owner_trait_id, method_name)
        else {
            continue;
        };
        targets.push(function_span);
    }

    targets.sort_by_key(|span| (span.file.0, span.start, span.end));
    targets.dedup();
    targets
}

fn trait_object_satisfies_required(
    ctx: &mut SemaContext<'_>,
    available_trait_ty: TypeId,
    required_trait_ty: TypeId,
) -> bool {
    let available_norm = ctx.normalize_concrete_type(available_trait_ty);
    let available_norm = ctx.type_registry.normalize(available_norm);
    let required_norm = ctx.normalize_concrete_type(required_trait_ty);
    let required_norm = ctx.type_registry.normalize(required_norm);

    let (
        TypeKind::TraitObject(available_def_id, available_args, available_assoc_bindings),
        TypeKind::TraitObject(required_def_id, required_args, required_assoc_bindings),
    ) = (
        ctx.type_registry.get(available_norm).clone(),
        ctx.type_registry.get(required_norm).clone(),
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
        .collect::<std::collections::HashMap<_, _>>();
    required_assoc_bindings
        .into_iter()
        .all(|(assoc_def_id, required_assoc_ty)| {
            available_assoc_bindings
                .get(&assoc_def_id)
                .is_some_and(|available_assoc_ty| {
                    ctx.type_registry.normalize(*available_assoc_ty)
                        == ctx.type_registry.normalize(required_assoc_ty)
                })
        })
}

fn dispatch_target_span_for_impl(
    ctx: &SemaContext<'_>,
    impl_def: &kernc_sema::def::ImplDef,
    owner_trait_id: DefId,
    method_name: kernc_utils::SymbolId,
) -> Option<Span> {
    for &method_id in &impl_def.methods {
        let Some(kernc_sema::def::Def::Function(function)) = ctx.defs.get(method_id.0 as usize)
        else {
            continue;
        };
        if function.name == method_name {
            return Some(function.name_span);
        }
    }

    let Some(kernc_sema::def::Def::Trait(trait_def)) = ctx.defs.get(owner_trait_id.0 as usize)
    else {
        return None;
    };
    let default_method_id = trait_def
        .methods
        .iter()
        .find(|method| method.signature.name == method_name)
        .and_then(|method| method.default_impl)?;
    let Some(kernc_sema::def::Def::Function(default_method)) =
        ctx.defs.get(default_method_id.0 as usize)
    else {
        return None;
    };
    Some(default_method.name_span)
}

fn callee_method_name(callee: &ast::Expr) -> Option<kernc_utils::SymbolId> {
    match &callee.kind {
        ast::ExprKind::FieldAccess { field, .. } => Some(*field),
        ast::ExprKind::GenericInstantiation { target, .. } => callee_method_name(target),
        _ => None,
    }
}

#[derive(Default)]
struct InterproceduralFunctionValueFacts {
    facts_by_parameter_span: std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
}

#[derive(Clone, Default)]
struct ParameterFunctionValueFact {
    targets: std::collections::BTreeSet<Span>,
    saw_partial_source: bool,
    saw_unknown_source: bool,
}

impl ParameterFunctionValueFact {
    fn add_sources(&mut self, sources: &FunctionValueSources) -> bool {
        let mut changed = false;
        for source in &sources.sources {
            if let FunctionValueSource::Target(target) = source {
                changed |= self.targets.insert(*target);
            }
        }
        changed | self.merge_completeness(sources.completeness)
    }

    fn add_fact(&mut self, fact: &Self) -> bool {
        let mut changed = false;
        for target in &fact.targets {
            changed |= self.targets.insert(*target);
        }
        changed |= self.set_partial(fact.saw_partial_source);
        changed |= self.set_unknown(fact.saw_unknown_source);
        changed
    }

    fn add_unknown_source(&mut self) -> bool {
        self.set_unknown(true)
    }

    fn merge_completeness(&mut self, completeness: AnalysisCallTargetCompleteness) -> bool {
        match completeness {
            AnalysisCallTargetCompleteness::Exact => false,
            AnalysisCallTargetCompleteness::Partial => self.set_partial(true),
            AnalysisCallTargetCompleteness::Unknown => self.set_unknown(true),
        }
    }

    fn set_partial(&mut self, value: bool) -> bool {
        let changed = value && !self.saw_partial_source;
        self.saw_partial_source |= value;
        changed
    }

    fn set_unknown(&mut self, value: bool) -> bool {
        let changed = value && !self.saw_unknown_source;
        self.saw_unknown_source |= value;
        changed
    }

    fn into_targets(self) -> ParameterFunctionValueTargets {
        let targets = self.targets.into_iter().collect::<Vec<_>>();
        let completeness = if self.saw_unknown_source && targets.is_empty() {
            AnalysisCallTargetCompleteness::Unknown
        } else if self.saw_unknown_source || self.saw_partial_source {
            AnalysisCallTargetCompleteness::Partial
        } else {
            AnalysisCallTargetCompleteness::Exact
        };
        ParameterFunctionValueTargets {
            targets,
            completeness,
        }
    }
}

struct ParameterFunctionValueTargets {
    targets: Vec<Span>,
    completeness: AnalysisCallTargetCompleteness,
}

impl InterproceduralFunctionValueFacts {
    fn collect(
        ctx: &mut SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
        callable_entries: &std::collections::BTreeMap<Span, Span>,
        function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
        function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
        flow_model: &FlowModel,
    ) -> Self {
        let mut facts_by_parameter_span =
            std::collections::BTreeMap::<Span, ParameterFunctionValueFact>::new();
        let mut parameter_edges = Vec::<(Span, Span)>::new();

        for (_module_id, module) in asts {
            for decl in &module.decls {
                collect_interprocedural_function_value_edges_in_decl(
                    ctx,
                    decl,
                    callable_entries,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    flow_model,
                    &mut facts_by_parameter_span,
                    &mut parameter_edges,
                );
            }
        }

        let mut changed = true;
        while changed {
            changed = false;
            for (source_parameter, target_parameter) in &parameter_edges {
                let source_fact = facts_by_parameter_span.get(source_parameter).cloned();
                let target_fact = facts_by_parameter_span
                    .entry(*target_parameter)
                    .or_default();
                if let Some(source_fact) = source_fact {
                    changed |= target_fact.add_fact(&source_fact);
                } else {
                    changed |= target_fact.add_unknown_source();
                }
            }
        }

        Self {
            facts_by_parameter_span,
        }
    }

    fn targets_for_parameter(&self, parameter_span: Span) -> ParameterFunctionValueTargets {
        self.facts_by_parameter_span
            .get(&parameter_span)
            .cloned()
            .map(ParameterFunctionValueFact::into_targets)
            .unwrap_or(ParameterFunctionValueTargets {
                targets: Vec::new(),
                completeness: AnalysisCallTargetCompleteness::Unknown,
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FunctionValueSource {
    Target(Span),
    Parameter(Span),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallableValueDefinitionSource {
    Target(Span),
    Reference(Span),
}

struct FunctionValueSources {
    sources: Vec<FunctionValueSource>,
    completeness: AnalysisCallTargetCompleteness,
}

impl FunctionValueSources {
    fn exact(source: FunctionValueSource) -> Self {
        Self {
            sources: vec![source],
            completeness: AnalysisCallTargetCompleteness::Exact,
        }
    }

    fn unknown() -> Self {
        Self {
            sources: Vec::new(),
            completeness: AnalysisCallTargetCompleteness::Unknown,
        }
    }

    fn merged(
        parts: impl IntoIterator<Item = Self>,
        completeness: AnalysisCallTargetCompleteness,
    ) -> Self {
        let mut sources = Vec::new();
        let mut saw_partial = completeness == AnalysisCallTargetCompleteness::Partial;
        let mut saw_unknown = completeness == AnalysisCallTargetCompleteness::Unknown;
        for part in parts {
            sources.extend(part.sources);
            match part.completeness {
                AnalysisCallTargetCompleteness::Exact => {}
                AnalysisCallTargetCompleteness::Partial => saw_partial = true,
                AnalysisCallTargetCompleteness::Unknown => saw_unknown = true,
            }
        }
        let result_completeness = if saw_partial || saw_unknown {
            AnalysisCallTargetCompleteness::Partial
        } else {
            AnalysisCallTargetCompleteness::Exact
        };
        Self::with_completeness(&mut sources, result_completeness)
    }

    fn with_completeness(
        sources: &mut Vec<FunctionValueSource>,
        completeness: AnalysisCallTargetCompleteness,
    ) -> Self {
        sources.sort();
        sources.dedup();
        if sources.is_empty() {
            Self::unknown()
        } else {
            Self {
                sources: std::mem::take(sources),
                completeness,
            }
        }
    }
}

struct IndirectCallTargets {
    targets: Vec<Span>,
    completeness: AnalysisCallTargetCompleteness,
}

impl IndirectCallTargets {
    fn unknown() -> Self {
        Self {
            targets: Vec::new(),
            completeness: AnalysisCallTargetCompleteness::Unknown,
        }
    }

    fn exact(target: Span) -> Self {
        Self {
            targets: vec![target],
            completeness: AnalysisCallTargetCompleteness::Exact,
        }
    }

    fn partial(mut targets: Vec<Span>) -> Self {
        targets.sort_by_key(|span| (span.file.0, span.start, span.end));
        targets.dedup();
        if targets.is_empty() {
            Self::unknown()
        } else {
            Self {
                targets,
                completeness: AnalysisCallTargetCompleteness::Partial,
            }
        }
    }
}

#[derive(Default)]
struct IndirectCallTargetFacts<'a> {
    direct_targets_by_span: std::collections::BTreeMap<Span, Span>,
    definition_sources_by_node_span:
        std::collections::BTreeMap<Span, CallableValueDefinitionSource>,
    closure_body_owners_by_closure_span: std::collections::BTreeMap<Span, ClosureBodyCallOwner>,
    closure_body_callers_by_call_span: std::collections::BTreeMap<Span, Option<Span>>,
    local_binding_by_reference_span: std::collections::BTreeMap<Span, AnalysisFlowBindingId>,
    flow_node_by_reference_binding_span:
        std::collections::BTreeMap<(Span, AnalysisFlowBindingId), AnalysisFlowNodeId>,
    flow_facts: Option<FlowFunctionValueFacts<'a>>,
}

#[derive(Clone, Copy)]
enum ClosureBodyCallOwner {
    Named(Span),
    Unnamed,
}

impl ClosureBodyCallOwner {
    fn definition_span(self) -> Option<Span> {
        match self {
            Self::Named(span) => Some(span),
            Self::Unnamed => None,
        }
    }
}

impl<'a> IndirectCallTargetFacts<'a> {
    fn collect(
        expr: &ast::Expr,
        callable_entries: &std::collections::BTreeMap<Span, Span>,
        flow_model: &'a FlowModel,
    ) -> Self {
        let owner_def_id = flow_model.owner_def_id(expr.span);
        let mut facts = Self::default();
        collect_indirect_call_target_facts(expr, callable_entries, &mut facts);
        if let Some(owner_def_id) = owner_def_id
            && let Some(flow_facts) = flow_model.function_value_facts(owner_def_id)
        {
            facts.local_binding_by_reference_span =
                local_binding_references_by_span(&flow_facts.owner.bindings);
            facts.flow_node_by_reference_binding_span = flow_reference_nodes_by_span(&flow_facts);
            facts.flow_facts = Some(flow_facts);
        }
        facts
    }

    fn target_for_callee(
        &self,
        callee: &ast::Expr,
        parameter_call_targets: &InterproceduralFunctionValueFacts,
    ) -> IndirectCallTargets {
        let sources = self.sources_for_expr(callee);
        let mut targets = Vec::new();
        let mut saw_partial_source =
            sources.completeness == AnalysisCallTargetCompleteness::Partial;
        let mut saw_unknown_source =
            sources.completeness == AnalysisCallTargetCompleteness::Unknown;
        for source in sources.sources {
            match source {
                FunctionValueSource::Target(target) => targets.push(target),
                FunctionValueSource::Parameter(parameter_span) => {
                    saw_partial_source = true;
                    let parameter_targets =
                        parameter_call_targets.targets_for_parameter(parameter_span);
                    targets.extend(parameter_targets.targets);
                    match parameter_targets.completeness {
                        AnalysisCallTargetCompleteness::Exact => {}
                        AnalysisCallTargetCompleteness::Partial => saw_partial_source = true,
                        AnalysisCallTargetCompleteness::Unknown => saw_unknown_source = true,
                    }
                }
            }
        }
        targets.sort_by_key(|span| (span.file.0, span.start, span.end));
        targets.dedup();
        let completeness = if saw_unknown_source && targets.is_empty() {
            AnalysisCallTargetCompleteness::Unknown
        } else if saw_unknown_source || saw_partial_source {
            AnalysisCallTargetCompleteness::Partial
        } else {
            AnalysisCallTargetCompleteness::Exact
        };
        match completeness {
            AnalysisCallTargetCompleteness::Exact => {
                if targets.len() == 1 {
                    IndirectCallTargets::exact(targets[0])
                } else {
                    IndirectCallTargets::partial(targets)
                }
            }
            AnalysisCallTargetCompleteness::Partial => IndirectCallTargets {
                targets,
                completeness: AnalysisCallTargetCompleteness::Partial,
            },
            AnalysisCallTargetCompleteness::Unknown => IndirectCallTargets::unknown(),
        }
    }

    fn closure_body_caller_for_call(&self, call_span: Span) -> Option<Option<Span>> {
        self.closure_body_callers_by_call_span
            .get(&call_span)
            .copied()
    }

    fn record_closure_body_owner_for_value(&mut self, expr: &ast::Expr, owner_span: Span) {
        match &expr.kind {
            ast::ExprKind::Closure { .. } => {
                self.closure_body_owners_by_closure_span
                    .insert(expr.span, ClosureBodyCallOwner::Named(owner_span));
            }
            ast::ExprKind::Grouped { expr } | ast::ExprKind::As { lhs: expr, .. } => {
                self.record_closure_body_owner_for_value(expr, owner_span);
            }
            _ => {}
        }
    }

    fn sources_for_expr(&self, expr: &ast::Expr) -> FunctionValueSources {
        match &expr.kind {
            ast::ExprKind::Grouped { expr }
            | ast::ExprKind::As { lhs: expr, .. }
            | ast::ExprKind::GenericInstantiation { target: expr, .. } => {
                return self.sources_for_expr(expr);
            }
            ast::ExprKind::Unary {
                op: ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf,
                operand,
            } => return self.sources_for_expr(operand),
            _ => {}
        }

        let reference_span = callee_reference_span(expr);
        if let Some(target) = self.direct_targets_by_span.get(&reference_span).copied() {
            return FunctionValueSources::exact(FunctionValueSource::Target(target));
        }
        self.flow_sources_for_reference(reference_span)
    }

    fn flow_sources_for_reference(&self, reference_span: Span) -> FunctionValueSources {
        let mut visited = std::collections::BTreeSet::new();
        self.flow_sources_for_reference_with_visited(reference_span, &mut visited)
    }

    fn flow_sources_for_reference_with_visited(
        &self,
        reference_span: Span,
        visited: &mut std::collections::BTreeSet<AnalysisFlowBindingId>,
    ) -> FunctionValueSources {
        let Some(flow_facts) = self.flow_facts.as_ref() else {
            return FunctionValueSources::unknown();
        };
        let binding_id = self
            .local_binding_by_reference_span
            .get(&reference_span)
            .copied();
        let Some(binding_id) = binding_id else {
            return FunctionValueSources::unknown();
        };
        let Some(binding) = flow_facts.binding(binding_id) else {
            return FunctionValueSources::unknown();
        };
        if binding.kind == kernc_flow::AnalysisFlowBindingKind::Parameter && !binding.is_mut {
            return FunctionValueSources::exact(FunctionValueSource::Parameter(
                binding.definition_span,
            ));
        }
        let node_id = self
            .flow_node_by_reference_binding_span
            .get(&(reference_span, binding_id))
            .copied();
        let Some(node_id) = node_id else {
            return FunctionValueSources::unknown();
        };
        if let Some(resolved) = flow_facts.resolved_use_for(node_id, binding_id) {
            let sources = resolved
                .candidate_definitions
                .iter()
                .map(|definition| {
                    let mut branch_visited = visited.clone();
                    self.resolve_function_value_definition_sources(*definition, &mut branch_visited)
                })
                .collect::<Vec<_>>();
            return match resolved.kind {
                kernc_flow::AnalysisFlowResolvedUseKind::Unique => {
                    FunctionValueSources::merged(sources, AnalysisCallTargetCompleteness::Exact)
                }
                kernc_flow::AnalysisFlowResolvedUseKind::Ambiguous => {
                    FunctionValueSources::merged(sources, AnalysisCallTargetCompleteness::Partial)
                }
                kernc_flow::AnalysisFlowResolvedUseKind::Missing => FunctionValueSources::unknown(),
            };
        }
        FunctionValueSources::unknown()
    }

    fn resolve_function_value_definition_sources(
        &self,
        definition: AnalysisFlowDefinitionRef,
        visited: &mut std::collections::BTreeSet<AnalysisFlowBindingId>,
    ) -> FunctionValueSources {
        let Some(flow_facts) = self.flow_facts.as_ref() else {
            return FunctionValueSources::unknown();
        };
        let Some(definition_facts) = flow_facts.definition_facts(definition) else {
            return FunctionValueSources::unknown();
        };
        if !matches!(
            definition_facts.kind,
            AnalysisFlowDefinitionKind::Initializer | AnalysisFlowDefinitionKind::Assignment
        ) {
            return FunctionValueSources::unknown();
        }
        if let Some(source_binding_id) = definition_facts.copy_source_binding_id {
            return self.resolve_function_value_binding_sources(source_binding_id, visited);
        }
        self.direct_definition_sources(definition, visited)
    }

    fn resolve_function_value_binding_sources(
        &self,
        binding_id: AnalysisFlowBindingId,
        visited: &mut std::collections::BTreeSet<AnalysisFlowBindingId>,
    ) -> FunctionValueSources {
        let Some(flow_facts) = self.flow_facts.as_ref() else {
            return FunctionValueSources::unknown();
        };
        let mut current = binding_id;

        loop {
            if !visited.insert(current) {
                return FunctionValueSources::unknown();
            }

            let Some(binding) = flow_facts.binding(current) else {
                return FunctionValueSources::unknown();
            };
            match binding.kind {
                kernc_flow::AnalysisFlowBindingKind::Parameter if !binding.is_mut => {
                    return FunctionValueSources::exact(FunctionValueSource::Parameter(
                        binding.definition_span,
                    ));
                }
                kernc_flow::AnalysisFlowBindingKind::Variable => {}
                _ => return FunctionValueSources::unknown(),
            }

            let Some(summary) = flow_facts.binding_summary(current) else {
                return FunctionValueSources::unknown();
            };
            if summary.definition_node_ids.len() != 1 {
                let sources = summary
                    .definition_node_ids
                    .iter()
                    .map(|node_id| {
                        let definition = AnalysisFlowDefinitionRef {
                            binding_id: current,
                            node_id: *node_id,
                        };
                        let mut branch_visited = visited.clone();
                        self.resolve_function_value_definition_sources(
                            definition,
                            &mut branch_visited,
                        )
                    })
                    .collect::<Vec<_>>();
                return FunctionValueSources::merged(
                    sources,
                    AnalysisCallTargetCompleteness::Partial,
                );
            }
            let definition = AnalysisFlowDefinitionRef {
                binding_id: current,
                node_id: summary.definition_node_ids[0],
            };
            let Some(definition_facts) = flow_facts.definition_facts(definition) else {
                return FunctionValueSources::unknown();
            };
            if !matches!(
                definition_facts.kind,
                AnalysisFlowDefinitionKind::Initializer | AnalysisFlowDefinitionKind::Assignment
            ) {
                return FunctionValueSources::unknown();
            }

            if let Some(source_binding_id) = definition_facts.copy_source_binding_id {
                if source_binding_id == current {
                    return FunctionValueSources::unknown();
                }
                current = source_binding_id;
                continue;
            }

            let direct_sources = self.direct_definition_sources(definition, visited);
            if direct_sources.completeness != AnalysisCallTargetCompleteness::Unknown {
                return direct_sources;
            }
            return FunctionValueSources::unknown();
        }
    }

    fn direct_definition_sources(
        &self,
        definition: AnalysisFlowDefinitionRef,
        visited: &mut std::collections::BTreeSet<AnalysisFlowBindingId>,
    ) -> FunctionValueSources {
        let Some(flow_facts) = self.flow_facts.as_ref() else {
            return FunctionValueSources::unknown();
        };
        if flow_facts.binding(definition.binding_id).is_none() {
            return FunctionValueSources::unknown();
        }
        let node_span = flow_facts
            .owner
            .cfg
            .nodes
            .get(definition.node_id.index())
            .map(|node| node.span);
        if let Some(source) =
            node_span.and_then(|span| self.definition_sources_by_node_span.get(&span).copied())
        {
            return match source {
                CallableValueDefinitionSource::Target(target) => {
                    FunctionValueSources::exact(FunctionValueSource::Target(target))
                }
                CallableValueDefinitionSource::Reference(reference_span) => {
                    self.flow_sources_for_reference_with_visited(reference_span, visited)
                }
            };
        }
        FunctionValueSources::unknown()
    }
}

fn collect_indirect_call_target_facts(
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    facts: &mut IndirectCallTargetFacts,
) {
    collect_indirect_call_target_facts_with_closure_owner(expr, callable_entries, facts, None);
}

fn collect_indirect_call_target_facts_with_closure_owner(
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    facts: &mut IndirectCallTargetFacts,
    closure_body_owner: Option<ClosureBodyCallOwner>,
) {
    if let Some(target_span) = callable_entries.get(&callee_reference_span(expr)).copied() {
        facts
            .direct_targets_by_span
            .insert(callee_reference_span(expr), target_span);
    }
    if let Some(source) = callable_value_definition_source(expr, callable_entries) {
        facts
            .definition_sources_by_node_span
            .insert(expr.span, source);
    }

    match &expr.kind {
        ast::ExprKind::Let {
            pattern,
            init,
            else_clause,
            ..
        } => {
            if let Some(source) = callable_value_definition_source(init, callable_entries) {
                facts
                    .definition_sources_by_node_span
                    .insert(expr.span, source);
            }
            if else_clause.is_none()
                && let ast::PatternKind::Binding(binding) = &pattern.pattern.kind
                && expr_is_closure_value(init)
            {
                facts.definition_sources_by_node_span.insert(
                    expr.span,
                    CallableValueDefinitionSource::Target(binding.name_span),
                );
                facts.record_closure_body_owner_for_value(init, binding.name_span);
            }
            collect_indirect_call_target_facts_with_closure_owner(
                init,
                callable_entries,
                facts,
                closure_body_owner,
            );
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_indirect_call_target_facts_with_closure_owner(
                            else_expr,
                            callable_entries,
                            facts,
                            closure_body_owner,
                        );
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_indirect_call_target_facts_with_closure_owner(
                                &arm.body,
                                callable_entries,
                                facts,
                                closure_body_owner,
                            );
                        }
                    }
                }
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_indirect_call_target_facts_with_closure_owner(
                            expr,
                            callable_entries,
                            facts,
                            closure_body_owner,
                        );
                    }
                }
            }
            if let Some(result) = result {
                collect_indirect_call_target_facts_with_closure_owner(
                    result,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Static { init, .. } => {
            if let Some(init) = init {
                collect_indirect_call_target_facts_with_closure_owner(
                    init,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_indirect_call_target_facts_with_closure_owner(
                lhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
            collect_indirect_call_target_facts_with_closure_owner(
                rhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            if let Some(source) = callable_value_definition_source(rhs, callable_entries) {
                facts
                    .definition_sources_by_node_span
                    .insert(expr.span, source);
            }
            collect_indirect_call_target_facts_with_closure_owner(
                lhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
            collect_indirect_call_target_facts_with_closure_owner(
                rhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_indirect_call_target_facts_with_closure_owner(
                    start,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
            if let Some(end) = end {
                collect_indirect_call_target_facts_with_closure_owner(
                    end,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Unary { operand, .. }
        | ast::ExprKind::Grouped { expr: operand }
        | ast::ExprKind::FieldAccess { lhs: operand, .. }
        | ast::ExprKind::As { lhs: operand, .. }
        | ast::ExprKind::Propagate { operand }
        | ast::ExprKind::Defer { expr: operand }
        | ast::ExprKind::GenericInstantiation {
            target: operand, ..
        } => collect_indirect_call_target_facts_with_closure_owner(
            operand,
            callable_entries,
            facts,
            closure_body_owner,
        ),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_indirect_call_target_facts_with_closure_owner(
                lhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
            collect_indirect_call_target_facts_with_closure_owner(
                index,
                callable_entries,
                facts,
                closure_body_owner,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            if let Some(owner) = closure_body_owner {
                facts
                    .closure_body_callers_by_call_span
                    .insert(expr.span, owner.definition_span());
            }
            collect_indirect_call_target_facts_with_closure_owner(
                callee,
                callable_entries,
                facts,
                closure_body_owner,
            );
            for arg in args {
                collect_indirect_call_target_facts_with_closure_owner(
                    arg,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_indirect_call_target_facts_with_closure_owner(
                        &field.value,
                        callable_entries,
                        facts,
                        closure_body_owner,
                    );
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_indirect_call_target_facts_with_closure_owner(
                        item,
                        callable_entries,
                        facts,
                        closure_body_owner,
                    );
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_indirect_call_target_facts_with_closure_owner(
                    value,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
                collect_indirect_call_target_facts_with_closure_owner(
                    count,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_indirect_call_target_facts_with_closure_owner(
                    value,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_indirect_call_target_facts_with_closure_owner(
                cond,
                callable_entries,
                facts,
                closure_body_owner,
            );
            collect_indirect_call_target_facts_with_closure_owner(
                then_branch,
                callable_entries,
                facts,
                closure_body_owner,
            );
            if let Some(else_branch) = else_branch {
                collect_indirect_call_target_facts_with_closure_owner(
                    else_branch,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_indirect_call_target_facts_with_closure_owner(
                target,
                callable_entries,
                facts,
                closure_body_owner,
            );
            for arm in arms {
                collect_indirect_call_target_facts_with_closure_owner(
                    &arm.body,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::While { cond, body } => {
            collect_indirect_call_target_facts_with_closure_owner(
                cond,
                callable_entries,
                facts,
                closure_body_owner,
            );
            collect_indirect_call_target_facts_with_closure_owner(
                body,
                callable_entries,
                facts,
                closure_body_owner,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_indirect_call_target_facts_with_closure_owner(
                lhs,
                callable_entries,
                facts,
                closure_body_owner,
            );
            if let Some(start) = start {
                collect_indirect_call_target_facts_with_closure_owner(
                    start,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
            if let Some(end) = end {
                collect_indirect_call_target_facts_with_closure_owner(
                    end,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_indirect_call_target_facts_with_closure_owner(
                    value,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_indirect_call_target_facts_with_closure_owner(
                    &capture.value,
                    callable_entries,
                    facts,
                    closure_body_owner,
                );
            }
            let body_owner = facts
                .closure_body_owners_by_closure_span
                .get(&expr.span)
                .copied()
                .unwrap_or(ClosureBodyCallOwner::Unnamed);
            collect_indirect_call_target_facts_with_closure_owner(
                body,
                callable_entries,
                facts,
                Some(body_owner),
            );
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
    }
}

fn callable_value_target_for_expr(
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
) -> Option<Span> {
    if let Some(target_span) = callable_entries.get(&callee_reference_span(expr)).copied() {
        return Some(target_span);
    }

    match &expr.kind {
        ast::ExprKind::Grouped { expr }
        | ast::ExprKind::As { lhs: expr, .. }
        | ast::ExprKind::GenericInstantiation { target: expr, .. } => {
            callable_value_target_for_expr(expr, callable_entries)
        }
        ast::ExprKind::Unary {
            op: ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf,
            operand,
        } => callable_value_target_for_expr(operand, callable_entries),
        _ => None,
    }
}

fn callable_value_definition_source(
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
) -> Option<CallableValueDefinitionSource> {
    if let Some(target) = callable_value_target_for_expr(expr, callable_entries) {
        return Some(CallableValueDefinitionSource::Target(target));
    }
    callable_value_source_reference_span(expr).map(CallableValueDefinitionSource::Reference)
}

fn callable_value_source_reference_span(expr: &ast::Expr) -> Option<Span> {
    match &expr.kind {
        ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::FieldAccess { .. } => Some(callee_reference_span(expr)),
        ast::ExprKind::Grouped { expr }
        | ast::ExprKind::As { lhs: expr, .. }
        | ast::ExprKind::GenericInstantiation { target: expr, .. } => {
            callable_value_source_reference_span(expr)
        }
        ast::ExprKind::Unary {
            op: ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf,
            operand,
        } => callable_value_source_reference_span(operand),
        _ => None,
    }
}

fn expr_is_closure_value(expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::Closure { .. } => true,
        ast::ExprKind::Grouped { expr } | ast::ExprKind::As { lhs: expr, .. } => {
            expr_is_closure_value(expr)
        }
        _ => false,
    }
}

fn collect_interprocedural_function_value_edges_in_decl(
    ctx: &mut SemaContext<'_>,
    decl: &ast::Decl,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
    function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
    flow_model: &FlowModel,
    facts_by_parameter_span: &mut std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
    parameter_edges: &mut Vec<(Span, Span)>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            let facts = IndirectCallTargetFacts::collect(body, callable_entries, flow_model);
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                body,
                callable_entries,
                &facts,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::DeclKind::Var {
            value: Some(value), ..
        } => {
            let facts = IndirectCallTargetFacts::collect(value, callable_entries, flow_model);
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                value,
                callable_entries,
                &facts,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. }
        | ast::DeclKind::Mod {
            decls: Some(decls), ..
        }
        | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_interprocedural_function_value_edges_in_decl(
                    ctx,
                    child,
                    callable_entries,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    flow_model,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        _ => {}
    }
}

fn collect_interprocedural_function_value_edges_in_expr(
    ctx: &mut SemaContext<'_>,
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
    function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
    facts_by_parameter_span: &mut std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
    parameter_edges: &mut Vec<(Span, Span)>,
) {
    match &expr.kind {
        ast::ExprKind::Let {
            init, else_clause, ..
        } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                init,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_interprocedural_function_value_edges_in_expr(
                            ctx,
                            else_expr,
                            callable_entries,
                            indirect_call_targets,
                            function_ids_by_definition_span,
                            function_param_spans_by_def_id,
                            facts_by_parameter_span,
                            parameter_edges,
                        );
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_interprocedural_function_value_edges_in_expr(
                                ctx,
                                &arm.body,
                                callable_entries,
                                indirect_call_targets,
                                function_ids_by_definition_span,
                                function_param_spans_by_def_id,
                                facts_by_parameter_span,
                                parameter_edges,
                            );
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static { init, .. } => {
            if let Some(init) = init {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    init,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                rhs,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    start,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
            if let Some(end) = end {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    end,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Unary { operand, .. }
        | ast::ExprKind::Grouped { expr: operand }
        | ast::ExprKind::FieldAccess { lhs: operand, .. }
        | ast::ExprKind::As { lhs: operand, .. }
        | ast::ExprKind::Propagate { operand }
        | ast::ExprKind::Defer { expr: operand }
        | ast::ExprKind::GenericInstantiation {
            target: operand, ..
        } => collect_interprocedural_function_value_edges_in_expr(
            ctx,
            operand,
            callable_entries,
            indirect_call_targets,
            function_ids_by_definition_span,
            function_param_spans_by_def_id,
            facts_by_parameter_span,
            parameter_edges,
        ),
        ast::ExprKind::IndexAccess { lhs, index, .. }
        | ast::ExprKind::SliceOp {
            lhs,
            start: Some(index),
            end: None,
            ..
        } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                index,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            if let Some(start) = start {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    start,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
            if let Some(end) = end {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    end,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Call { callee, args } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                callee,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            for arg in args {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    arg,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
            record_direct_parameter_function_value_edges(
                ctx,
                callee,
                args,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_interprocedural_function_value_edges_in_expr(
                        ctx,
                        &field.value,
                        callable_entries,
                        indirect_call_targets,
                        function_ids_by_definition_span,
                        function_param_spans_by_def_id,
                        facts_by_parameter_span,
                        parameter_edges,
                    );
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_interprocedural_function_value_edges_in_expr(
                        ctx,
                        item,
                        callable_entries,
                        indirect_call_targets,
                        function_ids_by_definition_span,
                        function_param_spans_by_def_id,
                        facts_by_parameter_span,
                        parameter_edges,
                    );
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    value,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    count,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    value,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                cond,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                then_branch,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            if let Some(else_branch) = else_branch {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    else_branch,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                target,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_interprocedural_function_value_edges_in_match_pattern(
                        ctx,
                        pattern,
                        callable_entries,
                        indirect_call_targets,
                        function_ids_by_definition_span,
                        function_param_spans_by_def_id,
                        facts_by_parameter_span,
                        parameter_edges,
                    );
                }
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    &arm.body,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_interprocedural_function_value_edges_in_expr(
                            ctx,
                            expr,
                            callable_entries,
                            indirect_call_targets,
                            function_ids_by_definition_span,
                            function_param_spans_by_def_id,
                            facts_by_parameter_span,
                            parameter_edges,
                        );
                    }
                }
            }
            if let Some(result) = result {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    result,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::While { cond, body } => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                cond,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                body,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    value,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_interprocedural_function_value_edges_in_expr(
                    ctx,
                    &capture.value,
                    callable_entries,
                    indirect_call_targets,
                    function_ids_by_definition_span,
                    function_param_spans_by_def_id,
                    facts_by_parameter_span,
                    parameter_edges,
                );
            }
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                body,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
    }
}

fn collect_interprocedural_function_value_edges_in_match_pattern(
    ctx: &mut SemaContext<'_>,
    pattern: &ast::MatchPattern,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
    function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
    facts_by_parameter_span: &mut std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
    parameter_edges: &mut Vec<(Span, Span)>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(expr) => {
            collect_interprocedural_function_value_edges_in_expr(
                ctx,
                expr,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
        ast::MatchPatternKind::Pattern(pattern) => {
            collect_interprocedural_function_value_edges_in_pattern(
                ctx,
                pattern,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
    }
}

fn collect_interprocedural_function_value_edges_in_pattern(
    ctx: &mut SemaContext<'_>,
    pattern: &ast::Pattern,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
    function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
    facts_by_parameter_span: &mut std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
    parameter_edges: &mut Vec<(Span, Span)>,
) {
    if let ast::PatternKind::Destructure(destructure) = &pattern.kind {
        for field in &destructure.fields {
            collect_interprocedural_function_value_edges_in_pattern(
                ctx,
                &field.pattern,
                callable_entries,
                indirect_call_targets,
                function_ids_by_definition_span,
                function_param_spans_by_def_id,
                facts_by_parameter_span,
                parameter_edges,
            );
        }
    }
}

fn record_direct_parameter_function_value_edges(
    ctx: &mut SemaContext<'_>,
    callee: &ast::Expr,
    args: &[ast::Expr],
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    function_ids_by_definition_span: &std::collections::BTreeMap<Span, DefId>,
    function_param_spans_by_def_id: &std::collections::HashMap<DefId, Vec<Span>>,
    facts_by_parameter_span: &mut std::collections::BTreeMap<Span, ParameterFunctionValueFact>,
    parameter_edges: &mut Vec<(Span, Span)>,
) {
    if analysis_call_kind(ctx, callee) != Some(AnalysisCallKind::Direct) {
        return;
    }
    let Some(callee_definition_span) = callable_entries
        .get(&callee_reference_span(callee))
        .copied()
    else {
        return;
    };
    let Some(callee_def_id) = function_ids_by_definition_span
        .get(&callee_definition_span)
        .copied()
    else {
        return;
    };
    let Some(parameter_spans) = function_param_spans_by_def_id.get(&callee_def_id) else {
        return;
    };

    for (arg, parameter_span) in args.iter().zip(parameter_spans.iter().copied()) {
        let sources = indirect_call_targets.sources_for_expr(arg);
        facts_by_parameter_span
            .entry(parameter_span)
            .or_default()
            .add_sources(&sources);
        for source in sources.sources {
            match source {
                FunctionValueSource::Target(_) => {}
                FunctionValueSource::Parameter(source_parameter_span) => {
                    parameter_edges.push((source_parameter_span, parameter_span));
                }
            }
        }
    }
}

fn local_binding_references_by_span(
    bindings: &[kernc_flow::FlowBindingFacts],
) -> std::collections::BTreeMap<Span, AnalysisFlowBindingId> {
    bindings
        .iter()
        .flat_map(|binding| {
            binding
                .reference_spans
                .iter()
                .copied()
                .map(move |reference_span| (reference_span, binding.id))
        })
        .collect()
}

fn flow_reference_nodes_by_span(
    flow_facts: &FlowFunctionValueFacts<'_>,
) -> std::collections::BTreeMap<(Span, AnalysisFlowBindingId), AnalysisFlowNodeId> {
    let mut candidates =
        std::collections::BTreeMap::<(Span, AnalysisFlowBindingId), Vec<AnalysisFlowNodeId>>::new();
    for binding in &flow_facts.owner.bindings {
        for reference_span in &binding.reference_spans {
            for node_id in reference_span_node_ids(flow_facts, binding.id, *reference_span) {
                candidates
                    .entry((*reference_span, binding.id))
                    .or_default()
                    .push(node_id);
            }
        }
    }

    candidates
        .into_iter()
        .filter_map(|(key, mut node_ids)| {
            node_ids.sort();
            node_ids.dedup();
            (node_ids.len() == 1).then_some((key, node_ids[0]))
        })
        .collect()
}

fn reference_span_node_ids(
    flow_facts: &FlowFunctionValueFacts<'_>,
    binding_id: AnalysisFlowBindingId,
    reference_span: Span,
) -> Vec<AnalysisFlowNodeId> {
    flow_facts
        .owner
        .node_facts
        .iter()
        .filter(|facts| {
            facts.use_binding_ids.contains(&binding_id)
                && flow_facts
                    .owner
                    .cfg
                    .nodes
                    .get(facts.node_id.index())
                    .is_some_and(|node| node.span == reference_span)
        })
        .map(|facts| facts.node_id)
        .collect()
}

fn collect_calls_in_expr(
    ctx: &mut SemaContext<'_>,
    expr: &ast::Expr,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    flow_model: &FlowModel,
    function_definition_spans: &std::collections::HashMap<DefId, Span>,
    parameter_call_targets: &InterproceduralFunctionValueFacts,
    calls: &mut Vec<AnalysisCall>,
) {
    match &expr.kind {
        ast::ExprKind::Let {
            init, else_clause, ..
        } => {
            collect_calls_in_expr(
                ctx,
                init,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => collect_calls_in_expr(
                        ctx,
                        else_expr,
                        callable_entries,
                        indirect_call_targets,
                        flow_model,
                        function_definition_spans,
                        parameter_call_targets,
                        calls,
                    ),
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_calls_in_expr(
                                ctx,
                                &arm.body,
                                callable_entries,
                                indirect_call_targets,
                                flow_model,
                                function_definition_spans,
                                parameter_call_targets,
                                calls,
                            );
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static { init, .. } => {
            if let Some(init) = init {
                collect_calls_in_expr(
                    ctx,
                    init,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_calls_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            collect_calls_in_expr(
                ctx,
                rhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_calls_in_expr(
                    ctx,
                    start,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
            if let Some(end) = end {
                collect_calls_in_expr(
                    ctx,
                    end,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Unary { operand, .. }
        | ast::ExprKind::Grouped { expr: operand }
        | ast::ExprKind::FieldAccess { lhs: operand, .. }
        | ast::ExprKind::As { lhs: operand, .. }
        | ast::ExprKind::Propagate { operand }
        | ast::ExprKind::Defer { expr: operand }
        | ast::ExprKind::GenericInstantiation {
            target: operand, ..
        } => collect_calls_in_expr(
            ctx,
            operand,
            callable_entries,
            indirect_call_targets,
            flow_model,
            function_definition_spans,
            parameter_call_targets,
            calls,
        ),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_calls_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            collect_calls_in_expr(
                ctx,
                index,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            collect_calls_in_expr(
                ctx,
                callee,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            for arg in args {
                collect_calls_in_expr(
                    ctx,
                    arg,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
            let callee_definition_span = callable_entries
                .get(&callee_reference_span(callee))
                .copied();
            let call_kind = analysis_call_kind(ctx, callee);
            let caller_definition_span = indirect_call_targets
                .closure_body_caller_for_call(expr.span)
                .unwrap_or_else(|| {
                    flow_model
                        .owner_def_id(expr.span)
                        .and_then(|caller_def_id| function_definition_spans.get(&caller_def_id))
                        .copied()
                });
            if let Some(mut kind) = call_kind
                && let Some(caller_definition_span) = caller_definition_span
            {
                if kind == AnalysisCallKind::Direct && callee_definition_span.is_none() {
                    kind = AnalysisCallKind::Indirect;
                }
                let indirect_targets = if kind == AnalysisCallKind::Indirect {
                    indirect_call_targets.target_for_callee(callee, parameter_call_targets)
                } else {
                    IndirectCallTargets::unknown()
                };
                calls.push(AnalysisCall {
                    kind,
                    call_span: expr.span,
                    callee_span: callee.span,
                    callee_definition_span,
                    caller_definition_span,
                    dynamic_dispatch_targets: dynamic_dispatch_targets(ctx, callee),
                    indirect_targets: indirect_targets.targets,
                    indirect_target_completeness: indirect_targets.completeness,
                });
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_calls_in_expr(
                        ctx,
                        &field.value,
                        callable_entries,
                        indirect_call_targets,
                        flow_model,
                        function_definition_spans,
                        parameter_call_targets,
                        calls,
                    );
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_calls_in_expr(
                        ctx,
                        item,
                        callable_entries,
                        indirect_call_targets,
                        flow_model,
                        function_definition_spans,
                        parameter_call_targets,
                        calls,
                    );
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_calls_in_expr(
                    ctx,
                    value,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
                collect_calls_in_expr(
                    ctx,
                    count,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
            ast::DataLiteralKind::Scalar(value) => collect_calls_in_expr(
                ctx,
                value,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            ),
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_calls_in_expr(
                ctx,
                cond,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            collect_calls_in_expr(
                ctx,
                then_branch,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            if let Some(else_branch) = else_branch {
                collect_calls_in_expr(
                    ctx,
                    else_branch,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_calls_in_expr(
                ctx,
                target,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_calls_in_match_pattern(
                        ctx,
                        pattern,
                        callable_entries,
                        indirect_call_targets,
                        flow_model,
                        function_definition_spans,
                        parameter_call_targets,
                        calls,
                    );
                }
                collect_calls_in_expr(
                    ctx,
                    &arm.body,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_calls_in_expr(
                            ctx,
                            expr,
                            callable_entries,
                            indirect_call_targets,
                            flow_model,
                            function_definition_spans,
                            parameter_call_targets,
                            calls,
                        );
                    }
                }
            }
            if let Some(result) = result {
                collect_calls_in_expr(
                    ctx,
                    result,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::While { cond, body } => {
            collect_calls_in_expr(
                ctx,
                cond,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            collect_calls_in_expr(
                ctx,
                body,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_calls_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            if let Some(start) = start {
                collect_calls_in_expr(
                    ctx,
                    start,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
            if let Some(end) = end {
                collect_calls_in_expr(
                    ctx,
                    end,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_calls_in_expr(
                    ctx,
                    value,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_calls_in_expr(
                ctx,
                lhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
            collect_calls_in_expr(
                ctx,
                rhs,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_calls_in_expr(
                    ctx,
                    &capture.value,
                    callable_entries,
                    indirect_call_targets,
                    flow_model,
                    function_definition_spans,
                    parameter_call_targets,
                    calls,
                );
            }
            collect_calls_in_expr(
                ctx,
                body,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
    }
}

fn collect_calls_in_match_pattern(
    ctx: &mut SemaContext<'_>,
    pattern: &ast::MatchPattern,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    flow_model: &FlowModel,
    function_definition_spans: &std::collections::HashMap<DefId, Span>,
    parameter_call_targets: &InterproceduralFunctionValueFacts,
    calls: &mut Vec<AnalysisCall>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(expr) => collect_calls_in_expr(
            ctx,
            expr,
            callable_entries,
            indirect_call_targets,
            flow_model,
            function_definition_spans,
            parameter_call_targets,
            calls,
        ),
        ast::MatchPatternKind::Pattern(pattern) => collect_calls_in_pattern(
            ctx,
            pattern,
            callable_entries,
            indirect_call_targets,
            flow_model,
            function_definition_spans,
            parameter_call_targets,
            calls,
        ),
    }
}

fn collect_calls_in_pattern(
    ctx: &mut SemaContext<'_>,
    pattern: &ast::Pattern,
    callable_entries: &std::collections::BTreeMap<Span, Span>,
    indirect_call_targets: &IndirectCallTargetFacts,
    flow_model: &FlowModel,
    function_definition_spans: &std::collections::HashMap<DefId, Span>,
    parameter_call_targets: &InterproceduralFunctionValueFacts,
    calls: &mut Vec<AnalysisCall>,
) {
    if let ast::PatternKind::Destructure(destructure) = &pattern.kind {
        for field in &destructure.fields {
            collect_calls_in_pattern(
                ctx,
                &field.pattern,
                callable_entries,
                indirect_call_targets,
                flow_model,
                function_definition_spans,
                parameter_call_targets,
                calls,
            );
        }
    }
}

fn callee_reference_span(expr: &ast::Expr) -> Span {
    match &expr.kind {
        ast::ExprKind::FieldAccess { field_span, .. } => *field_span,
        ast::ExprKind::AnchoredPath { name_span, .. } => *name_span,
        ast::ExprKind::GenericInstantiation { target, .. } => callee_reference_span(target),
        ast::ExprKind::Grouped { expr } => callee_reference_span(expr),
        ast::ExprKind::As { lhs, .. } => callee_reference_span(lhs),
        ast::ExprKind::Unary {
            op: ast::UnaryOperator::AddressOf | ast::UnaryOperator::MutAddressOf,
            operand,
        } => callee_reference_span(operand),
        _ => expr.span,
    }
}
