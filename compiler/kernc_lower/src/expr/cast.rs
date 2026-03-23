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
        if let TypeKind::Slice { .. } = exp_kind {
            if let TypeKind::Array { .. } = conc_kind {
                mast_kind = MastExprKind::Cast {
                    kind: MastCastKind::ArrayToSlice,
                    operand: Box::new(MastExpr::new(concrete_ty, mast_kind, span)),
                };
                return MastExpr::new(exp_ty, mast_kind, span);
            }
        }

        // 2. 具体类型指针隐式转换为 Trait Object 胖指针
        if let TypeKind::Pointer { elem: e_inner, .. } = exp_kind {
            let e_inner_norm = self.ctx.type_registry.normalize(e_inner);
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm) {
                if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } = conc_kind {
                    let vtable_id = self.get_or_create_vtable(concrete_ty, e_inner_norm);

                    let global_array_ty = match self
                        .module
                        .globals
                        .iter()
                        .find(|g| g.id == vtable_id)
                    {
                        Some(g) => g.ty,
                        None => {
                            self.ctx.emit_ice(span, "Kern ICE (Lowering): VTable global generated but not found in module globals map.");
                            unreachable!()
                        }
                    };
                    let array_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: false,
                        elem: global_array_ty,
                    });

                    let concrete_expr = MastExpr::new(concrete_ty, mast_kind, span);
                    let meta_expr = MastExpr::new(
                        TypeId::USIZE,
                        MastExprKind::Cast {
                            kind: MastCastKind::PtrToInt,
                            operand: Box::new(MastExpr::new(
                                array_ptr_ty,
                                MastExprKind::AddressOf(Box::new(MastExpr::new(
                                    global_array_ty,
                                    MastExprKind::GlobalRef(vtable_id),
                                    span,
                                ))),
                                span,
                            )),
                        },
                        span,
                    );

                    return MastExpr::new(
                        exp_ty,
                        MastExprKind::ConstructFatPointer {
                            data_ptr: Box::new(concrete_expr),
                            meta: Box::new(meta_expr),
                        },
                        span,
                    );
                }
            }
        }

        // 兜底返回
        MastExpr::new(exp_ty, mast_kind, span)
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
