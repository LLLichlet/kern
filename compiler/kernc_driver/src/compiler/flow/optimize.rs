use super::*;
use crate::compiler::{AnalysisFlowBindingSummary, AnalysisFlowDefinitionKind};
use kernc_ast as ast;
use kernc_lower::{
    FlowLoweringElisionHints, FlowLoweringForwardingHints, FlowLoweringOwnerHints,
};
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::{TypeId, TypeKind};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct FlowOwnerOptimizationFacts {
    elision: FlowOwnerElisionFacts,
    forwarding: FlowOwnerForwardingFacts,
}

#[derive(Default)]
struct FlowOwnerElisionFacts {
    pure_dead_initializer_expr_ids: HashSet<kernc_utils::NodeId>,
    pure_dead_assignment_expr_ids: HashSet<kernc_utils::NodeId>,
    elidable_binding_expr_ids: HashSet<kernc_utils::NodeId>,
}

#[derive(Default)]
struct FlowOwnerForwardingFacts {
    identifier_copy_sources: HashMap<kernc_utils::NodeId, String>,
    forwardable_binding_sources: HashMap<kernc_utils::NodeId, String>,
    forwardable_value_expr_ids: HashSet<kernc_utils::NodeId>,
}

struct FlowOwnerOptimizationContext<'a, 'ctx> {
    owner: &'a FlowOwnerFacts,
    ctx: &'a SemaContext<'ctx>,
    owner_exprs: HashMap<kernc_utils::NodeId, &'a ast::Expr>,
    simple_binding_let_expr_ids: HashMap<Span, kernc_utils::NodeId>,
    bindings_by_id: HashMap<AnalysisFlowBindingId, &'a FlowBindingFacts>,
    binding_summaries_by_id: HashMap<AnalysisFlowBindingId, &'a AnalysisFlowBindingSummary>,
}

impl FlowOwnerOptimizationFacts {
    pub(super) fn is_empty(&self) -> bool {
        self.elision.is_empty() && self.forwarding.is_empty()
    }

    pub(super) fn into_lowering_hints(self) -> FlowLoweringOwnerHints {
        FlowLoweringOwnerHints {
            elision: FlowLoweringElisionHints {
                pure_dead_initializer_expr_ids: self.elision.pure_dead_initializer_expr_ids,
                pure_dead_assignment_expr_ids: self.elision.pure_dead_assignment_expr_ids,
                elidable_binding_expr_ids: self.elision.elidable_binding_expr_ids,
            },
            forwarding: FlowLoweringForwardingHints {
                identifier_copy_sources: self.forwarding.identifier_copy_sources,
                forwardable_binding_sources: self.forwarding.forwardable_binding_sources,
                forwardable_value_expr_ids: self.forwarding.forwardable_value_expr_ids,
            },
        }
    }
}

impl FlowOwnerElisionFacts {
    fn is_empty(&self) -> bool {
        self.pure_dead_initializer_expr_ids.is_empty()
            && self.pure_dead_assignment_expr_ids.is_empty()
            && self.elidable_binding_expr_ids.is_empty()
    }
}

impl FlowOwnerForwardingFacts {
    fn is_empty(&self) -> bool {
        self.identifier_copy_sources.is_empty()
            && self.forwardable_binding_sources.is_empty()
            && self.forwardable_value_expr_ids.is_empty()
    }
}

impl<'a, 'ctx> FlowOwnerOptimizationContext<'a, 'ctx> {
    fn new(owner: &'a FlowOwnerFacts, ctx: &'a SemaContext<'ctx>) -> Self {
        Self {
            owner,
            ctx,
            owner_exprs: owner_expr_map(ctx, owner.def_id),
            simple_binding_let_expr_ids: owner_simple_binding_let_expr_ids(ctx, owner.def_id),
            bindings_by_id: owner
                .bindings
                .iter()
                .map(|binding| (binding.id, binding))
                .collect(),
            binding_summaries_by_id: owner
                .binding_summaries
                .iter()
                .map(|summary| (summary.binding_id, summary))
                .collect(),
        }
    }

    fn collect(self) -> FlowOwnerOptimizationFacts {
        FlowOwnerOptimizationFacts {
            elision: self.collect_elision_facts(),
            forwarding: self.collect_forwarding_facts(),
        }
    }

    fn collect_elision_facts(&self) -> FlowOwnerElisionFacts {
        let purity_ctx = FlowBindingPurityContext {
            ctx: self.ctx,
            definition_facts: &self.owner.definition_facts,
            bindings_by_id: &self.bindings_by_id,
            binding_summaries_by_id: &self.binding_summaries_by_id,
            owner_exprs: &self.owner_exprs,
            simple_binding_let_expr_ids: &self.simple_binding_let_expr_ids,
        };
        let def_use_by_definition = self
            .owner
            .def_uses
            .iter()
            .map(|def_use| (def_use.definition, def_use))
            .collect::<HashMap<_, _>>();
        let definition_groups = self.owner.definition_facts.iter().fold(
            HashMap::<AnalysisFlowNodeId, Vec<&super::AnalysisFlowDefinitionFacts>>::new(),
            |mut groups, facts| {
                groups
                    .entry(facts.definition.node_id)
                    .or_default()
                    .push(facts);
                groups
            },
        );

        let mut facts = FlowOwnerElisionFacts::default();
        for (node_id, definition_facts) in definition_groups {
            let all_dead = definition_facts.iter().all(|definition_facts| {
                def_use_by_definition
                    .get(&definition_facts.definition)
                    .is_some_and(|def_use| def_use.use_node_ids.is_empty())
            });
            if !all_dead {
                continue;
            }

            let Some(ast_node_id) = self
                .owner
                .cfg
                .nodes
                .get(node_id.index())
                .and_then(|node| node.ast_node_id)
            else {
                continue;
            };
            let Some(expr) = self.owner_exprs.get(&ast_node_id).copied() else {
                continue;
            };

            match definition_facts[0].kind {
                AnalysisFlowDefinitionKind::Initializer
                    if removable_initializer_is_pure(self.ctx, expr) =>
                {
                    facts.pure_dead_initializer_expr_ids.insert(ast_node_id);
                }
                AnalysisFlowDefinitionKind::Assignment
                    if removable_assignment_is_pure(self.ctx, expr) =>
                {
                    facts.pure_dead_assignment_expr_ids.insert(ast_node_id);
                }
                _ => {}
            }
        }

        for binding in &self.owner.bindings {
            let Some(let_expr_id) = self
                .simple_binding_let_expr_ids
                .get(&binding.definition_span)
                .copied()
            else {
                continue;
            };
            if purity_ctx.is_elidable_pure_binding(binding.id) {
                facts.elidable_binding_expr_ids.insert(let_expr_id);
            }
        }

        facts
    }

    fn collect_forwarding_facts(&self) -> FlowOwnerForwardingFacts {
        let purity_ctx = FlowBindingPurityContext {
            ctx: self.ctx,
            definition_facts: &self.owner.definition_facts,
            bindings_by_id: &self.bindings_by_id,
            binding_summaries_by_id: &self.binding_summaries_by_id,
            owner_exprs: &self.owner_exprs,
            simple_binding_let_expr_ids: &self.simple_binding_let_expr_ids,
        };
        let mut facts = FlowOwnerForwardingFacts::default();

        for binding in &self.owner.bindings {
            let Some(let_expr_id) = self
                .simple_binding_let_expr_ids
                .get(&binding.definition_span)
                .copied()
            else {
                continue;
            };

            if purity_ctx.is_forwardable_pure_value_binding(binding.id) {
                facts.forwardable_value_expr_ids.insert(let_expr_id);
            }

            let Some(source_binding_id) = resolve_immutable_copy_origin_binding(
                binding.id,
                &self.owner.definition_facts,
                &self.bindings_by_id,
                &self.binding_summaries_by_id,
            ) else {
                continue;
            };
            if source_binding_id == binding.id {
                continue;
            }

            let Some(source_name) = self.binding_source_name(source_binding_id) else {
                continue;
            };
            facts
                .forwardable_binding_sources
                .insert(let_expr_id, source_name);
        }

        for single_source in &self.owner.single_source_uses {
            let Some(node) = self.owner.cfg.nodes.get(single_source.node_id.index()) else {
                continue;
            };
            let Some(use_expr_id) = node.ast_node_id else {
                continue;
            };
            let Some(use_expr) = self.owner_exprs.get(&use_expr_id).copied() else {
                continue;
            };
            if !matches!(use_expr.kind, ast::ExprKind::Identifier(_)) {
                continue;
            }

            let Some(source_binding_id) = resolve_immutable_copy_origin_binding(
                single_source.binding_id,
                &self.owner.definition_facts,
                &self.bindings_by_id,
                &self.binding_summaries_by_id,
            ) else {
                continue;
            };
            let Some(source_name) = self.binding_source_name(source_binding_id) else {
                continue;
            };
            facts
                .identifier_copy_sources
                .insert(use_expr_id, source_name);
        }

        facts
    }

    fn binding_source_name(&self, binding_id: AnalysisFlowBindingId) -> Option<String> {
        let source_binding = self.bindings_by_id.get(&binding_id).copied()?;
        let source_name = self
            .ctx
            .sess
            .source_manager
            .slice_source(source_binding.definition_span)
            .trim()
            .to_string();
        if source_name.is_empty() {
            None
        } else {
            Some(source_name)
        }
    }
}

pub(super) fn collect_owner_optimization_facts(
    owner: &FlowOwnerFacts,
    ctx: &SemaContext<'_>,
) -> FlowOwnerOptimizationFacts {
    FlowOwnerOptimizationContext::new(owner, ctx).collect()
}

fn owner_expr_map<'a>(
    ctx: &'a SemaContext<'_>,
    def_id: DefId,
) -> HashMap<kernc_utils::NodeId, &'a ast::Expr> {
    let mut exprs = HashMap::new();

    match &ctx.defs[def_id.0 as usize] {
        Def::Function(function) => {
            if let Some(body) = function.body.as_ref() {
                collect_owner_exprs(body, &mut exprs);
            }
        }
        Def::Global(global) => {
            collect_owner_exprs(&global.value, &mut exprs);
        }
        _ => {}
    }

    exprs
}

fn owner_simple_binding_let_expr_ids(
    ctx: &SemaContext<'_>,
    def_id: DefId,
) -> HashMap<Span, kernc_utils::NodeId> {
    let mut expr_ids = HashMap::new();

    match &ctx.defs[def_id.0 as usize] {
        Def::Function(function) => {
            if let Some(body) = function.body.as_ref() {
                collect_simple_binding_let_expr_ids(body, &mut expr_ids);
            }
        }
        Def::Global(global) => {
            collect_simple_binding_let_expr_ids(&global.value, &mut expr_ids);
        }
        _ => {}
    }

    expr_ids
}

fn collect_owner_exprs<'a>(
    expr: &'a ast::Expr,
    exprs: &mut HashMap<kernc_utils::NodeId, &'a ast::Expr>,
) {
    exprs.insert(expr.id, expr);

    match &expr.kind {
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_owner_exprs(init, exprs);
            if let Some(else_branch) = else_branch {
                collect_owner_exprs(else_branch, exprs);
            }
        }
        ast::ExprKind::Static { init, .. } => collect_owner_exprs(init, exprs),
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(rhs, exprs);
        }
        ast::ExprKind::Unary { operand, .. } => collect_owner_exprs(operand, exprs),
        ast::ExprKind::FieldAccess { lhs, .. } => collect_owner_exprs(lhs, exprs),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(index, exprs);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_owner_exprs(callee, exprs);
            for arg in args {
                collect_owner_exprs(arg, exprs);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_owner_exprs(&field.value, exprs);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_owner_exprs(item, exprs);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_owner_exprs(value, exprs);
                collect_owner_exprs(count, exprs);
            }
            ast::DataLiteralKind::Scalar(value) => collect_owner_exprs(value, exprs),
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_owner_exprs(cond, exprs);
            collect_owner_exprs(then_branch, exprs);
            if let Some(else_branch) = else_branch {
                collect_owner_exprs(else_branch, exprs);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_owner_exprs(target, exprs);
            for arm in arms {
                collect_owner_exprs(&arm.body, exprs);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_owner_exprs(expr, exprs);
                    }
                }
            }
            if let Some(result) = result {
                collect_owner_exprs(result, exprs);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_owner_exprs(init, exprs);
            }
            if let Some(cond) = cond {
                collect_owner_exprs(cond, exprs);
            }
            if let Some(post) = post {
                collect_owner_exprs(post, exprs);
            }
            collect_owner_exprs(body, exprs);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_owner_exprs(lhs, exprs);
            if let Some(start) = start {
                collect_owner_exprs(start, exprs);
            }
            if let Some(end) = end {
                collect_owner_exprs(end, exprs);
            }
        }
        ast::ExprKind::Defer { expr } => collect_owner_exprs(expr, exprs),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_owner_exprs(value, exprs);
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_owner_exprs(lhs, exprs);
            collect_owner_exprs(rhs, exprs);
        }
        ast::ExprKind::As { lhs, .. } => collect_owner_exprs(lhs, exprs),
        ast::ExprKind::GenericInstantiation { target, .. } => collect_owner_exprs(target, exprs),
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_owner_exprs(&capture.value, exprs);
            }
            collect_owner_exprs(body, exprs);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => {}
    }
}

fn collect_simple_binding_let_expr_ids(
    expr: &ast::Expr,
    expr_ids: &mut HashMap<Span, kernc_utils::NodeId>,
) {
    if let ast::ExprKind::Let {
        pattern,
        else_branch,
        ..
    } = &expr.kind
        && else_branch.is_none()
        && let ast::PatternKind::Binding(binding) = &pattern.pattern.kind
    {
        expr_ids.insert(binding.name_span, expr.id);
    }

    match &expr.kind {
        ast::ExprKind::Let {
            init, else_branch, ..
        } => {
            collect_simple_binding_let_expr_ids(init, expr_ids);
            if let Some(else_branch) = else_branch {
                collect_simple_binding_let_expr_ids(else_branch, expr_ids);
            }
        }
        ast::ExprKind::Static { init, .. } => collect_simple_binding_let_expr_ids(init, expr_ids),
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(rhs, expr_ids);
        }
        ast::ExprKind::Unary { operand, .. } => {
            collect_simple_binding_let_expr_ids(operand, expr_ids);
        }
        ast::ExprKind::FieldAccess { lhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids)
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(index, expr_ids);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_simple_binding_let_expr_ids(callee, expr_ids);
            for arg in args {
                collect_simple_binding_let_expr_ids(arg, expr_ids);
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_simple_binding_let_expr_ids(&field.value, expr_ids);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_simple_binding_let_expr_ids(item, expr_ids);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_simple_binding_let_expr_ids(value, expr_ids);
                collect_simple_binding_let_expr_ids(count, expr_ids);
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_simple_binding_let_expr_ids(value, expr_ids);
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_simple_binding_let_expr_ids(cond, expr_ids);
            collect_simple_binding_let_expr_ids(then_branch, expr_ids);
            if let Some(else_branch) = else_branch {
                collect_simple_binding_let_expr_ids(else_branch, expr_ids);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_simple_binding_let_expr_ids(target, expr_ids);
            for arm in arms {
                collect_simple_binding_let_expr_ids(&arm.body, expr_ids);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_simple_binding_let_expr_ids(expr, expr_ids);
                    }
                }
            }
            if let Some(result) = result {
                collect_simple_binding_let_expr_ids(result, expr_ids);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                collect_simple_binding_let_expr_ids(init, expr_ids);
            }
            if let Some(cond) = cond {
                collect_simple_binding_let_expr_ids(cond, expr_ids);
            }
            if let Some(post) = post {
                collect_simple_binding_let_expr_ids(post, expr_ids);
            }
            collect_simple_binding_let_expr_ids(body, expr_ids);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            if let Some(start) = start {
                collect_simple_binding_let_expr_ids(start, expr_ids);
            }
            if let Some(end) = end {
                collect_simple_binding_let_expr_ids(end, expr_ids);
            }
        }
        ast::ExprKind::Defer { expr } => collect_simple_binding_let_expr_ids(expr, expr_ids),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                collect_simple_binding_let_expr_ids(value, expr_ids);
            }
        }
        ast::ExprKind::Assign { lhs, rhs, .. } => {
            collect_simple_binding_let_expr_ids(lhs, expr_ids);
            collect_simple_binding_let_expr_ids(rhs, expr_ids);
        }
        ast::ExprKind::As { lhs, .. } => collect_simple_binding_let_expr_ids(lhs, expr_ids),
        ast::ExprKind::GenericInstantiation { target, .. } => {
            collect_simple_binding_let_expr_ids(target, expr_ids);
        }
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_simple_binding_let_expr_ids(&capture.value, expr_ids);
            }
            collect_simple_binding_let_expr_ids(body, expr_ids);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => {}
    }
}

fn removable_initializer_is_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    let ast::ExprKind::Let { init, .. } = &expr.kind else {
        return false;
    };
    expr_is_strictly_pure(ctx, init)
}

fn removable_assignment_is_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    let ast::ExprKind::Assign { lhs, op, rhs } = &expr.kind else {
        return false;
    };
    matches!(lhs.kind, ast::ExprKind::Identifier(_))
        && *op == ast::AssignmentOperator::Assign
        && expr_is_strictly_pure(ctx, rhs)
}

fn expr_is_strictly_pure(ctx: &SemaContext<'_>, expr: &ast::Expr) -> bool {
    match &expr.kind {
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::EnumLiteral { .. }
        | ast::ExprKind::SelfValue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer => true,
        ast::ExprKind::Unary { op, operand } => {
            !matches!(
                op,
                ast::UnaryOperator::PointerDeRef
                    | ast::UnaryOperator::AddressOf
                    | ast::UnaryOperator::MutAddressOf
            ) && expr_is_strictly_pure(ctx, operand)
        }
        ast::ExprKind::Binary { lhs, rhs, .. } => {
            expr_is_strictly_pure(ctx, lhs) && expr_is_strictly_pure(ctx, rhs)
        }
        ast::ExprKind::DataInit { literal, .. } => {
            let ty = ctx
                .node_types
                .get(&expr.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_ty = ctx.type_registry.normalize(ty);

            if !matches!(ctx.type_registry.get(norm_ty), TypeKind::Primitive(_)) {
                return false;
            }

            match literal {
                ast::DataLiteralKind::Struct(fields) => fields
                    .iter()
                    .all(|field| expr_is_strictly_pure(ctx, &field.value)),
                ast::DataLiteralKind::Array(items) => {
                    items.iter().all(|item| expr_is_strictly_pure(ctx, item))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    expr_is_strictly_pure(ctx, value) && expr_is_strictly_pure(ctx, count)
                }
                ast::DataLiteralKind::Scalar(value) => expr_is_strictly_pure(ctx, value),
            }
        }
        ast::ExprKind::As { lhs, .. } => expr_is_strictly_pure(ctx, lhs),
        ast::ExprKind::GenericInstantiation { target, .. } => expr_is_strictly_pure(ctx, target),
        ast::ExprKind::Closure { captures, .. } => captures
            .iter()
            .all(|capture| expr_is_strictly_pure(ctx, &capture.value)),
        ast::ExprKind::FieldAccess { .. }
        | ast::ExprKind::IndexAccess { .. }
        | ast::ExprKind::Call { .. }
        | ast::ExprKind::If { .. }
        | ast::ExprKind::Match { .. }
        | ast::ExprKind::Block { .. }
        | ast::ExprKind::For { .. }
        | ast::ExprKind::SliceOp { .. }
        | ast::ExprKind::Defer { .. }
        | ast::ExprKind::Return(_)
        | ast::ExprKind::Assign { .. }
        | ast::ExprKind::Let { .. }
        | ast::ExprKind::Static { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue => false,
    }
}

fn resolve_immutable_copy_origin_binding(
    binding_id: AnalysisFlowBindingId,
    definition_facts: &[super::AnalysisFlowDefinitionFacts],
    bindings_by_id: &HashMap<AnalysisFlowBindingId, &FlowBindingFacts>,
    binding_summaries_by_id: &HashMap<AnalysisFlowBindingId, &AnalysisFlowBindingSummary>,
) -> Option<AnalysisFlowBindingId> {
    let mut current = binding_id;

    loop {
        let binding = bindings_by_id.get(&current).copied()?;
        if binding.kind == AnalysisFlowBindingKind::Parameter && !binding.is_mut {
            return Some(current);
        }
        if binding.kind != AnalysisFlowBindingKind::Variable || binding.is_mut {
            return None;
        }

        let summary = binding_summaries_by_id.get(&current).copied()?;
        if summary.definition_node_ids.len() != 1 {
            return None;
        }

        let definition_facts = definition_facts.iter().find(|facts| {
            facts.definition.binding_id == current
                && facts.definition.node_id == summary.definition_node_ids[0]
                && facts.kind == AnalysisFlowDefinitionKind::Initializer
        })?;

        let source_binding_id = definition_facts.copy_source_binding_id?;
        if source_binding_id == current {
            return None;
        }
        current = source_binding_id;
    }
}

struct FlowBindingPurityContext<'a, 'ctx> {
    ctx: &'a SemaContext<'ctx>,
    definition_facts: &'a [super::AnalysisFlowDefinitionFacts],
    bindings_by_id: &'a HashMap<AnalysisFlowBindingId, &'a FlowBindingFacts>,
    binding_summaries_by_id: &'a HashMap<AnalysisFlowBindingId, &'a AnalysisFlowBindingSummary>,
    owner_exprs: &'a HashMap<kernc_utils::NodeId, &'a ast::Expr>,
    simple_binding_let_expr_ids: &'a HashMap<Span, kernc_utils::NodeId>,
}

impl FlowBindingPurityContext<'_, '_> {
    fn is_elidable_pure_binding(&self, binding_id: AnalysisFlowBindingId) -> bool {
        let Some(binding) = self.bindings_by_id.get(&binding_id).copied() else {
            return false;
        };
        if binding.kind != AnalysisFlowBindingKind::Variable || binding.is_mut {
            return false;
        }

        let Some(summary) = self.binding_summaries_by_id.get(&binding_id).copied() else {
            return false;
        };
        if !summary.use_node_ids.is_empty() || summary.definition_node_ids.len() != 1 {
            return false;
        }

        let Some(let_expr_id) = self
            .simple_binding_let_expr_ids
            .get(&binding.definition_span)
            .copied()
        else {
            return false;
        };
        let Some(let_expr) = self.owner_exprs.get(&let_expr_id).copied() else {
            return false;
        };

        removable_initializer_is_pure(self.ctx, let_expr)
    }

    fn is_forwardable_pure_value_binding(&self, binding_id: AnalysisFlowBindingId) -> bool {
        let mut visiting = HashSet::new();
        let mut memo = HashMap::new();
        self.is_forwardable_pure_value_binding_inner(binding_id, &mut visiting, &mut memo)
    }

    fn is_forwardable_pure_value_binding_inner(
        &self,
        binding_id: AnalysisFlowBindingId,
        visiting: &mut HashSet<AnalysisFlowBindingId>,
        memo: &mut HashMap<AnalysisFlowBindingId, bool>,
    ) -> bool {
        if let Some(result) = memo.get(&binding_id).copied() {
            return result;
        }
        if !visiting.insert(binding_id) {
            return false;
        }

        let result = match self.bindings_by_id.get(&binding_id).copied() {
            Some(binding) if binding.kind == AnalysisFlowBindingKind::Parameter => !binding.is_mut,
            Some(binding)
                if binding.kind == AnalysisFlowBindingKind::Variable && !binding.is_mut =>
            {
                match self.binding_summaries_by_id.get(&binding_id).copied() {
                    Some(summary) if summary.definition_node_ids.len() == 1 => {
                        match self
                            .simple_binding_let_expr_ids
                            .get(&binding.definition_span)
                            .copied()
                        {
                            Some(let_expr_id) => {
                                match self.owner_exprs.get(&let_expr_id).copied() {
                                    Some(let_expr)
                                        if removable_initializer_is_pure(self.ctx, let_expr) =>
                                    {
                                        let definition =
                                            self.definition_facts.iter().find(|facts| {
                                                facts.definition.binding_id == binding_id
                                                    && facts.definition.node_id
                                                        == summary.definition_node_ids[0]
                                                    && facts.kind
                                                        == AnalysisFlowDefinitionKind::Initializer
                                            });
                                        definition.is_some_and(|facts| {
                                            facts.use_binding_ids.iter().all(|used_binding_id| {
                                                self.is_forwardable_pure_value_binding_inner(
                                                    *used_binding_id,
                                                    visiting,
                                                    memo,
                                                )
                                            })
                                        })
                                    }
                                    _ => false,
                                }
                            }
                            None => false,
                        }
                    }
                    _ => false,
                }
            }
            _ => false,
        };

        visiting.remove(&binding_id);
        memo.insert(binding_id, result);
        result
    }
}
