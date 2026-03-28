// compiler/kernc_lower/src/expr/cast.rs

use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_as_expr(
        &mut self,
        lhs: &Expr,
        target: &ast::TypeNode,
        concrete_ty: TypeId,
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExpr {
        let target_ty = self
            .ctx
            .node_types
            .get(&target.id)
            .copied()
            .unwrap_or(concrete_ty);
        let l = self.lower_expr(lhs, subst_map, None);
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

        // 1. Array 到 Slice 的隐式转换
        if let TypeKind::Slice { .. } = exp_kind
            && let TypeKind::Array { .. } = conc_kind
        {
            mast_kind = MastExprKind::Cast {
                kind: MastCastKind::ArrayToSlice,
                operand: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
            };
            return MastExpr::new(exp_ty, mast_kind, span);
        }

        // 2. 指针隐式转换为 Trait Object 胖指针
        if let TypeKind::Pointer { elem: e_inner, .. } = exp_kind {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm)
                && let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } = conc_kind
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

                let vtable_id = self.get_or_create_vtable(concrete_ty, e_inner_norm);
                let Some(meta_expr) = self.vtable_global_meta_expr(vtable_id, span) else {
                    return MastExpr::new(exp_ty, MastExprKind::Trap, span);
                };

                return MastExpr::new(
                    exp_ty,
                    MastExprKind::ConstructFatPointer {
                        data_ptr: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
                        meta: Box::new(meta_expr),
                    },
                    span,
                );
            }
        }

        // 3. 裸值隐式取址并打包为 Trait Object 胖指针 (BNC: T -> *Trait / T -> *mut Trait)
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm)
                && !matches!(
                    conc_kind,
                    TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
                )
            {
                let ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: e_mut,
                    elem: concrete_ty,
                });

                // 核心：无中生有，原地包装一个取址操作 (AddressOf)
                let data_ptr_expr = MastExpr::new(
                    ptr_ty,
                    MastExprKind::AddressOf(Box::new(MastExpr::new(concrete_ty, mast_kind, span))),
                    span,
                );

                // 剩下的逻辑和普通指针打包完全一样，提取 VTable
                let vtable_id = self.get_or_create_vtable(concrete_ty, e_inner_norm);
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

        // 4. 闭包 BNC: 函数/匿名状态 -> 闭包胖指针 (*Fn)
        if let TypeKind::Pointer { elem: e_inner, .. } = exp_kind {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::ClosureInterface { .. } = self.ctx.type_registry.get(e_inner_norm) {
                // 4.1 普通无状态函数 (FnDef / Function) -> 闭包胖指针
                if matches!(conc_kind, TypeKind::FnDef(..) | TypeKind::Function { .. }) {
                    // 数据指针 (data_ptr) 为 NULL
                    let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem: TypeId::VOID,
                    });
                    let null_ptr_expr = MastExpr::new(
                        void_ptr_ty,
                        MastExprKind::Cast {
                            kind: MastCastKind::IntToPtr,
                            operand: Box::new(MastExpr::new(
                                TypeId::USIZE,
                                MastExprKind::Integer(0),
                                span,
                            )),
                        },
                        span,
                    );

                    // 元数据 (meta) 为函数指针，转为 usize
                    let fn_ptr_expr = MastExpr::new(concrete_ty, mast_kind, span);
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
                            data_ptr: Box::new(null_ptr_expr),
                            meta: Box::new(meta_expr),
                        },
                        span,
                    );
                }

                // 4.2 匿名闭包状态结构体 (AnonymousState) -> 闭包胖指针
                if let TypeKind::AnonymousState {
                    closure_node_id, ..
                } = conc_kind
                {
                    // 数据指针 (data_ptr) 为 隐式取址 (AddressOf)
                    let ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: true, // 闭包状态通常需要修改
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

                    // 元数据 (meta) 为闭包实际执行函数的指针
                    // 注: 这里调用 lower_closure_expr 生成/缓存的包装函数 MonoId
                    let func_mono_id = self.get_closure_func_mono_id(closure_node_id);
                    // 我们只需构造出 FuncRef 即可
                    let fn_ptr_expr = MastExpr::new(
                        TypeId::VOID, // 此时由于只需强转 usize，随便塞个虚假类型也可以过 codegen，不过最好保持规范
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

        // 兜底返回原样 (如果遇到其他类型在 Sema 被判合法，但 Lowering 不需要干预)
        MastExpr::new(exp_ty, mast_kind, span)
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
        } else {
            let Some(super_slot) =
                self.vtable_supertrait_slot(source_trait_norm, target_trait_norm)
            else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): trait object upcast target `{}` is not a parent of `{}`.",
                        self.ctx.ty_to_string(target_trait_norm),
                        self.ctx.ty_to_string(source_trait_norm),
                    ),
                );
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

        // bool 在底层可以视同整数进行转换
        let f_int = self.ctx.type_registry.is_integer(f_norm) || f_norm == TypeId::BOOL;
        let t_int = self.ctx.type_registry.is_integer(t_norm);

        let f_float = self.ctx.type_registry.is_float(f_norm);
        let t_float = self.ctx.type_registry.is_float(t_norm);

        let f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );
        let t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        );

        // 1. 指针与整数的相互转换 (Bit-pattern preserving)
        if f_ptr && t_ptr {
            return MastCastKind::Bitcast;
        }
        if f_int && t_ptr {
            return MastCastKind::IntToPtr;
        }
        if f_ptr && t_int {
            return MastCastKind::PtrToInt;
        }

        // 2. 整数到整数的精细转换
        if f_int && t_int {
            return self.determine_int_cast_kind(f_norm, t_norm);
        }

        // 3. 浮点数之间的精度转换 (f32 <-> f64)
        if f_float && t_float {
            return MastCastKind::FloatCast;
        }

        // 4. 整数 到 浮点数 (sitofp / uitofp)
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

        // 5. 浮点数 到 整数 (fptosi / fptoui)
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

        // 兜底
        MastCastKind::Bitcast
    }

    /// 专门处理整数之间的转换逻辑 (未来应由 @zext, @truncate 等内置函数直接调用此逻辑)
    pub(crate) fn determine_int_cast_kind(&mut self, from: TypeId, to: TypeId) -> MastCastKind {
        let mut le = LayoutEngine::new(self.ctx);
        let f_size = le.compute_type_size(from);
        let t_size = le.compute_type_size(to);

        if f_size > t_size {
            MastCastKind::Trunc
        } else if f_size < t_size {
            // 判断目标类型是否为有符号整数
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
            // 大小相等 (例如 i32 到 u32，或者 i64 到 usize 在 64位机器上)
            MastCastKind::Bitcast
        }
    }
}
