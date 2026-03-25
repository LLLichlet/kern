use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_utils::Span;
use std::collections::HashMap;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> TypeId {
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
        let sig_kind = self.ctx.type_registry.get(sig_ty).clone();

        // 提取调用参数、返回值和可变参数标志
        let (params, ret, is_variadic) = match sig_kind {
            // A. 普通函数
            TypeKind::Function { params, ret, is_variadic } => (params, ret, is_variadic),
            
            // B. 闭包胖指针 (*Fn)
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let inner_norm = self.ctx.type_registry.normalize(elem);
                if let TypeKind::ClosureInterface { params, ret } = self.ctx.type_registry.get(inner_norm).clone() {
                    (params, ret, false) // 闭包不支持可变参数
                } else {
                    let callee_str = self.ctx.ty_to_string(callee_ty);
                    self.ctx.struct_error(callee.span, "expression is not callable")
                        .with_hint(format!("type is `{}`", callee_str))
                        .emit();
                    return TypeId::ERROR;
                }
            }
            
            // C. 其它类型一律不准调用
            _ => {
                let callee_str = self.ctx.ty_to_string(callee_ty);
                self.ctx.struct_error(callee.span, "expression is not callable")
                    .with_hint(format!("type is `{}`", callee_str))
                    .emit();
                return TypeId::ERROR;
            }
        };

        self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

        if is_method && !params.is_empty() {
            let expected_self = params[0];
            self.check_method_receiver(expected_self, receiver_ty, callee);
            if receiver_ty != expected_self {
                if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
                    self.ctx.node_types.insert(lhs.id, expected_self);
                }
            }
        }

        self.check_call_arguments(args, &params, is_method, is_variadic);
        return ret;
    }

    /// 助手：智能泛型推导与签名解析
    pub(crate) fn deduce_and_resolve_signature(
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
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
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
                let mut stripped_recv = self.resolve_tv(receiver_ty);
                let expected_recv = self.resolve_tv(raw_params[0]);
                if let TypeKind::Pointer { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                {
                    if let TypeKind::Pointer { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                    {
                        stripped_recv = self.ctx.type_registry.intern(TypeKind::Pointer {
                            is_mut: false,
                            elem,
                        });
                    }
                } else if let TypeKind::VolatilePtr { is_mut: false, .. } =
                    self.ctx.type_registry.get(expected_recv)
                {
                    if let TypeKind::VolatilePtr { is_mut: true, elem } =
                        self.ctx.type_registry.get(stripped_recv).clone()
                    {
                        stripped_recv = self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                            is_mut: false,
                            elem,
                        });
                    }
                }

                self.unify(expected_recv, stripped_recv, &mut map);
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

            self.check_generic_bounds(span, def_id, &generics, &resolved_args);

            // 构造包含具体参数的 FnDef 类型，以便稍后写入 AST
            let inferred_callee_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::FnDef(def_id, resolved_args));

            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            return (subst.substitute(raw_sig), Some(inferred_callee_ty));
        }

        (norm_callee, None)
    }

    /// 助手 2：判断这是否是一个方法调用，如果是，提取它的 Receiver 类型 (LHS)
    pub(crate) fn resolve_method_context(&self, callee: &Expr) -> (bool, TypeId) {
        if let ExprKind::FieldAccess { lhs, .. } = &callee.kind {
            // 使用类型来判断 lhs 是否为模块
            let callee_node_ty = self
                .ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let lhs_node_ty = self
                .ctx
                .node_types
                .get(&lhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let norm_lhs = self.ctx.type_registry.normalize(lhs_node_ty);

            // 如果 lhs 解析出是一个模块，显然它不是面向类型的方法调用
            if matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(..)) {
                return (false, TypeId::ERROR);
            }

            let norm_node_ty = self.ctx.type_registry.normalize(callee_node_ty);

            if matches!(
                self.ctx.type_registry.get(norm_node_ty),
                TypeKind::FnDef(..) | TypeKind::Function { .. }
            ) {
                return (true, lhs_node_ty);
            }
        }
        (false, TypeId::ERROR)
    }

    /// 助手 3：校验参数个数 (Arity)
    pub(crate) fn check_call_arity(
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
    fn check_method_receiver(&mut self, expected_self: TypeId, receiver_ty: TypeId, expr: &Expr) {
        let norm_expected = self.resolve_tv(expected_self);

        if !self.check_coercion(expr, expected_self, receiver_ty) {
            let is_exp_ptr = matches!(
                self.ctx.type_registry.get(norm_expected),
                TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
            );

            if is_exp_ptr {
                self.ctx.struct_error(expr.span, "method receiver type mismatch")
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
                self.check_coercion(arg, params[sig_param_idx], arg_ty);
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

    pub(crate) fn check_generic_instantiation(
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
            TypeKind::Enum(id, args) => (*id, args.clone()),
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

        self.check_generic_bounds(span, def_id, &generics, &arg_tys);

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

    fn check_generic_bounds(
        &mut self,
        span: Span,
        def_id: DefId,
        generics: &[ast::GenericParam],
        arg_tys: &[TypeId],
    ) {
        // 1. 提取实体的 Where 子句
        let where_clauses = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => f.where_clauses.clone(),
            Def::Struct(s) => s.where_clauses.clone(),
            Def::Union(u) => u.where_clauses.clone(),
            Def::TypeAlias(t) => t.where_clauses.clone(),
            Def::Impl(i) => i.where_clauses.clone(),
            Def::Enum(e) => e.where_clauses.clone(),
            Def::Trait(t) => t.where_clauses.clone(),
            _ => return,
        };

        if where_clauses.is_empty() {
            return;
        }

        // 2. 构建泛型实参映射表 (T -> Allocator)
        let mut map = std::collections::HashMap::new();
        for (i, param) in generics.iter().enumerate() {
            if i < arg_tys.len() {
                map.insert(param.name, arg_tys[i]);
            }
        }

        // 3. 收集需要检查的类型对
        let mut pairs_to_check = Vec::new();
        {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);

            for clause in where_clauses {
                let original_target = self
                    .ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_target = subst.substitute(original_target);

                for bound_ast in clause.bounds {
                    let original_bound = self
                        .ctx
                        .node_types
                        .get(&bound_ast.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let sub_bound = subst.substitute(original_bound);

                    pairs_to_check.push((sub_target, sub_bound));
                }
            }
        }

        // 4. 执行特征检查
        for (sub_target, sub_bound) in pairs_to_check {
            if sub_target != TypeId::ERROR && sub_bound != TypeId::ERROR {
                if !self.check_trait_impl(sub_target, sub_bound) {
                    let req_str = self.ctx.ty_to_string(sub_bound);
                    let act_str = self.ctx.ty_to_string(sub_target);
                    self.ctx
                        .struct_error(span, "type does not satisfy trait bounds")
                        .with_hint(format!("required bound: `{}: {}`", act_str, req_str))
                        .emit();
                }
            }
        }
    }

    pub(crate) fn check_closure(
        &mut self,
        node_id: kernc_utils::NodeId,
        captures: &[ast::CapturePattern],
        params: &[ast::FuncParam],
        ast_ret_ty: &ast::TypeNode,
        body: &ast::Expr,
        span: Span,
    ) -> TypeId {
        // 推导所有的捕获表达式
        let mut state_fields = Vec::new();
        let mut capture_env = Vec::new();

        for cap in captures {
            let cap_ty = self.check_expr(&cap.value, None);
            state_fields.push(cap_ty);
            capture_env.push((cap.name, cap_ty, cap.span));
        }
        
        let current_scope = match self.ctx.scopes.current_scope_id() {
            Some(id) => id,
            None => {
                self.ctx.emit_ice(span, "Compiler Bug: Closure evaluated outside of any active scope");
                crate::scope::ScopeId(0)
            }
        };

        // 在父作用域解析参数类型和返回类型 
        // (类型签名必须在外部环境解析，因为可能会用到外部引入的别名)
        let mut param_tys = Vec::new();
        let mut type_resolver = TypeResolver::new(self.ctx);
        for param in params {
            let p_ty = type_resolver.resolve_type(&param.type_node, current_scope);
            param_tys.push(p_ty);
        }
        let expected_ret = type_resolver.resolve_type(ast_ret_ty, current_scope);
        
        drop(type_resolver);

        let closure_state_ty = self.ctx.type_registry.intern(TypeKind::AnonymousState { 
            closure_node_id: node_id,
            captures: state_fields, 
            params: param_tys.clone(), 
            ret: expected_ret 
        });

        // 进入闭包内部的作用域
        let _ = self.ctx.scopes.enter_scope();

        // 将捕获值注入闭包作用域 (Pure Value Semantics，强制不可变)
        for (name, ty, cap_span) in capture_env {
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id, // 捕获环境暂时借用闭包表达式的 ID
                type_id: ty,
                def_id: None,
                span: cap_span,
                is_pub: false,
                is_mut: false,
            };
            let _ = self.ctx.scopes.define(name, info);
        }

        // 注入闭包参数
        for (i, param) in params.iter().enumerate() {
            let param_node_id = self.ctx.next_node_id(); 
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id: param_node_id,
                type_id: param_tys[i],
                def_id: None,
                span: param.span,
                is_pub: false,
                is_mut: param.pattern.is_mut, 
            };
            let _ = self.ctx.scopes.define(param.pattern.name, info);
        }

        // 推导函数体
        let (actual_ret_ty, has_returned) = {
            let mut sub_checker = ExprChecker::new(self.ctx, Some(expected_ret));
            let ty = sub_checker.check_expr(body, Some(expected_ret));
            (ty, sub_checker.has_returned)
        };

        // 类型兼容性校验
        if actual_ret_ty != TypeId::ERROR && expected_ret != TypeId::ERROR && actual_ret_ty != TypeId::NEVER {
            let norm_actual = self.ctx.type_registry.normalize(actual_ret_ty);
            let norm_expected = self.ctx.type_registry.normalize(expected_ret);
            
            // 如果实际返回是 VOID，但预期不是 VOID，说明缺少了尾随表达式
            let is_missing_tail = norm_actual == TypeId::VOID && norm_expected != TypeId::VOID;
            
            // 如果缺少尾随表达式，但函数内部有合法的 `return` 语句，暂时放行
            if is_missing_tail && has_returned {
                // Safe: 至少有一条路径合法返回了
            } else if norm_actual != norm_expected {
                let expected_str = self.ctx.ty_to_string(expected_ret);
                let actual_str = self.ctx.ty_to_string(actual_ret_ty);
                
                self.ctx.struct_error(
                    body.span, 
                    format!("closure body evaluates to `{}`, but signature expects `{}`", actual_str, expected_str)
                )
                .with_hint("ensure the final expression or return statements match the explicit return type")
                .emit();
            }
        }

        // 9. 退出作用域并记录
        self.ctx.scopes.exit_scope();
        self.ctx.node_types.insert(node_id, closure_state_ty);
        
        closure_state_ty
    }

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
                                    self.ctx.struct_error(reg_field.value.span, "inline assembly outputs must be bound to mutable pointers (e.g., `status..&`)")
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
                    self.check_coercion(&field.value, TypeId::BOOL, ty);
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
            TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => is_mut,
            _ => false,
        }
    }
}
