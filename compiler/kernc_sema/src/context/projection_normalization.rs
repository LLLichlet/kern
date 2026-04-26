use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProjectionAssocFrame {
    assoc_def_id: DefId,
    projection_ty: TypeId,
    saw_wrapper_growth: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ApplicableProjectionCandidate {
    impl_id: DefId,
    impl_assoc_id: DefId,
    assoc_ty: TypeId,
}

impl<'a> SemaContext<'a> {
    pub fn normalize_concrete_type(&mut self, ty: TypeId) -> TypeId {
        // Track both the projection types we are currently expanding and the impl-associated
        // definitions they pass through. The two stacks catch slightly different cycles:
        // syntactic `<T as Trait>::Assoc -> ... -> <T as Trait>::Assoc` loops versus repeated
        // re-entry into the same impl-provided associated type body.
        //
        // Roughly:
        // `projection_stack` guards immediate recursion while descending one projection node,
        // `projection_chain` remembers the outer normalization path,
        // `projection_assoc_chain` remembers which impl-associated bodies we entered to get there.
        let mut projection_stack = Vec::new();
        let mut projection_chain = Vec::new();
        let mut projection_assoc_chain = Vec::new();
        self.normalize_concrete_type_inner(
            ty,
            &mut projection_stack,
            &mut projection_chain,
            &mut projection_assoc_chain,
        )
    }

    fn normalize_concrete_type_inner(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> TypeId {
        let norm = self.type_registry.normalize(ty);
        if matches!(self.type_registry.get(norm), TypeKind::Projection { .. }) {
            return self.normalize_projection_root(
                norm,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            );
        }

        self.normalize_type_components(
            norm,
            projection_stack,
            projection_chain,
            projection_assoc_chain,
        )
    }

    fn normalize_projection_root(
        &mut self,
        projection_ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> TypeId {
        if let Some(ancestor_index) = projection_chain
            .iter()
            .position(|seen| *seen == projection_ty)
        {
            if let Some(assoc_ancestor_index) = projection_assoc_chain
                .iter()
                .position(|frame| frame.projection_ty == projection_ty && frame.saw_wrapper_growth)
            {
                let closing_assoc_id = projection_assoc_chain[assoc_ancestor_index].assoc_def_id;
                let cycle = projection_assoc_chain[assoc_ancestor_index..]
                    .iter()
                    .map(|frame| (frame.assoc_def_id, frame.projection_ty))
                    .chain(std::iter::once((closing_assoc_id, projection_ty)))
                    .collect::<Vec<_>>();
                self.emit_non_contractive_projection_cycle_diagnostic(&cycle);
                return TypeId::ERROR;
            }
            let cycle = projection_chain[ancestor_index..]
                .iter()
                .copied()
                .chain(std::iter::once(projection_ty))
                .collect::<Vec<_>>();
            self.emit_projection_cycle_diagnostic(&cycle);
            return TypeId::ERROR;
        }
        projection_chain.push(projection_ty);

        let result = match self.try_normalize_projection_type(
            projection_ty,
            projection_stack,
            projection_chain,
            projection_assoc_chain,
        ) {
            Some(next) if next == projection_ty => {
                self.emit_projection_cycle_diagnostic(&[projection_ty, projection_ty]);
                TypeId::ERROR
            }
            Some(next) => self.normalize_concrete_type_inner(
                next,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            ),
            None => projection_ty,
        };

        let popped = projection_chain.pop();
        debug_assert_eq!(popped, Some(projection_ty));
        result
    }

    fn normalize_assoc_body_type_inner(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> TypeId {
        let norm = self.type_registry.normalize(ty);
        if !matches!(
            self.type_registry.get(norm),
            TypeKind::Projection { .. }
                | TypeKind::Primitive(_)
                | TypeKind::Error
                | TypeKind::Module(_)
                | TypeKind::TypeVar(_)
                | TypeKind::Param(_)
        ) && let Some(frame) = projection_assoc_chain.last_mut()
        {
            frame.saw_wrapper_growth = true;
        }

        self.normalize_concrete_type_inner(
            norm,
            projection_stack,
            projection_chain,
            projection_assoc_chain,
        )
    }

    fn normalize_type_components(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> TypeId {
        let kind = self.type_registry.get(ty).clone();
        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_) => ty,
            _ => match kind {
                TypeKind::Simd { elem, lanes } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::Simd {
                            elem: new_elem,
                            lanes,
                        },
                    )
                }
                TypeKind::Pointer { is_mut, elem } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::Pointer {
                            is_mut,
                            elem: new_elem,
                        },
                    )
                }
                TypeKind::VolatilePtr { is_mut, elem } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::VolatilePtr {
                            is_mut,
                            elem: new_elem,
                        },
                    )
                }
                TypeKind::Slice { is_mut, elem } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::Slice {
                            is_mut,
                            elem: new_elem,
                        },
                    )
                }
                TypeKind::Array { elem, len } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::Array {
                            elem: new_elem,
                            len,
                        },
                    )
                }
                TypeKind::ArrayInfer { elem } => {
                    let new_elem = self.normalize_concrete_type_inner(
                        elem,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::ArrayInfer { elem: new_elem })
                }
                TypeKind::Def(def_id, args) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::Def(def_id, new_args))
                }
                TypeKind::Enum(def_id, args) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::Enum(def_id, new_args))
                }
                TypeKind::EnumPayload(def_id, args) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::EnumPayload(def_id, new_args))
                }
                TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    let new_assoc_bindings = assoc_bindings
                        .into_iter()
                        .map(|(assoc_def_id, assoc_ty)| {
                            (
                                assoc_def_id,
                                self.normalize_concrete_type_inner(
                                    assoc_ty,
                                    projection_stack,
                                    projection_chain,
                                    projection_assoc_chain,
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::TraitObject(def_id, new_args, new_assoc_bindings),
                    )
                }
                TypeKind::Projection { .. } => self.normalize_projection_root(
                    ty,
                    projection_stack,
                    projection_chain,
                    projection_assoc_chain,
                ),
                TypeKind::ClosureInterface { params, ret } => {
                    let new_params = params
                        .into_iter()
                        .map(|param| {
                            self.normalize_concrete_type_inner(
                                param,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            )
                        })
                        .collect::<Vec<_>>();
                    let new_ret = self.normalize_concrete_type_inner(
                        ret,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::ClosureInterface {
                            params: new_params,
                            ret: new_ret,
                        },
                    )
                }
                TypeKind::AnonymousState {
                    closure_node_id,
                    captures,
                    params,
                    ret,
                } => {
                    let new_captures = captures
                        .into_iter()
                        .map(|capture| {
                            self.normalize_concrete_type_inner(
                                capture,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            )
                        })
                        .collect::<Vec<_>>();
                    let new_params = params
                        .into_iter()
                        .map(|param| {
                            self.normalize_concrete_type_inner(
                                param,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            )
                        })
                        .collect::<Vec<_>>();
                    let new_ret = self.normalize_concrete_type_inner(
                        ret,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::AnonymousState {
                            closure_node_id,
                            captures: new_captures,
                            params: new_params,
                            ret: new_ret,
                        },
                    )
                }
                TypeKind::Alias(_, target) => self.normalize_concrete_type_inner(
                    target,
                    projection_stack,
                    projection_chain,
                    projection_assoc_chain,
                ),
                TypeKind::Associated(def_id, args) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::Associated(def_id, new_args))
                }
                TypeKind::Function {
                    params,
                    ret,
                    is_variadic,
                } => {
                    let new_params = params
                        .into_iter()
                        .map(|param| {
                            self.normalize_concrete_type_inner(
                                param,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            )
                        })
                        .collect::<Vec<_>>();
                    let new_ret = self.normalize_concrete_type_inner(
                        ret,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::Function {
                            params: new_params,
                            ret: new_ret,
                            is_variadic,
                        },
                    )
                }
                TypeKind::FnDef(def_id, args) => {
                    let new_args = self.normalize_generic_args(
                        &args,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::FnDef(def_id, new_args))
                }
                TypeKind::AnonymousStruct(is_extern, fields) => {
                    let new_fields = fields
                        .into_iter()
                        .map(|field| crate::ty::AnonymousField {
                            name: field.name,
                            ty: self.normalize_concrete_type_inner(
                                field.ty,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            ),
                        })
                        .collect::<Vec<_>>();
                    self.ctx_intern_if_changed(ty, TypeKind::AnonymousStruct(is_extern, new_fields))
                }
                TypeKind::AnonymousUnion(is_extern, fields) => {
                    let new_fields = fields
                        .into_iter()
                        .map(|field| crate::ty::AnonymousField {
                            name: field.name,
                            ty: self.normalize_concrete_type_inner(
                                field.ty,
                                projection_stack,
                                projection_chain,
                                projection_assoc_chain,
                            ),
                        })
                        .collect::<Vec<_>>();
                    self.ctx_intern_if_changed(ty, TypeKind::AnonymousUnion(is_extern, new_fields))
                }
                TypeKind::AnonymousEnum(enum_def) => {
                    let new_backing_ty = enum_def.backing_ty.map(|backing_ty| {
                        self.normalize_concrete_type_inner(
                            backing_ty,
                            projection_stack,
                            projection_chain,
                            projection_assoc_chain,
                        )
                    });
                    let new_variants = enum_def
                        .variants
                        .into_iter()
                        .map(|variant| crate::ty::AnonymousVariant {
                            name: variant.name,
                            name_span: variant.name_span,
                            payload_ty: variant.payload_ty.map(|payload_ty| {
                                self.normalize_concrete_type_inner(
                                    payload_ty,
                                    projection_stack,
                                    projection_chain,
                                    projection_assoc_chain,
                                )
                            }),
                            explicit_value: variant.explicit_value,
                        })
                        .collect::<Vec<_>>();
                    self.ctx_intern_if_changed(
                        ty,
                        TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                            backing_ty: new_backing_ty,
                            builtin: enum_def.builtin,
                            variants: new_variants,
                        }),
                    )
                }
                TypeKind::AnonymousEnumPayload(enum_ty) => {
                    let new_enum_ty = self.normalize_concrete_type_inner(
                        enum_ty,
                        projection_stack,
                        projection_chain,
                        projection_assoc_chain,
                    );
                    self.ctx_intern_if_changed(ty, TypeKind::AnonymousEnumPayload(new_enum_ty))
                }
                TypeKind::Primitive(_)
                | TypeKind::Error
                | TypeKind::Module(_)
                | TypeKind::TypeVar(_)
                | TypeKind::Param(_) => unreachable!(),
            },
        }
    }

    fn normalize_generic_args(
        &mut self,
        args: &[GenericArg],
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> Vec<GenericArg> {
        args.iter()
            .copied()
            .map(|arg| match arg {
                GenericArg::Type(ty) => GenericArg::Type(self.normalize_concrete_type_inner(
                    ty,
                    projection_stack,
                    projection_chain,
                    projection_assoc_chain,
                )),
                GenericArg::Const(value) => GenericArg::Const(value),
            })
            .collect()
    }

    fn ctx_intern_if_changed(&mut self, original: TypeId, rebuilt: TypeKind) -> TypeId {
        if self.type_registry.get(original) == &rebuilt {
            original
        } else {
            self.type_registry.intern(rebuilt)
        }
    }

    pub(crate) fn instantiate_assoc_projection_target(
        &mut self,
        assoc_def_id: DefId,
        assoc_args: &[GenericArg],
        assoc_ty: TypeId,
    ) -> TypeId {
        if assoc_ty == TypeId::ERROR || assoc_args.is_empty() {
            return assoc_ty;
        }

        let Some(assoc_generics) =
            self.defs
                .get(assoc_def_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.generics.clone()),
                    _ => None,
                })
        else {
            return assoc_ty;
        };
        if assoc_generics.len() != assoc_args.len() {
            debug_assert_eq!(assoc_generics.len(), assoc_args.len());
            return TypeId::ERROR;
        }

        let subst_map = assoc_generics
            .into_iter()
            .zip(assoc_args.iter().copied())
            .map(|(param, arg)| (param.name, arg))
            .collect::<FastHashMap<_, _>>();
        let mut subst = crate::checker::Substituter::new(&mut self.type_registry, &subst_map);
        subst.substitute(assoc_ty)
    }

    fn try_normalize_projection_type(
        &mut self,
        ty: TypeId,
        projection_stack: &mut Vec<TypeId>,
        projection_chain: &mut Vec<TypeId>,
        projection_assoc_chain: &mut Vec<ProjectionAssocFrame>,
    ) -> Option<TypeId> {
        let TypeKind::Projection {
            target,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        } = self.type_registry.get(ty).clone()
        else {
            return None;
        };

        // Kern currently treats associated-type projections as selecting a fully applied assoc
        // item. Assoc generic args are not a second projection layer; they are substitutions that
        // must be applied to the assoc target selected from a trait object or impl.
        if projection_stack.contains(&ty) {
            // Returning `None` here preserves the original projection node for the caller instead
            // of eagerly diagnosing. The surrounding concretenss gate decides later whether this
            // is a legitimately deferred generic projection or a concrete projection that must
            // now error.
            return None;
        }
        projection_stack.push(ty);

        let result = (|| {
            let target_norm = self.normalize_concrete_type_inner(
                target,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            );
            // Bounds already visible on a trait object are the strongest source of truth for a
            // projection: they come from the caller's environment or an earlier upcast step and
            // do not require re-selecting an impl.
            if let Some(assoc_ty) = crate::query::trait_object_assoc_from_hierarchy(
                self,
                target_norm,
                trait_def_id,
                &trait_args,
                assoc_def_id,
            ) {
                let assoc_ty =
                    self.instantiate_assoc_projection_target(assoc_def_id, &assoc_args, assoc_ty);
                return Some(self.normalize_concrete_type_inner(
                    assoc_ty,
                    projection_stack,
                    projection_chain,
                    projection_assoc_chain,
                ));
            }

            let candidates = if self.projection_is_fully_concrete(ty) {
                self.collect_specificity_maximal_projection_candidates(
                    target_norm,
                    trait_def_id,
                    &trait_args,
                    assoc_def_id,
                )
            } else {
                Vec::new()
            };
            let Some(candidate) = candidates.first().copied() else {
                // Only a fully concrete projection becomes a hard error here. Generic projections
                // are allowed to survive until later substitution or obligation solving provides
                // enough information to pick an impl or in-scope bound. This keeps normalization
                // from reverse-solving generic placeholders too early.
                if self.projection_is_fully_concrete(ty) {
                    self.emit_unresolved_projection_diagnostic(ty);
                    return Some(TypeId::ERROR);
                }
                return None;
            };
            if candidates.len() > 1 {
                // Coherence validation should reject incomparable overlapping impl heads, but
                // normalization can be queried before that pass has emitted its error. Refuse to
                // pick an arbitrary assoc result here, or downstream layout/checking may continue
                // with a fabricated type.
                if self.projection_is_fully_concrete(ty) {
                    self.emit_ambiguous_projection_diagnostic(ty, &candidates);
                    return Some(TypeId::ERROR);
                }
                return None;
            }
            let ApplicableProjectionCandidate {
                impl_assoc_id,
                assoc_ty,
                ..
            } = candidate;
            let assoc_ty =
                self.instantiate_assoc_projection_target(assoc_def_id, &assoc_args, assoc_ty);
            if let Some(ancestor_index) = projection_assoc_chain
                .iter()
                .position(|frame| frame.assoc_def_id == impl_assoc_id)
            {
                // Re-entering the same impl-associated body means normalization is no longer just
                // following projection syntax; the selected impl payload itself is recursively
                // expanding through wrappers or intermediate projections.
                let cycle = projection_assoc_chain[ancestor_index..]
                    .iter()
                    .map(|frame| (frame.assoc_def_id, frame.projection_ty))
                    .chain(std::iter::once((impl_assoc_id, ty)))
                    .collect::<Vec<_>>();
                self.emit_non_contractive_projection_cycle_diagnostic(&cycle);
                return Some(TypeId::ERROR);
            }
            projection_assoc_chain.push(ProjectionAssocFrame {
                assoc_def_id: impl_assoc_id,
                projection_ty: ty,
                saw_wrapper_growth: false,
            });
            let normalized_assoc_ty = self.normalize_assoc_body_type_inner(
                assoc_ty,
                projection_stack,
                projection_chain,
                projection_assoc_chain,
            );
            let popped_assoc = projection_assoc_chain.pop();
            debug_assert!(matches!(
                popped_assoc,
                Some(ProjectionAssocFrame {
                    assoc_def_id,
                    projection_ty,
                    ..
                }) if assoc_def_id == impl_assoc_id && projection_ty == ty
            ));
            Some(normalized_assoc_ty)
        })();

        let popped = projection_stack.pop();
        debug_assert_eq!(popped, Some(ty));
        result
    }

    fn collect_specificity_maximal_projection_candidates(
        &mut self,
        target_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        assoc_def_id: DefId,
    ) -> Vec<ApplicableProjectionCandidate> {
        // Snapshot the impl list so we can resolve signatures and compare specificity during the
        // scan without holding a borrow of the index across recursive queries.
        let trait_impls = self.trait_impl_ids_for_trait(trait_def_id);
        let mut applicable = Vec::new();

        for impl_id in trait_impls {
            if !matches!(self.defs.get(impl_id.0 as usize), Some(Def::Impl(_))) {
                continue;
            }

            {
                let mut resolver = TypeResolver::new(self);
                resolver.ensure_impl_signature_types_resolved(impl_id);
            }

            let Some(impl_args) = crate::query::resolve_trait_impl_head_obligation(
                self,
                target_ty,
                trait_def_id,
                trait_args,
                impl_id,
            ) else {
                continue;
            };

            let Some(inst_trait_ty) =
                crate::query::instantiate_impl_trait_ty(self, impl_id, &impl_args)
            else {
                continue;
            };

            let TypeKind::TraitObject(bound_trait_def_id, bound_trait_args, assoc_bindings) =
                self.type_registry.get(inst_trait_ty).clone()
            else {
                continue;
            };
            if bound_trait_def_id != trait_def_id || bound_trait_args != trait_args {
                continue;
            }

            let Some((_, assoc_ty)) = assoc_bindings
                .iter()
                .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
            else {
                continue;
            };
            let Some(impl_assoc_id) = self.impl_assoc_def_for_trait_assoc(impl_id, assoc_def_id)
            else {
                continue;
            };

            applicable.push(ApplicableProjectionCandidate {
                impl_id,
                impl_assoc_id,
                assoc_ty: *assoc_ty,
            });
        }

        // Keep every undominated candidate. In valid code this should collapse to one impl, but
        // during partial/erroneous analysis we still want projection normalization to observe the
        // same specialization frontier as proof search instead of whichever impl happened to be
        // scanned first.
        applicable
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !applicable.iter().enumerate().any(|(other_index, other)| {
                    other_index != *index
                        && matches!(
                            crate::query::compare_impl_specificity(
                                self,
                                other.impl_id,
                                candidate.impl_id,
                            ),
                            crate::query::ImplSpecificity::LeftMoreSpecific
                        )
                })
            })
            .map(|(_, candidate)| *candidate)
            .collect()
    }

    fn projection_is_fully_concrete(&self, ty: TypeId) -> bool {
        let TypeKind::Projection {
            target,
            trait_args,
            assoc_args,
            ..
        } = self
            .type_registry
            .get(self.type_registry.normalize(ty))
            .clone()
        else {
            return false;
        };

        !self.type_contains_params_or_vars(target)
            && trait_args
                .into_iter()
                .all(|arg| !self.generic_arg_contains_params_or_vars(arg))
            && assoc_args
                .into_iter()
                .all(|arg| !self.generic_arg_contains_params_or_vars(arg))
    }

    fn projection_diagnostic_span(&self, projection_ty: TypeId) -> Span {
        let projection_ty = self.type_registry.normalize(projection_ty);
        match self.type_registry.get(projection_ty) {
            TypeKind::Projection { assoc_def_id, .. } => self
                .defs
                .get(assoc_def_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.span),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => Span::default(),
        }
    }

    fn unresolved_projection_target_hint(&self, projection_ty: TypeId) -> Option<String> {
        let projection_ty = self.type_registry.normalize(projection_ty);
        let TypeKind::Projection {
            target,
            trait_def_id,
            assoc_def_id,
            ..
        } = self.type_registry.get(projection_ty).clone()
        else {
            return None;
        };

        let (trait_object_ty, pointer_prefix) = match self
            .type_registry
            .get(self.type_registry.normalize(target))
            .clone()
        {
            TypeKind::TraitObject(..) => (target, ""),
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. }
                if matches!(
                    self.type_registry.get(self.type_registry.normalize(elem)),
                    TypeKind::TraitObject(..)
                ) =>
            {
                (elem, "*")
            }
            _ => return None,
        };

        let TypeKind::TraitObject(target_trait_def_id, _, assoc_bindings) = self
            .type_registry
            .get(self.type_registry.normalize(trait_object_ty))
            .clone()
        else {
            return None;
        };
        if target_trait_def_id != trait_def_id
            || assoc_bindings
                .iter()
                .any(|(bound_assoc_def_id, _)| *bound_assoc_def_id == assoc_def_id)
        {
            return None;
        }

        let trait_name = self
            .defs
            .get(trait_def_id.0 as usize)
            .and_then(|def| def.name())
            .map(|name| self.resolve(name).to_string())
            .unwrap_or_else(|| "Trait".to_string());
        let assoc_name = self
            .defs
            .get(assoc_def_id.0 as usize)
            .and_then(|def| def.name())
            .map(|name| self.resolve(name).to_string())
            .unwrap_or_else(|| "Assoc".to_string());

        Some(format!(
            "trait object projections do not infer missing associated types; write the binding on the trait object itself, for example `{}{trait_name}[..., {assoc_name} = Concrete]`",
            pointer_prefix
        ))
    }

    fn type_contains_params_or_vars(&self, ty: TypeId) -> bool {
        let norm = self.type_registry.normalize(ty);
        // This gate decides whether an unresolved projection becomes a hard
        // error now or remains deferred until generics are instantiated, so it
        // must walk every container that can hide params/type vars. Existing
        // `TypeId::ERROR` placeholders also count as "not concrete enough" to
        // avoid layering a projection error on top of an earlier type failure.
        match self.type_registry.get(norm).clone() {
            TypeKind::Error | TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Simd { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_params_or_vars(elem),
            TypeKind::Array { elem, len, .. } => {
                self.type_contains_params_or_vars(elem)
                    || self.const_generic_contains_params_or_vars(len)
            }
            TypeKind::ArrayInfer { elem, .. } => self.type_contains_params_or_vars(elem),
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args
                .into_iter()
                .any(|arg| self.generic_arg_contains_params_or_vars(arg)),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter()
                    .any(|arg| self.generic_arg_contains_params_or_vars(arg))
                    || assoc_bindings
                        .into_iter()
                        .any(|(_, ty)| self.type_contains_params_or_vars(ty))
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.type_contains_params_or_vars(target)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.generic_arg_contains_params_or_vars(arg))
                    || assoc_args
                        .into_iter()
                        .any(|arg| self.generic_arg_contains_params_or_vars(arg))
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_params_or_vars(param))
                    || self.type_contains_params_or_vars(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_params_or_vars(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_params_or_vars(param))
                    || self.type_contains_params_or_vars(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_params_or_vars(field.ty)),
            TypeKind::AnonymousEnum(enum_def) => {
                enum_def
                    .backing_ty
                    .is_some_and(|backing_ty| self.type_contains_params_or_vars(backing_ty))
                    || enum_def.variants.into_iter().any(|variant| {
                        variant
                            .payload_ty
                            .is_some_and(|payload_ty| self.type_contains_params_or_vars(payload_ty))
                    })
            }
            _ => false,
        }
    }

    fn generic_arg_contains_params_or_vars(&self, arg: GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => ty == TypeId::ERROR || self.type_contains_params_or_vars(ty),
            GenericArg::Const(value) => self.const_generic_contains_params_or_vars(value),
        }
    }

    fn const_generic_contains_params_or_vars(&self, value: crate::ty::ConstGeneric) -> bool {
        self.type_registry.const_generic_contains_params(value)
    }

    pub(crate) fn emit_unresolved_projection_diagnostic(&mut self, projection_ty: TypeId) {
        let projection_ty = self.type_registry.normalize(projection_ty);
        let span = self.projection_diagnostic_span(projection_ty);
        let target_hint = self.unresolved_projection_target_hint(projection_ty);

        let mut diagnostic = self.struct_error(
            span,
            format!(
                "cannot normalize associated type projection `{}`",
                self.ty_to_string(projection_ty)
            ),
        )
        .with_hint("this projection is fully concrete here, so Kern expected one known associated-type result")
        .with_hint("normalization needs either an explicit trait-object binding or exactly one applicable impl")
        .with_hint("add a bound/where-clause proving the associated type, or make the receiver concrete enough to select an impl");
        if let Some(hint) = target_hint {
            diagnostic = diagnostic.with_hint(hint);
        }
        diagnostic.emit();
    }

    fn emit_ambiguous_projection_diagnostic(
        &mut self,
        projection_ty: TypeId,
        candidates: &[ApplicableProjectionCandidate],
    ) {
        let projection_ty = self.type_registry.normalize(projection_ty);
        let span = self.projection_diagnostic_span(projection_ty);
        let candidate_heads = candidates
            .iter()
            .filter_map(|candidate| {
                let Def::Impl(impl_def) = self.defs.get(candidate.impl_id.0 as usize)? else {
                    return None;
                };
                let Some(trait_ty_node) = &impl_def.trait_type else {
                    return None;
                };
                let target_ty = self.normalized_node_type_or_error(impl_def.target_type.id);
                let trait_ty = self.normalized_node_type_or_error(trait_ty_node.id);
                if target_ty == TypeId::ERROR || trait_ty == TypeId::ERROR {
                    return None;
                }
                Some(format!(
                    "`{}: {}`",
                    self.ty_to_string(target_ty),
                    self.ty_to_string(trait_ty)
                ))
            })
            .collect::<Vec<_>>();
        let candidate_spans = candidates
            .iter()
            .take(2)
            .filter_map(
                |candidate| match self.defs.get(candidate.impl_id.0 as usize) {
                    Some(Def::Impl(impl_def)) => Some(impl_def.span),
                    _ => None,
                },
            )
            .collect::<Vec<_>>();

        let mut diagnostic = self.struct_error(
            span,
            format!(
                "ambiguous associated type projection `{}`",
                self.ty_to_string(projection_ty)
            ),
        );
        diagnostic = diagnostic.with_hint(
            "this projection is fully concrete, but multiple equally specific impls still define the associated type",
        );
        diagnostic = diagnostic
            .with_hint("Kern refuses to guess one result here, because picking arbitrarily would hide an overlap bug");
        diagnostic = diagnostic
            .with_hint("fix the overlapping impl heads, or make one impl strictly more specific");
        if !candidate_heads.is_empty() {
            diagnostic = diagnostic.with_hint(format!(
                "conflicting impl heads: {}",
                candidate_heads.join(", ")
            ));
        }
        for span in candidate_spans {
            diagnostic = diagnostic
                .with_span_label(span, "equally specific impl applies to this projection");
        }
        diagnostic.emit();
    }

    pub(crate) fn emit_projection_cycle_diagnostic(&mut self, cycle: &[TypeId]) {
        // A single bad projection often gets revisited from layout, type checking, and method
        // lookup. Suppress duplicates so the first cycle report stays readable.
        if cycle.iter().any(|cycle_ty| {
            self.analysis
                .recursive_reports
                .reported_recursive_projection_types
                .contains(cycle_ty)
        }) {
            return;
        }
        for cycle_ty in cycle {
            self.analysis
                .recursive_reports
                .reported_recursive_projection_types
                .insert(*cycle_ty);
        }

        let head = cycle.first().copied().unwrap_or(TypeId::ERROR);
        let span = match self.type_registry.get(self.type_registry.normalize(head)) {
            TypeKind::Projection { assoc_def_id, .. } => self
                .defs
                .get(assoc_def_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.span),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => Span::default(),
        };
        let chain = cycle
            .iter()
            .map(|ty| self.ty_to_string(*ty))
            .collect::<Vec<_>>()
            .join(" -> ");

        self.struct_error(
            span,
            format!(
                "recursive associated type projection cycle detected while normalizing `{}`",
                self.ty_to_string(head)
            ),
        )
        .with_hint(format!("projection cycle: {}", chain))
        .with_hint("break the cycle by giving one associated type a concrete non-projecting result")
        .with_hint("if the recursion is intentional, move it behind an explicit pointer or another non-projecting indirection")
        .emit();
    }

    fn impl_assoc_def_for_trait_assoc(
        &self,
        impl_id: DefId,
        trait_assoc_id: DefId,
    ) -> Option<DefId> {
        let impl_def = self
            .defs
            .get(impl_id.0 as usize)
            .and_then(|def| match def {
                Def::Impl(def) => Some(def),
                _ => None,
            })?;
        impl_def.assoc_types.iter().copied().find(|assoc_id| {
            self.defs
                .get(assoc_id.0 as usize)
                .and_then(|def| match def {
                    Def::AssociatedType(def) => def.implemented_trait_assoc,
                    _ => None,
                })
                == Some(trait_assoc_id)
        })
    }

    pub(crate) fn emit_non_contractive_projection_cycle_diagnostic(
        &mut self,
        cycle: &[(DefId, TypeId)],
    ) {
        // This variant reports cycles that re-enter the same impl-provided associated type body,
        // even if the surface projection types along the way are not textually identical.
        if cycle.iter().any(|(assoc_id, _)| {
            self.analysis
                .recursive_reports
                .reported_recursive_projection_assoc_defs
                .contains(assoc_id)
        }) {
            return;
        }
        for (assoc_id, _) in cycle {
            self.analysis
                .recursive_reports
                .reported_recursive_projection_assoc_defs
                .insert(*assoc_id);
        }

        let head_assoc_id = cycle.first().map(|(assoc_id, _)| *assoc_id);
        let head_ty = cycle.first().map(|(_, ty)| *ty).unwrap_or(TypeId::ERROR);
        let span = head_assoc_id
            .and_then(|assoc_id| self.defs.get(assoc_id.0 as usize))
            .and_then(|def| match def {
                Def::AssociatedType(def) => Some(def.span),
                _ => None,
            })
            .unwrap_or_default();
        let chain = cycle
            .iter()
            .map(|(assoc_id, ty)| {
                let assoc_name = self
                    .defs
                    .get(assoc_id.0 as usize)
                    .and_then(|def| def.name())
                    .map(|name| self.resolve(name).to_string())
                    .unwrap_or_else(|| "<assoc>".to_string());
                format!("{} via {}", assoc_name, self.ty_to_string(*ty))
            })
            .collect::<Vec<_>>()
            .join(" -> ");

        self.struct_error(
            span,
            format!(
                "recursive associated type projection cycle detected while normalizing `{}`",
                self.ty_to_string(head_ty)
            ),
        )
        .with_hint(format!(
            "projection repeatedly re-enters the same impl-associated type: {}",
            chain
        ))
        .with_hint("break the cycle by giving one associated type a concrete non-projecting result")
        .with_hint("wrapping the recursive step in another constructor is not enough if normalization still re-enters the same assoc body")
        .emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{AssociatedTypeDef, TraitDef};
    use kernc_ast::Visibility;
    use kernc_utils::Session;

    #[test]
    fn projection_with_simd_target_param_is_not_fully_concrete() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "VectorTrait", "Item");
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let simd_target = ctx.type_registry.intern(TypeKind::Simd {
            elem: param_ty,
            lanes: 4,
        });
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: simd_target,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        assert!(!ctx.projection_is_fully_concrete(projection_ty));
    }

    #[test]
    fn projection_with_enum_payload_target_param_is_not_fully_concrete() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "PayloadTrait", "Item");
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let payload_target = ctx.type_registry.intern(TypeKind::EnumPayload(
            DefId(777),
            vec![GenericArg::Type(param_ty)],
        ));
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: payload_target,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        assert!(!ctx.projection_is_fully_concrete(projection_ty));
    }

    #[test]
    fn projection_with_anonymous_enum_backing_param_is_not_fully_concrete() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "EnumTrait", "Item");
        let param_t = ctx.intern("T");
        let param_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let enum_target =
            ctx.type_registry
                .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                    backing_ty: Some(param_ty),
                    builtin: None,
                    variants: Vec::new(),
                }));
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: enum_target,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        assert!(!ctx.projection_is_fully_concrete(projection_ty));
    }

    #[test]
    fn projection_with_error_target_is_not_fully_concrete() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "ErrorTrait", "Item");
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: TypeId::ERROR,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        assert!(!ctx.projection_is_fully_concrete(projection_ty));
    }

    #[test]
    fn unresolved_trait_object_projection_suggests_explicit_assoc_binding() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "Base", "Out");
        let arity = crate::ty::ConstGeneric::Value(crate::ty::ConstGenericValue {
            ty: TypeId::USIZE,
            kind: crate::ty::ConstGenericValueKind::Int(4),
        });
        let bare_trait_object = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            vec![GenericArg::Const(arity)],
            Vec::new(),
        ));
        let bare_trait_object_ptr = ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: bare_trait_object,
        });
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: bare_trait_object_ptr,
            trait_def_id: trait_id,
            trait_args: vec![GenericArg::Const(arity)],
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        assert_eq!(ctx.normalize_concrete_type(projection_ty), TypeId::ERROR);
        let messages = ctx
            .sess
            .diagnostics
            .iter()
            .map(|diag| diag.message.clone())
            .collect::<Vec<_>>();
        let hints = ctx
            .sess
            .diagnostics
            .iter()
            .flat_map(|diag| diag.hints.iter().cloned())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("cannot normalize associated type projection"))
        );
        assert!(hints.iter().any(|hint| {
            hint.contains("trait object projections do not infer missing associated types")
                && hint.contains("*Base[..., Out = Concrete]")
        }));
    }

    #[test]
    fn impl_assoc_lookup_ignores_same_named_assoc_from_other_trait() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_a_id, trait_a_assoc_id) = add_trait_with_assoc(&mut ctx, "TraitA", "Out");
        let (trait_b_id, _) = add_trait_with_assoc(&mut ctx, "TraitB", "Out");
        let impl_id = add_impl_with_assocs(
            &mut ctx,
            &[("Out", Some(trait_b_id)), ("Out", Some(trait_a_id))],
        );

        let found = ctx
            .impl_assoc_def_for_trait_assoc(impl_id, trait_a_assoc_id)
            .expect("expected impl assoc for TraitA::Out");
        let Def::AssociatedType(found_assoc) = &ctx.defs[found.0 as usize] else {
            panic!("expected associated type");
        };
        assert_eq!(found_assoc.parent_trait, Some(trait_a_id));
        assert_eq!(found_assoc.implemented_trait_assoc, Some(trait_a_assoc_id));
    }

    #[test]
    fn impl_assoc_lookup_requires_explicit_trait_assoc_link() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_a_id, trait_a_assoc_id) = add_trait_with_assoc(&mut ctx, "TraitA", "Out");
        let impl_id = add_impl_with_assocs(&mut ctx, &[("Out", Some(trait_a_id))]);

        let assoc_id = match &ctx.defs[impl_id.0 as usize] {
            Def::Impl(impl_def) => impl_def.assoc_types[0],
            _ => panic!("expected impl"),
        };
        let Def::AssociatedType(assoc_def) = &mut ctx.defs[assoc_id.0 as usize] else {
            panic!("expected associated type");
        };
        assoc_def.implemented_trait_assoc = None;

        assert_eq!(
            ctx.impl_assoc_def_for_trait_assoc(impl_id, trait_a_assoc_id),
            None
        );
    }

    #[test]
    fn ambiguous_projection_candidates_do_not_silently_pick_one_impl() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let (trait_id, assoc_id) = add_trait_with_assoc(&mut ctx, "TraitA", "Out");
        let param_t = ctx.intern("T");
        let generic_target_ty = ctx.type_registry.intern(TypeKind::Param(param_t));
        let generics = [kernc_ast::GenericParam {
            name: param_t,
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        }];
        add_trait_impl_with_assoc_target(
            &mut ctx,
            &generics,
            generic_target_ty,
            trait_id,
            assoc_id,
            TypeId::I32,
        );
        add_trait_impl_with_assoc_target(
            &mut ctx,
            &generics,
            generic_target_ty,
            trait_id,
            assoc_id,
            TypeId::BOOL,
        );

        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: TypeId::I32,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: Vec::new(),
        });

        let candidates = ctx.collect_specificity_maximal_projection_candidates(
            TypeId::I32,
            trait_id,
            &[],
            assoc_id,
        );
        assert_eq!(candidates.len(), 2);
        assert_eq!(ctx.normalize_concrete_type(projection_ty), TypeId::ERROR);
    }

    #[test]
    fn trait_object_projection_substitutes_assoc_generic_args() {
        let mut session = Session::new();
        let mut ctx = SemaContext::new(&mut session);
        let mapper_name = ctx.intern("Mapper");
        let apply_name = ctx.intern("Apply");
        let trait_id_value = DefId(ctx.defs.len() as u32);
        let trait_id = ctx.add_def(Def::Trait(TraitDef {
            id: trait_id_value,
            name: mapper_name,
            vis: Visibility::Private,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            supertraits: Vec::new(),
            resolved_supertraits: Vec::new(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            resolved_methods: Vec::new(),
            span: Span::default(),
            is_builtin: false,
            docs: None,
        }));
        let assoc_generic = kernc_ast::GenericParam {
            name: ctx.intern("T"),
            span: Span::default(),
            kind: kernc_ast::GenericParamKind::Type,
        };
        let assoc_generic_ty = ctx
            .type_registry
            .intern(TypeKind::Param(assoc_generic.name));
        let assoc_id_value = DefId(ctx.defs.len() as u32);
        let assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: assoc_id_value,
            name: apply_name,
            parent_trait: Some(trait_id),
            parent_impl: None,
            implemented_trait_assoc: None,
            is_imported: false,
            generics: vec![assoc_generic.clone()],
            bounds: Vec::new(),
            where_clauses: Vec::new(),
            target: None,
            resolved_bounds: Vec::new(),
            span: Span::default(),
            docs: None,
        }));
        let Def::Trait(trait_def) = &mut ctx.defs[trait_id.0 as usize] else {
            panic!("expected trait");
        };
        trait_def.assoc_types.push(assoc_id);

        let trait_object_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            Vec::new(),
            vec![(assoc_id, assoc_generic_ty)],
        ));
        let projection_ty = ctx.type_registry.intern(TypeKind::Projection {
            target: trait_object_ty,
            trait_def_id: trait_id,
            trait_args: Vec::new(),
            assoc_def_id: assoc_id,
            assoc_args: vec![GenericArg::Type(TypeId::I32)],
        });

        assert_eq!(ctx.normalize_concrete_type(projection_ty), TypeId::I32);
    }

    fn add_trait_with_assoc(
        ctx: &mut SemaContext<'_>,
        trait_name: &str,
        assoc_name: &str,
    ) -> (DefId, DefId) {
        let trait_id_value = DefId(ctx.defs.len() as u32);
        let trait_name = ctx.intern(trait_name);
        let trait_id = ctx.add_def(Def::Trait(TraitDef {
            id: trait_id_value,
            name: trait_name,
            vis: Visibility::Private,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            supertraits: Vec::new(),
            resolved_supertraits: Vec::new(),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            resolved_methods: Vec::new(),
            span: Span::default(),
            is_builtin: false,
            docs: None,
        }));
        let assoc_id_value = DefId(ctx.defs.len() as u32);
        let assoc_name = ctx.intern(assoc_name);
        let assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: assoc_id_value,
            name: assoc_name,
            parent_trait: Some(trait_id),
            parent_impl: None,
            implemented_trait_assoc: None,
            is_imported: false,
            generics: Vec::new(),
            bounds: Vec::new(),
            where_clauses: Vec::new(),
            target: None,
            resolved_bounds: Vec::new(),
            span: Span::default(),
            docs: None,
        }));

        let Def::Trait(trait_def) = &mut ctx.defs[trait_id.0 as usize] else {
            panic!("expected trait");
        };
        trait_def.assoc_types.push(assoc_id);
        (trait_id, assoc_id)
    }

    fn add_impl_with_assocs(
        ctx: &mut SemaContext<'_>,
        assoc_specs: &[(&str, Option<DefId>)],
    ) -> DefId {
        let impl_id_value = DefId(ctx.defs.len() as u32);
        let impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: impl_id_value,
            parent_module: None,
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            target_type: kernc_ast::TypeNode {
                id: NodeId(0),
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            },
            trait_type: None,
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let mut assoc_ids = Vec::new();
        for (name, parent_trait) in assoc_specs {
            let assoc_id_value = DefId(ctx.defs.len() as u32);
            let assoc_name = ctx.intern(name);
            let implemented_trait_assoc = parent_trait.and_then(|trait_id| {
                let Def::Trait(trait_def) = &ctx.defs[trait_id.0 as usize] else {
                    return None;
                };
                trait_def
                    .assoc_types
                    .iter()
                    .copied()
                    .find(|trait_assoc_id| {
                        matches!(
                            &ctx.defs[trait_assoc_id.0 as usize],
                            Def::AssociatedType(trait_assoc) if trait_assoc.name == assoc_name
                        )
                    })
            });
            let assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
                id: assoc_id_value,
                name: assoc_name,
                parent_trait: *parent_trait,
                parent_impl: Some(impl_id),
                implemented_trait_assoc,
                is_imported: false,
                generics: Vec::new(),
                bounds: Vec::new(),
                where_clauses: Vec::new(),
                target: None,
                resolved_bounds: Vec::new(),
                span: Span::default(),
                docs: None,
            }));
            assoc_ids.push(assoc_id);
        }

        let Def::Impl(impl_def) = &mut ctx.defs[impl_id.0 as usize] else {
            panic!("expected impl");
        };
        impl_def.assoc_types = assoc_ids;
        impl_id
    }

    fn add_trait_impl_with_assoc_target(
        ctx: &mut SemaContext<'_>,
        generics: &[kernc_ast::GenericParam],
        target_ty: TypeId,
        trait_id: DefId,
        trait_assoc_id: DefId,
        assoc_target_ty: TypeId,
    ) -> DefId {
        let base = ctx.defs.len() as u32;
        let target_node_id = NodeId(base + 1);
        let trait_node_id = NodeId(base + 2);
        let impl_id_value = DefId(base);
        let impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: impl_id_value,
            parent_module: None,
            is_imported: false,
            generics: generics.to_vec(),
            where_clauses: Vec::new(),
            target_type: kernc_ast::TypeNode {
                id: target_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            },
            trait_type: Some(kernc_ast::TypeNode {
                id: trait_node_id,
                kind: kernc_ast::TypeKind::Infer,
                span: Span::default(),
            }),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let assoc_name = match &ctx.defs[trait_assoc_id.0 as usize] {
            Def::AssociatedType(def) => def.name,
            _ => panic!("expected trait associated type"),
        };
        let impl_assoc_id_value = DefId(ctx.defs.len() as u32);
        let impl_assoc_id = ctx.add_def(Def::AssociatedType(AssociatedTypeDef {
            id: impl_assoc_id_value,
            name: assoc_name,
            parent_trait: Some(trait_id),
            parent_impl: Some(impl_id),
            implemented_trait_assoc: Some(trait_assoc_id),
            is_imported: false,
            generics: Vec::new(),
            bounds: Vec::new(),
            where_clauses: Vec::new(),
            target: None,
            resolved_bounds: Vec::new(),
            span: Span::default(),
            docs: None,
        }));

        let trait_ty = ctx.type_registry.intern(TypeKind::TraitObject(
            trait_id,
            Vec::new(),
            vec![(trait_assoc_id, assoc_target_ty)],
        ));
        ctx.facts.node_types.insert(target_node_id, target_ty);
        ctx.facts.node_types.insert(trait_node_id, trait_ty);
        ctx.impl_index.trait_impls.push(impl_id);

        let Def::Impl(impl_def) = &mut ctx.defs[impl_id.0 as usize] else {
            panic!("expected impl");
        };
        impl_def.assoc_types.push(impl_assoc_id);
        impl_id
    }
}
