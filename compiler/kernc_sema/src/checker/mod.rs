//! Semantic type-checking driver.
//!
//! The checker runs after collection and type resolution. It resolves global
//! initializer cycles with speculative retries, checks function and impl bodies
//! in their owning scopes, records timing data for the LSP/driver, and exposes
//! cancelable entry points for editor workloads.

mod constexpr;
pub(crate) mod expr;

pub use constexpr::{ConstEvalError, ConstEvalResult, ConstEvaluator, ConstValue, ScriptHost};
pub(crate) use expr::ExprChecker;

use crate::context::{EscapeSummary, SemaContext};
use crate::def::{Def, DefId, FunctionDef, GlobalDef, ImplDef};
use crate::passes::TypeResolver;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind, SymbolNamespace};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{ConstGeneric, GenericArg, TypeId, TypeKind};
use kernc_ast::{self as ast, Visibility};
use kernc_utils::{Canceled, CancellationToken, FileId, Span};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Main entry point for semantic type checking.
pub struct TypeckDriver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    body_timings: TypeckBodyTimings,
    cancellation: CancellationToken,
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
    TraitDefaultMethod,
}

impl<'a, 'ctx> TypeckDriver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            body_timings: TypeckBodyTimings::default(),
            cancellation: CancellationToken::new(),
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
        let expr = self.ctx.analysis.expr_timing_stats;
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

    fn bind_generics_into_scope(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);
        for param in generics {
            // Body checking needs generic parameters in the value/type namespace and in semantic
            // symbol tables so diagnostics, hovers, and references agree on the same binding.
            let (kind, param_ty, semantic_kind) = match &param.kind {
                ast::GenericParamKind::Type => (
                    SymbolKind::TypeParam,
                    self.ctx.type_registry.intern(TypeKind::Param(param.name)),
                    SemanticSymbolKind::TypeParameter,
                ),
                ast::GenericParamKind::Const { ty } => {
                    let mut resolver = crate::passes::TypeResolver::new(self.ctx);
                    let const_ty = resolver.resolve_const_generic_param_type(ty, scope, param.span);
                    (
                        SymbolKind::ConstParam,
                        const_ty,
                        SemanticSymbolKind::Constant,
                    )
                }
            };
            let node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind,
                node_id,
                type_id: param_ty,
                def_id: None,
                span: param.span,
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(param.name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    semantic_kind,
                    info.is_mut,
                    info.vis.is_public(),
                );
            }
        }
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

        // Phase 1: infer globals to a fixed point. Globals can depend on each other, so failed
        // attempts are rolled back and retried after more initializer types become known.
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

                let Some(global) = self.global_def_snapshot(item_id, "infer a global initializer")
                else {
                    continue;
                };

                // Snapshot diagnostic state so failed speculative inference can be rolled back.
                let old_err_cnt = self.ctx.sess.error_count;
                let old_diag_len = self.ctx.sess.diagnostics.len();
                let old_node_types = self.ctx.node_types_snapshot();

                // Try to infer the initializer type.
                self.ctx.scopes.set_current_scope(scope_id);
                let annotated_ty = if let Some(type_node) = &global.type_node {
                    let mut resolver = TypeResolver::new(self.ctx);
                    Some(resolver.resolve_type(type_node, scope_id))
                } else {
                    None
                };
                let init_ty = if let Some(value) = &global.value {
                    let mut checker =
                        ExprChecker::with_cancellation(self.ctx, None, self.cancellation.clone());
                    let init_ty = checker.check_expr(value, annotated_ty);
                    let init_ty = checker.finalize_numeric_inference(init_ty);
                    if let Some(expected) = annotated_ty {
                        checker.check_coercion(value, expected, init_ty);
                        expected
                    } else {
                        init_ty
                    }
                } else if global.is_extern {
                    annotated_ty.unwrap_or(TypeId::ERROR)
                } else {
                    self.ctx.emit_error(
                        global.span,
                        "static declarations without an initializer must be `extern`",
                    );
                    TypeId::ERROR
                };
                self.ctx.scopes.set_current_scope(scope_id);

                if init_ty != TypeId::ERROR {
                    resolved_globals.insert(item_id);
                    changed = true;
                    let had_type_errors = self.ctx.sess.error_count > old_err_cnt;

                    if self.ctx.scopes.resolve_local(global.name).is_some() {
                        self.ctx.scopes.update_type_in_namespace(
                            global.name,
                            SymbolNamespace::Value,
                            init_ty,
                        );
                    }

                    // Once the type is known, run constexpr validation as well.
                    if had_type_errors {
                        continue;
                    }

                    if !global.is_extern
                        && let Some(value) = &global.value
                    {
                        let mut evaluator = ConstEvaluator::new(self.ctx);
                        let _ = evaluator.eval_inner(value, 0);
                    }
                } else {
                    // Inference failed; roll back and retry on the next pass.
                    self.ctx.sess.error_count = old_err_cnt;
                    self.ctx.sess.diagnostics.truncate(old_diag_len);
                    self.ctx.restore_node_types(old_node_types);
                }
            }
        }

        // Phase 2: fallback reporting for anything still unresolved.
        if resolved_globals.len() < globals.len() {
            for &(item_id, scope_id) in &globals {
                if !resolved_globals.contains(&item_id) {
                    let Some(global) =
                        self.global_def_snapshot(item_id, "re-check an unresolved global")
                    else {
                        continue;
                    };

                    let old_err_cnt = self.ctx.sess.error_count;
                    self.ctx.scopes.set_current_scope(scope_id);
                    if let Some(value) = &global.value {
                        let mut checker = ExprChecker::with_cancellation(
                            self.ctx,
                            None,
                            self.cancellation.clone(),
                        );
                        checker.check_expr(value, None); // Let the real diagnostics reach the user.
                    }

                    if self.ctx.sess.error_count > old_err_cnt {
                        continue;
                    }

                    self.ctx.struct_error(global.span, format!("cannot resolve global constant `{}`", self.ctx.resolve(global.name)))
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
        self.resolve_global_worklist_cancelable(globals, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled");
    }

    pub fn resolve_global_worklist_cancelable(
        &mut self,
        globals: &[(DefId, ScopeId)],
        cancellation: &CancellationToken,
    ) -> Result<(), Canceled> {
        let previous = std::mem::replace(&mut self.cancellation, cancellation.clone());
        let result = (|| {
            let mut changed = true;
            let mut max_iters = 100;
            let mut resolved_globals = std::collections::HashSet::new();

            while changed && max_iters > 0 {
                self.check_canceled()?;
                changed = false;
                max_iters -= 1;

                for &(item_id, scope_id) in globals {
                    self.check_canceled()?;
                    if resolved_globals.contains(&item_id) {
                        continue;
                    }

                    let Some(global) =
                        self.global_def_snapshot(item_id, "infer a global initializer")
                    else {
                        continue;
                    };

                    let old_err_cnt = self.ctx.sess.error_count;
                    let old_diag_len = self.ctx.sess.diagnostics.len();
                    let old_node_types = self.ctx.node_types_snapshot();
                    let init_ty = self.check_global_initializer(scope_id, &global);

                    if init_ty != TypeId::ERROR {
                        resolved_globals.insert(item_id);
                        changed = true;
                        let had_type_errors = self.ctx.sess.error_count > old_err_cnt;

                        if self.ctx.scopes.resolve_local(global.name).is_some() {
                            self.ctx.scopes.update_type_in_namespace(
                                global.name,
                                SymbolNamespace::Value,
                                init_ty,
                            );
                        }

                        if had_type_errors {
                            continue;
                        }

                        if !global.is_extern
                            && let Some(value) = &global.value
                        {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            let _ = evaluator.eval_inner(value, 0);
                        }
                    } else {
                        self.ctx.sess.error_count = old_err_cnt;
                        self.ctx.sess.diagnostics.truncate(old_diag_len);
                        self.ctx.restore_node_types(old_node_types);
                    }
                }
            }

            if resolved_globals.len() < globals.len() {
                for &(item_id, scope_id) in globals {
                    self.check_canceled()?;
                    if !resolved_globals.contains(&item_id) {
                        let Some(global) =
                            self.global_def_snapshot(item_id, "re-check an unresolved global")
                        else {
                            continue;
                        };

                        let old_err_cnt = self.ctx.sess.error_count;
                        let _ = self.check_global_initializer(scope_id, &global);

                        if self.ctx.sess.error_count > old_err_cnt {
                            continue;
                        }

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
            Ok(())
        })();
        self.cancellation = previous;
        result
    }

    pub fn body_worklist(&self) -> Vec<BodyWorkItem> {
        self.worklists().1
    }

    pub fn body_worklist_for_file(&self, target_path: &Path) -> Vec<BodyWorkItem> {
        let target_file_ids = self.target_file_ids(target_path);
        if target_file_ids.is_empty() {
            return Vec::new();
        }

        self.worklists()
            .1
            .into_iter()
            .filter(|(def_id, _)| self.def_is_in_files(*def_id, &target_file_ids))
            .collect()
    }

    pub fn check_body_worklist(&mut self, worklist: &[BodyWorkItem]) -> TypeckBodyTimings {
        self.check_body_worklist_cancelable(worklist, &CancellationToken::new())
            .expect("fresh cancellation token cannot be canceled")
    }

    pub fn check_body_worklist_cancelable(
        &mut self,
        worklist: &[BodyWorkItem],
        cancellation: &CancellationToken,
    ) -> Result<TypeckBodyTimings, Canceled> {
        let previous = std::mem::replace(&mut self.cancellation, cancellation.clone());
        let result = (|| {
            self.ctx.analysis.expr_timing_stats = Default::default();
            for &(def_id, parent_scope) in worklist {
                self.check_canceled()?;
                self.ctx.scopes.set_current_scope(parent_scope);
                self.check_item(def_id, parent_scope);
                self.check_canceled()?;
            }
            self.check_canceled()?;
            self.emit_pending_temporary_address_escape_checks();
            Ok(self.body_timings)
        })();
        self.cancellation = previous;
        result
    }

    fn target_file_ids(&self, target_path: &Path) -> Vec<FileId> {
        let target_path = normalize_checker_path(target_path);
        self.ctx
            .sess
            .source_manager
            .files()
            .iter()
            .enumerate()
            .filter_map(|(index, file)| {
                (normalize_checker_path(&file.path) == target_path).then_some(FileId(index))
            })
            .collect()
    }

    fn def_is_in_files(&self, def_id: DefId, file_ids: &[FileId]) -> bool {
        let Some(def) = self.ctx.defs.get(def_id.0 as usize) else {
            return false;
        };
        let Some(span) = def_primary_span(def) else {
            return false;
        };
        file_ids.contains(&span.file)
    }

    fn check_canceled(&self) -> Result<(), Canceled> {
        self.cancellation.check()
    }

    fn check_global_initializer(&mut self, scope_id: ScopeId, global: &GlobalDef) -> TypeId {
        self.ctx.scopes.set_current_scope(scope_id);
        let annotated_ty = if let Some(type_node) = &global.type_node {
            let mut resolver = TypeResolver::new(self.ctx);
            Some(resolver.resolve_type(type_node, scope_id))
        } else {
            None
        };
        let Some(value) = &global.value else {
            return if global.is_extern {
                annotated_ty.unwrap_or(TypeId::ERROR)
            } else {
                self.ctx.emit_error(
                    global.span,
                    "static declarations without an initializer must be `extern`",
                );
                TypeId::ERROR
            };
        };
        let mut checker = ExprChecker::with_cancellation(self.ctx, None, self.cancellation.clone());
        let init_ty = {
            let init_ty = checker.check_expr(value, annotated_ty);
            checker.finalize_numeric_inference(init_ty)
        };
        if let Some(expected) = annotated_ty {
            checker.check_coercion(value, expected, init_ty);
            checker.reject_stack_pointer_escape(value, "static storage");
            return expected;
        }
        checker.reject_stack_pointer_escape(value, "static storage");
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
        if self.check_canceled().is_err() {
            return;
        }
        let Some(def) = self.def_ptr(id, "type-check an item body") else {
            return;
        };

        // SAFETY: type checking mutates inference state, scopes, and diagnostics, but it does not
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
                Def::Trait(t) => self.check_trait_default_methods(t, parent_scope),
                _ => {}
            }
        }
    }

    fn check_trait_default_methods(&mut self, t: &crate::def::TraitDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let trait_scope = self.ctx.scopes.enter_scope();
        self.bind_generics_into_scope(&t.generics, trait_scope);

        let prev_bounds_len = self.ctx.analysis.active_bounds.len();
        self.push_valid_where_clause_bounds(&t.where_clauses);
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }

        for method in &t.methods {
            if self.check_canceled().is_err() {
                break;
            }
            let Some(method_id) = method.default_impl else {
                continue;
            };
            let Some(method_def) =
                self.def_ptr(method_id, "type-check a trait default method body")
            else {
                continue;
            };
            // SAFETY: `method_def` points into immutable `ctx.defs`; checking the body mutates
            // analysis state but does not move or delete definitions.
            unsafe {
                if let Def::Function(f) = &*method_def {
                    self.check_function(f, trait_scope, FunctionBodyKind::TraitDefaultMethod);
                }
            }
        }

        self.ctx.analysis.active_bounds.truncate(prev_bounds_len);
        self.ctx.clear_active_bound_caches();
        self.ctx.scopes.exit_scope();
    }

    fn emit_pending_temporary_address_escape_checks(&mut self) {
        let pending = std::mem::take(&mut self.ctx.analysis.pending_escape_checks);
        for check in pending {
            let Some(summary) = self.ctx.analysis.escape_summaries.get(&check.callee) else {
                continue;
            };
            if !summary.stored_params.contains(&check.arg_index) {
                continue;
            }
            match check.origin {
                crate::checker::expr::PointerOrigin::Temporary(address_span)
                | crate::checker::expr::PointerOrigin::StaticLiteral(address_span) => {
                    self.ctx
                        .struct_error(
                            address_span,
                            "address of temporary value escapes through function call",
                        )
                        .with_hint(
                            "the callee stores the parameter receiving this temporary address",
                        )
                        .with_hint("bind the value to stable storage before taking its address")
                        .emit();
                }
                crate::checker::expr::PointerOrigin::Local(address_span) => {
                    self.ctx
                        .struct_error(
                            address_span,
                            "address of local value escapes through function call",
                        )
                        .with_hint(
                            "the callee stores the parameter receiving an address into the current stack frame",
                        )
                        .with_hint(
                            "move the value into storage that outlives the stored pointer",
                        )
                        .emit();
                }
                crate::checker::expr::PointerOrigin::CapturingClosure(closure_span) => {
                    self.ctx
                        .struct_error(
                            closure_span,
                            "capturing closure environment escapes through function call",
                        )
                        .with_span_label(
                            closure_span,
                            "this closure environment is stored in the current stack frame",
                        )
                        .with_hint(
                            "the callee stores the parameter receiving this closure object",
                        )
                        .with_hint(
                            "move the captured state into an explicit object that outlives the callback",
                        )
                        .emit();
                }
                crate::checker::expr::PointerOrigin::Parameter(_) => {}
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

    fn global_def_snapshot(
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

    fn push_valid_where_clause_bounds(&mut self, where_clauses: &[ast::WhereClause]) {
        for clause in where_clauses {
            let target_ty = self.ctx.normalized_node_type_or_error(clause.target_ty.id);
            if target_ty == TypeId::ERROR {
                continue;
            }

            let mut bounds = Vec::new();
            for bound in &clause.bounds {
                let bound_ty = self.ctx.normalized_node_type_or_error(bound.id);
                if bound_ty == TypeId::ERROR
                    || !matches!(
                        self.ctx.type_registry.get(bound_ty),
                        TypeKind::TraitObject(..)
                    )
                {
                    continue;
                }

                if !self.where_bound_may_contain_params(target_ty, bound_ty)
                    && !self.concrete_where_bound_is_satisfied(target_ty, bound_ty)
                {
                    let target_str = self.ctx.ty_to_string(target_ty);
                    let bound_str = self.ctx.ty_to_string(bound_ty);
                    self.ctx
                        .struct_error(bound.span, "concrete where-clause is not satisfied")
                        .with_hint(format!("required bound: `{}: {}`", target_str, bound_str))
                        .with_hint(
                            "generic where-clauses are assumptions, but fully concrete bounds must be proven by an actual impl",
                        )
                        .emit();
                    continue;
                }

                bounds.push(bound_ty);
            }

            if !bounds.is_empty() {
                self.ctx.analysis.active_bounds.push((target_ty, bounds));
            }
        }
    }

    fn concrete_where_bound_is_satisfied(&mut self, target_ty: TypeId, bound_ty: TypeId) -> bool {
        let mut checker = ExprChecker::with_cancellation(self.ctx, None, self.cancellation.clone());
        checker.check_trait_impl(target_ty, bound_ty)
    }

    fn where_bound_may_contain_params(&mut self, target_ty: TypeId, bound_ty: TypeId) -> bool {
        self.type_contains_params_or_vars(target_ty) || self.type_contains_params_or_vars(bound_ty)
    }

    fn type_contains_params_or_vars(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) | TypeKind::Error => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::ArrayInfer { elem }
            | TypeKind::AnonymousEnumPayload(elem)
            | TypeKind::Simd { elem, .. } => self.type_contains_params_or_vars(elem),
            TypeKind::Range { start, end, .. } => {
                start.is_some_and(|ty| self.type_contains_params_or_vars(ty))
                    || end.is_some_and(|ty| self.type_contains_params_or_vars(ty))
            }
            TypeKind::Array { elem, len } => {
                self.type_contains_params_or_vars(elem)
                    || self.const_generic_contains_params_or_vars(len)
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::EnumPayload(_, args)
            | TypeKind::Associated(_, args)
            | TypeKind::FnDef(_, args) => args
                .into_iter()
                .any(|arg| self.generic_arg_contains_params_or_vars(arg)),
            TypeKind::TraitObject(_, args, assoc_bindings) => {
                args.into_iter()
                    .any(|arg| self.generic_arg_contains_params_or_vars(arg))
                    || assoc_bindings
                        .into_iter()
                        .any(|(_, ty)| self.type_contains_params_or_vars(ty))
            }
            TypeKind::Projection {
                target,
                trait_args,
                assoc_args,
                ..
            } => {
                self.type_contains_params_or_vars(target)
                    || trait_args
                        .into_iter()
                        .any(|arg| self.generic_arg_contains_params_or_vars(arg))
                    || assoc_args
                        .into_iter()
                        .any(|arg| self.generic_arg_contains_params_or_vars(arg))
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_params_or_vars(param))
                    || self.type_contains_params_or_vars(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_params_or_vars(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_params_or_vars(param))
                    || self.type_contains_params_or_vars(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_params_or_vars(field.ty)),
            TypeKind::AnonymousEnum(enum_def) => {
                enum_def
                    .backing_ty
                    .is_some_and(|ty| self.type_contains_params_or_vars(ty))
                    || enum_def.variants.into_iter().any(|variant| {
                        variant
                            .payload_ty
                            .is_some_and(|payload_ty| self.type_contains_params_or_vars(payload_ty))
                    })
            }
            TypeKind::Primitive(_) | TypeKind::Module(_) => false,
        }
    }

    fn generic_arg_contains_params_or_vars(&mut self, arg: GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => self.type_contains_params_or_vars(ty),
            GenericArg::Const(value) => self.const_generic_contains_params_or_vars(value),
        }
    }

    fn const_generic_contains_params_or_vars(&mut self, value: ConstGeneric) -> bool {
        self.ctx.type_registry.const_generic_contains_params(value)
    }

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
        let (param_tys, ret_ty) = match self.ctx.type_registry.get(sig_ty) {
            TypeKind::Function { params, ret, .. } => (params.clone(), *ret),
            _ => (Vec::new(), TypeId::ERROR),
        };

        // 3. Rebuild the function-local scope.
        let setup_started = collect_timings.then(Instant::now);
        self.ctx.scopes.set_current_scope(parent_scope);
        let function_scope = self.ctx.scopes.enter_scope();
        self.bind_generics_into_scope(&f.generics, function_scope);

        let prev_bounds_len = self.ctx.analysis.active_bounds.len();
        self.push_valid_where_clause_bounds(&f.where_clauses);
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
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
        let parameter_bindings = f
            .params
            .iter()
            .enumerate()
            .filter_map(|(i, param_ast)| {
                (self.ctx.resolve(param_ast.pattern.name) != "_")
                    .then_some((param_ast.pattern.name, i))
            })
            .collect::<Vec<_>>();
        if let Some(info) = &f.default_trait_method {
            let trait_generics = self
                .ctx
                .defs
                .get(info.trait_id.0 as usize)
                .and_then(|def| match def {
                    Def::Trait(trait_def) => Some(trait_def.generics.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let trait_args = trait_generics
                .iter()
                .map(|param| match &param.kind {
                    ast::GenericParamKind::Type => {
                        GenericArg::Type(self.ctx.type_registry.intern(TypeKind::Param(param.name)))
                    }
                    ast::GenericParamKind::Const { ty } => GenericArg::Const(ConstGeneric::Param(
                        param.name,
                        self.ctx.node_type_or_error(ty.id),
                    )),
                })
                .collect::<Vec<_>>();
            let trait_ty = self.ctx.type_registry.intern(TypeKind::TraitObject(
                info.trait_id,
                trait_args,
                Vec::new(),
            ));
            let self_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::Param(info.self_param));
            self.ctx
                .analysis
                .active_bounds
                .push((self_ty, vec![trait_ty]));
            self.ctx.clear_active_bound_caches();
        }
        let cancellation = self.cancellation.clone();
        let mut checker = ExprChecker::with_cancellation(self.ctx, Some(ret_ty), cancellation);
        for (name, index) in parameter_bindings {
            checker.record_parameter_binding(name, index);
        }
        let body_eval_ty = {
            let body_eval_ty = checker.check_expr(body_expr, Some(ret_ty));
            checker.finalize_numeric_inference(body_eval_ty)
        };
        let stored_params = checker.stored_parameters.clone();
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
            } else if checker.reject_returned_capturing_closure(body_expr, ret_ty, body_eval_ty) {
                // The function's trailing expression would return a dangling stack closure.
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
        self.ctx
            .analysis
            .escape_summaries
            .insert(f.id, EscapeSummary { stored_params });

        self.ctx.analysis.active_bounds.truncate(prev_bounds_len); // Drop bounds introduced by this function scope.
        self.ctx.clear_active_bound_caches();
        self.ctx.scopes.exit_scope(); // Leave the function scope.

        if let Some(function_started) = function_started {
            let elapsed = function_started.elapsed();
            match kind {
                FunctionBodyKind::TopLevel => self.body_timings.top_level_functions += elapsed,
                FunctionBodyKind::ImplMethod | FunctionBodyKind::TraitDefaultMethod => {
                    self.body_timings.impl_methods += elapsed
                }
            }
        }
    }

    fn check_struct(&mut self, s: &crate::def::StructDef, parent_scope: ScopeId) {
        // 1. Rebuild the struct-local scope so default values can see generic parameters.
        self.ctx.scopes.set_current_scope(parent_scope);
        let struct_scope = self.ctx.scopes.enter_scope();
        self.bind_generics_into_scope(&s.generics, struct_scope);

        // 2. Check every field default expression.
        for field in &s.fields {
            if self.check_canceled().is_err() {
                break;
            }
            if let Some(default_expr) = &field.default_value {
                // Resolve the field's expected semantic type.
                let field_ty = self.ctx.node_type_or_error(field.type_node.id);

                // Type-check the default expression.
                let mut checker =
                    ExprChecker::with_cancellation(self.ctx, None, self.cancellation.clone());
                let eval_ty = {
                    let eval_ty = checker.check_expr(default_expr, Some(field_ty));
                    checker.finalize_numeric_inference(eval_ty)
                };

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
        let union_scope = self.ctx.scopes.enter_scope();
        self.bind_generics_into_scope(&u.generics, union_scope);

        // 2. Check every field default expression.
        for field in &u.fields {
            if self.check_canceled().is_err() {
                break;
            }
            if let Some(default_expr) = &field.default_value {
                // Resolve the field's expected semantic type.
                let field_ty = self.ctx.node_type_or_error(field.type_node.id);

                // Type-check the default expression.
                let mut checker =
                    ExprChecker::with_cancellation(self.ctx, None, self.cancellation.clone());
                let eval_ty = {
                    let eval_ty = checker.check_expr(default_expr, Some(field_ty));
                    checker.finalize_numeric_inference(eval_ty)
                };

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
        self.bind_generics_into_scope(&i.generics, impl_scope);

        if let Some(violation) = self.ctx.non_decreasing_impl_requirement(i.id) {
            let head_str = format!(
                "{}: {}",
                self.ctx.ty_to_string(violation.head_target_ty),
                self.ctx.ty_to_string(violation.head_trait_ty)
            );
            let requirement_str = format!(
                "{}: {}",
                self.ctx.ty_to_string(violation.requirement_target_ty),
                self.ctx.ty_to_string(violation.requirement_trait_ty)
            );
            let issue_hint = self.ctx.describe_paterson_issue(&violation.issue);
            let issue_label = self.ctx.describe_paterson_issue_brief(&violation.issue);
            self.ctx
                .struct_error(
                    violation.bound_span,
                    "impl prerequisite is not structurally bounded by the impl head",
                )
                .with_hint(
                    "termination check: a prerequisite may duplicate head parameters only after moving to a structurally smaller receiver",
                )
                .with_hint(format!("impl head: `{}`", head_str))
                .with_hint(format!("prerequisite: `{}`", requirement_str))
                .with_hint(issue_hint)
                .with_hint(
                    "common fix: put the bound on a smaller field/pointee/slice element instead of on the original input type",
                )
                .with_hint(
                    "use a prerequisite on smaller pieces of the input, or split the proof through an acyclic helper impl",
                )
                .with_span_label(violation.bound_span, issue_label)
                .with_span_label(i.span, "while checking this impl")
                .emit();
            self.ctx.scopes.exit_scope();
            return;
        }

        if let Some(requirement) = self.ctx.direct_self_referential_impl_requirement(i) {
            let target_str = self.ctx.ty_to_string(requirement.target_ty);
            let trait_str = self.ctx.ty_to_string(requirement.trait_ty);
            self.ctx
                .struct_error(
                    requirement.bound_span,
                    "impl cannot require itself in its own where-clause",
                )
                .with_hint(format!(
                    "this impl tries to prove `{}: {}` by assuming the same requirement",
                    target_str, trait_str
                ))
                .with_hint("remove the identical prerequisite, or replace it with a strictly smaller helper obligation")
                .with_hint(
                    "remove the self-referential bound or introduce a different prerequisite trait",
                )
                .with_span_label(i.span, "while checking this impl")
                .emit();
            self.ctx.scopes.exit_scope();
            return;
        }
        if let Some(cycle) = self.ctx.indirect_self_referential_impl_requirement(i.id) {
            let start_obligation = format!(
                "{}: {}",
                self.ctx.ty_to_string(cycle.target_ty),
                self.ctx.ty_to_string(cycle.trait_ty)
            );
            let mut chain = vec![start_obligation.clone()];
            for requirement in &cycle.requirements {
                chain.push(format!(
                    "{}: {}",
                    self.ctx.ty_to_string(requirement.target_ty),
                    self.ctx.ty_to_string(requirement.trait_ty)
                ));
            }
            let followup_labels = cycle
                .requirements
                .iter()
                .skip(1)
                .map(|requirement| {
                    let proof = format!(
                        "{}: {}",
                        self.ctx.ty_to_string(requirement.target_ty),
                        self.ctx.ty_to_string(requirement.trait_ty)
                    );
                    (
                        requirement.requirement_span,
                        format!("the proof then requires `{}`", proof),
                    )
                })
                .collect::<Vec<_>>();
            let followup_impl_labels = cycle
                .requirements
                .iter()
                .skip(1)
                .filter_map(|requirement| {
                    let impl_span = match self.ctx.defs.get(requirement.impl_id.0 as usize) {
                        Some(Def::Impl(impl_def)) => impl_def.span,
                        _ => Span::default(),
                    };
                    if impl_span == Span::default() {
                        None
                    } else {
                        Some((impl_span, "this impl contributes another edge in the cycle"))
                    }
                })
                .collect::<Vec<_>>();

            let mut diag = self
                .ctx
                .struct_error(
                    cycle.start_bound_span,
                    "impl requirement participates in a cyclic proof",
                )
                .with_hint(format!("proof cycle: {}", chain.join(" -> ")))
                .with_hint(format!(
                    "the prerequisite `{}` eventually depends on `{}` again",
                    chain[1], start_obligation
                ))
                .with_hint(
                    "follow the labeled prerequisites in order: one of them must stop requiring the next edge in the chain",
                )
                .with_hint(
                    "remove one edge in the cycle or add an acyclic impl that discharges one prerequisite",
                )
                .with_span_label(cycle.start_bound_span, "this prerequisite starts the cycle")
                .with_span_label(i.span, "while checking this impl");
            for (span, label) in followup_labels {
                diag = diag.with_span_label(span, label);
            }
            for (span, label) in followup_impl_labels {
                diag = diag.with_span_label(span, label);
            }
            diag.emit();
            self.ctx.scopes.exit_scope();
            return;
        }

        let prev_bounds_len = self.ctx.analysis.active_bounds.len();
        let target_ty = self.ctx.normalized_node_type_or_error(i.target_type.id);
        if let Some(trait_ty_node) = &i.trait_type {
            let trait_ty = self.ctx.normalized_node_type_or_error(trait_ty_node.id);
            if target_ty != TypeId::ERROR
                && trait_ty != TypeId::ERROR
                && matches!(
                    self.ctx.type_registry.get(trait_ty),
                    TypeKind::TraitObject(..)
                )
            {
                self.ctx
                    .analysis
                    .active_bounds
                    .push((target_ty, vec![trait_ty]));
            }
        }
        self.push_valid_where_clause_bounds(&i.where_clauses);
        if self.ctx.analysis.active_bounds.len() != prev_bounds_len {
            self.ctx.clear_active_bound_caches();
        }

        // Inject the `Self` type for the impl target.
        let self_sym = self.ctx.intern("Self");
        let _ = self.ctx.scopes.define(
            self_sym,
            SymbolInfo {
                kind: SymbolKind::TypeAlias,
                node_id: i.target_type.id,
                type_id: target_ty,
                def_id: None,
                span: Span::default(),
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
            // SAFETY: same as `check_item`; method definitions live in immutable `ctx.defs`
            // during type checking.
            unsafe {
                if let Def::Function(f) = &*method_def {
                    self.check_function(f, impl_scope, FunctionBodyKind::ImplMethod);
                }
            }
        }

        self.ctx.analysis.active_bounds.truncate(prev_bounds_len);
        self.ctx.clear_active_bound_caches();
        self.ctx.scopes.exit_scope();
    }
}

fn def_primary_span(def: &Def) -> Option<Span> {
    match def {
        Def::Module(module) => Some(Span {
            file: module.file_id,
            start: 0,
            end: 0,
        }),
        Def::Function(function) => Some(function.span),
        Def::Struct(strukt) => Some(strukt.span),
        Def::Union(union) => Some(union.span),
        Def::Enum(enm) => Some(enm.span),
        Def::Trait(trait_def) => Some(trait_def.span),
        Def::AssociatedType(assoc) => Some(assoc.span),
        Def::Impl(imp) => Some(imp.span),
        Def::Global(global) => Some(global.span),
        Def::TypeAlias(alias) => Some(alias.span),
    }
}

fn normalize_checker_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
