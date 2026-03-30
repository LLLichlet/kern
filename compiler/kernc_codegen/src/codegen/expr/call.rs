use crate::codegen::CodeGenerator;
use crate::intrinsics::Intrinsic;
use crate::llvm_sys::core::LLVMSetWeak;
use crate::types::{BasicMetadataTypeEnum, BasicTypeEnum, FunctionType, IntType};
use crate::values::{AsValueRef, BasicValueEnum, FunctionValue};
use crate::{AddressSpace, AtomicOrdering as LlvmAtomicOrdering, AtomicRMWBinOp};
use kernc_mast::{BitIntrinsicKind, MastAsmBlock, MastExpr, MastExprKind};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::{AtomicOrdering, AtomicRmwOp};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    fn llvm_atomic_ordering(ordering: AtomicOrdering) -> LlvmAtomicOrdering {
        match ordering {
            AtomicOrdering::Relaxed => LlvmAtomicOrdering::Monotonic,
            AtomicOrdering::Acquire => LlvmAtomicOrdering::Acquire,
            AtomicOrdering::Release => LlvmAtomicOrdering::Release,
            AtomicOrdering::AcqRel => LlvmAtomicOrdering::AcquireRelease,
            AtomicOrdering::SeqCst => LlvmAtomicOrdering::SequentiallyConsistent,
        }
    }

    fn struct_field_index_by_name(
        &mut self,
        struct_id: kernc_mast::MonoId,
        name: &str,
    ) -> Option<u32> {
        let Some(fields) = self.struct_fields.get(&struct_id).cloned() else {
            self.sess.emit_ice(
                kernc_utils::Span::default(),
                format!(
                    "Kern ICE (Codegen): missing field metadata for struct MonoId {:?}.",
                    struct_id
                ),
            );
            return None;
        };

        for (idx, field) in fields.iter().enumerate() {
            if self.resolve_symbol(*field) == name {
                return Some(idx as u32);
            }
        }

        self.sess.emit_ice(
            kernc_utils::Span::default(),
            format!(
                "Kern ICE (Codegen): field `{}` not found in struct MonoId {:?}.",
                name, struct_id
            ),
        );
        None
    }

    fn atomic_xchg_pointer_width_int(&self) -> IntType<'ctx> {
        self.context
            .custom_width_int_type((self.sess.target.pointer_size * 8) as u32)
    }

    fn lookup_function_value(
        &mut self,
        mono_id: kernc_mast::MonoId,
        span: kernc_utils::Span,
    ) -> Option<FunctionValue<'ctx>> {
        match self.functions.get(&mono_id).copied() {
            Some(func) => Some(func),
            None => {
                self.sess.emit_ice(
                    span,
                    format!(
                        "Kern ICE (Codegen): function MonoId {:?} not declared before call emission.",
                        mono_id
                    ),
                );
                None
            }
        }
    }

    fn llvm_fn_type_from_callable(
        &mut self,
        callee_ty: TypeId,
        span: kernc_utils::Span,
    ) -> Option<FunctionType<'ctx>> {
        let norm_ty = self.type_registry.normalize(callee_ty);
        let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = self.type_registry.get(norm_ty)
        else {
            self.sess.emit_ice(
                span,
                format!(
                    "Kern ICE (Codegen): indirect call expected function type, found `{:?}`.",
                    self.type_registry.get(norm_ty)
                ),
            );
            return None;
        };

        let mut param_types = Vec::new();
        for p in params {
            param_types.push(self.get_llvm_type(*p).into());
        }

        let fn_ty = if *ret == TypeId::VOID {
            self.context.void_type().fn_type(&param_types, *is_variadic)
        } else {
            match self.get_llvm_type(*ret) {
                BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, *is_variadic),
                BasicTypeEnum::ScalableVectorType(v) => v.fn_type(&param_types, *is_variadic),
            }
        };

        Some(fn_ty)
    }

    fn inline_asm_fn_type(
        &mut self,
        asm_block: &MastAsmBlock,
        param_types: &[BasicMetadataTypeEnum<'ctx>],
    ) -> FunctionType<'ctx> {
        match asm_block.output_tys.len() {
            0 => self.context.void_type().fn_type(param_types, false),
            1 => match self.get_llvm_type(asm_block.output_tys[0]) {
                BasicTypeEnum::IntType(i) => i.fn_type(param_types, false),
                BasicTypeEnum::FloatType(f) => f.fn_type(param_types, false),
                BasicTypeEnum::PointerType(p) => p.fn_type(param_types, false),
                BasicTypeEnum::StructType(s) => s.fn_type(param_types, false),
                BasicTypeEnum::ArrayType(a) => a.fn_type(param_types, false),
                BasicTypeEnum::VectorType(v) => v.fn_type(param_types, false),
                BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(param_types, false),
            },
            _ => {
                let mut struct_fields = Vec::new();
                for &ty in &asm_block.output_tys {
                    struct_fields.push(self.get_llvm_type(ty));
                }
                self.context
                    .struct_type(&struct_fields, false)
                    .fn_type(param_types, false)
            }
        }
    }

    pub(crate) fn compile_call(
        &mut self,
        callee: &MastExpr,
        args: &[MastExpr],
        expr_ty: TypeId,
    ) -> BasicValueEnum<'ctx> {
        let mut llvm_args = Vec::new();
        for arg in args {
            llvm_args.push(self.compile_expr(arg).into());
        }

        let call_site = if let MastExprKind::FuncRef(mono_id) = callee.kind {
            let Some(llvm_func) = self.lookup_function_value(mono_id, callee.span) else {
                let llvm_ty = self.get_llvm_type(expr_ty);
                return self.get_undef_val(llvm_ty);
            };
            self.builder
                .build_call(llvm_func, &llvm_args, "call_ret")
                .unwrap()
        } else {
            let ptr_val = self.compile_expr(callee).into_pointer_value();
            let Some(fn_type) = self.llvm_fn_type_from_callable(callee.ty, callee.span) else {
                let llvm_ty = self.get_llvm_type(expr_ty);
                return self.get_undef_val(llvm_ty);
            };

            self.builder
                .build_indirect_call(fn_type, ptr_val, &llvm_args, "icall")
                .unwrap()
        };

        if expr_ty == TypeId::VOID || expr_ty == TypeId::ERROR {
            self.context.i8_type().const_zero().into()
        } else {
            call_site.try_as_basic_value().unwrap_basic()
        }
    }

    pub(crate) fn compile_inline_asm(&mut self, asm_block: &MastAsmBlock) -> BasicValueEnum<'ctx> {
        // 1. 准备传入给汇编块的参数类型和对应的值
        let mut param_types = Vec::new();
        let mut arg_values = Vec::new();

        for arg_expr in &asm_block.input_args {
            let llvm_val = self.compile_expr(arg_expr);
            arg_values.push(llvm_val.into());
            param_types.push(llvm_val.get_type().into());
        }

        let asm_fn_type = self.inline_asm_fn_type(asm_block, &param_types);

        // 4. 创建 InlineAsm 实例
        let has_side_effects = asm_block.is_volatile || asm_block.output_tys.is_empty();
        let inline_asm = self.context.create_inline_asm(
            asm_fn_type,
            asm_block.asm_template.clone(),
            asm_block.constraints.clone(),
            has_side_effects,
            false,
            Some(self.asm_dialect),
            false,
        );

        // 5. 调用汇编指令
        let call_site = self
            .builder
            .build_indirect_call(asm_fn_type, inline_asm, &arg_values, "asm_call")
            .unwrap();

        // 6. 将 LLVM 返回的值提取并 Store 到用户的指针中
        if !asm_block.output_tys.is_empty() {
            let asm_result = call_site.try_as_basic_value().unwrap_basic();

            for (i, ptr_expr) in asm_block.output_ptrs.iter().enumerate() {
                let target_ptr = self.compile_expr(ptr_expr).into_pointer_value();

                let extracted_val = if asm_block.output_tys.len() == 1 {
                    asm_result
                } else {
                    self.builder
                        .build_extract_value(
                            asm_result.into_struct_value(),
                            i as u32,
                            &format!("asm_out_{}", i),
                        )
                        .unwrap()
                };

                self.builder.build_store(target_ptr, extracted_val).unwrap();
            }
        }

        self.context.i8_type().const_zero().into()
    }
    pub(crate) fn compile_bit_intrinsic(
        &mut self,
        kind: BitIntrinsicKind,
        operand: &MastExpr,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let val = self.compile_expr(operand);

        let intrinsic_name = match kind {
            BitIntrinsicKind::PopCount => "llvm.ctpop",
            BitIntrinsicKind::Clz => "llvm.ctlz",
            BitIntrinsicKind::Ctz => "llvm.cttz",
            BitIntrinsicKind::Bswap => "llvm.bswap",
        };

        let intrinsic = Intrinsic::find(intrinsic_name).unwrap();
        let decl = intrinsic
            .get_declaration(&self.module, &[expected_ty])
            .unwrap();

        let call_site = if kind == BitIntrinsicKind::PopCount || kind == BitIntrinsicKind::Bswap {
            self.builder
                .build_call(decl, &[val.into()], "bit_op")
                .unwrap()
        } else {
            let is_zero_poison = self.context.bool_type().const_zero();
            self.builder
                .build_call(decl, &[val.into(), is_zero_poison.into()], "lz_tz")
                .unwrap()
        };

        call_site.try_as_basic_value().unwrap_basic()
    }

    pub(crate) fn compile_atomic_load(
        &mut self,
        ptr: &MastExpr,
        ordering: AtomicOrdering,
        expected_ty: BasicTypeEnum<'ctx>,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(ptr).into_pointer_value();
        let load = self
            .builder
            .build_load(expected_ty, ptr_val, "atomic_load")
            .unwrap();
        load.as_instruction_value()
            .unwrap()
            .set_atomic_ordering(Self::llvm_atomic_ordering(ordering))
            .unwrap();
        load
    }

    pub(crate) fn compile_atomic_store(
        &mut self,
        ptr: &MastExpr,
        value: &MastExpr,
        ordering: AtomicOrdering,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(ptr).into_pointer_value();
        let value_val = self.compile_expr(value);
        let store = self.builder.build_store(ptr_val, value_val).unwrap();
        store
            .set_atomic_ordering(Self::llvm_atomic_ordering(ordering))
            .unwrap();
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_atomic_fence(
        &mut self,
        ordering: AtomicOrdering,
    ) -> BasicValueEnum<'ctx> {
        self.builder
            .build_fence(Self::llvm_atomic_ordering(ordering), 0, "")
            .unwrap();
        self.context.i8_type().const_zero().into()
    }

    pub(crate) fn compile_atomic_rmw(
        &mut self,
        expr_ty: TypeId,
        op: AtomicRmwOp,
        ptr: &MastExpr,
        value: &MastExpr,
        ordering: AtomicOrdering,
    ) -> BasicValueEnum<'ctx> {
        let llvm_order = Self::llvm_atomic_ordering(ordering);
        let ptr_val = self.compile_expr(ptr).into_pointer_value();
        let value_val = self.compile_expr(value);

        if matches!(
            self.type_registry
                .get(self.type_registry.normalize(expr_ty)),
            TypeKind::Pointer { .. }
        ) && op == AtomicRmwOp::Xchg
        {
            let ptr_int_ty = self.atomic_xchg_pointer_width_int();
            let int_ptr_ty = self.context.ptr_type(AddressSpace::default());
            let cast_ptr = self
                .builder
                .build_pointer_cast(ptr_val, int_ptr_ty, "atomic_xchg_ptr_cast")
                .unwrap();
            let result_ptr_ty = self.get_llvm_type(expr_ty).into_pointer_type();
            let cast_val = self
                .builder
                .build_ptr_to_int(
                    value_val.into_pointer_value(),
                    ptr_int_ty,
                    "atomic_xchg_val",
                )
                .unwrap();
            let old_val = self
                .builder
                .build_atomicrmw(AtomicRMWBinOp::Xchg, cast_ptr, cast_val, llvm_order)
                .unwrap();
            return self
                .builder
                .build_int_to_ptr(old_val, result_ptr_ty, "atomic_xchg_old_ptr")
                .unwrap()
                .into();
        }

        let llvm_op = match op {
            AtomicRmwOp::Xchg => AtomicRMWBinOp::Xchg,
            AtomicRmwOp::Add => AtomicRMWBinOp::Add,
            AtomicRmwOp::Sub => AtomicRMWBinOp::Sub,
            AtomicRmwOp::And => AtomicRMWBinOp::And,
            AtomicRmwOp::Nand => AtomicRMWBinOp::Nand,
            AtomicRmwOp::Or => AtomicRMWBinOp::Or,
            AtomicRmwOp::Xor => AtomicRMWBinOp::Xor,
            AtomicRmwOp::Max => AtomicRMWBinOp::Max,
            AtomicRmwOp::Min => AtomicRMWBinOp::Min,
            AtomicRmwOp::UMax => AtomicRMWBinOp::UMax,
            AtomicRmwOp::UMin => AtomicRMWBinOp::UMin,
        };

        self.builder
            .build_atomicrmw(llvm_op, ptr_val, value_val.into_int_value(), llvm_order)
            .unwrap()
            .into()
    }

    pub(crate) fn compile_atomic_cas(
        &mut self,
        expr_ty: TypeId,
        weak: bool,
        ptr: &MastExpr,
        expected: &MastExpr,
        desired: &MastExpr,
        success: AtomicOrdering,
        failure: AtomicOrdering,
    ) -> BasicValueEnum<'ctx> {
        let ptr_val = self.compile_expr(ptr).into_pointer_value();
        let expected_val = self.compile_expr(expected);
        let desired_val = self.compile_expr(desired);
        let cas_pair = self
            .builder
            .build_cmpxchg(
                ptr_val,
                expected_val,
                desired_val,
                Self::llvm_atomic_ordering(success),
                Self::llvm_atomic_ordering(failure),
            )
            .unwrap();
        if weak {
            let Some(cas_inst) = cas_pair.as_instruction() else {
                self.sess.emit_ice(
                    expected.span,
                    "Kern ICE (Codegen): cmpxchg result did not lower to an instruction value.",
                );
                let llvm_ty = self.get_llvm_type(expr_ty);
                return self.get_undef_val(llvm_ty);
            };
            unsafe { LLVMSetWeak(cas_inst.as_value_ref(), 1) };
        }

        let old_val = self
            .builder
            .build_extract_value(cas_pair, 0, "cas_old")
            .unwrap();
        let success_val = self
            .builder
            .build_extract_value(cas_pair, 1, "cas_success")
            .unwrap();

        let norm_ty = self.type_registry.normalize(expr_ty);
        let Some(&struct_id) = self.anon_struct_map.get(&norm_ty) else {
            self.sess.emit_ice(
                expected.span,
                format!(
                    "Kern ICE (Codegen): cmpxchg result type `{:?}` was not instantiated as an anonymous struct.",
                    norm_ty
                ),
            );
            let llvm_ty = self.get_llvm_type(expr_ty);
            return self.get_undef_val(llvm_ty);
        };

        let struct_ty = self.get_llvm_type(expr_ty).into_struct_type();
        let mut result = struct_ty.const_zero();
        if let Some(idx) = self.struct_field_index_by_name(struct_id, "success") {
            result = self
                .builder
                .build_insert_value(result, success_val, idx, "cas_insert_success")
                .unwrap()
                .into_struct_value();
        }
        if let Some(idx) = self.struct_field_index_by_name(struct_id, "value") {
            result = self
                .builder
                .build_insert_value(result, old_val, idx, "cas_insert_value")
                .unwrap()
                .into_struct_value();
        }

        result.into()
    }
}
