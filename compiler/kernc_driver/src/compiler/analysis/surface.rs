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

    pub(super) fn collect_parsed_module_symbols(
        &self,
        session: &Session,
        modules: &[ParsedModule],
    ) -> Vec<AnalysisSymbol> {
        modules
            .iter()
            .map(|module| {
                let children = module
                    .ast
                    .decls
                    .iter()
                    .filter_map(|decl| self.analysis_symbol_from_decl_name_only(session, decl))
                    .collect::<Vec<_>>();

                let module_span = children
                    .iter()
                    .map(|symbol| symbol.span)
                    .reduce(|acc, span| acc.to(span))
                    .unwrap_or_default();

                AnalysisSymbol {
                    name: module.name.clone(),
                    kind: AnalysisSymbolKind::Module,
                    span: module_span,
                    selection_span: module_span,
                    detail: Some(module.ast.path.clone()),
                    children,
                }
            })
            .collect()
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
            let Some(&trait_ty) = ctx.facts.node_types.get(&trait_type.id) else {
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
                    .find(|method| method.name == function.name)
                    .map(|method| method.name_span)
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
                                is_pub: field.is_pub,
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
                                is_pub: field.is_pub,
                            });
                    }
                }
                kernc_sema::def::Def::Trait(trait_def) => {
                    for method in &trait_def.methods {
                        definitions
                            .entry(method.name_span)
                            .or_insert(AnalysisSemanticEntry {
                                span: method.name_span,
                                definition_span: method.name_span,
                                kind: AnalysisSemanticKind::Method,
                                role: AnalysisSemanticRole::Definition,
                                is_mut: false,
                                is_pub: true,
                            });
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
                self.render_function_hover(ctx, function, name)?
            }
            kernc_sema::scope::SymbolKind::Const => {
                format!("const {}: {}", name, ctx.ty_to_string(info.type_id))
            }
            kernc_sema::scope::SymbolKind::ConstParam => {
                format!("const {}: {}", name, ctx.ty_to_string(info.type_id))
            }
            kernc_sema::scope::SymbolKind::Static => {
                let mut prefix = String::from("static");
                if info.is_mut {
                    prefix.push_str(" mut");
                }
                format!("{} {}: {}", prefix, name, ctx.ty_to_string(info.type_id))
            }
            kernc_sema::scope::SymbolKind::Var => {
                let mut prefix = String::from("var");
                if info.is_mut {
                    prefix.push_str(" mut");
                }
                format!("{} {}: {}", prefix, name, ctx.ty_to_string(info.type_id))
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
                    ctx.facts
                        .node_types
                        .get(&alias.target.id)
                        .copied()
                        .map(|target_ty| ctx.ty_to_string(target_ty))
                } else if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::AssociatedType(assoc) =
                        &ctx.defs[def_id.0 as usize]
                    && let Some(target) = assoc.target.as_ref()
                {
                    ctx.facts
                        .node_types
                        .get(&target.id)
                        .copied()
                        .map(|target_ty| ctx.ty_to_string(target_ty))
                } else {
                    Some(ctx.ty_to_string(info.type_id))
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
        let code = self.render_function_hover(ctx, function, name)?;
        Some(render_hover_markdown(&code, function.docs.as_ref()))
    }

    fn render_function_hover(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
        name: &str,
    ) -> Option<String> {
        let sig = function.resolved_sig?;
        Some(format!("fn {}: {}", name, ctx.ty_to_string(sig)))
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
                        if !self.is_hoverable_span(ctx, method.name_span) {
                            continue;
                        }
                        let Some(contents) = self.hover_contents_for_trait_method(
                            ctx,
                            method.name,
                            method.type_node.id,
                        ) else {
                            continue;
                        };
                        by_span.entry(method.name_span).or_insert(contents);
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

    fn hover_contents_for_field(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<String> {
        let ty = ctx.facts.node_types.get(&type_node_id).copied()?;
        Some(render_hover_markdown(
            &format!("field {}: {}", ctx.resolve(name), ctx.ty_to_string(ty)),
            self.field_doc_block(ctx, name, type_node_id),
        ))
    }

    fn hover_contents_for_trait_method(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<String> {
        let ty = ctx.facts.node_types.get(&type_node_id).copied()?;
        Some(render_hover_markdown(
            &format!("fn {}: {}", ctx.resolve(name), ctx.ty_to_string(ty)),
            self.trait_method_doc_block(ctx, name, type_node_id),
        ))
    }

    fn hover_contents_for_enum_variant(
        &self,
        ctx: &SemaContext<'_>,
        variant: &ast::EnumVariant,
    ) -> Option<String> {
        let name = ctx.resolve(variant.name);
        if let Some(payload_type) = &variant.payload_type {
            let ty = ctx.facts.node_types.get(&payload_type.id).copied()?;
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
                if method.name == name && method.type_node.id == type_node_id {
                    return method.docs.as_ref();
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
            ast::DeclKind::TypeAlias { target, .. } => Some(AnalysisSymbol {
                name,
                kind: match &target.kind {
                    ast::TypeKind::Struct { .. } => AnalysisSymbolKind::Struct,
                    ast::TypeKind::Union { .. } => AnalysisSymbolKind::Union,
                    ast::TypeKind::Enum { .. } => AnalysisSymbolKind::Enum,
                    ast::TypeKind::Trait { .. } => AnalysisSymbolKind::Trait,
                    _ => AnalysisSymbolKind::TypeAlias,
                },
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::ModDecl => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::ExternBlock { decls, .. } => Some(AnalysisSymbol {
                name: "extern".to_string(),
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.span,
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
                selection_span: decl.span,
                detail: Some("impl".to_string()),
                children: decls
                    .iter()
                    .filter_map(|child| self.analysis_symbol_from_impl_decl(ctx, child))
                    .collect(),
            }),
            ast::DeclKind::Use { .. } => None,
        }
    }

    fn analysis_symbol_from_decl_name_only(
        &self,
        session: &Session,
        decl: &ast::Decl,
    ) -> Option<AnalysisSymbol> {
        let name = session.resolve(decl.name).to_string();
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
            ast::DeclKind::TypeAlias { target, .. } => Some(AnalysisSymbol {
                name,
                kind: match &target.kind {
                    ast::TypeKind::Struct { .. } => AnalysisSymbolKind::Struct,
                    ast::TypeKind::Union { .. } => AnalysisSymbolKind::Union,
                    ast::TypeKind::Enum { .. } => AnalysisSymbolKind::Enum,
                    ast::TypeKind::Trait { .. } => AnalysisSymbolKind::Trait,
                    _ => AnalysisSymbolKind::TypeAlias,
                },
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::ModDecl => Some(AnalysisSymbol {
                name,
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.name_span,
                detail: None,
                children: Vec::new(),
            }),
            ast::DeclKind::ExternBlock { decls, .. } => Some(AnalysisSymbol {
                name: "extern".to_string(),
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.span,
                detail: None,
                children: decls
                    .iter()
                    .filter_map(|child| self.analysis_symbol_from_decl_name_only(session, child))
                    .collect(),
            }),
            ast::DeclKind::Impl { decls, .. } => Some(AnalysisSymbol {
                name: "impl".to_string(),
                kind: AnalysisSymbolKind::Namespace,
                span: decl.span,
                selection_span: decl.span,
                detail: Some("impl".to_string()),
                children: decls
                    .iter()
                    .filter_map(|child| {
                        let mut symbol =
                            self.analysis_symbol_from_decl_name_only(session, child)?;
                        if matches!(symbol.kind, AnalysisSymbolKind::Function) {
                            symbol.kind = AnalysisSymbolKind::Method;
                        }
                        Some(symbol)
                    })
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
        if let Some(ty_id) = ctx.facts.node_types.get(&ty.id).copied() {
            return ctx.ty_to_string(ty_id);
        }

        ctx.sess
            .source_manager
            .slice_source(ty.span)
            .trim()
            .to_string()
    }
}
