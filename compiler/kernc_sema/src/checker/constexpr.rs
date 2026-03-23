use crate::LayoutEngine;
use crate::SemaContext;
use crate::def::Def;
use crate::scope::SymbolKind;
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast::{self as ast, BinaryOperator, Expr, ExprKind, UnaryOperator};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<ConstValue>),
    Struct(HashMap<SymbolId, ConstValue>),
    Void,
    Undef,
}

pub struct ConstEvaluator<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    /// 提取数组长度等所需的无符号整数
    pub fn eval_usize(&mut self, expr: &Expr) -> Result<u64, ()> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => {
                if val < 0 {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "constant expression cannot evaluate to a negative number here",
                        )
                        .with_hint("array lengths and similar contexts require positive integers")
                        .emit();
                    Err(())
                } else {
                    Ok(val as u64)
                }
            }
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(())
            }
            Err(_) => Err(()),
        }
    }

    /// 提取普通的有符号整数常量
    pub fn eval_math(&mut self, expr: &Expr) -> Result<i128, ()> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => Ok(val),
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(())
            }
            Err(_) => Err(()),
        }
    }

    /// 核心递归求值引擎
    pub fn eval_inner(&mut self, expr: &Expr, depth: usize) -> Result<ConstValue, ()> {
        if depth > 100 {
            self.ctx
                .struct_error(
                    expr.span,
                    "constant evaluation exceeded maximum recursion depth",
                )
                .with_hint("check for circular references in your `const` declarations")
                .emit();
            return Err(());
        }

        let eval_result = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(val) => Ok(ConstValue::Int(*val as i128)),
            ExprKind::Float(val) => Ok(ConstValue::Float(*val)),
            ExprKind::Bool(b) => Ok(ConstValue::Bool(*b)),
            ExprKind::Char(c) => Ok(ConstValue::Int(*c as u32 as i128)),
            ExprKind::String(s) => Ok(ConstValue::String(s.clone())),
            ExprKind::Undef => Ok(ConstValue::Undef),

            // === 2. 算术与逻辑运算 ===
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(lhs, *op, rhs, depth, expr.span),
            ExprKind::Unary { op, operand } => {
                // 提前折叠负数字面量
                // 拦截 `-` 后面紧跟数字的情况，跳过对内部正数 (如 128) 的独立求值和越界检查，直接返回整体负数。
                if *op == UnaryOperator::Negate {
                    if let ExprKind::Integer(val) = &operand.kind {
                        Ok(ConstValue::Int(-(*val as i128)))
                    } else if let ExprKind::Float(val) = &operand.kind {
                        Ok(ConstValue::Float(-*val))
                    } else {
                        self.eval_unary(*op, operand, depth, expr.span)
                    }
                } else {
                    self.eval_unary(*op, operand, depth, expr.span)
                }
            }

            ExprKind::As { lhs, .. } => {
                let val = self.eval_inner(lhs, depth + 1)?;
                let target_ty = self
                    .ctx
                    .node_types
                    .get(&expr.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if let ConstValue::Int(v) = val {
                    let mut layout = LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(target_ty) * 8;
                    let mask = if bit_width >= 128 {
                        u128::MAX
                    } else {
                        (1 << bit_width) - 1
                    };
                    let u_val = (v as u128) & mask;

                    Ok(ConstValue::Int(u_val as i128))
                } else {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "only integer casts are supported in const context currently",
                        )
                        .emit();
                    Err(())
                }
            }

            // === 3. 查表代入全局 Const 变量 ===
            ExprKind::Identifier(name) => self.eval_identifier(*name, depth, expr.span),

            // === 4. 内置常量函数调用 (Intrinsics) ===
            ExprKind::Call { callee, args } => {
                self.eval_intrinsic_call(callee, args, depth, expr.span)
            }

            // === 5. 枚举字面量求值 ===
            ExprKind::EnumLiteral(variant_name) => {
                self.eval_enum_literal(expr.id, *variant_name, depth, expr.span)
            }

            // === 6. 数据初始化 (支持嵌套 Array 和 Struct) ===
            ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Scalar(inner) => self.eval_inner(inner, depth + 1),
                ast::DataLiteralKind::Array(elems) => {
                    let mut arr = Vec::new();
                    for e in elems {
                        arr.push(self.eval_inner(e, depth + 1)?);
                    }
                    Ok(ConstValue::Array(arr))
                }
                ast::DataLiteralKind::Struct(fields) => {
                    let mut map = HashMap::new();
                    for f in fields {
                        map.insert(f.name, self.eval_inner(&f.value, depth + 1)?);
                    }
                    Ok(ConstValue::Struct(map))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    let val = self.eval_inner(value, depth + 1)?;
                    let cnt = self.eval_usize(count)?;
                    Ok(ConstValue::Array(vec![val; cnt as usize]))
                }
            },

            // === 7. 常量聚合访问 (提取结构体字段和数组索引) ===
            ExprKind::FieldAccess { lhs, field } => {
                let lhs_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);

                if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(norm_lhs).clone() {
                    let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        m.scope_id
                    } else {
                        unreachable!()
                    };
                    if let Some(info) = self.ctx.scopes.resolve_in(mod_scope, *field).cloned() {
                        if info.kind == SymbolKind::Const {
                            if let Some(def_id) = info.def_id {
                                let const_expr =
                                    if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                        g.value.clone()
                                    } else {
                                        return Err(());
                                    };
                                self.eval_inner(&const_expr, depth + 1)
                            } else {
                                Err(())
                            }
                        } else {
                            let field_str = self.ctx.resolve(*field);
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!(
                                        "`{}` is a {}, not a compile-time constant",
                                        field_str,
                                        self.kind_to_string(info.kind)
                                    ),
                                )
                                .emit();
                            Err(())
                        }
                    } else {
                        let field_str = self.ctx.resolve(*field);
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!("constant `{}` not found in module", field_str),
                            )
                            .emit();
                        Err(())
                    }
                } else {
                    let base = self.eval_inner(lhs, depth + 1)?;
                    if let ConstValue::Struct(map) = base {
                        if let Some(val) = map.get(field) {
                            Ok(val.clone())
                        } else {
                            let field_str = self.ctx.resolve(*field);
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!("field `{}` not found in constant struct", field_str),
                                )
                                .emit();
                            Err(())
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                expr.span,
                                "attempted field access on a non-struct constant",
                            )
                            .emit();
                        Err(())
                    }
                }
            }

            ExprKind::IndexAccess { lhs, index, .. } => {
                let base = self.eval_inner(lhs, depth + 1)?;
                let idx = self.eval_usize(index)?;
                if let ConstValue::Array(arr) = base {
                    if idx < arr.len() as u64 {
                        Ok(arr[idx as usize].clone())
                    } else {
                        self.ctx
                            .struct_error(expr.span, "constant array index out of bounds")
                            .emit();
                        Err(())
                    }
                } else {
                    self.ctx
                        .struct_error(expr.span, "attempted indexing into a non-array constant")
                        .emit();
                    Err(())
                }
            }

            ExprKind::GenericInstantiation { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated directly as a value",
                    )
                    .emit();
                Err(())
            }
            _ => {
                self.ctx
                    .struct_error(expr.span, "expected a valid constant expression")
                    .emit();
                Err(())
            }
        };

        // 获取刚刚求出的结果
        let mut val = eval_result?;

        // 越界与符号断言
        if let ConstValue::Int(mut v) = val {
            let ty = self
                .ctx
                .node_types
                .get(&expr.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm = self.ctx.type_registry.normalize(ty);

            if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm).clone() {
                let is_signed = matches!(
                    p,
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                );
                let is_unsigned = matches!(
                    p,
                    PrimitiveType::U8
                        | PrimitiveType::U16
                        | PrimitiveType::U32
                        | PrimitiveType::U64
                        | PrimitiveType::U128
                        | PrimitiveType::USize
                );

                // 洗白 i128 算出来的伪负数（比如 !0 -> -1）
                if is_unsigned {
                    let mut layout = crate::LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(norm) * 8;
                    if bit_width < 128 {
                        let mask = (1i128 << bit_width) - 1;
                        v &= mask; // 此时 -1 会被截断为的 0xFF...FF
                    }
                }

                // 1. 无符号类型不接受负数(经过洗白和 Unary 拦截后，走到这里的都是非法越界的硬编码值)
                if is_unsigned && v < 0 {
                    self.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", v, self.ctx.ty_to_string(ty)))
                        .with_hint("if you need a bit-pattern of all 1s, use explicit bitwise negation (e.g., `~0`) or `as` cast")
                        .emit();
                    return Err(());
                }

                // 2. 检查数值是否溢出相应的位宽容量
                if (is_signed || is_unsigned)
                    && p != PrimitiveType::I128
                    && p != PrimitiveType::U128
                {
                    let mut layout = crate::LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(norm) * 8;

                    let (min, max) = if is_signed {
                        let max = (1i128 << (bit_width - 1)) - 1;
                        let min = -(1i128 << (bit_width - 1));
                        (min, max)
                    } else {
                        let max = ((1u128 << bit_width) - 1) as i128;
                        (0, max)
                    };

                    if v < min || v > max {
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!(
                                    "integer literal {} is out of bounds for type `{}`",
                                    v,
                                    self.ctx.ty_to_string(ty)
                                ),
                            )
                            .with_hint(format!("the valid range is {} to {}", min, max))
                            .emit();
                        return Err(());
                    }
                }
            }
            val = ConstValue::Int(v);
        }

        Ok(val)
    }

    // ==========================================
    //            Const Eval Helpers
    // ==========================================

    fn eval_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
        let left = self.eval_inner(lhs, depth + 1)?;
        let right = self.eval_inner(rhs, depth + 1)?;

        match (left, right) {
            (ConstValue::Int(l), ConstValue::Int(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Int(l.wrapping_add(r))),
                    Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
                    Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
                    Divide => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "division by zero in constant expression")
                                .emit();
                            Err(())
                        } else {
                            Ok(ConstValue::Int(l / r))
                        }
                    }
                    Modulo => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "modulo by zero in constant expression")
                                .emit();
                            Err(())
                        } else {
                            Ok(ConstValue::Int(l % r))
                        }
                    }
                    ShiftLeft => Ok(ConstValue::Int(l << r)),
                    ShiftRight => Ok(ConstValue::Int(l >> r)),
                    BitwiseAnd => Ok(ConstValue::Int(l & r)),
                    BitwiseOr => Ok(ConstValue::Int(l | r)),
                    BitwiseXor => Ok(ConstValue::Int(l ^ r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant integers")
                            .emit();
                        Err(())
                    }
                }
            }
            (ConstValue::Float(l), ConstValue::Float(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Float(l + r)),
                    Subtract => Ok(ConstValue::Float(l - r)),
                    Multiply => Ok(ConstValue::Float(l * r)),
                    Divide => Ok(ConstValue::Float(l / r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant floats")
                            .emit();
                        Err(())
                    }
                }
            }
            (ConstValue::Bool(l), ConstValue::Bool(r)) => {
                use BinaryOperator::*;
                match op {
                    LogicalAnd => Ok(ConstValue::Bool(l && r)),
                    LogicalOr => Ok(ConstValue::Bool(l || r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant booleans")
                            .emit();
                        Err(())
                    }
                }
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant binary expression",
                    )
                    .emit();
                Err(())
            }
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
        let val = self.eval_inner(operand, depth + 1)?;

        let op_ty = self
            .ctx
            .node_types
            .get(&operand.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_ty = self.ctx.type_registry.normalize(op_ty);
        let is_unsigned = if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm_ty) {
            matches!(
                p,
                PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            )
        } else {
            false
        };

        match (op, val) {
            (UnaryOperator::Negate, ConstValue::Int(v)) => {
                if is_unsigned {
                    self.ctx.struct_error(span, "cannot apply unary minus `-` to an unsigned type")
                        .with_hint("unsigned types cannot be negative. use `~` or bitwise operations if you intend to manipulate bits")
                        .emit();
                    return Err(());
                }
                Ok(ConstValue::Int(v.wrapping_neg()))
            }
            (UnaryOperator::Negate, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
            (UnaryOperator::BitwiseNot, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)),
            (UnaryOperator::LogicalNot, ConstValue::Bool(v)) => Ok(ConstValue::Bool(!v)),
            _ => {
                self.ctx
                    .struct_error(span, "invalid unary operator for the given constant type")
                    .emit();
                Err(())
            }
        }
    }

    fn eval_identifier(
        &mut self,
        name: SymbolId,
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
        let sym_info = self.ctx.scopes.resolve(name).cloned();

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                        g.value.clone()
                    } else {
                        return Err(());
                    };

                    return self.eval_inner(&const_expr, depth + 1);
                }
            } else {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a {}, not a compile-time constant",
                            name_str,
                            self.kind_to_string(info.kind)
                        ),
                    )
                    .with_hint("only `const` variables can be used in constant expressions")
                    .emit();
                return Err(());
            }
        }
        self.ctx
            .struct_error(span, "use of undeclared identifier in constant expression")
            .emit();
        Err(())
    }

    pub(crate) fn eval_intrinsic_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
        let callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_callee = self.ctx.type_registry.normalize(callee_ty);

        let (def_id, generic_args) = match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::FnDef(id, args) => (id, args),
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "function calls are not allowed in constant expressions",
                    )
                    .emit();
                return Err(());
            }
        };

        let (is_intrinsic, fn_name_id, generics_len) =
            if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                (f.is_intrinsic, f.name, f.generics.len())
            } else {
                return Err(());
            };

        if !is_intrinsic {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .with_hint(
                    "only compile-time intrinsics like `@sizeOf` or `@clz` are permitted here",
                )
                .emit();
            return Err(());
        }

        let name_str = self.ctx.resolve(fn_name_id).to_string();

        // 在 const 阶段，强制要求泛型参数被显式填满
        if generic_args.len() != generics_len {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "intrinsic `{}` requires explicit generic arguments in constant evaluation",
                        name_str
                    ),
                )
                .with_hint(format!("example: `{}[u32](...)`", name_str))
                .emit();
            return Err(());
        }

        // --- 核心路由 ---
        match name_str.as_str() {
            "@sizeOf" => self.eval_size_of(&generic_args, span),
            "@alignOf" => self.eval_align_of(&generic_args, span),
            "@popCount" | "@clz" | "@ctz" => {
                self.eval_bit_counting(name_str.as_str(), &generic_args, args, depth, span)
            }
            "@intCast" => self.eval_int_cast(&generic_args, args, depth, span),
            "@bswap" => self.eval_bswap(&generic_args, args, depth, span),
            "@memcpy" | "@memset" => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "memory intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(())
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(())
            }
        }
    }

    // ==========================================
    // 具体的宏实现逻辑 (拆分后极易维护)
    // ==========================================

    fn eval_size_of(&mut self, generic_args: &[TypeId], _span: Span) -> Result<ConstValue, ()> {
        if let Some(&target_ty) = generic_args.get(0) {
            let mut layout = LayoutEngine::new(self.ctx);
            let size = layout.compute_type_size(target_ty);
            Ok(ConstValue::Int(size as i128))
        } else {
            Err(()) // 这个错误理论上在前面检查泛型数量时已被拦截
        }
    }

    fn eval_align_of(&mut self, generic_args: &[TypeId], _span: Span) -> Result<ConstValue, ()> {
        if let Some(&target_ty) = generic_args.get(0) {
            let mut layout = LayoutEngine::new(self.ctx);
            let align = layout.compute_type_align(target_ty);
            Ok(ConstValue::Int(align as i128))
        } else {
            Err(())
        }
    }

    fn eval_bit_counting(
        &mut self,
        name: &str,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> Result<ConstValue, ()> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[0];
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8; // 使用 LayoutEngine

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match name {
                "@popCount" => u_val.count_ones() as i128,
                "@clz" => (u_val.leading_zeros() as i128) - (128 - bit_width as i128),
                "@ctz" => {
                    if u_val == 0 {
                        bit_width as i128
                    } else {
                        u_val.trailing_zeros() as i128
                    }
                }
                _ => unreachable!(),
            };
            return Ok(ConstValue::Int(res));
        }
        Err(())
    }

    fn eval_int_cast(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> Result<ConstValue, ()> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[1];

            // 使用 LayoutEngine 获取目标类型的位宽
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let mut u_val = (val as u128) & mask;

            let is_signed = matches!(
                self.ctx.type_registry.get(target_ty),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );

            if is_signed && bit_width < 128 && (u_val & (1 << (bit_width - 1))) != 0 {
                u_val |= u128::MAX << bit_width;
            }
            return Ok(ConstValue::Int(u_val as i128));
        }
        Err(())
    }

    fn eval_bswap(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> Result<ConstValue, ()> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[0];

            // 使用 LayoutEngine
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match bit_width {
                16 => (u_val as u16).swap_bytes() as i128,
                32 => (u_val as u32).swap_bytes() as i128,
                64 => (u_val as u64).swap_bytes() as i128,
                128 => u_val.swap_bytes() as i128,
                _ => u_val as i128,
            };
            return Ok(ConstValue::Int(res));
        }
        Err(())
    }

    fn eval_enum_literal(
        &mut self,
        node_id: NodeId,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> Result<ConstValue, ()> {
        let ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let norm_ty = self.ctx.type_registry.normalize(ty);

        let def_id = if let TypeKind::Enum(id, _) = self.ctx.type_registry.get(norm_ty) {
            *id
        } else {
            self.ctx
                .struct_error(
                    span,
                    "variant literal type could not be resolved to a data type during constant evaluation",
                )
                .emit();
            return Err(());
        };

        let data_def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            return Err(());
        };

        // 禁止对带有 Payload 的 ADT 变体进行常量整数求值
        for v in &data_def.variants {
            if v.payload_type.is_some() {
                self.ctx
                    .struct_error(
                        span,
                        "cannot evaluate ADT variants with payloads as integer constants",
                    )
                    .with_hint("only C-style `data` types (without payloads) can be implicitly evaluated to integers")
                    .emit();
                return Err(());
            }
        }

        let mut current_val: i128 = 0;
        for v in data_def.variants {
            if let Some(v_expr) = v.value {
                if let Ok(ConstValue::Int(val)) = self.eval_inner(&v_expr, depth + 1) {
                    current_val = val;
                }
            }
            if v.name == variant_name {
                return Ok(ConstValue::Int(current_val));
            }
            current_val += 1;
        }

        let v_str = self.ctx.resolve(variant_name).to_string();
        self.ctx
            .struct_error(span, format!("variant `.{}` not found in data type", v_str))
            .emit();
        Err(())
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable (`let`)",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "data type",
            _ => "symbol",
        }
    }
}
