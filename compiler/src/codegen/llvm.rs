// src/codegen/llvm.rs

use std::collections::HashMap;
use inkwell::context::Context as LlvmContext;
use inkwell::module::Module as LlvmModule;
use inkwell::builder::Builder;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue, GlobalValue};
use inkwell::types::{BasicType, BasicTypeEnum, StructType};
use inkwell::AddressSpace;
use inkwell::targets::{Target, RelocMode, CodeModel, FileType, InitializationConfig};

use crate::mast::ast::*;
use crate::sema::ty::{TypeId, TypeKind, PrimitiveType, TypeRegistry};
use crate::sema::def::Def;

pub struct CodeGenerator<'ctx, 'a> {
    pub context: &'ctx LlvmContext,
    pub builder: Builder<'ctx>,
    pub module: LlvmModule<'ctx>,
    
    // 前端类型注册表与原始定义，用于反查结构体名
    pub type_registry: &'a TypeRegistry,
    pub ctx_defs: &'a Vec<Def>, 
    pub ctx_resolve: &'a dyn Fn(crate::utils::SymbolId) -> &'a str,

    pub structs: HashMap<MonoId, StructType<'ctx>>,
    pub globals: HashMap<MonoId, GlobalValue<'ctx>>,
    pub functions: HashMap<MonoId, FunctionValue<'ctx>>,
    
    pub locals: HashMap<crate::utils::SymbolId, PointerValue<'ctx>>,
    pub loop_targets: Vec<(inkwell::basic_block::BasicBlock<'ctx>, inkwell::basic_block::BasicBlock<'ctx>)>,
}

impl<'ctx, 'a> CodeGenerator<'ctx, 'a> {
    pub fn new(
        context: &'ctx LlvmContext, 
        module_name: &str, 
        type_registry: &'a TypeRegistry,
        ctx_defs: &'a Vec<Def>,
        ctx_resolve: &'a dyn Fn(crate::utils::SymbolId) -> &'a str,
    ) -> Self {
        Self {
            context,
            builder: context.create_builder(),
            module: context.create_module(module_name),
            type_registry,
            ctx_defs,
            ctx_resolve,
            structs: HashMap::new(),
            globals: HashMap::new(),
            functions: HashMap::new(),
            locals: HashMap::new(),
            loop_targets: Vec::new(),
        }
    }

    pub fn compile(&mut self, module: &MastModule) {
        self.declare_structs(&module.structs);
        self.declare_globals(&module.globals);
        self.declare_functions(&module.functions);

        for global in &module.globals {
            self.compile_global(global); 
        }

        for function in &module.functions {
            if !function.is_extern {
                self.compile_function(function); 
            }
        }
    }

    fn compile_global(&mut self, global: &MastGlobal) {
        if global.is_extern { return; }
        let global_val = *self.globals.get(&global.id).expect("Global should be declared");
        
        if let Some(init) = &global.init {
            let const_val: inkwell::values::BasicValueEnum<'ctx> = match &init.kind {
                MastExprKind::Integer(val) => {
                    let int_type = self.get_llvm_type(init.ty).into_int_type();
                    int_type.const_int(*val as u64, false).into()
                }
                MastExprKind::Float(val) => {
                    let float_type = self.get_llvm_type(init.ty).into_float_type();
                    float_type.const_float(*val).into()
                }
                MastExprKind::Bool(val) => {
                    self.context.bool_type().const_int(if *val { 1 } else { 0 }, false).into()
                }
                MastExprKind::StringLiteral(s) => {
                    let bytes = self.context.const_string(s.as_bytes(), false);
                    bytes.into()
                }
                MastExprKind::ArrayInit(elems) => {
                    let mut ptr_vals = Vec::new();
                    for e in elems {
                        if let MastExprKind::FuncRef(mono_id) = e.kind {
                            let func_val = self.functions.get(&mono_id).unwrap();
                            ptr_vals.push(func_val.as_global_value().as_pointer_value());
                        } else {
                            // 兜底防爆：如果不是函数引用，塞入 Null 指针
                            ptr_vals.push(self.context.ptr_type(AddressSpace::default()).const_null());
                        }
                    }
                    let ptr_ty = self.context.ptr_type(AddressSpace::default());
                    ptr_ty.const_array(&ptr_vals).into()
                }
                _ => self.get_llvm_type(global.ty).const_zero()
            };
            
            global_val.set_initializer(&const_val);
        } else if !global.is_extern {
            let llvm_ty = self.get_llvm_type(global.ty);
            global_val.set_initializer(&llvm_ty.const_zero());
        }
    }

    pub fn print_ir(&self) {
        self.module.print_to_stderr();
    }

    // ==========================================
    //          Type Translation
    // ==========================================

    pub fn get_llvm_type(&self, ty: TypeId) -> BasicTypeEnum<'ctx> {
        let mut norm = self.type_registry.normalize(ty);
        loop {
            match self.type_registry.get(norm) {
                TypeKind::Mut(inner) => norm = *inner,
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
                PrimitiveType::Str => self.context.ptr_type(AddressSpace::default()).into(),
                PrimitiveType::Void => self.context.i8_type().into(),
            },
            TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) 
            | TypeKind::Function { .. } | TypeKind::FnDef(..) => {
                self.context.ptr_type(AddressSpace::default()).into()
            }
            TypeKind::Array { elem, len } => {
                let elem_ty = self.get_llvm_type(*elem);
                elem_ty.array_type(*len as u32).into()
            }
            // ✅ 确保包含这两个胖指针类型的正确映射
            TypeKind::TraitObject(_, _) | TypeKind::Slice(_) => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let len_ty = self.context.i64_type(); 
                self.context.struct_type(&[ptr_ty.into(), len_ty.into()], false).into()
            }
            TypeKind::Def(def_id, args) => {
                let def = &self.ctx_defs[def_id.0 as usize];
                let mut mangled_name = (self.ctx_resolve)(def.name().unwrap()).to_string();
                for arg in args { mangled_name.push_str(&format!("_{}", arg.0)); }
                
                if let Some(struct_ty) = self.module.get_struct_type(&mangled_name) {
                    struct_ty.into()
                } else {
                    self.context.i8_type().into()
                }
            }
            _ => unreachable!(
                "Frontend failed to resolve type! TypeId: {:?}, Kind: {:?}", 
                norm, 
                self.type_registry.get(norm)
            ),
        }
    }

    // ==========================================
    //          Phase 1: Declarations
    // ==========================================

    fn declare_structs(&mut self, structs: &[MastStruct]) {
        for s in structs {
            let llvm_struct = self.context.opaque_struct_type(&s.name);
            self.structs.insert(s.id, llvm_struct);
        }

        for s in structs {
            let llvm_struct = self.structs.get(&s.id).unwrap();
            let mut field_types = Vec::new();
            for field in &s.fields {
                field_types.push(self.get_llvm_type(field.ty));
            }
            llvm_struct.set_body(&field_types, false);
        }
    }

    fn declare_globals(&mut self, globals: &[MastGlobal]) {
        for g in globals {
            let llvm_ty = self.get_llvm_type(g.ty);

            let global_val = self.module.add_global(llvm_ty, None, &g.name);
            global_val.set_constant(!g.is_mut);

            if g.is_extern {
                global_val.set_linkage(inkwell::module::Linkage::External);
            } else {
                global_val.set_initializer(&llvm_ty.const_zero());
            }

            self.globals.insert(g.id, global_val);
        }
    }

    fn declare_functions(&mut self, functions: &[MastFunction]) {
        for f in functions {
            let ret_ty = self.get_llvm_type(f.ret_ty);
            
            let mut param_types = Vec::new();
            for p in &f.params {
                param_types.push(self.get_llvm_type(p.ty).into());
            }

            let fn_type = if f.ret_ty == TypeId::VOID {
                self.context.void_type().fn_type(&param_types, f.is_variadic)
            } else {
                match ret_ty {
                    BasicTypeEnum::IntType(i) => i.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::StructType(s) => s.fn_type(&param_types, f.is_variadic),
                    BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, f.is_variadic),
                    _ => unreachable!("Invalid return type"),
                }
            };

            let llvm_func = self.module.add_function(&f.name, fn_type, None);
            self.functions.insert(f.id, llvm_func);
        }
    }

    // ==========================================
    //          Phase 2: Code Generation
    // ==========================================

    pub fn compile_function(&mut self, func: &MastFunction) {
        let llvm_func = self.functions.get(&func.id).unwrap().clone();
        
        let entry_block = self.context.append_basic_block(llvm_func, "entry");
        self.builder.position_at_end(entry_block);
        self.locals.clear();

        for (i, param) in func.params.iter().enumerate() {
            let param_val = llvm_func.get_nth_param(i as u32).unwrap();
            let param_ty = self.get_llvm_type(param.ty);
            
            let alloca = self.builder.build_alloca(param_ty, &format!("arg_{}", param.name.0)).unwrap();
            self.builder.build_store(alloca, param_val).unwrap();
            self.locals.insert(param.name, alloca);
        }

        if let Some(body) = &func.body {
            // 🌟 1. 捕获 Block 执行完抛出的最后那个值 (比如 test3 里的 0)
            let block_res = self.compile_block(body);
            
            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_none() {
                // 🌟 2. 如果 Block 有返回值，自动帮它生成 ret 指令！
                if let Some(val) = block_res {
                    self.builder.build_return(Some(&val)).unwrap();
                } else if func.ret_ty == TypeId::VOID {
                    self.builder.build_return(None).unwrap();
                } else {
                    self.builder.build_unreachable().unwrap();
                }
            }
        }
    }

    fn compile_block(&mut self, block: &MastBlock) -> Option<BasicValueEnum<'ctx>> {
        for stmt in &block.stmts {
            let current_block = self.builder.get_insert_block().unwrap();
            if current_block.get_terminator().is_some() {
                break;
            }

            match stmt {
                MastStmt::Let { name, ty, init } => {
                    let init_val = self.compile_expr(init);
                    let llvm_ty = self.get_llvm_type(*ty);
                    let alloca = self.builder.build_alloca(llvm_ty, &format!("let_{}", name.0)).unwrap();
                    self.builder.build_store(alloca, init_val).unwrap();
                    self.locals.insert(*name, alloca);
                }
                MastStmt::Expr(expr) => {
                    self.compile_expr(expr);
                }
            }
        }
        
        let current_block = self.builder.get_insert_block().unwrap();
        if current_block.get_terminator().is_some() {
            return None; // 已经被终止了，不需要再返回结果
        }

        if let Some(result_expr) = &block.result {
            return Some(self.compile_expr(result_expr));
        }
        None
    }

    fn compile_expr(&mut self, expr: &MastExpr) -> inkwell::values::BasicValueEnum<'ctx> {
        let expected_llvm_ty = self.get_llvm_type(expr.ty);

        match &expr.kind {
            MastExprKind::Undef => expected_llvm_ty.const_zero(),
            MastExprKind::Integer(val) => expected_llvm_ty.into_int_type().const_int(*val as u64, false).into(),
            MastExprKind::Float(val) => expected_llvm_ty.into_float_type().const_float(*val).into(),
            MastExprKind::Bool(val) => self.context.bool_type().const_int(if *val { 1 } else { 0 }, false).into(),
            MastExprKind::StringLiteral(_) => unreachable!("Handled dynamically in Globals"),

            MastExprKind::Var(name) => {
                let ptr = self.locals.get(name).expect("Local variable not found");
                self.builder.build_load(expected_llvm_ty, *ptr, &format!("load_{}", name.0)).unwrap()
            }
            MastExprKind::GlobalRef(mono_id) => {
                let global_val = self.globals.get(mono_id).expect("Global not found");
                let ptr = global_val.as_pointer_value();
                self.builder.build_load(expected_llvm_ty, ptr, "global_load").unwrap()
            }
            MastExprKind::FuncRef(mono_id) => {
                let func_val = self.functions.get(mono_id).expect("Function not found");
                func_val.as_global_value().as_pointer_value().into()
            }
            MastExprKind::AddressOf(operand) => self.compile_lvalue(operand).into(),
            MastExprKind::Deref(operand) => {
                let ptr_val = self.compile_expr(operand).into_pointer_value();
                self.builder.build_load(expected_llvm_ty, ptr_val, "deref").unwrap()
            }

            MastExprKind::StructInit { struct_id, fields } => {
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                let mut current_struct = struct_llvm_ty.as_basic_type_enum().into_struct_type().const_zero();
                
                for (idx, field_expr) in fields.iter().enumerate() {
                    let field_val = self.compile_expr(field_expr);
                    current_struct = self.builder.build_insert_value(current_struct, field_val, idx as u32, "s_init").unwrap().into_struct_value();
                }
                current_struct.into()
            }
            
            MastExprKind::UnionInit { union_id, field_idx: _, value } => {
                // 加上 .copied() (或者解引用 *)，立刻释放对 self.structs 的不可变借用
                let union_llvm_ty = *self.structs.get(union_id).unwrap();
                
                // 分配内存 (此时 self 可以被可变借用了)
                let alloca = self.builder.build_alloca(union_llvm_ty, "union_init").unwrap();
                let val = self.compile_expr(value);
                
                self.builder.build_store(alloca, val).unwrap();
                
                self.builder.build_load(union_llvm_ty.as_basic_type_enum(), alloca, "union_load").unwrap()
            }

            MastExprKind::ArrayInit(elems) => {
                let array_llvm_ty = expected_llvm_ty.into_array_type();
                let mut current_array = array_llvm_ty.const_zero();
                for (idx, elem_expr) in elems.iter().enumerate() {
                    let elem_val = self.compile_expr(elem_expr);
                    current_array = self.builder.build_insert_value(current_array, elem_val, idx as u32, "arr_init").unwrap().into_array_value();
                }
                current_array.into()
            }

            MastExprKind::FieldAccess { lhs, struct_id, field_idx } => {
                let struct_ptr = self.compile_lvalue(lhs);
                let struct_llvm_ty = self.structs.get(struct_id).unwrap();
                let field_ptr = self.builder.build_struct_gep(*struct_llvm_ty, struct_ptr, *field_idx as u32, "field_gep").unwrap();
                self.builder.build_load(expected_llvm_ty, field_ptr, "field_load").unwrap()
            }

            // ✅ 核心修复：安全区分 Slice 索引和 Array 索引
           MastExprKind::IndexAccess { lhs, index } => {
                let idx_val = self.compile_expr(index).into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);
                
                let elem_ptr = if let TypeKind::Slice(_) = self.type_registry.get(norm_lhs) {
                    let slice_val = self.compile_expr(lhs).into_struct_value();
                    let ptr_val = self.builder.build_extract_value(slice_val, 0, "slice_ptr").unwrap().into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe { self.builder.build_gep(elem_ty, ptr_val, &[idx_val], "slice_idx").unwrap() }
                } else if let TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) = self.type_registry.get(norm_lhs) {
                    // 🌟 修复 1：支持对裸指针直接进行索引！直接 compile_expr 当做右值指针
                    let ptr_val = self.compile_expr(lhs).into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    // 注意：指针的 GEP 只需要一个索引参数 `[idx_val]`
                    unsafe { self.builder.build_gep(elem_ty, ptr_val, &[idx_val], "ptr_idx").unwrap() }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    // 注意：数组的 GEP 需要两个索引参数 `[0, idx_val]`
                    unsafe { self.builder.build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_idx").unwrap() }
                };
                
                // IndexAccess 本身自带 Load
                self.builder.build_load(expected_llvm_ty, elem_ptr, "idx_load").unwrap()
            }

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
                    
                    let fn_type = if let TypeKind::Function { params, ret, is_variadic } = self.type_registry.get(norm_ty) {
                        let mut param_types = Vec::new();
                        for p in params { param_types.push(self.get_llvm_type(*p).into()); }
                        if *ret == TypeId::VOID {
                            self.context.void_type().fn_type(&param_types, *is_variadic)
                        } else {
                            let ret_t = self.get_llvm_type(*ret);
                            match ret_t {
                                BasicTypeEnum::IntType(i) => i.fn_type(&param_types, *is_variadic),
                                BasicTypeEnum::FloatType(fl) => fl.fn_type(&param_types, *is_variadic),
                                BasicTypeEnum::PointerType(p) => p.fn_type(&param_types, *is_variadic),
                                BasicTypeEnum::StructType(s) => s.fn_type(&param_types, *is_variadic),
                                BasicTypeEnum::ArrayType(a) => a.fn_type(&param_types, *is_variadic),
                                _ => unreachable!(),
                            }
                        }
                    } else { unreachable!() };
                    
                    self.builder.build_indirect_call(fn_type, ptr_val, &llvm_args, "icall").unwrap()
                };
                
                if expr.ty == TypeId::VOID || expr.ty == TypeId::ERROR {
                    self.context.i8_type().const_zero().into() 
                } else {
                    call_site.try_as_basic_value().unwrap_basic() 
                }
            }

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
                        crate::ast::BinaryOperator::BitwiseAnd => self.builder.build_and(l_int, r_int, "and").unwrap().into(),
                        crate::ast::BinaryOperator::BitwiseOr => self.builder.build_or(l_int, r_int, "or").unwrap().into(),
                        crate::ast::BinaryOperator::BitwiseXor => self.builder.build_xor(l_int, r_int, "xor").unwrap().into(),
                        crate::ast::BinaryOperator::ShiftLeft => self.builder.build_left_shift(l_int, r_int, "shl").unwrap().into(),
                        crate::ast::BinaryOperator::ShiftRight => self.builder.build_right_shift(l_int, r_int, false, "shr").unwrap().into(),
                        crate::ast::BinaryOperator::Equal => self.builder.build_int_compare(inkwell::IntPredicate::EQ, l_int, r_int, "eq").unwrap().into(),
                        crate::ast::BinaryOperator::NotEqual => self.builder.build_int_compare(inkwell::IntPredicate::NE, l_int, r_int, "ne").unwrap().into(),
                        crate::ast::BinaryOperator::LessThan => self.builder.build_int_compare(inkwell::IntPredicate::SLT, l_int, r_int, "slt").unwrap().into(),
                        crate::ast::BinaryOperator::LessOrEqual => self.builder.build_int_compare(inkwell::IntPredicate::SLE, l_int, r_int, "sle").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterThan => self.builder.build_int_compare(inkwell::IntPredicate::SGT, l_int, r_int, "sgt").unwrap().into(),
                        crate::ast::BinaryOperator::GreaterOrEqual => self.builder.build_int_compare(inkwell::IntPredicate::SGE, l_int, r_int, "sge").unwrap().into(),
                        _ => unreachable!("Operator handled elsewhere"),
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
                        _ => unreachable!(),
                    }
                } else { unreachable!() }
            }

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
                        // 🌟 修复：给操作数剥离 Mut
                        let mut norm_ty = self.type_registry.normalize(operand.ty);
                        if let TypeKind::Mut(inner) = self.type_registry.get(norm_ty) {
                            norm_ty = *inner;
                        }
                        
                        match self.type_registry.get(norm_ty) {
                            TypeKind::Array { len, .. } => self.context.i64_type().const_int(*len, false).into(),
                            TypeKind::Slice(_) => self.builder.build_extract_value(op_val.into_struct_value(), 1, "slice_len").unwrap(),
                            _ => unreachable!()
                        }
                    }
                    _ => unreachable!() 
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
                            _ => unreachable!(),
                        }
                    } else {
                        unreachable!();
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
                if expr.ty != TypeId::VOID && then_result.is_some() && else_result.is_some() {
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

                // 🌟 核心修复：收集所有分支的返回值和对应的 Block，供 PHI 节点使用
                let mut incoming = Vec::new(); 

                self.builder.position_at_end(default_bb);
                if let Some(def_block) = default_case {
                    if let Some(val) = self.compile_block(def_block) {
                        incoming.push((val, self.builder.get_insert_block().unwrap()));
                    }
                    if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                        self.builder.build_unconditional_branch(merge_bb).unwrap();
                    }
                } else {
                    // 🌟 核心修复：前端保证了穷尽性，这里如果走到 default 就是不可达的
                    self.builder.build_unreachable().unwrap();
                }
                if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                    self.builder.build_unconditional_branch(merge_bb).unwrap();
                }

                for (i, case) in cases.iter().enumerate() {
                    self.builder.position_at_end(case_blocks[i]);
                    if let Some(val) = self.compile_block(&case.body) {
                        incoming.push((val, self.builder.get_insert_block().unwrap()));
                    }
                    if self.builder.get_insert_block().unwrap().get_terminator().is_none() {
                        self.builder.build_unconditional_branch(merge_bb).unwrap();
                    }
                }

                self.builder.position_at_end(merge_bb);
                
                // 生成汇编级的 PHI (多路选择) 节点
                if expr.ty != TypeId::VOID && !incoming.is_empty() {
                    let phi = self.builder.build_phi(expected_llvm_ty, "switchtmp").unwrap();
                    let mut incoming_refs = Vec::new();
                    for (val, bb) in &incoming {
                        incoming_refs.push((val as &dyn inkwell::values::BasicValue<'ctx>, *bb));
                    }
                    phi.add_incoming(&incoming_refs);
                    phi.as_basic_value()
                } else {
                    self.context.i8_type().const_zero().into()
                }
            }

            MastExprKind::Cast { kind, operand } => {
                let val = self.compile_expr(operand);
                let target_llvm_ty = expected_llvm_ty;

                match kind {
                    MastCastKind::Bitcast => {
                        // 防御性修复：如果前端错误地将 Slice 转 Ptr 标记为了 Bitcast
                        if val.is_struct_value() && target_llvm_ty.is_pointer_type() {
                            let fat_ptr = val.into_struct_value();
                            self.builder.build_extract_value(fat_ptr, 0, "slice_ptr_fallback").unwrap()
                                .into_pointer_value().into()
                        } else {
                            self.builder.build_bit_cast(val, target_llvm_ty, "bitcast").unwrap()
                        }
                    }
                    MastCastKind::PtrToInt => self.builder.build_ptr_to_int(val.into_pointer_value(), target_llvm_ty.into_int_type(), "ptr2int").unwrap().into(),
                    MastCastKind::IntToPtr => self.builder.build_int_to_ptr(val.into_int_value(), target_llvm_ty.into_pointer_type(), "int2ptr").unwrap().into(),
                    MastCastKind::ZeroExt => self.builder.build_int_z_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "zext").unwrap().into(),
                    MastCastKind::SignExt => self.builder.build_int_s_extend(val.into_int_value(), target_llvm_ty.into_int_type(), "sext").unwrap().into(),
                    MastCastKind::Trunc => self.builder.build_int_truncate(val.into_int_value(), target_llvm_ty.into_int_type(), "trunc").unwrap().into(),
                    MastCastKind::IntToFloat => self.builder.build_signed_int_to_float(val.into_int_value(), target_llvm_ty.into_float_type(), "i2f").unwrap().into(),
                    MastCastKind::FloatToInt => self.builder.build_float_to_signed_int(val.into_float_value(), target_llvm_ty.into_int_type(), "f2i").unwrap().into(),
                    MastCastKind::FloatCast => self.builder.build_float_cast(val.into_float_value(), target_llvm_ty.into_float_type(), "fcast").unwrap().into(),
                    MastCastKind::ArrayToSlice => {
                        let slice_ty = target_llvm_ty.into_struct_type();
                        let mut slice_val = slice_ty.const_zero();
                        
                        let array_ptr = self.compile_lvalue(operand);
                        slice_val = self.builder.build_insert_value(slice_val, array_ptr, 0, "slice_ptr").unwrap().into_struct_value();
                        
                        // 🌟 修复：剥离 Mut 以获取数组真实长度
                        let mut base_op_ty = self.type_registry.normalize(operand.ty);
                        if let TypeKind::Mut(inner) = self.type_registry.get(base_op_ty) {
                            base_op_ty = *inner;
                        }
                        
                        let len = if let TypeKind::Array { len, .. } = self.type_registry.get(base_op_ty) {
                            *len
                        } else { 0 };
                        
                        let len_val = self.context.i64_type().const_int(len, false);
                        slice_val = self.builder.build_insert_value(slice_val, len_val, 1, "slice_len").unwrap().into_struct_value();
                        
                        slice_val.into()
                    }
                    MastCastKind::SliceToPtr => {
                        let fat_ptr = val.into_struct_value();
                        
                        // 提取第 0 个元素 (即裸指针)
                        self.builder.build_extract_value(fat_ptr, 0, "slice_ptr").unwrap()
                            .into_pointer_value().into()
                    }
                }
            }

            MastExprKind::ConstructFatPointer { data_ptr, meta } => {
                let ptr_ty = self.context.ptr_type(AddressSpace::default());
                let len_ty = self.context.i64_type();
                // 显式声明这是一个 { ptr, i64 } 的结构体
                let fat_ptr_ty = self.context.struct_type(&[ptr_ty.into(), len_ty.into()], false);
                
                let mut fat_ptr = fat_ptr_ty.const_zero(); 
                
                let data_val = self.compile_expr(data_ptr);
                fat_ptr = self.builder.build_insert_value(fat_ptr, data_val, 0, "fat_data").unwrap().into_struct_value();
                
                let meta_val = self.compile_expr(meta);
                fat_ptr = self.builder.build_insert_value(fat_ptr, meta_val, 1, "fat_meta").unwrap().into_struct_value();
                
                fat_ptr.into()
            }

            MastExprKind::Block(block) => {
                if let Some(res) = self.compile_block(block) {
                    res
                } else {
                    self.context.i8_type().const_zero().into()
                }
            }

            MastExprKind::ExtractFatPtrData(fat_ptr_expr) => {
                let fat_ptr_val = self.compile_expr(fat_ptr_expr).into_struct_value();
                // 提取 0 号索引，即 data_ptr (*void)
                self.builder.build_extract_value(fat_ptr_val, 0, "extract_data").unwrap()
            }
            MastExprKind::ExtractFatPtrMeta(fat_ptr_expr) => {
                let fat_ptr_val = self.compile_expr(fat_ptr_expr).into_struct_value();
                // 提取 1 号索引，即 meta (usize)
                self.builder.build_extract_value(fat_ptr_val, 1, "extract_meta").unwrap()
            }
        }
    }

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
                let idx_val = self.compile_expr(index).into_int_value();
                let norm_lhs = self.type_registry.normalize(lhs.ty);
                
                if let TypeKind::Slice(_) = self.type_registry.get(norm_lhs) {
                    let slice_val = self.compile_expr(lhs).into_struct_value();
                    let ptr_val = self.builder.build_extract_value(slice_val, 0, "slice_ptr").unwrap().into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe { self.builder.build_gep(elem_ty, ptr_val, &[idx_val], "slice_lvalue").unwrap() }
                } else if let TypeKind::Pointer(_) | TypeKind::VolatilePtr(_) = self.type_registry.get(norm_lhs) {
                    // 🌟 修复 2：支持裸指针的左值推导
                    let ptr_val = self.compile_expr(lhs).into_pointer_value();
                    let elem_ty = self.get_llvm_type(expr.ty);
                    unsafe { self.builder.build_gep(elem_ty, ptr_val, &[idx_val], "ptr_lvalue").unwrap() }
                } else {
                    let array_ptr = self.compile_lvalue(lhs);
                    let zero = self.context.i64_type().const_zero();
                    let array_llvm_ty = self.get_llvm_type(lhs.ty);
                    unsafe { self.builder.build_gep(array_llvm_ty, array_ptr, &[zero, idx_val], "array_lvalue").unwrap() }
                }
            }
            MastExprKind::Deref(operand) => {
                self.compile_expr(operand).into_pointer_value()
            }
            _ => panic!("Expression is not a valid l-value: {:?}", expr.kind),
        }
    }

    // ==========================================
    //          Object File Generation
    // ==========================================

    pub fn emit_to_file(&self, target_triple_str: &str, output_path: &str) -> Result<(), String> {
        // 1. 初始化所有的 LLVM Target (x86, ARM, RISCV 等)
        Target::initialize_all(&InitializationConfig::default());

        // 2. 解析目标架构三元组
        let triple = inkwell::targets::TargetTriple::create(target_triple_str);
        
        let target = Target::from_triple(&triple).map_err(|e| e.to_string())?;
        
        // 3. 创建目标机器实例 (配置优化级别、重定位模式等)
        let target_machine = target.create_target_machine(
            &triple,
            "generic", // CPU 类型
            "",        // 特性 (Features)
            inkwell::OptimizationLevel::Default, // 可根据传入的 OptLevel 动态调整
            RelocMode::Default,
            CodeModel::Default,
        ).ok_or("Failed to create target machine")?;

        // 4. 将目标机器的数据布局 (Data Layout) 和三元组写入当前 Module
        self.module.set_data_layout(&target_machine.get_target_data().get_data_layout());
        self.module.set_triple(&triple);

        if let Err(err) = self.module.verify() {
            // 如果 IR 有问题，它会打印出极其详细的错误信息（比如哪一行的 PHI 节点类型不对）
            eprintln!("🔥 LLVM IR Verification Failed:\n{}", err.to_string());
            // 顺便把畸形的 IR 打印出来，方便我们肉眼对比
            self.print_ir();
            return Err("Invalid LLVM IR generated".to_string());
        }
        
        // 5. 触发 LLVM 后端，直接将 IR 编译为二进制的 Object (.o) 文件！
        let path = std::path::Path::new(output_path);
        target_machine.write_to_file(&self.module, FileType::Object, path).map_err(|e| e.to_string())?;

        Ok(())
    }
}