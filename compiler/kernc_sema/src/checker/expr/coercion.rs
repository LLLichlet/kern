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

        // 2. 值语义的结构体降级 (具名 -> 匿名) ===
        if self.is_struct_structurally_equivalent(exp, act) {
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

    fn check_type_var(&mut self, exp: TypeId, act: TypeId, exp_kind: &TypeKind, act_kind: &TypeKind) -> bool {
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
        if let TypeKind::Pointer { is_mut: e_mut, elem: e_inner } = exp_kind {
            let e_norm = self.resolve_tv(*e_inner);

            // A. 指针到指针的安全降级 (*mut T -> *T)
            if let TypeKind::Pointer { is_mut: a_mut, elem: a_inner } = act_kind {
                // 不可变指针绝不能升级为可变指针
                if !*e_mut || (*e_mut && *a_mut) {
                    let a_norm = self.resolve_tv(*a_inner);
                    if e_norm == a_norm {
                        return true;
                    }

                    // 1. *void 万能指针隐式降级 
                    // 只要目标期望类型是 void（且满足上方的 mut 安全性校验），则允许任何非胖指针自然降级
                    if self.ctx.type_registry.is_void(e_norm) {
                        return true;
                    }

                    // 2. 具名结构体 -> 匿名结构体
                    if self.is_struct_structurally_equivalent(e_norm, a_norm) {
                        return true;
                    }
                    
                    // 3. 指针隐式打包为 Trait Object (*mut Type -> *mut Trait)
                    if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm) {
                        let downgraded_act = if !*e_mut && *a_mut {
                            self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut: false,
                                elem: *a_inner,
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

            // D. BNC: 裸值到 Trait Object (T -> *Trait / T -> *mut Trait)
            if let TypeKind::TraitObject(..) = self.ctx.type_registry.get(e_norm) {
                // 如果 actual 并不是一个指针，而是一个裸值 (T)
                if !matches!(act_kind, TypeKind::Pointer { .. } | TypeKind::VolatilePtr { .. }) {
                    // 安全性检查：如果期望的是 *mut Trait，那么传入的 expr 必须是可变的
                    if *e_mut && !self.is_lvalue_mutable(expr) {
                        self.ctx
                            .struct_error(
                                expr.span,
                                "cannot implicitly borrow an immutable value as a mutable trait object `*mut Trait`",
                            )
                            .with_hint("consider declaring the variable as `let mut`")
                            .emit();
                        return false; // 可变性校验失败
                    }

                    // 构造一个虚拟的指针类型去查 Trait 约束表
                    let virtual_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
                        is_mut: *e_mut,
                        elem: act,
                    });

                    if self.check_trait_impl(virtual_ptr_ty, e_norm) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 核心辅助方法：检查一个具名结构体 (act_def) 能否降级为匿名结构体 (exp_anon)
    pub(crate) fn is_struct_structurally_equivalent(&mut self, exp_anon: TypeId, act_def: TypeId) -> bool {
        let exp_kind = self.ctx.type_registry.get(exp_anon).clone();
        let act_kind = self.ctx.type_registry.get(act_def).clone();

        let (exp_is_extern, exp_fields) = if let TypeKind::AnonymousStruct(is_ext, ref f) = exp_kind {
            (is_ext, f)
        } else {
            return false;
        };

        if let TypeKind::Def(def_id, ref act_args) = act_kind {
            let act_def_clone = self.ctx.defs[def_id.0 as usize].clone();

            if let Def::Struct(act_s) = act_def_clone {
                // extern 状态不一致不允许降级转换
                if exp_is_extern != act_s.is_extern {
                    return false;
                }

                // 字段数量不同，绝对不兼容
                if exp_fields.len() != act_s.fields.len() {
                    return false;
                }

                // 提取并替换具名结构体的真实字段类型
                let mut act_fields = Vec::new();
                for f in &act_s.fields {
                    let raw_ty = self.ctx.node_types.get(&f.type_node.id).copied().unwrap_or(TypeId::ERROR);
                    
                    let inst_ty = if !act_args.is_empty() {
                        let mut map = std::collections::HashMap::new();
                        for (i, param) in act_s.generics.iter().enumerate() {
                            map.insert(param.name, act_args[i]);
                        }
                        let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                        subst.substitute(raw_ty)
                    } else {
                        raw_ty
                    };

                    act_fields.push((f.name, self.resolve_tv(inst_ty)));
                }

                // 排序具名的字段，以便与匿名结构体进行zip比对
                act_fields.sort_by_key(|f| f.0);

                for (exp_f, act_f) in exp_fields.iter().zip(act_fields.iter()) {
                    if exp_f.name != act_f.0 || self.resolve_tv(exp_f.ty) != act_f.1 {
                        return false;
                    }
                }

                return true;
            }
        }
        false
    }
    fn check_volatile_coercions(
        &mut self,
        _expr: &Expr,
        _exp: TypeId,
        act: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::VolatilePtr { is_mut: e_mut, elem: e_inner } = exp_kind {
            if let TypeKind::VolatilePtr { is_mut: a_mut, elem: a_inner } = act_kind {
                if !*e_mut || (*e_mut && *a_mut) {
                    let e_norm = self.resolve_tv(*e_inner);
                    let a_norm = self.resolve_tv(*a_inner);
                    if e_norm == a_norm {
                        return true;
                    }
                    if self.ctx.type_registry.is_void(e_norm) {
                        return true;
                    }
                    if self.is_struct_structurally_equivalent(e_norm, a_norm) {
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
        false
    }

    fn check_slice_and_array_decay(
        &mut self,
        expr: &Expr,
        _exp: TypeId,
        exp_kind: &TypeKind,
        act_kind: &TypeKind,
    ) -> bool {
        if let TypeKind::Slice { is_mut: e_mut, elem: exp_elem } = exp_kind {
            if let TypeKind::Slice { is_mut: act_mut, elem: act_elem } = act_kind {
                if (!*e_mut || (*e_mut && *act_mut))
                    && self.resolve_tv(*exp_elem) == self.resolve_tv(*act_elem)
                {
                    return true;
                }
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
        // A. 闭包退化规则: AnonymousState -> C-ABI 函数指针 (捕获必须严格为空)
        if let TypeKind::Function { params: e_params, ret: e_ret, is_variadic: false } = exp_kind {
            if let TypeKind::AnonymousState { captures: a_caps, params: a_params, ret: a_ret, .. } = act_kind {
                if a_caps.is_empty() && e_params.len() == a_params.len() {
                    let mut map = HashMap::new();
                    let mut ok = true;
                    for (ep, ap) in e_params.iter().zip(a_params.iter()) {
                        if !self.unify(*ep, *ap, &mut map) { ok = false; break; }
                    }
                    if ok && self.unify(*e_ret, *a_ret, &mut map) {
                        return true;
                    }
                }
            }
        }

        // B. 闭包 BNC: 到动态胖指针接口
        if let TypeKind::Pointer { is_mut: e_mut, elem: e_inner } = exp_kind {
            let e_norm = self.resolve_tv(*e_inner);
            if let TypeKind::ClosureInterface { params: ref e_params, ret: e_ret } = self.ctx.type_registry.get(e_norm).clone() {
                
                // B.1 AnonymousState -> *Fn / *mut Fn
                if let TypeKind::AnonymousState { params: a_params, ret: a_ret, .. } = act_kind {
                    // 可变性安全检查：如果期待 *mut Fn，则闭包本身（匿名结构体）必须是可变的
                    if *e_mut && !self.is_lvalue_mutable(expr) {
                        self.ctx.struct_error(expr.span, "cannot implicitly borrow an immutable closure as a mutable closure `*mut Fn`")
                            .with_hint("consider declaring the closure variable as `let mut`")
                            .emit();
                        return false;
                    }

                    if e_params.len() == a_params.len() {
                        let mut map = HashMap::new();
                        let mut ok = true;
                        for (ep, ap) in e_params.iter().zip(a_params.iter()) {
                            if !self.unify(*ep, *ap, &mut map) { ok = false; break; }
                        }
                        if ok && self.unify(e_ret, *a_ret, &mut map) {
                            return true;
                        }
                    }
                }

                // B.2 Function / FnDef -> *Fn / *mut Fn (普通函数无状态，安全地塞入闭包胖指针)
                if matches!(act_kind, TypeKind::Function { is_variadic: false, .. } | TypeKind::FnDef(_, _)) {
                    let (a_params, a_ret) = self.extract_fn_sig_for_bnc(act_kind, expr.span);
                    
                    if e_params.len() == a_params.len() {
                        let mut map = HashMap::new();
                        let mut ok = true;
                        for (ep, ap) in e_params.iter().zip(a_params.iter()) {
                            if !self.unify(*ep, *ap, &mut map) { ok = false; break; }
                        }
                        if ok && self.unify(e_ret, a_ret, &mut map) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// 提取函数定义项 (FnDef) 的确切签名，处理泛型代入
    fn extract_fn_sig_for_bnc(&mut self, act_kind: &TypeKind, span: Span) -> (Vec<TypeId>, TypeId) {
        match act_kind {
            TypeKind::FnDef(def_id, args) => {
                // 为了避免 self 生命周期冲突，我们 clone 一下 Def
                let def = self.ctx.defs[def_id.0 as usize].clone(); 
                if let Def::Function(fn_def) = def {
                    if let Some(sig_ty) = fn_def.resolved_sig {
                        let norm_sig = self.resolve_tv(sig_ty);
                        if let TypeKind::Function { params, ret, .. } = self.ctx.type_registry.get(norm_sig).clone() {
                            if fn_def.generics.is_empty() {
                                return (params, ret);
                            } else {
                                // 处理泛型实例化
                                let mut map = HashMap::new();
                                for (i, param) in fn_def.generics.iter().enumerate() {
                                    map.insert(param.name, args[i]);
                                }
                                let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                                let inst_params = params.into_iter().map(|p| subst.substitute(p)).collect();
                                let inst_ret = subst.substitute(ret);
                                return (inst_params, inst_ret);
                            }
                        }
                    }
                    self.ctx.emit_error(span, "Compiler ICE: Function definition lacks resolved signature during BNC");
                    unreachable!()
                } else {
                    self.ctx.emit_error(span, "Compiler ICE: FnDef ID does not point to a Function Def");
                    unreachable!()
                }
            }
            TypeKind::Function { params, ret, .. } => {
                (params.clone(), *ret)
            }
            _ => unreachable!()
        }
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
                TypeKind::ClosureInterface { params: gp, ret: gr },
                TypeKind::ClosureInterface { params: cp, ret: cr },
            ) => {
                gp.len() == cp.len() 
                    && gp.iter().zip(cp.iter()).all(|(g, c)| self.unify(*g, *c, map)) 
                    && self.unify(gr, cr, map)
            }

            (
                TypeKind::AnonymousState { captures: gc, params: gp, ret: gr, .. },
                TypeKind::AnonymousState { captures: cc, params: cp, ret: cr, .. },
            ) => {
                gc.len() == cc.len() && gp.len() == cp.len()
                    && gc.iter().zip(cc.iter()).all(|(g, c)| self.unify(*g, *c, map))
                    && gp.iter().zip(cp.iter()).all(|(g, c)| self.unify(*g, *c, map))
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
                    if let TypeKind::TraitObject(inst_def_id, _) = self.ctx.type_registry.get(inst_norm) {
                        if visited.insert(*inst_def_id) {
                            if self.check_trait_impl_inner(inst_env_bound, target_trait_ty, visited) {
                                return true;
                            }
                        }
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

                    if let TypeKind::TraitObject(inst_def_id, _) = self.ctx.type_registry.get(inst_norm) {
                        if visited.insert(*inst_def_id) {
                            if let Def::Trait(trait_def) = self.ctx.defs[inst_def_id.0 as usize].clone() {
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
