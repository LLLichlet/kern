use crate::build_plan::{ActionPlan, BuildPlan, BuildUnit, LinkAction};
use crate::error::{Error, Result};
use kernc_driver::CompilerDriver;
use kernc_utils::config::{CompileOptions, DriverMode, LinkProfile};
use std::collections::HashMap;
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
    let local_aliases = local_dependency_aliases(action_plan);
    ensure_supported_external_dependencies(action_plan)?;

    for action in &action_plan.compile_actions {
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
            compile_module_aliases(action.package_id.name.as_str(), &local_aliases, build_plan);
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
        options.linker_inputs =
            std::iter::once(action.primary_object.to_string_lossy().to_string())
                .chain(
                    action
                        .local_library_objects
                        .iter()
                        .map(|path| path.to_string_lossy().to_string()),
                )
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
        compile_actions: action_plan.compile_count(),
        link_actions: action_plan.link_count(),
    })
}

pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    build(build_plan, action_plan)?;
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
    build(build_plan, action_plan)?;

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

fn ensure_supported_external_dependencies(action_plan: &ActionPlan) -> Result<()> {
    let unsupported = action_plan
        .link_actions
        .iter()
        .flat_map(|action| &action.external_dependencies)
        .map(|dep| dep.package_name.clone())
        .collect::<std::collections::BTreeSet<_>>();

    if unsupported.is_empty() {
        Ok(())
    } else {
        Err(Error::Execution(format!(
            "external package execution is not implemented yet; unresolved build inputs: {}",
            unsupported.into_iter().collect::<Vec<_>>().join(", ")
        )))
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| Error::from_io(parent, err))?;
    }
    Ok(())
}

fn local_dependency_aliases(action_plan: &ActionPlan) -> HashMap<String, PathBuf> {
    action_plan
        .compile_actions
        .iter()
        .filter(|action| action.target_kind == crate::plan::TargetKind::Lib)
        .map(|action| (action.package_id.name.clone(), action.source_path.clone()))
        .collect()
}

fn compile_module_aliases(
    current_package: &str,
    local_aliases: &HashMap<String, PathBuf>,
    build_plan: &BuildPlan,
) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for package in &build_plan.packages {
        for unit in &package.units {
            if unit.package_id.name != current_package {
                continue;
            }
            for dep in &unit.local_dependencies {
                if let Some(path) = local_aliases.get(&dep.name) {
                    aliases.insert(dep.name.clone(), path.to_string_lossy().to_string());
                }
            }
        }
    }
    aliases
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
}
