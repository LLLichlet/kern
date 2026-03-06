#![allow(unused)]
// src/sema/resolve_types.rs

use crate::ast;
use crate::context::Context;
use crate::sema::def::{Def, ImplDef};
use crate::sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::sema::ty::{DefId, PrimitiveType, TypeId, TypeKind};
use crate::utils::SymbolId;

pub struct TypeResolver<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> TypeResolver<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 执行完整的类型解析 Pass
    pub fn resolve_all(&mut self) {
        // 核心纠正：我们必须基于模块层级进行遍历，以获取正确的词法作用域上下文
        let module_ids: Vec<DefId> = self.ctx.defs.iter().filter_map(|def| {
            if let Def::Module(m) = def { Some(m.id) } else { None }
        }).collect();

        for mod_id in module_ids {
            self.resolve_module(mod_id);
        }
    }

    fn resolve_module(&mut self, mod_id: DefId) {
        let (mod_scope, items) = {
            if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
                (m.scope_id, m.items.clone())
            } else {
                unreachable!()
            }
        };

        for item_id in items {
            self.resolve_item(item_id, mod_scope);
        }
    }

    /// 解析具体的项，为其开辟作用域并绑定泛型
    fn resolve_item(&mut self, item_id: DefId, parent_scope: ScopeId) {
        let def = self.ctx.defs[item_id.0 as usize].clone();

        match &def {
            Def::Function(f) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let func_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&f.generics, func_scope);

                for param in &f.params {
                    self.resolve_type(&param.type_node, func_scope);
                }
                self.resolve_type(&f.ret_type, func_scope);

                self.ctx.scopes.exit_scope();
            }
            Def::Struct(s) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let struct_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&s.generics, struct_scope);

                for field in &s.fields {
                    self.resolve_type(&field.type_node, struct_scope);
                }
                self.ctx.scopes.exit_scope();
            }
            Def::Union(u) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let union_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&u.generics, union_scope);

                for field in &u.fields {
                    self.resolve_type(&field.type_node, union_scope);
                }
                self.ctx.scopes.exit_scope();
            }
            Def::Enum(e) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let enum_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&e.generics, enum_scope);

                if let Some(backing_ty) = &e.backing_type {
                    let resolved_ty = self.resolve_type(backing_ty, enum_scope);
                    // 严格检查 backing type 必须是整数类型
                    if !self.ctx.type_registry.is_integer(resolved_ty) && resolved_ty != TypeId::ERROR {
                        self.ctx.emit_error(backing_ty.span, "Enum backing type must be an integer".into());
                    }
                }
                self.ctx.scopes.exit_scope();
            }
            Def::Trait(t) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let trait_scope = self.ctx.scopes.enter_scope();

                // Kern 的特征内是函数签名 (这里你在 AST 中复用了 StructFieldDef 作为方法)
                for method in &t.methods {
                    self.resolve_type(&method.type_node, trait_scope);
                }
                self.ctx.scopes.exit_scope();
            }
            Def::TypeAlias(t) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let alias_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&t.generics, alias_scope);
                let target_ty = self.resolve_type(&t.target, alias_scope);
                
                // 写回符号表
                if let Some(mut info) = self.ctx.scopes.resolve_local(t.name).cloned() {
                    info.type_id = target_ty;
                    // Note: 在真实的 Scope 实现中需要一个 update() 方法。
                    // 暂时略过，因为 alias 逻辑通常可以直接查询 target_ty
                }
                self.ctx.scopes.exit_scope();
            }
            Def::Impl(i) => {
                self.ctx.scopes.set_current_scope(parent_scope);
                let impl_scope = self.ctx.scopes.enter_scope();
                
                self.bind_generics(&i.generics, impl_scope);

                // 绑定 Self 类型到上下文中
                let target_ty_id = self.resolve_type(&i.target_type, impl_scope);
                self.bind_self_type(target_ty_id, impl_scope);

                if let Some(trait_ty) = &i.trait_type {
                    self.resolve_type(trait_ty, impl_scope);
                }

                // 递归解析 impl 块内部的方法 (这些方法并没有注册在 module 的 items 里)
                for &method_id in &i.methods {
                    self.resolve_item(method_id, impl_scope);
                }

                self.ctx.scopes.exit_scope();
            }
            Def::Global(g) => {
                if let Some(ty_node) = &g.type_node {
                    self.resolve_type(ty_node, parent_scope);
                }
            }
            _ => {} // Module 自身无需在此解析
        }
    }

    // ==========================================
    //          核心类型转换逻辑
    // ==========================================

    /// 将 AST TypeNode 转换为语义 TypeId
    fn resolve_type(&mut self, ty_node: &ast::TypeNode, env_scope: ScopeId) -> TypeId {
        let ty_id = match &ty_node.kind {
            ast::TypeKind::Path { segments, generics } => {
                self.resolve_path_type(segments, generics, env_scope, ty_node.span)
            }
            ast::TypeKind::Pointer { elem, is_mut } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer { base, is_mut: *is_mut, is_volatile: false })
            }
            ast::TypeKind::VolatilePtr { elem, is_mut } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Pointer { base, is_mut: *is_mut, is_volatile: true })
            }
            ast::TypeKind::Slice { elem, is_mut } => {
                let base = self.resolve_type(elem, env_scope);
                self.ctx.type_registry.intern(TypeKind::Slice { elem: base, is_mut: *is_mut })
            }
            ast::TypeKind::Array { elem, len, is_mut: _ } => {
                let base = self.resolve_type(elem, env_scope);
                let length = self.evaluate_const_usize(len); 
                self.ctx.type_registry.intern(TypeKind::Array { elem: base, len: length })
            }
            ast::TypeKind::Function { params, ret } => {
                let mut param_tys = Vec::with_capacity(params.len());
                for p in params {
                    param_tys.push(self.resolve_type(p, env_scope));
                }
                let ret_ty = match ret {
                    Some(r) => self.resolve_type(r, env_scope),
                    None => TypeId::VOID,
                };
                self.ctx.type_registry.intern(TypeKind::Function { params: param_tys, ret: ret_ty })
            }
            ast::TypeKind::SelfType => {
                // 查找环境中绑定的 `Self` 符号
                self.ctx.scopes.set_current_scope(env_scope);
                // 假设我们在 bind_self_type 时使用了名为 `Self` 的符号
                let self_sym = self.ctx.intern("Self");
                if let Some(info) = self.ctx.scopes.resolve(self_sym) {
                    info.type_id
                } else {
                    self.ctx.emit_error(ty_node.span, "`Self` is only valid inside an `impl` block".into());
                    TypeId::ERROR
                }
            }
            ast::TypeKind::Infer => {
                self.ctx.emit_error(ty_node.span, "Type inference `_` is not allowed in this context".into());
                TypeId::ERROR
            }
            // Struct/Enum/Union/Trait 在这里不会直接作为匿名类型出现 (已被 Collect 提取)
            _ => {
                self.ctx.emit_error(ty_node.span, "Invalid or unsupported type construction".into());
                TypeId::ERROR
            }
        };

        self.ctx.node_types.insert(ty_node.id, ty_id);
        ty_id
    }

    /// 严格的路径类型解析 (支持 `module.submodule.Type[Generic]`)
    fn resolve_path_type(
        &mut self, 
        segments: &[SymbolId], 
        generics: &[ast::TypeNode], 
        env_scope: ScopeId, 
        span: crate::utils::Span
    ) -> TypeId {
        if segments.is_empty() { return TypeId::ERROR; }

        let mut curr_scope = env_scope;
        let mut target_symbol = None;

        // 逐级解析路径
        for (i, &segment) in segments.iter().enumerate() {
            if i == 0 {
                // 第一段：如果只有一段，优先检查内置基础类型
                if segments.len() == 1 {
                    let name_str = self.ctx.resolve(segment);
                    if let Some(prim_id) = self.resolve_builtin_primitive(name_str) {
                        if !generics.is_empty() {
                            self.ctx.emit_error(span, "Primitive types do not take generic arguments".into());
                        }
                        return prim_id;
                    }
                }

                // 沿着作用域树向上查找
                self.ctx.scopes.set_current_scope(curr_scope);
                target_symbol = self.ctx.scopes.resolve(segment).cloned();
            } else {
                // 后续段：严格只在前一个模块的内部作用域中查找
                target_symbol = self.ctx.scopes.resolve_in(curr_scope, segment).cloned();
            }

            let sym = match target_symbol.as_ref() {
                Some(s) => s,
                None => {
                    let name = self.ctx.resolve(segment).to_string();
                    if i == 0 {
                        self.ctx.emit_error(span, format!("Cannot find type `{}` in this scope", name));
                    } else {
                        self.ctx.emit_error(span, format!("Cannot find `{}` in the target module", name));
                    }
                    return TypeId::ERROR;
                }
            };

            // 如果还没到最后一段，当前符号必须是个模块
            if i < segments.len() - 1 {
                if sym.kind == SymbolKind::Module {
                    let mod_def_id = sym.def_id.unwrap();
                    if let Def::Module(m) = &self.ctx.defs[mod_def_id.0 as usize] {
                        curr_scope = m.scope_id;
                    } else { unreachable!() }
                } else {
                    let name = self.ctx.resolve(segment).to_string();
                    self.ctx.emit_error(span, format!("`{}` is not a module", name));
                    return TypeId::ERROR;
                }
            }
        }

        let final_sym = target_symbol.unwrap();

        // 解析附带的泛型参数 (在原始的作用域中解析)
        let mut resolved_generics = Vec::with_capacity(generics.len());
        for gen_ast in generics {
            resolved_generics.push(self.resolve_type(gen_ast, env_scope));
        }

        // 验证最终符号的类型
        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union | SymbolKind::Enum | SymbolKind::Trait => {
                let def_id = final_sym.def_id.unwrap();
                // 返回 Def 类型，并将实例化的泛型参数附带上
                self.ctx.type_registry.intern(TypeKind::Def(def_id, resolved_generics))
            }
            SymbolKind::TypeAlias => {
                // 注意：这里可能需要检查别名是否带有泛型并进行替换，这涉及代换机制 (Substitution)
                // Kern 作为一个重系统的语言，这里目前穿透返回。
                final_sym.type_id
            }
            _ => {
                let name = self.ctx.resolve(*segments.last().unwrap()).to_string();
                self.ctx.emit_error(span, format!("`{}` is a {}, not a type", name, self.kind_to_string(final_sym.kind)));
                TypeId::ERROR
            }
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn resolve_builtin_primitive(&self, name: &str) -> Option<TypeId> {
        match name {
            "void" => Some(TypeId::VOID), "bool" => Some(TypeId::BOOL),
            "i8" => Some(TypeId::I8), "i16" => Some(TypeId::I16), "i32" => Some(TypeId::I32), 
            "i64" => Some(TypeId::I64), "i128" => Some(TypeId::I128), "isize" => Some(TypeId::ISIZE),
            "u8" => Some(TypeId::U8), "u16" => Some(TypeId::U16), "u32" => Some(TypeId::U32), 
            "u64" => Some(TypeId::U64), "u128" => Some(TypeId::U128), "usize" => Some(TypeId::USIZE),
            "f32" => Some(TypeId::F32), "f64" => Some(TypeId::F64),
            _ => None,
        }
    }

    fn bind_generics(&mut self, generics: &[ast::GenericParam], scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);
        for param in generics {
            let param_ty = self.ctx.type_registry.intern(TypeKind::Param(param.name));
            let info = SymbolInfo {
                kind: SymbolKind::TypeAlias, 
                node_id: ast::NodeId(0), // 伪造 ID，或者传入 param.span 对应的真实 ID
                type_id: param_ty,
                def_id: None,
                is_mutable: false,
            };
            let _ = self.ctx.scopes.define(param.name, info);
        }
    }

    fn bind_self_type(&mut self, target_ty: TypeId, scope: ScopeId) {
        self.ctx.scopes.set_current_scope(scope);
        let self_sym = self.ctx.intern("Self");
        let info = SymbolInfo {
            kind: SymbolKind::TypeAlias,
            node_id: ast::NodeId(0),
            type_id: target_ty,
            def_id: None,
            is_mutable: false,
        };
        // 允许重复定义（覆盖外部可能存在的同名绑定）
        let _ = self.ctx.scopes.define(self_sym, info);
    }

    fn evaluate_const_usize(&mut self, expr: &ast::Expr) -> u64 {
        // 当前为简易处理。真正的常量折叠(Constant Folding)是一个庞大的主题。
        if let ast::ExprKind::Integer(val) = &expr.kind {
            *val as u64
        } else {
            self.ctx.emit_error(expr.span, "Array length must be an integer constant expression".into());
            0
        }
    }

    fn kind_to_string(&self, kind: SymbolKind) -> &'static str {
        match kind {
            SymbolKind::Var => "variable", SymbolKind::Const => "constant",
            SymbolKind::Static => "static variable", SymbolKind::Function => "function",
            SymbolKind::Module => "module", _ => "symbol",
        }
    }
}