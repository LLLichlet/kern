use super::*;

pub(super) fn collect_control_facts(
    expr: &ast::Expr,
) -> (Vec<FlowRegionFacts>, AnalysisFlowSummary) {
    let mut regions = Vec::new();
    let mut summary = AnalysisFlowSummary::default();
    collect_control_facts_expr(expr, &mut regions, &mut summary);
    (regions, summary)
}

fn collect_control_facts_expr(
    expr: &ast::Expr,
    regions: &mut Vec<FlowRegionFacts>,
    summary: &mut AnalysisFlowSummary,
) {
    match &expr.kind {
        ast::ExprKind::Grouped { expr: inner } => {
            collect_control_facts_expr(inner, regions, summary)
        }
        ast::ExprKind::Let {
            init, else_clause, ..
        } => {
            collect_control_facts_expr(init, regions, summary);
            if let Some(else_clause) = else_clause {
                summary.branch_count += 1;
                summary.let_else_count += 1;
                regions.push(FlowRegionFacts {
                    span: else_clause.span(),
                    kind: AnalysisFlowRegionKind::LetElse,
                });
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_control_facts_expr(else_expr, regions, summary);
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_control_facts_expr(&arm.body, regions, summary);
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static {
            init: Some(init), ..
        } => collect_control_facts_expr(init, regions, summary),
        ast::ExprKind::Error
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::Static { init: None, .. } => {}
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_control_facts_expr(lhs, regions, summary);
            collect_control_facts_expr(rhs, regions, summary);
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                collect_control_facts_expr(start, regions, summary);
            }
            if let Some(end) = end {
                collect_control_facts_expr(end, regions, summary);
            }
        }
        ast::ExprKind::Unary { operand, .. } => {
            collect_control_facts_expr(operand, regions, summary);
        }
        ast::ExprKind::FieldAccess { lhs, .. } => {
            collect_control_facts_expr(lhs, regions, summary);
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_control_facts_expr(lhs, regions, summary);
            collect_control_facts_expr(index, regions, summary);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_control_facts_expr(callee, regions, summary);
            for arg in args {
                collect_control_facts_expr(arg, regions, summary);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_control_facts_expr(&field.value, regions, summary);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_control_facts_expr(item, regions, summary);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_control_facts_expr(value, regions, summary);
                collect_control_facts_expr(count, regions, summary);
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_control_facts_expr(value, regions, summary);
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            summary.branch_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::If,
            });
            collect_control_facts_expr(cond, regions, summary);
            collect_control_facts_expr(then_branch, regions, summary);
            if let Some(else_expr) = else_branch {
                collect_control_facts_expr(else_expr, regions, summary);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            summary.branch_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Match,
            });
            collect_control_facts_expr(target, regions, summary);
            for arm in arms {
                regions.push(FlowRegionFacts {
                    span: arm.span,
                    kind: AnalysisFlowRegionKind::MatchArm,
                });
                for pattern in &arm.patterns {
                    collect_control_facts_match_pattern(pattern, regions, summary);
                }
                collect_control_facts_expr(&arm.body, regions, summary);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            summary.block_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Block,
            });
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(inner) | ast::StmtKind::ExprValue(inner) => {
                        collect_control_facts_expr(inner, regions, summary);
                    }
                }
            }
            if let Some(result) = result {
                collect_control_facts_expr(result, regions, summary);
            }
        }
        ast::ExprKind::While { cond, body } => {
            summary.loop_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Loop,
            });
            collect_control_facts_expr(cond, regions, summary);
            collect_control_facts_expr(body, regions, summary);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_control_facts_expr(lhs, regions, summary);
            if let Some(start) = start {
                collect_control_facts_expr(start, regions, summary);
            }
            if let Some(end) = end {
                collect_control_facts_expr(end, regions, summary);
            }
        }
        ast::ExprKind::Defer { expr: inner } => {
            summary.defer_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Defer,
            });
            collect_control_facts_expr(inner, regions, summary);
        }
        ast::ExprKind::Return(value) => {
            summary.return_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Return,
            });
            if let Some(value) = value {
                collect_control_facts_expr(value, regions, summary);
            }
        }
        ast::ExprKind::Break => {
            summary.break_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Break,
            });
        }
        ast::ExprKind::Continue => {
            summary.continue_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Continue,
            });
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_control_facts_expr(lhs, regions, summary);
            collect_control_facts_expr(rhs, regions, summary);
        }
        ast::ExprKind::As { lhs, .. } => collect_control_facts_expr(lhs, regions, summary),
        ast::ExprKind::Propagate { operand, .. } => {
            summary.branch_count += 1;
            summary.return_count += 1;
            collect_control_facts_expr(operand, regions, summary);
        }
        ast::ExprKind::GenericInstantiation { target, .. } => {
            collect_control_facts_expr(target, regions, summary);
        }
        ast::ExprKind::Closure { captures, .. } => {
            summary.closure_count += 1;
            regions.push(FlowRegionFacts {
                span: expr.span,
                kind: AnalysisFlowRegionKind::Closure,
            });
            for capture in captures {
                collect_control_facts_expr(&capture.value, regions, summary);
            }
        }
        ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer => {}
    }
}

fn collect_control_facts_match_pattern(
    pattern: &ast::MatchPattern,
    regions: &mut Vec<FlowRegionFacts>,
    summary: &mut AnalysisFlowSummary,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(expr) => collect_control_facts_expr(expr, regions, summary),
        ast::MatchPatternKind::Pattern(_) => {}
    }
}
