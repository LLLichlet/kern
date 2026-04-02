use super::*;

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub(super) fn global_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.ctx.defs.iter().find_map(|def| {
            let Def::Module(module) = def else {
                return None;
            };

            if module.items.contains(&def_id) {
                Some(module.scope_id)
            } else {
                None
            }
        })
    }

    pub(super) fn def_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => {
                let mut current_parent = f.parent;
                while let Some(parent_id) = current_parent {
                    match &self.ctx.defs[parent_id.0 as usize] {
                        Def::Module(module) => return Some(module.scope_id),
                        Def::Impl(impl_def) => current_parent = impl_def.parent_module,
                        _ => return None,
                    }
                }
                None
            }
            Def::Global(_) => self.global_owner_scope(def_id),
            _ => None,
        }
    }

    pub(super) fn resolved_type(&mut self, ty: TypeId) -> TypeId {
        let mut resolved = ty;
        for subst_map in &self.type_substs {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
            resolved = subst.substitute(resolved);
        }
        self.ctx.type_registry.normalize(resolved)
    }

    pub(super) fn node_type(&mut self, node_id: NodeId) -> TypeId {
        let ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        self.resolved_type(ty)
    }

    pub(super) fn expr_type(&mut self, expr: &Expr) -> TypeId {
        let ty = self.node_type(expr.id);
        if ty != TypeId::ERROR {
            return ty;
        }

        match &expr.kind {
            ExprKind::Identifier(name) => self
                .lookup_local_type(*name)
                .map(|ty| self.resolved_type(ty))
                .or_else(|| {
                    self.resolve_symbol_info(*name)
                        .map(|info| self.resolved_type(info.type_id))
                })
                .unwrap_or(TypeId::ERROR),
            ExprKind::SelfValue => {
                let self_name = self.ctx.intern("self");
                self.lookup_local_type(self_name)
                    .map(|ty| self.resolved_type(ty))
                    .unwrap_or(TypeId::ERROR)
            }
            ExprKind::Call { callee, .. } => self
                .resolve_callable(callee)
                .and_then(|(def_id, generic_args)| self.callable_return_type(def_id, &generic_args))
                .unwrap_or(TypeId::ERROR),
            ExprKind::DataInit { type_node, .. } => type_node
                .as_deref()
                .and_then(|ty| self.ctx.node_types.get(&ty.id).copied())
                .map(|ty| self.resolved_type(ty))
                .unwrap_or(TypeId::ERROR),
            _ => TypeId::ERROR,
        }
    }

    pub(super) fn callable_return_type(
        &mut self,
        def_id: DefId,
        generic_args: &[TypeId],
    ) -> Option<TypeId> {
        let Def::Function(func) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
            return None;
        };
        let sig = func.resolved_sig?;

        if func.generics.is_empty() {
            return match self.ctx.type_registry.get(sig).clone() {
                TypeKind::Function { ret, .. } => Some(ret),
                _ => None,
            };
        }

        if func.generics.len() != generic_args.len() {
            return None;
        }

        let mut generic_map = HashMap::new();
        for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
            generic_map.insert(param.name, *arg);
        }
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &generic_map);
        let sig = subst.substitute(sig);

        match self.ctx.type_registry.get(sig).clone() {
            TypeKind::Function { ret, .. } => Some(ret),
            _ => None,
        }
    }

    pub(super) fn push_local_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
        self.local_type_scopes.push(HashMap::new());
        self.local_mut_scopes.push(HashMap::new());
    }

    pub(super) fn pop_local_scope(&mut self) {
        let _ = self.local_scopes.pop();
        let _ = self.local_type_scopes.pop();
        let _ = self.local_mut_scopes.pop();
    }

    pub(super) fn define_local(&mut self, name: SymbolId, value: ConstValue) {
        if self.local_scopes.is_empty() {
            self.push_local_scope();
        }
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    pub(super) fn define_local_type(&mut self, name: SymbolId, ty: TypeId) {
        if self.local_type_scopes.is_empty() {
            self.push_local_scope();
        }
        if let Some(scope) = self.local_type_scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    pub(super) fn define_local_mutability(&mut self, name: SymbolId, is_mut: bool) {
        if self.local_mut_scopes.is_empty() {
            self.push_local_scope();
        }
        if let Some(scope) = self.local_mut_scopes.last_mut() {
            scope.insert(name, is_mut);
        }
    }

    pub(super) fn lookup_local(&self, name: SymbolId) -> Option<ConstValue> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).cloned())
    }

    pub(super) fn lookup_local_slot(&self, name: SymbolId) -> Option<usize> {
        self.local_scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(scope_idx, scope)| scope.contains_key(&name).then_some(scope_idx))
    }

    pub(super) fn lookup_local_at(&self, scope_idx: usize, name: SymbolId) -> Option<ConstValue> {
        self.local_scopes
            .get(scope_idx)
            .and_then(|scope| scope.get(&name).cloned())
    }

    pub(super) fn lookup_local_mutability_at(
        &self,
        scope_idx: usize,
        name: SymbolId,
    ) -> Option<bool> {
        self.local_mut_scopes
            .get(scope_idx)
            .and_then(|scope| scope.get(&name).copied())
    }

    pub(super) fn lookup_local_type(&self, name: SymbolId) -> Option<TypeId> {
        self.local_type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    pub(super) fn assign_local_at(
        &mut self,
        scope_idx: usize,
        name: SymbolId,
        value: ConstValue,
    ) -> bool {
        if let Some(scope) = self.local_scopes.get_mut(scope_idx)
            && let Some(slot) = scope.get_mut(&name)
        {
            *slot = value;
            return true;
        }
        false
    }

    pub(super) fn resolve_symbol_info(&self, name: SymbolId) -> Option<crate::scope::SymbolInfo> {
        if let Some(&scope_id) = self.const_scopes.last() {
            self.ctx.scopes.resolve_from(scope_id, name).cloned()
        } else {
            self.ctx.scopes.resolve(name).cloned()
        }
    }

    pub(super) fn module_scope_from_expr(&mut self, expr: &Expr) -> Option<ScopeId> {
        let expr_ty = self.node_type(expr.id);
        if let TypeKind::Module(def_id) = self.ctx.type_registry.get(expr_ty).clone()
            && let Def::Module(module) = &self.ctx.defs[def_id.0 as usize]
        {
            return Some(module.scope_id);
        }

        match &expr.kind {
            ExprKind::Identifier(name) => {
                let info = self.resolve_symbol_info(*name)?;
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let Def::Module(module) = &self.ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                Some(module.scope_id)
            }
            ExprKind::FieldAccess { lhs, field, .. } => {
                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self.ctx.scopes.resolve_in(mod_scope, *field)?.clone();
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let Def::Module(module) = &self.ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                Some(module.scope_id)
            }
            _ => None,
        }
    }

    pub(super) fn resolve_callable(&mut self, callee: &Expr) -> Option<(DefId, Vec<TypeId>)> {
        let callee_ty = self.node_type(callee.id);
        if let TypeKind::FnDef(def_id, args) = self.ctx.type_registry.get(callee_ty).clone() {
            return Some((def_id, args));
        }

        match &callee.kind {
            ExprKind::Identifier(name) => {
                let info = self.resolve_symbol_info(*name)?;
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            ExprKind::GenericInstantiation { target, types } => {
                let (def_id, _) = self.resolve_callable(target)?;
                let generic_args = types
                    .iter()
                    .map(|ty| {
                        let ty = self
                            .ctx
                            .node_types
                            .get(&ty.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        self.resolved_type(ty)
                    })
                    .collect();
                Some((def_id, generic_args))
            }
            ExprKind::FieldAccess { lhs, field, .. } => {
                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self.ctx.scopes.resolve_in(mod_scope, *field)?.clone();
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub(super) fn eval_const_def(
        &mut self,
        def_id: DefId,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
            g.value.clone()
        } else {
            return Err(ConstEvalError);
        };

        let prev_scope = self.ctx.scopes.current_scope_id();
        let owner_scope = self.def_owner_scope(def_id);
        if let Some(owner_scope) = owner_scope {
            self.ctx.scopes.set_current_scope(owner_scope);
            self.const_scopes.push(owner_scope);
        }

        let result = self.eval_inner(&const_expr, depth + 1);

        if owner_scope.is_some() {
            let _ = self.const_scopes.pop();
        }
        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }

        result
    }
}
