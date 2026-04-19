use super::*;

pub(in crate::compiler) fn collect_module_match_completion_facts(
    module: &ast::Module,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    for decl in &module.decls {
        collect_decl_match_completion_facts(
            decl,
            match_arm_binding_items_by_span,
            match_facts_by_span,
        );
    }
}

fn collect_decl_match_completion_facts(
    decl: &ast::Decl,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            collect_expr_match_completion_facts(
                body,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::DeclKind::Var { value, .. } => {
            collect_expr_match_completion_facts(
                value,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_match_completion_facts(
                    child,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        _ => {}
    }
}

fn collect_expr_match_completion_facts(
    expr: &ast::Expr,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                collect_stmt_match_completion_facts(
                    stmt,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            if let Some(result) = result {
                collect_expr_match_completion_facts(
                    result,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_expr_match_completion_facts(
                init,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_match_completion_facts(
                    else_branch,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::ExprKind::Static { init, .. }
        | ast::ExprKind::FieldAccess { lhs: init, .. }
        | ast::ExprKind::Unary { operand: init, .. }
        | ast::ExprKind::As { lhs: init, .. }
        | ast::ExprKind::GenericInstantiation { target: init, .. }
        | ast::ExprKind::Defer { expr: init } => {
            collect_expr_match_completion_facts(
                init,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_match_completion_facts(
                lhs,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            collect_expr_match_completion_facts(
                rhs,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_match_completion_facts(
                callee,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            for arg in args {
                collect_expr_match_completion_facts(
                    arg,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_match_completion_facts(
                literal,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_match_completion_facts(
                cond,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            collect_expr_match_completion_facts(
                then_branch,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_match_completion_facts(
                    else_branch,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_match_completion_facts(
                target,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            let mut arm_facts = Vec::new();
            for arm in arms {
                let binding_items = match_arm_binding_items_by_span
                    .get(&arm.span)
                    .cloned()
                    .unwrap_or_default();
                arm_facts.push(CompletionMatchArmFacts {
                    span: arm.span,
                    body_span: arm.body.span,
                    binding_items,
                });
                for pattern in &arm.patterns {
                    collect_match_pattern_match_completion_facts(
                        pattern,
                        match_arm_binding_items_by_span,
                        match_facts_by_span,
                    );
                }
                collect_expr_match_completion_facts(
                    &arm.body,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            match_facts_by_span.insert(
                query_span_for_expr(expr),
                CompletionMatchFacts { arms: arm_facts },
            );
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_expr_match_completion_facts(
                    init,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            if let Some(cond) = cond {
                collect_expr_match_completion_facts(
                    cond,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            if let Some(post) = post {
                collect_expr_match_completion_facts(
                    post,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            collect_expr_match_completion_facts(
                body,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_match_completion_facts(
                lhs,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            if let Some(start) = start {
                collect_expr_match_completion_facts(
                    start,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            if let Some(end) = end {
                collect_expr_match_completion_facts(
                    end,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_match_completion_facts(
                value,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_expr_match_completion_facts(
                    &capture.value,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
            collect_expr_match_completion_facts(
                body,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        _ => {}
    }
}

fn collect_stmt_match_completion_facts(
    stmt: &ast::Stmt,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    if let Some(expr) = stmt_expr(stmt) {
        collect_expr_match_completion_facts(expr, match_arm_binding_items_by_span, match_facts_by_span);
    }
}

fn collect_data_literal_match_completion_facts(
    literal: &ast::DataLiteralKind,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_match_completion_facts(
                    &field.value,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_match_completion_facts(
                    item,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_match_completion_facts(
                value,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            collect_expr_match_completion_facts(
                count,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_match_completion_facts(
                value,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
    }
}

fn collect_match_pattern_match_completion_facts(
    pattern: &ast::MatchPattern,
    match_arm_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_expr_match_completion_facts(
                value,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        ast::MatchPatternKind::Range { start, end, .. } => {
            collect_expr_match_completion_facts(
                start,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
            collect_expr_match_completion_facts(
                end,
                match_arm_binding_items_by_span,
                match_facts_by_span,
            );
        }
        _ => {}
    }
}
