mod analysis;
mod build_host;

use self::analysis::{
    BUILD_SCRIPT_ENTRY, CRAFT_SCRIPT_ENTRY, PreparedScript, prepare_script, validate_script,
};
use self::build_host::{BuildUnitHost, LinkArgPathFields, build_argument_value};
use crate::build_plan::{BuildUnit, StagedAction};
use crate::error::{Error, Result};
use crate::graph::BuildDomain;
use crate::graph::DependencyKind;
use crate::graph::PackageId;
use crate::manifest::Manifest;
use crate::plan::{PackagePlan, TargetKind};
use crate::resolver::ExternalPackageId;
use kernc_sema::checker::{ConstEvaluator, ConstValue, ScriptHost};
use kernc_utils::config::{CodeModel, CompileOptions, LtoMode};
use kernc_utils::{Session, Span, SymbolId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const DEV_DEFAULT_OPT_LEVEL: u8 = 0;
const RELEASE_DEFAULT_OPT_LEVEL: u8 = 3;
const RELEASE_DEFAULT_MAX_CODEGEN_UNITS: usize = 4;

const OPTION_SOME_TAG: i128 = 0;
const OPTION_NONE_TAG: i128 = 1;

const DEPENDENCY_KIND_NORMAL_TAG: i128 = 0;
const DEPENDENCY_KIND_DEV_TAG: i128 = 1;
const DEPENDENCY_KIND_BUILD_TAG: i128 = 2;

const SCRIPT_COMMAND_CHECK_TAG: i128 = 0;
const SCRIPT_COMMAND_FETCH_TAG: i128 = 1;
const SCRIPT_COMMAND_BUILD_TAG: i128 = 2;
const SCRIPT_COMMAND_RUN_TAG: i128 = 3;
const SCRIPT_COMMAND_TEST_TAG: i128 = 4;

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
pub struct CraftScriptContext {
    pub package: ScriptPackage,
    pub workspace: ScriptWorkspace,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildScriptContext {
    pub script: ScriptContext,
    pub unit: BuildScriptUnit,
    pub paths: BuildScriptPaths,
    pub tools: BTreeMap<String, Vec<BuildScriptTool>>,
    pub resources: BTreeMap<String, BuildScriptResource>,
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
pub struct BuildScriptResource {
    pub root_path: String,
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
    pub codegen_units: usize,
    pub lto_mode: LtoMode,
    pub code_model: CodeModel,
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
    let resolved_opt = profile
        .and_then(|profile| profile.opt)
        .unwrap_or(default_opt);

    ScriptProfile {
        name: selection.name().to_string(),
        opt: resolved_opt,
        debug: profile
            .and_then(|profile| profile.debug)
            .unwrap_or(default_debug),
        codegen_units: profile
            .and_then(|profile| profile.codegen_units)
            .unwrap_or_else(|| {
                default_profile_codegen_units(
                    selection,
                    resolved_opt,
                    std::thread::available_parallelism()
                        .map(|count| count.get())
                        .unwrap_or(1),
                )
            }),
        lto_mode: profile
            .and_then(|profile| profile.lto.as_deref())
            .map(LtoMode::parse)
            .transpose()
            .expect("manifest profile LTO should already be validated")
            .unwrap_or_else(|| default_profile_lto_mode(selection, resolved_opt)),
        code_model: profile
            .and_then(|profile| profile.code_model.as_deref())
            .map(CodeModel::parse)
            .transpose()
            .expect("manifest profile code model should already be validated")
            .unwrap_or_default(),
    }
}

fn default_profile_codegen_units(
    selection: ProfileSelection,
    opt: u8,
    available_parallelism: usize,
) -> usize {
    match selection {
        ProfileSelection::Dev => 1,
        ProfileSelection::Release if opt >= 2 => {
            available_parallelism.clamp(1, RELEASE_DEFAULT_MAX_CODEGEN_UNITS)
        }
        ProfileSelection::Release => 1,
    }
}

fn default_profile_lto_mode(selection: ProfileSelection, _opt: u8) -> LtoMode {
    match selection {
        ProfileSelection::Dev | ProfileSelection::Release => LtoMode::None,
    }
}

pub fn validate_craft_script(path: &Path) -> Result<()> {
    validate_script(path, CRAFT_SCRIPT_ENTRY)
}

pub fn validate_build_script(path: &Path) -> Result<()> {
    validate_script(path, BUILD_SCRIPT_ENTRY)
}

pub fn apply_craft_script(
    path: &Path,
    package_plan: &mut PackagePlan,
    script_context: &CraftScriptContext,
) -> Result<()> {
    let mut session = Session::new();
    let PreparedScript {
        script_path,
        mut ctx,
        entry_def,
    } = prepare_script(path, &mut session, CRAFT_SCRIPT_ENTRY)?;

    let mut host = PackagePlanHost { package_plan };
    let arg_values = vec![craft_plan_argument_value(&mut ctx, script_context)];
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

    Ok(())
}

pub fn apply_build_script(
    path: &Path,
    build_nodes: &mut Vec<StagedAction>,
    unit: &mut BuildUnit,
    script_context: &BuildScriptContext,
) -> Result<()> {
    let mut session = Session::new();
    let PreparedScript {
        script_path,
        mut ctx,
        entry_def,
    } = prepare_script(path, &mut session, BUILD_SCRIPT_ENTRY)?;

    let link_arg_path_fields = LinkArgPathFields {
        flag: ctx.intern("flag"),
        path: ctx.intern("path"),
    };
    let mut host = BuildUnitHost::new(build_nodes, unit, script_context, link_arg_path_fields);
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

fn pure_enum_value(tag: i128) -> ConstValue {
    ConstValue::Int(tag)
}

fn craft_plan_argument_value(
    ctx: &mut kernc_sema::SemaContext<'_>,
    script_context: &CraftScriptContext,
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

    let mut plan = HashMap::new();
    plan.insert(field("package", ctx), ConstValue::Struct(package));
    plan.insert(field("workspace", ctx), ConstValue::Struct(workspace));
    ConstValue::Struct(plan)
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
    profile.insert(
        field("codegen_units", ctx),
        ConstValue::Int(script_context.profile.codegen_units as i128),
    );
    profile.insert(
        field("lto", ctx),
        ConstValue::String(script_context.profile.lto_mode.as_str().to_string()),
    );
    profile.insert(
        field("code_model", ctx),
        ConstValue::String(script_context.profile.code_model.as_str().to_string()),
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

impl ScriptCommand {
    fn tag(self) -> i128 {
        match self {
            Self::Check => SCRIPT_COMMAND_CHECK_TAG,
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
    use super::{
        Manifest, ProfileSelection, default_profile_codegen_units, default_profile_lto_mode,
        manifest_profile, validate_build_script, validate_craft_script,
    };
    use kernc_utils::config::{CodeModel, LtoMode};
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
        let path = root.join("craft.kn");
        fs::write(
            &path,
            "use craft.plan;\npub fn craft(p: &mut plan.Plan) void { let _ = p; }\n",
        )
        .unwrap();

        let result = validate_craft_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_public_craft_entry() {
        let root = temp_dir("craft-script-missing-entry");
        let path = root.join("craft.kn");
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
        let path = root.join("craft.kn");
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
        let path = root.join("build.kn");
        fs::write(
            &path,
            "use craft.builder;\npub fn build(b: &mut builder.Builder) void { let _ = b; }\n",
        )
        .unwrap();

        let result = validate_build_script(&path);
        assert!(result.is_ok(), "unexpected result: {result:?}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn release_profile_defaults_to_capped_parallel_codegen_units() {
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Release, 3, 1),
            1
        );
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Release, 3, 2),
            2
        );
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Release, 3, 8),
            4
        );
    }

    #[test]
    fn release_profile_keeps_single_codegen_unit_for_low_opt_levels() {
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Release, 0, 8),
            1
        );
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Release, 1, 8),
            1
        );
    }

    #[test]
    fn dev_profile_defaults_to_single_codegen_unit() {
        assert_eq!(
            default_profile_codegen_units(ProfileSelection::Dev, 3, 8),
            1
        );
    }

    #[test]
    fn release_profile_defaults_to_no_lto_even_for_optimized_builds() {
        assert_eq!(
            default_profile_lto_mode(ProfileSelection::Release, 2),
            LtoMode::None
        );
        assert_eq!(
            default_profile_lto_mode(ProfileSelection::Release, 3),
            LtoMode::None
        );
    }

    #[test]
    fn low_opt_release_and_dev_profiles_default_to_no_lto() {
        assert_eq!(
            default_profile_lto_mode(ProfileSelection::Release, 1),
            LtoMode::None
        );
        assert_eq!(
            default_profile_lto_mode(ProfileSelection::Dev, 3),
            LtoMode::None
        );
    }

    #[test]
    fn manifest_profile_uses_default_release_codegen_units_when_unspecified() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.6"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let profile = manifest_profile(&manifest, ProfileSelection::Release);
        let expected = default_profile_codegen_units(
            ProfileSelection::Release,
            profile.opt,
            std::thread::available_parallelism()
                .map(|count| count.get())
                .unwrap_or(1),
        );
        assert_eq!(profile.codegen_units, expected);
    }

    #[test]
    fn manifest_profile_preserves_explicit_codegen_units() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.6"

[profile.release]
codegen-units = 7
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let profile = manifest_profile(&manifest, ProfileSelection::Release);
        assert_eq!(profile.codegen_units, 7);
    }

    #[test]
    fn manifest_profile_preserves_explicit_lto_mode() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.6"

[profile.release]
lto = "full"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let profile = manifest_profile(&manifest, ProfileSelection::Release);
        assert_eq!(profile.lto_mode, LtoMode::Full);
    }

    #[test]
    fn manifest_profile_preserves_explicit_code_model() {
        let manifest = Manifest::parse(
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.6"

[profile.release]
code-model = "kernel"
"#,
            std::path::Path::new("Craft.toml"),
        )
        .unwrap();

        let profile = manifest_profile(&manifest, ProfileSelection::Release);
        assert_eq!(profile.code_model, CodeModel::Kernel);
    }
}
