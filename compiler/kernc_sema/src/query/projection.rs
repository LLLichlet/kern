use super::*;

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    fn project_unbound_trait_assoc_arg(
        &mut self,
        arg: crate::ty::GenericArg,
        receiver_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
    ) -> crate::ty::GenericArg {
        match arg {
            crate::ty::GenericArg::Type(ty) => crate::ty::GenericArg::Type(
                self.project_unbound_trait_assoc_types(ty, receiver_ty, trait_def_id, trait_args),
            ),
            crate::ty::GenericArg::Const(value) => crate::ty::GenericArg::Const(value),
        }
    }

    pub(super) fn materialize_trait_assoc_placeholders(
        &mut self,
        ty: TypeId,
        receiver_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
        assoc_bindings: &[(DefId, TypeId)],
    ) -> TypeId {
        let assoc_binding_map = assoc_bindings
            .iter()
            .copied()
            .collect::<FastHashMap<_, _>>();
        let substituted = crate::ty::substitute_associated_types(
            &mut self.ctx.type_registry,
            &self.ctx.defs,
            ty,
            &assoc_binding_map,
        );
        self.project_unbound_trait_assoc_types(substituted, receiver_ty, trait_def_id, trait_args)
    }

    pub(super) fn project_unbound_trait_assoc_types(
        &mut self,
        ty: TypeId,
        receiver_ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[crate::ty::GenericArg],
    ) -> TypeId {
        let kind = self.ctx.type_registry.get(ty).clone();
        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_) => ty,
            // Trait method signatures and assoc targets are rewritten structurally. SIMD element
            // types therefore must stay in the walk even if surface syntax usually spells them as
            // builtin names like `i32x4`.
            TypeKind::Simd { elem, lanes } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Simd {
                    elem: new_elem,
                    lanes,
                })
            }
            TypeKind::Associated(assoc_def_id, assoc_args) => {
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect::<Vec<_>>();
                let belongs_to_trait = matches!(
                    self.ctx.defs.get(assoc_def_id.0 as usize),
                    Some(Def::AssociatedType(assoc_def))
                        if assoc_def.parent_trait == Some(trait_def_id)
                );
                if belongs_to_trait {
                    self.ctx.type_registry.intern(TypeKind::Projection {
                        target: receiver_ty,
                        trait_def_id,
                        trait_args: trait_args.to_vec(),
                        assoc_def_id,
                        assoc_args: new_assoc_args,
                    })
                } else {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Associated(assoc_def_id, new_assoc_args))
                }
            }
            TypeKind::Pointer { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Slice { is_mut, elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Array { elem, len } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Array {
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::ArrayInfer { elem } => {
                let new_elem = self.project_unbound_trait_assoc_types(
                    elem,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::ArrayInfer { elem: new_elem })
            }
            TypeKind::Range {
                start,
                end,
                is_inclusive,
            } => {
                let start = start.map(|start| {
                    self.project_unbound_trait_assoc_types(
                        start,
                        receiver_ty,
                        trait_def_id,
                        trait_args,
                    )
                });
                let end = end.map(|end| {
                    self.project_unbound_trait_assoc_types(
                        end,
                        receiver_ty,
                        trait_def_id,
                        trait_args,
                    )
                });
                self.ctx.type_registry.intern(TypeKind::Range {
                    start,
                    end,
                    is_inclusive,
                })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: new_ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Enum(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Enum(def_id, new_args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, assoc_ty)| {
                        (
                            assoc_def_id,
                            self.project_unbound_trait_assoc_types(
                                assoc_ty,
                                receiver_ty,
                                trait_def_id,
                                trait_args,
                            ),
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::TraitObject(
                    def_id,
                    new_args,
                    new_assoc_bindings,
                ))
            }
            TypeKind::Projection {
                target,
                trait_def_id: projection_trait_def_id,
                trait_args: projection_trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let new_target = self.project_unbound_trait_assoc_types(
                    target,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                let new_trait_args = projection_trait_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx.type_registry.intern(TypeKind::Projection {
                    target: new_target,
                    trait_def_id: projection_trait_def_id,
                    trait_args: new_trait_args,
                    assoc_def_id,
                    assoc_args: new_assoc_args,
                })
            }
            TypeKind::ClosureInterface { params, ret } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: new_params,
                    ret: new_ret,
                })
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
                        self.project_unbound_trait_assoc_types(
                            capture,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.project_unbound_trait_assoc_types(
                            param,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                let new_ret = self.project_unbound_trait_assoc_types(
                    ret,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx.type_registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures: new_captures,
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.project_unbound_trait_assoc_types(
                    target,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.project_unbound_trait_assoc_arg(
                            arg,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.project_unbound_trait_assoc_types(
                            field.ty,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(is_extern, new_fields))
            }
            TypeKind::AnonymousUnion(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.project_unbound_trait_assoc_types(
                            field.ty,
                            receiver_ty,
                            trait_def_id,
                            trait_args,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(is_extern, new_fields))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let new_backing_ty = enum_def.backing_ty.map(|backing_ty| {
                    self.project_unbound_trait_assoc_types(
                        backing_ty,
                        receiver_ty,
                        trait_def_id,
                        trait_args,
                    )
                });
                let new_variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| crate::ty::AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant.payload_ty.map(|payload_ty| {
                            self.project_unbound_trait_assoc_types(
                                payload_ty,
                                receiver_ty,
                                trait_def_id,
                                trait_args,
                            )
                        }),
                        explicit_value: variant.explicit_value,
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: new_backing_ty,
                        builtin: enum_def.builtin,
                        variants: new_variants,
                    }))
            }
            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let new_enum_ty = self.project_unbound_trait_assoc_types(
                    enum_ty,
                    receiver_ty,
                    trait_def_id,
                    trait_args,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(new_enum_ty))
            }
        }
    }

    pub(super) fn apply_generics_to_field(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[crate::ty::GenericArg],
        node_id: kernc_utils::NodeId,
    ) -> TypeId {
        if generics.is_empty() || args.is_empty() {
            return self.ctx.node_type_or_error(node_id);
        }

        let cache_key = (node_id, args.to_vec());
        if let Some(&field_ty) = self
            .ctx
            .analysis
            .query_caches
            .field_type_subst_cache
            .get(&cache_key)
        {
            return field_ty;
        }

        let mut field_ty = self.ctx.node_type_or_error(node_id);

        let mut map = FastHashMap::default();
        for (index, param) in generics.iter().enumerate() {
            if let Some(arg) = args.get(index).copied() {
                map.insert(param.name, arg);
            }
        }
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
        field_ty = subst.substitute(field_ty);
        self.ctx
            .analysis
            .query_caches
            .field_type_subst_cache
            .insert(cache_key, field_ty);

        field_ty
    }
}
