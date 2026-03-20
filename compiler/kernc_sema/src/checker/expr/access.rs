use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::scope::{SymbolInfo, SymbolKind};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr};
use kernc_utils::{NodeId, Span, SymbolId};
use std::collections::HashMap;

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_identifier(&mut self, name: SymbolId, span: Span) -> TypeId {
        if let Some(info) = self.ctx.scopes.resolve(name) {
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

    pub(crate) fn check_field_access(&mut self, lhs: &Expr, field: SymbolId, span: Span) -> TypeId {
        let lhs_ty = self.check_expr(lhs, None);
        if lhs_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let current_norm = self.get_base_type(lhs_ty);

        // 基于类型系统处理多级模块访问
        if let TypeKind::Module(mod_def_id) = self.ctx.type_registry.get(current_norm).clone() {
            let mod_scope = if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                m.scope_id
            } else {
                unreachable!()
            };

            if let Some(target_info) = self.ctx.scopes.resolve_in(mod_scope, field) {
                let real_ty = if target_info.kind == SymbolKind::Function {
                    self.ctx.type_registry.intern(TypeKind::FnDef(target_info.def_id.unwrap(), vec![]))
                } else if target_info.kind == SymbolKind::Module {
                    // 如果连续访问子模块 (比如 std.io 中的 io)，返回子模块类型
                    self.ctx.type_registry.intern(TypeKind::Module(target_info.def_id.unwrap()))
                } else {
                    target_info.type_id
                };
                
                // 缓存类型推导结果到 AST 节点
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

        // 2. 如果是 Trait Object，走虚表方法解析路径
        if let TypeKind::TraitObject(trait_def_id, trait_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            return self.resolve_trait_object_method(trait_def_id, &trait_args, field, lhs_ty, span);
        }

        // 3. 如果是具名类型 (Struct/Union/Enum)，查找字段或变体
        if let TypeKind::Def(def_id, generic_args) =
            self.ctx.type_registry.get(current_norm).clone()
        {
            if let Some(field_ty) = self.resolve_def_field(def_id, &generic_args, field) {
                return field_ty;
            }
        }

        // 4. 作为最后的 fallback，去全局的 impl 块中查找方法
        if let Some(method_ty) = self.resolve_impl_method(lhs_ty, field) {
            return method_ty;
        }

        // 5. 全部失败，抛出详细诊断
        let field_str = self.ctx.resolve(field);
        let lhs_str = self.ctx.ty_to_string(lhs_ty);

        self.ctx
            .struct_error(
                span,
                format!("no field or method named `{}` found on type `{}`", field_str, lhs_str),
            )
            .with_hint("if this is a method, ensure the trait defining it is imported and implemented")
            .with_hint("if this is a struct field, check for typos")
            .emit();

        TypeId::ERROR
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

        // 如果是 `..[`，必须确保目标内存具有可变性
        if is_mut && !self.is_lvalue_mutable(lhs) && lhs_ty != TypeId::ERROR {
            self.ctx
                .struct_error(
                    span,
                    "cannot create a mutable slice from an immutable location",
                )
                .with_hint("ensure the target is bound with `let mut` or is a mutable pointer")
                .emit();
        }

        let norm_lhs = self.resolve_tv(lhs_ty);
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

    /// 辅助方法 2：解析 Trait Object 的接口方法
    fn resolve_trait_object_method(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        field: SymbolId,
        receiver_ty: TypeId,
        span: Span,
    ) -> TypeId {
        let trait_def = match &self.ctx.defs[trait_def_id.0 as usize] {
            Def::Trait(t) => t.clone(),
            _ => unreachable!(),
        };

        if let Some(&(_, mut method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(m_name, _)| *m_name == field)
        {
            // 将原本的 SelfType(也就是 Writer) 强行替换为实际的指针类型 (*mut Writer)
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params.clone();
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
                });
            }

            // 泛型实例化替换
            if !trait_def.generics.is_empty() && !trait_args.is_empty() {
                let mut map = HashMap::new();
                for (i, param) in trait_def.generics.iter().enumerate() {
                    map.insert(param.name, trait_args[i]);
                }
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                method_ty = subst.substitute(method_ty);
            }
            return method_ty;
        }

        let field_str = self.ctx.resolve(field);
        self.ctx
            .struct_error(
                span,
                format!("method `{}` not found in trait object", field_str),
            )
            .with_hint("ensure the method is explicitly declared in the trait's contract")
            .emit();
        TypeId::ERROR
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

        // TODO: 注意：未来可以考虑在 Sema 收集阶段将这些方法缓存到 Context 中避免每次 O(N) 遍历
        let impl_blocks: Vec<_> = self
            .ctx
            .defs
            .iter()
            .filter_map(|def| {
                if let Def::Impl(impl_def) = def {
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
                // 将 Impl 块捕获的泛型参数提取出来
                for param in &impl_def.generics {
                    resolved_impl_args.push(map.get(&param.name).copied().unwrap_or(TypeId::ERROR));
                }
                // 在匹配的 Impl 块内寻找目标函数
                for &method_id in &impl_def.methods {
                    if let Def::Function(func_def) = &self.ctx.defs[method_id.0 as usize] {
                        if func_def.name == field {
                            found_method_id = Some(method_id);
                            break;
                        }
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
}
