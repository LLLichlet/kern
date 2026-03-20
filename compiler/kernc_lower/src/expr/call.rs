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
        let norm_callee = self.ctx.type_registry.normalize(
            self.ctx
                .node_types
                .get(&callee.id)
                .copied()
                .unwrap_or(TypeId::ERROR),
        );
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
        _span: Span,
    ) -> MastExprKind {
        let config_arg = &args[0];
        let fields = if let ExprKind::DataInit {
            literal: ast::DataLiteralKind::Struct(f),
            ..
        } = &config_arg.kind
        {
            f
        } else {
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
                                }
                            }
                            asm_template = lines.join("\n");
                        }
                        _ => unreachable!(),
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
        arg_masts: Vec<MastExpr>,
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
        // 1. 扒掉指针外衣，获取真正能定位到方法的基底类型
        let mut base_ty = recv.ty;
        loop {
            let norm = self.ctx.type_registry.normalize(base_ty);
            match self.ctx.type_registry.get(norm) {
                TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => {
                    base_ty = *elem;
                }
                _ => break,
            }
        }
        let norm_base = self.ctx.type_registry.normalize(base_ty);

        // 2. 根据基底类型，决定是动态分发还是静态分发
        if let TypeKind::TraitObject(trait_id, _) = self.ctx.type_registry.get(norm_base) {
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
            unreachable!("Invalid method call resolution")
        }
    }

    /// 辅助：构建静态方法调用 (泛型实例化)
    pub(crate) fn lower_static_method_dispatch(
        &mut self,
        recv: MastExpr,
        mut arg_masts: Vec<MastExpr>,
        method_id: DefId,
        generics: &[TypeId],
        norm_callee: TypeId,
        span: Span,
    ) -> MastExprKind {
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
            _ => unreachable!(),
        };

        let vtable_idx = trait_def
            .methods
            .iter()
            .position(|m| m.name == field)
            .expect("Method not found in trait");

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

                        let mut all_generic_params = Vec::new();
                        if let Some(parent_id) = f.parent {
                            if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                                all_generic_params.extend(impl_def.generics.clone());
                            }
                        }
                        all_generic_params.extend(f.generics.clone());

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
}
