//! Aggregate value helpers.
//!
//! Struct, union, array, enum payload, fat pointer, and anonymous aggregate
//! helpers live here so codegen can construct/extract aggregate values and
//! choose storage layouts consistently.

use super::CodeGenerator;
use crate::llvm_api::AsTypeRef;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn scalar_bit_width_of_type(&self, ty: BasicTypeEnum<'ctx>) -> Option<u64> {
        match ty {
            BasicTypeEnum::IntType(int_ty) => Some(int_ty.bit_width() as u64),
            BasicTypeEnum::FloatType(_) => Some(
                // SAFETY: `ty` is a live LLVM type handle from the current context; querying its
                // kind does not take ownership or mutate the type.
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
        let elem_ty = self.expect_int_type(
            array_ty.get_element_type(),
            Span::default(),
            "union storage array element type",
        )?;
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
                    let inserted = self
                        .builder
                        .build_insert_value(array_val, chunk, idx, "union_array")
                        .ok()?;
                    array_val =
                        self.expect_array_value(inserted, Span::default(), "union struct packing")?;
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
                    let inserted = self
                        .builder
                        .build_insert_value(array_val, chunk, idx, "union_array")
                        .ok()?;
                    array_val =
                        self.expect_array_value(inserted, Span::default(), "union array packing")?;
                }
                Some(array_val.into())
            }
            value => {
                let chunk = self.pack_union_storage_chunk(value, elem_ty, "union_chunk")?;
                let inserted = self
                    .builder
                    .build_insert_value(array_val, chunk, 0, "union_array")
                    .ok()?;
                Some(
                    self.expect_array_value(inserted, Span::default(), "union scalar packing")?
                        .into(),
                )
            }
        }
    }

    pub(crate) fn pack_union_runtime_value(
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

            let inserted = self
                .builder
                .build_insert_value(union_llvm_ty.get_undef(), storage_value, 0, "union_insert")
                .ok()?;
            return Some(
                self.expect_struct_value(inserted, Span::default(), "packed union storage")?
                    .into(),
            );
        }

        let inserted = self
            .builder
            .build_insert_value(union_llvm_ty.get_undef(), value, 0, "union_insert")
            .ok()?;
        Some(
            self.expect_struct_value(inserted, Span::default(), "direct union storage")?
                .into(),
        )
    }
}
