use crate::build_plan::BuildUnit;
use crate::error::{Error, Result};
use crate::graph::DependencyKind;
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use kernc_driver::CompilerDriver;
use kernc_sema::checker::{ConstEvaluator, ConstValue, ScriptHost};
use kernc_sema::def::{Def, Visibility};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Session;
use kernc_utils::config::CompileOptions;
use kernc_utils::{Span, SymbolId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptExecution {
    pub env_inputs: Vec<ScriptEnvInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEnvInput {
    pub name: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptContext {
    pub package: ScriptPackage,
    pub workspace: ScriptWorkspace,
    pub target: ScriptTarget,
    pub profile: ScriptProfile,
    pub command: ScriptCommand,
    pub features: BTreeSet<String>,
    pub env: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptContext {
    pub script: ScriptContext,
    pub unit: BuildScriptUnit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptUnit {
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub source_root: String,
    pub artifact_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPackage {
    pub name: String,
    pub version: String,
    pub root: String,
    pub is_root: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptWorkspace {
    pub root: String,
    pub has_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptTarget {
    pub os: ScriptOs,
    pub arch: String,
    pub vendor: String,
    pub env: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptProfile {
    pub name: String,
    pub opt: u8,
    pub debug: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptCommand {
    Check,
    Lock,
    Build,
    Run,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptOs {
    Unknown,
    Linux,
    Windows,
    Darwin,
}

pub fn host_target() -> ScriptTarget {
    let triple = CompileOptions::default().target.triple;
    let os = match triple.operating_system.to_string().as_str() {
        "linux" => ScriptOs::Linux,
        "windows" => ScriptOs::Windows,
        "darwin" | "macosx" => ScriptOs::Darwin,
        _ => ScriptOs::Unknown,
    };

    ScriptTarget {
        os,
        arch: triple.architecture.to_string(),
        vendor: triple.vendor.to_string(),
        env: triple.environment.to_string(),
    }
}

pub fn manifest_profile(manifest: &Manifest) -> ScriptProfile {
    let dev = manifest
        .profile
        .as_ref()
        .and_then(|profiles| profiles.dev.as_ref());
    ScriptProfile {
        name: "dev".to_string(),
        opt: dev.and_then(|profile| profile.opt).unwrap_or(0),
        debug: dev.and_then(|profile| profile.debug).unwrap_or(true),
    }
}

pub fn validate_kraft_script(path: &Path) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "kraft")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "kraft", "kraft.kr")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "kraft.kr",
        "kraft",
        "*mut plan.Plan",
    )?;

    Ok(())
}

pub fn validate_build_script(path: &Path) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "build")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "build", "build.kr")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "build.kr",
        "build",
        "*mut builder.Builder",
    )?;

    Ok(())
}

pub fn apply_kraft_script(
    path: &Path,
    package_plan: &mut PackagePlan,
    script_context: &ScriptContext,
) -> Result<ScriptExecution> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "kraft")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "kraft", "kraft.kr")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "kraft.kr",
        "kraft",
        "*mut plan.Plan",
    )?;

    let mut host = PackagePlanHost {
        package_plan,
        script_context,
        env_reads: BTreeSet::new(),
    };
    let arg_values = vec![plan_argument_value(&mut ctx, script_context)];
    let mut evaluator = ConstEvaluator::with_script_host(&mut ctx, &mut host);
    evaluator
        .eval_function(entry_def, &[], arg_values, Span::default())
        .map_err(|_| Error::ScriptValidation {
            path: script_path.clone(),
            message: ctx
                .sess
                .diagnostics
                .last()
                .map(|diag| format!("kraft script execution failed: {}", diag.message))
                .unwrap_or_else(|| "kraft script execution failed".to_string()),
        })?;

    Ok(ScriptExecution {
        env_inputs: host
            .env_reads
            .into_iter()
            .map(|name| ScriptEnvInput {
                value: host
                    .script_context
                    .env
                    .get(&name)
                    .cloned()
                    .expect("env read must come from declared env map"),
                name,
            })
            .collect(),
    })
}

pub fn apply_build_script(
    path: &Path,
    unit: &mut BuildUnit,
    script_context: &BuildScriptContext,
) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "build")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "build", "build.kr")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "build.kr",
        "build",
        "*mut builder.Builder",
    )?;

    let mut host = BuildUnitHost {
        unit,
        script_context,
    };
    let arg_values = vec![build_argument_value(&mut ctx, script_context)];
    let mut evaluator = ConstEvaluator::with_script_host(&mut ctx, &mut host);
    evaluator
        .eval_function(entry_def, &[], arg_values, Span::default())
        .map_err(|_| Error::ScriptValidation {
            path: script_path.clone(),
            message: ctx
                .sess
                .diagnostics
                .last()
                .map(|diag| format!("build script execution failed: {}", diag.message))
                .unwrap_or_else(|| "build script execution failed".to_string()),
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
    script_kind: &str,
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
            message: format!("{script_kind} script did not pass Kern parsing or semantic analysis"),
        })
}

fn find_script_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_path: &Path,
    entry_name: &str,
    script_name: &str,
) -> Result<kernc_sema::def::DefId> {
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
                script_name.trim_end_matches(".kr")
            ),
        })
}

fn validate_script_entry(
    ctx: &mut kernc_sema::SemaContext<'_>,
    entry_def: kernc_sema::def::DefId,
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

struct PackagePlanHost<'a> {
    package_plan: &'a mut PackagePlan,
    script_context: &'a ScriptContext,
    env_reads: BTreeSet<String>,
}

struct BuildUnitHost<'a> {
    unit: &'a mut BuildUnit,
    script_context: &'a BuildScriptContext,
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
                let feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(
                    self.script_context.features.contains(feature.as_str()),
                ))
            }
            "__kraft_plan_env" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let name = expect_string(args, 1, "environment name")?;
                let Some(value) = self.script_context.env.get(&name).cloned() else {
                    return Err(format!(
                        "environment `{name}` was not declared under `[kraft].env` (declared: {})",
                        self.script_context
                            .env
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                };
                self.env_reads.insert(name);
                Ok(match value {
                    Some(value) => ConstValue::Enum {
                        tag: 0,
                        payload: Some(Box::new(ConstValue::String(value))),
                    },
                    None => ConstValue::Enum {
                        tag: 1,
                        payload: None,
                    },
                })
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

impl ScriptHost for BuildUnitHost<'_> {
    fn call_extern(
        &mut self,
        name: &str,
        args: &[ConstValue],
        _span: Span,
    ) -> std::result::Result<ConstValue, String> {
        match name {
            "__kraft_build_feature_enabled" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(
                    self.script_context
                        .script
                        .features
                        .contains(feature.as_str()),
                ))
            }
            "__kraft_build_link_system_lib" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "system library name")?;
                push_unique(&mut self.unit.link.system_libs, name);
                Ok(ConstValue::Void)
            }
            "__kraft_build_link_framework" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "framework name")?;
                push_unique(&mut self.unit.link.frameworks, name);
                Ok(ConstValue::Void)
            }
            "__kraft_build_link_search" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let path = expect_string(args, 1, "link search path")?;
                push_unique(&mut self.unit.link.search_paths, path);
                Ok(ConstValue::Void)
            }
            "__kraft_build_link_arg" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let arg = expect_string(args, 1, "link argument")?;
                self.unit.link.args.push(arg);
                Ok(ConstValue::Void)
            }
            _ => Err(format!("unsupported build host function `{name}`")),
        }
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

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn plan_argument_value(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_context: &ScriptContext,
) -> ConstValue {
    fn field(name: &str, ctx: &mut kernc_sema::SemaContext<'_>) -> SymbolId {
        ctx.intern(name)
    }

    let mut package = HashMap::new();
    package.insert(
        field("name", ctx),
        ConstValue::String(script_context.package.name.clone()),
    );
    package.insert(
        field("version", ctx),
        ConstValue::String(script_context.package.version.clone()),
    );
    package.insert(
        field("root", ctx),
        ConstValue::String(script_context.package.root.clone()),
    );
    package.insert(
        field("is_root", ctx),
        ConstValue::Bool(script_context.package.is_root),
    );

    let mut workspace = HashMap::new();
    workspace.insert(
        field("root", ctx),
        ConstValue::String(script_context.workspace.root.clone()),
    );
    workspace.insert(
        field("has_workspace", ctx),
        ConstValue::Bool(script_context.workspace.has_workspace),
    );

    let mut target = HashMap::new();
    target.insert(
        field("os", ctx),
        ConstValue::Enum {
            tag: script_context.target.os.tag(),
            payload: None,
        },
    );
    target.insert(
        field("arch", ctx),
        ConstValue::String(script_context.target.arch.clone()),
    );
    target.insert(
        field("vendor", ctx),
        ConstValue::String(script_context.target.vendor.clone()),
    );
    target.insert(
        field("env", ctx),
        ConstValue::String(script_context.target.env.clone()),
    );

    let mut profile = HashMap::new();
    profile.insert(
        field("name", ctx),
        ConstValue::String(script_context.profile.name.clone()),
    );
    profile.insert(
        field("opt", ctx),
        ConstValue::Int(i128::from(script_context.profile.opt)),
    );
    profile.insert(
        field("debug", ctx),
        ConstValue::Bool(script_context.profile.debug),
    );

    let mut plan = HashMap::new();
    plan.insert(field("package", ctx), ConstValue::Struct(package));
    plan.insert(field("workspace", ctx), ConstValue::Struct(workspace));
    plan.insert(field("target", ctx), ConstValue::Struct(target));
    plan.insert(field("profile", ctx), ConstValue::Struct(profile));
    plan.insert(
        field("command", ctx),
        ConstValue::Enum {
            tag: script_context.command.tag(),
            payload: None,
        },
    );

    ConstValue::Struct(plan)
}

fn build_argument_value(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_context: &BuildScriptContext,
) -> ConstValue {
    fn field(name: &str, ctx: &mut kernc_sema::SemaContext<'_>) -> SymbolId {
        ctx.intern(name)
    }

    let mut unit = HashMap::new();
    unit.insert(
        field("kind", ctx),
        ConstValue::Enum {
            tag: target_kind_tag(script_context.unit.target_kind),
            payload: None,
        },
    );
    unit.insert(
        field("name", ctx),
        match &script_context.unit.target_name {
            Some(name) => ConstValue::Enum {
                tag: 0,
                payload: Some(Box::new(ConstValue::String(name.clone()))),
            },
            None => ConstValue::Enum {
                tag: 1,
                payload: None,
            },
        },
    );
    unit.insert(
        field("source_root", ctx),
        ConstValue::String(script_context.unit.source_root.clone()),
    );
    unit.insert(
        field("artifact_name", ctx),
        ConstValue::String(script_context.unit.artifact_name.clone()),
    );

    let mut builder = match plan_argument_value(ctx, &script_context.script) {
        ConstValue::Struct(value) => value,
        _ => unreachable!("plan_argument_value must return a struct"),
    };
    builder.insert(field("unit", ctx), ConstValue::Struct(unit));

    ConstValue::Struct(builder)
}

impl ScriptCommand {
    fn tag(self) -> i128 {
        match self {
            Self::Check => 0,
            Self::Lock => 1,
            Self::Build => 2,
            Self::Run => 3,
            Self::Test => 4,
        }
    }
}

fn target_kind_tag(kind: TargetKind) -> i128 {
    match kind {
        TargetKind::Lib => 0,
        TargetKind::Bin => 1,
        TargetKind::Test => 2,
        TargetKind::Example => 3,
    }
}

impl ScriptOs {
    fn tag(self) -> i128 {
        match self {
            Self::Unknown => 0,
            Self::Linux => 1,
            Self::Windows => 2,
            Self::Darwin => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_build_script, validate_kraft_script};
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

    #[test]
    fn accepts_public_build_entry() {
        let root = temp_dir("build-script-valid");
        let path = root.join("build.kr");
        fs::write(
            &path,
            "use kraft.builder;\npub fn build(b: *mut builder.Builder) void { let _ = b; }\n",
        )
        .unwrap();

        let result = validate_build_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }
}
