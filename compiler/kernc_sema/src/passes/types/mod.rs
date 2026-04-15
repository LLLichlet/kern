use crate::SemaContext;
use crate::checker::{ConstEvaluator, ExprChecker, Substituter};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{
    AnonymousEnum, AnonymousField, AnonymousVariant, BuiltinAnonymousEnumKind, TypeId, TypeKind,
};
use kernc_ast::{self as ast, Visibility};
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

mod helper;
mod items;

pub struct TypeResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

struct PendingTraitProjection {
    trait_def_id: DefId,
    trait_args: Vec<TypeId>,
    assoc_bindings: Vec<(DefId, TypeId)>,
}

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
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
                self.ctx.scopes.set_current_scope(env_scope);
                let mut evaluator = ConstEvaluator::new(self.ctx);
                let Ok(length) = evaluator.eval_usize(len) else {
                    return TypeId::ERROR;
                };
                if length > u32::MAX as u64 {
                    self.ctx
                        .struct_error(
                            len.span,
                            format!(
                                "array length {} exceeds the current compiler limit of {} elements",
                                length,
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
                    len: length,
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

    fn make_builtin_optional_type(&mut self, inner_ty: TypeId, span: Span) -> TypeId {
        let inner_norm = self.ctx.type_registry.normalize(inner_ty);
        if matches!(
            self.ctx.type_registry.get(inner_norm),
            TypeKind::VolatilePtr { .. }
        ) {
            self.ctx
                .struct_error(
                    span,
                    "`?^T` is not a valid type; `^T` already covers raw address `0`",
                )
                .with_hint("use `^T` for raw addresses or `?*T` for nullable object pointers")
                .emit();
            return TypeId::ERROR;
        }

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
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                            self.resolve_expr(e, scope);
                        }
                    }
                }
                if let Some(r) = result {
                    self.resolve_expr(r, scope);
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
            ast::ExprKind::GenericInstantiation { target, types } => {
                self.resolve_expr(target, scope);
                // Resolve generic arguments.
                for ty in types {
                    self.resolve_type(ty, scope);
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
                let target_symbol = if index == 0 {
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
                        self.ctx
                            .scopes
                            .resolve_in(curr_scope, segment.name)
                            .cloned()
                    } else {
                        self.ctx.scopes.set_current_scope(curr_scope);
                        self.ctx.scopes.resolve(segment.name).cloned()
                    }
                } else {
                    self.ctx
                        .scopes
                        .resolve_in(curr_scope, segment.name)
                        .cloned()
                };

                let Some(sym) = target_symbol else {
                    let name = self.ctx.resolve(segment.name).to_string();
                    if index == 0 {
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
            if trait_args == [TypeId::ERROR] {
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

    fn resolve_named_type_symbol(
        &mut self,
        final_sym: &crate::scope::SymbolInfo,
        segment: &ast::TypePathSegment,
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        let (resolved_generics, resolved_assoc_bindings) =
            self.resolve_type_args(&segment.args, env_scope);

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
        args: &[ast::TypeArg],
        env_scope: ScopeId,
    ) -> (Vec<TypeId>, Vec<(SymbolId, TypeId)>) {
        let mut positional = Vec::new();
        let mut assoc_bindings = Vec::new();
        for arg in args {
            match arg {
                ast::TypeArg::Positional(ty) => positional.push(self.resolve_type(ty, env_scope)),
                ast::TypeArg::AssocBinding { name, value, .. } => {
                    assoc_bindings.push((*name, self.resolve_type(value, env_scope)));
                }
            }
        }
        (positional, assoc_bindings)
    }

    fn resolve_trait_segment_args(
        &mut self,
        trait_def_id: DefId,
        args: &[ast::TypeArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<TypeId>, Vec<(DefId, TypeId)>) {
        let (resolved_generics, resolved_assoc_bindings) = self.resolve_type_args(args, env_scope);
        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };
        if !self.check_type_generic_bounds(span, trait_def_id, &resolved_generics) {
            return (vec![TypeId::ERROR], Vec::new());
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
        trait_args: Vec<TypeId>,
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

        let (assoc_args, nested_assoc_bindings) = self.resolve_type_args(&segment.args, env_scope);
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
