use super::*;
use crate::passes::TypeResolver;
use crate::query::{MemberQuery, MemberQueryEnv};
use crate::ty::{ConstGeneric, ConstGenericValue, ConstGenericValueKind, GenericArg};

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    fn symbol_is_type_namespace(kind: SymbolKind) -> bool {
        matches!(
            kind,
            SymbolKind::Struct
                | SymbolKind::Union
                | SymbolKind::Enum
                | SymbolKind::Trait
                | SymbolKind::TypeAlias
                | SymbolKind::TypeParam
        )
    }

    pub(super) fn def_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.host.ctx.def_owner_scope(def_id)
    }

    pub(super) fn resolved_type(&mut self, ty: TypeId) -> TypeId {
        let mut resolved = ty;
        for subst_map in self.core.type_substs() {
            let mut subst = Substituter::new(&mut self.host.ctx.type_registry, subst_map);
            resolved = subst.substitute(resolved);
        }
        self.host.normalize_type(resolved)
    }

    pub(super) fn resolved_const_generic(&mut self, value: ConstGeneric) -> ConstGeneric {
        let mut resolved = value;
        for subst_map in self.core.type_substs() {
            let mut subst = Substituter::new(&mut self.host.ctx.type_registry, subst_map);
            resolved = subst.substitute_const_generic(resolved);
        }
        resolved
    }

    pub(super) fn node_type(&mut self, node_id: NodeId) -> TypeId {
        let ty = self.host.ctx.node_type(node_id).unwrap_or(TypeId::ERROR);
        self.resolved_type(ty)
    }

    pub(super) fn type_is_enum_like(&mut self, ty: TypeId) -> bool {
        let ty = self.resolved_type(ty);
        matches!(
            self.host.type_kind(ty),
            TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
        )
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
            ExprKind::TypeNode(type_node) => self.resolve_explicit_type_node(type_node),
            ExprKind::SelfValue => {
                let self_name = self.host.ctx.intern("self");
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
                .map(|ty| self.resolve_explicit_type_node(ty))
                .unwrap_or(TypeId::ERROR),
            _ => TypeId::ERROR,
        }
    }

    pub(super) fn resolve_explicit_type_node(&mut self, ty_node: &ast::TypeNode) -> TypeId {
        if let Some(ty) = self.host.ctx.node_type(ty_node.id)
            && ty != TypeId::ERROR
        {
            return self.resolved_type(ty);
        }

        let Some(scope) = self.host.ctx.scopes.current_scope_id() else {
            return TypeId::ERROR;
        };

        let mut resolver = TypeResolver::new(self.host.ctx);
        let ty = resolver.resolve_type(ty_node, scope);
        self.resolved_type(ty)
    }

    pub(super) fn callable_return_type(
        &mut self,
        def_id: DefId,
        generic_args: &[GenericArg],
    ) -> Option<TypeId> {
        let func = self.function_def(def_id)?;
        let sig = func.resolved_sig?;

        if func.generics.is_empty() {
            return match self.host.type_kind(sig).clone() {
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
        let mut subst = Substituter::new(&mut self.host.ctx.type_registry, &generic_map);
        let sig = subst.substitute(sig);

        match self.host.type_kind(sig).clone() {
            TypeKind::Function { ret, .. } => Some(ret),
            _ => None,
        }
    }

    pub(super) fn push_local_scope(&mut self) {
        self.core.push_local_scope();
    }

    pub(super) fn pop_local_scope(&mut self) {
        self.core.pop_local_scope();
    }

    pub(super) fn define_local(&mut self, name: SymbolId, value: ConstValue) {
        self.core.define_local(name, value);
    }

    pub(super) fn define_local_type(&mut self, name: SymbolId, ty: TypeId) {
        self.core.define_local_type(name, ty);
    }

    pub(super) fn define_local_mutability(&mut self, name: SymbolId, is_mut: bool) {
        self.core.define_local_mutability(name, is_mut);
    }

    pub(super) fn lookup_local(&self, name: SymbolId) -> Option<ConstValue> {
        self.core.lookup_local(name)
    }

    pub(super) fn lookup_local_slot(&self, name: SymbolId) -> Option<usize> {
        self.core.lookup_local_slot(name)
    }

    pub(super) fn lookup_local_at(&self, scope_idx: usize, name: SymbolId) -> Option<ConstValue> {
        self.core.lookup_local_at(scope_idx, name)
    }

    pub(super) fn lookup_local_type(&self, name: SymbolId) -> Option<TypeId> {
        self.core.lookup_local_type(name)
    }

    pub(super) fn resolve_symbol_info(&self, name: SymbolId) -> Option<crate::scope::SymbolInfo> {
        if let Some(&scope_id) = self.host.const_scopes.last() {
            self.host
                .ctx
                .scopes
                .resolve_from_namespace(scope_id, name, crate::scope::SymbolNamespace::Value)
                .cloned()
        } else {
            self.host.ctx.scopes.resolve_value_symbol(name).cloned()
        }
    }

    pub(super) fn module_scope_from_expr(&mut self, expr: &Expr) -> Option<ScopeId> {
        let expr_ty = self.node_type(expr.id);
        if let TypeKind::Module(def_id) = self.host.type_kind(expr_ty).clone()
            && let Some(module) = self.module_def(def_id)
        {
            return Some(module.scope_id);
        }

        match &expr.kind {
            ExprKind::Identifier(name) => {
                let info = self.host.ctx.scopes.resolve_module_symbol(*name)?.clone();
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let module = self.module_def(def_id)?;
                Some(module.scope_id)
            }
            ExprKind::FieldAccess { lhs, field, .. } => {
                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self
                    .host
                    .ctx
                    .scopes
                    .resolve_module_in(mod_scope, *field)?
                    .clone();
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let module = self.module_def(def_id)?;
                Some(module.scope_id)
            }
            _ => None,
        }
    }

    pub(super) fn expr_is_type_namespace(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::TypeNode(_) => true,
            ExprKind::Grouped { expr: inner } => self.expr_is_type_namespace(inner),
            ExprKind::Identifier(name) => self
                .host
                .ctx
                .scopes
                .resolve_namespace_symbol(*name)
                .map(|info| Self::symbol_is_type_namespace(info.kind))
                .unwrap_or(false),
            ExprKind::GenericInstantiation { target, .. } => self.expr_is_type_namespace(target),
            ExprKind::FieldAccess { lhs, field, .. } => {
                let Some(mod_scope) = self.module_scope_from_expr(lhs) else {
                    return false;
                };

                self.host
                    .ctx
                    .scopes
                    .resolve_namespace_in(mod_scope, *field)
                    .map(|info| Self::symbol_is_type_namespace(info.kind))
                    .unwrap_or(false)
            }
            _ => false,
        }
    }

    pub(super) fn resolve_callable(&mut self, callee: &Expr) -> Option<(DefId, Vec<GenericArg>)> {
        let callee_ty = self.node_type(callee.id);
        if let TypeKind::FnDef(def_id, args) = self.host.type_kind(callee_ty).clone() {
            return Some((def_id, args));
        }

        let callee_norm = self.host.normalize_type(callee_ty);
        if let TypeKind::FnDef(def_id, args) = self.host.type_kind(callee_norm).clone() {
            return Some((def_id, args));
        }

        match &callee.kind {
            ExprKind::Identifier(name) => {
                let info = self.host.ctx.scopes.resolve_value_symbol(*name)?.clone();
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            ExprKind::GenericInstantiation { target, args } => {
                let (def_id, _) = self.resolve_callable(target)?;
                let generic_args = args
                    .iter()
                    .map(|arg| match arg {
                        ast::GenericArg::Type(ty)
                        | ast::GenericArg::AssocBinding { value: ty, .. } => {
                            let ty = self.host.ctx.node_type(ty.id).unwrap_or(TypeId::ERROR);
                            GenericArg::Type(self.resolved_type(ty))
                        }
                        ast::GenericArg::ConstExpr(expr) => {
                            let ty = self.expr_type(expr);
                            let value = match self
                                .eval_inner(expr, self.core.current_function_depth() + 1)
                            {
                                Ok(ConstValue::Int(value)) => {
                                    ConstGeneric::Value(ConstGenericValue {
                                        ty,
                                        kind: ConstGenericValueKind::Int(value),
                                    })
                                }
                                Ok(ConstValue::Bool(value)) => {
                                    ConstGeneric::Value(ConstGenericValue {
                                        ty,
                                        kind: ConstGenericValueKind::Bool(value),
                                    })
                                }
                                _ => ConstGeneric::Error,
                            };
                            GenericArg::Const(self.resolved_const_generic(value))
                        }
                    })
                    .collect();
                Some((def_id, generic_args))
            }
            ExprKind::FieldAccess { lhs, field, .. } => {
                if !self.expr_is_type_namespace(lhs)
                    && let Some(found) = self.resolve_method_callable_from_receiver(lhs, *field)
                {
                    return Some(found);
                }

                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self
                    .host
                    .ctx
                    .scopes
                    .resolve_value_in(mod_scope, *field)?
                    .clone();
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn resolve_method_callable_from_receiver(
        &mut self,
        receiver: &Expr,
        method_name: SymbolId,
    ) -> Option<(DefId, Vec<GenericArg>)> {
        let receiver_ty = self.expr_type(receiver);
        if receiver_ty == TypeId::ERROR {
            return None;
        }

        let env = MemberQueryEnv::from_current_active_bounds(self.host.ctx);
        let mut query = MemberQuery::new(self.host.ctx);
        let resolution =
            query.resolve_named_method(receiver_ty, method_name, &env, receiver.span)?;
        let TypeKind::FnDef(def_id, generic_args) = self
            .host
            .ctx
            .type_registry
            .get(resolution.candidate.type_id)
            .clone()
        else {
            return None;
        };
        Some((def_id, generic_args))
    }

    pub(super) fn eval_const_def(
        &mut self,
        def_id: DefId,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        let Some(global) = self.global_def(def_id) else {
            return Err(ConstEvalError);
        };
        let const_expr = global.value;

        let scope_frame = self.enter_def_scope(def_id);

        let result = self.eval_inner(&const_expr, depth + 1);
        self.leave_def_scope(scope_frame);

        result
    }
}
