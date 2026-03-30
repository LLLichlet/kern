use crate::build_plan::{ActionPlan, BuildPlan, BuildUnit, CompileAction, LinkAction};
use crate::elaborate::{self, FeatureSelection};
use crate::error::{Error, Result};
use crate::graph::PackageId;
use crate::manifest::Manifest;
use crate::resolver::{ExternalPackageId, ResolvedExternalPackage, ResolvedGraph};
use crate::source;
use crate::workspace;
use kernc_driver::CompilerDriver;
use kernc_utils::config::{CompileOptions, DriverMode, LinkProfile};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSummary {
    pub compile_actions: usize,
    pub link_actions: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    pub executed: usize,
}

pub fn build(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<ExecutionSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Build)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BuiltExternalPackage {
    lib_source_path: PathBuf,
    link_objects: Vec<PathBuf>,
    module_aliases: BTreeMap<String, PathBuf>,
}

#[derive(Debug)]
struct SourceConfigContext {
    manifest_path: PathBuf,
    manifest: Manifest,
}

fn build_with_command(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    command: crate::script::ScriptCommand,
) -> Result<ExecutionSummary> {
    let source_config = load_source_config(build_plan)?;
    let mut built_external_packages = BTreeMap::new();
    let mut external_build_stack = BTreeSet::new();
    let mut external_summary = ExecutionSummary {
        compile_actions: 0,
        link_actions: 0,
    };

    for dep in requested_external_dependencies(action_plan) {
        build_external_package(
            &source_config,
            &build_plan.workspace_root,
            &dep,
            command,
            &mut built_external_packages,
            &mut external_build_stack,
            &mut external_summary,
        )?;
    }

    let local_library_actions = local_library_actions(&action_plan.compile_actions);
    execute_compile_actions(
        &action_plan.compile_actions,
        &local_library_actions,
        &built_external_packages,
    )?;

    for action in &action_plan.link_actions {
        ensure_parent_dir(&action.artifact_path)?;

        let mut options = CompileOptions {
            output_file: action.artifact_path.to_string_lossy().to_string(),
            driver_mode: DriverMode::LinkOnly,
            link_profile: LinkProfile::Hosted,
            use_std: true,
            ..CompileOptions::default()
        };
        apply_host_linker_env(&mut options);
        options.linker_inputs = link_inputs_for_action(
            action,
            action_plan,
            &local_library_actions,
            &built_external_packages,
        )?
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
        options.linker_libraries = action.link.system_libs.clone();
        options.linker_search_paths = action.link.search_paths.clone();
        options.linker_args = action.link.args.clone();
        for framework in &action.link.frameworks {
            options.linker_args.push("-framework".to_string());
            options.linker_args.push(framework.clone());
        }

        let driver = CompilerDriver::new(options);
        if !driver.compile() {
            return Err(Error::Execution(format!(
                "link failed for `{}`",
                action.artifact_path.display()
            )));
        }
    }

    Ok(ExecutionSummary {
        compile_actions: external_summary.compile_actions + action_plan.compile_count(),
        link_actions: external_summary.link_actions + action_plan.link_count(),
    })
}

pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Run)?;
    let action = find_link_action(action_plan, unit)?;
    let status = Command::new(&action.artifact_path)
        .current_dir(&build_plan.workspace_root)
        .status()
        .map_err(Error::from_io_plain)?;
    if !status.success() {
        return Err(Error::Execution(format!(
            "`{}` exited with status {}",
            action.artifact_path.display(),
            status
        )));
    }

    Ok(RunSummary {
        executable: action.artifact_path.clone(),
    })
}

pub fn test(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
) -> Result<TestSummary> {
    build_with_command(build_plan, action_plan, crate::script::ScriptCommand::Test)?;

    let mut executed = 0;
    for unit in units {
        let action = find_link_action(action_plan, unit)?;
        let status = Command::new(&action.artifact_path)
            .current_dir(&build_plan.workspace_root)
            .status()
            .map_err(Error::from_io_plain)?;
        if !status.success() {
            return Err(Error::Execution(format!(
                "test `{}` exited with status {}",
                action.artifact_path.display(),
                status
            )));
        }
        executed += 1;
    }

    Ok(TestSummary { executed })
}

fn find_link_action<'a>(action_plan: &'a ActionPlan, unit: &BuildUnit) -> Result<&'a LinkAction> {
    action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.package_id == unit.package_id
                && action.target_kind == unit.target_kind
                && action.target_name == unit.target_name
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing link action for `{}` target `{}`",
                unit.package_id.name, unit.artifact_name
            ))
        })
}

fn compile_time_defines(
    cfg: &std::collections::BTreeMap<String, crate::plan::PlanValue>,
    define: &std::collections::BTreeMap<String, crate::plan::PlanValue>,
    source_path: &Path,
) -> Result<HashMap<String, String>> {
    let mut values = HashMap::new();

    for (name, value) in cfg {
        values.insert(name.clone(), plan_value_string(value));
    }
    for (name, value) in define {
        let value = plan_value_string(value);
        if let Some(existing) = values.get(name) {
            if existing != &value {
                return Err(Error::Execution(format!(
                    "compile-time key `{name}` has conflicting cfg/define values for `{}`",
                    source_path.display()
                )));
            }
        }
        values.insert(name.clone(), value);
    }

    Ok(values)
}

fn plan_value_string(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
    }
    Ok(())
}

fn load_source_config(build_plan: &BuildPlan) -> Result<SourceConfigContext> {
    let manifest_path = build_plan.workspace_root.join("Kraft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    Ok(SourceConfigContext {
        manifest_path,
        manifest,
    })
}

fn execute_compile_actions(
    actions: &[CompileAction],
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<()> {
    for action in actions {
        ensure_parent_dir(&action.object_path)?;
        ensure_parent_dir(&action.artifact_path)?;

        let mut options = CompileOptions {
            input_file: Some(action.source_path.to_string_lossy().to_string()),
            output_file: action.object_path.to_string_lossy().to_string(),
            driver_mode: DriverMode::CompileOnly,
            use_std: true,
            link_profile: LinkProfile::Hosted,
            ..CompileOptions::default()
        };
        apply_host_linker_env(&mut options);
        inject_driver_condition_defines(&mut options);
        inject_std_alias(&mut options);
        options.module_aliases =
            compile_module_aliases(action, local_library_actions, built_external_packages)?;
        options.custom_defines.extend(compile_time_defines(
            &action.cfg,
            &action.define,
            action.source_path.as_path(),
        )?);

        let driver = CompilerDriver::new(options);
        if !driver.compile() {
            return Err(Error::Execution(format!(
                "compile failed for `{}`",
                action.source_path.display()
            )));
        }
    }

    Ok(())
}

fn compile_module_aliases(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<HashMap<String, String>> {
    let aliases = module_alias_paths(action, local_library_actions, built_external_packages)?;
    Ok(aliases
        .into_iter()
        .map(|(name, path)| (name, path.to_string_lossy().to_string()))
        .collect())
}

fn requested_external_dependencies(action_plan: &ActionPlan) -> Vec<ExternalPackageId> {
    let mut requested = BTreeSet::new();
    for action in &action_plan.compile_actions {
        requested.extend(action.external_dependencies.iter().cloned());
    }
    for action in &action_plan.link_actions {
        requested.extend(action.external_dependencies.iter().cloned());
    }
    requested.into_iter().collect()
}

fn build_external_package(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
    command: crate::script::ScriptCommand,
    built_external_packages: &mut BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    external_build_stack: &mut BTreeSet<ExternalPackageId>,
    external_summary: &mut ExecutionSummary,
) -> Result<()> {
    if built_external_packages.contains_key(dep) {
        return Ok(());
    }
    if !external_build_stack.insert(dep.clone()) {
        return Err(Error::Execution(format!(
            "cyclic external package build detected for `{}`",
            dep.package_name
        )));
    }

    let fetched = fetch_external_package(source_config, dependency_workspace_root, dep)?;
    let manifest_path = fetched.cache_path.join("Kraft.toml");
    let manifest = Manifest::load(&manifest_path)?;
    manifest.validate(&manifest_path)?;
    let workspace_members = workspace::load_members(&manifest_path, &manifest)?;
    let elaboration = elaborate::plan(
        &manifest_path,
        &manifest,
        &workspace_members,
        manifest.workspace.is_some(),
        command,
        &FeatureSelection::default(),
    )?;
    let build_plan = crate::build_plan::derive(&elaboration, command)?;
    let action_plan = build_plan.derive_actions(&crate::script::host_target());
    let all_local_library_actions = local_library_actions(&action_plan.compile_actions);
    let root_library_action = root_external_library_action(dep, &all_local_library_actions)?;
    let required_local_packages =
        required_local_packages(root_library_action, &all_local_library_actions);
    let required_library_actions = action_plan
        .compile_actions
        .iter()
        .filter(|action| {
            action.target_kind == crate::plan::TargetKind::Lib
                && required_local_packages.contains(&action.package_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    let required_external_dependencies =
        required_external_dependencies(root_library_action, &all_local_library_actions);
    for child in required_external_dependencies {
        build_external_package(
            source_config,
            &elaboration.resolved_graph.workspace_root,
            &child,
            command,
            built_external_packages,
            external_build_stack,
            external_summary,
        )?;
    }

    let required_local_library_actions = local_library_actions(&required_library_actions);
    execute_compile_actions(
        &required_library_actions,
        &required_local_library_actions,
        built_external_packages,
    )?;

    let root_library_action = root_external_library_action(dep, &required_local_library_actions)?;
    let mut module_aliases = BTreeMap::new();
    module_aliases.insert(
        dep.package_name.clone(),
        root_library_action.source_path.clone(),
    );
    module_aliases.extend(module_alias_paths(
        root_library_action,
        &required_local_library_actions,
        built_external_packages,
    )?);
    let link_objects = link_objects_for_compile_action(
        root_library_action,
        &required_local_library_actions,
        built_external_packages,
    )?;
    built_external_packages.insert(
        dep.clone(),
        BuiltExternalPackage {
            lib_source_path: root_library_action.source_path.clone(),
            link_objects,
            module_aliases,
        },
    );
    external_summary.compile_actions += required_library_actions.len();
    external_build_stack.remove(dep);
    Ok(())
}

fn fetch_external_package(
    source_config: &SourceConfigContext,
    dependency_workspace_root: &Path,
    dep: &ExternalPackageId,
) -> Result<source::FetchedPackage> {
    let resolved = ResolvedGraph {
        workspace_root: dependency_workspace_root.to_path_buf(),
        packages: Vec::new(),
        external_packages: vec![ResolvedExternalPackage { id: dep.clone() }],
    };
    let mut fetched = source::fetch_external_packages(
        &source_config.manifest_path,
        &source_config.manifest,
        &resolved,
    )?;
    fetched.pop().ok_or_else(|| {
        Error::Execution(format!(
            "failed to fetch external package `{}`",
            dep.package_name
        ))
    })
}

fn root_external_library_action<'a>(
    dep: &ExternalPackageId,
    local_library_actions: &'a BTreeMap<PackageId, CompileAction>,
) -> Result<&'a CompileAction> {
    local_library_actions
        .values()
        .find(|action| {
            action.package_id.name == dep.package_name
                && action.target_kind == crate::plan::TargetKind::Lib
                && match &dep.version {
                    Some(version) => action.package_id.version == *version,
                    None => true,
                }
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "external package `{}` does not expose a buildable lib target",
                dep.package_name
            ))
        })
}

fn local_library_actions(actions: &[CompileAction]) -> BTreeMap<PackageId, CompileAction> {
    actions
        .iter()
        .filter(|action| action.target_kind == crate::plan::TargetKind::Lib)
        .map(|action| (action.package_id.clone(), action.clone()))
        .collect()
}

fn required_local_packages(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
) -> BTreeSet<PackageId> {
    let mut required = BTreeSet::new();
    collect_local_packages(root_action, local_library_actions, &mut required);
    required
}

fn collect_local_packages(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    required: &mut BTreeSet<PackageId>,
) {
    if !required.insert(action.package_id.clone()) {
        return;
    }
    for dep in &action.local_dependencies {
        if let Some(dep_action) = local_library_actions.get(dep) {
            collect_local_packages(dep_action, local_library_actions, required);
        }
    }
}

fn required_external_dependencies(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
) -> BTreeSet<ExternalPackageId> {
    let mut required = BTreeSet::new();
    collect_external_dependencies(root_action, local_library_actions, &mut required);
    required
}

fn module_alias_paths(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<BTreeMap<String, PathBuf>> {
    let mut aliases = BTreeMap::new();
    let mut visited_local = BTreeSet::new();
    let mut visited_external = BTreeSet::new();
    collect_module_alias_paths(
        root_action,
        local_library_actions,
        built_external_packages,
        &mut visited_local,
        &mut visited_external,
        &mut aliases,
    )?;
    Ok(aliases)
}

fn collect_module_alias_paths(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    visited_local: &mut BTreeSet<PackageId>,
    visited_external: &mut BTreeSet<ExternalPackageId>,
    aliases: &mut BTreeMap<String, PathBuf>,
) -> Result<()> {
    for dep in &action.local_dependencies {
        let Some(dep_action) = local_library_actions.get(dep) else {
            continue;
        };
        if visited_local.insert(dep.clone()) {
            aliases.insert(dep.name.clone(), dep_action.source_path.clone());
            collect_module_alias_paths(
                dep_action,
                local_library_actions,
                built_external_packages,
                visited_local,
                visited_external,
                aliases,
            )?;
        }
    }

    for dep in &action.external_dependencies {
        if !visited_external.insert(dep.clone()) {
            continue;
        }
        let package = built_external_packages.get(dep).ok_or_else(|| {
            Error::Execution(format!(
                "missing built external package `{}`",
                dep.package_name
            ))
        })?;
        aliases.extend(
            package
                .module_aliases
                .iter()
                .map(|(name, path)| (name.clone(), path.clone())),
        );
    }

    Ok(())
}

fn collect_external_dependencies(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    required: &mut BTreeSet<ExternalPackageId>,
) {
    for dep in &action.external_dependencies {
        required.insert(dep.clone());
    }
    for dep in &action.local_dependencies {
        if let Some(dep_action) = local_library_actions.get(dep) {
            collect_external_dependencies(dep_action, local_library_actions, required);
        }
    }
}

fn link_inputs_for_action(
    link_action: &LinkAction,
    _action_plan: &ActionPlan,
    _local_library_actions: &BTreeMap<PackageId, CompileAction>,
    _built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<PathBuf>> {
    // Module aliases resolve dependencies as source inputs during compilation, so
    // executable/test/example link steps only need their primary object file.
    Ok(vec![link_action.primary_object.clone()])
}

fn link_objects_for_compile_action(
    root_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageId, CompileAction>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
) -> Result<Vec<PathBuf>> {
    let mut objects = Vec::new();
    let mut seen = BTreeSet::new();
    push_link_object(&mut objects, &mut seen, &root_action.object_path);

    for package_id in required_local_packages(root_action, local_library_actions) {
        if package_id == root_action.package_id {
            continue;
        }
        if let Some(action) = local_library_actions.get(&package_id) {
            push_link_object(&mut objects, &mut seen, &action.object_path);
        }
    }
    for dep in required_external_dependencies(root_action, local_library_actions) {
        let package = built_external_packages.get(&dep).ok_or_else(|| {
            Error::Execution(format!(
                "missing built external package `{}`",
                dep.package_name
            ))
        })?;
        for object in &package.link_objects {
            push_link_object(&mut objects, &mut seen, object);
        }
    }

    Ok(objects)
}

fn push_link_object(objects: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, path: &Path) {
    if seen.insert(path.to_path_buf()) {
        objects.push(path.to_path_buf());
    }
}

fn apply_host_linker_env(options: &mut CompileOptions) {
    if let Ok(cc_env) = std::env::var("CC") {
        options.linker_cmd = cc_env;
    }
}

fn inject_driver_condition_defines(options: &mut CompileOptions) {
    let link_profile = match options.link_profile {
        LinkProfile::Kern => "kern",
        LinkProfile::Freestanding => "freestanding",
        LinkProfile::Hosted => "hosted",
        LinkProfile::None => "none",
    };

    let hosted = matches!(options.link_profile, LinkProfile::Hosted);
    let kern_rt = options.use_std && !hosted;

    options
        .custom_defines
        .insert("link_profile".to_string(), link_profile.to_string());
    options
        .custom_defines
        .insert("hosted".to_string(), hosted.to_string());
    options
        .custom_defines
        .insert("libc".to_string(), hosted.to_string());
    options
        .custom_defines
        .insert("kern_rt".to_string(), kern_rt.to_string());
}

fn inject_std_alias(options: &mut CompileOptions) {
    if !options.use_std || options.module_aliases.contains_key("std") {
        return;
    }
    let std_path = resolve_std_path();
    options
        .module_aliases
        .insert("std".to_string(), std_path.to_string_lossy().to_string());
}

fn resolve_std_path() -> PathBuf {
    if let Ok(custom_std) = std::env::var("KERN_STD_PATH") {
        return PathBuf::from(custom_std);
    }

    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("library/std")
}

#[cfg(test)]
mod tests {
    use super::{build, run, test};
    use crate::build_plan;
    use crate::elaborate::{FeatureSelection, plan};
    use crate::manifest::Manifest;
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
    fn builds_and_runs_hosted_package_with_local_library_dependency() {
        let root = temp_dir("kraft-exec-run");
        let app_dir = root.join("app");
        let util_dir = root.join("util");
        fs::create_dir_all(&app_dir).unwrap();
        fs::create_dir_all(&util_dir).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[workspace]
members = ["app", "util"]
"#,
        )
        .unwrap();
        fs::write(
            app_dir.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.kr"

[dependencies]
util = { path = "../util" }
"#,
        )
        .unwrap();
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("src/main.kr"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (util.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();

        fs::write(
            util_dir.join("Kraft.toml"),
            r#"
[package]
name = "util"
version = "0.1.0"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();
        fs::create_dir_all(util_dir.join("src")).unwrap();
        fs::write(
            util_dir.join("src/lib.kr"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let members = crate::workspace::load_members(&manifest_path, &manifest).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &members,
            true,
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan
            .packages
            .iter()
            .find(|package| package.package_id.name == "app")
            .unwrap()
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_executes_test_units() {
        let root = temp_dir("kraft-exec-test");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[test]]
name = "smoke"
root = "tests/smoke.kr"
"#,
        )
        .unwrap();
        fs::write(
            root.join("tests/smoke.kr"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    return 0;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Test,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Test).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let test_units = build_plan.packages[0]
            .units
            .iter()
            .filter(|unit| unit.target_kind == crate::plan::TargetKind::Test)
            .collect::<Vec<_>>();

        let summary = test(&build_plan, &action_plan, &test_units).unwrap();
        assert_eq!(summary.executed, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_compile_and_link_actions() {
        let root = temp_dir("kraft-exec-build");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "demo"
root = "src/main.kr"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.kr"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    return 0;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let summary = build(&build_plan, &action_plan).unwrap();
        assert_eq!(summary.compile_actions, 1);
        assert_eq!(summary.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_package_with_direct_external_registry_dependency() {
        let root = temp_dir("kraft-exec-external-direct");
        let registry_root = root.join("vendor-registry");
        let log_root = registry_root.join("log").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.kr"

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.kr"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (log.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("Kraft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("src/lib.kr"),
            r#"
pub fn answer() i32 {
    return 42;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Build,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Build).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());

        let summary = build(&build_plan, &action_plan).unwrap();
        assert_eq!(summary.compile_actions, 2);
        assert_eq!(summary.link_actions, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn builds_and_runs_hosted_package_with_transitive_external_registry_dependency() {
        let root = temp_dir("kraft-exec-external-transitive");
        let registry_root = root.join("vendor-registry");
        let log_root = registry_root.join("log").join("1");
        let corelog_root = registry_root.join("corelog").join("1");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(log_root.join("src")).unwrap();
        fs::create_dir_all(corelog_root.join("src")).unwrap();

        fs::write(
            root.join("Kraft.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
kern = "0.7"

[[bin]]
name = "app"
root = "src/main.kr"

[dependencies]
log = "1"

[source.default]
directory = "vendor-registry"
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/main.kr"),
            r#"
extern fn main(args: [][]u8) i32 {
    let _ = args;
    if (log.answer() == 42) {
        return 0;
    }
    return 1;
}
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("Kraft.toml"),
            r#"
[package]
name = "log"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.kr"

[dependencies]
corelog = "1"
"#,
        )
        .unwrap();
        fs::write(
            log_root.join("src/lib.kr"),
            r#"
pub fn answer() i32 {
    return corelog.base() + 1;
}
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("Kraft.toml"),
            r#"
[package]
name = "corelog"
version = "1"
kern = "0.7"

[lib]
root = "src/lib.kr"
"#,
        )
        .unwrap();
        fs::write(
            corelog_root.join("src/lib.kr"),
            r#"
pub fn base() i32 {
    return 41;
}
"#,
        )
        .unwrap();

        let manifest_path = root.join("Kraft.toml");
        let manifest = Manifest::load(&manifest_path).unwrap();
        let elaboration = plan(
            &manifest_path,
            &manifest,
            &[],
            false,
            crate::script::ScriptCommand::Run,
            &FeatureSelection::default(),
        )
        .unwrap();
        let build_plan =
            build_plan::derive(&elaboration, crate::script::ScriptCommand::Run).unwrap();
        let action_plan = build_plan.derive_actions(&crate::script::host_target());
        let unit = build_plan.packages[0]
            .units
            .iter()
            .find(|unit| unit.target_kind == crate::plan::TargetKind::Bin)
            .unwrap();

        let summary = run(&build_plan, &action_plan, unit).unwrap();
        assert!(summary.executable.is_file());

        let _ = fs::remove_dir_all(root);
    }
}
