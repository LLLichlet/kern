#![allow(unused)]
use std::collections::HashMap;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolId(pub usize);

#[derive(Default, Debug, Clone)]
pub struct Interner {
    /// string -> id
    map: HashMap<String, SymbolId>,
    /// id -> string
    vec: Vec<String>,
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
        let name_string = name.to_string();
        self.vec.push(name_string.clone());
        self.map.insert(name_string, sym);
        sym
    }

   pub fn resolve(&self, sym: SymbolId) -> Option<&str> {
        self.vec.get(sym.0).map(|s| s.as_str())
    }
}