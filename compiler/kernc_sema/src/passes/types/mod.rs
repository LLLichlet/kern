use super::ImportResolver;
use crate::SemaContext;
use crate::checker::{ConstEvaluator, ConstValue, ExprChecker, Substituter};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{
    AnonymousEnum, AnonymousField, AnonymousVariant, BuiltinAnonymousEnumKind, ConstExprBinaryOp,
    ConstExprKind, ConstExprUnaryOp, ConstGeneric, ConstGenericValue, ConstGenericValueKind,
    GenericArg, LayoutEngine, PrimitiveType, TypeId, TypeKind,
};
use kernc_ast::{self as ast, BinaryOperator, UnaryOperator, Visibility};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

mod helper;
mod items;
mod supertraits;

pub struct TypeResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
    suppress_unqualified_impl_assoc_types: bool,
}

struct PendingTraitProjection {
    trait_def_id: DefId,
    trait_args: Vec<GenericArg>,
    assoc_bindings: Vec<(DefId, TypeId)>,
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            suppress_unqualified_impl_assoc_types: false,
        }
    }

    pub fn context(&mut self) -> &mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn into_context(self) -> &'a mut SemaContext<'ctx> {
        self.ctx
    }

    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.ctx.scopes.current_scope_id()
    }

    // ==========================================
    //          Core type conversion logic
    // ==========================================

    /// Convert an AST `TypeNode` into a semantic `TypeId`.
    pub fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        // Prefer types already inferred by the expression checker, especially for `@typeOf`.
        if let Some(&cached_ty) = self.ctx.node_types.get(&ty_node.id)
            && cached_ty != TypeId::ERROR
        {
            return cached_ty;
        }

        let ty_id = match &ty_node.kind {
            ast::TypeKind::Path { anchor, segments } => {
                self.resolve_path_type(*anchor, segments, env_scope, ty_node.span)
            }
            ast::TypeKind::Void => TypeId::VOID,
            ast::TypeKind::Optional { inner } => {
                let inner_ty = self.resolve_type(inner, env_scope);
                self.make_builtin_optional_type(inner_ty, ty_node.span)
            }
            ast::TypeKind::Result { ok, err } => {
                let ok_ty = self.resolve_type(ok, env_scope);
                let err_ty = self.resolve_type(err, env_scope);
                self.make_builtin_result_type(ok_ty, err_ty)
            }

            // Inline anonymous struct.
            ast::TypeKind::Struct { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "struct", true);

                if !*is_extern {
                    anon_fields.sort_by_key(|f| f.name);
                }

                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous struct");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousStruct(*is_extern, anon_fields))
            }

            ast::TypeKind::Union { is_extern, fields } => {
                let mut anon_fields =
                    self.resolve_anonymous_fields(fields, env_scope, ty_node.span, "union", false);
                anon_fields.sort_by_key(|f| f.name);
                self.check_duplicate_anon_fields(&anon_fields, ty_node.span, "anonymous union");
                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousUnion(*is_extern, anon_fields))
            }

            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                let backing_ty = backing_type.as_ref().map(|bt| {
                    let resolved_ty = self.resolve_type(bt, env_scope);
                    if !self.ctx.type_registry.is_integer(resolved_ty)
                        && resolved_ty != TypeId::ERROR
                    {
                        self.ctx
                            .emit_error(bt.span, "anonymous enum backing type must be an integer");
                    }
                    resolved_ty
                });

                let mut anon_variants = Vec::new();
                for variant in variants {
                    let payload_ty = variant.payload_type.as_ref().map(|payload_ty| {
                        let resolved_ty = self.resolve_type(payload_ty, env_scope);
                        self.ensure_sized(resolved_ty, payload_ty.span);
                        resolved_ty
                    });

                    let explicit_value = variant.value.as_ref().map(|value_expr| {
                        self.resolve_expr(value_expr, env_scope);
                        self.ctx.scopes.set_current_scope(env_scope);
                        let mut evaluator = ConstEvaluator::new(self.ctx);
                        evaluator.eval_math(value_expr).unwrap_or(0)
                    });

                    anon_variants.push(AnonymousVariant {
                        name: variant.name,
                        name_span: variant.name_span,
                        payload_ty,
                        explicit_value,
                    });
                }

                self.check_duplicate_anon_variants(&anon_variants, ty_node.span);

                self.ctx
                    .type_registry
                    .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                        backing_ty,
                        builtin: None,
                        variants: anon_variants,
                    }))
            }

            ast::TypeKind::Pointer { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::VolatilePtr { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Slice { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Array { is_mut, elem, len } => {
                let base = self.resolve_type(elem, env_scope);
                let resolved_len =
                    self.resolve_const_generic_expr(len, TypeId::USIZE, env_scope, "array length");
                if matches!(resolved_len, ConstGeneric::Error) {
                    return TypeId::ERROR;
                }
                if let ConstGeneric::Value(value) = resolved_len
                    && let Some(value) = value.as_int()
                    && value > u32::MAX as i128
                {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                value,
                                u32::MAX
                            ),
                        )
                        .with_hint(
                            "LLVM array types are emitted with a 32-bit element count; split the object or allocate dynamically instead",
                        )
                        .emit();
                    return TypeId::ERROR;
                }
                self.ctx.type_registry.intern(TypeKind::Array {
                    is_mut: *is_mut,
                    elem: base,
                    len: resolved_len,
                })
            }
            ast::TypeKind::ArrayInfer { is_mut, elem } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::ArrayInfer {
                    is_mut: *is_mut,
                    elem: base,
                })
            }
            ast::TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::Function {
                    params: param_tys,
                    ret: ret_ty,
                    is_variadic: *is_variadic,
                })
            }
            ast::TypeKind::SelfType => {
                self.ctx.scopes.set_current_scope(env_scope);
                let self_sym = self.ctx.intern("Self");
                if let Some(info) = self.ctx.scopes.resolve(self_sym) {
                    info.type_id
                } else {
                    self.ctx.struct_error(ty_node.span, "the `Self` type is only valid inside `impl` blocks or `trait` definitions")
                        .with_hint("you are using it in a global or standard function context")
                        .emit();
                    TypeId::ERROR
                }
            }
            ast::TypeKind::Never => TypeId::NEVER,
            ast::TypeKind::Infer => {
                self.ctx.struct_error(ty_node.span, "type inference `_` is not allowed as a standalone type")
                    .with_hint("in Kern, the `_` placeholder is exclusively used for array length inference, e.g., `[_]u8.{ 1, 2, 3 }`")
                    .emit();
                TypeId::ERROR
            }
            ast::TypeKind::ClosureInterface { params, ret } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::ClosureInterface {
                    params: param_tys,
                    ret: ret_ty,
                })
            }

            ast::TypeKind::TypeOf(expr) => {
                // Placeholder until anonymous unions are fully modeled here.
                self.resolve_expr(expr, env_scope);
                TypeId::ERROR
            }
            // Named nominal types are collected earlier and should not appear as anonymous shapes here.
            _ => {
                self.ctx
                    .emit_error(ty_node.span, "Invalid or unsupported type construction");
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(ty_node.id, ty_id);
        ty_id
    }

    fn resolve_anonymous_fields(
        &mut self,
        fields: &[ast::StructFieldDef],
        env_scope: ScopeId,
        _span: Span,
        kind_name: &str,
        _allow_default_values: bool,
    ) -> Vec<AnonymousField> {
        let mut anon_fields = Vec::with_capacity(fields.len());

        for f in fields {
            let f_ty = self.resolve_type(&f.type_node, env_scope);
            self.ensure_sized(f_ty, f.type_node.span);

            if f.is_pub {
                let msg = format!("anonymous {} fields cannot be declared pub", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint(
                        "field-level `pub` is only supported on named declarations like `type Name = struct { ... }`",
                    )
                    .emit();
            }

            if f.default_value.is_some() {
                let msg = format!("anonymous {}s cannot have default field values", kind_name);
                self.ctx
                    .struct_error(f.span, msg)
                    .with_hint("default values are only allowed in named struct declarations (`type Name = struct { ... }`)")
                    .emit();
            }

            anon_fields.push(AnonymousField {
                name: f.name,
                ty: f_ty,
            });
        }

        anon_fields
    }

    fn make_builtin_optional_type(&mut self, inner_ty: TypeId, _span: Span) -> TypeId {
        let some = self.ctx.intern("Some");
        let none = self.ctx.intern("None");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Optional),
                variants: vec![
                    AnonymousVariant {
                        name: some,
                        name_span: Span::default(),
                        payload_ty: Some(inner_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: none,
                        name_span: Span::default(),
                        payload_ty: None,
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn make_builtin_result_type(&mut self, ok_ty: TypeId, err_ty: TypeId) -> TypeId {
        let ok = self.ctx.intern("Ok");
        let err = self.ctx.intern("Err");
        self.ctx
            .type_registry
            .intern(TypeKind::AnonymousEnum(AnonymousEnum {
                backing_ty: None,
                builtin: Some(BuiltinAnonymousEnumKind::Result),
                variants: vec![
                    AnonymousVariant {
                        name: ok,
                        name_span: Span::default(),
                        payload_ty: Some(ok_ty),
                        explicit_value: None,
                    },
                    AnonymousVariant {
                        name: err,
                        name_span: Span::default(),
                        payload_ty: Some(err_ty),
                        explicit_value: None,
                    },
                ],
            }))
    }

    fn check_duplicate_anon_fields(
        &mut self,
        fields: &[AnonymousField],
        span: Span,
        kind_name: &str,
    ) {
        for i in 1..fields.len() {
            if fields[i - 1].name == fields[i].name {
                let name_str = self.ctx.resolve(fields[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate field `{}` in {}", name_str, kind_name),
                    )
                    .emit();
            }
        }
    }

    fn check_duplicate_anon_variants(&mut self, variants: &[AnonymousVariant], span: Span) {
        let mut sorted = variants.to_vec();
        sorted.sort_by_key(|variant| variant.name);
        for i in 1..sorted.len() {
            if sorted[i - 1].name == sorted[i].name {
                let name_str = self.ctx.resolve(sorted[i].name).to_string();
                self.ctx
                    .struct_error(
                        span,
                        format!("duplicate variant `{}` in anonymous enum", name_str),
                    )
                    .emit();
            }
        }
    }

    // Recursively resolve every nested `TypeNode` inside an expression tree.
    fn resolve_pattern(&mut self, pattern: &ast::Pattern, scope: ScopeId) {
        match &pattern.kind {
            ast::PatternKind::Binding(_)
            | ast::PatternKind::Ignore
            | ast::PatternKind::Variant(_) => {
                if let ast::PatternKind::Variant(variant) = &pattern.kind
                    && let Some(ty) = &variant.target_type
                {
                    self.resolve_type(ty, scope);
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                if let Some(ty) = &destructure.target_type {
                    self.resolve_type(ty, scope);
                }
                for field in &destructure.fields {
                    self.resolve_pattern(&field.pattern, scope);
                }
            }
        }
    }

    fn resolve_expr(&mut self, expr: &ast::Expr, scope: ScopeId) {
        match &expr.kind {
            ast::ExprKind::Let {
                pattern,
                init,
                else_pattern,
                else_branch,
            } => {
                self.resolve_pattern(&pattern.pattern, scope);
                if let Some(else_pattern) = else_pattern {
                    self.resolve_pattern(else_pattern, scope);
                }
                self.resolve_expr(init, scope);
                if let Some(else_branch) = else_branch {
                    self.resolve_expr(else_branch, scope);
                }
            }
            ast::ExprKind::Static { init, .. } => {
                self.resolve_expr(init, scope);
            }
            ast::ExprKind::As { lhs, target } => {
                self.resolve_expr(lhs, scope);
                self.resolve_type(target, scope); // Resolve captured type nodes.
            }
            ast::ExprKind::TypeNode(type_node) => {
                self.resolve_type(type_node, scope);
            }
            ast::ExprKind::Block { stmts, result } => {
                let prev_scope = self.ctx.scopes.current_scope_id();
                let mut block_scope = scope;
                let mut entered_scope = false;

                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::Use(use_stmt) => {
                            let import = ImportDef {
                                path_kind: use_stmt.kind,
                                path: use_stmt.path.clone(),
                                target: use_stmt.target.clone(),
                                vis: Visibility::Private,
                                span: stmt.span,
                                binding_span: use_stmt.binding_span,
                            };

                            let needs_scope_extension = ImportResolver::binding_names(&import)
                                .into_iter()
                                .any(|name| {
                                    !entered_scope
                                        || self.ctx.scopes.resolve_from(block_scope, name).is_some()
                                });

                            if needs_scope_extension {
                                self.ctx.scopes.set_current_scope(block_scope);
                                block_scope = self.ctx.scopes.enter_scope();
                                entered_scope = true;
                            }

                            let Some(current_module) = self.ctx.module_for_scope(block_scope)
                            else {
                                self.ctx.emit_ice(
                                    stmt.span,
                                    "Kern ICE (Types): could not determine module for a local import",
                                );
                                continue;
                            };

                            {
                                let mut resolver = ImportResolver::new(self.ctx);
                                let _ = resolver.resolve_import_into_scope(
                                    current_module,
                                    block_scope,
                                    &import,
                                    true,
                                );
                            }
                        }
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            self.resolve_expr(e, block_scope);
                        }
                    }
                }
                if let Some(r) = result {
                    self.resolve_expr(r, block_scope);
                }
                if let Some(prev_scope) = prev_scope {
                    self.ctx.scopes.set_current_scope(prev_scope);
                }
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.resolve_expr(cond, scope);
                self.resolve_expr(then_branch, scope);
                if let Some(e) = else_branch {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.resolve_expr(target, scope);
                for arm in arms {
                    for pat in &arm.patterns {
                        match &pat.kind {
                            ast::MatchPatternKind::Value(e) => self.resolve_expr(e, scope),
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.resolve_expr(start, scope);
                                self.resolve_expr(end, scope);
                            }
                            ast::MatchPatternKind::Pattern(pattern) => {
                                self.resolve_pattern(pattern, scope);
                            }
                        }
                    }
                    self.resolve_expr(&arm.body, scope);
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                if let Some(e) = init {
                    self.resolve_expr(e, scope);
                }
                if let Some(e) = cond {
                    self.resolve_expr(e, scope);
                }
                if let Some(e) = post {
                    self.resolve_expr(e, scope);
                }
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Closure {
                captures,
                params,
                ret_type,
                body,
            } => {
                for cap in captures {
                    self.resolve_expr(&cap.value, scope);
                }
                for param in params {
                    self.resolve_type(&param.type_node, scope);
                }
                self.resolve_type(ret_type, scope);
                self.resolve_expr(body, scope);
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(rhs, scope);
            }
            ast::ExprKind::Unary { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                self.resolve_expr(lhs, scope);
            }
            ast::ExprKind::Propagate { operand, .. } => {
                self.resolve_expr(operand, scope);
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.resolve_expr(lhs, scope);
                self.resolve_expr(index, scope);
            }
            ast::ExprKind::Call { callee, args } => {
                self.resolve_expr(callee, scope);
                for arg in args {
                    self.resolve_expr(arg, scope);
                }
            }
            ast::ExprKind::GenericInstantiation { target, args } => {
                self.resolve_expr(target, scope);
                // Resolve generic arguments.
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty) => {
                            if let Some(expr) = self.reinterpret_type_arg_as_const_expr(ty)
                                && (self.expr_references_const_param(&expr, scope)
                                    || self.type_arg_is_payloadless_enum_value_ref(ty, scope))
                            {
                                self.resolve_expr(&expr, scope);
                            } else {
                                self.resolve_type(ty, scope);
                            }
                        }
                        ast::GenericArg::AssocBinding { value: ty, .. } => {
                            self.resolve_type(ty, scope);
                        }
                        ast::GenericArg::ConstExpr(expr) => self.resolve_expr(expr, scope),
                    }
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                // Resolve the elided-initialization prefix type.
                if let Some(ty) = type_node {
                    self.resolve_type(ty, scope);
                }
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        for f in fields {
                            self.resolve_expr(&f.value, scope);
                        }
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        for e in elems {
                            self.resolve_expr(e, scope);
                        }
                    }
                    ast::DataLiteralKind::Repeat { value, count } => {
                        self.resolve_expr(value, scope);
                        self.resolve_expr(count, scope);
                    }
                    ast::DataLiteralKind::Scalar(inner) => {
                        self.resolve_expr(inner, scope);
                    }
                }
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.resolve_expr(lhs, scope);
                if let Some(s) = start {
                    self.resolve_expr(s, scope);
                }
                if let Some(e) = end {
                    self.resolve_expr(e, scope);
                }
            }
            ast::ExprKind::Defer { expr: e } => self.resolve_expr(e, scope),
            ast::ExprKind::Return(Some(e)) => self.resolve_expr(e, scope),

            // Leaf nodes such as identifiers and literals contain no nested type nodes.
            _ => {}
        }
    }

    /// Resolve a segmented type path or projection chain.
    fn resolve_type_anchor_scope(
        &mut self,
        anchor: ast::PathAnchor,
        env_scope: ScopeId,
        span: Span,
    ) -> Option<ScopeId> {
        let Some(current_module) = self.ctx.module_for_scope(env_scope) else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Types): could not determine current module for anchored type path",
            );
            return None;
        };

        match anchor {
            ast::PathAnchor::Parent => {
                let Some(parent) = self.ctx.module_parent(current_module) else {
                    self.ctx
                        .struct_error(span, "Cannot use `..` in a root module type path")
                        .emit();
                    return None;
                };
                match &self.ctx.defs[parent.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => {
                        self.ctx.emit_ice(
                            span,
                            "Kern ICE (Types): parent module def is not a module while resolving anchored type path",
                        );
                        None
                    }
                }
            }
            ast::PathAnchor::Package => {
                let root = self.ctx.module_root(current_module);
                match &self.ctx.defs[root.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => {
                        self.ctx.emit_ice(
                            span,
                            "Kern ICE (Types): root module def is not a module while resolving anchored type path",
                        );
                        None
                    }
                }
            }
        }
    }

    fn resolve_path_type(
        &mut self,
        anchor: Option<ast::PathAnchor>,
        segments: &[ast::TypePathSegment],
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        if segments.is_empty() {
            return TypeId::ERROR;
        }

        let mut curr_scope = match anchor {
            Some(anchor) => match self.resolve_type_anchor_scope(anchor, env_scope, span) {
                Some(scope) => scope,
                None => return TypeId::ERROR,
            },
            None => env_scope,
        };
        let mut current_ty = None;
        let mut pending_trait_projection: Option<PendingTraitProjection> = None;

        for (index, segment) in segments.iter().enumerate() {
            if let Some(PendingTraitProjection {
                trait_def_id,
                trait_args,
                assoc_bindings,
            }) = pending_trait_projection.take()
            {
                current_ty = Some(self.resolve_projected_associated_type(
                    current_ty.unwrap_or(TypeId::ERROR),
                    trait_def_id,
                    trait_args,
                    assoc_bindings,
                    segment,
                    env_scope,
                ));
                continue;
            }

            if current_ty.is_none() {
                let (target_symbol, skipped_hidden_assoc) = if index == 0 {
                    if segments.len() == 1 {
                        let name_str = self.ctx.resolve(segment.name).to_string();
                        if let Some(prim_id) = self.resolve_builtin_primitive(&name_str) {
                            if !segment.args.is_empty() {
                                self.ctx.emit_error(
                                    span,
                                    "Primitive types do not take generic arguments",
                                );
                            }
                            return prim_id;
                        }
                    }
                    if anchor.is_some() {
                        (
                            self.ctx.scopes.resolve_in(curr_scope, segment.name).cloned(),
                            false,
                        )
                    } else {
                        self.resolve_head_type_symbol(curr_scope, segment.name)
                    }
                } else {
                    (
                        self.ctx.scopes.resolve_in(curr_scope, segment.name).cloned(),
                        false,
                    )
                };

                let Some(sym) = target_symbol else {
                    let name = self.ctx.resolve(segment.name).to_string();
                    if index == 0 && skipped_hidden_assoc {
                        self.ctx
                            .struct_error(
                                segment.name_span,
                                format!(
                                    "impl-associated type targets must resolve to a concrete type, but `{}` resolves to the impl's associated type placeholder here",
                                    name
                                ),
                            )
                            .with_hint(
                                "inside `type Name = ...;`, bare associated type names are not available as concrete aliases",
                            )
                            .with_hint(
                                "use a distinct concrete type name, a generic parameter, or an explicit projected type outside the impl-associated type definition",
                            )
                            .emit();
                    } else if index == 0 {
                        self.ctx
                            .emit_error(span, format!("Cannot find type `{}` in this scope", name));
                    } else {
                        self.ctx.emit_error(
                            span,
                            format!("Cannot find `{}` in the target module", name),
                        );
                    }
                    return TypeId::ERROR;
                };

                if index < segments.len() - 1 && sym.kind == SymbolKind::Module {
                    if !segment.args.is_empty() {
                        self.ctx.emit_error(
                            segment.name_span,
                            "module path segments cannot take type arguments",
                        );
                        return TypeId::ERROR;
                    }
                    let Some(mod_def_id) =
                        self.required_def_id(&sym, span, "module path segment", segment.name)
                    else {
                        return TypeId::ERROR;
                    };
                    let Some(module_scope) =
                        self.module_scope_from_def(mod_def_id, span, segment.name)
                    else {
                        return TypeId::ERROR;
                    };
                    curr_scope = module_scope;
                    continue;
                }

                current_ty = Some(self.resolve_named_type_symbol(&sym, segment, env_scope, span));
                continue;
            }

            let current = current_ty.unwrap_or(TypeId::ERROR);
            let trait_symbol = self.lookup_trait_projection_symbol(segment.name, env_scope);
            let Some((trait_def_id, _trait_symbol)) = trait_symbol else {
                self.ctx.emit_error(
                    segment.name_span,
                    format!(
                        "`{}` is not a trait projection on `{}`",
                        self.ctx.resolve(segment.name),
                        self.ctx.ty_to_string(current)
                    ),
                );
                return TypeId::ERROR;
            };

            let (trait_args, assoc_bindings) =
                self.resolve_trait_segment_args(trait_def_id, &segment.args, env_scope, span);
            if !trait_args.is_empty()
                && trait_args.iter().all(|arg| {
                    matches!(
                        arg,
                        GenericArg::Type(TypeId::ERROR) | GenericArg::Const(ConstGeneric::Error)
                    )
                })
            {
                return TypeId::ERROR;
            }

            if index == segments.len() - 1 {
                self.ctx
                    .struct_error(
                        segment.name_span,
                        format!(
                            "trait qualification `{}` must be followed by an associated type name",
                            self.ctx.resolve(segment.name)
                        ),
                    )
                    .emit();
                return TypeId::ERROR;
            }

            pending_trait_projection = Some(PendingTraitProjection {
                trait_def_id,
                trait_args,
                assoc_bindings,
            });
        }

        if pending_trait_projection.is_some() {
            self.ctx
                .emit_error(span, "expected associated type after trait qualification");
            return TypeId::ERROR;
        }

        current_ty.unwrap_or(TypeId::ERROR)
    }

    fn resolve_head_type_symbol(
        &mut self,
        scope_id: ScopeId,
        name: SymbolId,
    ) -> (Option<crate::scope::SymbolInfo>, bool) {
        let mut curr = Some(scope_id);
        let mut skipped_hidden_assoc = false;

        while let Some(scope_id) = curr {
            if let Some(info) = self.ctx.scopes.resolve_in(scope_id, name).cloned() {
                if self.suppress_unqualified_impl_assoc_types
                    && info.kind == SymbolKind::AssociatedType
                {
                    skipped_hidden_assoc = true;
                } else {
                    return (Some(info), skipped_hidden_assoc);
                }
            }
            curr = self.ctx.scopes.parent_scope(scope_id);
        }

        (None, skipped_hidden_assoc)
    }

    fn resolve_named_type_symbol(
        &mut self,
        final_sym: &crate::scope::SymbolInfo,
        segment: &ast::TypePathSegment,
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        let (resolved_generics, resolved_assoc_bindings) = if let Some(def_id) = final_sym.def_id {
            self.resolve_generic_args_for_def(def_id, &segment.args, env_scope, span)
        } else {
            self.resolve_type_args(&segment.args, env_scope)
        };

        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "named types do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) = self.required_def_id(final_sym, span, "type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, resolved_generics))
            }
            SymbolKind::Enum => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "enum types do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) = self.required_def_id(final_sym, span, "enum type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Enum(def_id, resolved_generics))
            }
            SymbolKind::Trait => {
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "trait object type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                let (trait_args, assoc_bindings) =
                    self.resolve_trait_segment_args(def_id, &segment.args, env_scope, span);
                self.ctx.type_registry.intern(TypeKind::TraitObject(
                    def_id,
                    trait_args,
                    assoc_bindings,
                ))
            }
            SymbolKind::TypeParam => {
                if !segment.args.is_empty() {
                    self.ctx
                        .emit_error(span, "Type parameters cannot take type arguments");
                }
                final_sym.type_id
            }
            SymbolKind::ConstParam => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a const generic parameter, not a type",
                            self.ctx.resolve(segment.name)
                        ),
                    )
                    .with_hint(
                        "const generic parameters can only appear in constant positions such as `[N]T` or `Type[T, N]`",
                    )
                    .emit();
                TypeId::ERROR
            }
            SymbolKind::AssociatedType => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "associated types do not accept nested associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "associated type", segment.name)
                else {
                    return TypeId::ERROR;
                };

                let Some(assoc_def) =
                    self.ctx
                        .defs
                        .get(def_id.0 as usize)
                        .and_then(|def| match def {
                            Def::AssociatedType(assoc) => Some(assoc.clone()),
                            _ => None,
                        })
                else {
                    self.ctx.emit_ice(
                        span,
                        "associated type symbol does not point to an associated type def",
                    );
                    return TypeId::ERROR;
                };

                if assoc_def.generics.len() != resolved_generics.len() {
                    self.ctx.emit_error(
                        span,
                        format!(
                            "associated type `{}` expects {} generic arguments, but {} were provided",
                            self.last_segment_name(std::slice::from_ref(segment)),
                            assoc_def.generics.len(),
                            resolved_generics.len()
                        ),
                    );
                    return TypeId::ERROR;
                }

                if let Some(target) = assoc_def.target.as_ref() {
                    let target_ty = self
                        .ctx
                        .node_types
                        .get(&target.id)
                        .copied()
                        .unwrap_or(final_sym.type_id);
                    if resolved_generics.is_empty() {
                        return target_ty;
                    }
                    let mut map = std::collections::HashMap::new();
                    for (param, arg) in assoc_def.generics.iter().zip(resolved_generics.iter()) {
                        map.insert(param.name, *arg);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    return subst.substitute(target_ty);
                }

                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, resolved_generics))
            }
            SymbolKind::TypeAlias => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "type aliases do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                if final_sym.def_id.is_none() {
                    return final_sym.type_id;
                }
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "type alias", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }

                let target_ty = if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    self.ctx
                        .node_types
                        .get(&t_def.target.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR)
                } else {
                    TypeId::ERROR
                };

                if target_ty == TypeId::ERROR {
                    let name = self.last_segment_name(std::slice::from_ref(segment));
                    self.ctx.struct_error(span, format!("type alias `{}` could not be resolved", name))
                        .with_hint("this might be caused by an invalid circular alias dependency or use before resolution")
                        .emit();
                    return TypeId::ERROR;
                }

                if resolved_generics.is_empty() {
                    target_ty
                } else if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    if t_def.generics.len() != resolved_generics.len() {
                        self.ctx.emit_error(
                            span,
                            format!(
                                "Type alias `{}` expects {} generic arguments, but {} were provided",
                                self.last_segment_name(std::slice::from_ref(segment)),
                                t_def.generics.len(),
                                resolved_generics.len()
                            ),
                        );
                        return TypeId::ERROR;
                    }
                    let mut map = std::collections::HashMap::new();
                    for (i, param) in t_def.generics.iter().enumerate() {
                        map.insert(param.name, resolved_generics[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(target_ty)
                } else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Type alias symbol `{}` resolved to non-alias def {:?}",
                            self.last_segment_name(std::slice::from_ref(segment)),
                            def_id
                        ),
                    );
                    TypeId::ERROR
                }
            }
            _ => {
                let name = self.last_segment_name(std::slice::from_ref(segment));
                self.ctx.emit_error(
                    span,
                    format!(
                        "`{}` is a {}, not a type",
                        name,
                        self.kind_to_string(final_sym.kind)
                    ),
                );
                TypeId::ERROR
            }
        }
    }

    fn resolve_type_args(
        &mut self,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        let mut positional = Vec::new();
        let mut assoc_bindings = Vec::new();
        for arg in args {
            match arg {
                ast::GenericArg::Type(ty) => {
                    positional.push(GenericArg::Type(self.resolve_type(ty, env_scope)))
                }
                ast::GenericArg::AssocBinding { name, value, .. } => {
                    assoc_bindings.push((*name, self.resolve_type(value, env_scope)));
                }
                ast::GenericArg::ConstExpr(expr) => {
                    positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                        expr,
                        TypeId::USIZE,
                        env_scope,
                        "const generic argument",
                    )));
                }
            }
        }
        (positional, assoc_bindings)
    }

    fn reinterpret_type_arg_as_const_expr(&mut self, ty_node: &ast::TypeNode) -> Option<ast::Expr> {
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return None;
        };
        if segments.is_empty() || segments.iter().any(|segment| !segment.args.is_empty()) {
            return None;
        }

        let first = &segments[0];
        let mut expr = if let Some(anchor) = *anchor {
            ast::Expr {
                id: self.ctx.next_node_id(),
                span: first.name_span,
                kind: ast::ExprKind::AnchoredPath {
                    anchor,
                    name: first.name,
                    name_span: first.name_span,
                },
            }
        } else {
            ast::Expr {
                id: self.ctx.next_node_id(),
                span: first.name_span,
                kind: ast::ExprKind::Identifier(first.name),
            }
        };

        for segment in &segments[1..] {
            expr = ast::Expr {
                id: self.ctx.next_node_id(),
                span: ty_node.span,
                kind: ast::ExprKind::FieldAccess {
                    lhs: Box::new(expr),
                    field: segment.name,
                    field_span: segment.name_span,
                },
            };
        }

        Some(expr)
    }

    fn type_arg_is_payloadless_enum_value_ref(
        &mut self,
        ty_node: &ast::TypeNode,
        env_scope: ScopeId,
    ) -> bool {
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return false;
        };
        if segments.len() < 2 || segments.iter().any(|segment| !segment.args.is_empty()) {
            return false;
        }

        let last_segment = segments.last().unwrap();
        let mut current_scope = match anchor {
            Some(anchor) => {
                let current_scope = self.ctx.scopes.current_scope_id().unwrap_or(env_scope);
                let Some(current_module) = self.ctx.module_for_scope(current_scope) else {
                    return false;
                };
                let target_module = match anchor {
                    ast::PathAnchor::Parent => {
                        let Some(parent) = self.ctx.module_parent(current_module) else {
                            return false;
                        };
                        parent
                    }
                    ast::PathAnchor::Package => self.ctx.module_root(current_module),
                };
                let Some(module_scope) =
                    self.module_scope_from_def(target_module, ty_node.span, last_segment.name)
                else {
                    return false;
                };
                module_scope
            }
            None => env_scope,
        };

        for (index, segment) in segments[..segments.len() - 1].iter().enumerate() {
            let symbol = if index == 0 && anchor.is_none() {
                self.ctx.scopes.resolve_from(current_scope, segment.name)
            } else {
                self.ctx.scopes.resolve_in(current_scope, segment.name)
            };
            let Some(symbol) = symbol.cloned() else {
                return false;
            };

            match symbol.kind {
                SymbolKind::Module => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(module_scope) =
                        self.module_scope_from_def(def_id, segment.name_span, segment.name)
                    else {
                        return false;
                    };
                    current_scope = module_scope;
                }
                SymbolKind::Enum if index == segments.len() - 2 => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize) else {
                        return false;
                    };
                    return enum_def.variants.iter().any(|variant| {
                        variant.name == last_segment.name && variant.payload_type.is_none()
                    });
                }
                SymbolKind::TypeAlias if index == segments.len() - 2 => {
                    let alias_ty = self.ctx.type_registry.normalize(symbol.type_id);
                    return match self.ctx.type_registry.get(alias_ty) {
                        TypeKind::Enum(def_id, _) => self
                            .ctx
                            .defs
                            .get(def_id.0 as usize)
                            .and_then(|def| match def {
                                Def::Enum(enum_def) => Some(enum_def),
                                _ => None,
                            })
                            .is_some_and(|enum_def| {
                                enum_def.variants.iter().any(|variant| {
                                    variant.name == last_segment.name
                                        && variant.payload_type.is_none()
                                })
                            }),
                        TypeKind::AnonymousEnum(enum_def) => {
                            enum_def.variants.iter().any(|variant| {
                                variant.name == last_segment.name && variant.payload_ty.is_none()
                            })
                        }
                        _ => false,
                    };
                }
                _ => return false,
            }
        }

        false
    }

    pub(crate) fn resolve_generic_args_for_params(
        &mut self,
        params: &[ast::GenericParam],
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        let positional_count = args
            .iter()
            .filter(|arg| !matches!(arg, ast::GenericArg::AssocBinding { .. }))
            .count();
        let mut positional = Vec::with_capacity(positional_count);
        let mut assoc_bindings = Vec::new();
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                ast::GenericArg::AssocBinding { name, value, .. } => {
                    assoc_bindings.push((*name, self.resolve_type(value, env_scope)));
                }
                ast::GenericArg::Type(ty_node) => {
                    let expected = params.get(positional_index).map(|param| &param.kind);
                    match expected {
                        Some(ast::GenericParamKind::Type) | None => {
                            let resolved_ty = self.resolve_type(ty_node, env_scope);
                            positional.push(GenericArg::Type(resolved_ty));
                        }
                        Some(ast::GenericParamKind::Const { ty }) => {
                            if let Some(expr) = self.reinterpret_type_arg_as_const_expr(ty_node) {
                                let expected_ty =
                                    self.resolve_const_generic_param_type(ty, env_scope, expr.span);
                                positional.push(GenericArg::Const(
                                    self.resolve_const_generic_expr(
                                        &expr,
                                        expected_ty,
                                        env_scope,
                                        "const generic argument",
                                    ),
                                ));
                            } else {
                                self.ctx
                                    .struct_error(
                                        ty_node.span,
                                        "expected a const generic argument here, but found a type",
                                    )
                                    .with_span_label(
                                        span,
                                        "while resolving this generic instantiation",
                                    )
                                    .emit();
                                positional.push(GenericArg::Const(ConstGeneric::Error));
                            }
                        }
                    }
                    positional_index += 1;
                }
                ast::GenericArg::ConstExpr(expr) => {
                    let expected = params.get(positional_index).map(|param| &param.kind);
                    match expected {
                        Some(ast::GenericParamKind::Const { ty }) => {
                            let expected_ty =
                                self.resolve_const_generic_param_type(ty, env_scope, expr.span);
                            positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                                expr,
                                expected_ty,
                                env_scope,
                                "const generic argument",
                            )));
                        }
                        Some(ast::GenericParamKind::Type) => {
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    "expected a type generic argument here, but found a constant",
                                )
                                .with_hint(
                                    "type parameters must be instantiated with a type, such as `i32` or `Array[u8, 4]`",
                                )
                                .emit();
                            positional.push(GenericArg::Type(TypeId::ERROR));
                        }
                        None => {
                            positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                                expr,
                                TypeId::USIZE,
                                env_scope,
                                "const generic argument",
                            )));
                        }
                    }
                    positional_index += 1;
                }
            }
        }

        (positional, assoc_bindings)
    }

    fn resolve_generic_args_for_def(
        &mut self,
        def_id: DefId,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        let generics = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => f.generics.clone(),
            Def::Struct(s) => s.generics.clone(),
            Def::Union(u) => u.generics.clone(),
            Def::Enum(e) => e.generics.clone(),
            Def::Trait(t) => t.generics.clone(),
            Def::TypeAlias(t) => t.generics.clone(),
            Def::AssociatedType(a) => a.generics.clone(),
            _ => Vec::new(),
        };
        self.resolve_generic_args_for_params(&generics, args, env_scope, span)
    }

    fn resolve_trait_segment_args(
        &mut self,
        trait_def_id: DefId,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(DefId, TypeId)>) {
        let (resolved_generics, resolved_assoc_bindings) =
            self.resolve_generic_args_for_def(trait_def_id, args, env_scope, span);
        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };
        if !self.check_type_generic_bounds(span, trait_def_id, &resolved_generics) {
            return (vec![GenericArg::Type(TypeId::ERROR)], Vec::new());
        }
        let mut bindings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (assoc_name, ty) in resolved_assoc_bindings {
            let Some(assoc_def_id) = trait_assoc_ids.iter().copied().find(|assoc_id| {
                self.ctx.defs[assoc_id.0 as usize]
                    .name()
                    .is_some_and(|name| name == assoc_name)
            }) else {
                self.ctx.emit_error(
                    span,
                    format!(
                        "trait `{}` does not declare associated type `{}`",
                        self.ctx.defs[trait_def_id.0 as usize]
                            .name()
                            .map(|sym| self.ctx.resolve(sym))
                            .unwrap_or("<trait>"),
                        self.ctx.resolve(assoc_name)
                    ),
                );
                continue;
            };
            if !seen.insert(assoc_def_id) {
                self.ctx.emit_error(
                    span,
                    format!(
                        "duplicate associated type binding `{}`",
                        self.ctx.resolve(assoc_name)
                    ),
                );
                continue;
            }
            bindings.push((assoc_def_id, ty));
        }
        bindings.sort_by_key(|(assoc_id, _)| assoc_id.0);
        (resolved_generics, bindings)
    }

    fn lookup_trait_projection_symbol(
        &mut self,
        name: SymbolId,
        env_scope: ScopeId,
    ) -> Option<(DefId, crate::scope::SymbolInfo)> {
        self.ctx.scopes.set_current_scope(env_scope);
        let symbol = self.ctx.scopes.resolve(name).cloned()?;
        if symbol.kind != SymbolKind::Trait {
            return None;
        }
        let def_id = symbol.def_id?;
        Some((def_id, symbol))
    }

    fn resolve_projected_associated_type(
        &mut self,
        target_ty: TypeId,
        trait_def_id: DefId,
        trait_args: Vec<GenericArg>,
        assoc_bindings: Vec<(DefId, TypeId)>,
        segment: &ast::TypePathSegment,
        env_scope: ScopeId,
    ) -> TypeId {
        let assoc_def_id = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.iter().copied().find(|assoc_id| {
                self.ctx.defs[assoc_id.0 as usize]
                    .name()
                    .is_some_and(|name| name == segment.name)
            }),
            _ => None,
        };
        let Some(assoc_def_id) = assoc_def_id else {
            self.ctx.emit_error(
                segment.name_span,
                format!(
                    "trait `{}` has no associated type `{}`",
                    self.ctx.defs[trait_def_id.0 as usize]
                        .name()
                        .map(|sym| self.ctx.resolve(sym))
                        .unwrap_or("<trait>"),
                    self.ctx.resolve(segment.name)
                ),
            );
            return TypeId::ERROR;
        };

        if let Some((_, ty)) = assoc_bindings
            .iter()
            .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
        {
            if !segment.args.is_empty() {
                self.ctx.emit_error(
                    segment.name_span,
                    "bound associated type projections cannot take extra generic arguments",
                );
                return TypeId::ERROR;
            }
            return *ty;
        }

        let (assoc_args, nested_assoc_bindings) = self.resolve_generic_args_for_def(
            assoc_def_id,
            &segment.args,
            env_scope,
            segment.name_span,
        );
        if !nested_assoc_bindings.is_empty() {
            self.ctx.emit_error(
                segment.name_span,
                "associated type projections do not accept nested associated bindings",
            );
            return TypeId::ERROR;
        }
        self.ctx.type_registry.intern(TypeKind::Projection {
            target: target_ty,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        })
    }

    pub(crate) fn resolve_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        if expected_ty == TypeId::ERROR {
            return ConstGeneric::Error;
        }

        let value = self.build_const_generic_expr(expr, expected_ty, env_scope, context);
        self.ctx.type_registry.fold_const_generic(value)
    }

    fn build_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        if !self.expr_references_const_param(expr, env_scope) {
            return self.resolve_closed_const_generic_expr(expr, expected_ty, env_scope, context);
        }

        self.ctx.scopes.set_current_scope(env_scope);
        match &expr.kind {
            ast::ExprKind::Identifier(name) => {
                let Some(info) = self.ctx.scopes.resolve(*name).cloned() else {
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!("{} must reference a known const generic parameter", context),
                        )
                        .emit();
                    return ConstGeneric::Error;
                };

                if info.kind != SymbolKind::ConstParam {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                }

                if info.type_id != expected_ty {
                    self.ctx
                        .struct_error(
                            expr.span,
                            format!(
                                "const generic parameter `{}` has type `{}`, but `{}` requires `{}`",
                                self.ctx.resolve(*name),
                                self.ctx.ty_to_string(info.type_id),
                                context,
                                self.ctx.ty_to_string(expected_ty)
                            ),
                        )
                        .with_hint(
                            "use an explicit `as` cast if you want to convert the const parameter to another integer type",
                        )
                        .emit();
                    return ConstGeneric::Error;
                }

                ConstGeneric::Param(*name, info.type_id)
            }
            ast::ExprKind::Unary { op, operand } => {
                let Some(op) = self.const_expr_unary_op(*op) else {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                };
                let operand =
                    self.build_const_generic_expr(operand, expected_ty, env_scope, context);
                if matches!(operand, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                ConstGeneric::Expr(
                    self.ctx
                        .type_registry
                        .intern_const_expr(ConstExprKind::Unary {
                            op,
                            expr: operand,
                            ty: expected_ty,
                        }),
                )
            }
            ast::ExprKind::Binary { lhs, op, rhs } => {
                let Some(op) = self.const_expr_binary_op(*op) else {
                    self.unsupported_parametric_const_generic_expr(expr.span, context);
                    return ConstGeneric::Error;
                };
                let lhs = self.build_const_generic_expr(lhs, expected_ty, env_scope, context);
                let rhs = self.build_const_generic_expr(rhs, expected_ty, env_scope, context);
                if matches!(lhs, ConstGeneric::Error) || matches!(rhs, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                    ConstExprKind::Binary {
                        op,
                        lhs,
                        rhs,
                        ty: expected_ty,
                    },
                ))
            }
            ast::ExprKind::As { lhs, target } => {
                let target_ty =
                    self.resolve_const_generic_param_type(target, env_scope, target.span);
                let lhs = self.build_const_generic_expr(lhs, target_ty, env_scope, context);
                if matches!(lhs, ConstGeneric::Error) {
                    return ConstGeneric::Error;
                }
                let cast_expr = ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                    ConstExprKind::Cast {
                        expr: lhs,
                        ty: target_ty,
                    },
                ));
                if target_ty == expected_ty {
                    cast_expr
                } else {
                    ConstGeneric::Expr(self.ctx.type_registry.intern_const_expr(
                        ConstExprKind::Cast {
                            expr: cast_expr,
                            ty: expected_ty,
                        },
                    ))
                }
            }
            _ => {
                self.unsupported_parametric_const_generic_expr(expr.span, context);
                ConstGeneric::Error
            }
        }
    }

    fn resolve_closed_const_generic_expr(
        &mut self,
        expr: &ast::Expr,
        expected_ty: TypeId,
        env_scope: ScopeId,
        context: &str,
    ) -> ConstGeneric {
        self.ctx.scopes.set_current_scope(env_scope);
        let checked_ty = {
            let mut checker = ExprChecker::new(self.ctx, None);
            checker.check_expr(expr, Some(expected_ty))
        };
        if checked_ty == TypeId::ERROR {
            return ConstGeneric::Error;
        }
        let mut evaluator = ConstEvaluator::new(self.ctx);
        let Ok(mut value) = evaluator.eval_const_value(expr) else {
            return ConstGeneric::Error;
        };
        let expected_norm = self.ctx.type_registry.normalize(expected_ty);
        let checked_norm = self.ctx.type_registry.normalize(checked_ty);
        if expected_norm == checked_norm
            && matches!(
                self.ctx.type_registry.get(expected_norm),
                TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)
            )
            && let ConstValue::Int(tag) = value
        {
            value = ConstValue::Enum { tag, payload: None };
        }
        let Some(value) = self.coerce_const_generic_value(value, expected_ty, expr.span, context)
        else {
            return ConstGeneric::Error;
        };
        ConstGeneric::Value(value)
    }

    fn unsupported_parametric_const_generic_expr(&mut self, span: Span, context: &str) {
        self.ctx
            .struct_error(
                span,
                format!(
                    "{} can only use symbolic computed expressions for integer const parameters",
                    context
                ),
            )
            .with_hint(
                "supported symbolic forms are direct const parameters, literals / const items, unary `-` or `~`, integer arithmetic / bitwise operators, and explicit `as` casts",
            )
            .with_hint(
                "non-integer const parameters such as `bool` may still be passed directly as literals, const items, or direct parameter references",
            )
            .emit();
    }

    fn const_expr_unary_op(&self, op: UnaryOperator) -> Option<ConstExprUnaryOp> {
        match op {
            UnaryOperator::Negate => Some(ConstExprUnaryOp::Negate),
            UnaryOperator::BitwiseNot => Some(ConstExprUnaryOp::BitwiseNot),
            UnaryOperator::LogicalNot
            | UnaryOperator::AddressOf
            | UnaryOperator::MutAddressOf
            | UnaryOperator::MetaOf
            | UnaryOperator::PointerDeRef => None,
        }
    }

    fn const_expr_binary_op(&self, op: BinaryOperator) -> Option<ConstExprBinaryOp> {
        match op {
            BinaryOperator::Add => Some(ConstExprBinaryOp::Add),
            BinaryOperator::Subtract => Some(ConstExprBinaryOp::Subtract),
            BinaryOperator::Multiply => Some(ConstExprBinaryOp::Multiply),
            BinaryOperator::Divide => Some(ConstExprBinaryOp::Divide),
            BinaryOperator::Modulo => Some(ConstExprBinaryOp::Modulo),
            BinaryOperator::BitwiseAnd => Some(ConstExprBinaryOp::BitwiseAnd),
            BinaryOperator::BitwiseOr => Some(ConstExprBinaryOp::BitwiseOr),
            BinaryOperator::BitwiseXor => Some(ConstExprBinaryOp::BitwiseXor),
            BinaryOperator::ShiftLeft => Some(ConstExprBinaryOp::ShiftLeft),
            BinaryOperator::ShiftRight => Some(ConstExprBinaryOp::ShiftRight),
            BinaryOperator::Equal
            | BinaryOperator::NotEqual
            | BinaryOperator::LessThan
            | BinaryOperator::GreaterThan
            | BinaryOperator::LessOrEqual
            | BinaryOperator::GreaterOrEqual
            | BinaryOperator::LogicalAnd
            | BinaryOperator::LogicalOr => None,
        }
    }

    fn coerce_const_generic_value(
        &mut self,
        value: ConstValue,
        expected_ty: TypeId,
        span: Span,
        context: &str,
    ) -> Option<ConstGenericValue> {
        let norm = self.ctx.type_registry.normalize(expected_ty);
        let ty_name = self.ctx.ty_to_string(expected_ty);
        let norm_kind = self.ctx.type_registry.get(norm).clone();

        match self.coerce_payloadless_enum_const_generic_value(
            &value, norm, &norm_kind, span, context, &ty_name,
        ) {
            Ok(Some(tag)) => {
                return Some(ConstGenericValue {
                    ty: norm,
                    kind: ConstGenericValueKind::Int(tag),
                });
            }
            Ok(None) => {}
            Err(()) => return None,
        }

        let TypeKind::Primitive(primitive) = norm_kind else {
            self.ctx
                .struct_error(
                    span,
                    format!("{} must use a scalar const-generic type", context),
                )
                .emit();
            return None;
        };

        if primitive == PrimitiveType::Bool {
            let value = match value {
                ConstValue::Bool(value) => value,
                _ => {
                    self.ctx
                        .struct_error(span, format!("{} must evaluate to `bool`", context))
                        .with_hint(format!("this const generic expects `{}`", ty_name))
                        .emit();
                    return None;
                }
            };
            return Some(ConstGenericValue {
                ty: norm,
                kind: ConstGenericValueKind::Bool(value),
            });
        }

        let ConstValue::Int(value) = value else {
            self.ctx
                .struct_error(
                    span,
                    format!("{} must evaluate to an integer constant", context),
                )
                .with_hint(format!("this const generic expects `{}`", ty_name))
                .emit();
            return None;
        };

        let bit_width = LayoutEngine::new(self.ctx).compute_type_size(norm) * 8;

        let coerced = match primitive {
            PrimitiveType::U8
            | PrimitiveType::U16
            | PrimitiveType::U32
            | PrimitiveType::U64
            | PrimitiveType::U128
            | PrimitiveType::USize => {
                if value < 0 {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} cannot be negative for `{}`", context, ty_name),
                        )
                        .emit();
                    return None;
                }
                let max = if bit_width >= 128 {
                    u128::MAX
                } else {
                    (1u128 << bit_width) - 1
                };
                if (value as u128) > max {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} is out of range for `{}`", context, ty_name),
                        )
                        .with_hint(format!("maximum value here is {}", max))
                        .emit();
                    return None;
                }
                value
            }
            PrimitiveType::I8
            | PrimitiveType::I16
            | PrimitiveType::I32
            | PrimitiveType::I64
            | PrimitiveType::I128
            | PrimitiveType::ISize => {
                let (min, max) = if bit_width >= 128 {
                    (i128::MIN, i128::MAX)
                } else {
                    let max = (1i128 << (bit_width - 1)) - 1;
                    let min = -(1i128 << (bit_width - 1));
                    (min, max)
                };
                if value < min || value > max {
                    self.ctx
                        .struct_error(
                            span,
                            format!("{} is out of range for `{}`", context, ty_name),
                        )
                        .with_hint(format!("valid range here is {} to {}", min, max))
                        .emit();
                    return None;
                }
                value
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        format!("{} must currently use an integer or `bool` type", context),
                    )
                    .emit();
                return None;
            }
        };

        Some(ConstGenericValue {
            ty: norm,
            kind: ConstGenericValueKind::Int(coerced),
        })
    }

    fn coerce_payloadless_enum_const_generic_value(
        &mut self,
        value: &ConstValue,
        norm: TypeId,
        norm_kind: &TypeKind,
        span: Span,
        context: &str,
        ty_name: &str,
    ) -> Result<Option<i128>, ()> {
        let tag = match value {
            ConstValue::Enum { tag, payload } if payload.is_none() => *tag,
            ConstValue::Int(_) => {
                if matches!(norm_kind, TypeKind::Enum(_, _) | TypeKind::AnonymousEnum(_)) {
                    let example = self.enum_const_generic_example(norm, norm_kind);
                    let mut diagnostic = self.ctx.struct_error(
                        span,
                        format!(
                            "{} must evaluate to a value of enum type `{}`",
                            context, ty_name
                        ),
                    );
                    if let Some(example) = example {
                        diagnostic = diagnostic.with_hint(format!(
                            "write an explicit enum value such as `{}`",
                            example
                        ));
                    } else {
                        diagnostic = diagnostic.with_hint(
                            "write an explicit payload-less enum variant instead of a raw integer",
                        );
                    }
                    diagnostic.emit();
                    return Err(());
                }
                return Ok(None);
            }
            _ => return Ok(None),
        };

        let is_valid = match norm_kind {
            TypeKind::Enum(def_id, _) => match &self.ctx.defs[def_id.0 as usize] {
                Def::Enum(def) => {
                    let variants = def.variants.clone();
                    if variants
                        .iter()
                        .any(|variant| variant.payload_type.is_some())
                    {
                        self.ctx
                            .struct_error(
                                span,
                                format!(
                                    "{} cannot use enum `{}` as a const generic type because it has payload-carrying variants",
                                    context, ty_name
                                ),
                            )
                            .with_hint(
                                "only payload-less enums are currently supported as const generic value types",
                            )
                            .emit();
                        return Err(());
                    }

                    let mut current_tag = 0i128;
                    let mut matched = false;
                    for variant in &variants {
                        if let Some(value_expr) = &variant.value {
                            let mut evaluator = ConstEvaluator::new(self.ctx);
                            if let Ok(ConstValue::Int(value)) =
                                evaluator.eval_const_value(value_expr)
                            {
                                current_tag = value;
                            }
                        }
                        if current_tag == tag {
                            matched = true;
                            break;
                        }
                        current_tag += 1;
                    }
                    matched
                }
                _ => false,
            },
            TypeKind::AnonymousEnum(enum_def) => {
                if enum_def
                    .variants
                    .iter()
                    .any(|variant| variant.payload_ty.is_some())
                {
                    self.ctx
                        .struct_error(
                            span,
                            format!(
                                "{} cannot use enum `{}` as a const generic type because it has payload-carrying variants",
                                context, ty_name
                            ),
                        )
                        .with_hint(
                            "only payload-less enums are currently supported as const generic value types",
                        )
                        .emit();
                    return Err(());
                }

                let mut current_tag = 0i128;
                let mut matched = false;
                for variant in &enum_def.variants {
                    if let Some(value) = variant.explicit_value {
                        current_tag = value;
                    }
                    if current_tag == tag {
                        matched = true;
                        break;
                    }
                    current_tag += 1;
                }
                matched
            }
            _ => return Ok(None),
        };

        if !is_valid {
            self.ctx
                .struct_error(
                    span,
                    format!("{} is not a valid value for `{}`", context, ty_name),
                )
                .with_hint("use one of the declared payload-less enum variants")
                .emit();
            return Err(());
        }

        Ok(Some(tag))
    }

    fn enum_const_generic_example(&self, norm: TypeId, norm_kind: &TypeKind) -> Option<String> {
        match norm_kind {
            TypeKind::Enum(def_id, _) => {
                let def = self.ctx.defs.get(def_id.0 as usize)?;
                let Def::Enum(enum_def) = def else {
                    return None;
                };
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.payload_type.is_none())?;
                Some(format!(
                    "{}.{}",
                    self.ctx.ty_to_string(norm),
                    self.ctx.resolve(variant.name)
                ))
            }
            TypeKind::AnonymousEnum(enum_def) => {
                let variant = enum_def
                    .variants
                    .iter()
                    .find(|variant| variant.payload_ty.is_none())?;
                Some(format!(".{}", self.ctx.resolve(variant.name)))
            }
            _ => None,
        }
    }

    pub(crate) fn expr_references_const_param(
        &mut self,
        expr: &ast::Expr,
        env_scope: ScopeId,
    ) -> bool {
        self.ctx.scopes.set_current_scope(env_scope);
        match &expr.kind {
            ast::ExprKind::Identifier(name) => self
                .ctx
                .scopes
                .resolve(*name)
                .is_some_and(|info| info.kind == SymbolKind::ConstParam),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(rhs, env_scope)
            }
            ast::ExprKind::Unary { operand, .. }
            | ast::ExprKind::FieldAccess { lhs: operand, .. }
            | ast::ExprKind::Return(Some(operand))
            | ast::ExprKind::Defer { expr: operand }
            | ast::ExprKind::As { lhs: operand, .. }
            | ast::ExprKind::Propagate { operand, .. } => {
                self.expr_references_const_param(operand, env_scope)
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(index, env_scope)
            }
            ast::ExprKind::Call { callee, args } => {
                self.expr_references_const_param(callee, env_scope)
                    || args
                        .iter()
                        .any(|arg| self.expr_references_const_param(arg, env_scope))
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => fields
                    .iter()
                    .any(|field| self.expr_references_const_param(&field.value, env_scope)),
                ast::DataLiteralKind::Array(items) => items
                    .iter()
                    .any(|item| self.expr_references_const_param(item, env_scope)),
                ast::DataLiteralKind::Repeat { value, count } => {
                    self.expr_references_const_param(value, env_scope)
                        || self.expr_references_const_param(count, env_scope)
                }
                ast::DataLiteralKind::Scalar(inner) => {
                    self.expr_references_const_param(inner, env_scope)
                }
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr_references_const_param(cond, env_scope)
                    || self.expr_references_const_param(then_branch, env_scope)
                    || else_branch
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::Match { target, arms } => {
                self.expr_references_const_param(target, env_scope)
                    || arms
                        .iter()
                        .any(|arm| self.expr_references_const_param(&arm.body, env_scope))
            }
            ast::ExprKind::Block { stmts, result } => {
                stmts.iter().any(|stmt| match &stmt.kind {
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        self.expr_references_const_param(expr, env_scope)
                    }
                    ast::StmtKind::Use(_) => false,
                }) || result
                    .as_deref()
                    .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                init.as_deref()
                    .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
                    || cond
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
                    || post
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
                    || self.expr_references_const_param(body, env_scope)
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.expr_references_const_param(lhs, env_scope)
                    || start
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
                    || end
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.expr_references_const_param(lhs, env_scope)
                    || self.expr_references_const_param(rhs, env_scope)
            }
            ast::ExprKind::Let {
                init, else_branch, ..
            } => {
                self.expr_references_const_param(init, env_scope)
                    || else_branch
                        .as_deref()
                        .is_some_and(|expr| self.expr_references_const_param(expr, env_scope))
            }
            ast::ExprKind::Static { init, .. } => self.expr_references_const_param(init, env_scope),
            ast::ExprKind::GenericInstantiation { target, args } => {
                self.expr_references_const_param(target, env_scope)
                    || args.iter().any(|arg| match arg {
                        ast::GenericArg::Type(ty) => self
                            .reinterpret_type_arg_as_const_expr(ty)
                            .is_some_and(|expr| self.expr_references_const_param(&expr, env_scope)),
                        ast::GenericArg::AssocBinding { .. } => false,
                        ast::GenericArg::ConstExpr(expr) => {
                            self.expr_references_const_param(expr, env_scope)
                        }
                    })
            }
            ast::ExprKind::Closure { body, .. } => {
                self.expr_references_const_param(body, env_scope)
            }
            ast::ExprKind::AnchoredPath { .. }
            | ast::ExprKind::TypeNode(_)
            | ast::ExprKind::Integer(_)
            | ast::ExprKind::Float(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::Char(_)
            | ast::ExprKind::ByteChar(_)
            | ast::ExprKind::String(_)
            | ast::ExprKind::EnumLiteral { .. }
            | ast::ExprKind::Break
            | ast::ExprKind::Continue
            | ast::ExprKind::Return(None)
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer
            | ast::ExprKind::SelfValue => false,
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn resolve_builtin_primitive(&mut self, name: &str) -> Option<TypeId> {
        let scalar = match name {
            "void" => Some(TypeId::VOID),
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            "str" => Some(TypeId::STR),
            "never" => Some(TypeId::NEVER),
            _ => None,
        };

        if scalar.is_some() {
            return scalar;
        }

        self.parse_builtin_simd(name)
    }

    fn parse_builtin_simd(&mut self, name: &str) -> Option<TypeId> {
        let (base, lanes) = name.rsplit_once('x')?;
        let lanes: u16 = lanes.parse().ok()?;
        if lanes == 0 {
            return None;
        }

        let elem = match base {
            "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8),
            "i16" => Some(TypeId::I16),
            "i32" => Some(TypeId::I32),
            "i64" => Some(TypeId::I64),
            "i128" => Some(TypeId::I128),
            "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8),
            "u16" => Some(TypeId::U16),
            "u32" => Some(TypeId::U32),
            "u64" => Some(TypeId::U64),
            "u128" => Some(TypeId::U128),
            "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32),
            "f64" => Some(TypeId::F64),
            _ => None,
        }?;

        Some(
            self.ctx
                .type_registry
                .intern(TypeKind::Simd { elem, lanes }),
        )
    }
}
