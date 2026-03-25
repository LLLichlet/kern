use super::Lowerer;
use std::collections::HashMap;

use kernc_ast::{self as ast, Expr, ExprKind};
use kernc_mast::*;
use kernc_sema::LayoutEngine;
use kernc_sema::checker::Substituter;
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{Span, SymbolId};

impl<'a, 'ctx> Lowerer<'a, 'ctx> {
    pub(crate) fn lower_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        // 拦截 @asm 宏调用
        // 必须在查询节点类型之前，因为 @asm 不是一个真实的函数
        if let ExprKind::Identifier(sym) = &callee.kind {
            if self.ctx.resolve(*sym) == "@asm" {
                return self.lower_asm_call(args, subst_map, span);
            }
        }
        let mut receiver_mast = None;
        let mut is_method = false;
        let mut method_field_sym = None;

        // 1. 嗅探是否为方法调用
        if let ExprKind::FieldAccess { lhs, field } = &callee.kind {
            let lhs_ty = self
                .ctx
                .node_types
                .get(&lhs.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);
            let is_module = matches!(self.ctx.type_registry.get(norm_lhs), TypeKind::Module(_));

            if !is_module {
                let callee_ty = self
                    .ctx
                    .node_types
                    .get(&callee.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let norm_callee = self.ctx.type_registry.normalize(callee_ty);

                if matches!(
                    self.ctx.type_registry.get(norm_callee),
                    TypeKind::FnDef(..) | TypeKind::Function { .. }
                ) {
                    is_method = true;
                    method_field_sym = Some(*field);
                    receiver_mast = Some(self.lower_expr(lhs, subst_map, None));
                }
            }
        }

        // 2. 提取预期的参数签名 (处理泛型替换)
        let raw_callee_ty = self
            .ctx
            .node_types
            .get(&callee.id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
        let substituted_callee = subst.substitute(raw_callee_ty);
        let norm_callee = self.ctx.type_registry.normalize(substituted_callee);
        let expected_param_tys = self.get_callee_expected_params(norm_callee);

        // 3. 准备实参 (处理方法调用的参数偏移)
        let mut arg_masts = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let param_idx = if is_method { i + 1 } else { i };
            let exp_ty = expected_param_tys.get(param_idx).copied();
            arg_masts.push(self.lower_expr(a, subst_map, exp_ty));
        }

        // 4. 执行调用的具体分发
        if is_method {
            let field = method_field_sym.unwrap();
            let recv = receiver_mast.unwrap();
            self.lower_method_call(recv, field, arg_masts, norm_callee, span)
        } else {
            self.lower_normal_call(callee, arg_masts, subst_map)
        }
    }

    pub(crate) fn lower_asm_call(
        &mut self,
        args: &[Expr],
        subst_map: &HashMap<SymbolId, TypeId>,
        span: Span,
    ) -> MastExprKind {
        let config_arg = &args[0];
        let fields = if let ExprKind::DataInit {
            literal: ast::DataLiteralKind::Struct(f),
            ..
        } = &config_arg.kind
        {
            f
        } else {
            self.ctx.emit_ice(span, "Kern ICE (Lowering): `@asm` macro argument must be a structural data literal (e.g. `.{ ... }`). Sema failed to validate this.");
            unreachable!()
        };

        let mut asm_template = String::new();
        let mut is_volatile = false;

        let mut outputs = Vec::new();
        let mut inputs = Vec::new();
        let mut clobbers = Vec::new();

        for field in fields {
            let field_name = self.ctx.resolve(field.name);
            match field_name {
                "asm" => {
                    match &field.value.kind {
                        // 支持单字符串 asm: "nop"
                        ExprKind::String(s) => asm_template = s.clone(),
                        // 支持数组形式 asm: .{ "out dx, al", "in al, dx" }
                        ExprKind::DataInit {
                            literal: ast::DataLiteralKind::Array(elems),
                            ..
                        } => {
                            let mut lines = Vec::new();
                            for e in elems {
                                if let ExprKind::String(s) = &e.kind {
                                    lines.push(s.as_str());
                                } else {
                                    self.ctx.emit_ice(e.span, "Kern ICE (Lowering): `@asm` template array must contain only strings.");
                                    unreachable!()
                                }
                            }
                            asm_template = lines.join("\n");
                        }
                        _ => {
                            self.ctx.emit_ice(field.value.span, "Kern ICE (Lowering): Invalid format for `asm` field in `@asm` macro.");
                            unreachable!()
                        }
                    }
                }
                "volatile" => {
                    if let ExprKind::Bool(b) = &field.value.kind {
                        is_volatile = *b;
                    }
                }
                "outputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM 约束：reg -> "=r", freg -> "=f", eax -> "={eax}"
                            let constraint = if reg_name == "reg" {
                                "=r".to_string()
                            } else if reg_name == "freg" {
                                "=f".to_string()
                            } else {
                                format!("={{{}}}", reg_name)
                            };

                            let ptr_expr = self.lower_expr(&reg.value, subst_map, None);
                            let val_ty = self.ctx.type_registry.get_elem_type(ptr_expr.ty).unwrap();
                            outputs.push((constraint, ptr_expr, val_ty));
                        }
                    }
                }
                "inputs" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Struct(regs),
                        ..
                    } = &field.value.kind
                    {
                        for reg in regs {
                            let reg_name = self.ctx.resolve(reg.name);
                            // LLVM 约束：reg -> "r", freg -> "f", eax -> "{eax}"
                            let constraint = if reg_name == "reg" {
                                "r".to_string()
                            } else if reg_name == "freg" {
                                "f".to_string()
                            } else {
                                format!("{{{}}}", reg_name)
                            };

                            let val_expr = self.lower_expr(&reg.value, subst_map, None);
                            inputs.push((constraint, val_expr));
                        }
                    }
                }
                "clobbers" => {
                    if let ExprKind::DataInit {
                        literal: ast::DataLiteralKind::Array(elems),
                        ..
                    } = &field.value.kind
                    {
                        for e in elems {
                            if let ExprKind::String(s) = &e.kind {
                                clobbers.push(format!("~{{{}}}", s));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // 组装最终的 LLVM 约束字符串 (顺序必须是: outputs, inputs, clobbers)
        let mut all_constraints = Vec::new();
        let mut output_ptrs = Vec::new();
        let mut output_tys = Vec::new();
        for (c, ptr, ty) in outputs {
            all_constraints.push(c);
            output_ptrs.push(ptr);
            output_tys.push(ty);
        }

        let mut input_args = Vec::new();
        for (c, expr) in inputs {
            all_constraints.push(c);
            input_args.push(expr);
        }

        for c in clobbers {
            all_constraints.push(c);
        }

        MastExprKind::Asm(MastAsmBlock {
            asm_template,
            constraints: all_constraints.join(","),
            input_args,
            output_ptrs,
            output_tys,
            is_volatile,
        })
    }

    pub(crate) fn lower_method_call(
        &mut self,
        recv: MastExpr,
        field: SymbolId,
        mut arg_masts: Vec<MastExpr>,
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
        // 方法在哪种类型上实现，就严格按该类型去查。
        let norm_base = self.ctx.type_registry.normalize(recv.ty);

        // Trait Object 在 Kern 中永远作为胖指针存在 (比如 *mut Allocator)。
        // 因此 recv.ty 实际上是 TypeKind::Pointer。我们需要探查其内部元素。
        let mut inner_ty = norm_base;
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_base).clone()
        {
            inner_ty = elem;
        }

        // 2. 根据探查到的类型，决定是动态分发(VTable)还是静态分发
        if let TypeKind::TraitObject(trait_id, _) = self.ctx.type_registry.get(inner_ty) {
            // 将完整的胖指针 recv 交给动态分发器提取 VTable
            self.lower_dynamic_method_dispatch(recv, field, arg_masts, *trait_id, norm_callee, span)
        } else if let TypeKind::FnDef(method_id, generics) =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            self.lower_static_method_dispatch(
                recv,
                arg_masts,
                method_id,
                &generics,
                norm_callee,
                span,
            )
        } else {
            // SEMA 传来的只是抽象的 TypeKind::Function，说明它来源于泛型约束。
            // 此时 T 已单态化，我们需要在全局寻找具体实现。
            let mut target_func_id = None;
            let mut resolved_impl_args = Vec::new();

            for def in &self.ctx.defs {
                if let Def::Impl(impl_def) = def {
                    let impl_target_raw = self
                        .ctx
                        .node_types
                        .get(&impl_def.target_type.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    let norm_impl_target = self.ctx.type_registry.normalize(impl_target_raw);

                    // 无泛型 Impl
                    if impl_def.generics.is_empty() {
                        let mut matched = false;

                        // 精确匹配：*mut i32 == *mut i32，或者 *i32 == *i32
                        if norm_base == norm_impl_target {
                            matched = true;
                        }
                        // 安全降级匹配：允许 *mut i32 调用挂载在 impl *i32 上的方法
                        else if let TypeKind::Pointer { is_mut: true, elem } =
                            self.ctx.type_registry.get(norm_base).clone()
                        {
                            let const_ptr = self.ctx.type_registry.intern(TypeKind::Pointer {
                                is_mut: false,
                                elem,
                            });
                            if const_ptr == norm_impl_target {
                                matched = true;
                            }
                        }

                        if matched {
                            for &m_id in &impl_def.methods {
                                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize] {
                                    if f.name == field {
                                        target_func_id = Some(m_id);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // 带泛型 Impl 的匹配
                    else {
                        // 核心修复 1：穿透指针。如果是泛型结构体的指针调用，
                        // 需要剥离指针，暴露出底层的 Def，才能正确提取泛型实参。
                        let mut check_base = norm_base;
                        let mut check_impl = norm_impl_target;
                        let mut matched_ptr = false;

                        // 同步处理指针降级与指针剥离
                        if let TypeKind::Pointer {
                            is_mut: base_mut,
                            elem: base_elem,
                        } = self.ctx.type_registry.get(check_base).clone()
                        {
                            if let TypeKind::Pointer {
                                is_mut: impl_mut,
                                elem: impl_elem,
                            } = self.ctx.type_registry.get(check_impl).clone()
                            {
                                // 允许精确匹配，或者 *mut T 安全降级为 *T
                                if base_mut == impl_mut || (base_mut && !impl_mut) {
                                    check_base = base_elem;
                                    check_impl = impl_elem;
                                    matched_ptr = true;
                                }
                            }
                        } else {
                            matched_ptr = true; // 都不是指针的情况，直接继续往下判断
                        }

                        if matched_ptr {
                            if let TypeKind::Def(base_def_id, base_args) =
                                self.ctx.type_registry.get(check_base).clone()
                            {
                                if let TypeKind::Def(impl_def_id, impl_raw_args) =
                                    self.ctx.type_registry.get(check_impl).clone()
                                {
                                    // 实体相同且参数数量一致，确认命中
                                    if base_def_id == impl_def_id
                                        && base_args.len() == impl_raw_args.len()
                                    {
                                        resolved_impl_args = base_args.clone();
                                        for &m_id in &impl_def.methods {
                                            if let Def::Function(f) =
                                                &self.ctx.defs[m_id.0 as usize]
                                            {
                                                if f.name == field {
                                                    target_func_id = Some(m_id);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if target_func_id.is_some() {
                        break;
                    }
                }
            }

            if let Some(func_id) = target_func_id {
                let expected_params = self.get_callee_expected_params(norm_callee);
                let mut final_recv = recv;

                // 核心修复 2：为 LLVM 后端抹平类型差异。
                // 如果发生了安全降级 (*mut -> *)，在此刻主动插入一个 Bitcast 节点。
                if let Some(&exp_self) = expected_params.first() {
                    if final_recv.ty != exp_self {
                        final_recv = MastExpr::new(
                            exp_self,
                            MastExprKind::Cast {
                                kind: MastCastKind::Bitcast,
                                operand: Box::new(final_recv),
                            },
                            span,
                        );
                    }
                }

                arg_masts.insert(0, final_recv);
                let mono_id = self.instantiate_function(func_id, &resolved_impl_args);
                let func_ref = MastExpr::new(norm_callee, MastExprKind::FuncRef(mono_id), span);
                MastExprKind::Call {
                    callee: Box::new(func_ref),
                    args: arg_masts,
                }
            } else {
                let type_name = self.ctx.ty_to_string(norm_base);
                let field_name = self.ctx.resolve(field);
                self.ctx.emit_ice(span, format!("Kern ICE (Lowering): Failed to devirtualize static trait method `{}` for exact type `{}`. Sema passed it, but no matching impl found.", field_name, type_name));
                unreachable!()
            }
        }
    }

    /// 辅助：构建静态方法调用 (泛型实例化)
    pub(crate) fn lower_static_method_dispatch(
        &mut self,
        mut recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        method_id: DefId,
        generics: &[TypeId],
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
        let expected_params = self.get_callee_expected_params(norm_callee);
        if let Some(&exp_self) = expected_params.first() {
            if recv.ty != exp_self {
                recv = MastExpr::new(
                    exp_self,
                    MastExprKind::Cast {
                        kind: MastCastKind::Bitcast,
                        operand: Box::new(recv),
                    },
                    span,
                );
            }
        }

        arg_masts.insert(0, recv);
        let func_id = self.instantiate_function(method_id, generics);
        let func_ref = MastExpr::new(norm_callee, MastExprKind::FuncRef(func_id), span);
        MastExprKind::Call {
            callee: Box::new(func_ref),
            args: arg_masts,
        }
    }

    /// 辅助：构建动态方法调用 (从 VTable 提取函数指针)
    pub(crate) fn lower_dynamic_method_dispatch(
        &mut self,
        recv: MastExpr,
        field: SymbolId,
        mut arg_masts: Vec<MastExpr>,
        trait_id: DefId,
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
        let trait_def = match &self.ctx.defs[trait_id.0 as usize] {
            Def::Trait(t) => t.clone(),
            _ => {
                self.ctx.emit_ice(span, format!("Kern ICE (Lowering): Expected Trait definition for TraitObject method call, found something else. DefId: {}", trait_id.0));
                unreachable!()
            }
        };

        let vtable_idx = match trait_def.methods.iter().position(|m| m.name == field) {
            Some(idx) => idx,
            None => {
                self.ctx.emit_ice(span, format!("Kern ICE (Lowering): Method `{}` not found in trait definition. Sema missed method bound check.", self.ctx.resolve(field)));
                unreachable!()
            }
        };

        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // 数据指针 (传递给方法的 self)
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(recv.clone())),
            span,
        );
        arg_masts.insert(0, data_ptr);

        // 虚表指针提取与转换
        let vtable_meta = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(recv)),
            span,
        );
        let vtable_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: void_ptr_ty,
        });

        let vtable_ptr = MastExpr::new(
            vtable_ptr_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(vtable_meta),
            },
            span,
        );

        // 获取函数指针
        let func_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::IndexAccess {
                lhs: Box::new(vtable_ptr),
                index: Box::new(MastExpr::new(
                    TypeId::USIZE,
                    MastExprKind::Integer(vtable_idx as u128),
                    span,
                )),
            },
            span,
        );

        // 拼接签名
        let (ret_ty, is_variadic, mut patched_params) = if let TypeKind::Function {
            ret,
            is_variadic,
            params,
            ..
        } =
            self.ctx.type_registry.get(norm_callee)
        {
            (*ret, *is_variadic, params.clone())
        } else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Lowering): Callee type of dynamic method dispatch is not a Function.",
            );
            unreachable!()
        };

        if !patched_params.is_empty() {
            patched_params[0] = void_ptr_ty;
        }

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret: ret_ty,
            is_variadic,
        });

        let func_ptr_typed = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::Bitcast,
                operand: Box::new(func_ptr),
            },
            span,
        );

        MastExprKind::Call {
            callee: Box::new(func_ptr_typed),
            args: arg_masts,
        }
    }

    pub(crate) fn lower_normal_call(
        &mut self,
        callee: &Expr,
        mut arg_masts: Vec<MastExpr>,
        subst_map: &HashMap<SymbolId, TypeId>,
    ) -> MastExprKind {
        let callee_mast = self.lower_expr(callee, subst_map, None);
        let norm_callee = self.ctx.type_registry.normalize(callee_mast.ty);

        // 拦截并处理闭包胖指针的动态调用
        if let TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } =
            self.ctx.type_registry.get(norm_callee).clone()
        {
            let inner_norm = self.ctx.type_registry.normalize(elem);
            if matches!(
                self.ctx.type_registry.get(inner_norm),
                TypeKind::ClosureInterface { .. }
            ) {
                return self.lower_closure_call(callee_mast, arg_masts, inner_norm, callee.span);
            }
        }

        if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty).clone()
        {
            // 拦截内置函数 (Intrinsic)
            if let Def::Function(f) = &self.ctx.defs[fn_id.0 as usize] {
                if f.is_intrinsic {
                    let name_str = self.ctx.resolve(f.name);

                    // 编译期常量折叠: @sizeof[T]() -> usize
                    if name_str == "@sizeOf" {
                        // 从函数调用的泛型参数中提取 T
                        let target_ty = if let TypeKind::FnDef(_, args) =
                            self.ctx.type_registry.get(callee_mast.ty)
                        {
                            args[0] // T 是第一个泛型参数
                        } else {
                            TypeId::ERROR
                        };
                        let mut le = LayoutEngine::new(self.ctx);
                        let size = le.compute_type_size(target_ty);
                        return MastExprKind::Integer(size as u128);
                    }
                    // 对齐计算 @alignOf[T]() -> usize
                    else if name_str == "@alignOf" {
                        let target_ty = if let TypeKind::FnDef(_, args) =
                            self.ctx.type_registry.get(callee_mast.ty)
                        {
                            args[0]
                        } else {
                            TypeId::ERROR
                        };
                        let mut le = LayoutEngine::new(self.ctx);
                        let align = le.compute_type_align(target_ty);
                        return MastExprKind::Integer(align as u128);
                    }
                    // 不可达: @unreachable() -> !
                    else if name_str == "@unreachable" {
                        return MastExprKind::Unreachable;
                    } else if name_str == "@popCount" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::PopCount,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@clz" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Clz,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@ctz" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Ctz,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@bswap" {
                        return MastExprKind::BitIntrinsic {
                            kind: BitIntrinsicKind::Bswap,
                            operand: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@trap" {
                        return MastExprKind::Trap;
                    } else if name_str == "@fence" {
                        return MastExprKind::Fence;
                    } else if name_str == "@breakpoint" {
                        return MastExprKind::Breakpoint;
                    } else if name_str == "@memcpy" {
                        return MastExprKind::Memcpy {
                            dest: Box::new(arg_masts.remove(0)),
                            src: Box::new(arg_masts.remove(0)),
                            len: Box::new(arg_masts.remove(0)),
                        };
                    } else if name_str == "@memset" {
                        return MastExprKind::Memset {
                            dest: Box::new(arg_masts.remove(0)),
                            val: Box::new(arg_masts.remove(0)),
                            len: Box::new(arg_masts.remove(0)),
                        };
                    }
                }
            }

            // 如果不是内置函数，走正常的实例化和函数调用逻辑
            let mono_id = self.instantiate_function(fn_id, &fn_args);
            let func_ref =
                MastExpr::new(callee_mast.ty, MastExprKind::FuncRef(mono_id), callee.span);
            MastExprKind::Call {
                callee: Box::new(func_ref),
                args: arg_masts,
            }
        } else {
            MastExprKind::Call {
                callee: Box::new(callee_mast),
                args: arg_masts,
            }
        }
    }

    pub(crate) fn lower_generic_instantiation(&mut self, concrete_ty: TypeId) -> MastExprKind {
        let fn_info =
            if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                Some((*fn_id, fn_args.clone()))
            } else {
                None
            };
        if let Some((fn_id, fn_args)) = fn_info {
            let mono_id = self.instantiate_function(fn_id, &fn_args);
            MastExprKind::FuncRef(mono_id)
        } else {
            MastExprKind::Integer(0)
        }
    }

    pub(crate) fn get_callee_expected_params(&mut self, norm_callee: TypeId) -> Vec<TypeId> {
        match self.ctx.type_registry.get(norm_callee).clone() {
            TypeKind::Function { params, .. } => params,
            TypeKind::FnDef(def_id, gen_args) => {
                if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                    if let Some(sig) = f.resolved_sig {
                        let norm_sig = self.ctx.type_registry.normalize(sig);
                        let raw_params = if let TypeKind::Function { params, .. } =
                            self.ctx.type_registry.get(norm_sig).clone()
                        {
                            params
                        } else {
                            Vec::new()
                        };

                        let all_generic_params = f.generics.clone();

                        let mut sig_subst_map = HashMap::new();
                        for (idx, param) in all_generic_params.iter().enumerate() {
                            if idx < gen_args.len() {
                                sig_subst_map.insert(param.name, gen_args[idx]);
                            }
                        }

                        let mut sig_subst =
                            Substituter::new(&mut self.ctx.type_registry, &sig_subst_map);
                        raw_params
                            .into_iter()
                            .map(|p| sig_subst.substitute(p))
                            .collect()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    pub(crate) fn lower_closure_call(
        &mut self,
        callee_mast: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        closure_interface_ty: TypeId,
        span: Span,
    ) -> MastExprKind {
        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::VOID,
        });

        // 1. 提取 data_ptr，强行插入为 arg[0]
        let data_ptr = MastExpr::new(
            void_ptr_ty,
            MastExprKind::ExtractFatPtrData(Box::new(callee_mast.clone())),
            span,
        );
        arg_masts.insert(0, data_ptr);

        // 2. 提取 meta_ptr (code_ptr)
        let code_ptr = MastExpr::new(
            TypeId::USIZE,
            MastExprKind::ExtractFatPtrMeta(Box::new(callee_mast.clone())),
            span,
        );

        // 3. 构建确切的底层函数签名，并将 USIZE 代码指针 IntToPtr 转换过去
        let (params, ret) = if let TypeKind::ClosureInterface { params, ret } =
            self.ctx.type_registry.get(closure_interface_ty).clone()
        {
            (params, ret)
        } else {
            let actual_ty_str = self.ctx.ty_to_string(closure_interface_ty);
            self.ctx.emit_ice(
                span,
                format!("Kern ICE (Lowering): Expected `ClosureInterface`, found `{}`.", actual_ty_str),
            );
            unreachable!()
        };

        let mut patched_params = params.clone();
        patched_params.insert(0, void_ptr_ty); // 补上对应的 env 参数

        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
            params: patched_params,
            ret,
            is_variadic: false,
        });

        let typed_code_ptr = MastExpr::new(
            patched_fn_ty,
            MastExprKind::Cast {
                kind: MastCastKind::IntToPtr,
                operand: Box::new(code_ptr),
            },
            span,
        );

        // 4. 间接调用函数指针
        MastExprKind::Call {
            callee: Box::new(typed_code_ptr),
            args: arg_masts,
        }
    }
}
