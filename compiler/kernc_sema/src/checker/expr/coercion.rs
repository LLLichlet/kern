use super::ExprChecker;
use crate::checker::Substituter;
use crate::def::{Def, DefId};
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{Expr, ExprKind, UnaryOperator};
use kernc_utils::{Span, SymbolId};
use std::collections::{HashMap, HashSet};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_coercion(&mut self, span: Span, expected: TypeId, actual: TypeId) -> bool {
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
        if let TypeKind::TypeVar(vid) = act_kind {
            self.type_vars[vid as usize] = Some(exp);
            return true;
        }
        if let TypeKind::TypeVar(vid) = exp_kind {
            self.type_vars[vid as usize] = Some(act);
            return true;
        }

        // 2. 指针降级与 Trait Object 隐式打包
        if let TypeKind::Pointer {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            if let TypeKind::Pointer {
                is_mut: a_mut,
                elem: a_inner,
            } = act_kind
            {
                // 权限校验：不可变指针绝不能升级为可变指针
                if !e_mut || (e_mut && a_mut) {
                    let e_norm = self.resolve_tv(e_inner);
                    let a_norm = self.resolve_tv(a_inner);

                    // a) 基础指针安全降级 (*mut T -> *T)
                    if e_norm == a_norm {
                        return true;
                    }

                    // b) 指针隐式打包为 Trait Object (*mut Type -> *mut Trait)
                    if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm) {
                        // 剥离可变性，构造一个不可变的实际指针类型去查 Trait
                        let downgraded_act = if !e_mut && a_mut {
                            self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut: false,
                                elem: a_inner,
                            })
                        } else {
                            act
                        };

                        if self.check_trait_impl(downgraded_act, e_norm) {
                            return true;
                        }
                    }
                }
            }
        }

        // 同样处理易失指针
        if let TypeKind::VolatilePtr {
            is_mut: e_mut,
            elem: e_inner,
        } = exp_kind
        {
            if let TypeKind::VolatilePtr {
                is_mut: a_mut,
                elem: a_inner,
            } = act_kind
            {
                if !e_mut || (e_mut && a_mut) {
                    let e_norm = self.resolve_tv(e_inner);
                    let a_norm = self.resolve_tv(a_inner);
                    if e_norm == a_norm {
                        return true;
                    }
                    if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm) {
                        if self.check_trait_impl(act, e_norm) {
                            return true;
                        }
                    }
                }
            }
        }

        // 3. 切片降级与数组退化
        if let TypeKind::Slice {
            is_mut: e_mut,
            elem: exp_elem,
        } = exp_kind
        {
            if let TypeKind::Slice {
                is_mut: act_mut,
                elem: act_elem,
            } = act_kind
            {
                if (!e_mut || (e_mut && act_mut))
                    && self.resolve_tv(exp_elem) == self.resolve_tv(act_elem)
                {
                    return true;
                }
            }
            match self.check_array_decay(e_mut, exp_elem, &act_kind, span) {
                Ok(true) => return true,
                Err(()) => return false,
                Ok(false) => {}
            }
        }

        self.emit_mismatch_error(span, expected, actual);
        false
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
                    TypeKind::Array { is_mut, .. } => is_mut,
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

    // TODO: Devide
    fn check_trait_impl_inner(
        &mut self,
        source_ty: TypeId,
        target_trait_ty: TypeId,
        visited: &mut std::collections::HashSet<DefId>,
    ) -> bool {
        // === 1. 优先检查当前环境上下文中的 Where 约束 (active_bounds) ===
        // 解决函数体内泛型参数自身满足约束的推导问题
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
                    {
                        if visited.insert(*inst_def_id) {
                            if self.check_trait_impl_inner(inst_env_bound, target_trait_ty, visited)
                            {
                                return true;
                            }
                        }
                    }
                }
            }
        }

        // === 2. 全局的 impl 块 ===
        let mut impl_blocks = Vec::new();
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                impl_blocks.push(impl_def.clone());
            }
        }

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

                    let inst_norm = self.resolve_tv(instantiated_trait_ty);
                    if let TypeKind::TraitObject(inst_def_id, _) =
                        self.ctx.type_registry.get(inst_norm)
                    {
                        if visited.insert(*inst_def_id) {
                            if let Def::Trait(trait_def) =
                                self.ctx.defs[inst_def_id.0 as usize].clone()
                            {
                                // 直接使用已经解析好的 resolved_supertraits
                                for &super_ty in &trait_def.resolved_supertraits {
                                    // 依然需要将 trait 自身的泛型代入父特征 (例如 TraitA[T] : TraitB[T])
                                    let inst_super_ty = {
                                        let mut subst =
                                            Substituter::new(&mut self.ctx.type_registry, &map);
                                        subst.substitute(super_ty)
                                    };

                                    if inst_super_ty == target_trait_ty
                                        || self.check_trait_impl_inner(
                                            source_ty,
                                            inst_super_ty,
                                            visited,
                                        )
                                    {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// 助手 格式化并输出类型不匹配错误
    fn emit_mismatch_error(&mut self, span: Span, expected: TypeId, actual: TypeId) {
        let exp_str = self.ctx.ty_to_string(expected);
        let act_str = self.ctx.ty_to_string(actual);

        self.ctx
            .struct_error(span, "mismatched types")
            .with_hint(format!("expected `{}`", exp_str))
            .with_hint(format!("   found `{}`", act_str))
            .emit();
    }
}
