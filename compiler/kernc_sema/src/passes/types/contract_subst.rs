use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub(super) fn instantiate_trait_assoc_contract_ty(
        &mut self,
        ty: TypeId,
        generic_args: &HashMap<SymbolId, GenericArg>,
        assoc_targets: &HashMap<DefId, TypeId>,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> TypeId {
        let ty = {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, generic_args);
            subst.substitute(ty)
        };
        let ty = crate::ty::substitute_associated_types(
            &mut self.ctx.type_registry,
            &self.ctx.defs,
            ty,
            assoc_targets,
        );
        self.substitute_trait_assoc_contract_self(ty, trait_def_id, trait_args, self_ty)
    }

    fn substitute_trait_assoc_contract_self_arg(
        &mut self,
        arg: GenericArg,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(self.substitute_trait_assoc_contract_self(
                ty,
                trait_def_id,
                trait_args,
                self_ty,
            )),
            GenericArg::Const(value) => GenericArg::Const(value),
        }
    }

    fn substitute_trait_assoc_contract_self(
        &mut self,
        ty: TypeId,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> TypeId {
        let kind = self.ctx.type_registry.get(ty).clone();
        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_)
            | TypeKind::Param(_) => ty,
            TypeKind::Simd { elem, lanes } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Simd {
                    elem: new_elem,
                    lanes,
                })
            }
            TypeKind::Associated(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, new_args))
            }
            TypeKind::Pointer { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Slice { is_mut, elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Array { elem, len } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::Array {
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::ArrayInfer { elem } => {
                let new_elem = self.substitute_trait_assoc_contract_self(
                    elem,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::ArrayInfer { elem: new_elem })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
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
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
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
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
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
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.substitute_trait_assoc_contract_self(
                    target,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect::<Vec<_>>();
                let new_assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, assoc_ty)| {
                        (
                            assoc_def_id,
                            self.substitute_trait_assoc_contract_self(
                                assoc_ty,
                                trait_def_id,
                                trait_args,
                                self_ty,
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                if def_id == trait_def_id
                    && new_assoc_bindings.is_empty()
                    && new_args.as_slice() == trait_args
                {
                    return self_ty;
                }
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
                let new_target = self.substitute_trait_assoc_contract_self(
                    target,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                let new_trait_args = projection_trait_args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| {
                        self.substitute_trait_assoc_contract_self_arg(
                            arg,
                            trait_def_id,
                            trait_args,
                            self_ty,
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
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
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
                        self.substitute_trait_assoc_contract_self(
                            capture,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_params = params
                    .into_iter()
                    .map(|param| {
                        self.substitute_trait_assoc_contract_self(
                            param,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        )
                    })
                    .collect();
                let new_ret = self.substitute_trait_assoc_contract_self(
                    ret,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx.type_registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures: new_captures,
                    params: new_params,
                    ret: new_ret,
                })
            }
            TypeKind::AnonymousStruct(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|field| crate::ty::AnonymousField {
                        name: field.name,
                        ty: self.substitute_trait_assoc_contract_self(
                            field.ty,
                            trait_def_id,
                            trait_args,
                            self_ty,
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
                        ty: self.substitute_trait_assoc_contract_self(
                            field.ty,
                            trait_def_id,
                            trait_args,
                            self_ty,
                        ),
                    })
                    .collect();
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(is_extern, new_fields))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let new_backing_ty = enum_def.backing_ty.map(|backing_ty| {
                    self.substitute_trait_assoc_contract_self(
                        backing_ty,
                        trait_def_id,
                        trait_args,
                        self_ty,
                    )
                });
                let new_variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| crate::ty::AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant.payload_ty.map(|payload_ty| {
                            self.substitute_trait_assoc_contract_self(
                                payload_ty,
                                trait_def_id,
                                trait_args,
                                self_ty,
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
                let substituted = self.substitute_trait_assoc_contract_self(
                    enum_ty,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnumPayload(substituted))
            }
        }
    }

    pub(super) fn push_instantiated_where_bounds(
        &mut self,
        where_clauses: &[ast::WhereClause],
        generic_args: &HashMap<SymbolId, GenericArg>,
        assoc_targets: &HashMap<DefId, TypeId>,
        trait_def_id: DefId,
        trait_args: &[GenericArg],
        self_ty: TypeId,
    ) -> usize {
        let prev_bounds_len = self.ctx.analysis.active_bounds.len();
        for clause in where_clauses {
            let target_ty = self.ctx.node_type_or_error(clause.target_ty.id);
            let instantiated_target = self.instantiate_trait_assoc_contract_ty(
                target_ty,
                generic_args,
                assoc_targets,
                trait_def_id,
                trait_args,
                self_ty,
            );
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                let bound_ty = self.ctx.node_type_or_error(bound.id);
                let instantiated_bound = self.instantiate_trait_assoc_contract_ty(
                    bound_ty,
                    generic_args,
                    assoc_targets,
                    trait_def_id,
                    trait_args,
                    self_ty,
                );
                bounds.push(self.ctx.type_registry.normalize(instantiated_bound));
            }
            self.ctx.analysis.active_bounds.push((
                self.ctx.type_registry.normalize(instantiated_target),
                bounds,
            ));
        }
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }
        prev_bounds_len
    }
}
