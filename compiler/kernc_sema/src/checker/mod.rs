mod constexpr;
mod expr;
mod subst;

pub use constexpr::{ConstEvaluator, ConstValue, ScriptHost};
pub(crate) use expr::ExprChecker;
pub use subst::Substituter;

use crate::context::SemaContext;
use crate::def::{Def, DefId, FunctionDef, GlobalDef, ImplDef};
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::Span;

/// Main entry point for semantic type checking.
pub struct TypeckDriver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

type BodyWorkItem = (DefId, ScopeId);

impl<'a, 'ctx> TypeckDriver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn check_all(&mut self) {
        let defs_clone = self.ctx.defs.clone();

        // === Phase 1: resolve global constant dependencies ===
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
        let mut max_iters = 100; // Prevent real dependency cycles from looping forever.
        let mut resolved_globals = std::collections::HashSet::new();

        while changed && max_iters > 0 {
            changed = false;
            max_iters -= 1;

            for &(item_id, scope_id) in &globals {
                if resolved_globals.contains(&item_id) {
                    continue; // Skip globals already inferred successfully.
                }

                let Some(g) = self.global_def(item_id, "infer a global initializer") else {
                    continue;
                };

                // Snapshot diagnostic state so failed speculative inference can be rolled back.
                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types.clone();

                // Try to infer the initializer type.
                self.ctx.scopes.set_current_scope(scope_id);
                let mut checker = ExprChecker::new(self.ctx, None);
                let init_ty = checker.check_expr(&g.value, None);
                self.ctx.scopes.set_current_scope(scope_id);

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;

                    if self.ctx.scopes.resolve_local(g.name).is_some() {
                        self.ctx.scopes.update_type(g.name, init_ty);
                    }

                    // Once the type is known, run constexpr validation as well.
                    if !g.is_extern {
                        if let ast::ExprKind::Undef = g.value.kind {
                            self.ctx.emit_error(g.span, "Global variables cannot be initialized with bare `undef`. Must provide a typed constant value (e.g., `.{undef}`).");
                        } else {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(&g.value, 0);
                        }
                    } else if !matches!(g.value.kind, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(ref inner), .. } if matches!(inner.kind, ast::ExprKind::Undef))
                    {
                        self.ctx.emit_error(g.span, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
                    }
                } else {
                    // Inference failed; roll back and retry on the next pass.
                    self.ctx.sess.error_count = old_err_cnt;
                    self.ctx.sess.diagnostics.truncate(old_diag_len);
                    self.ctx.node_types = old_node_types;
                }
            }
        }

        // Phase 2: fallback reporting for anything still unresolved.
        if resolved_globals.len() < globals.len() {
            for &(item_id, scope_id) in &globals {
                if !resolved_globals.contains(&item_id) {
                    let Some(g) = self.global_def(item_id, "re-check an unresolved global") else {
                        continue;
                    };

                    self.ctx.scopes.set_current_scope(scope_id);
                    let mut checker = ExprChecker::new(self.ctx, None);
                    checker.check_expr(&g.value, None); // Let the real diagnostics reach the user.

                    self.ctx.struct_error(g.span, format!("cannot resolve global constant `{}`", self.ctx.resolve(g.name)))
                        .with_hint("this is usually caused by a circular dependency (e.g., A depends on B, and B depends on A) or an undefined variable")
                        .emit();
                }
            }
        }

        // === Phase 3: check regular items such as functions and impl blocks ===
        for def in defs_clone {
            if let Def::Module(m) = def {
                self.ctx.scopes.set_current_scope(m.scope_id);
                for item_id in m.items {
                    let d = &self.ctx.defs[item_id.0 as usize];
                    if !matches!(d, Def::Global(_)) {
                        // Globals were already handled in the dependency pass.
                        self.check_item(item_id, m.scope_id);
                    }
                }
            }
        }
    }

    pub fn global_worklist(&self) -> Vec<(DefId, ScopeId)> {
        let mut globals = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Module(module) = def {
                for item_id in &module.items {
                    if matches!(self.ctx.defs[item_id.0 as usize], Def::Global(_)) {
                        globals.push((*item_id, module.scope_id));
                    }
                }
            }
        }
        globals
    }

    pub fn resolve_global_worklist(&mut self, globals: &[(DefId, ScopeId)]) {
        let mut changed = true;
        let mut max_iters = 100;
        let mut resolved_globals = std::collections::HashSet::new();

        while changed && max_iters > 0 {
            changed = false;
            max_iters -= 1;

            for &(item_id, scope_id) in globals {
                if resolved_globals.contains(&item_id) {
                    continue;
                }

                let Some(global) = self.global_def(item_id, "infer a global initializer") else {
                    continue;
                };

                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types.clone();
                let init_ty = self.check_global_initializer(scope_id, &global);

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;

                    if self.ctx.scopes.resolve_local(global.name).is_some() {
                        self.ctx.scopes.update_type(global.name, init_ty);
                    }

                    if !global.is_extern {
                        if let ast::ExprKind::Undef = global.value.kind {
                            self.ctx.emit_error(global.span, "Global variables cannot be initialized with bare `undef`. Must provide a typed constant value (e.g., `.{undef}`).");
                        } else {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(&global.value, 0);
                        }
                    } else if !matches!(global.value.kind, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(ref inner), .. } if matches!(inner.kind, ast::ExprKind::Undef))
                    {
                        self.ctx.emit_error(global.span, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
                    }
                } else {
                    self.ctx.sess.error_count = old_err_cnt;
                    self.ctx.sess.diagnostics.truncate(old_diag_len);
                    self.ctx.node_types = old_node_types;
                }
            }
        }

        if resolved_globals.len() < globals.len() {
            for &(item_id, scope_id) in globals {
                if !resolved_globals.contains(&item_id) {
                    let Some(global) = self.global_def(item_id, "re-check an unresolved global")
                    else {
                        continue;
                    };

                    let _ = self.check_global_initializer(scope_id, &global);

                    self.ctx
                        .struct_error(
                            global.span,
                            format!(
                                "cannot resolve global constant `{}`",
                                self.ctx.resolve(global.name)
                            ),
                        )
                        .with_hint("this is usually caused by a circular dependency (e.g., A depends on B, and B depends on A) or an undefined variable")
                        .emit();
                }
            }
        }
    }

    pub fn body_worklist(&self) -> Vec<BodyWorkItem> {
        let mut worklist = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Module(module) = def {
                for item_id in &module.items {
                    if !matches!(self.ctx.defs[item_id.0 as usize], Def::Global(_)) {
                        worklist.push((*item_id, module.scope_id));
                    }
                }
            }
        }
        worklist
    }

    pub fn check_body_worklist(&mut self, worklist: &[BodyWorkItem]) {
        for &(def_id, parent_scope) in worklist {
            self.ctx.scopes.set_current_scope(parent_scope);
            self.check_item(def_id, parent_scope);
        }
    }

    fn check_global_initializer(&mut self, scope_id: ScopeId, global: &GlobalDef) -> TypeId {
        self.ctx.scopes.set_current_scope(scope_id);
        let mut checker = ExprChecker::new(self.ctx, None);
        let init_ty = checker.check_expr(&global.value, None);
        self.ctx.scopes.set_current_scope(scope_id);
        init_ty
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

    fn global_def(
        &mut self,
        def_id: crate::def::DefId,
        context: &str,
    ) -> Option<crate::def::GlobalDef> {
        match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Global(global)) => Some(global),
            Some(other) => {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Typeck): Expected global definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    Span::default(),
                    format!(
                        "Kern ICE (Typeck): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    // ==========================================
    //          Item Checkers
    // ==========================================

    fn check_function(&mut self, f: &FunctionDef, parent_scope: ScopeId) {
        if f.is_const && f.is_extern {
            self.ctx
                .emit_error(f.span, "`const fn` cannot be declared `extern`");
        }

        // 1. Validate extern-related rules.
        if !f.is_extern && !f.is_imported && f.body.is_none() {
            self.ctx
                .emit_error(f.span, "Non-extern functions must have a body");
            return;
        }

        let body_expr = match &f.body {
            Some(b) => b,
            None => return,
        };

        // 2. Extract the resolved function signature.
        let sig_ty = f.resolved_sig.unwrap_or(TypeId::ERROR);
        let (param_tys, ret_ty) = match self.ctx.type_registry.get(sig_ty).clone() {
            TypeKind::Function { params, ret, .. } => (params, ret),
            _ => (Vec::new(), TypeId::ERROR),
        };

        // 3. Rebuild the function-local scope.
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope();
        // Inject generic parameters into the function scope.
        for param in &f.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id,
                type_id: param_ty,
                def_id: None,
                span: f.span,
                is_pub: false,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.is_pub,
                );
            }
        }

        // Push active bounds from the function's where-clauses into the current context.
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
                if self.ctx.resolve(param_ast.pattern.name) == "_" {
                    continue;
                }
                let info = SymbolInfo {
                    kind: SymbolKind::Var,
                    node_id: param_ast.type_node.id,
                    type_id: param_tys[i],
                    def_id: None,
                    span: param_ast.pattern.name_span,
                    is_pub: false,
                    is_mut: param_ast.pattern.is_mut,
                };
                if self
                    .ctx
                    .scopes
                    .define(param_ast.pattern.name, info.clone())
                    .is_ok()
                {
                    self.ctx.record_symbol_definition(
                        info.span,
                        SemanticSymbolKind::Parameter,
                        info.is_mut,
                        info.is_pub,
                    );
                }
            }
        }

        // 4. Run the expression checker on the body.
        let mut checker = ExprChecker::new(self.ctx, Some(ret_ty));

        let body_eval_ty = checker.check_expr(body_expr, Some(ret_ty));

        // 5. Verify that the trailing expression matches the declared return type.
        if ret_ty != TypeId::ERROR && body_eval_ty != TypeId::ERROR {
            if ret_ty == body_eval_ty {
                // Exact match.
            } else if body_eval_ty == TypeId::VOID && checker.has_returned {
                // Explicit `return` already handled the value flow.
            } else {
                // Force a coercion check to emit a proper type mismatch diagnostic.
                if !checker.check_coercion(body_expr, ret_ty, body_eval_ty) {
                    self.ctx.emit_error(
                        body_expr.span,
                        "Function body evaluates to a type that does not match its signature. \
                        (Hint: Missing a return statement or a trailing semicolon?)",
                    );
                }
            }
        }

        self.ctx.active_bounds.truncate(prev_bounds_len); // Drop bounds introduced by this function scope.
        self.ctx.scopes.exit_scope(); // Leave the function scope.
    }

    fn check_struct(&mut self, s: &crate::def::StructDef, parent_scope: ScopeId) {
        // 1. Rebuild the struct-local scope so default values can see generic parameters.
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope();

        for param in &s.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id,
                type_id: param_ty,
                def_id: None,
                span: s.span,
                is_pub: false,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.is_pub,
                );
            }
        }

        // 2. Check every field default expression.
        for field in &s.fields {
            if let Some(default_expr) = &field.default_value {
                // Resolve the field's expected semantic type.
                let field_ty = self
                    .ctx
                    .node_types
                    .get(&field.type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                // Type-check the default expression.
                let mut checker = ExprChecker::new(self.ctx, None);
                let eval_ty = checker.check_expr(default_expr, Some(field_ty));

                // 3. Verify the default value is compatible with the field type.
                if field_ty != TypeId::ERROR
                    && eval_ty != TypeId::ERROR
                    && !checker.check_coercion(default_expr, field_ty, eval_ty)
                {
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

        // Leave the struct-local scope.
        self.ctx.scopes.exit_scope();
    }

    fn check_union(&mut self, u: &crate::def::UnionDef, parent_scope: ScopeId) {
        // 1. Rebuild the union-local scope.
        self.ctx.scopes.set_current_scope(parent_scope);
        let _ = self.ctx.scopes.enter_scope();

        for param in &u.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id,
                type_id: param_ty,
                def_id: None,
                span: u.span,
                is_pub: false,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.is_pub,
                );
            }
        }

        // 2. Check every field default expression.
        for field in &u.fields {
            if let Some(default_expr) = &field.default_value {
                // Resolve the field's expected semantic type.
                let field_ty = self
                    .ctx
                    .node_types
                    .get(&field.type_node.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                // Type-check the default expression.
                let mut checker = ExprChecker::new(self.ctx, None);
                let eval_ty = checker.check_expr(default_expr, Some(field_ty));

                // 3. Verify the default value is compatible with the field type.
                if field_ty != TypeId::ERROR
                    && eval_ty != TypeId::ERROR
                    && !checker.check_coercion(default_expr, field_ty, eval_ty)
                {
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

        // Leave the union-local scope.
        self.ctx.scopes.exit_scope();
    }

    fn check_impl(&mut self, i: &ImplDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        // Inject impl-level generic parameters such as `T`.
        for param in &i.generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id,
                type_id: param_ty,
                def_id: None,
                span: i.span,
                is_pub: false,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.is_pub,
                );
            }
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

        // Inject the `Self` type for the impl target.
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

        // Recursively check every method body in the impl block.
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
