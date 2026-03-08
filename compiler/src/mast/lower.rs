// src/mast/lower.rs
use std::collections::HashMap;
use crate::ast::{self, Expr, ExprKind};
use crate::context::Context;
use crate::sema::def::Def;
use crate::sema::ty::{TypeId, TypeKind, PrimitiveType};
use crate::sema::typeck::subst::Substituter;
use crate::utils::SymbolId;
use super::ast::*; 

/// MAST 降级引擎 (Monomorphization & Lowering)
pub struct Lowerer<'a> {
    pub ctx: &'a mut Context,
    pub module: MastModule,
    
    /// 单态化缓存：记录 `(DefId, [TypeId, ...])` 对应的 MonoId，防止重复克隆
    mono_cache: HashMap<(crate::sema::ty::DefId, Vec<TypeId>), MonoId>,
    next_mono_id: u32,

    /// Defer 栈：处理块级作用域的清理。每个 Block 对应一个 Vec<MastExpr>
    defer_stack: Vec<Vec<MastExpr>>,

    // 维护前端 Global DefId 到 MAST MonoId 的映射
    global_map: HashMap<crate::sema::ty::DefId, MonoId>,

    // VTable 缓存，键是 (SourceType, TraitType)
    vtable_cache: HashMap<(TypeId, TypeId), MonoId>,

    // 维护降级时的局部变量类型栈
    pub local_types: Vec<HashMap<SymbolId, TypeId>>,
}

impl<'a> Lowerer<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self {
            ctx,
            module: MastModule {
                name: "kern_out".to_string(),
                structs: Vec::new(),
                globals: Vec::new(),
                functions: Vec::new(),
            },
            mono_cache: HashMap::new(),
            next_mono_id: 1, 
            defer_stack: Vec::new(),
            global_map: HashMap::new(),
            vtable_cache: HashMap::new(),
            local_types: Vec::new(),
        }
    }

    fn new_mono_id(&mut self) -> MonoId {
        let id = self.next_mono_id;
        self.next_mono_id += 1;
        MonoId(id)
    }

    /// 降级入口：寻找所有非泛型的根节点向下递归单态化
    pub fn lower_all(&mut self) -> MastModule {
        let def_ids: Vec<_> = (0..self.ctx.defs.len())
            .map(|i| crate::sema::ty::DefId(i as u32))
            .collect();

        // Phase 1: 预分配全局变量的 MonoId
        for &id in &def_ids {
            if let Def::Global(_) = &self.ctx.defs[id.0 as usize] {
                let mono_id = self.new_mono_id();
                self.global_map.insert(id, mono_id);
            }
        }

        // Phase 2: 执行真正的实体降级
        for id in def_ids {
            let def = self.ctx.defs[id.0 as usize].clone();
            match def {
                Def::Function(f) => {
                    // 🌟 核心修复：检查函数自身和其父级（Impl块）是否包含泛型
                    // 只有自己没泛型，且爹也没泛型的函数，才是真正的“自由函数”，才能在此刻被实例化！
                    let mut is_generic = !f.generics.is_empty();
                    if let Some(parent_id) = f.parent {
                        if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                            if !impl_def.generics.is_empty() {
                                is_generic = true;
                            }
                        }
                    }
                    
                    if !is_generic {
                        self.instantiate_function(id, &[]);
                    }
                }
                Def::Global(g) => self.lower_global(&g),
                _ => {}
            }
        }
        
        self.module.clone()
    }

    // ==========================================
    //          Monomorphization Engine
    // ==========================================

    fn instantiate_function(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] { f.clone() } else { unreachable!() };
        
        // ==========================================
        // 🌟 核心修复：合并父级作用域 (Impl 块) 的泛型参数
        // 泛型参数环境 = [Impl 泛型] + [函数自身泛型]
        // ==========================================
        let mut all_generic_params = Vec::new();
        
        // 1. 如果这个函数属于某个 Impl 块，先把它身上的 T, U 拿过来
        if let Some(parent_id) = def.parent {
            if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                all_generic_params.extend(impl_def.generics.clone());
            }
        }
        
        // 2. 追加函数自身的泛型参数
        all_generic_params.extend(def.generics.clone());

        // 3. 将外部传入的具体类型 args 依次与收集到的泛型名对齐
        let mut subst_map = HashMap::new();
        for (i, param) in all_generic_params.iter().enumerate() {
            if i < args.len() {
                subst_map.insert(param.name, args[i]);
            }
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        let raw_ret = def.resolved_sig.map_or(TypeId::VOID, |sig| {
            if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig) { *ret } else { TypeId::VOID }
        });

        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        
        let mut mast_params = Vec::new();
        for p in &def.params {
            let raw_ty = self.ctx.node_types.get(&p.type_node.id).copied().unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_params.push(MastParam { name: p.name, ty: conc_ty });
        }
        
        let conc_ret = subst.substitute(raw_ret);

        self.local_types.push(std::collections::HashMap::new());
        for p in &mast_params {
            self.local_types.last_mut().unwrap().insert(p.name, p.ty);
        }

        let body = if let Some(body_expr) = &def.body {
            Some(self.lower_block_as_body(body_expr, &subst_map, conc_ret))
        } else { None };

        self.local_types.pop();
        
        let mast_fn = MastFunction {
            id, name: mangled_name, params: mast_params, ret_ty: conc_ret,
            body, is_extern: def.is_extern, is_variadic: def.is_variadic,
        };

        self.module.functions.push(mast_fn);
        id
    }

    fn instantiate_struct(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) { return id; }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] { s.clone() } 
                  else if let Def::Union(_) = &self.ctx.defs[def_id.0 as usize] {
                      return self.instantiate_union(def_id, args, id);
                  } else { unreachable!() };
        
        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args { mangled_name.push_str(&format!("_{}", arg.0)); }

        let mut mast_fields = Vec::new();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        
        for f in &def.fields {
            let raw_ty = self.ctx.node_types.get(&f.type_node.id).copied().unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_fields.push(MastField { name: f.name, ty: conc_ty });
        }

        self.module.structs.push(MastStruct {
            id, name: mangled_name, fields: mast_fields, is_extern: def.is_extern,
        });
        id
    }

    fn instantiate_union(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId], id: MonoId) -> MonoId {
        let def = if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] { u.clone() } else { unreachable!() };
        
        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args { mangled_name.push_str(&format!("_{}", arg.0)); }

        let mut mast_fields = Vec::new();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        
        for f in &def.fields {
            let raw_ty = self.ctx.node_types.get(&f.type_node.id).copied().unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_fields.push(MastField { name: f.name, ty: conc_ty });
        }

        self.module.structs.push(MastStruct {
            id, name: mangled_name, fields: mast_fields, is_extern: false, 
        });
        id
    }

    fn lower_global(&mut self, g: &crate::sema::def::GlobalDef) {
        let id = *self.global_map.get(&g.id).expect("Global MonoId should be pre-allocated");
        let ty = self.ctx.node_types.get(&g.value.id).copied().unwrap_or(TypeId::ERROR);
        
        let is_mut = matches!(
            self.ctx.type_registry.get(self.ctx.type_registry.normalize(ty)), 
            TypeKind::Mut(_)
        );

        let init = if !g.is_extern {
            Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty)))
        } else { None };

        self.module.globals.push(MastGlobal {
            id, name: self.ctx.resolve(g.name).to_string(), ty, is_mut, init, is_extern: g.is_extern,
        });
    }

    // ==========================================
    //          Block & Defer Unrolling
    // ==========================================

    fn lower_block_as_body(&mut self, block_expr: &Expr, subst_map: &HashMap<SymbolId, TypeId>, expected_ty: TypeId) -> MastBlock {
        self.defer_stack.push(Vec::new());
        self.local_types.push(HashMap::new());

        let mut stmts = Vec::new();
        let mut result = None;

        if let ExprKind::Block { stmts: ast_stmts, result: ast_res } = &block_expr.kind {
            for stmt in ast_stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                        if let ExprKind::Defer { expr: def_expr } = &e.kind {
                            let lowered = self.lower_expr(def_expr, subst_map, None);
                            self.defer_stack.last_mut().unwrap().push(lowered);
                        } else {
                            if let ExprKind::Let { name, init } = &e.kind {
                                let init_mast = self.lower_expr(init, subst_map, None);
                                let var_ty = init_mast.ty; // 绝对信任右侧推导出的类型
                                
                                self.local_types.last_mut().unwrap().insert(*name, var_ty);
                                stmts.push(MastStmt::Let { name: *name, ty: var_ty, init: init_mast });
                            } else {
                                let lowered = self.lower_expr(e, subst_map, None);
                                if !matches!(e.kind, ExprKind::Static { .. }) {
                                    stmts.push(MastStmt::Expr(lowered));
                                }
                            }
                        }
                    }
                }
            }
            if let Some(res) = ast_res {
                result = Some(Box::new(self.lower_expr(res, subst_map, Some(expected_ty))));
            }
        } else {
            result = Some(Box::new(self.lower_expr(block_expr, subst_map, Some(expected_ty))));
        }

        let defers = self.defer_stack.pop().unwrap();
        for d in defers.into_iter().rev() {
            stmts.push(MastStmt::Expr(d));
        }
        
        self.local_types.pop();
        MastBlock { stmts, result }
    }

    // ==========================================
    //          Expression Lowering
    // ==========================================

    fn lower_expr(&mut self, expr: &Expr, subst_map: &HashMap<SymbolId, TypeId>, expected_ty: Option<TypeId>) -> MastExpr {
        let mut raw_ty = self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR);
        
        if raw_ty == TypeId::ERROR {
            if let ExprKind::Identifier(name) = &expr.kind {
                for scope in self.local_types.iter().rev() {
                    if let Some(&local_ty) = scope.get(name) {
                        raw_ty = local_ty;
                        break;
                    }
                }
            }
        }
        let mut subst = Substituter::new(&mut self.ctx.type_registry, subst_map);
        let concrete_ty = subst.substitute(raw_ty);
        let mut exp_ty = expected_ty.unwrap_or(concrete_ty);
        
        if exp_ty == TypeId::ERROR {
            println!("--------------------------------------------------");
            println!("🔥 [LOWER TRAP] Lowering an expression with ERROR type!");
            println!("Span: {:?}", expr.span);
            // println!("ExprKind: {:#?}", expr.kind);
            println!("--------------------------------------------------");
        }

        loop {
            let norm = self.ctx.type_registry.normalize(exp_ty);
            if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm) {
                exp_ty = *inner;
            } else {
                exp_ty = norm;
                break;
            }
        }
        let mut mast_kind = match &expr.kind {
            ExprKind::Integer(val) => MastExprKind::Integer(*val),
            ExprKind::Float(val) => MastExprKind::Float(*val),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::String(s) => {
                let global_id = self.new_mono_id();
                let len = s.len() as u64;
                let array_ty = self.ctx.type_registry.intern(TypeKind::Array { elem: TypeId::U8, len });
                
                self.module.globals.push(MastGlobal {
                    id: global_id,
                    name: format!(".str.{}", global_id.0),
                    ty: array_ty,
                    is_mut: false,
                    init: Some(MastExpr { ty: array_ty, span: expr.span, kind: MastExprKind::StringLiteral(s.clone()) }),
                    is_extern: false,
                });

                // ✅ 核心魔法：直接在 MAST 层面组装出一个标准的 FatPointer！
                let data_ptr = MastExpr {
                    ty: self.ctx.type_registry.intern(TypeKind::Pointer(array_ty)),
                    span: expr.span,
                    // AddressOf + GlobalRef 完美取得了全局字符串的裸指针
                    kind: MastExprKind::AddressOf(Box::new(MastExpr {
                        ty: array_ty, span: expr.span, kind: MastExprKind::GlobalRef(global_id)
                    }))
                };
                
                let meta = MastExpr {
                    ty: TypeId::USIZE, span: expr.span, kind: MastExprKind::Integer(len as u128)
                };

                // 因为 ExprKind::String 返回的是 Slice，这里的 MAST 类型完全匹配
                MastExprKind::ConstructFatPointer { data_ptr: Box::new(data_ptr), meta: Box::new(meta) }
            }

            ExprKind::Identifier(name) => {
                if let Some(info) = self.ctx.scopes.resolve(*name).cloned() {
                    match info.kind {
                        crate::sema::scope::SymbolKind::Static | crate::sema::scope::SymbolKind::Const => {
                            let def_id = info.def_id.unwrap();
                            if let Some(&mono_id) = self.global_map.get(&def_id) {
                                MastExprKind::GlobalRef(mono_id)
                            } else { unreachable!() }
                        }
                        crate::sema::scope::SymbolKind::Function => {
                            let fn_def_id = info.def_id.unwrap();
                            let mono_id = self.instantiate_function(fn_def_id, &[]);
                            MastExprKind::FuncRef(mono_id)
                        }
                        _ => MastExprKind::Var(*name),
                    }
                } else {
                    MastExprKind::Var(*name)
                }
            }

            ExprKind::Let { init, .. } => {
                return self.lower_expr(init, subst_map, Some(concrete_ty));
            }

            ExprKind::Static { name, init } => {
                let global_id = self.new_mono_id();
                let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));
                let is_mut = matches!(
                    self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)), 
                    TypeKind::Mut(_)
                );

                self.module.globals.push(MastGlobal {
                    id: global_id,
                    name: format!("local_static_{}_{}", self.ctx.resolve(*name), global_id.0),
                    ty: concrete_ty, is_mut, init: Some(lower_init), is_extern: false,
                });
                MastExprKind::GlobalRef(global_id)
            }

            // 短路逻辑运算直接降级为 If 分支
            ExprKind::Binary { lhs, op, rhs } => {
                if *op == crate::ast::BinaryOperator::LogicalAnd {
                    let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
                    let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
                    MastExprKind::If {
                        cond: Box::new(l),
                        then_branch: MastBlock { stmts: vec![], result: Some(Box::new(r)) },
                        else_branch: Some(MastBlock { stmts: vec![], result: Some(Box::new(MastExpr { ty: TypeId::BOOL, span: expr.span, kind: MastExprKind::Bool(false) })) }),
                    }
                } else if *op == crate::ast::BinaryOperator::LogicalOr {
                    let l = self.lower_expr(lhs, subst_map, Some(TypeId::BOOL));
                    let r = self.lower_expr(rhs, subst_map, Some(TypeId::BOOL));
                    MastExprKind::If {
                        cond: Box::new(l),
                        then_branch: MastBlock { stmts: vec![], result: Some(Box::new(MastExpr { ty: TypeId::BOOL, span: expr.span, kind: MastExprKind::Bool(true) })) },
                        else_branch: Some(MastBlock { stmts: vec![], result: Some(Box::new(r)) }),
                    }
                } else {
                    let l = self.lower_expr(lhs, subst_map, None);
                    let r = self.lower_expr(rhs, subst_map, Some(l.ty));
                    MastExprKind::Binary { op: *op, lhs: Box::new(l), rhs: Box::new(r) }
                }
            }

            ExprKind::Unary { op, operand } => {
                let op_mast = self.lower_expr(operand, subst_map, None);
                match op {
                    crate::ast::UnaryOperator::AddressOf => MastExprKind::AddressOf(Box::new(op_mast)),
                    crate::ast::UnaryOperator::PointerDeRef => MastExprKind::Deref(Box::new(op_mast)),
                    _ => MastExprKind::Unary { op: *op, operand: Box::new(op_mast) }
                }
            }

            ExprKind::Call { callee, args } => {
                let mut receiver_mast = None;
                let mut is_method = false;
                let mut method_field_sym = None;
                
                // 🚀 1. 嗅探是否为方法调用，并提前独占式提取 Receiver
                if let ExprKind::FieldAccess { lhs, field } = &callee.kind {
                    let callee_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
                    let norm_callee = self.ctx.type_registry.normalize(callee_ty);
                    
                    if matches!(self.ctx.type_registry.get(norm_callee), TypeKind::FnDef(..) | TypeKind::Function {..}) {
                        is_method = true;
                        method_field_sym = Some(*field);
                        // 🌟 只在这里下降一次 Receiver，绝不重复求值！
                        receiver_mast = Some(self.lower_expr(lhs, subst_map, None)); 
                    }
                }

                // 🌟 新增：在下降参数之前，统一提取被调用者的完整参数签名！
                let callee_ty = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR);
                let norm_callee = self.ctx.type_registry.normalize(callee_ty);
                
                let expected_param_tys = match self.ctx.type_registry.get(norm_callee).clone() {
                    TypeKind::Function { params, .. } => params.clone(), // 动态派发的 Trait Method
                    TypeKind::FnDef(def_id, gen_args) => {
                        if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] {
                            if let Some(sig) = f.resolved_sig {
                                // 🌟 步骤 1：在没有任何可变借用的时候，先去不可变地读取原始参数签名！
                                let norm_sig = self.ctx.type_registry.normalize(sig);
                                let raw_params = if let TypeKind::Function { params, .. } = self.ctx.type_registry.get(norm_sig).clone() {
                                    params
                                } else {
                                    Vec::new()
                                };

                                // 🌟 步骤 2：准备泛型映射表的数据
                                let mut all_generic_params = Vec::new();
                                if let Some(parent_id) = f.parent {
                                    if let Def::Impl(impl_def) = &self.ctx.defs[parent_id.0 as usize] {
                                        all_generic_params.extend(impl_def.generics.clone());
                                    }
                                }
                                all_generic_params.extend(f.generics.clone());
                                
                                let mut sig_subst_map = std::collections::HashMap::new();
                                for (idx, param) in all_generic_params.iter().enumerate() {
                                    if idx < gen_args.len() {
                                        sig_subst_map.insert(param.name, gen_args[idx]);
                                    }
                                }
                                
                                // 🌟 步骤 3：开启可变借用，此时再去执行替换！
                                let mut sig_subst = crate::sema::typeck::subst::Substituter::new(&mut self.ctx.type_registry, &sig_subst_map);
                                
                                raw_params.into_iter().map(|p| sig_subst.substitute(p)).collect()
                            } else { Vec::new() }
                        } else { Vec::new() }
                    },
                    _ => Vec::new(),
                };

                // 🚀 2. 准备普通的实参
                let mut arg_masts = Vec::new();
                for (i, a) in args.iter().enumerate() { 
                    // 🌟 修复：处理隐式的 self 参数偏移！
                    // 如果是方法，用户传的 args[0] 对应签名里的 params[1] (params[0] 是 receiver)
                    let param_idx = if is_method { i + 1 } else { i };
                    let exp_ty = expected_param_tys.get(param_idx).copied();

                    // 传递 expected_ty，触发 ArrayToSlice 隐式降级等机制！
                    arg_masts.push(self.lower_expr(a, subst_map, exp_ty)); 
                }

                // 🚀 3. 如果是方法调用，进入专门的分发逻辑
                if is_method {
                    let field = method_field_sym.unwrap();
                    let recv = receiver_mast.unwrap();
                    let _ = self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR); // 这个其实在原代码里用来获取原始类型
                    
                    // 剥离 Receiver 的指针/Mut 属性，看它到底是个啥
                    let mut base_ty = recv.ty;
                    loop {
                        let norm = self.ctx.type_registry.normalize(base_ty);
                        match self.ctx.type_registry.get(norm) {
                            TypeKind::Mut(inner) | TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => base_ty = *inner,
                            _ => break,
                        }
                    }
                    let norm_base = self.ctx.type_registry.normalize(base_ty);
                    let norm_callee = self.ctx.type_registry.normalize(self.ctx.node_types.get(&callee.id).copied().unwrap_or(TypeId::ERROR));

                    // ==========================================
                    // 🌊 分支 A：动态分发 (Trait Object 虚表查表)
                    // ==========================================
                    if let TypeKind::TraitObject(trait_id, _) = self.ctx.type_registry.get(norm_base) {
                        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_id.0 as usize] { t } else { unreachable!() };
                        let vtable_idx = trait_def.methods.iter().position(|m| m.name == field).expect("Method not found in trait");

                        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer(TypeId::VOID));

                        // a. 提取 Data 指针 (直接作为 *void 类型)
                        let data_ptr = MastExpr {
                            ty: void_ptr_ty, 
                            span: callee.span,
                            kind: MastExprKind::ExtractFatPtrData(Box::new(recv.clone()))
                        };
                        arg_masts.insert(0, data_ptr);

                        // b. 提取 vtable_ptr (目前是 USIZE 整数)
                        let vtable_meta = MastExpr {
                            ty: TypeId::USIZE, 
                            span: callee.span,
                            kind: MastExprKind::ExtractFatPtrMeta(Box::new(recv))
                        };

                        // c. 将 vtable_ptr 强转为 *(*void) 指针类型
                        let vtable_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer(void_ptr_ty));
                        let vtable_ptr = MastExpr {
                            ty: vtable_ptr_ty,
                            span: callee.span,
                            kind: MastExprKind::Cast { kind: MastCastKind::IntToPtr, operand: Box::new(vtable_meta) }
                        };

                        // d. 🌟 核心：利用 IndexAccess 取出对应的函数指针 (*void)。它自带 Load！
                        let func_ptr = MastExpr {
                            ty: void_ptr_ty,
                            span: callee.span,
                            kind: MastExprKind::IndexAccess {
                                lhs: Box::new(vtable_ptr),
                                index: Box::new(MastExpr { ty: TypeId::USIZE, span: callee.span, kind: MastExprKind::Integer(vtable_idx as u128) })
                            }
                        };

                        // e. 🌟 核心：构建打了补丁的函数签名: fn(*void, arg1...) -> ret
                        // 从 norm_callee 提取出原始的参数列表
                        let mut patched_params = if let TypeKind::Function { params, .. } = self.ctx.type_registry.get(norm_callee) {
                            params.clone()
                        } else {
                            Vec::new()
                        };
                        
                        if !patched_params.is_empty() {
                            patched_params[0] = void_ptr_ty; // 强制要求第一个参数接收 *void
                        }
                        
                        let ret_ty = if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(norm_callee) { *ret } else { TypeId::VOID };
                        let is_variadic = if let TypeKind::Function { is_variadic, .. } = self.ctx.type_registry.get(norm_callee) { *is_variadic } else { false };
                        
                        let patched_fn_ty = self.ctx.type_registry.intern(TypeKind::Function {
                            params: patched_params,
                            ret: ret_ty,
                            is_variadic,
                        });

                        // f. 将取出的 *void 函数指针 Bitcast 为打补丁后的签名类型
                        let func_ptr_typed = MastExpr {
                            ty: patched_fn_ty, 
                            span: callee.span,
                            kind: MastExprKind::Cast { kind: MastCastKind::Bitcast, operand: Box::new(func_ptr) }
                        };

                        // g. 生成最终 Call 节点
                        return MastExpr {
                            ty: self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR),
                            span: expr.span,
                            kind: MastExprKind::Call { callee: Box::new(func_ptr_typed), args: arg_masts }
                        };
                    }
                    // ==========================================
                    // ⚡ 分支 B：静态分发 (普通结构体的泛型方法)
                    // ==========================================
                    else if let TypeKind::FnDef(method_id, generics) = self.ctx.type_registry.get(norm_callee).clone() {
                        // 将 Receiver 原封不动作为第 0 个实参压入
                        arg_masts.insert(0, recv);
                        
                        let func_id = self.instantiate_function(method_id, &generics);
                        let func_ref = MastExpr { ty: norm_callee, span: callee.span, kind: MastExprKind::FuncRef(func_id) };
                        
                        return MastExpr {
                            ty: self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR),
                            span: expr.span,
                            kind: MastExprKind::Call { callee: Box::new(func_ref), args: arg_masts }
                        };
                    }
                }

                // 🚀 4. 如果不是方法调用，走常规的普通函数下降逻辑
                let callee_mast = self.lower_expr(callee, subst_map, None);
                let func_info = if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty) {
                    Some((*fn_id, fn_args.clone()))
                } else { None };

                if let Some((fn_id, fn_args)) = func_info {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    MastExprKind::Call { 
                        callee: Box::new(MastExpr { ty: callee_mast.ty, span: callee.span, kind: MastExprKind::FuncRef(mono_id) }), 
                        args: arg_masts 
                    }
                } else {
                    MastExprKind::Call { callee: Box::new(callee_mast), args: arg_masts }
                }
            }

            ExprKind::FieldAccess { lhs, field } => {
                let lhs_ty = self.ctx.node_types.get(&lhs.id).copied().unwrap_or(TypeId::ERROR);
                let norm_lhs = self.ctx.type_registry.normalize(lhs_ty);
                let expr_ty = self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR);
                let norm_expr = self.ctx.type_registry.normalize(expr_ty);

                // 🌟 防御拦截：如果在这走到函数/方法，说明试图获取闭包 (Bound Method)
                if matches!(self.ctx.type_registry.get(norm_expr), TypeKind::FnDef(..) | TypeKind::Function {..}) {
                    let field_name = self.ctx.resolve(*field).to_string();
                    unreachable!(
                        "Attempted to access method `{}` without calling it. Kern (C ABI) does not support Bound Methods / Closures. You must call it immediately using `()`.", 
                        field_name
                    );
                }

                // ==========================================
                // 🚀 核心修复：提前拦截方法访问 (Method Access)
                // 如果结果是一个函数类型，说明我们在访问一个方法！
                // ==========================================
                if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(norm_expr).clone() {
                    // ⚡ 静态分发 (Static Dispatch - 对应 b.&.get_val())
                    // 注意：这里绝不能 lower_expr(lhs)！
                    // 因为外层 Call 节点已经非常聪明地提前提取并 lower 了 Receiver。
                    // 直接返回泛型单态化后的函数指针即可。
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    return MastExpr {
                        ty: expr_ty,
                        span: expr.span,
                        kind: MastExprKind::FuncRef(mono_id),
                    };
                } else if let TypeKind::Function { .. } = self.ctx.type_registry.get(norm_expr) {
                    // 🌊 动态分发 (Dynamic Dispatch - 对应 r.read())
                    // 胖指针在内存中概念上是 { data: *void, vtable: *[usize; N] }
                    let mut base_ty = lhs_ty;
                    loop {
                        let norm = self.ctx.type_registry.normalize(base_ty);
                        match self.ctx.type_registry.get(norm) {
                            TypeKind::Mut(inner) | TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => base_ty = *inner,
                            _ => break,
                        }
                    }
                    let norm_base = self.ctx.type_registry.normalize(base_ty);
                    
                    if let TypeKind::TraitObject(trait_id, _) = self.ctx.type_registry.get(norm_base) {
                        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_id.0 as usize] { t } else { unreachable!() };
                        let vtable_idx = trait_def.methods.iter().position(|m| m.name == *field).expect("Method not found in trait");
                        
                        // 由于目前 ast.rs 中缺少从 FatPointer 中读取 meta(vtable) 的指令，
                        // 暂时用 Undef 占位并打印警告。稍后我们在 MastExprKind 补充对应指令！
                        println!("🚨 [MAST WARN] Dynamic VTable lookup triggered for `{}` at index {}. Requires IR expansion.", self.ctx.resolve(*field), vtable_idx);
                        
                        return MastExpr {
                            ty: expr_ty,
                            span: expr.span,
                            kind: MastExprKind::Undef, // FIXME: 等待补充 VTable 指令
                        };
                    }
                }

                // ==========================================
                // 原有逻辑：常规 Enum 和 Struct/Union 字段访问
                // ==========================================
                // 1. 提前克隆 EnumDef，立刻释放对 self.ctx.defs 的不可变借用
                let enum_def_opt = if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_lhs) {
                    if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                        Some(e.clone()) 
                    } else { None }
                } else { None };

                if let Some(enum_def) = enum_def_opt {
                    let mut current_val: i128 = 0;
                    let mut target_val: u128 = 0;
                    let mut found = false;
                    
                    for v in enum_def.variants { 
                        if let Some(v_expr) = v.value {
                            let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                            if let Ok(val) = ce.eval_math(&v_expr) {
                                current_val = val;
                            }
                        }
                        if v.name == *field {
                            target_val = current_val as u128;
                            found = true;
                            break;
                        }
                        current_val += 1;
                    }
                    
                    if !found {
                        let name_str = self.ctx.resolve(*field).to_string();
                        unreachable!("Enum variant `{}` not found in lowered Enum", name_str);
                    }
                    
                    MastExprKind::Integer(target_val) 
                } else {
                    // 3. 常规的 Struct/Union 字段访问
                    let l = self.lower_expr(lhs, subst_map, None);
                    
                    let mut base_ty = l.ty;
                    let mut deref_expr = l.clone();
                    
                    loop {
                        let norm = self.ctx.type_registry.normalize(base_ty);
                        match self.ctx.type_registry.get(norm) {
                            TypeKind::Mut(inner) => {
                                base_ty = *inner; 
                            }
                            TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) => {
                                base_ty = *inner;
                                deref_expr = MastExpr { ty: base_ty, span: l.span, kind: MastExprKind::Deref(Box::new(deref_expr)) };
                            }
                            _ => break,
                        }
                    }

                    let field_idx = self.get_field_index(base_ty, *field);
                    
                    let struct_def_info = if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(self.ctx.type_registry.normalize(base_ty)) {
                        Some((*def_id, gen_args.clone()))
                    } else { None };
                    
                    let struct_id = if let Some((def_id, gen_args)) = struct_def_info {
                        self.instantiate_struct(def_id, &gen_args)
                    } else { 
                        // 改写 Panic 信息，使其更容易 Debug
                        let err_field = self.ctx.resolve(*field).to_string();
                        unreachable!("Field access `{}` on non-struct type {:?}. Expected struct/union/enum.", err_field, base_ty); 
                    };
                    
                    MastExprKind::FieldAccess { lhs: Box::new(deref_expr), struct_id, field_idx }
                }
            }

            ExprKind::DataInit { type_node: _, literal } => {
                match literal {
                    ast::DataLiteralKind::Struct(fields) => {
                        let mut base_ty = self.ctx.type_registry.normalize(concrete_ty);
                        if let TypeKind::Mut(inner) = self.ctx.type_registry.get(base_ty) { base_ty = *inner; }

                        let (def_id, gen_args) = if let TypeKind::Def(id, args) = self.ctx.type_registry.get(base_ty) {
                            (*id, args.clone())
                        } else { unreachable!() };
                        
                        let mono_id = self.instantiate_struct(def_id, &gen_args);
                        
                        let def = self.ctx.defs[def_id.0 as usize].clone();
                        match def {
                            Def::Struct(s) => {
                                // 构建当前结构体的专属泛型映射
                                let mut struct_subst_map = std::collections::HashMap::new();
                                for (i, param) in s.generics.iter().enumerate() {
                                    struct_subst_map.insert(param.name, gen_args[i]);
                                }

                                let mut ordered_fields = Vec::new();
                                for f_def in &s.fields {
                                    // 🌟 修复：利用大括号 {} 限制 Substituter 的生命周期！
                                    let conc_f_ty = {
                                        let mut struct_subst = Substituter::new(&mut self.ctx.type_registry, &struct_subst_map);
                                        let raw_f_ty = self.ctx.node_types.get(&f_def.type_node.id).copied().unwrap_or(TypeId::ERROR);
                                        struct_subst.substitute(raw_f_ty)
                                    }; // ✨ struct_subst 在这里被销毁，归还了对 self 的可变借用！

                                    // 此时 self 已经完全自由了，可以放心调用 self.lower_expr
                                    if let Some(init_f) = fields.iter().find(|f| f.name == f_def.name) {
                                        ordered_fields.push(self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty)));
                                    } else {
                                        ordered_fields.push(self.lower_expr(f_def.default_value.as_ref().unwrap(), subst_map, Some(conc_f_ty)));
                                    }
                                }
                                MastExprKind::StructInit { struct_id: mono_id, fields: ordered_fields }
                            }
                            Def::Union(u) => {
                                // 🌟 同理，修改 Union 分支
                                let mut union_subst_map = std::collections::HashMap::new();
                                for (i, param) in u.generics.iter().enumerate() {
                                    union_subst_map.insert(param.name, gen_args[i]);
                                }

                                let init_f = &fields[0]; 
                                let field_idx = u.fields.iter().position(|f| f.name == init_f.name).unwrap();
                                let f_def = &u.fields[field_idx];
                                
                                // 🌟 缩短生命周期
                                let conc_f_ty = {
                                    let mut union_subst = Substituter::new(&mut self.ctx.type_registry, &union_subst_map);
                                    let raw_f_ty = self.ctx.node_types.get(&f_def.type_node.id).copied().unwrap_or(TypeId::ERROR);
                                    union_subst.substitute(raw_f_ty)
                                }; // ✨ union_subst 销毁

                                let val_expr = self.lower_expr(&init_f.value, subst_map, Some(conc_f_ty));
                                MastExprKind::UnionInit { union_id: mono_id, field_idx, value: Box::new(val_expr) }
                            }
                            _ => unreachable!()
                        }
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        let mut lowered_elems = Vec::new();
                        let elem_ty = if let TypeKind::Array { elem, .. } = self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)) {
                            Some(*elem)
                        } else { None };

                        for e in elems { lowered_elems.push(self.lower_expr(e, subst_map, elem_ty)); }
                        MastExprKind::ArrayInit(lowered_elems)
                    }
                    ast::DataLiteralKind::Repeat { value, count: _ } => {
                        let mut lowered_elems = Vec::new();
                        
                        // 🌟 顺手加固：提取 elem_ty，不要传 None，防止内部出现无法推导的情况
                        let elem_ty = if let TypeKind::Array { elem, .. } = self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)) {
                            Some(*elem)
                        } else { None };

                        let elem = self.lower_expr(value, subst_map, elem_ty);
                        let array_len = if let TypeKind::Array { len, .. } = self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)) { *len } else { 0 };

                        for _ in 0..array_len { lowered_elems.push(elem.clone()); }
                        MastExprKind::ArrayInit(lowered_elems)
                    }
                    ast::DataLiteralKind::Scalar(inner) => {
                        let inner_mast = self.lower_expr(inner, subst_map, Some(concrete_ty));
                        return inner_mast; 
                    }
                }
            }

            ExprKind::As { lhs, target } => {
                let target_ty = self.ctx.node_types.get(&target.id).copied().unwrap_or(concrete_ty);
                let l = self.lower_expr(lhs, subst_map, None);
                
                let mut target_norm = self.ctx.type_registry.normalize(target_ty);
                if let TypeKind::Mut(inner) = self.ctx.type_registry.get(target_norm) { target_norm = *inner; }
                
                if let TypeKind::TraitObject(def_id, _) = self.ctx.type_registry.get(target_norm) {
                    if let Def::Trait(_) = &self.ctx.defs[def_id.0 as usize] {
                        let vtable_id = self.get_or_create_vtable(l.ty, target_ty);
                        
                        // 🌟 核心修复：查找全局数组类型，并生成强转指针
                        let global_array_ty = self.module.globals.iter().find(|g| g.id == vtable_id).unwrap().ty;
                        let array_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer(global_array_ty));
                        
                        return MastExpr {
                            ty: target_ty, span: expr.span,
                            kind: MastExprKind::ConstructFatPointer {
                                data_ptr: Box::new(l),
                                meta: Box::new(MastExpr {
                                    ty: TypeId::USIZE, span: expr.span, 
                                    kind: MastExprKind::Cast {
                                        kind: MastCastKind::PtrToInt,
                                        operand: Box::new(MastExpr {
                                            ty: array_ptr_ty,
                                            span: expr.span, 
                                            // 🌟 关键补丁：利用 AddressOf 获取数组地址，阻止 GlobalRef 发生 Load 行为！
                                            kind: MastExprKind::AddressOf(Box::new(MastExpr {
                                                ty: global_array_ty,
                                                span: expr.span,
                                                kind: MastExprKind::GlobalRef(vtable_id)
                                            }))
                                        })
                                    }
                                }),
                            }
                        };
                    }
                }

                let cast_kind = self.determine_cast_kind(l.ty, target_ty);
                // 强制覆盖外层返回，确保 As 指令永远返回准确类型
                return MastExpr {
                    ty: target_ty, span: expr.span,
                    kind: MastExprKind::Cast { kind: cast_kind, operand: Box::new(l) }
                };
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                let c = self.lower_expr(cond, subst_map, Some(TypeId::BOOL));
                let t = self.lower_block_as_body(then_branch, subst_map, exp_ty);
                let e = else_branch.as_ref().map(|eb| self.lower_block_as_body(eb, subst_map, exp_ty));
                MastExprKind::If { cond: Box::new(c), then_branch: t, else_branch: e }
            }

            ExprKind::For { init, cond, post, body } => {
                let mut loop_stmts = Vec::new();
                
                if let Some(c) = cond {
                    let c_expr = self.lower_expr(c, subst_map, Some(TypeId::BOOL));
                    let not_c = MastExpr { ty: TypeId::BOOL, span: c.span, kind: MastExprKind::Unary { op: crate::ast::UnaryOperator::LogicalNot, operand: Box::new(c_expr) }};
                    
                    loop_stmts.push(MastStmt::Expr(MastExpr {
                        ty: TypeId::VOID, span: c.span, kind: MastExprKind::If {
                            cond: Box::new(not_c),
                            then_branch: MastBlock { stmts: vec![MastStmt::Expr(MastExpr { ty: TypeId::VOID, span: c.span, kind: MastExprKind::Break })], result: None },
                            else_branch: None,
                        }
                    }));
                }
                
                let b_expr = self.lower_expr(body, subst_map, None);
                loop_stmts.push(MastStmt::Expr(b_expr));
                
                if let Some(p) = post {
                    loop_stmts.push(MastStmt::Expr(self.lower_expr(p, subst_map, None)));
                }

                let loop_expr = MastExpr { 
                    ty: TypeId::VOID, span: expr.span, 
                    kind: MastExprKind::Loop(MastBlock { stmts: loop_stmts, result: None }) 
                };

                if let Some(i) = init {
                    let mut outer_stmts = Vec::new();
                    if let ExprKind::Let { name, init: let_init } = &i.kind {
                        let lowered_init = self.lower_expr(let_init, subst_map, None);
                        outer_stmts.push(MastStmt::Let { name: *name, ty: lowered_init.ty, init: lowered_init });
                    } else {
                        outer_stmts.push(MastStmt::Expr(self.lower_expr(i, subst_map, None)));
                    }
                    
                    outer_stmts.push(MastStmt::Expr(loop_expr));
                    MastExprKind::Block(MastBlock { stmts: outer_stmts, result: None })
                } else {
                    loop_expr.kind
                }
            }

            ExprKind::Return(val) => {
                let v = val.as_ref().map(|e| Box::new(self.lower_expr(e, subst_map, expected_ty)));
                let mut defer_stmts = Vec::new();
                for stack in self.defer_stack.iter().rev() {
                    for d in stack.iter().rev() {
                        defer_stmts.push(MastStmt::Expr(d.clone()));
                    }
                }
                
                if defer_stmts.is_empty() {
                    MastExprKind::Return(v)
                } else {
                    defer_stmts.push(MastStmt::Expr(MastExpr { ty: TypeId::VOID, span: expr.span, kind: MastExprKind::Return(v) }));
                    MastExprKind::Block(MastBlock { stmts: defer_stmts, result: None })
                }
            }

            ExprKind::Block { .. } => {
                MastExprKind::Block(self.lower_block_as_body(expr, subst_map, exp_ty))
            }

            ExprKind::Assign { lhs, op, rhs } => {
                let l = self.lower_expr(lhs, subst_map, None);
                let r = self.lower_expr(rhs, subst_map, Some(l.ty));
                MastExprKind::Assign { op: *op, lhs: Box::new(l), rhs: Box::new(r) }
            }

            ExprKind::IndexAccess { lhs, index } => {
                let l = self.lower_expr(lhs, subst_map, None);
                let idx = self.lower_expr(index, subst_map, Some(TypeId::USIZE));
                MastExprKind::IndexAccess { lhs: Box::new(l), index: Box::new(idx) }
            }

            ExprKind::Break => MastExprKind::Break,
            ExprKind::Continue => MastExprKind::Continue,
            ExprKind::Undef => MastExprKind::Undef,

            ExprKind::Switch { target, cases, default_case } => {
                let t = self.lower_expr(target, subst_map, None);
                let mut mast_cases = Vec::new();

                for case in cases {
                    let mut case_vals = Vec::new();
                    for pat in &case.patterns {
                        match pat {
                            ast::SwitchPattern::Value(val_expr) => {
                                let mut const_eval = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                                if let Ok(val) = const_eval.eval_math(val_expr) {
                                    case_vals.push(val as u128);
                                }
                            }
                            ast::SwitchPattern::Range { start, end, inclusive } => {
                                let s_val = {
                                    let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                                    ce.eval_math(start)
                                };
                                let e_val = {
                                    let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                                    ce.eval_math(end)
                                };
                                
                                if let (Ok(s), Ok(e)) = (s_val, e_val) {
                                    let end_bound = if *inclusive { e } else { e - 1 };
                                    for v in s..=end_bound {
                                        case_vals.push(v as u128);
                                    }
                                }
                            }
                        }
                    } 
                    let body = self.lower_block_as_body(&case.body, subst_map, exp_ty);
                    mast_cases.push(MastSwitchCase { values: case_vals, body });
                }

                let def_case = default_case.as_ref().map(|b| self.lower_block_as_body(b, subst_map, exp_ty));
                MastExprKind::Switch { target: Box::new(t), cases: mast_cases, default_case: def_case }
            }

            ExprKind::GenericInstantiation { .. } => {
                let fn_info = if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                    Some((*fn_id, fn_args.clone()))
                } else { None };

                if let Some((fn_id, fn_args)) = fn_info {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    MastExprKind::FuncRef(mono_id)
                } else {
                    MastExprKind::Integer(0) 
                }
            }
            ExprKind::EnumLiteral(variant_name) => {
                let norm_ty = self.ctx.type_registry.normalize(concrete_ty);
                
                // 1. 提前克隆 EnumDef，立刻释放对 self.ctx.defs 的不可变借用
                let enum_def_opt = if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm_ty) {
                    if let Def::Enum(e) = &self.ctx.defs[def_id.0 as usize] {
                        Some(e.clone()) // 🌟 核心操作：克隆一份，打断借用链！
                    } else { None }
                } else { None };

                // 2. 此时 self.ctx 已经完全自由
                if let Some(enum_def) = enum_def_opt {
                    let mut current_val: i128 = 0;
                    let mut target_val: u128 = 0;
                    let mut found = false;
                    
                    for v in enum_def.variants { 
                        if let Some(v_expr) = v.value {
                            let mut ce = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                            if let Ok(val) = ce.eval_math(&v_expr) {
                                current_val = val;
                            }
                        }
                        if v.name == *variant_name {
                            target_val = current_val as u128;
                            found = true;
                            break;
                        }
                        current_val += 1;
                    }
                    
                    if !found {
                        let name_str = self.ctx.resolve(*variant_name).to_string();
                        unreachable!("Enum variant `{}` not found in lowered Enum", name_str);
                    }
                    
                    MastExprKind::Integer(target_val)
                } else {
                    unreachable!("Lowering EnumLiteral on a non-enum type! Expected an Enum but got something else.");
                }
            }
            ExprKind::SelfValue => {
                MastExprKind::Var(self.ctx.intern("self"))
            }

            ExprKind::Null => {
                MastExprKind::Integer(0)
            }

            ExprKind::SliceOp { .. } => unreachable!("Should be handled or forbidden"),
            _ => unreachable!("Unhandled ExprKind in lowering: {:?}", expr.kind),
        };

        // ==========================================
        // 🌟 核心修复：隐式切片转换前，必须将双方的 Mut 彻底剥除！
        // ==========================================
        let mut conc_base = self.ctx.type_registry.normalize(concrete_ty);
        loop {
            if let TypeKind::Mut(inner) = self.ctx.type_registry.get(conc_base) { conc_base = *inner; } 
            else { break; }
        }

        let mut exp_base = self.ctx.type_registry.normalize(exp_ty);
        loop {
            if let TypeKind::Mut(inner) = self.ctx.type_registry.get(exp_base) { exp_base = *inner; } 
            else { break; }
        }

        if let TypeKind::Slice(_) = self.ctx.type_registry.get(exp_base) {
            if let TypeKind::Array { .. } = self.ctx.type_registry.get(conc_base) {
                mast_kind = MastExprKind::Cast {
                    kind: MastCastKind::ArrayToSlice,
                    operand: Box::new(MastExpr { ty: concrete_ty, span: expr.span, kind: mast_kind }),
                };
            }
        }

        MastExpr { ty: exp_ty, span: expr.span, kind: mast_kind }
    }

    // ==========================================
    //          Helpers
    // ==========================================

    fn get_field_index(&self, struct_ty: TypeId, field_name: SymbolId) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm) {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                return s.fields.iter().position(|f| f.name == field_name).unwrap();
            } else if let Def::Union(u) = &self.ctx.defs[def_id.0 as usize] {
                return u.fields.iter().position(|f| f.name == field_name).unwrap();
            }
        }
        0
    }

   fn determine_cast_kind(&self, from: TypeId, to: TypeId) -> MastCastKind {
        let f_norm = self.ctx.type_registry.normalize(from);
        let t_norm = self.ctx.type_registry.normalize(to);

        let f_int = self.ctx.type_registry.is_integer(f_norm);
        let t_int = self.ctx.type_registry.is_integer(t_norm);
        let f_ptr = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let t_ptr = matches!(self.ctx.type_registry.get(t_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let f_slice = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Slice(_));

        if f_ptr && t_ptr { return MastCastKind::Bitcast; }
        if f_int && t_ptr { return MastCastKind::IntToPtr; }
        if f_ptr && t_int { return MastCastKind::PtrToInt; }
        
        // 🌟 修复：精细化处理整数到整数的转换！
        if f_int && t_int {
            // 在 LLVM 里，整数之间的转换必须根据位宽来决定是用 Trunc 还是 Ext
            // 这里我们用一个极其粗略的 heuristic (因为目前你在 Codegen 里没有维护具体位宽)
            // 对于你的例子 `i64 as i32`，肯定是 Trunc
            let f_llvm_ty = self.ctx.type_registry.get(f_norm);
            let t_llvm_ty = self.ctx.type_registry.get(t_norm);
            
            let f_width = match f_llvm_ty {
                TypeKind::Primitive(PrimitiveType::I8) | TypeKind::Primitive(PrimitiveType::U8) => 8,
                TypeKind::Primitive(PrimitiveType::I16) | TypeKind::Primitive(PrimitiveType::U16) => 16,
                TypeKind::Primitive(PrimitiveType::I32) | TypeKind::Primitive(PrimitiveType::U32) => 32,
                TypeKind::Primitive(PrimitiveType::I64) | TypeKind::Primitive(PrimitiveType::U64) | TypeKind::Primitive(PrimitiveType::ISize) | TypeKind::Primitive(PrimitiveType::USize) => 64,
                TypeKind::Primitive(PrimitiveType::I128) | TypeKind::Primitive(PrimitiveType::U128) => 128,
                _ => 64,
            };
            
            let t_width = match t_llvm_ty {
                TypeKind::Primitive(PrimitiveType::I8) | TypeKind::Primitive(PrimitiveType::U8) => 8,
                TypeKind::Primitive(PrimitiveType::I16) | TypeKind::Primitive(PrimitiveType::U16) => 16,
                TypeKind::Primitive(PrimitiveType::I32) | TypeKind::Primitive(PrimitiveType::U32) => 32,
                TypeKind::Primitive(PrimitiveType::I64) | TypeKind::Primitive(PrimitiveType::U64) | TypeKind::Primitive(PrimitiveType::ISize) | TypeKind::Primitive(PrimitiveType::USize) => 64,
                TypeKind::Primitive(PrimitiveType::I128) | TypeKind::Primitive(PrimitiveType::U128) => 128,
                _ => 64,
            };

            if f_width > t_width {
                return MastCastKind::Trunc;
            } else if f_width < t_width {
                // 判断目标是否为有符号
                let is_signed = matches!(t_llvm_ty, TypeKind::Primitive(PrimitiveType::I8 | PrimitiveType::I16 | PrimitiveType::I32 | PrimitiveType::I64 | PrimitiveType::I128 | PrimitiveType::ISize));
                if is_signed {
                    return MastCastKind::SignExt;
                } else {
                    return MastCastKind::ZeroExt;
                }
            } else {
                return MastCastKind::Bitcast;
            }
        }
        
        if f_slice && t_ptr { return MastCastKind::SliceToPtr; }

        MastCastKind::Bitcast
    }

    fn get_or_create_vtable(&mut self, source_ty: TypeId, trait_ty: TypeId) -> MonoId {
        // 🌟 核心修复：安全剥离 TraitType 可能带有的 Mut 修饰符
        let mut actual_trait_ty = trait_ty;
        loop {
            let norm = self.ctx.type_registry.normalize(actual_trait_ty);
            if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm) {
                actual_trait_ty = *inner;
            } else {
                actual_trait_ty = norm;
                break;
            }
        }

        // 使用干净的 actual_trait_ty 作为缓存 Key
        let key = (source_ty, actual_trait_ty);
        if let Some(&id) = self.vtable_cache.get(&key) { return id; }

        let vtable_id = self.new_mono_id();
        self.vtable_cache.insert(key, vtable_id);

        // 现在匹配绝对安全！
        let trait_def_id = if let TypeKind::TraitObject(id, _) = self.ctx.type_registry.get(actual_trait_ty) {
            *id
        } else { 
            unreachable!("Target must be a TraitObject, found: {:?}", self.ctx.type_registry.get(actual_trait_ty)) 
        };

        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_def_id.0 as usize] {
            t.clone()
        } else { unreachable!() };

        let mut base_ty = source_ty;
        loop {
            match self.ctx.type_registry.get(base_ty) {
                TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) | TypeKind::Mut(inner) => {
                    base_ty = *inner;
                }
                _ => break,
            }
        }
        
        let source_args = if let TypeKind::Def(_, args) = self.ctx.type_registry.get(base_ty) {
            args.clone()
        } else { Vec::new() };

        let mut found_impl = None;
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                if let Some(impl_trait_node) = &impl_def.trait_type {
                    let i_trait_ty = self.ctx.node_types.get(&impl_trait_node.id).copied().unwrap_or(TypeId::ERROR);
                    
                    let mut i_trait_norm = i_trait_ty;
                    loop {
                        let n = self.ctx.type_registry.normalize(i_trait_norm);
                        if let TypeKind::Mut(inner) = self.ctx.type_registry.get(n) {
                            i_trait_norm = *inner;
                        } else {
                            i_trait_norm = n;
                            break;
                        }
                    }
                    
                    if let TypeKind::TraitObject(i_trait_id, _) = self.ctx.type_registry.get(i_trait_norm) {
                        if *i_trait_id == trait_def_id {
                            let i_target_ty = self.ctx.node_types.get(&impl_def.target_type.id).copied().unwrap_or(TypeId::ERROR);
                            
                            // 🌟 核心修复：对称剥离！同时剥离 Mut 和 Pointer，直达结构体 Def 本体
                            let mut i_target_base = i_target_ty;
                            loop {
                                let n = self.ctx.type_registry.normalize(i_target_base);
                                match self.ctx.type_registry.get(n) {
                                    TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) | TypeKind::Mut(inner) => {
                                        i_target_base = *inner;
                                    }
                                    _ => {
                                        i_target_base = n;
                                        break;
                                    }
                                }
                            }
                            
                            // 此时 i_target_base 是 Def(File)，base_ty 也是 Def(File)，完美匹配！
                            if let TypeKind::Def(i_target_id, _) = self.ctx.type_registry.get(i_target_base) {
                                if let TypeKind::Def(src_base_id, _) = self.ctx.type_registry.get(base_ty) {
                                    if *i_target_id == *src_base_id {
                                        found_impl = Some(impl_def.clone());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let impl_def = found_impl.expect("Impl block must exist for valid Trait Object cast. Sema missed this.");

        let void_ptr_ty = self.ctx.type_registry.intern(TypeKind::Pointer(TypeId::VOID));
        let mut vtable_methods = Vec::new();
        
        for trait_method in &trait_def.methods {
            let mut method_mono_id = None;
            for &m_id in &impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize] {
                    if f.name == trait_method.name {
                        method_mono_id = Some(self.instantiate_function(m_id, &source_args));
                        break;
                    }
                }
            }
            
            let m_id = method_mono_id.expect("Missing trait method implementation");
            vtable_methods.push(MastExpr {
                ty: void_ptr_ty, // ✅ 统一塞入指针类型
                span: crate::utils::Span::default(),
                kind: MastExprKind::FuncRef(m_id)
            });
        }

        let vtable_len = vtable_methods.len() as u64;
        let vtable_array_ty = self.ctx.type_registry.intern(TypeKind::Array { 
            elem: void_ptr_ty, // ✅ 告诉全局变量：我是一个由指针组成的数组
            len: vtable_len 
        });

        let vtable_init = MastExpr {
            ty: vtable_array_ty,
            span: crate::utils::Span::default(),
            kind: MastExprKind::ArrayInit(vtable_methods)
        };

        self.module.globals.push(MastGlobal {
            id: vtable_id,
            name: format!("__vtable_{}_{}", source_ty.0, actual_trait_ty.0),
            ty: vtable_array_ty, 
            is_mut: false,
            init: Some(vtable_init),
            is_extern: false,
        });

        vtable_id
    }
}
