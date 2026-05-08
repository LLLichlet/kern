use crate::def::{Def, DefId};
use crate::ty::{ConstGeneric, GenericArg, TypeId, TypeKind, TypeRegistry};
use kernc_utils::SymbolId;
use std::collections::HashMap;
use std::hash::BuildHasher;

pub trait TypeSubstMap {
    fn is_empty(&self) -> bool;
    fn get(&self, name: &SymbolId) -> Option<&TypeId>;
    fn get_const(&self, _name: &SymbolId) -> Option<&ConstGeneric> {
        None
    }
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

impl<S> TypeSubstMap for HashMap<SymbolId, GenericArg, S>
where
    S: BuildHasher,
{
    fn is_empty(&self) -> bool {
        HashMap::is_empty(self)
    }

    fn get(&self, name: &SymbolId) -> Option<&TypeId> {
        match HashMap::get(self, name) {
            Some(GenericArg::Type(ty)) => Some(ty),
            _ => None,
        }
    }

    fn get_const(&self, name: &SymbolId) -> Option<&ConstGeneric> {
        match HashMap::get(self, name) {
            Some(GenericArg::Const(value)) => Some(value),
            _ => None,
        }
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

    pub(crate) fn substitute_const_generic(&mut self, value: ConstGeneric) -> ConstGeneric {
        match value {
            ConstGeneric::Param(name, ty) => self
                .map
                .get_const(&name)
                .cloned()
                .unwrap_or(ConstGeneric::Param(name, ty)),
            ConstGeneric::Expr(id) => {
                let expr = *self.registry.const_expr(id);
                let rebuilt = match expr {
                    crate::ty::ConstExprKind::Unary { op, expr, ty } => {
                        crate::ty::ConstExprKind::Unary {
                            op,
                            expr: self.substitute_const_generic(expr),
                            ty,
                        }
                    }
                    crate::ty::ConstExprKind::Binary { op, lhs, rhs, ty } => {
                        crate::ty::ConstExprKind::Binary {
                            op,
                            lhs: self.substitute_const_generic(lhs),
                            rhs: self.substitute_const_generic(rhs),
                            ty,
                        }
                    }
                    crate::ty::ConstExprKind::Cast { expr, ty } => crate::ty::ConstExprKind::Cast {
                        expr: self.substitute_const_generic(expr),
                        ty,
                    },
                };
                let rebuilt_id = self.registry.intern_const_expr(rebuilt);
                self.registry
                    .fold_const_generic(ConstGeneric::Expr(rebuilt_id))
            }
            other => other,
        }
    }

    fn substitute_generic_arg(&mut self, arg: GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(self.substitute(ty)),
            GenericArg::Const(value) => GenericArg::Const(self.substitute_const_generic(value)),
        }
    }

    pub fn substitute(&mut self, ty: TypeId) -> TypeId {
        if ty == TypeId::ERROR || self.map.is_empty() {
            return ty;
        }

        let kind = self.registry.get(ty).clone();

        match kind {
            TypeKind::Primitive(_)
            | TypeKind::Error
            | TypeKind::Module(_)
            | TypeKind::TypeVar(_) => ty,

            // Even though source syntax currently names SIMD types through builtin aliases such as
            // `i32x4`, the semantic form is still structural. Keep substitution complete so
            // internal rewrites do not accidentally treat SIMD element types as opaque.
            TypeKind::Simd { elem, lanes } => {
                let new_elem = self.substitute(elem);
                self.registry.intern(TypeKind::Simd {
                    elem: new_elem,
                    lanes,
                })
            }

            // Replace matching generic parameters with their instantiated types.
            TypeKind::Param(name) => {
                if let Some(&new_ty) = self.map.get(&name) {
                    new_ty
                } else {
                    ty
                }
            }
            TypeKind::Associated(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
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
            TypeKind::Array { elem, len } => {
                let new_elem = self.substitute(elem);
                let new_len = self.substitute_const_generic(len);
                self.registry.intern(TypeKind::Array {
                    elem: new_elem,
                    len: new_len,
                })
            }
            TypeKind::ArrayInfer { elem } => {
                let new_elem = self.substitute(elem);
                self.registry
                    .intern(TypeKind::ArrayInfer { elem: new_elem })
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
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
                self.registry.intern(TypeKind::Def(def_id, new_args))
            }
            TypeKind::Enum(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
                self.registry.intern(TypeKind::Enum(def_id, new_args))
            }
            TypeKind::EnumPayload(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
                self.registry
                    .intern(TypeKind::EnumPayload(def_id, new_args))
            }
            TypeKind::FnDef(def_id, args) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
                self.registry.intern(TypeKind::FnDef(def_id, new_args))
            }
            TypeKind::Alias(name, target) => {
                let new_target = self.substitute(target);
                self.registry.intern(TypeKind::Alias(name, new_target))
            }
            TypeKind::TraitObject(def_id, args, assoc_bindings) => {
                let new_args = args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
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
                let new_trait_args = trait_args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
                let new_assoc_args = assoc_args
                    .into_iter()
                    .map(|arg| self.substitute_generic_arg(arg))
                    .collect();
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
    defs: &[Def],
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
        | TypeKind::Error
        | TypeKind::Module(_)
        | TypeKind::TypeVar(_)
        | TypeKind::Param(_) => ty,

        TypeKind::Simd { elem, lanes } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::Simd {
                elem: new_elem,
                lanes,
            })
        }

        TypeKind::Associated(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect::<Vec<_>>();
            if let Some(&bound_ty) = map.get(&def_id) {
                let bound_ty = substitute_associated_types(registry, defs, bound_ty, map);
                if new_args.is_empty() {
                    return bound_ty;
                }

                // Assoc bindings store the whole family target under the trait assoc `DefId`.
                // When the source mentions `Assoc[U]`, we must instantiate that stored family
                // with the applied assoc args instead of leaving a stale placeholder behind.
                let Some(assoc_generics) = defs.get(def_id.0 as usize).and_then(|def| match def {
                    Def::AssociatedType(def) => Some(def.generics.clone()),
                    _ => None,
                }) else {
                    return bound_ty;
                };
                if assoc_generics.len() != new_args.len() {
                    debug_assert_eq!(assoc_generics.len(), new_args.len());
                    return TypeId::ERROR;
                }
                let subst_map = assoc_generics
                    .into_iter()
                    .zip(new_args.iter().copied())
                    .map(|(param, arg)| (param.name, arg))
                    .collect::<HashMap<_, _>>();
                let mut subst = Substituter::new(registry, &subst_map);
                return subst.substitute(bound_ty);
            }
            registry.intern(TypeKind::Associated(def_id, new_args))
        }

        TypeKind::Pointer { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::Pointer {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::VolatilePtr { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::VolatilePtr {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::Slice { is_mut, elem } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::Slice {
                is_mut,
                elem: new_elem,
            })
        }
        TypeKind::Array { elem, len } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::Array {
                elem: new_elem,
                len,
            })
        }
        TypeKind::ArrayInfer { elem } => {
            let new_elem = substitute_associated_types(registry, defs, elem, map);
            registry.intern(TypeKind::ArrayInfer { elem: new_elem })
        }
        TypeKind::Function {
            params,
            ret,
            is_variadic,
        } => {
            let new_params = params
                .into_iter()
                .map(|param| substitute_associated_types(registry, defs, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, defs, ret, map);
            registry.intern(TypeKind::Function {
                params: new_params,
                ret: new_ret,
                is_variadic,
            })
        }
        TypeKind::Def(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            registry.intern(TypeKind::Def(def_id, new_args))
        }
        TypeKind::Enum(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            registry.intern(TypeKind::Enum(def_id, new_args))
        }
        TypeKind::EnumPayload(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            registry.intern(TypeKind::EnumPayload(def_id, new_args))
        }
        TypeKind::FnDef(def_id, args) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            registry.intern(TypeKind::FnDef(def_id, new_args))
        }
        TypeKind::Alias(name, target) => {
            let new_target = substitute_associated_types(registry, defs, target, map);
            registry.intern(TypeKind::Alias(name, new_target))
        }
        TypeKind::TraitObject(def_id, args, assoc_bindings) => {
            let new_args = args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            let new_assoc_bindings = assoc_bindings
                .into_iter()
                .map(|(assoc_def_id, ty)| {
                    (
                        assoc_def_id,
                        substitute_associated_types(registry, defs, ty, map),
                    )
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
            let new_target = substitute_associated_types(registry, defs, target, map);
            let new_trait_args = trait_args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
                .collect();
            let new_assoc_args = assoc_args
                .into_iter()
                .map(|arg| match arg {
                    GenericArg::Type(ty) => {
                        GenericArg::Type(substitute_associated_types(registry, defs, ty, map))
                    }
                    GenericArg::Const(value) => GenericArg::Const(value),
                })
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
                .map(|param| substitute_associated_types(registry, defs, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, defs, ret, map);
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
                .map(|capture| substitute_associated_types(registry, defs, capture, map))
                .collect();
            let new_params = params
                .into_iter()
                .map(|param| substitute_associated_types(registry, defs, param, map))
                .collect();
            let new_ret = substitute_associated_types(registry, defs, ret, map);
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
                    ty: substitute_associated_types(registry, defs, field.ty, map),
                })
                .collect();
            registry.intern(TypeKind::AnonymousStruct(is_extern, new_fields))
        }

        TypeKind::AnonymousUnion(is_extern, fields) => {
            let new_fields = fields
                .into_iter()
                .map(|field| crate::ty::AnonymousField {
                    name: field.name,
                    ty: substitute_associated_types(registry, defs, field.ty, map),
                })
                .collect();
            registry.intern(TypeKind::AnonymousUnion(is_extern, new_fields))
        }

        TypeKind::AnonymousEnum(enum_def) => {
            let new_backing_ty = enum_def
                .backing_ty
                .map(|backing_ty| substitute_associated_types(registry, defs, backing_ty, map));
            let new_variants = enum_def
                .variants
                .into_iter()
                .map(|variant| crate::ty::AnonymousVariant {
                    name: variant.name,
                    name_span: variant.name_span,
                    payload_ty: variant.payload_ty.map(|payload_ty| {
                        substitute_associated_types(registry, defs, payload_ty, map)
                    }),
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
            let substituted = substitute_associated_types(registry, defs, enum_ty, map);
            registry.intern(TypeKind::AnonymousEnumPayload(substituted))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_rewrites_simd_element_types() {
        let mut registry = TypeRegistry::new();
        let param = SymbolId(11);
        let elem = registry.intern(TypeKind::Param(param));
        let simd_ty = registry.intern(TypeKind::Simd { elem, lanes: 4 });
        let map = HashMap::from([(param, TypeId::I32)]);

        let substituted = {
            let mut subst = Substituter::new(&mut registry, &map);
            subst.substitute(simd_ty)
        };

        assert_eq!(
            registry.get(substituted),
            &TypeKind::Simd {
                elem: TypeId::I32,
                lanes: 4,
            }
        );
    }

    #[test]
    fn substitute_associated_types_rewrites_simd_element_types() {
        let mut registry = TypeRegistry::new();
        let assoc_id = DefId(7);
        let elem = registry.intern(TypeKind::Associated(assoc_id, Vec::new()));
        let simd_ty = registry.intern(TypeKind::Simd { elem, lanes: 8 });
        let map = HashMap::from([(assoc_id, TypeId::BOOL)]);

        let substituted = substitute_associated_types(&mut registry, &[], simd_ty, &map);

        assert_eq!(
            registry.get(substituted),
            &TypeKind::Simd {
                elem: TypeId::BOOL,
                lanes: 8,
            }
        );
    }

    #[test]
    fn substitute_associated_types_instantiates_generic_associated_binding() {
        let mut registry = TypeRegistry::new();
        let assoc_param = SymbolId(1);
        let assoc_id = DefId(0);
        let defs = vec![Def::AssociatedType(crate::def::AssociatedTypeDef {
            id: assoc_id,
            name: SymbolId(2),
            parent_trait: None,
            parent_impl: None,
            implemented_trait_assoc: None,
            is_imported: false,
            generics: vec![kernc_ast::GenericParam {
                name: assoc_param,
                span: Default::default(),
                kind: kernc_ast::GenericParamKind::Type,
            }],
            bounds: Vec::new(),
            where_clauses: Vec::new(),
            target: None,
            resolved_bounds: Vec::new(),
            span: Default::default(),
            docs: None,
        })];
        let assoc_param_ty = registry.intern(TypeKind::Param(assoc_param));
        let bound_ty = registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: assoc_param_ty,
        });
        let generic_assoc_ty = registry.intern(TypeKind::Associated(
            assoc_id,
            vec![GenericArg::Type(TypeId::I32)],
        ));
        let map = HashMap::from([(assoc_id, bound_ty)]);

        let substituted = substitute_associated_types(&mut registry, &defs, generic_assoc_ty, &map);

        assert_eq!(
            registry.get(substituted),
            &TypeKind::Pointer {
                is_mut: false,
                elem: TypeId::I32,
            }
        );
    }
}
