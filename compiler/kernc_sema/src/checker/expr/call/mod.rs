use super::ExprChecker;
use crate::def::{Def, DefId};
use crate::passes::TypeResolver;
use crate::scope::{SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{ConstGeneric, GenericArg, TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind, Visibility};
use kernc_utils::{FastHashMap, Span};

mod asm;
mod intrinsic;
mod signature;

struct SignatureDeductionInput<'a> {
    args: &'a [Expr],
    is_method: bool,
    receiver_ty: TypeId,
    expected_ty: Option<TypeId>,
    span: Span,
    has_user_explicit_generics: bool,
}

#[derive(Clone)]
struct ArgumentInferredMethodCandidate {
    impl_id: DefId,
    method_id: DefId,
    method_span: Span,
    impl_args: Vec<GenericArg>,
    receiver_ty: TypeId,
}

impl<'a, 'ctx> ExprChecker<'a, 'ctx> {
    pub(crate) fn check_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        expected_ty: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        if let ExprKind::Identifier(sym) = &callee.kind
            && self.ctx.resolve(*sym) == "@asm"
        {
            self.ctx.set_node_type(callee.id, TypeId::VOID);
            return self.check_asm_call(args, span);
        }

        let callee_ty = self.with_uninstantiated_generic_function_items_allowed(|this| {
            this.check_method_callee_expr(callee)
                .or_else(|| this.check_method_callee_expr_with_arguments(callee, args))
                .unwrap_or_else(|| this.check_expr(callee, None))
        });
        let norm_callee = self.resolve_tv(callee_ty);

        if norm_callee == TypeId::ERROR {
            for arg in args {
                self.check_expr(arg, None);
            }
            return TypeId::ERROR;
        }

        let (is_method, receiver_ty) = self.resolve_method_context(callee);
        let has_user_explicit_generics =
            matches!(callee.kind, ExprKind::GenericInstantiation { .. });

        let signature_started = self.timing_start();
        let (sig_ty, inferred_callee_ty, inferred_arg_tys) = self.deduce_and_resolve_signature(
            norm_callee,
            SignatureDeductionInput {
                args,
                is_method,
                receiver_ty,
                expected_ty,
                span: callee.span,
                has_user_explicit_generics,
            },
        );
        self.record_expr_timing(signature_started, |stats, elapsed| {
            stats.call_signature += elapsed;
        });

        if let Some(fixed_ty) = inferred_callee_ty {
            self.ctx.set_node_type(callee.id, fixed_ty);
        }

        if sig_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

        let grouped_field_method_hint = self.grouped_field_method_call_hint(callee);

        let (params_ptr, ret, is_variadic) = match self.ctx.type_registry.get(sig_ty) {
            TypeKind::Function {
                params,
                ret,
                is_variadic,
            } => (std::ptr::from_ref(params.as_slice()), *ret, *is_variadic),
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                let inner_norm = self.ctx.type_registry.normalize(*elem);
                if let TypeKind::ClosureInterface { params, ret } =
                    self.ctx.type_registry.get(inner_norm)
                {
                    (std::ptr::from_ref(params.as_slice()), *ret, false)
                } else {
                    let callee_str = self.ctx.ty_to_string(callee_ty);
                    let mut diag = self
                        .ctx
                        .struct_error(callee.span, "expression is not callable")
                        .with_hint(format!("type is `{}`", callee_str));
                    if let Some(hint) = &grouped_field_method_hint {
                        diag = diag.with_hint(hint.clone());
                    }
                    diag.emit();
                    return TypeId::ERROR;
                }
            }
            _ => {
                let callee_str = self.ctx.ty_to_string(callee_ty);
                let mut diag = self
                    .ctx
                    .struct_error(callee.span, "expression is not callable")
                    .with_hint(format!("type is `{}`", callee_str));
                if let Some(hint) = &grouped_field_method_hint {
                    diag = diag.with_hint(hint.clone());
                }
                diag.emit();
                return TypeId::ERROR;
            }
        };
        let params = unsafe { &*params_ptr };

        self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

        if is_method && !params.is_empty() {
            let expected_self = params[0];
            self.check_method_receiver(expected_self, receiver_ty, callee);
        }

        let mut final_ret = ret;
        let intrinsic_started = self.timing_start();
        let handled_intrinsic = self
            .intrinsic_def_from_callee_ty(inferred_callee_ty.unwrap_or(norm_callee))
            .and_then(|def_id| {
                let intrinsic_name = match &self.ctx.defs[def_id.0 as usize] {
                    Def::Function(func) if func.is_intrinsic => {
                        Some(self.ctx.resolve(func.name).to_string())
                    }
                    _ => None,
                }?;

                let atomic_handled = self.check_atomic_intrinsic_call(
                    intrinsic_name.as_str(),
                    inferred_callee_ty.unwrap_or(norm_callee),
                    args,
                    params,
                );
                if intrinsic_name == "@loc" {
                    final_ret = self.loc_intrinsic_result_type(span);
                }
                let bit_ret = self.check_bit_intrinsic_call(
                    intrinsic_name.as_str(),
                    inferred_callee_ty.unwrap_or(norm_callee),
                    args,
                    params,
                    ret,
                );
                if let Some(bit_ret) = bit_ret {
                    final_ret = bit_ret;
                }
                let simd_ret = self.check_simd_intrinsic_call(
                    intrinsic_name.as_str(),
                    inferred_callee_ty.unwrap_or(norm_callee),
                    args,
                    params,
                    ret,
                );
                if let Some(simd_ret) = simd_ret {
                    final_ret = simd_ret;
                }

                Some(atomic_handled || bit_ret.is_some() || simd_ret.is_some())
            })
            .unwrap_or(false);
        self.record_expr_timing(intrinsic_started, |stats, elapsed| {
            stats.call_intrinsic += elapsed;
        });

        if !handled_intrinsic {
            let arguments_started = self.timing_start();
            self.check_call_arguments(
                args,
                params,
                is_method,
                is_variadic,
                inferred_arg_tys.as_deref(),
            );
            self.record_expr_timing(arguments_started, |stats, elapsed| {
                stats.call_arguments += elapsed;
            });
        }
        self.record_pending_temporary_address_escape_checks(
            inferred_callee_ty.unwrap_or(norm_callee),
            args,
            is_method,
        );
        final_ret
    }

    fn record_pending_temporary_address_escape_checks(
        &mut self,
        callee_ty: TypeId,
        args: &[Expr],
        is_method: bool,
    ) {
        let norm_callee = self.resolve_tv(callee_ty);
        let TypeKind::FnDef(def_id, _) = self.ctx.type_registry.get(norm_callee).clone() else {
            return;
        };
        let param_offset = if is_method { 1 } else { 0 };

        for (arg_index, arg) in args.iter().enumerate() {
            let origins = self.pointer_origins(arg);
            let param_index = arg_index + param_offset;
            for origin in origins {
                let crate::checker::expr::PointerOrigin::Temporary(address_span) = origin else {
                    continue;
                };
                self.ctx
                    .analysis
                    .pending_escape_checks
                    .push(crate::context::PendingEscapeCheck {
                        callee: def_id,
                        arg_index: param_index,
                        address_span,
                    });
            }
        }
    }

    fn check_method_callee_expr(&mut self, callee: &Expr) -> Option<TypeId> {
        let ExprKind::FieldAccess {
            lhs,
            field,
            field_span,
        } = &callee.kind
        else {
            return None;
        };

        self.check_method_member_access(callee.id, lhs, *field, *field_span, callee.span)
    }

    fn check_method_callee_expr_with_arguments(
        &mut self,
        callee: &Expr,
        args: &[Expr],
    ) -> Option<TypeId> {
        let ExprKind::FieldAccess {
            lhs,
            field,
            field_span,
        } = &callee.kind
        else {
            return None;
        };

        let receiver_ty = self.check_value_or_namespace_expr(lhs);
        if receiver_ty == TypeId::ERROR {
            return Some(TypeId::ERROR);
        }
        if self.receiver_base_is_module(receiver_ty)
            || !self.ctx.has_impl_methods_named(*field)
        {
            return None;
        }

        let arg_tys = args
            .iter()
            .map(|arg| self.check_expr(arg, None))
            .collect::<Vec<_>>();
        if arg_tys.contains(&TypeId::ERROR) {
            return Some(TypeId::ERROR);
        }

        let candidate =
            match self.resolve_argument_inferred_method(receiver_ty, *field, &arg_tys, callee.span)
            {
                Ok(Some(candidate)) => candidate,
                Ok(None) => return None,
                Err(()) => {
                    self.ctx.set_node_type(callee.id, TypeId::ERROR);
                    self.touched_expr_nodes.push(callee.id);
                    return Some(TypeId::ERROR);
                }
            };
        self.ctx
            .record_identifier_reference(*field_span, candidate.method_span);
        if candidate.receiver_ty != receiver_ty {
            self.ctx
                .set_method_owner_ty(callee.id, candidate.receiver_ty);
        }
        let type_id = self
            .ctx
            .type_registry
            .intern(TypeKind::FnDef(candidate.method_id, candidate.impl_args));
        self.ctx.set_node_type(callee.id, type_id);
        self.touched_expr_nodes.push(callee.id);
        Some(type_id)
    }

    fn resolve_argument_inferred_method(
        &mut self,
        receiver_ty: TypeId,
        method_name: kernc_utils::SymbolId,
        arg_tys: &[TypeId],
        span: Span,
    ) -> Result<Option<ArgumentInferredMethodCandidate>, ()> {
        let Some(method_ids) = self.ctx.impl_method_ids_by_name(method_name) else {
            return Ok(None);
        };
        let method_ids = method_ids.to_vec();
        let mut candidates = Vec::new();

        for method_id in method_ids {
            let Some((impl_id, method_span)) =
                self.ctx
                    .defs
                    .get(method_id.0 as usize)
                    .and_then(|def| match def {
                        Def::Function(function) => {
                            function.parent.map(|parent| (parent, function.name_span))
                        }
                        _ => None,
                    })
            else {
                continue;
            };

            if let Some(candidate) = self.infer_impl_method_from_call_arguments(
                receiver_ty,
                arg_tys,
                impl_id,
                method_id,
                method_span,
            ) {
                candidates.push(candidate);
            }
        }

        if candidates.is_empty() {
            return Ok(None);
        }

        let maximal = candidates
            .iter()
            .enumerate()
            .filter(|(index, candidate)| {
                !candidates.iter().enumerate().any(|(other_index, other)| {
                    other_index != *index
                        && matches!(
                            crate::query::compare_impl_specificity(
                                self.ctx,
                                other.impl_id,
                                candidate.impl_id,
                            ),
                            crate::query::ImplSpecificity::LeftMoreSpecific
                        )
                })
            })
            .map(|(_, candidate)| candidate.clone())
            .collect::<Vec<_>>();

        if maximal.len() > 1 {
            let method_name_str = self.ctx.resolve(method_name).to_string();
            self.ctx
                .struct_error(span, format!("ambiguous impl method `{}`", method_name_str))
                .with_hint(
                    "method lookup remains ambiguous after inferring impl generics from call arguments",
                )
                .emit();
            return Err(());
        }

        Ok(maximal.into_iter().next())
    }

    fn infer_impl_method_from_call_arguments(
        &mut self,
        receiver_ty: TypeId,
        arg_tys: &[TypeId],
        impl_id: DefId,
        method_id: DefId,
        method_span: Span,
    ) -> Option<ArgumentInferredMethodCandidate> {
        {
            let mut resolver = TypeResolver::new(self.ctx);
            resolver.ensure_impl_signature_types_resolved(impl_id);
        }

        let impl_def = match self.ctx.defs.get(impl_id.0 as usize)? {
            Def::Impl(impl_def) => impl_def.clone(),
            _ => return None,
        };
        if self
            .ctx
            .direct_self_referential_impl_requirement(&impl_def)
            .is_some()
            || self
                .ctx
                .indirect_self_referential_impl_requirement(impl_id)
                .is_some()
            || self.ctx.non_decreasing_impl_requirement(impl_id).is_some()
        {
            return None;
        }

        let impl_target_ty = self.ctx.node_type_or_error(impl_def.target_type.id);
        let raw_sig = match self.ctx.defs.get(method_id.0 as usize)? {
            Def::Function(function) => function.resolved_sig?,
            _ => return None,
        };
        let (params, is_variadic) = match self.ctx.type_registry.get(raw_sig) {
            TypeKind::Function {
                params,
                is_variadic,
                ..
            } => (params.clone(), *is_variadic),
            _ => return None,
        };

        let required_arg_count = params.len().saturating_sub(1);
        if (!is_variadic && arg_tys.len() != required_arg_count)
            || (is_variadic && arg_tys.len() < required_arg_count)
        {
            return None;
        }

        for search_ty in self.method_receiver_search_types(receiver_ty) {
            let mut type_map = FastHashMap::default();
            let mut const_map = FastHashMap::default();
            if !self.match_available_type_against_requirement(
                impl_target_ty,
                search_ty,
                &mut type_map,
                &mut const_map,
            ) {
                continue;
            }

            if let Some(&self_param_ty) = params.first() {
                let _ = self.infer_generic_args_from_types(
                    self_param_ty,
                    search_ty,
                    &mut type_map,
                    &mut const_map,
                );
            }

            let mut argument_inference_matches = true;
            for (param_ty, arg_ty) in params.iter().skip(1).zip(arg_tys.iter().copied()) {
                let substituted_param =
                    self.substitute_type_with_unification_maps(*param_ty, &type_map, &const_map);
                if self.type_contains_unresolved_params(substituted_param)
                    && !self.infer_generic_args_from_types(
                        substituted_param,
                        arg_ty,
                        &mut type_map,
                        &mut const_map,
                    )
                {
                    argument_inference_matches = false;
                    break;
                }
            }
            if !argument_inference_matches {
                continue;
            }

            if !crate::query::impl_bounds_satisfied(
                self,
                &impl_def.where_clauses,
                &type_map,
                &const_map,
            ) {
                continue;
            }

            let impl_args = impl_def
                .generics
                .iter()
                .map(|param| match &param.kind {
                    ast::GenericParamKind::Type => GenericArg::Type(
                        type_map.get(&param.name).copied().unwrap_or(TypeId::ERROR),
                    ),
                    ast::GenericParamKind::Const { .. } => GenericArg::Const(
                        const_map
                            .get(&param.name)
                            .copied()
                            .unwrap_or(ConstGeneric::Error),
                    ),
                })
                .map(|arg| self.materialize_numeric_defaults_in_generic_arg(arg))
                .collect::<Vec<_>>();

            if crate::query::impl_generic_args_fully_resolved(&impl_args) {
                return Some(ArgumentInferredMethodCandidate {
                    impl_id,
                    method_id,
                    method_span,
                    impl_args,
                    receiver_ty: search_ty,
                });
            }
        }

        None
    }

    fn method_receiver_search_types(&mut self, receiver_ty: TypeId) -> Vec<TypeId> {
        let receiver_norm = self.resolve_tv(receiver_ty);
        let mut base_ty = receiver_ty;
        let base_norm = loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                }
                _ => break norm,
            }
        };
        let mut search_tys = vec![receiver_norm];

        let downgraded = match self.ctx.type_registry.get(receiver_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => Some(TypeKind::Pointer {
                is_mut: false,
                elem,
            }),
            TypeKind::VolatilePtr { is_mut: true, elem } => Some(TypeKind::VolatilePtr {
                is_mut: false,
                elem,
            }),
            TypeKind::Slice { is_mut: true, elem } => Some(TypeKind::Slice {
                is_mut: false,
                elem,
            }),
            _ => None,
        };
        if let Some(kind) = downgraded {
            let ty = self.ctx.type_registry.intern(kind);
            if !search_tys.contains(&ty) {
                search_tys.push(ty);
            }
        }
        if let TypeKind::Array { elem, .. } | TypeKind::ArrayInfer { elem } =
            self.ctx.type_registry.get(receiver_norm).clone()
        {
            let ty = self.ctx.type_registry.intern(TypeKind::Slice {
                is_mut: false,
                elem,
            });
            if !search_tys.contains(&ty) {
                search_tys.push(ty);
            }
        }
        if !search_tys.contains(&base_norm) {
            search_tys.push(base_norm);
        }

        search_tys
    }

    fn receiver_base_is_module(&mut self, receiver_ty: TypeId) -> bool {
        let mut base_ty = receiver_ty;
        loop {
            let norm = self.resolve_tv(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                }
                TypeKind::Module(..) => return true,
                _ => return false,
            }
        }
    }

    fn grouped_field_method_call_hint(&mut self, callee: &Expr) -> Option<String> {
        let ExprKind::Grouped { expr: inner } = &callee.kind else {
            return None;
        };
        let ExprKind::FieldAccess { lhs, field, .. } = &inner.kind else {
            return None;
        };

        let lhs_ty = self.ctx.node_type(lhs.id)?;
        self.try_find_method_silent(lhs_ty, *field, callee.span)?;
        Some(format!(
            "remove the parentheses to call method `{}()`; keep `(expr.{})()` to call a callable field explicitly",
            self.ctx.resolve(*field),
            self.ctx.resolve(*field)
        ))
    }

    pub(crate) fn check_closure(
        &mut self,
        node_id: kernc_utils::NodeId,
        captures: &[ast::CapturePattern],
        params: &[ast::FuncParam],
        ast_ret_ty: &ast::TypeNode,
        body: &ast::Expr,
        span: Span,
    ) -> TypeId {
        let mut state_fields = Vec::new();
        let mut capture_env = Vec::new();

        for cap in captures {
            let cap_ty = self.check_expr(&cap.value, None);
            state_fields.push(cap_ty);
            capture_env.push((cap.name, cap_ty, cap.name_span));
        }

        let current_scope = match self.ctx.scopes.current_scope_id() {
            Some(id) => id,
            None => {
                self.ctx.emit_ice(
                    span,
                    "Compiler Bug: Closure evaluated outside of any active scope",
                );
                crate::scope::ScopeId(0)
            }
        };

        let (param_tys, expected_ret) = {
            let mut param_tys = Vec::new();
            let mut type_resolver = TypeResolver::new(self.ctx);
            for param in params {
                let p_ty = type_resolver.resolve_type(&param.type_node, current_scope);
                param_tys.push(p_ty);
            }
            let expected_ret = type_resolver.resolve_type(ast_ret_ty, current_scope);
            (param_tys, expected_ret)
        };

        let closure_state_ty = self.ctx.type_registry.intern(TypeKind::AnonymousState {
            closure_node_id: node_id,
            captures: state_fields,
            params: param_tys.clone(),
            ret: expected_ret,
        });

        let _ = self.ctx.scopes.enter_scope();

        for (name, ty, cap_span) in capture_env {
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id,
                type_id: ty,
                def_id: None,
                span: cap_span,
                vis: Visibility::Private,
                is_mut: false,
            };
            if self.ctx.scopes.define(name, info.clone()).is_ok() {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::Variable,
                    info.is_mut,
                    info.vis.is_public(),
                );
            }
        }

        for (i, param) in params.iter().enumerate() {
            if self.ctx.resolve(param.pattern.name) == "_" {
                continue;
            }
            let param_node_id = self.ctx.next_node_id();
            let info = SymbolInfo {
                kind: SymbolKind::Var,
                node_id: param_node_id,
                type_id: param_tys[i],
                def_id: None,
                span: param.pattern.name_span,
                vis: Visibility::Private,
                is_mut: param.pattern.is_mut,
            };
            if self
                .ctx
                .scopes
                .define(param.pattern.name, info.clone())
                .is_ok()
            {
                self.ctx.record_symbol_definition(
                    info.span,
                    SemanticSymbolKind::Parameter,
                    info.is_mut,
                    info.vis.is_public(),
                );
            }
        }

        let (actual_ret_ty, has_returned) = {
            let mut sub_checker = ExprChecker::new(self.ctx, Some(expected_ret));
            let ty = {
                let ty = sub_checker.check_expr(body, Some(expected_ret));
                sub_checker.finalize_numeric_inference(ty)
            };
            (ty, sub_checker.has_returned)
        };

        if actual_ret_ty != TypeId::ERROR
            && expected_ret != TypeId::ERROR
            && actual_ret_ty != TypeId::NEVER
        {
            let norm_actual = self.ctx.type_registry.normalize(actual_ret_ty);
            let norm_expected = self.ctx.type_registry.normalize(expected_ret);
            let is_missing_tail = norm_actual == TypeId::VOID && norm_expected != TypeId::VOID;

            if !(is_missing_tail && has_returned) && norm_actual != norm_expected {
                let expected_str = self.ctx.ty_to_string(expected_ret);
                let actual_str = self.ctx.ty_to_string(actual_ret_ty);

                self.ctx
                    .struct_error(
                        body.span,
                        format!(
                            "closure body evaluates to `{}`, but signature expects `{}`",
                            actual_str, expected_str
                        ),
                    )
                    .with_hint(
                        "ensure the final expression or return statements match the explicit return type",
                    )
                    .emit();
            }
        }

        self.ctx.scopes.exit_scope();
        self.ctx.set_node_type(node_id, closure_state_ty);

        closure_state_ty
    }
}
