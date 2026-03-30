use crate::error::{Error, Result};
use crate::graph::DependencyKind;
use crate::plan::{PackagePlan, TargetKind};
use kernc_driver::CompilerDriver;
use kernc_sema::checker::{ConstEvaluator, ConstValue, ScriptHost};
use kernc_sema::def::{Def, Visibility};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Session;
use kernc_utils::config::CompileOptions;
use kernc_utils::{Span, SymbolId};
use std::collections::HashMap;
use std::path::Path;

pub fn validate_kraft_script(path: &Path) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session)?;
    let entry_def = find_kraft_entry(&mut ctx, &script_path)?;
    validate_kraft_entry(&mut ctx, entry_def, &script_path)?;

    Ok(())
}

pub fn apply_kraft_script(path: &Path, package_plan: &mut PackagePlan) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session)?;
    let entry_def = find_kraft_entry(&mut ctx, &script_path)?;
    validate_kraft_entry(&mut ctx, entry_def, &script_path)?;

    let mut host = PackagePlanHost { package_plan };
    let arg_values = vec![plan_argument_value(&mut ctx)];
    let mut evaluator = ConstEvaluator::with_script_host(&mut ctx, &mut host);
    evaluator
        .eval_function(entry_def, &[], arg_values, Span::default())
        .map_err(|_| Error::ScriptValidation {
            path: script_path,
            message: "kraft script execution failed".to_string(),
        })?;

    Ok(())
}

fn sdk_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk")
}

fn canonical_script_path(path: &Path) -> Result<std::path::PathBuf> {
    path.canonicalize().map_err(|err| Error::from_io(path, err))
}

fn analyze_script<'a>(
    script_path: &Path,
    script_input: &str,
    session: &'a mut Session,
) -> Result<kernc_sema::SemaContext<'a>> {
    let mut options = CompileOptions {
        input_file: Some(script_input.to_string()),
        ..CompileOptions::default()
    };
    options.module_aliases.insert(
        "kraft".to_string(),
        sdk_root().to_string_lossy().to_string(),
    );
    let driver = CompilerDriver::new(options);
    driver
        .analyze(session, script_input)
        .ok_or_else(|| Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "script did not pass Kern parsing or semantic analysis".to_string(),
        })
}

fn find_kraft_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_path: &Path,
) -> Result<kernc_sema::def::DefId> {
    let root_name = ctx.intern("root");
    let entry_name = ctx.intern("kraft");
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
            message: "missing required entry `pub fn kraft(...) ...` at script root".to_string(),
        })
}

fn validate_kraft_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    entry_def: kernc_sema::def::DefId,
    script_path: &Path,
) -> Result<()> {
    let Def::Function(entry) = &ctx.defs[entry_def.0 as usize] else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "kraft entry does not reference a function definition".to_string(),
        });
    };

    if entry.vis != Visibility::Public {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry function must be declared `pub`".to_string(),
        });
    }

    if entry.body.is_none() {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry function must provide a body".to_string(),
        });
    }

    if entry.is_extern {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry function cannot be `extern`".to_string(),
        });
    }

    let Some(sig_ty) = entry.resolved_sig else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry function signature was not resolved".to_string(),
        });
    };

    let TypeKind::Function { params, ret, .. } = ctx.type_registry.get(sig_ty).clone() else {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry does not resolve to a function type".to_string(),
        });
    };

    if params.len() != 1 {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry must have exactly one parameter: `*mut plan.Plan`"
                .to_string(),
        });
    }

    if ret != TypeId::VOID {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry must return `void`".to_string(),
        });
    }

    let param_ty = params[0];
    if !matches!(
        ctx.type_registry.get(param_ty),
        TypeKind::Pointer { is_mut: true, .. }
    ) {
        return Err(Error::ScriptValidation {
            path: script_path.to_path_buf(),
            message: "`kraft.kr` entry parameter must be a mutable pointer like `*mut plan.Plan`"
                .to_string(),
        });
    }

    Ok(())
}

struct PackagePlanHost<'a> {
    package_plan: &'a mut PackagePlan,
}

impl ScriptHost for PackagePlanHost<'_> {
    fn call_extern(
        &mut self,
        name: &str,
        args: &[ConstValue],
        _span: Span,
    ) -> std::result::Result<ConstValue, String> {
        match name {
            "__kraft_plan_feature_enabled" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let _feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(false))
            }
            "__kraft_plan_cfg_bool" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_bool(args, 2, "cfg value")?;
                self.package_plan
                    .set_cfg_bool(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_cfg_string" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_string(args, 2, "cfg value")?;
                self.package_plan
                    .set_cfg_string(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_define_bool" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_bool(args, 2, "define value")?;
                self.package_plan
                    .set_define_bool(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_define_string" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_string(args, 2, "define value")?;
                self.package_plan
                    .set_define_string(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_set_lib_root" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let root = expect_string(args, 1, "lib root")?;
                self.package_plan
                    .set_lib_root(root)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_add_bin" => self.add_named_target(args, TargetKind::Bin, "bin"),
            "__kraft_plan_add_test" => self.add_named_target(args, TargetKind::Test, "test"),
            "__kraft_plan_add_example" => {
                self.add_named_target(args, TargetKind::Example, "example")
            }
            "__kraft_plan_remove_lib" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                Ok(ConstValue::Bool(
                    self.package_plan.remove_target(TargetKind::Lib, None),
                ))
            }
            "__kraft_plan_remove_bin" => self.remove_named_target(args, TargetKind::Bin, "bin"),
            "__kraft_plan_remove_test" => self.remove_named_target(args, TargetKind::Test, "test"),
            "__kraft_plan_remove_example" => {
                self.remove_named_target(args, TargetKind::Example, "example")
            }
            "__kraft_plan_dep_version" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let version = expect_string(args, 3, "dependency version")?;
                self.package_plan
                    .set_dependency_version(kind, &name, version)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_dep_path" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let path = expect_string(args, 3, "dependency path")?;
                self.package_plan
                    .set_dependency_path(kind, &name, path)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_dep_registry" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let registry = expect_string(args, 3, "dependency registry")?;
                self.package_plan
                    .set_dependency_registry(kind, &name, registry)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_dep_workspace" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                self.package_plan
                    .use_workspace_dependency(kind, &name)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__kraft_plan_remove_dep" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                Ok(ConstValue::Bool(
                    self.package_plan
                        .remove_dependency(kind, &name)
                        .map_err(|err| err.to_string())?,
                ))
            }
            _ => Err(format!("unsupported kraft host function `{name}`")),
        }
    }
}

impl PackagePlanHost<'_> {
    fn add_named_target(
        &mut self,
        args: &[ConstValue],
        kind: TargetKind,
        label: &str,
    ) -> std::result::Result<ConstValue, String> {
        let _ = expect_arg(args, 0, "plan receiver")?;
        let name = expect_string(args, 1, &format!("{label} name"))?;
        let root = expect_string(args, 2, &format!("{label} root"))?;
        self.package_plan
            .add_named_target(kind, name, root)
            .map_err(|err| err.to_string())?;
        Ok(ConstValue::Void)
    }

    fn remove_named_target(
        &mut self,
        args: &[ConstValue],
        kind: TargetKind,
        label: &str,
    ) -> std::result::Result<ConstValue, String> {
        let _ = expect_arg(args, 0, "plan receiver")?;
        let name = expect_string(args, 1, &format!("{label} name"))?;
        Ok(ConstValue::Bool(
            self.package_plan.remove_target(kind, Some(&name)),
        ))
    }
}

fn expect_arg<'a>(
    args: &'a [ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<&'a ConstValue, String> {
    args.get(index)
        .ok_or_else(|| format!("missing kraft host argument `{label}`"))
}

fn expect_string(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<String, String> {
    match expect_arg(args, index, label)? {
        ConstValue::String(value) => Ok(value.clone()),
        _ => Err(format!("expected `{label}` to be a string")),
    }
}

fn expect_bool(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<bool, String> {
    match expect_arg(args, index, label)? {
        ConstValue::Bool(value) => Ok(*value),
        _ => Err(format!("expected `{label}` to be a bool")),
    }
}

fn expect_dependency_kind(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<DependencyKind, String> {
    let tag = match expect_arg(args, index, label)? {
        ConstValue::Enum { tag, .. } => *tag,
        ConstValue::Int(tag) => *tag,
        _ => return Err(format!("expected `{label}` to be a dependency kind enum")),
    };

    match tag {
        0 => Ok(DependencyKind::Normal),
        1 => Ok(DependencyKind::Dev),
        2 => Ok(DependencyKind::Build),
        _ => Err(format!("invalid `{label}` value `{tag}`")),
    }
}

fn plan_argument_value(ctx: &mut kernc_sema::SemaContext<'_>) -> ConstValue {
    fn field(name: &str, ctx: &mut kernc_sema::SemaContext<'_>) -> SymbolId {
        ctx.intern(name)
    }

    let mut target = HashMap::new();
    target.insert(field("os", ctx), ConstValue::Int(0));
    target.insert(
        field("arch", ctx),
        ConstValue::String("unknown".to_string()),
    );
    target.insert(
        field("vendor", ctx),
        ConstValue::String("unknown".to_string()),
    );
    target.insert(field("env", ctx), ConstValue::String("unknown".to_string()));

    let mut profile = HashMap::new();
    profile.insert(field("name", ctx), ConstValue::String("dev".to_string()));
    profile.insert(field("opt", ctx), ConstValue::Int(0));
    profile.insert(field("debug", ctx), ConstValue::Bool(true));

    let mut plan = HashMap::new();
    plan.insert(field("target", ctx), ConstValue::Struct(target));
    plan.insert(field("profile", ctx), ConstValue::Struct(profile));

    ConstValue::Struct(plan)
}

#[cfg(test)]
mod tests {
    use super::validate_kraft_script;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn accepts_public_kraft_entry() {
        let root = temp_dir("kraft-script-valid");
        let path = root.join("kraft.kr");
        fs::write(
            &path,
            "use kraft.plan;\npub fn kraft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let result = validate_kraft_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_public_kraft_entry() {
        let root = temp_dir("kraft-script-missing-entry");
        let path = root.join("kraft.kr");
        fs::write(&path, "fn helper() void {}\n").unwrap();

        let err = validate_kraft_script(&path).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("missing required entry"),
            "unexpected error: {message}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_entry_without_plan_parameter() {
        let root = temp_dir("kraft-script-missing-plan-param");
        let path = root.join("kraft.kr");
        fs::write(&path, "pub fn kraft() void {}\n").unwrap();

        let err = validate_kraft_script(&path).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("exactly one parameter"),
            "unexpected error: {message}"
        );

        let _ = fs::remove_dir_all(root);
    }
}
