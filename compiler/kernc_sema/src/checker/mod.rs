mod constexpr;
mod expr;
mod subst;

pub use constexpr::{ConstEvaluator, ConstValue, ScriptHost};
pub(crate) use expr::ExprChecker;
pub use subst::{Substituter, substitute_associated_types};

use crate::context::SemaContext;
use crate::def::{Def, DefId, FunctionDef, GlobalDef, ImplDef};
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Visibility};
use kernc_utils::Span;
use std::time::{Duration, Instant};

/// Main entry point for semantic type checking.
pub struct TypeckDriver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    body_timings: TypeckBodyTimings,
}

type BodyWorkItem = (DefId, ScopeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeckTiming {
    pub name: &'static str,
    pub duration: Duration,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TypeckBodyTimings {
    top_level_functions: Duration,
    impl_methods: Duration,
    function_setup: Duration,
    function_expr: Duration,
    function_return: Duration,
    structs: Duration,
    unions: Duration,
    impls: Duration,
    impl_setup: Duration,
}

impl TypeckBodyTimings {
    pub fn phase_timings(self) -> Vec<TypeckTiming> {
        [
            ("  fn_top_level", self.top_level_functions),
            ("  fn_impl_method", self.impl_methods),
            ("  fn_setup", self.function_setup),
            ("  fn_expr", self.function_expr),
            ("  fn_return", self.function_return),
            ("  structs", self.structs),
            ("  unions", self.unions),
            ("  impls", self.impls),
            ("  impl_setup", self.impl_setup),
        ]
        .into_iter()
        .filter_map(|(name, duration)| {
            if duration == Duration::default() {
                None
            } else {
                Some(TypeckTiming { name, duration })
            }
        })
        .collect()
    }
}

#[derive(Debug, Clone, Copy)]
enum FunctionBodyKind {
    TopLevel,
    ImplMethod,
}

impl<'a, 'ctx> TypeckDriver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            body_timings: TypeckBodyTimings::default(),
        }
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn body_phase_timings(&self) -> Vec<TypeckTiming> {
        if !self.ctx.collects_timings() {
            return Vec::new();
        }
        let mut timings = self.body_timings.phase_timings();
        let expr = self.ctx.expr_timing_stats;
        timings.extend(
            [
                ("    expr_bindings", expr.bindings),
                ("    expr_ops", expr.ops),
                ("    expr_access", expr.access),
                ("      expr_access_identifier", expr.access_identifier),
                ("      expr_access_field", expr.access_field),
                ("        expr_access_field_module", expr.access_field_module),
                (
                    "        expr_access_field_enum_variant",
                    expr.access_field_enum_variant,
                ),
                (
                    "        expr_access_field_member_query",
                    expr.access_field_member_query,
                ),
                (
                    "          expr_access_field_query_trait_object",
                    expr.access_field_query_trait_object,
                ),
                (
                    "          expr_access_field_query_named_type",
                    expr.access_field_query_named_type,
                ),
                (
                    "          expr_access_field_query_bound",
                    expr.access_field_query_bound,
                ),
                (
                    "          expr_access_field_query_impl",
                    expr.access_field_query_impl,
                ),
                ("        expr_access_field_miss", expr.access_field_miss),
                ("      expr_access_index", expr.access_index),
                ("      expr_access_slice", expr.access_slice),
                ("    expr_call", expr.call),
                ("      expr_call_plain", expr.call_plain),
                ("        expr_call_signature", expr.call_signature),
                ("        expr_call_intrinsic", expr.call_intrinsic),
                ("        expr_call_arguments", expr.call_arguments),
                (
                    "      expr_call_generic_instantiation",
                    expr.call_generic_instantiation,
                ),
                ("      expr_call_closure", expr.call_closure),
                ("    expr_aggregate", expr.aggregate),
                ("    expr_control", expr.control),
                ("      expr_control_block", expr.control_block),
                ("      expr_control_if", expr.control_if),
                ("      expr_control_match", expr.control_match),
                (
                    "        expr_control_match_patterns",
                    expr.control_match_patterns,
                ),
                (
                    "        expr_control_match_bodies",
                    expr.control_match_bodies,
                ),
                (
                    "        expr_control_match_exhaustiveness",
                    expr.control_match_exhaustiveness,
                ),
                ("      expr_control_for", expr.control_for),
                ("      expr_control_return", expr.control_return),
                ("      expr_control_defer", expr.control_defer),
                ("    expr_dynamic_typeof", expr.dynamic_typeof),
            ]
            .into_iter()
            .filter_map(|(name, duration)| {
                if duration == Duration::default() {
                    None
                } else {
                    Some(TypeckTiming { name, duration })
                }
            }),
        );
        timings
    }

    fn measure_body_timing<T, F, R>(&mut self, record: R, f: F) -> T
    where
        F: FnOnce(&mut Self) -> T,
        R: FnOnce(&mut TypeckBodyTimings, Duration),
    {
        if !self.ctx.collects_timings() {
            return f(self);
        }
        let started = Instant::now();
        let value = f(self);
        record(&mut self.body_timings, started.elapsed());
        value
    }

    pub fn check_all(&mut self) {
        let (globals, body_worklist) = self.worklists();

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

                let Some(g) = self.global_def_ptr(item_id, "infer a global initializer") else {
                    continue;
                };

                // Snapshot diagnostic state so failed speculative inference can be rolled back.
                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types.clone();

                // Try to infer the initializer type.
                self.ctx.scopes.set_current_scope(scope_id);
                let mut checker = ExprChecker::new(self.ctx, None);
                let init_ty = checker.check_expr(unsafe { &(*g).value }, None);
                self.ctx.scopes.set_current_scope(scope_id);

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;

                    if self
                        .ctx
                        .scopes
                        .resolve_local(unsafe { (*g).name })
                        .is_some()
                    {
                        self.ctx.scopes.update_type(unsafe { (*g).name }, init_ty);
                    }

                    // Once the type is known, run constexpr validation as well.
                    if !unsafe { (*g).is_extern } {
                        if let ast::ExprKind::Undef = unsafe { &(*g).value.kind } {
                            self.ctx.emit_error(unsafe { (*g).span }, "Global variables cannot be initialized with bare `undef`. Must provide a typed constant value (e.g., `.{undef}`).");
                        } else {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(unsafe { &(*g).value }, 0);
                        }
                    } else if !matches!(unsafe { &(*g).value.kind }, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(inner), .. } if matches!(inner.kind, ast::ExprKind::Undef))
                    {
                        self.ctx.emit_error(unsafe { (*g).span }, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
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
                    let Some(g) = self.global_def_ptr(item_id, "re-check an unresolved global")
                    else {
                        continue;
                    };

                    self.ctx.scopes.set_current_scope(scope_id);
                    let mut checker = ExprChecker::new(self.ctx, None);
                    checker.check_expr(unsafe { &(*g).value }, None); // Let the real diagnostics reach the user.

                    self.ctx.struct_error(unsafe { (*g).span }, format!("cannot resolve global constant `{}`", self.ctx.resolve(unsafe { (*g).name })))
                        .with_hint("this is usually caused by a circular dependency (e.g., A depends on B, and B depends on A) or an undefined variable")
                        .emit();
                }
            }
        }

        // === Phase 3: check regular items such as functions and impl blocks ===
        for (item_id, scope_id) in body_worklist {
            self.ctx.scopes.set_current_scope(scope_id);
            self.check_item(item_id, scope_id);
        }
    }

    pub fn worklists(&self) -> (Vec<(DefId, ScopeId)>, Vec<BodyWorkItem>) {
        self.collect_worklists()
    }

    pub fn global_worklist(&self) -> Vec<(DefId, ScopeId)> {
        self.worklists().0
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

                let Some(global) = self.global_def_ptr(item_id, "infer a global initializer")
                else {
                    continue;
                };

                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types.clone();
                let init_ty = self.check_global_initializer(scope_id, unsafe { &*global });

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;

                    if self
                        .ctx
                        .scopes
                        .resolve_local(unsafe { (*global).name })
                        .is_some()
                    {
                        self.ctx
                            .scopes
                            .update_type(unsafe { (*global).name }, init_ty);
                    }

                    if !unsafe { (*global).is_extern } {
                        if let ast::ExprKind::Undef = unsafe { &(*global).value.kind } {
                            self.ctx.emit_error(unsafe { (*global).span }, "Global variables cannot be initialized with bare `undef`. Must provide a typed constant value (e.g., `.{undef}`).");
                        } else {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(unsafe { &(*global).value }, 0);
                        }
                    } else if !matches!(unsafe { &(*global).value.kind }, ast::ExprKind::DataInit { literal: ast::DataLiteralKind::Scalar(inner), .. } if matches!(inner.kind, ast::ExprKind::Undef))
                    {
                        self.ctx.emit_error(unsafe { (*global).span }, "Extern statics must be initialized with `undef`, e.g., `static X = i32.{undef};`");
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
                    let Some(global) =
                        self.global_def_ptr(item_id, "re-check an unresolved global")
                    else {
                        continue;
                    };

                    let _ = self.check_global_initializer(scope_id, unsafe { &*global });

                    self.ctx
                        .struct_error(
                            unsafe { (*global).span },
                            format!(
                                "cannot resolve global constant `{}`",
                                self.ctx.resolve(unsafe { (*global).name })
                            ),
                        )
                        .with_hint("this is usually caused by a circular dependency (e.g., A depends on B, and B depends on A) or an undefined variable")
                        .emit();
                }
            }
        }
    }

    pub fn body_worklist(&self) -> Vec<BodyWorkItem> {
        self.worklists().1
    }

    pub fn check_body_worklist(&mut self, worklist: &[BodyWorkItem]) -> TypeckBodyTimings {
        self.ctx.expr_timing_stats = Default::default();
        for &(def_id, parent_scope) in worklist {
            self.ctx.scopes.set_current_scope(parent_scope);
            self.check_item(def_id, parent_scope);
        }
        self.body_timings
    }

    fn check_global_initializer(&mut self, scope_id: ScopeId, global: &GlobalDef) -> TypeId {
        self.ctx.scopes.set_current_scope(scope_id);
        let mut checker = ExprChecker::new(self.ctx, None);
        let init_ty = checker.check_expr(&global.value, None);
        self.ctx.scopes.set_current_scope(scope_id);
        init_ty
    }

    fn collect_worklists(&self) -> (Vec<(DefId, ScopeId)>, Vec<BodyWorkItem>) {
        let mut globals = Vec::new();
        let mut bodies = Vec::new();

        for def in &self.ctx.defs {
            let Def::Module(module) = def else {
                continue;
            };

            for item_id in &module.items {
                if matches!(self.ctx.defs[item_id.0 as usize], Def::Global(_)) {
                    globals.push((*item_id, module.scope_id));
                } else {
                    bodies.push((*item_id, module.scope_id));
                }
            }
        }

        (globals, bodies)
    }

    fn check_item(&mut self, id: crate::def::DefId, parent_scope: ScopeId) {
        let Some(def) = self.def_ptr(id, "type-check an item body") else {
            return;
        };

        // Safety: type checking mutates inference state, scopes, and diagnostics, but it does not
        // mutate `ctx.defs`. These raw pointers therefore remain valid for the duration of the
        // dispatch below while avoiding cloning whole AST-backed definitions.
        unsafe {
            match &*def {
                Def::Function(f) => {
                    self.check_function(f, parent_scope, FunctionBodyKind::TopLevel)
                }
                Def::Impl(i) => self.measure_body_timing(
                    |timings, elapsed| timings.impls += elapsed,
                    |this| this.check_impl(i, parent_scope),
                ),
                Def::Struct(s) => self.measure_body_timing(
                    |timings, elapsed| timings.structs += elapsed,
                    |this| this.check_struct(s, parent_scope),
                ),
                Def::Union(u) => self.measure_body_timing(
                    |timings, elapsed| timings.unions += elapsed,
                    |this| this.check_union(u, parent_scope),
                ),
                _ => {}
            }
        }
    }

    fn def_ptr(&mut self, def_id: crate::def::DefId, context: &str) -> Option<*const Def> {
        match self.ctx.defs.get(def_id.0 as usize) {
            Some(def) => Some(std::ptr::from_ref(def)),
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

    fn global_def_ptr(
        &mut self,
        def_id: crate::def::DefId,
        context: &str,
    ) -> Option<*const crate::def::GlobalDef> {
        let def = self.def_ptr(def_id, context)?;
        // Safety: same reasoning as `def_ptr`; semantic definition storage is immutable during
        // type checking.
        unsafe {
            match &*def {
                Def::Global(global) => Some(std::ptr::from_ref(global)),
                other => {
                    self.ctx.emit_ice(
                        Span::default(),
                        format!(
                            "Kern ICE (Typeck): Expected global definition while trying to {}, found {:?}.",
                            context, other
                        ),
                    );
                    None
                }
            }
        }
    }

    // ==========================================
    //          Item Checkers
    // ==========================================

    fn check_function(&mut self, f: &FunctionDef, parent_scope: ScopeId, kind: FunctionBodyKind) {
        let collect_timings = self.ctx.collects_timings();
        let function_started = collect_timings.then(Instant::now);
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
        let setup_started = collect_timings.then(Instant::now);
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
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.vis.is_public(),
                );
            }
        }

        // Push active bounds from the function's where-clauses into the current context.
        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in &f.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                if let Some(&bound_ty) = self.ctx.node_types.get(&bound.id) {
                    bounds.push(self.ctx.type_registry.normalize(bound_ty));
                }
            }
            self.ctx.active_bounds.push((target_ty, bounds));
        }
        if self.ctx.active_bounds.len() != prev_bounds_len {
            self.ctx.bound_trait_match_cache.clear();
            self.ctx.impl_applicability_cache.clear();
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
                    vis: Visibility::Private,
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
                        info.vis.is_public(),
                    );
                }
            }
        }
        if let Some(setup_started) = setup_started {
            self.body_timings.function_setup += setup_started.elapsed();
        }

        // 4. Run the expression checker on the body.
        let expr_started = collect_timings.then(Instant::now);
        let mut checker = ExprChecker::new(self.ctx, Some(ret_ty));
        let body_eval_ty = checker.check_expr(body_expr, Some(ret_ty));
        if let Some(expr_started) = expr_started {
            self.body_timings.function_expr += expr_started.elapsed();
        }

        // 5. Verify that the trailing expression matches the declared return type.
        let return_started = collect_timings.then(Instant::now);
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
        if let Some(return_started) = return_started {
            self.body_timings.function_return += return_started.elapsed();
        }

        self.ctx.active_bounds.truncate(prev_bounds_len); // Drop bounds introduced by this function scope.
        self.ctx.bound_trait_match_cache.clear();
        self.ctx.impl_applicability_cache.clear();
        self.ctx.scopes.exit_scope(); // Leave the function scope.

        if let Some(function_started) = function_started {
            let elapsed = function_started.elapsed();
            match kind {
                FunctionBodyKind::TopLevel => self.body_timings.top_level_functions += elapsed,
                FunctionBodyKind::ImplMethod => self.body_timings.impl_methods += elapsed,
            }
        }
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
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.vis.is_public(),
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
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.vis.is_public(),
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
        let setup_started = self.ctx.collects_timings().then(Instant::now);
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
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::TypeParameter,
                    info.is_mut,
                    info.vis.is_public(),
                );
            }
        }

        let prev_bounds_len = self.ctx.active_bounds.len();
        for clause in &i.where_clauses {
            let target_ty = self.ctx.type_registry.normalize(
                self.ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR),
            );
            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                if let Some(&bound_ty) = self.ctx.node_types.get(&bound.id) {
                    bounds.push(self.ctx.type_registry.normalize(bound_ty));
                }
            }
            self.ctx.active_bounds.push((target_ty, bounds));
        }
        if self.ctx.active_bounds.len() != prev_bounds_len {
            self.ctx.bound_trait_match_cache.clear();
            self.ctx.impl_applicability_cache.clear();
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
                vis: Visibility::Private,
                is_mut: false,
            },
        );
        if let Some(setup_started) = setup_started {
            self.body_timings.impl_setup += setup_started.elapsed();
        }

        // Recursively check every method body in the impl block.
        for &method_id in &i.methods {
            let Some(method_def) = self.def_ptr(method_id, "type-check an impl method body") else {
                continue;
            };
            // Safety: same as `check_item`; method definitions live in immutable `ctx.defs`
            // during type checking.
            unsafe {
                if let Def::Function(f) = &*method_def {
                    self.check_function(f, impl_scope, FunctionBodyKind::ImplMethod);
                }
            }
        }

        self.ctx.active_bounds.truncate(prev_bounds_len);
        self.ctx.bound_trait_match_cache.clear();
        self.ctx.impl_applicability_cache.clear();
        self.ctx.scopes.exit_scope();
    }
}
