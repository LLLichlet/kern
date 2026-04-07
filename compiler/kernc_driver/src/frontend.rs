use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use kernc_ast as ast;
use kernc_db::{Database, Input, Memo, Query};
use kernc_parser::Parser;
use kernc_sema::passes::Pruner;
use kernc_utils::{FileId, Session};

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
}

impl FrontendLoadTimings {
    fn add(&mut self, other: Self) {
        self.read_source += other.read_source;
        self.ensure_file_id += other.ensure_file_id;
        self.parse += other.parse;
        self.prune += other.prune;
    }
}

pub struct FrontendDatabase {
    db: Database,
    source_overrides: Input<PathBuf, String>,
    source_texts: Query<PathBuf, Option<String>>,
    #[allow(dead_code)]
    parsed_modules: Memo<PathBuf, Option<FrontendParsedModule>>,
    known_override_paths: Mutex<HashSet<PathBuf>>,
    #[cfg(test)]
    uncached_parse_count: AtomicUsize,
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
                    Ok(text) => Ok(Some(text)),
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
            known_override_paths: Mutex::new(HashSet::new()),
            #[cfg(test)]
            uncached_parse_count: AtomicUsize::new(0),
        }
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    #[allow(dead_code)]
    pub fn set_source_override(&self, path: PathBuf, text: String) {
        let _ = self.source_overrides.set(&self.db, path, text);
    }

    pub fn sync_source_overrides(&self, overrides: &crate::compiler::SourceOverrides) {
        let normalized = overrides
            .iter()
            .map(|(path, text)| (normalize_path(path), text.clone()))
            .collect::<HashMap<_, _>>();

        let mut known = self.known_override_paths.lock().unwrap();
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

        let known = self.known_override_paths.lock().unwrap();
        if known.contains(path) {
            return true;
        }

        let normalized = normalize_path(path);
        known.contains(&normalized)
    }

    #[allow(dead_code)]
    pub fn load_parsed_module(
        &self,
        session: &mut Session,
        path: &Path,
    ) -> Result<Option<FrontendParsedModule>, kernc_db::Cycle> {
        let normalized = normalize_path(path);

        self.parsed_modules.get_with(
            &self.db,
            "frontend_parsed_module",
            normalized.clone(),
            || {
                let Some(source) = self.source_texts.get(&self.db, normalized.clone())? else {
                    return Ok(None);
                };
                Ok(self.parse_frontend_module(session, &normalized, &source))
            },
        )
    }

    #[allow(dead_code)]
    pub fn load_parsed_module_uncached(
        &self,
        session: &mut Session,
        path: &Path,
    ) -> Result<Option<FrontendParsedModule>, kernc_db::Cycle> {
        let normalized = normalize_path(path);
        self.load_parsed_module_uncached_normalized(session, &normalized)
    }

    pub fn load_parsed_module_uncached_normalized(
        &self,
        session: &mut Session,
        normalized: &Path,
    ) -> Result<Option<FrontendParsedModule>, kernc_db::Cycle> {
        Ok(self
            .load_parsed_module_uncached_normalized_profiled(session, normalized, true)?
            .map(|(parsed, _)| parsed))
    }

    pub(crate) fn load_parsed_module_uncached_normalized_profiled(
        &self,
        session: &mut Session,
        normalized: &Path,
        collect_docs: bool,
    ) -> Result<Option<(FrontendParsedModule, FrontendLoadTimings)>, kernc_db::Cycle> {
        #[cfg(test)]
        self.uncached_parse_count.fetch_add(1, Ordering::SeqCst);

        let mut timings = FrontendLoadTimings::default();
        let read_started = Instant::now();
        let Some(source) = self
            .source_texts
            .get(&self.db, normalized.to_path_buf())?
        else {
            return Ok(None);
        };
        timings.read_source = read_started.elapsed();

        let (parsed, parse_timings) =
            self.parse_frontend_module_profiled(session, normalized, &source, collect_docs);
        timings.add(parse_timings);
        Ok(parsed.map(|parsed| (parsed, timings)))
    }

    #[cfg(test)]
    pub fn uncached_parse_count(&self) -> usize {
        self.uncached_parse_count.load(Ordering::SeqCst)
    }

    fn ensure_file_id(&self, session: &mut Session, path: &Path, source: &str) -> FileId {
        if let Some(file_id) = session.source_manager.find_file_id_by_path(path) {
            session
                .source_manager
                .update_file(file_id, source.to_string());
            return file_id;
        }

        session
            .source_manager
            .add_file(path.to_string_lossy().to_string(), source.to_string())
    }

    fn parse_frontend_module(
        &self,
        session: &mut Session,
        normalized: &Path,
        source: &str,
    ) -> Option<FrontendParsedModule> {
        self.parse_frontend_module_profiled(session, normalized, source, true)
            .0
    }

    fn parse_frontend_module_profiled(
        &self,
        session: &mut Session,
        normalized: &Path,
        source: &str,
        collect_docs: bool,
    ) -> (Option<FrontendParsedModule>, FrontendLoadTimings) {
        let mut timings = FrontendLoadTimings::default();

        let ensure_file_started = Instant::now();
        let file_id = self.ensure_file_id(session, normalized, source);
        timings.ensure_file_id = ensure_file_started.elapsed();

        let parse_started = Instant::now();
        let mut parser = if collect_docs {
            Parser::new(source, file_id, session)
        } else {
            Parser::new_without_docs(source, file_id, session)
        };
        let mut ast = match parser.parse_module() {
            Ok(ast) => ast,
            Err(_) => return (None, timings),
        };
        timings.parse = parse_started.elapsed();
        ast.path = normalized.to_string_lossy().to_string();

        if source_may_need_pruning(source) {
            let prune_started = Instant::now();
            let mut pruner = Pruner::new(session);
            pruner.prune_module(&mut ast);
            timings.prune = prune_started.elapsed();
        }

        (Some(FrontendParsedModule { file_id, ast }), timings)
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

fn source_may_need_pruning(source: &str) -> bool {
    source.contains("#[") || source.contains("#![")
}

#[cfg(test)]
mod tests {
    use super::FrontendDatabase;
    use kernc_utils::Session;

    #[test]
    fn source_override_reuses_file_id_when_reparsed() {
        let db = FrontendDatabase::new();
        let mut session = Session::new();
        let path = std::env::temp_dir().join(format!(
            "kern_frontend_db_{}_override.rn",
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
                .src,
            "fn main() i32 { return 2; }"
        );
    }

    #[test]
    fn parsed_module_memo_skips_reparse_when_source_is_stable() {
        let db = FrontendDatabase::new();
        let mut session = Session::new();
        let path =
            std::env::temp_dir().join(format!("kern_frontend_db_{}_stable.rn", std::process::id()));

        db.set_source_override(path.clone(), "fn main() i32 { return 1; }".to_string());

        let first = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");
        let node_id_after_first_parse = session.next_node_id;

        let second = db
            .load_parsed_module(&mut session, &path)
            .unwrap()
            .expect("module should parse");

        assert_eq!(first.file_id, second.file_id);
        assert_eq!(session.next_node_id, node_id_after_first_parse);
    }
}
