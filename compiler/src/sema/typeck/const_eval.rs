// src/sema/typeck/const_eval.rs

use crate::ast::{Expr, ExprKind, BinaryOperator, UnaryOperator};
use crate::context::Context;
use crate::sema::def::Def;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind};

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
            
            ExprKind::GenericInstantiation { .. } => {
                 // 这个节点本身不产生值，它通常作为 Call 的 callee (例如 @sizeof[T])。
                 // 如果单独出现，属于非法表达式。
                 self.ctx.emit_error(expr.span, "Generic instantiation cannot be evaluated as a standalone constant value. Did you mean to call it like `@sizeof[T]()`?".into());
                 Err(())
            }

            // === 3. 处理内置常量函数调用 ===
            ExprKind::Call { callee, .. } => {
                // 直接从节点类型缓存中拿到 Callee 的类型 (Typeck阶段已经解析过了)
                let callee_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
                let norm_callee = self.ctx.type_registry.normalize(callee_ty);
                
                // 完美的统一：如果它是一个绑定了泛型的函数定义 (比如 @sizeof[Point])
                if let TypeKind::FnDef(def_id, generic_args) = self.ctx.type_registry.get(norm_callee).clone() {
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

            ExprKind::EnumLiteral(variant_name) => {
                // 求值器从节点缓存中拿到确切类型
                let ty = self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR);
                let norm_ty = self.ctx.type_registry.normalize(ty);
                
                let def_id = if let TypeKind::Def(id, _) = self.ctx.type_registry.get(norm_ty) {
                    *id
                } else {
                    self.ctx.emit_error(expr.span, "Enum literal type not resolved".into());
                    return Err(());
                };
                
                let enum_def = if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                    e.clone() // 克隆以释放借用
                } else { return Err(()); };

                let mut current_val: i128 = 0;
                for v in enum_def.variants {
                    if let Some(v_expr) = v.value {
                        current_val = self.eval_math(&v_expr)?;
                    }
                    if v.name == *variant_name {
                        return Ok(current_val);
                    }
                    current_val += 1;
                }
                
                self.ctx.emit_error(expr.span, "Variant not found in Enum".into());
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
    fn compute_type_align(&mut self, ty: TypeId) -> u64 {
        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Primitive(p) => match p {
                crate::sema::ty::PrimitiveType::I8 | crate::sema::ty::PrimitiveType::U8 | crate::sema::ty::PrimitiveType::Bool => 1,
                crate::sema::ty::PrimitiveType::I16 | crate::sema::ty::PrimitiveType::U16 => 2,
                crate::sema::ty::PrimitiveType::I32 | crate::sema::ty::PrimitiveType::U32 | crate::sema::ty::PrimitiveType::F32 => 4,
                crate::sema::ty::PrimitiveType::I64 | crate::sema::ty::PrimitiveType::U64 | crate::sema::ty::PrimitiveType::F64 | 
                crate::sema::ty::PrimitiveType::ISize | crate::sema::ty::PrimitiveType::USize => 8,
                crate::sema::ty::PrimitiveType::I128 | crate::sema::ty::PrimitiveType::U128 => 16,
                _ => 1,
            },
            // ✅ 动态读取目标架构的指针大小
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) | TypeKind::Function { .. } => self.ctx.target.pointer_size, 
            TypeKind::Slice(_) | TypeKind::TraitObject(..) => self.ctx.target.pointer_size, // 胖指针内部对齐依然是单指针宽度
            TypeKind::Mut(inner) => self.compute_type_align(inner),
            TypeKind::Array { elem, .. } => self.compute_type_align(elem),
            
            TypeKind::Def(def_id, generic_args) => {
                let def = self.ctx.defs[def_id.0 as usize].clone();
                match def {
                    Def::Struct(s) => {
                        let mut max_align = 1;
                        
                        // 建立泛型映射表
                        let mut map = std::collections::HashMap::new();
                        if !s.generics.is_empty() && !generic_args.is_empty() {
                            for (i, param) in s.generics.iter().enumerate() {
                                map.insert(param.name, generic_args[i]);
                            }
                        }

                        for field in &s.fields {
                            let mut f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            
                            // ✅ 核心代换逻辑：如果字段是泛型，将具体类型代入计算
                            if !map.is_empty() {
                                let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                f_ty = subst.substitute(f_ty);
                            }
                            
                            let align = self.compute_type_align(f_ty);
                            if align > max_align { max_align = align; }
                        }
                        max_align
                    }
                    Def::Union(u) => {
                        let mut max_align = 1;
                        
                        let mut map = std::collections::HashMap::new();
                        if !u.generics.is_empty() && !generic_args.is_empty() {
                            for (i, param) in u.generics.iter().enumerate() {
                                map.insert(param.name, generic_args[i]);
                            }
                        }

                        for field in &u.fields {
                            let mut f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            if !map.is_empty() {
                                let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                f_ty = subst.substitute(f_ty);
                            }
                            let align = self.compute_type_align(f_ty);
                            if align > max_align { max_align = align; }
                        }
                        max_align
                    }
                    Def::Enum(e) => {
                        let back_ty = if let Some(bt) = &e.backing_type {
                            self.ctx.node_types.get(&bt.id).copied().unwrap_or(TypeId::U32)
                        } else {
                            TypeId::U32
                        };
                        self.compute_type_align(back_ty)
                    }
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
    fn compute_type_size(&mut self, ty: TypeId) -> u64 {
        let norm = self.ctx.type_registry.normalize(ty);
        let kind = self.ctx.type_registry.get(norm).clone();

        match kind {
            TypeKind::Primitive(p) => match p {
                crate::sema::ty::PrimitiveType::I8 | crate::sema::ty::PrimitiveType::U8 | crate::sema::ty::PrimitiveType::Bool => 1,
                crate::sema::ty::PrimitiveType::I16 | crate::sema::ty::PrimitiveType::U16 => 2,
                crate::sema::ty::PrimitiveType::I32 | crate::sema::ty::PrimitiveType::U32 | crate::sema::ty::PrimitiveType::F32 => 4,
                crate::sema::ty::PrimitiveType::I64 | crate::sema::ty::PrimitiveType::U64 | crate::sema::ty::PrimitiveType::F64 | 
                crate::sema::ty::PrimitiveType::ISize | crate::sema::ty::PrimitiveType::USize => 8,
                crate::sema::ty::PrimitiveType::I128 | crate::sema::ty::PrimitiveType::U128 => 16,
                _ => 0,
            },
            // ✅ 动态读取目标机器指针大小，处理胖指针
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) | TypeKind::Function { .. } => self.ctx.target.pointer_size,
            TypeKind::Slice(_) | TypeKind::TraitObject(..) => self.ctx.target.pointer_size * 2, // 胖指针占用两个普通指针的宽度
            TypeKind::Mut(inner) => self.compute_type_size(inner),
            TypeKind::Array { elem, len } => self.compute_type_size(elem) * len,
            
            TypeKind::Def(def_id, generic_args) => {
                let def = self.ctx.defs[def_id.0 as usize].clone();
                match def {
                    Def::Struct(s) => {
                        let mut offset = 0;
                        let mut max_align = 1;
                        
                        let mut map = std::collections::HashMap::new();
                        if !s.generics.is_empty() && !generic_args.is_empty() {
                            for (i, param) in s.generics.iter().enumerate() {
                                map.insert(param.name, generic_args[i]);
                            }
                        }

                        for field in &s.fields {
                            let mut f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            if !map.is_empty() {
                                let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                f_ty = subst.substitute(f_ty);
                            }
                            
                            let f_align = self.compute_type_align(f_ty);
                            let f_size = self.compute_type_size(f_ty);
                            
                            if f_align > max_align { max_align = f_align; }
                            offset = Self::align_to(offset, f_align);
                            offset += f_size;
                        }
                        // 整个结构体的大小必须是最大对齐数的整数倍
                        Self::align_to(offset, max_align)
                    }
                    Def::Union(u) => {
                        let mut max_size = 0;
                        let mut max_align = 1;
                        
                        let mut map = std::collections::HashMap::new();
                        if !u.generics.is_empty() && !generic_args.is_empty() {
                            for (i, param) in u.generics.iter().enumerate() {
                                map.insert(param.name, generic_args[i]);
                            }
                        }

                        for field in &u.fields {
                            let mut f_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                            if !map.is_empty() {
                                let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                f_ty = subst.substitute(f_ty);
                            }
                            
                            let f_align = self.compute_type_align(f_ty);
                            let f_size = self.compute_type_size(f_ty);
                            
                            if f_align > max_align { max_align = f_align; }
                            if f_size > max_size { max_size = f_size; }
                        }
                        // 联合体的大小是最大字段的大小，且要按最大对齐数进行对齐
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