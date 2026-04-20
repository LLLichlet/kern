use super::*;

use crate::ty::{ConstGeneric, GenericArg};

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(super) fn check_closure_coercions(
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

        if expected_mut && !self.can_take_mut_address_of(expr) {
            self.ctx
                .struct_error(
                    expr.span,
                    "cannot implicitly borrow an immutable closure as a mutable closure `*mut Fn`",
                )
                .with_code(DiagnosticCode::RequiresLetMut)
                .with_hint(
                    "consider declaring the closure variable as `let mut`, or pass a closure expression that can be materialized into a mutable stack temporary",
                )
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

        let mut map = FastHashMap::default();
        for (expected, actual) in expected_params.iter().zip(actual_params.iter()) {
            if !self.unify(*expected, *actual, &mut map) {
                return false;
            }
        }

        self.unify(expected_ret, actual_ret, &mut map)
    }

    /// Resolve the concrete signature of a function item after generic substitution.
    fn extract_fn_sig_for_bnc(
        &mut self,
        act_kind: &TypeKind,
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        match act_kind {
            TypeKind::FnDef(def_id, args) => self.instantiate_fn_def_signature(
                *def_id,
                &crate::ty::erase_non_type_generic_args(args),
                span,
            ),
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

        let mut map = FastHashMap::default();
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

    fn unify_signature_shape<S: BuildHasher>(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        actual_params: &[TypeId],
        actual_ret: TypeId,
        map: &mut HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        expected_params.len() == actual_params.len()
            && expected_params
                .iter()
                .zip(actual_params.iter())
                .all(|(expected, actual)| self.unify(*expected, *actual, map))
            && self.unify(expected_ret, actual_ret, map)
    }

    fn unify_closure_interface_with_concrete<S: BuildHasher>(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        concrete_kind: &TypeKind,
        map: &mut HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        match concrete_kind {
            TypeKind::AnonymousState { params, ret, .. }
            | TypeKind::ClosureInterface { params, ret } => {
                self.unify_signature_shape(expected_params, expected_ret, params, *ret, map)
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic: false,
            } => self.unify_signature_shape(expected_params, expected_ret, params, *ret, map),
            TypeKind::FnDef(def_id, args) => {
                let Some((params, ret)) = self.instantiate_fn_def_signature(
                    *def_id,
                    &crate::ty::erase_non_type_generic_args(args),
                    Span::default(),
                ) else {
                    return false;
                };
                self.unify_signature_shape(expected_params, expected_ret, &params, ret, map)
            }
            _ => false,
        }
    }

    fn unify_function_with_concrete<S: BuildHasher>(
        &mut self,
        expected_params: &[TypeId],
        expected_ret: TypeId,
        concrete_kind: &TypeKind,
        map: &mut HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        match concrete_kind {
            TypeKind::AnonymousState {
                captures,
                params,
                ret,
                ..
            } => {
                captures.is_empty()
                    && self.unify_signature_shape(expected_params, expected_ret, params, *ret, map)
            }
            TypeKind::Function {
                params,
                ret,
                is_variadic: false,
            }
            | TypeKind::ClosureInterface { params, ret } => {
                self.unify_signature_shape(expected_params, expected_ret, params, *ret, map)
            }
            TypeKind::FnDef(def_id, args) => {
                let Some((params, ret)) = self.instantiate_fn_def_signature(
                    *def_id,
                    &crate::ty::erase_non_type_generic_args(args),
                    Span::default(),
                ) else {
                    return false;
                };
                self.unify_signature_shape(expected_params, expected_ret, &params, ret, map)
            }
            _ => false,
        }
    }

    pub(crate) fn unify<S: BuildHasher>(
        &mut self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        map: &mut HashMap<SymbolId, TypeId, S>,
    ) -> bool {
        let mut const_map = FastHashMap::default();
        self.unify_with_const_map(generic_ty, concrete_ty, map, &mut const_map)
    }

    pub(crate) fn unify_with_const_map<TS: BuildHasher, CS: BuildHasher>(
        &mut self,
        generic_ty: TypeId,
        concrete_ty: TypeId,
        type_map: &mut HashMap<SymbolId, TypeId, TS>,
        const_map: &mut HashMap<SymbolId, ConstGeneric, CS>,
    ) -> bool {
        let gen_norm = self.resolve_tv(generic_ty);
        let con_norm = self.resolve_tv(concrete_ty);

        let gen_kind = self.ctx.type_registry.get(gen_norm).clone();
        let con_kind = self.ctx.type_registry.get(con_norm).clone();

        match (gen_kind, con_kind) {
            (TypeKind::TypeVar(vid), _) => {
                self.bind_type_var(vid, concrete_ty);
                true
            }
            (_, TypeKind::TypeVar(vid)) => {
                self.bind_type_var(vid, generic_ty);
                true
            }
            (TypeKind::Param(name), _) => {
                if let Some(&existing_ty) = type_map.get(&name) {
                    existing_ty == concrete_ty
                } else if matches!(self.ctx.type_registry.get(con_norm), TypeKind::Param(other) if *other == name)
                {
                    type_map.insert(name, concrete_ty);
                    true
                } else if self.generic_param_occurs_in_type_with_map(name, concrete_ty, type_map) {
                    false
                } else {
                    type_map.insert(name, concrete_ty);
                    true
                }
            }
            // Pointer and slice unification must respect mutability.
            (
                TypeKind::Pointer {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::Pointer {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify_with_const_map(g_e, c_e, type_map, const_map),
            (TypeKind::Pointer { elem: g_e, .. }, concrete_kind) => {
                let g_inner = self.resolve_tv(g_e);
                if let TypeKind::ClosureInterface { params, ret } =
                    self.ctx.type_registry.get(g_inner).clone()
                {
                    self.unify_closure_interface_with_concrete(
                        &params,
                        ret,
                        &concrete_kind,
                        type_map,
                    )
                } else {
                    false
                }
            }
            (
                TypeKind::VolatilePtr {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::VolatilePtr {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify_with_const_map(g_e, c_e, type_map, const_map),
            (
                TypeKind::Slice {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::Slice {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify_with_const_map(g_e, c_e, type_map, const_map),
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
            ) => {
                g_m == c_m
                    && self.unify_const_generic_with_map(g_l, c_l, const_map)
                    && self.unify_with_const_map(g_e, c_e, type_map, const_map)
            }
            (
                TypeKind::ArrayInfer {
                    is_mut: g_m,
                    elem: g_e,
                },
                TypeKind::ArrayInfer {
                    is_mut: c_m,
                    elem: c_e,
                },
            ) => g_m == c_m && self.unify_with_const_map(g_e, c_e, type_map, const_map),
            (
                TypeKind::Function {
                    params,
                    ret,
                    is_variadic: false,
                },
                concrete_kind,
            ) => self.unify_function_with_concrete(&params, ret, &concrete_kind, type_map),

            (TypeKind::Def(g_id, g_args), TypeKind::Def(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify_generic_arg_with_map(*ga, *ca, type_map, const_map))
            }
            (TypeKind::Enum(g_id, g_args), TypeKind::Enum(c_id, c_args)) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify_generic_arg_with_map(*ga, *ca, type_map, const_map))
            }
            (
                TypeKind::TraitObject(g_id, g_args, g_assoc_bindings),
                TypeKind::TraitObject(c_id, c_args, c_assoc_bindings),
            ) if g_id == c_id => {
                if g_args.len() != c_args.len() {
                    return false;
                }
                if !g_args
                    .iter()
                    .zip(c_args.iter())
                    .all(|(ga, ca)| self.unify_generic_arg_with_map(*ga, *ca, type_map, const_map))
                {
                    return false;
                }

                let concrete_assoc_bindings =
                    c_assoc_bindings.into_iter().collect::<FastHashMap<_, _>>();
                g_assoc_bindings
                    .into_iter()
                    .all(|(assoc_def_id, generic_assoc_ty)| {
                        let Some(&concrete_assoc_ty) = concrete_assoc_bindings.get(&assoc_def_id)
                        else {
                            return false;
                        };
                        self.unify_with_const_map(
                            generic_assoc_ty,
                            concrete_assoc_ty,
                            type_map,
                            const_map,
                        )
                    })
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
                        .all(|(g, c)| self.unify_with_const_map(*g, *c, type_map, const_map))
                    && self.unify_with_const_map(gr, cr, type_map, const_map)
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
                        .all(|(g, c)| self.unify_with_const_map(*g, *c, type_map, const_map))
                    && gp
                        .iter()
                        .zip(cp.iter())
                        .all(|(g, c)| self.unify_with_const_map(*g, *c, type_map, const_map))
                    && self.unify_with_const_map(gr, cr, type_map, const_map)
            }
            (TypeKind::AnonymousEnum(ge), TypeKind::AnonymousEnum(ce)) => {
                if ge.builtin != ce.builtin
                    || ge.backing_ty.is_some() != ce.backing_ty.is_some()
                    || ge.variants.len() != ce.variants.len()
                {
                    return false;
                }

                if let (Some(g_backing), Some(c_backing)) = (ge.backing_ty, ce.backing_ty)
                    && !self.unify_with_const_map(g_backing, c_backing, type_map, const_map)
                {
                    return false;
                }

                ge.variants.iter().zip(ce.variants.iter()).all(|(gv, cv)| {
                    if gv.name != cv.name
                        || gv.explicit_value != cv.explicit_value
                        || gv.payload_ty.is_some() != cv.payload_ty.is_some()
                    {
                        return false;
                    }

                    match (gv.payload_ty, cv.payload_ty) {
                        (Some(g_payload), Some(c_payload)) => {
                            self.unify_with_const_map(g_payload, c_payload, type_map, const_map)
                        }
                        (None, None) => true,
                        _ => false,
                    }
                })
            }
            _ => gen_norm == con_norm,
        }
    }

    fn unify_generic_arg_with_map<TS: BuildHasher, CS: BuildHasher>(
        &mut self,
        generic: GenericArg,
        concrete: GenericArg,
        type_map: &mut HashMap<SymbolId, TypeId, TS>,
        const_map: &mut HashMap<SymbolId, ConstGeneric, CS>,
    ) -> bool {
        match (generic, concrete) {
            (GenericArg::Type(generic_ty), GenericArg::Type(concrete_ty)) => {
                self.unify_with_const_map(generic_ty, concrete_ty, type_map, const_map)
            }
            (GenericArg::Const(generic), GenericArg::Const(concrete)) => {
                self.unify_const_generic_with_map(generic, concrete, const_map)
            }
            _ => false,
        }
    }

    fn unify_const_generic_with_map<CS: BuildHasher>(
        &mut self,
        generic: ConstGeneric,
        concrete: ConstGeneric,
        const_map: &mut HashMap<SymbolId, ConstGeneric, CS>,
    ) -> bool {
        let generic = {
            let subst_map = const_map
                .iter()
                .map(|(&name, &value)| (name, GenericArg::Const(value)))
                .collect::<FastHashMap<_, _>>();
            if subst_map.is_empty() {
                self.ctx.type_registry.fold_const_generic(generic)
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute_const_generic(generic)
            }
        };
        let concrete = {
            let subst_map = const_map
                .iter()
                .map(|(&name, &value)| (name, GenericArg::Const(value)))
                .collect::<FastHashMap<_, _>>();
            if subst_map.is_empty() {
                self.ctx.type_registry.fold_const_generic(concrete)
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
                subst.substitute_const_generic(concrete)
            }
        };

        let generic_ty = self.ctx.type_registry.const_generic_ty(generic);
        let concrete_ty = self.ctx.type_registry.const_generic_ty(concrete);
        if generic_ty != concrete_ty {
            return false;
        }

        match (generic, concrete) {
            (ConstGeneric::Param(name, _), other) => {
                if let Some(&existing) = const_map.get(&name) {
                    existing == other
                } else if self.const_param_occurs_in_const_generic_with_map(name, other, const_map)
                {
                    false
                } else {
                    const_map.insert(name, other);
                    true
                }
            }
            (other, ConstGeneric::Param(name, _)) => {
                if let Some(&existing) = const_map.get(&name) {
                    existing == other
                } else if self.const_param_occurs_in_const_generic_with_map(name, other, const_map)
                {
                    false
                } else {
                    const_map.insert(name, other);
                    true
                }
            }
            (ConstGeneric::Expr(_), ConstGeneric::Expr(_))
            | (ConstGeneric::Expr(_), ConstGeneric::Value(_))
            | (ConstGeneric::Value(_), ConstGeneric::Expr(_))
            | (ConstGeneric::Value(_), ConstGeneric::Value(_))
            | (ConstGeneric::Error, _)
            | (_, ConstGeneric::Error) => generic == concrete,
        }
    }
}
