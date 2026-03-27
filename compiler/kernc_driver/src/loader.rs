use std::collections::HashMap;
use std::path::{Path, PathBuf};

use kernc_ast as ast;
use kernc_parser::Parser;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, ModuleDef};
use kernc_sema::passes::Pruner;
use kernc_utils::SymbolId;

pub struct ModuleLoader<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
    // 避免循环依赖：物理绝对路径 -> 模块 ID
    pub loaded_files: HashMap<PathBuf, DefId>,
    // 暂存所有解析好的 AST，等待下一阶段 Collector 提取符号
    pub asts: Vec<(DefId, ast::Module)>,
}

impl<'a, 'ctx> ModuleLoader<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self {
            ctx,
            loaded_files: HashMap::new(),
            asts: Vec::new(),
        }
    }

    pub fn load_root(&mut self, root_file: &str) -> Option<DefId> {
        let path = PathBuf::from(root_file);
        let name = self.ctx.intern("root");
        let root_id = self.load_module(path, None, name);
        self.load_alias_roots();
        root_id
    }

    fn load_alias_roots(&mut self) {
        let aliases = self.ctx.module_aliases.clone();
        for (alias_name, alias_path) in aliases {
            let sym = self.ctx.intern(&alias_name);
            let Some(path) = self.resolve_root_module_path(&PathBuf::from(&alias_path)) else {
                eprintln!(
                    "Error: Cannot find module path for alias `{}` at `{}`",
                    alias_name, alias_path
                );
                continue;
            };

            if let Some(mod_id) = self.load_module(path, None, sym) {
                self.ctx.alias_roots.insert(sym, mod_id);
            }
        }
    }

    fn resolve_root_module_path(&self, base_path: &Path) -> Option<PathBuf> {
        let dir_init = base_path.join("init.kr");
        let file_kn = PathBuf::from(format!("{}.kr", base_path.display()));

        if dir_init.exists() {
            Some(dir_init)
        } else if file_kn.exists() {
            Some(file_kn)
        } else if base_path.exists() && base_path.is_file() {
            Some(base_path.to_path_buf())
        } else {
            None
        }
    }

    fn resolve_submodule_path(&mut self, dir_path: &Path, decl: &ast::Decl) -> Option<PathBuf> {
        let mod_name_str = self.ctx.resolve(decl.name);
        let dir_init = dir_path.join(mod_name_str).join("init.kr");
        let file_kn = dir_path.join(format!("{}.kr", mod_name_str));

        if dir_init.exists() {
            Some(dir_init)
        } else if file_kn.exists() {
            Some(file_kn)
        } else {
            self.ctx
                .struct_error(
                    decl.span,
                    format!("Cannot find module file for `{}`", mod_name_str),
                )
                .with_hint(format!(
                    "expected to find `{}` or `{}`",
                    file_kn.display(),
                    dir_init.display()
                ))
                .emit();
            None
        }
    }

    fn read_module_source(&mut self, abs_path: &PathBuf) -> Option<String> {
        match std::fs::read_to_string(abs_path) {
            Ok(s) => Some(s),
            Err(e) => {
                self.ctx.sess.error_count += 1;
                eprintln!(
                    "Error: Cannot read module file '{}': {}",
                    abs_path.display(),
                    e
                );
                None
            }
        }
    }

    fn load_module(
        &mut self,
        path: PathBuf,
        parent: Option<DefId>,
        name: SymbolId,
    ) -> Option<DefId> {
        let abs_path = std::fs::canonicalize(&path).unwrap_or(path.clone());

        if let Some(&mod_id) = self.loaded_files.get(&abs_path) {
            return Some(mod_id);
        }

        let src = self.read_module_source(&abs_path)?;
        let Some(dir_path) = abs_path.parent().map(|p| p.to_path_buf()) else {
            self.ctx.sess.error_count += 1;
            eprintln!(
                "Error: Cannot determine parent directory for module file '{}'.",
                abs_path.display()
            );
            return None;
        };

        let mod_id = DefId(self.ctx.defs.len() as u32);
        self.loaded_files.insert(abs_path.clone(), mod_id);
        let file_id = self
            .ctx
            .sess
            .source_manager
            .add_file(abs_path.to_string_lossy().to_string(), src.clone());

        let scope_id = self.ctx.scopes.enter_scope();
        self.ctx.scopes.exit_scope();

        let is_init = abs_path.file_name().and_then(|n| n.to_str()) == Some("init.kr");

        let dummy_def = ModuleDef {
            id: mod_id,
            name,
            parent,
            scope_id,
            dir_path: dir_path.clone(),
            file_id,
            is_init,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
        };
        self.ctx.add_def(Def::Module(dummy_def));
        let mut ast = {
            let mut parser = Parser::new(&src, file_id, self.ctx.sess);
            match parser.parse_module() {
                Ok(ast) => ast,
                Err(_) => return None,
            }
        };
        ast.path = abs_path.to_string_lossy().to_string();

        let mut pruner = Pruner::new(self.ctx.sess);
        pruner.prune_module(&mut ast);

        let mut submodules = HashMap::new();

        for decl in &ast.decls {
            if let ast::DeclKind::ModDecl { .. } = &decl.kind
                && let Some(p) = self.resolve_submodule_path(&dir_path, decl)
                && let Some(sub_id) = self.load_module(p, Some(mod_id), decl.name)
            {
                submodules.insert(decl.name, sub_id);
            }
        }

        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.submodules = submodules;
        }

        self.asts.push((mod_id, ast));
        Some(mod_id)
    }
}
