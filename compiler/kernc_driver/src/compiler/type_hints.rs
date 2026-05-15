use super::{AnalysisTypeHint, AnalysisTypeHintKind, CompilerDriver};
use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::TypeId;
use kernc_utils::FileId;

impl CompilerDriver {
    pub(super) fn collect_analysis_type_hints(
        &self,
        ctx: &SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> Vec<AnalysisTypeHint> {
        let mut hints = Vec::new();

        for (mod_id, module) in asts {
            let Def::Module(module_def) = &ctx.defs[mod_id.0 as usize] else {
                continue;
            };
            let file_id = module_def.file_id;

            for decl in &module.decls {
                collect_type_hints_in_decl(ctx, file_id, decl, &mut hints);
            }
        }

        hints.sort_by_key(|hint| {
            (
                hint.span.file.0,
                hint.span.start,
                hint.span.end,
                hint.label.clone(),
            )
        });
        hints.dedup_by(|left, right| {
            left.span == right.span && left.label == right.label && left.kind == right.kind
        });
        hints
    }
}

fn collect_type_hints_in_decl(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    decl: &ast::Decl,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => collect_type_hints_in_expr(ctx, file_id, body, hints),
        ast::DeclKind::Function { body: None, .. } => {}
        ast::DeclKind::Var {
            value: Some(value), ..
        } => collect_type_hints_in_expr(ctx, file_id, value, hints),
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_type_hints_in_decl(ctx, file_id, child, hints);
            }
        }
        _ => {}
    }
}

fn collect_type_hints_in_expr(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    expr: &ast::Expr,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    match &expr.kind {
        ast::ExprKind::Let {
            pattern,
            type_node,
            init,
            else_clause,
        } => {
            if type_node.is_none() {
                collect_let_pattern_type_hints(ctx, file_id, pattern, hints);
            }
            collect_type_hints_in_expr(ctx, file_id, init, hints);
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_type_hints_in_expr(ctx, file_id, else_expr, hints);
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_type_hints_in_expr(ctx, file_id, &arm.body, hints);
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static {
            pattern,
            type_node,
            init: Some(init),
        } => {
            if type_node.is_none() {
                collect_binding_type_hint(ctx, file_id, pattern, hints);
            }
            collect_type_hints_in_expr(ctx, file_id, init, hints);
        }
        ast::ExprKind::Static {
            pattern,
            type_node,
            init: None,
        } => {
            if type_node.is_none() {
                collect_binding_type_hint(ctx, file_id, pattern, hints);
            }
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_type_hints_in_expr(ctx, file_id, lhs, hints);
            collect_type_hints_in_expr(ctx, file_id, rhs, hints);
        }
        ast::ExprKind::FieldAccess { lhs, .. }
        | ast::ExprKind::Unary { operand: lhs, .. }
        | ast::ExprKind::As { lhs, .. }
        | ast::ExprKind::GenericInstantiation { target: lhs, .. }
        | ast::ExprKind::Defer { expr: lhs }
        | ast::ExprKind::Propagate { operand: lhs }
        | ast::ExprKind::Grouped { expr: lhs } => {
            collect_type_hints_in_expr(ctx, file_id, lhs, hints);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_type_hints_in_expr(ctx, file_id, callee, hints);
            for arg in args {
                collect_type_hints_in_expr(ctx, file_id, arg, hints);
            }
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_type_hints_in_expr(ctx, file_id, start, hints);
            }
            if let Some(end) = end {
                collect_type_hints_in_expr(ctx, file_id, end, hints);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => {
            collect_type_hints_in_data_literal(ctx, file_id, literal, hints);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_type_hints_in_expr(ctx, file_id, cond, hints);
            collect_type_hints_in_expr(ctx, file_id, then_branch, hints);
            if let Some(else_branch) = else_branch {
                collect_type_hints_in_expr(ctx, file_id, else_branch, hints);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_type_hints_in_expr(ctx, file_id, target, hints);
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_type_hints_in_match_pattern(ctx, file_id, pattern, hints);
                }
                collect_type_hints_in_expr(ctx, file_id, &arm.body, hints);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_type_hints_in_expr(ctx, file_id, expr, hints);
                    }
                }
            }
            if let Some(result) = result {
                collect_type_hints_in_expr(ctx, file_id, result, hints);
            }
        }
        ast::ExprKind::While { cond, body } => {
            collect_type_hints_in_expr(ctx, file_id, cond, hints);
            collect_type_hints_in_expr(ctx, file_id, body, hints);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_type_hints_in_expr(ctx, file_id, lhs, hints);
            if let Some(start) = start {
                collect_type_hints_in_expr(ctx, file_id, start, hints);
            }
            if let Some(end) = end {
                collect_type_hints_in_expr(ctx, file_id, end, hints);
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_type_hints_in_expr(ctx, file_id, value, hints)
        }
        ast::ExprKind::Return(None) => {}
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_type_hints_in_expr(ctx, file_id, &capture.value, hints);
            }
            collect_type_hints_in_expr(ctx, file_id, body, hints);
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
    }
}

fn collect_let_pattern_type_hints(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    pattern: &ast::LetPattern,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    collect_pattern_type_hints(ctx, file_id, &pattern.pattern, hints);
}

fn collect_pattern_type_hints(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    pattern: &ast::Pattern,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    match &pattern.kind {
        ast::PatternKind::Binding(binding) => {
            if binding.name_span.file != file_id {
                return;
            }
            let Some(type_id) = binding_type_for_span(ctx, binding.name_span) else {
                return;
            };
            let Some(ty) = hint_type_label(ctx, type_id) else {
                return;
            };
            hints.push(AnalysisTypeHint {
                span: binding.name_span,
                label: format!(": {ty}"),
                kind: AnalysisTypeHintKind::Variable,
            });
        }
        ast::PatternKind::Destructure(destructure) => {
            for field in &destructure.fields {
                collect_pattern_type_hints(ctx, file_id, &field.pattern, hints);
            }
        }
        ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
    }
}

fn collect_binding_type_hint(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    binding: &ast::BindingPattern,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    if binding.name_span.file != file_id {
        return;
    }
    let Some(type_id) = binding_type_for_span(ctx, binding.name_span) else {
        return;
    };
    let Some(ty) = hint_type_label(ctx, type_id) else {
        return;
    };
    hints.push(AnalysisTypeHint {
        span: binding.name_span,
        label: format!(": {ty}"),
        kind: AnalysisTypeHintKind::Variable,
    });
}

fn collect_type_hints_in_match_pattern(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    pattern: &ast::MatchPattern,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_type_hints_in_expr(ctx, file_id, value, hints)
        }
        ast::MatchPatternKind::Pattern(pattern) => {
            collect_pattern_type_hints(ctx, file_id, pattern, hints)
        }
    }
}

fn collect_type_hints_in_data_literal(
    ctx: &SemaContext<'_>,
    file_id: FileId,
    literal: &ast::DataLiteralKind,
    hints: &mut Vec<AnalysisTypeHint>,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                collect_type_hints_in_expr(ctx, file_id, &field.value, hints);
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                collect_type_hints_in_expr(ctx, file_id, item, hints);
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            collect_type_hints_in_expr(ctx, file_id, value, hints);
            collect_type_hints_in_expr(ctx, file_id, count, hints);
        }
        ast::DataLiteralKind::Scalar(value) => {
            collect_type_hints_in_expr(ctx, file_id, value, hints)
        }
    }
}

fn binding_type_for_span(ctx: &SemaContext<'_>, span: kernc_utils::Span) -> Option<TypeId> {
    ctx.scopes
        .all_symbols()
        .find(|(_, info)| info.span == span)
        .map(|(_, info)| info.type_id)
}

fn hint_type_label(ctx: &SemaContext<'_>, ty: TypeId) -> Option<String> {
    if ty == TypeId::ERROR || ty == TypeId::VOID || ty == TypeId::NEVER {
        return None;
    }
    let normalized = ctx.type_registry.normalize(ty);
    if normalized == TypeId::ERROR || normalized == TypeId::VOID || normalized == TypeId::NEVER {
        return None;
    }
    let label = ctx.ty_to_string(normalized);
    (!label.contains("{error}") && !label.contains("<infer")).then_some(label)
}
