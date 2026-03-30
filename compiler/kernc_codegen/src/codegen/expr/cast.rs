use crate::AddressSpace;
use crate::codegen::CodeGenerator;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use kernc_mast::{MastCastKind, MastExpr, MastExprKind};
use kernc_sema::ty::TypeKind;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn compile_cast(
        &mut self,
        kind: MastCastKind,
        operand: &MastExpr,
        target_llvm_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val = self.compile_expr(operand);
        match kind {
            MastCastKind::Bitcast => {
                if val.is_struct_value() && target_llvm_ty.is_pointer_type() {
                    let fat_ptr = val.into_struct_value();
                    self.builder
                        .build_extract_value(fat_ptr, 0, "slice_ptr_fallback")
                        .unwrap()
                        .into_pointer_value()
                        .into()
                } else {
                    self.builder
                        .build_bit_cast(val, target_llvm_ty, "bitcast")
                        .unwrap()
                }
            }
            MastCastKind::PtrToInt => self
                .builder
                .build_ptr_to_int(
                    val.into_pointer_value(),
                    target_llvm_ty.into_int_type(),
                    "ptr2int",
                )
                .unwrap()
                .into(),
            MastCastKind::IntToPtr => self
                .builder
                .build_int_to_ptr(
                    val.into_int_value(),
                    target_llvm_ty.into_pointer_type(),
                    "int2ptr",
                )
                .unwrap()
                .into(),
            MastCastKind::ZeroExt => self
                .builder
                .build_int_z_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "zext")
                .unwrap()
                .into(),
            MastCastKind::SignExt => self
                .builder
                .build_int_s_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "sext")
                .unwrap()
                .into(),
            MastCastKind::Trunc => self
                .builder
                .build_int_truncate(
                    val.into_int_value(),
                    target_llvm_ty.into_int_type(),
                    "trunc",
                )
                .unwrap()
                .into(),
            MastCastKind::SIntToFloat => self
                .builder
                .build_signed_int_to_float(
                    val.into_int_value(),
                    target_llvm_ty.into_float_type(),
                    "sitofp",
                )
                .unwrap()
                .into(),
            MastCastKind::UIntToFloat => self
                .builder
                .build_unsigned_int_to_float(
                    val.into_int_value(),
                    target_llvm_ty.into_float_type(),
                    "uitofp",
                )
                .unwrap()
                .into(),
            MastCastKind::FloatToSInt => self
                .builder
                .build_float_to_signed_int(
                    val.into_float_value(),
                    target_llvm_ty.into_int_type(),
                    "fptosi",
                )
                .unwrap()
                .into(),
            MastCastKind::FloatToUInt => self
                .builder
                .build_float_to_unsigned_int(
                    val.into_float_value(),
                    target_llvm_ty.into_int_type(),
                    "fptoui",
                )
                .unwrap()
                .into(),
            MastCastKind::FloatCast => self
                .builder
                .build_float_cast(
                    val.into_float_value(),
                    target_llvm_ty.into_float_type(),
                    "fcast",
                )
                .unwrap()
                .into(),

            // ArrayDecay: [N]T -> []T (将数组隐式转换为带长度的胖指针)
            MastCastKind::ArrayToSlice => {
                // 临时变量具象化 (Materialize Temporary)
                let array_ptr = match &operand.kind {
                    // 如果本身就是合法的左值（比如变量名），直接取它的地址，避免无意义的拷贝
                    MastExprKind::Var(_)
                    | MastExprKind::GlobalRef(_)
                    | MastExprKind::FieldAccess { .. }
                    | MastExprKind::IndexAccess { .. }
                    | MastExprKind::Deref(_) => self.compile_lvalue(operand),
                    // 如果是右值（比如 ArrayInit 临时数组），在栈上开辟临时空间存进去
                    _ => {
                        let array_val = self.compile_expr(operand);
                        let array_llvm_ty = self.get_llvm_type(operand.ty);
                        let temp_ptr =
                            self.create_entry_block_alloca(array_llvm_ty, "tmp_array_for_slice");
                        self.builder.build_store(temp_ptr, array_val).unwrap();
                        temp_ptr
                    }
                };

                // 获取长度
                let array_len = if let TypeKind::Array { len, .. } = self
                    .type_registry
                    .get(self.type_registry.normalize(operand.ty))
                {
                    *len
                } else {
                    self.sess.emit_ice(
                        operand.span,
                        format!(
                            "Kern ICE (Codegen): Expected array operand for ArrayToSlice cast, found {:?}.",
                            self.type_registry
                                .get(self.type_registry.normalize(operand.ty))
                        ),
                    );
                    0
                };

                // 组装 Slice 胖指针
                let slice_llvm_ty = target_llvm_ty.into_struct_type();
                let mut slice_val = slice_llvm_ty.get_undef();

                slice_val = self
                    .builder
                    .build_insert_value(slice_val, array_ptr, 0, "slice_ptr")
                    .unwrap()
                    .into_struct_value();

                let len_val = self.context.i64_type().const_int(array_len as u64, false);
                slice_val = self
                    .builder
                    .build_insert_value(slice_val, len_val, 1, "slice_len")
                    .unwrap()
                    .into_struct_value();

                slice_val.into()
            }
        }
    }

    pub(crate) fn compile_construct_fat_ptr(
        &mut self,
        data_ptr: &MastExpr,
        meta: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let ptr_ty = self.context.ptr_type(AddressSpace::default());
        let len_ty = self.context.i64_type();
        let fat_ptr_ty = self
            .context
            .struct_type(&[ptr_ty.into(), len_ty.into()], false);

        let mut fat_ptr = fat_ptr_ty.const_zero();

        let data_val = self.compile_expr(data_ptr);
        fat_ptr = self
            .builder
            .build_insert_value(fat_ptr, data_val, 0, "fat_data")
            .unwrap()
            .into_struct_value();

        let meta_val = self.compile_expr(meta);
        fat_ptr = self
            .builder
            .build_insert_value(fat_ptr, meta_val, 1, "fat_meta")
            .unwrap()
            .into_struct_value();

        fat_ptr.into()
    }

    pub(crate) fn compile_extract_fat_ptr(
        &mut self,
        fat_ptr_expr: &MastExpr,
        index: u32,
        name: &str,
    ) -> BasicValueEnum<'ctx> {
        let fat_ptr_val = self.compile_expr(fat_ptr_expr).into_struct_value();
        self.builder
            .build_extract_value(fat_ptr_val, index, name)
            .unwrap()
    }
}
