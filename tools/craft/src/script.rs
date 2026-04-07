use crate::build_plan::{
    BuildUnit, GeneratedFile, GeneratedFileOrigin, SourceRootBinding, StagedAction,
    StagedActionKind, StagedActionPhase,
};
use crate::error::{Error, Result};
use crate::graph::BuildDomain;
use crate::graph::DependencyKind;
use crate::graph::PackageId;
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::resolver::ExternalPackageId;
use kernc_driver::CompilerDriver;
use kernc_sema::checker::{ConstEvaluator, ConstValue, ScriptHost};
use kernc_sema::def::{Def, Visibility};
use kernc_sema::ty::{TypeId, TypeKind};
use kernc_utils::Session;
use kernc_utils::config::CompileOptions;
use kernc_utils::{Span, SymbolId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

const DEV_DEFAULT_OPT_LEVEL: u8 = 0;
const RELEASE_DEFAULT_OPT_LEVEL: u8 = 3;

const OPTION_SOME_TAG: i128 = 0;
const OPTION_NONE_TAG: i128 = 1;

const DEPENDENCY_KIND_NORMAL_TAG: i128 = 0;
const DEPENDENCY_KIND_DEV_TAG: i128 = 1;
const DEPENDENCY_KIND_BUILD_TAG: i128 = 2;

const SCRIPT_COMMAND_CHECK_TAG: i128 = 0;
const SCRIPT_COMMAND_LOCK_TAG: i128 = 1;
const SCRIPT_COMMAND_FETCH_TAG: i128 = 2;
const SCRIPT_COMMAND_BUILD_TAG: i128 = 3;
const SCRIPT_COMMAND_RUN_TAG: i128 = 4;
const SCRIPT_COMMAND_TEST_TAG: i128 = 5;

const TARGET_KIND_LIB_TAG: i128 = 0;
const TARGET_KIND_BIN_TAG: i128 = 1;
const TARGET_KIND_TEST_TAG: i128 = 2;
const TARGET_KIND_EXAMPLE_TAG: i128 = 3;

const BUILD_DOMAIN_HOST_TAG: i128 = 0;
const BUILD_DOMAIN_TARGET_TAG: i128 = 1;

const SCRIPT_OS_UNKNOWN_TAG: i128 = 0;
const SCRIPT_OS_LINUX_TAG: i128 = 1;
const SCRIPT_OS_WINDOWS_TAG: i128 = 2;
const SCRIPT_OS_DARWIN_TAG: i128 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptExecution {
    pub env_inputs: Vec<ScriptEnvInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptEnvInput {
    pub name: String,
    pub value: Option<String>,
}

fn option_some(value: ConstValue) -> ConstValue {
    ConstValue::Enum {
        tag: OPTION_SOME_TAG,
        payload: Some(Box::new(value)),
    }
}

fn option_none() -> ConstValue {
    ConstValue::Enum {
        tag: OPTION_NONE_TAG,
        payload: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptContext {
    pub package: ScriptPackage,
    pub workspace: ScriptWorkspace,
    pub host: ScriptTarget,
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
    pub paths: BuildScriptPaths,
    pub tools: BTreeMap<String, Vec<BuildScriptTool>>,
    pub package_root_path: PathBuf,
    pub workspace_root_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildScriptToolOrigin {
    LocalPackage {
        package_id: PackageId,
    },
    ExternalPackage {
        dependency_id: ExternalPackageId,
        package_id: PackageId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptTool {
    pub target_name: String,
    pub executable_path: String,
    pub origin: BuildScriptToolOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptUnit {
    pub domain: BuildDomain,
    pub target_kind: TargetKind,
    pub target_name: Option<String>,
    pub source_root: String,
    pub artifact_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptPaths {
    pub build_root: String,
    pub generated_root: String,
    pub artifact_root: String,
    pub object_path: String,
    pub artifact_path: String,
    pub metadata_path: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfileSelection {
    #[default]
    Dev,
    Release,
}

impl ProfileSelection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptCommand {
    Check,
    Lock,
    Fetch,
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

pub fn manifest_profile(manifest: &Manifest, selection: ProfileSelection) -> ScriptProfile {
    let profile = manifest
        .profile
        .as_ref()
        .and_then(|profiles| match selection {
            ProfileSelection::Dev => profiles.dev.as_ref(),
            ProfileSelection::Release => profiles.release.as_ref(),
        });

    let (default_opt, default_debug) = match selection {
        ProfileSelection::Dev => (DEV_DEFAULT_OPT_LEVEL, true),
        ProfileSelection::Release => (RELEASE_DEFAULT_OPT_LEVEL, false),
    };

    ScriptProfile {
        name: selection.name().to_string(),
        opt: profile
            .and_then(|profile| profile.opt)
            .unwrap_or(default_opt),
        debug: profile
            .and_then(|profile| profile.debug)
            .unwrap_or(default_debug),
    }
}

pub fn validate_craft_script(path: &Path) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "craft")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "craft", "craft.rn")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "craft.rn",
        "craft",
        "*mut plan.Plan",
    )?;

    Ok(())
}

pub fn validate_build_script(path: &Path) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "build")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "build", "build.rn")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "build.rn",
        "build",
        "*mut builder.Builder",
    )?;

    Ok(())
}

pub fn apply_craft_script(
    path: &Path,
    package_plan: &mut PackagePlan,
    script_context: &ScriptContext,
) -> Result<ScriptExecution> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "craft")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "craft", "craft.rn")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "craft.rn",
        "craft",
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
                .map(|diag| format!("craft script execution failed: {}", diag.message))
                .unwrap_or_else(|| "craft script execution failed".to_string()),
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
    build_nodes: &mut Vec<StagedAction>,
    unit: &mut BuildUnit,
    script_context: &BuildScriptContext,
) -> Result<()> {
    let script_path = canonical_script_path(path)?;
    let script_input = script_path.to_string_lossy().to_string();
    let mut session = Session::new();
    let mut ctx = analyze_script(&script_path, &script_input, &mut session, "build")?;
    let entry_def = find_script_entry(&mut ctx, &script_path, "build", "build.rn")?;
    validate_script_entry(
        &mut ctx,
        entry_def,
        &script_path,
        "build.rn",
        "build",
        "*mut builder.Builder",
    )?;

    let mut host = BuildUnitHost {
        build_nodes,
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

fn normalized_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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
                script_name.trim_end_matches(".rn")
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
    build_nodes: &'a mut Vec<StagedAction>,
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
            "__craft_plan_feature_enabled" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(
                    self.script_context.features.contains(feature.as_str()),
                ))
            }
            "__craft_plan_env" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let name = expect_string(args, 1, "environment name")?;
                let Some(value) = self.script_context.env.get(&name).cloned() else {
                    return Err(format!(
                        "environment `{name}` was not declared under `[craft].env` (declared: {})",
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
                    Some(value) => option_some(ConstValue::String(value)),
                    None => option_none(),
                })
            }
            "__craft_plan_cfg_bool" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_bool(args, 2, "cfg value")?;
                self.package_plan
                    .set_cfg_bool(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_cfg_string" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_string(args, 2, "cfg value")?;
                self.package_plan
                    .set_cfg_string(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_define_bool" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_bool(args, 2, "define value")?;
                self.package_plan
                    .set_define_bool(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_define_string" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_string(args, 2, "define value")?;
                self.package_plan
                    .set_define_string(&key, value)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_set_lib_root" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let root = expect_string(args, 1, "lib root")?;
                self.package_plan
                    .set_lib_root(root)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_add_bin" => self.add_named_target(args, TargetKind::Bin, "bin"),
            "__craft_plan_add_test" => self.add_test_target(args),
            "__craft_plan_add_example" => {
                self.add_named_target(args, TargetKind::Example, "example")
            }
            "__craft_plan_remove_lib" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                Ok(ConstValue::Bool(
                    self.package_plan.remove_target(TargetKind::Lib, None),
                ))
            }
            "__craft_plan_remove_bin" => self.remove_named_target(args, TargetKind::Bin, "bin"),
            "__craft_plan_remove_test" => self.remove_test_target(args),
            "__craft_plan_remove_example" => {
                self.remove_named_target(args, TargetKind::Example, "example")
            }
            "__craft_plan_dep_version" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let version = expect_string(args, 3, "dependency version")?;
                self.package_plan
                    .set_dependency_version(kind, &name, version)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_dep_path" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let path = expect_string(args, 3, "dependency path")?;
                self.package_plan
                    .set_dependency_path(kind, &name, path)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_dep_git" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                let git = expect_string(args, 3, "dependency git")?;
                self.package_plan
                    .set_dependency_git(kind, &name, git)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_dep_workspace" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                self.package_plan
                    .use_workspace_dependency(kind, &name)
                    .map_err(|err| err.to_string())?;
                Ok(ConstValue::Void)
            }
            "__craft_plan_remove_dep" => {
                let _ = expect_arg(args, 0, "plan receiver")?;
                let kind = expect_dependency_kind(args, 1, "dependency kind")?;
                let name = expect_string(args, 2, "dependency name")?;
                Ok(ConstValue::Bool(
                    self.package_plan
                        .remove_dependency(kind, &name)
                        .map_err(|err| err.to_string())?,
                ))
            }
            _ => Err(format!("unsupported craft host function `{name}`")),
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

    fn add_test_target(&mut self, args: &[ConstValue]) -> std::result::Result<ConstValue, String> {
        let _ = expect_arg(args, 0, "plan receiver")?;
        let root = expect_string(args, 1, "test root")?;
        self.package_plan
            .add_test_target(root)
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

    fn remove_test_target(
        &mut self,
        args: &[ConstValue],
    ) -> std::result::Result<ConstValue, String> {
        let _ = expect_arg(args, 0, "plan receiver")?;
        let root = expect_string(args, 1, "test root")?;
        Ok(ConstValue::Bool(
            self.package_plan.remove_test_target(&root),
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
            "__craft_build_feature_enabled" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let feature = expect_string(args, 1, "feature name")?;
                Ok(ConstValue::Bool(
                    self.script_context
                        .script
                        .features
                        .contains(feature.as_str()),
                ))
            }
            "__craft_build_tool_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                Ok(ConstValue::String(tool.executable_path.clone()))
            }
            "__craft_build_output_path" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                Ok(ConstValue::String(output.path))
            }
            "__craft_build_link_system_lib" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "system library name")?;
                push_unique(&mut self.unit.link.system_libs, name);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_framework" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let name = expect_string(args, 1, "framework name")?;
                push_unique(&mut self.unit.link.frameworks, name);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_search" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let path = expect_string(args, 1, "link search path")?;
                push_unique(&mut self.unit.link.search_paths, path);
                Ok(ConstValue::Void)
            }
            "__craft_build_link_arg" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let arg = expect_string(args, 1, "link argument")?;
                self.unit.link.args.push(arg);
                Ok(ConstValue::Void)
            }
            "__craft_build_cfg_bool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_bool(args, 2, "cfg value")?;
                self.unit
                    .cfg
                    .insert(key, crate::plan::PlanValue::Bool(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_cfg_string" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "cfg name")?;
                let value = expect_string(args, 2, "cfg value")?;
                self.unit
                    .cfg
                    .insert(key, crate::plan::PlanValue::String(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_define_bool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_bool(args, 2, "define value")?;
                self.unit
                    .define
                    .insert(key, crate::plan::PlanValue::Bool(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_define_string" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let key = expect_string(args, 1, "define name")?;
                let value = expect_string(args, 2, "define value")?;
                self.unit
                    .define
                    .insert(key, crate::plan::PlanValue::String(value));
                Ok(ConstValue::Void)
            }
            "__craft_build_set_source_root" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let path = expect_string(args, 1, "source root")?;
                self.unit.source_root = source_root_binding_from_script_path(&path)?;
                Ok(ConstValue::Void)
            }
            "__craft_build_set_source_root_from" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                self.unit.source_root = SourceRootBinding::BuildOutput {
                    id: output.id,
                    path: normalize_path_display(&output.path),
                };
                Ok(ConstValue::Void)
            }
            "__craft_build_stage_generated" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let relative_path = expect_string(args, 1, "generated relative path")?;
                let contents = expect_string(args, 2, "generated file contents")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &relative_path,
                )?;
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Emitted,
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::WriteFile { contents },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_file" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source path")?;
                let generated_relative = expect_string(args, 2, "generated relative path")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_file() {
                    return Err(format!(
                        "package source file `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Copied {
                        source: source.clone(),
                    },
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::CopyFile { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_output" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_output = expect_output(args, 1, "source output")?;
                let generated_relative = expect_string(args, 2, "generated relative path")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                let source_display = relative_display(
                    &self.script_context.workspace_root_path,
                    Path::new(&source_output.path),
                );
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Copied {
                        source: source_display,
                    },
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::CopyFile {
                        source: source_output.path,
                    },
                );
                add_staged_dependency(self.build_nodes, output.id, source_output.id)?;
                Ok(output_value(&output))
            }
            "__craft_build_stage_generated_from_tool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let generated_relative = expect_string(args, 3, "generated relative path")?;
                let args = expect_string_list(args, 4, "tool arguments")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.generated_root),
                    &generated_relative,
                )?;
                record_generated_file(
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    GeneratedFileOrigin::Emitted,
                );
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PreCompile,
                    StagedActionKind::RunTool {
                        tool: Box::new(tool.clone()),
                        args,
                    },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let relative_path = expect_string(args, 1, "artifact relative path")?;
                let contents = expect_string(args, 2, "artifact file contents")?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &relative_path,
                )?;
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::WriteFile { contents },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_artifact_file_from_tool" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let dependency_name = expect_string(args, 1, "tool dependency name")?;
                let tool_name = expect_string(args, 2, "tool target name")?;
                let artifact_relative = expect_string(args, 3, "artifact relative path")?;
                let args = expect_string_list(args, 4, "tool arguments")?;
                let tool = resolve_build_tool(self.script_context, &dependency_name, &tool_name)?;
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::RunTool {
                        tool: Box::new(tool.clone()),
                        args,
                    },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_file_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source path")?;
                let artifact_relative = expect_string(args, 2, "artifact relative path")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_file() {
                    return Err(format!(
                        "package source file `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyFile { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_stage_copy_package_dir_to_artifact" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let source_relative = expect_string(args, 1, "package relative source dir")?;
                let artifact_relative = expect_string(args, 2, "artifact relative dir")?;
                let source_path =
                    package_input_path(&self.script_context.package_root_path, &source_relative)?;
                if !source_path.is_dir() {
                    return Err(format!(
                        "package source directory `{}` does not exist",
                        source_path.display()
                    ));
                }
                let dest_path = generated_output_path(
                    Path::new(&self.script_context.paths.artifact_root),
                    &artifact_relative,
                )?;
                let source =
                    relative_display(&self.script_context.workspace_root_path, &source_path);
                let output = record_staged_action(
                    self.build_nodes,
                    self.unit,
                    &self.script_context.workspace_root_path,
                    &dest_path,
                    StagedActionPhase::PostLink,
                    StagedActionKind::CopyDirectory { source },
                );
                Ok(output_value(&output))
            }
            "__craft_build_depend" => {
                let _ = expect_arg(args, 0, "builder receiver")?;
                let output = expect_output(args, 1, "output")?;
                let dependency = expect_output(args, 2, "dependency")?;
                add_staged_dependency(self.build_nodes, output.id, dependency.id)?;
                Ok(ConstValue::Void)
            }
            _ => Err(format!("unsupported build host function `{name}`")),
        }
    }
}

fn generated_output_path(root: &Path, relative_path: &str) -> std::result::Result<PathBuf, String> {
    Ok(root.join(normalize_relative_path(
        relative_path,
        "generated relative path",
    )?))
}

fn source_root_binding_from_script_path(
    path: &str,
) -> std::result::Result<SourceRootBinding, String> {
    if path.trim().is_empty() {
        return Err("source root must not be empty".to_string());
    }
    if Path::new(path).is_absolute() {
        return Ok(SourceRootBinding::AbsolutePath(normalize_path_display(
            path,
        )));
    }
    Ok(SourceRootBinding::PackagePath(normalize_relative_display(
        path,
        "source root",
    )?))
}

fn package_input_path(root: &Path, relative_path: &str) -> std::result::Result<PathBuf, String> {
    Ok(root.join(normalize_relative_path(
        relative_path,
        "package relative source path",
    )?))
}

fn normalize_relative_path(
    relative_path: &str,
    label: &str,
) -> std::result::Result<PathBuf, String> {
    if relative_path.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }

    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(format!("{label} must not be absolute"));
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("{label} must not contain `..`"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{label} must stay within its declared root"));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(format!("{label} must not be empty"));
    }

    Ok(normalized)
}

fn normalize_relative_display(
    relative_path: &str,
    label: &str,
) -> std::result::Result<String, String> {
    Ok(normalize_relative_path(relative_path, label)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn normalize_path_display(path: &str) -> String {
    path.replace('\\', "/")
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

fn record_generated_file(
    unit: &mut BuildUnit,
    workspace_root: &Path,
    path: &Path,
    origin: GeneratedFileOrigin,
) {
    let path = relative_display(workspace_root, path);
    if let Some(existing) = unit
        .generated_files
        .iter_mut()
        .find(|entry| entry.path == path)
    {
        existing.origin = origin;
        return;
    }
    unit.generated_files.push(GeneratedFile { path, origin });
}

fn record_staged_action(
    build_nodes: &mut Vec<StagedAction>,
    unit: &mut BuildUnit,
    workspace_root: &Path,
    path: &Path,
    phase: StagedActionPhase,
    kind: StagedActionKind,
) -> BuildOutput {
    let output = relative_display(workspace_root, path);
    let node_ids = unit_bound_node_ids(unit, phase);
    if let Some(existing_id) = node_ids.iter().copied().find(|id| {
        build_nodes
            .iter()
            .any(|action| action.id == *id && action.phase == phase && action.output == output)
    }) {
        let existing = build_nodes
            .iter_mut()
            .find(|action| action.id == existing_id)
            .expect("build node id must exist");
        existing.kind = kind;
        return BuildOutput {
            id: existing_id,
            path: normalized_path_string(path),
        };
    }
    let id = next_staged_action_id(build_nodes);
    build_nodes.push(StagedAction {
        id,
        phase,
        output,
        depends_on: Vec::new(),
        kind,
    });
    unit_bound_node_ids_mut(unit, phase).push(id);
    BuildOutput {
        id,
        path: normalized_path_string(path),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildOutput {
    id: usize,
    path: String,
}

fn next_staged_action_id(build_nodes: &[StagedAction]) -> usize {
    build_nodes
        .iter()
        .map(|action| action.id)
        .max()
        .unwrap_or(0)
        + 1
}

fn add_staged_dependency(
    build_nodes: &mut [StagedAction],
    output_id: usize,
    dependency_id: usize,
) -> std::result::Result<(), String> {
    if output_id == dependency_id {
        return Err("build outputs cannot depend on themselves".to_string());
    }

    let output_phase = build_nodes
        .iter()
        .find(|action| action.id == output_id)
        .map(|action| action.phase)
        .ok_or_else(|| format!("unknown build output id `{output_id}`"))?;
    let dependency_phase = build_nodes
        .iter()
        .find(|action| action.id == dependency_id)
        .map(|action| action.phase)
        .ok_or_else(|| format!("unknown build output id `{dependency_id}`"))?;
    if output_phase != dependency_phase {
        return Err("build output dependencies must stay within a single stage phase".to_string());
    }

    let action = build_nodes
        .iter_mut()
        .find(|action| action.id == output_id)
        .expect("staged action must exist after phase lookup");
    if !action.depends_on.contains(&dependency_id) {
        action.depends_on.push(dependency_id);
    }
    Ok(())
}

fn unit_bound_node_ids(unit: &BuildUnit, phase: StagedActionPhase) -> &[usize] {
    match phase {
        StagedActionPhase::PreCompile => &unit.build.compile_inputs,
        StagedActionPhase::PostLink => &unit.build.artifact_outputs,
    }
}

fn unit_bound_node_ids_mut(unit: &mut BuildUnit, phase: StagedActionPhase) -> &mut Vec<usize> {
    match phase {
        StagedActionPhase::PreCompile => &mut unit.build.compile_inputs,
        StagedActionPhase::PostLink => &mut unit.build.artifact_outputs,
    }
}

fn output_value(output: &BuildOutput) -> ConstValue {
    ConstValue::String(format!("{}|{}", output.id, output.path))
}

fn resolve_build_tool<'a>(
    script_context: &'a BuildScriptContext,
    dependency_name: &str,
    tool_name: &str,
) -> std::result::Result<&'a BuildScriptTool, String> {
    let Some(tools) = script_context.tools.get(dependency_name) else {
        return Err(format!(
            "build dependency `{dependency_name}` does not expose a host tool"
        ));
    };
    tools.iter()
        .find(|tool| tool.target_name == tool_name)
        .ok_or_else(|| {
            format!(
                "build dependency `{dependency_name}` does not expose host tool `{tool_name}` (available: {})",
                tools.iter()
                    .map(|tool| tool.target_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn expect_arg<'a>(
    args: &'a [ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<&'a ConstValue, String> {
    args.get(index)
        .ok_or_else(|| format!("missing craft host argument `{label}`"))
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

fn expect_string_list(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<Vec<String>, String> {
    match expect_arg(args, index, label)? {
        ConstValue::Array(values) => values
            .iter()
            .map(|value| match value {
                ConstValue::String(value) => Ok(value.clone()),
                _ => Err(format!("expected every `{label}` entry to be a string")),
            })
            .collect(),
        _ => Err(format!("expected `{label}` to be an array of strings")),
    }
}

fn expect_output(
    args: &[ConstValue],
    index: usize,
    label: &str,
) -> std::result::Result<BuildOutput, String> {
    let value = expect_string(args, index, label)?;
    let Some((id, path)) = value.split_once('|') else {
        return Err(format!("expected `{label}` to be a build output handle"));
    };
    let id = id
        .parse::<usize>()
        .map_err(|_| format!("expected `{label}` to carry a numeric build output id"))?;
    if path.is_empty() {
        return Err(format!("expected `{label}` to carry a build output path"));
    }
    Ok(BuildOutput {
        id,
        path: path.to_string(),
    })
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
        DEPENDENCY_KIND_NORMAL_TAG => Ok(DependencyKind::Normal),
        DEPENDENCY_KIND_DEV_TAG => Ok(DependencyKind::Dev),
        DEPENDENCY_KIND_BUILD_TAG => Ok(DependencyKind::Build),
        _ => Err(format!("invalid `{label}` value `{tag}`")),
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn pure_enum_value(tag: i128) -> ConstValue {
    ConstValue::Int(tag)
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
    plan.insert(
        field("host", ctx),
        ConstValue::Struct(target_value(ctx, &script_context.host)),
    );
    plan.insert(
        field("target", ctx),
        ConstValue::Struct(target_value(ctx, &script_context.target)),
    );
    plan.insert(field("profile", ctx), ConstValue::Struct(profile));
    plan.insert(
        field("command", ctx),
        pure_enum_value(script_context.command.tag()),
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
        field("domain", ctx),
        pure_enum_value(build_domain_tag(script_context.unit.domain)),
    );
    unit.insert(
        field("kind", ctx),
        pure_enum_value(target_kind_tag(script_context.unit.target_kind)),
    );
    unit.insert(
        field("name", ctx),
        match &script_context.unit.target_name {
            Some(name) => option_some(ConstValue::String(name.clone())),
            None => option_none(),
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
    let mut paths = HashMap::new();
    paths.insert(
        field("build_root", ctx),
        ConstValue::String(script_context.paths.build_root.clone()),
    );
    paths.insert(
        field("generated_root", ctx),
        ConstValue::String(script_context.paths.generated_root.clone()),
    );
    paths.insert(
        field("artifact_root", ctx),
        ConstValue::String(script_context.paths.artifact_root.clone()),
    );
    paths.insert(
        field("object", ctx),
        ConstValue::String(script_context.paths.object_path.clone()),
    );
    paths.insert(
        field("artifact", ctx),
        ConstValue::String(script_context.paths.artifact_path.clone()),
    );
    paths.insert(
        field("metadata", ctx),
        match &script_context.paths.metadata_path {
            Some(path) => option_some(ConstValue::String(path.clone())),
            None => option_none(),
        },
    );
    builder.insert(field("paths", ctx), ConstValue::Struct(paths));

    ConstValue::Struct(builder)
}

impl ScriptCommand {
    fn tag(self) -> i128 {
        match self {
            Self::Check => SCRIPT_COMMAND_CHECK_TAG,
            Self::Lock => SCRIPT_COMMAND_LOCK_TAG,
            Self::Fetch => SCRIPT_COMMAND_FETCH_TAG,
            Self::Build => SCRIPT_COMMAND_BUILD_TAG,
            Self::Run => SCRIPT_COMMAND_RUN_TAG,
            Self::Test => SCRIPT_COMMAND_TEST_TAG,
        }
    }
}

fn target_kind_tag(kind: TargetKind) -> i128 {
    match kind {
        TargetKind::Lib => TARGET_KIND_LIB_TAG,
        TargetKind::Bin => TARGET_KIND_BIN_TAG,
        TargetKind::Test => TARGET_KIND_TEST_TAG,
        TargetKind::Example => TARGET_KIND_EXAMPLE_TAG,
    }
}

fn build_domain_tag(domain: BuildDomain) -> i128 {
    match domain {
        BuildDomain::Host => BUILD_DOMAIN_HOST_TAG,
        BuildDomain::Target => BUILD_DOMAIN_TARGET_TAG,
    }
}

fn target_value(
    ctx: &mut kernc_sema::SemaContext<'_>,
    target: &ScriptTarget,
) -> HashMap<SymbolId, ConstValue> {
    fn field(name: &str, ctx: &mut kernc_sema::SemaContext<'_>) -> SymbolId {
        ctx.intern(name)
    }

    let mut value = HashMap::new();
    value.insert(field("os", ctx), pure_enum_value(target.os.tag()));
    value.insert(field("arch", ctx), ConstValue::String(target.arch.clone()));
    value.insert(
        field("vendor", ctx),
        ConstValue::String(target.vendor.clone()),
    );
    value.insert(field("env", ctx), ConstValue::String(target.env.clone()));
    value
}

impl ScriptOs {
    fn tag(self) -> i128 {
        match self {
            Self::Unknown => SCRIPT_OS_UNKNOWN_TAG,
            Self::Linux => SCRIPT_OS_LINUX_TAG,
            Self::Windows => SCRIPT_OS_WINDOWS_TAG,
            Self::Darwin => SCRIPT_OS_DARWIN_TAG,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_build_script, validate_craft_script};
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
    fn accepts_public_craft_entry() {
        let root = temp_dir("craft-script-valid");
        let path = root.join("craft.rn");
        fs::write(
            &path,
            "use craft.plan;\npub fn craft(p: *mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let result = validate_craft_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_public_craft_entry() {
        let root = temp_dir("craft-script-missing-entry");
        let path = root.join("craft.rn");
        fs::write(&path, "fn helper() void {}\n").unwrap();

        let err = validate_craft_script(&path).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("missing required entry"),
            "unexpected error: {message}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_entry_without_plan_parameter() {
        let root = temp_dir("craft-script-missing-plan-param");
        let path = root.join("craft.rn");
        fs::write(&path, "pub fn craft() void {}\n").unwrap();

        let err = validate_craft_script(&path).unwrap_err();
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
        let path = root.join("build.rn");
        fs::write(
            &path,
            "use craft.builder;\npub fn build(b: *mut builder.Builder) void { let _ = b; }\n",
        )
        .unwrap();

        let result = validate_build_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }
}
