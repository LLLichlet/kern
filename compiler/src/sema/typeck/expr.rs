use crate::driver::Context;
use crate::parser::ast::{self, BinaryOperator, Expr, ExprKind, StmtKind, UnaryOperator};
use crate::sema::def::Def;
use crate::sema::resolve_types::TypeResolver;
use crate::sema::typeck::subst::Substituter;
use crate::sema::scope::SymbolInfo;
use crate::sema::scope::SymbolKind;
use crate::sema::ty::{TypeId, TypeKind};
use crate::utils::Span;

use std::collections::HashMap;

pub struct ExprChecker<'a> {
    pub ctx: &'a mut Context,
    pub current_return_type: Option<TypeId>,
    pub has_returned: bool,
    pub type_vars: Vec<Option<TypeId>>,
}

impl<'a> ExprChecker<'a> {
    pub fn new(ctx: &'a mut Context, current_return_type: Option<TypeId>) -> Self {
        Self {
            ctx,
            current_return_type,
            has_returned: false,
            type_vars: Vec::new(),
        }
    }

    /// 创建一个新的未知类型变量 `?T`
    pub fn new_type_var(&mut self) -> TypeId {
        let vid = self.type_vars.len() as u32;
        self.type_vars.push(None);
        self.ctx.type_registry.intern(TypeKind::TypeVar(vid))
    }

    /// 核心入口：检查表达式类型
    pub fn check_expr(&mut self, expr: &Expr, expected_ty: Option<TypeId>) -> TypeId {
        let ty = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(_) => expected_ty.unwrap_or_else(|| self.new_type_var()),
            ExprKind::Float(_) => expected_ty.unwrap_or_else(|| self.new_type_var()),
            ExprKind::Bool(_) => TypeId::BOOL,
            ExprKind::Char(_) => TypeId::U32,
            ExprKind::String(_) => self.ctx.type_registry.intern(TypeKind::Slice {
                is_mut: false,
                elem: TypeId::U8,
            }),

            // === 2. 标识符与变量 ===
            ExprKind::Identifier(name) => self.check_identifier(*name, expr.span),
            ExprKind::SelfValue => self.check_self_value(expr.span),

            // === 3. 声明与绑定 ===
            ExprKind::Let { pattern, init, .. } => {
                self.check_let_or_static(expr.id, pattern, init, expected_ty, false, expr.span)
            }
            ExprKind::Static { pattern, init, .. } => {
                self.check_let_or_static(expr.id, pattern, init, expected_ty, true, expr.span)
            }

            // === 4. 运算与赋值 ===
            ExprKind::Binary { lhs, op, rhs } => self.check_binary(lhs, *op, rhs, expected_ty),
            ExprKind::Unary { op, operand } => {
                self.check_unary(*op, operand, expr.span, expected_ty)
            }
            ExprKind::Assign { lhs, rhs, .. } => self.check_assign(lhs, rhs, expr.span),

            // === 5. 转换 ===
            ExprKind::As { lhs, target } => self.check_as_expr(lhs, target),

            // === 6. 内存访问 (索引, 字段, 切片) ===
            ExprKind::IndexAccess { lhs, index, is_mut } => {
                self.check_index_access(lhs, index, *is_mut, expr.span)
            }
            ExprKind::FieldAccess { lhs, field } => self.check_field_access(lhs, *field, expr.span),
            ExprKind::SliceOp {
                lhs, start, end, is_inclusive, is_mut,
            } => self.check_slice_op(
                lhs, start.as_deref(), end.as_deref(), *is_inclusive, *is_mut, expr.span,
            ),

            // === 7. 函数/宏调用 ===
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),

            // === 8. 复杂字面量 ===
            ExprKind::DataInit { type_node, literal } => {
                self.check_data_init_expr(type_node.as_deref(), literal, expected_ty, expr.span)
            }
            ExprKind::EnumLiteral(variant_name) => {
                self.check_enum_literal(*variant_name, expected_ty, expr.span)
            }
            ExprKind::Undef => self.check_undef(expected_ty, expr.span),

            // === 9. 控制流 ===
            ExprKind::Block { stmts, result } => {
                self.check_block(stmts, result.as_deref(), expected_ty)
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.check_if(cond, then_branch, else_branch.as_deref(), expected_ty),
            ExprKind::Switch {
                target,
                cases,
                default_case,
            } => self.check_switch_expr(
                target,
                cases,
                default_case.as_deref(),
                expected_ty,
                expr.span,
            ),
            ExprKind::Match { target, arms } => {
                self.check_match_expr(target, arms, expected_ty, expr.span)
            }
            ExprKind::For {
                init,
                cond,
                post,
                body,
            } => self.check_for(init.as_deref(), cond.as_deref(), post.as_deref(), body),
            ExprKind::Defer { expr: defer_expr } => self.check_defer(defer_expr),
            // Break, Continue, Return 会导致控制流发散，类型应为 NEVER
            ExprKind::Break | ExprKind::Continue => TypeId::NEVER,
            ExprKind::Return(val) => {
                self.check_return(val.as_deref(), expr.span);
                TypeId::NEVER
            }

            // === 10. 泛型实例化 ===
            ExprKind::GenericInstantiation { target, types } => {
                self.check_generic_instantiation(target, types, expr.span)
            }

            ExprKind::Lambda {
                params,
                ret_type,
                body,
            } => self.check_lambda(params, ret_type, body),

            ExprKind::Infer => {
                self.ctx.struct_error(expr.span, "type placeholder `_` cannot be evaluated as an expression")
                    .with_hint("in Kern, `_` is only used as a discard binding (`let _ =`) or in array length inference (`[_]T`)")
                    .emit();
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(expr.id, ty);
        ty
    }

    /// 检查一个独立执行的表达式，其返回值是否被非法隐式丢弃
    fn check_discarded_expr(&mut self, expr: &Expr) {
        let ty = self.check_expr(expr, None);
        let norm_ty = self.resolve_tv(ty);

        // 如果既不是 void，也不是发散的 never，更不是已经报错的 error，那就是非法丢弃
        if norm_ty != TypeId::VOID && norm_ty != TypeId::NEVER && norm_ty != TypeId::ERROR {
            let ty_str = self.ctx.ty_to_string(ty);
            self.ctx
                .struct_error(expr.span, "ignored non-void return value")
                .with_hint(format!(
                    "expression evaluates to `{}`, which must be explicitly used or discarded",
                    ty_str
                ))
                .with_hint("in Kern, use `let _ = ...;` to explicitly discard the value")
                .emit();
        }
    }

    fn check_identifier(&mut self, name: crate::utils::SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name) {
            if info.kind == SymbolKind::Function {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(info.def_id.unwrap(), vec![]));
            }
            info.type_id
        } else {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(span, format!("use of undeclared identifier `{}`", name_str))
                .with_hint("make sure the variable or function is defined before using it")
                .emit();
            TypeId::ERROR
        }
    }

    fn check_self_value(&mut self, span: Span) -> TypeId {
        let self_var = self.ctx.intern("self");
        let self_type = self.ctx.intern("Self");

        if let Some(info) = self.ctx.scopes.resolve(self_var) {
            info.type_id
        } else if let Some(info) = self.ctx.scopes.resolve(self_type) {
            info.type_id
        } else {
            self.ctx
                .struct_error(span, "`self` is not available in this context")
                .with_hint("the `self` keyword is only valid inside method implementations")
                .emit();
            TypeId::ERROR
        }
    }

    fn check_let_or_static(
        &mut self,
        node_id: ast::NodeId,
        pattern: &ast::BindingPattern, 
        init: &Expr,
        expected_ty: Option<TypeId>,
        is_static: bool,
        span: Span,
    ) -> TypeId {
        let init_ty = self.check_expr(init, expected_ty);
        let sym_kind = if is_static { SymbolKind::Static } else { SymbolKind::Var };

        let info = SymbolInfo {
            kind: sym_kind,
            node_id,
            type_id: init_ty,
            def_id: None,
            span,
            is_pub: false,
            is_mut: pattern.is_mut, 
        };
        let _ = self.ctx.scopes.define(pattern.name, info);
        TypeId::VOID
    }

    fn check_as_expr(&mut self, lhs: &Expr, target: &ast::TypeNode) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        let mut resolver = TypeResolver::new(self.ctx);
        let scope = resolver.ctx.scopes.current_scope_id().unwrap();
        let target_ty = resolver.resolve_type(target, scope);

        self.check_cast(lhs.span, lhs_ty, target_ty);
        target_ty
    }

    fn check_index_access(&mut self, lhs: &Expr, index: &Expr, is_mut: bool, span: Span) -> TypeId {
        if is_mut {
            self.ctx.struct_error(span, "mutable indexing `..[]` is not supported for single elements")
                .with_hint("use standard indexing `.[]` instead. Mutability is inherited automatically.")
                .emit();
        }

        let lhs_ty = self.check_expr(lhs, None);
        let idx_ty = self.check_expr(index, Some(TypeId::USIZE));

        let norm_idx = self.resolve_tv(idx_ty);
        if !self.ctx.type_registry.is_integer(norm_idx) && norm_idx != TypeId::ERROR {
            self.ctx.struct_error(index.span, "index must be an integer type").emit();
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. } | TypeKind::Slice { elem, .. } => elem,
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx.struct_error(lhs.span, "cannot index into a non-array/non-slice type").emit();
                TypeId::ERROR
            }
        }
    }

    fn check_slice_op(
        &mut self,
        lhs: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        _is_inclusive: bool,
        is_mut: bool,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        if let Some(s) = start {
            let s_ty = self.check_expr(s, Some(TypeId::USIZE));
            let s_ty_id = self.resolve_tv(s_ty);
            if !self.ctx.type_registry.is_integer(s_ty_id) {
                self.ctx.struct_error(s.span, "slice start index must be an integer").emit();
            }
        }
        if let Some(e) = end {
            let e_ty = self.check_expr(e, Some(TypeId::USIZE));
            let e_ty_id = self.resolve_tv(e_ty);
            if !self.ctx.type_registry.is_integer(e_ty_id) {
                self.ctx.struct_error(e.span, "slice end index must be an integer").emit();
            }
        }

        // 如果是 `..[`，必须确保目标内存具有可变性
        if is_mut && !self.is_lvalue_mutable(lhs) && lhs_ty != TypeId::ERROR {
            self.ctx.struct_error(span, "cannot create a mutable slice from an immutable location")
                .with_hint("ensure the target is bound with `let mut` or is a mutable pointer")
                .emit();
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. } => {
                self.ctx.type_registry.intern(TypeKind::Slice { is_mut, elem })
            }
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx.struct_error(lhs.span, "cannot slice a non-array/non-slice type").emit();
                TypeId::ERROR
            }
        }
    }

    fn check_data_init_expr(
        &mut self,
        type_node: Option<&ast::TypeNode>,
        literal: &ast::DataLiteralKind,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        // 智能决定目标类型
        let target_ty = if let Some(ty_ast) = type_node {
            // 情况 A: 显式指定了类型前缀 (如 Result[i32, i32].{ ... })
            let mut resolver = TypeResolver::new(self.ctx);
            let scope = resolver.ctx.scopes.current_scope_id().unwrap();
            resolver.resolve_type(ty_ast, scope)
        } else if let Some(exp) = expected_ty {
            // 情况 B: 省略了前缀，但外层有期望的类型 (如 `(ret_type)` Option[i32] = .{ ... })
            // 去除 Mut 修饰符，拿到真正的数据类型
            self.resolve_tv(exp)
        } else {
            // 情况 C: 既没写前缀，外层又不知道该是什么类型 (如 let x = .{ 10 })
            self.ctx.struct_error(span, "cannot infer type for anonymous initialization `.{...}`")
                .with_hint("provide an explicit type context or prepend the type name, e.g., `MyStruct.{...}`")
                .emit();
            return TypeId::ERROR;
        };

        // 将确定的 target_ty 继续下传给具体的字面量检查器
        self.check_data_literal(literal, target_ty, span)
    }

    fn check_undef(&mut self, expected_ty: Option<TypeId>, span: Span) -> TypeId {
        if expected_ty.is_none() {
            self.ctx
                .struct_error(span, "`undef` must have a known expected type context")
                .emit();
            TypeId::ERROR
        } else {
            expected_ty.unwrap()
        }
    }

    fn check_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                    self.check_discarded_expr(e);
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

    fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond.span, TypeId::BOOL, cond_ty);

        let then_ty = self.check_expr(then_branch, expected_ty);
        if let Some(else_expr) = else_branch {
            let else_ty = self.check_expr(else_expr, expected_ty);

            // 如果有一边发散(NEVER)，类型以另一边为准
            if then_ty == TypeId::NEVER {
                return else_ty;
            } else if else_ty == TypeId::NEVER {
                return then_ty;
            }

            self.check_coercion(else_expr.span, then_ty, else_ty);
            then_ty
        } else {
            TypeId::VOID
        }
    }

    fn check_switch_expr(
        &mut self,
        target: &Expr,
        cases: &[ast::SwitchCase],
        default_case: Option<&Expr>,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
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
            if common_ret_ty.is_none() || common_ret_ty == Some(TypeId::NEVER) {
                common_ret_ty = Some(body_ty);
            } else if body_ty != TypeId::NEVER {
                self.check_coercion(case.body.span, common_ret_ty.unwrap(), body_ty);
            }
        }

        if let Some(def) = default_case {
            let def_ty = self.check_expr(def, common_ret_ty);
            if common_ret_ty.is_none() || common_ret_ty == Some(TypeId::NEVER) {
                common_ret_ty = Some(def_ty);
            } else if def_ty != TypeId::NEVER {
                self.check_coercion(def.span, common_ret_ty.unwrap(), def_ty);
            }
        }
        self.check_switch_exhaustiveness(target_ty, cases, default_case.is_some(), span);
        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    fn check_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        if let Some(i) = init {
            self.check_discarded_expr(i);
        }
        if let Some(c) = cond {
            let c_ty = self.check_expr(c, Some(TypeId::BOOL));
            self.check_coercion(c.span, TypeId::BOOL, c_ty);
        }
        if let Some(p) = post {
            self.check_discarded_expr(p);
        }
        self.check_discarded_expr(body);
        self.ctx.scopes.exit_scope();
        TypeId::VOID
    }

    fn check_defer(&mut self, defer_expr: &Expr) -> TypeId {
        self.check_discarded_expr(defer_expr);
        TypeId::VOID
    }

    fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            // 将当前函数期待的类型 (expected_ret) 传给要 return 的表达式
            // 如果是 `return .{ Some: 1 }`，这里 expected_ret 就会传进 DataInit 中
            let val_ty = self.check_expr(v, Some(expected_ret));

            if let Some(ret_ty) = self.current_return_type {
                self.check_coercion(v.span, ret_ty, val_ty);
            }
        } else {
            if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
                let ret_str = self.ctx.ty_to_string(expected_ret);
                self.ctx
                    .struct_error(span, "expected a return value, but found empty return")
                    .with_hint(format!("function is expected to return `{}`", ret_str))
                    .emit();
            }
        }
        TypeId::VOID
    }

    fn check_generic_instantiation(
        &mut self,
        target: &Expr,
        types: &[ast::TypeNode],
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let target_norm = self.resolve_tv(target_ty);

        if target_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let mut arg_tys = Vec::new();
        {
            let mut resolver = TypeResolver::new(self.ctx);
            let scope = resolver.ctx.scopes.current_scope_id().unwrap();
            for ty_node in types {
                arg_tys.push(resolver.resolve_type(ty_node, scope));
            }
        }

        let (def_id, _) = match self.ctx.type_registry.get(target_norm) {
            TypeKind::FnDef(id, args) => (*id, args.clone()),
            TypeKind::Def(id, args) => (*id, args.clone()),
            TypeKind::Data(id, args) => (*id, args.clone()),
            TypeKind::TraitObject(id, args) => (*id, args.clone()),
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "this expression does not support generic instantiation",
                    )
                    .emit();
                return TypeId::ERROR;
            }
        };

        let generics = {
            let def = &self.ctx.defs[def_id.0 as usize];
            match def {
                Def::Function(f) => f.generics.clone(),
                Def::Struct(s) => s.generics.clone(),
                Def::Union(u) => u.generics.clone(),
                Def::TypeAlias(t) => t.generics.clone(),
                _ => unreachable!(),
            }
        };

        if generics.len() != arg_tys.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "expected {} generic arguments, but {} were provided",
                        generics.len(),
                        arg_tys.len()
                    ),
                )
                .emit();
            return TypeId::ERROR;
        }

        self.check_generic_bounds(span, &generics, &arg_tys);

        if matches!(self.ctx.type_registry.get(target_norm), TypeKind::FnDef(..)) {
            self.ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, arg_tys))
        } else {
            self.ctx
                .type_registry
                .intern(TypeKind::Def(def_id, arg_tys))
        }
    }

    fn check_lambda(
        &mut self,
        params: &[ast::FuncParam],
        ret_type_node: &ast::TypeNode,
        body: &Expr,
    ) -> TypeId {
        // 1. 解析参数类型和显式返回类型
        let (param_tys, ret_ty) = {
            let mut resolver = TypeResolver::new(self.ctx);
            let current_scope = resolver.ctx.scopes.current_scope_id().unwrap();

            let mut ptys = Vec::new();
            for param in params {
                ptys.push(resolver.resolve_type(&param.type_node, current_scope));
            }
            let rty = resolver.resolve_type(ret_type_node, current_scope);
            (ptys, rty)
        };

        // 2. 注册该 Lambda 的物理类型
        let func_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: param_tys.clone(),
            ret: ret_ty,
            is_variadic: false, // 匿名函数绝对不可能是变长参数
        });

        // 3. 准备进入 Lambda 内部作用域
        self.ctx.scopes.enter_scope();

        // 注入参数到局部作用域
        for (i, param) in params.iter().enumerate() {
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id: self.ctx.next_node_id(),
                type_id: param_tys[i],
                def_id: None,
                span: param.span,
                is_pub: false,
                is_mut: param.pattern.is_mut, 
            };
            let _ = self.ctx.scopes.define(param.pattern.name, info); 
        }

        // 保存外部函数的返回上下文，防止内部 return 污染外部
        let prev_return_type = self.current_return_type;
        let prev_has_returned = self.has_returned;

        // 设置当前 Lambda 的返回期望
        self.current_return_type = Some(ret_ty);
        self.has_returned = false;

        // 4. 对 Lambda 内部的代码块进行类型检查
        let body_ty = self.check_expr(body, Some(ret_ty));

        // 如果内部没有调用过显式的 return，才需要强制约束 Block 的尾表达式类型；
        // 如果内部已经 return 过了，Block 自身的 void 类型就不必与 ret_ty 强制匹配了。
        if !self.has_returned {
            self.check_coercion(body.span, ret_ty, body_ty);
        } else if body_ty != TypeId::VOID && body_ty != TypeId::ERROR {
            // 如果既有 return，又有隐式的尾表达式，校验一下尾表达式
            self.check_coercion(body.span, ret_ty, body_ty);
        }

        // 恢复外部上下文
        self.current_return_type = prev_return_type;
        self.has_returned = prev_has_returned;

        self.ctx.scopes.exit_scope();

        func_ty
    }

    fn check_generic_bounds(
        &mut self,
        span: Span,
        generics: &[ast::GenericParam],
        arg_tys: &[TypeId],
    ) {
        for (i, param) in generics.iter().enumerate() {
            if i >= arg_tys.len() {
                break;
            }
            let act_ty = arg_tys[i];

            for constraint_node in &param.constraints {
                let constraint_ty = self
                    .ctx
                    .node_types
                    .get(&constraint_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if constraint_ty != TypeId::ERROR {
                    if !self.check_trait_impl(act_ty, constraint_ty) {
                        let param_name = self.ctx.resolve(param.name);
                        let req_str = self.ctx.ty_to_string(constraint_ty);
                        let act_str = self.ctx.ty_to_string(act_ty);
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "type does not satisfy trait bounds for generic parameter `{}`",
                                    param_name
                                ),
                            )
                            .with_hint(format!("required trait: `{}`", req_str))
                            .with_hint(format!("provided type: `{}`", act_str))
                            .emit();
                    }
                }
            }
        }
    }

    // ==========================================
    //          Core Operations & Coercion
    // ==========================================

    fn check_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, expected_ty);
        let rhs_ty = self.check_expr(rhs, Some(lhs_ty));

        let l_norm = self.resolve_tv(lhs_ty);
        let r_norm = self.resolve_tv(rhs_ty);

        if l_norm == TypeId::ERROR || r_norm == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let is_l_ptr = matches!(
            self.ctx.type_registry.get(l_norm),
            TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
        );
        let is_r_ptr = matches!(
            self.ctx.type_registry.get(r_norm),
            TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
        );

        use BinaryOperator::*;
        match op {
            Add | Subtract | Multiply | Divide | Modulo => {
                if is_l_ptr || is_r_ptr {
                    self.ctx.struct_error(lhs.span, "implicit pointer arithmetic is strictly forbidden in Kern")
                        .with_hint("use explicit `as usize` casts to perform arithmetic, or use pointer methods")
                        .emit();
                    return TypeId::ERROR;
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
            Equal | NotEqual | LessThan | GreaterThan | LessOrEqual | GreaterOrEqual => {
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                TypeId::BOOL
            }
            LogicalAnd | LogicalOr => {
                self.check_coercion(lhs.span, TypeId::BOOL, l_norm);
                self.check_coercion(rhs.span, TypeId::BOOL, r_norm);
                TypeId::BOOL
            }
            _ => {
                // Bitwise Ops
                if !self.ctx.type_registry.is_integer(l_norm) {
                    self.ctx
                        .struct_error(lhs.span, "bitwise operations require integer types")
                        .emit();
                }
                if !self.check_coercion(rhs.span, l_norm, r_norm) {
                    return TypeId::ERROR;
                }
                l_norm
            }
        }
    }

    fn check_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        span: Span,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let inner_expected = match op {
            UnaryOperator::Negate | UnaryOperator::BitwiseNot => expected_ty,
            // 兼容不可变取址和可变取址
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                if let Some(exp) = expected_ty {
                    let norm = self.resolve_tv(exp);
                    match self.ctx.type_registry.get(norm) {
                        TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                            Some(*elem)
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        let op_ty = self.check_expr(operand, inner_expected);
        if op_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        match op {
            UnaryOperator::AddressOf | UnaryOperator::MutAddressOf => {
                let is_mut = op == UnaryOperator::MutAddressOf;
                
                // 🌟 核心拦截：不允许对不可变的左值使用 `..&` 获取可变指针
                if is_mut && !self.is_lvalue_mutable(operand) {
                    self.ctx.struct_error(span, "cannot take mutable address `..&` of immutable memory")
                        .with_hint("declare the variable with `let mut` or ensure the target is mutable")
                        .emit();
                }
                
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut,
                    elem: op_ty,
                })
            }
            UnaryOperator::PointerDeRef => {
                let norm = self.resolve_tv(op_ty);
                match self.ctx.type_registry.get(norm).clone() {
                    TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => elem,
                    _ => {
                        let ty_str = self.ctx.ty_to_string(op_ty);
                        self.ctx
                            .struct_error(span, "cannot dereference a non-pointer type")
                            .with_hint(format!("type is `{}`", ty_str))
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::LengthOf => {
                let norm = self.resolve_tv(op_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Array { .. } | TypeKind::Slice { .. } => TypeId::USIZE,
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "length operator `#` can only be applied to arrays and slices",
                            )
                            .emit();
                        TypeId::ERROR
                    }
                }
            }
            UnaryOperator::Negate => {
                let op_ty_id = self.resolve_tv(op_ty);
                if !self.ctx.type_registry.is_integer(op_ty_id)
                    && !self.ctx.type_registry.is_float(op_ty_id)
                {
                    self.ctx
                        .struct_error(span, "negation requires a numeric type")
                        .emit();
                }
                op_ty
            }
            UnaryOperator::LogicalNot => {
                self.check_coercion(span, TypeId::BOOL, op_ty);
                TypeId::BOOL
            }
            UnaryOperator::BitwiseNot => {
                let op_ty_id = self.resolve_tv(op_ty);
                if !self.ctx.type_registry.is_integer(op_ty_id) {
                    self.ctx
                        .struct_error(span, "bitwise NOT requires an integer type")
                        .emit();
                }
                op_ty
            }
        }
    }

    fn check_assign(&mut self, lhs: &Expr, rhs: &Expr, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        // 使用继承可变性分析器
        if !self.is_lvalue_mutable(lhs) && lhs_ty != TypeId::ERROR {
            self.ctx.struct_error(lhs.span, "cannot assign to an immutable variable or location")
                .with_hint("if this is a variable, declare it with `let mut`")
                .with_hint("if this is a pointer dereference, ensure it is a mutable pointer (`*mut T`)")
                .emit();
        }

        let l_norm = self.resolve_tv(lhs_ty);
        let rhs_ty = self.check_expr(rhs, Some(l_norm));

        if lhs_ty == TypeId::ERROR || rhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let rhs_ty_id = self.resolve_tv(rhs_ty);
        self.check_coercion(span, l_norm, rhs_ty_id);
        TypeId::VOID
    }

    // ==========================================
    //            Coercion & Mutability
    // ==========================================

    pub fn check_coercion(&mut self, span: Span, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.resolve_tv(expected);
        let act = self.resolve_tv(actual);

        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR { return true; }
        if act == TypeId::NEVER { return true; }

        let exp_kind = self.ctx.type_registry.get(exp).clone();
        let act_kind = self.ctx.type_registry.get(act).clone();

        // 1. 触发类型合一 (Unification)
        if let TypeKind::TypeVar(vid) = act_kind {
            self.type_vars[vid as usize] = Some(exp);
            return true;
        }
        if let TypeKind::TypeVar(vid) = exp_kind {
            self.type_vars[vid as usize] = Some(act);
            return true;
        }

        // 2. 指针与易失指针安全降级
        if self.check_pointer_downgrade(&exp_kind, &act_kind) { return true; }

        // 3. 切片降级与数组退化 (逻辑保持原样，但依赖修改后的 downgrade 助手)
        if let TypeKind::Slice { is_mut: e_mut, elem: exp_elem } = exp_kind {
            if self.check_slice_downgrade(e_mut, exp_elem, &act_kind) { return true; }
            match self.check_array_decay(e_mut, exp_elem, &act_kind, span) {
                Ok(true) => return true,
                Err(()) => return false,
                Ok(false) => {}
            }
        }

        // 4. 指针到 Trait Object 的隐式转换 
        if let TypeKind::Pointer { is_mut: e_mut, elem: e_inner } = exp_kind {
            if let TypeKind::Pointer { is_mut: a_mut, .. } = act_kind {
                // 确保可变性安全：不能把不可变指针隐式塞进期望可变指针的 TraitObject 里
                if !(e_mut && !a_mut) {
                    let e_inner_norm = self.resolve_tv(e_inner);
                    if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_inner_norm) {
                        // 校验底层指针类型是否实现了该 Trait
                        if self.check_trait_impl(act, e_inner_norm) {
                            return true;
                        }
                    }
                }
            }
        }

        self.emit_mismatch_error(span, expected, actual);
        false
    }

    /// 助手 1：指针降级校验
    fn check_pointer_downgrade(&mut self, exp_kind: &TypeKind, act_kind: &TypeKind) -> bool {
        match (exp_kind, act_kind) {
            (TypeKind::Pointer { is_mut: e_mut, elem: e_inner }, TypeKind::Pointer { is_mut: a_mut, elem: a_inner })
            | (TypeKind::VolatilePtr { is_mut: e_mut, elem: e_inner }, TypeKind::VolatilePtr { is_mut: a_mut, elem: a_inner }) => {
                if *e_mut && !*a_mut { return false; }
                self.check_coercion(Span::default(), *e_inner, *a_inner)
            }
            _ => false,
        }
    }

    /// 助手 2：切片降级校验
    fn check_slice_downgrade(&mut self, exp_is_mut: bool, exp_elem: TypeId, act_kind: &TypeKind) -> bool {
        if let TypeKind::Slice { is_mut: act_mut, elem: act_elem } = act_kind {
            if exp_is_mut && !*act_mut { return false; }
            self.check_coercion(Span::default(), exp_elem, *act_elem)
        } else { false }
    }

    /// 助手 3：数组到切片的退化
    fn check_array_decay(&mut self, exp_is_mut: bool, exp_elem: TypeId, act_kind: &TypeKind, span: Span) -> Result<bool, ()> {
        if let TypeKind::Array { is_mut: act_mut, elem: act_elem, .. } = act_kind {
            let exp_base = self.resolve_tv(exp_elem);
            let act_base = self.resolve_tv(*act_elem);

            if exp_base == act_base {
                if exp_is_mut && !*act_mut {
                    self.ctx.struct_error(span, "cannot implicitly convert an immutable array to a mutable slice `[]mut T`").emit();
                    return Err(());
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 助手 4：格式化并输出类型不匹配错误
    fn emit_mismatch_error(&mut self, span: Span, expected: TypeId, actual: TypeId) {
        let exp_str = self.ctx.ty_to_string(expected);
        let act_str = self.ctx.ty_to_string(actual);

        self.ctx
            .struct_error(span, "mismatched types")
            .with_hint(format!("expected `{}`", exp_str))
            .with_hint(format!("   found `{}`", act_str))
            .emit();
    }

    fn check_cast(&mut self, span: Span, from: TypeId, to: TypeId) {
        if from == to || from == TypeId::ERROR || to == TypeId::ERROR {
            return;
        }

        let f_norm = self.resolve_tv(from);
        let t_norm = self.resolve_tv(to);

        let is_f_int = self.ctx.type_registry.is_integer(f_norm);
        let is_t_int = self.ctx.type_registry.is_integer(t_norm);
        let is_f_ptr = matches!(
            self.ctx.type_registry.get(f_norm),
            TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
        );
        let is_t_ptr = matches!(
            self.ctx.type_registry.get(t_norm),
            TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
        );

        // 1. 允许指针视角转换 (例如 *i32 as *u8)
        if is_f_ptr && is_t_ptr {
            return;
        }

        // 2. 允许指针和 usize 互转 (用于底层地址算术和 0 as *T)
        // 在 Kern 中，字面量 0 默认推导为 usize，所以 `0 as *i32` 是完全合法的
        if (is_f_ptr && t_norm == TypeId::USIZE) || (f_norm == TypeId::USIZE && is_t_ptr) {
            return;
        }

        // 3. 严禁纯数字之间的 as 转换
        if is_f_int && is_t_int {
            let from_str = self.ctx.ty_to_string(from);
            let to_str = self.ctx.ty_to_string(to);
            self.ctx.struct_error(span, "numeric casting via `as` is forbidden to prevent implicit truncation or extension")
                .with_hint(format!("use `@intCast[{}, {}](val)` for explicit integer conversions", from_str, to_str))
                .emit();
            return;
        }

        // 4. 兜底报错
        let from_str = self.ctx.ty_to_string(from);
        let to_str = self.ctx.ty_to_string(to);
        self.ctx
            .struct_error(span, "invalid `as` cast")
            .with_hint("`as` is strictly limited to pointer casts and pointer-to-usize conversions")
            .with_hint("for trait objects, use explicit constructor syntax: `Trait.{ ptr }`")
            .with_hint("for strings/slices to pointers, use explicit indexing: `slice.[0].&`")
            .with_hint(format!(
                "attempted to cast from `{}` to `{}`",
                from_str, to_str
            ))
            .emit();
    }

    fn is_mutable_pointer(&mut self, ty: TypeId) -> bool {
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }

    // ==========================================
    //            Field & Method Access
    // ==========================================

    fn check_field_access(
        &mut self,
        lhs: &Expr,
        field: crate::utils::SymbolId,
        span: Span,
    ) -> TypeId {
        if let ExprKind::Identifier(name) = &lhs.kind {
            if let Some(info) = self.ctx.scopes.resolve(*name) {
                if info.kind == SymbolKind::Module {
                    let mod_def_id = info.def_id.unwrap();
                    let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        m.scope_id
                    } else {
                        unreachable!()
                    };

                    // 去那个模块的作用域里寻找真实的符号
                    if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field) {
                        let real_ty = if target_info.kind == SymbolKind::Function {
                            self.ctx
                                .type_registry
                                .intern(TypeKind::FnDef(target_info.def_id.unwrap(), vec![]))
                        } else {
                            target_info.type_id
                        };
                        let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                        self.ctx.node_types.insert(lhs.id, mod_ty);
                        return real_ty;
                    } else {
                        let mod_name = self.ctx.resolve(*name);
                        let field_name = self.ctx.resolve(field);
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "module `{}` has no public member `{}`",
                                    mod_name, field_name
                                ),
                            )
                            .emit();
                        return TypeId::ERROR;
                    }
                }
            }
        }

        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        // 1. 获取底层规范类型以及访问路径的 Mut 属性 (自动解引用逻辑)
        let current_norm = self.get_base_type(lhs_ty);

        // 2. 如果是 Trait Object，走虚表方法解析路径
        if let TypeKind::TraitObject(trait_def_id, trait_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            return self.resolve_trait_object_method(trait_def_id, &trait_args, field, span);
        }

        // 3. 如果是具名类型 (Struct/Union/Enum)，查找字段或变体
        if let TypeKind::Def(def_id, generic_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            if let Some(field_ty) =
                self.resolve_def_field(def_id, &generic_args, field)
            {
                return field_ty;
            }
        }

        // 4. 作为最后的 fallback，去全局的 impl 块中查找方法 (处理形如 `10.to_string()` 或普通结构体方法)
        if let Some(method_ty) = self.resolve_impl_method(lhs_ty, field) {
            return method_ty;
        }

        // 5. 全部失败，抛出详细诊断
        let field_str = self.ctx.resolve(field);
        let lhs_str = self.ctx.ty_to_string(lhs_ty);

        self.ctx
            .struct_error(
                span,
                format!(
                    "no field or method named `{}` found on type `{}`",
                    field_str, lhs_str
                ),
            )
            .with_hint(
                "if this is a method, ensure the trait defining it is imported and implemented",
            )
            .with_hint("if this is a struct field, check for typos")
            .emit();

        TypeId::ERROR
    }

    /// 辅助方法 1：自动解引用 Pointer/VolatilePtr，获取底层的 Struct/Union/Enum 类型
    fn get_base_type(&mut self, mut base_ty: TypeId) -> TypeId {
        loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm).clone() {
                // 遇到指针，自动扒掉外衣继续往下找
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = elem;
                }
                // 找到底了，返回
                _ => return norm,
            }
        }
    }

    /// 辅助方法 2：解析 Trait Object 的接口方法
    fn resolve_trait_object_method(
        &mut self,
        trait_def_id: crate::sema::ty::DefId,
        trait_args: &[TypeId],
        field: crate::utils::SymbolId,
        span: Span,
    ) -> TypeId {
        let trait_def = match &self.ctx.defs[trait_def_id.0 as usize] {
            Def::Trait(t) => t.clone(),
            _ => unreachable!(),
        };

        if let Some(&(_, mut method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(m_name, _)| *m_name == field)
        {
            // 泛型实例化替换
            if !trait_def.generics.is_empty() && !trait_args.is_empty() {
                let mut map = HashMap::new();
                for (i, param) in trait_def.generics.iter().enumerate() {
                    map.insert(param.name, trait_args[i]);
                }
                let mut subst =
                    Substituter::new(&mut self.ctx.type_registry, &map);
                method_ty = subst.substitute(method_ty);
            }
            return method_ty;
        }

        let field_str = self.ctx.resolve(field);
        self.ctx
            .struct_error(
                span,
                format!("method `{}` not found in trait object", field_str),
            )
            .with_hint("ensure the method is explicitly declared in the trait's contract")
            .emit();
        TypeId::ERROR
    }

    /// 辅助方法 3：解析 Struct/Union 字段或 Enum 变体
    fn resolve_def_field(
        &mut self,
        def_id: crate::sema::ty::DefId,
        generic_args: &[TypeId],
        field: crate::utils::SymbolId,
    ) -> Option<TypeId> {
        let def = self.ctx.defs[def_id.0 as usize].clone();

        match &def {
            Def::Struct(s) => {
                if let Some(f) = s.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_to_field(
                        &s.generics,
                        generic_args,
                        f.type_node.id,
                    ));
                }
            }
            Def::Union(u) => {
                if let Some(f) = u.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_to_field(
                        &u.generics,
                        generic_args,
                        f.type_node.id,
                    ));
                }
            }
            _ => {}
        }
        None
    }

    /// 辅助方法 3.1：处理字段提取后的泛型替换
    fn apply_generics_to_field(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
        node_id: ast::NodeId,
    ) -> TypeId {
        let mut field_ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        if !generics.is_empty() && !args.is_empty() {
            let mut map = std::collections::HashMap::new();
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
            let mut subst =
                crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
            field_ty = subst.substitute(field_ty);
        }
        
        field_ty 
    }

    /// 辅助方法 4：通过全局 Impl 块进行方法分发 (Method Dispatch)
    fn resolve_impl_method(
        &mut self,
        lhs_ty: TypeId,
        field: crate::utils::SymbolId,
    ) -> Option<TypeId> {
        let mut found_method_id = None;
        let mut resolved_impl_args = Vec::new();

        // TODO: 注意：未来可以考虑在 Sema 收集阶段将这些方法缓存到 Context 中避免每次 O(N) 遍历
        let impl_blocks: Vec<_> = self.ctx.defs.iter().filter_map(|def| {
            if let Def::Impl(impl_def) = def {
                Some(impl_def.clone())
            } else {
                None
            }
        }).collect();

        for impl_def in impl_blocks {
            let impl_target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut map = std::collections::HashMap::new();

            if self.unify(impl_target_ty, lhs_ty, &mut map) {
                // 将 Impl 块捕获的泛型参数提取出来
                for param in &impl_def.generics {
                    resolved_impl_args
                        .push(map.get(&param.name).copied().unwrap_or(TypeId::ERROR));
                }
                // 在匹配的 Impl 块内寻找目标函数
                for &method_id in &impl_def.methods {
                    if let Def::Function(func_def) = &self.ctx.defs[method_id.0 as usize] {
                        if func_def.name == field {
                            found_method_id = Some(method_id);
                            break;
                        }
                    }
                }
            }
            if found_method_id.is_some() {
                break;
            }
        }

        found_method_id.map(|method_id| {
            self.ctx
                .type_registry
                .intern(TypeKind::FnDef(method_id, resolved_impl_args))
        })
    }

    // ==========================================
    //            Function & Method Calls
    // ==========================================

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
        // 1. 拦截 @asm 宏调用
        if let ExprKind::Identifier(sym) = &callee.kind {
            if self.ctx.resolve(*sym) == "@asm" {
                self.ctx.node_types.insert(callee.id, TypeId::VOID);
                return self.check_asm_call(args, span);
            }
        }

        let callee_ty = self.check_expr(callee, None);
        let norm_callee = self.resolve_tv(callee_ty);

        if norm_callee == TypeId::ERROR {
            // 防止 AST 产生洞
            for arg in args {
                self.check_expr(arg, None);
            }
            return TypeId::ERROR;
        }

        // 2. 探查是否是方法调用，提取接收者 (Receiver) 信息
        let (is_method, receiver_ty) = self.resolve_method_context(callee);

        // 3. 智能推导泛型参数，获取解析后的签名与修复后的 Callee 类型
        let (sig_ty, inferred_callee_ty) = self.deduce_and_resolve_signature(
            norm_callee,
            args,
            is_method,
            receiver_ty,
            callee.span,
        );

        // 4. 如果推导成功，将补全了泛型参数的类型重新写入 AST 节点
        // 这样 LLVM 降级层就能拿到具体的泛型实参
        if let Some(fixed_ty) = inferred_callee_ty {
            self.ctx.node_types.insert(callee.id, fixed_ty);
        }

        // 5. 校验最终签名并执行分发
        if let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = self.ctx.type_registry.get(sig_ty).clone()
        {
            self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

            if is_method && !params.is_empty() {
                self.check_method_receiver(params[0], receiver_ty, callee.span);
            }

            self.check_call_arguments(args, &params, is_method, is_variadic);
            return ret;
        }

        let callee_str = self.ctx.ty_to_string(callee_ty);
        self.ctx
            .struct_error(callee.span, "expression is not callable")
            .with_hint(format!("type is `{}`", callee_str))
            .emit();
        TypeId::ERROR
    }

    /// 助手：智能泛型推导与签名解析
    fn deduce_and_resolve_signature(
        &mut self,
        norm_callee: TypeId,
        args: &[Expr],
        is_method: bool,
        receiver_ty: TypeId,
        span: Span,
    ) -> (TypeId, Option<TypeId>) {
        if let TypeKind::FnDef(def_id, explicit_args) =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            let (raw_sig, generics, fn_name_id) = match &self.ctx.defs[def_id.0 as usize] {
                Def::Function(func) => (
                    func.resolved_sig.expect("Function signature missing"),
                    func.generics.clone(), // 提取并拷贝一份泛型参数列表
                    func.name,
                ),
                _ => unreachable!(),
            };

            let generics_count = generics.len();

            // 如果没有泛型，直接返回原始签名
            if generics_count == 0 {
                return (raw_sig, None);
            }

            // 规则 A：用户显式提供了完整的泛型参数
            if explicit_args.len() == generics_count {
                let mut map = HashMap::new();
                for (i, param) in generics.iter().enumerate() {
                    map.insert(param.name, explicit_args[i]);
                }
                let mut subst =
                    Substituter::new(&mut self.ctx.type_registry, &map);
                return (subst.substitute(raw_sig), None);
            }

            // 规则 B：不允许部分提供泛型参数
            if !explicit_args.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx.struct_error(span, format!("function `{}` requires exactly {} generic arguments, but {} were provided", name_str, generics_count, explicit_args.len()))
                    .with_hint("either provide all generic arguments or omit them entirely to let the compiler infer them")
                    .emit();
                return (TypeId::ERROR, None);
            }

            // 规则 C：泛型完全省略，启动单向参数推导
            let mut map = HashMap::new();
            let raw_params = if let TypeKind::Function { params, .. } =
                self.ctx.type_registry.get(raw_sig).clone()
            {
                params
            } else {
                unreachable!()
            };

            let param_offset = if is_method { 1 } else { 0 };

            // 1. 优先从 Receiver (比如 list.push) 推导
            if is_method && !raw_params.is_empty() {
                let stripped_recv = self.resolve_tv(receiver_ty);
                self.unify(raw_params[0], stripped_recv, &mut map);
            }

            // 2. 从实参推导
            for (i, arg) in args.iter().enumerate() {
                let sig_idx = i + param_offset;
                if sig_idx < raw_params.len() {
                    let arg_ty = self.check_expr(arg, None);
                    let arg_norm = self.resolve_tv(arg_ty);
                    if arg_norm != TypeId::ERROR {
                        self.unify(raw_params[sig_idx], arg_norm, &mut map);
                    }
                }
            }

            // 3. 检查是否所有泛型参数都被成功推导
            let mut missing_generics = Vec::new();
            let mut resolved_args = Vec::new();
            for param in &generics {
                if let Some(&inferred_ty) = map.get(&param.name) {
                    resolved_args.push(inferred_ty);
                } else {
                    missing_generics.push(self.ctx.resolve(param.name).to_string());
                }
            }

            // 规则 D：存在无法推导的泛型参数，报错
            if !missing_generics.is_empty() {
                let name_str = self.ctx.resolve(fn_name_id).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "cannot infer generic type(s) `{}` for function `{}`",
                            missing_generics.join(", "),
                            name_str
                        ),
                    )
                    .with_hint("the compiler needs these generic types to be explicitly specified")
                    .emit();
                return (TypeId::ERROR, None);
            }

            // 构造包含具体参数的 FnDef 类型，以便稍后写入 AST
            let inferred_callee_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, resolved_args));

            let mut subst =
                Substituter::new(&mut self.ctx.type_registry, &map);
            return (subst.substitute(raw_sig), Some(inferred_callee_ty));
        }

        (norm_callee, None)
    }

    /// 助手 2：判断这是否是一个方法调用，如果是，提取它的 Receiver 类型 (LHS)
    fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
            // 拦截：如果 lhs 是一个模块，则绝对不是运行时的方法调用
            if let ExprKind::Identifier(name) = &lhs.kind {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    if info.kind == SymbolKind::Module {
                        return (false, TypeId::ERROR);
                    }
                }
            }

            let callee_node_ty = self
                .ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_node_ty = self.ctx.type_registry.normalize(callee_node_ty);

            if matches!(
                self.ctx.type_registry.get(norm_node_ty),
                TypeKind::FnDef(..) | TypeKind::Function { .. }
            ) {
                let receiver_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                return (true, receiver_ty);
            }
        }
        (false, TypeId::ERROR)
    }

    /// 助手 3：校验参数个数 (Arity)
    fn check_call_arity(
        &mut self,
        arg_count: usize,
        param_count: usize,
        is_method: bool,
        is_variadic: bool,
        span: Span,
    ) {
        let expected_arg_count = if is_method {
            param_count.saturating_sub(1)
        } else {
            param_count
        };

        if is_variadic {
            if arg_count < expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects at least {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        } else {
            if arg_count != expected_arg_count {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "function expects exactly {} arguments, but {} were provided",
                            expected_arg_count, arg_count
                        ),
                    )
                    .emit();
            }
        }
    }

    /// 助手 4：Kern 专属校验 - 方法调用的接收者类型匹配
    fn check_method_receiver(&mut self, expected_self: TypeId, receiver_ty: TypeId, span: Span) {
        if !self.check_coercion(span, expected_self, receiver_ty) {
            let norm_expected = self.resolve_tv(expected_self);
            let is_exp_ptr = matches!(
                self.ctx.type_registry.get(norm_expected),
                TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
            );

            if is_exp_ptr {
                self.ctx.struct_error(span, "method receiver type mismatch")
                    .with_hint("the method expects a pointer receiver")
                    .with_hint("Kern does not implicitly take addresses for method calls. Try using `(&obj).method()` or `obj.&.method()`")
                    .emit();
            }
        }
    }

    /// 助手 5：逐一检查参数的类型转换，并处理 C ABI 可变参数 (Varargs) 的类型提升规则
    fn check_call_arguments(
        &mut self,
        args: &[Expr],
        params: &[TypeId],
        is_method: bool,
        _is_variadic: bool,
    ) {
        let param_offset = if is_method { 1 } else { 0 };

        for (i, arg) in args.iter().enumerate() {
            let sig_param_idx = i + param_offset;

            if sig_param_idx < params.len() {
                // 1. 常规参数校验
                let arg_ty = self.check_expr(arg, Some(params[sig_param_idx]));
                self.check_coercion(arg.span, params[sig_param_idx], arg_ty);
            } else {
                // 2. Variadic 额外参数校验 (C ABI Rules)
                let arg_ty = self.check_expr(arg, None);
                let norm_arg = self.resolve_tv(arg_ty);

                if norm_arg == TypeId::ERROR {
                    continue;
                }

                // C ABI 整型提升规则：传入可变参数的整型不能小于 32位
                let is_small_int = matches!(
                    norm_arg,
                    TypeId::I8 | TypeId::I16 | TypeId::U8 | TypeId::U16
                );

                if is_small_int {
                    self.ctx.struct_error(arg.span, "C ABI requires integer arguments passed to `...` to be at least 32-bit")
                        .with_hint("please cast it explicitly (e.g., `as i32` or `as u32`)")
                        .emit();
                } else if norm_arg == TypeId::F32 {
                    // C ABI 浮点型提升规则：传入可变参数的浮点数必须被提升为 64位 (double)
                    self.ctx
                        .struct_error(
                            arg.span,
                            "C ABI requires float arguments passed to `...` to be 64-bit",
                        )
                        .with_hint("please cast it explicitly (e.g., `as f64`)")
                        .emit();
                }
            }
        }
    }

    // ==========================================
    //            Data Literal Checking
    // ==========================================

    /// 辅助方法 1：校验普通数组字面量 `.{ 1, 2, 3 }`
    fn check_array_literal(
        &mut self,
        elems: &[Expr],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 动态剥离类型信息
        let (exp_elem_ty, expected_len, exp_is_mut) = match self.ctx.type_registry.get(exp_norm) {
            TypeKind::Array { elem, len, is_mut } => (*elem, Some(*len), *is_mut),
            TypeKind::ArrayInfer { elem, is_mut } => (*elem, None, *is_mut),
            TypeKind::Slice { elem, is_mut } => (*elem, None, *is_mut),
            _ => {
                let ty_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        span,
                        "expected an array or slice type for literal `.{ ... }`",
                    )
                    .with_hint(format!("context expects `{}`", ty_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. 如果是定长数组，校验长度
        if let Some(len) = expected_len {
            if elems.len() as u64 != len {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "array literal length ({}) does not match expected length ({})",
                            elems.len(),
                            len
                        ),
                    )
                    .emit();
            }
        }

        // 3. 校验所有元素的类型
        for e in elems {
            let act_ty = self.check_expr(e, Some(exp_elem_ty));
            self.check_coercion(e.span, exp_elem_ty, act_ty);
        }

        // 4. 返回最终确定的类型
        if expected_len.is_none() {
            self.ctx.type_registry.intern(TypeKind::Array {
                is_mut: exp_is_mut, 
                elem: exp_elem_ty,
                len: elems.len() as u64,
            })
        } else {
            // 原本就是 [N]T
            expected
        }
    }

    /// 辅助方法 2：校验重复数组字面量 `.{ 0; 1024 }`
    fn check_repeat_literal(
        &mut self,
        value: &Expr,
        count: &Expr,
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 动态剥离类型信息
        let (exp_elem_ty, is_infer, exp_is_mut) = match self.ctx.type_registry.get(exp_norm) {
            TypeKind::Array { elem, is_mut, .. } => (*elem, false, *is_mut),
            TypeKind::ArrayInfer { elem, is_mut } => (*elem, true, *is_mut),
            TypeKind::Slice { elem, is_mut } => (*elem, true, *is_mut),
            _ => {
                let ty_str = self.ctx.ty_to_string(expected);
                self.ctx
                    .struct_error(
                        span,
                        "expected an array or slice type for repeat literal `.{ v; N }`",
                    )
                    .with_hint(format!("context expects `{}`", ty_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        // 2. 校验重复的元素值
        let val_ty = self.check_expr(value, Some(exp_elem_ty));
        self.check_coercion(value.span, exp_elem_ty, val_ty);

        // 3. 校验重复次数
        let c_ty = self.check_expr(count, Some(TypeId::USIZE));
        let c_ty_id = self.resolve_tv(c_ty);
        if !self.ctx.type_registry.is_integer(c_ty_id) {
            self.ctx
                .struct_error(count.span, "repeat count must be an integer")
                .emit();
        }

        // 4. 返回最终类型
        if is_infer {
            let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
            let actual_len = match ce.eval_usize(count) {
                Ok(val) => val,
                Err(_) => 0, // 兜底填0
            };

            self.ctx.type_registry.intern(TypeKind::Array {
                is_mut: exp_is_mut, 
                elem: exp_elem_ty,
                len: actual_len,
            })
        } else {
            expected
        }
    }

    /// 辅助方法 3：校验结构体或联合体初始化 `.{ x: 10, y: 20 }` 或 Union `.{ as_int: 123 }`
    fn check_struct_or_union_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        // 1. 提取定义信息与泛型实参，同时识别是 Struct 还是 Union
        let (def_fields, def_name, def_generics, generic_args, is_union) =
            if let TypeKind::Def(def_id, args) = self.ctx.type_registry.get(exp_norm) {
                match &self.ctx.defs[def_id.0 as usize] {
                    Def::Struct(s) => (
                        s.fields.clone(),
                        self.ctx.resolve(s.name).to_string(),
                        s.generics.clone(),
                        args.clone(),
                        false,
                    ),
                    Def::Union(u) => (
                        u.fields.clone(),
                        self.ctx.resolve(u.name).to_string(),
                        u.generics.clone(),
                        args.clone(),
                        true,
                    ),
                    _ => {
                        self.ctx
                            .struct_error(
                                span,
                                "expected a struct or union type for literal initialization",
                            )
                            .emit();
                        return TypeId::ERROR;
                    }
                }
            } else {
                self.ctx
                    .struct_error(
                        span,
                        "expected a struct or union type for literal initialization",
                    )
                    .emit();
                return TypeId::ERROR;
            };

        let mut initialized = std::collections::HashSet::new();

        // 2. 校验用户提供的初始化字段的类型
        for init_f in init_fields {
            if let Some(def_f) = def_fields.iter().find(|f| f.name == init_f.name) {
                let mut f_ty = self
                    .ctx
                    .node_types
                    .get(&def_f.type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                // 如果结构体本身的字段类型就是错的，必须强制抛出异常阻断编译
                if f_ty == TypeId::ERROR {
                    self.ctx.struct_error(init_f.span, "internal compiler error: field type was unresolved prior to Typeck")
                        .with_hint("this is usually caused by a failing type resolver that missed emitting a diagnostic")
                        .emit();
                }

                // 处理泛型字段类型替换
                if !def_generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in def_generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(
                        &mut self.ctx.type_registry,
                        &map,
                    );
                    f_ty = subst.substitute(f_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(f_ty));
                self.check_coercion(init_f.span, f_ty, val_ty);

                // 检查重复初始化的字段（对 struct 和 union 都有效）
                if !initialized.insert(init_f.name) {
                    let name_str = self.ctx.resolve(init_f.name);
                    self.ctx
                        .struct_error(
                            init_f.span,
                            format!("field `{}` is initialized more than once", name_str),
                        )
                        .emit();
                }
            } else {
                let name_str = self.ctx.resolve(init_f.name);
                self.ctx
                    .struct_error(
                        init_f.span,
                        format!("field `{}` does not exist in `{}`", name_str, def_name),
                    )
                    .emit();
            }
        }

        // 3. 校验 Kern 核心规则：针对 Struct 和 Union 分别处理
        if is_union {
            // Kern Union 规则：必须且只能初始化 1 个字段
            if initialized.len() != 1 {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "union `{}` must be initialized with exactly one field",
                            def_name
                        ),
                    )
                    .with_hint(format!("you provided {} fields", initialized.len()))
                    .with_hint(
                        "unions share memory across fields, so multiple initializers are ambiguous",
                    )
                    .emit();
            }
        } else {
            // Kern Struct 规则：无隐式零初始化。漏掉字段必须显式使用 undef 或具有默认值
            for def_f in &def_fields {
                if !initialized.contains(&def_f.name) && def_f.default_value.is_none() {
                    let name_str = self.ctx.resolve(def_f.name).to_string();
                    self.ctx.struct_error(span, format!("field `{}` is missing and has no default value", name_str))
                        .with_hint("Kern structs do not zero-initialize implicitly.")
                        .with_hint(format!("use `{}: type.{{undef}}` if you intentionally want to leave memory uninitialized", name_str))
                        .emit();
                }
            }
        }

        expected
    }

    /// 辅助方法 4：校验标量构造 `.{ 10 }`
    fn check_scalar_literal(&mut self, inner: &Expr, expected: TypeId) -> TypeId {
        let inner_ty = self.check_expr(inner, Some(expected));
        self.check_coercion(inner.span, expected, inner_ty);
        expected
    }

    // ==========================================
    //            ADT & Pattern Matching
    // ==========================================

    fn check_data_literal(
        &mut self,
        kind: &ast::DataLiteralKind,
        expected: TypeId,
        span: Span,
    ) -> TypeId {
        let exp_norm = self.resolve_tv(expected);
        let kind_enum = self.ctx.type_registry.get(exp_norm).clone();

        // 拦截 Trait Object 构造
        if let TypeKind::TraitObject(..) = kind_enum {
            if let ast::DataLiteralKind::Scalar(inner) = kind {
                return self.check_trait_object_init(inner, expected, exp_norm, span);
            } else {
                self.ctx
                    .struct_error(
                        span,
                        "trait objects must be initialized with a single pointer",
                    )
                    .with_hint("example: `Reader.{ file_ptr }`")
                    .emit();
                return TypeId::ERROR;
            }
        }
        
        // 🌟 统一识别 Data 类型
        let is_data = matches!(kind_enum, TypeKind::Data(..));

        match kind {
            ast::DataLiteralKind::Array(elems) => {
                let is_target_array_like = matches!(
                    kind_enum,
                    TypeKind::Array { .. } | TypeKind::ArrayInfer{ .. } | TypeKind::Slice{ .. }
                );
                if elems.is_empty() && !is_target_array_like {
                    if is_data {
                        self.check_data_payload_literal(&[], expected, exp_norm, span)
                    } else {
                        self.check_struct_or_union_literal(&[], expected, exp_norm, span)
                    }
                } else {
                    self.check_array_literal(elems, expected, exp_norm, span)
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.check_repeat_literal(value, count, expected, exp_norm, span)
            }
            ast::DataLiteralKind::Struct(init_fields) => {
                if is_data {
                    // 🌟 重命名后的带载荷的 Data 初始化
                    self.check_data_payload_literal(init_fields, expected, exp_norm, span)
                } else {
                    self.check_struct_or_union_literal(init_fields, expected, exp_norm, span)
                }
            }
            ast::DataLiteralKind::Scalar(inner) => {
                if is_data {
                    // 🌟 核心融合：如果是 `.{ None }` 形式，直接提取出变体名，复用简写逻辑！
                    if let ExprKind::Identifier(variant_name) = &inner.kind {
                        self.check_enum_literal(*variant_name, Some(expected), inner.span)
                    } else {
                        self.ctx.struct_error(inner.span, "expected a simple variant name for data literal").emit();
                        TypeId::ERROR
                    }
                } else {
                    self.check_scalar_literal(inner, expected)
                }
            }
        }
    }

    /// 统一处理 `.Variant` 简写和 `.{ Variant }` 无负载初始化的校验
    fn check_enum_literal(
        &mut self,
        variant_name: crate::utils::SymbolId,
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut res_ty = TypeId::ERROR;
        if let Some(exp_ty) = expected_ty {
            let norm_exp = self.resolve_tv(exp_ty);
            if let TypeKind::Data(def_id, _) = self.ctx.type_registry.get(norm_exp) {
                if let Def::Data(d) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(v) = d.variants.iter().find(|v| v.name == variant_name) {
                        // 如果有 payload，必须使用 Struct() 初始化，不能用这种标量形式
                        if v.payload_type.is_some() {
                            let v_str = self.ctx.resolve(variant_name).to_string();
                            self.ctx.struct_error(span, format!("variant `{}` requires a payload", v_str))
                                .with_hint(format!("initialize it as `.{{ {}: value }}`", v_str))
                                .emit();
                        } else {
                            res_ty = exp_ty;
                        }
                    } else {
                        let v_str = self.ctx.resolve(variant_name).to_string();
                        let exp_str = self.ctx.ty_to_string(norm_exp);
                        let available_variants: Vec<String> = d.variants.iter()
                            .map(|v| format!(".{}", self.ctx.resolve(v.name)))
                            .collect();
                        let mut diag = self.ctx.struct_error(span, format!("variant `.{}` does not exist in the expected data type", v_str))
                            .with_hint(format!("expected data type is `{}`", exp_str));

                        if !available_variants.is_empty() {
                            diag = diag.with_hint(format!("available variants: {}", available_variants.join(", ")));
                        }
                        diag.emit();
                    }
                }
            } else if norm_exp != TypeId::ERROR {
                let exp_str = self.ctx.ty_to_string(norm_exp);
                self.ctx.struct_error(span, "expected a data/enum type for variant literal")
                    .with_hint(format!("but context expects `{}`", exp_str))
                    .emit();
            }
        } else {
            self.ctx.struct_error(span, "cannot infer data type for variant literal without context")
                .with_hint("try prepending the type name, e.g., `Result.Ok` instead of `.Ok`")
                .emit();
        }
        res_ty
    }

    /// 专门处理带有负载的 Data 初始化，例如 `Result.{ Ok: 10 }`
    fn check_data_payload_literal(
        &mut self,
        init_fields: &[ast::StructFieldInit],
        expected: TypeId,
        exp_norm: TypeId,
        span: Span,
    ) -> TypeId {
        let (def_id, generic_args) =
            if let TypeKind::Data(id, args) = self.ctx.type_registry.get(exp_norm) {
                (*id, args.clone())
            } else {
                unreachable!()
            };

        let data_def = match &self.ctx.defs[def_id.0 as usize] {
            Def::Data(d) => d.clone(),
            _ => unreachable!(),
        };

        if init_fields.len() != 1 {
            self.ctx.struct_error(span, "Data literal must specify exactly one variant").emit();
            return TypeId::ERROR;
        }

        let init_f = &init_fields[0];
        let variant = data_def.variants.iter().find(|v| v.name == init_f.name);

        if let Some(v) = variant {
            if let Some(payload_ast) = &v.payload_type {
                let mut payload_ty = self.ctx.node_types.get(&payload_ast.id).copied().unwrap_or(TypeId::ERROR);

                if !data_def.generics.is_empty() && !generic_args.is_empty() {
                    let mut map = HashMap::new();
                    for (i, param) in data_def.generics.iter().enumerate() {
                        map.insert(param.name, generic_args[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    payload_ty = subst.substitute(payload_ty);
                }

                let val_ty = self.check_expr(&init_f.value, Some(payload_ty));
                self.check_coercion(init_f.span, payload_ty, val_ty);
            } else {
                let v_str = self.ctx.resolve(v.name).to_string();
                self.ctx.struct_error(init_f.span, format!("variant `{}` does not take a payload", v_str))
                    .with_hint(format!("initialize it as `.{{ {} }}` instead", v_str))
                    .emit();
            }
        } else {
            let v_str = self.ctx.resolve(init_f.name);
            let data_str = self.ctx.resolve(data_def.name);
            self.ctx.struct_error(init_f.span, format!("variant `{}` not found in data type `{}`", v_str, data_str)).emit();
        }

        expected
    }

    fn check_trait_object_init(
        &mut self,
        inner: &Expr,
        expected: TypeId,
        exp_norm: TypeId, // 这里 exp_norm 是 TraitObject 本身
        span: Span,
    ) -> TypeId {
        let inner_ty = self.check_expr(inner, None);
        if inner_ty == TypeId::ERROR { return TypeId::ERROR; }

        let is_inner_mut = self.is_mutable_pointer(inner_ty);
        let inner_ty_id = self.resolve_tv(inner_ty);
        let is_inner_ptr = matches!(
            self.ctx.type_registry.get(inner_ty_id),
            TypeKind::Pointer{ .. } | TypeKind::VolatilePtr{ .. }
        );

        if !is_inner_ptr {
            self.ctx.struct_error(inner.span, "trait objects can only be constructed from pointers").emit();
            return TypeId::ERROR;
        }

        if !self.check_trait_impl(inner_ty, exp_norm) {
            self.ctx.struct_error(span, "the provided pointer type does not implement the target trait").emit();
            return TypeId::ERROR;
        }

        // 将底层的 TraitObject 包上指针，并将传入指针的 is_mut 原样传递
        self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: is_inner_mut,
            elem: expected, // 保留原有的 TraitObject ID
        })
    }

    /// 核心 Match 检查逻辑：环境提取与详尽性检查
    fn check_match_expr(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let norm_target = self.resolve_tv(target_ty);

        if norm_target == TypeId::ERROR {
            for arm in arms { self.check_expr(&arm.body, None); }
            return TypeId::ERROR;
        }

        let (def_id, generic_args) =
            if let TypeKind::Data(id, args) = self.ctx.type_registry.get(norm_target) {
                (*id, args.clone())
            } else {
                self.ctx.struct_error(target.span, "match expression target must be an ADT").emit();
                for arm in arms { self.check_expr(&arm.body, None); }
                return TypeId::ERROR;
            };

        let adt_def = match &self.ctx.defs[def_id.0 as usize] {
            Def::Data(a) => a.clone(),
            _ => unreachable!(),
        };

        let mut common_ret_ty = expected_ty;
        let mut handled_variants = std::collections::HashSet::new();
        let mut has_catch_all = false;

        for arm in arms {
            let body_ty = self.check_match_arm(
                arm, &adt_def, &generic_args, norm_target, common_ret_ty,
                &mut handled_variants, &mut has_catch_all
            );

            if common_ret_ty.is_none() {
                common_ret_ty = Some(body_ty);
            } else {
                self.check_coercion(arm.body.span, common_ret_ty.unwrap(), body_ty);
            }
        }

        // 详尽性检查
        if !has_catch_all {
            let missing: Vec<_> = adt_def.variants.iter()
                .filter(|v| !handled_variants.contains(&v.name))
                .map(|v| self.ctx.resolve(v.name).to_string())
                .collect();

            if !missing.is_empty() {
                self.ctx.struct_error(span, "match expression is not exhaustive")
                    .with_hint(format!("missing variants: {}", missing.join(", ")))
                    .emit();
            }
        }

        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    /// 单独抽离的分支检查逻辑
    fn check_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        adt_def: &crate::sema::def::DataDef,
        generic_args: &[TypeId],
        norm_target: TypeId,
        common_ret_ty: Option<TypeId>,
        handled_variants: &mut std::collections::HashSet<crate::utils::SymbolId>,
        has_catch_all: &mut bool,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();

        match &arm.pattern {
            ast::MatchPattern::Variant { target_type, variant_name, binding, span: pat_span } => {
                if let Some(explicit_ty_ast) = target_type {
                    let mut resolver = TypeResolver::new(self.ctx);
                    let scope = resolver.ctx.scopes.current_scope_id().unwrap();
                    let explicit_ty = resolver.resolve_type(explicit_ty_ast, scope);
                    self.check_coercion(*pat_span, norm_target, explicit_ty);
                }

                if let Some(v) = adt_def.variants.iter().find(|v| v.name == *variant_name) {
                    handled_variants.insert(*variant_name);

                    if let Some(bind_pattern) = binding {
                        if let Some(payload_ast) = &v.payload_type {
                            let mut payload_ty = self.ctx.node_types.get(&payload_ast.id).copied().unwrap_or(TypeId::ERROR);

                            if !adt_def.generics.is_empty() && !generic_args.is_empty() {
                                let mut map = std::collections::HashMap::new();
                                for (i, param) in adt_def.generics.iter().enumerate() {
                                    map.insert(param.name, generic_args[i]);
                                }
                                let mut subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &map);
                                payload_ty = subst.substitute(payload_ty);
                            }

                            // 注册变量，支持 `let mut` 模式
                            let info = SymbolInfo {
                                kind: SymbolKind::Var,
                                node_id: arm.body.id,
                                type_id: payload_ty,
                                def_id: None,
                                span: *pat_span,
                                is_pub: false,
                                is_mut: bind_pattern.is_mut,
                            };
                            let _ = self.ctx.scopes.define(bind_pattern.name, info);
                        } else {
                            self.ctx.struct_error(*pat_span, format!("variant `{}` has no payload", self.ctx.resolve(*variant_name))).emit();
                        }
                    } else if v.payload_type.is_some() {
                        self.ctx.struct_error(*pat_span, format!("variant `{}` requires a binding for its payload", self.ctx.resolve(*variant_name))).emit();
                    }
                } else {
                    self.ctx.struct_error(*pat_span, "variant not found in ADT").emit();
                }
            }
            ast::MatchPattern::CatchAll(_) => {
                *has_catch_all = true;
            }
        }

        let body_ty = self.check_expr(&arm.body, common_ret_ty);
        self.ctx.scopes.exit_scope();
        body_ty
    }

    /// 循环并找出类型变量 `?T` 最终绑定的真实类型
    pub fn resolve_tv(&mut self, ty: TypeId) -> TypeId {
        let mut curr = ty;
        loop {
            let norm = self.ctx.type_registry.normalize(curr);
            if let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(norm) {
                if let Some(target) = self.type_vars[*vid as usize] {
                    curr = target;
                } else {
                    return norm; // 没被推导出来，原样返回 `?T`
                }
            } else {
                return norm;
            }
        }
    }

    /// 左值 (LValue) 可变性推导
    pub fn is_lvalue_mutable(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    info.is_mut // 普通变量：取决于是不是 `let mut a` 定义的
                } else { false }
            }
            ExprKind::Unary { op: UnaryOperator::PointerDeRef, operand } => {
                let ptr_ty = self.check_expr(operand, None);
                let norm = self.resolve_tv(ptr_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => *is_mut,
                    _ => false,
                }
            }
            ExprKind::FieldAccess { lhs, .. } | ExprKind::IndexAccess { lhs, .. } => {
                // 检查 lhs 的真实类型。如果是指针或切片，说明发生了自动解引用或索引。
                // 此时的可变性取决于指针/切片本身，而不是包裹它的变量
                let lhs_ty = self.check_expr(lhs, None);
                let norm_lhs = self.resolve_tv(lhs_ty);
                
                match self.ctx.type_registry.get(norm_lhs).clone() {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        is_mut // 指针的自动解引用 (e.g. ptr.field)
                    }
                    TypeKind::Slice { is_mut, .. } => {
                        is_mut // 切片索引 (e.g. slice.[0])
                    }
                    _ => {
                        // 普通的值类型，严格继承父级左值的可变性
                        self.is_lvalue_mutable(lhs)
                    }
                }
            }
            // SliceOp (如 a.[0..2]) 产生的是一个新的切片视图值（右值），
            // 在 Rust 的语义中，它只有赋值给变量或传递给函数时才有意义。
            // 它的 is_mut 只代表切片本身的类型属性。
            ExprKind::SliceOp { is_mut, .. } => *is_mut,
            _ => false,
        }
    }

    fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let mut visited = std::collections::HashSet::new();
        self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited)
    }

    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<crate::sema::ty::DefId>,
    ) -> bool {
        let mut impl_blocks = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                impl_blocks.push(impl_def.clone());
            }
        }

        for impl_def in impl_blocks {
            if let Some(trait_ast) = &impl_def.trait_type {
                let impl_target_ty = self
                    .ctx
                    .node_types
                    .get(&impl_def.target_type.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let impl_trait_ty = self
                    .ctx
                    .node_types
                    .get(&trait_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                    continue;
                }

                let mut map = HashMap::new();

                if self.unify(impl_target_ty, source_ty, &mut map) {
                    let instantiated_trait_ty = {
                        let mut subst = Substituter::new(
                            &mut self.ctx.type_registry,
                            &map,
                        );
                        subst.substitute(impl_trait_ty)
                    };

                    let inst_norm = self.resolve_tv(instantiated_trait_ty);
                    let target_norm = self.resolve_tv(target_trait_ty);

                    if inst_norm == target_norm || instantiated_trait_ty == target_trait_ty {
                        return true;
                    }

                    let inst_norm = self.resolve_tv(instantiated_trait_ty);
                    if let TypeKind::TraitObject(inst_def_id, _) =
                        self.ctx.type_registry.get(inst_norm)
                    {
                        if visited.insert(*inst_def_id) {
                            if let Def::Trait(trait_def) =
                                self.ctx.defs[inst_def_id.0 as usize].clone()
                            {
                                for supertrait_ast in &trait_def.supertraits {
                                    let super_ty = self
                                        .ctx
                                        .node_types
                                        .get(&supertrait_ast.id)
                                        .copied()
                                        .unwrap_or(TypeId::ERROR);
                                    let inst_super_ty = {
                                        let mut subst =
                                            Substituter::new(
                                                &mut self.ctx.type_registry,
                                                &map,
                                            );
                                        subst.substitute(super_ty)
                                    };

                                    if inst_super_ty == target_trait_ty
                                        || self.check_trait_impl_inner(
                                            source_ty,
                                            inst_super_ty,
                                            visited,
                                        )
                                    {
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

    fn check_switch_exhaustiveness(
        &mut self,
        target_ty: TypeId,
        cases: &[ast::SwitchCase],
        has_default: bool,
        span: Span,
    ) {
        if has_default {
            return;
        }

        let norm_target = self.resolve_tv(target_ty);
        if let TypeKind::Data(def_id, _) = self.ctx.type_registry.get(norm_target) {
            if let Def::Data(d) = &self.ctx.defs[def_id.0 as usize] {
                let mut unhandled_variants: std::collections::HashSet<crate::utils::SymbolId> =
                    d.variants.iter().map(|v| v.name).collect();

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
                    let missing: Vec<String> = unhandled_variants
                        .into_iter()
                        .map(|id| self.ctx.resolve(id).to_string())
                        .collect();
                    self.ctx
                        .struct_error(span, "switch expression is not exhaustive")
                        .with_hint(format!("missing data variants: {}", missing.join(", ")))
                        .emit();
                }
                return;
            }
        }

        self.ctx
            .struct_error(span, "switch expression must be exhaustive")
            .with_hint("consider adding an `else =>` catch-all branch")
            .emit();
    }

    fn unify(
        &mut self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        map: &mut std::collections::HashMap<crate::utils::SymbolId, TypeId>,
    ) -> bool {
        let gen_norm = self.resolve_tv(generic_ty);
        let con_norm = self.resolve_tv(concrete_ty);

        let gen_kind = self.ctx.type_registry.get(gen_norm).clone();
        let con_kind = self.ctx.type_registry.get(con_norm).clone();

        match (gen_kind, con_kind) {
            (TypeKind::Param(name), _) => {
                if let Some(&existing_ty) = map.get(&name) {
                    existing_ty == concrete_ty
                } else {
                    map.insert(name, concrete_ty);
                    true
                }
            }
            // 指针和切片的 Unify 必须同时匹配其 mut 属性
            (TypeKind::Pointer { is_mut: g_m, elem: g_e }, TypeKind::Pointer { is_mut: c_m, elem: c_e }) => {
                g_m == c_m && self.unify(g_e, c_e, map)
            }
            (TypeKind::VolatilePtr { is_mut: g_m, elem: g_e }, TypeKind::VolatilePtr { is_mut: c_m, elem: c_e }) => {
                g_m == c_m && self.unify(g_e, c_e, map)
            }
            (TypeKind::Slice { is_mut: g_m, elem: g_e }, TypeKind::Slice { is_mut: c_m, elem: c_e }) => {
                g_m == c_m && self.unify(g_e, c_e, map)
            }
            (TypeKind::Array { is_mut: g_m, elem: g_e, len: g_l }, TypeKind::Array { is_mut: c_m, elem: c_e, len: c_l }) => {
                g_m == c_m && g_l == c_l && self.unify(g_e, c_e, map)
            }
            (TypeKind::ArrayInfer { is_mut: g_m, elem: g_e }, TypeKind::ArrayInfer { is_mut: c_m, elem: c_e }) => {
                g_m == c_m && self.unify(g_e, c_e, map)
            }
            
            (TypeKind::Def(g_id, g_args), TypeKind::Def(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() { return false; }
                g_args.iter().zip(c_args.iter()).all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            (TypeKind::Data(g_id, g_args), TypeKind::Data(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() { return false; }
                g_args.iter().zip(c_args.iter()).all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            (TypeKind::TraitObject(g_id, g_args), TypeKind::TraitObject(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() { return false; }
                g_args.iter().zip(c_args.iter()).all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            _ => gen_norm == con_norm,
        }
    }

    // ==========================================
    //            Inline Assembly (@asm)
    // ==========================================

    /// 专门校验 @asm(.{ ... }) 结构
    fn check_asm_call(&mut self, args: &[Expr], span: Span) -> TypeId {
        if args.len() != 1 {
            self.ctx
                .struct_error(span, "`@asm` expects exactly one anonymous struct argument")
                .with_hint("example: `@asm(.{ asm: \"nop\", volatile: true })`")
                .emit();
            return TypeId::ERROR;
        }

        let config_arg = &args[0];
        let fields = match &config_arg.kind {
            ExprKind::DataInit {
                literal: ast::DataLiteralKind::Struct(f),
                type_node: None,
            } => f,
            _ => {
                self.ctx
                    .struct_error(
                        config_arg.span,
                        "`@asm` argument must be an untyped anonymous struct `.{ ... }`",
                    )
                    .emit();
                // 继续推导内部可能的错误以防止级联，但标记外层为 ERROR
                self.check_expr(config_arg, None);
                return TypeId::ERROR;
            }
        };

        let mut has_asm = false;

        for field in fields {
            let field_name = self.ctx.resolve(field.name).to_string();
            match field_name.as_str() {
                "asm" => {
                    has_asm = true;
                    match &field.value.kind {
                        ExprKind::String(_) => {
                            self.check_expr(&field.value, None);
                        }
                        ExprKind::DataInit {
                            literal: ast::DataLiteralKind::Array(elems),
                            ..
                        } => {
                            for e in elems {
                                if !matches!(e.kind, ExprKind::String(_)) {
                                    self.ctx
                                        .struct_error(
                                            e.span,
                                            "all elements in asm array must be string literals",
                                        )
                                        .emit();
                                }
                                self.check_expr(e, None);
                            }
                        }
                        _ => {
                            self.ctx.struct_error(field.value.span, "`asm` template must be a string literal or an array of strings").emit();
                        }
                    }
                }
                "outputs" | "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg_field in regs {
                            let val_ty = self.check_expr(&reg_field.value, None);
                            let val_ty_str = self.ctx.ty_to_string(val_ty);

                            if field_name == "outputs" && val_ty != TypeId::ERROR {
                                if !self.is_mut_pointer(val_ty) {
                                    self.ctx.struct_error(reg_field.value.span, "inline assembly outputs must be bound to mutable pointers (e.g., `status.&`)")
                                        .with_hint(format!("type found: {}", val_ty_str))
                                        .emit();
                                }
                            }
                        }
                    } else {
                        self.ctx.struct_error(field.value.span, format!("`{}` must be an anonymous struct mapping registers to variables", field_name)).emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(clobbers),
                        ..
                    } = &field.value.kind
                    {
                        for c in clobbers {
                            if !matches!(c.kind, ExprKind::String(_)) {
                                self.ctx.struct_error(c.span, "clobbers must be a list of string literals (e.g., `.{ \"memory\", \"cc\" }`)").emit();
                            }
                            self.check_expr(c, None);
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                field.value.span,
                                "`clobbers` must be a slice/array of strings",
                            )
                            .emit();
                        self.check_expr(&field.value, None);
                    }
                }
                "volatile" => {
                    let ty = self.check_expr(&field.value, Some(TypeId::BOOL));
                    self.check_coercion(field.value.span, TypeId::BOOL, ty);
                }
                _ => {
                    self.ctx
                        .struct_error(
                            field.span,
                            format!("unknown field `{}` in `@asm` configuration", field_name),
                        )
                        .emit();
                    self.check_expr(&field.value, None);
                }
            }
        }

        if !has_asm {
            self.ctx
                .struct_error(
                    span,
                    "`@asm` configuration is missing the required `asm` template string",
                )
                .emit();
        }

        // 绑定 config_arg 的类型为 VOID，防止 AST 树产生洞
        self.ctx.node_types.insert(config_arg.id, TypeId::VOID);

        // 内联汇编不返回值，通过 outputs 的指针写入状态
        TypeId::VOID
    }

    /// 辅助方法：判断内联汇编 output 绑定的类型是否为可变指针 (`*mut T` 或 `^mut T`)
    fn is_mut_pointer(&mut self, ty: TypeId) -> bool { 
        let norm = self.resolve_tv(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                is_mut 
            }
            _ => false,
        }
    }
}
