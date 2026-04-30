use crate::def::DefId;
use crate::ty::TypeId;
use kernc_ast::Visibility;
use kernc_utils::{FastHashMap, NodeId, Span, SymbolId};

/// Globally unique scope identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub usize);

/// Kinds of symbols stored in scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Var,        // Local variable, including `let` bindings and parameters.
    Const,      // Immutable constant item.
    ConstParam, // Const generic parameter.
    Static,     // Static storage item.
    Function,   // Function item.
    Struct,     // Struct definition.
    Union,      // Union definition.
    Enum,       // Enum or algebraic data type definition.
    Trait,      // Trait definition.
    Module,     // Module namespace.
    TypeAlias,  // Type alias.
    AssociatedType,
    TypeParam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolNamespace {
    Value,
    Type,
    Module,
}

impl SymbolKind {
    pub fn namespace(self) -> SymbolNamespace {
        match self {
            SymbolKind::Var
            | SymbolKind::Const
            | SymbolKind::ConstParam
            | SymbolKind::Static
            | SymbolKind::Function => SymbolNamespace::Value,
            SymbolKind::Struct
            | SymbolKind::Union
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::TypeAlias
            | SymbolKind::AssociatedType
            | SymbolKind::TypeParam => SymbolNamespace::Type,
            SymbolKind::Module => SymbolNamespace::Module,
        }
    }
}

/// Semantic information tracked for one scoped symbol.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub node_id: NodeId,       // Owning AST node.
    pub type_id: TypeId,       // Semantic type assigned to the symbol.
    pub def_id: Option<DefId>, // Optional backing definition-table entry.
    pub span: Span,
    pub vis: Visibility,
    pub is_mut: bool,
}

/// One lexical scope node stored in the persistent scope arena.
#[derive(Debug, Clone)]
pub struct Scope {
    pub id: ScopeId,
    /// Parent scope. Module roots may point to the builtin scope or be absent.
    pub parent: Option<ScopeId>,
    pub symbols: FastHashMap<(SymbolId, SymbolNamespace), SymbolInfo>,
}

impl Scope {
    pub fn new(id: ScopeId, parent: Option<ScopeId>) -> Self {
        Self {
            id,
            parent,
            symbols: FastHashMap::default(),
        }
    }
}

/// Scope arena plus the active traversal cursor.
#[derive(Clone)]
pub struct SymbolTable {
    /// All allocated scopes live here permanently.
    scopes: Vec<Scope>,
    /// Scope currently active for resolution and insertion.
    current_scope: Option<ScopeId>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut table = Self {
            scopes: Vec::new(),
            current_scope: None,
        };
        // Start with a single root scope for builtins and global aliases.
        let root_id = table.create_scope(None);
        table.current_scope = Some(root_id);
        table
    }

    /// Allocate a new scope node without changing the active scope.
    fn create_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.scopes.len());
        self.scopes.push(Scope::new(id, parent));
        id
    }

    /// Enter a new child scope and make it current.
    /// The returned `ScopeId` can be attached to modules during collection.
    pub fn enter_scope(&mut self) -> ScopeId {
        let new_id = self.create_scope(self.current_scope);
        self.current_scope = Some(new_id);
        new_id
    }

    /// Leave the current scope and restore its parent.
    pub fn exit_scope(&mut self) {
        if let Some(current) = self.current_scope {
            // Follow the lexical parent link.
            self.current_scope = self.scopes[current.0].parent;
        } else {
            // Recover to the root scope if the stack has become unbalanced.
            self.current_scope = self.scopes.first().map(|scope| scope.id);
        }
    }

    /// Force the active scope to a known scope node.
    pub fn set_current_scope(&mut self, scope_id: ScopeId) {
        self.current_scope = Some(scope_id);
    }

    /// Return the currently active scope, if any.
    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.current_scope
    }

    pub fn parent_scope_id(&self, scope_id: ScopeId) -> Option<ScopeId> {
        self.scopes.get(scope_id.0).and_then(|scope| scope.parent)
    }

    /// Define a symbol in the current scope.
    /// On conflicts, return the previous symbol info so callers can build diagnostics.
    pub fn define(&mut self, name: SymbolId, info: SymbolInfo) -> Result<(), SymbolInfo> {
        let current_id = if let Some(scope_id) = self.current_scope {
            scope_id
        } else {
            let fallback = self
                .scopes
                .first()
                .map(|scope| scope.id)
                .unwrap_or_else(|| self.create_scope(None));
            self.current_scope = Some(fallback);
            fallback
        };
        let current_scope = &mut self.scopes[current_id.0];

        let key = (name, info.kind.namespace());
        if let Some(existing) = current_scope.symbols.get(&key) {
            // Preserve the previous symbol for "previous definition is here" diagnostics.
            return Err(existing.clone());
        }
        current_scope.symbols.insert(key, info);
        Ok(())
    }

    fn namespace_priority() -> [SymbolNamespace; 3] {
        [
            SymbolNamespace::Value,
            SymbolNamespace::Type,
            SymbolNamespace::Module,
        ]
    }

    fn namespace_priority_for_namespaces() -> [SymbolNamespace; 2] {
        [SymbolNamespace::Module, SymbolNamespace::Type]
    }

    /// Resolve a symbol by walking outward through lexical parents.
    pub fn resolve(&self, name: SymbolId) -> Option<&SymbolInfo> {
        for namespace in Self::namespace_priority() {
            if let Some(info) = self.resolve_in_namespace(name, namespace) {
                return Some(info);
            }
        }
        None
    }

    pub fn resolve_namespace_symbol(&self, name: SymbolId) -> Option<&SymbolInfo> {
        for namespace in Self::namespace_priority_for_namespaces() {
            if let Some(info) = self.resolve_in_namespace(name, namespace) {
                return Some(info);
            }
        }
        None
    }

    pub fn resolve_type_symbol(&self, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_in_namespace(name, SymbolNamespace::Type)
    }

    pub fn resolve_module_symbol(&self, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_in_namespace(name, SymbolNamespace::Module)
    }

    pub fn resolve_value_symbol(&self, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_in_namespace(name, SymbolNamespace::Value)
    }

    pub fn resolve_in_namespace(
        &self,
        name: SymbolId,
        namespace: SymbolNamespace,
    ) -> Option<&SymbolInfo> {
        let mut curr = self.current_scope;

        while let Some(id) = curr {
            let scope = &self.scopes[id.0];
            if let Some(info) = scope.symbols.get(&(name, namespace)) {
                return Some(info);
            }
            curr = scope.parent; // Continue searching outward.
        }
        None
    }

    pub fn resolve_from(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        for namespace in Self::namespace_priority() {
            if let Some(info) = self.resolve_from_namespace(scope_id, name, namespace) {
                return Some(info);
            }
        }
        None
    }

    pub fn resolve_namespace_from(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        for namespace in Self::namespace_priority_for_namespaces() {
            if let Some(info) = self.resolve_from_namespace(scope_id, name, namespace) {
                return Some(info);
            }
        }
        None
    }

    pub fn resolve_type_from(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_from_namespace(scope_id, name, SymbolNamespace::Type)
    }

    pub fn resolve_module_from(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_from_namespace(scope_id, name, SymbolNamespace::Module)
    }

    pub fn resolve_from_namespace(
        &self,
        scope_id: ScopeId,
        name: SymbolId,
        namespace: SymbolNamespace,
    ) -> Option<&SymbolInfo> {
        let mut curr = Some(scope_id);

        while let Some(id) = curr {
            let scope = &self.scopes[id.0];
            if let Some(info) = scope.symbols.get(&(name, namespace)) {
                return Some(info);
            }
            curr = scope.parent;
        }

        None
    }

    /// Resolve only within the current scope.
    pub fn resolve_local(&self, name: SymbolId) -> Option<&SymbolInfo> {
        let current_id = self.current_scope?;
        self.resolve_in_scope_with_priority(current_id, name, &Self::namespace_priority())
    }

    /// Resolve a symbol directly inside the specified scope without walking parents.
    /// This is used for imports such as `use std.math.add`.
    pub fn resolve_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_in_scope_with_priority(scope_id, name, &Self::namespace_priority())
    }

    pub fn resolve_namespace_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.resolve_in_scope_with_priority(
            scope_id,
            name,
            &Self::namespace_priority_for_namespaces(),
        )
    }

    pub fn resolve_type_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.scopes[scope_id.0]
            .symbols
            .get(&(name, SymbolNamespace::Type))
    }

    pub fn resolve_module_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.scopes[scope_id.0]
            .symbols
            .get(&(name, SymbolNamespace::Module))
    }

    pub fn resolve_value_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.scopes[scope_id.0]
            .symbols
            .get(&(name, SymbolNamespace::Value))
    }

    fn resolve_in_scope_with_priority(
        &self,
        scope_id: ScopeId,
        name: SymbolId,
        namespaces: &[SymbolNamespace],
    ) -> Option<&SymbolInfo> {
        for namespace in namespaces {
            if let Some(info) = self.scopes[scope_id.0].symbols.get(&(name, *namespace)) {
                return Some(info);
            }
        }
        None
    }

    pub fn symbols_in_scope(
        &self,
        scope_id: ScopeId,
    ) -> impl Iterator<Item = (SymbolId, &SymbolInfo)> + '_ {
        self.scopes[scope_id.0]
            .symbols
            .iter()
            .map(|((name, _), info)| (*name, info))
    }

    pub fn distance_to_ancestor(&self, scope_id: ScopeId, ancestor: ScopeId) -> Option<usize> {
        let mut curr = Some(scope_id);
        let mut distance = 0;

        while let Some(id) = curr {
            if id == ancestor {
                return Some(distance);
            }

            curr = self.scopes[id.0].parent;
            distance += 1;
        }

        None
    }

    pub fn parent_scope(&self, scope_id: ScopeId) -> Option<ScopeId> {
        self.scopes.get(scope_id.0).and_then(|scope| scope.parent)
    }

    /// Update the type of an existing symbol after inference.
    pub fn update_type(&mut self, name: SymbolId, ty: TypeId) {
        let mut curr = self.current_scope;

        // Update the scope where the symbol was originally defined.
        while let Some(id) = curr {
            let scope = &mut self.scopes[id.0];
            for namespace in Self::namespace_priority() {
                if let Some(info) = scope.symbols.get_mut(&(name, namespace)) {
                    info.type_id = ty;
                    return;
                }
            }
            curr = scope.parent;
        }
    }

    pub fn update_type_in_namespace(
        &mut self,
        name: SymbolId,
        namespace: SymbolNamespace,
        ty: TypeId,
    ) {
        let mut curr = self.current_scope;

        while let Some(id) = curr {
            let scope = &mut self.scopes[id.0];
            if let Some(info) = scope.symbols.get_mut(&(name, namespace)) {
                info.type_id = ty;
                return;
            }
            curr = scope.parent;
        }
    }

    pub fn update_type_in_scope(&mut self, scope_id: ScopeId, name: SymbolId, ty: TypeId) -> bool {
        for namespace in Self::namespace_priority() {
            if let Some(info) = self.scopes[scope_id.0].symbols.get_mut(&(name, namespace)) {
                info.type_id = ty;
                return true;
            }
        }

        false
    }

    pub fn update_span_in_scope(&mut self, scope_id: ScopeId, name: SymbolId, span: Span) -> bool {
        for namespace in Self::namespace_priority() {
            if let Some(info) = self.scopes[scope_id.0].symbols.get_mut(&(name, namespace)) {
                info.span = span;
                return true;
            }
        }

        false
    }

    pub fn all_symbols(&self) -> impl Iterator<Item = (SymbolId, &SymbolInfo)> + '_ {
        self.scopes
            .iter()
            .flat_map(|scope| scope.symbols.iter().map(|((name, _), info)| (*name, info)))
    }
}
