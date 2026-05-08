use crate::{MirBlockId, MirLocalId};
use kernc_ast::MetaItem;
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_mono::{MonoId, MonoModuleMetadata};
use kernc_ty::TypeId;
use kernc_utils::{AtomicOrdering, AtomicRmwOp, Span, SymbolId};

#[derive(Debug, Clone)]
pub struct MirModule {
    pub name: String,
    pub structs: Vec<MirStruct>,
    pub globals: Vec<MirGlobal>,
    pub functions: Vec<MirFunction>,
    pub mono: MonoModuleMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirLinkage {
    External,
    LinkOnceOdr,
    Internal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MirInlineHint {
    #[default]
    None,
    Inline,
    NoInline,
}

#[derive(Debug, Clone)]
pub struct MirStruct {
    pub id: MonoId,
    pub name: String,
    pub fields: Vec<MirField>,
    pub is_extern: bool,
    pub is_union: bool,
    pub largest_field_idx: usize,
    pub union_size: usize,
    pub union_align: usize,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MirField {
    pub name: SymbolId,
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct MirGlobal {
    pub id: MonoId,
    pub name: String,
    pub span: Span,
    pub linkage: MirLinkage,
    pub ty: TypeId,
    pub is_mut: bool,
    pub init: Option<MirStaticInit>,
    pub is_extern: bool,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MirFunction {
    pub id: MonoId,
    pub name: String,
    pub span: Span,
    pub linkage: MirLinkage,
    pub params: Vec<MirParam>,
    pub ret_ty: TypeId,
    pub body: Option<MirBody>,
    pub is_extern: bool,
    pub is_variadic: bool,
    pub inline_hint: MirInlineHint,
    pub attributes: Vec<MetaItem>,
}

#[derive(Debug, Clone)]
pub struct MirParam {
    pub name: SymbolId,
    pub ty: TypeId,
    pub is_mut: bool,
}

#[derive(Debug, Clone)]
pub struct MirBody {
    pub entry: MirBlockId,
    pub locals: Vec<MirLocal>,
    pub blocks: Vec<MirBlock>,
}

#[derive(Debug, Clone)]
pub struct MirBlock {
    pub id: MirBlockId,
    pub instructions: Vec<MirInstructionData>,
    pub terminator: MirTerminatorData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirLocalKind {
    Param,
    Let,
}

#[derive(Debug, Clone)]
pub struct MirLocal {
    pub id: MirLocalId,
    pub name: SymbolId,
    pub span: Span,
    pub ty: TypeId,
    pub is_mut: bool,
    pub kind: MirLocalKind,
}

#[derive(Debug, Clone)]
pub enum MirPlace {
    Local(MirLocalId),
    Global(MonoId),
    Deref(MirOperand),
    Field {
        base: Box<MirPlace>,
        struct_id: MonoId,
        field_idx: usize,
        field_ty: TypeId,
    },
    Index {
        base: Box<MirPlace>,
        index: MirOperand,
    },
}

#[derive(Debug, Clone)]
pub enum MirConst {
    Undef { ty: TypeId },
    Integer { ty: TypeId, value: u128 },
    Float { ty: TypeId, value: f64 },
    Bool { value: bool },
    StringLiteral { ty: TypeId, value: String },
    GlobalRef { ty: TypeId, id: MonoId },
    FuncRef { ty: TypeId, id: MonoId },
}

impl MirConst {
    pub fn ty(&self) -> TypeId {
        match self {
            Self::Undef { ty }
            | Self::Integer { ty, .. }
            | Self::Float { ty, .. }
            | Self::StringLiteral { ty, .. }
            | Self::GlobalRef { ty, .. }
            | Self::FuncRef { ty, .. } => *ty,
            Self::Bool { .. } => TypeId::BOOL,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MirStaticInit {
    Const(MirConst),
    Array {
        ty: TypeId,
        elems: Vec<MirStaticInit>,
    },
    FatPointer {
        ty: TypeId,
        data_ptr: Box<MirStaticInit>,
        meta: Box<MirStaticInit>,
    },
    Struct {
        ty: TypeId,
        struct_id: MonoId,
        fields: Vec<MirStaticInit>,
    },
    Union {
        ty: TypeId,
        union_id: MonoId,
        field_idx: usize,
        value: Box<MirStaticInit>,
    },
    Data {
        ty: TypeId,
        data_struct_id: MonoId,
        tag_value: u128,
        payload: Option<Box<MirStaticInit>>,
    },
}

impl MirStaticInit {
    pub fn ty(&self) -> TypeId {
        match self {
            Self::Const(value) => value.ty(),
            Self::Array { ty, .. }
            | Self::FatPointer { ty, .. }
            | Self::Struct { ty, .. }
            | Self::Union { ty, .. }
            | Self::Data { ty, .. } => *ty,
        }
    }
}

#[derive(Debug, Clone)]
pub enum MirOperand {
    Local(MirLocalId),
    Const(MirConst),
}

#[derive(Debug, Clone)]
pub enum MirSliceBase {
    Operand(MirOperand),
    Place(MirPlace),
}

#[derive(Debug, Clone)]
pub enum MirCallTarget {
    Direct(MonoId),
    Operand(MirOperand),
}

#[derive(Debug, Clone)]
pub enum MirAggregateKind {
    Struct {
        struct_id: MonoId,
    },
    Union {
        union_id: MonoId,
        field_idx: usize,
    },
    Array,
    FatPointer,
    Data {
        data_struct_id: MonoId,
        tag_value: u128,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirProjectionKind {
    FatPtrData,
    FatPtrMeta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCastKind {
    Bitcast,
    PtrToInt,
    IntToPtr,
    SignExt,
    ZeroExt,
    Trunc,
    SIntToFloat,
    UIntToFloat,
    FloatToSInt,
    FloatToUInt,
    FloatCast,
    ArrayToSlice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBitIntrinsicKind {
    PopCount,
    Clz,
    Ctz,
    Bswap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirSimdUnaryIntrinsicKind {
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirSimdBinaryIntrinsicKind {
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirSimdReduceKind {
    Add,
    Mul,
    And,
    Or,
    Xor,
    Min,
    Max,
}

#[derive(Debug, Clone)]
pub enum MirRvalue {
    Use(MirOperand),
    Call {
        callee: MirCallTarget,
        args: Vec<MirOperand>,
    },
    Aggregate {
        ty: TypeId,
        kind: MirAggregateKind,
        fields: Vec<MirOperand>,
    },
    Projection {
        kind: MirProjectionKind,
        operand: MirOperand,
    },
    Unary {
        op: UnaryOperator,
        operand: MirOperand,
    },
    Binary {
        op: BinaryOperator,
        lhs: MirOperand,
        rhs: MirOperand,
    },
    Cast {
        kind: MirCastKind,
        operand: MirOperand,
    },
    BitIntrinsic {
        kind: MirBitIntrinsicKind,
        operand: MirOperand,
    },
    AtomicLoad {
        ptr: MirOperand,
        ordering: AtomicOrdering,
    },
    AtomicCas {
        weak: bool,
        ptr: MirOperand,
        expected: MirOperand,
        desired: MirOperand,
        success: AtomicOrdering,
        failure: AtomicOrdering,
    },
    AtomicRmw {
        op: AtomicRmwOp,
        ptr: MirOperand,
        value: MirOperand,
        ordering: AtomicOrdering,
    },
    SimdUnaryIntrinsic {
        kind: MirSimdUnaryIntrinsicKind,
        operand: MirOperand,
    },
    SimdBinaryIntrinsic {
        kind: MirSimdBinaryIntrinsicKind,
        lhs: MirOperand,
        rhs: MirOperand,
    },
    SimdReduce {
        kind: MirSimdReduceKind,
        operand: MirOperand,
    },
    SimdAny {
        operand: MirOperand,
    },
    SimdAll {
        operand: MirOperand,
    },
    SimdBitmask {
        operand: MirOperand,
    },
    SimdSplat {
        value: MirOperand,
    },
    SimdCast {
        value: MirOperand,
    },
    SimdBitcast {
        value: MirOperand,
    },
    SimdSelect {
        mask: MirOperand,
        on_true: MirOperand,
        on_false: MirOperand,
    },
    SimdShuffle {
        lhs: MirOperand,
        rhs: MirOperand,
        indices: Vec<u32>,
    },
    SimdInsertHalf {
        base: MirOperand,
        half: MirOperand,
        high_half: bool,
    },
    SimdLoad {
        ptr: MirOperand,
        align: u32,
    },
    SimdMaskedLoad {
        ptr: MirOperand,
        mask: MirOperand,
        or_else: MirOperand,
        align: u32,
    },
    SimdGather {
        ptr: MirOperand,
        indices: MirOperand,
    },
    SimdMaskedGather {
        ptr: MirOperand,
        indices: MirOperand,
        mask: MirOperand,
        or_else: MirOperand,
    },
    SliceOp {
        lhs: MirSliceBase,
        start: Option<MirOperand>,
        end: Option<MirOperand>,
        is_inclusive: bool,
    },
    AddressOf(MirPlace),
    Load(MirPlace),
}

#[derive(Debug, Clone)]
pub enum MirMemoryIntrinsic {
    Copy {
        dest: MirOperand,
        src: MirOperand,
        len: MirOperand,
    },
    Move {
        dest: MirOperand,
        src: MirOperand,
        len: MirOperand,
    },
    Set {
        dest: MirOperand,
        val: MirOperand,
        len: MirOperand,
    },
}

#[derive(Debug, Clone)]
pub struct MirInlineAsm {
    pub asm_template: String,
    pub constraints: String,
    pub input_args: Vec<MirOperand>,
    pub output_ptrs: Vec<MirOperand>,
    pub output_tys: Vec<TypeId>,
    pub is_volatile: bool,
}

#[derive(Debug, Clone)]
pub struct MirInstructionData {
    pub span: Span,
    pub kind: MirInstruction,
}

#[derive(Debug, Clone)]
pub enum MirInstruction {
    Let {
        place: MirPlace,
        init: MirRvalue,
    },
    Assign {
        place: MirPlace,
        op: AssignmentOperator,
        value: MirRvalue,
    },
    Memory(MirMemoryIntrinsic),
    InlineAsm(MirInlineAsm),
    SimdStore {
        ptr: MirOperand,
        value: MirOperand,
        align: u32,
    },
    SimdMaskedStore {
        ptr: MirOperand,
        mask: MirOperand,
        value: MirOperand,
        align: u32,
    },
    SimdScatter {
        ptr: MirOperand,
        indices: MirOperand,
        value: MirOperand,
    },
    SimdMaskedScatter {
        ptr: MirOperand,
        indices: MirOperand,
        mask: MirOperand,
        value: MirOperand,
    },
    AtomicStore {
        ptr: MirOperand,
        value: MirOperand,
        ordering: AtomicOrdering,
    },
    Fence {
        ordering: AtomicOrdering,
    },
    Trap,
    Breakpoint,
    Eval(MirRvalue),
    Defer(MirRvalue),
}

#[derive(Debug, Clone)]
pub struct MirTerminatorData {
    pub span: Span,
    pub kind: MirTerminator,
}

#[derive(Debug, Clone)]
pub enum MirTerminator {
    Goto(MirBlockId),
    Branch {
        cond: MirRvalue,
        then_block: MirBlockId,
        else_block: MirBlockId,
    },
    Switch {
        target: MirRvalue,
        cases: Vec<MirSwitchTarget>,
        default_block: Option<MirBlockId>,
    },
    Return(Option<MirRvalue>),
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct MirSwitchTarget {
    pub values: Vec<u128>,
    pub block: MirBlockId,
}
