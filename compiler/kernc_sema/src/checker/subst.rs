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
            TypeKind::TraitObject(def_id, args) => {
                let new_args = args.into_iter().map(|a| self.substitute(a)).collect();
                self.registry
                    .intern(TypeKind::TraitObject(def_id, new_args))
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
