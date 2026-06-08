//! Completion fact aggregation.
//!
//! Fact collection indexes syntax regions that need special completion behavior,
//! such as local block scopes, closure captures, if-let arms, and match arms.

use super::model::push_completion_item;
use super::*;

mod block;
mod closure;
mod if_facts;
mod match_facts;

pub(super) use self::block::collect_module_block_completion_facts;
pub(super) use self::closure::collect_module_closure_completion_facts;
pub(super) use self::if_facts::collect_module_if_completion_facts;
pub(super) use self::match_facts::collect_module_match_completion_facts;

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
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            collect_expr_binding_completion_facts(
                body,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
        }
        ast::DeclKind::Var {
            value: Some(value), ..
        } => {
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
            else_clause,
            ..
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
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_expr_binding_completion_facts(
                            else_expr,
                            items_by_span,
                            expr_binding_items_by_span,
                            match_arm_binding_items_by_span,
                            closure_binding_items_by_body_span,
                            let_else_facts_by_span,
                        );
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            let mut bindings = Vec::new();
                            collect_pattern_binding_items(
                                &arm.pattern,
                                items_by_span,
                                &mut bindings,
                            );
                            let_else_facts_by_span.insert(
                                query_span_for_expr(&arm.body),
                                CompletionLetElseFacts {
                                    binding_items: bindings,
                                },
                            );
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
                }
            }
        }
        ast::ExprKind::Static {
            pattern,
            init: Some(init),
            ..
        } => {
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
        ast::ExprKind::While { cond, body } => {
            collect_expr_binding_completion_facts(
                cond,
                items_by_span,
                expr_binding_items_by_span,
                match_arm_binding_items_by_span,
                closure_binding_items_by_body_span,
                let_else_facts_by_span,
            );
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
    if let Some(expr) = stmt_expr(stmt) {
        collect_expr_binding_completion_facts(
            expr,
            items_by_span,
            expr_binding_items_by_span,
            match_arm_binding_items_by_span,
            closure_binding_items_by_body_span,
            let_else_facts_by_span,
        );
    }
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
        ast::MatchPatternKind::Value(_) => {}
        ast::MatchPatternKind::Pattern(inner) => {
            collect_pattern_binding_items(inner, items_by_span, bindings);
        }
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
        ast::PatternKind::Ignore | ast::PatternKind::Variant(_) | ast::PatternKind::Value(_) => {}
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
        ast::DeclKind::Function {
            body: Some(body), ..
        } => regions.push(query_span_for_expr(body)),
        ast::DeclKind::Var {
            value: Some(value), ..
        } => {
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
    stmt_expr(stmt)
        .map(query_span_for_expr)
        .unwrap_or(stmt.span)
}

pub(super) fn stmt_expr(stmt: &ast::Stmt) -> Option<&ast::Expr> {
    match &stmt.kind {
        ast::StmtKind::Use(_) => None,
        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => Some(expr),
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
