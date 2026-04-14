use super::Lowerer;
use kernc_mast::*;
use kernc_mono::MonoId;
use kernc_sema::ty::TypeId;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

#[derive(Debug, Clone)]
struct InlineTemplate {
    params: Vec<MastParam>,
    stmts: Vec<MastStmt>,
    result: MastExpr,
}

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn run_inline_pass(&mut self) {
        let templates = self.collect_inline_templates();
        if templates.is_empty() {
            return;
        }

        for index in 0..self.module.functions.len() {
            let function_id = self.module.functions[index].id;
            let Some(mut body) = self.module.functions[index].body.take() else {
                continue;
            };

            let mut stack = vec![function_id];
            self.inline_block(&mut body, &templates, &mut stack);
            self.module.functions[index].body = Some(body);
        }
    }

    fn collect_inline_templates(&self) -> HashMap<MonoId, InlineTemplate> {
        let mut templates = HashMap::new();
        for function in &self.module.functions {
            let Some(template) = self.inline_template(function) else {
                continue;
            };
            templates.insert(function.id, template);
        }
        templates
    }

    fn inline_template(&self, function: &MastFunction) -> Option<InlineTemplate> {
        if function.is_extern
            || function.is_variadic
            || !matches!(function.inline_hint, MastInlineHint::Inline)
        {
            return None;
        }

        let body = function.body.as_ref()?;
        if !body.defers.is_empty() {
            return None;
        }

        if let Some(result) = &body.result {
            if body.stmts.iter().any(stmt_contains_return) || expr_contains_return(result) {
                return None;
            }

            return Some(InlineTemplate {
                params: function.params.clone(),
                stmts: body.stmts.clone(),
                result: (**result).clone(),
            });
        }

        let (stmts, result) = split_tail_control(body)?;
        Some(InlineTemplate {
            params: function.params.clone(),
            stmts,
            result,
        })
    }
    fn inline_block(
        &mut self,
        block: &mut MastBlock,
        templates: &HashMap<MonoId, InlineTemplate>,
        stack: &mut Vec<MonoId>,
    ) {
        for stmt in &mut block.stmts {
            self.inline_stmt(stmt, templates, stack);
        }
        if let Some(result) = &mut block.result {
            self.inline_expr(result, templates, stack);
        }
        for defer in &mut block.defers {
            self.inline_expr(defer, templates, stack);
        }
    }

    fn inline_stmt(
        &mut self,
        stmt: &mut MastStmt,
        templates: &HashMap<MonoId, InlineTemplate>,
        stack: &mut Vec<MonoId>,
    ) {
        match stmt {
            MastStmt::Let { init, .. } => self.inline_expr(init, templates, stack),
            MastStmt::Expr(expr) => self.inline_expr(expr, templates, stack),
        }
    }

    fn inline_expr(
        &mut self,
        expr: &mut MastExpr,
        templates: &HashMap<MonoId, InlineTemplate>,
        stack: &mut Vec<MonoId>,
    ) {
        let replacement = match &mut expr.kind {
            MastExprKind::Undef
            | MastExprKind::Unreachable
            | MastExprKind::Trap
            | MastExprKind::Breakpoint
            | MastExprKind::Integer(_)
            | MastExprKind::Float(_)
            | MastExprKind::Bool(_)
            | MastExprKind::StringLiteral(_)
            | MastExprKind::Var(_)
            | MastExprKind::GlobalRef(_)
            | MastExprKind::FuncRef(_)
            | MastExprKind::Break
            | MastExprKind::Continue
            | MastExprKind::Fence { .. } => None,

            MastExprKind::AddressOf(inner)
            | MastExprKind::Deref(inner)
            | MastExprKind::ExtractFatPtrData(inner)
            | MastExprKind::ExtractFatPtrMeta(inner)
            | MastExprKind::Unary { operand: inner, .. }
            | MastExprKind::Cast { operand: inner, .. }
            | MastExprKind::BitIntrinsic { operand: inner, .. }
            | MastExprKind::SimdUnaryIntrinsic { operand: inner, .. }
            | MastExprKind::SimdReduce { operand: inner, .. }
            | MastExprKind::SimdAny { operand: inner, .. }
            | MastExprKind::SimdAll { operand: inner, .. }
            | MastExprKind::SimdBitmask { operand: inner, .. }
            | MastExprKind::SimdSplat { value: inner, .. }
            | MastExprKind::SimdCast { value: inner, .. }
            | MastExprKind::SimdBitcast { value: inner, .. } => {
                self.inline_expr(inner, templates, stack);
                None
            }

            MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
                for field in fields {
                    self.inline_expr(field, templates, stack);
                }
                None
            }

            MastExprKind::UnionInit { value, .. } => {
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::FieldAccess { lhs, .. } => {
                self.inline_expr(lhs, templates, stack);
                None
            }

            MastExprKind::IndexAccess { lhs, index } => {
                self.inline_expr(lhs, templates, stack);
                self.inline_expr(index, templates, stack);
                None
            }

            MastExprKind::Call { callee, args } => {
                self.inline_expr(callee, templates, stack);
                for arg in args.iter_mut() {
                    self.inline_expr(arg, templates, stack);
                }

                let MastExprKind::FuncRef(callee_id) = callee.kind else {
                    return;
                };
                let Some(template) = templates.get(&callee_id).cloned() else {
                    return;
                };
                if args.len() != template.params.len() || stack.contains(&callee_id) {
                    return;
                }

                let block = self.build_inlined_call_block(
                    callee_id,
                    &template,
                    std::mem::take(args),
                    expr.span,
                    templates,
                    stack,
                );
                Some(MastExprKind::Block(block))
            }

            MastExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.inline_expr(cond, templates, stack);
                self.inline_block(then_branch, templates, stack);
                if let Some(else_branch) = else_branch {
                    self.inline_block(else_branch, templates, stack);
                }
                None
            }

            MastExprKind::Loop { body, latch } => {
                self.inline_block(body, templates, stack);
                if let Some(latch) = latch {
                    self.inline_block(latch, templates, stack);
                }
                None
            }

            MastExprKind::Switch {
                target,
                cases,
                default_case,
            } => {
                self.inline_expr(target, templates, stack);
                for case in cases {
                    self.inline_block(&mut case.body, templates, stack);
                }
                if let Some(default_case) = default_case {
                    self.inline_block(default_case, templates, stack);
                }
                None
            }

            MastExprKind::Return(value) => {
                if let Some(value) = value {
                    self.inline_expr(value, templates, stack);
                }
                None
            }

            MastExprKind::Binary { lhs, rhs, .. }
            | MastExprKind::Assign { lhs, rhs, .. }
            | MastExprKind::SimdBinaryIntrinsic { lhs, rhs, .. } => {
                self.inline_expr(lhs, templates, stack);
                self.inline_expr(rhs, templates, stack);
                None
            }

            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                self.inline_expr(data_ptr, templates, stack);
                self.inline_expr(meta, templates, stack);
                None
            }

            MastExprKind::Block(block) => {
                self.inline_block(block, templates, stack);
                None
            }

            MastExprKind::DataInit { payload, .. } => {
                self.inline_expr(payload, templates, stack);
                None
            }

            MastExprKind::Asm(asm) => {
                for input in &mut asm.input_args {
                    self.inline_expr(input, templates, stack);
                }
                for output in &mut asm.output_ptrs {
                    self.inline_expr(output, templates, stack);
                }
                None
            }

            MastExprKind::SimdSelect {
                mask,
                on_true,
                on_false,
            } => {
                self.inline_expr(mask, templates, stack);
                self.inline_expr(on_true, templates, stack);
                self.inline_expr(on_false, templates, stack);
                None
            }

            MastExprKind::SimdShuffle { lhs, rhs, .. } => {
                self.inline_expr(lhs, templates, stack);
                self.inline_expr(rhs, templates, stack);
                None
            }

            MastExprKind::SimdInsertHalf { base, half, .. } => {
                self.inline_expr(base, templates, stack);
                self.inline_expr(half, templates, stack);
                None
            }

            MastExprKind::SimdLoad { ptr, .. } => {
                self.inline_expr(ptr, templates, stack);
                None
            }

            MastExprKind::SimdStore { ptr, value, .. } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::SimdMaskedLoad {
                ptr, mask, or_else, ..
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(mask, templates, stack);
                self.inline_expr(or_else, templates, stack);
                None
            }

            MastExprKind::SimdMaskedStore {
                ptr, mask, value, ..
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(mask, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::SimdGather { ptr, indices } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(indices, templates, stack);
                None
            }

            MastExprKind::SimdScatter {
                ptr,
                indices,
                value,
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(indices, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::SimdMaskedGather {
                ptr,
                indices,
                mask,
                or_else,
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(indices, templates, stack);
                self.inline_expr(mask, templates, stack);
                self.inline_expr(or_else, templates, stack);
                None
            }

            MastExprKind::SimdMaskedScatter {
                ptr,
                indices,
                mask,
                value,
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(indices, templates, stack);
                self.inline_expr(mask, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::AtomicLoad { ptr, .. } => {
                self.inline_expr(ptr, templates, stack);
                None
            }

            MastExprKind::AtomicStore { ptr, value, .. } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::AtomicCas {
                ptr,
                expected,
                desired,
                ..
            } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(expected, templates, stack);
                self.inline_expr(desired, templates, stack);
                None
            }

            MastExprKind::AtomicRmw { ptr, value, .. } => {
                self.inline_expr(ptr, templates, stack);
                self.inline_expr(value, templates, stack);
                None
            }

            MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
                self.inline_expr(dest, templates, stack);
                self.inline_expr(src, templates, stack);
                self.inline_expr(len, templates, stack);
                None
            }

            MastExprKind::Memset { dest, val, len } => {
                self.inline_expr(dest, templates, stack);
                self.inline_expr(val, templates, stack);
                self.inline_expr(len, templates, stack);
                None
            }

            MastExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.inline_expr(lhs, templates, stack);
                if let Some(start) = start {
                    self.inline_expr(start, templates, stack);
                }
                if let Some(end) = end {
                    self.inline_expr(end, templates, stack);
                }
                None
            }
        };

        if let Some(kind) = replacement {
            expr.kind = kind;
        }
    }

    fn build_inlined_call_block(
        &mut self,
        callee_id: MonoId,
        template: &InlineTemplate,
        args: Vec<MastExpr>,
        span: Span,
        templates: &HashMap<MonoId, InlineTemplate>,
        stack: &mut Vec<MonoId>,
    ) -> MastBlock {
        let mut stmts =
            Vec::with_capacity(args.len() + template.params.len() + template.stmts.len());
        let mut arg_temps = Vec::with_capacity(args.len());

        for (index, arg) in args.into_iter().enumerate() {
            let temp_name = self.fresh_inline_symbol(callee_id, "arg", index);
            let temp_ty = arg.ty;
            stmts.push(MastStmt::Let {
                name: temp_name,
                ty: temp_ty,
                is_mut: false,
                init: arg,
            });
            arg_temps.push((temp_name, temp_ty));
        }

        for (param, (temp_name, temp_ty)) in template.params.iter().zip(arg_temps.iter().copied()) {
            stmts.push(MastStmt::Let {
                name: param.name,
                ty: param.ty,
                is_mut: param.is_mut,
                init: MastExpr::new(temp_ty, MastExprKind::Var(temp_name), span),
            });
        }

        stmts.extend(template.stmts.clone());
        let mut block = MastBlock {
            stmts,
            result: Some(Box::new(template.result.clone())),
            defers: vec![],
        };

        stack.push(callee_id);
        self.inline_block(&mut block, templates, stack);
        stack.pop();
        block
    }

    fn fresh_inline_symbol(&mut self, callee_id: MonoId, kind: &str, index: usize) -> SymbolId {
        let unique = self.new_mono_id();
        self.ctx.intern(&format!(
            "__inline_{}_{}_{}_{}",
            kind, callee_id.0, index, unique.0
        ))
    }
}

fn split_tail_control(body: &MastBlock) -> Option<(Vec<MastStmt>, MastExpr)> {
    split_tail_return(body)
        .or_else(|| split_guard_if_tail_return(body))
        .or_else(|| split_terminal_if_return(body))
}

fn split_tail_return(body: &MastBlock) -> Option<(Vec<MastStmt>, MastExpr)> {
    let (last, prefix) = body.stmts.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }

    let MastStmt::Expr(expr) = last else {
        return None;
    };
    let MastExprKind::Return(Some(value)) = &expr.kind else {
        return None;
    };
    if expr_contains_return(value) {
        return None;
    }

    Some((prefix.to_vec(), (**value).clone()))
}

fn split_guard_if_tail_return(body: &MastBlock) -> Option<(Vec<MastStmt>, MastExpr)> {
    let (tail_stmt, head) = body.stmts.split_last()?;
    let (guard_stmt, prefix) = head.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }

    let tail_value = return_stmt_value(tail_stmt)?;
    let MastStmt::Expr(guard_expr) = guard_stmt else {
        return None;
    };
    let MastExprKind::If {
        cond,
        then_branch,
        else_branch,
    } = &guard_expr.kind
    else {
        return None;
    };
    if else_branch.is_some() {
        return None;
    }

    let then_branch = normalize_returning_block(then_branch)?;
    let result_ty = tail_value.ty;
    let else_branch = Some(return_value_block(tail_value, guard_expr.span));
    let result = MastExpr::new(
        result_ty,
        MastExprKind::If {
            cond: cond.clone(),
            then_branch,
            else_branch,
        },
        guard_expr.span,
    );
    Some((prefix.to_vec(), result))
}

fn split_terminal_if_return(body: &MastBlock) -> Option<(Vec<MastStmt>, MastExpr)> {
    let (last, prefix) = body.stmts.split_last()?;
    if prefix.iter().any(stmt_contains_return) {
        return None;
    }

    let MastStmt::Expr(if_expr) = last else {
        return None;
    };
    let MastExprKind::If {
        cond,
        then_branch,
        else_branch,
    } = &if_expr.kind
    else {
        return None;
    };

    let then_branch = normalize_returning_block(then_branch)?;
    let result_ty = then_branch
        .result
        .as_ref()
        .map(|result| result.ty)
        .unwrap_or(TypeId::VOID);
    let else_branch = Some(normalize_returning_block(else_branch.as_ref()?)?);
    let result = MastExpr::new(
        result_ty,
        MastExprKind::If {
            cond: cond.clone(),
            then_branch,
            else_branch,
        },
        if_expr.span,
    );
    Some((prefix.to_vec(), result))
}

fn normalize_returning_block(block: &MastBlock) -> Option<MastBlock> {
    if !block.defers.is_empty() {
        return None;
    }
    let (stmts, result) = split_tail_control(block)?;
    Some(MastBlock {
        stmts,
        result: Some(Box::new(result)),
        defers: vec![],
    })
}

fn return_stmt_value(stmt: &MastStmt) -> Option<MastExpr> {
    let MastStmt::Expr(expr) = stmt else {
        return None;
    };
    let MastExprKind::Return(Some(value)) = &expr.kind else {
        return None;
    };
    if expr_contains_return(value) {
        return None;
    }
    Some((**value).clone())
}

fn return_value_block(value: MastExpr, _span: Span) -> MastBlock {
    MastBlock {
        stmts: vec![],
        result: Some(Box::new(value)),
        defers: vec![],
    }
}

fn stmt_contains_return(stmt: &MastStmt) -> bool {
    match stmt {
        MastStmt::Let { init, .. } => expr_contains_return(init),
        MastStmt::Expr(expr) => expr_contains_return(expr),
    }
}

fn block_contains_return(block: &MastBlock) -> bool {
    block.stmts.iter().any(stmt_contains_return)
        || block
            .result
            .as_ref()
            .is_some_and(|result| expr_contains_return(result))
        || block.defers.iter().any(expr_contains_return)
}

fn expr_contains_return(expr: &MastExpr) -> bool {
    match &expr.kind {
        MastExprKind::Return(_) => true,

        MastExprKind::Undef
        | MastExprKind::Unreachable
        | MastExprKind::Trap
        | MastExprKind::Breakpoint
        | MastExprKind::Integer(_)
        | MastExprKind::Float(_)
        | MastExprKind::Bool(_)
        | MastExprKind::StringLiteral(_)
        | MastExprKind::Var(_)
        | MastExprKind::GlobalRef(_)
        | MastExprKind::FuncRef(_)
        | MastExprKind::Break
        | MastExprKind::Continue
        | MastExprKind::Fence { .. } => false,

        MastExprKind::AddressOf(inner)
        | MastExprKind::Deref(inner)
        | MastExprKind::ExtractFatPtrData(inner)
        | MastExprKind::ExtractFatPtrMeta(inner)
        | MastExprKind::Unary { operand: inner, .. }
        | MastExprKind::Cast { operand: inner, .. }
        | MastExprKind::BitIntrinsic { operand: inner, .. }
        | MastExprKind::SimdUnaryIntrinsic { operand: inner, .. }
        | MastExprKind::SimdReduce { operand: inner, .. }
        | MastExprKind::SimdAny { operand: inner, .. }
        | MastExprKind::SimdAll { operand: inner, .. }
        | MastExprKind::SimdBitmask { operand: inner, .. }
        | MastExprKind::SimdSplat { value: inner, .. }
        | MastExprKind::SimdCast { value: inner, .. }
        | MastExprKind::SimdBitcast { value: inner, .. } => expr_contains_return(inner),

        MastExprKind::StructInit { fields, .. } | MastExprKind::ArrayInit(fields) => {
            fields.iter().any(expr_contains_return)
        }

        MastExprKind::UnionInit { value, .. } => expr_contains_return(value),
        MastExprKind::FieldAccess { lhs, .. } => expr_contains_return(lhs),
        MastExprKind::IndexAccess { lhs, index } => {
            expr_contains_return(lhs) || expr_contains_return(index)
        }

        MastExprKind::Call { callee, args } => {
            expr_contains_return(callee) || args.iter().any(expr_contains_return)
        }

        MastExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_contains_return(cond)
                || block_contains_return(then_branch)
                || else_branch.as_ref().is_some_and(block_contains_return)
        }

        MastExprKind::Loop { body, latch } => {
            block_contains_return(body) || latch.as_ref().is_some_and(block_contains_return)
        }

        MastExprKind::Switch {
            target,
            cases,
            default_case,
        } => {
            expr_contains_return(target)
                || cases.iter().any(|case| block_contains_return(&case.body))
                || default_case.as_ref().is_some_and(block_contains_return)
        }

        MastExprKind::Binary { lhs, rhs, .. }
        | MastExprKind::Assign { lhs, rhs, .. }
        | MastExprKind::SimdBinaryIntrinsic { lhs, rhs, .. } => {
            expr_contains_return(lhs) || expr_contains_return(rhs)
        }

        MastExprKind::ConstructFatPointer { data_ptr, meta } => {
            expr_contains_return(data_ptr) || expr_contains_return(meta)
        }

        MastExprKind::Block(block) => block_contains_return(block),
        MastExprKind::DataInit { payload, .. } => expr_contains_return(payload),

        MastExprKind::Asm(asm) => {
            asm.input_args.iter().any(expr_contains_return)
                || asm.output_ptrs.iter().any(expr_contains_return)
        }

        MastExprKind::SimdSelect {
            mask,
            on_true,
            on_false,
        } => {
            expr_contains_return(mask)
                || expr_contains_return(on_true)
                || expr_contains_return(on_false)
        }

        MastExprKind::SimdShuffle { lhs, rhs, .. } => {
            expr_contains_return(lhs) || expr_contains_return(rhs)
        }

        MastExprKind::SimdInsertHalf { base, half, .. } => {
            expr_contains_return(base) || expr_contains_return(half)
        }

        MastExprKind::SimdLoad { ptr, .. } => expr_contains_return(ptr),
        MastExprKind::SimdStore { ptr, value, .. } => {
            expr_contains_return(ptr) || expr_contains_return(value)
        }

        MastExprKind::SimdMaskedLoad {
            ptr, mask, or_else, ..
        } => {
            expr_contains_return(ptr) || expr_contains_return(mask) || expr_contains_return(or_else)
        }

        MastExprKind::SimdMaskedStore {
            ptr, mask, value, ..
        } => expr_contains_return(ptr) || expr_contains_return(mask) || expr_contains_return(value),

        MastExprKind::SimdGather { ptr, indices } => {
            expr_contains_return(ptr) || expr_contains_return(indices)
        }

        MastExprKind::SimdScatter {
            ptr,
            indices,
            value,
        } => {
            expr_contains_return(ptr)
                || expr_contains_return(indices)
                || expr_contains_return(value)
        }

        MastExprKind::SimdMaskedGather {
            ptr,
            indices,
            mask,
            or_else,
        } => {
            expr_contains_return(ptr)
                || expr_contains_return(indices)
                || expr_contains_return(mask)
                || expr_contains_return(or_else)
        }

        MastExprKind::SimdMaskedScatter {
            ptr,
            indices,
            mask,
            value,
        } => {
            expr_contains_return(ptr)
                || expr_contains_return(indices)
                || expr_contains_return(mask)
                || expr_contains_return(value)
        }

        MastExprKind::AtomicLoad { ptr, .. } => expr_contains_return(ptr),
        MastExprKind::AtomicStore { ptr, value, .. } => {
            expr_contains_return(ptr) || expr_contains_return(value)
        }

        MastExprKind::AtomicCas {
            ptr,
            expected,
            desired,
            ..
        } => {
            expr_contains_return(ptr)
                || expr_contains_return(expected)
                || expr_contains_return(desired)
        }

        MastExprKind::AtomicRmw { ptr, value, .. } => {
            expr_contains_return(ptr) || expr_contains_return(value)
        }

        MastExprKind::Memcpy { dest, src, len } | MastExprKind::Memmove { dest, src, len } => {
            expr_contains_return(dest) || expr_contains_return(src) || expr_contains_return(len)
        }

        MastExprKind::Memset { dest, val, len } => {
            expr_contains_return(dest) || expr_contains_return(val) || expr_contains_return(len)
        }

        MastExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            expr_contains_return(lhs)
                || start
                    .as_ref()
                    .is_some_and(|start| expr_contains_return(start))
                || end.as_ref().is_some_and(|end| expr_contains_return(end))
        }
    }
}
