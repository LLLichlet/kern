//! Runtime execution helpers for `craft run` and `craft test`.
//!
//! Runtime actions locate the selected executable artifact, apply package/test
//! environment variables, and execute the process while preserving structured
//! stdout, stderr, and exit status summaries.

use super::build_with_command;
use crate::build_plan::{ActionPlan, BuildPlan, BuildUnit, LinkAction};
use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Stdio;
use std::process::{Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub executable: PathBuf,
    pub build: super::ExecutionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSummary {
    pub executed: usize,
    pub failures: Vec<TestFailure>,
    pub build: super::ExecutionSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    pub label: String,
    pub status: ExitStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCase {
    index: usize,
    name: String,
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
        false,
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
        false,
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
    let mut failures = Vec::new();
    for unit in units {
        let action = find_link_action(action_plan, unit)?;
        let executable_path = resolve_invocation_path(&action.artifact_path)?;
        let test_name = unit.target_name.as_deref().unwrap_or(&unit.artifact_name);
        let compile_action = find_compile_action(action_plan, unit)?;
        let cases = read_test_cases(compile_action.test_metadata_path.as_deref())?;

        for case in cases {
            let tmp_dir = create_test_tmp_dir(unit, &case.name)?;
            let status = test_runtime_command(
                &executable_path,
                action,
                &build_plan.workspace_root,
                test_name,
                &case,
                &tmp_dir,
                args,
            )
            .status()
            .map_err(Error::from_io_plain)?;
            let _ = fs::remove_dir_all(&tmp_dir);
            if !status.success() {
                failures.push(TestFailure {
                    label: test_case_label(unit, action, &case),
                    status,
                });
            }
            executed += 1;
        }
    }

    Ok(TestSummary {
        executed,
        failures,
        build,
    })
}

fn test_case_label(unit: &BuildUnit, action: &LinkAction, case: &TestCase) -> String {
    let name = unit.target_name.as_deref().unwrap_or(&unit.artifact_name);
    format!(
        "{} {} `{}` case `{}` ({})",
        unit.package_id.name,
        unit.target_kind.as_str(),
        name,
        case.name,
        action.artifact_path.display()
    )
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

fn test_runtime_command(
    executable_path: &Path,
    action: &LinkAction,
    workspace_root: &Path,
    test_name: &str,
    case: &TestCase,
    tmp_dir: &Path,
    args: &[String],
) -> Command {
    let mut runtime_args = Vec::with_capacity(args.len() + 3);
    runtime_args.push("--kern-test-case".to_string());
    runtime_args.push(case.index.to_string());
    runtime_args.extend(args.iter().cloned());
    let mut command = runtime_command(executable_path, action, workspace_root, &runtime_args);
    command.env("CRAFT_TEST_NAME", test_name);
    command.env("CRAFT_TEST_CASE", &case.name);
    command.env("CRAFT_TEST_TMPDIR", tmp_dir);
    command
}

fn read_test_cases(path: Option<&Path>) -> Result<Vec<TestCase>> {
    let Some(path) = path else {
        return Err(Error::Execution(
            "missing test case metadata path for test target".to_string(),
        ));
    };
    let contents = fs::read_to_string(path).map_err(|err| Error::from_io(path, err))?;
    let mut cases = Vec::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line == "version=1" || line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix("case=") else {
            return Err(Error::Execution(format!(
                "test case metadata `{}` contains unsupported line {}: `{}`",
                path.display(),
                line_no + 1,
                line
            )));
        };
        let Some((index, name)) = rest.split_once('\t') else {
            return Err(Error::Execution(format!(
                "test case metadata `{}` contains malformed case line {}",
                path.display(),
                line_no + 1
            )));
        };
        let index = index.parse::<usize>().map_err(|_| {
            Error::Execution(format!(
                "test case metadata `{}` contains invalid case index `{}` on line {}",
                path.display(),
                index,
                line_no + 1
            ))
        })?;
        if name.is_empty() {
            return Err(Error::Execution(format!(
                "test case metadata `{}` contains empty case name on line {}",
                path.display(),
                line_no + 1
            )));
        }
        cases.push(TestCase {
            index,
            name: name.to_string(),
        });
    }
    Ok(cases)
}

fn create_test_tmp_dir(unit: &BuildUnit, test_name: &str) -> Result<PathBuf> {
    let root = std::env::temp_dir().join("craft-test");
    fs::create_dir_all(&root).map_err(Error::from_io_plain)?;
    for attempt in 0..100 {
        let path = root.join(format!(
            "{}-{}-{}-{}",
            sanitize_tmp_component(&unit.package_id.name),
            sanitize_tmp_component(test_name),
            std::process::id(),
            unique_nanos().saturating_add(attempt)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(Error::from_io_plain(err)),
        }
    }
    Err(Error::Execution(format!(
        "failed to create temporary directory for test target `{test_name}`"
    )))
}

fn sanitize_tmp_component(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' {
            out.push(byte as char);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "test".to_string()
    } else {
        out
    }
}

fn unique_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
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

fn find_compile_action<'a>(
    action_plan: &'a ActionPlan,
    unit: &BuildUnit,
) -> Result<&'a crate::build_plan::CompileAction> {
    action_plan
        .compile_actions
        .iter()
        .find(|action| {
            action.domain == unit.domain
                && action.package_id == unit.package_id
                && action.target_kind == unit.target_kind
                && action.target_name == unit.target_name
        })
        .ok_or_else(|| {
            Error::Execution(format!(
                "missing compile action for `{}` target `{}`",
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
