use super::*;
use crate::ty::GenericArg;

impl<'a, 'ctx> ConstEvaluator<'a, 'ctx> {
    fn compute_layout_size(&mut self, ty: TypeId) -> ConstEvalResult<u64> {
        let errors_before = self.ctx.sess.error_count;
        let size = {
            let mut layout = LayoutEngine::new(self.ctx);
            layout.compute_type_size(ty)
        };
        if self.ctx.sess.error_count != errors_before {
            Err(ConstEvalError)
        } else {
            Ok(size)
        }
    }

    fn compute_layout_align(&mut self, ty: TypeId) -> ConstEvalResult<u64> {
        let errors_before = self.ctx.sess.error_count;
        let align = {
            let mut layout = LayoutEngine::new(self.ctx);
            layout.compute_type_align(ty)
        };
        if self.ctx.sess.error_count != errors_before {
            Err(ConstEvalError)
        } else {
            Ok(align)
        }
    }

    fn check_bit_intrinsic_target_type(
        &mut self,
        ty: TypeId,
        span: Span,
        intrinsic_name: &str,
    ) -> Option<TypeId> {
        let norm = self.ctx.type_registry.normalize(ty);
        if norm == TypeId::ERROR {
            return None;
        }

        let is_supported = self.ctx.type_registry.is_integer(norm)
            || self
                .ctx
                .type_registry
                .simd_info(norm)
                .is_some_and(|(elem_ty, _)| self.ctx.type_registry.is_integer(elem_ty));

        if !is_supported {
            let ty_str = self.ctx.ty_to_string(norm);
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "`{}` only supports integer scalar or integer SIMD types",
                        intrinsic_name
                    ),
                )
                .with_hint(format!("found `{}`", ty_str))
                .with_hint("examples: `u32`, `i64`, `usize`, `u32x4`, `i16x8`")
                .emit();
            return None;
        }

        Some(norm)
    }

    pub(super) fn eval_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some((def_id, generic_args)) = self.resolve_callable(callee) else {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let func = match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Function(func)) => func,
            _ => return Err(ConstEvalError),
        };

        if func.is_intrinsic {
            return self.eval_intrinsic_call(callee, args, depth, span);
        }

        let mut arg_values = Vec::new();
        if let Some(receiver) = self.method_receiver(callee) {
            arg_values.push(self.eval_inner(receiver, depth + 1)?);
        }
        for arg in args {
            arg_values.push(self.eval_inner(arg, depth + 1)?);
        }

        self.invoke_function(def_id, &generic_args, arg_values, depth, span)
    }

    pub fn eval_function(
        &mut self,
        def_id: DefId,
        generic_args: &[GenericArg],
        arg_values: Vec<ConstValue>,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        self.invoke_function(def_id, generic_args, arg_values, 0, span)
    }

    pub(super) fn invoke_function(
        &mut self,
        def_id: DefId,
        generic_args: &[GenericArg],
        arg_values: Vec<ConstValue>,
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let func = match self.ctx.defs.get(def_id.0 as usize).cloned() {
            Some(Def::Function(func)) => func,
            _ => return Err(ConstEvalError),
        };

        if !func.generics.is_empty() && generic_args.len() != func.generics.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "const function `{}` requires fully resolved generic arguments during constant evaluation",
                        self.ctx.resolve(func.name)
                    ),
                )
                .emit();
            return Err(ConstEvalError);
        }

        if arg_values.len() != func.params.len() {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "const function `{}` expects {} arguments, but {} were provided",
                        self.ctx.resolve(func.name),
                        func.params.len(),
                        arg_values.len()
                    ),
                )
                .emit();
            return Err(ConstEvalError);
        }

        if func.is_extern {
            return self.eval_script_host_call(&func, &arg_values, span);
        }

        if !func.is_const && !self.allow_non_const_calls {
            self.ctx
                .struct_error(
                    span,
                    "only `const fn` can be called in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        }

        let prev_scope = self.ctx.scopes.current_scope_id();
        let owner_scope = self.def_owner_scope(def_id);
        if let Some(owner_scope) = owner_scope {
            self.ctx.scopes.set_current_scope(owner_scope);
            self.const_scopes.push(owner_scope);
        }

        let mut generic_map = HashMap::new();
        for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
            generic_map.insert(param.name, *arg);
        }
        if !generic_map.is_empty() {
            self.type_substs.push(generic_map);
        }

        self.function_depth += 1;
        let saved_loop_depth = self.loop_depth;
        let saved_loop_control = self.loop_control.take();
        self.loop_depth = 0;
        self.push_local_scope();
        let (param_tys, return_ty) = match self.callable_return_and_params(def_id, generic_args) {
            Some((params, ret)) => (params, ret),
            None => (vec![TypeId::ERROR; func.params.len()], TypeId::ERROR),
        };
        for ((param, value), param_ty) in func.params.iter().zip(arg_values.into_iter()).zip(
            param_tys
                .into_iter()
                .chain(std::iter::repeat(TypeId::ERROR)),
        ) {
            self.define_local(param.pattern.name, value);
            self.define_local_type(param.pattern.name, param_ty);
            self.define_local_mutability(param.pattern.name, param.pattern.is_mut);
        }

        let saved_return = self.return_value.take();
        self.function_return_types.push(return_ty);
        let body_result = if let Some(body) = &func.body {
            self.eval_inner(body, depth + 1)
        } else {
            self.ctx
                .struct_error(span, "`const fn` must have a body")
                .emit();
            Err(ConstEvalError)
        };
        let _ = self.function_return_types.pop();
        let fn_return = self.return_value.take();
        self.return_value = saved_return;

        self.pop_local_scope();
        self.loop_depth = saved_loop_depth;
        self.loop_control = saved_loop_control;
        self.function_depth -= 1;

        if !func.generics.is_empty() {
            let _ = self.type_substs.pop();
        }

        if owner_scope.is_some() {
            let _ = self.const_scopes.pop();
        }
        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }

        let body_result = body_result?;
        Ok(fn_return.unwrap_or(body_result))
    }

    pub(super) fn eval_script_host_call(
        &mut self,
        func: &crate::def::FunctionDef,
        arg_values: &[ConstValue],
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(host) = self.script_host else {
            self.ctx
                .struct_error(
                    span,
                    "`extern const fn` is not supported in constant evaluation",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let name = self.ctx.resolve(func.name).to_string();
        let result = unsafe { (host.call_extern)(host.data, &name, arg_values, span) };
        match result {
            Ok(value) => Ok(value),
            Err(message) => {
                self.ctx.struct_error(span, message).emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn apply_assignment_operator(
        &mut self,
        lhs: ConstValue,
        op: AssignmentOperator,
        rhs: ConstValue,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        use AssignmentOperator::*;

        match op {
            Assign => Ok(rhs),
            AddAssign => self.apply_binary_assignment(lhs, BinaryOperator::Add, rhs, span),
            SubtractAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::Subtract, rhs, span)
            }
            MultiplyAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::Multiply, rhs, span)
            }
            DivideAssign => self.apply_binary_assignment(lhs, BinaryOperator::Divide, rhs, span),
            ModuloAssign => self.apply_binary_assignment(lhs, BinaryOperator::Modulo, rhs, span),
            BitwiseAndAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::BitwiseAnd, rhs, span)
            }
            BitwiseOrAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::BitwiseOr, rhs, span)
            }
            BitwiseXorAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::BitwiseXor, rhs, span)
            }
            ShiftLeftAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::ShiftLeft, rhs, span)
            }
            ShiftRightAssign => {
                self.apply_binary_assignment(lhs, BinaryOperator::ShiftRight, rhs, span)
            }
        }
    }

    pub(super) fn apply_binary_assignment(
        &mut self,
        lhs: ConstValue,
        op: BinaryOperator,
        rhs: ConstValue,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        match (lhs, rhs) {
            (ConstValue::Int(l), ConstValue::Int(r)) => match op {
                BinaryOperator::Add => Ok(ConstValue::Int(l.wrapping_add(r))),
                BinaryOperator::Subtract => Ok(ConstValue::Int(l.wrapping_sub(r))),
                BinaryOperator::Multiply => Ok(ConstValue::Int(l.wrapping_mul(r))),
                BinaryOperator::Divide => {
                    if r == 0 {
                        self.ctx
                            .struct_error(span, "division by zero in constant expression")
                            .emit();
                        Err(ConstEvalError)
                    } else {
                        self.eval_const_int_division(l, r, span)
                    }
                }
                BinaryOperator::Modulo => {
                    if r == 0 {
                        self.ctx
                            .struct_error(span, "modulo by zero in constant expression")
                            .emit();
                        Err(ConstEvalError)
                    } else {
                        self.eval_const_int_modulo(l, r, span)
                    }
                }
                BinaryOperator::ShiftLeft => self.eval_const_int_shift(l, r, true, false, span),
                BinaryOperator::ShiftRight => self.eval_const_int_shift(l, r, false, false, span),
                BinaryOperator::BitwiseAnd => Ok(ConstValue::Int(l & r)),
                BinaryOperator::BitwiseOr => Ok(ConstValue::Int(l | r)),
                BinaryOperator::BitwiseXor => Ok(ConstValue::Int(l ^ r)),
                _ => {
                    self.ctx
                        .struct_error(
                            span,
                            "unsupported compound assignment for constant integers",
                        )
                        .emit();
                    Err(ConstEvalError)
                }
            },
            (ConstValue::Float(l), ConstValue::Float(r)) => match op {
                BinaryOperator::Add => Ok(ConstValue::Float(l + r)),
                BinaryOperator::Subtract => Ok(ConstValue::Float(l - r)),
                BinaryOperator::Multiply => Ok(ConstValue::Float(l * r)),
                BinaryOperator::Divide => Ok(ConstValue::Float(l / r)),
                _ => {
                    self.ctx
                        .struct_error(span, "unsupported compound assignment for constant floats")
                        .emit();
                    Err(ConstEvalError)
                }
            },
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        "type mismatch or unsupported types in constant compound assignment",
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    pub(super) fn method_receiver<'b>(&mut self, callee: &'b Expr) -> Option<&'b Expr> {
        let ExprKind::FieldAccess { lhs, .. } = &callee.kind else {
            return None;
        };

        let lhs_ty = self.node_type(lhs.id);
        if matches!(self.ctx.type_registry.get(lhs_ty), TypeKind::Module(..)) {
            None
        } else {
            Some(lhs.as_ref())
        }
    }

    pub(super) fn callable_return_and_params(
        &mut self,
        def_id: DefId,
        generic_args: &[GenericArg],
    ) -> Option<(Vec<TypeId>, TypeId)> {
        let Def::Function(func) = self.ctx.defs.get(def_id.0 as usize)?.clone() else {
            return None;
        };
        let sig = func.resolved_sig?;

        let sig = if func.generics.is_empty() {
            sig
        } else {
            if func.generics.len() != generic_args.len() {
                return None;
            }
            let mut generic_map = HashMap::new();
            for (param, arg) in func.generics.iter().zip(generic_args.iter()) {
                generic_map.insert(param.name, *arg);
            }
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &generic_map);
            subst.substitute(sig)
        };

        match self.ctx.type_registry.get(sig).clone() {
            TypeKind::Function { params, ret, .. } => Some((params, ret)),
            _ => None,
        }
    }

    pub(crate) fn eval_intrinsic_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some((def_id, generic_args)) = self.resolve_callable(callee) else {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .emit();
            return Err(ConstEvalError);
        };

        let (is_intrinsic, fn_name_id, generics_len) =
            if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                (f.is_intrinsic, f.name, f.generics.len())
            } else {
                return Err(ConstEvalError);
            };

        if !is_intrinsic {
            self.ctx
                .struct_error(
                    span,
                    "function calls are not allowed in constant expressions",
                )
                .with_hint(
                    "only compile-time intrinsics like `@sizeOf` or `@clz` are permitted here",
                )
                .emit();
            return Err(ConstEvalError);
        }

        let name_str = self.ctx.resolve(fn_name_id).to_string();

        // Constant-evaluated intrinsics require explicit generic arguments.
        if generic_args.len() != generics_len {
            self.ctx
                .struct_error(
                    span,
                    format!(
                        "intrinsic `{}` requires explicit generic arguments in constant evaluation",
                        name_str
                    ),
                )
                .with_hint(format!("example: `{}[u32](...)`", name_str))
                .emit();
            return Err(ConstEvalError);
        }

        // --- Core intrinsic dispatch ---
        let intrinsic_type_args = crate::ty::erase_non_type_generic_args(&generic_args);
        match name_str.as_str() {
            "@loc" => self.eval_loc(span),
            "@sizeOf" => self.eval_size_of(&intrinsic_type_args, span),
            "@alignOf" => self.eval_align_of(&intrinsic_type_args, span),
            "@popCount" | "@clz" | "@ctz" => {
                self.eval_bit_counting(name_str.as_str(), &intrinsic_type_args, args, depth, span)
            }
            "@intCast" => self.eval_int_cast(&intrinsic_type_args, args, depth, span),
            "@bswap" => self.eval_bswap(&intrinsic_type_args, args, depth, span),
            "@memcpy" | "@memmove" | "@memset" => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "memory intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
            _ => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "intrinsic `{}` cannot be evaluated at compile time",
                            name_str
                        ),
                    )
                    .emit();
                Err(ConstEvalError)
            }
        }
    }

    // ==========================================
    // Concrete intrinsic evaluators
    // ==========================================

    pub(super) fn eval_size_of(
        &mut self,
        generic_args: &[TypeId],
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(&target_ty) = generic_args.first() {
            let size = self.compute_layout_size(target_ty)?;
            Ok(ConstValue::Int(size as i128))
        } else {
            Err(ConstEvalError) // Already guarded by the generic-arity check above.
        }
    }

    pub(super) fn eval_loc(&mut self, span: Span) -> ConstEvalResult<ConstValue> {
        let file_name = self.ctx.intern("file");
        let line_name = self.ctx.intern("line");
        let col_name = self.ctx.intern("col");
        let file = self
            .ctx
            .sess
            .source_manager
            .get_file_path(span.file)
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let (line, col) = self
            .ctx
            .sess
            .source_manager
            .lookup_location(span)
            .map(|loc| (loc.line, loc.col))
            .unwrap_or((0, 0));

        let mut fields = HashMap::new();
        fields.insert(file_name, ConstValue::String(file));
        fields.insert(line_name, ConstValue::Int(line as i128));
        fields.insert(col_name, ConstValue::Int(col as i128));
        Ok(ConstValue::Struct(fields))
    }

    pub(super) fn eval_align_of(
        &mut self,
        generic_args: &[TypeId],
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Some(&target_ty) = generic_args.first() {
            let align = self.compute_layout_align(target_ty)?;
            Ok(ConstValue::Int(align as i128))
        } else {
            Err(ConstEvalError)
        }
    }

    pub(super) fn eval_bit_counting(
        &mut self,
        name: &str,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(target_ty) = self.check_bit_intrinsic_target_type(generic_args[0], span, name)
        else {
            return Err(ConstEvalError);
        };

        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let bit_width = self.compute_layout_size(target_ty)? * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match name {
                "@popCount" => u_val.count_ones() as i128,
                "@clz" => (u_val.leading_zeros() as i128) - (128 - bit_width as i128),
                "@ctz" => {
                    if u_val == 0 {
                        bit_width as i128
                    } else {
                        u_val.trailing_zeros() as i128
                    }
                }
                _ => {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Kern ICE (ConstEval): Unsupported bit intrinsic `{}` in constant evaluation.",
                            name
                        ),
                    );
                    return Err(ConstEvalError);
                }
            };
            return Ok(ConstValue::Int(res));
        }
        Err(ConstEvalError)
    }

    pub(super) fn eval_int_cast(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        _span: Span,
    ) -> ConstEvalResult<ConstValue> {
        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            let target_ty = generic_args[1];

            // Use the layout engine so pointer-sized integers are handled correctly.
            let bit_width = self.compute_layout_size(target_ty)? * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let mut u_val = (val as u128) & mask;

            let is_signed = matches!(
                self.ctx.type_registry.get(target_ty),
                TypeKind::Primitive(
                    PrimitiveType::I8
                        | PrimitiveType::I16
                        | PrimitiveType::I32
                        | PrimitiveType::I64
                        | PrimitiveType::I128
                        | PrimitiveType::ISize
                )
            );

            if is_signed && bit_width < 128 && (u_val & (1 << (bit_width - 1))) != 0 {
                u_val |= u128::MAX << bit_width;
            }
            return Ok(ConstValue::Int(u_val as i128));
        }
        Err(ConstEvalError)
    }

    pub(super) fn eval_bswap(
        &mut self,
        generic_args: &[TypeId],
        args: &[Expr],
        depth: usize,
        span: Span,
    ) -> ConstEvalResult<ConstValue> {
        let Some(target_ty) = self.check_bit_intrinsic_target_type(generic_args[0], span, "@bswap")
        else {
            return Err(ConstEvalError);
        };

        if let Ok(ConstValue::Int(val)) = self.eval_inner(&args[0], depth + 1) {
            // Use the layout engine so the operation respects target bit width.
            let bit_width = self.compute_layout_size(target_ty)? * 8;

            let mask = if bit_width == 128 {
                u128::MAX
            } else {
                (1 << bit_width) - 1
            };
            let u_val = (val as u128) & mask;

            let res = match bit_width {
                16 => (u_val as u16).swap_bytes() as i128,
                32 => (u_val as u32).swap_bytes() as i128,
                64 => (u_val as u64).swap_bytes() as i128,
                128 => u_val.swap_bytes() as i128,
                _ => u_val as i128,
            };
            return Ok(ConstValue::Int(res));
        }
        Err(ConstEvalError)
    }
}
