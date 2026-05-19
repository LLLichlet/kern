//! String interning for identifiers and compiler-created symbols.
//!
//! Most AST and semantic structures store `SymbolId` instead of cloning names.
//! The backing vector is append-only, so IDs remain stable for the lifetime of
//! a `Session` and can be copied freely between compiler tables.

use std::sync::Arc;

use crate::FastHashMap;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub usize);

#[derive(Default, Debug, Clone)]
pub struct Interner {
    /// string -> id
    map: FastHashMap<Arc<str>, SymbolId>,
    /// id -> string
    vec: Vec<Arc<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, name: &str) -> SymbolId {
        if let Some(&sym) = self.map.get(name) {
            return sym;
        }

        let sym = SymbolId(self.vec.len());
        let shared = Arc::<str>::from(name);
        self.vec.push(shared.clone());
        self.map.insert(shared, sym);
        sym
    }

    pub fn resolve(&self, sym: SymbolId) -> Option<&str> {
        self.vec.get(sym.0).map(|s| s.as_ref())
    }

    pub fn intern_snapshot(&mut self, symbols: &[Arc<str>]) -> Vec<SymbolId> {
        self.map.reserve(symbols.len());
        self.vec.reserve(symbols.len());

        // Preserve existing IDs when a snapshot is merged into a live session.
        // This is used by cached analysis state where symbols may already have
        // been interned through normal parsing.
        let mut ids = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            if let Some(&sym) = self.map.get(symbol.as_ref()) {
                ids.push(sym);
                continue;
            }

            let sym = SymbolId(self.vec.len());
            self.vec.push(symbol.clone());
            self.map.insert(symbol.clone(), sym);
            ids.push(sym);
        }
        ids
    }

    pub fn snapshot_symbols(&self) -> Vec<Arc<str>> {
        self.vec.clone()
    }
}
