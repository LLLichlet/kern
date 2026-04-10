// compiler/kernc_codegen/src/codegen/expr/literal.rs

use crate::codegen::CodeGenerator;
use crate::llvm_api::AsTypeRef;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use kernc_mast::{MastExpr, MonoId};
use kernc_sema::ty::TypeId;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn scalar_bit_width_of_type(&self, ty: BasicTypeEnum<'ctx>) -> Option<u64> {
        match ty {
            BasicTypeEnum::IntType(int_ty) => Some(int_ty.bit_width() as u64),
            BasicTypeEnum::FloatType(_) => Some(
                match unsafe { llvm_sys::core::LLVMGetTypeKind(ty.as_type_ref()) } {
                    llvm_sys::LLVMTypeKind::LLVMFloatTypeKind => 32,
                    llvm_sys::LLVMTypeKind::LLVMDoubleTypeKind => 64,
                    _ => return None,
                },
            ),
            BasicTypeEnum::PointerType(_) => Some(self.sess.target.pointer_size * 8),
            _ => None,
        }
    }

    fn pack_union_storage_chunk(
        &mut self,
        value: BasicValueEnum<'ctx>,
        target_ty: crate::types::IntType<'ctx>,
        name: &str,
    ) -> Option<BasicValueEnum<'ctx>> {
        let value_bits = self.scalar_bit_width_of_type(value.get_type())?;
        let target_bits = target_ty.bit_width() as u64;
        if value_bits != target_bits {
            return None;
        }

        match value {
            BasicValueEnum::IntValue(int_val) => {
                if int_val.get_type() == target_ty {
                    Some(int_val.into())
                } else {
                    self.builder.build_bit_cast(int_val, target_ty, name).ok()
                }
            }
            BasicValueEnum::FloatValue(float_val) => {
                self.builder.build_bit_cast(float_val, target_ty, name).ok()
            }
            BasicValueEnum::PointerValue(ptr_val) => self
                .builder
                .build_ptr_to_int(ptr_val, target_ty, name)
                .ok()
                .map(Into::into),
            _ => None,
        }
    }

    fn pack_union_storage_array_value(
        &mut self,
        array_ty: crate::types::ArrayType<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        let elem_ty = array_ty.get_element_type().into_int_type();
        let mut array_val = array_ty.get_undef();

        match value {
            BasicValueEnum::StructValue(struct_val) => {
                let struct_ty = struct_val.get_type();
                if struct_ty.count_fields() > array_ty.len() {
                    return None;
                }
                for idx in 0..struct_ty.count_fields() {
                    let field_val = self
                        .builder
                        .build_extract_value(struct_val, idx, "union_field")
                        .ok()?;
                    let chunk = self.pack_union_storage_chunk(field_val, elem_ty, "union_chunk")?;
                    array_val = self
                        .builder
                        .build_insert_value(array_val, chunk, idx, "union_array")
                        .ok()?
                        .into_array_value();
                }
                Some(array_val.into())
            }
            BasicValueEnum::ArrayValue(array_val_in) => {
                let value_ty = array_val_in.get_type();
                if value_ty.len() > array_ty.len() {
                    return None;
                }
                for idx in 0..value_ty.len() {
                    let elem_val = self
                        .builder
                        .build_extract_value(array_val_in, idx, "union_elem")
                        .ok()?;
                    let chunk = self.pack_union_storage_chunk(elem_val, elem_ty, "union_chunk")?;
                    array_val = self
                        .builder
                        .build_insert_value(array_val, chunk, idx, "union_array")
                        .ok()?
                        .into_array_value();
                }
                Some(array_val.into())
            }
            value => {
                let chunk = self.pack_union_storage_chunk(value, elem_ty, "union_chunk")?;
                Some(
                    self.builder
                        .build_insert_value(array_val, chunk, 0, "union_array")
                        .ok()?
                        .into_array_value()
                        .into(),
                )
            }
        }
    }

    fn pack_union_runtime_value(
        &mut self,
        union_llvm_ty: crate::types::StructType<'ctx>,
        value: BasicValueEnum<'ctx>,
    ) -> Option<BasicValueEnum<'ctx>> {
        if union_llvm_ty.count_fields() != 1 {
            return None;
        }
        let field_ty = union_llvm_ty.get_field_type_at_index(0)?;
        if field_ty != value.get_type() {
            let storage_value = match field_ty {
                BasicTypeEnum::ArrayType(array_ty) => {
                    self.pack_union_storage_array_value(array_ty, value)?
                }
                _ => return None,
            };

            return Some(
                self.builder
                    .build_insert_value(union_llvm_ty.get_undef(), storage_value, 0, "union_insert")
                    .ok()?
                    .into_struct_value()
                    .into(),
            );
        }

        Some(
            self.builder
                .build_insert_value(union_llvm_ty.get_undef(), value, 0, "union_insert")
                .ok()?
                .into_struct_value()
                .into(),
        )
    }

    pub(crate) fn compile_struct_init(
        &mut self,
        struct_id: MonoId,
        fields: &[MastExpr],
    ) -> BasicValueEnum<'ctx> {
        let struct_llvm_ty = *self.structs.get(&struct_id).unwrap();
        let mut current_struct = struct_llvm_ty
            .as_basic_type_enum()
            .into_struct_type()
            .const_zero();

        for (idx, field_expr) in fields.iter().enumerate() {
            let field_val = self.compile_expr(field_expr);
            if self.current_block_is_terminated() {
                return struct_llvm_ty.as_basic_type_enum().const_zero();
            }
            current_struct = self
                .builder
                .build_insert_value(current_struct, field_val, idx as u32, "s_init")
                .unwrap()
                .into_struct_value();
        }
        current_struct.into()
    }

    pub(crate) fn compile_union_init(
        &mut self,
        union_id: MonoId,
        value: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let union_llvm_ty = *self.structs.get(&union_id).unwrap();
        let val = self.compile_expr(value);
        if self.current_block_is_terminated() {
            return union_llvm_ty.as_basic_type_enum().const_zero();
        }
        if let Some(packed) = self.pack_union_runtime_value(union_llvm_ty, val) {
            return packed;
        }

        let alloca =
            self.create_entry_block_alloca(union_llvm_ty.as_basic_type_enum(), "union_init");
        self.builder.build_store(alloca, val).unwrap();

        self.builder
            .build_load(union_llvm_ty.as_basic_type_enum(), alloca, "union_load")
            .unwrap()
    }

    /// Compile a payload-carrying enum, which lowers like a tagged union.
    pub(crate) fn compile_data_init(
        &mut self,
        data_struct_id: MonoId,
        tag_value: u128,
        payload: &MastExpr,
    ) -> BasicValueEnum<'ctx> {
        let struct_llvm_ty = *self.structs.get(&data_struct_id).unwrap();

        let tag_llvm_ty = struct_llvm_ty
            .get_field_type_at_index(0)
            .unwrap()
            .into_int_type();
        let tag_val = tag_llvm_ty.const_int(tag_value as u64, false);

        let union_llvm_ty = struct_llvm_ty
            .get_field_type_at_index(1)
            .unwrap()
            .into_struct_type();

        // Store the payload into the union storage.
        let union_val = if payload.ty != TypeId::VOID && payload.ty != TypeId::ERROR {
            let payload_val = self.compile_expr(payload);
            if self.current_block_is_terminated() {
                return struct_llvm_ty.as_basic_type_enum().const_zero();
            }
            if let Some(packed) = self.pack_union_runtime_value(union_llvm_ty, payload_val) {
                packed.into_struct_value()
            } else {
                let union_alloca =
                    self.create_entry_block_alloca(union_llvm_ty.into(), "data_union_init");
                self.builder.build_store(union_alloca, payload_val).unwrap();
                self.builder
                    .build_load(union_llvm_ty, union_alloca, "data_union_load")
                    .unwrap()
                    .into_struct_value()
            }
        } else {
            union_llvm_ty.const_zero()
        };

        // Assemble the final `{ tag, union }` struct.
        let mut data_struct = struct_llvm_ty.const_zero();
        data_struct = self
            .builder
            .build_insert_value(data_struct, tag_val, 0, "data_insert_tag")
            .unwrap()
            .into_struct_value();
        data_struct = self
            .builder
            .build_insert_value(data_struct, union_val, 1, "data_insert_union")
            .unwrap()
            .into_struct_value();

        data_struct.into()
    }

    pub(crate) fn compile_array_init(
        &mut self,
        elems: &[MastExpr],
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        match expected_ty {
            BasicTypeEnum::ArrayType(array_llvm_ty) => {
                let mut current_array = array_llvm_ty.const_zero();
                for (idx, elem_expr) in elems.iter().enumerate() {
                    let elem_val = self.compile_expr(elem_expr);
                    if self.current_block_is_terminated() {
                        return array_llvm_ty.const_zero().into();
                    }
                    current_array = self
                        .builder
                        .build_insert_value(current_array, elem_val, idx as u32, "arr_init")
                        .unwrap()
                        .into_array_value();
                }
                current_array.into()
            }
            BasicTypeEnum::VectorType(vector_llvm_ty) => {
                let mut current_vector = vector_llvm_ty.const_zero().into_vector_value();
                for (idx, elem_expr) in elems.iter().enumerate() {
                    let elem_val = self.compile_expr(elem_expr);
                    if self.current_block_is_terminated() {
                        return vector_llvm_ty.const_zero();
                    }
                    let index = self.context.i32_type().const_int(idx as u64, false);
                    current_vector = self
                        .builder
                        .build_insert_element(current_vector, elem_val, index, "vec_init")
                        .unwrap()
                        .into_vector_value();
                }
                current_vector.into()
            }
            _ => {
                self.sess.emit_ice(
                    kernc_utils::Span::default(),
                    "Kern ICE (Codegen): array initializer used with a non-array/non-vector type.",
                );
                expected_ty.const_zero()
            }
        }
    }
}
