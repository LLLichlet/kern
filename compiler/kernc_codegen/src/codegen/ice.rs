//! Backend ICE helpers.
//!
//! Codegen should not silently fabricate LLVM for invalid lowered input. These
//! helpers centralize backend internal-error reporting and trap generation.

use super::CodeGenerator;
use crate::intrinsics::Intrinsic;
use crate::llvm_api::{
    BasicTypeEnum, BasicValueEnum, FloatType, FloatValue, FunctionValue, IntType, IntValue,
    PointerType, PointerValue, StructType, StructValue, VectorValue,
};
use kernc_utils::Span;

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub(crate) fn lookup_intrinsic_declaration(
        &mut self,
        name: &'static str,
        types: &[BasicTypeEnum<'ctx>],
        span: Span,
        context: &str,
    ) -> Option<FunctionValue<'ctx>> {
        let Some(intrinsic) = Intrinsic::find(name) else {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): LLVM intrinsic `{}` is not registered while compiling {}.",
                    name, context
                ),
            );
            return None;
        };

        let Some(decl) = intrinsic.get_declaration(&self.module, types) else {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): LLVM declaration for intrinsic `{}` is unavailable while compiling {}.",
                    name, context
                ),
            );
            return None;
        };

        Some(decl)
    }

    pub(crate) fn expect_int_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<IntValue<'ctx>> {
        match value {
            BasicValueEnum::IntValue(value) => Some(value),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected integer LLVM value while compiling {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_float_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<FloatValue<'ctx>> {
        match value {
            BasicValueEnum::FloatValue(value) => Some(value),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected floating-point LLVM value while compiling {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_pointer_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<PointerValue<'ctx>> {
        match value {
            BasicValueEnum::PointerValue(value) => Some(value),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected pointer LLVM value while compiling {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_struct_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<StructValue<'ctx>> {
        match value {
            BasicValueEnum::StructValue(value) => Some(value),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected struct LLVM value while compiling {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_vector_value(
        &mut self,
        value: BasicValueEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<VectorValue<'ctx>> {
        match value {
            BasicValueEnum::VectorValue(value) => Some(value),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected vector LLVM value while compiling {}, found {:?}.",
                        context,
                        other.get_type()
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_int_type(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<IntType<'ctx>> {
        match ty {
            BasicTypeEnum::IntType(ty) => Some(ty),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected integer LLVM type while compiling {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_float_type(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<FloatType<'ctx>> {
        match ty {
            BasicTypeEnum::FloatType(ty) => Some(ty),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected floating-point LLVM type while compiling {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_pointer_type(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<PointerType<'ctx>> {
        match ty {
            BasicTypeEnum::PointerType(ty) => Some(ty),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected pointer LLVM type while compiling {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn expect_struct_type(
        &mut self,
        ty: BasicTypeEnum<'ctx>,
        span: Span,
        context: &str,
    ) -> Option<StructType<'ctx>> {
        match ty {
            BasicTypeEnum::StructType(ty) => Some(ty),
            other => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): expected struct LLVM type while compiling {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
        }
    }
}
