use super::*;

impl<'a> FlowCfgBuilder<'a> {
    pub(super) fn build(
        expr: &ast::Expr,
        owner_span: Span,
        binding_ids_by_span: &'a HashMap<Span, AnalysisFlowBindingId>,
        reference_to_binding: &'a HashMap<Span, AnalysisFlowBindingId>,
    ) -> FlowCfgBuildResult {
        let mut builder = Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            incoming_counts: Vec::new(),
            node_uses: Vec::new(),
            node_value_uses: Vec::new(),
            node_defs: Vec::new(),
            node_def_kinds: Vec::new(),
            node_copy_sources: Vec::new(),
            node_effects: Vec::new(),
            local_bindings_by_span: binding_ids_by_span,
            reference_to_binding,
            entry: AnalysisFlowNodeId(0),
            exit: AnalysisFlowNodeId(0),
        };
        let entry = builder.add_node(AnalysisFlowCfgNodeKind::Entry, owner_span, None);
        let exit = builder.add_node(AnalysisFlowCfgNodeKind::Exit, owner_span, None);
        builder.entry = entry;
        builder.exit = exit;

        let exits = builder.lower_expr(
            expr,
            vec![PendingEdge {
                from: entry,
                kind: AnalysisFlowCfgEdgeKind::Next,
            }],
            None,
        );
        builder.connect_to_node(exits, exit);

        FlowCfgBuildResult {
            cfg: AnalysisFlowCfg {
                entry,
                exit,
                nodes: builder.nodes,
                edges: builder.edges,
            },
            node_uses: builder.node_uses,
            node_value_uses: builder.node_value_uses,
            node_defs: builder.node_defs,
            node_def_kinds: builder.node_def_kinds,
            node_copy_sources: builder.node_copy_sources,
            node_effects: builder.node_effects,
        }
    }

    fn add_node(
        &mut self,
        kind: AnalysisFlowCfgNodeKind,
        span: Span,
        ast_node_id: Option<kernc_utils::NodeId>,
    ) -> AnalysisFlowNodeId {
        let id = AnalysisFlowNodeId(self.nodes.len());
        self.nodes.push(AnalysisFlowCfgNode {
            id,
            span,
            kind,
            ast_node_id,
        });
        self.incoming_counts.push(0);
        self.node_uses.push(Vec::new());
        self.node_value_uses.push(Vec::new());
        self.node_defs.push(Vec::new());
        self.node_def_kinds.push(None);
        self.node_copy_sources.push(None);
        self.node_effects.push(AnalysisFlowNodeEffects {
            node_id: id,
            has_call: false,
            has_memory_read: false,
            has_memory_write: false,
            has_control_flow: false,
            is_pure: true,
        });
        id
    }

    fn add_edge(
        &mut self,
        from: AnalysisFlowNodeId,
        to: AnalysisFlowNodeId,
        kind: AnalysisFlowCfgEdgeKind,
    ) {
        self.edges.push(AnalysisFlowCfgEdge { from, to, kind });
        self.incoming_counts[to.index()] += 1;
    }

    fn connect_to_node(&mut self, incoming: Vec<PendingEdge>, to: AnalysisFlowNodeId) {
        for edge in incoming {
            self.add_edge(edge.from, to, edge.kind);
        }
    }

    fn fallthrough(&self, from: AnalysisFlowNodeId) -> Vec<PendingEdge> {
        vec![PendingEdge {
            from,
            kind: AnalysisFlowCfgEdgeKind::Next,
        }]
    }

    fn join_pending(&mut self, incoming: Vec<PendingEdge>, span: Span) -> Vec<PendingEdge> {
        if incoming.is_empty() {
            return Vec::new();
        }
        let join = self.add_node(AnalysisFlowCfgNodeKind::Join, span, None);
        self.connect_to_node(incoming, join);
        self.fallthrough(join)
    }

    fn lower_eval(&mut self, expr: &ast::Expr, incoming: Vec<PendingEdge>) -> AnalysisFlowNodeId {
        let node = self.add_node(AnalysisFlowCfgNodeKind::Eval, expr.span, Some(expr.id));
        self.node_effects[node.index()] = classify_expr_effects(node, expr);
        self.connect_to_node(incoming, node);
        node
    }

    fn record_use(&mut self, node: AnalysisFlowNodeId, binding_id: AnalysisFlowBindingId) {
        if self.node_uses[node.index()].last() != Some(&binding_id) {
            self.node_uses[node.index()].push(binding_id);
        }
    }

    fn record_defs(
        &mut self,
        node: AnalysisFlowNodeId,
        definitions: Vec<AnalysisFlowBindingId>,
        kind: AnalysisFlowDefinitionKind,
        copy_source_binding_id: Option<AnalysisFlowBindingId>,
        value_use_bindings: Vec<AnalysisFlowBindingId>,
    ) {
        self.node_defs[node.index()] = definitions;
        self.node_def_kinds[node.index()] = Some(kind);
        self.node_value_uses[node.index()] = value_use_bindings;
        self.node_copy_sources[node.index()] = (self.node_defs[node.index()].len() == 1)
            .then_some(copy_source_binding_id)
            .flatten();
    }

    fn local_binding_use(&self, expr: &ast::Expr) -> Option<AnalysisFlowBindingId> {
        let ast::ExprKind::Identifier(_) = expr.kind else {
            return None;
        };
        self.reference_to_binding.get(&expr.span).copied()
    }

    fn local_binding_uses_in_expr(&self, expr: &ast::Expr) -> Vec<AnalysisFlowBindingId> {
        let mut uses = Vec::new();
        collect_local_binding_uses_in_expr(expr, self.reference_to_binding, &mut uses);
        uses.sort();
        uses.dedup();
        uses
    }

    fn lower_expr(
        &mut self,
        expr: &ast::Expr,
        incoming: Vec<PendingEdge>,
        loop_ctx: Option<LoopContext>,
    ) -> Vec<PendingEdge> {
        match &expr.kind {
            ast::ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                let init_out = self.lower_expr(init, incoming, loop_ctx);
                if let Some(else_expr) = else_branch {
                    let branch =
                        self.add_node(AnalysisFlowCfgNodeKind::Branch, expr.span, Some(expr.id));
                    self.node_effects[branch.index()] = classify_expr_effects(branch, expr);
                    self.connect_to_node(init_out, branch);

                    let let_node =
                        self.add_node(AnalysisFlowCfgNodeKind::Eval, expr.span, Some(expr.id));
                    self.node_effects[let_node.index()] = classify_expr_effects(let_node, expr);
                    self.add_edge(branch, let_node, AnalysisFlowCfgEdgeKind::TrueBranch);
                    self.record_defs(
                        let_node,
                        self.collect_pattern_binding_ids(&pattern.pattern),
                        AnalysisFlowDefinitionKind::Initializer,
                        self.let_copy_source_binding_id(pattern, init, true),
                        self.local_binding_uses_in_expr(init),
                    );
                    let success_out = self.fallthrough(let_node);
                    let else_out = if let Some(else_pattern) = else_pattern {
                        let else_node = self.add_node(
                            AnalysisFlowCfgNodeKind::Eval,
                            else_pattern.span,
                            Some(expr.id),
                        );
                        self.add_edge(branch, else_node, AnalysisFlowCfgEdgeKind::FalseBranch);
                        self.record_defs(
                            else_node,
                            self.collect_pattern_binding_ids(else_pattern),
                            AnalysisFlowDefinitionKind::Initializer,
                            None,
                            Vec::new(),
                        );
                        self.lower_expr(else_expr, self.fallthrough(else_node), loop_ctx)
                    } else {
                        self.lower_expr(
                            else_expr,
                            vec![PendingEdge {
                                from: branch,
                                kind: AnalysisFlowCfgEdgeKind::FalseBranch,
                            }],
                            loop_ctx,
                        )
                    };
                    let mut merged = success_out;
                    merged.extend(else_out);
                    self.join_pending(merged, expr.span)
                } else {
                    let node = self.lower_eval(expr, init_out);
                    self.record_defs(
                        node,
                        self.collect_pattern_binding_ids(&pattern.pattern),
                        AnalysisFlowDefinitionKind::Initializer,
                        self.let_copy_source_binding_id(pattern, init, false),
                        self.local_binding_uses_in_expr(init),
                    );
                    self.fallthrough(node)
                }
            }
            ast::ExprKind::Static { pattern, init } => {
                let init_out = self.lower_expr(init, incoming, loop_ctx);
                let node = self.lower_eval(expr, init_out);
                if let Some(binding_id) =
                    self.local_bindings_by_span.get(&pattern.name_span).copied()
                {
                    self.record_defs(
                        node,
                        vec![binding_id],
                        AnalysisFlowDefinitionKind::Initializer,
                        self.local_binding_use(init),
                        self.local_binding_uses_in_expr(init),
                    );
                }
                self.fallthrough(node)
            }
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                let lhs_out = self.lower_expr(lhs, incoming, loop_ctx);
                let rhs_out = self.lower_expr(rhs, lhs_out, loop_ctx);
                let node = self.lower_eval(expr, rhs_out);
                self.fallthrough(node)
            }
            ast::ExprKind::Unary { operand, .. } => {
                let operand_out = self.lower_expr(operand, incoming, loop_ctx);
                let node = self.lower_eval(expr, operand_out);
                self.fallthrough(node)
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                let lhs_out = self.lower_expr(lhs, incoming, loop_ctx);
                let node = self.lower_eval(expr, lhs_out);
                self.fallthrough(node)
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                let lhs_out = self.lower_expr(lhs, incoming, loop_ctx);
                let index_out = self.lower_expr(index, lhs_out, loop_ctx);
                let node = self.lower_eval(expr, index_out);
                self.fallthrough(node)
            }
            ast::ExprKind::Call { callee, args } => {
                let mut current = self.lower_expr(callee, incoming, loop_ctx);
                for arg in args {
                    current = self.lower_expr(arg, current, loop_ctx);
                }
                let node = self.lower_eval(expr, current);
                self.fallthrough(node)
            }
            ast::ExprKind::DataInit { literal, .. } => {
                let mut current = incoming;
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        for field in fields {
                            current = self.lower_expr(&field.value, current, loop_ctx);
                        }
                    }
                    ast::DataLiteralKind::Array(items) => {
                        for item in items {
                            current = self.lower_expr(item, current, loop_ctx);
                        }
                    }
                    ast::DataLiteralKind::Repeat { value, count } => {
                        current = self.lower_expr(value, current, loop_ctx);
                        current = self.lower_expr(count, current, loop_ctx);
                    }
                    ast::DataLiteralKind::Scalar(value) => {
                        current = self.lower_expr(value, current, loop_ctx);
                    }
                }
                let node = self.lower_eval(expr, current);
                self.fallthrough(node)
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_out = self.lower_expr(cond, incoming, loop_ctx);
                let branch =
                    self.add_node(AnalysisFlowCfgNodeKind::Branch, cond.span, Some(expr.id));
                self.node_effects[branch.index()] = classify_expr_effects(branch, expr);
                self.connect_to_node(cond_out, branch);

                let then_out = self.lower_expr(
                    then_branch,
                    vec![PendingEdge {
                        from: branch,
                        kind: AnalysisFlowCfgEdgeKind::TrueBranch,
                    }],
                    loop_ctx,
                );
                let else_out = if let Some(else_expr) = else_branch {
                    self.lower_expr(
                        else_expr,
                        vec![PendingEdge {
                            from: branch,
                            kind: AnalysisFlowCfgEdgeKind::FalseBranch,
                        }],
                        loop_ctx,
                    )
                } else {
                    vec![PendingEdge {
                        from: branch,
                        kind: AnalysisFlowCfgEdgeKind::FalseBranch,
                    }]
                };
                let mut merged = then_out;
                merged.extend(else_out);
                self.join_pending(merged, expr.span)
            }
            ast::ExprKind::Match { target, arms } => {
                let target_out = self.lower_expr(target, incoming, loop_ctx);
                let match_node =
                    self.add_node(AnalysisFlowCfgNodeKind::Match, target.span, Some(expr.id));
                self.node_effects[match_node.index()] = classify_expr_effects(match_node, expr);
                self.connect_to_node(target_out, match_node);

                let mut merged = Vec::new();
                for arm in arms {
                    let arm_node = self.add_node(AnalysisFlowCfgNodeKind::MatchArm, arm.span, None);
                    self.add_edge(match_node, arm_node, AnalysisFlowCfgEdgeKind::CaseBranch);
                    let arm_out = self.lower_expr(
                        &arm.body,
                        vec![PendingEdge {
                            from: arm_node,
                            kind: AnalysisFlowCfgEdgeKind::Next,
                        }],
                        loop_ctx,
                    );
                    merged.extend(arm_out);
                }
                self.join_pending(merged, expr.span)
            }
            ast::ExprKind::Block { stmts, result } => {
                let mut current = incoming;
                for stmt in stmts {
                    current = match &stmt.kind {
                        ast::StmtKind::Use(_) => current,
                        ast::StmtKind::ExprStmt(inner) | ast::StmtKind::ExprValue(inner) => {
                            self.lower_expr(inner, current, loop_ctx)
                        }
                    };
                }
                if let Some(result) = result {
                    self.lower_expr(result, current, loop_ctx)
                } else {
                    current
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let mut current = incoming;
                if let Some(init) = init {
                    current = self.lower_expr(init, current, loop_ctx);
                }

                let head = self.add_node(
                    AnalysisFlowCfgNodeKind::LoopHead,
                    cond.as_deref().map_or(expr.span, |cond| cond.span),
                    Some(expr.id),
                );
                self.node_effects[head.index()] = classify_expr_effects(head, expr);
                self.connect_to_node(current, head);
                let after_loop = self.add_node(AnalysisFlowCfgNodeKind::Join, expr.span, None);

                let body_in = if let Some(cond) = cond {
                    let cond_out = self.lower_expr(cond, self.fallthrough(head), loop_ctx);
                    let branch =
                        self.add_node(AnalysisFlowCfgNodeKind::Branch, cond.span, Some(expr.id));
                    self.node_effects[branch.index()] = classify_expr_effects(branch, expr);
                    self.connect_to_node(cond_out, branch);
                    self.add_edge(branch, after_loop, AnalysisFlowCfgEdgeKind::FalseBranch);
                    vec![PendingEdge {
                        from: branch,
                        kind: AnalysisFlowCfgEdgeKind::TrueBranch,
                    }]
                } else {
                    self.fallthrough(head)
                };

                let continue_target = if let Some(post_expr) = post {
                    self.add_node(AnalysisFlowCfgNodeKind::LoopLatch, post_expr.span, None)
                } else {
                    head
                };
                let body_out = self.lower_expr(
                    body,
                    body_in,
                    Some(LoopContext {
                        break_target: after_loop,
                        continue_target,
                    }),
                );

                if let Some(post_expr) = post {
                    self.connect_to_node(body_out, continue_target);
                    let post_out = self.lower_expr(
                        post_expr,
                        self.fallthrough(continue_target),
                        Some(LoopContext {
                            break_target: after_loop,
                            continue_target: head,
                        }),
                    );
                    for edge in post_out {
                        self.add_edge(edge.from, head, AnalysisFlowCfgEdgeKind::LoopBack);
                    }
                } else {
                    for edge in body_out {
                        self.add_edge(edge.from, head, AnalysisFlowCfgEdgeKind::LoopBack);
                    }
                }

                if self.incoming_counts[after_loop.index()] > 0 {
                    self.fallthrough(after_loop)
                } else {
                    Vec::new()
                }
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                let mut current = self.lower_expr(lhs, incoming, loop_ctx);
                if let Some(start) = start {
                    current = self.lower_expr(start, current, loop_ctx);
                }
                if let Some(end) = end {
                    current = self.lower_expr(end, current, loop_ctx);
                }
                let node = self.lower_eval(expr, current);
                self.fallthrough(node)
            }
            ast::ExprKind::Defer { expr: inner } => {
                let inner_out = self.lower_expr(inner, incoming, loop_ctx);
                let node = self.lower_eval(expr, inner_out);
                self.fallthrough(node)
            }
            ast::ExprKind::Return(value) => {
                let value_out = if let Some(value) = value {
                    self.lower_expr(value, incoming, loop_ctx)
                } else {
                    incoming
                };
                let node = self.add_node(AnalysisFlowCfgNodeKind::Return, expr.span, Some(expr.id));
                self.node_effects[node.index()] = classify_expr_effects(node, expr);
                self.connect_to_node(value_out, node);
                self.add_edge(node, self.exit, AnalysisFlowCfgEdgeKind::ReturnFlow);
                Vec::new()
            }
            ast::ExprKind::Break => {
                let node = self.add_node(AnalysisFlowCfgNodeKind::Break, expr.span, Some(expr.id));
                self.node_effects[node.index()] = classify_expr_effects(node, expr);
                self.connect_to_node(incoming, node);
                let target = loop_ctx.map(|ctx| ctx.break_target).unwrap_or(self.exit);
                self.add_edge(node, target, AnalysisFlowCfgEdgeKind::BreakFlow);
                Vec::new()
            }
            ast::ExprKind::Continue => {
                let node =
                    self.add_node(AnalysisFlowCfgNodeKind::Continue, expr.span, Some(expr.id));
                self.node_effects[node.index()] = classify_expr_effects(node, expr);
                self.connect_to_node(incoming, node);
                let target = loop_ctx.map(|ctx| ctx.continue_target).unwrap_or(self.exit);
                self.add_edge(node, target, AnalysisFlowCfgEdgeKind::ContinueFlow);
                Vec::new()
            }
            ast::ExprKind::Assign { lhs, op, rhs } => {
                let current = if self.local_binding_use(lhs).is_some() {
                    incoming
                } else {
                    self.lower_expr(lhs, incoming, loop_ctx)
                };
                let rhs_out = self.lower_expr(rhs, current, loop_ctx);
                let node = self.lower_eval(expr, rhs_out);
                if let Some(binding_id) = self.local_binding_use(lhs) {
                    self.record_defs(
                        node,
                        vec![binding_id],
                        AnalysisFlowDefinitionKind::Assignment,
                        (*op == ast::AssignmentOperator::Assign)
                            .then_some(self.local_binding_use(rhs))
                            .flatten(),
                        {
                            let mut uses = self.local_binding_uses_in_expr(rhs);
                            if *op != ast::AssignmentOperator::Assign {
                                uses.push(binding_id);
                                uses.sort();
                                uses.dedup();
                            }
                            uses
                        },
                    );
                    if *op != ast::AssignmentOperator::Assign {
                        self.record_use(node, binding_id);
                    }
                }
                self.fallthrough(node)
            }
            ast::ExprKind::As { lhs, .. } => {
                let lhs_out = self.lower_expr(lhs, incoming, loop_ctx);
                let node = self.lower_eval(expr, lhs_out);
                self.fallthrough(node)
            }
            ast::ExprKind::Propagate { operand, .. } => {
                let operand_out = self.lower_expr(operand, incoming, loop_ctx);
                let node = self.lower_eval(expr, operand_out);
                self.fallthrough(node)
            }
            ast::ExprKind::GenericInstantiation { target, .. } => {
                let target_out = self.lower_expr(target, incoming, loop_ctx);
                let node = self.lower_eval(expr, target_out);
                self.fallthrough(node)
            }
            ast::ExprKind::Closure { captures, .. } => {
                let mut current = incoming;
                for capture in captures {
                    current = self.lower_expr(&capture.value, current, loop_ctx);
                }
                let node = self.lower_eval(expr, current);
                self.fallthrough(node)
            }
            ast::ExprKind::Integer(_)
            | ast::ExprKind::Float(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::Char(_)
            | ast::ExprKind::ByteChar(_)
            | ast::ExprKind::String(_)
            | ast::ExprKind::Identifier(_)
            | ast::ExprKind::AnchoredPath { .. }
            | ast::ExprKind::EnumLiteral { .. }
            | ast::ExprKind::TypeNode(_)
            | ast::ExprKind::SelfValue
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer => {
                let node = self.lower_eval(expr, incoming);
                if let Some(binding_id) = self.local_binding_use(expr) {
                    self.record_use(node, binding_id);
                }
                self.fallthrough(node)
            }
        }
    }

    fn collect_pattern_binding_ids(&self, pattern: &ast::Pattern) -> Vec<AnalysisFlowBindingId> {
        let mut spans = HashSet::new();
        collect_pattern_binding_spans(pattern, &mut spans);
        let mut ids = spans
            .into_iter()
            .filter_map(|span| self.local_bindings_by_span.get(&span).copied())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }

    fn let_copy_source_binding_id(
        &self,
        pattern: &ast::LetPattern,
        init: &ast::Expr,
        has_else_branch: bool,
    ) -> Option<AnalysisFlowBindingId> {
        if has_else_branch {
            return None;
        }

        match &pattern.pattern.kind {
            ast::PatternKind::Binding(_) => self.local_binding_use(init),
            _ => None,
        }
    }
}

fn collect_pattern_binding_spans(pattern: &ast::Pattern, spans: &mut HashSet<Span>) {
    match &pattern.kind {
        ast::PatternKind::Binding(binding) => {
            spans.insert(binding.name_span);
        }
        ast::PatternKind::Destructure(destructure) => {
            for field in &destructure.fields {
                collect_pattern_binding_spans(&field.pattern, spans);
            }
        }
        ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {}
    }
}

fn collect_local_binding_uses_in_expr(
    expr: &ast::Expr,
    reference_to_binding: &HashMap<Span, AnalysisFlowBindingId>,
    uses: &mut Vec<AnalysisFlowBindingId>,
) {
    if let ast::ExprKind::Identifier(_) = expr.kind
        && let Some(binding_id) = reference_to_binding.get(&expr.span).copied()
    {
        uses.push(binding_id);
    }

    match &expr.kind {
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_local_binding_uses_in_expr(init, reference_to_binding, uses);
            if let Some(else_branch) = else_branch {
                collect_local_binding_uses_in_expr(else_branch, reference_to_binding, uses);
            }
        }
        ast::ExprKind::Static { init, .. } => {
            collect_local_binding_uses_in_expr(init, reference_to_binding, uses);
        }
        ast::ExprKind::AnchoredPath { .. } => {}
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
            collect_local_binding_uses_in_expr(rhs, reference_to_binding, uses);
        }
        ast::ExprKind::Unary { operand, .. } => {
            collect_local_binding_uses_in_expr(operand, reference_to_binding, uses);
        }
        ast::ExprKind::FieldAccess { lhs, .. } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
            collect_local_binding_uses_in_expr(index, reference_to_binding, uses);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_local_binding_uses_in_expr(callee, reference_to_binding, uses);
            for arg in args {
                collect_local_binding_uses_in_expr(arg, reference_to_binding, uses);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_local_binding_uses_in_expr(&field.value, reference_to_binding, uses);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_local_binding_uses_in_expr(item, reference_to_binding, uses);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_local_binding_uses_in_expr(value, reference_to_binding, uses);
                collect_local_binding_uses_in_expr(count, reference_to_binding, uses);
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_local_binding_uses_in_expr(value, reference_to_binding, uses);
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_local_binding_uses_in_expr(cond, reference_to_binding, uses);
            collect_local_binding_uses_in_expr(then_branch, reference_to_binding, uses);
            if let Some(else_branch) = else_branch {
                collect_local_binding_uses_in_expr(else_branch, reference_to_binding, uses);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_local_binding_uses_in_expr(target, reference_to_binding, uses);
            for arm in arms {
                collect_local_binding_uses_in_expr(&arm.body, reference_to_binding, uses);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_local_binding_uses_in_expr(expr, reference_to_binding, uses);
                    }
                }
            }
            if let Some(result) = result {
                collect_local_binding_uses_in_expr(result, reference_to_binding, uses);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_local_binding_uses_in_expr(init, reference_to_binding, uses);
            }
            if let Some(cond) = cond {
                collect_local_binding_uses_in_expr(cond, reference_to_binding, uses);
            }
            if let Some(post) = post {
                collect_local_binding_uses_in_expr(post, reference_to_binding, uses);
            }
            collect_local_binding_uses_in_expr(body, reference_to_binding, uses);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
            if let Some(start) = start {
                collect_local_binding_uses_in_expr(start, reference_to_binding, uses);
            }
            if let Some(end) = end {
                collect_local_binding_uses_in_expr(end, reference_to_binding, uses);
            }
        }
        ast::ExprKind::Defer { expr } => {
            collect_local_binding_uses_in_expr(expr, reference_to_binding, uses);
        }
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_local_binding_uses_in_expr(value, reference_to_binding, uses);
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
            collect_local_binding_uses_in_expr(rhs, reference_to_binding, uses);
        }
        ast::ExprKind::As { lhs, .. } => {
            collect_local_binding_uses_in_expr(lhs, reference_to_binding, uses);
        }
        ast::ExprKind::Propagate { operand, .. } => {
            collect_local_binding_uses_in_expr(operand, reference_to_binding, uses);
        }
        ast::ExprKind::GenericInstantiation { target, .. } => {
            collect_local_binding_uses_in_expr(target, reference_to_binding, uses);
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_local_binding_uses_in_expr(&capture.value, reference_to_binding, uses);
            }
            collect_local_binding_uses_in_expr(body, reference_to_binding, uses);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Identifier(_) => {}
    }
}

fn classify_expr_effects(node_id: AnalysisFlowNodeId, expr: &ast::Expr) -> AnalysisFlowNodeEffects {
    let mut effects = AnalysisFlowNodeEffects {
        node_id,
        has_call: false,
        has_memory_read: false,
        has_memory_write: false,
        has_control_flow: false,
        is_pure: true,
    };
    accumulate_expr_effects(expr, &mut effects);
    effects.is_pure = !effects.has_call
        && !effects.has_memory_read
        && !effects.has_memory_write
        && !effects.has_control_flow;
    effects
}

fn accumulate_expr_effects(expr: &ast::Expr, effects: &mut AnalysisFlowNodeEffects) {
    match &expr.kind {
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::TypeNode(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer => {}
        ast::ExprKind::Unary { op, operand } => {
            if matches!(op, ast::UnaryOperator::PointerDeRef) {
                effects.has_memory_read = true;
            }
            accumulate_expr_effects(operand, effects);
        }
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            accumulate_expr_effects(lhs, effects);
            accumulate_expr_effects(rhs, effects);
        }
        ast::ExprKind::FieldAccess { lhs, .. } => {
            effects.has_memory_read = true;
            accumulate_expr_effects(lhs, effects);
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            effects.has_memory_read = true;
            accumulate_expr_effects(lhs, effects);
            accumulate_expr_effects(index, effects);
        }
        ast::ExprKind::Call { callee, args } => {
            effects.has_call = true;
            accumulate_expr_effects(callee, effects);
            for arg in args {
                accumulate_expr_effects(arg, effects);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    accumulate_expr_effects(&field.value, effects);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    accumulate_expr_effects(item, effects);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                accumulate_expr_effects(value, effects);
                accumulate_expr_effects(count, effects);
            }
            ast::DataLiteralKind::Scalar(value) => accumulate_expr_effects(value, effects),
        },
        ast::ExprKind::As { lhs, .. } => accumulate_expr_effects(lhs, effects),
        ast::ExprKind::Propagate { operand, .. } => {
            effects.has_control_flow = true;
            accumulate_expr_effects(operand, effects);
        }
        ast::ExprKind::GenericInstantiation { target, .. } => {
            accumulate_expr_effects(target, effects);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            effects.has_memory_read = true;
            accumulate_expr_effects(lhs, effects);
            if let Some(start) = start {
                accumulate_expr_effects(start, effects);
            }
            if let Some(end) = end {
                accumulate_expr_effects(end, effects);
            }
        }
        ast::ExprKind::Closure { captures, .. } => {
            for capture in captures {
                accumulate_expr_effects(&capture.value, effects);
            }
        }
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            effects.has_control_flow = true;
            accumulate_expr_effects(init, effects);
            if let Some(else_branch) = else_branch {
                accumulate_expr_effects(else_branch, effects);
            }
        }
        ast::ExprKind::Static { init, .. } => {
            effects.has_memory_write = true;
            accumulate_expr_effects(init, effects);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            effects.has_control_flow = true;
            accumulate_expr_effects(cond, effects);
            accumulate_expr_effects(then_branch, effects);
            if let Some(else_branch) = else_branch {
                accumulate_expr_effects(else_branch, effects);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            effects.has_control_flow = true;
            accumulate_expr_effects(target, effects);
            for arm in arms {
                accumulate_expr_effects(&arm.body, effects);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            effects.has_control_flow = true;
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        accumulate_expr_effects(expr, effects);
                    }
                }
            }
            if let Some(result) = result {
                accumulate_expr_effects(result, effects);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            effects.has_control_flow = true;
            if let Some(init) = init {
                accumulate_expr_effects(init, effects);
            }
            if let Some(cond) = cond {
                accumulate_expr_effects(cond, effects);
            }
            if let Some(post) = post {
                accumulate_expr_effects(post, effects);
            }
            accumulate_expr_effects(body, effects);
        }
        ast::ExprKind::Defer { expr } => {
            effects.has_control_flow = true;
            accumulate_expr_effects(expr, effects);
        }
        ast::ExprKind::Return(value) => {
            effects.has_control_flow = true;
            if let Some(value) = value {
                accumulate_expr_effects(value, effects);
            }
        }
        ast::ExprKind::Break | ast::ExprKind::Continue => {
            effects.has_control_flow = true;
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            effects.has_memory_write = true;
            accumulate_expr_effects(lhs, effects);
            accumulate_expr_effects(rhs, effects);
        }
    }
}
