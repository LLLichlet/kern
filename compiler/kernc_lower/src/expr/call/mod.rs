//! Call lowering.
//!
//! This module lowers direct calls, method calls, dynamic dispatch, intrinsics,
//! inline assembly, and call argument materialization into MAST call and
//! dispatch forms suitable for monomorphization and MIR lowering.

use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::{ConstEvaluator, ConstValue};

mod asm;
mod dispatch;
mod intrinsic;

use kernc_sema::def::{Def, DefId};
use kernc_sema::query::MemberQuery;
use kernc_sema::ty::{AnonymousField, TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, AtomicRmwOp, NodeId, Span, SymbolId};

pub(crate) struct DynamicDispatchCall {
    pub(crate) field: SymbolId,
    pub(crate) recv_trait_ty: TypeId,
    pub(crate) owner_trait_ty: TypeId,
    pub(crate) norm_callee: TypeId,
    pub(crate) span: Span,
}

#[derive(Clone, Copy)]
pub(crate) struct MethodCallSite {
    pub(crate) field: SymbolId,
    pub(crate) norm_callee: TypeId,
    pub(crate) expected_self_ty: Option<TypeId>,
    pub(crate) default_ret_ty: TypeId,
    pub(crate) span: Span,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_loc_intrinsic(&mut self, result_ty: TypeId, span: Span) -> MastExprKind {
        let norm_result_ty = self.ctx.type_registry.normalize(result_ty);
        let TypeKind::AnonymousStruct(_, _) = self.ctx.type_registry.get(norm_result_ty).clone()
        else {
            self.ctx
                .struct_error(
                    span,
                    "`@loc` must return an anonymous struct containing `file`, `line`, and `col`",
                )
                .emit();
            return MastExprKind::Trap;
        };

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

        let file_name = self.ctx.intern("file");
        let line_name = self.ctx.intern("line");
        let col_name = self.ctx.intern("col");
        let file_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: TypeId::U8,
            len: self.usize_const_generic(file_text.len() as u64),
        });
        let natural_fields = vec![
            AnonymousField {
                name: file_name,
                ty: file_ty,
            },
            AnonymousField {
                name: line_name,
                ty: TypeId::USIZE,
            },
            AnonymousField {
                name: col_name,
                ty: TypeId::USIZE,
            },
        ];
        let natural_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AnonymousStruct(false, natural_fields.clone()));
        let struct_id = self.instantiate_anon_struct(natural_ty);
        let (_, physical_to_ast) =
            self.cached_anon_struct_mapping(natural_ty, false, &natural_fields);

        let mut field_exprs = Vec::with_capacity(natural_fields.len());
        for &ast_idx in &physical_to_ast {
            if self.check_canceled().is_err() {
                break;
            }
            let field = &natural_fields[ast_idx];
            let name = self.ctx.resolve(field.name);
            let expr = match name {
                "file" => MastExpr::new(
                    field.ty,
                    self.lower_string_literal_array(&file_text, span),
                    span,
                ),
                "line" => MastExpr::new(field.ty, MastExprKind::Integer(line as u128), span),
                "col" => MastExpr::new(field.ty, MastExprKind::Integer(col as u128), span),
                _ => {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`@loc` result type contains unsupported field `{}`; expected only `file`, `line`, and `col`",
                                name
                            ),
                        )
                        .emit();
                    return MastExprKind::Trap;
                }
            };
            field_exprs.push(expr);
        }

        let natural_kind = MastExprKind::StructInit {
            struct_id,
            fields: field_exprs,
        };

        self.apply_implicit_cast(natural_kind, natural_ty, result_ty, span)
            .kind
    }

    pub(crate) fn lower_check_intrinsic(
        &mut self,
        result_ty: TypeId,
        value_expr: MastExpr,
        arg_span: Span,
        span: Span,
    ) -> MastExprKind {
        let norm_result_ty = self.ctx.type_registry.normalize(result_ty);
        let TypeKind::AnonymousStruct(_, _) = self.ctx.type_registry.get(norm_result_ty).clone()
        else {
            self.ctx
                .struct_error(
                    span,
                    "`@check` must return an anonymous struct containing `value` and `source`",
                )
                .emit();
            return MastExprKind::Trap;
        };

        let source_text = self
            .ctx
            .sess
            .source_manager
            .slice_source(arg_span)
            .to_string();
        let value_name = self.ctx.intern("value");
        let source_name = self.ctx.intern("source");
        let source_ty = self.ctx.type_registry.intern(TypeKind::Array {
            elem: TypeId::U8,
            len: self.usize_const_generic(source_text.len() as u64),
        });
        let natural_fields = vec![
            AnonymousField {
                name: value_name,
                ty: value_expr.ty,
            },
            AnonymousField {
                name: source_name,
                ty: source_ty,
            },
        ];
        let natural_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::AnonymousStruct(false, natural_fields.clone()));
        let struct_id = self.instantiate_anon_struct(natural_ty);
        let (_, physical_to_ast) =
            self.cached_anon_struct_mapping(natural_ty, false, &natural_fields);

        let mut field_exprs = Vec::with_capacity(natural_fields.len());
        for &ast_idx in &physical_to_ast {
            if self.check_canceled().is_err() {
                break;
            }
            let field = &natural_fields[ast_idx];
            let name = self.ctx.resolve(field.name);
            let expr = match name {
                "value" => value_expr.clone(),
                "source" => MastExpr::new(
                    field.ty,
                    self.lower_string_literal_array(&source_text, arg_span),
                    arg_span,
                ),
                _ => {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "`@check` result type contains unsupported field `{}`; expected only `value` and `source`",
                                name
                            ),
                        )
                        .emit();
                    return MastExprKind::Trap;
                }
            };
            field_exprs.push(expr);
        }

        let natural_kind = MastExprKind::StructInit {
            struct_id,
            fields: field_exprs,
        };

        self.apply_implicit_cast(natural_kind, natural_ty, result_ty, span)
            .kind
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

    fn is_builtin_trait_named(&mut self, trait_ty: TypeId, expected_name: &str) -> bool {
        let norm = self.ctx.type_registry.normalize(trait_ty);
        let TypeKind::TraitObject(def_id, _, _) = self.ctx.type_registry.get(norm).clone() else {
            return false;
        };
        let Def::Trait(trait_def) = &self.ctx.defs[def_id.0 as usize] else {
            return false;
        };
        trait_def.is_builtin && self.ctx.resolve(trait_def.name) == expected_name
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
            TypeKind::Array { elem, len, .. } => {
                self.type_contains_generic_placeholders(elem)
                    || self.ctx.type_registry.const_generic_contains_params(len)
            }
            TypeKind::ArrayInfer { elem, .. } => self.type_contains_generic_placeholders(elem),
            TypeKind::Range { start, end, .. } => {
                start.is_some_and(|ty| self.type_contains_generic_placeholders(ty))
                    || end.is_some_and(|ty| self.type_contains_generic_placeholders(ty))
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args.into_iter().any(|arg| match arg {
                kernc_sema::ty::GenericArg::Type(arg) => {
                    self.type_contains_generic_placeholders(arg)
                }
                kernc_sema::ty::GenericArg::Const(value) => {
                    self.ctx.type_registry.const_generic_contains_params(value)
                }
            }),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter().any(|arg| match arg {
                    kernc_sema::ty::GenericArg::Type(arg) => {
                        self.type_contains_generic_placeholders(arg)
                    }
                    kernc_sema::ty::GenericArg::Const(value) => {
                        self.ctx.type_registry.const_generic_contains_params(value)
                    }
                }) || assoc_bindings
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
                    || trait_args.into_iter().any(|arg| match arg {
                        kernc_sema::ty::GenericArg::Type(arg) => {
                            self.type_contains_generic_placeholders(arg)
                        }
                        kernc_sema::ty::GenericArg::Const(value) => {
                            self.ctx.type_registry.const_generic_contains_params(value)
                        }
                    })
                    || assoc_args.into_iter().any(|arg| match arg {
                        kernc_sema::ty::GenericArg::Type(arg) => {
                            self.type_contains_generic_placeholders(arg)
                        }
                        kernc_sema::ty::GenericArg::Const(value) => {
                            self.ctx.type_registry.const_generic_contains_params(value)
                        }
                    })
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
}
