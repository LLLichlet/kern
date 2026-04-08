// compiler/kernc_codegen/src/codegen/expr/literal.rs

use crate::codegen::CodeGenerator;
use crate::types::BasicTypeEnum;
use crate::values::BasicValueEnum;
use kernc_mast::{MastExpr, MonoId};
use kernc_sema::ty::TypeId;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
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
            return None;
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

        let union_llvm_ty = struct_llvm_ty.get_field_type_at_index(1).unwrap().into_struct_type();

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
        let array_llvm_ty = expected_ty.into_array_type();
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
}
