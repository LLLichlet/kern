use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
#[cfg(windows)]
use llvm_sys::core::LLVMPrintModuleToFile;
#[cfg(not(windows))]
use llvm_sys::core::LLVMPrintModuleToString;
use llvm_sys::core::{
    LLVMAddFunction, LLVMAddGlobal, LLVMDisposeMessage, LLVMDisposeModule,
    LLVMGetFirstFunction, LLVMGetIntrinsicDeclaration, LLVMGetNamedFunction, LLVMGetNamedGlobal,
    LLVMGetNextFunction, LLVMSetLinkage,
};
use llvm_sys::prelude::LLVMModuleRef;
use llvm_sys::target::LLVMSetModuleDataLayout;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ptr;

use super::{
    AddressSpace, AsTypeRef, BasicTypeEnum, Context, FunctionType, FunctionValue, GlobalValue,
    Linkage, to_c_string,
};

#[derive(Debug)]
pub struct Module<'ctx> {
    pub(super) raw: LLVMModuleRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Module<'ctx> {
    pub(super) fn new(raw: LLVMModuleRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn as_mut_ptr(&self) -> LLVMModuleRef {
        self.raw
    }

    pub fn add_function(
        &self,
        name: &str,
        ty: FunctionType<'ctx>,
        linkage: Option<Linkage>,
    ) -> FunctionValue<'ctx> {
        let name = to_c_string(name);
        let value = unsafe { LLVMAddFunction(self.raw, name.as_ptr(), ty.as_type_ref()) };
        let func = FunctionValue::new(value);
        if let Some(linkage) = linkage {
            unsafe { LLVMSetLinkage(value, linkage.into()) };
        }
        func
    }

    pub fn get_function(&self, name: &str) -> Option<FunctionValue<'ctx>> {
        let name = to_c_string(name);
        let value = unsafe { LLVMGetNamedFunction(self.raw, name.as_ptr()) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    pub fn get_first_function(&self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetFirstFunction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    pub fn add_global(
        &self,
        ty: BasicTypeEnum<'ctx>,
        _addr_space: Option<AddressSpace>,
        name: &str,
    ) -> GlobalValue<'ctx> {
        let name = to_c_string(name);
        let value = unsafe { LLVMAddGlobal(self.raw, ty.as_type_ref(), name.as_ptr()) };
        GlobalValue::new(value)
    }

    pub fn get_global(&self, name: &str) -> Option<GlobalValue<'ctx>> {
        let name = to_c_string(name);
        let value = unsafe { LLVMGetNamedGlobal(self.raw, name.as_ptr()) };
        if value.is_null() {
            None
        } else {
            Some(GlobalValue::new(value))
        }
    }

    pub fn set_triple(&self, triple: &str) {
        let triple = to_c_string(triple);
        unsafe { llvm_sys::core::LLVMSetTarget(self.raw, triple.as_ptr()) };
    }

    /// # Safety
    /// `target_data` must be a valid LLVM target-data handle for the intended
    /// target, and it must remain valid for the duration of this call.
    pub unsafe fn set_data_layout_from_target(
        &self,
        target_data: llvm_sys::target::LLVMTargetDataRef,
    ) {
        unsafe { LLVMSetModuleDataLayout(self.raw, target_data) };
    }

    pub fn verify(&self) -> Result<(), String> {
        let mut message = ptr::null_mut();
        let failed = unsafe {
            LLVMVerifyModule(
                self.raw,
                LLVMVerifierFailureAction::LLVMReturnStatusAction,
                &mut message,
            )
        } != 0;
        if failed {
            let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
            unsafe { LLVMDisposeMessage(message) };
            Err(text)
        } else {
            Ok(())
        }
    }

    pub fn ir_string(&self) -> Result<String, String> {
        #[cfg(windows)]
        {
            self.ir_string_via_temp_file()
        }

        #[cfg(not(windows))]
        {
            self.ir_string_via_llvm_string()
        }
    }

    #[cfg(windows)]
    fn ir_string_via_temp_file(&self) -> Result<String, String> {
        let unique = format!(
            "kernc_llvm_ir_{}_{}.ll",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let path = std::env::temp_dir().join(unique);
        let path_cstr = to_c_string(&path.to_string_lossy());
        let mut message = ptr::null_mut();

        let failed =
            unsafe { LLVMPrintModuleToFile(self.raw, path_cstr.as_ptr(), &mut message) } != 0;
        if failed {
            let text = if message.is_null() {
                "Unknown LLVM error".to_string()
            } else {
                let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
                unsafe { LLVMDisposeMessage(message) };
                text
            };
            return Err(text);
        }

        let read_result = std::fs::read_to_string(&path).map_err(|err| {
            format!(
                "Failed to read printed LLVM IR from `{}`: {}",
                path.display(),
                err
            )
        });
        let _ = std::fs::remove_file(path);
        read_result
    }

    #[cfg(not(windows))]
    fn ir_string_via_llvm_string(&self) -> Result<String, String> {
        let text = unsafe { LLVMPrintModuleToString(self.raw) };
        if text.is_null() {
            return Err("LLVM returned a null IR buffer".to_string());
        }

        let rendered = unsafe { CStr::from_ptr(text).to_string_lossy().into_owned() };
        unsafe { LLVMDisposeMessage(text) };
        Ok(rendered)
    }

    pub fn get_intrinsic_declaration(
        &self,
        name: &str,
        types: &[BasicTypeEnum<'ctx>],
    ) -> Option<FunctionValue<'ctx>> {
        let name = name.as_bytes();
        let intrinsic_id =
            unsafe { llvm_sys::core::LLVMLookupIntrinsicID(name.as_ptr() as *const _, name.len()) };
        if intrinsic_id == 0 {
            return None;
        }
        let mut overloads = types.iter().map(|ty| ty.as_type_ref()).collect::<Vec<_>>();
        let value = unsafe {
            LLVMGetIntrinsicDeclaration(
                self.raw,
                intrinsic_id,
                overloads.as_mut_ptr(),
                overloads.len(),
            )
        };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }
}

impl<'ctx> FunctionValue<'ctx> {
    pub fn get_next_function(self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetNextFunction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }
}

impl<'ctx> Drop for Module<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeModule(self.raw) };
    }
}
