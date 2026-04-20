use super::*;

pub(in crate::compiler) fn collect_module_for_completion_facts(
    module: &ast::Module,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    for decl in &module.decls {
        collect_decl_for_completion_facts(decl, expr_binding_items_by_span, for_facts_by_span);
    }
}

fn collect_decl_for_completion_facts(
    decl: &ast::Decl,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            collect_expr_for_completion_facts(body, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::DeclKind::Var { value, .. } => {
            collect_expr_for_completion_facts(value, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_for_completion_facts(
                    child,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        _ => {}
    }
}

fn collect_expr_for_completion_facts(
    expr: &ast::Expr,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                collect_stmt_for_completion_facts(
                    stmt,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
            if let Some(result) = result {
                collect_expr_for_completion_facts(
                    result,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::ExprKind::Let {
            init, else_clause, ..
        } => {
            collect_expr_for_completion_facts(init, expr_binding_items_by_span, for_facts_by_span);
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_expr_for_completion_facts(
                            else_expr,
                            expr_binding_items_by_span,
                            for_facts_by_span,
                        );
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_expr_for_completion_facts(
                                &arm.body,
                                expr_binding_items_by_span,
                                for_facts_by_span,
                            );
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static { init, .. }
        | ast::ExprKind::FieldAccess { lhs: init, .. }
        | ast::ExprKind::Unary { operand: init, .. }
        | ast::ExprKind::As { lhs: init, .. }
        | ast::ExprKind::GenericInstantiation { target: init, .. }
        | ast::ExprKind::Defer { expr: init } => {
            collect_expr_for_completion_facts(init, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_for_completion_facts(lhs, expr_binding_items_by_span, for_facts_by_span);
            collect_expr_for_completion_facts(rhs, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_for_completion_facts(
                callee,
                expr_binding_items_by_span,
                for_facts_by_span,
            );
            for arg in args {
                collect_expr_for_completion_facts(
                    arg,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_for_completion_facts(
                literal,
                expr_binding_items_by_span,
                for_facts_by_span,
            );
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_for_completion_facts(cond, expr_binding_items_by_span, for_facts_by_span);
            collect_expr_for_completion_facts(
                then_branch,
                expr_binding_items_by_span,
                for_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_for_completion_facts(
                    else_branch,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_for_completion_facts(
                target,
                expr_binding_items_by_span,
                for_facts_by_span,
            );
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_match_pattern_for_completion_facts(
                        pattern,
                        expr_binding_items_by_span,
                        for_facts_by_span,
                    );
                }
                collect_expr_for_completion_facts(
                    &arm.body,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            let mut scope_items = Vec::new();
            if let Some(init) = init {
                collect_expr_for_completion_facts(
                    init,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
                if let Some(items) = expr_binding_items_by_span.get(&query_span_for_expr(init)) {
                    scope_items.extend(items.iter().cloned());
                }
            }
            for_facts_by_span.insert(
                query_span_for_expr(expr),
                CompletionForFacts { scope_items },
            );
            if let Some(cond) = cond {
                collect_expr_for_completion_facts(
                    cond,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
            if let Some(post) = post {
                collect_expr_for_completion_facts(
                    post,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
            collect_expr_for_completion_facts(body, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_for_completion_facts(lhs, expr_binding_items_by_span, for_facts_by_span);
            if let Some(start) = start {
                collect_expr_for_completion_facts(
                    start,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
            if let Some(end) = end {
                collect_expr_for_completion_facts(
                    end,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_for_completion_facts(value, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_expr_for_completion_facts(
                    &capture.value,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
            collect_expr_for_completion_facts(body, expr_binding_items_by_span, for_facts_by_span);
        }
        _ => {}
    }
}

fn collect_stmt_for_completion_facts(
    stmt: &ast::Stmt,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    if let Some(expr) = stmt_expr(stmt) {
        collect_expr_for_completion_facts(expr, expr_binding_items_by_span, for_facts_by_span);
    }
}

fn collect_data_literal_for_completion_facts(
    literal: &ast::DataLiteralKind,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_for_completion_facts(
                    &field.value,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_for_completion_facts(
                    item,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_for_completion_facts(value, expr_binding_items_by_span, for_facts_by_span);
            collect_expr_for_completion_facts(count, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_for_completion_facts(value, expr_binding_items_by_span, for_facts_by_span);
        }
    }
}

fn collect_match_pattern_for_completion_facts(
    pattern: &ast::MatchPattern,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    for_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionForFacts>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_expr_for_completion_facts(value, expr_binding_items_by_span, for_facts_by_span);
        }
        ast::MatchPatternKind::Range { start, end, .. } => {
            collect_expr_for_completion_facts(start, expr_binding_items_by_span, for_facts_by_span);
            collect_expr_for_completion_facts(end, expr_binding_items_by_span, for_facts_by_span);
        }
        _ => {}
    }
}
