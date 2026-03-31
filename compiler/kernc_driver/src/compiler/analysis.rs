use super::completion::CompletionModel;
use super::{
    AnalysisArtifact, AnalysisHover, AnalysisReference, AnalysisReport, AnalysisSymbol,
    AnalysisSymbolKind, CompilerDriver, SourceOverrides,
};
use crate::loader::ModuleLoader;
use kernc_ast as ast;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::DefId;
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_sema::{BuiltinInjector, SemaContext};
use kernc_utils::Session;

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

        let mut ctx = self.build_sema_context(&mut session);
        let Some(asts) = self.load_asts(&mut ctx, input_file, source_overrides) else {
            return AnalysisArtifact {
                session,
                succeeded: false,
                symbols: Vec::new(),
                references: Vec::new(),
                hovers: Vec::new(),
                completion_model: CompletionModel::default(),
            };
        };

        let analysis_asts = asts.clone();
        let succeeded = self.run_sema_pipeline(&mut ctx, asts);
        let symbols = self.collect_analysis_symbols(&ctx, &analysis_asts);
        let references = ctx
            .identifier_references()
            .iter()
            .map(|(reference_span, definition_span)| AnalysisReference {
                reference_span: *reference_span,
                definition_span: *definition_span,
            })
            .collect();
        let hovers = self.collect_analysis_hovers(&ctx);
        let completion_model = self.collect_completion_model(&mut ctx, &analysis_asts);
        drop(ctx);

        AnalysisArtifact {
            session,
            succeeded,
            symbols,
            references,
            hovers,
            completion_model,
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

    pub(super) fn run_sema_pipeline<'a>(
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

        let mut typeck = TypeckDriver::new(type_resolver.into_context());
        typeck.check_all();
        let ctx = typeck.into_context();
        if !Self::report_diagnostics_if_errors(ctx) {
            return false;
        }

        let mut linkage_checker = LinkageChecker::new(ctx);
        linkage_checker.check_all();
        Self::report_diagnostics_if_errors(linkage_checker.context())
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

        by_span
            .into_iter()
            .map(|(span, contents)| AnalysisHover { span, contents })
            .collect()
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
                let sig = function.resolved_sig?;
                format!("fn {}: {}", name, ctx.ty_to_string(sig))
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
            ast::TypeKind::Path { segments, generics } => {
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
