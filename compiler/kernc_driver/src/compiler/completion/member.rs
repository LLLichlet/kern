use super::*;

impl CompilerDriver {
    pub(super) fn collect_member_completion_items_in_module(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        module: &ast::Module,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        for decl in &module.decls {
            self.collect_member_completion_items_in_decl(
                member_query,
                module_id,
                decl,
                member_env,
                member_items_by_span,
            );
        }
    }

    fn collect_member_completion_items_in_decl(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        decl: &ast::Decl,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &decl.kind {
            ast::DeclKind::Function {
                where_clauses,
                body,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                if let Some(body) = body {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        body,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    value,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::DeclKind::Impl {
                where_clauses,
                decls,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            _ => {}
        }
    }

    fn collect_member_completion_items_in_expr(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        expr: &ast::Expr,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &expr.kind {
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if let Some(lhs_ty) = member_query.context().node_type(lhs.id) {
                    let items = member_query
                        .member_candidates_in_env(Some(module_id), lhs_ty, member_env)
                        .into_iter()
                        .filter_map(|candidate| {
                            self.completion_item_for_member_candidate(
                                member_query.context(),
                                candidate,
                            )
                        })
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        member_items_by_span.insert(expr.span, items);
                    }
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::Use(_) => {}
                        ast::StmtKind::ExprStmt(inner) | ast::StmtKind::ExprValue(inner) => {
                            self.collect_member_completion_items_in_expr(
                                member_query,
                                module_id,
                                inner,
                                member_env,
                                member_items_by_span,
                            );
                        }
                    }
                }
                if let Some(result) = result {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        result,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Let {
                init, else_clause, ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(else_expr) => {
                            self.collect_member_completion_items_in_expr(
                                member_query,
                                module_id,
                                else_expr,
                                member_env,
                                member_items_by_span,
                            );
                        }
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    &arm.body,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                        }
                    }
                }
            }
            ast::ExprKind::Static { init, .. }
            | ast::ExprKind::Unary { operand: init, .. }
            | ast::ExprKind::Defer { expr: init } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    rhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    index,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Call { callee, args } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    callee,
                    member_env,
                    member_items_by_span,
                );
                for arg in args {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        arg,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => {
                    for field in fields {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            &field.value,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Array(items) => {
                    for item in items {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            item,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        count,
                        member_env,
                        member_items_by_span,
                    );
                }
                ast::DataLiteralKind::Scalar(value) => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                }
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    cond,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    then_branch,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_branch) = else_branch {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        else_branch,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    target,
                    member_env,
                    member_items_by_span,
                );
                for arm in arms {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &arm.body,
                        member_env,
                        member_items_by_span,
                    );
                    for pattern in &arm.patterns {
                        match &pattern.kind {
                            ast::MatchPatternKind::Value(value) => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    value,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    start,
                                    member_env,
                                    member_items_by_span,
                                );
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    end,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            ast::ExprKind::While { cond, body } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    cond,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                if let Some(start) = start {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        start,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(end) = end {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        end,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Return(Some(value)) => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    value,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Return(None) => {}
            ast::ExprKind::As { lhs, .. }
            | ast::ExprKind::GenericInstantiation { target: lhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &capture.value,
                        member_env,
                        member_items_by_span,
                    );
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            _ => {}
        }
    }
}
