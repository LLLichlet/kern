use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::Def;
use crate::passes::TypeResolver;
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind, StmtKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    /// 核心 Match 检查逻辑：环境提取与详尽性检查
    pub(crate) fn check_match_expr(
        &mut self,
        target: &Expr,
        arms: &[ast::MatchArm],
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let target_ty = self.check_expr(target, None);
        let norm_target = self.resolve_tv(target_ty);

        if norm_target == TypeId::ERROR {
            for arm in arms {
                self.check_expr(&arm.body, None);
            }
            return TypeId::ERROR;
        }

        // 尝试判断 Target 是否为 ADT (Enum)
        let is_adt = matches!(
            self.ctx.type_registry.get(norm_target),
            TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
        );

        let mut common_ret_ty = expected_ty;
        let mut handled_variants = std::collections::HashSet::new();
        let mut has_catch_all = false;

        for arm in arms {
            let body_ty = self.check_match_arm(
                arm,
                norm_target,
                is_adt,
                common_ret_ty,
                &mut handled_variants,
                &mut has_catch_all,
            );

            if common_ret_ty.is_none() || common_ret_ty == Some(TypeId::NEVER) {
                common_ret_ty = Some(body_ty);
            } else if body_ty != TypeId::NEVER {
                self.check_coercion(&arm.body, common_ret_ty.unwrap(), body_ty);
            }
        }

        // --- 详尽性检查 (Exhaustiveness) ---
        if !has_catch_all {
            if is_adt {
                let missing: Vec<_> = match self.ctx.type_registry.get(norm_target).clone() {
                    TypeKind::Enum(def_id, _) => {
                        let adt_def = match &self.ctx.defs[def_id.0 as usize] {
                            Def::Enum(a) => a,
                            _ => unreachable!(),
                        };

                        adt_def
                            .variants
                            .iter()
                            .filter(|v| !handled_variants.contains(&v.name))
                            .map(|v| self.ctx.resolve(v.name).to_string())
                            .collect()
                    }
                    TypeKind::AnonymousEnum(enum_def) => enum_def
                        .variants
                        .iter()
                        .filter(|v| !handled_variants.contains(&v.name))
                        .map(|v| self.ctx.resolve(v.name).to_string())
                        .collect(),
                    _ => Vec::new(),
                };

                if !missing.is_empty() {
                    self.ctx
                        .struct_error(span, "match expression is not exhaustive")
                        .with_hint(format!("missing variants: {}", missing.join(", ")))
                        .emit();
                }
            } else {
                // 对于非 ADT 类型 (整数, 字符串等)，如果不带 catch-all 必须报错
                self.ctx
                    .struct_error(span, "match expression must be exhaustive")
                    .with_hint("for non-ADT types (like integers or strings), consider adding an `else =>` catch-all branch")
                    .emit();
            }
        }

        common_ret_ty.unwrap_or(TypeId::VOID)
    }

    /// 单独抽离的分支检查逻辑
    fn check_match_arm(
        &mut self,
        arm: &ast::MatchArm,
        norm_target: TypeId,
        is_adt: bool,
        common_ret_ty: Option<TypeId>,
        handled_variants: &mut std::collections::HashSet<SymbolId>,
        has_catch_all: &mut bool,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();

        for pat in &arm.patterns {
            match &pat.kind {
                ast::MatchPatternKind::Value(v) => {
                    let v_ty = self.check_expr(v, Some(norm_target));
                    self.check_coercion(&v, norm_target, v_ty);

                    // 尝试从值匹配中回收 EnumLiteral 以辅助穷尽性检查
                    if is_adt {
                        if let ExprKind::EnumLiteral(name) = &v.kind {
                            handled_variants.insert(*name);
                        }
                    }
                }
                ast::MatchPatternKind::Range { start, end, .. } => {
                    let s_ty = self.check_expr(start, Some(norm_target));
                    let e_ty = self.check_expr(end, Some(norm_target));
                    self.check_coercion(&start, norm_target, s_ty);
                    self.check_coercion(&end, norm_target, e_ty);
                }
                ast::MatchPatternKind::Variant {
                    target_type,
                    variant_name,
                    binding,
                } => {
                    if !is_adt {
                        self.ctx.emit_error(
                            pat.span,
                            "variant matching is only allowed on ADT targets",
                        );
                        continue;
                    }

                    if let Some(explicit_ty_ast) = target_type {
                        let mut resolver = TypeResolver::new(self.ctx);
                        let scope = resolver.ctx.scopes.current_scope_id().unwrap();
                        let explicit_ty = resolver.resolve_type(explicit_ty_ast, scope);
                        
                        // 纯类型匹配检查，不涉及表达式和 BNC
                        let mut map = std::collections::HashMap::new();
                        if !self.unify(norm_target, explicit_ty, &mut map) && norm_target != explicit_ty {
                            self.emit_mismatch_error(pat.span, norm_target, explicit_ty);
                        }
                    }

                    match self.ctx.type_registry.get(norm_target).clone() {
                        TypeKind::Enum(def_id, generic_args) => {
                            let adt_def = match &self.ctx.defs[def_id.0 as usize] {
                                Def::Enum(a) => a.clone(),
                                _ => unreachable!(),
                            };

                            if let Some(v) = adt_def.variants.iter().find(|v| v.name == *variant_name) {
                                handled_variants.insert(*variant_name);

                                if let Some(bind_pattern) = binding {
                                    if let Some(payload_ast) = &v.payload_type {
                                        let mut payload_ty = self
                                            .ctx
                                            .node_types
                                            .get(&payload_ast.id)
                                            .copied()
                                            .unwrap_or(TypeId::ERROR);

                                        if !adt_def.generics.is_empty() && !generic_args.is_empty() {
                                            let mut map = std::collections::HashMap::new();
                                            for (i, param) in adt_def.generics.iter().enumerate() {
                                                map.insert(param.name, generic_args[i]);
                                            }
                                            let mut subst =
                                                Substituter::new(&mut self.ctx.type_registry, &map);
                                            payload_ty = subst.substitute(payload_ty);
                                        }

                                        let info = SymbolInfo {
                                            kind: SymbolKind::Var,
                                            node_id: arm.body.id,
                                            type_id: payload_ty,
                                            def_id: None,
                                            span: pat.span,
                                            is_pub: false,
                                            is_mut: bind_pattern.is_mut,
                                        };
                                        let _ = self.ctx.scopes.define(bind_pattern.name, info);
                                    } else {
                                        self.ctx
                                            .struct_error(
                                                pat.span,
                                                format!(
                                                    "variant `{}` has no payload",
                                                    self.ctx.resolve(*variant_name)
                                                ),
                                            )
                                            .emit();
                                    }
                                } else if v.payload_type.is_some() {
                                    self.ctx
                                        .struct_error(
                                            pat.span,
                                            format!(
                                                "variant `{}` requires a binding for its payload",
                                                self.ctx.resolve(*variant_name)
                                            ),
                                        )
                                        .emit();
                                }
                            } else {
                                self.ctx
                                    .struct_error(pat.span, "variant not found in ADT")
                                    .emit();
                            }
                        }
                        TypeKind::AnonymousEnum(enum_def) => {
                            if let Some(v) = enum_def.variants.iter().find(|v| v.name == *variant_name) {
                                handled_variants.insert(*variant_name);

                                if let Some(bind_pattern) = binding {
                                    if let Some(payload_ty) = v.payload_ty {
                                        let info = SymbolInfo {
                                            kind: SymbolKind::Var,
                                            node_id: arm.body.id,
                                            type_id: payload_ty,
                                            def_id: None,
                                            span: pat.span,
                                            is_pub: false,
                                            is_mut: bind_pattern.is_mut,
                                        };
                                        let _ = self.ctx.scopes.define(bind_pattern.name, info);
                                    } else {
                                        self.ctx
                                            .struct_error(
                                                pat.span,
                                                format!(
                                                    "variant `{}` has no payload",
                                                    self.ctx.resolve(*variant_name)
                                                ),
                                            )
                                            .emit();
                                    }
                                } else if v.payload_ty.is_some() {
                                    self.ctx
                                        .struct_error(
                                            pat.span,
                                            format!(
                                                "variant `{}` requires a binding for its payload",
                                                self.ctx.resolve(*variant_name)
                                            ),
                                        )
                                        .emit();
                                }
                            } else {
                                self.ctx
                                    .struct_error(pat.span, "variant not found in ADT")
                                    .emit();
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                ast::MatchPatternKind::CatchAll => {
                    *has_catch_all = true;
                }
            }
        }

        let body_ty = self.check_expr(&arm.body, common_ret_ty);
        self.ctx.scopes.exit_scope();
        body_ty
    }

    pub(crate) fn check_return(&mut self, val: Option<&Expr>, span: Span) -> TypeId {
        self.has_returned = true;
        let expected_ret = self.current_return_type.unwrap_or(TypeId::VOID);

        if let Some(v) = val {
            // 将当前函数期待的类型 (expected_ret) 传给要 return 的表达式
            // 如果是 `return .{ Some: 1 }`，这里 expected_ret 就会传进 DataInit 中
            let val_ty = self.check_expr(v, Some(expected_ret));

            if let Some(ret_ty) = self.current_return_type {
                self.check_coercion(v, ret_ty, val_ty);
            }
        } else {
            if expected_ret != TypeId::VOID && expected_ret != TypeId::ERROR {
                let ret_str = self.ctx.ty_to_string(expected_ret);
                self.ctx
                    .struct_error(span, "expected a return value, but found empty return")
                    .with_hint(format!("function is expected to return `{}`", ret_str))
                    .emit();
            }
        }
        TypeId::VOID
    }

    pub(crate) fn check_for(
        &mut self,
        init: Option<&Expr>,
        cond: Option<&Expr>,
        post: Option<&Expr>,
        body: &Expr,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        if let Some(i) = init {
            self.check_discarded_expr(i);
        }
        if let Some(c) = cond {
            let c_ty = self.check_expr(c, Some(TypeId::BOOL));
            self.check_coercion(c, TypeId::BOOL, c_ty);
        }
        if let Some(p) = post {
            self.check_discarded_expr(p);
        }
        self.check_discarded_expr(body);
        self.ctx.scopes.exit_scope();
        TypeId::VOID
    }

    /// 检查一个独立执行的表达式，其返回值是否被非法隐式丢弃
    fn check_discarded_expr(&mut self, expr: &Expr) {
        let ty = self.check_expr(expr, None);
        let norm_ty = self.resolve_tv(ty);

        // 如果既不是 void，也不是发散的 never，更不是已经报错的 error，那就是非法丢弃
        if norm_ty != TypeId::VOID && norm_ty != TypeId::NEVER && norm_ty != TypeId::ERROR {
            let ty_str = self.ctx.ty_to_string(ty);
            self.ctx
                .struct_error(expr.span, "ignored non-void return value")
                .with_hint(format!(
                    "expression evaluates to `{}`, which must be explicitly used or discarded",
                    ty_str
                ))
                .with_hint("in Kern, use `let _ = ...;` to explicitly discard the value")
                .emit();
        }
    }

    pub(crate) fn check_block(
        &mut self,
        stmts: &[ast::Stmt],
        result: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        self.ctx.scopes.enter_scope();
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::ExprStmt(e) | StmtKind::ExprValue(e) => {
                    self.check_discarded_expr(e);
                }
            }
        }
        let ret_ty = if let Some(res) = result {
            self.check_expr(res, expected_ty)
        } else {
            TypeId::VOID
        };
        self.ctx.scopes.exit_scope();
        ret_ty
    }

    pub(crate) fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: Option<&Expr>,
        expected_ty: Option<TypeId>,
    ) -> TypeId {
        let cond_ty = self.check_expr(cond, Some(TypeId::BOOL));
        self.check_coercion(cond, TypeId::BOOL, cond_ty);

        let then_ty = self.check_expr(then_branch, expected_ty);
        if let Some(else_expr) = else_branch {
            let else_ty = self.check_expr(else_expr, expected_ty);

            // 如果有一边发散(NEVER)，类型以另一边为准
            if then_ty == TypeId::NEVER {
                return else_ty;
            } else if else_ty == TypeId::NEVER {
                return then_ty;
            }

            self.check_coercion(else_expr, then_ty, else_ty);
            then_ty
        } else {
            TypeId::VOID
        }
    }

    pub(crate) fn check_defer(&mut self, defer_expr: &Expr) -> TypeId {
        self.check_discarded_expr(defer_expr);
        TypeId::VOID
    }
}
