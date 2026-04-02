use crate::SemaContext;
use crate::checker::{ConstEvaluator, ExprChecker, Substituter};
use crate::def::*;
use crate::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::ty::{AnonymousEnum, AnonymousField, AnonymousVariant, TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;

pub struct TypeResolver<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
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

    /// 执行完整的类型解析 Pass (Two-Pass 架构)
    pub fn resolve_all(&mut self) {
        let module_ids = self.collect_module_ids();
        self.resolve_module_pass(&module_ids, true);
        self.resolve_module_pass(&module_ids, false);
    }

    fn collect_module_ids(&self) -> Vec<DefId> {
        self.ctx
            .defs
            .iter()
            .filter_map(|def| {
                if let Def::Module(m) = def {
                    Some(m.id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn resolve_module_pass(&mut self, module_ids: &[DefId], aliases_only: bool) {
        for &mod_id in module_ids {
            let Some((mod_scope, items)) = self.module_scope_and_items(mod_id) else {
                continue;
            };

            for item_id in items {
                let is_alias = matches!(self.ctx.defs[item_id.0 as usize], Def::TypeAlias(_));
                if aliases_only == is_alias {
                    self.resolve_item(item_id, mod_scope);
                }
            }
        }
    }

    fn module_scope_and_items(&mut self, mod_id: DefId) -> Option<(ScopeId, Vec<DefId>)> {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            Some((m.scope_id, m.items.clone()))
        } else {
            self.ctx.emit_ice(
                Span::default(),
                format!("TypeResolver expected DefId {:?} to be a module", mod_id),
            );
            None
        }
    }

    fn resolve_item(&mut self, item_id: DefId, parent_scope: ScopeId) {
        let def = self.ctx.defs[item_id.0 as usize].clone();

        match &def {
            Def::Function(f) => self.resolve_function_item(item_id, f, parent_scope),
            Def::Struct(s) => self.resolve_struct_item(item_id, s, parent_scope),
            Def::Union(u) => self.resolve_union_item(item_id, u, parent_scope),
            Def::Trait(t) => self.resolve_trait_item(item_id, t, parent_scope),
            Def::TypeAlias(t) => self.resolve_type_alias_item(t, parent_scope),
            Def::Impl(i) => self.resolve_impl_item(i, parent_scope),
            Def::Global(_) => {}
            Def::Enum(a) => self.resolve_enum_item(item_id, a, parent_scope),
            _ => {}
        }
    }

    fn resolve_function_item(&mut self, item_id: DefId, f: &FunctionDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let func_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&f.generics, func_scope);
        self.resolve_where_clauses(&f.where_clauses, func_scope);
        if let Some(parent_id) = f.parent
            && let Def::Impl(i) = &self.ctx.defs[parent_id.0 as usize]
        {
            let target_ty = self
                .ctx
                .node_types
                .get(&i.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            self.bind_self_type(target_ty, func_scope, f.span);
        }

        let mut param_tys = Vec::new();
        for param in &f.params {
            let p_ty = self.resolve_type(&param.type_node, func_scope);
            self.ensure_sized(p_ty, param.type_node.span);
            param_tys.push(p_ty);
        }
        let ret_ty = self.resolve_type(&f.ret_type, func_scope);
        if ret_ty != TypeId::VOID {
            self.ensure_sized(ret_ty, f.ret_type.span);
        }

        let sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: param_tys,
            ret: ret_ty,
            is_variadic: f.is_variadic,
        });

        if let Def::Function(mut updated_f) = self.ctx.defs[item_id.0 as usize].clone() {
            updated_f.resolved_sig = Some(sig_ty);
            self.ctx.defs[item_id.0 as usize] = Def::Function(updated_f);
        }

        self.ctx.scopes.exit_scope();

        let gen_args = f
            .generics
            .iter()
            .map(|param| self.ctx.type_registry.intern(TypeKind::Param(param.name)))
            .collect();
        let fn_def_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::FnDef(item_id, gen_args));

        self.ctx.scopes.set_current_scope(parent_scope);

        let is_impl_method = f
            .parent
            .is_some_and(|p_id| matches!(self.ctx.defs[p_id.0 as usize], Def::Impl(_)));
        if !is_impl_method {
            self.ctx.scopes.update_type(f.name, fn_def_ty);
        }
    }

    fn resolve_struct_item(&mut self, item_id: DefId, s: &StructDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let struct_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&s.generics, struct_scope);
        self.resolve_where_clauses(&s.where_clauses, struct_scope);

        for field in &s.fields {
            let f_ty = self.resolve_type(&field.type_node, struct_scope);
            self.ensure_sized(f_ty, field.type_node.span);
            if let Some(def_val) = &field.default_value {
                self.resolve_expr(def_val, struct_scope);
            }
        }
        self.ctx.scopes.exit_scope();

        let struct_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(item_id, Vec::new()));
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(s.name, struct_ty);
    }

    fn resolve_union_item(&mut self, item_id: DefId, u: &UnionDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let union_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&u.generics, union_scope);
        self.resolve_where_clauses(&u.where_clauses, union_scope);

        for field in &u.fields {
            let f_ty = self.resolve_type(&field.type_node, union_scope);
            self.ensure_sized(f_ty, field.type_node.span);
            if let Some(def_val) = &field.default_value {
                self.resolve_expr(def_val, union_scope);
            }
        }
        self.ctx.scopes.exit_scope();

        let union_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Def(item_id, Vec::new()));
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(u.name, union_ty);
    }

    fn resolve_trait_item(&mut self, item_id: DefId, t: &TraitDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let trait_scope = self.ctx.scopes.enter_scope();

        let self_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::TraitObject(item_id, vec![]));
        self.bind_self_type(self_ty, trait_scope, t.span);

        self.bind_generics(&t.generics, trait_scope);
        self.resolve_where_clauses(&t.where_clauses, trait_scope);

        let mut resolved_supertraits = Vec::new();
        for supertrait in &t.supertraits {
            resolved_supertraits.push(self.resolve_type(supertrait, trait_scope));
        }

        let mut resolved_methods = Vec::new();
        for method in &t.methods {
            let sig_ty = self.resolve_type(&method.type_node, trait_scope);
            resolved_methods.push((method.name, sig_ty));
        }
        self.ctx.scopes.exit_scope();

        if let Def::Trait(mut updated_t) = self.ctx.defs[item_id.0 as usize].clone() {
            updated_t.resolved_methods = resolved_methods;
            updated_t.resolved_supertraits = resolved_supertraits;
            self.ctx.defs[item_id.0 as usize] = Def::Trait(updated_t);
        }
    }

    fn resolve_type_alias_item(&mut self, t: &TypeAliasDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let alias_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&t.generics, alias_scope);
        self.resolve_where_clauses(&t.where_clauses, alias_scope);
        let target_ty = self.resolve_type(&t.target, alias_scope);

        self.ctx.scopes.exit_scope();
        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(t.name, target_ty);
    }

    fn resolve_impl_item(&mut self, i: &ImplDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let impl_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&i.generics, impl_scope);
        self.resolve_where_clauses(&i.where_clauses, impl_scope);

        let target_ty_id = self.resolve_type(&i.target_type, impl_scope);
        self.bind_self_type(target_ty_id, impl_scope, i.span);

        if let Some(trait_ty) = &i.trait_type {
            self.resolve_type(trait_ty, impl_scope);
        }

        for &method_id in &i.methods {
            self.resolve_item(method_id, impl_scope);
        }

        self.ctx.scopes.exit_scope();
    }

    fn resolve_enum_item(&mut self, item_id: DefId, a: &EnumDef, parent_scope: ScopeId) {
        self.ctx.scopes.set_current_scope(parent_scope);
        let adt_scope = self.ctx.scopes.enter_scope();

        self.bind_generics(&a.generics, adt_scope);
        self.resolve_where_clauses(&a.where_clauses, adt_scope);

        if let Some(backing_ty) = &a.backing_type {
            let resolved_ty = self.resolve_type(backing_ty, adt_scope);
            if !self.ctx.type_registry.is_integer(resolved_ty) && resolved_ty != TypeId::ERROR {
                self.ctx
                    .emit_error(backing_ty.span, "Enum backing type must be an integer");
            }
        }

        for variant in &a.variants {
            if let Some(payload_ty) = &variant.payload_type {
                self.resolve_type(payload_ty, adt_scope);
            }
        }

        self.ctx.scopes.exit_scope();

        let adt_ty = self
            .ctx
            .type_registry
            .intern(TypeKind::Enum(item_id, Vec::new()));

        self.ctx.scopes.set_current_scope(parent_scope);
        self.ctx.scopes.update_type(a.name, adt_ty);
    }

    // ==========================================
    //          核心类型转换逻辑
    // ==========================================

    /// 将 AST TypeNode 转换为语义 TypeId
    pub fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        // 优先检查是否已被 ExprChecker 现场推导过
        // 用于实现 @typeOf 的动态求类型
        if let Some(&cached_ty) = self.ctx.node_types.get(&ty_node.id)
            && cached_ty != TypeId::ERROR
        {
            return cached_ty;
        }

        let ty_id = match &ty_node.kind {
            ast::TypeKind::Path {
                segments, generics, ..
            } => self.resolve_path_type(segments, generics, env_scope, ty_node.span),
            ast::TypeKind::Void => TypeId::VOID,

            // 内联的匿名结构体
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
                // 占位
                self.resolve_expr(expr, env_scope);
                TypeId::ERROR
            }
            // Struct/Enum/Union/Trait 在这里不会直接作为匿名类型出现 (已被 Collect 提取)
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

    // 递归查找并解析表达式内部的所有 TypeNode
    fn resolve_pattern(&mut self, pattern: &ast::Pattern, scope: ScopeId) {
        match &pattern.kind {
            ast::PatternKind::Binding(_) | ast::PatternKind::Ignore | ast::PatternKind::Variant(_) => {
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
                else_branch,
            } => {
                self.resolve_pattern(&pattern.pattern, scope);
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
                self.resolve_type(target, scope); // 捕获 TypeNode
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
                // 捕获泛型实参
                for ty in types {
                    self.resolve_type(ty, scope);
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                // 捕获 Elided Initialization 的前缀类型
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

            // 所有叶子节点 (Identifier, Int, EnumLiteral, Break 等) 直接忽略
            _ => {}
        }
    }

    /// 严格的路径类型解析 (支持 `module.submodule.Type[Generic]`)
    fn resolve_path_type(
        &mut self,
        segments: &[SymbolId],
        generics: &[ast::TypeNode],
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        if segments.is_empty() {
            return TypeId::ERROR;
        }

        let mut curr_scope = env_scope;
        let mut target_symbol = None;

        // 逐级解析路径
        for (i, &segment) in segments.iter().enumerate() {
            if i == 0 {
                // 第一段：如果只有一段，优先检查内置基础类型
                if segments.len() == 1 {
                    let name_str = self.ctx.resolve(segment);
                    if let Some(prim_id) = self.resolve_builtin_primitive(name_str) {
                        if !generics.is_empty() {
                            self.ctx
                                .emit_error(span, "Primitive types do not take generic arguments");
                        }
                        return prim_id;
                    }
                }

                // 沿着作用域树向上查找
                self.ctx.scopes.set_current_scope(curr_scope);
                target_symbol = self.ctx.scopes.resolve(segment).cloned();
            } else {
                // 后续段：严格只在前一个模块的内部作用域中查找
                target_symbol = self.ctx.scopes.resolve_in(curr_scope, segment).cloned();
            }

            let sym = match target_symbol.as_ref() {
                Some(s) => s,
                None => {
                    let name = self.ctx.resolve(segment).to_string();
                    if i == 0 {
                        self.ctx
                            .emit_error(span, format!("Cannot find type `{}` in this scope", name));
                    } else {
                        self.ctx.emit_error(
                            span,
                            format!("Cannot find `{}` in the target module", name),
                        );
                    }
                    return TypeId::ERROR;
                }
            };

            // 如果还没到最后一段，当前符号必须是个模块
            if i < segments.len() - 1 {
                if sym.kind == SymbolKind::Module {
                    let Some(mod_def_id) =
                        self.required_def_id(sym, span, "module path segment", segment)
                    else {
                        return TypeId::ERROR;
                    };
                    let Some(module_scope) = self.module_scope_from_def(mod_def_id, span, segment)
                    else {
                        return TypeId::ERROR;
                    };
                    curr_scope = module_scope;
                } else {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx
                        .emit_error(span, format!("`{}` is not a module", name));
                    return TypeId::ERROR;
                }
            }
        }

        let Some(final_sym) = target_symbol else {
            self.ctx.emit_ice(
                span,
                "Type path resolution reached the end of a non-empty path without a final symbol",
            );
            return TypeId::ERROR;
        };

        // 解析附带的泛型参数 (在原始的作用域中解析)
        let mut resolved_generics = Vec::with_capacity(generics.len());
        for gen_ast in generics {
            resolved_generics.push(self.resolve_type(gen_ast, env_scope));
        }

        // 验证最终符号的类型
        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union => {
                let Some(def_id) = self.required_def_id(&final_sym, span, "type", segments[0])
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
                let Some(def_id) = self.required_def_id(&final_sym, span, "enum type", segments[0])
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
                    self.required_def_id(&final_sym, span, "trait object type", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::TraitObject(def_id, resolved_generics))
            }
            SymbolKind::TypeParam => {
                if !resolved_generics.is_empty() {
                    self.ctx
                        .emit_error(span, "Type parameters cannot take generic arguments");
                }
                final_sym.type_id // 直接返回 Param(SymbolId)
            }
            SymbolKind::TypeAlias => {
                // 如果是编译器虚拟注入的泛型参数 T 或者 Self，它们没有物理 Def
                // 直接返回在注入时就准备好的 type_id即可
                if final_sym.def_id.is_none() {
                    return final_sym.type_id;
                }
                let Some(def_id) =
                    self.required_def_id(&final_sym, span, "type alias", segments[0])
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }

                // 动态获取最新解析的 AST 类型，不要用 Import 克隆带来的陈旧 final_sym.type_id
                let target_ty = if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    self.ctx
                        .node_types
                        .get(&t_def.target.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR)
                } else {
                    TypeId::ERROR
                };

                // 防止因循环依赖或解析顺序导致的静默 ERROR 污染 AST
                if target_ty == TypeId::ERROR {
                    let name = self.last_segment_name(segments);
                    self.ctx.struct_error(span, format!("type alias `{}` could not be resolved", name))
                        .with_hint("this might be caused by an invalid circular alias dependency or use before resolution")
                        .emit();
                    return TypeId::ERROR;
                }

                if resolved_generics.is_empty() {
                    // 没有传入泛型，直接穿透返回
                    target_ty
                } else {
                    // 获取别名的定义以提取泛型名字
                    if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                        if t_def.generics.len() != resolved_generics.len() {
                            self.ctx.emit_error(
                                span,
                                format!(
                                    "Type alias `{}` expects {} generic arguments, but {} were provided",
                                    self.last_segment_name(segments),
                                    t_def.generics.len(),
                                    resolved_generics.len()
                                ),
                            );
                            return TypeId::ERROR;
                        }

                        // 构造映射字典并执行替换
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
                                self.last_segment_name(segments),
                                def_id
                            ),
                        );
                        TypeId::ERROR
                    }
                }
            }
            _ => {
                let name = self.last_segment_name(segments);
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

    // ==========================================
    //               Helpers
    // ==========================================

    fn resolve_builtin_primitive(&self, name: &str) -> Option<TypeId> {
        match name {
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
        }
    }

    fn check_type_generic_bounds(&mut self, span: Span, def_id: DefId, arg_tys: &[TypeId]) -> bool {
        let Some((item_name, generics, where_clauses, kind_name)) =
            self.generic_def_bounds_info(def_id)
        else {
            return true;
        };

        if generics.len() != arg_tys.len() {
            self.ctx.emit_error(
                span,
                format!(
                    "{} `{}` expects {} generic arguments, but {} were provided",
                    kind_name,
                    item_name,
                    generics.len(),
                    arg_tys.len()
                ),
            );
            return false;
        }

        if arg_tys
            .iter()
            .any(|&ty| ty == TypeId::ERROR || self.type_contains_params(ty))
        {
            return true;
        }

        if where_clauses.is_empty() {
            return true;
        }

        let mut map = HashMap::new();
        for (param, arg_ty) in generics.iter().zip(arg_tys.iter()) {
            map.insert(param.name, *arg_ty);
        }

        let mut pairs_to_check = Vec::new();
        {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            for clause in where_clauses {
                let original_target = self
                    .ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_target = subst.substitute(original_target);

                for bound_ast in clause.bounds {
                    let original_bound = self
                        .ctx
                        .node_types
                        .get(&bound_ast.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let sub_bound = subst.substitute(original_bound);
                    pairs_to_check.push((sub_target, sub_bound));
                }
            }
        }

        let mut ok = true;
        for (sub_target, sub_bound) in pairs_to_check {
            if sub_target == TypeId::ERROR || sub_bound == TypeId::ERROR {
                ok = false;
                continue;
            }

            let bound_ok = {
                let mut checker = ExprChecker::new(self.ctx, None);
                checker.check_trait_impl(sub_target, sub_bound)
            };

            if !bound_ok {
                ok = false;
                let target_str = self.ctx.ty_to_string(sub_target);
                let bound_str = self.ctx.ty_to_string(sub_bound);
                self.ctx
                    .struct_error(span, "type does not satisfy trait bounds")
                    .with_hint(format!("required bound: `{}: {}`", target_str, bound_str))
                    .emit();
            }
        }

        ok
    }

    fn generic_def_bounds_info(
        &self,
        def_id: DefId,
    ) -> Option<(
        String,
        Vec<ast::GenericParam>,
        Vec<ast::WhereClause>,
        &'static str,
    )> {
        match &self.ctx.defs[def_id.0 as usize] {
            Def::Struct(s) => Some((
                self.ctx.resolve(s.name).to_string(),
                s.generics.clone(),
                s.where_clauses.clone(),
                "struct",
            )),
            Def::Union(u) => Some((
                self.ctx.resolve(u.name).to_string(),
                u.generics.clone(),
                u.where_clauses.clone(),
                "union",
            )),
            Def::Enum(e) => Some((
                self.ctx.resolve(e.name).to_string(),
                e.generics.clone(),
                e.where_clauses.clone(),
                "enum",
            )),
            Def::Trait(t) => Some((
                self.ctx.resolve(t.name).to_string(),
                t.generics.clone(),
                t.where_clauses.clone(),
                "trait",
            )),
            Def::TypeAlias(t) => Some((
                self.ctx.resolve(t.name).to_string(),
                t.generics.clone(),
                t.where_clauses.clone(),
                "type alias",
            )),
            _ => None,
        }
    }

    fn type_contains_params(&mut self, ty: TypeId) -> bool {
        let norm = self.ctx.type_registry.normalize(ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Param(_) | TypeKind::TypeVar(_) => true,
            TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Alias(_, elem)
            | TypeKind::AnonymousEnumPayload(elem) => self.type_contains_params(elem),
            TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem, .. } => {
                self.type_contains_params(elem)
            }
            TypeKind::Def(_, args)
            | TypeKind::Enum(_, args)
            | TypeKind::TraitObject(_, args)
            | TypeKind::FnDef(_, args) => {
                args.into_iter().any(|arg| self.type_contains_params(arg))
            }
            TypeKind::Function { params, ret, .. } | TypeKind::ClosureInterface { params, ret } => {
                params
                    .into_iter()
                    .any(|param| self.type_contains_params(param))
                    || self.type_contains_params(ret)
            }
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures
                    .into_iter()
                    .any(|capture| self.type_contains_params(capture))
                    || params
                        .into_iter()
                        .any(|param| self.type_contains_params(param))
                    || self.type_contains_params(ret)
            }
            TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => fields
                .into_iter()
                .any(|field| self.type_contains_params(field.ty)),
            TypeKind::AnonymousEnum(enum_def) => enum_def.variants.into_iter().any(|variant| {
                variant
                    .payload_ty
                    .is_some_and(|payload_ty| self.type_contains_params(payload_ty))
            }),
            _ => false,
        }
    }

    fn required_def_id(
        &mut self,
        symbol: &SymbolInfo,
        span: Span,
        context: &str,
        segment: SymbolId,
    ) -> Option<DefId> {
        if let Some(def_id) = symbol.def_id {
            Some(def_id)
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Resolved {} `{}` is missing a DefId",
                    context,
                    self.ctx.resolve(segment)
                ),
            );
            None
        }
    }

    fn module_scope_from_def(
        &mut self,
        def_id: DefId,
        span: Span,
        segment: SymbolId,
    ) -> Option<ScopeId> {
        if let Def::Module(m) = &self.ctx.defs[def_id.0 as usize] {
            Some(m.scope_id)
        } else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Resolved module path segment `{}` points to non-module def {:?}",
                    self.ctx.resolve(segment),
                    def_id
                ),
            );
            None
        }
    }

    fn last_segment_name(&self, segments: &[SymbolId]) -> String {
        segments
            .last()
            .map(|sym| self.ctx.resolve(*sym).to_string())
            .unwrap_or_else(|| "<empty-path>".to_string())
    }

    fn bind_generics(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);

        // 把所有的泛型参数名注入作用域
        for param in generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let info = SymbolInfo {
                kind: SymbolKind::TypeParam,
                node_id: self.ctx.next_node_id(),
                type_id: param_ty,
                def_id: None,
                span: param.span,
                is_pub: false,
                is_mut: false,
            };
            let _ = self.ctx.scopes.define(param.name, info);
        }
    }

    /// 解析 where 子句中的所有 TypeNode，确保它们被注册到 ctx.node_types 中
    fn resolve_where_clauses(&mut self, clauses: &[ast::WhereClause], scope: ScopeId) {
        for clause in clauses {
            // 解析左侧目标类型 (例如 *mut T)
            self.resolve_type(&clause.target_ty, scope);
            // 解析右侧的所有 Trait 约束
            for bound in &clause.bounds {
                self.resolve_type(bound, scope);
            }
        }
    }

    fn bind_self_type(&mut self, target_ty: TypeId, scope: ScopeId, span: Span) {
        self.ctx.scopes.set_current_scope(scope);
        let self_sym = self.ctx.intern("Self");
        let info = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            node_id: self.ctx.next_node_id(),
            type_id: target_ty,
            def_id: None,
            span,
            is_pub: false,
            is_mut: false,
        };
        // 允许重复定义（覆盖外部可能存在的同名绑定）
        let _ = self.ctx.scopes.define(self_sym, info);
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable",
            SymbolKind::Const => "constant",
            SymbolKind::Static => "static variable",
            SymbolKind::Function => "function",
            SymbolKind::Module => "module",
            SymbolKind::Struct => "struct",
            SymbolKind::Union => "union",
            SymbolKind::Enum => "algebraic data type",
            SymbolKind::Trait => "trait",
            SymbolKind::TypeAlias => "type alias",
            SymbolKind::TypeParam => "type parameter",
        }
    }

    fn ensure_sized(&mut self, ty: TypeId, span: Span) {
        let norm = self.ctx.type_registry.normalize(ty);
        if matches!(self.ctx.type_registry.get(norm), TypeKind::TraitObject(..)) {
            self.ctx.struct_error(span, "trait objects have dynamic size and cannot be used as naked types")
                .with_hint("in Kern, you must explicitly use a pointer for dynamic dispatch, e.g., `*Trait` or `*mut Trait`")
                .emit();
        }
    }
}
