use crate::llvm::CodeGenerator;
use inkwell::types::BasicTypeEnum;
use inkwell::values::BasicValueEnum;
use kernc_mast::{BitIntrinsicKind, MastAsmBlock, MastExpr, MastExprKind};
use kernc_sema::ty::{TypeId, TypeKind};

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
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
            let llvm_func = self.functions.get(&mono_id).unwrap();
            self.builder
                .build_call(*llvm_func, &llvm_args, "call_ret")
                .unwrap()
        } else {
            let ptr_val = self.compile_expr(callee).into_pointer_value();
            let norm_ty = self.type_registry.normalize(callee.ty);

            let fn_type = if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.type_registry.get(norm_ty)
            {
                let mut param_types = Vec::new();
                for p in params {
                    param_types.push(self.get_llvm_type(*p).into());
                }
                if *ret == TypeId::VOID {
                    self.context.void_type().fn_type(&param_types, *is_variadic)
                } else {
                    match self.get_llvm_type(*ret) {
                        BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                        BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                        _ => unreachable!(),
                    }
                }
            } else {
                unreachable!()
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

        // 2 & 3. 确定返回值类型，并直接构建函数签名 FunctionType
        let asm_fn_type = match asm_block.output_tys.len() {
            0 => {
                // 纯副作用汇编，返回 VoidType
                self.context.void_type().fn_type(&param_types, false)
            }
            1 => {
                // 单一返回值，使用 BasicTypeEnum
                match self.get_llvm_type(asm_block.output_tys[0]) {
                    BasicTypeEnum::IntType(i) => i.fn_type(&param_types, false),
                    BasicTypeEnum::FloatType(f) => f.fn_type(&param_types, false),
                    BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, false),
                    BasicTypeEnum::StructType(s) => s.fn_type(&param_types, false),
                    BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, false),
                    BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, false),
                    BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(&param_types, false),
                }
            }
            _ => {
                // 多个返回值，打包成匿名的 StructType
                let mut struct_fields = Vec::new();
                for &ty in &asm_block.output_tys {
                    struct_fields.push(self.get_llvm_type(ty));
                }
                let struct_ty = self.context.struct_type(&struct_fields, false);
                struct_ty.fn_type(&param_types, false)
            }
        };

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
        if asm_block.output_tys.len() > 0 {
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

        let intrinsic = inkwell::intrinsics::Intrinsic::find(intrinsic_name).unwrap();
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
}
