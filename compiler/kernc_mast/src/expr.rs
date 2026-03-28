use super::{MastBlock, MonoId};
use kernc_ast::{AssignmentOperator, BinaryOperator, UnaryOperator};
use kernc_sema::ty::TypeId;
use kernc_utils::{AtomicOrdering, AtomicRmwOp, Span, SymbolId};

/// 每一个 MAST 表达式都必须显式携带它的具体类型。
#[derive(Debug, Clone)]
pub struct MastExpr {
    pub ty: TypeId,
    pub span: Span, // 仅用于报错或生成 Debug Info (DWARF)
    pub kind: MastExprKind,
}

impl MastExpr {
    pub fn new(ty: TypeId, kind: MastExprKind, span: Span) -> Self {
        Self { ty, kind, span }
    }
}

#[derive(Debug, Clone)]
pub enum MastExprKind {
    // --- 1. 基本字面量 ---
    Undef,
    Unreachable,
    Trap,
    Breakpoint,
    Integer(u128),
    Float(f64),
    Bool(bool),
    /// 字符串在 LLVM 中通常生成一个全局常量数组。
    /// 保留 StringLiteral 方便 Codegen 时自动生成 Global Variable 并返回指针。
    StringLiteral(String),

    // --- 2. 引用 ---
    Var(SymbolId),     // 局部变量/函数参数引用
    GlobalRef(MonoId), // 引用 static 全局变量 (返回的是指针)
    FuncRef(MonoId),   // 引用具体的函数 (返回函数指针)

    // --- 3. 内存操作 ---
    AddressOf(Box<MastExpr>),
    Deref(Box<MastExpr>),

    // --- 4. 聚合数据访问与构造 ---
    StructInit {
        struct_id: MonoId,
        /// 已经按照结构体内存布局排序好的字段初始化值
        fields: Vec<MastExpr>,
    },
    UnionInit {
        union_id: MonoId,
        field_idx: usize,
        value: Box<MastExpr>,
    },
    ArrayInit(Vec<MastExpr>),

    /// 结构体字段访问
    FieldAccess {
        lhs: Box<MastExpr>,
        struct_id: MonoId, // 显式记录所属结构体的具体 MonoId
        field_idx: usize,
    },

    /// 数组或切片索引
    IndexAccess {
        lhs: Box<MastExpr>,
        index: Box<MastExpr>,
    },

    // --- 5. 执行与控制流 ---
    /// 统一的调用接口 (方法调用、泛型调用均已被 Lowerer 转换为普通的 FuncRef 或 Var 调用)
    Call {
        callee: Box<MastExpr>,
        args: Vec<MastExpr>,
    },

    If {
        cond: Box<MastExpr>,
        then_branch: MastBlock,
        else_branch: Option<MastBlock>,
    },

    /// 包含循环体和一个专门的 Latch (锁存) 块，用于执行 `i += 1` 等 post 语句。
    /// 遇到 continue 时，会直接跳转到 latch 块执行，然后再判断是否进入下一轮。
    Loop {
        body: MastBlock,
        latch: Option<MastBlock>, // 对应 for 循环的 post 语句
    },

    /// Switch 被保留，因为 LLVM 有原生的 `switch` 指令，比 if-else 链快得多。
    Switch {
        target: Box<MastExpr>,
        cases: Vec<MastSwitchCase>,
        default_case: Option<MastBlock>,
    },

    Break,
    Continue,
    Return(Option<Box<MastExpr>>), // 包含的表达式已经过 Coercion 类型转换

    // --- 6. 运算 ---
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

    // --- 7. 类型转换 (细化，讨好 LLVM) ---
    /// 在前端，一切转换都是 `as`。但在 MAST，必须拆分成 LLVM 级别的具体操作。
    Cast {
        kind: MastCastKind,
        operand: Box<MastExpr>,
    },

    // --- 8. 胖指针 / Trait Object 构建 ---
    /// `let r = p as mut Reader;` 降级为手动拼装一个包含两个指针的 Struct
    ConstructFatPointer {
        data_ptr: Box<MastExpr>,
        /// 如果是 Trait Object，这是 vtable_ptr；
        /// 如果是 Slice/String，这是一个常量 Integer 表示长度！
        meta: Box<MastExpr>,
    },

    /// 提取胖指针的数据指针 (相当于 llvm extractvalue 0)
    ExtractFatPtrData(Box<MastExpr>),
    /// 提取胖指针的元数据 (vtable_ptr 或 slice_len，相当于 extractvalue 1)
    ExtractFatPtrMeta(Box<MastExpr>),

    // --- 9. 执行块 ---
    /// 作为一个整体表达式执行的代码块 (用于嵌套作用域和 Defer 展开)
    Block(MastBlock),

    // --- 10. Enum 原语 (背后是 Struct+Union 或 纯整数 布局) ---
    /// 构建一个带负载的 Enum 实例。
    /// 在物理上，LLVM 把它当作一个 `{ TagType, UnionType }` 的结构体。
    DataInit {
        data_struct_id: MonoId, // 降级后的包装结构体 ID
        tag_value: u128,        // 具体的枚举鉴别器
        /// 变体的具体负载，如果没有负载就是 Undef
        payload: Box<MastExpr>,
    },

    // --- 11. LLVM Inline Assembly ---
    /// 经过 Lowering 降级后，完美契合 LLVM `call asm` 指令的数据结构
    Asm(MastAsmBlock),

    BitIntrinsic {
        kind: BitIntrinsicKind,
        operand: Box<MastExpr>,
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
    Memset {
        dest: Box<MastExpr>,
        val: Box<MastExpr>,
        len: Box<MastExpr>,
    },

    /// 底层切片组装指令
    SliceOp {
        lhs: Box<MastExpr>,
        start: Option<Box<MastExpr>>,
        end: Option<Box<MastExpr>>,
        is_inclusive: bool,
    },
}

#[derive(Debug, Clone)]
pub struct MastSwitchCase {
    // 经过 Const Eval 后，所有的 case pattern 都变成了确定的整数值
    pub values: Vec<u128>,
    pub body: MastBlock,
}

/// 详尽的类型转换分类，与 LLVM IR 指令一一对应
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastCastKind {
    Bitcast,      // 相同大小的位模式转换 (如 *i32 到 *u8)
    PtrToInt,     // 指针转整数 (如 *u8 到 usize)
    IntToPtr,     // 整数转指针 (如 usize 到 *u8)
    SignExt,      // 有符号整数扩展 (如 i8 到 i32)
    ZeroExt,      // 无符号整数扩展 (如 u8 到 u32)
    Trunc,        // 整数截断 (如 i32 到 i8)
    SIntToFloat,  // sitofp
    UIntToFloat,  // uitofp
    FloatToSInt,  // fptosi
    FloatToUInt,  // fptoui
    FloatCast,    // 浮点数精度转换 (f32 <=> f64)
    ArrayToSlice, // 隐式降级：构造切片胖指针
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitIntrinsicKind {
    PopCount,
    Clz,
    Ctz,
    Bswap,
}

#[derive(Debug, Clone)]
pub struct MastAsmBlock {
    /// 经过合并的汇编模板字符串，例如 "out dx, al \n in al, dx"
    pub asm_template: String,

    /// LLVM 标准约束字符串，例如 "={al},{dx},{al},~{memory}"
    pub constraints: String,

    /// 传给内联汇编的实参 (仅包含 inputs)
    pub input_args: Vec<MastExpr>,

    /// 接收返回值的指针 (对应 outputs)
    /// Codegen 阶段会自动将汇编返回的结果 Store 到这些指针里
    pub output_ptrs: Vec<MastExpr>,

    /// 输出变量的基础类型 (用于 Codegen 生成正确的接收和提取指令)
    pub output_tys: Vec<TypeId>,

    pub is_volatile: bool,
}
