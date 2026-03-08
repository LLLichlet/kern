#![allow(unused)]

use crate::ast::{UsePathKind, UseTarget};
use crate::context::Context;
use crate::sema::def::{Def, ImportDef, Visibility};
use crate::sema::scope::{ScopeId, SymbolInfo, SymbolKind};
use crate::sema::ty::DefId;
use crate::utils::SymbolId;

pub struct ImportResolver<'a> {
    pub ctx: &'a mut Context,
}

impl<'a> ImportResolver<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self { ctx }
    }

    /// 执行完整的导入解析 Pass
    pub fn resolve_all(&mut self) {
        // 提取所有模块 ID。这里使用 clone() 避开后续借用冲突
        let module_ids: Vec<DefId> = self.ctx.defs.iter().filter_map(|def| {
            if let Def::Module(m) = def { Some(m.id) } else { None }
        }).collect();

        // 提示：如果要完美支持乱序的 `pub use` 链条 (A 导 B，B 导 C)，
        // 应当在这里加入 fixed-point iteration (不动点迭代) 或者对导入依赖做拓扑排序。
        // 对于 Kern 这种显式要求清晰依赖树的语言，单次严格遍历通常已能满足大部分需求。
        for mod_id in module_ids {
            self.resolve_module_imports(mod_id);
        }
    }

    fn resolve_module_imports(&mut self, mod_id: DefId) {
        let imports = if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            m.imports.clone() // 克隆 imports 列表，释放 ctx 的借用
        } else {
            return;
        };

        for import in imports {
            self.resolve_single_import(mod_id, &import);
        }
    }

    fn resolve_single_import(&mut self, current_mod_id: DefId, import: &ImportDef) {
        let current_scope = self.get_module_scope(current_mod_id);

        // 1. 定位目标路径的 Module DefId 和 ScopeId
        // 注意：Use 语句的 `path` 部分 (如 std.math) 永远指向一个模块！具体的项在 target 中。
        let (target_mod_id, target_scope) = match self.resolve_path(current_mod_id, import.path_kind, &import.path, import.span) {
            Some(res) => res,
            None => return, // 寻址失败，错误已由 resolve_path 发出
        };

        // 2. 根据 Target 类型，将符号注入到当前作用域
        match &import.target {
            UseTarget::Module(alias) => {
            let (parent_path, last_segment) = import.path.split_at(import.path.len() - 1);
            let target_name = last_segment[0];

            // 1. 解析父级路径 (如果 parent_path 为空，resolve_path 会直接返回起点)
            let (parent_mod_id, parent_scope) = match self.resolve_path(current_mod_id, import.path_kind, parent_path, import.span) {
                Some(res) => res,
                None => return,
            };

            // 2. 在父模块中查找最终的目标符号 (可能是 Module，也可能是 Item)
            if let Some(symbol_info) = self.ctx.scopes.resolve_in(parent_scope, target_name) {
                // 3. 可见性检查
                if !self.check_visibility(symbol_info.def_id, current_mod_id, parent_mod_id) {
                    let name_str = self.ctx.resolve(target_name).to_string();
                    self.ctx.emit_error(import.span, format!("Symbol `{}` is private", name_str));
                    return;
                }

                let name_to_bind = alias.unwrap_or(target_name);
                self.define_import(current_scope, name_to_bind, symbol_info.clone(), import.span);
            } else {
                let name_str = self.ctx.resolve(target_name).to_string();
                self.ctx.emit_error(import.span, format!("Cannot find `{}`", name_str));
            }
        }
            UseTarget::Members(members) => {
                // 场景: `use std.math.{ PI, max as maximum };`
                for member in members {
                    if let Some(symbol_info) = self.ctx.scopes.resolve_in(target_scope, member.name) {
                        
                        // 2.1 可见性检查
                        if !self.check_visibility(symbol_info.def_id, current_mod_id, target_mod_id) {
                            let name_str = self.ctx.resolve(member.name).to_string();
                            self.ctx.emit_error(member.span, format!("Symbol `{}` is private and cannot be imported", name_str));
                            continue;
                        }

                        // 2.2 注入作用域
                        let name_to_bind = member.alias.unwrap_or(member.name);
                        
                        // 注意：对于 `pub use` (is_reexport == true)，
                        // 我们仅仅将其放入当前 Scope。要让其他模块知道它是 pub 的，
                        // 后续的 check_visibility 机制或 SymbolInfo 需要扩展以记录“导出状态”。
                        self.define_import(current_scope, name_to_bind, symbol_info.clone(), member.span);
                    } else {
                        let name_str = self.ctx.resolve(member.name).to_string();
                        self.ctx.emit_error(member.span, format!("Cannot find `{}` in the target module", name_str));
                    }
                }
            }
        }
    }

    // ==========================================
    //               Core Resolution
    // ==========================================

    /// 解析路径，返回目标模块的 (DefId, ScopeId)
    fn resolve_path(
        &mut self,
        current_mod_id: DefId,
        kind: UsePathKind,
        path: &[SymbolId],
        span: crate::utils::Span,
    ) -> Option<(DefId, ScopeId)> {
        
        // 确定查找起点
        let (mut curr_mod_id, mut curr_scope) = match kind {
            UsePathKind::Absolute => {
                (DefId(0), ScopeId(0)) 
            }
            UsePathKind::Relative => {
                (current_mod_id, self.get_module_scope(current_mod_id))
            }
            UsePathKind::Super => {
                // 获取当前模块的父模块
                if let Def::Module(m) = &self.ctx.defs[current_mod_id.0 as usize] {
                    if let Some(parent_id) = m.parent {
                        // 【语言设计特性：严格层级约束】
                        // 如果父模块是 `init` (目录模块)，严禁子模块回溯导入，打断循环依赖！
                        if self.is_init_module(parent_id) {
                            self.ctx.emit_error(span, "Child modules are strictly forbidden from importing `init.kn` contents".into());
                            return None;
                        }
                        (parent_id, self.get_module_scope(parent_id))
                    } else {
                        self.ctx.emit_error(span, "Cannot use `..` because the current module is a top-level module".into());
                        return None;
                    }
                } else {
                    unreachable!()
                }
            }
        };

        // 如果路径为空（例如只写了 `use . ;`），直接返回起点
        if path.is_empty() {
            return Some((curr_mod_id, curr_scope));
        }

        // 逐级深入路径查找子模块
        for &segment in path {
            if let Some(symbol) = self.ctx.scopes.resolve_in(curr_scope, segment) {
                if symbol.kind == SymbolKind::Module {
                    if let Some(target_def_id) = symbol.def_id {
                        curr_mod_id = target_def_id;
                        curr_scope = self.get_module_scope(target_def_id);
                        continue;
                    }
                }
                
                let name = self.ctx.resolve(segment).to_string();
                self.ctx.emit_error(span, format!("`{}` is not a module", name));
                return None;

            } else {
                let name = self.ctx.resolve(segment).to_string();
                self.ctx.emit_error(span, format!("Unresolved import: cannot find module `{}`", name));
                return None;
            }
        }

        Some((curr_mod_id, curr_scope))
    }

    /// 检查符号是否对当前模块可见
    fn check_visibility(&self, symbol_def_id: Option<DefId>, current_mod: DefId, target_mod: DefId) -> bool {
        // 如果是从当前模块自身导入，永远可见
        if current_mod == target_mod {
            return true;
        }

        let def_id = match symbol_def_id {
            Some(id) => id,
            None => return true, // 如果没有 DefId（例如内置虚假符号），假定可见
        };

        let def = &self.ctx.defs[def_id.0 as usize];
        
        // 提取定义本身的可见性
        let vis = match def {
            Def::Function(d) => d.vis,
            Def::Struct(d) => d.vis,
            Def::Union(d) => d.vis,
            Def::Enum(d) => d.vis,
            Def::Trait(d) => d.vis,
            Def::Global(d) => d.vis,
            Def::TypeAlias(d) => d.vis,
            Def::Module(_) => Visibility::Public, // 模块本身的可见性通常由其导出方式决定，默认为 Pub
            Def::Impl(_) => return true,          // Impl 块没有直接的可见性概念
        };

        match vis {
            Visibility::Public => true,
            Visibility::Private => {
                // 私有成员仅对同模块，或该模块的直接子模块可见
                // 这里我们做严格限制：私有成员仅对同模块可见。
                false 
            }
        }
    }

    // ==========================================
    //               Helpers
    // ==========================================

    fn get_module_scope(&self, mod_id: DefId) -> ScopeId {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            m.scope_id
        } else {
            panic!("DefId {:?} is not a module", mod_id)
        }
    }

    /// 判断模块是否名为 `init`，用于检查严格层级约束
    fn is_init_module(&self, mod_id: DefId) -> bool {
        if let Def::Module(m) = &self.ctx.defs[mod_id.0 as usize] {
            let name_str = self.ctx.resolve(m.name);
            name_str == "init"
        } else {
            false
        }
    }

    /// 将解析好的符号注入到当前模块的作用域
    fn define_import(&mut self, target_scope: ScopeId, name: SymbolId, info: SymbolInfo, span: crate::utils::Span) {
        // 保存之前的执行上下文
        let prev_scope = self.ctx.scopes.current_scope_id();
        
        // 切换到目标作用域进行绑定
        self.ctx.scopes.set_current_scope(target_scope);
        
        if self.ctx.scopes.define(name, info).is_err() {
            let name_str = self.ctx.resolve(name).to_string();
            self.ctx.emit_error(span, format!("A symbol named `{}` already exists in this module", name_str));
        }

        // 恢复执行上下文
        if let Some(prev) = prev_scope {
            self.ctx.scopes.set_current_scope(prev);
        }
    }
}