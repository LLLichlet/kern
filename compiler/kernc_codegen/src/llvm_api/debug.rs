use llvm_sys::LLVMModuleFlagBehavior;
use llvm_sys::core::{LLVMAddModuleFlag, LLVMValueAsMetadata};
use llvm_sys::debuginfo::{
    LLVMCreateDIBuilder, LLVMDIBuilderCreateCompileUnit, LLVMDIBuilderCreateDebugLocation,
    LLVMDIBuilderCreateFile, LLVMDIBuilderCreateFunction, LLVMDIBuilderCreateSubroutineType,
    LLVMDIBuilderFinalize, LLVMDWARFEmissionKind, LLVMDWARFSourceLanguage,
    LLVMDebugMetadataVersion, LLVMDisposeDIBuilder,
};
use llvm_sys::prelude::{LLVMDIBuilderRef, LLVMMetadataRef};
use std::marker::PhantomData;

use super::{BasicValue, Context, Module};

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
