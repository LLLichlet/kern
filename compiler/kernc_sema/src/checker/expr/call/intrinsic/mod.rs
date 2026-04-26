use super::ExprChecker;
use crate::LayoutEngine;
use crate::checker::{ConstEvaluator, ConstValue};
use crate::def::DefId;
use crate::scope::ScopeId;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::Expr;
use kernc_utils::{AtomicOrdering, Span};

struct SimdRelationOperand<'a> {
    ty: TypeId,
    span: Span,
    label: &'a str,
}

mod atomic;
mod simd_check;
mod simd_eval;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_bit_intrinsic_target_type(
        &mut self,
        ty: TypeId,
        span: Span,
        intrinsic_name: &str,
    ) -> TypeId {
        let norm = self.resolve_tv(ty);
        if norm == TypeId::ERROR {
            return TypeId::ERROR;
        }
        if self.type_contains_unresolved_params(norm) {
            return norm;
        }

        let is_supported = self.ctx.type_registry.is_integer(norm)
            || self
                .ctx
                .type_registry
                .simd_info(norm)
                .is_some_and(|(elem_ty, _)| self.ctx.type_registry.is_integer(elem_ty));

        if !is_supported {
            let ty_str = self.ctx.ty_to_string(norm);
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "`{}` only supports integer scalar or integer SIMD types",
                        intrinsic_name
                    ),
                )
                .with_hint(format!("found `{}`", ty_str))
                .with_hint("examples: `u32`, `i64`, `usize`, `u32x4`, `i16x8`")
                .emit();
            return TypeId::ERROR;
        }

        norm
    }

    pub(crate) fn check_bit_intrinsic_call(
        &mut self,
        intrinsic_name: &str,
        callee_ty: TypeId,
        args: &[Expr],
        params: &[TypeId],
        default_ret: TypeId,
    ) -> Option<TypeId> {
        match intrinsic_name {
            "@popCount" | "@clz" | "@ctz" | "@bswap" => {
                let value_ty = self.check_expr(&args[0], Some(params[0]));
                self.check_coercion(&args[0], params[0], value_ty);

                let target_ty = self
                    .intrinsic_generic_arg(callee_ty, 0)
                    .unwrap_or(default_ret);
                Some(self.check_bit_intrinsic_target_type(target_ty, args[0].span, intrinsic_name))
            }
            _ => None,
        }
    }

    pub(crate) fn type_contains_unresolved_params(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_unresolved_params(elem),
            TypeKind::Array { elem, len, .. } => {
                self.type_contains_unresolved_params(elem)
                    || self.ctx.type_registry.const_generic_contains_params(len)
            }
            TypeKind::ArrayInfer { elem, .. } => self.type_contains_unresolved_params(elem),
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args.into_iter().any(|arg| match arg {
                crate::ty::GenericArg::Type(ty) => self.type_contains_unresolved_params(ty),
                crate::ty::GenericArg::Const(value) => {
                    self.ctx.type_registry.const_generic_contains_params(value)
                }
            }),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter().any(|arg| match arg {
                    crate::ty::GenericArg::Type(ty) => self.type_contains_unresolved_params(ty),
                    crate::ty::GenericArg::Const(value) => {
                        self.ctx.type_registry.const_generic_contains_params(value)
                    }
                }) || assoc_bindings
                    .into_iter()
                    .any(|(_, ty)| self.type_contains_unresolved_params(ty))
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.type_contains_unresolved_params(target)
                    || trait_args.into_iter().any(|arg| match arg {
                        crate::ty::GenericArg::Type(ty) => self.type_contains_unresolved_params(ty),
                        crate::ty::GenericArg::Const(value) => {
                            self.ctx.type_registry.const_generic_contains_params(value)
                        }
                    })
                    || assoc_args.into_iter().any(|arg| match arg {
                        crate::ty::GenericArg::Type(ty) => self.type_contains_unresolved_params(ty),
                        crate::ty::GenericArg::Const(value) => {
                            self.ctx.type_registry.const_generic_contains_params(value)
                        }
                    })
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_unresolved_params(param))
                    || self.type_contains_unresolved_params(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_unresolved_params(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_unresolved_params(param))
                    || self.type_contains_unresolved_params(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_unresolved_params(field.ty)),
            TypeKind::AnonymousEnum(enum_def) => enum_def.variants.into_iter().any(|variant| {
                variant
                    .payload_ty
                    .is_some_and(|payload_ty| self.type_contains_unresolved_params(payload_ty))
            }),
            _ => false,
        }
    }

    pub(super) fn resolve_current_scope_for_types(
        &mut self,
        span: Span,
        context: &str,
    ) -> Option<ScopeId> {
        match self.ctx.scopes.current_scope_id() {
            Some(id) => Some(id),
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Compiler ICE: missing active scope while resolving types for {}.",
                        context
                    ),
                );
                None
            }
        }
    }

    pub(super) fn intrinsic_def_from_callee_ty(&self, callee_ty: TypeId) -> Option<DefId> {
        match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(callee_ty))
        {
            TypeKind::FnDef(def_id, _) => Some(*def_id),
            _ => None,
        }
    }

    fn intrinsic_generic_arg(&self, callee_ty: TypeId, index: usize) -> Option<TypeId> {
        match self
            .ctx
            .type_registry
            .get(self.ctx.type_registry.normalize(callee_ty))
        {
            TypeKind::FnDef(_, args) => args.get(index).and_then(|arg| arg.as_type()),
            _ => None,
        }
    }
}
