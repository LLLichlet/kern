use super::ExprChecker;
use crate::def::Def;
use crate::passes::TypeResolver;
use crate::scope::{SymbolInfo, SymbolKind};
use crate::semantic::SemanticSymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast::{self as ast, Expr, ExprKind, Visibility};
use kernc_utils::Span;

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
            self.ctx.node_types.insert(callee.id, TypeId::VOID);
            return self.check_asm_call(args, span);
        }

        let callee_ty = self.with_uninstantiated_generic_function_items_allowed(|this| {
            this.check_expr(callee, None)
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
            self.ctx.node_types.insert(callee.id, fixed_ty);
        }

        if sig_ty == TypeId::ERROR {
            return TypeId::ERROR;
        }

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
                    self.ctx
                        .struct_error(callee.span, "expression is not callable")
                        .with_hint(format!("type is `{}`", callee_str))
                        .emit();
                    return TypeId::ERROR;
                }
            }
            _ => {
                let callee_str = self.ctx.ty_to_string(callee_ty);
                self.ctx
                    .struct_error(callee.span, "expression is not callable")
                    .with_hint(format!("type is `{}`", callee_str))
                    .emit();
                return TypeId::ERROR;
            }
        };
        let params = unsafe { &*params_ptr };

        self.check_call_arity(args.len(), params.len(), is_method, is_variadic, span);

        if is_method && !params.is_empty() {
            let expected_self = params[0];
            self.check_method_receiver(expected_self, receiver_ty, callee);
            if receiver_ty != expected_self
                && let ExprKind::FieldAccess { lhs, .. } = &callee.kind
            {
                self.ctx.node_types.insert(lhs.id, expected_self);
            }
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
        final_ret
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
            let ty = sub_checker.check_expr(body, Some(expected_ret));
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
        self.ctx.node_types.insert(node_id, closure_state_ty);

        closure_state_ty
    }
}
