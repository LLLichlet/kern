mod constexpr;
mod expr;
mod subst;

pub use constexpr::{ConstEvaluator, ConstValue};
pub(crate) use expr::ExprChecker;
pub use subst::Substituter;

use crate::context::SemaContext;
use crate::def::{Def, FunctionDef, ImplDef};
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast as ast;

/// 类型检查的主驱动器
pub struct TypeckDriver<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> TypeckDriver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn check_all(&mut self) {
        let defs_clone = self.ctx.defs.clone();

        // === 阶段 1：全局常量依赖解析 ===
        let mut globals = Vec::new();
        for def in &defs_clone {
            if let Def::Module(m) = def {
                for item_id in &m.items {
                    if matches!(self.ctx.defs[item_id.0 as usize], Def::Global(_)) {
                        globals.push((*item_id, m.scope_id));
                    }
                }
            }
        }

        let mut changed = true;
        let mut max_iters = 100; // 防止真正的循环依赖导致死循环
        let mut resolved_globals = std::collections::HashSet::new();

        while changed && max_iters > 0 {
            changed = false;
            max_iters -= 1;

            for &(item_id, scope_id) in &globals {
                if resolved_globals.contains(&item_id) {
                    continue; // 已经成功推导的跳过
                }

                let g = if let Def::Global(g) = &self.ctx.defs[item_id.0 as usize] {
                    g.clone()
                } else {
                    unreachable!()
                };

                // 备份当前的报错状态。如果推导失败，静默回滚，不污染终端。
                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types.clone();

                // 尝试推导
                self.ctx.scopes.set_current_scope(scope_id);
                let mut checker = ExprChecker::new(self.ctx, None);
                let init_ty = checker.check_expr(&g.value, None);

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;
                    
                    if self.ctx.scopes.resolve_local(g.name).is_some() {
                        self.ctx.scopes.update_type(g.name, init_ty);
                    }

                    // 既然类型正确了，顺便执行 ConstEval 常量折叠校验
                    if !g.is_extern {
                        if let ast::ExprKind::Undef = g.value.kind {
                            self.ctx.emit_error(g.span, "Global variables cannot be initialized with bare `undef`. Must provide a typed constant value (e.g., `.{undef}`).");
                        } else {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(&g.value, 0);
                        }
                    } else {
                        if !matches!(g.value.kind, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(ref inner), .. } if matches!(inner.kind, ast::ExprKind::Undef)) {
                            self.ctx.emit_error(g.span, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
                        }
                    }
                } else {
                    // 推导失败，回滚报错，等待下一轮迭代
                    self.ctx.sess.error_count = old_err_cnt;
                    self.ctx.sess.diagnostics.truncate(old_diag_len);
                    self.ctx.node_types = old_node_types;
                }
            }
        }

        // 阶段 2：兜底 
        // 如果循环结束了，但还有没推导出来的全局常量，说明要么是死循环依赖，要么是有真正的语法/类型错误。
        // 再强制跑一次
        if resolved_globals.len() < globals.len() {
            for &(item_id, scope_id) in &globals {
                if !resolved_globals.contains(&item_id) {
                    let g = if let Def::Global(g) = &self.ctx.defs[item_id.0 as usize] {
                        g.clone()
                    } else {
                        unreachable!()
                    };

                    self.ctx.scopes.set_current_scope(scope_id);
                    let mut checker = ExprChecker::new(self.ctx, None);
                    checker.check_expr(&g.value, None); // 这里的报错会直接输出到终端

                    self.ctx.struct_error(g.span, format!("cannot resolve global constant `{}`", self.ctx.resolve(g.name)))
                        .with_hint("this is usually caused by a circular dependency (e.g., A depends on B, and B depends on A) or an undefined variable")
                        .emit();
                }
            }
        }

        // === 阶段 3：常规实体检查 (函数、Impl等) ===
        for def in defs_clone {
            if let Def::Module(m) = def {
                self.ctx.scopes.set_current_scope(m.scope_id);
                for item_id in m.items {
                    let d = &self.ctx.defs[item_id.0 as usize];
                    if !matches!(d, Def::Global(_)) { // 跳过已经处理完的全局常量
                        self.check_item(item_id, m.scope_id);
                    }
                }
            }
        }
    }

    fn check_item(&mut self, id: crate::def::DefId, parent_scope: ScopeId) {
        let def = self.ctx.defs[id.0 as usize].clone();

        match def {
            Def::Function(f) => self.check_function(&f, parent_scope),
            Def::Impl(i) => self.check_impl(&i, parent_scope),
            Def::Struct(s) => self.check_struct(&s, parent_scope),
            Def::Union(u) => self.check_union(&u, parent_scope),
            _ => {} 
        }
    }

    // ==========================================
    //          Item Checkers
    // ==========================================

    fn check_function(&mut self, f: &FunctionDef, parent_scope: ScopeId) {
        // 1. 验证 Extern 规则
        if !f.is_extern && f.body.is_none() {
            self.ctx
                .emit_error(f.span, "Non-extern functions must have a body");
            return;
        }

        let body_expr = match &f.body {
            Some(b) => b,
            None => return,
        };

        // 2. 提取解析好的函数签名
        let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
        let (param_tys, ret_ty) = match self.ctx.type_registry.get(sig_ty).clone() {
            TypeKind::Function { params, ret, .. } => (params, ret),
            _ => (Vec::new(), TypeId::ERROR),
        };

        // 3. 作用域环境重建
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope();
        // 将函数的泛型参数注入作用域
        for param in &f.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                param.name,
                SymbolInfo {
                    kind: SymbolKind::TypeParam,
                    node_id,
                    type_id: param_ty,
                    def_id: None,
                    span: f.span,
                    is_pub: false,
                    is_mut: false,
                },
            );
        }

        // 将 Where 子句的约束压入当前上下文的 active_bounds 中
        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in &f.where_clauses {
            let target_ty = self
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                if let Some(&bound_ty) = self.ctx.node_types.get(&bound.id) {
                    bounds.push(bound_ty);
                }
            }
            self.ctx.active_bounds.push((target_ty, bounds));
        }

        for (i, param_ast) in f.params.iter().enumerate() {
            if i < param_tys.len() {
                let info = SymbolInfo {
                    kind: SymbolKind::Var,
                    node_id: param_ast.type_node.id,
                    type_id: param_tys[i],
                    def_id: None,
                    span: param_ast.span,
                    is_pub: false,
                    is_mut: param_ast.pattern.is_mut,
                };
                let _ = self.ctx.scopes.define(param_ast.pattern.name, info);
            }
        }

        // 4. 启动表达式检查器
        let mut checker = ExprChecker::new(self.ctx, Some(ret_ty));

        let body_eval_ty = checker.check_expr(body_expr, Some(ret_ty));

        // 5. 校验函数的最终末尾表达式是否匹配签名
        if ret_ty != TypeId::ERROR && body_eval_ty != TypeId::ERROR {
            if ret_ty == body_eval_ty {
                // 类型完美匹配
            } else if body_eval_ty == TypeId::VOID && checker.has_returned {
                // 已经通过 return 语句返回过，允许块自身计算为 VOID
            } else {
                // 强制检查 Coercion（会打印 Type mismatch）
                if !checker.check_coercion(body_expr.span, ret_ty, body_eval_ty) {
                    self.ctx.emit_error(
                        body_expr.span,
                        "Function body evaluates to a type that does not match its signature. \
                        (Hint: Missing a return statement or a trailing semicolon?)",
                    );
                }
            }
        }

        self.ctx.active_bounds.truncate(prev_bounds_len); // 退出函数局部作用域前，清理本次注册的 Where 约束
        self.ctx.scopes.exit_scope(); // 退出函数局部作用域
    }

    fn check_struct(&mut self, s: &crate::def::StructDef, parent_scope: ScopeId) {
        // 1. 作用域环境重建 (为了让默认值能认识结构体的泛型参数 T)
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope(); 

        for param in &s.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                param.name,
                SymbolInfo {
                    kind: SymbolKind::TypeParam,
                    node_id,
                    type_id: param_ty,
                    def_id: None,
                    span: s.span,
                    is_pub: false,
                    is_mut: false,
                },
            );
        }

        // 2. 遍历检查所有字段的默认值
        for field in &s.fields {
            if let Some(default_expr) = &field.default_value {
                // 获取字段的预期类型
                let field_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                
                // 启动表达式检查器
                let mut checker = ExprChecker::new(self.ctx, None);
                let eval_ty = checker.check_expr(default_expr, Some(field_ty));

                // 3. 检查默认值的类型是否与字段兼容
                if field_ty != TypeId::ERROR && eval_ty != TypeId::ERROR {
                    if !checker.check_coercion(default_expr.span, field_ty, eval_ty) {
                        self.ctx.emit_error(
                            default_expr.span,
                            format!(
                                "Default value type mismatch for field `{}`. Expected `{}`, found `{}`", 
                                self.ctx.resolve(field.name),
                                self.ctx.ty_to_string(field_ty), 
                                self.ctx.ty_to_string(eval_ty)
                            ),
                        );
                    }
                }
            }
        }

        // 退出结构体词法作用域
        self.ctx.scopes.exit_scope();
    }

    fn check_union(&mut self, u: &crate::def::UnionDef, parent_scope: ScopeId) {
        // 1. 作用域环境重建
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope(); 

        for param in &u.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                param.name,
                SymbolInfo {
                    kind: SymbolKind::TypeParam,
                    node_id,
                    type_id: param_ty,
                    def_id: None,
                    span: u.span,
                    is_pub: false,
                    is_mut: false,
                },
            );
        }

        // 2. 遍历检查所有字段的默认值
        for field in &u.fields {
            if let Some(default_expr) = &field.default_value {
                // 获取字段的预期类型
                let field_ty = self.ctx.node_types.get(&field.type_node.id).copied().unwrap_or(TypeId::ERROR);
                
                // 启动表达式检查器
                let mut checker = ExprChecker::new(self.ctx, None);
                let eval_ty = checker.check_expr(default_expr, Some(field_ty));

                // 3. 检查默认值的类型是否与字段兼容
                if field_ty != TypeId::ERROR && eval_ty != TypeId::ERROR {
                    if !checker.check_coercion(default_expr.span, field_ty, eval_ty) {
                        self.ctx.emit_error(
                            default_expr.span,
                            format!(
                                "Default value type mismatch for union field `{}`. Expected `{}`, found `{}`", 
                                self.ctx.resolve(field.name),
                                self.ctx.ty_to_string(field_ty), 
                                self.ctx.ty_to_string(eval_ty)
                            ),
                        );
                    }
                }
            }
        }

        // 退出联合体词法作用域
        self.ctx.scopes.exit_scope();
    }

    fn check_impl(&mut self, i: &ImplDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        // 将 Impl 块的泛型参数（如 T）注入作用域
        for param in &i.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                param.name,
                SymbolInfo {
                    kind: SymbolKind::TypeParam,
                    node_id,
                    type_id: param_ty,
                    def_id: None,
                    span: i.span,
                    is_pub: false,
                    is_mut: false,
                },
            );
        }

        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in &i.where_clauses {
            let target_ty = self
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                if let Some(&bound_ty) = self.ctx.node_types.get(&bound.id) {
                    bounds.push(bound_ty);
                }
            }
            self.ctx.active_bounds.push((target_ty, bounds));
        }

        // 为 Impl 块注入 `Self` 类型
        let target_ty = self
            .ctx
            .node_types
            .get(&i.target_type.id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        let self_sym = self.ctx.intern("Self");
        let _ = self.ctx.scopes.define(
            self_sym,
            SymbolInfo {
                kind: SymbolKind::TypeAlias,
                node_id: i.target_type.id,
                type_id: target_ty,
                def_id: None,
                span: i.span,
                is_pub: false,
                is_mut: false,
            },
        );

        // 递归检查所有方法
        for &method_id in &i.methods {
            let method_def = self.ctx.defs[method_id.0 as usize].clone();
            if let Def::Function(f) = method_def {
                self.check_function(&f, impl_scope);
            }
        }

        self.ctx.active_bounds.truncate(prev_bounds_len);
        self.ctx.scopes.exit_scope();
    }
}
