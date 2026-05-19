//! Frontend source, parse, and incremental cache database.
//!
//! The frontend owns file loading, source overrides for editor requests, parser
//! invocation, AST pruning, and memoized parse artifacts. Later driver stages
//! build semantic and codegen artifacts from these cached frontend results.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use kernc_ast as ast;
use kernc_db::Memo;
use kernc_db::{Database, Input, Query};
use kernc_parser::Parser;
use kernc_sema::passes::Pruner;
#[cfg(test)]
use kernc_utils::expect_uncancelable;
use kernc_utils::{
    Canceled, CancellationToken, Diagnostic, DiagnosticLevel, FileId, NodeId, Session, Span,
    SymbolId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct FrontendParsedModule {
    pub file_id: FileId,
    pub ast: ast::Module,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FrontendLoadTimings {
    pub(crate) read_source: Duration,
    pub(crate) ensure_file_id: Duration,
    pub(crate) parse: Duration,
    pub(crate) prune: Duration,
    pub(crate) rebind: Duration,
}

impl FrontendLoadTimings {
    fn add(&mut self, other: Self) {
        self.read_source += other.read_source;
        self.ensure_file_id += other.ensure_file_id;
        self.parse += other.parse;
        self.prune += other.prune;
        self.rebind += other.rebind;
    }
}

fn recover_known_override_paths_lock<'a>(
    lock: &'a Mutex<HashSet<PathBuf>>,
) -> MutexGuard<'a, HashSet<PathBuf>> {
    // A panic while synchronizing editor source overrides should not leave the
    // compiler frontend permanently unusable. The protected set is only a
    // best-effort index for override existence checks; recovering the poisoned
    // value is safer than panicking on every later request.
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug, Clone)]
struct CachedParsedModule {
    ast: ast::Module,
    symbols: Vec<Arc<str>>,
    node_count: u32,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct CachedParseFailure {
    diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
struct CachedParseRecord {
    source: Arc<str>,
    outcome: CachedParseOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FrontendPruneProfile {
    target_triple: String,
    custom_defines: Vec<(String, String)>,
}

impl FrontendPruneProfile {
    fn from_session(session: &Session) -> Self {
        let mut custom_defines = session
            .custom_defines
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();
        custom_defines.sort();
        Self {
            target_triple: session.target.triple.to_string(),
            custom_defines,
        }
    }

    fn apply(&self, session: &mut Session) {
        session.target =
            kernc_utils::config::TargetMachine::new(&self.target_triple).unwrap_or_default();
        session.custom_defines = self.custom_defines.iter().cloned().collect();
    }
}

#[derive(Debug, Clone)]
enum CachedParseOutcome {
    Parsed(CachedParsedModule),
    Failed(CachedParseFailure),
}

#[derive(Clone)]
pub struct FrontendDatabase {
    db: Database,
    source_overrides: Input<PathBuf, Arc<str>>,
    source_texts: Query<PathBuf, Option<Arc<str>>>,
    parsed_modules: Memo<(PathBuf, bool), Option<Arc<CachedParseRecord>>>,
    pruned_modules: Memo<(PathBuf, bool, FrontendPruneProfile), Option<Arc<CachedParseRecord>>>,
    known_override_paths: Arc<Mutex<HashSet<PathBuf>>>,
    uncached_parse_count: Arc<AtomicUsize>,
    uncached_prune_count: Arc<AtomicUsize>,
}

impl FrontendDatabase {
    pub fn new() -> Self {
        let source_overrides = Input::new("frontend_source_override");
        let source_texts = Query::new("frontend_source_text", {
            let source_overrides = source_overrides.clone();
            move |db, path: &PathBuf| {
                if let Some(text) = source_overrides.get(db, path.clone())? {
                    return Ok(Some(text));
                }

                match std::fs::read_to_string(path) {
                    Ok(text) => Ok(Some(Arc::<str>::from(text))),
                    Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
                    Err(_) => Ok(None),
                }
            }
        });

        Self {
            db: Database::new(),
            source_overrides,
            source_texts,
            parsed_modules: Memo::new(),
            pruned_modules: Memo::new(),
            known_override_paths: Arc::new(Mutex::new(HashSet::new())),
            uncached_parse_count: Arc::new(AtomicUsize::new(0)),
            uncached_prune_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    #[cfg(test)]
    pub fn set_source_override(&self, path: PathBuf, text: String) {
        let _ = self
            .source_overrides
            .set(&self.db, path, Arc::<str>::from(text));
    }

    pub fn sync_source_overrides(&self, overrides: &crate::compiler::SourceOverrides) {
        let normalized = overrides
            .iter()
            .map(|(path, text)| (normalize_path(path), Arc::<str>::from(text.as_str())))
            .collect::<HashMap<_, _>>();

        let mut known = recover_known_override_paths_lock(&self.known_override_paths);
        let stale = known
            .iter()
            .filter(|path| !normalized.contains_key(*path))
            .cloned()
            .collect::<Vec<_>>();

        for path in stale {
            let _ = self.source_overrides.clear(&self.db, path.clone());
            known.remove(&path);
        }

        for (path, text) in normalized {
            let _ = self.source_overrides.set(&self.db, path.clone(), text);
            known.insert(path);
        }
    }

    pub fn source_exists(&self, path: &Path) -> bool {
        if std::fs::metadata(path).is_ok() {
            return true;
        }

        let known = recover_known_override_paths_lock(&self.known_override_paths);
        if known.contains(path) {
            return true;
        }

        let normalized = normalize_path(path);
        known.contains(&normalized)
    }

    #[cfg(test)]
    pub fn load_parsed_module(
        &self,
        session: &mut Session,
        path: &Path,
    ) -> Result<Option<FrontendParsedModule>, kernc_db::Cycle> {
        let normalized = normalize_path(path);
        self.load_parsed_module_normalized_profiled(session, &normalized, true)
            .map(|parsed| parsed.map(|(parsed, _)| parsed))
    }

    #[cfg(test)]
    pub(crate) fn load_parsed_module_normalized_profiled(
        &self,
        session: &mut Session,
        normalized: &Path,
        collect_docs: bool,
    ) -> Result<Option<(FrontendParsedModule, FrontendLoadTimings)>, kernc_db::Cycle> {
        expect_uncancelable(
            self.load_parsed_module_normalized_profiled_cancelable(
                session,
                normalized,
                collect_docs,
                &CancellationToken::new(),
            ),
            "loading parsed frontend module",
        )
    }

    pub(crate) fn load_parsed_module_normalized_profiled_cancelable(
        &self,
        session: &mut Session,
        normalized: &Path,
        collect_docs: bool,
        cancellation: &CancellationToken,
    ) -> Result<
        Result<Option<(FrontendParsedModule, FrontendLoadTimings)>, kernc_db::Cycle>,
        Canceled,
    > {
        cancellation.check()?;
        let mut timings = FrontendLoadTimings::default();
        let key = (normalized.to_path_buf(), collect_docs);
        let mut computed_parse_timings = FrontendLoadTimings::default();
        let raw_record =
            self.parsed_modules
                .try_get_with(&self.db, "frontend_parsed_module", key, || {
                    cancellation.check()?;
                    let read_started = Instant::now();
                    let source = match self.source_texts.get(&self.db, normalized.to_path_buf()) {
                        Ok(Some(source)) => source,
                        Ok(None) => return Ok(Ok(None)),
                        Err(cycle) => return Ok(Err(cycle)),
                    };
                    computed_parse_timings.read_source = read_started.elapsed();
                    let (outcome, parse_timings) = self.compute_cached_parse_outcome_cancelable(
                        &source,
                        collect_docs,
                        cancellation,
                    )?;
                    computed_parse_timings.add(parse_timings);
                    Ok(Ok(Some(Arc::new(CachedParseRecord { source, outcome }))))
                })?;
        let raw_record = match raw_record {
            Ok(raw_record) => raw_record,
            Err(cycle) => return Ok(Err(cycle)),
        };
        let Some(raw_record) = raw_record else {
            return Ok(Ok(None));
        };
        timings.add(computed_parse_timings);
        cancellation.check()?;

        let mut computed_prune_timings = FrontendLoadTimings::default();
        let record = if source_may_need_conditional_pruning(raw_record.source.as_ref()) {
            let prune_profile = FrontendPruneProfile::from_session(session);
            let raw_record_for_prune = raw_record.clone();
            let pruned_record = match self.pruned_modules.get_with(
                &self.db,
                "frontend_pruned_module",
                (
                    normalized.to_path_buf(),
                    collect_docs,
                    prune_profile.clone(),
                ),
                || {
                    let prune_started = Instant::now();
                    let record = self
                        .compute_pruned_parse_record(raw_record_for_prune.as_ref(), &prune_profile);
                    computed_prune_timings.prune = prune_started.elapsed();
                    Ok(record.map(Arc::new))
                },
            ) {
                Ok(Some(pruned_record)) => pruned_record,
                Ok(None) => return Ok(Ok(None)),
                Err(cycle) => return Ok(Err(cycle)),
            };
            timings.add(computed_prune_timings);
            pruned_record
        } else {
            raw_record
        };

        let ensure_started = Instant::now();
        cancellation.check()?;
        let file_id = self.ensure_file_id(session, normalized, &record.source);
        timings.ensure_file_id += ensure_started.elapsed();

        let rebind_started = Instant::now();
        cancellation.check()?;
        let parsed = match &record.outcome {
            CachedParseOutcome::Parsed(cached) => {
                self.replay_diagnostics(session, file_id, &cached.diagnostics);
                Some(self.bind_cached_module(session, normalized, file_id, cached))
            }
            CachedParseOutcome::Failed(failure) => {
                self.replay_diagnostics(session, file_id, &failure.diagnostics);
                None
            }
        };
        timings.rebind += rebind_started.elapsed();

        cancellation.check()?;
        Ok(Ok(parsed.map(|parsed| (parsed, timings))))
    }

    pub fn uncached_parse_count(&self) -> usize {
        self.uncached_parse_count.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub fn uncached_prune_count(&self) -> usize {
        self.uncached_prune_count.load(Ordering::SeqCst)
    }

    fn ensure_file_id(&self, session: &mut Session, path: &Path, source: &Arc<str>) -> FileId {
        if let Some(file_id) = session.source_manager.find_file_id_by_path(path) {
            let needs_update = session
                .source_manager
                .get_file(file_id)
                .map(|file| file.src.as_ref() != source.as_ref())
                .unwrap_or(true);
            if needs_update {
                session.source_manager.update_file(file_id, source.clone());
            }
            return file_id;
        }

        session
            .source_manager
            .add_file(path.to_string_lossy().to_string(), source.clone())
    }

    fn compute_cached_parse_outcome_cancelable(
        &self,
        source: &Arc<str>,
        collect_docs: bool,
        cancellation: &CancellationToken,
    ) -> Result<(CachedParseOutcome, FrontendLoadTimings), Canceled> {
        self.uncached_parse_count.fetch_add(1, Ordering::SeqCst);

        let mut parse_session = Session::new();
        let (parsed, timings) = self.parse_frontend_module_profiled_cancelable(
            &mut parse_session,
            FileId(0),
            source,
            collect_docs,
            cancellation,
        )?;
        let diagnostics = parse_session.diagnostics.clone();
        let outcome = match parsed {
            Some(ast) => CachedParseOutcome::Parsed(CachedParsedModule {
                ast,
                symbols: parse_session.interner.snapshot_symbols(),
                node_count: parse_session.next_node_id,
                diagnostics,
            }),
            None => CachedParseOutcome::Failed(CachedParseFailure { diagnostics }),
        };
        Ok((outcome, timings))
    }

    fn compute_pruned_parse_record(
        &self,
        raw_record: &CachedParseRecord,
        prune_profile: &FrontendPruneProfile,
    ) -> Option<CachedParseRecord> {
        self.uncached_prune_count.fetch_add(1, Ordering::SeqCst);

        match &raw_record.outcome {
            CachedParseOutcome::Parsed(cached) => {
                let mut prune_session = Session::new();
                prune_profile.apply(&mut prune_session);
                let _ = prune_session.interner.intern_snapshot(&cached.symbols);

                let mut ast = cached.ast.clone();
                if source_may_need_conditional_pruning(raw_record.source.as_ref()) {
                    let mut pruner = Pruner::new(&mut prune_session);
                    pruner.prune_module(&mut ast);
                }

                let mut diagnostics = cached.diagnostics.clone();
                diagnostics.extend(prune_session.diagnostics.iter().cloned());

                Some(CachedParseRecord {
                    source: raw_record.source.clone(),
                    outcome: CachedParseOutcome::Parsed(CachedParsedModule {
                        ast,
                        symbols: cached.symbols.clone(),
                        node_count: cached.node_count,
                        diagnostics,
                    }),
                })
            }
            CachedParseOutcome::Failed(failure) => Some(CachedParseRecord {
                source: raw_record.source.clone(),
                outcome: CachedParseOutcome::Failed(failure.clone()),
            }),
        }
    }

    fn parse_frontend_module_profiled_cancelable(
        &self,
        session: &mut Session,
        file_id: FileId,
        source: &Arc<str>,
        collect_docs: bool,
        cancellation: &CancellationToken,
    ) -> Result<(Option<ast::Module>, FrontendLoadTimings), Canceled> {
        let mut timings = FrontendLoadTimings::default();

        let parse_started = Instant::now();
        let mut parser = if collect_docs {
            Parser::new_cancelable(source.as_ref(), file_id, session, cancellation)
        } else {
            Parser::new_without_docs_cancelable(source.as_ref(), file_id, session, cancellation)
        };
        let ast = match parser.parse_module_cancelable()? {
            Ok(ast) => ast,
            Err(_) => return Ok((None, timings)),
        };
        timings.parse = parse_started.elapsed();

        Ok((Some(ast), timings))
    }

    fn bind_cached_module(
        &self,
        session: &mut Session,
        normalized: &Path,
        file_id: FileId,
        cached: &CachedParsedModule,
    ) -> FrontendParsedModule {
        let mut ast = cached.ast.clone();
        ast.path = normalized.to_string_lossy().to_string();
        let symbol_map = session.interner.intern_snapshot(&cached.symbols);
        let node_base = session.reserve_node_ids(cached.node_count);
        let mut rebinder = CachedAstRebinder {
            file_id,
            node_base,
            symbol_map: &symbol_map,
        };
        rebinder.rebind_module(&mut ast);
        FrontendParsedModule { file_id, ast }
    }

    fn replay_diagnostics(
        &self,
        session: &mut Session,
        file_id: FileId,
        diagnostics: &[Diagnostic],
    ) {
        for diagnostic in diagnostics {
            session
                .diagnostics
                .push(rebind_diagnostic(diagnostic, file_id));
            if matches!(
                diagnostic.level,
                DiagnosticLevel::Error | DiagnosticLevel::Ice
            ) {
                session.error_count += 1;
            }
        }
    }
}

impl Default for FrontendDatabase {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    normalize_platform_path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

fn source_may_need_conditional_pruning(source: &str) -> bool {
    source.contains("#[if") || source.contains("#![if")
}

struct CachedAstRebinder<'a> {
    file_id: FileId,
    node_base: NodeId,
    symbol_map: &'a [SymbolId],
}

impl CachedAstRebinder<'_> {
    fn rebind_module(&mut self, module: &mut ast::Module) {
        self.rebind_doc_block(module.docs.as_mut());
        for attribute in &mut module.attributes {
            self.rebind_attribute(attribute);
        }
        for decl in &mut module.decls {
            self.rebind_decl(decl);
        }
    }

    fn rebind_decl(&mut self, decl: &mut ast::Decl) {
        decl.id = self.rebind_node_id(decl.id);
        self.rebind_span(&mut decl.span);
        self.rebind_span(&mut decl.name_span);
        decl.name = self.rebind_symbol(decl.name);
        self.rebind_doc_block(decl.docs.as_mut());
        for attribute in &mut decl.attributes {
            self.rebind_attribute(attribute);
        }

        match &mut decl.kind {
            ast::DeclKind::Function {
                generics,
                where_clauses,
                params,
                ret_type,
                body,
                ..
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                for param in params {
                    self.rebind_func_param(param);
                }
                self.rebind_type_node(ret_type);
                if let Some(body) = body {
                    self.rebind_expr(body);
                }
            }
            ast::DeclKind::Var {
                type_node, value, ..
            } => {
                if let Some(type_node) = type_node {
                    self.rebind_type_node(type_node);
                }
                if let Some(value) = value {
                    self.rebind_expr(value);
                }
            }
            ast::DeclKind::TypeAlias {
                generics,
                where_clauses,
                target,
                ..
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                self.rebind_type_node(target);
            }
            ast::DeclKind::Struct {
                generics,
                where_clauses,
                fields,
                ..
            }
            | ast::DeclKind::Union {
                generics,
                where_clauses,
                fields,
                ..
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                for field in fields {
                    self.rebind_struct_field_def(field);
                }
            }
            ast::DeclKind::Enum {
                generics,
                where_clauses,
                backing_type,
                variants,
                ..
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                if let Some(backing_type) = backing_type {
                    self.rebind_type_node(backing_type);
                }
                for variant in variants {
                    self.rebind_enum_variant(variant);
                }
            }
            ast::DeclKind::Trait {
                generics,
                where_clauses,
                supertraits,
                assoc_types,
                methods,
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                for supertrait in supertraits {
                    self.rebind_type_node(supertrait);
                }
                for assoc in assoc_types {
                    assoc.name = self.rebind_symbol(assoc.name);
                    self.rebind_span(&mut assoc.name_span);
                    self.rebind_doc_block(assoc.docs.as_mut());
                    for generic in &mut assoc.generics {
                        self.rebind_generic_param(generic);
                    }
                    for bound in &mut assoc.bounds {
                        self.rebind_type_node(bound);
                    }
                    for clause in &mut assoc.where_clauses {
                        self.rebind_where_clause(clause);
                    }
                    self.rebind_span(&mut assoc.span);
                }
                for method in methods {
                    self.rebind_trait_method_def(method);
                }
            }
            ast::DeclKind::Mod { decls } => {
                if let Some(decls) = decls {
                    for decl in decls {
                        self.rebind_decl(decl);
                    }
                }
            }
            ast::DeclKind::Use { path, target, .. } => {
                self.rebind_symbols(path);
                self.rebind_use_target(target);
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                for decl in decls {
                    self.rebind_decl(decl);
                }
            }
            ast::DeclKind::Impl {
                generics,
                where_clauses,
                target_type,
                trait_type,
                decls,
            } => {
                for generic in generics {
                    self.rebind_generic_param(generic);
                }
                for clause in where_clauses {
                    self.rebind_where_clause(clause);
                }
                self.rebind_type_node(target_type);
                if let Some(trait_type) = trait_type {
                    self.rebind_type_node(trait_type);
                }
                for decl in decls {
                    self.rebind_decl(decl);
                }
            }
        }
    }

    fn rebind_where_clause(&mut self, clause: &mut ast::WhereClause) {
        self.rebind_span(&mut clause.span);
        self.rebind_type_node(&mut clause.target_ty);
        for bound in &mut clause.bounds {
            self.rebind_type_node(bound);
        }
    }

    fn rebind_generic_param(&mut self, generic: &mut ast::GenericParam) {
        generic.name = self.rebind_symbol(generic.name);
        self.rebind_span(&mut generic.span);
        if let ast::GenericParamKind::Const { ty } = &mut generic.kind {
            self.rebind_type_node(ty);
        }
    }

    fn rebind_func_param(&mut self, param: &mut ast::FuncParam) {
        self.rebind_binding_pattern(&mut param.pattern);
        self.rebind_type_node(&mut param.type_node);
        self.rebind_span(&mut param.span);
    }

    fn rebind_use_target(&mut self, target: &mut ast::UseTarget) {
        match target {
            ast::UseTarget::Module(alias) => {
                if let Some(alias) = alias {
                    *alias = self.rebind_symbol(*alias);
                }
            }
            ast::UseTarget::Tree(items) => {
                for item in items {
                    self.rebind_use_tree(item);
                }
            }
        }
    }

    fn rebind_use_tree(&mut self, tree: &mut ast::UseTree) {
        match tree {
            ast::UseTree::SelfModule {
                alias,
                span,
                binding_span,
            } => {
                if let Some(alias) = alias {
                    *alias = self.rebind_symbol(*alias);
                }
                self.rebind_span(span);
                self.rebind_span(binding_span);
            }
            ast::UseTree::Path {
                path,
                alias,
                nested,
                span,
                binding_span,
            } => {
                self.rebind_symbols(path);
                if let Some(alias) = alias {
                    *alias = self.rebind_symbol(*alias);
                }
                if let Some(nested) = nested {
                    for item in nested {
                        self.rebind_use_tree(item);
                    }
                }
                self.rebind_span(span);
                self.rebind_span(binding_span);
            }
        }
    }

    fn rebind_expr(&mut self, expr: &mut ast::Expr) {
        expr.id = self.rebind_node_id(expr.id);
        self.rebind_span(&mut expr.span);

        match &mut expr.kind {
            ast::ExprKind::Let {
                pattern,
                type_node,
                init,
                else_clause,
                ..
            } => {
                self.rebind_let_pattern(pattern);
                if let Some(type_node) = type_node {
                    self.rebind_type_node(type_node);
                }
                self.rebind_expr(init);
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(branch) => self.rebind_expr(branch),
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                self.rebind_pattern(&mut arm.pattern);
                                self.rebind_expr(&mut arm.body);
                                self.rebind_span(&mut arm.span);
                            }
                        }
                    }
                }
            }
            ast::ExprKind::Static {
                pattern,
                type_node,
                init,
                ..
            } => {
                self.rebind_binding_pattern(pattern);
                if let Some(type_node) = type_node {
                    self.rebind_type_node(type_node);
                }
                if let Some(init) = init {
                    self.rebind_expr(init);
                }
            }
            ast::ExprKind::Error
            | ast::ExprKind::Integer { .. }
            | ast::ExprKind::Float { .. }
            | ast::ExprKind::Bool(..)
            | ast::ExprKind::Char(..)
            | ast::ExprKind::ByteChar(..)
            | ast::ExprKind::String(..)
            | ast::ExprKind::Break
            | ast::ExprKind::Continue
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer
            | ast::ExprKind::SelfValue => {}
            ast::ExprKind::Identifier(symbol) => *symbol = self.rebind_symbol(*symbol),
            ast::ExprKind::AnchoredPath {
                name, name_span, ..
            } => {
                *name = self.rebind_symbol(*name);
                self.rebind_span(name_span);
            }
            ast::ExprKind::TypeNode(type_node) => self.rebind_type_node(type_node),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.rebind_expr(lhs);
                self.rebind_expr(rhs);
            }
            ast::ExprKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.rebind_expr(start);
                }
                if let Some(end) = end {
                    self.rebind_expr(end);
                }
            }
            ast::ExprKind::Unary { operand, .. } => self.rebind_expr(operand),
            ast::ExprKind::Grouped { expr: inner } => self.rebind_expr(inner),
            ast::ExprKind::FieldAccess {
                lhs,
                field,
                field_span,
            } => {
                self.rebind_expr(lhs);
                *field = self.rebind_symbol(*field);
                self.rebind_span(field_span);
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.rebind_expr(lhs);
                self.rebind_expr(index);
            }
            ast::ExprKind::Call { callee, args } => {
                self.rebind_expr(callee);
                for arg in args {
                    self.rebind_expr(arg);
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                if let Some(type_node) = type_node {
                    self.rebind_type_node(type_node);
                }
                self.rebind_data_literal(literal);
            }
            ast::ExprKind::EnumLiteral {
                variant,
                variant_span,
            } => {
                *variant = self.rebind_symbol(*variant);
                self.rebind_span(variant_span);
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.rebind_expr(cond);
                self.rebind_expr(then_branch);
                if let Some(branch) = else_branch {
                    self.rebind_expr(branch);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.rebind_expr(target);
                for arm in arms {
                    self.rebind_match_arm(arm);
                }
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    self.rebind_stmt(stmt);
                }
                if let Some(result) = result {
                    self.rebind_expr(result);
                }
            }
            ast::ExprKind::While { cond, body } => {
                self.rebind_expr(cond);
                self.rebind_expr(body);
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.rebind_expr(lhs);
                if let Some(start) = start {
                    self.rebind_expr(start);
                }
                if let Some(end) = end {
                    self.rebind_expr(end);
                }
            }
            ast::ExprKind::Defer { expr } => self.rebind_expr(expr),
            ast::ExprKind::Return(expr) => {
                if let Some(expr) = expr {
                    self.rebind_expr(expr);
                }
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.rebind_expr(lhs);
                self.rebind_expr(rhs);
            }
            ast::ExprKind::As { lhs, target } => {
                self.rebind_expr(lhs);
                self.rebind_type_node(target);
            }
            ast::ExprKind::Propagate { operand, .. } => self.rebind_expr(operand),
            ast::ExprKind::GenericInstantiation { target, args } => {
                self.rebind_expr(target);
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty) => self.rebind_type_node(ty),
                        ast::GenericArg::ConstExpr(expr) => self.rebind_expr(expr),
                        ast::GenericArg::AssocBinding {
                            name,
                            name_span,
                            value,
                        } => {
                            *name = self.rebind_symbol(*name);
                            self.rebind_span(name_span);
                            self.rebind_type_node(value);
                        }
                    }
                }
            }
            ast::ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                for capture in captures {
                    self.rebind_capture_pattern(capture);
                }
                for param in params {
                    self.rebind_func_param(param);
                }
                self.rebind_type_node(ret_type);
                self.rebind_expr(body);
            }
        }
    }

    fn rebind_data_literal(&mut self, literal: &mut ast::DataLiteralKind) {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    self.rebind_struct_field_init(field);
                }
            }
            ast::DataLiteralKind::Array(exprs) => {
                for expr in exprs {
                    self.rebind_expr(expr);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.rebind_expr(value);
                self.rebind_expr(count);
            }
            ast::DataLiteralKind::Scalar(expr) => self.rebind_expr(expr),
        }
    }

    fn rebind_struct_field_init(&mut self, field: &mut ast::StructFieldInit) {
        field.name = self.rebind_symbol(field.name);
        self.rebind_span(&mut field.name_span);
        self.rebind_expr(&mut field.value);
        self.rebind_span(&mut field.span);
    }

    fn rebind_capture_pattern(&mut self, capture: &mut ast::CapturePattern) {
        capture.name = self.rebind_symbol(capture.name);
        self.rebind_span(&mut capture.name_span);
        self.rebind_expr(&mut capture.value);
        self.rebind_span(&mut capture.span);
    }

    fn rebind_match_arm(&mut self, arm: &mut ast::MatchArm) {
        for pattern in &mut arm.patterns {
            self.rebind_match_pattern(pattern);
        }
        self.rebind_expr(&mut arm.body);
        self.rebind_span(&mut arm.span);
    }

    fn rebind_stmt(&mut self, stmt: &mut ast::Stmt) {
        stmt.id = self.rebind_node_id(stmt.id);
        self.rebind_span(&mut stmt.span);
        for attribute in &mut stmt.attributes {
            self.rebind_attribute(attribute);
        }
        match &mut stmt.kind {
            ast::StmtKind::Use(use_stmt) => {
                self.rebind_symbols(&mut use_stmt.path);
                self.rebind_use_target(&mut use_stmt.target);
                self.rebind_span(&mut use_stmt.binding_span);
            }
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.rebind_expr(expr)
            }
        }
    }

    fn rebind_type_node(&mut self, ty: &mut ast::TypeNode) {
        ty.id = self.rebind_node_id(ty.id);
        self.rebind_span(&mut ty.span);

        match &mut ty.kind {
            ast::TypeKind::Path { segments, .. } => {
                for segment in segments {
                    segment.name = self.rebind_symbol(segment.name);
                    self.rebind_span(&mut segment.name_span);
                    for arg in &mut segment.args {
                        match arg {
                            ast::GenericArg::Type(generic) => self.rebind_type_node(generic),
                            ast::GenericArg::ConstExpr(expr) => self.rebind_expr(expr),
                            ast::GenericArg::AssocBinding {
                                name,
                                name_span,
                                value,
                            } => {
                                *name = self.rebind_symbol(*name);
                                self.rebind_span(name_span);
                                self.rebind_type_node(value);
                            }
                        }
                    }
                }
            }
            ast::TypeKind::Optional { inner } => self.rebind_type_node(inner),
            ast::TypeKind::Result { ok, err } => {
                self.rebind_type_node(ok);
                self.rebind_type_node(err);
            }
            ast::TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    self.rebind_type_node(start);
                }
                if let Some(end) = end {
                    self.rebind_type_node(end);
                }
            }
            ast::TypeKind::Pointer { elem, .. }
            | ast::TypeKind::VolatilePtr { elem, .. }
            | ast::TypeKind::ArrayInfer { elem, .. }
            | ast::TypeKind::Slice { elem, .. } => self.rebind_type_node(elem),
            ast::TypeKind::Array { elem, len, .. } => {
                self.rebind_type_node(elem);
                self.rebind_expr(len);
            }
            ast::TypeKind::Function { params, ret, .. }
            | ast::TypeKind::ClosureInterface { params, ret } => {
                for param in params {
                    self.rebind_type_node(param);
                }
                if let Some(ret) = ret {
                    self.rebind_type_node(ret);
                }
            }
            ast::TypeKind::Struct { fields, .. } | ast::TypeKind::Union { fields, .. } => {
                for field in fields {
                    self.rebind_struct_field_def(field);
                }
            }
            ast::TypeKind::Trait {
                assoc_types,
                methods,
            } => {
                for assoc in assoc_types {
                    assoc.name = self.rebind_symbol(assoc.name);
                    self.rebind_span(&mut assoc.name_span);
                    self.rebind_doc_block(assoc.docs.as_mut());
                    for bound in &mut assoc.bounds {
                        self.rebind_type_node(bound);
                    }
                    for clause in &mut assoc.where_clauses {
                        self.rebind_type_node(&mut clause.target_ty);
                        for bound in &mut clause.bounds {
                            self.rebind_type_node(bound);
                        }
                        self.rebind_span(&mut clause.span);
                    }
                    self.rebind_span(&mut assoc.span);
                }
                for method in methods {
                    self.rebind_trait_method_def(method);
                }
            }
            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                if let Some(backing_type) = backing_type {
                    self.rebind_type_node(backing_type);
                }
                for variant in variants {
                    self.rebind_enum_variant(variant);
                }
            }
            ast::TypeKind::Error
            | ast::TypeKind::Infer
            | ast::TypeKind::SelfType
            | ast::TypeKind::Never
            | ast::TypeKind::Void => {}
            ast::TypeKind::TypeOf(expr) => self.rebind_expr(expr),
        }
    }

    fn rebind_struct_field_def(&mut self, field: &mut ast::StructFieldDef) {
        field.name = self.rebind_symbol(field.name);
        self.rebind_span(&mut field.name_span);
        self.rebind_doc_block(field.docs.as_mut());
        self.rebind_type_node(&mut field.type_node);
        if let Some(default_value) = &mut field.default_value {
            self.rebind_expr(default_value);
        }
        self.rebind_span(&mut field.span);
    }

    fn rebind_trait_method_def(&mut self, method: &mut ast::TraitMethodDef) {
        self.rebind_struct_field_def(&mut method.signature);
        for param in &mut method.params {
            self.rebind_func_param(param);
        }
        if let Some(body) = &mut method.body {
            self.rebind_expr(body);
        }
        self.rebind_span(&mut method.span);
    }

    fn rebind_enum_variant(&mut self, variant: &mut ast::EnumVariant) {
        variant.name = self.rebind_symbol(variant.name);
        self.rebind_span(&mut variant.name_span);
        self.rebind_doc_block(variant.docs.as_mut());
        if let Some(payload_type) = &mut variant.payload_type {
            self.rebind_type_node(payload_type);
        }
        if let Some(value) = &mut variant.value {
            self.rebind_expr(value);
        }
        self.rebind_span(&mut variant.span);
    }

    fn rebind_attribute(&mut self, attribute: &mut ast::Attribute) {
        self.rebind_span(&mut attribute.span);
        match &mut attribute.kind {
            ast::AttributeKind::If(expr) => self.rebind_expr(expr),
            ast::AttributeKind::Meta(items) => {
                for item in items {
                    self.rebind_meta_item(item);
                }
            }
        }
    }

    fn rebind_meta_item(&mut self, item: &mut ast::MetaItem) {
        match item {
            ast::MetaItem::Marker(symbol) => *symbol = self.rebind_symbol(*symbol),
            ast::MetaItem::Call(symbol, expr) => {
                *symbol = self.rebind_symbol(*symbol);
                self.rebind_expr(expr);
            }
        }
    }

    fn rebind_binding_pattern(&mut self, pattern: &mut ast::BindingPattern) {
        pattern.name = self.rebind_symbol(pattern.name);
        self.rebind_span(&mut pattern.name_span);
        self.rebind_span(&mut pattern.span);
    }

    fn rebind_pattern(&mut self, pattern: &mut ast::Pattern) {
        self.rebind_span(&mut pattern.span);
        match &mut pattern.kind {
            ast::PatternKind::Binding(binding) => self.rebind_binding_pattern(binding),
            ast::PatternKind::Ignore => {}
            ast::PatternKind::Variant(variant) => {
                if let Some(target_type) = &mut variant.target_type {
                    self.rebind_type_node(target_type);
                }
                variant.variant_name = self.rebind_symbol(variant.variant_name);
                self.rebind_span(&mut variant.variant_span);
            }
            ast::PatternKind::Destructure(destructure) => {
                if let Some(target_type) = &mut destructure.target_type {
                    self.rebind_type_node(target_type);
                }
                for field in &mut destructure.fields {
                    field.name = self.rebind_symbol(field.name);
                    self.rebind_span(&mut field.name_span);
                    self.rebind_pattern(&mut field.pattern);
                    self.rebind_span(&mut field.span);
                }
            }
        }
    }

    fn rebind_let_pattern(&mut self, pattern: &mut ast::LetPattern) {
        self.rebind_pattern(&mut pattern.pattern);
        self.rebind_span(&mut pattern.span);
    }

    fn rebind_match_pattern(&mut self, pattern: &mut ast::MatchPattern) {
        self.rebind_span(&mut pattern.span);
        match &mut pattern.kind {
            ast::MatchPatternKind::Value(expr) => self.rebind_expr(expr),
            ast::MatchPatternKind::Pattern(pattern) => self.rebind_pattern(pattern),
        }
    }

    fn rebind_doc_block(&mut self, doc: Option<&mut ast::DocBlock>) {
        let Some(doc) = doc else {
            return;
        };
        self.rebind_span(&mut doc.span);
        for line in &mut doc.lines {
            self.rebind_span(&mut line.span);
        }
    }

    fn rebind_symbols(&self, symbols: &mut [SymbolId]) {
        for symbol in symbols {
            *symbol = self.rebind_symbol(*symbol);
        }
    }

    fn rebind_node_id(&self, id: NodeId) -> NodeId {
        NodeId(self.node_base.0 + id.0)
    }

    fn rebind_symbol(&self, symbol: SymbolId) -> SymbolId {
        self.symbol_map
            .get(symbol.0)
            .copied()
            .expect("cached symbol id must exist in cached symbol table")
    }

    fn rebind_span(&self, span: &mut Span) {
        span.file = self.file_id;
    }
}

fn rebind_diagnostic(diagnostic: &Diagnostic, file_id: FileId) -> Diagnostic {
    let mut rebound = diagnostic.clone();
    rebound.primary_span.file = file_id;
    for (span, _) in &mut rebound.related_spans {
        span.file = file_id;
    }
    rebound
}

#[cfg(test)]
mod tests {
    use kernc_ast as ast;

    use super::FrontendDatabase;
    use kernc_utils::Session;

    #[test]
    fn source_override_reuses_file_id_when_reparsed() {
        let db = FrontendDatabase::new();
        let mut session = Session::new();
        let path = std::env::temp_dir().join(format!(
            "kern_frontend_db_{}_override.kn",
            std::process::id()
        ));

        db.set_source_override(path.clone(), "fn main() i32 { return 1; }".to_string());
        let first = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");

        db.set_source_override(path.clone(), "fn main() i32 { return 2; }".to_string());
        let second = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(first.file_id, second.file_id);
        assert_eq!(
            session
                .source_manager
                .get_file(first.file_id)
                .expect("file should stay registered")
                .src
                .as_ref(),
            "fn main() i32 { return 2; }"
        );
    }

    #[test]
    fn parsed_module_memo_skips_reparse_when_source_is_stable() {
        let db = FrontendDatabase::new();
        let mut session = Session::new();
        let path =
            std::env::temp_dir().join(format!("kern_frontend_db_{}_stable.kn", std::process::id()));

        db.set_source_override(path.clone(), "fn main() i32 { return 1; }".to_string());

        let first = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");
        let parse_count_after_first_load = db.uncached_parse_count();

        let second = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(first.file_id, second.file_id);
        assert_eq!(db.uncached_parse_count(), parse_count_after_first_load);
    }

    #[test]
    fn cached_parse_rebinds_symbols_into_new_sessions() {
        let db = FrontendDatabase::new();
        let mut first_session = Session::new();
        let mut second_session = Session::new();
        let path =
            std::env::temp_dir().join(format!("kern_frontend_db_{}_rebind.kn", std::process::id()));

        db.set_source_override(
            path.clone(),
            "fn answer() i32 { return helper; }".to_string(),
        );

        let _ = db
            .load_parsed_module(&mut first_session, &path)
            .unwrap()
            .expect("module should parse");
        let parse_count_after_first_load = db.uncached_parse_count();

        let second = db
            .load_parsed_module(&mut second_session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(db.uncached_parse_count(), parse_count_after_first_load);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &second.ast.decls[0].kind
        else {
            panic!("expected cached function body");
        };
        let mut seen_helper = false;
        collect_identifier_symbols(body, &mut |symbol| {
            if second_session.resolve(symbol) == "helper" {
                seen_helper = true;
            }
        });
        assert!(
            seen_helper,
            "expected cached symbols to rebind into the new session"
        );
    }

    #[test]
    fn cached_parse_rebinds_const_generic_parameter_types() {
        let db = FrontendDatabase::new();
        let mut first_session = Session::new();
        let mut second_session = Session::new();
        let path = std::env::temp_dir().join(format!(
            "kern_frontend_db_{}_const_generic_rebind.kn",
            std::process::id()
        ));

        db.set_source_override(
            path.clone(),
            "enum Mode { Fast, Safe };\nstruct SettingDef[M: Mode] {};\ntype Setting[M: Mode] = SettingDef[M];\n".to_string(),
        );

        let _ = db
            .load_parsed_module(&mut first_session, &path)
            .unwrap()
            .expect("module should parse");
        let parse_count_after_first_load = db.uncached_parse_count();

        let second = db
            .load_parsed_module(&mut second_session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(db.uncached_parse_count(), parse_count_after_first_load);

        let ast::DeclKind::TypeAlias { generics, .. } = &second.ast.decls[2].kind else {
            panic!("expected cached type alias");
        };
        let ast::GenericParamKind::Const { ty } = &generics[0].kind else {
            panic!("expected const generic parameter");
        };
        let ast::TypeKind::Path { segments, .. } = &ty.kind else {
            panic!("expected path type for const generic parameter");
        };

        assert_eq!(second_session.resolve(segments[0].name), "Mode");
    }

    #[test]
    fn conditional_prune_memo_skips_recompute_for_stable_define_profile() {
        let db = FrontendDatabase::new();
        let mut first_session = Session::new();
        let mut second_session = Session::new();
        let path = std::env::temp_dir().join(format!(
            "kern_frontend_db_{}_conditional_prune_stable.kn",
            std::process::id()
        ));

        db.set_source_override(
            path.clone(),
            "#[if(feature)]\nfn hidden() i32 { return 7; }\nfn main() i32 { return 1; }\n"
                .to_string(),
        );
        first_session
            .custom_defines
            .insert("feature".to_string(), "false".to_string());
        second_session
            .custom_defines
            .insert("feature".to_string(), "false".to_string());

        let first = db
            .load_parsed_module(&mut first_session, &path)
            .unwrap()
            .expect("module should parse");
        let prune_count = db.uncached_prune_count();
        let second = db
            .load_parsed_module(&mut second_session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(first.ast.decls.len(), 1);
        assert_eq!(second.ast.decls.len(), 1);
        assert_eq!(db.uncached_prune_count(), prune_count);
    }

    #[test]
    fn conditional_prune_memo_invalidates_for_changed_define_profile() {
        let db = FrontendDatabase::new();
        let mut disabled_session = Session::new();
        let mut enabled_session = Session::new();
        let path = std::env::temp_dir().join(format!(
            "kern_frontend_db_{}_conditional_prune_changed.kn",
            std::process::id()
        ));

        db.set_source_override(
            path.clone(),
            "#[if(feature)]\nfn hidden() i32 { return 7; }\nfn main() i32 { return 1; }\n"
                .to_string(),
        );
        disabled_session
            .custom_defines
            .insert("feature".to_string(), "false".to_string());
        enabled_session
            .custom_defines
            .insert("feature".to_string(), "true".to_string());

        let disabled = db
            .load_parsed_module(&mut disabled_session, &path)
            .unwrap()
            .expect("module should parse");
        let prune_count_after_disabled = db.uncached_prune_count();
        let enabled = db
            .load_parsed_module(&mut enabled_session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(disabled.ast.decls.len(), 1);
        assert_eq!(enabled.ast.decls.len(), 2);
        assert!(db.uncached_prune_count() > prune_count_after_disabled);
    }

    fn collect_identifier_symbols(expr: &ast::Expr, visit: &mut impl FnMut(kernc_utils::SymbolId)) {
        match &expr.kind {
            ast::ExprKind::Identifier(symbol) => visit(*symbol),
            ast::ExprKind::AnchoredPath { name, .. } => visit(*name),
            ast::ExprKind::Let {
                type_node,
                init,
                else_clause,
                ..
            } => {
                if let Some(type_node) = type_node {
                    collect_type_identifier_symbols(type_node, visit);
                }
                collect_identifier_symbols(init, visit);
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(else_expr) => {
                            collect_identifier_symbols(else_expr, visit);
                        }
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                collect_identifier_symbols(&arm.body, visit);
                            }
                        }
                    }
                }
            }
            ast::ExprKind::Static {
                type_node, init, ..
            } => {
                if let Some(type_node) = type_node {
                    collect_type_identifier_symbols(type_node, visit);
                }
                if let Some(init) = init {
                    collect_identifier_symbols(init, visit);
                }
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                collect_identifier_symbols(lhs, visit);
                collect_identifier_symbols(rhs, visit);
            }
            ast::ExprKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    collect_identifier_symbols(start, visit);
                }
                if let Some(end) = end {
                    collect_identifier_symbols(end, visit);
                }
            }
            ast::ExprKind::Unary { operand, .. } | ast::ExprKind::Defer { expr: operand } => {
                collect_identifier_symbols(operand, visit)
            }
            ast::ExprKind::Grouped { expr: inner } => collect_identifier_symbols(inner, visit),
            ast::ExprKind::FieldAccess { lhs, .. } => collect_identifier_symbols(lhs, visit),
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                collect_identifier_symbols(lhs, visit);
                collect_identifier_symbols(index, visit);
            }
            ast::ExprKind::Call { callee, args } => {
                collect_identifier_symbols(callee, visit);
                for arg in args {
                    collect_identifier_symbols(arg, visit);
                }
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => {
                    for field in fields {
                        collect_identifier_symbols(&field.value, visit);
                    }
                }
                ast::DataLiteralKind::Array(values) => {
                    for value in values {
                        collect_identifier_symbols(value, visit);
                    }
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    collect_identifier_symbols(value, visit);
                    collect_identifier_symbols(count, visit);
                }
                ast::DataLiteralKind::Scalar(value) => collect_identifier_symbols(value, visit),
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                collect_identifier_symbols(cond, visit);
                collect_identifier_symbols(then_branch, visit);
                if let Some(else_branch) = else_branch {
                    collect_identifier_symbols(else_branch, visit);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                collect_identifier_symbols(target, visit);
                for arm in arms {
                    collect_identifier_symbols(&arm.body, visit);
                }
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::Use(use_stmt) => {
                            collect_identifier_symbols_from_use_target(
                                &use_stmt.path,
                                &use_stmt.target,
                                visit,
                            );
                        }
                        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                            collect_identifier_symbols(expr, visit);
                        }
                    }
                }
                if let Some(result) = result {
                    collect_identifier_symbols(result, visit);
                }
            }
            ast::ExprKind::While { cond, body } => {
                collect_identifier_symbols(cond, visit);
                collect_identifier_symbols(body, visit);
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                collect_identifier_symbols(lhs, visit);
                if let Some(start) = start {
                    collect_identifier_symbols(start, visit);
                }
                if let Some(end) = end {
                    collect_identifier_symbols(end, visit);
                }
            }
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    collect_identifier_symbols(value, visit);
                }
            }
            ast::ExprKind::As { lhs, .. }
            | ast::ExprKind::GenericInstantiation { target: lhs, .. } => {
                collect_identifier_symbols(lhs, visit);
            }
            ast::ExprKind::TypeNode(type_node) => collect_type_identifier_symbols(type_node, visit),
            ast::ExprKind::Propagate { operand, .. } => {
                collect_identifier_symbols(operand, visit);
            }
            ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    collect_identifier_symbols(&capture.value, visit);
                }
                collect_identifier_symbols(body, visit);
            }
            ast::ExprKind::Error
            | ast::ExprKind::EnumLiteral { .. }
            | ast::ExprKind::Integer { .. }
            | ast::ExprKind::Float { .. }
            | ast::ExprKind::Bool(..)
            | ast::ExprKind::Char(..)
            | ast::ExprKind::ByteChar(..)
            | ast::ExprKind::String(..)
            | ast::ExprKind::Break
            | ast::ExprKind::Continue
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer
            | ast::ExprKind::SelfValue => {}
        }
    }

    fn collect_type_identifier_symbols(
        ty: &ast::TypeNode,
        visit: &mut impl FnMut(kernc_utils::SymbolId),
    ) {
        match &ty.kind {
            ast::TypeKind::Path { segments, .. } => {
                for segment in segments {
                    visit(segment.name);
                    for arg in &segment.args {
                        match arg {
                            ast::GenericArg::Type(generic) => {
                                collect_type_identifier_symbols(generic, visit);
                            }
                            ast::GenericArg::ConstExpr(expr) => {
                                collect_identifier_symbols(expr, visit);
                            }
                            ast::GenericArg::AssocBinding { name, value, .. } => {
                                visit(*name);
                                collect_type_identifier_symbols(value, visit);
                            }
                        }
                    }
                }
            }
            ast::TypeKind::Optional { inner } => collect_type_identifier_symbols(inner, visit),
            ast::TypeKind::Result { ok, err } => {
                collect_type_identifier_symbols(ok, visit);
                collect_type_identifier_symbols(err, visit);
            }
            ast::TypeKind::Range { start, end, .. } => {
                if let Some(start) = start {
                    collect_type_identifier_symbols(start, visit);
                }
                if let Some(end) = end {
                    collect_type_identifier_symbols(end, visit);
                }
            }
            ast::TypeKind::Pointer { elem, .. }
            | ast::TypeKind::VolatilePtr { elem, .. }
            | ast::TypeKind::ArrayInfer { elem, .. }
            | ast::TypeKind::Slice { elem, .. } => collect_type_identifier_symbols(elem, visit),
            ast::TypeKind::Array { elem, .. } => {
                collect_type_identifier_symbols(elem, visit);
            }
            ast::TypeKind::Function { params, ret, .. }
            | ast::TypeKind::ClosureInterface { params, ret } => {
                for param in params {
                    collect_type_identifier_symbols(param, visit);
                }
                if let Some(ret) = ret {
                    collect_type_identifier_symbols(ret, visit);
                }
            }
            ast::TypeKind::Struct { fields, .. } | ast::TypeKind::Union { fields, .. } => {
                for field in fields {
                    collect_type_identifier_symbols(&field.type_node, visit);
                }
            }
            ast::TypeKind::Trait {
                assoc_types,
                methods,
            } => {
                for assoc in assoc_types {
                    for bound in &assoc.bounds {
                        collect_type_identifier_symbols(bound, visit);
                    }
                    for clause in &assoc.where_clauses {
                        collect_type_identifier_symbols(&clause.target_ty, visit);
                        for bound in &clause.bounds {
                            collect_type_identifier_symbols(bound, visit);
                        }
                    }
                }
                for method in methods {
                    collect_type_identifier_symbols(&method.signature.type_node, visit);
                }
            }
            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                if let Some(backing_type) = backing_type {
                    collect_type_identifier_symbols(backing_type, visit);
                }
                for variant in variants {
                    if let Some(payload_type) = &variant.payload_type {
                        collect_type_identifier_symbols(payload_type, visit);
                    }
                }
            }
            ast::TypeKind::Error
            | ast::TypeKind::TypeOf(_)
            | ast::TypeKind::Infer
            | ast::TypeKind::SelfType
            | ast::TypeKind::Never
            | ast::TypeKind::Void => {}
        }
    }

    fn collect_identifier_symbols_from_use_target(
        path: &[kernc_utils::SymbolId],
        target: &ast::UseTarget,
        visit: &mut impl FnMut(kernc_utils::SymbolId),
    ) {
        for symbol in path {
            visit(*symbol);
        }
        match target {
            ast::UseTarget::Module(alias) => {
                if let Some(alias) = alias {
                    visit(*alias);
                }
            }
            ast::UseTarget::Tree(items) => {
                for item in items {
                    collect_identifier_symbols_from_use_tree(item, visit);
                }
            }
        }
    }

    fn collect_identifier_symbols_from_use_tree(
        tree: &ast::UseTree,
        visit: &mut impl FnMut(kernc_utils::SymbolId),
    ) {
        match tree {
            ast::UseTree::SelfModule { alias, .. } => {
                if let Some(alias) = alias {
                    visit(*alias);
                }
            }
            ast::UseTree::Path {
                path,
                alias,
                nested,
                ..
            } => {
                for symbol in path {
                    visit(*symbol);
                }
                if let Some(alias) = alias {
                    visit(*alias);
                }
                if let Some(nested) = nested {
                    for item in nested {
                        collect_identifier_symbols_from_use_tree(item, visit);
                    }
                }
            }
        }
    }
}
