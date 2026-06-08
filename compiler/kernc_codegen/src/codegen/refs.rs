//! Global and function reference lookup.
//!
//! This module resolves MIR ids and names to LLVM values, creating declarations
//! or reporting backend ICEs when the lowering/codegen symbol maps disagree.

use super::CodeGenerator;
use crate::types::{BasicTypeEnum, StructType};
use crate::values::{BasicValueEnum, FunctionValue, PointerValue};
use kernc_mono::MonoId;
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn null_ptr(&self) -> PointerValue<'ctx> {
        self.context.ptr_type(Default::default()).const_zero()
    }

    pub(crate) fn lookup_struct_type(
        &mut self,
        struct_id: MonoId,
        span: Span,
        context: &str,
    ) -> Option<StructType<'ctx>> {
        match self.structs.get(&struct_id).copied() {
            Some(ty) => Some(ty),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): missing struct MonoId {:?} while compiling {}.",
                        struct_id, context
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn compile_global_ref(
        &mut self,
        mono_id: MonoId,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let Some(global_val) = self.globals.get(&mono_id) else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Global MonoId {:?} not found during code generation",
                    mono_id
                ),
            );
            return expected_ty.const_zero();
        };
        let ptr = global_val.as_pointer_value();
        self.builder
            .build_load(expected_ty, ptr, "global_load")
            .unwrap()
    }

    pub(crate) fn compile_func_ref(&mut self, mono_id: MonoId) -> BasicValueEnum<'ctx> {
        let Some(func_val) = self.functions.get(&mono_id) else {
            self.sess.emit_ice(
                Span::default(),
                format!(
                    "Function MonoId {:?} not found during code generation",
                    mono_id
                ),
            );
            return self
                .context
                .ptr_type(Default::default())
                .const_zero()
                .into();
        };
        func_val.as_global_value().as_pointer_value().into()
    }

    pub(crate) fn lookup_function_value(
        &mut self,
        mono_id: MonoId,
        span: Span,
    ) -> Option<FunctionValue<'ctx>> {
        match self.functions.get(&mono_id).copied() {
            Some(func) => Some(func),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): function MonoId {:?} not declared before call emission.",
                        mono_id
                    ),
                );
                None
            }
        }
    }
}
