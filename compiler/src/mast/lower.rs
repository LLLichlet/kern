// src/mast/ast.rs

use crate::sema::ty::TypeId;
use crate::utils::{SymbolId, Span};

/// 单态化后的函数/结构体唯一标识符 (例如 `std.collections.ArrayList_i32`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonoId(pub u32);

/// MAST 顶层程序 (一个扁平的实体列表，没有嵌套的模块，直接喂给 LLVM)
#[derive(Debug, Clone)]
pub struct MastProgram {
    pub structs: Vec<MastStruct>,
    pub globals: Vec<MastGlobal>,
    pub functions: Vec<MastFunction>,
    // 注意：Trait 和 TypeAlias 在这里彻底消失了！
}

// ==========================================
//          Top-Level Definitions
// ==========================================

#[derive(Debug, Clone)]
pub struct MastStruct {
    pub id: MonoId,
    pub name: String, // 扁平化后的名字，例如 "Point_i32"
    pub fields: Vec<MastField>,
    pub is_extern: bool,
}

#[derive(Debug, Clone)]
pub struct MastField {
    pub name: SymbolId,
    pub ty: TypeId, // 这里保证绝对不会出现 TypeKind::Param
}

#[derive(Debug, Clone)]
pub struct MastGlobal {
    pub id: MonoId,
    pub name: String,
    pub ty: TypeId,
    pub init: Option<MastExpr>, // extern 的时候为 None
    pub is_extern: bool,
}

#[derive(Debug, Clone)]
pub struct MastFunction {
    pub id: MonoId,
    pub name: String, // 扁平化后的名字，例如 "Point_i32_move_by"
    pub params: Vec<MastParam>,
    pub ret_ty: TypeId,
    pub body: Option<MastBlock>, // extern 时为 None
    pub is_extern: bool,
    pub is_variadic: bool,
}

#[derive(Debug, Clone)]
pub struct MastParam {
    pub name: SymbolId,
    pub ty: TypeId,
}

// ==========================================
//          Statements & Expressions
// ==========================================

#[derive(Debug, Clone)]
pub struct MastBlock {
    pub stmts: Vec<MastStmt>,
    pub result: Option<Box<MastExpr>>,
}

#[derive(Debug, Clone)]
pub enum MastStmt {
    Let {
        name: SymbolId,
        ty: TypeId,
        init: MastExpr,
    },
    Expr(MastExpr),
    // Defer 在这里消失了，被 Lowering 引擎塞入了 Block 的末尾
}

/// 每一个表达式都强制携带具体的类型
#[derive(Debug, Clone)]
pub struct MastExpr {
    pub ty: TypeId, // 生成 LLVM IR 时极度依赖这个字段
    pub span: Span,
    pub kind: MastExprKind,
}

#[derive(Debug, Clone)]
pub enum MastExprKind {
    // 基础字面量
    Integer(u128),
    Float(f64),
    Bool(bool),
    String(String), // 或者可以直接在 lowering 时转成全局常量的引用
    
    // 变量引用
    Var(SymbolId),       // 局部变量
    GlobalRef(MonoId),   // 对全局变量/常量的引用
    FuncRef(MonoId),     // 对具体函数的引用

    // 内存与指针操作
    AddressOf(Box<MastExpr>),
    Deref(Box<MastExpr>),
    
    // 聚合类型操作
    StructInit {
        struct_id: MonoId,
        fields: Vec<(SymbolId, MastExpr)>,
    },
    ArrayInit(Vec<MastExpr>),
    FieldAccess {
        lhs: Box<MastExpr>,
        field_idx: usize, // 已经计算好了是在第几个字段，方便 LLVM 的 getelementptr
    },
    IndexAccess {
        lhs: Box<MastExpr>,
        index: Box<MastExpr>,
    },

    // 函数调用
    Call {
        callee: Box<MastExpr>,
        args: Vec<MastExpr>,
    },

    // 控制流 (只有最基础的)
    If {
        cond: Box<MastExpr>,
        then_branch: MastBlock,
        else_branch: Option<MastBlock>,
    },
    Loop(MastBlock), // for 已经被降级成 loop
    Break,
    Continue,
    Return(Option<Box<MastExpr>>),
    
    // 操作符
    Binary {
        op: crate::ast::BinaryOperator,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    Unary {
        op: crate::ast::UnaryOperator,
        operand: Box<MastExpr>,
    },
    Assign {
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    Cast {
        lhs: Box<MastExpr>,
        target_ty: TypeId,
    },

    // Trait Object 的构建 (极其底层，直接构建胖指针)
    // 将 `p as mut Reader` 降级为一个包含指针和 VTable 地址的结构体初始化
    ConstructTraitObject {
        data_ptr: Box<MastExpr>,
        vtable_ptr: Box<MastExpr>, // VTable 实际上被 lowering 成了一个 Global
    },
}