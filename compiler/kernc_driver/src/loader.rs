use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::frontend::FrontendDatabase;
use crate::metadata;
use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, ModuleDef};
use kernc_utils::SymbolId;

struct ResolvedRootModule {
    entry_path: PathBuf,
    declared_root_name: Option<SymbolId>,
}

pub struct ModuleLoader<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
    // Prevent import cycles: physical absolute path -> module ID.
    pub loaded_files: HashMap<PathBuf, DefId>,
    // Cache parsed ASTs until the collector extracts semantic symbols.
    pub asts: Vec<(DefId, ast::Module)>,
    frontend: &'a FrontendDatabase,
}

impl<'a, 'ctx> ModuleLoader<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>, frontend: &'a FrontendDatabase) -> Self {
        Self {
            ctx,
            loaded_files: HashMap::new(),
            asts: Vec::new(),
            frontend,
        }
    }

    pub fn load_root(&mut self, root_file: &str, root_name: SymbolId) -> Option<DefId> {
        let path = PathBuf::from(root_file);
        let root_id = self.load_module(path, None, root_name, false);
        self.ctx.root_module = root_id;
        self.load_alias_roots(false);
        self.load_alias_roots(true);
        root_id
    }

    fn load_alias_roots(&mut self, imported: bool) {
        let aliases = if imported {
            self.ctx.module_interface_aliases.clone()
        } else {
            self.ctx.module_aliases.clone()
        };
        for (alias_name, alias_path) in aliases {
            let alias_sym = self.ctx.intern(&alias_name);
            let Some(root) = self.resolve_root_module(&PathBuf::from(&alias_path), imported) else {
                continue;
            };

            let module_name = root.declared_root_name.unwrap_or(alias_sym);
            if let Some(mod_id) = self.load_module(root.entry_path, None, module_name, imported) {
                self.ctx.alias_roots.insert(alias_sym, mod_id);
            }
        }
    }

    fn resolve_root_module(
        &mut self,
        base_path: &Path,
        require_manifest: bool,
    ) -> Option<ResolvedRootModule> {
        if base_path.is_dir() {
            match metadata::load_manifest(base_path) {
                Ok(Some(manifest)) => {
                    let entry_path = base_path.join(&manifest.entry_module_path);
                    if !entry_path.is_file() {
                        eprintln!(
                            "Error: kmeta package at `{}` points to missing entry module `{}`",
                            base_path.display(),
                            entry_path.display()
                        );
                        return None;
                    }

                    let declared_root_name = Some(self.ctx.intern(&manifest.root_module_name));
                    return Some(ResolvedRootModule {
                        entry_path,
                        declared_root_name,
                    });
                }
                Ok(None) => {
                    if require_manifest {
                        eprintln!(
                            "Error: Imported package path `{}` is missing `{}`",
                            base_path.display(),
                            metadata::KMETA_MANIFEST_FILE
                        );
                        return None;
                    }
                }
                Err(err) => {
                    eprintln!(
                        "Error: Failed to read kmeta manifest from `{}`: {}",
                        base_path.display(),
                        err
                    );
                    return None;
                }
            }
        }

        if require_manifest {
            eprintln!(
                "Error: Imported package alias expects a kmeta package root at `{}`",
                base_path.display()
            );
            return None;
        }

        let dir_init = base_path.join("init.rn");
        let file_kn = PathBuf::from(format!("{}.rn", base_path.display()));

        if self.path_exists(&dir_init) {
            Some(ResolvedRootModule {
                entry_path: dir_init,
                declared_root_name: None,
            })
        } else if self.path_exists(&file_kn) {
            Some(ResolvedRootModule {
                entry_path: file_kn,
                declared_root_name: None,
            })
        } else if self.path_exists(base_path) && !base_path.is_dir() {
            Some(ResolvedRootModule {
                entry_path: base_path.to_path_buf(),
                declared_root_name: None,
            })
        } else {
            None
        }
    }

    fn resolve_submodule_path(&mut self, dir_path: &Path, decl: &ast::Decl) -> Option<PathBuf> {
        let mod_name_str = self.ctx.resolve(decl.name);
        let dir_init = dir_path.join(mod_name_str).join("init.rn");
        let file_kn = dir_path.join(format!("{}.rn", mod_name_str));

        if self.path_exists(&dir_init) {
            Some(dir_init)
        } else if self.path_exists(&file_kn) {
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

    fn load_module(
        &mut self,
        path: PathBuf,
        parent: Option<DefId>,
        name: SymbolId,
        is_imported: bool,
    ) -> Option<DefId> {
        let abs_path = Self::normalize_path(&path);

        if let Some(&mod_id) = self.loaded_files.get(&abs_path) {
            return Some(mod_id);
        }

        let parsed = match self
            .frontend
            .load_parsed_module_uncached(self.ctx.sess, &abs_path)
        {
            Ok(Some(parsed)) => parsed,
            Ok(None) => {
                self.ctx.sess.error_count += 1;
                eprintln!(
                    "Error: Cannot read or parse module file '{}'.",
                    abs_path.display()
                );
                return None;
            }
            Err(err) => {
                self.ctx.sess.error_count += 1;
                eprintln!(
                    "Error: Query cycle while loading module '{}': {}",
                    abs_path.display(),
                    err
                );
                return None;
            }
        };
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
        let file_id = parsed.file_id;

        let scope_id = self.ctx.scopes.enter_scope();
        self.ctx.scopes.exit_scope();

        let is_init = abs_path.file_name().and_then(|n| n.to_str()) == Some("init.rn");

        let dummy_def = ModuleDef {
            id: mod_id,
            name,
            parent,
            is_imported,
            scope_id,
            dir_path: dir_path.clone(),
            file_id,
            is_init,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
            docs: None,
        };
        self.ctx.add_def(Def::Module(dummy_def));
        let ast = parsed.ast;

        let mut submodules = HashMap::new();

        for decl in &ast.decls {
            if let ast::DeclKind::ModDecl { .. } = &decl.kind
                && let Some(p) = self.resolve_submodule_path(&dir_path, decl)
                && let Some(sub_id) = self.load_module(p, Some(mod_id), decl.name, is_imported)
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

    fn normalize_path(path: &Path) -> PathBuf {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path = Self::strip_windows_verbatim_prefix(path);
        Self::strip_macos_private_var_prefix(path)
    }

    #[cfg(windows)]
    fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
        let raw = path.to_string_lossy();
        if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
            return PathBuf::from(format!("\\\\{stripped}"));
        }
        if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
            return PathBuf::from(stripped);
        }
        path
    }

    #[cfg(not(windows))]
    fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
        path
    }

    #[cfg(target_os = "macos")]
    fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
        let raw = path.to_string_lossy();
        if let Some(stripped) = raw.strip_prefix("/private/var/") {
            return PathBuf::from(format!("/var/{stripped}"));
        }
        if raw == "/private/var" {
            return PathBuf::from("/var");
        }
        path
    }

    #[cfg(not(target_os = "macos"))]
    fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
        path
    }

    fn path_exists(&self, path: &Path) -> bool {
        self.frontend.source_exists(path).unwrap_or(false)
    }
}
