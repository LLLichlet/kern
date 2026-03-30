use llvm_sys::LLVMAttributeFunctionIndex;
use llvm_sys::analysis::{LLVMVerifierFailureAction, LLVMVerifyModule};
use llvm_sys::core::{
    LLVMAddAttributeAtIndex, LLVMAddCase, LLVMAddFunction, LLVMAddGlobal, LLVMAddIncoming,
    LLVMAppendBasicBlockInContext, LLVMArrayType2, LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAlloca,
    LLVMBuildAnd, LLVMBuildAtomicCmpXchg, LLVMBuildAtomicRMW, LLVMBuildBitCast, LLVMBuildBr,
    LLVMBuildCall2, LLVMBuildCondBr, LLVMBuildExtractValue, LLVMBuildFCmp, LLVMBuildFAdd,
    LLVMBuildFDiv, LLVMBuildFence, LLVMBuildFMul, LLVMBuildFNeg, LLVMBuildFPToSI,
    LLVMBuildFPToUI, LLVMBuildFPCast, LLVMBuildFRem, LLVMBuildFSub, LLVMBuildGEP2,
    LLVMBuildICmp, LLVMBuildInsertValue, LLVMBuildIntToPtr, LLVMBuildLoad2,
    LLVMBuildLShr, LLVMBuildMemCpy, LLVMBuildMemSet, LLVMBuildMul, LLVMBuildNeg, LLVMBuildNot,
    LLVMBuildOr, LLVMBuildPhi, LLVMBuildPointerCast, LLVMBuildPtrDiff2, LLVMBuildPtrToInt,
    LLVMBuildRet, LLVMBuildRetVoid, LLVMBuildSDiv, LLVMBuildSExt, LLVMBuildSIToFP,
    LLVMBuildSRem, LLVMBuildShl, LLVMBuildStore, LLVMBuildStructGEP2, LLVMBuildSub, LLVMBuildSwitch,
    LLVMBuildTrunc, LLVMBuildUDiv, LLVMBuildUIToFP, LLVMBuildURem, LLVMBuildUnreachable,
    LLVMBuildXor, LLVMBuildZExt, LLVMClearInsertionPosition, LLVMConstArray2, LLVMConstInt,
    LLVMConstNamedStruct, LLVMConstNull, LLVMConstPointerNull, LLVMConstReal,
    LLVMConstStringInContext2, LLVMContextCreate, LLVMContextDispose, LLVMCountParams,
    LLVMCountStructElementTypes, LLVMCreateBuilderInContext, LLVMCreateEnumAttribute,
    LLVMDisposeBuilder, LLVMDisposeMessage, LLVMDisposeModule, LLVMDoubleTypeInContext,
    LLVMFloatTypeInContext, LLVMFunctionType, LLVMGetBasicBlockParent, LLVMGetBasicBlockTerminator,
    LLVMGetElementType, LLVMGetEnumAttributeKindForName, LLVMGetFirstBasicBlock,
    LLVMGetFirstInstruction, LLVMGetInlineAsm, LLVMGetInsertBlock, LLVMGetIntrinsicDeclaration,
    LLVMGetNamedFunction, LLVMGetNamedGlobal, LLVMGetParam, LLVMGetReturnType, LLVMGetTypeKind,
    LLVMGetUndef, LLVMGlobalGetValueType, LLVMInt1TypeInContext,
    LLVMInt16TypeInContext, LLVMInt32TypeInContext, LLVMInt64TypeInContext, LLVMInt8TypeInContext,
    LLVMIntTypeInContext, LLVMIsAInstruction, LLVMModuleCreateWithNameInContext,
    LLVMPointerTypeInContext, LLVMPositionBuilderAtEnd, LLVMPositionBuilderBefore, LLVMPrintModuleToFile,
    LLVMSetAlignment, LLVMSetGlobalConstant, LLVMSetInitializer, LLVMSetLinkage, LLVMSetOrdering,
    LLVMSetSection, LLVMStructCreateNamed, LLVMStructGetTypeAtIndex, LLVMStructSetBody,
    LLVMStructTypeInContext, LLVMTypeOf, LLVMVoidTypeInContext,
};
use llvm_sys::prelude::{
    LLVMAttributeRef, LLVMBasicBlockRef, LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMTypeRef,
    LLVMValueRef,
};
use llvm_sys::target::LLVMSetModuleDataLayout;
use llvm_sys::{
    LLVMAtomicOrdering, LLVMAtomicRMWBinOp, LLVMInlineAsmDialect, LLVMIntPredicate,
    LLVMRealPredicate, LLVMTypeKind, LLVMLinkage,
};
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ptr;

fn to_c_string(input: &str) -> CString {
    CString::new(input).expect("LLVM strings cannot contain interior NUL bytes")
}

fn bool_to_llvm(value: bool) -> i32 {
    if value { 1 } else { 0 }
}

pub trait AsTypeRef {
    fn as_type_ref(&self) -> LLVMTypeRef;
}

pub trait BasicType<'ctx>: AsTypeRef + Copy {}

pub trait AsValueRef {
    fn as_value_ref(&self) -> LLVMValueRef;
}

pub trait BasicValue<'ctx>: AsValueRef {
    fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx>;
}

pub trait AggregateValue<'ctx>: BasicValue<'ctx> {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AddressSpace(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineAsmDialect {
    ATT,
    Intel,
}

impl From<InlineAsmDialect> for LLVMInlineAsmDialect {
    fn from(value: InlineAsmDialect) -> Self {
        match value {
            InlineAsmDialect::ATT => LLVMInlineAsmDialect::LLVMInlineAsmDialectATT,
            InlineAsmDialect::Intel => LLVMInlineAsmDialect::LLVMInlineAsmDialectIntel,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntPredicate {
    EQ,
    NE,
    UGT,
    UGE,
    ULT,
    ULE,
    SGT,
    SGE,
    SLT,
    SLE,
}

impl From<IntPredicate> for LLVMIntPredicate {
    fn from(value: IntPredicate) -> Self {
        match value {
            IntPredicate::EQ => LLVMIntPredicate::LLVMIntEQ,
            IntPredicate::NE => LLVMIntPredicate::LLVMIntNE,
            IntPredicate::UGT => LLVMIntPredicate::LLVMIntUGT,
            IntPredicate::UGE => LLVMIntPredicate::LLVMIntUGE,
            IntPredicate::ULT => LLVMIntPredicate::LLVMIntULT,
            IntPredicate::ULE => LLVMIntPredicate::LLVMIntULE,
            IntPredicate::SGT => LLVMIntPredicate::LLVMIntSGT,
            IntPredicate::SGE => LLVMIntPredicate::LLVMIntSGE,
            IntPredicate::SLT => LLVMIntPredicate::LLVMIntSLT,
            IntPredicate::SLE => LLVMIntPredicate::LLVMIntSLE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatPredicate {
    OEQ,
    OGT,
    OGE,
    OLT,
    OLE,
    ONE,
}

impl From<FloatPredicate> for LLVMRealPredicate {
    fn from(value: FloatPredicate) -> Self {
        match value {
            FloatPredicate::OEQ => LLVMRealPredicate::LLVMRealOEQ,
            FloatPredicate::OGT => LLVMRealPredicate::LLVMRealOGT,
            FloatPredicate::OGE => LLVMRealPredicate::LLVMRealOGE,
            FloatPredicate::OLT => LLVMRealPredicate::LLVMRealOLT,
            FloatPredicate::OLE => LLVMRealPredicate::LLVMRealOLE,
            FloatPredicate::ONE => LLVMRealPredicate::LLVMRealONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicOrdering {
    Monotonic,
    Acquire,
    Release,
    AcquireRelease,
    SequentiallyConsistent,
}

impl From<AtomicOrdering> for LLVMAtomicOrdering {
    fn from(value: AtomicOrdering) -> Self {
        match value {
            AtomicOrdering::Monotonic => LLVMAtomicOrdering::LLVMAtomicOrderingMonotonic,
            AtomicOrdering::Acquire => LLVMAtomicOrdering::LLVMAtomicOrderingAcquire,
            AtomicOrdering::Release => LLVMAtomicOrdering::LLVMAtomicOrderingRelease,
            AtomicOrdering::AcquireRelease => LLVMAtomicOrdering::LLVMAtomicOrderingAcquireRelease,
            AtomicOrdering::SequentiallyConsistent => {
                LLVMAtomicOrdering::LLVMAtomicOrderingSequentiallyConsistent
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicRMWBinOp {
    Xchg,
    Add,
    Sub,
    And,
    Nand,
    Or,
    Xor,
    Max,
    Min,
    UMax,
    UMin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationLevel {
    None,
    Less,
    Default,
    Aggressive,
}

impl From<AtomicRMWBinOp> for LLVMAtomicRMWBinOp {
    fn from(value: AtomicRMWBinOp) -> Self {
        match value {
            AtomicRMWBinOp::Xchg => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpXchg,
            AtomicRMWBinOp::Add => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpAdd,
            AtomicRMWBinOp::Sub => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpSub,
            AtomicRMWBinOp::And => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpAnd,
            AtomicRMWBinOp::Nand => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpNand,
            AtomicRMWBinOp::Or => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpOr,
            AtomicRMWBinOp::Xor => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpXor,
            AtomicRMWBinOp::Max => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpMax,
            AtomicRMWBinOp::Min => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpMin,
            AtomicRMWBinOp::UMax => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpUMax,
            AtomicRMWBinOp::UMin => LLVMAtomicRMWBinOp::LLVMAtomicRMWBinOpUMin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    External,
}

impl From<Linkage> for LLVMLinkage {
    fn from(value: Linkage) -> Self {
        match value {
            Linkage::External => LLVMLinkage::LLVMExternalLinkage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeLoc {
    Function,
}

#[derive(Debug, Clone, Copy)]
pub struct Attribute {
    raw: LLVMAttributeRef,
}

impl Attribute {
    pub fn get_named_enum_kind_id(name: &str) -> u32 {
        unsafe { LLVMGetEnumAttributeKindForName(name.as_ptr() as *const _, name.len()) }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Context {
    raw: LLVMContextRef,
}

impl Context {
    pub fn create() -> Self {
        let raw = unsafe { LLVMContextCreate() };
        assert!(!raw.is_null());
        Self { raw }
    }

    pub fn create_builder<'ctx>(&'ctx self) -> Builder<'ctx> {
        let raw = unsafe { LLVMCreateBuilderInContext(self.raw) };
        assert!(!raw.is_null());
        Builder {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn create_module<'ctx>(&'ctx self, name: &str) -> Module<'ctx> {
        let name = to_c_string(name);
        let raw = unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), self.raw) };
        assert!(!raw.is_null());
        Module {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn create_inline_asm<'ctx>(
        &'ctx self,
        ty: FunctionType<'ctx>,
        mut assembly: String,
        mut constraints: String,
        sideeffects: bool,
        alignstack: bool,
        dialect: Option<InlineAsmDialect>,
        can_throw: bool,
    ) -> PointerValue<'ctx> {
        let raw = unsafe {
            LLVMGetInlineAsm(
                ty.as_type_ref(),
                assembly.as_mut_ptr() as *mut _,
                assembly.len(),
                constraints.as_mut_ptr() as *mut _,
                constraints.len(),
                bool_to_llvm(sideeffects),
                bool_to_llvm(alignstack),
                dialect.unwrap_or(InlineAsmDialect::ATT).into(),
                bool_to_llvm(can_throw),
            )
        };
        PointerValue::new(raw)
    }

    pub fn create_enum_attribute(&self, kind_id: u32, val: u64) -> Attribute {
        Attribute {
            raw: unsafe { LLVMCreateEnumAttribute(self.raw, kind_id, val) },
        }
    }

    pub fn void_type<'ctx>(&'ctx self) -> VoidType<'ctx> {
        VoidType::new(unsafe { LLVMVoidTypeInContext(self.raw) })
    }

    pub fn bool_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt1TypeInContext(self.raw) })
    }

    pub fn i8_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt8TypeInContext(self.raw) })
    }

    pub fn i16_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt16TypeInContext(self.raw) })
    }

    pub fn i32_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt32TypeInContext(self.raw) })
    }

    pub fn i64_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMInt64TypeInContext(self.raw) })
    }

    pub fn i128_type<'ctx>(&'ctx self) -> IntType<'ctx> {
        self.custom_width_int_type(128)
    }

    pub fn custom_width_int_type<'ctx>(&'ctx self, bits: u32) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMIntTypeInContext(self.raw, bits) })
    }

    pub fn f32_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMFloatTypeInContext(self.raw) })
    }

    pub fn f64_type<'ctx>(&'ctx self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMDoubleTypeInContext(self.raw) })
    }

    pub fn ptr_type<'ctx>(&'ctx self, address_space: AddressSpace) -> PointerType<'ctx> {
        PointerType::new(unsafe { LLVMPointerTypeInContext(self.raw, address_space.0) })
    }

    pub fn struct_type<'ctx>(
        &'ctx self,
        fields: &[BasicTypeEnum<'ctx>],
        packed: bool,
    ) -> StructType<'ctx> {
        let mut fields = fields.iter().map(|field| field.as_type_ref()).collect::<Vec<_>>();
        StructType::new(unsafe {
            LLVMStructTypeInContext(
                self.raw,
                fields.as_mut_ptr(),
                fields.len() as u32,
                bool_to_llvm(packed),
            )
        })
    }

    pub fn opaque_struct_type<'ctx>(&'ctx self, name: &str) -> StructType<'ctx> {
        let name = to_c_string(name);
        StructType::new(unsafe { LLVMStructCreateNamed(self.raw, name.as_ptr()) })
    }

    pub fn append_basic_block<'ctx>(
        &'ctx self,
        function: FunctionValue<'ctx>,
        name: &str,
    ) -> BasicBlock<'ctx> {
        let name = to_c_string(name);
        BasicBlock::new(unsafe {
            LLVMAppendBasicBlockInContext(self.raw, function.as_value_ref(), name.as_ptr())
        })
    }

    pub fn const_string<'ctx>(&'ctx self, bytes: &[u8], dont_null_terminate: bool) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe {
            LLVMConstStringInContext2(
                self.raw,
                bytes.as_ptr() as *const _,
                bytes.len(),
                bool_to_llvm(dont_null_terminate),
            )
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { LLVMContextDispose(self.raw) };
    }
}

#[derive(Debug)]
pub struct Module<'ctx> {
    raw: LLVMModuleRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Module<'ctx> {
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

    pub fn set_data_layout_from_target(&self, target_data: llvm_sys::target::LLVMTargetDataRef) {
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

    pub fn print_to_stderr(&self) {
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
            let text = unsafe { CStr::from_ptr(message).to_string_lossy().into_owned() };
            unsafe { LLVMDisposeMessage(message) };
            eprintln!("Failed to print LLVM IR: {}", text);
            return;
        }

        match std::fs::read_to_string(&path) {
            Ok(text) => eprintln!("{}", text),
            Err(err) => eprintln!("Failed to read printed LLVM IR from `{}`: {}", path.display(), err),
        }

        let _ = std::fs::remove_file(path);
    }

    pub fn get_intrinsic_declaration(
        &self,
        name: &str,
        types: &[BasicTypeEnum<'ctx>],
    ) -> Option<FunctionValue<'ctx>> {
        let name = name.as_bytes();
        let intrinsic_id = unsafe { llvm_sys::core::LLVMLookupIntrinsicID(name.as_ptr() as *const _, name.len()) };
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

impl<'ctx> Drop for Module<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeModule(self.raw) };
    }
}

#[derive(Debug)]
pub struct Builder<'ctx> {
    raw: LLVMBuilderRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> Builder<'ctx> {
    pub fn get_insert_block(&self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetInsertBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    pub fn position_at_end(&self, block: BasicBlock<'ctx>) {
        unsafe { LLVMPositionBuilderAtEnd(self.raw, block.raw) };
    }

    pub fn position_before(&self, instruction: &InstructionValue<'ctx>) {
        unsafe { LLVMPositionBuilderBefore(self.raw, instruction.raw) };
    }

    pub fn clear_insertion_position(&self) {
        unsafe { LLVMClearInsertionPosition(self.raw) };
    }

    pub fn build_alloca<T: AsTypeRef>(&self, ty: T, name: &str) -> Result<PointerValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(PointerValue::new(unsafe { LLVMBuildAlloca(self.raw, ty.as_type_ref(), name.as_ptr()) }))
    }

    pub fn build_store<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        value: V,
    ) -> Result<InstructionValue<'ctx>, ()> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildStore(self.raw, value.as_value_ref(), ptr.as_value_ref())
        }))
    }

    pub fn build_load<T: AsTypeRef>(
        &self,
        ty: T,
        ptr: PointerValue<'ctx>,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(BasicValueEnum::new(unsafe {
            LLVMBuildLoad2(self.raw, ty.as_type_ref(), ptr.as_value_ref(), name.as_ptr())
        }))
    }

    pub unsafe fn build_gep<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        ptr: PointerValue<'ctx>,
        indexes: &[IntValue<'ctx>],
        name: &str,
    ) -> Result<PointerValue<'ctx>, ()> {
        let name = to_c_string(name);
        let mut indexes = indexes.iter().map(|idx| idx.as_value_ref()).collect::<Vec<_>>();
        Ok(PointerValue::new(unsafe {
            LLVMBuildGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                indexes.as_mut_ptr(),
                indexes.len() as u32,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_struct_gep<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        ptr: PointerValue<'ctx>,
        index: u32,
        name: &str,
    ) -> Result<PointerValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(PointerValue::new(unsafe {
            LLVMBuildStructGEP2(
                self.raw,
                pointee_ty.as_type_ref(),
                ptr.as_value_ref(),
                index,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_ptr_diff<T: AsTypeRef>(
        &self,
        pointee_ty: T,
        lhs: PointerValue<'ctx>,
        rhs: PointerValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe {
            LLVMBuildPtrDiff2(
                self.raw,
                pointee_ty.as_type_ref(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_phi<T: AsTypeRef>(&self, ty: T, name: &str) -> Result<PhiValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(PhiValue::new(unsafe { LLVMBuildPhi(self.raw, ty.as_type_ref(), name.as_ptr()) }))
    }

    pub fn build_call(
        &self,
        function: FunctionValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<CallSiteValue<'ctx>, ()> {
        self.build_call2(function.get_type(), function.as_value_ref(), args, name)
    }

    pub fn build_indirect_call(
        &self,
        function_type: FunctionType<'ctx>,
        function_pointer: PointerValue<'ctx>,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<CallSiteValue<'ctx>, ()> {
        self.build_call2(function_type, function_pointer.as_value_ref(), args, name)
    }

    fn build_call2(
        &self,
        function_type: FunctionType<'ctx>,
        callee: LLVMValueRef,
        args: &[BasicMetadataValueEnum<'ctx>],
        name: &str,
    ) -> Result<CallSiteValue<'ctx>, ()> {
        let name = if function_type.get_return_type().is_none() { "" } else { name };
        let name = to_c_string(name);
        let mut args = args.iter().map(|arg| arg.as_value_ref()).collect::<Vec<_>>();
        Ok(CallSiteValue::new(unsafe {
            LLVMBuildCall2(
                self.raw,
                function_type.as_type_ref(),
                callee,
                args.as_mut_ptr(),
                args.len() as u32,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_return(
        &self,
        value: Option<&dyn BasicValue<'ctx>>,
    ) -> Result<InstructionValue<'ctx>, ()> {
        Ok(InstructionValue::new(unsafe {
            match value {
                Some(value) => LLVMBuildRet(self.raw, value.as_value_ref()),
                None => LLVMBuildRetVoid(self.raw),
            }
        }))
    }

    pub fn build_unreachable(&self) -> Result<InstructionValue<'ctx>, ()> {
        Ok(InstructionValue::new(unsafe { LLVMBuildUnreachable(self.raw) }))
    }

    pub fn build_unconditional_branch(
        &self,
        destination: BasicBlock<'ctx>,
    ) -> Result<InstructionValue<'ctx>, ()> {
        Ok(InstructionValue::new(unsafe { LLVMBuildBr(self.raw, destination.raw) }))
    }

    pub fn build_conditional_branch(
        &self,
        comparison: IntValue<'ctx>,
        then_block: BasicBlock<'ctx>,
        else_block: BasicBlock<'ctx>,
    ) -> Result<InstructionValue<'ctx>, ()> {
        Ok(InstructionValue::new(unsafe {
            LLVMBuildCondBr(
                self.raw,
                comparison.as_value_ref(),
                then_block.raw,
                else_block.raw,
            )
        }))
    }

    pub fn build_switch(
        &self,
        value: IntValue<'ctx>,
        else_block: BasicBlock<'ctx>,
        cases: &[(IntValue<'ctx>, BasicBlock<'ctx>)],
    ) -> Result<InstructionValue<'ctx>, ()> {
        let inst = unsafe {
            LLVMBuildSwitch(
                self.raw,
                value.as_value_ref(),
                else_block.raw,
                cases.len() as u32,
            )
        };
        for (case_value, block) in cases {
            unsafe { LLVMAddCase(inst, case_value.as_value_ref(), block.raw) };
        }
        Ok(InstructionValue::new(inst))
    }

    pub fn build_extract_value<AV: AggregateValue<'ctx>>(
        &self,
        aggregate: AV,
        index: u32,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(BasicValueEnum::new(unsafe {
            LLVMBuildExtractValue(self.raw, aggregate.as_value_ref(), index, name.as_ptr())
        }))
    }

    pub fn build_insert_value<AV: AggregateValue<'ctx>, BV: BasicValue<'ctx>>(
        &self,
        aggregate: AV,
        value: BV,
        index: u32,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(BasicValueEnum::new(unsafe {
            LLVMBuildInsertValue(
                self.raw,
                aggregate.as_value_ref(),
                value.as_value_ref(),
                index,
                name.as_ptr(),
            )
        }))
    }

    pub fn build_memcpy(
        &self,
        dest: PointerValue<'ctx>,
        dest_align: u32,
        src: PointerValue<'ctx>,
        src_align: u32,
        size: IntValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, ()> {
        Ok(PointerValue::new(unsafe {
            LLVMBuildMemCpy(
                self.raw,
                dest.as_value_ref(),
                dest_align,
                src.as_value_ref(),
                src_align,
                size.as_value_ref(),
            )
        }))
    }

    pub fn build_memset(
        &self,
        dest: PointerValue<'ctx>,
        align: u32,
        value: IntValue<'ctx>,
        size: IntValue<'ctx>,
    ) -> Result<PointerValue<'ctx>, ()> {
        Ok(PointerValue::new(unsafe {
            LLVMBuildMemSet(
                self.raw,
                dest.as_value_ref(),
                value.as_value_ref(),
                size.as_value_ref(),
                align,
            )
        }))
    }

    pub fn build_fence(
        &self,
        ordering: AtomicOrdering,
        sync_scope: i32,
        name: &str,
    ) -> Result<InstructionValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(InstructionValue::new(unsafe {
            LLVMBuildFence(self.raw, ordering.into(), sync_scope, name.as_ptr())
        }))
    }

    pub fn build_atomicrmw(
        &self,
        op: AtomicRMWBinOp,
        ptr: PointerValue<'ctx>,
        value: IntValue<'ctx>,
        ordering: AtomicOrdering,
    ) -> Result<IntValue<'ctx>, ()> {
        Ok(IntValue::new(unsafe {
            LLVMBuildAtomicRMW(
                self.raw,
                op.into(),
                ptr.as_value_ref(),
                value.as_value_ref(),
                ordering.into(),
                0,
            )
        }))
    }

    pub fn build_cmpxchg<V: BasicValue<'ctx>>(
        &self,
        ptr: PointerValue<'ctx>,
        expected: V,
        desired: V,
        success: AtomicOrdering,
        failure: AtomicOrdering,
    ) -> Result<StructValue<'ctx>, ()> {
        Ok(StructValue::new(unsafe {
            LLVMBuildAtomicCmpXchg(
                self.raw,
                ptr.as_value_ref(),
                expected.as_value_ref(),
                desired.as_value_ref(),
                success.into(),
                failure.into(),
                0,
            )
        }))
    }

    pub fn build_int_add(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildAdd, lhs, rhs, name)
    }

    pub fn build_int_sub(&self, lhs: IntValue<'ctx>, rhs: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildSub, lhs, rhs, name)
    }

    pub fn build_int_mul(&self, lhs: IntValue<'ctx>, rhs: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildMul, lhs, rhs, name)
    }

    pub fn build_int_signed_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildSDiv, lhs, rhs, name)
    }

    pub fn build_int_unsigned_div(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildUDiv, lhs, rhs, name)
    }

    pub fn build_int_signed_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildSRem, lhs, rhs, name)
    }

    pub fn build_int_unsigned_rem(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildURem, lhs, rhs, name)
    }

    pub fn build_and(&self, lhs: IntValue<'ctx>, rhs: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildAnd, lhs, rhs, name)
    }

    pub fn build_or(&self, lhs: IntValue<'ctx>, rhs: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildOr, lhs, rhs, name)
    }

    pub fn build_xor(&self, lhs: IntValue<'ctx>, rhs: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildXor, lhs, rhs, name)
    }

    pub fn build_left_shift(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        build_int_bin(self.raw, LLVMBuildShl, lhs, rhs, name)
    }

    pub fn build_right_shift(
        &self,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        signed: bool,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        if signed {
            build_int_bin(self.raw, LLVMBuildAShr, lhs, rhs, name)
        } else {
            build_int_bin(self.raw, LLVMBuildLShr, lhs, rhs, name)
        }
    }

    pub fn build_int_compare(
        &self,
        pred: IntPredicate,
        lhs: IntValue<'ctx>,
        rhs: IntValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe {
            LLVMBuildICmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_float_add(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        build_float_bin(self.raw, LLVMBuildFAdd, lhs, rhs, name)
    }

    pub fn build_float_sub(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        build_float_bin(self.raw, LLVMBuildFSub, lhs, rhs, name)
    }

    pub fn build_float_mul(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        build_float_bin(self.raw, LLVMBuildFMul, lhs, rhs, name)
    }

    pub fn build_float_div(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        build_float_bin(self.raw, LLVMBuildFDiv, lhs, rhs, name)
    }

    pub fn build_float_rem(
        &self,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        build_float_bin(self.raw, LLVMBuildFRem, lhs, rhs, name)
    }

    pub fn build_float_compare(
        &self,
        pred: FloatPredicate,
        lhs: FloatValue<'ctx>,
        rhs: FloatValue<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe {
            LLVMBuildFCmp(
                self.raw,
                pred.into(),
                lhs.as_value_ref(),
                rhs.as_value_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_int_neg(&self, value: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe { LLVMBuildNeg(self.raw, value.as_value_ref(), name.as_ptr()) }))
    }

    pub fn build_float_neg(
        &self,
        value: FloatValue<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(FloatValue::new(unsafe {
            LLVMBuildFNeg(self.raw, value.as_value_ref(), name.as_ptr())
        }))
    }

    pub fn build_not(&self, value: IntValue<'ctx>, name: &str) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe { LLVMBuildNot(self.raw, value.as_value_ref(), name.as_ptr()) }))
    }

    pub fn build_bit_cast<V: BasicValue<'ctx>, T: AsTypeRef>(
        &self,
        value: V,
        target: T,
        name: &str,
    ) -> Result<BasicValueEnum<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(BasicValueEnum::new(unsafe {
            LLVMBuildBitCast(self.raw, value.as_value_ref(), target.as_type_ref(), name.as_ptr())
        }))
    }

    pub fn build_pointer_cast(
        &self,
        value: PointerValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(PointerValue::new(unsafe {
            LLVMBuildPointerCast(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_ptr_to_int(
        &self,
        value: PointerValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(IntValue::new(unsafe {
            LLVMBuildPtrToInt(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_int_to_ptr(
        &self,
        value: IntValue<'ctx>,
        target: PointerType<'ctx>,
        name: &str,
    ) -> Result<PointerValue<'ctx>, ()> {
        let name = to_c_string(name);
        Ok(PointerValue::new(unsafe {
            LLVMBuildIntToPtr(
                self.raw,
                value.as_value_ref(),
                target.as_type_ref(),
                name.as_ptr(),
            )
        }))
    }

    pub fn build_int_z_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        cast_int(self.raw, LLVMBuildZExt, value, target, name)
    }

    pub fn build_int_s_extend(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        cast_int(self.raw, LLVMBuildSExt, value, target, name)
    }

    pub fn build_int_truncate(
        &self,
        value: IntValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        cast_int(self.raw, LLVMBuildTrunc, value, target, name)
    }

    pub fn build_signed_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        cast_float(self.raw, LLVMBuildSIToFP, value, target, name)
    }

    pub fn build_unsigned_int_to_float(
        &self,
        value: IntValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        cast_float(self.raw, LLVMBuildUIToFP, value, target, name)
    }

    pub fn build_float_to_signed_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        cast_int_from_float(self.raw, LLVMBuildFPToSI, value, target, name)
    }

    pub fn build_float_to_unsigned_int(
        &self,
        value: FloatValue<'ctx>,
        target: IntType<'ctx>,
        name: &str,
    ) -> Result<IntValue<'ctx>, ()> {
        cast_int_from_float(self.raw, LLVMBuildFPToUI, value, target, name)
    }

    pub fn build_float_cast(
        &self,
        value: FloatValue<'ctx>,
        target: FloatType<'ctx>,
        name: &str,
    ) -> Result<FloatValue<'ctx>, ()> {
        cast_float(self.raw, LLVMBuildFPCast, value, target, name)
    }
}

impl<'ctx> Drop for Builder<'ctx> {
    fn drop(&mut self) {
        unsafe { LLVMDisposeBuilder(self.raw) };
    }
}

fn build_int_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: IntValue<'ctx>,
    rhs: IntValue<'ctx>,
    name: &str,
) -> Result<IntValue<'ctx>, ()> {
    let name = to_c_string(name);
    Ok(IntValue::new(unsafe {
        f(builder, lhs.as_value_ref(), rhs.as_value_ref(), name.as_ptr())
    }))
}

fn build_float_bin<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMValueRef, *const i8) -> LLVMValueRef,
    lhs: FloatValue<'ctx>,
    rhs: FloatValue<'ctx>,
    name: &str,
) -> Result<FloatValue<'ctx>, ()> {
    let name = to_c_string(name);
    Ok(FloatValue::new(unsafe {
        f(builder, lhs.as_value_ref(), rhs.as_value_ref(), name.as_ptr())
    }))
}

fn cast_int<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: IntValue<'ctx>,
    target: IntType<'ctx>,
    name: &str,
) -> Result<IntValue<'ctx>, ()> {
    let name = to_c_string(name);
    Ok(IntValue::new(unsafe {
        f(builder, value.as_value_ref(), target.as_type_ref(), name.as_ptr())
    }))
}

fn cast_float<'ctx, V: AsValueRef>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: V,
    target: FloatType<'ctx>,
    name: &str,
) -> Result<FloatValue<'ctx>, ()> {
    let name = to_c_string(name);
    Ok(FloatValue::new(unsafe {
        f(builder, value.as_value_ref(), target.as_type_ref(), name.as_ptr())
    }))
}

fn cast_int_from_float<'ctx>(
    builder: LLVMBuilderRef,
    f: unsafe extern "C" fn(LLVMBuilderRef, LLVMValueRef, LLVMTypeRef, *const i8) -> LLVMValueRef,
    value: FloatValue<'ctx>,
    target: IntType<'ctx>,
    name: &str,
) -> Result<IntValue<'ctx>, ()> {
    let name = to_c_string(name);
    Ok(IntValue::new(unsafe {
        f(builder, value.as_value_ref(), target.as_type_ref(), name.as_ptr())
    }))
}

macro_rules! impl_type_wrapper {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx> {
            raw: LLVMTypeRef,
            _marker: PhantomData<&'ctx Context>,
        }

        impl<'ctx> $name<'ctx> {
            fn new(raw: LLVMTypeRef) -> Self {
                assert!(!raw.is_null());
                Self {
                    raw,
                    _marker: PhantomData,
                }
            }
        }

        impl<'ctx> AsTypeRef for $name<'ctx> {
            fn as_type_ref(&self) -> LLVMTypeRef {
                self.raw
            }
        }
    };
}

impl_type_wrapper!(VoidType);
impl_type_wrapper!(IntType);
impl_type_wrapper!(FloatType);
impl_type_wrapper!(PointerType);
impl_type_wrapper!(StructType);
impl_type_wrapper!(ArrayType);
impl_type_wrapper!(VectorType);
impl_type_wrapper!(ScalableVectorType);
impl_type_wrapper!(FunctionType);

impl<'ctx> VoidType<'ctx> {
    pub fn fn_type(
        self,
        params: &[BasicMetadataTypeEnum<'ctx>],
        variadic: bool,
    ) -> FunctionType<'ctx> {
        let mut params = params.iter().map(|param| param.as_type_ref()).collect::<Vec<_>>();
        FunctionType::new(unsafe {
            LLVMFunctionType(
                self.as_type_ref(),
                params.as_mut_ptr(),
                params.len() as u32,
                bool_to_llvm(variadic),
            )
        })
    }
}

macro_rules! impl_basic_type_methods {
    ($name:ident) => {
        impl<'ctx> $name<'ctx> {
            pub fn fn_type(
                self,
                params: &[BasicMetadataTypeEnum<'ctx>],
                variadic: bool,
            ) -> FunctionType<'ctx> {
                let mut params =
                    params.iter().map(|param| param.as_type_ref()).collect::<Vec<_>>();
                FunctionType::new(unsafe {
                    LLVMFunctionType(
                        self.as_type_ref(),
                        params.as_mut_ptr(),
                        params.len() as u32,
                        bool_to_llvm(variadic),
                    )
                })
            }
            
            pub fn array_type(self, len: u32) -> ArrayType<'ctx> {
                ArrayType::new(unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) })
            }
        }
    };
}

impl_basic_type_methods!(IntType);
impl_basic_type_methods!(FloatType);
impl_basic_type_methods!(PointerType);
impl_basic_type_methods!(StructType);
impl_basic_type_methods!(ArrayType);
impl_basic_type_methods!(VectorType);
impl_basic_type_methods!(ScalableVectorType);

impl<'ctx> IntType<'ctx> {
    pub fn const_int(self, value: u64, sign_extend: bool) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMConstInt(self.as_type_ref(), value, bool_to_llvm(sign_extend)) })
    }

    pub fn const_array(self, values: &[IntValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }

    pub fn const_zero(self) -> IntValue<'ctx> {
        self.const_int(0, false)
    }

    pub fn get_undef(self) -> IntValue<'ctx> {
        IntValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> FloatType<'ctx> {
    pub fn const_float(self, value: f64) -> FloatValue<'ctx> {
        FloatValue::new(unsafe { LLVMConstReal(self.as_type_ref(), value) })
    }

    pub fn const_array(self, values: &[FloatValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }

    pub fn const_zero(self) -> FloatValue<'ctx> {
        self.const_float(0.0)
    }

    pub fn get_undef(self) -> FloatValue<'ctx> {
        FloatValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> PointerType<'ctx> {
    pub fn const_zero(self) -> PointerValue<'ctx> {
        self.const_null()
    }

    pub fn const_null(self) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMConstPointerNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> PointerValue<'ctx> {
        PointerValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[PointerValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }
}

impl<'ctx> StructType<'ctx> {
    pub fn as_basic_type_enum(self) -> BasicTypeEnum<'ctx> {
        BasicTypeEnum::StructType(self)
    }

    pub fn set_body(self, fields: &[BasicTypeEnum<'ctx>], packed: bool) {
        let mut fields = fields.iter().map(|field| field.as_type_ref()).collect::<Vec<_>>();
        unsafe {
            LLVMStructSetBody(
                self.as_type_ref(),
                fields.as_mut_ptr(),
                fields.len() as u32,
                bool_to_llvm(packed),
            )
        };
    }

    pub fn count_fields(self) -> u32 {
        unsafe { LLVMCountStructElementTypes(self.as_type_ref()) }
    }

    pub fn get_field_type_at_index(self, index: u32) -> Option<BasicTypeEnum<'ctx>> {
        if index >= self.count_fields() {
            None
        } else {
            Some(BasicTypeEnum::new(unsafe {
                LLVMStructGetTypeAtIndex(self.as_type_ref(), index)
            }))
        }
    }

    pub fn const_named_struct(self, values: &[BasicValueEnum<'ctx>]) -> StructValue<'ctx> {
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        StructValue::new(unsafe {
            LLVMConstNamedStruct(self.as_type_ref(), values.as_mut_ptr(), values.len() as u32)
        })
    }

    pub fn const_zero(self) -> StructValue<'ctx> {
        StructValue::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> StructValue<'ctx> {
        StructValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[StructValue<'ctx>]) -> ArrayValue<'ctx> {
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        ArrayValue::new(unsafe {
            LLVMConstArray2(self.as_type_ref(), values.as_mut_ptr(), values.len() as u64)
        })
    }
}

impl<'ctx> ArrayType<'ctx> {
    pub fn len(self) -> u32 {
        unsafe { llvm_sys::core::LLVMGetArrayLength2(self.as_type_ref()) as u32 }
    }

    pub fn const_zero(self) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> ArrayValue<'ctx> {
        ArrayValue::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }

    pub fn const_array(self, values: &[ArrayValue<'ctx>]) -> ArrayValue<'ctx> {
        let elem_ty = unsafe { LLVMGetElementType(self.as_type_ref()) };
        let mut values = values.iter().map(|value| value.as_value_ref()).collect::<Vec<_>>();
        ArrayValue::new(unsafe { LLVMConstArray2(elem_ty, values.as_mut_ptr(), values.len() as u64) })
    }
}

impl<'ctx> VectorType<'ctx> {
    pub fn const_zero(self) -> BasicValueEnum<'ctx> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> BasicValueEnum<'ctx> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> ScalableVectorType<'ctx> {
    pub fn const_zero(self) -> BasicValueEnum<'ctx> {
        BasicValueEnum::new(unsafe { LLVMConstNull(self.as_type_ref()) })
    }

    pub fn get_undef(self) -> BasicValueEnum<'ctx> {
        BasicValueEnum::new(unsafe { LLVMGetUndef(self.as_type_ref()) })
    }
}

impl<'ctx> FunctionType<'ctx> {
    pub fn get_return_type(self) -> Option<BasicTypeEnum<'ctx>> {
        let ty = unsafe { LLVMGetReturnType(self.as_type_ref()) };
        if unsafe { LLVMGetTypeKind(ty) } == LLVMTypeKind::LLVMVoidTypeKind {
            None
        } else {
            Some(BasicTypeEnum::new(ty))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicTypeEnum<'ctx> {
    ArrayType(ArrayType<'ctx>),
    FloatType(FloatType<'ctx>),
    IntType(IntType<'ctx>),
    PointerType(PointerType<'ctx>),
    StructType(StructType<'ctx>),
    VectorType(VectorType<'ctx>),
    ScalableVectorType(ScalableVectorType<'ctx>),
}

impl<'ctx> BasicTypeEnum<'ctx> {
    fn new(raw: LLVMTypeRef) -> Self {
        match unsafe { LLVMGetTypeKind(raw) } {
            LLVMTypeKind::LLVMArrayTypeKind => Self::ArrayType(ArrayType::new(raw)),
            LLVMTypeKind::LLVMFloatTypeKind | LLVMTypeKind::LLVMDoubleTypeKind => {
                Self::FloatType(FloatType::new(raw))
            }
            LLVMTypeKind::LLVMIntegerTypeKind => Self::IntType(IntType::new(raw)),
            LLVMTypeKind::LLVMPointerTypeKind => Self::PointerType(PointerType::new(raw)),
            LLVMTypeKind::LLVMStructTypeKind => Self::StructType(StructType::new(raw)),
            LLVMTypeKind::LLVMVectorTypeKind => Self::VectorType(VectorType::new(raw)),
            LLVMTypeKind::LLVMScalableVectorTypeKind => {
                Self::ScalableVectorType(ScalableVectorType::new(raw))
            }
            other => panic!("unsupported LLVM basic type kind: {:?}", other),
        }
    }

    pub fn const_zero(self) -> BasicValueEnum<'ctx> {
        match self {
            Self::ArrayType(t) => t.const_zero().into(),
            Self::FloatType(t) => t.const_zero().into(),
            Self::IntType(t) => t.const_zero().into(),
            Self::PointerType(t) => t.const_zero().into(),
            Self::StructType(t) => t.const_zero().into(),
            Self::VectorType(t) => t.const_zero().into(),
            Self::ScalableVectorType(t) => t.const_zero().into(),
        }
    }

    pub fn array_type(self, len: u32) -> ArrayType<'ctx> {
        ArrayType::new(unsafe { LLVMArrayType2(self.as_type_ref(), len as u64) })
    }

    pub fn is_pointer_type(self) -> bool {
        matches!(self, Self::PointerType(_))
    }

    pub fn into_array_type(self) -> ArrayType<'ctx> {
        match self {
            Self::ArrayType(value) => value,
            _ => panic!("expected array type"),
        }
    }

    pub fn into_float_type(self) -> FloatType<'ctx> {
        match self {
            Self::FloatType(value) => value,
            _ => panic!("expected float type"),
        }
    }

    pub fn into_int_type(self) -> IntType<'ctx> {
        match self {
            Self::IntType(value) => value,
            _ => panic!("expected int type"),
        }
    }

    pub fn into_pointer_type(self) -> PointerType<'ctx> {
        match self {
            Self::PointerType(value) => value,
            _ => panic!("expected pointer type"),
        }
    }

    pub fn into_struct_type(self) -> StructType<'ctx> {
        match self {
            Self::StructType(value) => value,
            _ => panic!("expected struct type"),
        }
    }
}

impl<'ctx> AsTypeRef for BasicTypeEnum<'ctx> {
    fn as_type_ref(&self) -> LLVMTypeRef {
        match self {
            Self::ArrayType(value) => value.as_type_ref(),
            Self::FloatType(value) => value.as_type_ref(),
            Self::IntType(value) => value.as_type_ref(),
            Self::PointerType(value) => value.as_type_ref(),
            Self::StructType(value) => value.as_type_ref(),
            Self::VectorType(value) => value.as_type_ref(),
            Self::ScalableVectorType(value) => value.as_type_ref(),
        }
    }
}

impl<'ctx> BasicType<'ctx> for BasicTypeEnum<'ctx> {}
impl<'ctx> BasicType<'ctx> for IntType<'ctx> {}
impl<'ctx> BasicType<'ctx> for FloatType<'ctx> {}
impl<'ctx> BasicType<'ctx> for PointerType<'ctx> {}
impl<'ctx> BasicType<'ctx> for StructType<'ctx> {}
impl<'ctx> BasicType<'ctx> for ArrayType<'ctx> {}
impl<'ctx> BasicType<'ctx> for VectorType<'ctx> {}
impl<'ctx> BasicType<'ctx> for ScalableVectorType<'ctx> {}

impl<'ctx> From<IntType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: IntType<'ctx>) -> Self {
        Self::IntType(value)
    }
}

impl<'ctx> From<FloatType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: FloatType<'ctx>) -> Self {
        Self::FloatType(value)
    }
}

impl<'ctx> From<PointerType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: PointerType<'ctx>) -> Self {
        Self::PointerType(value)
    }
}

impl<'ctx> From<StructType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: StructType<'ctx>) -> Self {
        Self::StructType(value)
    }
}

impl<'ctx> From<ArrayType<'ctx>> for BasicTypeEnum<'ctx> {
    fn from(value: ArrayType<'ctx>) -> Self {
        Self::ArrayType(value)
    }
}

pub type BasicMetadataTypeEnum<'ctx> = BasicTypeEnum<'ctx>;

macro_rules! impl_value_wrapper {
    ($name:ident, $basic_method:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name<'ctx> {
            raw: LLVMValueRef,
            _marker: PhantomData<&'ctx Context>,
        }

        impl<'ctx> $name<'ctx> {
            fn new(raw: LLVMValueRef) -> Self {
                assert!(!raw.is_null());
                Self {
                    raw,
                    _marker: PhantomData,
                }
            }
        }

        impl<'ctx> AsValueRef for $name<'ctx> {
            fn as_value_ref(&self) -> LLVMValueRef {
                self.raw
            }
        }

        impl<'ctx> BasicValue<'ctx> for $name<'ctx> {
            fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx> {
                BasicValueEnum::$basic_method(*self)
            }
        }
    };
}

impl_value_wrapper!(IntValue, IntValue);
impl_value_wrapper!(FloatValue, FloatValue);
impl_value_wrapper!(PointerValue, PointerValue);
impl_value_wrapper!(StructValue, StructValue);
impl_value_wrapper!(ArrayValue, ArrayValue);

impl<'ctx> AggregateValue<'ctx> for StructValue<'ctx> {}
impl<'ctx> AggregateValue<'ctx> for ArrayValue<'ctx> {}

impl<'ctx> IntValue<'ctx> {
    pub fn get_type(self) -> IntType<'ctx> {
        IntType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

impl<'ctx> FloatValue<'ctx> {
    pub fn get_type(self) -> FloatType<'ctx> {
        FloatType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

impl<'ctx> PointerValue<'ctx> {
    pub fn get_type(self) -> PointerType<'ctx> {
        PointerType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

impl<'ctx> StructValue<'ctx> {
    pub fn get_type(self) -> StructType<'ctx> {
        StructType::new(unsafe { LLVMTypeOf(self.raw) })
    }

    pub fn as_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMIsAInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }
}

impl<'ctx> ArrayValue<'ctx> {
    pub fn get_type(self) -> ArrayType<'ctx> {
        ArrayType::new(unsafe { LLVMTypeOf(self.raw) })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicValueEnum<'ctx> {
    ArrayValue(ArrayValue<'ctx>),
    FloatValue(FloatValue<'ctx>),
    IntValue(IntValue<'ctx>),
    PointerValue(PointerValue<'ctx>),
    StructValue(StructValue<'ctx>),
}

impl<'ctx> BasicValueEnum<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        match BasicTypeEnum::new(unsafe { LLVMTypeOf(raw) }) {
            BasicTypeEnum::ArrayType(_) => Self::ArrayValue(ArrayValue::new(raw)),
            BasicTypeEnum::FloatType(_) => Self::FloatValue(FloatValue::new(raw)),
            BasicTypeEnum::IntType(_) => Self::IntValue(IntValue::new(raw)),
            BasicTypeEnum::PointerType(_) => Self::PointerValue(PointerValue::new(raw)),
            BasicTypeEnum::StructType(_) => Self::StructValue(StructValue::new(raw)),
            BasicTypeEnum::VectorType(_) | BasicTypeEnum::ScalableVectorType(_) => {
                panic!("vector values are not supported in kernc llvm wrapper")
            }
        }
    }

    pub fn get_type(self) -> BasicTypeEnum<'ctx> {
        BasicTypeEnum::new(unsafe { LLVMTypeOf(self.as_value_ref()) })
    }

    pub fn is_int_value(self) -> bool {
        matches!(self, Self::IntValue(_))
    }

    pub fn is_float_value(self) -> bool {
        matches!(self, Self::FloatValue(_))
    }

    pub fn is_pointer_value(self) -> bool {
        matches!(self, Self::PointerValue(_))
    }

    pub fn is_struct_value(self) -> bool {
        matches!(self, Self::StructValue(_))
    }

    pub fn into_array_value(self) -> ArrayValue<'ctx> {
        match self {
            Self::ArrayValue(value) => value,
            _ => panic!("expected array value"),
        }
    }

    pub fn into_float_value(self) -> FloatValue<'ctx> {
        match self {
            Self::FloatValue(value) => value,
            _ => panic!("expected float value"),
        }
    }

    pub fn into_int_value(self) -> IntValue<'ctx> {
        match self {
            Self::IntValue(value) => value,
            _ => panic!("expected int value"),
        }
    }

    pub fn into_pointer_value(self) -> PointerValue<'ctx> {
        match self {
            Self::PointerValue(value) => value,
            _ => panic!("expected pointer value"),
        }
    }

    pub fn into_struct_value(self) -> StructValue<'ctx> {
        match self {
            Self::StructValue(value) => value,
            _ => panic!("expected struct value"),
        }
    }

    pub fn as_instruction_value(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMIsAInstruction(self.as_value_ref()) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }
}

impl<'ctx> AsValueRef for BasicValueEnum<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        match self {
            Self::ArrayValue(value) => value.as_value_ref(),
            Self::FloatValue(value) => value.as_value_ref(),
            Self::IntValue(value) => value.as_value_ref(),
            Self::PointerValue(value) => value.as_value_ref(),
            Self::StructValue(value) => value.as_value_ref(),
        }
    }
}

impl<'ctx> BasicValue<'ctx> for BasicValueEnum<'ctx> {
    fn as_basic_value_enum(&self) -> BasicValueEnum<'ctx> {
        *self
    }
}

impl<'ctx> From<IntValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: IntValue<'ctx>) -> Self {
        Self::IntValue(value)
    }
}

impl<'ctx> From<FloatValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: FloatValue<'ctx>) -> Self {
        Self::FloatValue(value)
    }
}

impl<'ctx> From<PointerValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: PointerValue<'ctx>) -> Self {
        Self::PointerValue(value)
    }
}

impl<'ctx> From<StructValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: StructValue<'ctx>) -> Self {
        Self::StructValue(value)
    }
}

impl<'ctx> From<ArrayValue<'ctx>> for BasicValueEnum<'ctx> {
    fn from(value: ArrayValue<'ctx>) -> Self {
        Self::ArrayValue(value)
    }
}

pub type BasicMetadataValueEnum<'ctx> = BasicValueEnum<'ctx>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionValue<'ctx> {
    raw: LLVMValueRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> FunctionValue<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn get_nth_param(self, index: u32) -> Option<BasicValueEnum<'ctx>> {
        let count = unsafe { LLVMCountParams(self.raw) };
        if index >= count {
            None
        } else {
            Some(BasicValueEnum::new(unsafe { LLVMGetParam(self.raw, index) }))
        }
    }

    pub fn get_first_basic_block(self) -> Option<BasicBlock<'ctx>> {
        let block = unsafe { LLVMGetFirstBasicBlock(self.raw) };
        if block.is_null() {
            None
        } else {
            Some(BasicBlock::new(block))
        }
    }

    pub fn get_type(self) -> FunctionType<'ctx> {
        FunctionType::new(unsafe { LLVMGlobalGetValueType(self.raw) })
    }

    pub fn add_attribute(self, loc: AttributeLoc, attribute: Attribute) {
        let index = match loc {
            AttributeLoc::Function => LLVMAttributeFunctionIndex,
        };
        unsafe { LLVMAddAttributeAtIndex(self.raw, index, attribute.raw) };
    }

    pub fn as_global_value(self) -> GlobalValue<'ctx> {
        GlobalValue::new(self.raw)
    }
}

impl<'ctx> AsValueRef for FunctionValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalValue<'ctx> {
    raw: LLVMValueRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> GlobalValue<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn as_pointer_value(self) -> PointerValue<'ctx> {
        PointerValue::new(self.raw)
    }

    pub fn set_initializer<V: BasicValue<'ctx>>(self, value: &V) {
        unsafe { LLVMSetInitializer(self.raw, value.as_value_ref()) };
    }

    pub fn set_constant(self, constant: bool) {
        unsafe { LLVMSetGlobalConstant(self.raw, bool_to_llvm(constant)) };
    }

    pub fn set_linkage(self, linkage: Linkage) {
        unsafe { LLVMSetLinkage(self.raw, linkage.into()) };
    }

    pub fn set_section(self, section: Option<&str>) {
        let section = section.unwrap_or("");
        let section = to_c_string(section);
        unsafe { LLVMSetSection(self.raw, section.as_ptr()) };
    }

    pub fn set_alignment(self, bytes: u32) {
        unsafe { LLVMSetAlignment(self.raw, bytes) };
    }
}

impl<'ctx> AsValueRef for GlobalValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BasicBlock<'ctx> {
    raw: LLVMBasicBlockRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> BasicBlock<'ctx> {
    fn new(raw: LLVMBasicBlockRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn get_parent(self) -> Option<FunctionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockParent(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(FunctionValue::new(value))
        }
    }

    pub fn get_terminator(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetBasicBlockTerminator(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }

    pub fn get_first_instruction(self) -> Option<InstructionValue<'ctx>> {
        let value = unsafe { LLVMGetFirstInstruction(self.raw) };
        if value.is_null() {
            None
        } else {
            Some(InstructionValue::new(value))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstructionValue<'ctx> {
    raw: LLVMValueRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> InstructionValue<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn set_atomic_ordering(self, ordering: AtomicOrdering) -> Result<(), ()> {
        unsafe { LLVMSetOrdering(self.raw, ordering.into()) };
        Ok(())
    }
}

impl<'ctx> AsValueRef for InstructionValue<'ctx> {
    fn as_value_ref(&self) -> LLVMValueRef {
        self.raw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhiValue<'ctx> {
    raw: LLVMValueRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> PhiValue<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn add_incoming(self, incoming: &[(&dyn BasicValue<'ctx>, BasicBlock<'ctx>)]) {
        let mut values = incoming.iter().map(|(value, _)| value.as_value_ref()).collect::<Vec<_>>();
        let mut blocks = incoming.iter().map(|(_, block)| block.raw).collect::<Vec<_>>();
        unsafe { LLVMAddIncoming(self.raw, values.as_mut_ptr(), blocks.as_mut_ptr(), incoming.len() as u32) };
    }

    pub fn as_basic_value(self) -> BasicValueEnum<'ctx> {
        BasicValueEnum::new(self.raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallSiteValue<'ctx> {
    raw: LLVMValueRef,
    _marker: PhantomData<&'ctx Context>,
}

impl<'ctx> CallSiteValue<'ctx> {
    fn new(raw: LLVMValueRef) -> Self {
        assert!(!raw.is_null());
        Self {
            raw,
            _marker: PhantomData,
        }
    }

    pub fn try_as_basic_value(self) -> CallSiteTryAsValue<'ctx> {
        let ty = unsafe { LLVMTypeOf(self.raw) };
        let value = if unsafe { LLVMGetTypeKind(ty) } == LLVMTypeKind::LLVMVoidTypeKind {
            None
        } else {
            Some(BasicValueEnum::new(self.raw))
        };
        CallSiteTryAsValue { value }
    }
}

pub struct CallSiteTryAsValue<'ctx> {
    value: Option<BasicValueEnum<'ctx>>,
}

impl<'ctx> CallSiteTryAsValue<'ctx> {
    pub fn unwrap_basic(self) -> BasicValueEnum<'ctx> {
        self.value.expect("expected non-void call result")
    }
}
