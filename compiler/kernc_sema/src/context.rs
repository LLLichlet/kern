use kernc_utils::AtomicOrdering;
use kernc_utils::{DiagnosticBuilder, DiagnosticLevel, FileId, NodeId, Session, Span, SymbolId};
use std::collections::HashMap;

use crate::def::{Def, DefId};
use crate::scope::{ScopeId, SymbolTable};
use crate::ty::{TypeFormatter, TypeId, TypeRegistry};

pub struct SemaContext<'a> {
    // 1. 底层设施：持有全局 Session 的可变借用
    // 通过它报错、分配新 ID、查询字符串
    pub sess: &'a mut Session,

    // 2. 类型系统核心
    pub type_registry: TypeRegistry,
    // 记录每个 AST 节点推导出的最终类型
    pub node_types: HashMap<NodeId, TypeId>,
    pub atomic_orderings: HashMap<NodeId, AtomicOrdering>,
    // 用于临时存储当前作用域下泛型参数的 Trait 约束 (Bounds)
    pub active_bounds: Vec<(TypeId, Vec<TypeId>)>,

    // 3. 符号与作用域系统
    pub defs: Vec<Def>,
    pub scopes: SymbolTable,
    pub global_impls: Vec<DefId>,

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
            atomic_orderings: HashMap::new(),
            active_bounds: Vec::new(),
            defs: Vec::new(),
            scopes: SymbolTable::new(),
            module_aliases: HashMap::new(),
            alias_roots: HashMap::new(),
            global_impls: Vec::new(),
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
                node_id,
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

    /// 为类型生成确定性且唯一的修饰后缀
    pub fn mangle_type(&self, ty: TypeId) -> String {
        let norm_ty = self.type_registry.normalize(ty);
        match self.type_registry.get(norm_ty).clone() {
            crate::ty::TypeKind::Primitive(p) => match p {
                crate::ty::PrimitiveType::Void => "void".to_string(),
                crate::ty::PrimitiveType::Bool => "bool".to_string(),
                crate::ty::PrimitiveType::I8 => "i8".to_string(),
                crate::ty::PrimitiveType::I16 => "i16".to_string(),
                crate::ty::PrimitiveType::I32 => "i32".to_string(),
                crate::ty::PrimitiveType::I64 => "i64".to_string(),
                crate::ty::PrimitiveType::I128 => "i128".to_string(),
                crate::ty::PrimitiveType::ISize => "isize".to_string(),
                crate::ty::PrimitiveType::U8 => "u8".to_string(),
                crate::ty::PrimitiveType::U16 => "u16".to_string(),
                crate::ty::PrimitiveType::U32 => "u32".to_string(),
                crate::ty::PrimitiveType::U64 => "u64".to_string(),
                crate::ty::PrimitiveType::U128 => "u128".to_string(),
                crate::ty::PrimitiveType::USize => "usize".to_string(),
                crate::ty::PrimitiveType::F32 => "f32".to_string(),
                crate::ty::PrimitiveType::F64 => "f64".to_string(),
                crate::ty::PrimitiveType::Str => "str".to_string(),
                crate::ty::PrimitiveType::Never => "never".to_string(),
            },
            crate::ty::TypeKind::Pointer { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Pmut{}", inner)
                } else {
                    format!("P{}", inner)
                }
            }
            crate::ty::TypeKind::VolatilePtr { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("Vmut{}", inner)
                } else {
                    format!("V{}", inner)
                }
            }
            crate::ty::TypeKind::Slice { is_mut, elem } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("S{}_mut", inner)
                } else {
                    format!("S{}", inner)
                }
            }
            crate::ty::TypeKind::Array { is_mut, elem, len } => {
                let inner = self.mangle_type(elem);
                if is_mut {
                    format!("A{}mut{}", len, inner)
                } else {
                    format!("A{}{}", len, inner)
                }
            }
            crate::ty::TypeKind::Def(def_id, gen_args)
            | crate::ty::TypeKind::Enum(def_id, gen_args)
            | crate::ty::TypeKind::TraitObject(def_id, gen_args) => {
                let def = &self.defs[def_id.0 as usize];
                let base_name = if let Some(n) = def.name() {
                    self.resolve(n).to_string()
                } else {
                    format!("AnonDef{}", def_id.0)
                };

                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_type(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::Function { params, ret, .. }
            | crate::ty::TypeKind::ClosureInterface { params, ret } => {
                let mut s = String::from("F");
                for p in params {
                    let p_str = self.mangle_type(p);
                    s.push_str(&format!("{}{}", p_str.len(), p_str));
                }
                s.push('R');
                let r_str = self.mangle_type(ret);
                s.push_str(&format!("{}{}", r_str.len(), r_str));
                s
            }
            crate::ty::TypeKind::FnDef(def_id, gen_args) => {
                let def = &self.defs[def_id.0 as usize];
                let base_name = if let Some(n) = def.name() {
                    self.resolve(n).to_string()
                } else {
                    format!("AnonFn{}", def_id.0)
                };
                if gen_args.is_empty() {
                    base_name
                } else {
                    let mut s = format!("{}I", base_name);
                    for arg in gen_args {
                        let arg_mangled = self.mangle_type(arg);
                        s.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
                    }
                    s.push('E');
                    s
                }
            }
            crate::ty::TypeKind::AnonymousState {
                closure_node_id, ..
            } => {
                format!("ClosureState{}", closure_node_id.0)
            }
            crate::ty::TypeKind::AnonymousStruct(is_extern, fields) => {
                // 格式：AStr + (字段名长度+字段名) + (字段类型长度+字段类型) + E
                let mut s = if is_extern {
                    String::from("EStr")
                } else {
                    String::from("AStr")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousUnion(is_extern, fields) => {
                let mut s = if is_extern {
                    String::from("EUni")
                } else {
                    String::from("AUni")
                };
                for f in fields {
                    let name_str = self.resolve(f.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    let ty_str = self.mangle_type(f.ty);
                    s.push_str(&format!("{}{}", ty_str.len(), ty_str));
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnum(enum_def) => {
                let mut s = String::from("AEnum");
                if let Some(backing_ty) = enum_def.backing_ty {
                    let backing = self.mangle_type(backing_ty);
                    s.push_str(&format!("B{}{}", backing.len(), backing));
                }
                for variant in &enum_def.variants {
                    let name_str = self.resolve(variant.name);
                    s.push_str(&format!("{}{}", name_str.len(), name_str));
                    if let Some(payload_ty) = variant.payload_ty {
                        let payload = self.mangle_type(payload_ty);
                        s.push_str(&format!("P{}{}", payload.len(), payload));
                    } else {
                        s.push('N');
                    }
                    if let Some(value) = variant.explicit_value {
                        s.push_str(&format!("V{}", value));
                    }
                    s.push('_');
                }
                s.push('E');
                s
            }
            crate::ty::TypeKind::AnonymousEnumPayload(enum_ty) => {
                let inner = self.mangle_type(enum_ty);
                format!("AEnumPayload{}{}", inner.len(), inner)
            }
            _ => "unknown".to_string(),
        }
    }

    /// 获取实体的最终全局链接符号名
    pub fn get_export_name(&self, def_id: DefId, args: &[TypeId]) -> String {
        let def = &self.defs[def_id.0 as usize];
        let name_str = def
            .name()
            .map(|name_sym| self.resolve(name_sym).to_string())
            .unwrap_or_else(|| format!("AnonDef{}", def_id.0));

        let empty_attrs: &[kernc_ast::Attribute] = &[]; // 静态空切片
        let (is_extern, attrs, parent_id) = match def {
            Def::Function(f) => (f.is_extern, f.attributes.as_slice(), f.parent),
            Def::Global(g) => (g.is_extern, g.attributes.as_slice(), None),
            Def::Struct(s) => (s.is_extern, s.attributes.as_slice(), None),
            Def::Enum(_) => (false, empty_attrs, None),
            Def::Union(_) => (false, empty_attrs, None),
            _ => return name_str,
        };

        // 1. export_name 属性覆盖 (仅限无泛型)
        if args.is_empty() {
            for attr in attrs {
                if let kernc_ast::AttributeKind::Meta(items) = &attr.kind {
                    for item in items {
                        if let kernc_ast::MetaItem::Call(sym_id, arg_expr) = item
                            && self.resolve(*sym_id) == "export_name"
                            && let kernc_ast::ExprKind::String(ref s) = arg_expr.kind
                        {
                            return s.clone();
                        }
                    }
                }
            }
        }

        // 2. extern 函数保持原样
        if is_extern && args.is_empty() {
            return name_str;
        }

        // 3. 构建 Itanium 风格路径
        let mut mangled = String::from("_K");
        let mut path_components = Vec::new();

        let mut current_parent = parent_id;
        while let Some(pid) = current_parent {
            match &self.defs[pid.0 as usize] {
                Def::Module(m) => {
                    path_components.push(self.resolve(m.name).to_string());
                    current_parent = m.parent;
                }
                Def::Impl(i) => {
                    let target_ty = self
                        .node_types
                        .get(&i.target_type.id)
                        .copied()
                        .unwrap_or(TypeId::ERROR);
                    path_components.push(self.mangle_type(target_ty));
                    current_parent = i.parent_module;
                }
                _ => break,
            }
        }

        for comp in path_components.into_iter().rev() {
            mangled.push_str(&format!("{}{}", comp.len(), comp));
        }

        // 4. 压入本体名字与泛型参数
        mangled.push_str(&format!("{}{}", name_str.len(), name_str));

        if !args.is_empty() {
            mangled.push('I');
            for &arg in args {
                let arg_mangled = self.mangle_type(arg);
                mangled.push_str(&format!("{}{}", arg_mangled.len(), arg_mangled));
            }
            mangled.push('E');
        }

        mangled
    }
}
