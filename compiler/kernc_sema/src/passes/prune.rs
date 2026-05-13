use crate::def::DefId;
use kernc_ast::*;
use kernc_utils::Session;

pub struct Pruner<'a> {
    sess: &'a mut Session,
}

impl<'a> Pruner<'a> {
    pub fn new(sess: &'a mut Session) -> Self {
        Self { sess }
    }

    /// Walk every module and prune declarations gated by conditional attributes.
    pub fn prune_all(&mut self, asts: &mut [(DefId, Module)]) {
        for (_, module) in asts.iter_mut() {
            self.prune_module(module);
        }
    }

    pub fn prune_module(&mut self, module: &mut Module) {
        // Handle module-level `#![if(...)]` attributes first.
        if !self.eval_attributes(&module.attributes) {
            // A false module-level condition drops the entire module body.
            module.decls.clear();
            return;
        }

        // Filter top-level declarations in place.
        module.decls.retain_mut(|decl| self.prune_decl(decl));
    }

    /// Return whether a declaration survives conditional pruning.
    fn prune_decl(&mut self, decl: &mut Decl) -> bool {
        // 1. Evaluate the declaration's own condition.
        if !self.eval_attributes(&decl.attributes) {
            return false;
        }

        // 2. Recurse into nested item containers such as impl and extern blocks.
        match &mut decl.kind {
            DeclKind::Impl { decls, .. } | DeclKind::ExternBlock { decls, .. } => {
                decls.retain_mut(|d| self.prune_decl(d));
            }
            DeclKind::Function { body: Some(b), .. } => {
                self.prune_expr(b);
            }
            _ => {}
        }
        true
    }

    fn prune_expr(&mut self, expr: &mut Expr) {
        match &mut expr.kind {
            // Recurse into blocks and filter their statements.
            ExprKind::Block { stmts, result } => {
                stmts.retain_mut(|stmt| {
                    if !self.eval_attributes(&stmt.attributes) {
                        false
                    } else {
                        match &mut stmt.kind {
                            StmtKind::Use(_) => {}
                            StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                                self.prune_expr(e);
                            }
                        }
                        true
                    }
                });
                if let Some(r) = result {
                    self.prune_expr(r);
                }
            }

            // Recursively visit every expression form that owns child expressions.
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.prune_expr(cond);
                self.prune_expr(then_branch);
                if let Some(e) = else_branch {
                    self.prune_expr(e);
                }
            }
            ExprKind::Match { target, arms } => {
                self.prune_expr(target);
                for arm in arms {
                    // Visit expressions embedded inside patterns such as values and ranges.
                    for pat in &mut arm.patterns {
                        match &mut pat.kind {
                            MatchPatternKind::Value(e) => self.prune_expr(e),
                            MatchPatternKind::Range { start, end, .. } => {
                                self.prune_expr(start);
                                self.prune_expr(end);
                            }
                            _ => {} // `Variant` and `CatchAll` contain no standalone expression nodes.
                        }
                    }
                    self.prune_expr(&mut arm.body);
                }
            }
            ExprKind::While { cond, body } => {
                self.prune_expr(cond);
                self.prune_expr(body);
            }
            ExprKind::Closure { body, .. } => self.prune_expr(body),
            ExprKind::Let {
                init, else_clause, ..
            } => {
                self.prune_expr(init);
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        LetElseClause::Expr(else_expr) => self.prune_expr(else_expr),
                        LetElseClause::Arms(arms) => {
                            for arm in arms {
                                self.prune_expr(&mut arm.body);
                            }
                        }
                    }
                }
            }
            ExprKind::Static { init, .. } => {
                if let Some(init) = init {
                    self.prune_expr(init);
                }
            }
            ExprKind::Binary { lhs, rhs, .. } | ExprKind::Assign { lhs, rhs, .. } => {
                self.prune_expr(lhs);
                self.prune_expr(rhs);
            }
            ExprKind::Unary { operand, .. } => self.prune_expr(operand),
            ExprKind::FieldAccess { lhs, .. } | ExprKind::As { lhs, .. } => self.prune_expr(lhs),
            ExprKind::Propagate { operand, .. } => self.prune_expr(operand),
            ExprKind::IndexAccess { lhs, index, .. } => {
                self.prune_expr(lhs);
                self.prune_expr(index);
            }
            ExprKind::Call { callee, args } => {
                self.prune_expr(callee);
                for arg in args {
                    self.prune_expr(arg);
                }
            }
            ExprKind::DataInit { literal, .. } => match literal {
                DataLiteralKind::Struct(fields) => {
                    for f in fields {
                        self.prune_expr(&mut f.value);
                    }
                }
                DataLiteralKind::Array(elems) => {
                    for e in elems {
                        self.prune_expr(e);
                    }
                }
                DataLiteralKind::Repeat { value, count } => {
                    self.prune_expr(value);
                    self.prune_expr(count);
                }
                DataLiteralKind::Scalar(val) => self.prune_expr(val),
            },
            ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.prune_expr(lhs);
                if let Some(s) = start {
                    self.prune_expr(s);
                }
                if let Some(e) = end {
                    self.prune_expr(e);
                }
            }
            ExprKind::Defer { expr: e } => self.prune_expr(e),
            ExprKind::Return(Some(e)) => self.prune_expr(e),
            ExprKind::GenericInstantiation { target, .. } => self.prune_expr(target),

            // Leaf nodes such as literals and control markers need no recursion.
            _ => {}
        }
    }

    /// Return `false` when any `#[if(expr)]` attribute evaluates to false.
    fn eval_attributes(&mut self, attributes: &[Attribute]) -> bool {
        for attr in attributes {
            if let AttributeKind::If(cond_expr) = &attr.kind {
                // Evaluate the condition with the lightweight attribute evaluator.
                let mut evaluator = ConditionEvaluator::new(self.sess);
                match evaluator.eval(cond_expr) {
                    Ok(val) => {
                        if !val {
                            return false; // One false condition is enough to prune the item.
                        }
                    }
                    Err(_) => {
                        // Treat evaluation failures as false to avoid cascading errors.
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// Lightweight evaluator used specifically for `#[if(...)]`.
/// It supports boolean logic, string comparison, and injected environment variables without full type checking.
struct ConditionEvaluator<'a> {
    sess: &'a mut Session,
}

impl<'a> ConditionEvaluator<'a> {
    fn new(sess: &'a mut Session) -> Self {
        Self { sess }
    }

    fn eval(&mut self, expr: &Expr) -> Result<bool, ()> {
        let val = self.eval_inner(expr)?;
        match val {
            CondValue::Bool(b) => Ok(b),
            _ => {
                self.sess
                    .struct_error(expr.span, "condition must evaluate to a boolean value")
                    .emit();
                Err(())
            }
        }
    }

    fn eval_inner(&mut self, expr: &Expr) -> Result<CondValue, ()> {
        match &expr.kind {
            ExprKind::Bool(b) => Ok(CondValue::Bool(*b)),
            ExprKind::String(s) => Ok(CondValue::String(s.clone())),

            // Resolve bare identifiers as injected environment variables.
            ExprKind::Identifier(sym) => {
                let name = self.sess.resolve(*sym);

                // 1. Check builtin platform keys first.
                if name == "os" {
                    let raw_os = self.sess.target.triple.operating_system.to_string();
                    let os_str = if raw_os.starts_with("darwin") || raw_os.starts_with("macosx") {
                        "darwin".to_string()
                    } else {
                        raw_os
                    };
                    return Ok(CondValue::String(os_str));
                }
                if name == "arch" {
                    let arch_str = self.sess.target.triple.architecture.to_string();
                    return Ok(CondValue::String(arch_str));
                }

                // 2. Fall back to user-provided `--define` variables.
                if let Some(val) = self.sess.custom_defines.get(name) {
                    if val == "true" {
                        return Ok(CondValue::Bool(true));
                    }
                    if val == "false" {
                        return Ok(CondValue::Bool(false));
                    }
                    return Ok(CondValue::String(val.clone()));
                }

                // Unknown variables are rejected.
                self.sess.struct_error(expr.span, format!("unknown environment variable `{}` in compilation condition", name))
                    .with_hint("available variables: `os`, `arch`, or custom defines via CLI (e.g., `--define feature=true`)")
                    .emit();
                Err(())
            }

            ExprKind::Unary {
                op: UnaryOperator::LogicalNot,
                operand,
            } => {
                let val = self.eval_inner(operand)?;
                if let CondValue::Bool(b) = val {
                    Ok(CondValue::Bool(!b))
                } else {
                    self.sess
                        .struct_error(operand.span, "cannot apply `!` to a non-boolean value")
                        .emit();
                    Err(())
                }
            }

            ExprKind::Grouped { expr: inner } => self.eval_inner(inner),

            ExprKind::Binary { lhs, op, rhs } => {
                let left = self.eval_inner(lhs)?;
                // Support short-circuit evaluation for `and` and `or`.
                if *op == BinaryOperator::LogicalAnd {
                    if let CondValue::Bool(false) = left {
                        return Ok(CondValue::Bool(false));
                    }
                    return self.eval_inner(rhs); // Left side is true, so the right side decides.
                }
                if *op == BinaryOperator::LogicalOr {
                    if let CondValue::Bool(true) = left {
                        return Ok(CondValue::Bool(true));
                    }
                    return self.eval_inner(rhs); // Left side is false, so the right side decides.
                }

                // Other comparison operators such as `==` and `!=`.
                let right = self.eval_inner(rhs)?;
                match op {
                    BinaryOperator::Equal => Ok(CondValue::Bool(left == right)),
                    BinaryOperator::NotEqual => Ok(CondValue::Bool(left != right)),
                    _ => {
                        self.sess
                            .struct_error(
                                expr.span,
                                "unsupported operator in compilation condition",
                            )
                            .emit();
                        Err(())
                    }
                }
            }

            _ => {
                self.sess.struct_error(expr.span, "invalid expression in compilation condition")
                    .with_hint("conditions only support booleans, strings, `and`, `or`, `!`, `==`, and `!=`")
                    .emit();
                Err(())
            }
        }
    }
}

#[derive(PartialEq)]
enum CondValue {
    Bool(bool),
    String(String),
}
