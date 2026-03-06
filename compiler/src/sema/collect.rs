#![allow(unused)]

use crate::ast::{self, Decl, DeclKind, TypeKind};
use crate::context::Context;
use crate::sema::def::*;
use crate::sema::scope::{SymbolInfo, SymbolKind};
use crate::sema::ty::{DefId, TypeId};
use crate::utils::SymbolId;

pub struct Collector<'a> {
    pub ctx: &'a mut Context,
    pub current_module: Option<DefId>,
}

impl<'a> Collector<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self {
            ctx,
            current_module: None,
        }
    }

    /// 收集整个模块
    pub fn collect_module(&mut self, module: &ast::Module) -> DefId {
        let mod_name = self.ctx.intern(&module.path); 
        let mod_id = DefId(self.ctx.defs.len() as u32);
        
        let parent_module = self.current_module;
        
        // 【关键改动 1】开启模块的专属作用域，并获取其持久化的 ScopeId
        let scope_id = self.ctx.scopes.enter_scope();
        
        // 预先注册 ModuleDef，并绑定 scope_id
        self.ctx.add_def(Def::Module(ModuleDef {
            id: mod_id,
            name: mod_name,
            parent: parent_module, 
            scope_id, // <--- 绑定专属作用域
            items: Vec::new(),
            imports: Vec::new(),
        }));

        self.current_module = Some(mod_id);
        
        let mut item_ids = Vec::new();
        let mut imports = Vec::new();

        // 遍历并收集模块内的所有顶层声明
        for decl in &module.decls {
            if let DeclKind::Use { kind, path, target, is_reexport } = &decl.kind {
                imports.push(ImportDef {
                    path_kind: *kind,
                    path: path.clone(),
                    target: target.clone(),
                    is_reexport: *is_reexport,
                    span: decl.span,
                });
            } else if let Some(def_id) = self.collect_decl(decl, None, false) {
                item_ids.push(def_id);
            }
        }

        // 收集完毕，将内部成员和导入列表更新回 ModuleDef
        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.items = item_ids;
            m.imports = imports;
        }

        // 【关键改动 2】退出当前作用域，恢复上下文
        self.ctx.scopes.exit_scope();
        self.current_module = parent_module;

        mod_id
    }

    /// 收集单个声明
    /// `parent_impl`: 如果当前声明位于 impl 块内，传入 impl 的 DefId
    /// `force_extern`: 如果当前声明位于 extern 块内，强制标记为 extern
    fn collect_decl(&mut self, decl: &Decl, parent_impl: Option<DefId>, force_extern: bool) -> Option<DefId> {
        let vis = decl.is_pub.into();

        match &decl.kind {
            DeclKind::Function { generics, params, ret_type, body, is_extern, is_variadic } => {
                self.collect_function(
                    decl, vis, parent_impl, force_extern || *is_extern, 
                    generics, params, ret_type, body, *is_variadic
                )
            }
            DeclKind::Var { type_node, value, is_mut, is_static, is_extern } => {
                self.collect_global(decl, vis, force_extern || *is_extern, type_node, value, *is_mut, *is_static)
            }
            DeclKind::TypeAlias { generics, target, is_extern } => {
                self.collect_type_alias_or_struct(decl, vis, force_extern || *is_extern, generics, target)
            }
            DeclKind::Impl { generics, target_type, trait_type, decls } => {
                self.collect_impl(decl, generics, target_type, trait_type, decls)
            }
            DeclKind::ExternBlock { abi: _, decls } => {
                // 进入 extern 块，强制块内声明的 is_extern 为 true
                for ext_decl in decls {
                    self.collect_decl(ext_decl, parent_impl, true);
                }
                None // Extern 块本身不产生单独的 Def，只影响其内部元素
            }
            DeclKind::Use { .. } => {
                // TODO: 导入解析将在 Resolve 阶段或者这里进一步处理
                // 暂时略过
                None 
            }
            DeclKind::Macro { .. } => {
                // 初期不支持宏
                None
            }
        }
    }

    fn collect_function(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        parent_impl: Option<DefId>,
        is_extern: bool,
        generics: &[ast::GenericParam],
        params: &[ast::FuncParam],
        ret_type: &ast::TypeNode,
        body: &Option<Box<ast::Expr>>,
        is_variadic: bool,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);
        
        let func_def = FunctionDef {
            id: def_id,
            name: decl.name,
            vis,
            parent: parent_impl.or(self.current_module),
            generics: generics.to_vec(),
            params: params.to_vec(),
            ret_type: ret_type.clone(),
            body: body.clone(),
            is_extern,
            is_variadic,
            span: decl.span,
        };

        self.ctx.add_def(Def::Function(func_def));

        // 如果不是 impl 块中的方法，则将其注册到当前词法作用域
        if parent_impl.is_none() {
            self.define_symbol(decl.name, SymbolKind::Function, decl.id, Some(def_id), false, decl.span);
        }

        Some(def_id)
    }

    fn collect_global(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        is_extern: bool,
        type_node: &Option<ast::TypeNode>,
        value: &ast::Expr,
        is_mut: bool,
        is_static: bool,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);

        let global_def = GlobalDef {
            id: def_id,
            name: decl.name,
            vis,
            type_node: type_node.clone(),
            value: value.clone(),
            is_mut,
            is_static,
            is_extern,
            span: decl.span,
        };

        self.ctx.add_def(Def::Global(global_def));

        let sym_kind = if is_static { SymbolKind::Static } else { SymbolKind::Const };
        self.define_symbol(decl.name, sym_kind, decl.id, Some(def_id), is_mut, decl.span);

        Some(def_id)
    }

    /// 核心逻辑：将 `type Name = Target` 解包为对应的实体定义
    fn collect_type_alias_or_struct(
        &mut self,
        decl: &Decl,
        vis: Visibility,
        is_extern: bool,
        generics: &[ast::GenericParam],
        target: &ast::TypeNode,
    ) -> Option<DefId> {
        let def_id = DefId(self.ctx.defs.len() as u32);
        let mut sym_kind = SymbolKind::TypeAlias;

        let def = match &target.kind {
            TypeKind::Struct { fields } => {
                sym_kind = SymbolKind::Struct;
                Def::Struct(StructDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    fields: fields.clone(),
                    is_extern,
                    span: decl.span,
                })
            }
            TypeKind::Union { fields } => {
                sym_kind = SymbolKind::Union;
                Def::Union(UnionDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    fields: fields.clone(),
                    span: decl.span,
                })
            }
            TypeKind::Enum { backing_type, variants } => {
                sym_kind = SymbolKind::Enum;
                Def::Enum(EnumDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    backing_type: backing_type.clone(),
                    variants: variants.clone(),
                    span: decl.span,
                })
            }
            TypeKind::Trait { fields } => {
                sym_kind = SymbolKind::Trait;
                Def::Trait(TraitDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    methods: fields.clone(),
                    span: decl.span,
                })
            }
            _ => {
                // 真正的类型别名，例如 `type MyInt = i32;`
                Def::TypeAlias(TypeAliasDef {
                    id: def_id,
                    name: decl.name,
                    vis,
                    generics: generics.to_vec(),
                    target: target.clone(),
                    span: decl.span,
                })
            }
        };

        self.ctx.add_def(def);
        self.define_symbol(decl.name, sym_kind, decl.id, Some(def_id), false, decl.span);

        Some(def_id)
    }

    fn collect_impl(
        &mut self,
        decl: &Decl,
        generics: &[ast::GenericParam],
        target_type: &ast::TypeNode,
        trait_type: &Option<ast::TypeNode>,
        decls: &[Decl],
    ) -> Option<DefId> {
        let impl_id = DefId(self.ctx.defs.len() as u32);
        let mut method_ids = Vec::new();

        // Impl 块会引入新的作用域（为了泛型参数）
        self.ctx.scopes.enter_scope();

        for method_decl in decls {
            // Impl 块内只允许存在 Function
            if matches!(method_decl.kind, DeclKind::Function { .. }) {
                if let Some(m_id) = self.collect_decl(method_decl, Some(impl_id), false) {
                    method_ids.push(m_id);
                }
            } else {
                self.ctx.emit_error(method_decl.span, "Only functions are allowed inside `impl` blocks".into());
            }
        }

        self.ctx.scopes.exit_scope();

        let impl_def = ImplDef {
            id: impl_id,
            parent_module: self.current_module,
            generics: generics.to_vec(),
            target_type: target_type.clone(),
            trait_type: trait_type.clone(),
            methods: method_ids,
            span: decl.span,
        };

        self.ctx.add_def(Def::Impl(impl_def));

        // Impl 块没有显式的名字，不注册到当前命名空间
        Some(impl_id)
    }

    // ==========================================
    //               Helpers
    // ==========================================

    /// 向当前作用域注册符号，处理重定义错误
    fn define_symbol(
        &mut self,
        name: SymbolId,
        kind: SymbolKind,
        node_id: ast::NodeId,
        def_id: Option<DefId>,
        is_mutable: bool,
        span: crate::utils::Span,
    ) {
        let info = SymbolInfo {
            kind,
            node_id,
            type_id: TypeId::ERROR, // Collect 阶段尚未推导类型
            def_id,
            is_mutable,
        };

        if self.ctx.scopes.define(name, info).is_err() {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx.emit_error(span, format!("Symbol `{}` has already been defined in this scope", name_str));
        }
    }
}