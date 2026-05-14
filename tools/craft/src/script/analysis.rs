use crate::error::{Error, Result};
use crate::sdk;
use kernc_driver::CompilerDriver;
use kernc_sema::def::{Def, DefId, Visibility};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Session;
use kernc_utils::config::CompileOptions;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub(super) struct ScriptEntrySpec {
    pub(super) script_kind: &'static str,
    pub(super) entry_name: &'static str,
    pub(super) script_name: &'static str,
    pub(super) param_display: &'static str,
}

pub(super) const CRAFT_SCRIPT_ENTRY: ScriptEntrySpec = ScriptEntrySpec {
    script_kind: "craft",
    entry_name: "craft",
    script_name: "craft.kn",
    param_display: "&mut plan.Plan",
};

pub(super) const BUILD_SCRIPT_ENTRY: ScriptEntrySpec = ScriptEntrySpec {
    script_kind: "build",
    entry_name: "build",
    script_name: "build.kn",
    param_display: "&mut builder.Builder",
};

pub(super) struct PreparedScript<'a> {
    pub(super) script_path: PathBuf,
    pub(super) ctx: kernc_sema::SemaContext<'a>,
    pub(super) entry_def: DefId,
}

pub(super) fn validate_script(path: &Path, spec: ScriptEntrySpec) -> Result<()> {
    let mut session = Session::new();
    let _ = prepare_script(path, &mut session, spec)?;
    Ok(())
}

pub(super) fn prepare_script<'a>(
    path: &Path,
    session: &'a mut Session,
    spec: ScriptEntrySpec,
) -> Result<PreparedScript<'a>> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut ctx = analyze_script(&script_path, &script_input, session, spec.script_kind)?;
    let entry_def = find_script_entry(&mut ctx, &script_path, spec.entry_name, spec.script_name)?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        spec.script_name,
        spec.entry_name,
        spec.param_display,
    )?;

    Ok(PreparedScript {
        script_path,
        ctx,
        entry_def,
    })
}

fn canonical_script_path(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|err| Error::from_io(path, err))
}

fn analyze_script<'a>(
    script_path: &Path,
    script_input: &str,
    session: &'a mut Session,
    script_kind: &str,
) -> Result<kernc_sema::SemaContext<'a>> {
    let mut options = CompileOptions {
        input_file: Some(script_input.to_string()),
        ..CompileOptions::default()
    };
    options.module_aliases.insert(
        "craft".to_string(),
        sdk::sdk_root().to_string_lossy().to_string(),
    );
    let driver = CompilerDriver::new(options);
    driver
        .analyze(session, script_input)
        .ok_or_else(|| Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("{script_kind} script did not pass Kern parsing or semantic analysis"),
        })
}

fn find_script_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_path: &Path,
    entry_name: &str,
    script_name: &str,
) -> Result<DefId> {
    let root_name = ctx.intern("root");
    let entry_name = ctx.intern(entry_name);
    let Some(root_module_id) = ctx.defs.iter().find_map(|def| match def {
        Def::Module(module) if module.parent.is_none() && module.name == root_name => {
            Some(module.id)
        }
        _ => None,
    }) else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "script root module was not constructed".to_string(),
        });
    };

    ctx.defs
        .iter()
        .find_map(|def| match def {
            Def::Function(func)
                if func.parent == Some(root_module_id) && func.name == entry_name =>
            {
                Some(func.id)
            }
            _ => None,
        })
        .ok_or_else(|| Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!(
                "missing required entry `pub fn {}`(...) ... at script root",
                script_name.trim_end_matches(".kn")
            ),
        })
}

fn validate_script_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    entry_def: DefId,
    script_path: &Path,
    script_name: &str,
    entry_name: &str,
    param_display: &str,
) -> Result<()> {
    let Def::Function(entry) = &ctx.defs[entry_def.0 as usize] else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("{entry_name} entry does not reference a function definition"),
        });
    };

    if entry.vis != Visibility::Public {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry function must be declared `pub`"),
        });
    }

    if entry.body.is_none() {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry function must provide a body"),
        });
    }

    if entry.is_extern {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry function cannot be `extern`"),
        });
    }

    let Some(sig_ty) = entry.resolved_sig else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry function signature was not resolved"),
        });
    };

    let TypeKind::Function { params, ret, .. } = ctx.type_registry.get(sig_ty).clone() else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry does not resolve to a function type"),
        });
    };

    if params.len() != 1 {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!(
                "`{script_name}` entry must have exactly one parameter: `{param_display}`"
            ),
        });
    }

    if ret != TypeId::VOID {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!("`{script_name}` entry must return `void`"),
        });
    }

    let param_ty = params[0];
    if !matches!(
        ctx.type_registry.get(param_ty),
        TypeKind::Pointer { is_mut: true, .. }
    ) {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: format!(
                "`{script_name}` entry parameter must be a mutable pointer like `{param_display}`"
            ),
        });
    }

    Ok(())
}
