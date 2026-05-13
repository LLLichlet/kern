// compiler/kernc_lower/src/expr/cast.rs

use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::def::Def;
use kernc_sema::ty::{GenericArg, PrimitiveType, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

struct NamedStructAnonRewrite<'a> {
    def_id: kernc_sema::def::DefId,
    gen_args: &'a [GenericArg],
    anon_is_extern: bool,
    anon_fields: &'a [kernc_sema::ty::AnonymousField],
    fields: Vec<MastExpr>,
    exp_base: TypeId,
    span: Span,
}

struct NamedStructValueAnonRewrite<'a> {
    def_id: kernc_sema::def::DefId,
    gen_args: &'a [GenericArg],
    anon_is_extern: bool,
    anon_fields: &'a [kernc_sema::ty::AnonymousField],
    value_kind: MastExprKind,
    concrete_ty: TypeId,
    exp_base: TypeId,
    span: Span,
}

struct NamedUnionAnonRewrite<'a> {
    def_id: kernc_sema::def::DefId,
    anon_is_extern: bool,
    anon_fields: &'a [kernc_sema::ty::AnonymousField],
    field_idx: usize,
    value: MastExpr,
    exp_base: TypeId,
    span: Span,
}

struct NamedEnumAnonRewrite<'a> {
    def_id: kernc_sema::def::DefId,
    anon_enum: &'a kernc_sema::ty::AnonymousEnum,
    tag_value: u128,
    payload: MastExpr,
    exp_base: TypeId,
    span: Span,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_as_expr(
        &mut self,
        lhs: &Expr,
        target: &ast::TypeNode,
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, GenericArg>,
        span: Span,
    ) -> MastExpr {
        let raw_target_ty = self.ctx.node_type(target.id).unwrap_or(concrete_ty);
        let target_ty = self.substitute_type_with_map(raw_target_ty, subst_map);
        let l = self.lower_expr(lhs, subst_map, None);

        if let Some(packed) = self.lower_explicit_fat_pointer_cast(l.clone(), target_ty, span) {
            return packed;
        }

        let cast_kind = self.determine_cast_kind(l.ty, target_ty);

        MastExpr::new(
            target_ty,
            MastExprKind::Cast {
                kind: cast_kind,
                operand: Box::new(l),
            },
            span,
        )
    }

    fn lower_explicit_fat_pointer_cast(
        &mut self,
        source: MastExpr,
        target_ty: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let target_norm = self.ctx.type_registry.normalize(target_ty);
        let target_elem = match self.ctx.type_registry.get(target_norm).clone() {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => elem,
            _ => return None,
        };
        let target_elem_norm = self.ctx.type_registry.normalize(target_elem);

        match self.ctx.type_registry.get(target_elem_norm).clone() {
            TypeKind::TraitObject(..) => Some(MastExpr::new(
                target_ty,
                self.lower_trait_object_init_from_mast(source, target_ty, target_elem_norm, span),
                span,
            )),
            TypeKind::ClosureInterface { .. } => {
                Some(self.lower_closure_object_cast_from_pointer(source, target_ty, span))
            }
            _ => None,
        }
    }

    pub(crate) fn apply_implicit_cast(
        &mut self,
        mut mast_kind: MastExprKind,
        concrete_ty: TypeId,
        exp_ty: TypeId,
        span: Span,
    ) -> MastExpr {
        let conc_base = self.ctx.type_registry.normalize(concrete_ty);
        let exp_base = self.ctx.type_registry.normalize(exp_ty);

        if conc_base == exp_base {
            return MastExpr::new(exp_ty, mast_kind, span);
        }

        let conc_kind = self.ctx.type_registry.get(conc_base).clone();
        let exp_kind = self.ctx.type_registry.get(exp_base).clone();

        if let Some(rewritten) = self.try_rewrite_named_aggregate_to_anonymous(
            mast_kind.clone(),
            concrete_ty,
            conc_base,
            exp_base,
            span,
        ) {
            return rewritten;
        }

        if let Some(rewritten) = self.try_rewrite_anonymous_struct_to_anonymous(
            mast_kind.clone(),
            concrete_ty,
            conc_base,
            exp_base,
            span,
        ) {
            return rewritten;
        }

        if let Some(rewritten) = self.rewrite_array_init_for_expected_container(
            mast_kind.clone(),
            conc_base,
            exp_ty,
            exp_base,
            span,
        ) {
            return rewritten;
        }

        if let TypeKind::Pointer { elem, .. } = exp_kind
            && matches!(
                self.ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(elem)),
                TypeKind::TraitObject(..)
            )
            && let TypeKind::Array {
                elem: array_elem, ..
            } = conc_kind
        {
            let slice_ty = self.ctx.type_registry.intern(TypeKind::Slice {
                elem: array_elem,
                is_mut: false,
            });
            let slice_expr = self.apply_implicit_cast(mast_kind, concrete_ty, slice_ty, span);
            return self.apply_implicit_cast(slice_expr.kind, slice_ty, exp_ty, span);
        }

        // 1. Implicit array-to-slice conversion.
        if let TypeKind::Slice { .. } = exp_kind
            && let TypeKind::Array { .. } = conc_kind
        {
            mast_kind = MastExprKind::Cast {
                kind: MastCastKind::ArrayToSlice,
                operand: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
            };
            return MastExpr::new(exp_ty, mast_kind, span);
        }

        let conc_is_fat_pointer_value = match conc_kind {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => matches!(
                self.ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(elem)),
                TypeKind::TraitObject(..) | TypeKind::ClosureInterface { .. }
            ),
            _ => false,
        };
        let conc_trait_object_elem = match conc_kind {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let elem_norm = self.ctx.type_registry.normalize(elem);
                matches!(
                    self.ctx.type_registry.get(elem_norm),
                    TypeKind::TraitObject(..)
                )
                .then_some(elem_norm)
            }
            _ => None,
        };

        // 2. Implicit thin-pointer-to-trait-object packing.
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm)
                && let Some(actual_elem_norm) = conc_trait_object_elem
                && conc_is_fat_pointer_value
                && self.is_trait_object_upcast(actual_elem_norm, e_inner_norm)
            {
                return self.lower_trait_object_upcast(
                    MastExpr::new(concrete_ty, mast_kind, span),
                    exp_ty,
                    actual_elem_norm,
                    e_inner_norm,
                    span,
                );
            }

            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm)
                && let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } = conc_kind
                && !conc_is_fat_pointer_value
            {
                let actual_elem_norm = match conc_kind {
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                        self.ctx.type_registry.normalize(elem)
                    }
                    _ => TypeId::ERROR,
                };

                if matches!(
                    self.ctx.type_registry.get(actual_elem_norm),
                    TypeKind::TraitObject(..)
                ) && self.is_trait_object_upcast(actual_elem_norm, e_inner_norm)
                {
                    return self.lower_trait_object_upcast(
                        MastExpr::new(concrete_ty, mast_kind, span),
                        exp_ty,
                        actual_elem_norm,
                        e_inner_norm,
                        span,
                    );
                }

                let (data_ptr_expr, data_ptr_ty) = if !e_mut {
                    match conc_kind {
                        TypeKind::Pointer { is_mut: true, elem } => {
                            let shared_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut: false,
                                elem,
                            });
                            (
                                MastExpr::new(
                                    shared_ptr_ty,
                                    MastExprKind::Cast {
                                        kind: MastCastKind::Bitcast,
                                        operand: Box::new(MastExpr::new(
                                            concrete_ty,
                                            mast_kind,
                                            span,
                                        )),
                                    },
                                    span,
                                ),
                                shared_ptr_ty,
                            )
                        }
                        TypeKind::VolatilePtr { is_mut: true, elem } => {
                            let shared_ptr_ty =
                                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                                    is_mut: false,
                                    elem,
                                });
                            (
                                MastExpr::new(
                                    shared_ptr_ty,
                                    MastExprKind::Cast {
                                        kind: MastCastKind::Bitcast,
                                        operand: Box::new(MastExpr::new(
                                            concrete_ty,
                                            mast_kind,
                                            span,
                                        )),
                                    },
                                    span,
                                ),
                                shared_ptr_ty,
                            )
                        }
                        _ => (MastExpr::new(concrete_ty, mast_kind, span), concrete_ty),
                    }
                } else {
                    (MastExpr::new(concrete_ty, mast_kind, span), concrete_ty)
                };

                let vtable_id = self.get_or_create_vtable(data_ptr_ty, data_ptr_ty, e_inner_norm);
                let Some(meta_expr) = self.vtable_global_meta_expr(vtable_id, span) else {
                    return MastExpr::new(exp_ty, MastExprKind::Trap, span);
                };

                return MastExpr::new(
                    exp_ty,
                    MastExprKind::ConstructFatPointer {
                        data_ptr: Box::new(data_ptr_expr),
                        meta: Box::new(meta_expr),
                    },
                    span,
                );
            }
        }

        // 3. Implicitly take the address of non-thin values and pack them as trait objects.
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm)
                && (!matches!(
                    conc_kind,
                    TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
                ) || conc_is_fat_pointer_value)
            {
                let ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: e_mut,
                    elem: concrete_ty,
                });

                // Synthesize an address-of operation in place.
                let data_ptr_expr = MastExpr::new(
                    ptr_ty,
                    MastExprKind::AddressOf(Box::new(MastExpr::new(concrete_ty, mast_kind, span))),
                    span,
                );

                // After materialization, trait-object packing is identical to the pointer path.
                let vtable_id = self.get_or_create_vtable(ptr_ty, concrete_ty, e_inner_norm);
                let Some(meta_expr) = self.vtable_global_meta_expr(vtable_id, span) else {
                    return MastExpr::new(exp_ty, MastExprKind::Trap, span);
                };

                return MastExpr::new(
                    exp_ty,
                    MastExprKind::ConstructFatPointer {
                        data_ptr: Box::new(data_ptr_expr),
                        meta: Box::new(meta_expr),
                    },
                    span,
                );
            }
        }

        // 4. Closure BNC: functions or anonymous state to closure fat pointers.
        if let TypeKind::Pointer { elem: e_inner, .. } = exp_kind {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::ClosureInterface { .. } = self.ctx.type_registry.get(e_inner_norm) {
                // 4.1 Stateless functions become closure fat pointers.
                if matches!(conc_kind, TypeKind::FnDef(..) | TypeKind::Function { .. }) {
                    let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem: TypeId::VOID,
                    });
                    let fn_ptr_int = MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Cast {
                            kind: MastCastKind::PtrToInt,
                            operand: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
                        },
                        span,
                    );
                    let data_ptr_expr = MastExpr::new(
                        void_ptr_ty,
                        MastExprKind::Cast {
                            kind: MastCastKind::IntToPtr,
                            operand: Box::new(fn_ptr_int),
                        },
                        span,
                    );

                    let Some(adapter_id) = self.get_or_create_fn_closure_adapter(concrete_ty, span)
                    else {
                        return MastExpr::new(exp_ty, MastExprKind::Trap, span);
                    };

                    // Metadata stores the adapter entry pointer cast to `usize`.
                    let fn_ptr_expr =
                        MastExpr::new(TypeId::VOID, MastExprKind::FuncRef(adapter_id), span);
                    let meta_expr = MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Cast {
                            kind: MastCastKind::PtrToInt,
                            operand: Box::new(fn_ptr_expr),
                        },
                        span,
                    );

                    return MastExpr::new(
                        exp_ty,
                        MastExprKind::ConstructFatPointer {
                            data_ptr: Box::new(data_ptr_expr),
                            meta: Box::new(meta_expr),
                        },
                        span,
                    );
                }

                // 4.2 Anonymous closure state becomes a closure fat pointer.
                if let TypeKind::AnonymousState {
                    closure_node_id, ..
                } = conc_kind
                {
                    // The data pointer comes from an implicit address-of.
                    let ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: true, // Closure state is usually mutated by the callee.
                        elem: concrete_ty,
                    });
                    let data_ptr_expr = MastExpr::new(
                        ptr_ty,
                        MastExprKind::AddressOf(Box::new(MastExpr::new(
                            concrete_ty,
                            mast_kind,
                            span,
                        ))),
                        span,
                    );

                    // Metadata stores the generated closure entry function pointer.
                    let func_mono_id = self.get_closure_func_mono_id(closure_node_id);
                    // A plain `FuncRef` is enough here.
                    let fn_ptr_expr = MastExpr::new(
                        TypeId::VOID, // Any placeholder works for the cast, but keeping it tidy helps maintenance.
                        MastExprKind::FuncRef(func_mono_id),
                        span,
                    );

                    let meta_expr = MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Cast {
                            kind: MastCastKind::PtrToInt,
                            operand: Box::new(fn_ptr_expr),
                        },
                        span,
                    );

                    return MastExpr::new(
                        exp_ty,
                        MastExprKind::ConstructFatPointer {
                            data_ptr: Box::new(data_ptr_expr),
                            meta: Box::new(meta_expr),
                        },
                        span,
                    );
                }
            }
        }

        // Otherwise leave the expression unchanged.
        MastExpr::new(exp_ty, mast_kind, span)
    }

    fn rewrite_array_init_for_expected_container(
        &mut self,
        mast_kind: MastExprKind,
        conc_base: TypeId,
        exp_ty: TypeId,
        exp_base: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let MastExprKind::ArrayInit(fields) = mast_kind else {
            return None;
        };

        let conc_elem = match self.ctx.type_registry.get(conc_base).clone() {
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem } => elem,
            _ => return None,
        };

        let (exp_elem, preserve_slice_ty) = match self.ctx.type_registry.get(exp_base).clone() {
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem } => (elem, false),
            TypeKind::Slice { elem, .. } => (elem, true),
            _ => return None,
        };

        if self.ctx.type_registry.normalize(conc_elem) == self.ctx.type_registry.normalize(exp_elem)
        {
            return None;
        }

        let rewritten_fields = fields
            .into_iter()
            .map(|field| self.apply_implicit_cast(field.kind, field.ty, exp_elem, field.span))
            .collect::<Vec<_>>();

        let rewritten_array_ty = match self.ctx.type_registry.get(exp_base).clone() {
            TypeKind::Array { .. } => exp_ty,
            TypeKind::ArrayInfer { .. } | TypeKind::Slice { .. } => {
                self.ctx.type_registry.intern(TypeKind::Array {
                    elem: exp_elem,
                    len: self.usize_const_generic(rewritten_fields.len() as u64),
                })
            }
            _ => return None,
        };

        let rewritten_array = MastExpr::new(
            rewritten_array_ty,
            MastExprKind::ArrayInit(rewritten_fields),
            span,
        );

        if preserve_slice_ty {
            Some(self.apply_implicit_cast(rewritten_array.kind, rewritten_array.ty, exp_ty, span))
        } else {
            Some(rewritten_array)
        }
    }

    fn try_rewrite_named_aggregate_to_anonymous(
        &mut self,
        mast_kind: MastExprKind,
        concrete_ty: TypeId,
        conc_base: TypeId,
        exp_base: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let conc_kind = self.ctx.type_registry.get(conc_base).clone();
        let exp_kind = self.ctx.type_registry.get(exp_base).clone();

        match (exp_kind, conc_kind, mast_kind) {
            (
                TypeKind::AnonymousStruct(is_extern, anon_fields),
                TypeKind::Def(def_id, gen_args),
                MastExprKind::StructInit { fields, .. },
            ) => self.rewrite_named_struct_init_to_anon(NamedStructAnonRewrite {
                def_id,
                gen_args: &gen_args,
                anon_is_extern: is_extern,
                anon_fields: &anon_fields,
                fields,
                exp_base,
                span,
            }),
            (
                TypeKind::AnonymousStruct(is_extern, anon_fields),
                TypeKind::Def(def_id, gen_args),
                value_kind,
            ) => self.rewrite_named_struct_value_to_anon(NamedStructValueAnonRewrite {
                def_id,
                gen_args: &gen_args,
                anon_is_extern: is_extern,
                anon_fields: &anon_fields,
                value_kind,
                concrete_ty,
                exp_base,
                span,
            }),
            (
                TypeKind::AnonymousUnion(is_extern, anon_fields),
                TypeKind::Def(def_id, gen_args),
                MastExprKind::UnionInit {
                    field_idx, value, ..
                },
            ) => {
                let _ = gen_args;
                self.rewrite_named_union_init_to_anon(NamedUnionAnonRewrite {
                    def_id,
                    anon_is_extern: is_extern,
                    anon_fields: &anon_fields,
                    field_idx,
                    value: *value,
                    exp_base,
                    span,
                })
            }
            (TypeKind::AnonymousUnion(..), TypeKind::Def(..), value_kind) => self
                .rewrite_named_value_reinterpret_to_anonymous(
                    value_kind,
                    concrete_ty,
                    exp_base,
                    span,
                ),
            (
                TypeKind::AnonymousEnum(anon_enum),
                TypeKind::Enum(def_id, gen_args),
                MastExprKind::DataInit {
                    tag_value, payload, ..
                },
            ) => {
                let _ = gen_args;
                self.rewrite_named_enum_init_to_anon(NamedEnumAnonRewrite {
                    def_id,
                    anon_enum: &anon_enum,
                    tag_value,
                    payload: *payload,
                    exp_base,
                    span,
                })
            }
            (
                TypeKind::AnonymousEnum(anon_enum),
                TypeKind::Enum(_, _),
                MastExprKind::Integer(tag),
            ) if anon_enum
                .variants
                .iter()
                .all(|variant| variant.payload_ty.is_none()) =>
            {
                Some(MastExpr::new(exp_base, MastExprKind::Integer(tag), span))
            }
            (TypeKind::AnonymousEnum(..), TypeKind::Enum(..), value_kind) => self
                .rewrite_named_value_reinterpret_to_anonymous(
                    value_kind,
                    concrete_ty,
                    exp_base,
                    span,
                ),
            _ => None,
        }
    }

    fn rewrite_named_value_reinterpret_to_anonymous(
        &mut self,
        value_kind: MastExprKind,
        concrete_ty: TypeId,
        exp_base: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let source_name = self.fresh_synth_symbol("anon_reinterpret");
        let source_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: concrete_ty,
        });
        let target_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: exp_base,
        });

        Some(MastExpr::new(
            exp_base,
            MastExprKind::Block(MastBlock {
                stmts: vec![MastStmt::Let {
                    name: source_name,
                    ty: concrete_ty,
                    is_mut: false,
                    init: MastExpr::new(concrete_ty, value_kind, span),
                }],
                result: Some(Box::new(MastExpr::new(
                    exp_base,
                    MastExprKind::Deref(Box::new(MastExpr::new(
                        target_ptr_ty,
                        MastExprKind::Cast {
                            kind: MastCastKind::Bitcast,
                            operand: Box::new(MastExpr::new(
                                source_ptr_ty,
                                MastExprKind::AddressOf(Box::new(MastExpr::new(
                                    concrete_ty,
                                    MastExprKind::Var(source_name),
                                    span,
                                ))),
                                span,
                            )),
                        },
                        span,
                    ))),
                    span,
                ))),
                defers: Vec::new(),
            }),
            span,
        ))
    }

    fn rewrite_named_struct_init_to_anon(
        &mut self,
        rewrite: NamedStructAnonRewrite<'_>,
    ) -> Option<MastExpr> {
        let Def::Struct(struct_def) = self.ctx.defs.get(rewrite.def_id.0 as usize).cloned()? else {
            return None;
        };
        if struct_def.is_extern != rewrite.anon_is_extern {
            return None;
        }

        let (_, physical_to_ast) =
            self.cached_named_struct_mapping(rewrite.def_id, rewrite.gen_args);
        if physical_to_ast.len() != rewrite.fields.len() {
            self.ctx
                .struct_error(
                    rewrite.span,
                    "implicit decay from named struct to anonymous struct has mismatched field counts",
                )
                .emit();
            return Some(MastExpr::new(
                rewrite.exp_base,
                MastExprKind::Trap,
                rewrite.span,
            ));
        }

        let source_by_name = physical_to_ast
            .iter()
            .enumerate()
            .map(|(phys_idx, &ast_idx)| {
                (
                    struct_def.fields[ast_idx].name,
                    rewrite.fields[phys_idx].clone(),
                )
            })
            .collect::<HashMap<_, _>>();

        let struct_id = self.instantiate_anon_struct(rewrite.exp_base);
        let (_, anon_physical_to_ast) = self.cached_anon_struct_mapping(
            rewrite.exp_base,
            rewrite.anon_is_extern,
            rewrite.anon_fields,
        );

        let mut rewritten_fields = Vec::with_capacity(rewrite.anon_fields.len());
        for &ast_idx in &anon_physical_to_ast {
            let field = &rewrite.anon_fields[ast_idx];
            let Some(source_expr) = source_by_name.get(&field.name).cloned() else {
                self.ctx
                    .struct_error(
                        rewrite.span,
                        format!(
                            "implicit anonymous-struct decay is missing source field `{}`",
                            self.ctx.resolve(field.name)
                        ),
                    )
                    .emit();
                return Some(MastExpr::new(
                    rewrite.exp_base,
                    MastExprKind::Trap,
                    rewrite.span,
                ));
            };
            rewritten_fields.push(self.apply_implicit_cast(
                source_expr.kind,
                source_expr.ty,
                field.ty,
                source_expr.span,
            ));
        }

        Some(MastExpr::new(
            rewrite.exp_base,
            MastExprKind::StructInit {
                struct_id,
                fields: rewritten_fields,
            },
            rewrite.span,
        ))
    }

    fn rewrite_named_struct_value_to_anon(
        &mut self,
        rewrite: NamedStructValueAnonRewrite<'_>,
    ) -> Option<MastExpr> {
        let Def::Struct(struct_def) = self.ctx.defs.get(rewrite.def_id.0 as usize).cloned()? else {
            return None;
        };
        if struct_def.is_extern != rewrite.anon_is_extern {
            return None;
        }

        let mut subst_map = HashMap::new();
        for (index, param) in struct_def.generics.iter().enumerate() {
            let arg = rewrite
                .gen_args
                .get(index)
                .copied()
                .unwrap_or(kernc_sema::ty::GenericArg::Type(TypeId::ERROR));
            subst_map.insert(param.name, arg);
        }

        let source_name = self.fresh_synth_symbol("anon_decay");
        let source_value = MastExpr::new(rewrite.concrete_ty, rewrite.value_kind, rewrite.span);
        let source_ref = MastExpr::new(
            rewrite.concrete_ty,
            MastExprKind::Var(source_name),
            rewrite.span,
        );
        let source_struct_id = self.instantiate_struct(rewrite.def_id, rewrite.gen_args);
        let target_struct_id = self.instantiate_anon_struct(rewrite.exp_base);

        let (_, named_physical_to_ast) =
            self.cached_named_struct_mapping(rewrite.def_id, rewrite.gen_args);
        let (_, anon_physical_to_ast) = self.cached_anon_struct_mapping(
            rewrite.exp_base,
            rewrite.anon_is_extern,
            rewrite.anon_fields,
        );

        let mut source_by_name = HashMap::new();
        for (phys_idx, &ast_idx) in named_physical_to_ast.iter().enumerate() {
            let field = &struct_def.fields[ast_idx];
            let raw_ty = self
                .ctx
                .node_type(field.type_node.id)
                .unwrap_or(TypeId::ERROR);
            let field_ty = self.substitute_type_with_map(raw_ty, &subst_map);
            source_by_name.insert(
                field.name,
                MastExpr::new(
                    field_ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(source_ref.clone()),
                        struct_id: source_struct_id,
                        field_idx: phys_idx,
                    },
                    rewrite.span,
                ),
            );
        }

        let mut rewritten_fields = Vec::with_capacity(rewrite.anon_fields.len());
        for &ast_idx in &anon_physical_to_ast {
            let field = &rewrite.anon_fields[ast_idx];
            let source_expr = source_by_name.get(&field.name).cloned()?;
            rewritten_fields.push(self.apply_implicit_cast(
                source_expr.kind,
                source_expr.ty,
                field.ty,
                rewrite.span,
            ));
        }

        Some(MastExpr::new(
            rewrite.exp_base,
            MastExprKind::Block(MastBlock {
                stmts: vec![MastStmt::Let {
                    name: source_name,
                    ty: rewrite.concrete_ty,
                    is_mut: false,
                    init: source_value,
                }],
                result: Some(Box::new(MastExpr::new(
                    rewrite.exp_base,
                    MastExprKind::StructInit {
                        struct_id: target_struct_id,
                        fields: rewritten_fields,
                    },
                    rewrite.span,
                ))),
                defers: Vec::new(),
            }),
            rewrite.span,
        ))
    }

    fn try_rewrite_anonymous_struct_to_anonymous(
        &mut self,
        value_kind: MastExprKind,
        concrete_ty: TypeId,
        conc_base: TypeId,
        exp_base: TypeId,
        span: Span,
    ) -> Option<MastExpr> {
        let TypeKind::AnonymousStruct(act_is_extern, act_fields) =
            self.ctx.type_registry.get(conc_base).clone()
        else {
            return None;
        };
        let TypeKind::AnonymousStruct(exp_is_extern, exp_fields) =
            self.ctx.type_registry.get(exp_base).clone()
        else {
            return None;
        };
        if act_is_extern != exp_is_extern || act_fields.len() != exp_fields.len() {
            return None;
        }

        match value_kind {
            MastExprKind::StructInit { fields, .. } => self
                .rewrite_anonymous_struct_fields_to_anonymous(
                    fields,
                    conc_base,
                    &act_fields,
                    exp_base,
                    exp_is_extern,
                    &exp_fields,
                    span,
                )
                .map(|fields| {
                    let struct_id = self.instantiate_anon_struct(exp_base);
                    MastExpr::new(
                        exp_base,
                        MastExprKind::StructInit { struct_id, fields },
                        span,
                    )
                }),
            other => self.rewrite_anonymous_struct_value_to_anonymous(
                other,
                concrete_ty,
                conc_base,
                &act_fields,
                exp_base,
                exp_is_extern,
                &exp_fields,
                span,
            ),
        }
    }

    fn rewrite_anonymous_struct_fields_to_anonymous(
        &mut self,
        fields: Vec<MastExpr>,
        conc_base: TypeId,
        act_fields: &[kernc_sema::ty::AnonymousField],
        exp_base: TypeId,
        exp_is_extern: bool,
        exp_fields: &[kernc_sema::ty::AnonymousField],
        span: Span,
    ) -> Option<Vec<MastExpr>> {
        let (_, act_physical_to_ast) =
            self.cached_anon_struct_mapping(conc_base, exp_is_extern, act_fields);
        let (_, exp_physical_to_ast) =
            self.cached_anon_struct_mapping(exp_base, exp_is_extern, exp_fields);

        let mut source_by_name = HashMap::new();
        for (phys_idx, &ast_idx) in act_physical_to_ast.iter().enumerate() {
            let field = act_fields.get(ast_idx)?;
            let value = fields.get(phys_idx)?.clone();
            source_by_name.insert(field.name, value);
        }

        let mut rewritten_fields = Vec::with_capacity(exp_fields.len());
        for &ast_idx in &exp_physical_to_ast {
            let field = exp_fields.get(ast_idx)?;
            let source_expr = source_by_name.get(&field.name).cloned()?;
            rewritten_fields.push(self.apply_implicit_cast(
                source_expr.kind,
                source_expr.ty,
                field.ty,
                span,
            ));
        }
        Some(rewritten_fields)
    }

    fn rewrite_anonymous_struct_value_to_anonymous(
        &mut self,
        value_kind: MastExprKind,
        concrete_ty: TypeId,
        conc_base: TypeId,
        act_fields: &[kernc_sema::ty::AnonymousField],
        exp_base: TypeId,
        exp_is_extern: bool,
        exp_fields: &[kernc_sema::ty::AnonymousField],
        span: Span,
    ) -> Option<MastExpr> {
        let (_, act_physical_to_ast) =
            self.cached_anon_struct_mapping(conc_base, exp_is_extern, act_fields);
        let (_, exp_physical_to_ast) =
            self.cached_anon_struct_mapping(exp_base, exp_is_extern, exp_fields);
        let source_struct_id = self.instantiate_anon_struct(conc_base);
        let target_struct_id = self.instantiate_anon_struct(exp_base);
        let source_name = self.fresh_synth_symbol("anon_decay");
        let source_value = MastExpr::new(concrete_ty, value_kind, span);
        let source_ref = MastExpr::new(concrete_ty, MastExprKind::Var(source_name), span);

        let mut source_by_name = HashMap::new();
        for (phys_idx, &ast_idx) in act_physical_to_ast.iter().enumerate() {
            let field = act_fields.get(ast_idx)?;
            source_by_name.insert(
                field.name,
                MastExpr::new(
                    field.ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(source_ref.clone()),
                        struct_id: source_struct_id,
                        field_idx: phys_idx,
                    },
                    span,
                ),
            );
        }

        let mut rewritten_fields = Vec::with_capacity(exp_fields.len());
        for &ast_idx in &exp_physical_to_ast {
            let field = exp_fields.get(ast_idx)?;
            let source_expr = source_by_name.get(&field.name).cloned()?;
            rewritten_fields.push(self.apply_implicit_cast(
                source_expr.kind,
                source_expr.ty,
                field.ty,
                span,
            ));
        }

        Some(MastExpr::new(
            exp_base,
            MastExprKind::Block(MastBlock {
                stmts: vec![MastStmt::Let {
                    name: source_name,
                    ty: concrete_ty,
                    is_mut: false,
                    init: source_value,
                }],
                result: Some(Box::new(MastExpr::new(
                    exp_base,
                    MastExprKind::StructInit {
                        struct_id: target_struct_id,
                        fields: rewritten_fields,
                    },
                    span,
                ))),
                defers: Vec::new(),
            }),
            span,
        ))
    }

    fn rewrite_named_union_init_to_anon(
        &mut self,
        rewrite: NamedUnionAnonRewrite<'_>,
    ) -> Option<MastExpr> {
        let Def::Union(union_def) = self.ctx.defs.get(rewrite.def_id.0 as usize).cloned()? else {
            return None;
        };
        if union_def.is_extern != rewrite.anon_is_extern {
            return None;
        }

        let Some(source_field) = union_def.fields.get(rewrite.field_idx) else {
            self.ctx
                .struct_error(
                    rewrite.span,
                    "implicit decay from named union referenced an out-of-bounds field index",
                )
                .emit();
            return Some(MastExpr::new(
                rewrite.exp_base,
                MastExprKind::Trap,
                rewrite.span,
            ));
        };
        let Some(target_idx) = rewrite
            .anon_fields
            .iter()
            .position(|field| field.name == source_field.name)
        else {
            self.ctx
                .struct_error(
                    rewrite.span,
                    format!(
                        "implicit anonymous-union decay is missing target field `{}`",
                        self.ctx.resolve(source_field.name)
                    ),
                )
                .emit();
            return Some(MastExpr::new(
                rewrite.exp_base,
                MastExprKind::Trap,
                rewrite.span,
            ));
        };

        let union_id = self.instantiate_anon_union(rewrite.exp_base);
        let target_field_ty = rewrite.anon_fields[target_idx].ty;
        let rewritten_value = self.apply_implicit_cast(
            rewrite.value.kind,
            rewrite.value.ty,
            target_field_ty,
            rewrite.value.span,
        );

        Some(MastExpr::new(
            rewrite.exp_base,
            MastExprKind::UnionInit {
                union_id,
                field_idx: target_idx,
                value: Box::new(rewritten_value),
            },
            rewrite.span,
        ))
    }

    fn rewrite_named_enum_init_to_anon(
        &mut self,
        rewrite: NamedEnumAnonRewrite<'_>,
    ) -> Option<MastExpr> {
        let Def::Enum(_) = self.ctx.defs.get(rewrite.def_id.0 as usize).cloned()? else {
            return None;
        };

        let Some(target_payload_ty) =
            self.anon_enum_payload_ty_for_tag(rewrite.anon_enum, rewrite.tag_value as i128)
        else {
            self.ctx
                .struct_error(
                    rewrite.span,
                    format!(
                        "implicit anonymous-enum decay is missing a variant for tag `{}`",
                        rewrite.tag_value
                    ),
                )
                .emit();
            return Some(MastExpr::new(
                rewrite.exp_base,
                MastExprKind::Trap,
                rewrite.span,
            ));
        };

        let payload = if let Some(target_payload_ty) = target_payload_ty {
            self.apply_implicit_cast(
                rewrite.payload.kind,
                rewrite.payload.ty,
                target_payload_ty,
                rewrite.payload.span,
            )
        } else {
            rewrite.payload
        };

        Some(MastExpr::new(
            rewrite.exp_base,
            MastExprKind::DataInit {
                data_struct_id: self.instantiate_anon_enum(rewrite.exp_base),
                tag_value: rewrite.tag_value,
                payload: Box::new(payload),
            },
            rewrite.span,
        ))
    }

    fn anon_enum_payload_ty_for_tag(
        &mut self,
        anon_enum: &kernc_sema::ty::AnonymousEnum,
        expected_tag: i128,
    ) -> Option<Option<TypeId>> {
        let mut current_tag = 0_i128;
        for variant in &anon_enum.variants {
            if let Some(explicit) = variant.explicit_value {
                current_tag = explicit;
            }
            if current_tag == expected_tag {
                return Some(variant.payload_ty);
            }
            current_tag += 1;
        }
        None
    }

    pub(crate) fn lower_trait_object_upcast(
        &mut self,
        source_expr: MastExpr,
        target_ptr_ty: TypeId,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
        span: Span,
    ) -> MastExpr {
        let source_trait_norm = self.ctx.type_registry.normalize(source_trait_ty);
        let target_trait_norm = self.ctx.type_registry.normalize(target_trait_ty);

        let data_ptr = MastExpr::new(
            self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut: false,
                elem: TypeId::VOID,
            }),
            MastExprKind::ExtractFatPtrData(Box::new(source_expr.clone())),
            span,
        );

        let meta_expr = if source_trait_norm == target_trait_norm {
            MastExpr::new(
                TypeId::USIZE,
                MastExprKind::ExtractFatPtrMeta(Box::new(source_expr)),
                span,
            )
        } else if self.trait_object_satisfies_required(source_trait_norm, target_trait_norm) {
            // Forgetting extra assoc bindings does not require rebuilding the
            // vtable; the existing metadata still points at the same receiver
            // implementation and method layout.
            MastExpr::new(
                TypeId::USIZE,
                MastExprKind::ExtractFatPtrMeta(Box::new(source_expr)),
                span,
            )
        } else {
            let Some(super_slot) =
                self.vtable_supertrait_slot(source_trait_norm, target_trait_norm)
            else {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "cannot upcast trait object from `{}` to unrelated trait `{}`",
                            self.ctx.ty_to_string(source_trait_norm),
                            self.ctx.ty_to_string(target_trait_norm),
                        ),
                    )
                    .emit();
                return MastExpr::new(target_ptr_ty, MastExprKind::Trap, span);
            };

            let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut: false,
                elem: TypeId::VOID,
            });
            let vtable_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                is_mut: false,
                elem: void_ptr_ty,
            });

            let source_vtable_meta = MastExpr::new(
                TypeId::USIZE,
                MastExprKind::ExtractFatPtrMeta(Box::new(source_expr)),
                span,
            );
            let source_vtable_ptr = MastExpr::new(
                vtable_ptr_ty,
                MastExprKind::Cast {
                    kind: MastCastKind::IntToPtr,
                    operand: Box::new(source_vtable_meta),
                },
                span,
            );
            let target_vtable_raw = MastExpr::new(
                void_ptr_ty,
                MastExprKind::IndexAccess {
                    lhs: Box::new(source_vtable_ptr),
                    index: Box::new(MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Integer(super_slot as u128),
                        span,
                    )),
                },
                span,
            );

            MastExpr::new(
                TypeId::USIZE,
                MastExprKind::Cast {
                    kind: MastCastKind::PtrToInt,
                    operand: Box::new(target_vtable_raw),
                },
                span,
            )
        };

        MastExpr::new(
            target_ptr_ty,
            MastExprKind::ConstructFatPointer {
                data_ptr: Box::new(data_ptr),
                meta: Box::new(meta_expr),
            },
            span,
        )
    }

    pub(crate) fn determine_cast_kind(&mut self, from: TypeId, to: TypeId) -> MastCastKind {
        let f_norm = self.ctx.type_registry.normalize(from);
        let t_norm = self.ctx.type_registry.normalize(to);

        // `bool` lowers like an integer for cast purposes.
        let f_int = self.ctx.type_registry.is_integer(f_norm) || f_norm == TypeId::BOOL;
        let t_int = self.ctx.type_registry.is_integer(t_norm);

        let f_float = self.ctx.type_registry.is_float(f_norm);
        let t_float = self.ctx.type_registry.is_float(t_norm);

        let f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer { .. }
                | TypeKind::VolatilePtr { .. }
                | TypeKind::Function { .. }
                | TypeKind::FnDef(..)
        );
        let t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer { .. }
                | TypeKind::VolatilePtr { .. }
                | TypeKind::Function { .. }
                | TypeKind::FnDef(..)
        );

        // 1. Pointer/integer casts preserve raw bit patterns.
        if f_ptr && t_ptr {
            return MastCastKind::Bitcast;
        }
        if f_int && t_ptr {
            return MastCastKind::IntToPtr;
        }
        if f_ptr && t_int {
            return MastCastKind::PtrToInt;
        }

        // 2. Refined integer-to-integer conversion.
        if f_int && t_int {
            return self.determine_int_cast_kind(f_norm, t_norm);
        }

        // 3. Floating-point precision conversions (`f32 <-> f64`).
        if f_float && t_float {
            return MastCastKind::FloatCast;
        }

        // 4. Integer-to-float conversion.
        if f_int && t_float {
            let is_signed = matches!(
                self.ctx.type_registry.get(f_norm),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );
            return if is_signed {
                MastCastKind::SIntToFloat
            } else {
                MastCastKind::UIntToFloat
            };
        }

        // 5. Float-to-integer conversion.
        if f_float && t_int {
            let is_signed = matches!(
                self.ctx.type_registry.get(t_norm),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );
            return if is_signed {
                MastCastKind::FloatToSInt
            } else {
                MastCastKind::FloatToUInt
            };
        }

        // Conservative fallback.
        MastCastKind::Bitcast
    }

    /// Handle integer-to-integer conversion details.
    pub(crate) fn determine_int_cast_kind(&mut self, from: TypeId, to: TypeId) -> MastCastKind {
        let mut le = LayoutEngine::new(self.ctx);
        let f_size = le.compute_type_size(from);
        let t_size = le.compute_type_size(to);

        if f_size > t_size {
            MastCastKind::Trunc
        } else if f_size < t_size {
            // Detect whether the destination integer type is signed.
            let is_signed = matches!(
                self.ctx.type_registry.get(to),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );
            if is_signed {
                MastCastKind::SignExt
            } else {
                MastCastKind::ZeroExt
            }
        } else {
            // Equal-width casts simply reinterpret the same bit width.
            MastCastKind::Bitcast
        }
    }
}
