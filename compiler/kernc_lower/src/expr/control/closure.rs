use super::*;

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn fn_like_signature(
        &mut self,
        fn_like_ty: TypeId,
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        let norm = self.ctx.type_registry.normalize(fn_like_ty);
        match self.ctx.type_registry.get(norm).clone() {
            TypeKind::Function {
                params,
                ret,
                is_variadic: false,
            } => Some((params, ret)),
            TypeKind::Function {
                is_variadic: true, ..
            } => None,
            TypeKind::FnDef(def_id, fn_args) => {
                let def = self.ctx.defs[def_id.0 as usize].clone();
                let Def::Function(fn_def) = def else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): FnDef `{}` does not point to a function while building a closure adapter.",
                            def_id.0
                        ),
                    );
                    return None;
                };

                let Some(sig_ty) = fn_def.resolved_sig else {
                    self.ctx.emit_ice(
                        span,
                        "Kern ICE (Lowering): function definition lacks resolved signature while building a closure adapter",
                    );
                    return None;
                };

                let TypeKind::Function {
                    params,
                    ret,
                    is_variadic,
                } = self.ctx.type_registry.get(sig_ty).clone()
                else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (Lowering): resolved signature for FnDef `{}` is not a function type while building a closure adapter.",
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

                let mut subst_map = HashMap::new();
                for (param, arg) in fn_def.generics.iter().zip(fn_args.iter().copied()) {
                    subst_map.insert(param.name, arg);
                }

                let inst_params = params
                    .into_iter()
                    .map(|param| self.substitute_type_with_map(param, &subst_map))
                    .collect();
                let inst_ret = self.substitute_type_with_map(ret, &subst_map);
                Some((inst_params, inst_ret))
            }
            _ => None,
        }
    }

    pub(crate) fn get_or_create_fn_closure_adapter(
        &mut self,
        fn_like_ty: TypeId,
        span: Span,
    ) -> Option<MonoId> {
        let key = self.ctx.type_registry.normalize(fn_like_ty);
        if let Some(&adapter_id) = self.fn_closure_adapter_cache.get(&key) {
            return Some(adapter_id);
        }

        let (params, ret_ty) = self.fn_like_signature(key, span)?;
        let adapter_id = self.new_mono_id();
        let env_sym = self.fresh_synth_symbol("fn_env");
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });
        let fn_sig_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: params.clone(),
            ret: ret_ty,
            is_variadic: false,
        });

        let mut mast_params = Vec::with_capacity(params.len() + 1);
        mast_params.push(MastParam {
            name: env_sym,
            ty: void_ptr_ty,
            is_mut: false,
        });

        let mut call_args = Vec::with_capacity(params.len());
        for param_ty in params.iter().copied() {
            let arg_sym = self.fresh_synth_symbol("fn_arg");
            mast_params.push(MastParam {
                name: arg_sym,
                ty: param_ty,
                is_mut: false,
            });
            call_args.push(MastExpr::new(param_ty, MastExprKind::Var(arg_sym), span));
        }

        let fn_ptr_expr = MastExpr::new(
            fn_sig_ty,
            MastExprKind::Cast {
                kind: MastCastKind::Bitcast,
                operand: Box::new(MastExpr::new(void_ptr_ty, MastExprKind::Var(env_sym), span)),
            },
            span,
        );

        let call_expr = MastExpr::new(
            ret_ty,
            MastExprKind::Call {
                callee: Box::new(fn_ptr_expr),
                args: call_args,
            },
            span,
        );

        self.module.functions.push(MastFunction {
            id: adapter_id,
            name: format!("__fn_closure_adapter_{}", adapter_id.0),
            linkage: MastLinkage::Internal,
            params: mast_params,
            ret_ty,
            body: Some(MastBlock {
                stmts: vec![],
                result: Some(Box::new(call_expr)),
                defers: vec![],
            }),
            is_extern: false,
            is_variadic: false,
            inline_hint: MastInlineHint::None,
            attributes: vec![],
        });

        self.fn_closure_adapter_cache.insert(key, adapter_id);
        Some(adapter_id)
    }

    pub(super) fn anonymous_state_signature(
        &mut self,
        concrete_ty: TypeId,
        span: Span,
    ) -> Option<(Vec<TypeId>, TypeId)> {
        match self.ctx.type_registry.get(concrete_ty).clone() {
            TypeKind::AnonymousState { params, ret, .. } => Some((params, ret)),
            other => {
                let concrete_ty_str = self.ctx.ty_to_string(concrete_ty);
                self.ctx.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Lowering): closure expected `AnonymousState`, found `{}` ({other:?}).",
                        concrete_ty_str
                    ),
                );
                None
            }
        }
    }

    pub(super) fn lower_closure_captures(
        &mut self,
        captures: &[ast::CapturePattern],
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> (Vec<MastField>, Vec<MastExpr>) {
        let mut env_struct_fields = Vec::new();
        let mut cap_exprs = Vec::new();

        for cap in captures {
            let cap_mast = self.lower_expr(&cap.value, subst_map, None);
            env_struct_fields.push(MastField {
                name: cap.name,
                ty: cap_mast.ty,
            });
            cap_exprs.push(cap_mast);
        }

        (env_struct_fields, cap_exprs)
    }

    pub(super) fn register_closure_state_struct(
        &mut self,
        struct_id: MonoId,
        fields: Vec<MastField>,
    ) {
        self.module.structs.push(MastStruct {
            id: struct_id,
            name: format!("__closure_state_{}", struct_id.0),
            fields,
            is_extern: false,
            is_union: false,
            largest_field_idx: 0,
            union_size: 0,
            union_align: 1,
            attributes: vec![],
        });
    }

    pub(crate) fn lower_closure_expr(&mut self, spec: ClosureLowerSpec<'_>) -> MastExprKind {
        let struct_id = self.new_mono_id();
        let func_id = self.new_mono_id();
        self.closure_fn_map.insert(spec.node_id, func_id);

        // 0. ================= Detect decay context =================
        let is_decay = spec.captures.is_empty()
            && matches!(
                self.ctx
                    .type_registry
                    .get(self.ctx.type_registry.normalize(spec.exp_ty))
                    .clone(),
                TypeKind::Function { .. } | TypeKind::FnDef(..)
            );

        // 1. ================= Build the capture-state struct =================
        let (env_struct_fields, cap_exprs) =
            self.lower_closure_captures(spec.captures, spec.subst_map);
        self.register_closure_state_struct(struct_id, env_struct_fields.clone());

        // 2. ================= Build the closure entry function =================
        let env_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: true,
            elem: spec.concrete_ty,
        });

        let mut mast_params = Vec::new();
        let env_sym = self.ctx.intern("__env");

        // Decayed closures become plain C ABI functions with no hidden context pointer.
        if !is_decay {
            mast_params.push(MastParam {
                name: env_sym,
                ty: env_ptr_ty,
                is_mut: false,
            });
        }

        let Some((param_tys, ret_ty)) =
            self.anonymous_state_signature(spec.concrete_ty, spec.body.span)
        else {
            return MastExprKind::Trap;
        };

        for (i, p) in spec.params.iter().enumerate() {
            mast_params.push(MastParam {
                name: p.pattern.name,
                ty: param_tys[i],
                is_mut: p.pattern.is_mut,
            });
        }

        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_local_forwardings = std::mem::take(&mut self.local_forwardings);
        let saved_local_value_forwardings = std::mem::take(&mut self.local_value_forwardings);
        let saved_defer_stack = std::mem::take(&mut self.defer_stack);
        let saved_loop_frames = std::mem::take(&mut self.loop_frames);
        let saved_local_statics = std::mem::take(&mut self.local_statics);

        self.local_types.push(HashMap::new());
        self.local_forwardings.push(HashMap::new());
        self.local_value_forwardings.push(HashMap::new());
        self.current_return_types.push(ret_ty);
        for p in &mast_params {
            self.bind_local_type(spec.body.span, p.name, p.ty, p.is_mut, "closure parameter");
        }

        // Restore captured values when the closure was not decayed away.
        let mut injected_stmts = Vec::new();
        if !is_decay {
            let env_var_expr =
                MastExpr::new(env_ptr_ty, MastExprKind::Var(env_sym), Span::default());

            for (i, cap) in spec.captures.iter().enumerate() {
                let deref_env = MastExpr::new(
                    spec.concrete_ty,
                    MastExprKind::Deref(Box::new(env_var_expr.clone())),
                    Span::default(),
                );
                let field_access = MastExpr::new(
                    env_struct_fields[i].ty,
                    MastExprKind::FieldAccess {
                        lhs: Box::new(deref_env),
                        struct_id,
                        field_idx: i,
                    },
                    Span::default(),
                );

                injected_stmts.push(MastStmt::Let {
                    name: cap.name,
                    ty: env_struct_fields[i].ty,
                    is_mut: false,
                    init: field_access,
                });
                self.bind_local_type(
                    spec.body.span,
                    cap.name,
                    env_struct_fields[i].ty,
                    false,
                    "closure capture restore",
                );
            }
        }

        let mut body_block = self.lower_block_as_body(spec.body, spec.subst_map, ret_ty);
        injected_stmts.append(&mut body_block.stmts);
        body_block.stmts = injected_stmts;

        self.local_types.pop();
        self.local_forwardings.pop();
        self.local_value_forwardings.pop();
        self.current_return_types.pop();

        self.local_types = saved_local_types;
        self.local_forwardings = saved_local_forwardings;
        self.local_value_forwardings = saved_local_value_forwardings;
        self.defer_stack = saved_defer_stack;
        self.loop_frames = saved_loop_frames;
        self.local_statics = saved_local_statics;

        self.module.functions.push(MastFunction {
            id: func_id,
            name: format!("__closure_fn_{}", func_id.0),
            linkage: MastLinkage::Internal,
            params: mast_params,
            ret_ty,
            body: Some(body_block),
            is_extern: false,
            is_variadic: false,
            inline_hint: MastInlineHint::None,
            attributes: vec![],
        });

        // 3. ================= Assemble the expression at the current site =================
        let struct_init = MastExpr::new(
            spec.concrete_ty,
            MastExprKind::StructInit {
                struct_id,
                fields: cap_exprs,
            },
            Span::default(),
        );

        // 4. ================= Apply BNC and decay rules =================
        if is_decay {
            // Return the generated static C function pointer directly.
            return MastExprKind::FuncRef(func_id);
        }

        struct_init.kind
    }
}
