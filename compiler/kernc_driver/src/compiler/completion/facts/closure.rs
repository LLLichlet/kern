use super::*;

pub(in crate::compiler) fn collect_module_closure_completion_facts(
    module: &ast::Module,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    for decl in &module.decls {
        collect_decl_closure_completion_facts(
            decl,
            closure_binding_items_by_body_span,
            closure_facts_by_span,
        );
    }
}

fn collect_decl_closure_completion_facts(
    decl: &ast::Decl,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            collect_expr_closure_completion_facts(
                body,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::DeclKind::Var { value, .. } => {
            collect_expr_closure_completion_facts(
                value,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_closure_completion_facts(
                    child,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        _ => {}
    }
}

fn collect_expr_closure_completion_facts(
    expr: &ast::Expr,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                collect_stmt_closure_completion_facts(
                    stmt,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
            if let Some(result) = result {
                collect_expr_closure_completion_facts(
                    result,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_expr_closure_completion_facts(
                init,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_closure_completion_facts(
                    else_branch,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::Static { init, .. }
        | ast::ExprKind::FieldAccess { lhs: init, .. }
        | ast::ExprKind::Unary { operand: init, .. }
        | ast::ExprKind::As { lhs: init, .. }
        | ast::ExprKind::GenericInstantiation { target: init, .. }
        | ast::ExprKind::Defer { expr: init } => {
            collect_expr_closure_completion_facts(
                init,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_closure_completion_facts(
                lhs,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            collect_expr_closure_completion_facts(
                rhs,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_closure_completion_facts(
                callee,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            for arg in args {
                collect_expr_closure_completion_facts(
                    arg,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_closure_completion_facts(
                literal,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_closure_completion_facts(
                cond,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            collect_expr_closure_completion_facts(
                then_branch,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_closure_completion_facts(
                    else_branch,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_closure_completion_facts(
                target,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_match_pattern_closure_completion_facts(
                        pattern,
                        closure_binding_items_by_body_span,
                        closure_facts_by_span,
                    );
                }
                collect_expr_closure_completion_facts(
                    &arm.body,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_expr_closure_completion_facts(
                    init,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
            if let Some(cond) = cond {
                collect_expr_closure_completion_facts(
                    cond,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
            if let Some(post) = post {
                collect_expr_closure_completion_facts(
                    post,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
            collect_expr_closure_completion_facts(
                body,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_closure_completion_facts(
                lhs,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            if let Some(start) = start {
                collect_expr_closure_completion_facts(
                    start,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
            if let Some(end) = end {
                collect_expr_closure_completion_facts(
                    end,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_closure_completion_facts(
                value,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::ExprKind::Closure { body, .. } => {
            let body_span = body.span;
            let binding_items = closure_binding_items_by_body_span
                .get(&body_span)
                .cloned()
                .unwrap_or_default();
            closure_facts_by_span.insert(
                query_span_for_expr(expr),
                CompletionClosureFacts {
                    body_span,
                    binding_items,
                },
            );
            collect_expr_closure_completion_facts(
                body,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        _ => {}
    }
}

fn collect_stmt_closure_completion_facts(
    stmt: &ast::Stmt,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    if let Some(expr) = stmt_expr(stmt) {
        collect_expr_closure_completion_facts(expr, closure_binding_items_by_body_span, closure_facts_by_span);
    }
}

fn collect_data_literal_closure_completion_facts(
    literal: &ast::DataLiteralKind,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_closure_completion_facts(
                    &field.value,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_closure_completion_facts(
                    item,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_closure_completion_facts(
                value,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            collect_expr_closure_completion_facts(
                count,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_closure_completion_facts(
                value,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
    }
}

fn collect_match_pattern_closure_completion_facts(
    pattern: &ast::MatchPattern,
    closure_binding_items_by_body_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_expr_closure_completion_facts(
                value,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        ast::MatchPatternKind::Range { start, end, .. } => {
            collect_expr_closure_completion_facts(
                start,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
            collect_expr_closure_completion_facts(
                end,
                closure_binding_items_by_body_span,
                closure_facts_by_span,
            );
        }
        _ => {}
    }
}
