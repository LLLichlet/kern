use llvm_sys::LLVMModuleFlagBehavior;
use llvm_sys::core::{LLVMAddModuleFlag, LLVMValueAsMetadata};
use llvm_sys::debuginfo::{
    LLVMCreateDIBuilder, LLVMDIBuilderCreateAutoVariable, LLVMDIBuilderCreateBasicType,
    LLVMDIBuilderCreateCompileUnit, LLVMDIBuilderCreateDebugLocation,
    LLVMDIBuilderCreateExpression, LLVMDIBuilderCreateFile, LLVMDIBuilderCreateFunction,
    LLVMDIBuilderCreateParameterVariable, LLVMDIBuilderCreatePointerType,
    LLVMDIBuilderCreateSubroutineType, LLVMDIBuilderCreateUnspecifiedType, LLVMDIBuilderFinalize,
    LLVMDIBuilderInsertDeclareRecordAtEnd as LLVMDIBuilderInsertDeclareAtEnd,
    LLVMDWARFEmissionKind, LLVMDWARFSourceLanguage, LLVMDWARFTypeEncoding,
    LLVMDebugMetadataVersion, LLVMDisposeDIBuilder,
};
use llvm_sys::prelude::{LLVMDIBuilderRef, LLVMMetadataRef};
use std::marker::PhantomData;

use super::{AsValueRef, BasicBlock, BasicValue, Context, InstructionValue, Module, PointerValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleFlagBehavior {
    Warning,
}

impl From<ModuleFlagBehavior> for LLVMModuleFlagBehavior {
    fn from(value: ModuleFlagBehavior) -> Self {
        match value {
            ModuleFlagBehavior::Warning => LLVMModuleFlagBehavior::LLVMModuleFlagBehaviorWarning,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIFile<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIFile<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DICompileUnit<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DICompileUnit<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubroutineType<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubroutineType<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DISubprogram<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DISubprogram<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DILocation<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIType<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIType<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DILocalVariable<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DILocalVariable<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DIExpression<'ctx> {
    pub(super) raw: LLVMMetadataRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DIExpression<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

impl<'ctx> DILocation<'ctx> {
    fn new(raw: LLVMMetadataRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }
}

#[derive(Debug)]
pub struct DebugInfoBuilder<'ctx> {
    raw: LLVMDIBuilderRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> DebugInfoBuilder<'ctx> {
    pub(super) fn new(raw: LLVMDIBuilderRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn create_file(&self, filename: &str, directory: &str) -> DIFile<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateFile(
                self.raw,
                filename.as_ptr() as *const _,
                filename.len(),
                directory.as_ptr() as *const _,
                directory.len(),
            )
        };
        DIFile::new(raw)
    }

    pub fn create_compile_unit(
        &self,
        file: DIFile<'ctx>,
        producer: &str,
        is_optimized: bool,
    ) -> DICompileUnit<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateCompileUnit(
                self.raw,
                LLVMDWARFSourceLanguage::LLVMDWARFSourceLanguageC,
                file.raw,
                producer.as_ptr() as *const _,
                producer.len(),
                if is_optimized { 1 } else { 0 },
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
                0,
                LLVMDWARFEmissionKind::LLVMDWARFEmissionKindFull,
                0,
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
            )
        };
        DICompileUnit::new(raw)
    }

    pub fn create_unspecified_type(&self, name: &str) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateUnspecifiedType(self.raw, name.as_ptr() as *const _, name.len())
        };
        DIType::new(raw)
    }

    pub fn create_basic_type(
        &self,
        name: &str,
        size_in_bits: u64,
        encoding: LLVMDWARFTypeEncoding,
    ) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateBasicType(
                self.raw,
                name.as_ptr() as *const _,
                name.len(),
                size_in_bits,
                encoding,
                0,
            )
        };
        DIType::new(raw)
    }

    pub fn create_pointer_type(
        &self,
        pointee: DIType<'ctx>,
        size_in_bits: u64,
        align_in_bits: u32,
        name: &str,
    ) -> DIType<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreatePointerType(
                self.raw,
                pointee.raw,
                size_in_bits,
                align_in_bits,
                0,
                name.as_ptr() as *const _,
                name.len(),
            )
        };
        DIType::new(raw)
    }

    pub fn create_subroutine_type(&self, file: DIFile<'ctx>) -> DISubroutineType<'ctx> {
        let mut tys = [std::ptr::null_mut()];
        let raw = unsafe {
            LLVMDIBuilderCreateSubroutineType(
                self.raw,
                file.raw,
                tys.as_mut_ptr(),
                tys.len() as u32,
                0,
            )
        };
        DISubroutineType::new(raw)
    }

    pub fn create_function(
        &self,
        scope: DICompileUnit<'ctx>,
        file: DIFile<'ctx>,
        name: &str,
        linkage_name: &str,
        line: u32,
        scope_line: u32,
        subroutine_type: DISubroutineType<'ctx>,
        is_local_to_unit: bool,
        is_optimized: bool,
    ) -> DISubprogram<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateFunction(
                self.raw,
                scope.raw,
                name.as_ptr() as *const _,
                name.len(),
                linkage_name.as_ptr() as *const _,
                linkage_name.len(),
                file.raw,
                line,
                subroutine_type.raw,
                if is_local_to_unit { 1 } else { 0 },
                1,
                scope_line,
                0,
                if is_optimized { 1 } else { 0 },
            )
        };
        DISubprogram::new(raw)
    }

    pub fn create_debug_location(
        &self,
        context: &'ctx Context,
        line: u32,
        column: u32,
        scope: DISubprogram<'ctx>,
    ) -> DILocation<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateDebugLocation(
                context.raw,
                line,
                column,
                scope.raw,
                std::ptr::null_mut(),
            )
        };
        DILocation::new(raw)
    }

    pub fn create_parameter_variable(
        &self,
        scope: DISubprogram<'ctx>,
        name: &str,
        arg_no: u32,
        file: DIFile<'ctx>,
        line: u32,
        ty: DIType<'ctx>,
    ) -> DILocalVariable<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateParameterVariable(
                self.raw,
                scope.raw,
                name.as_ptr() as *const _,
                name.len(),
                arg_no,
                file.raw,
                line,
                ty.raw,
                1,
                0,
            )
        };
        DILocalVariable::new(raw)
    }

    pub fn create_auto_variable(
        &self,
        scope: DISubprogram<'ctx>,
        name: &str,
        file: DIFile<'ctx>,
        line: u32,
        ty: DIType<'ctx>,
        align_in_bits: u32,
    ) -> DILocalVariable<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderCreateAutoVariable(
                self.raw,
                scope.raw,
                name.as_ptr() as *const _,
                name.len(),
                file.raw,
                line,
                ty.raw,
                1,
                0,
                align_in_bits,
            )
        };
        DILocalVariable::new(raw)
    }

    pub fn create_expression(&self) -> DIExpression<'ctx> {
        let raw = unsafe { LLVMDIBuilderCreateExpression(self.raw, std::ptr::null_mut(), 0) };
        DIExpression::new(raw)
    }

    pub fn insert_declare_at_end(
        &self,
        storage: PointerValue<'ctx>,
        variable: DILocalVariable<'ctx>,
        expr: DIExpression<'ctx>,
        location: DILocation<'ctx>,
        block: BasicBlock<'ctx>,
    ) -> InstructionValue<'ctx> {
        let raw = unsafe {
            LLVMDIBuilderInsertDeclareAtEnd(
                self.raw,
                storage.as_value_ref(),
                variable.raw,
                expr.raw,
                location.raw,
                block.raw,
            )
        };
        InstructionValue::new(raw as _)
    }

    pub fn finalize(&self) {
        unsafe { LLVMDIBuilderFinalize(self.raw) };
    }
}

impl<'ctx> Drop for DebugInfoBuilder<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeDIBuilder(self.raw) };
    }
}

impl<'ctx> Context {
    pub fn debug_metadata_version(&self) -> u32 {
        unsafe { LLVMDebugMetadataVersion() }
    }
}

impl<'ctx> Module<'ctx> {
    pub fn create_debug_info_builder(&self) -> DebugInfoBuilder<'ctx> {
        DebugInfoBuilder::new(unsafe { LLVMCreateDIBuilder(self.raw) })
    }

    pub fn add_basic_value_flag<V: BasicValue<'ctx>>(
        &self,
        key: &str,
        behavior: ModuleFlagBehavior,
        value: V,
    ) {
        let metadata = unsafe { LLVMValueAsMetadata(value.as_value_ref()) };
        unsafe {
            LLVMAddModuleFlag(
                self.raw,
                behavior.into(),
                key.as_ptr() as *mut _,
                key.len(),
                metadata,
            )
        };
    }
}
