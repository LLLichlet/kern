use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    fn resolve_pattern(&mut self, pattern: &ast::Pattern, scope: ScopeId) {
        match &pattern.kind {
            ast::PatternKind::Binding(_)
            | ast::PatternKind::Ignore
            | ast::PatternKind::Variant(_) => {
                if let ast::PatternKind::Variant(variant) = &pattern.kind
                    && let Some(ty) = &variant.target_type
                {
                    self.resolve_type(ty, scope);
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                if let Some(ty) = &destructure.target_type {
                    self.resolve_type(ty, scope);
                }
                for field in &destructure.fields {
                    self.resolve_pattern(&field.pattern, scope);
                }
            }
        }
    }

    pub(super) fn resolve_expr(&mut self, expr: &ast::Expr, scope: ScopeId) {
        match &expr.kind {
            ast::ExprKind::Let {
                pattern,
                init,
                else_clause,
            } => {
                self.resolve_pattern(&pattern.pattern, scope);
                self.resolve_expr(init, scope);
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(else_expr) => self.resolve_expr(else_expr, scope),
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                self.resolve_pattern(&arm.pattern, scope);
                                self.resolve_expr(&arm.body, scope);
                            }
                        }
                    }
                }
            }
            ast::ExprKind::Static { init, .. } => {
                self.resolve_expr(init, scope);
            }
            ast::ExprKind::As { lhs, target } => {
                self.resolve_expr(lhs, scope);
                self.resolve_type(target, scope);
            }
            ast::ExprKind::TypeNode(type_node) => {
                self.resolve_type(type_node, scope);
            }
            ast::ExprKind::Block { stmts, result } => {
                let prev_scope = self.ctx.scopes.current_scope_id();
                let mut block_scope = scope;
                let mut entered_scope = false;

                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::Use(use_stmt) => {
                            let import = ImportDef {
                                path_kind: use_stmt.kind,
                                path: use_stmt.path.clone(),
                                target: use_stmt.target.clone(),
                                vis: Visibility::Private,
                                span: stmt.span,
                                binding_span: use_stmt.binding_span,
                            };

                            let needs_scope_extension = ImportResolver::binding_names(&import)
                                .into_iter()
                                .any(|name| {
                                    !entered_scope
                                        || self.ctx.scopes.resolve_from(block_scope, name).is_some()
                                });

                            if needs_scope_extension {
                                self.ctx.scopes.set_current_scope(block_scope);
                                block_scope = self.ctx.scopes.enter_scope();
                                entered_scope = true;
                            }

                            let Some(current_module) = self.ctx.module_for_scope(block_scope)
                            else {
                                self.ctx.emit_ice(
                                    stmt.span,
                                    "Kern ICE (Types): could not determine module for a local import",
                                );
                                continue;
                            };

                            {
                                let mut resolver = ImportResolver::new(self.ctx);
                                let _ = resolver.resolve_import_into_scope(
                                    current_module,
                                    block_scope,
                                    &import,
                                    true,
                                );
                            }
                        }
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            self.resolve_expr(e, block_scope);
                        }
                    }
                }
                if let Some(r) = result {
                    self.resolve_expr(r, block_scope);
                }
                if let Some(prev_scope) = prev_scope {
                    self.ctx.scopes.set_current_scope(prev_scope);
                }
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(then_branch, scope);
                if let Some(e) = else_branch {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.resolve_expr(target, scope);
                for arm in arms {
                    for pat in &arm.patterns {
                        match &pat.kind {
                            ast::MatchPatternKind::Value(e) => self.resolve_expr(e, scope),
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.resolve_expr(start, scope);
                                self.resolve_expr(end, scope);
                            }
                            ast::MatchPatternKind::Pattern(pattern) => {
                                self.resolve_pattern(pattern, scope);
                            }
                        }
                    }
                    self.resolve_expr(&arm.body, scope);
                }
            }
            ast::ExprKind::While { cond, body } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                for cap in captures {
                    self.resolve_expr(&cap.value, scope);
                }
                for param in params {
                    self.resolve_type(&param.type_node, scope);
                }
                self.resolve_type(ret_type, scope);
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(rhs, scope);
            }
            ast::ExprKind::Unary { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                self.resolve_expr(lhs, scope);
            }
            ast::ExprKind::Propagate { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(index, scope);
            }
            ast::ExprKind::Call { callee, args } => {
                self.resolve_expr(callee, scope);
                for arg in args {
                    self.resolve_expr(arg, scope);
                }
            }
            ast::ExprKind::GenericInstantiation { target, args } => {
                self.resolve_expr(target, scope);
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty) => {
                            if let Some(expr) = self.reinterpret_type_arg_as_const_expr(ty)
                                && (self.expr_references_const_param(&expr, scope)
                                    || self.type_arg_is_payloadless_enum_value_ref(ty, scope))
                            {
                                self.resolve_expr(&expr, scope);
                            } else {
                                self.resolve_type(ty, scope);
                            }
                        }
                        ast::GenericArg::AssocBinding { value: ty, .. } => {
                            self.resolve_type(ty, scope);
                        }
                        ast::GenericArg::ConstExpr(expr) => self.resolve_expr(expr, scope),
                    }
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                if let Some(ty) = type_node {
                    self.resolve_type(ty, scope);
                }
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        for f in fields {
                            self.resolve_expr(&f.value, scope);
                        }
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        for e in elems {
                            self.resolve_expr(e, scope);
                        }
                    }
                    ast::DataLiteralKind::Repeat { value, count } => {
                        self.resolve_expr(value, scope);
                        self.resolve_expr(count, scope);
                    }
                    ast::DataLiteralKind::Scalar(inner) => {
                        self.resolve_expr(inner, scope);
                    }
                }
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.resolve_expr(lhs, scope);
                if let Some(s) = start {
                    self.resolve_expr(s, scope);
                }
                if let Some(e) = end {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Defer { expr: e } => self.resolve_expr(e, scope),
            ast::ExprKind::Return(Some(e)) => self.resolve_expr(e, scope),
            _ => {}
        }
    }
}
