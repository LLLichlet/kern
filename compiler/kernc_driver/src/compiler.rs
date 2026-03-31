use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use kernc_ast as ast;
use kernc_codegen::{CodeGenerator, Context, InlineAsmDialect};
use kernc_lower::Lowerer;
use kernc_sema::BuiltinInjector;
use kernc_sema::MemberCandidate;
use kernc_sema::MemberQuery;
use kernc_sema::MemberQueryEnv;
use kernc_sema::SemaContext;
use kernc_sema::checker::TypeckDriver;
use kernc_sema::def::DefId;
use kernc_sema::passes::{Collector, ImportResolver, LinkageChecker, TypeResolver};
use kernc_utils::{FileId, Session};
use kernc_utils::config::{AsmDialect, CompileOptions, DriverMode, LinkProfile};

use crate::loader::ModuleLoader;
use crate::metadata;

pub type SourceOverrides = HashMap<PathBuf, String>;

pub struct AnalysisReport {
    pub session: Session,
    pub succeeded: bool,
}

#[derive(Debug, Clone)]
pub struct AnalysisReference {
    pub reference_span: kernc_utils::Span,
    pub definition_span: kernc_utils::Span,
}

#[derive(Debug, Clone)]
pub struct AnalysisHover {
    pub span: kernc_utils::Span,
    pub contents: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCompletionKind {
    Variable,
    Function,
    Module,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
    TypeParameter,
}

#[derive(Debug, Clone)]
pub struct AnalysisCompletionItem {
    pub label: String,
    pub kind: AnalysisCompletionKind,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisSymbolKind {
    Module,
    Namespace,
    Function,
    Method,
    Struct,
    Union,
    Enum,
    Trait,
    TypeAlias,
    Constant,
    Static,
}

#[derive(Debug, Clone)]
pub struct AnalysisSymbol {
    pub name: String,
    pub kind: AnalysisSymbolKind,
    pub span: kernc_utils::Span,
    pub selection_span: kernc_utils::Span,
    pub detail: Option<String>,
    pub children: Vec<AnalysisSymbol>,
}

pub struct AnalysisArtifact {
    pub session: Session,
    pub succeeded: bool,
    pub symbols: Vec<AnalysisSymbol>,
    pub references: Vec<AnalysisReference>,
    pub hovers: Vec<AnalysisHover>,
    completion_model: CompletionModel,
}

#[derive(Debug, Clone)]
struct CompletionModule {
    file_id: FileId,
    ast: ast::Module,
    top_level_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone, Default)]
struct CompletionModel {
    root_items: Vec<AnalysisCompletionItem>,
    items_by_span: BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    function_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    member_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    modules: Vec<CompletionModule>,
}

pub struct CompilerDriver {
    pub options: CompileOptions,
}

/// 临时文件守卫 (RAII)
/// 当变量离开作用域时，自动删除产生的临时文件
struct TempFileGuard {
    path: String,
}

struct LinkTarget {
    triple: String,
    is_windows: bool,
    is_darwin: bool,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl AnalysisArtifact {
    pub fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        self.completion_model
            .completion_items(&self.session, target_path, offset)
    }
}

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self { options }
    }

    pub fn compile(&self) -> bool {
        if self.options.driver_mode == DriverMode::LinkOnly {
            return self.link_only();
        }

        let Some(input_file) = self.options.input_file.as_deref() else {
            eprintln!("Error: compile mode requires a source input.");
            return false;
        };

        let mut session = Session::new();
        let Some(mut ctx) = self.analyze(&mut session, input_file) else {
            return false;
        };

        let Some(mast_module) = self.lower_module(&mut ctx) else {
            return false;
        };

        if let Some(metadata_output) = self.options.metadata_output.as_deref()
            && let Err(err) = metadata::emit_package_metadata(
                &ctx,
                std::path::Path::new(metadata_output),
                self.options
                    .metadata_package_name
                    .as_deref()
                    .or(self.options.root_module_name.as_deref())
                    .unwrap_or("root"),
                self.options.metadata_package_version.as_deref(),
            )
        {
            eprintln!("Error: Failed to emit kmeta snapshot: {}", err);
            return false;
        }

        let codegen_ctx = Context::create();
        let mut codegen = CodeGenerator::new(
            &codegen_ctx,
            &self.module_name_for_codegen(input_file),
            &mut *ctx.sess,
            &ctx.type_registry,
        );

        codegen.set_asm_dialect(match self.options.asm_dialect {
            AsmDialect::Intel => InlineAsmDialect::Intel,
            AsmDialect::Att => InlineAsmDialect::ATT,
        });

        codegen.compile(&mast_module);

        if self.options.driver_mode == DriverMode::EmitLlvmIr {
            return match codegen.print_ir() {
                Ok(()) => true,
                Err(err) => {
                    eprintln!("Error: Failed to print LLVM IR: {}", err);
                    false
                }
            };
        }

        let target = self.normalized_target();
        let link_input_path = self.prepare_link_input_path(&target);
        let _guard = self.temp_link_input_guard(&link_input_path);

        if let Err(e) =
            codegen.emit_to_file(&target.triple, &link_input_path, self.options.opt_level)
        {
            eprintln!("Error: LLVM failed to generate intermediate file: {}", e);
            return false;
        }

        if self.options.driver_mode.emits_linker_input() {
            println!(
                "Successfully emitted linker input to `{}`",
                self.options.output_file
            );
            return true;
        }

        self.run_link_command(Some(&link_input_path), &target, "Successfully compiled")
    }

    pub fn analyze<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
    ) -> Option<SemaContext<'a>> {
        self.analyze_with_overrides(session, input_file, &SourceOverrides::new())
    }

    pub fn analyze_with_overrides<'a>(
        &self,
        session: &'a mut Session,
        input_file: &str,
        source_overrides: &SourceOverrides,
    ) -> Option<SemaContext<'a>> {
        session.apply_options(&self.options);

        let mut ctx = self.build_sema_context(session);
        let asts = self.load_asts(&mut ctx, input_file, source_overrides)?;
        if !self.run_sema_pipeline(&mut ctx, asts) {
            return None;
        }

        Some(ctx)
    }

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

    fn link_only(&self) -> bool {
        if self.options.linker_inputs.is_empty() {
            eprintln!("Error: `--link-only` requires at least one `--link-input`.");
            return false;
        }

        let target = self.normalized_target();
        self.run_link_command(None, &target, "Successfully linked")
    }

    fn build_sema_context<'a>(&self, session: &'a mut Session) -> SemaContext<'a> {
        let mut ctx = SemaContext::new(session);
        ctx.module_aliases = self.options.module_aliases.clone();
        ctx.module_interface_aliases = self.options.module_interface_aliases.clone();

        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();
        ctx
    }

    fn load_asts<'a>(
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

    fn run_sema_pipeline<'a>(
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
                kernc_sema::def::Def::Module(module_def) => ctx.resolve(module_def.name).to_string(),
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
        let mut by_span = BTreeMap::new();
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

    fn collect_completion_model(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> CompletionModel {
        let mut items_by_span = BTreeMap::new();
        for (name, info) in ctx.scopes.all_symbols() {
            let Some(item) = self.completion_item_for_symbol(ctx, name, info) else {
                continue;
            };

            items_by_span.entry(info.span).or_insert(item);
        }

        let root_items = ctx
            .scopes
            .symbols_in_scope(kernc_sema::scope::ScopeId(0))
            .filter_map(|(name, info)| self.completion_item_for_symbol(ctx, name, info))
            .collect();

        let mut function_items_by_span = BTreeMap::new();
        for def in &ctx.defs {
            let kernc_sema::def::Def::Function(function) = def else {
                continue;
            };

            function_items_by_span.insert(
                function.span,
                self.initial_completion_items_for_function(ctx, function),
            );
        }

        let mut member_items_by_span = BTreeMap::new();
        let mut member_query = MemberQuery::new(ctx);
        let mut member_env = MemberQueryEnv::default();
        let modules = asts
            .iter()
            .filter_map(|(mod_id, ast)| {
                let kernc_sema::def::Def::Module(module_def) = &member_query.context().defs[mod_id.0 as usize] else {
                    return None;
                };
                let module_file_id = module_def.file_id;
                let module_scope_id = module_def.scope_id;

                let top_level_items = member_query
                    .context()
                    .scopes
                    .symbols_in_scope(module_scope_id)
                    .filter_map(|(name, info)| self.completion_item_for_symbol(member_query.context(), name, info))
                    .collect();

                self.collect_member_completion_items_in_module(
                    &mut member_query,
                    *mod_id,
                    ast,
                    &mut member_env,
                    &mut member_items_by_span,
                );

                Some(CompletionModule {
                    file_id: module_file_id,
                    ast: ast.clone(),
                    top_level_items,
                })
            })
            .collect();

        CompletionModel {
            root_items,
            items_by_span,
            function_items_by_span,
            member_items_by_span,
            modules,
        }
    }

    fn collect_member_completion_items_in_module(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        module: &ast::Module,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        for decl in &module.decls {
            self.collect_member_completion_items_in_decl(
                member_query,
                module_id,
                decl,
                member_env,
                member_items_by_span,
            );
        }
    }

    fn collect_member_completion_items_in_decl(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        decl: &ast::Decl,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &decl.kind {
            ast::DeclKind::Function {
                where_clauses,
                body,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                if let Some(body) = body {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        body,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    value,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::DeclKind::Impl {
                where_clauses,
                decls,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            _ => {}
        }
    }

    fn collect_member_completion_items_in_expr(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        expr: &ast::Expr,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &expr.kind {
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if let Some(lhs_ty) = member_query.context().node_types.get(&lhs.id).copied() {
                    let items = member_query
                        .member_candidates_in_env(Some(module_id), lhs_ty, member_env)
                        .into_iter()
                        .filter_map(|candidate| {
                            self.completion_item_for_member_candidate(member_query.context(), candidate)
                        })
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        member_items_by_span.insert(expr.span, items);
                    }
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::ExprStmt(inner) | ast::StmtKind::ExprValue(inner) => {
                            self.collect_member_completion_items_in_expr(
                                member_query,
                                module_id,
                                inner,
                                member_env,
                                member_items_by_span,
                            );
                        }
                    }
                }
                if let Some(result) = result {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        result,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Let {
                init,
                else_branch,
                ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_branch) = else_branch {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        else_branch,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Static { init, .. }
            | ast::ExprKind::Unary { operand: init, .. }
            | ast::ExprKind::Defer { expr: init } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Binary { lhs, rhs, .. }
            | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    rhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    index,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Call { callee, args } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    callee,
                    member_env,
                    member_items_by_span,
                );
                for arg in args {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        arg,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => {
                    for field in fields {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            &field.value,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Array(items) => {
                    for item in items {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            item,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        count,
                        member_env,
                        member_items_by_span,
                    );
                }
                ast::DataLiteralKind::Scalar(value) => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                }
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    cond,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    then_branch,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_branch) = else_branch {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        else_branch,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    target,
                    member_env,
                    member_items_by_span,
                );
                for arm in arms {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &arm.body,
                        member_env,
                        member_items_by_span,
                    );
                    for pattern in &arm.patterns {
                        match &pattern.kind {
                            ast::MatchPatternKind::Value(value) => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    value,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    start,
                                    member_env,
                                    member_items_by_span,
                                );
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    end,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                if let Some(init) = init {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        init,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(cond) = cond {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        cond,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(post) = post {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        post,
                        member_env,
                        member_items_by_span,
                    );
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::SliceOp {
                lhs,
                start,
                end,
                ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                if let Some(start) = start {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        start,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(end) = end {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        end,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::As { lhs, .. } | ast::ExprKind::GenericInstantiation { target: lhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &capture.value,
                        member_env,
                        member_items_by_span,
                    );
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            _ => {}
        }
    }

    fn initial_completion_items_for_function(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
    ) -> Vec<AnalysisCompletionItem> {
        let mut items = Vec::new();

        for generic in &function.generics {
            push_completion_item(
                &mut items,
                AnalysisCompletionItem {
                    label: ctx.resolve(generic.name).to_string(),
                    kind: AnalysisCompletionKind::TypeParameter,
                    detail: Some("type".to_string()),
                },
            );
        }

        let Some(sig) = function.resolved_sig else {
            return items;
        };
        let kernc_sema::ty::TypeKind::Function { params, .. } = ctx.type_registry.get(sig).clone() else {
            return items;
        };

        for (index, param) in function.params.iter().enumerate() {
            let name = ctx.resolve(param.pattern.name);
            if name == "_" {
                continue;
            }

            push_completion_item(
                &mut items,
                AnalysisCompletionItem {
                    label: name.to_string(),
                    kind: AnalysisCompletionKind::Variable,
                    detail: params
                        .get(index)
                        .copied()
                        .map(|ty| ctx.ty_to_string(ty)),
                },
            );
        }

        items
    }

    fn completion_item_for_symbol(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        info: &kernc_sema::scope::SymbolInfo,
    ) -> Option<AnalysisCompletionItem> {
        let label = ctx.resolve(name);
        if label == "_" {
            return None;
        }

        let (kind, detail) = self.symbol_completion_presentation(ctx, name, info)?;
        Some(AnalysisCompletionItem {
            label: label.to_string(),
            kind,
            detail,
        })
    }

    fn symbol_completion_presentation(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        info: &kernc_sema::scope::SymbolInfo,
    ) -> Option<(AnalysisCompletionKind, Option<String>)> {
        let name = ctx.resolve(name);

        let presentation = match info.kind {
            kernc_sema::scope::SymbolKind::Function => {
                let def_id = info.def_id?;
                let kernc_sema::def::Def::Function(function) = &ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                let sig = function.resolved_sig?;
                (
                    AnalysisCompletionKind::Function,
                    Some(ctx.ty_to_string(sig)),
                )
            }
            kernc_sema::scope::SymbolKind::Const => {
                let mut detail = String::from("const");
                if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(info.type_id));
                }
                (AnalysisCompletionKind::Constant, Some(detail))
            }
            kernc_sema::scope::SymbolKind::Static => {
                let mut detail = String::from("static");
                if info.is_mut {
                    detail.push_str(" mut");
                }
                if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(info.type_id));
                }
                (AnalysisCompletionKind::Static, Some(detail))
            }
            kernc_sema::scope::SymbolKind::Var => (
                AnalysisCompletionKind::Variable,
                Some(ctx.ty_to_string(info.type_id)),
            ),
            kernc_sema::scope::SymbolKind::Struct => {
                (AnalysisCompletionKind::Struct, Some("struct".to_string()))
            }
            kernc_sema::scope::SymbolKind::Union => {
                (AnalysisCompletionKind::Union, Some("union".to_string()))
            }
            kernc_sema::scope::SymbolKind::Enum => {
                (AnalysisCompletionKind::Enum, Some("enum".to_string()))
            }
            kernc_sema::scope::SymbolKind::Trait => {
                (AnalysisCompletionKind::Trait, Some("trait".to_string()))
            }
            kernc_sema::scope::SymbolKind::Module => (
                AnalysisCompletionKind::Module,
                Some(format!("module {}", name)),
            ),
            kernc_sema::scope::SymbolKind::TypeAlias => {
                let detail = if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                {
                    ctx.node_types
                        .get(&alias.target.id)
                        .copied()
                        .map(|target_ty| format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    Some(format!("type = {}", ctx.ty_to_string(info.type_id)))
                } else {
                    Some("type".to_string())
                };

                (AnalysisCompletionKind::TypeAlias, detail)
            }
            kernc_sema::scope::SymbolKind::TypeParam => {
                (AnalysisCompletionKind::TypeParameter, Some("type".to_string()))
            }
        };

        Some(presentation)
    }

    fn completion_item_for_member_candidate(
        &self,
        ctx: &SemaContext<'_>,
        candidate: MemberCandidate,
    ) -> Option<AnalysisCompletionItem> {
        let detail = match candidate.kind {
            kernc_sema::scope::SymbolKind::Function => Some(ctx.ty_to_string(candidate.type_id)),
            kernc_sema::scope::SymbolKind::Var => Some(ctx.ty_to_string(candidate.type_id)),
            kernc_sema::scope::SymbolKind::Static => {
                let mut detail = String::from("static");
                if candidate.is_mut {
                    detail.push_str(" mut");
                }
                if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(candidate.type_id));
                }
                Some(detail)
            }
            kernc_sema::scope::SymbolKind::Const => {
                let mut detail = String::from("const");
                if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(candidate.type_id));
                }
                Some(detail)
            }
            kernc_sema::scope::SymbolKind::Module => Some(format!(
                "module {}",
                ctx.resolve(candidate.name)
            )),
            kernc_sema::scope::SymbolKind::Struct => Some("struct".to_string()),
            kernc_sema::scope::SymbolKind::Union => Some("union".to_string()),
            kernc_sema::scope::SymbolKind::Enum => Some("enum".to_string()),
            kernc_sema::scope::SymbolKind::Trait => Some("trait".to_string()),
            kernc_sema::scope::SymbolKind::TypeParam => Some("type".to_string()),
            kernc_sema::scope::SymbolKind::TypeAlias => {
                if let Some(def_id) = candidate.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                    && let Some(target_ty) = ctx.node_types.get(&alias.target.id).copied()
                {
                    Some(format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    Some(format!("type = {}", ctx.ty_to_string(candidate.type_id)))
                } else {
                    Some("type".to_string())
                }
            }
        };

        Some(AnalysisCompletionItem {
            label: ctx.resolve(candidate.name).to_string(),
            kind: completion_kind_from_symbol_kind(candidate.kind),
            detail,
        })
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
            format!("impl {} : {}", target, self.describe_type_node(ctx, trait_type))
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

    fn lower_module<'a>(&self, ctx: &mut SemaContext<'a>) -> Option<kernc_mast::MastModule> {
        let mut lowerer = Lowerer::new(ctx);
        let module = lowerer.lower_all();
        if !Self::report_diagnostics_if_errors(lowerer.context()) {
            return None;
        }
        Some(module)
    }

    fn module_name_for_codegen(&self, input_file: &str) -> String {
        Path::new(input_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("kern_module")
            .to_string()
    }

    fn report_diagnostics_if_errors(ctx: &mut SemaContext<'_>) -> bool {
        if ctx.has_errors() {
            ctx.sess.print_diagnostics();
            return false;
        }
        true
    }

    fn normalized_target(&self) -> LinkTarget {
        let raw_triple = self.options.target.triple.to_string();
        let is_windows = raw_triple.contains("windows");
        let is_darwin = raw_triple.contains("darwin") || raw_triple.contains("macosx");
        let triple = if is_darwin {
            normalize_darwin_triple_str(&raw_triple)
        } else {
            raw_triple
        };

        LinkTarget {
            triple,
            is_windows,
            is_darwin,
        }
    }

    fn prepare_link_input_path(&self, _target: &LinkTarget) -> String {
        if self.options.driver_mode.emits_linker_input() {
            self.options.output_file.clone()
        } else {
            self.make_temp_link_input_path()
        }
    }

    fn temp_link_input_guard(&self, link_input_path: &str) -> Option<TempFileGuard> {
        if self.options.driver_mode.emits_linker_input() {
            None
        } else {
            Some(TempFileGuard {
                path: link_input_path.to_string(),
            })
        }
    }

    fn run_link_command(
        &self,
        link_input_path: Option<&str>,
        target: &LinkTarget,
        success_prefix: &str,
    ) -> bool {
        println!("Linking for target: {} ...", target.triple);
        let mut cmd = self.build_link_command(link_input_path, target);
        self.maybe_print_link_command(&cmd);

        match cmd.status() {
            Ok(s) if s.success() => {
                println!("{} to `{}`", success_prefix, self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                let cc_compiler = self.resolve_linker_driver(target.is_windows);
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure Clang or GCC is in your PATH. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }

    fn make_temp_link_input_path(&self) -> String {
        let tmp_ext = "o";
        format!("{}.tmp.{}", self.options.output_file, tmp_ext)
    }

    fn resolve_linker_driver(&self, is_windows: bool) -> String {
        if is_windows && self.options.linker_cmd == "cc" {
            "clang".to_string()
        } else {
            self.options.linker_cmd.clone()
        }
    }

    fn build_link_command(&self, link_input_path: Option<&str>, target: &LinkTarget) -> Command {
        let cc_compiler = self.resolve_linker_driver(target.is_windows);
        let mut cmd = Command::new(&cc_compiler);

        if let Some(link_input_path) = link_input_path {
            cmd.arg(link_input_path);
        }

        for input in &self.options.linker_inputs {
            cmd.arg(input);
        }

        cmd.arg("-o").arg(&self.options.output_file);

        self.apply_link_profile(&mut cmd, target.is_windows, target.is_darwin);

        for path in &self.options.linker_search_paths {
            cmd.arg(format!("-L{}", path));
        }

        for lib in &self.options.linker_libraries {
            cmd.arg(format!("-l{}", lib));
        }

        for arg in &self.options.linker_args {
            cmd.arg(arg);
        }

        cmd
    }

    fn apply_link_profile(&self, cmd: &mut Command, is_windows: bool, is_darwin: bool) {
        match self.options.link_profile {
            LinkProfile::None => {}
            LinkProfile::Hosted => {
                if !is_windows && !is_darwin {
                    cmd.arg("-no-pie");
                }
                if let Some(entry_symbol) = &self.options.entry_symbol {
                    cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                }
            }
            LinkProfile::Freestanding => {
                if is_windows {
                    cmd.arg("-Wno-override-module");
                    cmd.arg("-nostdlib");
                    cmd.arg("-Wl,/subsystem:console");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                    }
                } else if is_darwin {
                    cmd.arg("-nostdlib");
                    cmd.arg(format!(
                        "-Wl,-e,{}",
                        self.options.entry_symbol.as_deref().unwrap_or("_start")
                    ));
                } else {
                    cmd.arg("-no-pie");
                    cmd.arg("-nostdlib");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
            LinkProfile::Kern => {
                if is_windows {
                    cmd.arg("-Wno-override-module");
                    cmd.arg("-nostdlib");
                    cmd.arg("-Wl,/subsystem:console");
                    cmd.arg("-lkernel32");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,/entry:{}", entry_symbol));
                    }
                } else if is_darwin {
                    cmd.arg("-nostdlib");
                    cmd.arg("-lSystem");
                    cmd.arg(format!(
                        "-Wl,-e,{}",
                        self.options.entry_symbol.as_deref().unwrap_or("_start")
                    ));
                } else {
                    cmd.arg("-no-pie");
                    cmd.arg("-nostdlib");
                    if let Some(entry_symbol) = &self.options.entry_symbol {
                        cmd.arg(format!("-Wl,-e,{}", entry_symbol));
                    }
                }
            }
        }
    }

    fn maybe_print_link_command(&self, cmd: &Command) {
        if self.options.print_link_command {
            println!("Link command: {}", self.format_command(cmd));
        }
    }

    fn format_command(&self, cmd: &Command) -> String {
        let mut parts = Vec::new();
        parts.push(shell_quote(cmd.get_program().to_string_lossy().as_ref()));

        for arg in cmd.get_args() {
            parts.push(shell_quote(arg.to_string_lossy().as_ref()));
        }

        parts.join(" ")
    }
}

impl CompletionModel {
    fn completion_items(
        &self,
        session: &Session,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        let Some(module) = self.modules.iter().find(|module| {
            session
                .source_manager
                .get_file_path(module.file_id)
                .map(|path| normalize_analysis_path(path) == target_path)
                .unwrap_or(false)
        }) else {
            return Vec::new();
        };

        let mut visible = Vec::new();
        for item in &self.root_items {
            push_completion_item(&mut visible, item.clone());
        }
        for item in &module.top_level_items {
            push_completion_item(&mut visible, item.clone());
        }

        for decl in &module.ast.decls {
            if self.collect_in_decl(decl, &mut visible, offset) {
                break;
            }
        }

        visible
    }

    fn collect_in_decl(
        &self,
        decl: &ast::Decl,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(decl.span, offset) {
            return false;
        }

        match &decl.kind {
            ast::DeclKind::Function { body, .. } => {
                if let Some(items) = self.function_items_by_span.get(&decl.span) {
                    for item in items {
                        push_completion_item(visible, item.clone());
                    }
                }

                if let Some(body) = body
                    && span_contains_offset(body.span, offset)
                {
                    self.collect_in_expr(body, visible, offset);
                }
                true
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_in_expr(value, visible, offset);
                true
            }
            ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
                for child in decls {
                    if self.collect_in_decl(child, visible, offset) {
                        return true;
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn collect_in_expr(
        &self,
        expr: &ast::Expr,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(query_span_for_expr(expr), offset) {
            return false;
        }

        match &expr.kind {
            ast::ExprKind::Block { stmts, result } => {
                let mut block_visible = visible.clone();

                for stmt in stmts {
                    let stmt_span = query_span_for_stmt(stmt);
                    if stmt_span.start > offset {
                        break;
                    }

                    if span_contains_offset(stmt_span, offset) {
                        self.collect_in_stmt(stmt, &mut block_visible, offset);
                        *visible = block_visible;
                        return true;
                    }

                    if stmt_span.end <= offset {
                        self.record_stmt_bindings(stmt, &mut block_visible);
                    }
                }

                if let Some(result) = result
                    && span_contains_offset(result.span, offset)
                {
                    self.collect_in_expr(result, &mut block_visible, offset);
                }

                *visible = block_visible;
                true
            }
            ast::ExprKind::Let {
                pattern: _,
                init,
                else_branch,
            } => {
                if self.collect_in_expr(init, visible, offset) {
                    return true;
                }
                if let Some(else_branch) = else_branch {
                    return self.collect_in_expr(else_branch, visible, offset);
                }
                true
            }
            ast::ExprKind::Static { init, .. } => self.collect_in_expr(init, visible, offset),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::Unary { operand, .. } => self.collect_in_expr(operand, visible, offset),
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if span_contains_offset(lhs.span, offset) {
                    self.collect_in_expr(lhs, visible, offset)
                } else if let Some(items) = self.member_items_by_span.get(&expr.span) {
                    *visible = items.clone();
                    true
                } else {
                    true
                }
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(index, visible, offset)
            }
            ast::ExprKind::Call { callee, args } => {
                if self.collect_in_expr(callee, visible, offset) {
                    return true;
                }
                for arg in args {
                    if self.collect_in_expr(arg, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::ExprKind::DataInit { literal, .. } => self.collect_in_data_literal(literal, visible, offset),
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if self.collect_in_expr(cond, visible, offset) {
                    return true;
                }

                if span_contains_offset(then_branch.span, offset) {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(then_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                if let Some(else_branch) = else_branch
                    && span_contains_offset(else_branch.span, offset)
                {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(else_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                true
            }
            ast::ExprKind::Match { target, arms } => {
                if self.collect_in_expr(target, visible, offset) {
                    return true;
                }

                for arm in arms {
                    if !span_contains_offset(arm.span, offset) {
                        continue;
                    }

                    if span_contains_offset(arm.body.span, offset) {
                        let mut arm_visible = visible.clone();
                        self.record_match_arm_bindings(arm, &mut arm_visible);
                        self.collect_in_expr(&arm.body, &mut arm_visible, offset);
                        *visible = arm_visible;
                        return true;
                    }

                    for pattern in &arm.patterns {
                        if self.collect_in_match_pattern(pattern, visible, offset) {
                            return true;
                        }
                    }

                    return true;
                }

                true
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let mut loop_visible = visible.clone();

                if let Some(init) = init {
                    if self.collect_in_expr(init, &mut loop_visible, offset) {
                        *visible = loop_visible;
                        return true;
                    }
                    if init.span.end <= offset {
                        self.record_expr_bindings(init, &mut loop_visible);
                    }
                }

                if let Some(cond) = cond
                    && self.collect_in_expr(cond, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if let Some(post) = post
                    && self.collect_in_expr(post, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if span_contains_offset(body.span, offset) {
                    self.collect_in_expr(body, &mut loop_visible, offset);
                    *visible = loop_visible;
                }

                true
            }
            ast::ExprKind::SliceOp { lhs, start, end, .. } => {
                if self.collect_in_expr(lhs, visible, offset) {
                    return true;
                }
                if let Some(start) = start
                    && self.collect_in_expr(start, visible, offset)
                {
                    return true;
                }
                if let Some(end) = end
                    && self.collect_in_expr(end, visible, offset)
                {
                    return true;
                }
                true
            }
            ast::ExprKind::Defer { expr } => self.collect_in_expr(expr, visible, offset),
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    return self.collect_in_expr(value, visible, offset);
                }
                true
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::As { lhs, .. } => self.collect_in_expr(lhs, visible, offset),
            ast::ExprKind::GenericInstantiation { target, .. } => {
                self.collect_in_expr(target, visible, offset)
            }
            ast::ExprKind::Closure {
                captures,
                params,
                body,
                ..
            } => {
                for capture in captures {
                    if self.collect_in_expr(&capture.value, visible, offset) {
                        return true;
                    }
                }

                if span_contains_offset(body.span, offset) {
                    let mut closure_visible = visible.clone();
                    for capture in captures {
                        self.record_span_binding(capture.span, &mut closure_visible);
                    }
                    for param in params {
                        self.record_span_binding(param.span, &mut closure_visible);
                    }
                    self.collect_in_expr(body, &mut closure_visible, offset);
                    *visible = closure_visible;
                }

                true
            }
            _ => true,
        }
    }

    fn collect_in_stmt(
        &self,
        stmt: &ast::Stmt,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match &stmt.kind {
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.collect_in_expr(expr, visible, offset)
            }
        }
    }

    fn collect_in_data_literal(
        &self,
        literal: &ast::DataLiteralKind,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    if self.collect_in_expr(&field.value, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    if self.collect_in_expr(item, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.collect_in_expr(value, visible, offset)
                    || self.collect_in_expr(count, visible, offset)
            }
            ast::DataLiteralKind::Scalar(value) => self.collect_in_expr(value, visible, offset),
        }
    }

    fn collect_in_match_pattern(
        &self,
        pattern: &ast::MatchPattern,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(pattern.span, offset) {
            return false;
        }

        match &pattern.kind {
            ast::MatchPatternKind::Value(value) => self.collect_in_expr(value, visible, offset),
            ast::MatchPatternKind::Range { start, end, .. } => {
                self.collect_in_expr(start, visible, offset)
                    || self.collect_in_expr(end, visible, offset)
            }
            _ => true,
        }
    }

    fn record_stmt_bindings(&self, stmt: &ast::Stmt, visible: &mut Vec<AnalysisCompletionItem>) {
        match &stmt.kind {
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.record_expr_bindings(expr, visible);
            }
        }
    }

    fn record_expr_bindings(&self, expr: &ast::Expr, visible: &mut Vec<AnalysisCompletionItem>) {
        match &expr.kind {
            ast::ExprKind::Let { pattern, .. } => self.record_let_pattern_bindings(pattern, visible),
            ast::ExprKind::Static { pattern, .. } => self.record_span_binding(pattern.span, visible),
            _ => {}
        }
    }

    fn record_let_pattern_bindings(
        &self,
        pattern: &ast::LetPattern,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        match &pattern.kind {
            ast::LetPatternKind::Binding(binding) => {
                self.record_span_binding(binding.span, visible);
            }
            ast::LetPatternKind::Variant(variant) => {
                if let Some(binding) = &variant.binding {
                    self.record_span_binding(binding.span, visible);
                }
            }
        }
    }

    fn record_match_arm_bindings(
        &self,
        arm: &ast::MatchArm,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        for pattern in &arm.patterns {
            let ast::MatchPatternKind::Variant(variant) = &pattern.kind else {
                continue;
            };
            if let Some(binding) = &variant.binding {
                self.record_span_binding(binding.span, visible);
            }
        }
    }

    fn record_span_binding(
        &self,
        span: kernc_utils::Span,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        let Some(item) = self.items_by_span.get(&span) else {
            return;
        };
        push_completion_item(visible, item.clone());
    }
}

fn push_completion_item(items: &mut Vec<AnalysisCompletionItem>, item: AnalysisCompletionItem) {
    if let Some(index) = items.iter().position(|existing| existing.label == item.label) {
        items.remove(index);
    }
    items.push(item);
}

fn query_span_for_stmt(stmt: &ast::Stmt) -> kernc_utils::Span {
    match &stmt.kind {
        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => query_span_for_expr(expr),
    }
}

fn completion_kind_from_symbol_kind(kind: kernc_sema::scope::SymbolKind) -> AnalysisCompletionKind {
    match kind {
        kernc_sema::scope::SymbolKind::Var => AnalysisCompletionKind::Variable,
        kernc_sema::scope::SymbolKind::Const => AnalysisCompletionKind::Constant,
        kernc_sema::scope::SymbolKind::Static => AnalysisCompletionKind::Static,
        kernc_sema::scope::SymbolKind::Function => AnalysisCompletionKind::Function,
        kernc_sema::scope::SymbolKind::Struct => AnalysisCompletionKind::Struct,
        kernc_sema::scope::SymbolKind::Union => AnalysisCompletionKind::Union,
        kernc_sema::scope::SymbolKind::Enum => AnalysisCompletionKind::Enum,
        kernc_sema::scope::SymbolKind::Trait => AnalysisCompletionKind::Trait,
        kernc_sema::scope::SymbolKind::Module => AnalysisCompletionKind::Module,
        kernc_sema::scope::SymbolKind::TypeAlias => AnalysisCompletionKind::TypeAlias,
        kernc_sema::scope::SymbolKind::TypeParam => AnalysisCompletionKind::TypeParameter,
    }
}

fn query_span_for_expr(expr: &ast::Expr) -> kernc_utils::Span {
    match &expr.kind {
        ast::ExprKind::Return(Some(value)) => expr.span.to(query_span_for_expr(value)),
        _ => expr.span,
    }
}

fn span_contains_offset(span: kernc_utils::Span, offset: usize) -> bool {
    let end = if span.end > span.start {
        span.end
    } else {
        span.start.saturating_add(1)
    };
    offset >= span.start && offset < end
}

fn normalize_analysis_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn shell_quote(input: &str) -> String {
    if input.is_empty() {
        return "''".to_string();
    }

    if input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '=' | ':'))
    {
        return input.to_string();
    }

    format!("'{}'", input.replace('\'', "'\"'\"'"))
}

fn normalize_darwin_triple_str(triple_str: &str) -> String {
    if triple_str.contains("macosx")
        && triple_str
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit())
    {
        return triple_str.to_string();
    }

    if triple_str.contains("darwin")
        && triple_str
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_digit())
    {
        return triple_str.to_string();
    }

    let Some(version) = detect_darwin_deployment_target() else {
        return triple_str.to_string();
    };

    if let Some(prefix) = triple_str.strip_suffix("-darwin") {
        return format!("{}-macosx{}.0.0", prefix, version);
    }

    if let Some(prefix) = triple_str.strip_suffix("-macosx") {
        return format!("{}-macosx{}.0.0", prefix, version);
    }

    triple_str.to_string()
}

fn detect_darwin_deployment_target() -> Option<u16> {
    if let Ok(version) = env::var("MACOSX_DEPLOYMENT_TARGET") {
        return parse_darwin_deployment_target_major(&version);
    }

    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let version = String::from_utf8_lossy(&output.stdout);
    parse_darwin_deployment_target_major(version.trim())
}

fn parse_darwin_deployment_target_major(version: &str) -> Option<u16> {
    version.trim().split('.').next()?.parse().ok()
}

