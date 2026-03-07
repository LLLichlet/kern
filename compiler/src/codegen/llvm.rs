// src/codegen/llvm.rs

use std::collections::HashMap;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::builder::Builder;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue, GlobalValue, BasicMetadataValueEnum};
use inkwell::types::{BasicType, BasicTypeEnum, StructType};
use inkwell::AddressSpace;

use crate::mast::ast::*;
use crate::sema::ty::{TypeId, TypeKind, PrimitiveType, TypeRegistry};

pub struct CodeGenerator<'ctx, 'a> {
    pub context: &'ctx LlvmContext,
    pub builder: Builder<'ctx>,
    pub module: LlvmModule<'ctx>,
    
    // 前端类型注册表，用于查询具体类型
    pub type_registry: &'a TypeRegistry, 

    // === LLVM 实体映射表 (根据 MonoId 快速查找) ===
    pub structs: HashMap<MonoId, StructType<'ctx>>,
    pub globals: HashMap<MonoId, GlobalValue<'ctx>>,
    pub functions: HashMap<MonoId, FunctionValue<'ctx>>,
    
    // 当前正在编译的函数内部的局部变量映射 (SymbolId -> 栈上分配的内存指针)
    pub locals: HashMap<crate::utils::SymbolId, PointerValue<'ctx>>,

    // 用于追踪当前所处 Loop 的 (Continue Block, Break Block)
    pub loop_targets: Vec<(inkwell::basic_block::BasicBlock<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)>,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn new(context: &'ctx LlvmContext, module_name: &str, type_registry: &'a TypeRegistry) -> Self {
        Self {
            context,
            builder: context.create_builder(),
            module: context.create_module(module_name),
            type_registry,
            structs: HashMap::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            locals: HashMap::new(),
            loop_targets: Vec::new(),
        }
    }

    /// 核心编译入口
    pub fn compile(&mut self, module: &MastModule) {
        // 1. 声明阶段 (Declarations) - 解决函数和变量互相引用的问题
        self.declare_structs(&module.structs);
        self.declare_globals(&module.globals);
        self.declare_functions(&module.functions);

        // 2. 定义阶段 (Definitions) - 真正生成机器码和内存初始值
        for global in &module.globals {
            self.compile_global(global); // 初始化全局变量 (如 OFFSET = 100)
        }

        for function in &module.functions {
            if !function.is_extern {
                self.compile_function(function); // 🌟 将 AST 翻译为 LLVM IR 指令！
            }
        }
    }

    /// 编译全局变量的初始值
    fn compile_global(&mut self, global: &MastGlobal) {
        let global_val = *self.globals.get(&global.id).expect("Global should be declared");
        
        if let Some(init) = &global.init {
            let const_val: inkwell::values::BasicValueEnum<'ctx> = match &init.kind {
                crate::mast::ast::MastExprKind::Integer(val) => {
                    let int_type = self.get_llvm_type(init.ty).into_int_type();
                    int_type.const_int(*val as u64, false).into()
                }
                crate::mast::ast::MastExprKind::Float(val) => {
                    let float_type = self.get_llvm_type(init.ty).into_float_type();
                    float_type.const_float(*val).into()
                }
                crate::mast::ast::MastExprKind::Bool(val) => {
                    self.context.bool_type().const_int(if *val { 1 } else { 0 }, false).into()
                }
                // 复杂的结构体常量初始化可以后续扩展，这里暂时用 const_zero 兜底
                _ => self.get_llvm_type(global.ty).const_zero()
            };
            
            global_val.set_initializer(&const_val);
        } else if !global.is_extern {
            // 非 extern 的全局变量如果没有初始值，默认置为 0 (BSS段)
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    /// 调试用：打印生成的 LLVM IR 到标准错误输出
    pub fn print_ir(&self) {
        self.module.print_to_stderr();
    }

    // ==========================================
    //          Type Translation
    // ==========================================

    /// 将前端的 TypeId 无缝翻译为 LLVM 的 BasicType
    pub fn get_llvm_type(&self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        let mut norm = self.type_registry.normalize(ty);
        loop {
            match self.type_registry.get(norm) {
                crate::sema::ty::TypeKind::Mut(inner) => norm = *inner,
                _ => break,
            }
        }
        match self.type_registry.get(norm) {
            TypeKind::Primitive(p) => match p {
                PrimitiveType::I8 | PrimitiveType::U8 => self.context.i8_type().into(),
                PrimitiveType::I16 | PrimitiveType::U16 => self.context.i16_type().into(),
                PrimitiveType::I32 | PrimitiveType::U32 => self.context.i32_type().into(),
                PrimitiveType::I64 | PrimitiveType::U64 | PrimitiveType::ISize | PrimitiveType::USize => self.context.i64_type().into(),
                PrimitiveType::I128 | PrimitiveType::U128 => self.context.i128_type().into(),
                PrimitiveType::F32 => self.context.f32_type().into(),
                PrimitiveType::F64 => self.context.f64_type().into(),
                PrimitiveType::Bool => self.context.bool_type().into(),
                PrimitiveType::Str => self.context.ptr_type(inkwell::AddressSpace::default()).into(),
                PrimitiveType::Void => self.context.i8_type().into(),
            },
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) | TypeKind::Mut(_) => {
                // LLVM 15+ 拥抱不透明指针 (Opaque Pointers)
                self.context.ptr_type(AddressSpace::default()).into()
            }
            TypeKind::Array { elem, len } => {
                let elem_ty = self.get_llvm_type(*elem);
                elem_ty.array_type(*len as u32).into()
            }
            TypeKind::Slice(_) => {
                // 胖指针: { ptr, usize }
                self.context.struct_type(&[
                    self.context.ptr_type(AddressSpace::default()).into(),
                    self.context.i64_type().into(),
                ], false).into()
            }
            TypeKind::Def(def_id, _) => {
                self.context.ptr_type(AddressSpace::default()).into()
            }
            _ => self.context.i8_type().into(), // 兜底
        }
    }

    // ==========================================
    //          Phase 1: Declarations
    // ==========================================

    /// 声明所有结构体 (解决自引用问题)
    fn declare_structs(&mut self, structs: &[MastStruct]) {
        // 第一遍：创建所有不透明的结构体类型 (Opaque Structs)
        for s in structs {
            let llvm_struct = self.context.opaque_struct_type(&s.name);
            self.structs.insert(s.id, llvm_struct);
        }

        // 第二遍：填充结构体的字段 (Body)
        for s in structs {
            let llvm_struct = self.structs.get(&s.id).unwrap();
            let mut field_types = Vec::new();
            for field in &s.fields {
                field_types.push(self.get_llvm_type(field.ty));
            }
            // Kern 默认按照 C ABI 进行打包 (非 packed)
            llvm_struct.set_body(&field_types, false);
        }
    }

    /// 声明全局变量
    fn declare_globals(&mut self, globals: &[MastGlobal]) {
        for g in globals {
            // 注意：VTable 在 MAST 中被定义为 TypeId::ERROR（占位），我们需要特殊处理它
            let llvm_ty = if g.name.starts_with("__vtable") {
                // 虚表本质上是函数指针数组，我们简单将其声明为一个包含若干指针的结构体/数组
                // 实际上 LLVM 中可以直接声明为不透明类型的全局变量
                self.context.ptr_type(AddressSpace::default()).into()
            } else {
                self.get_llvm_type(g.ty)
            };

            let global_val = self.module.add_global(llvm_ty, None, &g.name);
            global_val.set_constant(!g.is_mut);

            // 如果是 extern 的，设置链接属性
            if g.is_extern {
                global_val.set_linkage(inkwell::module::Linkage::External);
            } else {
                // TODO: 全局变量的常量初始化 (Constant Initializer)
                // LLVM 要求全局变量必须有一个常量初始化器。
                // 我们需要在编译常量表达式后，调用 global_val.set_initializer(&const_val)
                // 暂时用 null / zero 占位：
                global_val.set_initializer(&llvm_ty.const_zero());
            }

            self.globals.insert(g.id, global_val);
        }
    }

    /// 声明函数签名
    fn declare_functions(&mut self, functions: &[MastFunction]) {
        for f in functions {
            let ret_ty = self.get_llvm_type(f.ret_ty);
            
            let mut param_types = Vec::new();
            for p in &f.params {
                param_types.push(self.get_llvm_type(p.ty).into());
            }

            // 构造函数签名类型
            let fn_type = match ret_ty {
                BasicTypeEnum::IntType(i) => i.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::StructType(s) => s.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, f.is_variadic),
                BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(&param_types, f.is_variadic),
            };

            // 特殊处理 void 返回值
            let fn_type = if f.ret_ty == TypeId::VOID {
                self.context.void_type().fn_type(&param_types, f.is_variadic)
            } else {
                fn_type
            };

            let llvm_func = self.module.add_function(&f.name, fn_type, None);
            self.functions.insert(f.id, llvm_func);
        }
    }

    // ==========================================
    //          Phase 2: Code Generation
    // ==========================================

    /// 编译单个函数体
    pub fn compile_function(&mut self, func: &MastFunction) {
        let llvm_func = self.functions.get(&func.id).unwrap().clone();
        
        // 1. 创建入口基本块 (Entry Basic Block)
        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);

        // 2. 清空上一段函数的局部变量映射表
        self.locals.clear();

        // 3. 将所有传入的参数在栈上分配内存，并将传进来的值 Store 进去
        // 这样参数就变成了普通的局部变量，完美统一 `Var` 的寻址逻辑
        for (i, param) in func.params.iter().enumerate() {
            let param_val = llvm_func.get_nth_param(i as u32).unwrap();
            let param_ty = self.get_llvm_type(param.ty);
            
            // 在当前入口块分配栈内存
            let alloca = self.builder.build_alloca(param_ty, &format!("arg_{}", param.name.0)).unwrap();
            
            // 将参数值存入栈中
            self.builder.build_store(alloca, param_val).unwrap();
            
            // 记录到本地变量映射表中
            self.locals.insert(param.name, alloca);
        }

        // 4. 编译函数体 (Block)
        if let Some(body) = &func.body {
            self.compile_block(body);
            
            // 兜底：如果函数没有显式 Return，且返回类型是 Void，自动补上 return void
            // （在真正的编译器中，最好在 MAST lowering 阶段就保证所有的末尾都有 return）
            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_none() {
                if func.ret_ty == TypeId::VOID {
                    self.builder.build_return(None).unwrap();
                } else {
                    // 对于必须有返回值的函数，如果没有 return，这是一个严重错误
                    // LLVM 会生成一个 unreachable 指令
                    self.builder.build_unreachable().unwrap();
                }
            }
        }
    }

    /// 编译代码块
    fn compile_block(&mut self, block: &MastBlock) -> Option<BasicValueEnum<'ctx>> {
        for stmt in &block.stmts {
            self.compile_stmt(stmt);
        }
        
        // 如果 Block 有返回值，编译并返回它
        if let Some(result_expr) = &block.result {
            return Some(self.compile_expr(result_expr));
        }
        None
    }

    /// 编译单条语句
    fn compile_stmt(&mut self, stmt: &MastStmt) {
        match stmt {
            MastStmt::Let { name, ty, init } => {
                let init_val = self.compile_expr(init);
                let llvm_ty = self.get_llvm_type(*ty);
                
                // Let 语句等价于：分配内存 -> 求出初始值 -> 写入内存 -> 登记到字典
                let alloca = self.builder.build_alloca(llvm_ty, &format!("let_{}", name.0)).unwrap();
                self.builder.build_store(alloca, init_val).unwrap();
                self.locals.insert(*name, alloca);
            }
            MastStmt::Expr(expr) => {
                // 单纯执行表达式，丢弃返回值 (比如单独的函数调用)
                self.compile_expr(expr);
            }
        }
    }

    /// 核心：将 MAST 表达式编译为具体的 LLVM 指令 (Value)
    fn compile_expr(&mut self, expr: &crate::mast::ast::MastExpr) -> inkwell::values::BasicValueEnum<'ctx> {
        let expected_llvm_ty = self.get_llvm_type(expr.ty);

        use crate::mast::ast::MastExprKind;
        match &expr.kind {
            // === 1. 字面量 ===
            MastExprKind::Undef => expected_llvm_ty.const_zero(), // 使用 const_zero 完美替代 undef
            MastExprKind::Integer(val) => {
                let int_type = expected_llvm_ty.into_int_type();
                int_type.const_int(*val as u64, false).into()
            }
            MastExprKind::Float(val) => {
                let float_type = expected_llvm_ty.into_float_type();
                float_type.const_float(*val).into()
            }
            MastExprKind::Bool(val) => {
                let bool_type = self.context.bool_type();
                bool_type.const_int(if *val { 1 } else { 0 }, false).into()
            }
            MastExprKind::StringLiteral(s) => {
                // LLVM 中字符串是全局常量数组，返回指向它的指针
                self.builder.build_global_string_ptr(s, "str_lit").unwrap().as_pointer_value().into()
            }

            // === 2. 内存与寻址 ===
            MastExprKind::Var(name) => {
                let ptr = self.locals.get(name).expect("Local variable not found in LLVM codegen");
                self.builder.build_load(expected_llvm_ty, *ptr, &format!("load_{}", name.0)).unwrap()
            }
            MastExprKind::GlobalRef(mono_id) => {
                let global_val = self.globals.get(mono_id).expect("Global not found");
                let ptr = global_val.as_pointer_value();
                self.builder.build_load(expected_llvm_ty, ptr, "global_load").unwrap()
            }
            MastExprKind::FuncRef(mono_id) => {
                // 函数本身在 LLVM 中就是一个全局指针
                let func_val = self.functions.get(mono_id).expect("Function not found");
                func_val.as_global_value().as_pointer_value().into()
            }
            MastExprKind::AddressOf(operand) => {
                self.compile_lvalue(operand).into()
            }
            MastExprKind::Deref(operand) => {
                let ptr_val = self.compile_expr(operand).into_pointer_value();
                self.builder.build_load(expected_llvm_ty, ptr_val, "deref").unwrap()
            }

            // === 3. 聚合类型操作 (Struct / Array / Field / Index) ===
            MastExprKind::StructInit { struct_id, fields } => {
                // ✅ 核心修复：无视带有 Mut 外壳的 expected_llvm_ty，直接从字典中取出 100% 纯净的 StructType
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                
                // 无论字典里存的是 BasicTypeEnum 还是 StructType，这一步都能安全拿到 StructValue 的 zero 底板
                use inkwell::types::BasicType;
                let mut current_struct = struct_llvm_ty.as_basic_type_enum().into_struct_type().const_zero();
                
                // 按照字段索引，将值逐个“塞入”结构体中
                for (idx, field_expr) in fields.iter().enumerate() {
                    let field_val = self.compile_expr(field_expr);
                    current_struct = self.builder
                        .build_insert_value(current_struct, field_val, idx as u32, "struct_init")
                        .unwrap()
                        .into_struct_value();
                }
                
                current_struct.into()
            }

            MastExprKind::ArrayInit(elems) => {
                // 对于 Array，为了防止 Mut 外壳干扰，我们在这里手动剥离
                let norm_ty = self.type_registry.normalize(expr.ty);
                let base_ty = if let crate::sema::ty::TypeKind::Mut(inner) = self.type_registry.get(norm_ty) {
                    *inner
                } else {
                    norm_ty
                };
                
                // 获取纯净的 ArrayType
                let array_llvm_ty = self.get_llvm_type(base_ty).into_array_type();
                let mut current_array = array_llvm_ty.const_zero();

                for (idx, elem_expr) in elems.iter().enumerate() {
                    let elem_val = self.compile_expr(elem_expr);
                    current_array = self.builder
                        .build_insert_value(current_array, elem_val, idx as u32, "array_init")
                        .unwrap()
                        .into_array_value();
                }

                current_array.into()
            }
            MastExprKind::FieldAccess { lhs, struct_id, field_idx } => {
                let struct_ptr = self.compile_expr(lhs).into_pointer_value();
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                let field_ptr = self.builder.build_struct_gep(*struct_llvm_ty, struct_ptr, *field_idx as u32, "field_gep").unwrap();
                self.builder.build_load(expected_llvm_ty, field_ptr, "field_load").unwrap()
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let array_ptr = self.compile_lvalue(lhs);
                let idx_val = self.compile_expr(index).into_int_value();
                let zero = self.context.i64_type().const_zero();
                let array_llvm_ty = self.get_llvm_type(lhs.ty);
                
                let elem_ptr = unsafe {
                    self.builder.build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "idx_gep").unwrap()
                };
                self.builder.build_load(expected_llvm_ty, elem_ptr, "idx_load").unwrap()
            }

            // === 4. 调用机制 ===
            MastExprKind::Call { callee, args } => {
                let mut llvm_args = Vec::new();
                for arg in args {
                    llvm_args.push(self.compile_expr(arg).into());
                }

                let call_site = if let MastExprKind::FuncRef(mono_id) = callee.kind {
                    let llvm_func = self.functions.get(&mono_id).unwrap();
                    self.builder.build_call(*llvm_func, &llvm_args, "call_ret").unwrap()
                } else {
                    let ptr_val = self.compile_expr(callee).into_pointer_value();
                    let norm_ty = self.type_registry.normalize(callee.ty);
                    
                    let fn_type = if let crate::sema::ty::TypeKind::Function { params, ret, is_variadic } = self.type_registry.get(norm_ty) {
                        let mut param_types = Vec::new();
                        for p in params {
                            param_types.push(self.get_llvm_type(*p).into());
                        }
                        let ret_ty = *ret;
                        
                        if ret_ty == crate::sema::ty::TypeId::VOID {
                            self.context.void_type().fn_type(&param_types, *is_variadic)
                        } else {
                            match self.get_llvm_type(ret_ty) {
                                inkwell::types::BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::VectorType(v) => v.fn_type(&param_types, *is_variadic),
                                inkwell::types::BasicTypeEnum::ScalableVectorType(sv) => sv.fn_type(&param_types, *is_variadic),
                            }
                        }
                    } else {
                        unreachable!("Callee must be a function type");
                    };
                    
                    self.builder.build_indirect_call(fn_type, ptr_val, &llvm_args, "icall").unwrap()
                };
                
                if expr.ty == crate::sema::ty::TypeId::VOID {
                    self.context.i8_type().const_zero().into() 
                } else {
                    call_site.try_as_basic_value().unwrap_basic() 
                }
            }

            // === 5. 简单的算术运算 ===
            MastExprKind::Binary { op, lhs, rhs } => {
                let l_val = self.compile_expr(lhs);
                let r_val = self.compile_expr(rhs);
                
                if l_val.is_int_value() && r_val.is_int_value() {
                    let l_int = l_val.into_int_value();
                    let r_int = r_val.into_int_value();
                    match op {
                        crate::ast::BinaryOperator::Add => self.builder.build_int_add(l_int, r_int, "add").unwrap().into(),
                        crate::ast::BinaryOperator::Subtract => self.builder.build_int_sub(l_int, r_int, "sub").unwrap().into(),
                        crate::ast::BinaryOperator::Multiply => self.builder.build_int_mul(l_int, r_int, "mul").unwrap().into(),
                        crate::ast::BinaryOperator::Divide => self.builder.build_int_signed_div(l_int, r_int, "sdiv").unwrap().into(),
                        crate::ast::BinaryOperator::Modulo => self.builder.build_int_signed_rem(l_int, r_int, "srem").unwrap().into(),
                        crate::ast::BinaryOperator::BitwiseAnd | crate::ast::BinaryOperator::LogicalAnd => self.builder.build_and(l_int, r_int, "and").unwrap().into(),
                        crate::ast::BinaryOperator::BitwiseOr | crate::ast::BinaryOperator::LogicalOr => self.builder.build_or(l_int, r_int, "or").unwrap().into(),
                        crate::ast::BinaryOperator::BitwiseXor => self.builder.build_xor(l_int, r_int, "xor").unwrap().into(),
                        crate::ast::BinaryOperator::ShiftLeft => self.builder.build_left_shift(l_int, r_int, "shl").unwrap().into(),
                        crate::ast::BinaryOperator::ShiftRight => self.builder.build_right_shift(l_int, r_int, false, "shr").unwrap().into(),
                        crate::ast::BinaryOperator::Equal => self.builder.build_int_compare(inkwell::IntPredicate::EQ, l_int, r_int, "eq").unwrap().into(),
                        crate::ast::BinaryOperator::NotEqual => self.builder.build_int_compare(inkwell::IntPredicate::NE, l_int, r_int, "ne").unwrap().into(),
                        crate::ast::BinaryOperator::LessThan => self.builder.build_int_compare(inkwell::IntPredicate::SLT, l_int, r_int, "slt").unwrap().into(),
                        crate::ast::BinaryOperator::LessOrEqual => self.builder.build_int_compare(inkwell::IntPredicate::SLE, l_int, r_int, "sle").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterThan => self.builder.build_int_compare(inkwell::IntPredicate::SGT, l_int, r_int, "sgt").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterOrEqual => self.builder.build_int_compare(inkwell::IntPredicate::SGE, l_int, r_int, "sge").unwrap().into(),
                    }
                } else if l_val.is_float_value() && r_val.is_float_value() {
                    let l_float = l_val.into_float_value();
                    let r_float = r_val.into_float_value();
                    match op {
                        crate::ast::BinaryOperator::Add => self.builder.build_float_add(l_float, r_float, "fadd").unwrap().into(),
                        crate::ast::BinaryOperator::Subtract => self.builder.build_float_sub(l_float, r_float, "fsub").unwrap().into(),
                        crate::ast::BinaryOperator::Multiply => self.builder.build_float_mul(l_float, r_float, "fmul").unwrap().into(),
                        crate::ast::BinaryOperator::Divide => self.builder.build_float_div(l_float, r_float, "fdiv").unwrap().into(),
                        crate::ast::BinaryOperator::Modulo => self.builder.build_float_rem(l_float, r_float, "frem").unwrap().into(),
                        crate::ast::BinaryOperator::Equal => self.builder.build_float_compare(inkwell::FloatPredicate::OEQ, l_float, r_float, "feq").unwrap().into(),
                        crate::ast::BinaryOperator::NotEqual => self.builder.build_float_compare(inkwell::FloatPredicate::ONE, l_float, r_float, "fne").unwrap().into(),
                        crate::ast::BinaryOperator::LessThan => self.builder.build_float_compare(inkwell::FloatPredicate::OLT, l_float, r_float, "flt").unwrap().into(),
                        crate::ast::BinaryOperator::LessOrEqual => self.builder.build_float_compare(inkwell::FloatPredicate::OLE, l_float, r_float, "fle").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterThan => self.builder.build_float_compare(inkwell::FloatPredicate::OGT, l_float, r_float, "fgt").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterOrEqual => self.builder.build_float_compare(inkwell::FloatPredicate::OGE, l_float, r_float, "fge").unwrap().into(),
                        _ => unreachable!("Invalid binary operator for float"),
                    }
                } else {
                    unreachable!("Binary operation on incompatible types");
                }
            }
            
            // === 6. 单目运算 ===
            MastExprKind::Unary { op, operand } => {
                let op_val = self.compile_expr(operand);
                match op {
                    crate::ast::UnaryOperator::Negate => {
                        if op_val.is_int_value() {
                            self.builder.build_int_neg(op_val.into_int_value(), "neg").unwrap().into()
                        } else {
                            self.builder.build_float_neg(op_val.into_float_value(), "fneg").unwrap().into()
                        }
                    }
                    crate::ast::UnaryOperator::LogicalNot | crate::ast::UnaryOperator::BitwiseNot => {
                        self.builder.build_not(op_val.into_int_value(), "not").unwrap().into()
                    }
                    crate::ast::UnaryOperator::LengthOf => {
                        let norm_ty = self.type_registry.normalize(operand.ty);
                        match self.type_registry.get(norm_ty) {
                            crate::sema::ty::TypeKind::Array { len, .. } => {
                                self.context.i64_type().const_int(*len, false).into()
                            }
                            crate::sema::ty::TypeKind::Slice(_) => {
                                // Slice 在 LLVM 是胖指针 { ptr, len }，len 在索引 1
                                self.builder.build_extract_value(op_val.into_struct_value(), 1, "slice_len").unwrap()
                            }
                            _ => unreachable!("LengthOf on invalid type")
                        }
                    }
                    _ => unreachable!() // AddressOf 和 Deref 已经是独立 MastExprKind
                }
            }

            MastExprKind::Return(ret_val) => {
                if let Some(val) = ret_val {
                    let llvm_val = self.compile_expr(val);
                    self.builder.build_return(Some(&llvm_val)).unwrap();
                } else {
                    self.builder.build_return(None).unwrap();
                }
                self.context.i8_type().const_zero().into()
            }

            MastExprKind::Assign { op, lhs, rhs } => {
                let ptr = self.compile_lvalue(lhs);
                let rhs_val = self.compile_expr(rhs);
                
                if *op == crate::ast::AssignmentOperator::Assign {
                    self.builder.build_store(ptr, rhs_val).unwrap();
                } else {
                    let expected_lhs_ty = self.get_llvm_type(lhs.ty);
                    let lhs_val = self.builder.build_load(expected_lhs_ty, ptr, "assign_load").unwrap();
                    
                    let new_val: inkwell::values::BasicValueEnum<'ctx> = if lhs_val.is_int_value() {
                        let l_int = lhs_val.into_int_value();
                        let r_int = rhs_val.into_int_value();
                        match op {
                            crate::ast::AssignmentOperator::AddAssign => self.builder.build_int_add(l_int, r_int, "add_a").unwrap().into(),
                            crate::ast::AssignmentOperator::SubtractAssign => self.builder.build_int_sub(l_int, r_int, "sub_a").unwrap().into(),
                            crate::ast::AssignmentOperator::MultiplyAssign => self.builder.build_int_mul(l_int, r_int, "mul_a").unwrap().into(),
                            crate::ast::AssignmentOperator::DivideAssign => self.builder.build_int_signed_div(l_int, r_int, "div_a").unwrap().into(),
                            crate::ast::AssignmentOperator::ModuloAssign => self.builder.build_int_signed_rem(l_int, r_int, "rem_a").unwrap().into(),
                            crate::ast::AssignmentOperator::BitwiseAndAssign => self.builder.build_and(l_int, r_int, "and_a").unwrap().into(),
                            crate::ast::AssignmentOperator::BitwiseOrAssign => self.builder.build_or(l_int, r_int, "or_a").unwrap().into(),
                            crate::ast::AssignmentOperator::BitwiseXorAssign => self.builder.build_xor(l_int, r_int, "xor_a").unwrap().into(),
                            crate::ast::AssignmentOperator::ShiftLeftAssign => self.builder.build_left_shift(l_int, r_int, "shl_a").unwrap().into(),
                            crate::ast::AssignmentOperator::ShiftRightAssign => self.builder.build_right_shift(l_int, r_int, false, "shr_a").unwrap().into(),
                            _ => unreachable!("Invalid integer assignment operator"),
                        }
                    } else if lhs_val.is_float_value() {
                        let l_float = lhs_val.into_float_value();
                        let r_float = rhs_val.into_float_value();
                        match op {
                            crate::ast::AssignmentOperator::AddAssign => self.builder.build_float_add(l_float, r_float, "fadd_a").unwrap().into(),
                            crate::ast::AssignmentOperator::SubtractAssign => self.builder.build_float_sub(l_float, r_float, "fsub_a").unwrap().into(),
                            crate::ast::AssignmentOperator::MultiplyAssign => self.builder.build_float_mul(l_float, r_float, "fmul_a").unwrap().into(),
                            crate::ast::AssignmentOperator::DivideAssign => self.builder.build_float_div(l_float, r_float, "fdiv_a").unwrap().into(),
                            crate::ast::AssignmentOperator::ModuloAssign => self.builder.build_float_rem(l_float, r_float, "frem_a").unwrap().into(),
                            _ => unreachable!("Invalid float assignment operator"),
                        }
                    } else {
                        unreachable!("Compound assignment on unsupported type");
                    };
                    self.builder.build_store(ptr, new_val).unwrap();
                }
                self.context.i8_type().const_zero().into()
            }

            MastExprKind::If { cond, then_branch, else_branch } => {
                let cond_val = self.compile_expr(cond).into_int_value();
                let parent_func = self.builder.get_insert_block().unwrap().get_parent().unwrap();

                let then_bb = self.context.append_basic_block(parent_func, "then");
                let else_bb = self.context.append_basic_block(parent_func, "else");
                let merge_bb = self.context.append_basic_block(parent_func, "ifcont");

                if else_branch.is_some() {
                    self.builder.build_conditional_branch(cond_val, then_bb, else_bb).unwrap();
                } else {
                    self.builder.build_conditional_branch(cond_val, then_bb, merge_bb).unwrap();
                }

                self.builder.position_at_end(then_bb);
                let then_result = self.compile_block(then_branch);
                let then_exit_bb = self.builder.get_insert_block().unwrap();
                if then_exit_bb.get_terminator().is_none() {
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }

                self.builder.position_at_end(else_bb);
                let mut else_result = None;
                if let Some(eb) = else_branch {
                    else_result = self.compile_block(eb);
                }
                let else_exit_bb = self.builder.get_insert_block().unwrap();
                if else_exit_bb.get_terminator().is_none() {
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }

                self.builder.position_at_end(merge_bb);
                if expr.ty != crate::sema::ty::TypeId::VOID && then_result.is_some() && else_result.is_some() {
                    let phi = self.builder.build_phi(expected_llvm_ty, "iftmp").unwrap();
                    phi.add_incoming(&[
                        (&then_result.unwrap(), then_exit_bb),
                        (&else_result.unwrap(), else_exit_bb),
                    ]);
                    phi.as_basic_value()
                } else {
                    self.context.i8_type().const_zero().into()
                }
            }

            MastExprKind::Loop(body) => {
                let parent_func = self.builder.get_insert_block().unwrap().get_parent().unwrap();
                let loop_bb = self.context.append_basic_block(parent_func, "loop");
                let merge_bb = self.context.append_basic_block(parent_func, "loopcont");

                self.builder.build_unconditional_branch(loop_bb).unwrap();
                self.builder.position_at_end(loop_bb);
                self.loop_targets.push((loop_bb, merge_bb));
                self.compile_block(body);
                
                let loop_exit_bb = self.builder.get_insert_block().unwrap();
                if loop_exit_bb.get_terminator().is_none() {
                    self.builder.build_unconditional_branch(loop_bb).unwrap();
                }
                self.loop_targets.pop();
                self.builder.position_at_end(merge_bb);
                self.context.i8_type().const_zero().into()
            }
            MastExprKind::Break => {
                let (_, merge_bb) = self.loop_targets.last().expect("Break outside of loop");
                self.builder.build_unconditional_branch(*merge_bb).unwrap();
                self.context.i8_type().const_zero().into()
            }
            MastExprKind::Continue => {
                let (loop_bb, _) = self.loop_targets.last().expect("Continue outside of loop");
                self.builder.build_unconditional_branch(*loop_bb).unwrap();
                self.context.i8_type().const_zero().into()
            }

            MastExprKind::Switch { target, cases, default_case } => {
                let target_val = self.compile_expr(target).into_int_value();
                let parent_func = self.builder.get_insert_block().unwrap().get_parent().unwrap();

                let merge_bb = self.context.append_basic_block(parent_func, "switchcont");
                let default_bb = self.context.append_basic_block(parent_func, "default");
                let mut case_blocks = Vec::new(); 
                let mut llvm_cases = Vec::new();

                for case in cases {
                    let case_bb = self.context.append_basic_block(parent_func, "case");
                    case_blocks.push(case_bb); 
                    for &val in &case.values {
                        let int_val = target_val.get_type().const_int(val as u64, false);
                        llvm_cases.push((int_val, case_bb));
                    }
                }

                self.builder.build_switch(target_val, default_bb, &llvm_cases).unwrap();

                self.builder.position_at_end(default_bb);
                if let Some(def_block) = default_case {
                    self.compile_block(def_block);
                }
                if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }

                for (i, case) in cases.iter().enumerate() {
                    self.builder.position_at_end(case_blocks[i]);
                    self.compile_block(&case.body);
                    if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                        self.builder.build_unconditional_branch(merge_bb).unwrap();
                    }
                }

                self.builder.position_at_end(merge_bb);
                self.context.i8_type().const_zero().into()
            }

            // === 7. 详尽的类型转换 Cast ===
            MastExprKind::Cast { kind, operand } => {
                let val = self.compile_expr(operand);
                let target_llvm_ty = expected_llvm_ty;

                use crate::mast::ast::MastCastKind::*;
                match kind {
                    Bitcast => self.builder.build_bit_cast(val, target_llvm_ty, "bitcast").unwrap(),
                    PtrToInt => self.builder.build_ptr_to_int(val.into_pointer_value(), target_llvm_ty.into_int_type(), "ptr2int").unwrap().into(),
                    IntToPtr => self.builder.build_int_to_ptr(val.into_int_value(), target_llvm_ty.into_pointer_type(), "int2ptr").unwrap().into(),
                    ZeroExt => self.builder.build_int_z_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "zext").unwrap().into(),
                    SignExt => self.builder.build_int_s_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "sext").unwrap().into(),
                    Trunc => self.builder.build_int_truncate(val.into_int_value(), target_llvm_ty.into_int_type(), "trunc").unwrap().into(),
                    IntToFloat => self.builder.build_signed_int_to_float(val.into_int_value(), target_llvm_ty.into_float_type(), "i2f").unwrap().into(),
                    FloatToInt => self.builder.build_float_to_signed_int(val.into_float_value(), target_llvm_ty.into_int_type(), "f2i").unwrap().into(),
                    FloatCast => self.builder.build_float_cast(val.into_float_value(), target_llvm_ty.into_float_type(), "fcast").unwrap().into(),
                    ArrayToSlice => {
                        // 切片是一个胖指针 { ptr, len }
                        let slice_ty = target_llvm_ty.into_struct_type();
                        let mut slice_val = slice_ty.const_zero();
                        
                        let array_ptr = self.compile_lvalue(operand);
                        slice_val = self.builder.build_insert_value(slice_val, array_ptr, 0, "slice_ptr").unwrap().into_struct_value();
                        
                        let len = if let crate::sema::ty::TypeKind::Array { len, .. } = self.type_registry.get(self.type_registry.normalize(operand.ty)) {
                            *len
                        } else { 0 };
                        
                        let len_val = self.context.i64_type().const_int(len, false);
                        slice_val = self.builder.build_insert_value(slice_val, len_val, 1, "slice_len").unwrap().into_struct_value();
                        
                        slice_val.into()
                    }
                }
            }

            // === 8. 胖指针 / Trait Object ===
            MastExprKind::ConstructFatPointer { data_ptr, vtable_ptr } => {
                let fat_ptr_ty = expected_llvm_ty.into_struct_type();
                let mut fat_ptr = fat_ptr_ty.const_zero(); 
                
                let data_val = self.compile_expr(data_ptr);
                fat_ptr = self.builder.build_insert_value(fat_ptr, data_val, 0, "fat_data").unwrap().into_struct_value();
                
                let vtable_val = self.globals.get(vtable_ptr).unwrap().as_pointer_value();
                fat_ptr = self.builder.build_insert_value(fat_ptr, vtable_val, 1, "fat_vtable").unwrap().into_struct_value();
                
                fat_ptr.into()
            }

            // === 9. 块表达式 ===
            MastExprKind::Block(block) => {
                if let Some(res) = self.compile_block(block) {
                    res
                } else {
                    self.context.i8_type().const_zero().into()
                }
            }
        }
    }

    /// 计算左值表达式的内存地址 (Pointer)
    /// 用于赋值 (=, +=) 和 取址 (.&) 操作
    fn compile_lvalue(&mut self, expr: &MastExpr) -> PointerValue<'ctx> {
        match &expr.kind {
            MastExprKind::Var(name) => {
                *self.locals.get(name).expect("Local variable not found")
            }
            MastExprKind::GlobalRef(mono_id) => {
                self.globals.get(mono_id).expect("Global not found").as_pointer_value()
            }
            MastExprKind::FieldAccess { lhs, struct_id, field_idx } => {
                let struct_ptr = self.compile_lvalue(lhs);
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();            
                self.builder.build_struct_gep(*struct_llvm_ty, struct_ptr, *field_idx as u32, "lvalue_gep").unwrap()
            }
            MastExprKind::IndexAccess { lhs, index } => {
                let array_ptr = self.compile_lvalue(lhs);
                let idx_val = self.compile_expr(index).into_int_value();
                
                // 对于数组索引，LLVM GEP 需要两个索引：一个是解开指针本身的 0，一个是数组实际的 index
                let zero = self.context.i64_type().const_zero();
                
                let array_llvm_ty = self.get_llvm_type(lhs.ty); // 获取具体的 ArrayType
                unsafe {
                    self.builder.build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "lvalue_idx").unwrap()
                }
            }
            MastExprKind::Deref(operand) => {
                // 指针解引用的左值，就是指针本身的值！
                self.compile_expr(operand).into_pointer_value()
            }
            _ => panic!("Expression is not a valid l-value: {:?}", expr.kind),
        }
    }
}