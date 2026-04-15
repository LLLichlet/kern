use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::{ConstEvaluator, ConstValue, Substituter};

mod asm;
mod dispatch;
mod intrinsic;

use kernc_sema::def::{Def, DefId};
use kernc_sema::query::MemberQuery;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, AtomicRmwOp, NodeId, Span, SymbolId};

pub(crate) struct DynamicDispatchCall {
    pub(crate) field: SymbolId,
    pub(crate) recv_trait_ty: TypeId,
    pub(crate) owner_trait_ty: TypeId,
    pub(crate) norm_callee: TypeId,
    pub(crate) span: Span,
}

pub(crate) struct MethodCallSite {
    pub(crate) field: SymbolId,
    pub(crate) norm_callee: TypeId,
    pub(crate) expected_self_ty: Option<TypeId>,
    pub(crate) span: Span,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    fn lower_loc_intrinsic(&mut self, result_ty: TypeId, span: Span) -> MastExprKind {
        let norm_result_ty = self.ctx.type_registry.normalize(result_ty);
        let TypeKind::AnonymousStruct(_, fields) =
            self.ctx.type_registry.get(norm_result_ty).clone()
        else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): `@loc` must return an anonymous struct.",
            );
            return MastExprKind::Trap;
        };

        let struct_id = self.instantiate_anon_struct(norm_result_ty);
        let file_text = self
            .ctx
            .sess
            .source_manager
            .get_file_path(span.file)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let (line, col) = self
            .ctx
            .sess
            .source_manager
            .lookup_location(span)
            .map(|loc| (loc.line, loc.col))
            .unwrap_or((0, 0));

        let mut field_exprs = Vec::with_capacity(fields.len());
        for field in &fields {
            let name = self.ctx.resolve(field.name);
            let expr = match name {
                "file" => {
                    MastExpr::new(field.ty, self.lower_string_literal(&file_text, span), span)
                }
                "line" => MastExpr::new(field.ty, MastExprKind::Integer(line as u128), span),
                "col" => MastExpr::new(field.ty, MastExprKind::Integer(col as u128), span),
                _ => {
                    self.ctx.emit_ice(
                        span,
                        format!("Kern ICE (Lowering): unknown `@loc` field `{}`.", name),
                    );
                    return MastExprKind::Trap;
                }
            };
            field_exprs.push(expr);
        }

        MastExprKind::StructInit {
            struct_id,
            fields: field_exprs,
        }
    }

    fn receiver_search_types(&mut self, receiver_ty: TypeId) -> Vec<TypeId> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        let mut search_tys = vec![receiver_norm];

        let downgraded = match self.ctx.type_registry.get(receiver_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::Slice { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }))
            }
            _ => None,
        };

        if let Some(down_ty) = downgraded
            && !search_tys.contains(&down_ty)
        {
            search_tys.push(down_ty);
        }

        search_tys
    }

    fn intrinsic_name_for_lowering(&mut self, callee_ty: TypeId) -> Option<String> {
        let norm = self.ctx.type_registry.normalize(callee_ty);
        let TypeKind::FnDef(def_id, _) = self.ctx.type_registry.get(norm).clone() else {
            return None;
        };
        let Def::Function(func) = &self.ctx.defs[def_id.0 as usize] else {
            return None;
        };
        if !func.is_intrinsic {
            return None;
        }
        Some(self.ctx.resolve(func.name).to_string())
    }

    fn builtin_trait_name(&mut self, trait_ty: TypeId) -> Option<String> {
        let norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(def_id, _, _) = self.ctx.type_registry.get(norm).clone() else {
            return None;
        };
        let Def::Trait(trait_def) = &self.ctx.defs[def_id.0 as usize] else {
            return None;
        };
        if !trait_def.is_builtin {
            return None;
        }
        Some(self.ctx.resolve(trait_def.name).to_string())
    }

    fn is_pure_enum_value_type(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Enum(def_id, _) => {
                let Def::Enum(def) = &self.ctx.defs[def_id.0 as usize] else {
                    return false;
                };
                self.is_pure_enum(def)
            }
            TypeKind::AnonymousEnum(anon) => anon
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none()),
            _ => false,
        }
    }

    fn type_contains_generic_placeholders(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_generic_placeholders(elem),
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.type_contains_generic_placeholders(elem)
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args
                .into_iter()
                .any(|arg| self.type_contains_generic_placeholders(arg)),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter()
                    .any(|arg| self.type_contains_generic_placeholders(arg))
                    || assoc_bindings
                        .into_iter()
                        .any(|(_, ty)| self.type_contains_generic_placeholders(ty))
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.type_contains_generic_placeholders(target)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.type_contains_generic_placeholders(arg))
                    || assoc_args
                        .into_iter()
                        .any(|arg| self.type_contains_generic_placeholders(arg))
            }
            TypeKind::Function { params, ret, .. } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_generic_placeholders(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_generic_placeholders(param))
                    || self.type_contains_generic_placeholders(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_generic_placeholders(field.ty)),
            TypeKind::AnonymousEnum(anon) => anon
                .variants
                .into_iter()
                .filter_map(|variant| variant.payload_ty)
                .any(|payload_ty| self.type_contains_generic_placeholders(payload_ty)),
            TypeKind::Primitive(_)
            | TypeKind::Simd { .. }
            | TypeKind::Error
            | TypeKind::Module(_) => false,
        }
    }

    fn trait_ty_satisfies_requirement(
        &mut self,
        required_trait_ty: TypeId,
        candidate_trait_ty: TypeId,
    ) -> bool {
        let required_norm = self.ctx.type_registry.normalize(required_trait_ty);
        let candidate_norm = self.ctx.type_registry.normalize(candidate_trait_ty);

        match (
            self.ctx.type_registry.get(required_norm).clone(),
            self.ctx.type_registry.get(candidate_norm).clone(),
        ) {
            (
                TypeKind::TraitObject(required_def_id, required_args, required_assoc_bindings),
                TypeKind::TraitObject(candidate_def_id, candidate_args, candidate_assoc_bindings),
            ) if required_def_id == candidate_def_id && required_args == candidate_args => {
                required_assoc_bindings
                    .into_iter()
                    .all(|(required_assoc_id, required_assoc_ty)| {
                        let Some((_, candidate_assoc_ty)) =
                            candidate_assoc_bindings
                                .iter()
                                .find(|(candidate_assoc_id, _)| {
                                    *candidate_assoc_id == required_assoc_id
                                })
                        else {
                            return false;
                        };
                        self.ctx.type_registry.normalize(required_assoc_ty)
                            == self.ctx.type_registry.normalize(*candidate_assoc_ty)
                    })
            }
            _ => required_norm == candidate_norm,
        }
    }
}
