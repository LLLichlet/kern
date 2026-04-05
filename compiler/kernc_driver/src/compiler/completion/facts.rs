use super::model::push_completion_item;
use super::*;

pub(super) fn collect_module_binding_completion_facts(
    module: &ast::Module,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    expr_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_arm_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_binding_items_by_body_span: &mut BTreeMap<
        kernc_utils::Span,
        Vec<AnalysisCompletionItem>,
    >,
    let_else_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
) {
    for decl in &module.decls {
        collect_decl_binding_completion_facts(
            decl,
            items_by_span,
            expr_binding_items_by_span,
            match_arm_binding_items_by_span,
            closure_binding_items_by_body_span,
            let_else_facts_by_span,
        );
    }
}

fn collect_decl_binding_completion_facts(
    decl: &ast::Decl,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    expr_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_arm_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_binding_items_by_body_span: &mut BTreeMap<
        kernc_utils::Span,
        Vec<AnalysisCompletionItem>,
    >,
    let_else_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_binding_completion_facts(
                    body,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::DeclKind::Var { value, .. } => {
            collect_expr_binding_completion_facts(
                value,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_binding_completion_facts(
                    child,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        _ => {}
    }
}

fn collect_expr_binding_completion_facts(
    expr: &ast::Expr,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    expr_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_arm_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_binding_items_by_body_span: &mut BTreeMap<
        kernc_utils::Span,
        Vec<AnalysisCompletionItem>,
    >,
    let_else_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                collect_stmt_binding_completion_facts(
                    stmt,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
            if let Some(result) = result {
                collect_expr_binding_completion_facts(
                    result,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::ExprKind::Let {
            pattern,
            init,
            else_pattern,
            else_branch,
        } => {
            let mut bindings = Vec::new();
            collect_pattern_binding_items(&pattern.pattern, items_by_span, &mut bindings);
            expr_binding_items_by_span.insert(query_span_for_expr(expr), bindings);
            collect_expr_binding_completion_facts(
                init,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                if let Some(else_pattern) = else_pattern {
                    let mut bindings = Vec::new();
                    collect_pattern_binding_items(else_pattern, items_by_span, &mut bindings);
                    let_else_facts_by_span.insert(
                        query_span_for_expr(else_branch),
                        CompletionLetElseFacts {
                            binding_items: bindings,
                        },
                    );
                }
                collect_expr_binding_completion_facts(
                    else_branch,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::ExprKind::Static { pattern, init, .. } => {
            let mut bindings = Vec::new();
            push_binding_item_from_span(items_by_span, pattern.span, &mut bindings);
            expr_binding_items_by_span.insert(query_span_for_expr(expr), bindings);
            collect_expr_binding_completion_facts(
                init,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::FieldAccess { lhs, .. }
        | ast::ExprKind::Unary { operand: lhs, .. }
        | ast::ExprKind::As { lhs, .. }
        | ast::ExprKind::GenericInstantiation { target: lhs, .. }
        | ast::ExprKind::Defer { expr: lhs } => {
            collect_expr_binding_completion_facts(
                lhs,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_binding_completion_facts(
                lhs,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            collect_expr_binding_completion_facts(
                rhs,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_binding_completion_facts(
                callee,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            for arg in args {
                collect_expr_binding_completion_facts(
                    arg,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_binding_completion_facts(
                literal,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_binding_completion_facts(
                cond,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            collect_expr_binding_completion_facts(
                then_branch,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_binding_completion_facts(
                    else_branch,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_binding_completion_facts(
                target,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            for arm in arms {
                let mut bindings = Vec::new();
                for pattern in &arm.patterns {
                    collect_match_pattern_binding_completion_facts(
                        pattern,
                        items_by_span,
                        &mut bindings,
                    );
                }
                match_arm_binding_items_by_span.insert(arm.span, bindings);
                collect_expr_binding_completion_facts(
                    &arm.body,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
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
                collect_expr_binding_completion_facts(
                    init,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
            if let Some(cond) = cond {
                collect_expr_binding_completion_facts(
                    cond,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
            if let Some(post) = post {
                collect_expr_binding_completion_facts(
                    post,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
            collect_expr_binding_completion_facts(
                body,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_binding_completion_facts(
                lhs,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            if let Some(start) = start {
                collect_expr_binding_completion_facts(
                    start,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
            if let Some(end) = end {
                collect_expr_binding_completion_facts(
                    end,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_binding_completion_facts(
                value,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::ExprKind::Closure {
            captures,
            params,
            body,
            ..
        } => {
            let mut bindings = Vec::new();
            for capture in captures {
                collect_expr_binding_completion_facts(
                    &capture.value,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
                push_binding_item_from_span(items_by_span, capture.span, &mut bindings);
            }
            for param in params {
                push_binding_item_from_span(items_by_span, param.pattern.span, &mut bindings);
            }
            closure_binding_items_by_body_span.insert(body.span, bindings);
            collect_expr_binding_completion_facts(
                body,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        _ => {}
    }
}

fn collect_stmt_binding_completion_facts(
    stmt: &ast::Stmt,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    expr_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_arm_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_binding_items_by_body_span: &mut BTreeMap<
        kernc_utils::Span,
        Vec<AnalysisCompletionItem>,
    >,
    let_else_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
) {
    collect_expr_binding_completion_facts(
        stmt_expr(stmt),
        items_by_span,
        expr_binding_items_by_span,
        match_arm_binding_items_by_span,
        closure_binding_items_by_body_span,
        let_else_facts_by_span,
    );
}

fn collect_data_literal_binding_completion_facts(
    literal: &ast::DataLiteralKind,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    expr_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    match_arm_binding_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    closure_binding_items_by_body_span: &mut BTreeMap<
        kernc_utils::Span,
        Vec<AnalysisCompletionItem>,
    >,
    let_else_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_binding_completion_facts(
                    &field.value,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_binding_completion_facts(
                    item,
                    items_by_span,
                    expr_binding_items_by_span,
                    match_arm_binding_items_by_span,
                    closure_binding_items_by_body_span,
                    let_else_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_binding_completion_facts(
                value,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
            collect_expr_binding_completion_facts(
                count,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_binding_completion_facts(
                value,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
    }
}

fn collect_match_pattern_binding_completion_facts(
    pattern: &ast::MatchPattern,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    bindings: &mut Vec<AnalysisCompletionItem>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(_) | ast::MatchPatternKind::Range { .. } => {}
        ast::MatchPatternKind::Pattern(inner) => {
            collect_pattern_binding_items(inner, items_by_span, bindings);
        }
    }
}

pub(super) fn collect_module_block_completion_facts(
    module: &ast::Module,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    for decl in &module.decls {
        collect_decl_block_completion_facts(decl, expr_binding_items_by_span, block_facts_by_span);
    }
}

fn collect_decl_block_completion_facts(
    decl: &ast::Decl,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    match &decl.kind {
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_block_completion_facts(
                    body,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::DeclKind::Var { value, .. } => {
            collect_expr_block_completion_facts(
                value,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_block_completion_facts(
                    child,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        _ => {}
    }
}

fn collect_expr_block_completion_facts(
    expr: &ast::Expr,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    match &expr.kind {
        ast::ExprKind::Block { stmts, result } => {
            let mut stmt_facts = Vec::new();
            let mut visible = Vec::new();
            for stmt in stmts {
                stmt_facts.push(CompletionBlockStmtFacts {
                    span: query_span_for_stmt(stmt),
                    prefix_items: visible.clone(),
                });
                if let Some(items) = expr_binding_items_by_span.get(&query_span_for_stmt(stmt)) {
                    for item in items {
                        push_completion_item(&mut visible, item.clone());
                    }
                }
                collect_stmt_block_completion_facts(
                    stmt,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            let tail_items = visible;
            if let Some(result) = result {
                collect_expr_block_completion_facts(
                    result,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            block_facts_by_span.insert(
                query_span_for_expr(expr),
                CompletionBlockFacts {
                    stmt_facts,
                    tail_items,
                },
            );
        }
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_expr_block_completion_facts(
                init,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_block_completion_facts(
                    else_branch,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::ExprKind::Static { init, .. }
        | ast::ExprKind::FieldAccess { lhs: init, .. }
        | ast::ExprKind::Unary { operand: init, .. }
        | ast::ExprKind::As { lhs: init, .. }
        | ast::ExprKind::GenericInstantiation { target: init, .. }
        | ast::ExprKind::Defer { expr: init } => {
            collect_expr_block_completion_facts(
                init,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_expr_block_completion_facts(
                lhs,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            collect_expr_block_completion_facts(
                rhs,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::ExprKind::Call { callee, args } => {
            collect_expr_block_completion_facts(
                callee,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            for arg in args {
                collect_expr_block_completion_facts(
                    arg,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_data_literal_block_completion_facts(
                literal,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_block_completion_facts(
                cond,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            collect_expr_block_completion_facts(
                then_branch,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            if let Some(else_branch) = else_branch {
                collect_expr_block_completion_facts(
                    else_branch,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_expr_block_completion_facts(
                target,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_match_pattern_block_completion_facts(
                        pattern,
                        expr_binding_items_by_span,
                        block_facts_by_span,
                    );
                }
                collect_expr_block_completion_facts(
                    &arm.body,
                    expr_binding_items_by_span,
                    block_facts_by_span,
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
                collect_expr_block_completion_facts(
                    init,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            if let Some(cond) = cond {
                collect_expr_block_completion_facts(
                    cond,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            if let Some(post) = post {
                collect_expr_block_completion_facts(
                    post,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            collect_expr_block_completion_facts(
                body,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_expr_block_completion_facts(
                lhs,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            if let Some(start) = start {
                collect_expr_block_completion_facts(
                    start,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            if let Some(end) = end {
                collect_expr_block_completion_facts(
                    end,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_expr_block_completion_facts(
                value,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_expr_block_completion_facts(
                    &capture.value,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
            collect_expr_block_completion_facts(
                body,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        _ => {}
    }
}

fn collect_stmt_block_completion_facts(
    stmt: &ast::Stmt,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    collect_expr_block_completion_facts(
        stmt_expr(stmt),
        expr_binding_items_by_span,
        block_facts_by_span,
    );
}

fn collect_data_literal_block_completion_facts(
    literal: &ast::DataLiteralKind,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_expr_block_completion_facts(
                    &field.value,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_expr_block_completion_facts(
                    item,
                    expr_binding_items_by_span,
                    block_facts_by_span,
                );
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_expr_block_completion_facts(
                value,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            collect_expr_block_completion_facts(
                count,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_expr_block_completion_facts(
                value,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
    }
}

fn collect_match_pattern_block_completion_facts(
    pattern: &ast::MatchPattern,
    expr_binding_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    block_facts_by_span: &mut BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_expr_block_completion_facts(
                value,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        ast::MatchPatternKind::Range { start, end, .. } => {
            collect_expr_block_completion_facts(
                start,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
            collect_expr_block_completion_facts(
                end,
                expr_binding_items_by_span,
                block_facts_by_span,
            );
        }
        _ => {}
    }
}

pub(super) fn collect_module_for_completion_facts(
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
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_for_completion_facts(
                    body,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
            }
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
            init, else_branch, ..
        } => {
            collect_expr_for_completion_facts(init, expr_binding_items_by_span, for_facts_by_span);
            if let Some(else_branch) = else_branch {
                collect_expr_for_completion_facts(
                    else_branch,
                    expr_binding_items_by_span,
                    for_facts_by_span,
                );
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
    collect_expr_for_completion_facts(
        stmt_expr(stmt),
        expr_binding_items_by_span,
        for_facts_by_span,
    );
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

pub(super) fn collect_module_match_completion_facts(
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
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_match_completion_facts(
                    body,
                    match_arm_binding_items_by_span,
                    match_facts_by_span,
                );
            }
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
    collect_expr_match_completion_facts(
        stmt_expr(stmt),
        match_arm_binding_items_by_span,
        match_facts_by_span,
    );
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

pub(super) fn collect_module_closure_completion_facts(
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
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_closure_completion_facts(
                    body,
                    closure_binding_items_by_body_span,
                    closure_facts_by_span,
                );
            }
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
    collect_expr_closure_completion_facts(
        stmt_expr(stmt),
        closure_binding_items_by_body_span,
        closure_facts_by_span,
    );
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

pub(super) fn collect_module_if_completion_facts(
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
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                collect_expr_if_completion_facts(body, if_facts_by_span);
            }
        }
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

fn collect_pattern_binding_items(
    pattern: &ast::Pattern,
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    bindings: &mut Vec<AnalysisCompletionItem>,
) {
    match &pattern.kind {
        ast::PatternKind::Binding(binding) => {
            push_binding_item_from_span(items_by_span, binding.span, bindings);
        }
        ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
        ast::PatternKind::Destructure(destructure) => {
            for field in &destructure.fields {
                collect_pattern_binding_items(&field.pattern, items_by_span, bindings);
            }
        }
    }
}

fn push_binding_item_from_span(
    items_by_span: &BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    span: kernc_utils::Span,
    bindings: &mut Vec<AnalysisCompletionItem>,
) {
    let Some(item) = items_by_span.get(&span) else {
        return;
    };
    push_completion_item(bindings, item.clone());
}

pub(in crate::compiler) fn module_body_completion_regions(
    module: &ast::Module,
) -> Vec<kernc_utils::Span> {
    let mut regions = Vec::new();
    for decl in &module.decls {
        collect_decl_body_regions(decl, &mut regions);
    }
    regions
}

pub(super) fn module_surface_decls(
    module: &ast::Module,
    function_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
) -> Vec<CompletionSurfaceDecl> {
    module
        .decls
        .iter()
        .map(|decl| surface_decl_from_decl(decl, function_items_by_span))
        .collect()
}

fn surface_decl_from_decl(
    decl: &ast::Decl,
    function_items_by_span: &BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
) -> CompletionSurfaceDecl {
    let function_items = function_items_by_span
        .get(&decl.span)
        .cloned()
        .unwrap_or_default();
    let children = match &decl.kind {
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => decls
            .iter()
            .map(|child| surface_decl_from_decl(child, function_items_by_span))
            .collect(),
        _ => Vec::new(),
    };

    CompletionSurfaceDecl {
        span: decl.span,
        function_items,
        children,
    }
}

fn collect_decl_body_regions(decl: &ast::Decl, regions: &mut Vec<kernc_utils::Span>) {
    match &decl.kind {
        ast::DeclKind::Function { body, .. } => {
            if let Some(body) = body {
                regions.push(query_span_for_expr(body));
            }
        }
        ast::DeclKind::Var { value, .. } => {
            regions.push(query_span_for_expr(value));
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_decl_body_regions(child, regions);
            }
        }
        _ => {}
    }
}

fn query_span_for_stmt(stmt: &ast::Stmt) -> kernc_utils::Span {
    query_span_for_expr(stmt_expr(stmt))
}

pub(super) fn stmt_expr(stmt: &ast::Stmt) -> &ast::Expr {
    match &stmt.kind {
        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => expr,
    }
}

pub(super) fn query_span_for_expr(expr: &ast::Expr) -> kernc_utils::Span {
    match &expr.kind {
        ast::ExprKind::Return(Some(value)) => expr.span.to(query_span_for_expr(value)),
        _ => expr.span,
    }
}

pub(super) fn span_contains_offset(span: kernc_utils::Span, offset: usize) -> bool {
    let end = if span.end > span.start {
        span.end
    } else {
        span.start.saturating_add(1)
    };
    offset >= span.start && offset < end
}
