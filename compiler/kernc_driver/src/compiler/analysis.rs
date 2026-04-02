use super::completion::CompletionModel;
use super::signature::SignatureModel;
use super::{
    AnalysisArtifact, AnalysisHover, AnalysisOutline, AnalysisReference, AnalysisReport,
    AnalysisSemanticEntry, AnalysisSemanticKind, AnalysisSemanticRole, AnalysisSpanReplacement,
    AnalysisSymbol, AnalysisSymbolKind, CompilerDriver, ParsedModule, ParsedModuleArtifact,
    SourceOverrides, StructureArtifact, TargetedAnalysisReport,
};
use crate::loader::ModuleLoader;
use kernc_ast as ast;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::DefId;
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_sema::scope::ScopeId;
use kernc_sema::{
    BuiltinInjector, SemaContext, SemaStructureSnapshot, SemanticDefinition, SemanticSymbolKind,
};
use kernc_utils::{NodeId, Session, Span};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct FunctionBodyReusePlan {
    worklist: Vec<(DefId, ScopeId)>,
    replaced_spans: Vec<AnalysisSpanReplacement>,
}

impl CompilerDriver {
    pub fn analyze_report(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisReport {
        let mut session = Session::new();
        let succeeded = {
            let ctx = self.analyze_with_overrides(&mut session, input_file, source_overrides);
            ctx.is_some()
        };

        AnalysisReport { session, succeeded }
    }

    pub fn analyze_artifact(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisArtifact {
        let mut session = Session::new();
        session.apply_options(&self.options);

        match self.try_analyze_structure(session, input_file, source_overrides) {
            Ok(structure) => self.analyze_artifact_from_structure(&structure),
            Err(session) => self.empty_analysis_artifact(session),
        }
    }

    pub fn analyze_outline(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> AnalysisOutline {
        match self.parse_modules(input_file, source_overrides) {
            Some(parsed) => self.analyze_outline_from_parsed(&parsed),
            None => {
                let mut session = Session::new();
                session.apply_options(&self.options);
                AnalysisOutline {
                    session,
                    symbols: Vec::new(),
                }
            }
        }
    }

    pub fn parse_modules(
        &self,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<ParsedModuleArtifact> {
        let mut session = Session::new();
        session.apply_options(&self.options);
        self.try_parse_modules(session, input_file, source_overrides)
            .ok()
    }

    pub fn analyze_artifact_from_structure(
        &self,
        structure: &StructureArtifact,
    ) -> AnalysisArtifact {
        let mut session = structure.session.clone();
        let analysis_asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        let succeeded = self.run_body_pipeline(&mut ctx);
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect::<Vec<_>>();
        let hovers = self.collect_analysis_hovers(&ctx);
        let semantic_entries = self.collect_analysis_semantic_entries(&symbols, &ctx, &references);
        let completion_model = self.collect_completion_model(&mut ctx, &analysis_asts);
        let signature_model = self.collect_signature_model(&mut ctx, &analysis_asts);
        let resolved_globals = self.collect_resolved_globals(&ctx);
        drop(ctx);

        AnalysisArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            semantic_entries,
            asts: analysis_asts,
            resolved_globals,
            completion_model,
            signature_model,
        }
    }

    pub fn analyze_report_from_structure_and_parsed(
        &self,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> Option<AnalysisReport> {
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        if !self.rebind_body_only_modules(&mut ctx, structure, parsed) {
            return None;
        }
        let succeeded = self.run_body_pipeline(&mut ctx);
        drop(ctx);

        Some(AnalysisReport { session, succeeded })
    }

    pub fn analyze_report_with_function_body_reuse(
        &self,
        clean_artifact: &AnalysisArtifact,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> Option<TargetedAnalysisReport> {
        let mut session = parsed.session.clone();
        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        self.apply_resolved_globals(&mut ctx, &clean_artifact.resolved_globals);

        let plan = self.build_function_body_reuse_plan(&ctx, &clean_artifact.asts, parsed)?;
        if plan.worklist.is_empty() {
            return None;
        }
        if !self.rebind_body_only_modules(&mut ctx, structure, parsed) {
            return None;
        }

        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_body_worklist(&plan.worklist);
        let ctx = typeck.into_context();
        let succeeded = Self::report_diagnostics_if_errors(ctx);

        Some(TargetedAnalysisReport {
            report: AnalysisReport { session, succeeded },
            replaced_spans: plan.replaced_spans,
        })
    }

    pub fn analyze_outline_from_structure(&self, structure: &StructureArtifact) -> AnalysisOutline {
        let mut session = structure.session.clone();
        let asts = structure.asts.clone();

        let mut ctx = self.build_sema_context(&mut session);
        ctx.restore_structure(structure.snapshot.clone());
        let symbols = self.collect_analysis_symbols(&ctx, &asts);
        drop(ctx);

        AnalysisOutline { session, symbols }
    }

    pub fn analyze_outline_from_parsed(&self, parsed: &ParsedModuleArtifact) -> AnalysisOutline {
        AnalysisOutline {
            session: parsed.session.clone(),
            symbols: self.collect_parsed_module_symbols(&parsed.session, &parsed.modules),
        }
    }

    pub(super) fn build_sema_context<'a>(&self, session: &'a mut Session) -> SemaContext<'a> {
        let mut ctx = SemaContext::new(session);
        ctx.module_aliases = self.options.module_aliases.clone();
        ctx.module_interface_aliases = self.options.module_interface_aliases.clone();

        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();
        ctx
    }

    pub(super) fn load_asts<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<Vec<(DefId, ast::Module)>> {
        let mut loader = ModuleLoader::new(ctx, source_overrides);
        let root_name = loader
            .ctx
            .intern(self.options.root_module_name.as_deref().unwrap_or("root"));
        if loader.load_root(input_file, root_name).is_none() {
            loader.ctx.sess.print_diagnostics();
            return None;
        }
        if !Self::report_diagnostics_if_errors(loader.ctx) {
            return None;
        }

        loader.ctx.inject_alias_roots();
        Some(std::mem::take(&mut loader.asts))
    }

    pub(super) fn try_analyze_structure(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<StructureArtifact, Session> {
        let mut ctx = self.build_sema_context(&mut session);
        let Some(asts) = self.load_asts(&mut ctx, input_file, source_overrides) else {
            return Err(session);
        };
        let Some(snapshot) = self.build_structure_snapshot(&mut ctx, asts.clone()) else {
            return Err(session);
        };
        let completion_model = self.collect_structure_completion_model(&ctx, &asts);
        drop(ctx);

        Ok(StructureArtifact {
            session,
            asts,
            snapshot,
            completion_model,
        })
    }

    pub(super) fn try_parse_modules(
        &self,
        mut session: Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Result<ParsedModuleArtifact, Session> {
        let mut ctx = self.build_sema_context(&mut session);
        let Some(asts) = self.load_asts(&mut ctx, input_file, source_overrides) else {
            return Err(session);
        };
        let modules = asts
            .into_iter()
            .map(|(mod_id, ast)| {
                let name = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => {
                        ctx.resolve(module_def.name).to_string()
                    }
                    _ => "<unknown>".to_string(),
                };
                let file_id = match &ctx.defs[mod_id.0 as usize] {
                    kernc_sema::def::Def::Module(module_def) => module_def.file_id,
                    _ => kernc_utils::FileId(0),
                };
                ParsedModule { name, file_id, ast }
            })
            .collect();
        drop(ctx);

        Ok(ParsedModuleArtifact { session, modules })
    }

    pub(super) fn build_structure_snapshot<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> Option<SemaStructureSnapshot> {
        if !self.run_structure_pipeline(ctx, asts) {
            return None;
        }
        Some(ctx.structure_snapshot())
    }

    pub(super) fn run_structure_pipeline<'a>(
        &self,
        ctx: &mut SemaContext<'a>,
        asts: Vec<(DefId, ast::Module)>,
    ) -> bool {
        let mut collector = Collector::new(ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }
        if !Self::report_diagnostics_if_errors(collector.context()) {
            return false;
        }

        let mut import_resolver = ImportResolver::new(collector.into_context());
        import_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(import_resolver.context()) {
            return false;
        }

        let mut type_resolver = TypeResolver::new(import_resolver.into_context());
        type_resolver.resolve_all();
        if !Self::report_diagnostics_if_errors(type_resolver.context()) {
            return false;
        }

        let _ctx = type_resolver.into_context();
        true
    }

    pub(super) fn run_body_pipeline<'a>(&self, ctx: &mut SemaContext<'a>) -> bool {
        let mut typeck = TypeckDriver::new(ctx);
        let globals = typeck.global_worklist();
        typeck.resolve_global_worklist(&globals);
        let worklist = typeck.body_worklist();
        typeck.check_body_worklist(&worklist);
        let ctx = typeck.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return false;
        }

        let mut linkage_checker = LinkageChecker::new(ctx);
        linkage_checker.check_all();
        Self::report_diagnostics_if_errors(linkage_checker.context())
    }

    fn empty_analysis_artifact(&self, session: Session) -> AnalysisArtifact {
        AnalysisArtifact {
            session,
            succeeded: false,
            symbols: Vec::new(),
            references: Vec::new(),
            hovers: Vec::new(),
            semantic_entries: Vec::new(),
            asts: Vec::new(),
            resolved_globals: Vec::new(),
            completion_model: CompletionModel::default(),
            signature_model: SignatureModel::default(),
        }
    }

    fn collect_analysis_symbols(
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

    fn collect_parsed_module_symbols(
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

    fn rebind_body_only_modules(
        &self,
        ctx: &mut SemaContext<'_>,
        structure: &StructureArtifact,
        parsed: &ParsedModuleArtifact,
    ) -> bool {
        let mut clean_modules = Vec::with_capacity(structure.asts.len());
        for (module_id, module_ast) in &structure.asts {
            let Some(path) = structure
                .session
                .source_manager
                .get_file_path(module_file_id(&ctx.defs, *module_id))
            else {
                return false;
            };
            clean_modules.push((normalize_driver_path(path), *module_id, module_ast));
        }

        if clean_modules.len() != parsed.modules.len() {
            return false;
        }

        for parsed_module in &parsed.modules {
            let Some(path) = parsed
                .session
                .source_manager
                .get_file_path(parsed_module.file_id)
            else {
                return false;
            };
            let normalized = normalize_driver_path(path);
            let Some((module_id, clean_module)) =
                clean_modules
                    .iter()
                    .find_map(|(path, module_id, module_ast)| {
                        (path == &normalized).then_some((*module_id, *module_ast))
                    })
            else {
                return false;
            };

            let clean_file_id = module_file_id(&ctx.defs, module_id);
            let module_changed = module_source_changed(
                &structure.session,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if module_changed && !modules_match_ignoring_body_only(clean_module, &parsed_module.ast)
            {
                return false;
            }

            if !rebind_module_defs(ctx, module_id, parsed_module) {
                return false;
            }
        }

        true
    }

    fn apply_resolved_globals(
        &self,
        ctx: &mut SemaContext<'_>,
        globals: &[super::ResolvedGlobalType],
    ) {
        for global in globals {
            let _ = ctx
                .scopes
                .update_type_in_scope(global.scope_id, global.name, global.ty);
        }
    }

    fn collect_resolved_globals(&self, ctx: &SemaContext<'_>) -> Vec<super::ResolvedGlobalType> {
        let mut globals = Vec::new();

        for def in &ctx.defs {
            let kernc_sema::def::Def::Module(module) = def else {
                continue;
            };

            for item_id in &module.items {
                let kernc_sema::def::Def::Global(global) = &ctx.defs[item_id.0 as usize] else {
                    continue;
                };
                let Some(info) = ctx.scopes.resolve_in(module.scope_id, global.name) else {
                    continue;
                };
                if info.type_id == kernc_sema::ty::TypeId::ERROR {
                    continue;
                }

                globals.push(super::ResolvedGlobalType {
                    scope_id: module.scope_id,
                    name: global.name,
                    ty: info.type_id,
                });
            }
        }

        globals
    }

    fn build_function_body_reuse_plan(
        &self,
        ctx: &SemaContext<'_>,
        clean_asts: &[(DefId, ast::Module)],
        parsed: &ParsedModuleArtifact,
    ) -> Option<FunctionBodyReusePlan> {
        let mut clean_modules = Vec::with_capacity(clean_asts.len());
        for (module_id, module_ast) in clean_asts {
            let path = ctx
                .sess
                .source_manager
                .get_file_path(module_file_id(&ctx.defs, *module_id))?;
            clean_modules.push((normalize_driver_path(path), *module_id, module_ast));
        }

        let mut worklist = Vec::new();
        let mut replaced_spans = Vec::new();

        for parsed_module in &parsed.modules {
            let path = parsed
                .session
                .source_manager
                .get_file_path(parsed_module.file_id)?;
            let normalized = normalize_driver_path(path);
            let Some((module_id, clean_module)) =
                clean_modules
                    .iter()
                    .find_map(|(path, module_id, module_ast)| {
                        (path == &normalized).then_some((*module_id, *module_ast))
                    })
            else {
                return None;
            };

            let clean_file_id = module_file_id(&ctx.defs, module_id);
            let module_changed = module_source_changed(
                ctx.sess,
                clean_file_id,
                &parsed.session,
                parsed_module.file_id,
            );
            if !module_changed {
                continue;
            }

            let module_scope = match &ctx.defs[module_id.0 as usize] {
                kernc_sema::def::Def::Module(module) => module.scope_id,
                _ => return None,
            };
            let module_items = match &ctx.defs[module_id.0 as usize] {
                kernc_sema::def::Def::Module(module) => module.items.clone(),
                _ => return None,
            };

            let mut item_iter = module_items.iter();
            if !classify_function_body_decl_changes(
                clean_module,
                &parsed_module.ast,
                &mut item_iter,
                module_scope,
                &mut worklist,
                &mut replaced_spans,
            ) {
                return None;
            }
            if item_iter.next().is_some() {
                return None;
            }
        }

        Some(FunctionBodyReusePlan {
            worklist,
            replaced_spans,
        })
    }

    fn collect_analysis_hovers(&self, ctx: &SemaContext<'_>) -> Vec<AnalysisHover> {
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

    fn collect_analysis_semantic_entries(
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

        for reference in references {
            let Some(definition) = definition_by_span.get(&reference.definition_span) else {
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
            kernc_sema::scope::SymbolKind::TypeAlias => {
                let detail = if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                {
                    ctx.node_types
                        .get(&alias.target.id)
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

        Some(format!("```kern\n{}\n```", code))
    }

    fn hover_contents_for_function(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
    ) -> Option<String> {
        let name = ctx.resolve(function.name);
        let code = self.render_function_hover(ctx, function, name)?;
        Some(format!("```kern\n{}\n```", code))
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
        let ty = ctx.node_types.get(&type_node_id).copied()?;
        Some(format!(
            "```kern\nfield {}: {}\n```",
            ctx.resolve(name),
            ctx.ty_to_string(ty)
        ))
    }

    fn hover_contents_for_trait_method(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        type_node_id: kernc_utils::NodeId,
    ) -> Option<String> {
        let ty = ctx.node_types.get(&type_node_id).copied()?;
        Some(format!(
            "```kern\nfn {}: {}\n```",
            ctx.resolve(name),
            ctx.ty_to_string(ty)
        ))
    }

    fn hover_contents_for_enum_variant(
        &self,
        ctx: &SemaContext<'_>,
        variant: &ast::EnumVariant,
    ) -> Option<String> {
        let name = ctx.resolve(variant.name);
        if let Some(payload_type) = &variant.payload_type {
            let ty = ctx.node_types.get(&payload_type.id).copied()?;
            Some(format!(
                "```kern\nvariant {}: {}\n```",
                name,
                ctx.ty_to_string(ty)
            ))
        } else {
            Some(format!("```kern\nvariant {}\n```", name))
        }
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
            ast::DeclKind::ModDecl { .. } => Some(AnalysisSymbol {
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
            ast::DeclKind::ModDecl { .. } => Some(AnalysisSymbol {
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
        match &ty.kind {
            ast::TypeKind::Path {
                segments, generics, ..
            } => {
                let mut rendered = segments
                    .iter()
                    .map(|segment| ctx.resolve(*segment).to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                if !generics.is_empty() {
                    let generic_text = generics
                        .iter()
                        .map(|generic| self.describe_type_node(ctx, generic))
                        .collect::<Vec<_>>()
                        .join(", ");
                    rendered.push('[');
                    rendered.push_str(&generic_text);
                    rendered.push(']');
                }
                rendered
            }
            ast::TypeKind::Pointer { is_mut, elem } => {
                if *is_mut {
                    format!("*mut {}", self.describe_type_node(ctx, elem))
                } else {
                    format!("*{}", self.describe_type_node(ctx, elem))
                }
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                if *is_mut {
                    format!("^mut {}", self.describe_type_node(ctx, elem))
                } else {
                    format!("^{}", self.describe_type_node(ctx, elem))
                }
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                if *is_mut {
                    format!("[]mut {}", self.describe_type_node(ctx, elem))
                } else {
                    format!("[]{}", self.describe_type_node(ctx, elem))
                }
            }
            ast::TypeKind::Array { is_mut, elem, .. } => {
                if *is_mut {
                    format!("[_]mut {}", self.describe_type_node(ctx, elem))
                } else {
                    format!("[_]{}", self.describe_type_node(ctx, elem))
                }
            }
            ast::TypeKind::ArrayInfer { is_mut, elem } => {
                if *is_mut {
                    format!("[_]mut {}", self.describe_type_node(ctx, elem))
                } else {
                    format!("[_]{}", self.describe_type_node(ctx, elem))
                }
            }
            ast::TypeKind::SelfType => "Self".to_string(),
            ast::TypeKind::Void => "void".to_string(),
            ast::TypeKind::Never => "!".to_string(),
            _ => "<type>".to_string(),
        }
    }
}

fn module_file_id(defs: &[kernc_sema::def::Def], module_id: DefId) -> kernc_utils::FileId {
    match &defs[module_id.0 as usize] {
        kernc_sema::def::Def::Module(module) => module.file_id,
        _ => kernc_utils::FileId(0),
    }
}

fn normalize_driver_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn module_source_changed(
    clean_session: &Session,
    clean_file_id: kernc_utils::FileId,
    parsed_session: &Session,
    parsed_file_id: kernc_utils::FileId,
) -> bool {
    let clean_source = clean_session
        .source_manager
        .get_file(clean_file_id)
        .map(|file| file.src.as_str());
    let parsed_source = parsed_session
        .source_manager
        .get_file(parsed_file_id)
        .map(|file| file.src.as_str());
    clean_source != parsed_source
}

fn classify_function_body_decl_changes<'a>(
    clean_module: &ast::Module,
    dirty_module: &ast::Module,
    item_ids: &mut std::slice::Iter<'a, DefId>,
    module_scope: ScopeId,
    worklist: &mut Vec<(DefId, ScopeId)>,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    if clean_module.decls.len() != dirty_module.decls.len() {
        return false;
    }

    for (clean_decl, dirty_decl) in clean_module.decls.iter().zip(&dirty_module.decls) {
        match (&clean_decl.kind, &dirty_decl.kind) {
            (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                if decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    continue;
                }
                if decls_equal_ignoring_body_only(clean_decl, dirty_decl) {
                    worklist.push((def_id, module_scope));
                    replaced_spans.push(AnalysisSpanReplacement {
                        clean: clean_decl.span,
                        dirty: dirty_decl.span,
                    });
                    continue;
                }
                return false;
            }
            (ast::DeclKind::Impl { .. }, ast::DeclKind::Impl { .. }) => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                if decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    continue;
                }
                if decls_equal_ignoring_body_only(clean_decl, dirty_decl) {
                    worklist.push((def_id, module_scope));
                    if !collect_impl_method_replacements(clean_decl, dirty_decl, replaced_spans) {
                        return false;
                    }
                    continue;
                }
                return false;
            }
            (
                ast::DeclKind::ExternBlock { decls: clean, .. },
                ast::DeclKind::ExternBlock { decls: dirty, .. },
            ) => {
                if !classify_function_body_decls(
                    clean,
                    dirty,
                    item_ids,
                    module_scope,
                    worklist,
                    replaced_spans,
                ) {
                    return false;
                }
            }
            _ => {
                if matches!(
                    clean_decl.kind,
                    ast::DeclKind::Var { .. } | ast::DeclKind::TypeAlias { .. }
                ) {
                    let Some(_def_id) = item_ids.next().copied() else {
                        return false;
                    };
                }
                if !decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    return false;
                }
            }
        }
    }

    true
}

fn classify_function_body_decls<'a>(
    clean_decls: &[ast::Decl],
    dirty_decls: &[ast::Decl],
    item_ids: &mut std::slice::Iter<'a, DefId>,
    module_scope: ScopeId,
    worklist: &mut Vec<(DefId, ScopeId)>,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    if clean_decls.len() != dirty_decls.len() {
        return false;
    }

    for (clean_decl, dirty_decl) in clean_decls.iter().zip(dirty_decls) {
        match (&clean_decl.kind, &dirty_decl.kind) {
            (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) => {
                let Some(_def_id) = item_ids.next().copied() else {
                    return false;
                };
                if !decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    return false;
                }
            }
            _ => return false,
        }
    }

    let _ = module_scope;
    let _ = worklist;
    let _ = replaced_spans;
    true
}

fn collect_impl_method_replacements(
    clean_decl: &ast::Decl,
    dirty_decl: &ast::Decl,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    let (
        ast::DeclKind::Impl {
            decls: clean_methods,
            ..
        },
        ast::DeclKind::Impl {
            decls: dirty_methods,
            ..
        },
    ) = (&clean_decl.kind, &dirty_decl.kind)
    else {
        return false;
    };

    if clean_methods.len() != dirty_methods.len() {
        return false;
    }

    for (clean_method, dirty_method) in clean_methods.iter().zip(dirty_methods) {
        let (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) =
            (&clean_method.kind, &dirty_method.kind)
        else {
            return false;
        };
        replaced_spans.push(AnalysisSpanReplacement {
            clean: clean_method.span,
            dirty: dirty_method.span,
        });
    }

    true
}

fn decls_equal_ignoring_ids_and_spans(left: &ast::Decl, right: &ast::Decl) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_reuse_comparison(&mut left);
    normalize_decl_for_reuse_comparison(&mut right);
    left == right
}

fn decls_equal_ignoring_body_only(left: &ast::Decl, right: &ast::Decl) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_body_only_comparison(&mut left);
    normalize_decl_for_body_only_comparison(&mut right);
    left == right
}

fn normalize_decl_for_reuse_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
    normalize_attributes_for_body_only_comparison(&mut decl.attributes);

    match &mut decl.kind {
        ast::DeclKind::Function {
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            if let Some(body) = body {
                normalize_expr_for_body_only_comparison(body);
            }
        }
        ast::DeclKind::Var { value, .. } => normalize_expr_for_body_only_comparison(value),
        ast::DeclKind::TypeAlias {
            generics,
            bounds,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            for bound in bounds {
                normalize_type_for_body_only_comparison(bound);
            }
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
        }
        ast::DeclKind::Use { target, .. } => normalize_use_target_for_body_only_comparison(target),
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                normalize_decl_for_reuse_comparison(child);
            }
        }
        ast::DeclKind::Impl {
            generics,
            where_clauses,
            target_type,
            trait_type,
            decls,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target_type);
            if let Some(trait_type) = trait_type {
                normalize_type_for_body_only_comparison(trait_type);
            }
            for child in decls {
                normalize_decl_for_reuse_comparison(child);
            }
        }
        ast::DeclKind::ModDecl { .. } => {}
    }
}

fn rebind_module_defs(
    ctx: &mut SemaContext<'_>,
    module_id: DefId,
    parsed_module: &ParsedModule,
) -> bool {
    let item_ids = match &mut ctx.defs[module_id.0 as usize] {
        kernc_sema::def::Def::Module(module) => {
            module.file_id = parsed_module.file_id;
            module.items.clone()
        }
        _ => return false,
    };

    let mut iter = item_ids.iter();
    if !rebind_decl_sequence(ctx, &mut iter, &parsed_module.ast.decls) {
        return false;
    }

    iter.next().is_none()
}

fn rebind_decl_sequence<'a>(
    ctx: &mut SemaContext<'_>,
    item_ids: &mut std::slice::Iter<'a, DefId>,
    decls: &[ast::Decl],
) -> bool {
    for decl in decls {
        match &decl.kind {
            ast::DeclKind::Function { body, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let kernc_sema::def::Def::Function(function) = &mut ctx.defs[def_id.0 as usize]
                else {
                    return false;
                };
                function.span = decl.span;
                function.name_span = decl.name_span;
                function.body = body.clone();
            }
            ast::DeclKind::Var { value, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let kernc_sema::def::Def::Global(global) = &mut ctx.defs[def_id.0 as usize] else {
                    return false;
                };
                global.span = decl.span;
                global.value = value.clone();
            }
            ast::DeclKind::TypeAlias { target, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                match (&mut ctx.defs[def_id.0 as usize], &target.kind) {
                    (
                        kernc_sema::def::Def::Struct(struct_def),
                        ast::TypeKind::Struct { fields, .. },
                    ) => {
                        struct_def.span = decl.span;
                        struct_def.fields = fields.clone();
                    }
                    (
                        kernc_sema::def::Def::Union(union_def),
                        ast::TypeKind::Union { fields, .. },
                    ) => {
                        union_def.span = decl.span;
                        union_def.fields = fields.clone();
                    }
                    (kernc_sema::def::Def::Enum(enum_def), _) => {
                        enum_def.span = decl.span;
                    }
                    (kernc_sema::def::Def::Trait(trait_def), _) => {
                        trait_def.span = decl.span;
                    }
                    (kernc_sema::def::Def::TypeAlias(alias_def), _) => {
                        alias_def.span = decl.span;
                    }
                    _ => return false,
                }
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                if !rebind_decl_sequence(ctx, item_ids, decls) {
                    return false;
                }
            }
            ast::DeclKind::Impl { decls, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let method_ids = match &mut ctx.defs[def_id.0 as usize] {
                    kernc_sema::def::Def::Impl(impl_def) => {
                        impl_def.span = decl.span;
                        impl_def.methods.clone()
                    }
                    _ => return false,
                };
                let mut method_iter = method_ids.iter();
                if !rebind_impl_methods(ctx, &mut method_iter, decls) {
                    return false;
                }
                if method_iter.next().is_some() {
                    return false;
                }
            }
            ast::DeclKind::Use { .. } | ast::DeclKind::ModDecl { .. } => {}
        }
    }

    true
}

fn rebind_impl_methods<'a>(
    ctx: &mut SemaContext<'_>,
    method_ids: &mut std::slice::Iter<'a, DefId>,
    decls: &[ast::Decl],
) -> bool {
    for decl in decls {
        let ast::DeclKind::Function { body, .. } = &decl.kind else {
            return false;
        };
        let Some(def_id) = method_ids.next().copied() else {
            return false;
        };
        let kernc_sema::def::Def::Function(function) = &mut ctx.defs[def_id.0 as usize] else {
            return false;
        };
        function.span = decl.span;
        function.name_span = decl.name_span;
        function.body = body.clone();
    }

    true
}

fn modules_match_ignoring_body_only(left: &ast::Module, right: &ast::Module) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_module_for_body_only_comparison(&mut left);
    normalize_module_for_body_only_comparison(&mut right);
    left == right
}

fn normalize_module_for_body_only_comparison(module: &mut ast::Module) {
    for decl in &mut module.decls {
        normalize_decl_for_body_only_comparison(decl);
    }
}

fn normalize_decl_for_body_only_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
    normalize_attributes_for_body_only_comparison(&mut decl.attributes);

    match &mut decl.kind {
        ast::DeclKind::Function {
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            *body = None;
        }
        ast::DeclKind::Var { value, .. } => {
            *value = placeholder_expr();
        }
        ast::DeclKind::TypeAlias {
            generics,
            bounds,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            for bound in bounds {
                normalize_type_for_body_only_comparison(bound);
            }
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
        }
        ast::DeclKind::Use { target, .. } => normalize_use_target_for_body_only_comparison(target),
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                normalize_decl_for_body_only_comparison(child);
            }
        }
        ast::DeclKind::Impl {
            generics,
            where_clauses,
            target_type,
            trait_type,
            decls,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target_type);
            if let Some(trait_type) = trait_type {
                normalize_type_for_body_only_comparison(trait_type);
            }
            for child in decls {
                normalize_decl_for_body_only_comparison(child);
            }
        }
        ast::DeclKind::ModDecl { .. } => {}
    }
}

fn normalize_attributes_for_body_only_comparison(attributes: &mut [ast::Attribute]) {
    for attribute in attributes {
        attribute.span = Span::default();
        match &mut attribute.kind {
            ast::AttributeKind::If(expr) => normalize_expr_for_body_only_comparison(expr),
            ast::AttributeKind::Meta(items) => {
                for item in items {
                    if let ast::MetaItem::Call(_, expr) = item {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                }
            }
        }
    }
}

fn normalize_generics_for_body_only_comparison(generics: &mut [ast::GenericParam]) {
    for generic in generics {
        generic.span = Span::default();
    }
}

fn normalize_where_clauses_for_body_only_comparison(where_clauses: &mut [ast::WhereClause]) {
    for clause in where_clauses {
        clause.span = Span::default();
        normalize_type_for_body_only_comparison(&mut clause.target_ty);
        for bound in &mut clause.bounds {
            normalize_type_for_body_only_comparison(bound);
        }
    }
}

fn normalize_func_param_for_body_only_comparison(param: &mut ast::FuncParam) {
    param.span = Span::default();
    normalize_binding_pattern_for_body_only_comparison(&mut param.pattern);
    normalize_type_for_body_only_comparison(&mut param.type_node);
}

fn normalize_binding_pattern_for_body_only_comparison(pattern: &mut ast::BindingPattern) {
    pattern.name_span = Span::default();
    pattern.span = Span::default();
}

fn normalize_use_target_for_body_only_comparison(target: &mut ast::UseTarget) {
    if let ast::UseTarget::Members(members) = target {
        for member in members {
            member.span = Span::default();
        }
    }
}

fn normalize_type_for_body_only_comparison(ty: &mut ast::TypeNode) {
    ty.id = NodeId(0);
    ty.span = Span::default();
    match &mut ty.kind {
        ast::TypeKind::Path {
            generics,
            segment_spans,
            ..
        } => {
            for span in segment_spans {
                *span = Span::default();
            }
            for generic in generics {
                normalize_type_for_body_only_comparison(generic);
            }
        }
        ast::TypeKind::Pointer { elem, .. }
        | ast::TypeKind::VolatilePtr { elem, .. }
        | ast::TypeKind::ArrayInfer { elem, .. }
        | ast::TypeKind::Slice { elem, .. } => normalize_type_for_body_only_comparison(elem),
        ast::TypeKind::Array { elem, len, .. } => {
            normalize_type_for_body_only_comparison(elem);
            normalize_expr_for_body_only_comparison(len);
        }
        ast::TypeKind::Function { params, ret, .. }
        | ast::TypeKind::ClosureInterface { params, ret } => {
            for param in params {
                normalize_type_for_body_only_comparison(param);
            }
            if let Some(ret) = ret {
                normalize_type_for_body_only_comparison(ret);
            }
        }
        ast::TypeKind::Struct { fields, .. }
        | ast::TypeKind::Union { fields, .. }
        | ast::TypeKind::Trait { fields } => {
            for field in fields {
                normalize_struct_field_for_body_only_comparison(field);
            }
        }
        ast::TypeKind::Enum {
            backing_type,
            variants,
        } => {
            if let Some(backing_type) = backing_type {
                normalize_type_for_body_only_comparison(backing_type);
            }
            for variant in variants {
                variant.span = Span::default();
                variant.name_span = Span::default();
                if let Some(payload_type) = &mut variant.payload_type {
                    normalize_type_for_body_only_comparison(payload_type);
                }
                if let Some(value) = &mut variant.value {
                    normalize_expr_for_body_only_comparison(value);
                }
            }
        }
        ast::TypeKind::TypeOf(expr) => normalize_expr_for_body_only_comparison(expr),
        ast::TypeKind::Infer
        | ast::TypeKind::SelfType
        | ast::TypeKind::Never
        | ast::TypeKind::Void => {}
    }
}

fn normalize_struct_field_for_body_only_comparison(field: &mut ast::StructFieldDef) {
    field.span = Span::default();
    field.name_span = Span::default();
    normalize_type_for_body_only_comparison(&mut field.type_node);
    field.default_value = None;
}

fn normalize_expr_for_body_only_comparison(expr: &mut ast::Expr) {
    expr.id = NodeId(0);
    expr.span = Span::default();
    match &mut expr.kind {
        ast::ExprKind::Let {
            pattern,
            init,
            else_branch,
        } => {
            normalize_let_pattern_for_body_only_comparison(pattern);
            normalize_expr_for_body_only_comparison(init);
            if let Some(else_branch) = else_branch {
                normalize_expr_for_body_only_comparison(else_branch);
            }
        }
        ast::ExprKind::Static { pattern, init } => {
            normalize_binding_pattern_for_body_only_comparison(pattern);
            normalize_expr_for_body_only_comparison(init);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
        ast::ExprKind::EnumLiteral { variant_span, .. } => {
            *variant_span = Span::default();
        }
        ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_expr_for_body_only_comparison(rhs);
        }
        ast::ExprKind::Unary { operand, .. } => normalize_expr_for_body_only_comparison(operand),
        ast::ExprKind::FieldAccess { lhs, .. } => normalize_expr_for_body_only_comparison(lhs),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_expr_for_body_only_comparison(index);
        }
        ast::ExprKind::Call { callee, args } => {
            normalize_expr_for_body_only_comparison(callee);
            for arg in args {
                normalize_expr_for_body_only_comparison(arg);
            }
        }
        ast::ExprKind::DataInit { type_node, literal } => {
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            normalize_data_literal_for_body_only_comparison(literal);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            normalize_expr_for_body_only_comparison(cond);
            normalize_expr_for_body_only_comparison(then_branch);
            if let Some(else_branch) = else_branch {
                normalize_expr_for_body_only_comparison(else_branch);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            normalize_expr_for_body_only_comparison(target);
            for arm in arms {
                arm.span = Span::default();
                for pattern in &mut arm.patterns {
                    normalize_match_pattern_for_body_only_comparison(pattern);
                }
                normalize_expr_for_body_only_comparison(&mut arm.body);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                stmt.id = NodeId(0);
                stmt.span = Span::default();
                normalize_attributes_for_body_only_comparison(&mut stmt.attributes);
                match &mut stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                }
            }
            if let Some(result) = result {
                normalize_expr_for_body_only_comparison(result);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                normalize_expr_for_body_only_comparison(init);
            }
            if let Some(cond) = cond {
                normalize_expr_for_body_only_comparison(cond);
            }
            if let Some(post) = post {
                normalize_expr_for_body_only_comparison(post);
            }
            normalize_expr_for_body_only_comparison(body);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            normalize_expr_for_body_only_comparison(lhs);
            if let Some(start) = start {
                normalize_expr_for_body_only_comparison(start);
            }
            if let Some(end) = end {
                normalize_expr_for_body_only_comparison(end);
            }
        }
        ast::ExprKind::Defer { expr: inner } => normalize_expr_for_body_only_comparison(inner),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                normalize_expr_for_body_only_comparison(value);
            }
        }
        ast::ExprKind::As { lhs, target } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_type_for_body_only_comparison(target);
        }
        ast::ExprKind::GenericInstantiation { target, types } => {
            normalize_expr_for_body_only_comparison(target);
            for ty in types {
                normalize_type_for_body_only_comparison(ty);
            }
        }
        ast::ExprKind::Closure {
            captures,
            params,
            ret_type,
            body,
        } => {
            for capture in captures {
                capture.name_span = Span::default();
                capture.span = Span::default();
                normalize_expr_for_body_only_comparison(&mut capture.value);
            }
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            normalize_expr_for_body_only_comparison(body);
        }
    }
}

fn normalize_data_literal_for_body_only_comparison(literal: &mut ast::DataLiteralKind) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                field.span = Span::default();
                field.name_span = Span::default();
                normalize_expr_for_body_only_comparison(&mut field.value);
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                normalize_expr_for_body_only_comparison(item);
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            normalize_expr_for_body_only_comparison(value);
            normalize_expr_for_body_only_comparison(count);
        }
        ast::DataLiteralKind::Scalar(value) => normalize_expr_for_body_only_comparison(value),
    }
}

fn normalize_let_pattern_for_body_only_comparison(pattern: &mut ast::LetPattern) {
    pattern.span = Span::default();
    match &mut pattern.kind {
        ast::LetPatternKind::Binding(binding) => {
            normalize_binding_pattern_for_body_only_comparison(binding);
        }
        ast::LetPatternKind::Variant(variant) => {
            variant.variant_span = Span::default();
            if let Some(target_type) = &mut variant.target_type {
                normalize_type_for_body_only_comparison(target_type);
            }
            if let Some(binding) = &mut variant.binding {
                normalize_binding_pattern_for_body_only_comparison(binding);
            }
        }
    }
}

fn normalize_match_pattern_for_body_only_comparison(pattern: &mut ast::MatchPattern) {
    pattern.span = Span::default();
    match &mut pattern.kind {
        ast::MatchPatternKind::Value(value) => normalize_expr_for_body_only_comparison(value),
        ast::MatchPatternKind::Range { start, end, .. } => {
            normalize_expr_for_body_only_comparison(start);
            normalize_expr_for_body_only_comparison(end);
        }
        ast::MatchPatternKind::Variant(variant) => {
            variant.variant_span = Span::default();
            if let Some(target_type) = &mut variant.target_type {
                normalize_type_for_body_only_comparison(target_type);
            }
            if let Some(binding) = &mut variant.binding {
                normalize_binding_pattern_for_body_only_comparison(binding);
            }
        }
        ast::MatchPatternKind::CatchAll => {}
    }
}

fn placeholder_expr() -> ast::Expr {
    ast::Expr {
        id: NodeId(0),
        span: Span::default(),
        kind: ast::ExprKind::Infer,
    }
}
