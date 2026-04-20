use super::*;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    /// Infer whether an expression can be treated as a mutable lvalue.
    pub(crate) fn is_lvalue_mutable(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    info.is_mut
                } else {
                    false
                }
            }
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                operand,
            } => {
                let ptr_ty = self
                    .ctx
                    .node_types
                    .get(&operand.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm = self.resolve_tv(ptr_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        *is_mut
                    }
                    _ => false,
                }
            }
            ExprKind::FieldAccess { lhs, .. } => {
                let lhs_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_lhs = self.resolve_tv(lhs_ty);

                match self.ctx.type_registry.get(norm_lhs).clone() {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        is_mut
                    }
                    _ => self.is_lvalue_mutable(lhs),
                }
            }
            ExprKind::IndexAccess { lhs, .. } => {
                let lhs_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_lhs = self.resolve_tv(lhs_ty);

                match self.ctx.type_registry.get(norm_lhs).clone() {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        is_mut
                    }
                    TypeKind::Slice { is_mut, .. } => is_mut,
                    TypeKind::Array { .. } | TypeKind::ArrayInfer { .. } => {
                        // Arrays are inline aggregates. Element writes follow the mutability of the
                        // storage path that reaches the array value, not a separate element-level
                        // mutability flag on the array type itself.
                        self.is_lvalue_mutable(lhs)
                    }
                    _ => self.is_lvalue_mutable(lhs),
                }
            }
            ExprKind::SliceOp { is_mut, .. } => *is_mut,

            // Materialized rvalues become mutable stack temporaries by default.
            ExprKind::DataInit { .. }
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Call { .. } => {
                true // Materialized temporaries are owned by the current scope.
            }
            ExprKind::String(_) => {
                false // String literals live in `.rodata` and cannot be mutably borrowed.
            }

            _ => false,
        }
    }

    /// Whether `expr..&` may explicitly materialize a mutable stack temporary.
    ///
    /// This is the generalized "stack mode" path for value expressions such as
    /// `Type.{ ... }..&`, `(if (...) ... else ... )..&`, `call()..&`, or
    /// `({ ... })..&`. Existing places such as identifiers or field accesses are
    /// intentionally excluded here so they continue to borrow the original
    /// storage instead of silently copying.
    pub(crate) fn can_materialize_mut_temporary(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(_)
            | ExprKind::SelfValue
            | ExprKind::FieldAccess { .. }
            | ExprKind::IndexAccess { .. } => false,
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                ..
            } => false,

            ExprKind::String(_) => false,

            ExprKind::Let { .. }
            | ExprKind::Static { .. }
            | ExprKind::TypeNode(_)
            | ExprKind::For { .. }
            | ExprKind::Defer { .. }
            | ExprKind::Break
            | ExprKind::Continue
            | ExprKind::Return(_)
            | ExprKind::Assign { .. }
            | ExprKind::Undef
            | ExprKind::Infer => false,

            _ => true,
        }
    }

    pub(crate) fn can_take_mut_address_of(&mut self, expr: &Expr) -> bool {
        self.is_lvalue_mutable(expr) || self.can_materialize_mut_temporary(expr)
    }

    /// Follow inference variables until reaching their final concrete binding.
    pub(crate) fn resolve_tv(&mut self, ty: TypeId) -> TypeId {
        let mut curr = ty;
        let mut projection_chain = Vec::new();
        loop {
            let norm = self.ctx.type_registry.normalize(curr);
            let is_projection = matches!(
                self.ctx.type_registry.get(norm),
                TypeKind::Projection { .. }
            );
            if is_projection {
                if let Some(ancestor_index) = projection_chain.iter().position(|seen| *seen == norm)
                {
                    let cycle = projection_chain[ancestor_index..]
                        .iter()
                        .copied()
                        .chain(std::iter::once(norm))
                        .collect::<Vec<_>>();
                    self.ctx.emit_projection_cycle_diagnostic(&cycle);
                    return TypeId::ERROR;
                }
                projection_chain.push(norm);
            }
            match self.ctx.type_registry.get(norm) {
                TypeKind::TypeVar(vid) => {
                    let Some(slot) = self.type_vars.get(*vid as usize) else {
                        return norm;
                    };
                    if let Some(target) = *slot {
                        curr = target;
                    } else {
                        return norm; // Unresolved inference variables remain as-is.
                    }
                }
                TypeKind::Projection { .. } => {
                    if let Some(projected) = self.try_normalize_projection(norm) {
                        if projected == norm {
                            self.ctx.emit_projection_cycle_diagnostic(&[norm, norm]);
                            return TypeId::ERROR;
                        }
                        curr = projected;
                    } else {
                        return norm;
                    }
                }
                _ => return norm,
            }
        }
    }

    fn try_normalize_projection(&mut self, projection_ty: TypeId) -> Option<TypeId> {
        if self.projection_normalization_stack.contains(&projection_ty) {
            return None;
        }
        self.projection_normalization_stack.push(projection_ty);

        let result = (|| {
            let TypeKind::Projection {
                target,
                trait_def_id,
                trait_args,
                assoc_def_id,
                assoc_args,
            } = self.ctx.type_registry.get(projection_ty).clone()
            else {
                return None;
            };

            if !assoc_args.is_empty() {
                return None;
            }

            let target_norm = self.resolve_tv(target);
            if let TypeKind::TraitObject(target_trait_def_id, _, assoc_bindings) =
                self.ctx.type_registry.get(target_norm).clone()
                && target_trait_def_id == trait_def_id
                && let Some((_, assoc_ty)) = assoc_bindings
                    .iter()
                    .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
            {
                return Some(self.resolve_tv(*assoc_ty));
            }

            if let Some(bound_ty) = self.projection_assoc_from_env_bounds(
                target_norm,
                trait_def_id,
                &trait_args,
                assoc_def_id,
            ) {
                return Some(self.resolve_tv(bound_ty));
            }

            if let Some(bound_ty) = self.projection_assoc_from_global_impls(
                target_norm,
                trait_def_id,
                &trait_args,
                assoc_def_id,
            ) {
                return Some(self.resolve_tv(bound_ty));
            }

            None
        })();

        let popped = self.projection_normalization_stack.pop();
        debug_assert_eq!(popped, Some(projection_ty));
        result
    }

    fn projection_assoc_from_env_bounds(
        &mut self,
        target_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
        assoc_def_id: DefId,
    ) -> Option<TypeId> {
        if self.ctx.active_bounds.is_empty() {
            return None;
        }

        let active_bounds_ptr = std::ptr::from_ref(self.ctx.active_bounds.as_slice());

        for (env_target, env_bounds) in unsafe { &*active_bounds_ptr } {
            if *env_target != target_ty {
                continue;
            }

            for bound in env_bounds.iter().copied() {
                let inst_bound_norm = self.resolve_tv(bound);
                let TypeKind::TraitObject(bound_trait_def_id, bound_trait_args, assoc_bindings) =
                    self.ctx.type_registry.get(inst_bound_norm).clone()
                else {
                    continue;
                };
                if bound_trait_def_id != trait_def_id || bound_trait_args != trait_args {
                    continue;
                }

                if let Some((_, assoc_ty)) = assoc_bindings
                    .iter()
                    .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
                {
                    return Some(*assoc_ty);
                }
            }
        }

        None
    }

    fn projection_assoc_from_global_impls(
        &mut self,
        target_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
        assoc_def_id: DefId,
    ) -> Option<TypeId> {
        let trait_impl_ids_ptr = std::ptr::from_ref(self.ctx.trait_impls.as_slice());
        let mut selected: Option<(DefId, TypeId)> = None;

        for impl_id in unsafe { &*trait_impl_ids_ptr }.iter().copied() {
            if !matches!(self.ctx.defs.get(impl_id.0 as usize), Some(Def::Impl(_))) {
                continue;
            }

            {
                let mut resolver = TypeResolver::new(self.ctx);
                resolver.ensure_impl_signature_types_resolved(impl_id);
            }

            let Some(impl_args) = crate::query::resolve_trait_impl_head_obligation(
                self.ctx,
                target_ty,
                trait_def_id,
                trait_args,
                impl_id,
            ) else {
                continue;
            };
            let Some(inst_trait_norm) =
                crate::query::instantiate_impl_trait_ty(self.ctx, impl_id, &impl_args)
            else {
                continue;
            };
            let TypeKind::TraitObject(bound_trait_def_id, bound_trait_args, assoc_bindings) =
                self.ctx.type_registry.get(inst_trait_norm).clone()
            else {
                continue;
            };
            if bound_trait_def_id != trait_def_id {
                continue;
            }
            if bound_trait_args != trait_args {
                continue;
            }

            if let Some((_, assoc_ty)) = assoc_bindings
                .iter()
                .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
            {
                let replace = match selected {
                    None => true,
                    Some((selected_impl_id, _)) => matches!(
                        crate::query::compare_impl_specificity(self.ctx, impl_id, selected_impl_id),
                        crate::query::ImplSpecificity::LeftMoreSpecific
                    ),
                };
                if replace {
                    selected = Some((impl_id, *assoc_ty));
                }
            }
        }

        selected.map(|(_, assoc_ty)| assoc_ty)
    }

    pub(super) fn bind_type_var(&mut self, vid: u32, ty: TypeId) {
        let ty = self.resolve_tv(ty);
        if matches!(self.ctx.type_registry.get(ty), TypeKind::TypeVar(bound_vid) if *bound_vid == vid)
        {
            return;
        }

        let vid = vid as usize;
        if self.type_vars.len() <= vid {
            self.type_vars.resize(vid + 1, None);
        }
        self.type_vars[vid] = Some(ty);
    }
}
