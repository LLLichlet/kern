use super::MastBlock;
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_mono::MonoId;
use kernc_ty::TypeId;
use kernc_utils::{AtomicOrdering, AtomicRmwOp, Span, SymbolId};

/// Every MAST expression carries its fully resolved type explicitly.
#[derive(Debug, Clone)]
pub struct MastExpr {
    pub ty: TypeId,
    /// Used for diagnostics and optional debug info generation.
    pub span: Span,
    pub kind: MastExprKind,
}

impl MastExpr {
    pub fn new(ty: TypeId, kind: MastExprKind, span: Span) -> Self {
        Self { ty, kind, span }
    }
}

#[derive(Debug, Clone)]
pub enum MastExprKind {
    // --- 1. Basic literals ---
    Undef,
    Unreachable,
    Trap,
    Breakpoint,
    Integer(u128),
    Float(f64),
    Bool(bool),
    /// Compiler-owned string bytes used by internal synthetic values.
    /// Source string literals lower to ordinary byte array values.
    StringLiteral(String),

    // --- 2. References ---
    /// Local variable or parameter reference.
    Var(SymbolId),
    /// Reference to a static global, producing a pointer value.
    GlobalRef(MonoId),
    /// Reference to a concrete function, producing a function pointer.
    FuncRef(MonoId),

    // --- 3. Memory operations ---
    AddressOf(Box<MastExpr>),
    Deref(Box<MastExpr>),

    // --- 4. Aggregate construction and access ---
    StructInit {
        struct_id: MonoId,
        /// Field initializers already ordered in physical layout order.
        fields: Vec<MastExpr>,
    },
    UnionInit {
        union_id: MonoId,
        field_idx: usize,
        value: Box<MastExpr>,
    },
    ArrayInit(Vec<MastExpr>),

    /// Struct field access.
    FieldAccess {
        lhs: Box<MastExpr>,
        /// Concrete owner struct used for layout lookup.
        struct_id: MonoId,
        field_idx: usize,
    },

    /// Array or slice indexing.
    IndexAccess {
        lhs: Box<MastExpr>,
        index: Box<MastExpr>,
    },

    // --- 5. Execution and control flow ---
    /// Unified call form used after methods and generics are lowered to plain callees.
    Call {
        callee: Box<MastExpr>,
        args: Vec<MastExpr>,
    },

    If {
        cond: Box<MastExpr>,
        then_branch: MastBlock,
        else_branch: Option<MastBlock>,
    },

    /// Loop body plus an optional latch block for lowered `for`-loop post expressions.
    /// `continue` jumps to the latch before reevaluating the loop.
    Loop {
        body: MastBlock,
        /// Lowered `for` post clause.
        latch: Option<MastBlock>,
    },

    /// Switch is preserved because LLVM has a native `switch` instruction.
    Switch {
        target: Box<MastExpr>,
        cases: Vec<MastSwitchCase>,
        default_case: Option<MastBlock>,
    },

    Break,
    Continue,
    /// Return expression after coercions have already been applied.
    Return(Option<Box<MastExpr>>),

    // --- 6. Operators ---
    Binary {
        op: BinaryOperator,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    Unary {
        op: UnaryOperator,
        operand: Box<MastExpr>,
    },
    Assign {
        op: AssignmentOperator,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    /// Explicitly evaluate an expression for effects and drop its value.
    Discard(Box<MastExpr>),

    // --- 7. Casts ---
    /// Frontend `as` casts are lowered into LLVM-oriented cast categories here.
    Cast {
        kind: MastCastKind,
        operand: Box<MastExpr>,
    },

    // --- 8. Fat pointers / trait objects ---
    /// Manual construction of a two-field fat pointer struct.
    ConstructFatPointer {
        data_ptr: Box<MastExpr>,
        /// Vtable pointer for trait objects, or immediate metadata such as slice length.
        meta: Box<MastExpr>,
    },

    /// Extracts the data pointer from a fat pointer (`extractvalue 0`).
    ExtractFatPtrData(Box<MastExpr>),
    /// Extracts fat-pointer metadata (`extractvalue 1`).
    ExtractFatPtrMeta(Box<MastExpr>),

    // --- 9. Executable blocks ---
    /// A block executed as a standalone expression value.
    Block(MastBlock),

    // --- 10. Enum primitives ---
    /// Constructs a payload-carrying enum value.
    /// Physically this is emitted as a `{ TagType, UnionType }` struct.
    DataInit {
        /// Wrapper struct generated during lowering.
        data_struct_id: MonoId,
        /// Concrete enum discriminant value.
        tag_value: u128,
        /// Variant payload, or `Undef` when the variant is payload-free.
        payload: Box<MastExpr>,
    },

    // --- 11. LLVM inline assembly ---
    /// Lowered representation tailored to LLVM `call asm`.
    Asm(MastAsmBlock),

    BitIntrinsic {
        kind: BitIntrinsicKind,
        operand: Box<MastExpr>,
    },
    SimdUnaryIntrinsic {
        kind: SimdUnaryIntrinsicKind,
        operand: Box<MastExpr>,
    },
    SimdBinaryIntrinsic {
        kind: SimdBinaryIntrinsicKind,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    SimdReduce {
        kind: SimdReduceKind,
        operand: Box<MastExpr>,
    },
    SimdAny {
        operand: Box<MastExpr>,
    },
    SimdAll {
        operand: Box<MastExpr>,
    },
    SimdBitmask {
        operand: Box<MastExpr>,
    },
    SimdSplat {
        value: Box<MastExpr>,
    },
    SimdCast {
        value: Box<MastExpr>,
    },
    SimdBitcast {
        value: Box<MastExpr>,
    },
    SimdSelect {
        mask: Box<MastExpr>,
        on_true: Box<MastExpr>,
        on_false: Box<MastExpr>,
    },
    SimdShuffle {
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
        indices: Vec<u32>,
    },
    SimdInsertHalf {
        base: Box<MastExpr>,
        half: Box<MastExpr>,
        high_half: bool,
    },
    SimdLoad {
        ptr: Box<MastExpr>,
        align: u32,
    },
    SimdStore {
        ptr: Box<MastExpr>,
        value: Box<MastExpr>,
        align: u32,
    },
    SimdMaskedLoad {
        ptr: Box<MastExpr>,
        mask: Box<MastExpr>,
        or_else: Box<MastExpr>,
        align: u32,
    },
    SimdMaskedStore {
        ptr: Box<MastExpr>,
        mask: Box<MastExpr>,
        value: Box<MastExpr>,
        align: u32,
    },
    SimdGather {
        ptr: Box<MastExpr>,
        indices: Box<MastExpr>,
    },
    SimdScatter {
        ptr: Box<MastExpr>,
        indices: Box<MastExpr>,
        value: Box<MastExpr>,
    },
    SimdMaskedGather {
        ptr: Box<MastExpr>,
        indices: Box<MastExpr>,
        mask: Box<MastExpr>,
        or_else: Box<MastExpr>,
    },
    SimdMaskedScatter {
        ptr: Box<MastExpr>,
        indices: Box<MastExpr>,
        mask: Box<MastExpr>,
        value: Box<MastExpr>,
    },
    AtomicLoad {
        ptr: Box<MastExpr>,
        ordering: AtomicOrdering,
    },
    AtomicStore {
        ptr: Box<MastExpr>,
        value: Box<MastExpr>,
        ordering: AtomicOrdering,
    },
    AtomicCas {
        weak: bool,
        ptr: Box<MastExpr>,
        expected: Box<MastExpr>,
        desired: Box<MastExpr>,
        success: AtomicOrdering,
        failure: AtomicOrdering,
    },
    AtomicRmw {
        op: AtomicRmwOp,
        ptr: Box<MastExpr>,
        value: Box<MastExpr>,
        ordering: AtomicOrdering,
    },
    Fence {
        ordering: AtomicOrdering,
    },

    Memcpy {
        dest: Box<MastExpr>,
        src: Box<MastExpr>,
        len: Box<MastExpr>,
    },
    Memmove {
        dest: Box<MastExpr>,
        src: Box<MastExpr>,
        len: Box<MastExpr>,
    },
    Memset {
        dest: Box<MastExpr>,
        val: Box<MastExpr>,
        len: Box<MastExpr>,
    },

    /// Primitive slice assembly operation.
    SliceOp {
        lhs: Box<MastExpr>,
        start: Option<Box<MastExpr>>,
        end: Option<Box<MastExpr>>,
        is_inclusive: bool,
    },
}

#[derive(Debug, Clone)]
pub struct MastSwitchCase {
    // After const-eval, every case pattern becomes a concrete integer.
    pub values: Vec<u128>,
    pub body: MastBlock,
}

/// Detailed cast categories chosen to map cleanly onto LLVM IR operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastCastKind {
    Bitcast,      // Same-size bit reinterpretation, e.g. `*i32` to `*u8`.
    PtrToInt,     // Pointer to integer, e.g. `*u8` to `usize`.
    IntToPtr,     // Integer to pointer, e.g. `usize` to `*u8`.
    SignExt,      // Signed integer extension, e.g. `i8` to `i32`.
    ZeroExt,      // Unsigned integer extension, e.g. `u8` to `u32`.
    Trunc,        // Integer truncation, e.g. `i32` to `i8`.
    SIntToFloat,  // sitofp
    UIntToFloat,  // uitofp
    FloatToSInt,  // fptosi
    FloatToUInt,  // fptoui
    FloatCast,    // Floating-point precision conversion, e.g. `f32` <=> `f64`.
    ArrayToSlice, // Implicit decay that constructs a slice fat pointer.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitIntrinsicKind {
    PopCount,
    Clz,
    Ctz,
    Bswap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdUnaryIntrinsicKind {
    Abs,
    Sqrt,
    Floor,
    Ceil,
    Trunc,
    Round,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdBinaryIntrinsicKind {
    Min,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimdReduceKind {
    Add,
    Mul,
    And,
    Or,
    Xor,
    Min,
    Max,
}

#[derive(Debug, Clone)]
pub struct MastAsmBlock {
    /// Merged assembly template, for example `"out dx, al \n in al, dx"`.
    pub asm_template: String,

    /// LLVM constraint string, for example `"={al},{dx},{al},~{memory}"`.
    pub constraints: String,

    /// Runtime input operands passed to the asm call.
    pub input_args: Vec<MastExpr>,

    /// Pointers that receive asm outputs.
    /// Codegen stores the returned values into these locations automatically.
    pub output_ptrs: Vec<MastExpr>,

    /// Base output types used by codegen to materialize correct receive logic.
    pub output_tys: Vec<TypeId>,

    pub is_volatile: bool,
}
