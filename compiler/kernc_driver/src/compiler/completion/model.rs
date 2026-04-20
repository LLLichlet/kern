use super::facts::{query_span_for_expr, span_contains_offset};
use super::*;

impl CompletionModel {
    pub(in crate::compiler) fn requires_body_completion(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> bool {
        let Some(module) = self.module_for_path(target_path) else {
            return true;
        };

        module
            .body_regions
            .iter()
            .copied()
            .any(|span| span_contains_offset(span, offset))
    }

    pub(in crate::compiler) fn completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        let Some(module) = self.module_for_path(target_path) else {
            return Vec::new();
        };

        let mut visible = Vec::new();
        for item in &self.root_items {
            push_completion_item(&mut visible, item.clone());
        }
        for item in &module.top_level_items {
            push_completion_item(&mut visible, item.clone());
        }

        for decl in &module.ast.decls {
            if self.collect_in_decl(decl, &mut visible, offset) {
                break;
            }
        }

        visible
    }

    pub(in crate::compiler) fn surface_completion_items(
        &self,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        let Some(module) = self.module_for_path(target_path) else {
            return Vec::new();
        };

        let mut visible = Vec::new();
        for item in &self.root_items {
            push_completion_item(&mut visible, item.clone());
        }
        for item in &module.top_level_items {
            push_completion_item(&mut visible, item.clone());
        }

        for decl in &module.surface_decls {
            if self.collect_in_surface_decl(decl, &mut visible, offset) {
                break;
            }
        }

        visible
    }

    fn module_for_path(&self, target_path: &Path) -> Option<&CompletionModule> {
        self.modules
            .iter()
            .find(|module| module.path == target_path)
    }

    fn collect_in_decl(
        &self,
        decl: &ast::Decl,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(decl.span, offset) {
            return false;
        }

        match &decl.kind {
            ast::DeclKind::Function { body, .. } => {
                if let Some(items) = self.function_items_by_span.get(&decl.span) {
                    for item in items {
                        push_completion_item(visible, item.clone());
                    }
                }

                if let Some(body) = body
                    && span_contains_offset(body.span, offset)
                {
                    self.collect_in_expr(body, visible, offset);
                }
                true
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_in_expr(value, visible, offset);
                true
            }
            ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
                for child in decls {
                    if self.collect_in_decl(child, visible, offset) {
                        return true;
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn collect_in_surface_decl(
        &self,
        decl: &CompletionSurfaceDecl,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(decl.span, offset) {
            return false;
        }

        for item in &decl.function_items {
            push_completion_item(visible, item.clone());
        }

        for child in &decl.children {
            if self.collect_in_surface_decl(child, visible, offset) {
                return true;
            }
        }

        true
    }

    fn collect_in_expr(
        &self,
        expr: &ast::Expr,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(query_span_for_expr(expr), offset) {
            return false;
        }

        match &expr.kind {
            ast::ExprKind::Block { stmts, result } => self.collect_in_block(
                query_span_for_expr(expr),
                stmts,
                result.as_deref(),
                visible,
                offset,
            ),
            ast::ExprKind::Let {
                pattern: _,
                init,
                else_clause,
            } => {
                if self.collect_in_expr(init, visible, offset) {
                    return true;
                }
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(else_expr) => {
                            if span_contains_offset(query_span_for_expr(else_expr), offset) {
                                let mut branch_visible = visible.clone();
                                self.collect_in_expr(else_expr, &mut branch_visible, offset);
                                *visible = branch_visible;
                                return true;
                            }
                        }
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                if span_contains_offset(query_span_for_expr(&arm.body), offset) {
                                    let mut branch_visible = visible.clone();
                                    if let Some(facts) = self
                                        .let_else_facts_by_span
                                        .get(&query_span_for_expr(&arm.body))
                                    {
                                        extend_completion_items(
                                            &mut branch_visible,
                                            &facts.binding_items,
                                        );
                                    }
                                    self.collect_in_expr(&arm.body, &mut branch_visible, offset);
                                    *visible = branch_visible;
                                    return true;
                                }
                            }
                        }
                    }
                }
                true
            }
            ast::ExprKind::Static { init, .. } => self.collect_in_expr(init, visible, offset),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::Unary { operand, .. } => self.collect_in_expr(operand, visible, offset),
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if span_contains_offset(lhs.span, offset) {
                    self.collect_in_expr(lhs, visible, offset)
                } else if let Some(items) = self.member_items_by_span.get(&expr.span) {
                    *visible = items.clone();
                    true
                } else {
                    true
                }
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(index, visible, offset)
            }
            ast::ExprKind::Call { callee, args } => {
                if self.collect_in_expr(callee, visible, offset) {
                    return true;
                }
                for arg in args {
                    if self.collect_in_expr(arg, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::ExprKind::DataInit { literal, .. } => {
                self.collect_in_data_literal(literal, visible, offset)
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if self.collect_in_expr(cond, visible, offset) {
                    return true;
                }

                let if_facts = self.if_facts_by_span.get(&query_span_for_expr(expr));
                let then_span = if_facts
                    .map(|facts| facts.then_span)
                    .unwrap_or_else(|| query_span_for_expr(then_branch));
                if span_contains_offset(then_span, offset) {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(then_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                let else_span = if_facts.and_then(|facts| facts.else_span);
                if let Some(else_branch) = else_branch
                    && let Some(else_span) =
                        else_span.or_else(|| Some(query_span_for_expr(else_branch)))
                    && span_contains_offset(else_span, offset)
                {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(else_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                true
            }
            ast::ExprKind::Match { target, arms } => {
                if self.collect_in_expr(target, visible, offset) {
                    return true;
                }

                let match_facts = self.match_facts_by_span.get(&query_span_for_expr(expr));
                for (index, arm) in arms.iter().enumerate() {
                    let arm_span = match_facts
                        .and_then(|facts| facts.arms.get(index).map(|arm| arm.span))
                        .unwrap_or(arm.span);
                    if !span_contains_offset(arm_span, offset) {
                        continue;
                    }

                    let body_span = match_facts
                        .and_then(|facts| facts.arms.get(index).map(|arm| arm.body_span))
                        .unwrap_or(arm.body.span);
                    if span_contains_offset(body_span, offset) {
                        let mut arm_visible = visible.clone();
                        if let Some(arm_facts) = match_facts.and_then(|facts| facts.arms.get(index))
                        {
                            extend_completion_items(&mut arm_visible, &arm_facts.binding_items);
                        }
                        self.collect_in_expr(&arm.body, &mut arm_visible, offset);
                        *visible = arm_visible;
                        return true;
                    }

                    for pattern in &arm.patterns {
                        if self.collect_in_match_pattern(pattern, visible, offset) {
                            return true;
                        }
                    }

                    return true;
                }

                true
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let mut loop_visible = visible.clone();

                if let Some(init) = init
                    && self.collect_in_expr(init, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if let Some(loop_facts) = self.for_facts_by_span.get(&query_span_for_expr(expr)) {
                    extend_completion_items(&mut loop_visible, &loop_facts.scope_items);
                }

                if let Some(cond) = cond
                    && self.collect_in_expr(cond, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if let Some(post) = post
                    && self.collect_in_expr(post, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if span_contains_offset(body.span, offset) {
                    self.collect_in_expr(body, &mut loop_visible, offset);
                    *visible = loop_visible;
                }

                true
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                if self.collect_in_expr(lhs, visible, offset) {
                    return true;
                }
                if let Some(start) = start
                    && self.collect_in_expr(start, visible, offset)
                {
                    return true;
                }
                if let Some(end) = end
                    && self.collect_in_expr(end, visible, offset)
                {
                    return true;
                }
                true
            }
            ast::ExprKind::Defer { expr } => self.collect_in_expr(expr, visible, offset),
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    return self.collect_in_expr(value, visible, offset);
                }
                true
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::As { lhs, .. } => self.collect_in_expr(lhs, visible, offset),
            ast::ExprKind::GenericInstantiation { target, .. } => {
                self.collect_in_expr(target, visible, offset)
            }
            ast::ExprKind::Closure {
                captures,
                params: _,
                body,
                ..
            } => {
                for capture in captures {
                    if self.collect_in_expr(&capture.value, visible, offset) {
                        return true;
                    }
                }

                let closure_facts = self.closure_facts_by_span.get(&query_span_for_expr(expr));
                let body_span = closure_facts
                    .map(|facts| facts.body_span)
                    .unwrap_or(body.span);
                if span_contains_offset(body_span, offset) {
                    let mut closure_visible = visible.clone();
                    if let Some(closure_facts) = closure_facts {
                        extend_completion_items(&mut closure_visible, &closure_facts.binding_items);
                    }
                    self.collect_in_expr(body, &mut closure_visible, offset);
                    *visible = closure_visible;
                }

                true
            }
            _ => true,
        }
    }

    fn collect_in_stmt(
        &self,
        stmt: &ast::Stmt,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match &stmt.kind {
            ast::StmtKind::Use(_) => false,
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.collect_in_expr(expr, visible, offset)
            }
        }
    }

    fn collect_in_block(
        &self,
        block_span: kernc_utils::Span,
        stmts: &[ast::Stmt],
        result: Option<&ast::Expr>,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        let Some(block_facts) = self.block_facts_by_span.get(&block_span) else {
            debug_assert!(false, "missing block completion facts for block span");
            return true;
        };
        debug_assert_eq!(
            block_facts.stmt_facts.len(),
            stmts.len(),
            "block completion facts must stay aligned with block statements"
        );

        let mut block_visible = visible.clone();
        for (stmt, stmt_facts) in stmts.iter().zip(&block_facts.stmt_facts) {
            if stmt_facts.span.start > offset {
                break;
            }

            if span_contains_offset(stmt_facts.span, offset) {
                extend_completion_items(&mut block_visible, &stmt_facts.prefix_items);
                self.collect_in_stmt(stmt, &mut block_visible, offset);
                *visible = block_visible;
                return true;
            }
        }

        extend_completion_items(&mut block_visible, &block_facts.tail_items);
        if let Some(result) = result
            && span_contains_offset(result.span, offset)
        {
            self.collect_in_expr(result, &mut block_visible, offset);
        }

        *visible = block_visible;
        true
    }

    fn collect_in_data_literal(
        &self,
        literal: &ast::DataLiteralKind,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    if self.collect_in_expr(&field.value, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    if self.collect_in_expr(item, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.collect_in_expr(value, visible, offset)
                    || self.collect_in_expr(count, visible, offset)
            }
            ast::DataLiteralKind::Scalar(value) => self.collect_in_expr(value, visible, offset),
        }
    }

    fn collect_in_match_pattern(
        &self,
        pattern: &ast::MatchPattern,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(pattern.span, offset) {
            return false;
        }

        match &pattern.kind {
            ast::MatchPatternKind::Value(value) => self.collect_in_expr(value, visible, offset),
            ast::MatchPatternKind::Range { start, end, .. } => {
                self.collect_in_expr(start, visible, offset)
                    || self.collect_in_expr(end, visible, offset)
            }
            _ => true,
        }
    }
}

pub(in crate::compiler) fn parsed_requires_body_completion(
    modules: &[super::super::ParsedModule],
    target_path: &Path,
    offset: usize,
) -> bool {
    let Some(module) = modules.iter().find(|module| module.path == target_path) else {
        return true;
    };

    module
        .body_regions
        .iter()
        .copied()
        .any(|span| span_contains_offset(span, offset))
}

pub(super) fn extend_completion_items(
    visible: &mut Vec<AnalysisCompletionItem>,
    items: &[AnalysisCompletionItem],
) {
    for item in items {
        push_completion_item(visible, item.clone());
    }
}

pub(super) fn push_completion_item(
    items: &mut Vec<AnalysisCompletionItem>,
    item: AnalysisCompletionItem,
) {
    if let Some(index) = items
        .iter()
        .position(|existing| existing.label == item.label)
    {
        items.remove(index);
    }
    items.push(item);
}
