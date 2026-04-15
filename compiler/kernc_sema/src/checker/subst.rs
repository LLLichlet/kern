use crate::def::DefId;
use crate::ty::{TypeId, TypeKind, TypeRegistry};
use kernc_utils::SymbolId;
use std::collections::HashMap;
use std::hash::BuildHasher;

pub trait TypeSubstMap {
    fn is_empty(&self) -> bool;
    fn get(&self, name: &SymbolId) -> Option<&TypeId>;
}

impl<S> TypeSubstMap for HashMap<SymbolId, TypeId, S>
where
    S: BuildHasher,
{
    fn is_empty(&self) -> bool {
        HashMap::is_empty(self)
    }

    fn get(&self, name: &SymbolId) -> Option<&TypeId> {
        HashMap::get(self, name)
    }
}

pub struct Substituter<'a, M> {
    registry: &'a mut TypeRegistry,
    map: &'a M,
}

impl<'a, M> Substituter<'a, M>
where
    M: TypeSubstMap,
{
    pub fn new(registry: &'a mut TypeRegistry, map: &'a M) -> Self {
        Self { registry, map }
    }

    pub fn substitute(&mut self, ty: TypeId) -> TypeId {
        if ty == TypeId::ERROR || self.map.is_empty() {
            return ty;
        }

        let kind = self.registry.get(ty).clone();

        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_) => ty,

            // Replace matching generic parameters with their instantiated types.
            TypeKind::Param(name) => {
                if let Some(&new_ty) = self.map.get(&name) {
                    new_ty
                } else {
                    ty
                }
            }
            TypeKind::Associated(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Associated(def_id, new_args))
            }

            TypeKind::Pointer { is_mut, elem } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::VolatilePtr { is_mut, elem } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::VolatilePtr {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Slice { is_mut, elem } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::Slice {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Array { is_mut, elem, len } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::Array {
                    is_mut,
                    elem: new_elem,
                    len,
                })
            }
            TypeKind::ArrayInfer { is_mut, elem } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::ArrayInfer {
                    is_mut,
                    elem: new_elem,
                })
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let new_params = params.into_iter().map(|p| self.substitute(p)).collect();
                let new_ret = self.substitute(ret);
                self.registry.intern(TypeKind::Function {
                    params: new_params,
                    ret: new_ret,
                    is_variadic,
                })
            }
            TypeKind::Def(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Enum(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Enum(def_id, new_args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.substitute(target);
                self.registry.intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                let new_assoc_bindings = assoc_bindings
                    .into_iter()
                    .map(|(assoc_def_id, ty)| (assoc_def_id, self.substitute(ty)))
                    .collect();
                self.registry
                    .intern(TypeKind::TraitObject(def_id, new_args, new_assoc_bindings))
            }
            TypeKind::Projection {
                target,
                trait_def_id,
                trait_args,
                assoc_def_id,
                assoc_args,
            } => {
                let new_target = self.substitute(target);
                let new_trait_args = trait_args.into_iter().map(|a| self.substitute(a)).collect();
                let new_assoc_args = assoc_args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry.intern(TypeKind::Projection {
                    target: new_target,
                    trait_def_id,
                    trait_args: new_trait_args,
                    assoc_def_id,
                    assoc_args: new_assoc_args,
                })
            }
            TypeKind::ClosureInterface { params, ret } => {
                let new_params = params.into_iter().map(|p| self.substitute(p)).collect();
                let new_ret = self.substitute(ret);
                self.registry.intern(TypeKind::ClosureInterface {
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
                let new_caps = captures.into_iter().map(|c| self.substitute(c)).collect();
                let new_params = params.into_iter().map(|p| self.substitute(p)).collect();
                let new_ret = self.substitute(ret);
                self.registry.intern(TypeKind::AnonymousState {
                    closure_node_id,
                    captures: new_caps,
                    params: new_params,
                    ret: new_ret,
                })
            }

            TypeKind::AnonymousStruct(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|f| crate::ty::AnonymousField {
                        name: f.name,
                        ty: self.substitute(f.ty),
                    })
                    .collect();
                self.registry
                    .intern(TypeKind::AnonymousStruct(is_extern, new_fields))
            }

            TypeKind::AnonymousUnion(is_extern, fields) => {
                let new_fields = fields
                    .into_iter()
                    .map(|f| crate::ty::AnonymousField {
                        name: f.name,
                        ty: self.substitute(f.ty),
                    })
                    .collect();
                self.registry
                    .intern(TypeKind::AnonymousUnion(is_extern, new_fields))
            }

            TypeKind::AnonymousEnum(enum_def) => {
                let new_backing_ty = enum_def.backing_ty.map(|ty| self.substitute(ty));
                let new_variants = enum_def
                    .variants
                    .into_iter()
                    .map(|variant| crate::ty::AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty: variant.payload_ty.map(|ty| self.substitute(ty)),
                        explicit_value: variant.explicit_value,
                    })
                    .collect();
                self.registry
                    .intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                        backing_ty: new_backing_ty,
                        builtin: enum_def.builtin,
                        variants: new_variants,
                    }))
            }

            TypeKind::AnonymousEnumPayload(enum_ty) => {
                let substituted = self.substitute(enum_ty);
                self.registry
                    .intern(TypeKind::AnonymousEnumPayload(substituted))
            }
        }
    }
}

pub fn substitute_associated_types<S>(
    registry: &mut TypeRegistry,
    ty: TypeId,
    map: &HashMap<DefId, TypeId, S>,
) -> TypeId
where
    S: BuildHasher,
{
    if ty == TypeId::ERROR || map.is_empty() {
        return ty;
    }

    let kind = registry.get(ty).clone();
    match kind {
        TypeKind::Primitive(_)
        | TypeKind::Simd { .. }
        | TypeKind::Error
        | TypeKind::Module(_)
        | TypeKind::TypeVar(_)
        | TypeKind::Param(_) => ty,

        TypeKind::Associated(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect::<Vec<_>>();
            if new_args.is_empty()
                && let Some(&bound_ty) = map.get(&def_id)
            {
                return bound_ty;
            }
            registry.intern(TypeKind::Associated(def_id, new_args))
        }

        TypeKind::Pointer { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, elem, map);
            registry.intern(TypeKind::Pointer {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::VolatilePtr { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, elem, map);
            registry.intern(TypeKind::VolatilePtr {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::Slice { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, elem, map);
            registry.intern(TypeKind::Slice {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::Array { is_mut, elem, len } => {
            let new_elem = substitute_associated_types(registry, elem, map);
            registry.intern(TypeKind::Array {
                is_mut,
                elem: new_elem,
                len,
            })
        }
        TypeKind::ArrayInfer { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, elem, map);
            registry.intern(TypeKind::ArrayInfer {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::Function {
            params,
            ret,
            is_variadic,
        } => {
            let new_params = params
                .into_iter()
                .map(|param| substitute_associated_types(registry, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, ret, map);
            registry.intern(TypeKind::Function {
                params: new_params,
                ret: new_ret,
                is_variadic,
            })
        }
        TypeKind::Def(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            registry.intern(TypeKind::Def(def_id, new_args))
        }
        TypeKind::Enum(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            registry.intern(TypeKind::Enum(def_id, new_args))
        }
        TypeKind::EnumPayload(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            registry.intern(TypeKind::EnumPayload(def_id, new_args))
        }
        TypeKind::FnDef(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            registry.intern(TypeKind::FnDef(def_id, new_args))
        }
        TypeKind::Alias(name, target) => {
            let new_target = substitute_associated_types(registry, target, map);
            registry.intern(TypeKind::Alias(name, new_target))
        }
        TypeKind::TraitObject(def_id, args, assoc_bindings) => {
            let new_args = args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            let new_assoc_bindings = assoc_bindings
                .into_iter()
                .map(|(assoc_def_id, ty)| {
                    (assoc_def_id, substitute_associated_types(registry, ty, map))
                })
                .collect();
            registry.intern(TypeKind::TraitObject(def_id, new_args, new_assoc_bindings))
        }
        TypeKind::Projection {
            target,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        } => {
            let new_target = substitute_associated_types(registry, target, map);
            let new_trait_args = trait_args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            let new_assoc_args = assoc_args
                .into_iter()
                .map(|arg| substitute_associated_types(registry, arg, map))
                .collect();
            registry.intern(TypeKind::Projection {
                target: new_target,
                trait_def_id,
                trait_args: new_trait_args,
                assoc_def_id,
                assoc_args: new_assoc_args,
            })
        }
        TypeKind::ClosureInterface { params, ret } => {
            let new_params = params
                .into_iter()
                .map(|param| substitute_associated_types(registry, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, ret, map);
            registry.intern(TypeKind::ClosureInterface {
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
                .map(|capture| substitute_associated_types(registry, capture, map))
                .collect();
            let new_params = params
                .into_iter()
                .map(|param| substitute_associated_types(registry, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, ret, map);
            registry.intern(TypeKind::AnonymousState {
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
                    ty: substitute_associated_types(registry, field.ty, map),
                })
                .collect();
            registry.intern(TypeKind::AnonymousStruct(is_extern, new_fields))
        }

        TypeKind::AnonymousUnion(is_extern, fields) => {
            let new_fields = fields
                .into_iter()
                .map(|field| crate::ty::AnonymousField {
                    name: field.name,
                    ty: substitute_associated_types(registry, field.ty, map),
                })
                .collect();
            registry.intern(TypeKind::AnonymousUnion(is_extern, new_fields))
        }

        TypeKind::AnonymousEnum(enum_def) => {
            let new_backing_ty = enum_def
                .backing_ty
                .map(|backing_ty| substitute_associated_types(registry, backing_ty, map));
            let new_variants = enum_def
                .variants
                .into_iter()
                .map(|variant| crate::ty::AnonymousVariant {
                    name: variant.name,
                    name_span: variant.name_span,
                    payload_ty: variant
                        .payload_ty
                        .map(|payload_ty| substitute_associated_types(registry, payload_ty, map)),
                    explicit_value: variant.explicit_value,
                })
                .collect();
            registry.intern(TypeKind::AnonymousEnum(crate::ty::AnonymousEnum {
                backing_ty: new_backing_ty,
                builtin: enum_def.builtin,
                variants: new_variants,
            }))
        }

        TypeKind::AnonymousEnumPayload(enum_ty) => {
            let substituted = substitute_associated_types(registry, enum_ty, map);
            registry.intern(TypeKind::AnonymousEnumPayload(substituted))
        }
    }
}
