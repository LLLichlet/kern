// src/mast/lower.rs
#![allow(unused)]
use std::collections::HashMap;
use crate::ast::{self, Expr, ExprKind};
use crate::context::Context;
use crate::sema::def::Def;
use crate::sema::ty::{TypeId, TypeKind};
use crate::sema::typeck::subst::Substituter;
use crate::utils::SymbolId;
use super::ast::*; // 引入 MAST 的全部定义

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

    // 维护降级时的局部变量类型栈 (解决 Typeck 丢弃局部作用域导致的“失忆”问题)
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
            next_mono_id: 1, // 0 保留作为 null 或无效 ID
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

    /// 降级入口：寻找所有非泛型的根节点 (Root Items) 并向下递归单态化
    pub fn lower_all(&mut self) -> MastModule {
        let def_ids: Vec<_> = (0..self.ctx.defs.len())
            .map(|i| crate::sema::ty::DefId(i as u32))
            .collect();

        // Phase 1: 预分配全局变量的 MonoId (解决互相引用和提前引用的问题)
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
                    if f.generics.is_empty() {
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

    /// 实例化或获取一个具体的函数
    fn instantiate_function(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) {
            return id;
        }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id); // 先插入缓存，防止递归死循环

        let def = if let Def::Function(f) = &self.ctx.defs[def_id.0 as usize] { f.clone() } else { unreachable!() };
        
        // 构造泛型替换映射表
        let mut subst_map = HashMap::new();
        for (i, param) in def.generics.iter().enumerate() {
            subst_map.insert(param.name, args[i]);
        }

        // 生成扁平化的名字 (Name Mangling)
        let mut mangled_name = self.ctx.resolve(def.name).to_string();
        for arg in args {
            mangled_name.push_str(&format!("_{}", arg.0));
        }

        // 处理参数和返回值
        let mut mast_params = Vec::new();
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        
        for p in &def.params {
            let raw_ty = self.ctx.node_types.get(&p.type_node.id).copied().unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_params.push(MastParam { name: p.name, ty: conc_ty });
        }
        
        // 1. 先安全地提取原始返回类型 (此时没有任何可变借用)
        let raw_ret = def.resolved_sig.map_or(TypeId::VOID, |sig| {
            if let TypeKind::Function { ret, .. } = self.ctx.type_registry.get(sig) { 
                *ret 
            } else { 
                TypeId::VOID 
            }
        });

        // 2. 创建替换器 (独占 type_registry 的可变借用)
        let mut subst = Substituter::new(&mut self.ctx.type_registry, &subst_map);
        
        // 3. 执行参数类型的代换
        let mut mast_params = Vec::new();
        for p in &def.params {
            // self.ctx.node_types 与 self.ctx.type_registry 是独立的字段，所以这里读取是安全的
            let raw_ty = self.ctx.node_types.get(&p.type_node.id).copied().unwrap_or(TypeId::ERROR);
            let conc_ty = subst.substitute(raw_ty);
            mast_params.push(MastParam { name: p.name, ty: conc_ty });
        }
        
        // 4. 执行返回类型的代换
        let conc_ret = subst.substitute(raw_ret);

        self.local_types.push(std::collections::HashMap::new());
        for p in &mast_params {
            self.local_types.last_mut().unwrap().insert(p.name, p.ty);
        }

        // 递归处理函数体
        let body = if let Some(body_expr) = &def.body {
            Some(self.lower_block_as_body(body_expr, &subst_map, conc_ret))
        } else { None };

        self.local_types.pop();
        
        let mast_fn = MastFunction {
            id,
            name: mangled_name,
            params: mast_params,
            ret_ty: conc_ret,
            body,
            is_extern: def.is_extern,
            is_variadic: def.is_variadic,
        };

        self.module.functions.push(mast_fn);
        id
    }

    /// 实例化或获取一个具体的结构体
    fn instantiate_struct(&mut self, def_id: crate::sema::ty::DefId, args: &[TypeId]) -> MonoId {
        let key = (def_id, args.to_vec());
        if let Some(&id) = self.mono_cache.get(&key) { return id; }

        let id = self.new_mono_id();
        self.mono_cache.insert(key, id);

        let def = if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] { s.clone() } else { unreachable!() };
        
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

    fn lower_global(&mut self, g: &crate::sema::def::GlobalDef) {
        // 从全局映射表中获取预先分配的 ID（马上在第 2 点里讲怎么预分配）
        let id = *self.global_map.get(&g.id).expect("Global MonoId should be pre-allocated");
        let ty = self.ctx.node_types.get(&g.type_node.as_ref().unwrap().id).copied().unwrap_or(TypeId::ERROR);
        let is_mut = matches!(
            self.ctx.type_registry.get(self.ctx.type_registry.normalize(ty)), 
            TypeKind::Mut(_)
        );

        let init = if !g.is_extern {
            Some(self.lower_expr(&g.value, &HashMap::new(), Some(ty)))
        } else { None };

        self.module.globals.push(MastGlobal {
            id,
            name: self.ctx.resolve(g.name).to_string(),
            ty,
            is_mut, 
            init,
            is_extern: g.is_extern,
        });
    }

    // ==========================================
    //          Block & Defer Unrolling
    // ==========================================

    fn lower_block_as_body(&mut self, block_expr: &Expr, subst_map: &HashMap<SymbolId, TypeId>, expected_ty: TypeId) -> MastBlock {
        self.defer_stack.push(Vec::new()); // 开启新的 defer 作用域
        self.local_types.push(HashMap::new()); // 开启新的局部类型作用域

        let mut stmts = Vec::new();
        let mut result = None;

        if let ExprKind::Block { stmts: ast_stmts, result: ast_res } = &block_expr.kind {
            for stmt in ast_stmts {
                match &stmt.kind {
                    ast::StmtKind::ExprStmt(e) | ast::StmtKind::ExprValue(e) => {
                        // 拦截 Defer，存入栈中不立刻生成代码
                        if let ExprKind::Defer { expr: def_expr } = &e.kind {
                            let lowered = self.lower_expr(def_expr, subst_map, None);
                            self.defer_stack.last_mut().unwrap().push(lowered);
                        } else {
                            // 🌟 拦截 Let，手动推导和注册类型
                            if let ExprKind::Let { name, type_node, init } = &e.kind {
                                let init_mast = self.lower_expr(init, subst_map, None);
                                
                                // 提取变量声明的真实类型 (如果是 None 则用初始值的类型)
                                let var_ty = if let Some(tn) = type_node {
                                    self.ctx.node_types.get(&tn.id).copied().unwrap_or(TypeId::ERROR)
                                } else {
                                    init_mast.ty
                                };
                                
                                // ✅ 核心操作：将变量名字和真实类型压入当前临时栈！
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
        }

        // 块自然结束，倒序展开并执行 Defer
        let defers = self.defer_stack.pop().unwrap();
        for d in defers.into_iter().rev() {
            stmts.push(MastStmt::Expr(d));
        }
        
        self.local_types.pop(); // 退出局部类型作用域

        MastBlock { stmts, result }
    }

    // ==========================================
    //          Expression Lowering
    // ==========================================

    fn lower_expr(&mut self, expr: &Expr, subst_map: &HashMap<SymbolId, TypeId>, expected_ty: Option<TypeId>) -> MastExpr {
        // 1. 推导并代换得到该表达式在当前上下文中的【绝对真实类型】
        let mut raw_ty = self.ctx.node_types.get(&expr.id).copied().unwrap_or(TypeId::ERROR);
        
        // 核心兜底：如果是 Identifier 且全局没查到，去我们的 local_types 栈里捞
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
        let exp_ty = expected_ty.unwrap_or(concrete_ty);

        // 2. 深度转换 AST -> MAST
        let mut mast_kind = match &expr.kind {
            ExprKind::Integer(val) => MastExprKind::Integer(*val),
            ExprKind::Float(val) => MastExprKind::Float(*val),
            ExprKind::Bool(val) => MastExprKind::Bool(*val),
            ExprKind::String(s) => MastExprKind::StringLiteral(s.clone()),

            ExprKind::Identifier(name) => {
                // 去全局/当前符号表查询
                if let Some(info) = self.ctx.scopes.resolve(*name).cloned() {
                    match info.kind {
                        crate::sema::scope::SymbolKind::Static | crate::sema::scope::SymbolKind::Const => {
                            let def_id = info.def_id.unwrap();
                            if let Some(&mono_id) = self.global_map.get(&def_id) {
                                MastExprKind::GlobalRef(mono_id)
                            } else {
                                unreachable!("Global should be pre-allocated")
                            }
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
                let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));
                return lower_init; // Let 返回初始化表达式供外层组装
            }

            ExprKind::Static { name, type_node: _, init } => {
                let global_id = self.new_mono_id();
                let lower_init = self.lower_expr(init, subst_map, Some(concrete_ty));
                
                // 动态提取真实的可变性
                let is_mut = matches!(
                    self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)), 
                    TypeKind::Mut(_)
                );

                self.module.globals.push(MastGlobal {
                    id: global_id,
                    name: format!("local_static_{}_{}", self.ctx.resolve(*name), global_id.0), // 加 ID 防重名
                    ty: concrete_ty,
                    is_mut, 
                    init: Some(lower_init),
                    is_extern: false,
                });
                MastExprKind::GlobalRef(global_id)
            }

            ExprKind::Binary { lhs, op, rhs } => {
                let l = self.lower_expr(lhs, subst_map, None);
                let r = self.lower_expr(rhs, subst_map, Some(l.ty));
                MastExprKind::Binary { op: *op, lhs: Box::new(l), rhs: Box::new(r) }
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
                let callee_mast = self.lower_expr(callee, subst_map, None);
                let mut arg_masts = Vec::new();
                for a in args { arg_masts.push(self.lower_expr(a, subst_map, None)); }
                
                // 【核心修复】：提前提取并克隆信息，迅速释放 type_registry 的借用
                let func_info = if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(callee_mast.ty) {
                    Some((*fn_id, fn_args.clone()))
                } else {
                    None
                };

                // 现在没有任何针对 self 的不可变借用了，可以安全调用 &mut self 方法
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
                let l = self.lower_expr(lhs, subst_map, None);
                
                // 自动解引用的透传处理
                let mut base_ty = l.ty;
                let mut deref_expr = l.clone();
                if let TypeKind::Pointer(inner) | TypeKind::VolatilePtr(inner) = self.ctx.type_registry.get(base_ty) {
                    base_ty = *inner;
                    deref_expr = MastExpr { ty: base_ty, span: l.span, kind: MastExprKind::Deref(Box::new(l)) };
                }

                let field_idx = self.get_field_index(base_ty, *field);
                let mut norm_base = self.ctx.type_registry.normalize(base_ty);
                if let TypeKind::Mut(inner) = self.ctx.type_registry.get(norm_base) {
                    norm_base = *inner;
                }

                let struct_def_info = if let TypeKind::Def(def_id, gen_args) = self.ctx.type_registry.get(norm_base) {
                    Some((*def_id, gen_args.clone()))
                } else {
                    None
                };
                
                let struct_id = if let Some((def_id, gen_args)) = struct_def_info {
                    self.instantiate_struct(def_id, &gen_args)
                } else {
                    unreachable!("Field access on non-struct type: {:?}", self.ctx.type_registry.get(norm_base))
                };
                
                MastExprKind::FieldAccess { lhs: Box::new(deref_expr), struct_id, field_idx }
            }

            ExprKind::DataLiteral(kind) => {
                match kind {
                    ast::DataLiteralKind::Struct(fields) => {
                        let mut base_ty = self.ctx.type_registry.normalize(concrete_ty);
                        if let TypeKind::Mut(inner) = self.ctx.type_registry.get(base_ty) {
                            base_ty = *inner;
                        }

                        // 1. 获取 Struct 的 MonoId
                        let (struct_def_id, gen_args) = if let TypeKind::Def(id, args) = self.ctx.type_registry.get(base_ty) {
                            (*id, args.clone())
                        } else { 
                            unreachable!("Expected Def for struct literal, but got {:?}", self.ctx.type_registry.get(base_ty)) 
                        };
                        
                        let mono_id = self.instantiate_struct(struct_def_id, &gen_args);
                        
                        // 将 fields 克隆出来，打断对 ctx.defs 的借用
                        let struct_fields = if let Def::Struct(s) = &self.ctx.defs[struct_def_id.0 as usize] {
                            s.fields.clone()
                        } else {
                            Vec::new()
                        };

                        // 2. 对齐字段：按结构体声明的物理顺序重排字段
                        let mut ordered_fields = Vec::new();
                        for f_def in &struct_fields {
                            // 查找用户提供的初始化值
                            if let Some(init_f) = fields.iter().find(|f| f.name == f_def.name) {
                                ordered_fields.push(self.lower_expr(&init_f.value, subst_map, None));
                            } else {
                                // 用户没提供，使用 default_value
                                ordered_fields.push(self.lower_expr(f_def.default_value.as_ref().unwrap(), subst_map, None));
                            }
                        }
                        
                        MastExprKind::StructInit { 
                            struct_id: mono_id, 
                            fields: ordered_fields 
                        }
                    }
                    _ => unimplemented!("Array/Repeat lower"),
                }
            }

            ExprKind::As { lhs, target: _ } => {
                let l = self.lower_expr(lhs, subst_map, None);
                
                // 拦截 Trait Object 构造
                let mut target_norm = self.ctx.type_registry.normalize(concrete_ty);
                if let TypeKind::Mut(inner) = self.ctx.type_registry.get(target_norm) {
                    target_norm = *inner; 
                }
                if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(target_norm) {
                    if let Def::Trait(_) = &self.ctx.defs[def_id.0 as usize] {
                        
                        // 这是一个向 Trait Object 的强转
                        // 需要获取或生成该类型对应的虚表 (VTable)
                        let vtable_id = self.get_or_create_vtable(l.ty, concrete_ty);
                        
                        return MastExpr {
                            ty: concrete_ty,
                            span: expr.span,
                            kind: MastExprKind::ConstructFatPointer {
                                data_ptr: Box::new(l),
                                vtable_ptr: vtable_id,
                            }
                        };
                    }
                }

                // 常规类型强转
                let cast_kind = self.determine_cast_kind(l.ty, concrete_ty);
                MastExprKind::Cast { kind: cast_kind, operand: Box::new(l) }
            }

            ExprKind::If { cond, then_branch, else_branch } => {
                let c = self.lower_expr(cond, subst_map, Some(TypeId::BOOL));
                let t = self.lower_block_as_body(then_branch, subst_map, exp_ty);
                let e = else_branch.as_ref().map(|eb| self.lower_block_as_body(eb, subst_map, exp_ty));
                MastExprKind::If { cond: Box::new(c), then_branch: t, else_branch: e }
            }

            ExprKind::For { init, cond, post, body } => {
                let mut loop_stmts = Vec::new();
                
                // 1. 条件判断: if (!cond) break;
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
                
                // 2. 循环体
                let b_expr = self.lower_expr(body, subst_map, None);
                loop_stmts.push(MastStmt::Expr(b_expr));
                
                // 3. 步进表达式 (post)
                if let Some(p) = post {
                    loop_stmts.push(MastStmt::Expr(self.lower_expr(p, subst_map, None)));
                }

                let loop_expr = MastExpr { 
                    ty: TypeId::VOID, span: expr.span, 
                    kind: MastExprKind::Loop(MastBlock { stmts: loop_stmts, result: None }) 
                };

                // 把 init 连同 loop 一起塞进一个外层 Block
                if let Some(i) = init {
                    let mut outer_stmts = Vec::new();
                    
                    // 如果 init 是 let 声明，在 MAST 中必须转为 Let 语句
                    if let ExprKind::Let { name, init: let_init, .. } = &i.kind {
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
                
                // 触发 Defer 爆炸：在遇到 return 时，将栈里所有的 defer 展开并作为一个大块执行
                let mut defer_stmts = Vec::new();
                for stack in self.defer_stack.iter().rev() {
                    for d in stack.iter().rev() {
                        defer_stmts.push(MastStmt::Expr(d.clone()));
                    }
                }
                
                if defer_stmts.is_empty() {
                    MastExprKind::Return(v)
                } else {
                    // 构造一个 Block：先执行所有的 defers，然后执行真正的 return 指令
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
                MastExprKind::Assign { 
                    op: *op, 
                    lhs: Box::new(l), 
                    rhs: Box::new(r) 
                }
            }

            ExprKind::DataLiteral(kind) => {
                match kind {
                    ast::DataLiteralKind::Struct(fields) => {
                        // ... (我们之前写好的 Struct 初始化逻辑保持不变) ...
                        // (为了排版简洁，这里省略上面写过的代码)
                        unreachable!("(Keep your existing struct logic here)")
                    }
                    ast::DataLiteralKind::Array(elems) => {
                        let mut lowered_elems = Vec::new();
                        // 尝试从期望类型中提取元素类型
                        let elem_ty = if let TypeKind::Array { elem, .. } = self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)) {
                            Some(*elem)
                        } else { 
                            None 
                        };

                        for e in elems {
                            lowered_elems.push(self.lower_expr(e, subst_map, elem_ty));
                        }
                        MastExprKind::ArrayInit(lowered_elems)
                    }
                    ast::DataLiteralKind::Repeat { value, count: _ } => {
                        // 语法如 [0; 100]。在 MAST 阶段，为了 Codegen 方便，我们将其彻底展开。
                        // (在真实工业编译器中，如果是非常大的数组，会调用 llvm.memset，这里做简易展开)
                        let mut lowered_elems = Vec::new();
                        let elem = self.lower_expr(value, subst_map, None);
                        
                        let array_len = if let TypeKind::Array { len, .. } = self.ctx.type_registry.get(self.ctx.type_registry.normalize(concrete_ty)) {
                            *len
                        } else { 0 };

                        for _ in 0..array_len {
                            lowered_elems.push(elem.clone());
                        }
                        MastExprKind::ArrayInit(lowered_elems)
                    }
                }
            }

            // ==========================================
            // 补充 3：数组/切片索引访问 (Index Access)
            // ==========================================
            ExprKind::IndexAccess { lhs, index } => {
                let l = self.lower_expr(lhs, subst_map, None);
                // 索引在 Kern 中固定要求是 usize (64位无符号整数)
                let idx = self.lower_expr(index, subst_map, Some(TypeId::USIZE));
                
                MastExprKind::IndexAccess { 
                    lhs: Box::new(l), 
                    index: Box::new(idx) 
                }
            }

            ExprKind::Break => MastExprKind::Break,
            ExprKind::Continue => MastExprKind::Continue,
            ExprKind::Switch { target, cases, default_case } => {
                let t = self.lower_expr(target, subst_map, None);
                let mut mast_cases = Vec::new();
                let mut const_eval = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);

                for case in cases {
                    let mut case_vals = Vec::new();
                    {
                        let mut const_eval = crate::sema::typeck::const_eval::ConstEvaluator::new(self.ctx);
                        for pat in &case.patterns {
                            match pat {
                                ast::SwitchPattern::Value(val_expr) => {
                                    if let Ok(val) = const_eval.eval_math(val_expr) {
                                        case_vals.push(val as u128);
                                    } else {
                                        const_eval.ctx.emit_error(val_expr.span, "Switch case value must be a compile-time constant integer".into());
                                    }
                                }
                                ast::SwitchPattern::Range { start, end, inclusive } => {
                                    let s_val = const_eval.eval_math(start);
                                    let e_val = const_eval.eval_math(end);
                                    
                                    if let (Ok(s), Ok(e)) = (s_val, e_val) {
                                        let end_bound = if *inclusive { e } else { e - 1 };
                                        if s > end_bound {
                                            const_eval.ctx.emit_error(start.span, "Invalid range: start is greater than end".into());
                                        } else {
                                            for v in s..=end_bound {
                                                case_vals.push(v as u128);
                                            }
                                        }
                                    } else {
                                        const_eval.ctx.emit_error(pat.span(), "Switch range bounds must be compile-time constants".into());
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

            ExprKind::GenericInstantiation { target: _, types: _ } => {
                // 1. 提取并克隆，立刻释放借用
                let fn_info = if let TypeKind::FnDef(fn_id, fn_args) = self.ctx.type_registry.get(concrete_ty) {
                    Some((*fn_id, fn_args.clone()))
                } else {
                    None
                };

                // 2. 安全地调用 &mut self
                if let Some((fn_id, fn_args)) = fn_info {
                    let mono_id = self.instantiate_function(fn_id, &fn_args);
                    MastExprKind::FuncRef(mono_id)
                } else {
                    // 如果走到这里，通常是 @sizeof[T] 泄露到了运行时代码中
                    MastExprKind::Integer(0) 
                }
            }

            ExprKind::Undef => MastExprKind::Undef,
            ExprKind::SliceOp { .. } => {
                unreachable!("Should have been handled by other lowering passes or forbidden")
            }
            // 如果遇到解析器里的奇怪东西直接 panic，说明我们的编译器漏写了支持
            _ => unreachable!("Unhandled ExprKind in lowering: {:?}", expr.kind),
        };

        // 【隐式转换显式化】 (Implicit Coercion Materialization)
        // 如果内部表达式的原始类型是 Array，但上下文期待 Slice，我们强制塞入一个 Cast
        if let TypeKind::Slice(_) = self.ctx.type_registry.get(exp_ty) {
            if let TypeKind::Array { .. } = self.ctx.type_registry.get(concrete_ty) {
                mast_kind = MastExprKind::Cast {
                    kind: MastCastKind::ArrayToSlice,
                    operand: Box::new(MastExpr { ty: concrete_ty, span: expr.span, kind: mast_kind }),
                };
            }
        }

        MastExpr {
            ty: exp_ty,
            span: expr.span,
            kind: mast_kind,
        }
    }

    // ==========================================
    //          Helpers
    // ==========================================

    fn get_field_index(&self, struct_ty: TypeId, field_name: SymbolId) -> usize {
        let norm = self.ctx.type_registry.normalize(struct_ty);
        if let TypeKind::Def(def_id, _) = self.ctx.type_registry.get(norm) {
            if let Def::Struct(s) = &self.ctx.defs[def_id.0 as usize] {
                return s.fields.iter().position(|f| f.name == field_name).unwrap();
            }
        }
        0
    }

    /// 确定类型转换的具体指令。
    /// Kern 的 `as` 极其严格，仅处理内存模型不变的转换 (如 ptr -> ptr, ptr <-> int, 以及同尺寸的 int <-> int)。
    /// 截断 (Truncate) 或扩展 (Extend) 必须通过显式的内置函数如 `@truncate` 完成。
    fn determine_cast_kind(&self, from: TypeId, to: TypeId) -> MastCastKind {
        let f_norm = self.ctx.type_registry.normalize(from);
        let t_norm = self.ctx.type_registry.normalize(to);

        let f_int = self.ctx.type_registry.is_integer(f_norm);
        let t_int = self.ctx.type_registry.is_integer(t_norm);
        let f_ptr = matches!(self.ctx.type_registry.get(f_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));
        let t_ptr = matches!(self.ctx.type_registry.get(t_norm), TypeKind::Pointer(_) | TypeKind::VolatilePtr(_));

        if f_ptr && t_ptr { 
            return MastCastKind::Bitcast; 
        }
        if f_int && t_ptr { 
            return MastCastKind::IntToPtr; 
        }
        if f_ptr && t_int { 
            return MastCastKind::PtrToInt; 
        }
        if f_int && t_int { 
            // 在 Kern 的 `as` 哲学下，整型之间的转换默认都是 Bitcast (前提是它们大小相同，Sema 阶段已拦截大小不同的 as)。
            return MastCastKind::Bitcast; 
        }

        // 如果走到了这里，说明是 Sema 阶段允许的其他无损强转 (例如相同布局的包装体/NewType)
        MastCastKind::Bitcast
    }

    /// 获取或生成特定类型到特定 Trait 的虚表 (VTable)
    /// 返回的是一个指向静态 VTable 数组的全局变量 MonoId
    fn get_or_create_vtable(&mut self, source_ty: TypeId, trait_ty: TypeId) -> MonoId {
        let key = (source_ty, trait_ty);
        if let Some(&id) = self.vtable_cache.get(&key) {
            return id;
        }

        let vtable_id = self.new_mono_id();
        self.vtable_cache.insert(key, vtable_id);

        // 1. 获取 Trait 定义以确立 VTable 布局 (方法顺序极其重要)
        let trait_norm = self.ctx.type_registry.normalize(trait_ty);
        let trait_def_id = if let TypeKind::Def(id, _) = self.ctx.type_registry.get(trait_norm) {
            *id
        } else { unreachable!("Target of trait object cast must be a Trait") };

        let trait_def = if let Def::Trait(t) = &self.ctx.defs[trait_def_id.0 as usize] {
            t.clone()
        } else { unreachable!() };

        // 2. 剥离指针/Mut壳，找到 Base Type 的 DefId 和 泛型参数
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

        // 3. 寻找匹配的 Impl 块 (Sema 阶段已验证其存在，这里直接捞取)
        let mut found_impl = None;
        for def in &self.ctx.defs {
            if let Def::Impl(impl_def) = def {
                if let Some(impl_trait_node) = &impl_def.trait_type {
                    let i_trait_ty = self.ctx.node_types.get(&impl_trait_node.id).copied().unwrap_or(TypeId::ERROR);
                    let i_trait_norm = self.ctx.type_registry.normalize(i_trait_ty);
                    
                    if let TypeKind::Def(i_trait_id, _) = self.ctx.type_registry.get(i_trait_norm) {
                        // 简易匹配：如果 Trait ID 一致，且这是我们要的 Base Type (严格来说需要用 unify 匹配目标)
                        // Kern 规范中 Impl 类型匹配非常严格，这里可以直接采信
                        if *i_trait_id == trait_def_id {
                            let i_target_ty = self.ctx.node_types.get(&impl_def.target_type.id).copied().unwrap_or(TypeId::ERROR);
                            let i_target_norm = self.ctx.type_registry.normalize(i_target_ty);
                            if let TypeKind::Def(i_target_id, _) = self.ctx.type_registry.get(i_target_norm) {
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

        // 4. 收集并单态化该 Impl 块中的所有方法，顺序必须与 Trait 声明严格一致！
        let mut vtable_methods = Vec::new();
        for trait_method in &trait_def.methods {
            let mut method_mono_id = None;
            for &m_id in &impl_def.methods {
                if let Def::Function(f) = &self.ctx.defs[m_id.0 as usize] {
                    if f.name == trait_method.name {
                        // 🌟 核心：用宿主对象真实的泛型参数 (source_args) 实例化这个方法！
                        method_mono_id = Some(self.instantiate_function(m_id, &source_args));
                        break;
                    }
                }
            }
            
            let m_id = method_mono_id.expect("Missing trait method implementation");
            
            // 将函数指针包装成 MastExpr
            vtable_methods.push(MastExpr {
                ty: TypeId::USIZE, // 在 LLVM 中，函数指针通常处理为地址
                span: crate::utils::Span::default(),
                kind: MastExprKind::FuncRef(m_id)
            });
        }

        // 5. 将 VTable 生成为一个全局常量数组
        let vtable_init = MastExpr {
            ty: TypeId::ERROR, // 理想情况下注册一个 `[N]*void` 类型，这里用 ERROR 占位，Codegen 时当作纯数组处理即可
            span: crate::utils::Span::default(),
            kind: MastExprKind::ArrayInit(vtable_methods)
        };

        self.module.globals.push(MastGlobal {
            id: vtable_id,
            name: format!("__vtable_{}_{}", source_ty.0, trait_ty.0), // 防止命名冲突的内部变量
            ty: TypeId::ERROR, 
            is_mut: false,     // VTable 是绝对不可变的！
            init: Some(vtable_init),
            is_extern: false,
        });

        vtable_id
    }
}