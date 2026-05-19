//! ABI classification helpers.
//!
//! These helpers decide how values are passed/returned at the LLVM boundary,
//! including direct values, indirect sret-style returns, aggregate payloads, and
//! platform-visible function signatures.

use super::CodeGenerator;
use crate::AtomicOrdering as LlvmAtomicOrdering;
use crate::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, IntType};
use crate::values::{BasicValueEnum, PointerValue};
use kernc_mono::MonoId;
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, Span};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn llvm_atomic_ordering(ordering: AtomicOrdering) -> LlvmAtomicOrdering {
        match ordering {
            AtomicOrdering::Relaxed => LlvmAtomicOrdering::Monotonic,
            AtomicOrdering::Acquire => LlvmAtomicOrdering::Acquire,
            AtomicOrdering::Release => LlvmAtomicOrdering::Release,
            AtomicOrdering::AcqRel => LlvmAtomicOrdering::AcquireRelease,
            AtomicOrdering::SeqCst => LlvmAtomicOrdering::SequentiallyConsistent,
        }
    }

    pub(crate) fn struct_field_index_by_name(
        &mut self,
        struct_id: MonoId,
        name: &str,
    ) -> Option<u32> {
        let Some(fields) = self.struct_fields.get(&struct_id).cloned() else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Kern ICE (Codegen): missing field metadata for struct MonoId {:?}.",
                    struct_id
                ),
            );
            return None;
        };

        for (idx, field) in fields.iter().enumerate() {
            if self.resolve_symbol(*field) == name {
                return Some(idx as u32);
            }
        }

        self.sess.emit_ice(
            Span::default(),
            format!(
                "Kern ICE (Codegen): field `{}` not found in struct MonoId {:?}.",
                name, struct_id
            ),
        );
        None
    }

    pub(crate) fn atomic_xchg_pointer_width_int(&self) -> IntType<'ctx> {
        self.context
            .custom_width_int_type((self.sess.target.pointer_size * 8) as u32)
    }

    pub(crate) fn llvm_fn_type_from_callable(
        &mut self,
        callee_ty: TypeId,
        span: Span,
    ) -> Option<FunctionType<'ctx>> {
        let norm_ty = self.type_registry.normalize(callee_ty);
        let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = self.type_registry.get(norm_ty)
        else {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): indirect call expected function type, found `{:?}`.",
                    self.type_registry.get(norm_ty)
                ),
            );
            return None;
        };

        let mut param_types = Vec::new();
        for p in params {
            param_types.push(self.get_llvm_type(*p));
        }

        let fn_ty = if *ret == TypeId::VOID {
            self.context.void_type().fn_type(&param_types, *is_variadic)
        } else {
            match self.get_llvm_type(*ret) {
                BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::ScalableVectorType(v) => v.fn_type(&param_types, *is_variadic),
            }
        };

        Some(fn_ty)
    }

    fn inline_asm_fn_type(
        &mut self,
        output_tys: &[TypeId],
        param_types: &[BasicMetadataTypeEnum<'ctx>],
    ) -> FunctionType<'ctx> {
        match output_tys.len() {
            0 => self.context.void_type().fn_type(param_types, false),
            1 => match self.get_llvm_type(output_tys[0]) {
                BasicTypeEnum::IntType(i) => i.fn_type(param_types, false),
                BasicTypeEnum::FloatType(f) => f.fn_type(param_types, false),
                BasicTypeEnum::PointerType(p) => p.fn_type(param_types, false),
                BasicTypeEnum::StructType(s) => s.fn_type(param_types, false),
                BasicTypeEnum::ArrayType(a) => a.fn_type(param_types, false),
                BasicTypeEnum::VectorType(v) => v.fn_type(param_types, false),
                BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(param_types, false),
            },
            _ => {
                let mut struct_fields = Vec::new();
                for &ty in output_tys {
                    struct_fields.push(self.get_llvm_type(ty));
                }
                self.context
                    .struct_type(&struct_fields, false)
                    .fn_type(param_types, false)
            }
        }
    }

    pub(crate) fn compile_inline_asm_parts(
        &mut self,
        asm_template: &str,
        constraints: &str,
        input_args: &[BasicValueEnum<'ctx>],
        output_ptrs: &[PointerValue<'ctx>],
        output_tys: &[TypeId],
        is_volatile: bool,
    ) {
        let param_types = input_args
            .iter()
            .map(|arg| arg.get_type())
            .collect::<Vec<_>>();
        let asm_fn_type = self.inline_asm_fn_type(output_tys, &param_types);
        let has_side_effects = is_volatile || output_tys.is_empty();
        let inline_asm = self.context.create_inline_asm(
            asm_fn_type,
            asm_template.to_string(),
            constraints.to_string(),
            crate::llvm_api::InlineAsmOptions {
                sideeffects: has_side_effects,
                alignstack: false,
                dialect: Some(self.asm_dialect),
                can_throw: false,
            },
        );

        let call_site = self
            .builder
            .build_indirect_call(asm_fn_type, inline_asm, input_args, "asm_call")
            .unwrap();

        if !output_tys.is_empty() {
            let result_ty = asm_fn_type
                .get_return_type()
                .unwrap_or_else(|| self.context.i8_type().into());
            let asm_result =
                self.expect_call_result(call_site, result_ty, Span::default(), "inline asm");
            for (i, target_ptr) in output_ptrs.iter().enumerate() {
                let extracted_val = if output_tys.len() == 1 {
                    asm_result
                } else {
                    let Some(asm_result_struct) = self.expect_struct_value(
                        asm_result,
                        Span::default(),
                        "multi-output inline asm result",
                    ) else {
                        return;
                    };
                    self.builder
                        .build_extract_value(asm_result_struct, i as u32, &format!("asm_out_{}", i))
                        .unwrap()
                };
                self.builder
                    .build_store(*target_ptr, extracted_val)
                    .unwrap();
            }
        }
    }
}
