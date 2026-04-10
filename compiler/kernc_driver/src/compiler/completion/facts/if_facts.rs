use super::*;

pub(in crate::compiler) fn collect_module_if_completion_facts(
    module: &ast::Module,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    for decl in &module.decls {
        collect_decl_if_completion_facts(decl, if_facts_by_span);
    }
}

fn collect_decl_if_completion_facts(
    decl: &ast::Decl,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => collect_expr_if_completion_facts(body, if_facts_by_span),
        ast::DeclKind::Var { value, .. } => {
            collect_expr_if_completion_facts(value, if_facts_by_span);
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_if_completion_facts(child, if_facts_by_span);
            }
        }
        _ => {}
    }
}

fn collect_expr_if_completion_facts(
    expr: &ast::Expr,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                collect_stmt_if_completion_facts(stmt, if_facts_by_span);
            }
            if let Some(result) = result {
                collect_expr_if_completion_facts(result, if_facts_by_span);
            }
        }
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_expr_if_completion_facts(init, if_facts_by_span);
            if let Some(else_branch) = else_branch {
                collect_expr_if_completion_facts(else_branch, if_facts_by_span);
            }
        }
        ast::ExprKind::Static { init, .. }
        | ast::ExprKind::FieldAccess { lhs: init, .. }
        | ast::ExprKind::Unary { operand: init, .. }
        | ast::ExprKind::As { lhs: init, .. }
        | ast::ExprKind::GenericInstantiation { target: init, .. }
        | ast::ExprKind::Defer { expr: init } => {
            collect_expr_if_completion_facts(init, if_facts_by_span);
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_if_completion_facts(lhs, if_facts_by_span);
            collect_expr_if_completion_facts(rhs, if_facts_by_span);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_if_completion_facts(callee, if_facts_by_span);
            for arg in args {
                collect_expr_if_completion_facts(arg, if_facts_by_span);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_if_completion_facts(literal, if_facts_by_span);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            if_facts_by_span.insert(
                query_span_for_expr(expr),
                CompletionIfFacts {
                    then_span: query_span_for_expr(then_branch),
                    else_span: else_branch.as_deref().map(query_span_for_expr),
                },
            );
            collect_expr_if_completion_facts(cond, if_facts_by_span);
            collect_expr_if_completion_facts(then_branch, if_facts_by_span);
            if let Some(else_branch) = else_branch {
                collect_expr_if_completion_facts(else_branch, if_facts_by_span);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_if_completion_facts(target, if_facts_by_span);
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_match_pattern_if_completion_facts(pattern, if_facts_by_span);
                }
                collect_expr_if_completion_facts(&arm.body, if_facts_by_span);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_expr_if_completion_facts(init, if_facts_by_span);
            }
            if let Some(cond) = cond {
                collect_expr_if_completion_facts(cond, if_facts_by_span);
            }
            if let Some(post) = post {
                collect_expr_if_completion_facts(post, if_facts_by_span);
            }
            collect_expr_if_completion_facts(body, if_facts_by_span);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_if_completion_facts(lhs, if_facts_by_span);
            if let Some(start) = start {
                collect_expr_if_completion_facts(start, if_facts_by_span);
            }
            if let Some(end) = end {
                collect_expr_if_completion_facts(end, if_facts_by_span);
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_if_completion_facts(value, if_facts_by_span);
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_expr_if_completion_facts(&capture.value, if_facts_by_span);
            }
            collect_expr_if_completion_facts(body, if_facts_by_span);
        }
        _ => {}
    }
}

fn collect_stmt_if_completion_facts(
    stmt: &ast::Stmt,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    collect_expr_if_completion_facts(stmt_expr(stmt), if_facts_by_span);
}

fn collect_data_literal_if_completion_facts(
    literal: &ast::DataLiteralKind,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_if_completion_facts(&field.value, if_facts_by_span);
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_if_completion_facts(item, if_facts_by_span);
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_if_completion_facts(value, if_facts_by_span);
            collect_expr_if_completion_facts(count, if_facts_by_span);
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_if_completion_facts(value, if_facts_by_span);
        }
    }
}

fn collect_match_pattern_if_completion_facts(
    pattern: &ast::MatchPattern,
    if_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionIfFacts>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_expr_if_completion_facts(value, if_facts_by_span);
        }
        ast::MatchPatternKind::Range { start, end, .. } => {
            collect_expr_if_completion_facts(start, if_facts_by_span);
            collect_expr_if_completion_facts(end, if_facts_by_span);
        }
        _ => {}
    }
}
