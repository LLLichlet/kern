// src/sema/typeck/const_eval.rs

use crate::ast::{Expr, ExprKind, BinaryOperator, UnaryOperator};
use crate::context::Context;
use crate::sema::def::Def;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind};
use crate::sema::resolve_types::TypeResolver;

/// 编译期常量求值器 (Const Evaluator)
pub struct ConstEvaluator<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> ConstEvaluator<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 计算并返回安全的 usize (例如用于数组长度)
    pub fn eval_usize(&mut self, expr: &Expr) -> u64 {
        match self.eval_math(expr) {
            Ok(val) => {
                if val < 0 {
                    self.ctx.emit_error(expr.span, "Constant expression cannot evaluate to a negative number here".into());
                    0
                } else {
                    val as u64
                }
            }
            Err(_) => 0, // 错误已在内部抛出
        }
    }

    /// 核心递归求值引擎 (使用 i128 防止计算过程溢出)
    pub fn eval_math(&mut self, expr: &Expr) -> Result<i128, ()> {
        match &expr.kind {
            ExprKind::Integer(val) => Ok(*val as i128),
            ExprKind::Char(c) => Ok(*c as i128),
            ExprKind::Bool(b) => Ok(if *b { 1 } else { 0 }),

            // === 1. 递归计算算术运算 ===
            ExprKind::Binary { lhs, op, rhs } => {
                let left = self.eval_math(lhs)?;
                let right = self.eval_math(rhs)?;

                match op {
                    BinaryOperator::Add => Ok(left.wrapping_add(right)),
                    BinaryOperator::Subtract => Ok(left.wrapping_sub(right)),
                    BinaryOperator::Multiply => Ok(left.wrapping_mul(right)),
                    BinaryOperator::Divide => {
                        if right == 0 {
                            self.ctx.emit_error(rhs.span, "Division by zero in constant expression".into());
                            Err(())
                        } else {
                            Ok(left / right)
                        }
                    }
                    BinaryOperator::Modulo => {
                        if right == 0 {
                            self.ctx.emit_error(rhs.span, "Modulo by zero in constant expression".into());
                            Err(())
                        } else {
                            Ok(left % right)
                        }
                    }
                    BinaryOperator::ShiftLeft => Ok(left << right),
                    BinaryOperator::ShiftRight => Ok(left >> right),
                    BinaryOperator::BitwiseAnd => Ok(left & right),
                    BinaryOperator::BitwiseOr => Ok(left | right),
                    BinaryOperator::BitwiseXor => Ok(left ^ right),
                    _ => {
                        self.ctx.emit_error(expr.span, "Operator not supported in constant expression".into());
                        Err(())
                    }
                }
            }

            ExprKind::Unary { op, operand } => {
                let val = self.eval_math(operand)?;
                match op {
                    UnaryOperator::Negate => Ok(-val),
                    UnaryOperator::BitwiseNot => Ok(!val),
                    UnaryOperator::LogicalNot => Ok(if val == 0 { 1 } else { 0 }),
                    _ => {
                        self.ctx.emit_error(expr.span, "Unary operator not supported in constant expression".into());
                        Err(())
                    }
                }
            }

            // === 2. 查表代入全局 Const 变量 ===
            ExprKind::Identifier(name) => {
                let sym_info = self.ctx.scopes.resolve(*name).cloned();
                
                if let Some(info) = sym_info {
                    // Kern 规范：只有 `const` (编译期常量) 能参与 Const Eval，`static` (运行时全局变量) 和 `let` 不行
                    if info.kind == SymbolKind::Const {
                        if let Some(def_id) = info.def_id {
                            // 为了避免 Borrow Checker 冲突，我们克隆出对应的 AST 表达式
                            let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                g.value.clone()
                            } else {
                                return Err(());
                            };
                            
                            // 递归计算该常量的值
                            return self.eval_math(&const_expr);
                        }
                    } else {
                        let name_str = self.ctx.resolve(*name).to_string();
                        self.ctx.emit_error(expr.span, format!("`{}` is a {}, not a compile-time constant. Only `const` variables can be used here.", name_str, self.kind_to_string(info.kind)));
                        return Err(());
                    }
                }
                self.ctx.emit_error(expr.span, "Undeclared identifier in constant expression".into());
                Err(())
            }
            
            ExprKind::GenericInstantiation { target, types } => {
                 // 处理 @sizeof[T] 这种情况
                 // 如果 target 是 Intrinsic，我们需要在这里截获，并利用 `types` 计算大小
                 // TODO: 集成 Types 内存布局计算
                 Ok(8) // 打桩
            }

            // === 3. 处理内置常量函数调用 ===
            ExprKind::Call { callee, args } => {
                // 直接从节点类型缓存中拿到 Callee 的类型 (Typeck阶段已经解析过了)
                let callee_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
                let norm_callee = self.ctx.type_registry.normalize(callee_ty);
                
                // 完美的统一：如果它是一个绑定了泛型的函数定义 (比如 @sizeof[Point])
                if let TypeKind::FnDef(def_id, generic_args) = self.ctx.type_registry.get(norm_callee) {
                    if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                        if f.is_intrinsic {
                            let name_str = self.ctx.resolve(f.name);
                            match name_str {
                                "@sizeof" => {
                                    // 我们要测量的类型，正是附带在 FnDef 里的第一个泛型实参！
                                    if let Some(&target_ty) = generic_args.get(0) {
                                        let size = self.compute_type_size(target_ty);
                                        return Ok(size as i128);
                                    }
                                }
                                "@alignof" => {
                                    // 顺手把 @alignof 也支持了
                                    if let Some(&target_ty) = generic_args.get(0) {
                                        let align = self.compute_type_align(target_ty);
                                        return Ok(align as i128);
                                    }
                                }
                                _ => {
                                    self.ctx.emit_error(expr.span, format!("Intrinsic `{}` cannot be evaluated at compile time", name_str));
                                    return Err(());
                                }
                            }
                        }
                    }
                }
                
                self.ctx.emit_error(expr.span, "Function calls are not allowed in constant expressions (except for certain compile-time intrinsics like @sizeof)".into());
                Err(())
            }

            _ => {
                self.ctx.emit_error(expr.span, "Expected a constant expression".into());
                Err(())
            }
        }
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable (`let`)",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            _ => "symbol",
        }
    }

    // ==========================================
    //          Memory Layout Engine
    // ==========================================

    /// 计算类型的内存对齐要求 (Alignment)
    fn compute_type_align(&self, ty: TypeId) -> u64 {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Primitive(p) => match p {
                crate::sema::ty::PrimitiveType::I8 | crate::sema::ty::PrimitiveType::U8 | crate::sema::ty::PrimitiveType::Bool => 1,
                crate::sema::ty::PrimitiveType::I16 | crate::sema::ty::PrimitiveType::U16 => 2,
                crate::sema::ty::PrimitiveType::I32 | crate::sema::ty::PrimitiveType::U32 | crate::sema::ty::PrimitiveType::F32 => 4,
                crate::sema::ty::PrimitiveType::I64 | crate::sema::ty::PrimitiveType::U64 | crate::sema::ty::PrimitiveType::F64 | 
                crate::sema::ty::PrimitiveType::ISize | crate::sema::ty::PrimitiveType::USize => 8,
                crate::sema::ty::PrimitiveType::I128 | crate::sema::ty::PrimitiveType::U128 => 16,
                _ => 1,
            },
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) => 8, // TODO: 假设 64-bit 架构
            TypeKind::Slice(_) => 8, // 切片(胖指针)包含指针和长度，两者都是 8 字节对齐
            TypeKind::Mut(inner) => self.compute_type_align(*inner),
            TypeKind::Array { elem, .. } => self.compute_type_align(*elem),
            
            TypeKind::Def(def_id, generic_args) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                match def {
                    Def::Struct(s) => {
                        let mut max_align = 1;
                        for field in &s.fields {
                            // TODO: 完整的布局引擎需要处理结构体内部泛型字段的替换。
                            // 简化处理：拿到字段类型后计算对齐
                            let f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            let align = self.compute_type_align(f_ty);
                            if align > max_align { max_align = align; }
                        }
                        max_align
                    }
                    Def::Union(u) => {
                        let mut max_align = 1;
                        for field in &u.fields {
                            let f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            let align = self.compute_type_align(f_ty);
                            if align > max_align { max_align = align; }
                        }
                        max_align
                    }
                    Def::Enum(e) => {
                        // 枚举的对齐由其 backing_type 决定，默认为 u32
                        let back_ty = if let Some(bt) = &e.backing_type {
                            self.ctx.node_types.get(&bt.id).copied().unwrap_or(TypeId::U32)
                        } else {
                            TypeId::U32
                        };
                        self.compute_type_align(back_ty)
                    }
                    Def::Trait(_) => 8, // Trait 本身无大小，但转换为 TraitObject 时对齐为 8
                    _ => 1,
                }
            }
            _ => 1,
        }
    }

    /// 辅助方法：将偏移量向上取整到指定的对齐边界
    fn align_to(offset: u64, align: u64) -> u64 {
        (offset + align - 1) & !(align - 1)
    }

    /// 计算类型的内存占用大小 (Size)
    fn compute_type_size(&self, ty: TypeId) -> u64 {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Primitive(p) => match p {
                crate::sema::ty::PrimitiveType::I8 | crate::sema::ty::PrimitiveType::U8 | crate::sema::ty::PrimitiveType::Bool => 1,
                crate::sema::ty::PrimitiveType::I16 | crate::sema::ty::PrimitiveType::U16 => 2,
                crate::sema::ty::PrimitiveType::I32 | crate::sema::ty::PrimitiveType::U32 | crate::sema::ty::PrimitiveType::F32 => 4,
                crate::sema::ty::PrimitiveType::I64 | crate::sema::ty::PrimitiveType::U64 | crate::sema::ty::PrimitiveType::F64 | 
                crate::sema::ty::PrimitiveType::ISize | crate::sema::ty::PrimitiveType::USize => 8,
                crate::sema::ty::PrimitiveType::I128 | crate::sema::ty::PrimitiveType::U128 => 16,
                _ => 0,
            },
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) => 8, // 假设 64-bit 架构
            TypeKind::Slice(_) => 16, // 胖指针：ptr(8) + len(8)
            TypeKind::Mut(inner) => self.compute_type_size(*inner),
            TypeKind::Array { elem, len } => self.compute_type_size(*elem) * len,
            
            TypeKind::Def(def_id, _) => {
                let def = &self.ctx.defs[def_id.0 as usize];
                match def {
                    Def::Struct(s) => {
                        let mut offset = 0;
                        let mut max_align = 1;
                        for field in &s.fields {
                            let f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            let f_align = self.compute_type_align(f_ty);
                            let f_size = self.compute_type_size(f_ty);
                            
                            if f_align > max_align { max_align = f_align; }
                            // 对齐当前字段
                            offset = Self::align_to(offset, f_align);
                            // 加上大小
                            offset += f_size;
                        }
                        // 整个结构体的大小必须是最大对齐数的整数倍
                        Self::align_to(offset, max_align)
                    }
                    Def::Union(u) => {
                        let mut max_size = 0;
                        let mut max_align = 1;
                        for field in &u.fields {
                            let f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            let f_align = self.compute_type_align(f_ty);
                            let f_size = self.compute_type_size(f_ty);
                            
                            if f_align > max_align { max_align = f_align; }
                            if f_size > max_size { max_size = f_size; }
                        }
                        Self::align_to(max_size, max_align)
                    }
                    Def::Enum(e) => {
                        let back_ty = if let Some(bt) = &e.backing_type {
                            self.ctx.node_types.get(&bt.id).copied().unwrap_or(TypeId::U32)
                        } else {
                            TypeId::U32
                        };
                        self.compute_type_size(back_ty)
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}