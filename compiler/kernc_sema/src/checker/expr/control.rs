use super::ExprChecker;
use crate::def::Def;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind, StmtKind};
use kernc_utils::{DiagnosticCode, Span, SymbolId};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn match_enum_def(
        &mut self,
        def_id: crate::def::DefId,
        span: Span,
        context: &str,
    ) -> Option<*const crate::def::EnumDef> {
        match self.ctx.defs.get(def_id.0 as usize) {
            Some(Def::Enum(def)) => Some(std::ptr::from_ref(def)),
            Some(other) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Expected enum definition while trying to {}, found {:?}.",
                        context, other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Missing DefId {} while trying to {}.",
                        def_id.0, context
                    ),
                );
                None
            }
        }
    }

    fn top_level_pattern_variant_name(&self, pattern: &ast::Pattern) -> Option<SymbolId> {
        match &pattern.kind {
            ast::PatternKind::Variant(variant) => Some(variant.variant_name),
            ast::PatternKind::Destructure(destructure) if destructure.fields.len() == 1 => {
                Some(destructure.fields[0].name)
            }
            _ => None,
        }
    }

    /// Core match-checking logic, including environment extraction and exhaustiveness.
    pub(crate) fn check_match_expr(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let norm_target = self.resolve_tv(target_ty);

        if norm_target == TypeId::ERROR {
            for arm in arms {
                self.check_expr(&arm.body, None);
            }
            return TypeId::ERROR;
        }

        // Detect whether the matched target is an ADT-backed enum.
        let is_adt = matches!(
            self.ctx.type_registry.get(norm_target),
            TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
        );

        let mut common_ret_ty = expected_ty;
        let mut handled_variants = std::collections::HashSet::new();
        let mut has_catch_all = false;

        for arm in arms {
            let body_ty = self.check_match_arm(
                arm,
                norm_target,
                is_adt,
                common_ret_ty,
                &mut handled_variants,
                &mut has_catch_all,
            );

            if common_ret_ty.is_none() || common_ret_ty == Some(TypeId::NEVER) {
                common_ret_ty = Some(body_ty);
            } else if let Some(common_ty) = common_ret_ty.filter(|ty| *ty != TypeId::NEVER)
                && body_ty != TypeId::NEVER
            {
                let body_started = self.timing_start();
                self.check_coercion(&arm.body, common_ty, body_ty);
                self.record_expr_timing(body_started, |stats, elapsed| {
                    stats.control_match_bodies += elapsed;
                });
            }
        }

        // --- Exhaustiveness checking ---
        if !has_catch_all {
            let exhaustiveness_started = self.timing_start();
            if is_adt {
                let missing: Vec<_> = match self.ctx.type_registry.get(norm_target) {
                    TypeKind::Enum(def_id, _) => self
                        .match_enum_def(*def_id, span, "check match exhaustiveness")
                        .map(|adt_def| unsafe {
                            let adt_def = &*adt_def;
                            adt_def
                                .variants
                                .iter()
                                .filter(|v| !handled_variants.contains(&v.name))
                                .map(|v| self.ctx.resolve(v.name).to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                    TypeKind::AnonymousEnum(enum_def) => enum_def
                        .variants
                        .iter()
                        .filter(|v| !handled_variants.contains(&v.name))
                        .map(|v| self.ctx.resolve(v.name).to_string())
                        .collect(),
                    _ => Vec::new(),
                };

                if !missing.is_empty() {
                    self.ctx
                        .struct_error(span, "match expression is not exhaustive")
                        .with_code(DiagnosticCode::NonexhaustiveMatch)
                        .with_hint(format!("missing variants: {}", missing.join(", ")))
                        .emit();
                }
            } else {
                // Non-ADT matches require a catch-all arm.
                self.ctx
                    .struct_error(span, "match expression must be exhaustive")
                    .with_code(DiagnosticCode::NonexhaustiveMatch)
                    .with_hint("for non-ADT types (like integers or strings), consider adding an `else =>` catch-all branch")
                    .emit();
            }
            self.record_expr_timing(exhaustiveness_started, |stats, elapsed| {
                stats.control_match_exhaustiveness += elapsed;
            });
        }

        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    /// Check a single match arm in isolation.
    fn check_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        norm_target: TypeId,
        is_adt: bool,
        common_ret_ty: Option<TypeId>,
        handled_variants: &mut std::collections::HashSet<SymbolId>,
        has_catch_all: &mut bool,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();

        let pattern_started = self.timing_start();
        for pat in &arm.patterns {
            match &pat.kind {
                ast::MatchPatternKind::Value(v) => {
                    let v_ty = self.check_expr(v, Some(norm_target));
                    self.check_coercion(v, norm_target, v_ty);

                    // Recover enum-literal information from value patterns for exhaustiveness checks.
                    if is_adt && let ExprKind::EnumLiteral { variant, .. } = &v.kind {
                        handled_variants.insert(*variant);
                    }
                }
                ast::MatchPatternKind::Range { start, end, .. } => {
                    let s_ty = self.check_expr(start, Some(norm_target));
                    let e_ty = self.check_expr(end, Some(norm_target));
                    self.check_coercion(start, norm_target, s_ty);
                    self.check_coercion(end, norm_target, e_ty);
                }
                ast::MatchPatternKind::Pattern(pattern) => {
                    self.check_pattern(arm.body.id, pattern, norm_target);

                    if is_adt
                        && let Some(variant_name) = self.top_level_pattern_variant_name(pattern)
                    {
                        handled_variants.insert(variant_name);
                    }

                    if self.pattern_is_irrefutable(pattern, norm_target) {
                        *has_catch_all = true;
                    }
                }
            }
        }
        self.record_expr_timing(pattern_started, |stats, elapsed| {
            stats.control_match_patterns += elapsed;
        });

        let body_started = self.timing_start();
        let body_ty = self.check_expr(&arm.body, common_ret_ty);
        self.record_expr_timing(body_started, |stats, elapsed| {
            stats.control_match_bodies += elapsed;
        });
        self.ctx.scopes.exit_scope();
        body_ty
    }

    pub(crate) fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            // Thread the function's expected return type into the returned expression.
            let val_ty = self.check_expr(v, Some(expected_ret));

            if let Some(ret_ty) = self.current_return_type {
                self.check_coercion(v, ret_ty, val_ty);
            }
        } else if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
            let ret_str = self.ctx.ty_to_string(expected_ret);
            self.ctx
                .struct_error(span, "expected a return value, but found empty return")
                .with_hint(format!("function is expected to return `{}`", ret_str))
                .emit();
        }
        TypeId::VOID
    }

    pub(crate) fn check_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        if let Some(i) = init {
            let _ = self.check_expr(i, None);
        }
        if let Some(c) = cond {
            let c_ty = self.check_expr(c, Some(TypeId::BOOL));
            self.check_coercion(c, TypeId::BOOL, c_ty);
        }
        if let Some(p) = post {
            let _ = self.check_expr(p, None);
        }
        let _ = self.check_expr(body, None);
        self.ctx.scopes.exit_scope();
        TypeId::VOID
    }

    /// Check whether a standalone expression illegally discards a non-void value.
    fn check_discarded_expr(&mut self, expr: &Expr) -> TypeId {
        let ty = self.check_expr(expr, None);
        let norm_ty = self.resolve_tv(ty);

        // Only `void`, `never`, or already-invalid expressions may be dropped implicitly.
        if norm_ty != TypeId::VOID && norm_ty != TypeId::NEVER && norm_ty != TypeId::ERROR {
            let ty_str = self.ctx.ty_to_string(ty);
            self.ctx
                .struct_error(expr.span, "ignored non-void return value")
                .with_code(DiagnosticCode::IgnoredNonvoidValue)
                .with_hint(format!(
                    "expression evaluates to `{}`, which must be explicitly used or discarded",
                    ty_str
                ))
                .with_hint("in Kern, use `let _ = ...;` to explicitly discard the value")
                .emit();
        }
        ty
    }

    pub(crate) fn check_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let outer_scope = self.ctx.scopes.current_scope_id();
        let mut entered_scope = false;
        let mut saw_diverging_stmt = false;
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                    let needs_scope_extension = match &e.kind {
                        ExprKind::Let { pattern, .. } => {
                            self.let_pattern_needs_scope_extension(pattern, entered_scope)
                        }
                        ExprKind::Static { pattern, .. } => {
                            self.binding_pattern_needs_scope_extension(pattern, entered_scope)
                        }
                        _ => false,
                    };
                    if needs_scope_extension {
                        // The first binding creates the block-local environment. Subsequent
                        // bindings only need a fresh child scope when they shadow a visible name.
                        entered_scope = true;
                        self.ctx.scopes.enter_scope();
                    }
                    let stmt_ty = self.check_discarded_expr(e);
                    if self.resolve_tv(stmt_ty) == TypeId::NEVER {
                        saw_diverging_stmt = true;
                    }
                }
            }
        }
        let ret_ty = if saw_diverging_stmt {
            if let Some(res) = result {
                let _ = self.check_expr(res, expected_ty);
            }
            TypeId::NEVER
        } else if let Some(res) = result {
            self.check_expr(res, expected_ty)
        } else {
            TypeId::VOID
        };
        if entered_scope {
            if let Some(scope_id) = outer_scope {
                self.ctx.scopes.set_current_scope(scope_id);
            } else {
                self.ctx.scopes.exit_scope();
            }
        } else if let Some(scope_id) = outer_scope {
            self.ctx.scopes.set_current_scope(scope_id);
        }
        ret_ty
    }

    pub(crate) fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond, TypeId::BOOL, cond_ty);

        let then_ty = self.check_expr(then_branch, expected_ty);
        if let Some(else_expr) = else_branch {
            let else_ty = self.check_expr(else_expr, expected_ty);

            // If one branch diverges, use the other branch's type.
            if then_ty == TypeId::NEVER {
                return else_ty;
            } else if else_ty == TypeId::NEVER {
                return then_ty;
            }

            self.check_coercion(else_expr, then_ty, else_ty);
            then_ty
        } else {
            TypeId::VOID
        }
    }

    pub(crate) fn check_defer(&mut self, defer_expr: &Expr) -> TypeId {
        let _ = self.check_discarded_expr(defer_expr);
        TypeId::VOID
    }
}
