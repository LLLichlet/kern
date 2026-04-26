use super::build_with_command;
use crate::build_plan::{ActionPlan, BuildPlan, BuildUnit, LinkAction};
use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(test)]
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub executable: PathBuf,
    pub build: super::ExecutionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    pub executed: usize,
    pub build: super::ExecutionSummary,
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn run(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
) -> Result<RunSummary> {
    let build = build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Run,
        None,
    )?;
    run_built(build_plan, action_plan, unit, build, &[])
}

pub fn run_built(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    unit: &BuildUnit,
    build: super::ExecutionSummary,
    args: &[String],
) -> Result<RunSummary> {
    let action = find_link_action(action_plan, unit)?;
    let executable_path = resolve_invocation_path(&action.artifact_path)?;
    let status = runtime_command(&executable_path, action, &build_plan.workspace_root, args)
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
        build,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn test(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
) -> Result<TestSummary> {
    let build = build_with_command(
        build_plan,
        action_plan,
        crate::script::ScriptCommand::Test,
        None,
    )?;
    test_built(build_plan, action_plan, units, build, &[])
}

pub fn test_built(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
    units: &[&BuildUnit],
    build: super::ExecutionSummary,
    args: &[String],
) -> Result<TestSummary> {
    let mut executed = 0;
    for unit in units {
        let action = find_link_action(action_plan, unit)?;
        let executable_path = resolve_invocation_path(&action.artifact_path)?;
        let status = runtime_command(&executable_path, action, &build_plan.workspace_root, args)
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

    Ok(TestSummary { executed, build })
}

fn runtime_command(
    executable_path: &Path,
    action: &LinkAction,
    workspace_root: &Path,
    args: &[String],
) -> Command {
    let mut command = Command::new(executable_path);
    command.args(args);
    command.current_dir(&action.package_root_path);
    command.env("CRAFT_WORKSPACE_ROOT", workspace_root);
    command.env("CRAFT_PACKAGE_ROOT", &action.package_root_path);
    configure_runtime_stdio_for_tests(&mut command);
    command
}

#[cfg(test)]
fn configure_runtime_stdio_for_tests(command: &mut Command) {
    if std::env::var_os("CRAFT_TEST_INHERIT_RUNTIME_OUTPUT").is_none() {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
}

#[cfg(not(test))]
fn configure_runtime_stdio_for_tests(_command: &mut Command) {}

fn find_link_action<'a>(action_plan: &'a ActionPlan, unit: &BuildUnit) -> Result<&'a LinkAction> {
    action_plan
        .link_actions
        .iter()
        .find(|action| {
            action.domain == unit.domain
                && action.package_id == unit.package_id
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

fn resolve_invocation_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .map_err(Error::from_io_plain)?
        .join(path))
}
