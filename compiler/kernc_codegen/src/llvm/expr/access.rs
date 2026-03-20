use crate::llvm::CodeGenerator;
use inkwell::types::BasicTypeEnum;
use inkwell::values::{BasicValueEnum, PointerValue};
use kernc_mast::{MastExpr, MastExprKind, MonoId};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::SymbolId;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_lvalue(&mut self, expr: &MastExpr) -> PointerValue<'ctx> {
        match &expr.kind {
            MastExprKind::Var(name) => *self.locals.get(name).expect("Local variable not found"),
            MastExprKind::GlobalRef(mono_id) => self
                .globals
                .get(mono_id)
                .expect("Global not found")
                .as_pointer_value(),
            MastExprKind::FieldAccess {
                lhs,
                struct_id,
                field_idx,
            } => {
                let struct_ptr = self.compile_lvalue(lhs);
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                self.builder
                    .build_struct_gep(*struct_llvm_ty, struct_ptr, *field_idx as u32, "lvalue_gep")
                    .unwrap()
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let idx_val = self.compile_expr(index).into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);

                if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
                    let slice_val = self.compile_expr(lhs).into_struct_value();
                    let ptr_val = self
                        .builder
                        .build_extract_value(slice_val, 0, "slice_ptr")
                        .unwrap()
                        .into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "slice_lvalue")
                            .unwrap()
                    }
                } else if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } =
                    self.type_registry.get(norm_lhs)
                {
                    let ptr_val = self.compile_expr(lhs).into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe {
                        self.builder
                            .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_lvalue")
                            .unwrap()
                    }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    unsafe {
                        self.builder
                            .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_lvalue")
                            .unwrap()
                    }
                }
            }
            MastExprKind::Deref(operand) => self.compile_expr(operand).into_pointer_value(),
            _ => panic!("Expression is not a valid l-value: {:?}", expr.kind),
        }
    }
    pub(crate) fn compile_var_ref(
        &self,
        name: SymbolId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr = self.locals.get(&name).expect("Local variable not found");
        self.builder
            .build_load(expected_ty, *ptr, &format!("load_{}", name.0))
            .unwrap()
    }

    pub(crate) fn compile_global_ref(
        &self,
        mono_id: MonoId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let global_val = self.globals.get(&mono_id).expect("Global not found");
        let ptr = global_val.as_pointer_value();
        self.builder
            .build_load(expected_ty, ptr, "global_load")
            .unwrap()
    }

    pub(crate) fn compile_func_ref(&self, mono_id: MonoId) -> BasicValueEnum<'ctx> {
        let func_val = self.functions.get(&mono_id).expect("Function not found");
        func_val.as_global_value().as_pointer_value().into()
    }

    pub(crate) fn compile_deref(
        &mut self,
        operand: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(operand).into_pointer_value();
        self.builder
            .build_load(expected_ty, ptr_val, "deref")
            .unwrap()
    }

    pub(crate) fn compile_field_access(
        &mut self,
        lhs: &MastExpr,
        struct_id: MonoId,
        field_idx: usize,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let struct_ptr = self.compile_lvalue(lhs);
        let struct_llvm_ty = self.structs.get(&struct_id).unwrap();
        let is_union = self.union_ids.contains(&struct_id);

        if is_union {
            self.builder
                .build_load(expected_ty, struct_ptr, "union_field_load")
                .unwrap()
        } else {
            let field_ptr = self
                .builder
                .build_struct_gep(*struct_llvm_ty, struct_ptr, field_idx as u32, "field_gep")
                .unwrap();
            self.builder
                .build_load(expected_ty, field_ptr, "field_load")
                .unwrap()
        }
    }

    pub(crate) fn compile_index_access(
        &mut self,
        lhs: &MastExpr,
        index: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
        expr_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let idx_val = self.compile_expr(index).into_int_value();
        let norm_lhs = self.type_registry.normalize(lhs.ty);

        let elem_ptr = if let TypeKind::Slice { .. } = self.type_registry.get(norm_lhs) {
            let slice_val = self.compile_expr(lhs).into_struct_value();
            let ptr_val = self
                .builder
                .build_extract_value(slice_val, 0, "slice_ptr")
                .unwrap()
                .into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "slice_idx")
                    .unwrap()
            }
        } else if let TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } =
            self.type_registry.get(norm_lhs)
        {
            let ptr_val = self.compile_expr(lhs).into_pointer_value();
            let elem_ty = self.get_llvm_type(expr_ty);
            unsafe {
                self.builder
                    .build_gep(elem_ty, ptr_val, &[idx_val], "ptr_idx")
                    .unwrap()
            }
        } else {
            let array_ptr = self.compile_lvalue(lhs);
            let zero = self.context.i64_type().const_zero();
            let array_llvm_ty = self.get_llvm_type(lhs.ty);
            unsafe {
                self.builder
                    .build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_idx")
                    .unwrap()
            }
        };

        self.builder
            .build_load(expected_ty, elem_ptr, "idx_load")
            .unwrap()
    }

    /// 专门处理切片构造 [start..end] 的底层 LLVM 生成
    pub(crate) fn compile_slice_op(
        &mut self,
        lhs: &MastExpr,
        start: Option<&MastExpr>,
        end: Option<&MastExpr>,
        is_inclusive: bool,
        expected_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let lhs_val = self.compile_expr(lhs);
        let norm_lhs = self.type_registry.normalize(lhs.ty);

        // 1. 提取基底指针和基底长度
        let (base_ptr, base_len) = match self.type_registry.get(norm_lhs) {
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. } => {
                // 原始指针没有长度，完全依赖用户提供的 end
                (lhs_val.into_pointer_value(), None)
            }
            TypeKind::Slice { .. } => {
                // 从现有的 Fat Pointer 结构体中提取
                let struct_val = lhs_val.into_struct_value();
                let ptr = self
                    .builder
                    .build_extract_value(struct_val, 0, "s_ptr")
                    .unwrap()
                    .into_pointer_value();
                let len = self
                    .builder
                    .build_extract_value(struct_val, 1, "s_len")
                    .unwrap()
                    .into_int_value();
                (ptr, Some(len))
            }
            TypeKind::Array { len, .. } => {
                // 获取数组的内存地址
                let ptr = if lhs_val.is_pointer_value() {
                    lhs_val.into_pointer_value()
                } else {
                    let alloca = self.create_entry_block_alloca(lhs_val.get_type(), "arr_tmp");
                    self.builder.build_store(alloca, lhs_val).unwrap();
                    alloca
                };
                let len_val = self.context.i64_type().const_int(*len, false);
                (ptr, Some(len_val))
            }
            _ => unreachable!("Invalid base type for slice operation"),
        };

        // 2. 计算 start (缺省为 0)
        let start_val = if let Some(s) = start {
            self.compile_expr(s).into_int_value()
        } else {
            self.context.i64_type().const_zero()
        };

        // 3. 计算 end (缺省为基底长度)
        let end_val = if let Some(e) = end {
            self.compile_expr(e).into_int_value()
        } else {
            base_len.expect("Fatal: slicing a raw pointer without an end index!")
        };

        // 4. 计算新切片的长度: len = end - start + (1 if inclusive)
        let mut slice_len = self
            .builder
            .build_int_sub(end_val, start_val, "slice_len")
            .unwrap();
        if is_inclusive {
            let one = self.context.i64_type().const_int(1, false);
            slice_len = self
                .builder
                .build_int_add(slice_len, one, "slice_len_inc")
                .unwrap();
        }

        // 5. 偏移基底指针: ptr = base_ptr + start
        let elem_ty = match self.type_registry.get(norm_lhs) {
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. } => *elem,
            TypeKind::Array { elem, .. } => *elem,
            _ => unreachable!(),
        };
        let llvm_elem_ty = self.get_llvm_type(elem_ty);

        let slice_ptr = unsafe {
            self.builder
                .build_gep(llvm_elem_ty, base_ptr, &[start_val], "slice_ptr")
                .unwrap()
        };

        // 6. 组装并返回新的胖指针结构体
        let struct_ty = expected_llvm_ty.into_struct_type();
        let mut slice_struct = struct_ty.get_undef();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_ptr, 0, "insert_ptr")
            .unwrap()
            .into_struct_value();
        slice_struct = self
            .builder
            .build_insert_value(slice_struct, slice_len, 1, "insert_len")
            .unwrap()
            .into_struct_value();

        slice_struct.into()
    }
}
