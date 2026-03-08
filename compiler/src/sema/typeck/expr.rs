#![allow(unused)]

use crate::ast::{self, Expr, ExprKind, BinaryOperator, UnaryOperator, AssignmentOperator, StmtKind};
use crate::context::Context;
use crate::sema::def::Def;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind, PrimitiveType};
use crate::sema::resolve_types::TypeResolver;
use crate::sema::typeck::SymbolInfo;
use crate::utils::Span;

pub struct ExprChecker<'a> {
    pub ctx: &'a mut Context,
    pub current_return_type: Option<TypeId>,
    pub has_returned: bool,
}

impl<'a> ExprChecker<'a> {
    pub fn new(ctx: &'a mut Context, current_return_type: Option<TypeId>) -> Self {
        Self { ctx, current_return_type, has_returned: false } 
    }

    /// 核心入口：检查表达式类型
    pub fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(_) => expected_ty.unwrap_or(TypeId::USIZE),
            ExprKind::Float(_) => expected_ty.unwrap_or(TypeId::F32),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32, 
            
            ExprKind::String(s) => {
                self.ctx.type_registry.intern(TypeKind::Slice(TypeId::U8))
            }
            
            ExprKind::Null => {
                self.ctx.emit_error(expr.span, "Null must be explicitly cast to a pointer type (e.g., `0 as *i32`).".into());
                TypeId::ERROR
            }
            
            // === 2. 标识符与变量 ===
            ExprKind::Identifier(name) => {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    info.type_id
                } else {
                    let name_str = self.ctx.resolve(*name).to_string();
                    self.ctx.emit_error(expr.span, format!("Use of undeclared identifier `{}`", name_str));
                    TypeId::ERROR
                }
            }

            ExprKind::SelfValue => {
                let self_var = self.ctx.intern("self"); // 小写的变量 self
                let self_type = self.ctx.intern("Self"); // 大写的类型 Self
                
                // 先尝试找变量，找不到就直接提取上下文中的 Self 类型
                if let Some(info) = self.ctx.scopes.resolve(self_var) {
                    info.type_id
                } else if let Some(info) = self.ctx.scopes.resolve(self_type) {
                    info.type_id
                } else {
                    self.ctx.emit_error(expr.span, "`self` is not available in this context".into());
                    TypeId::ERROR
                }
            }

            // === 3. 声明与绑定 ===
            ExprKind::Let { name, init } |
            ExprKind::Static { name, init } => {
                let init_ty = self.check_expr(init, expected_ty);

                let sym_kind = if matches!(expr.kind, ExprKind::Static { .. }) {
                    SymbolKind::Static
                } else {
                    SymbolKind::Var
                };

                let info = SymbolInfo {
                    kind: sym_kind,
                    node_id: expr.id,
                    type_id: init_ty,
                    def_id: None,
                };
                let _ = self.ctx.scopes.define(*name, info); 
                
                TypeId::VOID
            }

            // === 4. 运算与赋值 ===
            ExprKind::Binary { lhs, op, rhs } => self.check_binary(lhs, *op, rhs, expected_ty),
            ExprKind::Unary { op, operand } => self.check_unary(*op, operand, expr.span, expected_ty),
            ExprKind::Assign { lhs, op, rhs } => self.check_assign(lhs, *op, rhs, expr.span),
            
            // === 5. 转换与内建调用 ===
            ExprKind::As { lhs, target } => {
                let lhs_ty = self.check_expr(lhs, None);
                let mut resolver = TypeResolver::new(self.ctx);
                let scope = resolver.ctx.scopes.current_scope_id().unwrap();
                let target_ty = resolver.resolve_type(target, scope);
                
                self.check_cast(lhs.span, lhs_ty, target_ty);
                target_ty
            }

            // === 6. 内存访问 (索引, 字段, 切片) ===
            ExprKind::IndexAccess { lhs, index } => {
                let lhs_ty = self.check_expr(lhs, None);
                let idx_ty = self.check_expr(index, Some(TypeId::USIZE));
                
                let norm_idx = self.strip_mut(idx_ty);
                if !self.ctx.type_registry.is_integer(norm_idx) && norm_idx != TypeId::ERROR {
                    self.ctx.emit_error(index.span, "Index must be an integer type".into());
                }

                let norm_lhs = self.strip_mut(lhs_ty);
                match self.ctx.type_registry.get(norm_lhs) {
                    TypeKind::Array { elem, .. } | TypeKind::Slice(elem) => {
                        // 继承 LHS 的 Mut 属性
                        if self.is_mut_type(lhs_ty) {
                            self.ctx.type_registry.intern(TypeKind::Mut(*elem))
                        } else {
                            *elem
                        }
                    },
                    TypeKind::Error => TypeId::ERROR,
                    _ => {
                        self.ctx.emit_error(lhs.span, "Cannot index into a non-array/non-slice type".into());
                        TypeId::ERROR
                    }
                }
            }

            ExprKind::FieldAccess { lhs, field } => self.check_field_access(lhs, *field, expr.span),

            ExprKind::SliceOp { lhs, start, end, is_inclusive } => {
                let lhs_ty = self.check_expr(lhs, None);
                
                if let Some(s) = start {
                    let s_ty = self.check_expr(s, Some(TypeId::USIZE));
                    if !self.ctx.type_registry.is_integer(self.strip_mut(s_ty)) {
                        self.ctx.emit_error(s.span, "Slice start index must be an integer".into());
                    }
                }
                if let Some(e) = end {
                    let e_ty = self.check_expr(e, Some(TypeId::USIZE));
                    if !self.ctx.type_registry.is_integer(self.strip_mut(e_ty)) {
                        self.ctx.emit_error(e.span, "Slice end index must be an integer".into());
                    }
                }

                let norm_lhs = self.strip_mut(lhs_ty);
                match self.ctx.type_registry.get(norm_lhs) {
                    TypeKind::Array { elem, .. } | TypeKind::Slice(elem) => {
                        let base_elem = *elem;
                        let slice_elem = if self.is_mut_type(lhs_ty) { 
                            self.ctx.type_registry.intern(TypeKind::Mut(base_elem)) 
                        } else { 
                            base_elem 
                        };
                        
                        self.ctx.type_registry.intern(TypeKind::Slice(slice_elem))
                    },
                    TypeKind::Error => TypeId::ERROR,
                    _ => {
                        self.ctx.emit_error(lhs.span, "Cannot slice a non-array/non-slice type".into());
                        TypeId::ERROR
                    }
                }
            }

            // === 7. 函数/宏调用 ===
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),

            // === 8. 复杂字面量 ===
            ExprKind::DataInit { type_node, literal } => {
                let mut actual_expected = expected_ty;
                if let Some(ty_ast) = type_node {
                    let mut resolver = crate::sema::resolve_types::TypeResolver::new(self.ctx);
                    let scope = resolver.ctx.scopes.current_scope_id().unwrap();
                    let prefix_ty = resolver.resolve_type(ty_ast, scope);
                    
                    actual_expected = Some(prefix_ty);
                }

                if let Some(exp_ty) = actual_expected {
                    self.check_data_literal(literal, exp_ty, expr.span)
                } else {
                    if let ast::DataLiteralKind::Scalar(inner) = literal {
                        self.check_expr(inner, None)
                    } else {
                        self.ctx.emit_error(expr.span, "Cannot infer type for anonymous initialization `.{...}`.".into());
                        TypeId::ERROR
                    }
                }
            }
            ExprKind::EnumLiteral(variant_name) => {
                let mut res_ty = TypeId::ERROR;
                if let Some(exp_ty) = expected_ty {
                    let norm_exp = self.strip_mut(exp_ty);
                    if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_exp) {
                        if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                            if e.variants.iter().any(|v| v.name == *variant_name) {
                                res_ty = exp_ty; 
                            } else {
                                // ==========================================
                                // 探针：打印出到底哪里不匹配
                                // ==========================================
                                println!("--------------------------------------------------");
                                println!("🔥 ENUM MISMATCH DETECTED!");
                                println!("Target (Usage)   : SymbolId({:?}) -> \"{}\"", variant_name.0, self.ctx.resolve(*variant_name));
                                
                                for v in &e.variants {
                                    println!("Available (Def)  : SymbolId({:?}) -> \"{}\"", v.name.0, self.ctx.resolve(v.name));
                                }
                                println!("--------------------------------------------------");

                                let v_str = self.ctx.resolve(*variant_name);
                                self.ctx.emit_error(expr.span, format!("Variant `.{}` does not exist in the expected enum", v_str));
                            }
                        } else {
                            self.ctx.emit_error(expr.span, "Expected enum type for variant literal".into());
                        }
                    } else if norm_exp != TypeId::ERROR {
                        self.ctx.emit_error(expr.span, "Expected enum type for variant literal".into());
                    }
                } else {
                    self.ctx.emit_error(expr.span, "Cannot infer enum type for variant literal without context".into());
                }
                res_ty
            }
            ExprKind::Undef => {
                if expected_ty.is_none() {
                    self.ctx.emit_error(expr.span, "`undef` must have a known expected type".into());
                    TypeId::ERROR
                } else {
                    expected_ty.unwrap()
                }
            }

            // === 9. 控制流 ===
            ExprKind::Block { stmts, result } => {
                self.ctx.scopes.enter_scope();
                for stmt in stmts {
                    match &stmt.kind {
                        StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                            self.check_expr(e, None);
                        }
                    }
                }
                let ret_ty = if let Some(res) = result {
                    self.check_expr(res, expected_ty)
                } else {
                    TypeId::VOID
                };
                self.ctx.scopes.exit_scope();
                
                ret_ty
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
                self.check_coercion(cond.span, TypeId::BOOL, cond_ty);

                let then_ty = self.check_expr(then_branch, expected_ty);
                if let Some(else_expr) = else_branch {
                    let else_ty = self.check_expr(else_expr, expected_ty);
                    self.check_coercion(else_expr.span, then_ty, else_ty);
                    then_ty
                } else {
                    TypeId::VOID
                }
            }

            ExprKind::Switch { target, cases, default_case } => {
                let target_ty = self.check_expr(target, None);
                let mut common_ret_ty = expected_ty;

                for case in cases {
                    for pat in &case.patterns {
                        match pat {
                            ast::SwitchPattern::Value(v) => {
                                let v_ty = self.check_expr(v, Some(target_ty));
                                self.check_coercion(v.span, target_ty, v_ty);
                            }
                            ast::SwitchPattern::Range { start, end, .. } => {
                                let s_ty = self.check_expr(start, Some(target_ty));
                                let e_ty = self.check_expr(end, Some(target_ty));
                                self.check_coercion(start.span, target_ty, s_ty);
                                self.check_coercion(end.span, target_ty, e_ty);
                            }
                        }
                    }
                    let body_ty = self.check_expr(&case.body, common_ret_ty);
                    if common_ret_ty.is_none() {
                        common_ret_ty = Some(body_ty);
                    } else {
                        self.check_coercion(case.body.span, common_ret_ty.unwrap(), body_ty);
                    }
                }

                if let Some(def) = default_case {
                    let def_ty = self.check_expr(def, common_ret_ty);
                    if common_ret_ty.is_none() {
                        common_ret_ty = Some(def_ty);
                    } else {
                        self.check_coercion(def.span, common_ret_ty.unwrap(), def_ty);
                    }
                }
                self.check_switch_exhaustiveness(target_ty, cases, default_case.is_some(), expr.span);
                common_ret_ty.unwrap_or(TypeId::VOID)
            }

            ExprKind::For { init, cond, post, body } => {
                // 为 For 循环单独开辟作用域 (保护 init 里的 let)
                self.ctx.scopes.enter_scope();
                
                if let Some(i) = init { self.check_expr(i, None); }
                if let Some(c) = cond {
                    let c_ty = self.check_expr(c, Some(TypeId::BOOL));
                    self.check_coercion(c.span, TypeId::BOOL, c_ty);
                }
                if let Some(p) = post { self.check_expr(p, None); }
                
                self.check_expr(body, None);
                
                self.ctx.scopes.exit_scope();
                
                TypeId::VOID
            }

            ExprKind::Defer { expr: defer_expr } => {
                self.check_expr(defer_expr, None);
                TypeId::VOID
            }
            ExprKind::Break | ExprKind::Continue => TypeId::VOID,
            ExprKind::Return(val) => {
                self.has_returned = true;
                let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);
                
                if let Some(v) = val {
                    // 使用函数级别的 expected_ret 进行推导和强制转换检查
                    let v_ty = self.check_expr(v, Some(expected_ret));
                    self.check_coercion(v.span, expected_ret, v_ty);
                } else {
                    // 如果写了空的 return; 但函数不是返回 void
                    if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
                        self.ctx.emit_error(expr.span, "Expected a return value, but found empty return".into());
                    }
                }
                TypeId::VOID
            }
            
            ExprKind::GenericInstantiation { target, types } => {
                let target_ty = self.check_expr(target, None);
                let target_norm = self.strip_mut(target_ty);

                if target_norm == TypeId::ERROR { return TypeId::ERROR; }

                // 1. 解析传入的泛型实参 (e.g., [i32, u8])
                let mut arg_tys = Vec::new();
                {
                    let mut resolver = TypeResolver::new(self.ctx);
                    let scope = resolver.ctx.scopes.current_scope_id().unwrap();
                    for ty_node in types {
                        arg_tys.push(resolver.resolve_type(ty_node, scope));
                    }
                }

                // 2. 提取类型的真实身份 (DefId 和 旧的参数)
                let (def_id, _old_args) = match self.ctx.type_registry.get(target_norm) {
                    TypeKind::FnDef(id, args) => (*id, args.clone()),
                    TypeKind::Def(id, args) => (*id, args.clone()),
                    _ => {
                        self.ctx.emit_error(expr.span, "This expression does not support generic instantiation".into());
                        return TypeId::ERROR;
                    }
                };

                // 3. 校验泛型参数数量
                let generics = {
                    let def = &self.ctx.defs[def_id.0 as usize];
                    match def {
                        Def::Function(f) => f.generics.clone(),
                        Def::Struct(s) => s.generics.clone(),
                        Def::Union(u) => u.generics.clone(),
                        Def::Enum(e) => e.generics.clone(),
                        Def::TypeAlias(t) => t.generics.clone(),
                        _ => unreachable!(),
                    }
                };

                if generics.len() != arg_tys.len() {
                    self.ctx.emit_error(expr.span, format!("Expected {} generic arguments, but {} were provided", generics.len(), arg_tys.len()));
                    return TypeId::ERROR;
                }

                // 4. 执行类型约束边界检查
                self.check_generic_bounds(expr.span, &generics, &arg_tys);

                // 5. 生成全新的、包含具体实参的类型项
                if matches!(self.ctx.type_registry.get(target_norm), TypeKind::FnDef(..)) {
                    self.ctx.type_registry.intern(TypeKind::FnDef(def_id, arg_tys))
                } else {
                    self.ctx.type_registry.intern(TypeKind::Def(def_id, arg_tys))
                }
            }
        };

        // === 调试探针 ===
        if ty == TypeId::ERROR {
            println!("--------------------------------------------------");
            println!("🚨 [SEMA TRAP] Expression evaluated to ERROR!");
            println!("Span: {:?}", expr.span); 
            println!("ExprKind: {:#?}", expr.kind); 
            println!("--------------------------------------------------");
        }
        self.ctx.node_types.insert(expr.id, ty);
        ty
    }

    /// 检查一组提供的实际类型参数是否满足泛型声明中的 Trait 约束
    fn check_generic_bounds(&mut self, span: Span, generics: &[ast::GenericParam], arg_tys: &[TypeId]) {
        for (i, param) in generics.iter().enumerate() {
            if i >= arg_tys.len() { break; }
            let act_ty = arg_tys[i];
            
            // 遍历该泛型参数身上的所有约束 (如 [T: Reader + Writer])
            for constraint_node in &param.constraints {
                let constraint_ty = self.ctx.node_types.get(&constraint_node.id).copied().unwrap_or(TypeId::ERROR);
                
                if constraint_ty != TypeId::ERROR {
                    if !self.check_trait_impl(act_ty, constraint_ty) {
                        let param_name = self.ctx.resolve(param.name);
                        self.ctx.emit_error(span, format!("Type does not satisfy trait bounds for generic parameter `{}`", param_name));
                    }
                }
            }
        }
    }

    // ==========================================
    //          Core Operations
    // ==========================================

    fn check_binary(&mut self, lhs: &Expr, op: BinaryOperator, rhs: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let lhs_ty = self.check_expr(lhs, expected_ty);
        let rhs_ty = self.check_expr(rhs, Some(lhs_ty)); 

        let l_norm = self.strip_mut(lhs_ty);
        let r_norm = self.strip_mut(rhs_ty);

        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR { return TypeId::ERROR; }

        let is_l_ptr = matches!(self.ctx.type_registry.get(l_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let is_r_ptr = matches!(self.ctx.type_registry.get(r_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));

        use BinaryOperator::*;
        match op {
            Add | Subtract | Multiply | Divide | Modulo => {
                if is_l_ptr || is_r_ptr {
                    self.ctx.emit_error(lhs.span, "Implicit pointer arithmetic is strictly forbidden. Use explicit `as usize` casts.".into());
                    return TypeId::ERROR;
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) { return TypeId::ERROR; }
                l_norm
            }
            Equal | NotEqual | LessThan | GreaterThan | LessOrEqual | GreaterOrEqual => {
                if !self.check_coercion(rhs.span, l_norm, r_norm) { return TypeId::ERROR; }
                TypeId::BOOL
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs.span, TypeId::BOOL, l_norm);
                self.check_coercion(rhs.span, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            _ => { // Bitwise Ops
                if !self.ctx.type_registry.is_integer(l_norm) {
                    self.ctx.emit_error(lhs.span, "Bitwise operations require integer types".into());
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) { return TypeId::ERROR; }
                l_norm
            }
        }
    }

    fn check_unary(&mut self, op: UnaryOperator, operand: &Expr, span: Span, expected_ty: Option<TypeId>) -> TypeId {
        // 根据操作符智能传递期望类型
        let inner_expected = match op {
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => expected_ty,
            UnaryOperator::AddressOf => {
                if let Some(exp) = expected_ty {
                    let norm = self.strip_mut(exp);
                    if let TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) = self.ctx.type_registry.get(norm) {
                        Some(*inner)
                    } else { None }
                } else { None }
            }
            _ => None,
        };

        let op_ty = self.check_expr(operand, inner_expected);
        if op_ty == TypeId::ERROR { return TypeId::ERROR; }

        match op {
            UnaryOperator::AddressOf => self.ctx.type_registry.intern(TypeKind::Pointer(op_ty)),
            UnaryOperator::PointerDeRef => {
                let norm = self.strip_mut(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => *inner,
                    _ => {
                        self.ctx.emit_error(span, "Cannot dereference a non-pointer type".into());
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::LengthOf => {
                let norm = self.strip_mut(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Array { .. } | TypeKind::Slice(_) => TypeId::USIZE,
                    _ => {
                        self.ctx.emit_error(span, "Length operator `#` can only be applied to arrays and slices".into());
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::Negate => {
                if !self.ctx.type_registry.is_integer(self.strip_mut(op_ty)) && !self.ctx.type_registry.is_float(self.strip_mut(op_ty)) {
                    self.ctx.emit_error(span, "Negation requires a numeric type".into());
                }
                op_ty
            }
            UnaryOperator::LogicalNot => {
                self.check_coercion(span, TypeId::BOOL, op_ty);
                TypeId::BOOL
            }
            UnaryOperator::BitwiseNot => {
                if !self.ctx.type_registry.is_integer(self.strip_mut(op_ty)) {
                    self.ctx.emit_error(span, "Bitwise NOT requires an integer type".into());
                }
                op_ty
            }
            _ => TypeId::ERROR,
        }
    }

    fn check_assign(&mut self, lhs: &Expr, _op: AssignmentOperator, rhs: &Expr, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        
        if !self.is_mut_type(lhs_ty) && lhs_ty != TypeId::ERROR {
            self.ctx.emit_error(lhs.span, "Cannot assign to an immutable variable/location. Use `mut` type modifier.".into());
        }

        let l_norm = self.strip_mut(lhs_ty); // 剥离 mut
        
        let rhs_ty = self.check_expr(rhs, Some(l_norm));

        if lhs_ty == TypeId::ERROR || rhs_ty == TypeId::ERROR { return TypeId::ERROR; }
        
        self.check_coercion(span, l_norm, self.strip_mut(rhs_ty));
        TypeId::VOID
    }

    fn check_field_access(&mut self, lhs: &Expr, field: crate::utils::SymbolId, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR { return TypeId::ERROR; }
        let mut base_ty = lhs_ty;
        let mut is_target_mut = false;

        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Mut(inner) => {
                    is_target_mut = true;
                    base_ty = *inner;
                }
                TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                    is_target_mut = false; 
                    base_ty = *inner;
                }
                _ => break,
            }
        }

        let current_norm = self.ctx.type_registry.normalize(base_ty);

        // 1. 优先处理 Trait Object 的动态派发调用
        if let TypeKind::TraitObject(trait_def_id, trait_args) = self.ctx.type_registry.get(current_norm).clone() {
            if let Def::Trait(trait_def) = &self.ctx.defs[trait_def_id.0 as usize] {
                if let Some(method_ast) = trait_def.methods.iter().find(|m| m.name == field) {
                    let mut method_ty = self.ctx.node_types.get(&method_ast.type_node.id).copied().unwrap_or(TypeId::ERROR);
                    
                    if !trait_def.generics.is_empty() && !trait_args.is_empty() {
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in trait_def.generics.iter().enumerate() {
                            map.insert(param.name, trait_args[i]);
                        }
                        let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                        method_ty = subst.substitute(method_ty);
                    }
                    return method_ty;
                }
            }
            
            let field_str = self.ctx.resolve(field);
            self.ctx.emit_error(span, format!("Method `{}` not found in this trait object", field_str));
            return TypeId::ERROR;
        }

        // 2. 提取普通类型的 DefId (Struct/Union/Enum)
        let (def_id, generic_args) = match self.ctx.type_registry.get(current_norm) {
            TypeKind::Def(id, args) => (*id, args.clone()),
            _ => {
                self.ctx.emit_error(span, "Type does not support field or method access".into());
                return TypeId::ERROR;
            }
        };

        let def = self.ctx.defs[def_id.0 as usize].clone();

        // 3. 查找结构体/联合体的字段
        match &def {
            Def::Struct(s) => {
                if let Some(f) = s.fields.iter().find(|f| f.name == field) {
                    let mut field_ty = self.ctx.node_types.get(&f.type_node.id).copied().unwrap_or(TypeId::ERROR);
                    if !s.generics.is_empty() && !generic_args.is_empty() {
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in s.generics.iter().enumerate() {
                            map.insert(param.name, generic_args[i]);
                        }
                        let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                        field_ty = subst.substitute(field_ty);
                    }
                    if is_target_mut {
                        field_ty = self.ctx.type_registry.intern(TypeKind::Mut(field_ty));
                    }
                    return field_ty;
                }
            }
            Def::Union(u) => {
                if let Some(f) = u.fields.iter().find(|f| f.name == field) {
                    let mut field_ty = self.ctx.node_types.get(&f.type_node.id).copied().unwrap_or(TypeId::ERROR);
                    if !u.generics.is_empty() && !generic_args.is_empty() {
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in u.generics.iter().enumerate() {
                            map.insert(param.name, generic_args[i]);
                        }
                        let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                        field_ty = subst.substitute(field_ty);
                    }
                    if is_target_mut {
                        field_ty = self.ctx.type_registry.intern(TypeKind::Mut(field_ty));
                    }
                    return field_ty;
                }
            }
            Def::Enum(e) => {
                if e.variants.iter().any(|v| v.name == field) {
                    // 枚举变体的类型，也就是这个枚举本身
                    return current_norm; 
                }
            }
            _ => {}
        }

        // 4. 查找方法 (在 Impl 块中)
        let mut found_method_id = None;
        let mut found_impl_def = None;
        let mut resolved_impl_args = Vec::new(); 

        for global_def in &self.ctx.defs {
            if let Def::Impl(impl_def) = global_def {
                let impl_target_ty = self.ctx.node_types.get(&impl_def.target_type.id).copied().unwrap_or(TypeId::ERROR);
                
                let mut map = std::collections::HashMap::new();
                if self.unify(impl_target_ty, lhs_ty, &mut map) {
                    // 按 Impl 声明的泛型顺序，组装实参
                    for param in &impl_def.generics {
                        resolved_impl_args.push(map.get(&param.name).copied().unwrap_or(TypeId::ERROR));
                    }
                    // 既然类型统一成功，说明这个 Impl 块绝对属于当前类型，开始找方法
                    for &method_id in &impl_def.methods {
                        if let Def::Function(func_def) = &self.ctx.defs[method_id.0 as usize] {
                            if func_def.name == field {
                                found_method_id = Some(method_id);
                                found_impl_def = Some(impl_def.clone());
                                break;
                            }
                        }
                    }
                }
            }
            if found_method_id.is_some() { break; }
        }

        if let Some(method_id) = found_method_id {
            return self.ctx.type_registry.intern(TypeKind::FnDef(method_id, resolved_impl_args));
        }

        let field_str = self.ctx.resolve(field);
        self.ctx.emit_error(span, format!("No field or method named `{}` found on this type", field_str));
        TypeId::ERROR
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        let callee_ty = self.check_expr(callee, None);
        let norm_callee = self.strip_mut(callee_ty);

        if norm_callee == TypeId::ERROR { return TypeId::ERROR; }

        // === 核心逻辑：提取或计算最终的函数签名 ===
        let sig_ty = if let TypeKind::FnDef(def_id, generic_args) = self.ctx.type_registry.get(norm_callee).clone() {
            // 如果是一个确切的函数定义，我们需要将其泛型参数代入签名
            let f = match &self.ctx.defs[def_id.0 as usize] {
                Def::Function(func) => func,
                _ => unreachable!(),
            };
            
            let raw_sig = f.resolved_sig.expect("Function signature should be resolved by TypeResolver");
            
            // 构造泛型映射表
            let mut map = std::collections::HashMap::new();
            for (i, param) in f.generics.iter().enumerate() {
                map.insert(param.name, generic_args[i]);
            }
            
            // 生成代换后的具体签名
            let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
            subst.substitute(raw_sig)
        } else {
            // 如果只是一个普通的函数指针，直接使用
            norm_callee
        };

        // === 校验参数 ===
        if let TypeKind::Function { params, ret, is_variadic } = self.ctx.type_registry.get(sig_ty).clone() {
            // 识别方法调用并提取 Receiver 类型
            let mut is_method = false;
            let mut receiver_ty = TypeId::ERROR;
            // 仅当左侧是一个实例（即不是纯粹的模块路径时），才把它当成 Method 的 Receiver
            if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
                let callee_node_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
                // 如果它是个 FnDef，说明它是从 Impl 块或全局函数里捞出来的
                // 如果它是个 Function，说明它是从 TraitObject 的虚表签名里捞出来的
                if matches!(self.ctx.type_registry.get(self.ctx.type_registry.normalize(callee_node_ty)), TypeKind::FnDef(..) | TypeKind::Function {..}) {
                    is_method = true;
                    receiver_ty = self.ctx.node_types.get(&lhs.id).copied().unwrap_or(TypeId::ERROR);
                }
            }

            // 计算用户需要填写的实际参数数量
            let expected_arg_count = if is_method { params.len().saturating_sub(1) } else { params.len() };
            if is_variadic {
                if args.len() < expected_arg_count {
                    self.ctx.emit_error(span, format!("Function expects at least {} arguments, but {} were provided", expected_arg_count, args.len()));
                }
            } else {
                if args.len() != expected_arg_count {
                    self.ctx.emit_error(span, format!("Function expects exactly {} arguments, but {} were provided", expected_arg_count, args.len()));
                }
            }

            if is_method && !params.is_empty() {
                // 对隐式的 self 进行强制类型转换检查
                self.check_coercion(callee.span, params[0], receiver_ty);
            }

            // 如果是方法调用，用户传入的第 0 个参数，对应签名里的第 1 个参数
            let param_offset = if is_method { 1 } else { 0 };

            for (i, arg) in args.iter().enumerate() {
                let sig_param_idx = i + param_offset; // 计算出在签名 `params` 数组中的实际位置

                if sig_param_idx < params.len() {
                    let arg_ty = self.check_expr(arg, Some(params[sig_param_idx]));
                    self.check_coercion(arg.span, params[sig_param_idx], arg_ty);
                } else {
                    // === Variadic Args 处理保持不变 ===
                    let arg_ty = self.check_expr(arg, None); 
                    let norm_arg = self.strip_mut(arg_ty);

                    if norm_arg == TypeId::ERROR { continue; }

                    let is_small_int = norm_arg == TypeId::I8 || norm_arg == TypeId::I16 
                                    || norm_arg == TypeId::U8 || norm_arg == TypeId::U16;
                    
                    if is_small_int {
                        self.ctx.emit_error(
                            arg.span, 
                            "C ABI requires integer arguments passed to `...` to be at least 32-bit. Please cast it explicitly (e.g., `as i32`).".into()
                        );
                    } else if norm_arg == TypeId::F32 {
                        self.ctx.emit_error(
                            arg.span, 
                            "C ABI requires float arguments passed to `...` to be 64-bit. Please cast it explicitly (e.g., `as f64`).".into()
                        );
                    }
                }
            }
            return ret;
        }

        self.ctx.emit_error(callee.span, "Expression is not callable".into());
        TypeId::ERROR
    }

    fn check_data_literal(&mut self, kind: &ast::DataLiteralKind, expected: TypeId, span: Span) -> TypeId {
        let exp_norm = self.strip_mut(expected);
        
        match kind {
            ast::DataLiteralKind::Array(elems) => {
                if let TypeKind::Array { elem: exp_elem, len } = self.ctx.type_registry.get(exp_norm) {
                    let exp_elem_ty = *exp_elem;
                    if elems.len() as u64 != *len {
                        self.ctx.emit_error(span, format!("Array literal length ({}) does not match expected length ({})", elems.len(), len));
                    }
                    for e in elems {
                        let act_ty = self.check_expr(e, Some(exp_elem_ty));
                        self.check_coercion(e.span, exp_elem_ty, act_ty);
                    }
                    expected
                } else {
                    self.ctx.emit_error(span, "Expected an array type for array literal `.{ ... }`".into());
                    TypeId::ERROR
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                if let TypeKind::Array { elem: exp_elem, .. } = self.ctx.type_registry.get(exp_norm) {
                    let exp_elem_ty = *exp_elem;
                    let val_ty = self.check_expr(value, Some(exp_elem_ty));
                    self.check_coercion(value.span, exp_elem_ty, val_ty);
                    
                    let c_ty = self.check_expr(count, Some(TypeId::USIZE));
                    if !self.ctx.type_registry.is_integer(self.strip_mut(c_ty)) {
                        self.ctx.emit_error(count.span, "Repeat count must be an integer".into());
                    }
                    expected
                } else {
                    self.ctx.emit_error(span, "Expected an array type for repeat literal `.{ v; N }`".into());
                    TypeId::ERROR
                }
            }
            ast::DataLiteralKind::Struct(init_fields) => {
                let mut struct_fields = Vec::new();
                let mut struct_name = String::new();

                if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(exp_norm) {
                    if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                        struct_fields = s.fields.clone();
                        struct_name = self.ctx.resolve(s.name).to_string();
                    } else {
                        self.ctx.emit_error(span, "Expected a struct type for struct literal".into());
                        return TypeId::ERROR;
                    }
                } else {
                    self.ctx.emit_error(span, "Expected a struct type for struct literal".into());
                    return TypeId::ERROR;
                }

                let mut initialized = std::collections::HashSet::new();

                for init_f in init_fields {
                    if let Some(def_f) = struct_fields.iter().find(|f| f.name == init_f.name) {
                        let f_ty = self.ctx.node_types.get(&def_f.type_node.id).copied().unwrap_or(TypeId::ERROR);
                        let val_ty = self.check_expr(&init_f.value, Some(f_ty)); 
                        self.check_coercion(init_f.span, f_ty, val_ty);
                        
                        initialized.insert(init_f.name);
                    } else {
                        let name_str = self.ctx.resolve(init_f.name);
                        self.ctx.emit_error(init_f.span, format!("Field `{}` does not exist in struct `{}`", name_str, struct_name));
                    }
                }

                // 严格检查未初始化的字段
                for def_f in &struct_fields {
                    if !initialized.contains(&def_f.name) && def_f.default_value.is_none() {
                        let name_str = self.ctx.resolve(def_f.name);
                        self.ctx.emit_error(span, format!("Field `{}` is missing and has no default value. Use `undef` if intentional.", name_str));
                    }
                }
                
                return expected;
            }
            ast::DataLiteralKind::Scalar(inner) => {
                let inner_ty = self.check_expr(inner, Some(expected));
                self.check_coercion(inner.span, expected, inner_ty);
                expected
            }
        }
    }

    // ==========================================
    //          Type System Helpers
    // ==========================================

    pub fn strip_mut(&self, ty: TypeId) -> TypeId {
        let norm = self.ctx.type_registry.normalize(ty);
        if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm) {
            *inner
        } else {
            norm
        }
    }

    pub fn is_mut_type(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        matches!(self.ctx.type_registry.get(norm), TypeKind::Mut(_))
    }

    pub fn check_coercion(&mut self, span: Span, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.strip_mut(expected);
        let act = self.strip_mut(actual);

        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR {
            return true;
        }

        // [N]T -> []T (数组衰减为切片)
        if let TypeKind::Slice(exp_elem) = self.ctx.type_registry.get(exp) {
            if let TypeKind::Array { elem: act_elem, .. } = self.ctx.type_registry.get(act) {
                let exp_is_mut = self.is_mut_type(*exp_elem);
                let act_is_mut = self.is_mut_type(*act_elem);
                
                if exp_is_mut && !act_is_mut {
                    self.ctx.emit_error(span, "Cannot implicitly convert an immutable array to a mutable slice `[]mut T`".into());
                    return false;
                }
                
                let exp_base = self.strip_mut(*exp_elem);
                let act_base = self.strip_mut(*act_elem);
                if exp_base == act_base {
                    return true;
                }
            }
        }

        self.ctx.emit_error(span, format!("Type mismatch. Expected {}, found {}", exp.0, act.0));
        false
    }

    fn check_cast(&mut self, span: Span, from: TypeId, to: TypeId) {
        if from == to || from == TypeId::ERROR || to == TypeId::ERROR { return; }

        let f_norm = self.strip_mut(from);
        let t_norm = self.strip_mut(to);

        // === 1. Trait Object 强转 (Fat Pointer 生成) ===
        if let TypeKind::TraitObject(to_def_id, _) = self.ctx.type_registry.get(t_norm) {    
            // a. 校验可变性安全 (不能把只读指针转为 mut TraitObject)
            let is_to_mut = self.is_mut_type(to);
            let is_from_mut_ptr = self.is_mutable_pointer(from);

            if is_to_mut && !is_from_mut_ptr {
                self.ctx.emit_error(span, "Cannot cast a read-only pointer to a mutable trait object `mut Trait`".into());
                return;
            }

            // b. 校验来源是否是指针 (Kern 规范：转为 trait object 的实现方必须显式是指针)
            // TODO
            let is_from_ptr = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
            let is_from_trait_obj = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Def(id, _) if matches!(&self.ctx.defs[id.0 as usize], Def::Trait(_)));

            if !is_from_ptr && !is_from_trait_obj {
                self.ctx.emit_error(span, "Only pointers (or other trait objects) can be cast to a trait object".into());
                return;
            }
            
            let sym = self.ctx.defs[to_def_id.0 as usize].name().unwrap();
            // c. 校验是否真正实现了该 Trait
            if !self.check_trait_impl(from, t_norm) { 
                let trait_name = self.ctx.resolve(sym);
                self.ctx.emit_error(span, format!("The source type does not implement trait `{}`", trait_name));
            }
            
            return; // 成功转为胖指针
        }

        // === 2. 常规的 Bit-Pattern 强转 (数值、普通指针互转) ===
        let is_f_int = self.ctx.type_registry.is_integer(f_norm);
        let is_t_int = self.ctx.type_registry.is_integer(t_norm);
        let is_f_ptr = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let is_t_ptr = matches!(self.ctx.type_registry.get(t_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let is_f_slice = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Slice(_));
        
        if is_f_slice && is_t_ptr { return; }
        if (is_f_int && is_t_int) || (is_f_ptr && is_t_ptr) || (is_f_int && is_t_ptr) || (is_f_ptr && is_t_int) {
            return;
        }

        self.ctx.emit_error(span, "Invalid `as` cast. `as` only supports bit-pattern preservation (e.g., int to ptr) or Trait Object construction.".into());
    }

    /// 检查一个类型是否是可变数据的指针，例如 `*mut File` (即 Pointer(Mut(File)))
    fn is_mutable_pointer(&self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm) {
            TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                self.is_mut_type(*inner)
            }
            _ => false
        }
    }

   /// 检查具体类型是否实现了指定的 Trait (支持泛型 Impl 和 Supertraits 继承)
    fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let mut visited = std::collections::HashSet::new();
        self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited)
    }

    fn check_trait_impl_inner(&mut self, source_ty: TypeId, target_trait_ty: TypeId, visited: &mut std::collections::HashSet<crate::sema::ty::DefId>) -> bool {
        let mut impl_blocks = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                impl_blocks.push(impl_def.clone());
            }
        }

        for impl_def in impl_blocks {
            if let Some(trait_ast) = &impl_def.trait_type {
                // 直接查表，不借用 Resolver
                let impl_target_ty = self.ctx.node_types.get(&impl_def.target_type.id).copied().unwrap_or(TypeId::ERROR);
                let impl_trait_ty = self.ctx.node_types.get(&trait_ast.id).copied().unwrap_or(TypeId::ERROR);

                if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR { continue; }

                let mut map = std::collections::HashMap::new();

               // 核心：使用 unify 匹配 target，推导出 T
                if self.unify(impl_target_ty, source_ty, &mut map) {
                    
                    // 利用 `{}` 隔离 Substituter 的生命周期
                    let instantiated_trait_ty = {
                        let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                        subst.substitute(impl_trait_ty)
                    }; 

                    // 比较前必须剥离 Mut
                    let inst_norm = self.strip_mut(instantiated_trait_ty);
                    let target_norm = self.strip_mut(target_trait_ty);

                    if inst_norm == target_norm {
                        return true;
                    }

                    // 1. 直接匹配成功
                    if instantiated_trait_ty == target_trait_ty {
                        return true;
                    }

                    // 2. 检查 Supertraits (特征继承)
                    let inst_norm = self.strip_mut(instantiated_trait_ty);
                    if let TypeKind::Def(inst_def_id, _) = self.ctx.type_registry.get(inst_norm) {
                        // 防止特征循环依赖导致死循环
                        if visited.insert(*inst_def_id) {
                            if let Def::Trait(trait_def) = self.ctx.defs[inst_def_id.0 as usize].clone() {
                                // 遍历它的所有父特征 (如 Reader + Writer)
                                for supertrait_ast in &trait_def.supertraits {
                                    let super_ty = self.ctx.node_types.get(&supertrait_ast.id).copied().unwrap_or(TypeId::ERROR);
                                    
                                    // 再次用 `{}` 隔离生命周期，执行替换
                                    let inst_super_ty = {
                                        let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                        subst.substitute(super_ty)
                                    };
                                    
                                    if inst_super_ty == target_trait_ty {
                                        return true;
                                    }
                                    if self.check_trait_impl_inner(source_ty, inst_super_ty, visited) {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn check_switch_exhaustiveness(&mut self, target_ty: TypeId, cases: &[ast::SwitchCase], has_default: bool, span: Span) {
        if has_default {
            // 如果写了 else =>，无论什么类型都算作 Exhaustive
            return;
        }

        let norm_target = self.strip_mut(target_ty);
        
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_target) {
            if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                // 收集 enum 定义的所有 variant 名字
                let mut unhandled_variants: std::collections::HashSet<crate::utils::SymbolId> = e.variants.iter().map(|v| v.name).collect();

                // 遍历用户写的 case，划掉已处理的
                for case in cases {
                    for pat in &case.patterns {
                        if let ast::SwitchPattern::Value(v_expr) = pat {
                            if let ExprKind::EnumLiteral(name) = &v_expr.kind {
                                unhandled_variants.remove(name);
                            }
                        }
                    }
                }

                if !unhandled_variants.is_empty() {
                    let missing: Vec<String> = unhandled_variants.into_iter().map(|id| self.ctx.resolve(id).to_string()).collect();
                    self.ctx.emit_error(span, format!("Switch is not exhaustive. Missing enum variants: {}", missing.join(", ")));
                }
                return;
            }
        }

        // 如果不是 Enum，且没有写 else =>，一律报错
        self.ctx.emit_error(span, "Switch expression must be exhaustive. Consider adding an `else =>` branch.".into());
    }

    /// 模式匹配/统一引擎：将带有 Param 的泛型类型与具体类型进行匹配，提取泛型实参
    /// 例如：generic_ty = `*mut List[T]`, concrete_ty = `*mut List[i32]`
    /// 结果：返回 true，并在 map 中插入 `T` -> `i32`
    fn unify(&self, generic_ty: TypeId, concrete_ty: TypeId, map: &mut std::collections::HashMap<crate::utils::SymbolId, TypeId>) -> bool {
        let gen_norm = self.strip_mut(generic_ty);
        let con_norm = self.strip_mut(concrete_ty);
        
        let gen_kind = self.ctx.type_registry.get(gen_norm).clone();
        let con_kind = self.ctx.type_registry.get(con_norm).clone();

        match (gen_kind, con_kind) {
            // 命中泛型参数，记录映射
            (TypeKind::Param(name), _) => {
                // 如果同一个参数 T 出现多次，确保它们匹配的是同一个类型
                if let Some(&existing_ty) = map.get(&name) {
                    existing_ty == concrete_ty
                } else {
                    map.insert(name, concrete_ty);
                    true
                }
            }
            // 剥开指针继续匹配
            (TypeKind::Pointer(g), TypeKind::Pointer(c)) => self.unify(g, c, map),
            (TypeKind::VolatilePtr(g), TypeKind::VolatilePtr(c)) => self.unify(g, c, map),
            (TypeKind::Mut(g), TypeKind::Mut(c)) => self.unify(g, c, map),
            (TypeKind::Slice(g), TypeKind::Slice(c)) => self.unify(g, c, map),
            (TypeKind::Array { elem: ge, len: gl }, TypeKind::Array { elem: ce, len: cl }) => {
                gl == cl && self.unify(ge, ce, map)
            }
            // 剥开 Def 继续匹配泛型参数
            (TypeKind::Def(g_id, g_args), TypeKind::Def(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() { return false; }
                for (ga, ca) in g_args.iter().zip(c_args.iter()) {
                    if !self.unify(*ga, *ca, map) { return false; }
                }
                true
            }
            (TypeKind::TraitObject(g_id, g_args), TypeKind::TraitObject(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() { return false; }
                for (ga, ca) in g_args.iter().zip(c_args.iter()) {
                    if !self.unify(*ga, *ca, map) { return false; }
                }
                true
            }
            // 其他情况（如 Primitive）必须绝对相等
            _ => gen_norm == con_norm,
        }
    }
}