use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

struct TraitMethodLookup {
    owner_trait_ty: TypeId,
    method_ty: TypeId,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    fn global_owner_scope(&self, def_id: DefId) -> Option<crate::scope::ScopeId> {
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

    fn trait_def_for_access(
        &mut self,
        trait_def_id: DefId,
        span: Span,
    ) -> Option<crate::def::TraitDef> {
        match self.ctx.defs.get(trait_def_id.0 as usize).cloned() {
            Some(Def::Trait(def)) => Some(def),
            Some(other) => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Expected trait definition during member lookup, found {:?}.",
                        other
                    ),
                );
                None
            }
            None => {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Missing DefId {} during trait member lookup.",
                        trait_def_id.0
                    ),
                );
                None
            }
        }
    }

    pub(crate) fn check_identifier(&mut self, name: SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name).cloned() {
            if info.kind == SymbolKind::Function {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(info.def_id.unwrap(), vec![]));
            }
            // 如果是模块，显式包装并返回 TypeKind::Module
            if info.kind == SymbolKind::Module {
                return self
                    .ctx
                    .type_registry
                    .intern(TypeKind::Module(info.def_id.unwrap()));
            }

            // 处理 `use` 导入或乱序声明的常量/静态变量的按需推导
            if info.type_id == TypeId::ERROR
                && let Some(def_id) = info.def_id
            {
                let global_expr_opt = if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                    Some(g.value.clone())
                } else {
                    None
                };

                if let Some(g_expr) = global_expr_opt {
                    if let Some(&actual_ty) = self.ctx.node_types.get(&g_expr.id) {
                        return actual_ty;
                    }
                    let prev_scope = self.ctx.scopes.current_scope_id();
                    if let Some(owner_scope) = self.global_owner_scope(def_id) {
                        self.ctx.scopes.set_current_scope(owner_scope);
                    }
                    let computed_ty = self.check_expr(&g_expr, None);
                    if let Some(prev_scope) = prev_scope {
                        self.ctx.scopes.set_current_scope(prev_scope);
                    }
                    return computed_ty;
                }
            }

            info.type_id
        } else {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx
                .struct_error(span, format!("use of undeclared identifier `{}`", name_str))
                .with_hint("make sure the variable or function is defined before using it")
                .emit();
            TypeId::ERROR
        }
    }

    pub(crate) fn check_self_value(&mut self, span: Span) -> TypeId {
        let self_var = self.ctx.intern("self");
        let self_type = self.ctx.intern("Self");

        if let Some(info) = self.ctx.scopes.resolve(self_var) {
            info.type_id
        } else if let Some(info) = self.ctx.scopes.resolve(self_type) {
            info.type_id
        } else {
            self.ctx
                .struct_error(span, "`self` is not available in this context")
                .with_hint("the `self` keyword is only valid inside method implementations")
                .emit();
            TypeId::ERROR
        }
    }

    pub(crate) fn check_let_or_static(
        &mut self,
        node_id: NodeId,
        pattern: &ast::BindingPattern,
        init: &Expr,
        expected_ty: Option<TypeId>,
        is_static: bool,
        span: Span,
    ) -> TypeId {
        let init_ty = self.check_expr(init, expected_ty);
        let norm_init = self.resolve_tv(init_ty);
        if matches!(
            self.ctx.type_registry.get(norm_init),
            TypeKind::TraitObject(..)
        ) {
            self.ctx
                .struct_error(span, "cannot store a naked trait object in a variable")
                .with_hint(
                    "trait objects are dynamically sized; store a pointer (`*mut Trait`) instead",
                )
                .emit();
        }
        let sym_kind = if is_static {
            SymbolKind::Static
        } else {
            SymbolKind::Var
        };

        let info = SymbolInfo {
            kind: sym_kind,
            node_id,
            type_id: init_ty,
            def_id: None,
            span,
            is_pub: false,
            is_mut: pattern.is_mut,
        };
        let _ = self.ctx.scopes.define(pattern.name, info);
        TypeId::VOID
    }

    pub(crate) fn check_index_access(
        &mut self,
        lhs: &Expr,
        index: &Expr,
        is_mut: bool,
        span: Span,
    ) -> TypeId {
        if is_mut {
            self.ctx
                .struct_error(
                    span,
                    "mutable indexing `..[]` is not supported for single elements",
                )
                .with_hint(
                    "use standard indexing `.[]` instead. Mutability is inherited automatically.",
                )
                .emit();
        }

        let lhs_ty = self.check_expr(lhs, None);
        let idx_ty = self.check_expr(index, Some(TypeId::USIZE));

        let norm_idx = self.resolve_tv(idx_ty);
        if !self.ctx.type_registry.is_integer(norm_idx) && norm_idx != TypeId::ERROR {
            self.ctx
                .struct_error(index.span, "index must be an integer type")
                .emit();
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. } | TypeKind::Slice { elem, .. } => elem,
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx
                    .struct_error(lhs.span, "cannot index into a non-array/non-slice type")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    pub(crate) fn check_field_access(
        &mut self,
        expr_id: NodeId,
        lhs: &Expr,
        field: SymbolId,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let lhs_norm = self.resolve_tv(lhs_ty);

        // 1. 获取解引用后的基础类型（Struct/Union/Enum/Module），仅用于模块判定和最后兜底的字段查找
        let base_norm = self.get_base_type(lhs_ty);

        // 2. 基于类型系统处理多级模块访问
        // 模块不会包裹在指针里，所以用 base_norm 查是安全的
        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                m.scope_id
            } else {
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Typeck): Expected module definition during module field lookup for DefId {}.",
                        mod_def_id.0
                    ),
                );
                return TypeId::ERROR;
            };
            if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field) {
                let real_ty = if target_info.kind == SymbolKind::Function {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::FnDef(target_info.def_id.unwrap(), vec![]))
                } else if target_info.kind == SymbolKind::Module {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Module(target_info.def_id.unwrap()))
                } else if target_info.type_id == TypeId::ERROR {
                    if let Some(def_id) = target_info.def_id {
                        let global_expr_opt =
                            if let Def::Global(g) = &self.ctx.defs[def_id.0 as usize] {
                                Some(g.value.clone())
                            } else {
                                None
                            };

                        if let Some(g_expr) = global_expr_opt {
                            if let Some(&actual_ty) = self.ctx.node_types.get(&g_expr.id) {
                                actual_ty
                            } else {
                                let prev_scope = self.ctx.scopes.current_scope_id();
                                if let Some(owner_scope) = self.global_owner_scope(def_id) {
                                    self.ctx.scopes.set_current_scope(owner_scope);
                                }
                                let computed_ty = self.check_expr(&g_expr, None);
                                if let Some(prev_scope) = prev_scope {
                                    self.ctx.scopes.set_current_scope(prev_scope);
                                }
                                computed_ty
                            }
                        } else {
                            target_info.type_id
                        }
                    } else {
                        target_info.type_id
                    }
                } else {
                    target_info.type_id
                };

                let mod_ty = self.ctx.type_registry.intern(TypeKind::Module(mod_def_id));
                self.ctx.node_types.insert(lhs.id, mod_ty);
                return real_ty;
            } else {
                let field_name = self.ctx.resolve(field);
                self.ctx
                    .struct_error(
                        span,
                        format!("module has no public member `{}`", field_name),
                    )
                    .emit();
                return TypeId::ERROR;
            }
        }

        // === 核心修复：构造正确的查找备选队列 ===
        // 队列优先级：精确类型匹配 -> 降级不可变指针 -> 隐式解引用(Auto-deref)字段访问
        let mut search_tys = vec![lhs_norm];

        // 3. 如果是可变类型，自动推入不可变版本作为 Fallback
        // 现在的 lhs_norm 带有完整的指针上下文，你的降级逻辑被成功激活了！
        match self.ctx.type_registry.get(lhs_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                search_tys.push(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }));
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                search_tys.push(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }));
            }
            TypeKind::Slice { is_mut: true, elem } => {
                search_tys.push(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }));
            }
            _ => {}
        }

        // 4. 推入解引用后的基础类型
        // 如果我们是在指针上调用 struct 的内部字段，靠这个兜底
        if !search_tys.contains(&base_norm) {
            search_tys.push(base_norm);
        }

        // 5. 按顺序逐级查找，一旦找到立刻返回
        for search_norm in search_tys {
            if let Some((ty, owner_trait_ty)) =
                self.try_find_field_or_method_silent(search_norm, lhs_ty, field)
            {
                if let Some(owner_trait_ty) = owner_trait_ty {
                    self.ctx.trait_method_owners.insert(expr_id, owner_trait_ty);
                }
                return ty;
            }
        }

        // 全部失败，抛出详细诊断
        let field_str = self.ctx.resolve(field);
        let lhs_str = self.ctx.ty_to_string(lhs_ty);

        self.ctx
            .struct_error(
                span,
                format!(
                    "no field or method named `{}` found on type `{}`",
                    field_str, lhs_str
                ),
            )
            .with_hint(
                "if this is a method, ensure the trait defining it is imported and implemented",
            )
            .with_hint("if this is a struct field, check for typos")
            .emit();

        TypeId::ERROR
    }

    /// 助手 2.5：静默查找字段或方法 (不报错，查不到返回 None)
    fn try_find_field_or_method_silent(
        &mut self,
        search_norm: TypeId,
        lhs_ty: TypeId,
        field: SymbolId,
    ) -> Option<(TypeId, Option<TypeId>)> {
        // 1. 如果是 Trait Object
        if let TypeKind::TraitObject(trait_def_id, trait_args) =
            self.ctx.type_registry.get(search_norm).clone()
            && let Some(m) =
                self.resolve_trait_object_method_silent(trait_def_id, &trait_args, field, lhs_ty)
        {
            return Some((m.method_ty, Some(m.owner_trait_ty)));
        }

        // 2. 如果是具名类型 (Struct/Union/Enum)
        if let TypeKind::Def(def_id, generic_args) = self.ctx.type_registry.get(search_norm).clone()
            && let Some(field_ty) = self.resolve_def_field(def_id, &generic_args, field)
        {
            return Some((field_ty, None));
        }
        // 支持匿名结构体/联合体的字段访问
        if let TypeKind::AnonymousStruct(_, ref fields) | TypeKind::AnonymousUnion(_, ref fields) =
            self.ctx.type_registry.get(search_norm).clone()
            && let Some(f) = fields.iter().find(|f| f.name == field)
        {
            return Some((f.ty, None));
        }

        // 3. 检查 active_bounds 环境约束
        for i in 0..self.ctx.active_bounds.len() {
            let (env_target, bounds) = self.ctx.active_bounds[i].clone();
            let mut map = HashMap::new();
            if self.unify(env_target, search_norm, &mut map) {
                let instantiated_bounds: Vec<TypeId> = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    bounds
                        .into_iter()
                        .map(|bound| subst.substitute(bound))
                        .collect()
                };

                for bound_ty in instantiated_bounds {
                    let bound_norm = self.resolve_tv(bound_ty);
                    if let TypeKind::TraitObject(trait_def_id, trait_args) =
                        self.ctx.type_registry.get(bound_norm).clone()
                        && let Some(m) = self.resolve_trait_object_method_silent(
                            trait_def_id,
                            &trait_args,
                            field,
                            lhs_ty,
                        )
                    {
                        return Some((m.method_ty, Some(m.owner_trait_ty)));
                    }
                }
            }
        }

        // 4. 检查全局的 impl 块
        if let Some(method_ty) = self.resolve_impl_method(search_norm, field) {
            return Some((method_ty, None));
        }

        None
    }

    /// 静默版：解析 Trait Object 的接口方法
    fn resolve_trait_object_method_silent(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        field: SymbolId,
        receiver_ty: TypeId,
    ) -> Option<TraitMethodLookup> {
        let mut visited = std::collections::HashSet::new();
        self.resolve_trait_object_method_in_hierarchy(
            trait_def_id,
            trait_args,
            field,
            receiver_ty,
            &mut visited,
        )
    }

    fn resolve_trait_object_method_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        field: SymbolId,
        receiver_ty: TypeId,
        visited: &mut std::collections::HashSet<DefId>,
    ) -> Option<TraitMethodLookup> {
        if !visited.insert(trait_def_id) {
            return None;
        }

        let trait_def = self.trait_def_for_access(trait_def_id, Span::default())?;
        let trait_arg_map: HashMap<SymbolId, TypeId> = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();

        if let Some(&(_, mut method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(m_name, _)| *m_name == field)
        {
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params.clone();
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty; // 维持调用的实际 LHS 为 receiver
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
                });
            }

            if !trait_arg_map.is_empty() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                method_ty = subst.substitute(method_ty);
            }
            let owner_trait_ty = self
                .ctx
                .type_registry
                .intern(TypeKind::TraitObject(trait_def_id, trait_args.to_vec()));
            return Some(TraitMethodLookup {
                owner_trait_ty,
                method_ty,
            });
        }

        let mut matches = Vec::new();
        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_norm = self.resolve_tv(inst_super_ty);

            if let TypeKind::TraitObject(super_def_id, super_args) =
                self.ctx.type_registry.get(inst_super_norm).clone()
                && let Some(method_lookup) = self.resolve_trait_object_method_in_hierarchy(
                    super_def_id,
                    &super_args,
                    field,
                    receiver_ty,
                    visited,
                )
            {
                matches.push(method_lookup);
            }
        }

        if matches.len() > 1 {
            let owners: Vec<String> = matches
                .iter()
                .map(|m| self.ctx.ty_to_string(m.owner_trait_ty))
                .collect();
            self.ctx
                .struct_error(
                    Span::default(),
                    format!(
                        "ambiguous inherited trait method `{}`",
                        self.ctx.resolve(field)
                    ),
                )
                .with_hint(format!(
                    "the method is inherited from multiple parent traits: {}",
                    owners.join(", ")
                ))
                .emit();
            return None;
        }

        matches.into_iter().next()
    }

    pub(crate) fn check_slice_op(
        &mut self,
        lhs: &Expr,
        start: Option<&Expr>,
        end: Option<&Expr>,
        _is_inclusive: bool,
        is_mut: bool,
        span: Span,
    ) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);

        if let Some(s) = start {
            let s_ty = self.check_expr(s, Some(TypeId::USIZE));
            let s_ty_id = self.resolve_tv(s_ty);
            if !self.ctx.type_registry.is_integer(s_ty_id) {
                self.ctx
                    .struct_error(s.span, "slice start index must be an integer")
                    .emit();
            }
        }
        if let Some(e) = end {
            let e_ty = self.check_expr(e, Some(TypeId::USIZE));
            let e_ty_id = self.resolve_tv(e_ty);
            if !self.ctx.type_registry.is_integer(e_ty_id) {
                self.ctx
                    .struct_error(e.span, "slice end index must be an integer")
                    .emit();
            }
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
        let base_allows_mut_slice = match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Pointer {
                is_mut: true, ..
            }
            | TypeKind::VolatilePtr {
                is_mut: true, ..
            }
            | TypeKind::Slice {
                is_mut: true, ..
            }
            | TypeKind::Array {
                is_mut: true, ..
            }
            | TypeKind::ArrayInfer {
                is_mut: true, ..
            } => true,
            _ => false,
        } || self.is_lvalue_mutable(lhs);

        // 如果是 `..[`，必须确保目标内存具有可变权限
        if is_mut && !base_allows_mut_slice && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    span,
                    "cannot create a mutable slice from an immutable location",
                )
                .with_hint("ensure the target is bound with `let mut` or is a mutable pointer")
                .emit();
        }

        match self.ctx.type_registry.get(norm_lhs).clone() {
            TypeKind::Array { elem, .. }
            | TypeKind::Slice { elem, .. }
            | TypeKind::Pointer { elem, .. }
            | TypeKind::VolatilePtr { elem, .. } => self
                .ctx
                .type_registry
                .intern(TypeKind::Slice { is_mut, elem }),
            TypeKind::Error => TypeId::ERROR,
            _ => {
                self.ctx
                    .struct_error(lhs.span, "cannot slice a non-array/non-slice type")
                    .emit();
                TypeId::ERROR
            }
        }
    }

    /// 辅助方法 1：自动解引用 Pointer/VolatilePtr，获取底层的 Struct/Union/Enum 类型
    fn get_base_type(&mut self, mut base_ty: TypeId) -> TypeId {
        loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm).clone() {
                // 遇到指针，自动扒掉外衣继续往下找
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = elem;
                }
                // 找到底了，返回
                _ => return norm,
            }
        }
    }

    /// 辅助方法 3：解析 Struct/Union 字段或 Enum 变体
    fn resolve_def_field(
        &mut self,
        def_id: DefId,
        generic_args: &[TypeId],
        field: SymbolId,
    ) -> Option<TypeId> {
        let def = self.ctx.defs[def_id.0 as usize].clone();

        match &def {
            Def::Struct(s) => {
                if let Some(f) = s.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_to_field(
                        &s.generics,
                        generic_args,
                        f.type_node.id,
                    ));
                }
            }
            Def::Union(u) => {
                if let Some(f) = u.fields.iter().find(|f| f.name == field) {
                    return Some(self.apply_generics_to_field(
                        &u.generics,
                        generic_args,
                        f.type_node.id,
                    ));
                }
            }
            _ => {}
        }
        None
    }

    /// 辅助方法 3.1：处理字段提取后的泛型替换
    fn apply_generics_to_field(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
        node_id: NodeId,
    ) -> TypeId {
        let mut field_ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        if !generics.is_empty() && !args.is_empty() {
            let mut map = std::collections::HashMap::new();
            for (i, param) in generics.iter().enumerate() {
                map.insert(param.name, args[i]);
            }
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            field_ty = subst.substitute(field_ty);
        }

        field_ty
    }

    /// 辅助方法 4：通过全局 Impl 块进行方法分发 (Method Dispatch)
    fn resolve_impl_method(&mut self, lhs_ty: TypeId, field: SymbolId) -> Option<TypeId> {
        let mut found_method_id = None;
        let mut resolved_impl_args = Vec::new();

        let impl_blocks: Vec<_> = self
            .ctx
            .global_impls
            .iter()
            .filter_map(|&id| {
                if let Def::Impl(impl_def) = &self.ctx.defs[id.0 as usize] {
                    Some(impl_def.clone())
                } else {
                    None
                }
            })
            .collect();

        for impl_def in impl_blocks {
            let impl_target_ty = self
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut map = std::collections::HashMap::new();

            if self.unify(impl_target_ty, lhs_ty, &mut map) {
                // 检查该 Impl 块的 Where 约束是否被满足
                if !self.satisfies_bounds(&impl_def.where_clauses, &map) {
                    continue; // 约束不满足，说明这个 Impl 块不适用，跳过
                }

                // 将 Impl 块捕获的泛型参数提取出来
                let mut candidate_impl_args = Vec::new();
                for param in &impl_def.generics {
                    candidate_impl_args.push(map.get(&param.name).copied().unwrap_or(TypeId::ERROR));
                }
                // 在匹配的 Impl 块内寻找目标函数
                for &method_id in &impl_def.methods {
                    if let Def::Function(func_def) = &self.ctx.defs[method_id.0 as usize]
                        && func_def.name == field
                    {
                        found_method_id = Some(method_id);
                        resolved_impl_args = candidate_impl_args;
                        break;
                    }
                }
            }
            if found_method_id.is_some() {
                break;
            }
        }

        found_method_id.map(|method_id| {
            self.ctx
                .type_registry
                .intern(TypeKind::FnDef(method_id, resolved_impl_args))
        })
    }

    /// 辅助方法：静默检查 Where 子句约束是否满足
    fn satisfies_bounds(
        &mut self,
        where_clauses: &[ast::WhereClause],
        map: &HashMap<SymbolId, TypeId>,
    ) -> bool {
        let mut pairs_to_check = Vec::new();

        {
            let mut subst = Substituter::new(&mut self.ctx.type_registry, map);

            for clause in where_clauses {
                let original_target = self
                    .ctx
                    .node_types
                    .get(&clause.target_ty.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_target = subst.substitute(original_target);

                for bound_ast in &clause.bounds {
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

        for (sub_target, sub_bound) in pairs_to_check {
            if sub_target != TypeId::ERROR && sub_bound != TypeId::ERROR {
                // 如果存在任何一个特征未被实现，直接返回 false
                if !self.check_trait_impl(sub_target, sub_bound) {
                    return false;
                }
            }
        }

        true
    }
}
