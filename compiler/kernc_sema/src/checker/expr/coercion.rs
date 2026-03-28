use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{Expr, ExprKind, UnaryOperator};
use kernc_utils::{Span, SymbolId};
use std::collections::{HashMap, HashSet};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    /// 检查表达式能否隐式转换为目标类型，包含指针降级与 BNC
    pub(crate) fn check_coercion(&mut self, expr: &Expr, expected: TypeId, actual: TypeId) -> bool {
        let exp = self.resolve_tv(expected);
        let act = self.resolve_tv(actual);

        if exp == act || exp == TypeId::ERROR || act == TypeId::ERROR {
            return true;
        }
        if act == TypeId::NEVER {
            return true;
        }

        let exp_kind = self.ctx.type_registry.get(exp).clone();
        let act_kind = self.ctx.type_registry.get(act).clone();

        // 1. 触发类型合一
        if self.check_type_var(exp, act, &exp_kind, &act_kind) {
            return true;
        }

        // 2. 值语义的匿名聚合降级 (具名 -> 匿名) ===
        if self.is_anonymous_aggregate_equivalent(exp, act) {
            return true;
        }

        // 3. 指针降级与 Trait Object 处理
        if self.check_pointer_coercions(expr, exp, act, &exp_kind, &act_kind) {
            return true;
        }

        // 4. 易失指针降级与 Trait Object 处理
        if self.check_volatile_coercions(expr, exp, act, &exp_kind, &act_kind) {
            return true;
        }

        // 5. 切片降级与数组退化
        if self.check_slice_and_array_decay(expr, exp, &exp_kind, &act_kind) {
            return true;
        }

        // 6. 闭包相关的退化与边界自然转换
        if self.check_closure_coercions(expr, &exp_kind, &act_kind) {
            return true;
        }

        // 如果所有规则都匹配失败，输出不匹配错误
        self.emit_mismatch_error(expr.span, expected, actual);
        false
    }

    fn check_type_var(
        &mut self,
        exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::TypeVar(vid) = act_kind {
            self.type_vars[*vid as usize] = Some(exp);
            return true;
        }
        if let TypeKind::TypeVar(vid) = exp_kind {
            self.type_vars[*vid as usize] = Some(act);
            return true;
        }
        false
    }

    fn check_pointer_coercions(
        &mut self,
        expr: &Expr,
        _exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_norm = self.resolve_tv(*e_inner);
            if self.check_pointer_to_pointer_coercion(*e_mut, e_norm, act, act_kind) {
                return true;
            }
            if self.check_value_to_trait_object_pointer(expr, *e_mut, e_norm, act, act_kind) {
                return true;
            }
        }
        false
    }

    fn pointer_mutability_allows(expected_mut: bool, actual_mut: bool) -> bool {
        !expected_mut || actual_mut
    }

    fn check_pointer_to_pointer_coercion(
        &mut self,
        expected_mut: bool,
        expected_elem: TypeId,
        actual_ty: TypeId,
        act_kind: &TypeKind,
    ) -> bool {
        let TypeKind::Pointer {
            is_mut: actual_mut,
            elem: actual_elem,
        } = act_kind
        else {
            return false;
        };

        if !Self::pointer_mutability_allows(expected_mut, *actual_mut) {
            return false;
        }

        let actual_elem_norm = self.resolve_tv(*actual_elem);
        if expected_elem == actual_elem_norm
            || self.ctx.type_registry.is_void(expected_elem)
            || self.is_anonymous_aggregate_equivalent(expected_elem, actual_elem_norm)
        {
            return true;
        }

        if matches!(
            (
                self.ctx.type_registry.get(expected_elem),
                self.ctx.type_registry.get(actual_elem_norm)
            ),
            (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
        ) && self.is_trait_object_upcast(actual_elem_norm, expected_elem)
        {
            return true;
        }

        if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(expected_elem) {
            let trait_source_ty = if !expected_mut && *actual_mut {
                self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem: *actual_elem,
                })
            } else {
                actual_ty
            };

            if self.check_trait_impl(trait_source_ty, expected_elem) {
                return true;
            }
        }

        false
    }

    pub(crate) fn is_trait_object_upcast(
        &mut self,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
    ) -> bool {
        let source_norm = self.resolve_tv(source_trait_ty);
        let target_norm = self.resolve_tv(target_trait_ty);

        if source_norm == target_norm {
            return true;
        }

        let mut visited = HashSet::new();
        self.find_supertrait_in_hierarchy(source_norm, target_norm, &mut visited)
            .is_some()
    }

    pub(crate) fn find_supertrait_in_hierarchy(
        &mut self,
        source_trait_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut HashSet<TypeId>,
    ) -> Option<TypeId> {
        let source_norm = self.resolve_tv(source_trait_ty);
        let target_norm = self.resolve_tv(target_trait_ty);

        if !visited.insert(source_norm) {
            return None;
        }

        let TypeKind::TraitObject(source_def_id, source_args) =
            self.ctx.type_registry.get(source_norm).clone()
        else {
            return None;
        };

        let Def::Trait(trait_def) = self.ctx.defs[source_def_id.0 as usize].clone() else {
            return None;
        };

        let trait_arg_map: HashMap<SymbolId, TypeId> = trait_def
            .generics
            .iter()
            .zip(source_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();

        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_norm = self.resolve_tv(inst_super_ty);

            if inst_super_norm == target_norm {
                return Some(inst_super_norm);
            }

            if let Some(found) =
                self.find_supertrait_in_hierarchy(inst_super_norm, target_norm, visited)
            {
                return Some(found);
            }
        }

        None
    }

    fn check_value_to_trait_object_pointer(
        &mut self,
        expr: &Expr,
        expected_mut: bool,
        expected_elem: TypeId,
        actual_ty: TypeId,
        act_kind: &TypeKind,
    ) -> bool {
        if !matches!(
            self.ctx.type_registry.get(expected_elem),
            TypeKind::TraitObject(..)
        ) {
            return false;
        }

        if matches!(
            act_kind,
            TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }
        ) {
            return false;
        }

        if expected_mut && !self.is_lvalue_mutable(expr) {
            self.ctx
                .struct_error(
                    expr.span,
                    "cannot implicitly borrow an immutable value as a mutable trait object `*mut Trait`",
                )
                .with_hint("consider declaring the variable as `let mut`")
                .emit();
            return false;
        }

        let virtual_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: expected_mut,
            elem: actual_ty,
        });

        self.check_trait_impl(virtual_ptr_ty, expected_elem)
    }

    /// 核心辅助方法：检查一个具名聚合类型能否降级为匿名聚合类型
    pub(crate) fn is_anonymous_aggregate_equivalent(
        &mut self,
        exp_anon: TypeId,
        act_def: TypeId,
    ) -> bool {
        let exp_kind = self.ctx.type_registry.get(exp_anon).clone();
        let act_kind = self.ctx.type_registry.get(act_def).clone();

        if let TypeKind::Def(def_id, ref act_args) = act_kind {
            let act_def_clone = self.ctx.defs[def_id.0 as usize].clone();

            match (exp_kind.clone(), act_def_clone) {
                (TypeKind::AnonymousStruct(exp_is_extern, exp_fields), Def::Struct(act_s)) => {
                    if exp_is_extern != act_s.is_extern {
                        return false;
                    }
                    return self.compare_named_fields_to_anonymous(
                        &act_s.generics,
                        &act_s.fields,
                        act_args,
                        &exp_fields,
                        true,
                    );
                }
                (TypeKind::AnonymousUnion(exp_is_extern, exp_fields), Def::Union(act_u)) => {
                    if exp_is_extern != act_u.is_extern {
                        return false;
                    }
                    return self.compare_named_fields_to_anonymous(
                        &act_u.generics,
                        &act_u.fields,
                        act_args,
                        &exp_fields,
                        false,
                    );
                }
                _ => {}
            }
        }

        if let TypeKind::Enum(def_id, ref act_args) = act_kind {
            let act_def_clone = self.ctx.defs[def_id.0 as usize].clone();
            if let (TypeKind::AnonymousEnum(exp_enum), Def::Enum(act_enum)) =
                (exp_kind, act_def_clone)
            {
                let exp_backing = exp_enum.backing_ty.unwrap_or(TypeId::U32);
                let act_backing = act_enum.backing_type.as_ref().map_or(TypeId::U32, |bt| {
                    self.ctx
                        .node_types
                        .get(&bt.id)
                        .copied()
                        .unwrap_or(TypeId::U32)
                });

                if self.resolve_tv(exp_backing) != self.resolve_tv(act_backing) {
                    return false;
                }

                if exp_enum.variants.len() != act_enum.variants.len() {
                    return false;
                }

                let mut subst_map = std::collections::HashMap::new();
                for (i, param) in act_enum.generics.iter().enumerate() {
                    subst_map.insert(param.name, act_args[i]);
                }

                let mut current_val: i128 = 0;
                for (exp_variant, act_variant) in
                    exp_enum.variants.iter().zip(act_enum.variants.iter())
                {
                    if let Some(v_expr) = &act_variant.value {
                        let mut ce = crate::checker::ConstEvaluator::new(self.ctx);
                        if let Ok(val) = ce.eval_math(v_expr) {
                            current_val = val;
                        }
                    }

                    if exp_variant.name != act_variant.name {
                        return false;
                    }

                    let act_payload = act_variant.payload_type.as_ref().map(|payload_ast| {
                        let raw_ty = self
                            .ctx
                            .node_types
                            .get(&payload_ast.id)
                            .copied()
                            .unwrap_or(TypeId::ERROR);
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                        let substituted = subst.substitute(raw_ty);
                        self.resolve_tv(substituted)
                    });

                    if exp_variant.payload_ty.map(|ty| self.resolve_tv(ty)) != act_payload {
                        return false;
                    }

                    let exp_value = exp_variant.explicit_value.unwrap_or(current_val);
                    if exp_value != current_val {
                        return false;
                    }

                    current_val += 1;
                }

                return true;
            }
        }
        false
    }

    fn compare_named_fields_to_anonymous(
        &mut self,
        generics: &[kernc_ast::GenericParam],
        named_fields: &[kernc_ast::StructFieldDef],
        args: &[TypeId],
        anon_fields: &[crate::ty::AnonymousField],
        _sort_named: bool,
    ) -> bool {
        if anon_fields.len() != named_fields.len() {
            return false;
        }

        let mut act_fields = Vec::new();
        for f in named_fields {
            let raw_ty = self
                .ctx
                .node_types
                .get(&f.type_node.id)
                .copied()
                .unwrap_or(TypeId::ERROR);

            let inst_ty = if !generics.is_empty() && !args.is_empty() {
                let mut map = std::collections::HashMap::new();
                for (i, param) in generics.iter().enumerate() {
                    map.insert(param.name, args[i]);
                }
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                subst.substitute(raw_ty)
            } else {
                raw_ty
            };

            act_fields.push((f.name, self.resolve_tv(inst_ty)));
        }

        act_fields.sort_by_key(|f| f.0);

        for (exp_f, act_f) in anon_fields.iter().zip(act_fields.iter()) {
            if exp_f.name != act_f.0 || self.resolve_tv(exp_f.ty) != act_f.1 {
                return false;
            }
        }

        true
    }
    fn check_volatile_coercions(
        &mut self,
        _expr: &Expr,
        _exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::VolatilePtr {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
            && let TypeKind::VolatilePtr {
                is_mut: a_mut,
                elem: a_inner,
            } = act_kind
            && (!*e_mut || *a_mut)
        {
            let e_norm = self.resolve_tv(*e_inner);
            let a_norm = self.resolve_tv(*a_inner);
            if e_norm == a_norm {
                return true;
            }
            if self.ctx.type_registry.is_void(e_norm) {
                return true;
            }
            if self.is_anonymous_aggregate_equivalent(e_norm, a_norm) {
                return true;
            }
            if matches!(
                (
                    self.ctx.type_registry.get(e_norm),
                    self.ctx.type_registry.get(a_norm)
                ),
                (TypeKind::TraitObject(..), TypeKind::TraitObject(..))
            ) && self.is_trait_object_upcast(a_norm, e_norm)
            {
                return true;
            }
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm)
                && self.check_trait_impl(act, e_norm)
            {
                return true;
            }
        }
        false
    }

    fn check_slice_and_array_decay(
        &mut self,
        expr: &Expr,
        _exp: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Slice {
            is_mut: e_mut,
            elem: exp_elem,
        } = exp_kind
        {
            if let TypeKind::Slice {
                is_mut: act_mut,
                elem: act_elem,
            } = act_kind
                && (!*e_mut || *act_mut)
                && self.resolve_tv(*exp_elem) == self.resolve_tv(*act_elem)
            {
                return true;
            }
            match self.check_array_decay(*e_mut, *exp_elem, act_kind, expr.span) {
                Ok(true) => return true,
                Err(()) => return false,
                Ok(false) => {}
            }
        }
        false
    }

    fn check_closure_coercions(
        &mut self,
        expr: &Expr,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Function {
            params: e_params,
            ret: e_ret,
            is_variadic: false,
        } = exp_kind
        {
            if self.check_closure_decay_to_function(e_params, *e_ret, act_kind) {
                return true;
            }
            if self.check_fn_like_to_closure_interface(e_params, *e_ret, act_kind, expr.span) {
                return true;
            }
        }

        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            let e_norm = self.resolve_tv(*e_inner);
            if let TypeKind::ClosureInterface {
                params: ref e_params,
                ret: e_ret,
            } = self.ctx.type_registry.get(e_norm).clone()
            {
                if self.check_state_to_closure_interface(expr, *e_mut, e_params, e_ret, act_kind) {
                    return true;
                }
                if self.check_fn_like_to_closure_interface(e_params, e_ret, act_kind, expr.span) {
                    return true;
                }
            }
        }
        false
    }

    fn check_closure_decay_to_function(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        act_kind: &TypeKind,
    ) -> bool {
        let TypeKind::AnonymousState {
            captures,
            params,
            ret,
            ..
        } = act_kind
        else {
            return false;
        };

        captures.is_empty()
            && self.signatures_compatible(expected_params, expected_ret, params, *ret)
    }

    fn check_state_to_closure_interface(
        &mut self,
        expr: &Expr,
        expected_mut: bool,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        act_kind: &TypeKind,
    ) -> bool {
        let TypeKind::AnonymousState { params, ret, .. } = act_kind else {
            return false;
        };

        if expected_mut && !self.is_lvalue_mutable(expr) {
            self.ctx
                .struct_error(
                    expr.span,
                    "cannot implicitly borrow an immutable closure as a mutable closure `*mut Fn`",
                )
                .with_hint("consider declaring the closure variable as `let mut`")
                .emit();
            return false;
        }

        self.signatures_compatible(expected_params, expected_ret, params, *ret)
    }

    fn check_fn_like_to_closure_interface(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        act_kind: &TypeKind,
        span: Span,
    ) -> bool {
        let Some((actual_params, actual_ret)) = self.extract_fn_sig_for_bnc(act_kind, span) else {
            return false;
        };

        self.signatures_compatible(expected_params, expected_ret, &actual_params, actual_ret)
    }

    fn signatures_compatible(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        actual_params: &[TypeId],
        actual_ret: TypeId,
    ) -> bool {
        if expected_params.len() != actual_params.len() {
            return false;
        }

        let mut map = HashMap::new();
        for (expected, actual) in expected_params.iter().zip(actual_params.iter()) {
            if !self.unify(*expected, *actual, &mut map) {
                return false;
            }
        }

        self.unify(expected_ret, actual_ret, &mut map)
    }

    /// 提取函数定义项 (FnDef) 的确切签名，处理泛型代入
    fn extract_fn_sig_for_bnc(
        &mut self,
        act_kind: &TypeKind,
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        match act_kind {
            TypeKind::FnDef(def_id, args) => self.instantiate_fn_def_signature(*def_id, args, span),
            TypeKind::Function {
                params,
                ret,
                is_variadic: false,
            } => Some((params.clone(), *ret)),
            TypeKind::Function {
                is_variadic: true, ..
            } => None,
            _ => None,
        }
    }

    fn instantiate_fn_def_signature(
        &mut self,
        def_id: DefId,
        args: &[TypeId],
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        let def = self.ctx.defs[def_id.0 as usize].clone();
        let Def::Function(fn_def) = def else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Compiler ICE: FnDef `{}` does not point to a function during closure BNC",
                    def_id.0
                ),
            );
            return None;
        };

        let Some(sig_ty) = fn_def.resolved_sig else {
            self.ctx.emit_ice(
                span,
                "Compiler ICE: function definition lacks resolved signature during closure BNC",
            );
            return None;
        };

        let norm_sig = self.resolve_tv(sig_ty);
        let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = self.ctx.type_registry.get(norm_sig).clone()
        else {
            self.ctx.emit_ice(
                span,
                format!(
                    "Compiler ICE: resolved signature for FnDef `{}` is not a function type",
                    def_id.0
                ),
            );
            return None;
        };

        if is_variadic {
            return None;
        }

        if fn_def.generics.is_empty() {
            return Some((params, ret));
        }

        let mut map = HashMap::new();
        for (i, param) in fn_def.generics.iter().enumerate() {
            if let Some(&arg) = args.get(i) {
                map.insert(param.name, arg);
            }
        }

        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
        let inst_params = params.into_iter().map(|p| subst.substitute(p)).collect();
        let inst_ret = subst.substitute(ret);
        Some((inst_params, inst_ret))
    }

    pub(crate) fn unify(
        &mut self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        map: &mut std::collections::HashMap<SymbolId, TypeId>,
    ) -> bool {
        let gen_norm = self.resolve_tv(generic_ty);
        let con_norm = self.resolve_tv(concrete_ty);

        let gen_kind = self.ctx.type_registry.get(gen_norm).clone();
        let con_kind = self.ctx.type_registry.get(con_norm).clone();

        match (gen_kind, con_kind) {
            (TypeKind::Param(name), _) => {
                if let Some(&existing_ty) = map.get(&name) {
                    existing_ty == concrete_ty
                } else {
                    map.insert(name, concrete_ty);
                    true
                }
            }
            // 指针和切片的 Unify 必须同时匹配其 mut 属性
            (
                TypeKind::Pointer {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::Pointer {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify(g_e, c_e, map),
            (
                TypeKind::VolatilePtr {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::VolatilePtr {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify(g_e, c_e, map),
            (
                TypeKind::Slice {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::Slice {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify(g_e, c_e, map),
            (
                TypeKind::Array {
                    is_mut: g_m,
                    elem: g_e,
                    len: g_l,
                },
                TypeKind::Array {
                    is_mut: c_m,
                    elem: c_e,
                    len: c_l,
                },
            ) => g_m == c_m && g_l == c_l && self.unify(g_e, c_e, map),
            (
                TypeKind::ArrayInfer {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::ArrayInfer {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify(g_e, c_e, map),

            (TypeKind::Def(g_id, g_args), TypeKind::Def(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            (TypeKind::Enum(g_id, g_args), TypeKind::Enum(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            (TypeKind::TraitObject(g_id, g_args), TypeKind::TraitObject(c_id, c_args))
                if g_id == c_id =>
            {
                if g_args.len() != c_args.len() {
                    return false;
                }
                g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify(*ga, *ca, map))
            }
            (
                TypeKind::ClosureInterface {
                    params: gp,
                    ret: gr,
                },
                TypeKind::ClosureInterface {
                    params: cp,
                    ret: cr,
                },
            ) => {
                gp.len() == cp.len()
                    && gp
                        .iter()
                        .zip(cp.iter())
                        .all(|(g, c)| self.unify(*g, *c, map))
                    && self.unify(gr, cr, map)
            }

            (
                TypeKind::AnonymousState {
                    captures: gc,
                    params: gp,
                    ret: gr,
                    ..
                },
                TypeKind::AnonymousState {
                    captures: cc,
                    params: cp,
                    ret: cr,
                    ..
                },
            ) => {
                gc.len() == cc.len()
                    && gp.len() == cp.len()
                    && gc
                        .iter()
                        .zip(cc.iter())
                        .all(|(g, c)| self.unify(*g, *c, map))
                    && gp
                        .iter()
                        .zip(cp.iter())
                        .all(|(g, c)| self.unify(*g, *c, map))
                    && self.unify(gr, cr, map)
            }
            _ => gen_norm == con_norm,
        }
    }

    /// 左值 (LValue) 可变性推导
    pub(crate) fn is_lvalue_mutable(&mut self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                if let Some(info) = self.ctx.scopes.resolve(*name) {
                    info.is_mut
                } else {
                    false
                }
            }
            ExprKind::Unary {
                op: UnaryOperator::PointerDeRef,
                operand,
            } => {
                let ptr_ty = self
                    .ctx
                    .node_types
                    .get(&operand.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm = self.resolve_tv(ptr_ty);
                match self.ctx.type_registry.get(norm) {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        *is_mut
                    }
                    _ => false,
                }
            }
            ExprKind::FieldAccess { lhs, .. } | ExprKind::IndexAccess { lhs, .. } => {
                let lhs_ty = self
                    .ctx
                    .node_types
                    .get(&lhs.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_lhs = self.resolve_tv(lhs_ty);

                match self.ctx.type_registry.get(norm_lhs).clone() {
                    TypeKind::Pointer { is_mut, .. } | TypeKind::VolatilePtr { is_mut, .. } => {
                        is_mut
                    }
                    TypeKind::Slice { is_mut, .. } => is_mut,
                    TypeKind::Array { is_mut, .. } | TypeKind::ArrayInfer { is_mut, .. } => {
                        is_mut
                            || matches!(lhs.kind, ExprKind::FieldAccess { .. })
                                && self.is_lvalue_mutable(lhs)
                    }
                    _ => self.is_lvalue_mutable(lhs),
                }
            }
            ExprKind::SliceOp { is_mut, .. } => *is_mut,

            // 右值实体化 (R-value Materialization) 的栈内存默认可变
            ExprKind::DataInit { .. }
            | ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Char(_)
            | ExprKind::ByteChar(_)
            | ExprKind::Call { .. } => {
                true // 纯右值被实体化为临时栈变量后，完全归当前作用域所有，允许就地可变借用
            }
            ExprKind::String(_) => {
                false // 字符串字面量硬编码在 .rodata 中，不能获取它的可变指针
            }

            _ => false,
        }
    }

    /// 循环并找出类型变量 `?T` 最终绑定的真实类型
    pub(crate) fn resolve_tv(&mut self, ty: TypeId) -> TypeId {
        let mut curr = ty;
        loop {
            let norm = self.ctx.type_registry.normalize(curr);
            if let TypeKind::TypeVar(vid) = self.ctx.type_registry.get(norm) {
                if let Some(target) = self.type_vars[*vid as usize] {
                    curr = target;
                } else {
                    return norm; // 没被推导出来，原样返回 `?T`
                }
            } else {
                return norm;
            }
        }
    }

    /// 数组到切片的退化
    fn check_array_decay(
        &mut self,
        exp_is_mut: bool,
        exp_elem: TypeId,
        act_kind: &TypeKind,
        span: Span,
    ) -> Result<bool, ()> {
        if let TypeKind::Array {
            is_mut: act_mut,
            elem: act_elem,
            ..
        } = act_kind
        {
            let exp_base = self.resolve_tv(exp_elem);
            let act_base = self.resolve_tv(*act_elem);

            if exp_base == act_base {
                if exp_is_mut && !*act_mut {
                    self.ctx.struct_error(span, "cannot implicitly convert an immutable array to a mutable slice `[]mut T`").emit();
                    return Err(());
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn check_trait_impl(&mut self, source_ty: TypeId, target_trait_ty: TypeId) -> bool {
        let mut visited = HashSet::new();
        if self.check_trait_impl_inner(source_ty, target_trait_ty, &mut visited) {
            return true;
        }

        // 如果可变指针/切片没有直接实现特征，尝试检查其不可变版本。
        // 因为方法调用时接收者可以安全降权，不可变版本实现的特征，可变版本理应兼容。
        let source_norm = self.resolve_tv(source_ty);
        let downgraded = match self.ctx.type_registry.get(source_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Pointer {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }))
            }
            TypeKind::Slice { is_mut: true, elem } => {
                Some(self.ctx.type_registry.intern(TypeKind::Slice {
                    is_mut: false,
                    elem,
                }))
            }
            _ => None,
        };

        if let Some(down_ty) = downgraded {
            let mut visited = HashSet::new(); // 清空 visited 重新查
            return self.check_trait_impl_inner(down_ty, target_trait_ty, &mut visited);
        }

        false
    }

    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<DefId>,
    ) -> bool {
        // === 1. 优先检查当前环境上下文中的 Where 约束 (active_bounds) ===
        if self.check_trait_impl_in_env_bounds(source_ty, target_trait_ty, visited) {
            return true;
        }

        // === 2. 检查全局的 impl 块  ===
        if self.check_trait_impl_in_global_impls(source_ty, target_trait_ty, visited) {
            return true;
        }

        false
    }

    /// 子方法 1：检查环境上下文中 active_bounds 提供的约束
    fn check_trait_impl_in_env_bounds(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<DefId>,
    ) -> bool {
        for i in 0..self.ctx.active_bounds.len() {
            let (env_target, env_bounds) = self.ctx.active_bounds[i].clone();
            let mut map = HashMap::new();

            // 如果查询的 source_ty (比如 *T) 匹配了环境里的 target (比如 *T)
            if self.unify(env_target, source_ty, &mut map) {
                // 利用临时块隔离可变借用
                let instantiated_bounds: Vec<TypeId> = {
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    env_bounds
                        .into_iter()
                        .map(|b| subst.substitute(b))
                        .collect()
                };

                for inst_env_bound in instantiated_bounds {
                    let inst_norm = self.resolve_tv(inst_env_bound);
                    let target_norm = self.resolve_tv(target_trait_ty);

                    if inst_norm == target_norm || inst_env_bound == target_trait_ty {
                        return true;
                    }

                    // 环境约束自身也可能继承自某个 Supertrait，递归检查
                    if let TypeKind::TraitObject(inst_def_id, _) =
                        self.ctx.type_registry.get(inst_norm)
                        && visited.insert(*inst_def_id)
                        && self.check_trait_impl_inner(inst_env_bound, target_trait_ty, visited)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 子方法 2：检查全局的 Impl 块
    fn check_trait_impl_in_global_impls(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<DefId>,
    ) -> bool {
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
            if let Some(trait_ast) = &impl_def.trait_type {
                let impl_target_ty = self
                    .ctx
                    .node_types
                    .get(&impl_def.target_type.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let impl_trait_ty = self
                    .ctx
                    .node_types
                    .get(&trait_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);

                if impl_target_ty == TypeId::ERROR || impl_trait_ty == TypeId::ERROR {
                    continue;
                }

                let mut map = HashMap::new();

                if self.unify(impl_target_ty, source_ty, &mut map) {
                    let instantiated_trait_ty = {
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                        subst.substitute(impl_trait_ty)
                    };

                    let inst_norm = self.resolve_tv(instantiated_trait_ty);
                    let target_norm = self.resolve_tv(target_trait_ty);

                    if inst_norm == target_norm || instantiated_trait_ty == target_trait_ty {
                        return true;
                    }

                    if let TypeKind::TraitObject(inst_def_id, _) =
                        self.ctx.type_registry.get(inst_norm)
                        && visited.insert(*inst_def_id)
                        && let Def::Trait(trait_def) = self.ctx.defs[inst_def_id.0 as usize].clone()
                    {
                        // 检查父特征 (Supertraits)
                        for &super_ty in &trait_def.resolved_supertraits {
                            let inst_super_ty = {
                                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                                subst.substitute(super_ty)
                            };

                            if inst_super_ty == target_trait_ty
                                || self.check_trait_impl_inner(source_ty, inst_super_ty, visited)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// 助手 格式化并输出类型不匹配错误
    pub fn emit_mismatch_error(&mut self, span: Span, expected: TypeId, actual: TypeId) {
        let exp_str = self.ctx.ty_to_string(expected);
        let act_str = self.ctx.ty_to_string(actual);

        self.ctx
            .struct_error(span, "mismatched types")
            .with_hint(format!("expected `{}`", exp_str))
            .with_hint(format!("   found `{}`", act_str))
            .emit();
    }
}
