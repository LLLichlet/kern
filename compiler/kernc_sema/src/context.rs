use kernc_utils::{DiagnosticBuilder, DiagnosticLevel, FileId, NodeId, Session, Span, SymbolId};
use std::collections::HashMap;

use crate::def::{Def, DefId};
use crate::scope::{SymbolTable, ScopeId};
use crate::ty::{TypeFormatter, TypeId, TypeRegistry};

pub struct SemaContext<'a> {
    // 1. 底层设施：持有全局 Session 的可变借用
    // 通过它报错、分配新 ID、查询字符串
    pub sess: &'a mut Session,

    // 2. 类型系统核心
    pub type_registry: TypeRegistry,
    // 记录每个 AST 节点推导出的最终类型
    pub node_types: HashMap<NodeId, TypeId>,
    // 用于临时存储当前作用域下泛型参数的 Trait 约束 (Bounds)
    pub active_bounds: Vec<(TypeId, Vec<TypeId>)>,

    // 3. 符号与作用域系统
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,

    // 4. 模块与包管理
    pub module_aliases: HashMap<String, String>,
    pub alias_roots: HashMap<SymbolId, DefId>,
}

impl<'a> SemaContext<'a> {
    /// 初始化 SemaContext 需要传入已经存在的 Session
    pub fn new(sess: &'a mut Session) -> Self {
        Self {
            sess,
            type_registry: TypeRegistry::new(),
            node_types: HashMap::new(),
            active_bounds: Vec::new(),
            defs: Vec::new(),
            scopes: SymbolTable::new(),
            module_aliases: HashMap::new(),
            alias_roots: HashMap::new(),
        }
    }

    // ==========================================
    // 核心操作
    // ==========================================

    pub fn add_def(&mut self, def: Def) -> DefId {
        let id = DefId(self.defs.len() as u32);
        self.defs.push(def);
        id
    }

    pub fn ty_to_string(&self, ty: TypeId) -> String {
        TypeFormatter { ctx: self }.format(ty)
    }

    /// 将所有通过 -M 传入的模块别名（如 std）注入到全局的根作用域中。
    /// 这样可以直接在任何地方使用 `std.io`，而无需 `use std;`
    pub fn inject_alias_roots(&mut self) {
        // 获取当前的 Scope (为了注入后能恢复)
        let prev_scope = self.scopes.current_scope_id();
        
        // SymbolTable 初始化时创建的第一个 ScopeId(0) 就是全局 Builtin 作用域
        let global_scope = ScopeId(0);
        self.scopes.set_current_scope(global_scope);

        // 克隆一份 alias_roots 的键值对，避免和 scopes 产生借用冲突
        let aliases: Vec<(SymbolId, DefId)> = self
            .alias_roots
            .iter()
            .map(|(&name, &mod_id)| (name, mod_id))
            .collect();

        let node_id = self.next_node_id();
        for (name, mod_id) in aliases {
            let info = crate::scope::SymbolInfo {
                kind: crate::scope::SymbolKind::Module,
                node_id: node_id, 
                type_id: TypeId::ERROR, 
                def_id: Some(mod_id),
                span: kernc_utils::Span::default(),
                is_pub: true,
                is_mut: false,
            };

            // 忽略重复定义的错误（如果有同名的全局变量，在后续的 Collect 阶段会报出冲突）
            let _ = self.scopes.define(name, info);
        }

        // 恢复之前的上下文
        if let Some(prev) = prev_scope {
            self.scopes.set_current_scope(prev);
        }
    }

    // ==========================================
    // 代理便捷方法
    // ==========================================

    pub fn report(&mut self, span: Span, level: DiagnosticLevel, msg: String) {
        self.sess.report(span, level, msg);
    }

    pub fn has_errors(&self) -> bool {
        self.sess.has_errors()
    }

    pub fn struct_error(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_error(span, msg)
    }

    pub fn struct_warning(&mut self, span: Span, msg: impl Into<String>) -> DiagnosticBuilder<'_> {
        self.sess.struct_warning(span, msg)
    }

    pub fn emit_error(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_error(span, msg);
    }

    pub fn emit_warning(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_warning(span, msg.into());
    }

    pub fn emit_ice(&mut self, span: Span, msg: impl Into<String>) {
        self.sess.emit_ice(span, msg);
    }

    pub fn next_node_id(&mut self) -> NodeId {
        self.sess.next_node_id()
    }

    pub fn intern(&mut self, string: &str) -> SymbolId {
        self.sess.interner.intern(string)
    }

    pub fn resolve(&self, sym: SymbolId) -> &str {
        self.sess.interner.resolve(sym).unwrap_or("<unknown>")
    }

    pub fn load_file<P: AsRef<std::path::Path>>(&mut self, path: P) -> std::io::Result<FileId> {
        self.sess.load_file(path)
    }
}
