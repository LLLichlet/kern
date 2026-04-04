use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

    pub fn source_exists(&self, path: &Path) -> Result<bool, kernc_db::Cycle> {
        Ok(self
            .source_texts
            .get(&self.db, normalize_path(path))?
            .is_some())
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

                let file_id = self.ensure_file_id(session, &normalized, &source);
                let mut parser = Parser::new(&source, file_id, session);
                let mut ast = match parser.parse_module() {
                    Ok(ast) => ast,
                    Err(_) => return Ok(None),
                };
                ast.path = normalized.to_string_lossy().to_string();

                let mut pruner = Pruner::new(session);
                pruner.prune_module(&mut ast);

                Ok(Some(FrontendParsedModule { file_id, ast }))
            },
        )
    }

    pub fn load_parsed_module_uncached(
        &self,
        session: &mut Session,
        path: &Path,
    ) -> Result<Option<FrontendParsedModule>, kernc_db::Cycle> {
        #[cfg(test)]
        self.uncached_parse_count.fetch_add(1, Ordering::SeqCst);

        let normalized = normalize_path(path);
        let Some(source) = self.source_texts.get(&self.db, normalized.clone())? else {
            return Ok(None);
        };

        let file_id = self.ensure_file_id(session, &normalized, &source);
        let mut parser = Parser::new(&source, file_id, session);
        let mut ast = match parser.parse_module() {
            Ok(ast) => ast,
            Err(_) => return Ok(None),
        };
        ast.path = normalized.to_string_lossy().to_string();

        let mut pruner = Pruner::new(session);
        pruner.prune_module(&mut ast);

        Ok(Some(FrontendParsedModule { file_id, ast }))
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
}

impl Default for FrontendDatabase {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
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
