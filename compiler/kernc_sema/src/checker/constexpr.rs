use crate::LayoutEngine;
use crate::SemaContext;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::scope::ScopeId;
use crate::scope::SymbolKind;
use crate::ty::{PrimitiveType, TypeId, TypeKind};
use kernc_ast::{self as ast, BinaryOperator, Expr, ExprKind, StmtKind, UnaryOperator};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    String(String),
    Array(Vec<ConstValue>),
    Struct(HashMap<SymbolId, ConstValue>),
    Enum {
        tag: i128,
        payload: Option<Box<ConstValue>>,
    },
    Void,
    Undef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConstEvalError;

type ConstEvalResult<T> = Result<T, ConstEvalError>;

pub struct ConstEvaluator<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    const_scopes: Vec<ScopeId>,
    local_scopes: Vec<HashMap<SymbolId, ConstValue>>,
    local_type_scopes: Vec<HashMap<SymbolId, TypeId>>,
    type_substs: Vec<HashMap<SymbolId, TypeId>>,
    return_value: Option<ConstValue>,
    function_depth: usize,
}

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        let mut const_scopes = Vec::new();
        if let Some(scope_id) = ctx.scopes.current_scope_id() {
            const_scopes.push(scope_id);
        }

        Self {
            ctx,
            const_scopes,
            local_scopes: Vec::new(),
            local_type_scopes: Vec::new(),
            type_substs: Vec::new(),
            return_value: None,
            function_depth: 0,
        }
    }

    /// 提取数组长度等所需的无符号整数
    pub fn eval_usize(&mut self, expr: &Expr) -> ConstEvalResult<u64> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => {
                if val < 0 {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "constant expression cannot evaluate to a negative number here",
                        )
                        .with_hint("array lengths and similar contexts require positive integers")
                        .emit();
                    Err(ConstEvalError)
                } else {
                    Ok(val as u64)
                }
            }
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(ConstEvalError)
            }
            Err(_) => Err(ConstEvalError),
        }
    }

    /// 提取普通的有符号整数常量
    pub fn eval_math(&mut self, expr: &Expr) -> ConstEvalResult<i128> {
        match self.eval_inner(expr, 0) {
            Ok(ConstValue::Int(val)) => Ok(val),
            Ok(_) => {
                self.ctx
                    .struct_error(expr.span, "expected an integer constant")
                    .emit();
                Err(ConstEvalError)
            }
            Err(_) => Err(ConstEvalError),
        }
    }

    fn global_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        self.ctx.defs.iter().find_map(|def| {
            let Def::Module(module) = def else {
                return None;
            };

            if module.items.contains(&def_id) {
                Some(module.scope_id)
            } else {
                None
            }
        })
    }

    fn def_owner_scope(&self, def_id: DefId) -> Option<ScopeId> {
        match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => {
                let mut current_parent = f.parent;
                while let Some(parent_id) = current_parent {
                    match &self.ctx.defs[parent_id.0 as usize] {
                        Def::Module(module) => return Some(module.scope_id),
                        Def::Impl(impl_def) => current_parent = impl_def.parent_module,
                        _ => return None,
                    }
                }
                None
            }
            Def::Global(_) => self.global_owner_scope(def_id),
            _ => None,
        }
    }

    fn resolved_type(&mut self, ty: TypeId) -> TypeId {
        let mut resolved = ty;
        for subst_map in &self.type_substs {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
            resolved = subst.substitute(resolved);
        }
        self.ctx.type_registry.normalize(resolved)
    }

    fn node_type(&mut self, node_id: NodeId) -> TypeId {
        let ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);
        self.resolved_type(ty)
    }

    fn expr_type(&mut self, expr: &Expr) -> TypeId {
        let ty = self.node_type(expr.id);
        if ty != TypeId::ERROR {
            return ty;
        }

        match &expr.kind {
            ExprKind::Identifier(name) => self
                .lookup_local_type(*name)
                .map(|ty| self.resolved_type(ty))
                .or_else(|| {
                    self.resolve_symbol_info(*name)
                        .map(|info| self.resolved_type(info.type_id))
                })
                .unwrap_or(TypeId::ERROR),
            ExprKind::SelfValue => {
                let self_name = self.ctx.intern("self");
                self.lookup_local_type(self_name)
                    .map(|ty| self.resolved_type(ty))
                    .unwrap_or(TypeId::ERROR)
            }
            ExprKind::Call { callee, .. } => self
                .resolve_callable(callee)
                .and_then(|(def_id, generic_args)| self.callable_return_type(def_id, &generic_args))
                .unwrap_or(TypeId::ERROR),
            ExprKind::DataInit { type_node, .. } => type_node
                .as_deref()
                .and_then(|ty| self.ctx.node_types.get(&ty.id).copied())
                .map(|ty| self.resolved_type(ty))
                .unwrap_or(TypeId::ERROR),
            _ => TypeId::ERROR,
        }
    }

    fn callable_return_type(&mut self, def_id: DefId, generic_args: &[TypeId]) -> Option<TypeId> {
        let Def::Function(func) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
            return None;
        };
        let sig = func.resolved_sig?;

        if func.generics.is_empty() {
            return match self.ctx.type_registry.get(sig).clone() {
                TypeKind::Function { ret, .. } => Some(ret),
                _ => None,
            };
        }

        if func.generics.len() != generic_args.len() {
            return None;
        }

        let mut generic_map = HashMap::new();
        for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
            generic_map.insert(param.name, *arg);
        }
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &generic_map);
        let sig = subst.substitute(sig);

        match self.ctx.type_registry.get(sig).clone() {
            TypeKind::Function { ret, .. } => Some(ret),
            _ => None,
        }
    }

    fn push_local_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
        self.local_type_scopes.push(HashMap::new());
    }

    fn pop_local_scope(&mut self) {
        let _ = self.local_scopes.pop();
        let _ = self.local_type_scopes.pop();
    }

    fn define_local(&mut self, name: SymbolId, value: ConstValue) {
        if self.local_scopes.is_empty() {
            self.push_local_scope();
        }
        if let Some(scope) = self.local_scopes.last_mut() {
            scope.insert(name, value);
        }
    }

    fn define_local_type(&mut self, name: SymbolId, ty: TypeId) {
        if self.local_type_scopes.is_empty() {
            self.push_local_scope();
        }
        if let Some(scope) = self.local_type_scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    fn lookup_local(&self, name: SymbolId) -> Option<ConstValue> {
        self.local_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).cloned())
    }

    fn lookup_local_type(&self, name: SymbolId) -> Option<TypeId> {
        self.local_type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }

    fn resolve_symbol_info(&self, name: SymbolId) -> Option<crate::scope::SymbolInfo> {
        if let Some(&scope_id) = self.const_scopes.last() {
            self.ctx.scopes.resolve_from(scope_id, name).cloned()
        } else {
            self.ctx.scopes.resolve(name).cloned()
        }
    }

    fn module_scope_from_expr(&mut self, expr: &Expr) -> Option<ScopeId> {
        let expr_ty = self.node_type(expr.id);
        if let TypeKind::Module(def_id) = self.ctx.type_registry.get(expr_ty).clone()
            && let Def::Module(module) = &self.ctx.defs[def_id.0 as usize]
        {
            return Some(module.scope_id);
        }

        match &expr.kind {
            ExprKind::Identifier(name) => {
                let info = self.resolve_symbol_info(*name)?;
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let Def::Module(module) = &self.ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                Some(module.scope_id)
            }
            ExprKind::FieldAccess { lhs, field } => {
                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self.ctx.scopes.resolve_in(mod_scope, *field)?.clone();
                if info.kind != SymbolKind::Module {
                    return None;
                }
                let def_id = info.def_id?;
                let Def::Module(module) = &self.ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                Some(module.scope_id)
            }
            _ => None,
        }
    }

    fn resolve_callable(&mut self, callee: &Expr) -> Option<(DefId, Vec<TypeId>)> {
        let callee_ty = self.node_type(callee.id);
        if let TypeKind::FnDef(def_id, args) = self.ctx.type_registry.get(callee_ty).clone() {
            return Some((def_id, args));
        }

        match &callee.kind {
            ExprKind::Identifier(name) => {
                let info = self.resolve_symbol_info(*name)?;
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            ExprKind::GenericInstantiation { target, types } => {
                let (def_id, _) = self.resolve_callable(target)?;
                let generic_args = types
                    .iter()
                    .map(|ty| {
                        let ty = self
                            .ctx
                            .node_types
                            .get(&ty.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        self.resolved_type(ty)
                    })
                    .collect();
                Some((def_id, generic_args))
            }
            ExprKind::FieldAccess { lhs, field } => {
                let mod_scope = self.module_scope_from_expr(lhs)?;
                let info = self.ctx.scopes.resolve_in(mod_scope, *field)?.clone();
                if info.kind == SymbolKind::Function {
                    Some((info.def_id?, Vec::new()))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn eval_const_def(&mut self, def_id: DefId, depth: usize) -> ConstEvalResult<ConstValue> {
        let const_expr = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
            g.value.clone()
        } else {
            return Err(ConstEvalError);
        };

        let prev_scope = self.ctx.scopes.current_scope_id();
        let owner_scope = self.def_owner_scope(def_id);
        if let Some(owner_scope) = owner_scope {
            self.ctx.scopes.set_current_scope(owner_scope);
            self.const_scopes.push(owner_scope);
        }

        let result = self.eval_inner(&const_expr, depth + 1);

        if owner_scope.is_some() {
            let _ = self.const_scopes.pop();
        }
        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }

        result
    }

    fn eval_data_init(
        &mut self,
        expr: &Expr,
        literal: &ast::DataLiteralKind,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        let norm_target = self.expr_type(expr);

        match self.ctx.type_registry.get(norm_target).clone() {
            TypeKind::Enum(def_id, _) => {
                self.eval_named_enum_data_init(def_id, literal, depth, expr.span)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                self.eval_anon_enum_data_init(&enum_def, literal, depth, expr.span)
            }
            _ => match literal {
                ast::DataLiteralKind::Scalar(inner) => self.eval_inner(inner, depth + 1),
                ast::DataLiteralKind::Array(elems) => {
                    let mut arr = Vec::new();
                    for e in elems {
                        arr.push(self.eval_inner(e, depth + 1)?);
                    }
                    Ok(ConstValue::Array(arr))
                }
                ast::DataLiteralKind::Struct(fields) => {
                    let mut map = HashMap::new();
                    for f in fields {
                        map.insert(f.name, self.eval_inner(&f.value, depth + 1)?);
                    }
                    Ok(ConstValue::Struct(map))
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    let val = self.eval_inner(value, depth + 1)?;
                    let cnt = self.eval_usize(count)?;
                    Ok(ConstValue::Array(vec![val; cnt as usize]))
                }
            },
        }
    }

    fn eval_named_enum_data_init(
        &mut self,
        def_id: crate::def::DefId,
        literal: &ast::DataLiteralKind,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize).cloned() else {
            return Err(ConstEvalError);
        };

        match literal {
            ast::DataLiteralKind::Scalar(inner) => {
                let Some(variant_name) = self.enum_ctor_variant_name(inner, span) else {
                    return Err(ConstEvalError);
                };
                let Some((variant, tag)) =
                    self.named_enum_variant_and_tag(&enum_def, variant_name, depth, span)
                else {
                    return Err(ConstEvalError);
                };
                if variant.payload_type.is_some() {
                    self.ctx
                        .struct_error(
                            inner.span,
                            format!(
                                "variant `{}` requires a payload in constant initialization",
                                self.ctx.resolve(variant_name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                if enum_def.variants.iter().all(|v| v.payload_type.is_none()) {
                    Ok(ConstValue::Int(tag))
                } else {
                    Ok(ConstValue::Enum { tag, payload: None })
                }
            }
            ast::DataLiteralKind::Struct(fields) => {
                if fields.len() != 1 {
                    self.ctx
                        .struct_error(
                            span,
                            "enum constant initialization must specify exactly one variant",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }
                let init = &fields[0];
                let Some((variant, tag)) =
                    self.named_enum_variant_and_tag(&enum_def, init.name, depth, init.span)
                else {
                    return Err(ConstEvalError);
                };
                let Some(_) = variant.payload_type else {
                    self.ctx
                        .struct_error(
                            init.span,
                            format!(
                                "variant `{}` does not take a payload in constant initialization",
                                self.ctx.resolve(init.name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                };
                let payload = self.eval_inner(&init.value, depth + 1)?;
                Ok(ConstValue::Enum {
                    tag,
                    payload: Some(Box::new(payload)),
                })
            }
            _ => {
                self.ctx
                    .struct_error(span, "invalid enum constant initializer")
                    .with_hint("use `Type.{ Variant }` or `Type.{ Variant: payload }`")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_anon_enum_data_init(
        &mut self,
        enum_def: &crate::ty::AnonymousEnum,
        literal: &ast::DataLiteralKind,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match literal {
            ast::DataLiteralKind::Scalar(inner) => {
                let Some(variant_name) = self.enum_ctor_variant_name(inner, span) else {
                    return Err(ConstEvalError);
                };
                let Some((variant, tag)) =
                    self.anon_enum_variant_and_tag(enum_def, variant_name, span)
                else {
                    return Err(ConstEvalError);
                };
                if variant.payload_ty.is_some() {
                    self.ctx
                        .struct_error(
                            inner.span,
                            format!(
                                "variant `{}` requires a payload in constant initialization",
                                self.ctx.resolve(variant_name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                }

                if enum_def.variants.iter().all(|v| v.payload_ty.is_none()) {
                    Ok(ConstValue::Int(tag))
                } else {
                    Ok(ConstValue::Enum { tag, payload: None })
                }
            }
            ast::DataLiteralKind::Struct(fields) => {
                if fields.len() != 1 {
                    self.ctx
                        .struct_error(
                            span,
                            "enum constant initialization must specify exactly one variant",
                        )
                        .emit();
                    return Err(ConstEvalError);
                }
                let init = &fields[0];
                let Some((variant, tag)) =
                    self.anon_enum_variant_and_tag(enum_def, init.name, init.span)
                else {
                    return Err(ConstEvalError);
                };
                let Some(_) = variant.payload_ty else {
                    self.ctx
                        .struct_error(
                            init.span,
                            format!(
                                "variant `{}` does not take a payload in constant initialization",
                                self.ctx.resolve(init.name)
                            ),
                        )
                        .emit();
                    return Err(ConstEvalError);
                };
                let payload = self.eval_inner(&init.value, depth + 1)?;
                Ok(ConstValue::Enum {
                    tag,
                    payload: Some(Box::new(payload)),
                })
            }
            _ => {
                self.ctx
                    .struct_error(span, "invalid enum constant initializer")
                    .with_hint("use `Type.{ Variant }` or `Type.{ Variant: payload }`")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn enum_ctor_variant_name(&mut self, inner: &Expr, span: Span) -> Option<SymbolId> {
        match inner.kind {
            ExprKind::Identifier(name) | ExprKind::EnumLiteral(name) => Some(name),
            _ => {
                self.ctx
                    .struct_error(span, "enum constant initialization expects a variant name")
                    .with_hint("write `Type.{ Variant }` for payload-less variants")
                    .emit();
                None
            }
        }
    }

    fn named_enum_variant_and_tag(
        &mut self,
        enum_def: &crate::def::EnumDef,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> Option<(ast::EnumVariant, i128)> {
        let mut current_val: i128 = 0;
        for variant in &enum_def.variants {
            if let Some(value_expr) = &variant.value
                && let Ok(ConstValue::Int(val)) = self.eval_inner(value_expr, depth + 1)
            {
                current_val = val;
            }
            if variant.name == variant_name {
                return Some((variant.clone(), current_val));
            }
            current_val += 1;
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "variant `.{}` not found in enum constant initialization",
                    self.ctx.resolve(variant_name)
                ),
            )
            .emit();
        None
    }

    fn anon_enum_variant_and_tag(
        &mut self,
        enum_def: &crate::ty::AnonymousEnum,
        variant_name: SymbolId,
        span: Span,
    ) -> Option<(crate::ty::AnonymousVariant, i128)> {
        let mut current_val: i128 = 0;
        for variant in &enum_def.variants {
            if let Some(explicit_value) = variant.explicit_value {
                current_val = explicit_value;
            }
            if variant.name == variant_name {
                return Some((variant.clone(), current_val));
            }
            current_val += 1;
        }

        self.ctx
            .struct_error(
                span,
                format!(
                    "variant `.{}` not found in enum constant initialization",
                    self.ctx.resolve(variant_name)
                ),
            )
            .emit();
        None
    }

    /// 核心递归求值引擎
    pub fn eval_inner(&mut self, expr: &Expr, depth: usize) -> ConstEvalResult<ConstValue> {
        if depth > 100 {
            self.ctx
                .struct_error(
                    expr.span,
                    "constant evaluation exceeded maximum recursion depth",
                )
                .with_hint("check for circular references in your `const` declarations")
                .emit();
            return Err(ConstEvalError);
        }

        let eval_result = match &expr.kind {
            // === 1. 基础字面量 ===
            ExprKind::Integer(val) => Ok(ConstValue::Int(*val as i128)),
            ExprKind::Float(val) => Ok(ConstValue::Float(*val)),
            ExprKind::Bool(b) => Ok(ConstValue::Bool(*b)),
            ExprKind::Char(c) => Ok(ConstValue::Int(*c as u32 as i128)),
            ExprKind::ByteChar(c) => Ok(ConstValue::Int(*c as i128)),
            ExprKind::String(s) => Ok(ConstValue::String(s.clone())),
            ExprKind::Undef => Ok(ConstValue::Undef),

            // === 2. 算术与逻辑运算 ===
            ExprKind::Binary { lhs, op, rhs } => self.eval_binary(lhs, *op, rhs, depth, expr.span),
            ExprKind::Unary { op, operand } => {
                // 提前折叠负数字面量
                // 拦截 `-` 后面紧跟数字的情况，跳过对内部正数 (如 128) 的独立求值和越界检查，直接返回整体负数。
                if *op == UnaryOperator::Negate {
                    if let ExprKind::Integer(val) = &operand.kind {
                        Ok(ConstValue::Int(-(*val as i128)))
                    } else if let ExprKind::Float(val) = &operand.kind {
                        Ok(ConstValue::Float(-*val))
                    } else {
                        self.eval_unary(*op, operand, depth, expr.span)
                    }
                } else {
                    self.eval_unary(*op, operand, depth, expr.span)
                }
            }

            ExprKind::As { lhs, .. } => {
                let val = self.eval_inner(lhs, depth + 1)?;
                let target_ty = self.node_type(expr.id);

                if let ConstValue::Int(v) = val {
                    let mut layout = LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(target_ty) * 8;
                    let mask = if bit_width >= 128 {
                        u128::MAX
                    } else {
                        (1 << bit_width) - 1
                    };
                    let u_val = (v as u128) & mask;

                    Ok(ConstValue::Int(u_val as i128))
                } else {
                    self.ctx
                        .struct_error(
                            expr.span,
                            "only integer casts are supported in const context currently",
                        )
                        .emit();
                    Err(ConstEvalError)
                }
            }

            // === 3. 查表代入全局 Const 变量 ===
            ExprKind::Identifier(name) => self.eval_identifier(*name, depth, expr.span),
            ExprKind::SelfValue => {
                let self_name = self.ctx.intern("self");
                self.eval_identifier(self_name, depth, expr.span)
            }

            // === 4. 常量函数调用 ===
            ExprKind::Call { callee, args } => self.eval_call(callee, args, depth, expr.span),

            // === 5. 枚举字面量求值 ===
            ExprKind::EnumLiteral(variant_name) => {
                self.eval_enum_literal(expr.id, *variant_name, depth, expr.span)
            }

            // === 6. 数据初始化 (支持嵌套 Array 和 Struct) ===
            ExprKind::DataInit { literal, .. } => self.eval_data_init(expr, literal, depth),

            // === 7. 局部控制流 ===
            ExprKind::Let { pattern, init } => {
                let value = self.eval_inner(init, depth + 1)?;
                let init_ty = self.expr_type(init);
                self.define_local(pattern.name, value);
                self.define_local_type(pattern.name, init_ty);
                Ok(ConstValue::Void)
            }
            ExprKind::Block { stmts, result } => self.eval_block(stmts, result.as_deref(), depth),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => self.eval_if(cond, then_branch, else_branch.as_deref(), depth, expr.span),
            ExprKind::Match { target, arms } => self.eval_match(target, arms, depth, expr.span),
            ExprKind::Return(value) => self.eval_return(value.as_deref(), depth, expr.span),

            // === 7. 常量聚合访问 (提取结构体字段和数组索引) ===
            ExprKind::FieldAccess { lhs, field } => {
                let norm_lhs = self.node_type(lhs.id);

                if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(norm_lhs).clone() {
                    let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        m.scope_id
                    } else {
                        self.ctx.emit_ice(
                            expr.span,
                            format!(
                                "Kern ICE (ConstEval): Expected module definition for DefId {} during constant field access.",
                                mod_def_id.0
                            ),
                        );
                        return Err(ConstEvalError);
                    };
                    if let Some(info) = self.ctx.scopes.resolve_in(mod_scope, *field).cloned() {
                        if info.kind == SymbolKind::Const {
                            if let Some(def_id) = info.def_id {
                                self.eval_const_def(def_id, depth)
                            } else {
                                Err(ConstEvalError)
                            }
                        } else {
                            let field_str = self.ctx.resolve(*field);
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!(
                                        "`{}` is a {}, not a compile-time constant",
                                        field_str,
                                        self.kind_to_string(info.kind)
                                    ),
                                )
                                .emit();
                            Err(ConstEvalError)
                        }
                    } else {
                        let field_str = self.ctx.resolve(*field);
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!("constant `{}` not found in module", field_str),
                            )
                            .emit();
                        Err(ConstEvalError)
                    }
                } else {
                    let base = self.eval_inner(lhs, depth + 1)?;
                    if let ConstValue::Struct(map) = base {
                        if let Some(val) = map.get(field) {
                            Ok(val.clone())
                        } else {
                            let field_str = self.ctx.resolve(*field);
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    format!("field `{}` not found in constant struct", field_str),
                                )
                                .emit();
                            Err(ConstEvalError)
                        }
                    } else {
                        self.ctx
                            .struct_error(
                                expr.span,
                                "attempted field access on a non-struct constant",
                            )
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }

            ExprKind::IndexAccess { lhs, index, .. } => {
                let base = self.eval_inner(lhs, depth + 1)?;
                let idx = self.eval_usize(index)?;
                if let ConstValue::Array(arr) = base {
                    if idx < arr.len() as u64 {
                        Ok(arr[idx as usize].clone())
                    } else {
                        self.ctx
                            .struct_error(expr.span, "constant array index out of bounds")
                            .emit();
                        Err(ConstEvalError)
                    }
                } else {
                    self.ctx
                        .struct_error(expr.span, "attempted indexing into a non-array constant")
                        .emit();
                    Err(ConstEvalError)
                }
            }

            ExprKind::GenericInstantiation { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "generic instantiation cannot be evaluated directly as a value",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            ExprKind::Static { .. }
            | ExprKind::For { .. }
            | ExprKind::Defer { .. }
            | ExprKind::Break
            | ExprKind::Continue
            | ExprKind::Assign { .. }
            | ExprKind::Closure { .. } => {
                self.ctx
                    .struct_error(
                        expr.span,
                        "this construct is not supported in constant evaluation",
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => {
                self.ctx
                    .struct_error(expr.span, "expected a valid constant expression")
                    .emit();
                Err(ConstEvalError)
            }
        };

        // 获取刚刚求出的结果
        let mut val = eval_result?;

        // 越界与符号断言
        if let ConstValue::Int(mut v) = val {
            let ty = self.node_type(expr.id);
            let norm = self.ctx.type_registry.normalize(ty);

            if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm).clone() {
                let is_signed = matches!(
                    p,
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                );
                let is_unsigned = matches!(
                    p,
                    PrimitiveType::U8
                        | PrimitiveType::U16
                        | PrimitiveType::U32
                        | PrimitiveType::U64
                        | PrimitiveType::U128
                        | PrimitiveType::USize
                );

                // 洗白 i128 算出来的伪负数（比如 !0 -> -1）
                if is_unsigned {
                    let mut layout = crate::LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(norm) * 8;
                    if bit_width < 128 {
                        let mask = (1i128 << bit_width) - 1;
                        v &= mask; // 此时 -1 会被截断为的 0xFF...FF
                    }
                }

                // 1. 无符号类型不接受负数(经过洗白和 Unary 拦截后，走到这里的都是非法越界的硬编码值)
                if is_unsigned && v < 0 {
                    self.ctx.struct_error(expr.span, format!("cannot assign a negative value ({}) to an unsigned type `{}`", v, self.ctx.ty_to_string(ty)))
                        .with_hint("if you need a bit-pattern of all 1s, use explicit bitwise negation (e.g., `~0`) or `as` cast")
                        .emit();
                    return Err(ConstEvalError);
                }

                // 2. 检查数值是否溢出相应的位宽容量
                if (is_signed || is_unsigned)
                    && p != PrimitiveType::I128
                    && p != PrimitiveType::U128
                {
                    let mut layout = crate::LayoutEngine::new(self.ctx);
                    let bit_width = layout.compute_type_size(norm) * 8;

                    let (min, max) = if is_signed {
                        let max = (1i128 << (bit_width - 1)) - 1;
                        let min = -(1i128 << (bit_width - 1));
                        (min, max)
                    } else {
                        let max = ((1u128 << bit_width) - 1) as i128;
                        (0, max)
                    };

                    if v < min || v > max {
                        self.ctx
                            .struct_error(
                                expr.span,
                                format!(
                                    "integer literal {} is out of bounds for type `{}`",
                                    v,
                                    self.ctx.ty_to_string(ty)
                                ),
                            )
                            .with_hint(format!("the valid range is {} to {}", min, max))
                            .emit();
                        return Err(ConstEvalError);
                    }
                }
            }
            val = ConstValue::Int(v);
        }

        Ok(val)
    }

    // ==========================================
    //            Const Eval Helpers
    // ==========================================

    fn eval_binary(
        &mut self,
        lhs: &Expr,
        op: BinaryOperator,
        rhs: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let left = self.eval_inner(lhs, depth + 1)?;
        let right = self.eval_inner(rhs, depth + 1)?;

        match (left, right) {
            (ConstValue::Int(l), ConstValue::Int(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Int(l.wrapping_add(r))),
                    Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
                    Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
                    Divide => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "division by zero in constant expression")
                                .emit();
                            Err(ConstEvalError)
                        } else {
                            Ok(ConstValue::Int(l / r))
                        }
                    }
                    Modulo => {
                        if r == 0 {
                            self.ctx
                                .struct_error(span, "modulo by zero in constant expression")
                                .emit();
                            Err(ConstEvalError)
                        } else {
                            Ok(ConstValue::Int(l % r))
                        }
                    }
                    ShiftLeft => Ok(ConstValue::Int(l << r)),
                    ShiftRight => Ok(ConstValue::Int(l >> r)),
                    BitwiseAnd => Ok(ConstValue::Int(l & r)),
                    BitwiseOr => Ok(ConstValue::Int(l | r)),
                    BitwiseXor => Ok(ConstValue::Int(l ^ r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant integers")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            (ConstValue::Float(l), ConstValue::Float(r)) => {
                use BinaryOperator::*;
                match op {
                    Add => Ok(ConstValue::Float(l + r)),
                    Subtract => Ok(ConstValue::Float(l - r)),
                    Multiply => Ok(ConstValue::Float(l * r)),
                    Divide => Ok(ConstValue::Float(l / r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    LessThan => Ok(ConstValue::Bool(l < r)),
                    LessOrEqual => Ok(ConstValue::Bool(l <= r)),
                    GreaterThan => Ok(ConstValue::Bool(l > r)),
                    GreaterOrEqual => Ok(ConstValue::Bool(l >= r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant floats")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            (ConstValue::Bool(l), ConstValue::Bool(r)) => {
                use BinaryOperator::*;
                match op {
                    LogicalAnd => Ok(ConstValue::Bool(l && r)),
                    LogicalOr => Ok(ConstValue::Bool(l || r)),
                    Equal => Ok(ConstValue::Bool(l == r)),
                    NotEqual => Ok(ConstValue::Bool(l != r)),
                    _ => {
                        self.ctx
                            .struct_error(span, "unsupported operator for constant booleans")
                            .emit();
                        Err(ConstEvalError)
                    }
                }
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant binary expression",
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_unary(
        &mut self,
        op: UnaryOperator,
        operand: &Expr,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let val = self.eval_inner(operand, depth + 1)?;

        let norm_ty = self.node_type(operand.id);
        let is_unsigned = if let TypeKind::Primitive(p) = self.ctx.type_registry.get(norm_ty) {
            matches!(
                p,
                PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            )
        } else {
            false
        };

        match (op, val) {
            (UnaryOperator::Negate, ConstValue::Int(v)) => {
                if is_unsigned {
                    self.ctx.struct_error(span, "cannot apply unary minus `-` to an unsigned type")
                        .with_hint("unsigned types cannot be negative. use `~` or bitwise operations if you intend to manipulate bits")
                        .emit();
                    return Err(ConstEvalError);
                }
                Ok(ConstValue::Int(v.wrapping_neg()))
            }
            (UnaryOperator::Negate, ConstValue::Float(v)) => Ok(ConstValue::Float(-v)),
            (UnaryOperator::BitwiseNot, ConstValue::Int(v)) => Ok(ConstValue::Int(!v)),
            (UnaryOperator::LogicalNot, ConstValue::Bool(v)) => Ok(ConstValue::Bool(!v)),
            _ => {
                self.ctx
                    .struct_error(span, "invalid unary operator for the given constant type")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_identifier(
        &mut self,
        name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(value) = self.lookup_local(name) {
            return Ok(value);
        }

        let sym_info = if let Some(&scope_id) = self.const_scopes.last() {
            self.ctx.scopes.resolve_from(scope_id, name).cloned()
        } else {
            self.ctx.scopes.resolve(name).cloned()
        };

        if let Some(info) = sym_info {
            if info.kind == SymbolKind::Const {
                if let Some(def_id) = info.def_id {
                    return self.eval_const_def(def_id, depth);
                }
            } else {
                let name_str = self.ctx.resolve(name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a {}, not a compile-time constant",
                            name_str,
                            self.kind_to_string(info.kind)
                        ),
                    )
                    .with_hint("only `const` variables can be used in constant expressions")
                    .emit();
                return Err(ConstEvalError);
            }
        }
        self.ctx
            .struct_error(span, "use of undeclared identifier in constant expression")
            .emit();
        Err(ConstEvalError)
    }

    fn eval_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        depth: usize,
    ) -> ConstEvalResult<ConstValue> {
        self.push_local_scope();

        for stmt in stmts {
            let stmt_expr = match &stmt.kind {
                StmtKind::ExprStmt(expr) | StmtKind::ExprValue(expr) => expr,
            };
            let _ = self.eval_inner(stmt_expr, depth + 1)?;
            if self.return_value.is_some() {
                self.pop_local_scope();
                return Ok(ConstValue::Void);
            }
        }

        let value = if let Some(result_expr) = result {
            self.eval_inner(result_expr, depth + 1)?
        } else {
            ConstValue::Void
        };

        self.pop_local_scope();
        Ok(value)
    }

    fn eval_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let cond_val = self.eval_inner(cond, depth + 1)?;
        match cond_val {
            ConstValue::Bool(true) => self.eval_inner(then_branch, depth + 1),
            ConstValue::Bool(false) => {
                if let Some(else_branch) = else_branch {
                    self.eval_inner(else_branch, depth + 1)
                } else {
                    Ok(ConstValue::Void)
                }
            }
            _ => {
                self.ctx
                    .struct_error(span, "if condition must evaluate to a boolean constant")
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    fn eval_match(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let target_value = self.eval_inner(target, depth + 1)?;
        let target_ty = self.expr_type(target);

        for arm in arms {
            let mut bindings = None;

            for pattern in &arm.patterns {
                if let Some(found) =
                    self.match_pattern(pattern, &target_value, target_ty, depth + 1)?
                {
                    bindings = Some(found);
                    break;
                }
            }

            let Some(bindings) = bindings else {
                continue;
            };

            self.push_local_scope();
            for (name, value) in bindings {
                self.define_local(name, value);
            }
            let body_value = self.eval_inner(&arm.body, depth + 1);
            self.pop_local_scope();
            return body_value;
        }

        self.ctx
            .struct_error(span, "match expression did not resolve to any constant arm")
            .emit();
        Err(ConstEvalError)
    }

    fn eval_return(
        &mut self,
        value: Option<&Expr>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if self.function_depth == 0 {
            self.ctx
                .struct_error(span, "`return` is only valid inside a `const fn` body")
                .emit();
            return Err(ConstEvalError);
        }

        let value = if let Some(expr) = value {
            self.eval_inner(expr, depth + 1)?
        } else {
            ConstValue::Void
        };
        self.return_value = Some(value);
        Ok(ConstValue::Void)
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some((def_id, generic_args)) = self.resolve_callable(callee) else {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let func = match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Function(func)) => func,
            _ => return Err(ConstEvalError),
        };

        if func.is_intrinsic {
            return self.eval_intrinsic_call(callee, args, depth, span);
        }

        if !func.is_const {
            self.ctx
                .struct_error(
                    span,
                    "only `const fn` can be called in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        }

        if func.is_extern {
            self.ctx
                .struct_error(
                    span,
                    "`extern const fn` is not supported in constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        }

        if !func.generics.is_empty() && generic_args.len() != func.generics.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "const function `{}` requires fully resolved generic arguments during constant evaluation",
                        self.ctx.resolve(func.name)
                    ),
                )
                .emit();
            return Err(ConstEvalError);
        }

        let mut arg_values = Vec::new();
        if let Some(receiver) = self.method_receiver(callee) {
            arg_values.push(self.eval_inner(receiver, depth + 1)?);
        }
        for arg in args {
            arg_values.push(self.eval_inner(arg, depth + 1)?);
        }

        if arg_values.len() != func.params.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "const function `{}` expects {} arguments, but {} were provided",
                        self.ctx.resolve(func.name),
                        func.params.len(),
                        arg_values.len()
                    ),
                )
                .emit();
            return Err(ConstEvalError);
        }

        let prev_scope = self.ctx.scopes.current_scope_id();
        let owner_scope = self.def_owner_scope(def_id);
        if let Some(owner_scope) = owner_scope {
            self.ctx.scopes.set_current_scope(owner_scope);
            self.const_scopes.push(owner_scope);
        }

        let mut generic_map = HashMap::new();
        for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
            generic_map.insert(param.name, *arg);
        }
        if !generic_map.is_empty() {
            self.type_substs.push(generic_map);
        }

        self.function_depth += 1;
        self.push_local_scope();
        let param_tys = match self.callable_return_and_params(def_id, &generic_args) {
            Some((params, _)) => params,
            None => vec![TypeId::ERROR; func.params.len()],
        };
        for ((param, value), param_ty) in func.params.iter().zip(arg_values.into_iter()).zip(
            param_tys
                .into_iter()
                .chain(std::iter::repeat(TypeId::ERROR)),
        ) {
            self.define_local(param.pattern.name, value);
            self.define_local_type(param.pattern.name, param_ty);
        }

        let saved_return = self.return_value.take();
        let body_result = if let Some(body) = &func.body {
            self.eval_inner(body, depth + 1)
        } else {
            self.ctx
                .struct_error(span, "`const fn` must have a body")
                .emit();
            Err(ConstEvalError)
        };
        let fn_return = self.return_value.take();
        self.return_value = saved_return;

        self.pop_local_scope();
        self.function_depth -= 1;

        if !func.generics.is_empty() {
            let _ = self.type_substs.pop();
        }

        if owner_scope.is_some() {
            let _ = self.const_scopes.pop();
        }
        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }

        let body_result = body_result?;
        Ok(fn_return.unwrap_or(body_result))
    }

    fn method_receiver<'b>(&mut self, callee: &'b Expr) -> Option<&'b Expr> {
        let ExprKind::FieldAccess { lhs, .. } = &callee.kind else {
            return None;
        };

        let lhs_ty = self.node_type(lhs.id);
        if matches!(self.ctx.type_registry.get(lhs_ty), TypeKind::Module(..)) {
            None
        } else {
            Some(lhs.as_ref())
        }
    }

    fn callable_return_and_params(
        &mut self,
        def_id: DefId,
        generic_args: &[TypeId],
    ) -> Option<(Vec<TypeId>, TypeId)> {
        let Def::Function(func) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
            return None;
        };
        let sig = func.resolved_sig?;

        let sig = if func.generics.is_empty() {
            sig
        } else {
            if func.generics.len() != generic_args.len() {
                return None;
            }
            let mut generic_map = HashMap::new();
            for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
                generic_map.insert(param.name, *arg);
            }
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &generic_map);
            subst.substitute(sig)
        };

        match self.ctx.type_registry.get(sig).clone() {
            TypeKind::Function { params, ret, .. } => Some((params, ret)),
            _ => None,
        }
    }

    fn match_pattern(
        &mut self,
        pattern: &ast::MatchPattern,
        target_value: &ConstValue,
        target_ty: TypeId,
        depth: usize,
    ) -> ConstEvalResult<Option<HashMap<SymbolId, ConstValue>>> {
        match &pattern.kind {
            ast::MatchPatternKind::Value(expr) => {
                let value = self.eval_inner(expr, depth + 1)?;
                if value == *target_value {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(None)
                }
            }
            ast::MatchPatternKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start = self.eval_inner(start, depth + 1)?;
                let end = self.eval_inner(end, depth + 1)?;
                let matches = match (target_value, start, end) {
                    (ConstValue::Int(target), ConstValue::Int(start), ConstValue::Int(end)) => {
                        if *inclusive {
                            start <= *target && *target <= end
                        } else {
                            start <= *target && *target < end
                        }
                    }
                    _ => false,
                };
                if matches {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(None)
                }
            }
            ast::MatchPatternKind::Variant {
                variant_name,
                binding,
                ..
            } => self.match_variant_pattern(
                *variant_name,
                binding.as_ref(),
                target_value,
                target_ty,
                depth,
                pattern.span,
            ),
            ast::MatchPatternKind::CatchAll => Ok(Some(HashMap::new())),
        }
    }

    fn match_variant_pattern(
        &mut self,
        variant_name: SymbolId,
        binding: Option<&ast::BindingPattern>,
        target_value: &ConstValue,
        target_ty: TypeId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<Option<HashMap<SymbolId, ConstValue>>> {
        let expected_tag = match self.variant_tag(target_ty, variant_name, depth, span)? {
            Some(tag) => tag,
            None => return Ok(None),
        };

        let mut bindings = HashMap::new();
        match target_value {
            ConstValue::Enum { tag, payload } if *tag == expected_tag => {
                if let Some(binding) = binding
                    && let Some(payload) = payload
                {
                    bindings.insert(binding.name, payload.as_ref().clone());
                }
                Ok(Some(bindings))
            }
            ConstValue::Int(tag) if *tag == expected_tag => Ok(Some(bindings)),
            _ => Ok(None),
        }
    }

    fn variant_tag(
        &mut self,
        target_ty: TypeId,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<Option<i128>> {
        match self.ctx.type_registry.get(target_ty).clone() {
            TypeKind::Enum(def_id, _) => {
                let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize).cloned()
                else {
                    return Err(ConstEvalError);
                };
                let mut current_val = 0i128;
                for variant in enum_def.variants {
                    if let Some(value_expr) = &variant.value
                        && let Ok(ConstValue::Int(value)) = self.eval_inner(value_expr, depth + 1)
                    {
                        current_val = value;
                    }
                    if variant.name == variant_name {
                        return Ok(Some(current_val));
                    }
                    current_val += 1;
                }
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "variant `.{}` not found in enum",
                            self.ctx.resolve(variant_name)
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let mut current_val = 0i128;
                for variant in enum_def.variants {
                    if let Some(value) = variant.explicit_value {
                        current_val = value;
                    }
                    if variant.name == variant_name {
                        return Ok(Some(current_val));
                    }
                    current_val += 1;
                }
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "variant `.{}` not found in anonymous enum",
                            self.ctx.resolve(variant_name)
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn eval_intrinsic_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some((def_id, generic_args)) = self.resolve_callable(callee) else {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let (is_intrinsic, fn_name_id, generics_len) =
            if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                (f.is_intrinsic, f.name, f.generics.len())
            } else {
                return Err(ConstEvalError);
            };

        if !is_intrinsic {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .with_hint(
                    "only compile-time intrinsics like `@sizeOf` or `@clz` are permitted here",
                )
                .emit();
            return Err(ConstEvalError);
        }

        let name_str = self.ctx.resolve(fn_name_id).to_string();

        // 在 const 阶段，强制要求泛型参数被显式填满
        if generic_args.len() != generics_len {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "intrinsic `{}` requires explicit generic arguments in constant evaluation",
                        name_str
                    ),
                )
                .with_hint(format!("example: `{}[u32](...)`", name_str))
                .emit();
            return Err(ConstEvalError);
        }

        // --- 核心路由 ---
        match name_str.as_str() {
            "@sizeOf" => self.eval_size_of(&generic_args, span),
            "@alignOf" => self.eval_align_of(&generic_args, span),
            "@popCount" | "@clz" | "@ctz" => {
                self.eval_bit_counting(name_str.as_str(), &generic_args, args, depth, span)
            }
            "@intCast" => self.eval_int_cast(&generic_args, args, depth, span),
            "@bswap" => self.eval_bswap(&generic_args, args, depth, span),
            "@memcpy" | "@memset" => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "memory intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    // ==========================================
    // 具体的宏实现逻辑 (拆分后极易维护)
    // ==========================================

    fn eval_size_of(
        &mut self,
        generic_args: &[TypeId],
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(&target_ty) = generic_args.first() {
            let mut layout = LayoutEngine::new(self.ctx);
            let size = layout.compute_type_size(target_ty);
            Ok(ConstValue::Int(size as i128))
        } else {
            Err(ConstEvalError) // 这个错误理论上在前面检查泛型数量时已被拦截
        }
    }

    fn eval_align_of(
        &mut self,
        generic_args: &[TypeId],
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(&target_ty) = generic_args.first() {
            let mut layout = LayoutEngine::new(self.ctx);
            let align = layout.compute_type_align(target_ty);
            Ok(ConstValue::Int(align as i128))
        } else {
            Err(ConstEvalError)
        }
    }

    fn eval_bit_counting(
        &mut self,
        name: &str,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[0];
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8; // 使用 LayoutEngine

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match name {
                "@popCount" => u_val.count_ones() as i128,
                "@clz" => (u_val.leading_zeros() as i128) - (128 - bit_width as i128),
                "@ctz" => {
                    if u_val == 0 {
                        bit_width as i128
                    } else {
                        u_val.trailing_zeros() as i128
                    }
                }
                _ => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (ConstEval): Unsupported bit intrinsic `{}` in constant evaluation.",
                            name
                        ),
                    );
                    return Err(ConstEvalError);
                }
            };
            return Ok(ConstValue::Int(res));
        }
        Err(ConstEvalError)
    }

    fn eval_int_cast(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[1];

            // 使用 LayoutEngine 获取目标类型的位宽
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let mut u_val = (val as u128) & mask;

            let is_signed = matches!(
                self.ctx.type_registry.get(target_ty),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );

            if is_signed && bit_width < 128 && (u_val & (1 << (bit_width - 1))) != 0 {
                u_val |= u128::MAX << bit_width;
            }
            return Ok(ConstValue::Int(u_val as i128));
        }
        Err(ConstEvalError)
    }

    fn eval_bswap(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[0];

            // 使用 LayoutEngine
            let mut layout = LayoutEngine::new(self.ctx);
            let bit_width = layout.compute_type_size(target_ty) * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match bit_width {
                16 => (u_val as u16).swap_bytes() as i128,
                32 => (u_val as u32).swap_bytes() as i128,
                64 => (u_val as u64).swap_bytes() as i128,
                128 => u_val.swap_bytes() as i128,
                _ => u_val as i128,
            };
            return Ok(ConstValue::Int(res));
        }
        Err(ConstEvalError)
    }

    fn eval_enum_literal(
        &mut self,
        node_id: NodeId,
        variant_name: SymbolId,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let norm_ty = self.node_type(node_id);

        let def_id = if let TypeKind::Enum(id, _) = self.ctx.type_registry.get(norm_ty) {
            *id
        } else {
            self.ctx
                .struct_error(
                    span,
                    "variant literal type could not be resolved to a data type during constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let data_def = if let Def::Enum(d) = &self.ctx.defs[def_id.0 as usize] {
            d.clone()
        } else {
            return Err(ConstEvalError);
        };

        // 禁止对带有 Payload 的 ADT 变体进行常量整数求值
        for v in &data_def.variants {
            if v.payload_type.is_some() {
                self.ctx
                    .struct_error(
                        span,
                        "cannot evaluate ADT variants with payloads as integer constants",
                    )
                    .with_hint("only C-style `data` types (without payloads) can be implicitly evaluated to integers")
                    .emit();
                return Err(ConstEvalError);
            }
        }

        let mut current_val: i128 = 0;
        for v in data_def.variants {
            if let Some(v_expr) = v.value
                && let Ok(ConstValue::Int(val)) = self.eval_inner(&v_expr, depth + 1)
            {
                current_val = val;
            }
            if v.name == variant_name {
                return Ok(ConstValue::Int(current_val));
            }
            current_val += 1;
        }

        let v_str = self.ctx.resolve(variant_name).to_string();
        self.ctx
            .struct_error(span, format!("variant `.{}` not found in data type", v_str))
            .emit();
        Err(ConstEvalError)
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable (`let`)",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "data type",
            _ => "symbol",
        }
    }
}
