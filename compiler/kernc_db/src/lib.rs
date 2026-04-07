#![doc = include_str!("../README.md")]

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

pub type QueryResult<T> = Result<T, Cycle>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Revision(u64);

impl Revision {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DependencyToken(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    participants: Vec<&'static str>,
}

impl Cycle {
    pub fn participants(&self) -> &[&'static str] {
        &self.participants
    }
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.participants.is_empty() {
            return write!(f, "query cycle detected");
        }

        write!(
            f,
            "query cycle detected: {}",
            self.participants.join(" -> ")
        )
    }
}

impl std::error::Error for Cycle {}

#[derive(Clone, Default)]
pub struct Database {
    runtime: Arc<Mutex<RuntimeState>>,
}

impl Database {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> Revision {
        self.runtime.lock().unwrap().current_revision
    }

    fn allocate_token(&self) -> DependencyToken {
        let mut runtime = self.runtime.lock().unwrap();
        let token = DependencyToken(runtime.next_token);
        runtime.next_token += 1;
        token
    }

    fn bump_revision(&self) -> Revision {
        let mut runtime = self.runtime.lock().unwrap();
        runtime.current_revision = Revision(runtime.current_revision.0 + 1);
        runtime.current_revision
    }

    fn record_dependency(&self, dep: Box<dyn DependencyEdge>) {
        let mut runtime = self.runtime.lock().unwrap();
        let Some(frame) = runtime.stack.last_mut() else {
            return;
        };
        if frame
            .deps
            .iter()
            .any(|existing| existing.token() == dep.token())
        {
            return;
        }
        frame.deps.push(dep);
    }

    fn execute_frame<T, F>(
        &self,
        name: &'static str,
        token: DependencyToken,
        f: F,
    ) -> QueryResult<(T, Vec<Box<dyn DependencyEdge>>)>
    where
        F: FnOnce() -> QueryResult<T>,
    {
        {
            let mut runtime = self.runtime.lock().unwrap();
            runtime.stack.push(ActiveFrame {
                token,
                name,
                deps: Vec::new(),
            });
        }

        let result = f();

        let frame = {
            let mut runtime = self.runtime.lock().unwrap();
            runtime
                .stack
                .pop()
                .expect("active query frame must exist while executing")
        };

        result.map(|value| (value, frame.deps))
    }

    fn cycle_for(&self, token: DependencyToken, name: &'static str) -> Cycle {
        let runtime = self.runtime.lock().unwrap();
        let start = runtime
            .stack
            .iter()
            .position(|frame| frame.token == token)
            .unwrap_or(runtime.stack.len());

        let mut participants = runtime.stack[start..]
            .iter()
            .map(|frame| frame.name)
            .collect::<Vec<_>>();
        participants.push(name);
        Cycle { participants }
    }
}

#[derive(Default)]
struct RuntimeState {
    current_revision: Revision,
    next_token: u64,
    stack: Vec<ActiveFrame>,
}

struct ActiveFrame {
    token: DependencyToken,
    name: &'static str,
    deps: Vec<Box<dyn DependencyEdge>>,
}

trait DependencyEdge: Send + Sync {
    fn token(&self) -> DependencyToken;
    fn changed_since(&self, db: &Database, revision: Revision) -> QueryResult<bool>;
    fn clone_box(&self) -> Box<dyn DependencyEdge>;
}

impl Clone for Box<dyn DependencyEdge> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Clone)]
pub struct Input<K, V> {
    inner: Arc<InputInner<K, V>>,
}

impl<K, V> Input<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    pub fn new(_name: &'static str) -> Self {
        Self {
            inner: Arc::new(InputInner {
                entries: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn set(&self, db: &Database, key: K, value: V) -> bool {
        self.set_optional(db, key, Some(value))
    }

    pub fn clear(&self, db: &Database, key: K) -> bool {
        self.set_optional(db, key, None)
    }

    pub fn get(&self, db: &Database, key: K) -> QueryResult<Option<V>> {
        let (token, value) = self.inner.ensure_entry(db, key.clone());
        db.record_dependency(Box::new(InputDependency {
            input: self.clone(),
            key,
            token,
        }));
        Ok(value)
    }

    pub fn changed_at(&self, db: &Database, key: K) -> Revision {
        let (_, _, changed_at) = self.inner.ensure_entry_snapshot(db, key);
        changed_at
    }

    fn set_optional(&self, db: &Database, key: K, value: Option<V>) -> bool {
        let mut entries = self.inner.entries.lock().unwrap();
        let entry = entries.entry(key).or_insert_with(|| InputEntry {
            token: db.allocate_token(),
            value: None,
            changed_at: db.revision(),
        });

        if entry.value == value {
            return false;
        }

        entry.value = value;
        entry.changed_at = db.bump_revision();
        true
    }

    fn changed_since(&self, db: &Database, key: &K, revision: Revision) -> QueryResult<bool> {
        let (_, _, changed_at) = self.inner.ensure_entry_snapshot(db, key.clone());
        Ok(changed_at > revision)
    }
}

struct InputInner<K, V> {
    entries: Mutex<HashMap<K, InputEntry<V>>>,
}

impl<K, V> InputInner<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn ensure_entry(&self, db: &Database, key: K) -> (DependencyToken, Option<V>) {
        let (token, value, _) = self.ensure_entry_snapshot(db, key);
        (token, value)
    }

    fn ensure_entry_snapshot(
        &self,
        db: &Database,
        key: K,
    ) -> (DependencyToken, Option<V>, Revision) {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(key).or_insert_with(|| InputEntry {
            token: db.allocate_token(),
            value: None,
            changed_at: db.revision(),
        });
        (entry.token, entry.value.clone(), entry.changed_at)
    }
}

struct InputEntry<V> {
    token: DependencyToken,
    value: Option<V>,
    changed_at: Revision,
}

#[derive(Clone)]
pub struct Query<K, V> {
    inner: Arc<QueryInner<K, V>>,
}

#[derive(Clone)]
pub struct Memo<K, V> {
    inner: Arc<MemoInner<K, V>>,
}

type QueryCompute<K, V> = dyn Fn(&Database, &K) -> QueryResult<V> + Send + Sync;

impl<K, V> Query<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    pub fn new<F>(name: &'static str, compute: F) -> Self
    where
        F: Fn(&Database, &K) -> QueryResult<V> + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(QueryInner {
                name,
                compute: Box::new(compute),
                entries: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn get(&self, db: &Database, key: K) -> QueryResult<V> {
        loop {
            let current_revision = db.revision();
            let snapshot = self.inner.ensure_entry_snapshot(db, key.clone());

            if snapshot.state == QueryState::Evaluating {
                return Err(db.cycle_for(snapshot.token, self.inner.name));
            }

            if snapshot.value.is_some() && snapshot.verified_at == current_revision {
                let value = snapshot
                    .value
                    .clone()
                    .expect("ready query entry must have a cached value");
                db.record_dependency(Box::new(QueryDependency {
                    query: self.clone(),
                    key,
                    token: snapshot.token,
                }));
                return Ok(value);
            }

            if snapshot.value.is_some()
                && !self.dependencies_changed_since(db, &snapshot.deps, snapshot.verified_at)?
            {
                let mut entries = self.inner.entries.lock().unwrap();
                let entry = entries
                    .get_mut(&key)
                    .expect("query entry must exist while refreshing verification");
                if entry.state == QueryState::Ready {
                    entry.verified_at = current_revision;
                    let value = entry
                        .value
                        .clone()
                        .expect("ready query entry must have a cached value");
                    drop(entries);
                    db.record_dependency(Box::new(QueryDependency {
                        query: self.clone(),
                        key,
                        token: snapshot.token,
                    }));
                    return Ok(value);
                }
                continue;
            }

            {
                let mut entries = self.inner.entries.lock().unwrap();
                let entry = entries
                    .get_mut(&key)
                    .expect("query entry must exist before computing");
                if entry.state == QueryState::Evaluating {
                    return Err(db.cycle_for(snapshot.token, self.inner.name));
                }
                entry.state = QueryState::Evaluating;
            }

            let computed = db.execute_frame(self.inner.name, snapshot.token, || {
                (self.inner.compute)(db, &key)
            });

            match computed {
                Ok((value, deps)) => {
                    let changed_at = if snapshot.value.as_ref() == Some(&value) {
                        snapshot.changed_at
                    } else {
                        current_revision
                    };

                    let mut entries = self.inner.entries.lock().unwrap();
                    let entry = entries
                        .get_mut(&key)
                        .expect("query entry must exist after computing");
                    entry.value = Some(value.clone());
                    entry.changed_at = changed_at;
                    entry.verified_at = current_revision;
                    entry.deps = deps;
                    entry.state = QueryState::Ready;
                    drop(entries);

                    db.record_dependency(Box::new(QueryDependency {
                        query: self.clone(),
                        key,
                        token: snapshot.token,
                    }));
                    return Ok(value);
                }
                Err(err) => {
                    let mut entries = self.inner.entries.lock().unwrap();
                    let entry = entries
                        .get_mut(&key)
                        .expect("query entry must exist after failed computation");
                    entry.state = snapshot.state;
                    entry.value = snapshot.value.clone();
                    entry.changed_at = snapshot.changed_at;
                    entry.verified_at = snapshot.verified_at;
                    entry.deps = snapshot.deps.clone();
                    return Err(err);
                }
            }
        }
    }

    pub fn changed_at(&self, db: &Database, key: K) -> QueryResult<Revision> {
        self.refresh(db, key)
    }

    fn dependencies_changed_since(
        &self,
        db: &Database,
        deps: &[Box<dyn DependencyEdge>],
        verified_at: Revision,
    ) -> QueryResult<bool> {
        for dep in deps {
            if dep.changed_since(db, verified_at)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn refresh(&self, db: &Database, key: K) -> QueryResult<Revision> {
        let _ = self.get(db, key.clone())?;
        let entries = self.inner.entries.lock().unwrap();
        let entry = entries
            .get(&key)
            .expect("query entry must exist after refresh");
        Ok(entry.changed_at)
    }
}

struct QueryInner<K, V> {
    name: &'static str,
    compute: Box<QueryCompute<K, V>>,
    entries: Mutex<HashMap<K, QueryEntry<V>>>,
}

struct MemoInner<K, V> {
    entries: Mutex<HashMap<K, QueryEntry<V>>>,
}

impl<K, V> QueryInner<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn ensure_entry_snapshot(&self, db: &Database, key: K) -> QueryEntrySnapshot<V> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(key).or_insert_with(|| QueryEntry {
            token: db.allocate_token(),
            state: QueryState::Ready,
            value: None,
            changed_at: db.revision(),
            verified_at: Revision(0),
            deps: Vec::new(),
        });

        QueryEntrySnapshot {
            token: entry.token,
            state: entry.state,
            value: entry.value.clone(),
            changed_at: entry.changed_at,
            verified_at: entry.verified_at,
            deps: entry.deps.clone(),
        }
    }
}

impl<K, V> Memo<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoInner {
                entries: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn get_with<F>(
        &self,
        db: &Database,
        name: &'static str,
        key: K,
        compute: F,
    ) -> QueryResult<V>
    where
        F: FnOnce() -> QueryResult<V>,
    {
        loop {
            let current_revision = db.revision();
            let snapshot = self.inner.ensure_entry_snapshot(db, key.clone());

            if snapshot.state == QueryState::Evaluating {
                return Err(db.cycle_for(snapshot.token, name));
            }

            if snapshot.value.is_some() && snapshot.verified_at == current_revision {
                return Ok(snapshot
                    .value
                    .clone()
                    .expect("ready memo entry must have a cached value"));
            }

            if snapshot.value.is_some()
                && !dependencies_changed_since(db, &snapshot.deps, snapshot.verified_at)?
            {
                let mut entries = self.inner.entries.lock().unwrap();
                let entry = entries
                    .get_mut(&key)
                    .expect("memo entry must exist while refreshing verification");
                if entry.state == QueryState::Ready {
                    entry.verified_at = current_revision;
                    return Ok(entry
                        .value
                        .clone()
                        .expect("ready memo entry must have a cached value"));
                }
                continue;
            }

            {
                let mut entries = self.inner.entries.lock().unwrap();
                let entry = entries
                    .get_mut(&key)
                    .expect("memo entry must exist before computing");
                if entry.state == QueryState::Evaluating {
                    return Err(db.cycle_for(snapshot.token, name));
                }
                entry.state = QueryState::Evaluating;
            }

            let computed = db.execute_frame(name, snapshot.token, compute);

            match computed {
                Ok((value, deps)) => {
                    let mut entries = self.inner.entries.lock().unwrap();
                    let entry = entries
                        .get_mut(&key)
                        .expect("memo entry must exist after computing");
                    entry.value = Some(value.clone());
                    entry.changed_at = current_revision;
                    entry.verified_at = current_revision;
                    entry.deps = deps;
                    entry.state = QueryState::Ready;
                    return Ok(value);
                }
                Err(err) => {
                    let mut entries = self.inner.entries.lock().unwrap();
                    let entry = entries
                        .get_mut(&key)
                        .expect("memo entry must exist after failed computation");
                    entry.state = snapshot.state;
                    entry.value = snapshot.value.clone();
                    entry.changed_at = snapshot.changed_at;
                    entry.verified_at = snapshot.verified_at;
                    entry.deps = snapshot.deps.clone();
                    return Err(err);
                }
            }
        }
    }

    pub fn get_cached(&self, db: &Database, name: &'static str, key: K) -> QueryResult<Option<V>> {
        let current_revision = db.revision();
        let snapshot = self.inner.ensure_entry_snapshot(db, key.clone());

        if snapshot.state == QueryState::Evaluating {
            return Err(db.cycle_for(snapshot.token, name));
        }

        if snapshot.value.is_some() && snapshot.verified_at == current_revision {
            return Ok(snapshot.value);
        }

        if snapshot.value.is_some()
            && !dependencies_changed_since(db, &snapshot.deps, snapshot.verified_at)?
        {
            let mut entries = self.inner.entries.lock().unwrap();
            let entry = entries
                .get_mut(&key)
                .expect("memo entry must exist while refreshing verification");
            if entry.state == QueryState::Ready {
                entry.verified_at = current_revision;
                return Ok(entry.value.clone());
            }
        }

        Ok(None)
    }
}

impl<K, V> Default for Memo<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> MemoInner<K, V>
where
    K: Eq + Hash + Clone,
    V: Clone,
{
    fn ensure_entry_snapshot(&self, db: &Database, key: K) -> QueryEntrySnapshot<V> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.entry(key).or_insert_with(|| QueryEntry {
            token: db.allocate_token(),
            state: QueryState::Ready,
            value: None,
            changed_at: db.revision(),
            verified_at: Revision(0),
            deps: Vec::new(),
        });

        QueryEntrySnapshot {
            token: entry.token,
            state: entry.state,
            value: entry.value.clone(),
            changed_at: entry.changed_at,
            verified_at: entry.verified_at,
            deps: entry.deps.clone(),
        }
    }
}

fn dependencies_changed_since(
    db: &Database,
    deps: &[Box<dyn DependencyEdge>],
    verified_at: Revision,
) -> QueryResult<bool> {
    for dep in deps {
        if dep.changed_since(db, verified_at)? {
            return Ok(true);
        }
    }
    Ok(false)
}

struct QueryEntry<V> {
    token: DependencyToken,
    state: QueryState,
    value: Option<V>,
    changed_at: Revision,
    verified_at: Revision,
    deps: Vec<Box<dyn DependencyEdge>>,
}

#[derive(Clone)]
struct QueryEntrySnapshot<V> {
    token: DependencyToken,
    state: QueryState,
    value: Option<V>,
    changed_at: Revision,
    verified_at: Revision,
    deps: Vec<Box<dyn DependencyEdge>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryState {
    Ready,
    Evaluating,
}

#[derive(Clone)]
struct InputDependency<K, V> {
    input: Input<K, V>,
    key: K,
    token: DependencyToken,
}

impl<K, V> DependencyEdge for InputDependency<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    fn token(&self) -> DependencyToken {
        self.token
    }

    fn changed_since(&self, db: &Database, revision: Revision) -> QueryResult<bool> {
        self.input.changed_since(db, &self.key, revision)
    }

    fn clone_box(&self) -> Box<dyn DependencyEdge> {
        Box::new(self.clone())
    }
}

#[derive(Clone)]
struct QueryDependency<K, V> {
    query: Query<K, V>,
    key: K,
    token: DependencyToken,
}

impl<K, V> DependencyEdge for QueryDependency<K, V>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + PartialEq + Send + Sync + 'static,
{
    fn token(&self) -> DependencyToken {
        self.token
    }

    fn changed_since(&self, db: &Database, revision: Revision) -> QueryResult<bool> {
        Ok(self.query.changed_at(db, self.key.clone())? > revision)
    }

    fn clone_box(&self) -> Box<dyn DependencyEdge> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Cycle, Database, Input, Memo, Query};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn query_reuses_cached_value_inside_a_revision() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");
        let evals = Arc::new(AtomicUsize::new(0));

        let query = Query::new("parsed_len", {
            let input = input.clone();
            let evals = evals.clone();
            move |db, key| {
                evals.fetch_add(1, Ordering::SeqCst);
                let text = input.get(db, *key)?.unwrap_or_default();
                Ok(text.len())
            }
        });

        input.set(&db, 0, "kern".to_string());

        assert_eq!(query.get(&db, 0).unwrap(), 4);
        assert_eq!(query.get(&db, 0).unwrap(), 4);
        assert_eq!(evals.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn same_input_value_does_not_advance_revision() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");

        assert!(input.set(&db, 0, "kern".to_string()));
        let revision = db.revision();
        assert!(!input.set(&db, 0, "kern".to_string()));
        assert_eq!(db.revision(), revision);
    }

    #[test]
    fn transitive_queries_skip_recomputation_when_intermediate_value_is_stable() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");
        let parse_evals = Arc::new(AtomicUsize::new(0));
        let lower_evals = Arc::new(AtomicUsize::new(0));

        let parsed = Query::new("parsed_len", {
            let input = input.clone();
            let parse_evals = parse_evals.clone();
            move |db, key| {
                parse_evals.fetch_add(1, Ordering::SeqCst);
                let text = input.get(db, *key)?.unwrap_or_default();
                Ok(text.len())
            }
        });

        let lowered = Query::new("lowered_metric", {
            let parsed = parsed.clone();
            let lower_evals = lower_evals.clone();
            move |db, key| {
                lower_evals.fetch_add(1, Ordering::SeqCst);
                Ok(parsed.get(db, *key)? * 2)
            }
        });

        input.set(&db, 0, "ab".to_string());
        assert_eq!(lowered.get(&db, 0).unwrap(), 4);
        assert_eq!(parse_evals.load(Ordering::SeqCst), 1);
        assert_eq!(lower_evals.load(Ordering::SeqCst), 1);

        input.set(&db, 0, "cd".to_string());
        assert_eq!(lowered.get(&db, 0).unwrap(), 4);
        assert_eq!(parse_evals.load(Ordering::SeqCst), 2);
        assert_eq!(lower_evals.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn querying_a_missing_input_creates_a_real_dependency() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");
        let evals = Arc::new(AtomicUsize::new(0));

        let parsed = Query::new("parsed_len", {
            let input = input.clone();
            let evals = evals.clone();
            move |db, key| {
                evals.fetch_add(1, Ordering::SeqCst);
                Ok(input.get(db, *key)?.unwrap_or_default().len())
            }
        });

        assert_eq!(parsed.get(&db, 7).unwrap(), 0);
        assert_eq!(evals.load(Ordering::SeqCst), 1);

        input.set(&db, 7, "kern".to_string());
        assert_eq!(parsed.get(&db, 7).unwrap(), 4);
        assert_eq!(evals.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn detects_self_recursive_query_cycles() {
        let db = Database::new();
        let slot = Arc::new(Mutex::new(None::<Query<u32, u32>>));

        let query = Query::new("self_cycle", {
            let slot = slot.clone();
            move |db, key| {
                let query = slot.lock().unwrap().clone().unwrap();
                query.get(db, *key)
            }
        });
        *slot.lock().unwrap() = Some(query.clone());

        let err = query.get(&db, 0).unwrap_err();
        assert_cycle(err, &["self_cycle", "self_cycle"]);
    }

    #[test]
    fn memo_get_cached_reuses_a_validated_value_without_recomputing() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");
        let memo = Memo::<u32, usize>::new();
        let evals = Arc::new(AtomicUsize::new(0));

        input.set(&db, 0, "kern".to_string());

        assert_eq!(memo.get_cached(&db, "parsed_len_memo", 0).unwrap(), None);

        let compute_db = db.clone();
        let value = memo
            .get_with(&db, "parsed_len_memo", 0, {
                let input = input.clone();
                let evals = evals.clone();
                move || {
                    evals.fetch_add(1, Ordering::SeqCst);
                    Ok(input.get(&compute_db, 0)?.unwrap_or_default().len())
                }
            })
            .unwrap();
        assert_eq!(value, 4);
        assert_eq!(evals.load(Ordering::SeqCst), 1);

        assert_eq!(memo.get_cached(&db, "parsed_len_memo", 0).unwrap(), Some(4));
        assert_eq!(evals.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn memo_get_cached_drops_stale_entries_after_dependency_changes() {
        let db = Database::new();
        let input = Input::<u32, String>::new("source_text");
        let memo = Memo::<u32, usize>::new();

        input.set(&db, 0, "kern".to_string());
        let compute_db = db.clone();
        let _ = memo
            .get_with(&db, "parsed_len_memo", 0, {
                let input = input.clone();
                move || Ok(input.get(&compute_db, 0)?.unwrap_or_default().len())
            })
            .unwrap();

        input.set(&db, 0, "lang".to_string());
        assert_eq!(memo.get_cached(&db, "parsed_len_memo", 0).unwrap(), None);
    }

    fn assert_cycle(cycle: Cycle, expected: &[&str]) {
        assert_eq!(cycle.participants(), expected);
        assert!(cycle.to_string().contains("query cycle detected"));
    }
}
