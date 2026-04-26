use super::{
    ExecutionPhase, ExecutionSession, ensure_link_action_built, ensure_parent_dir,
    prepare_output_path,
};
use crate::build_plan::{CompileAction, LinkAction, StagedAction, StagedActionKind};
use crate::build_state;
use crate::error::{Error, Result};
use crate::graph::BuildDomain;
use crate::operation_lock::OutputOperationLock;
use kernc_driver::CompilerDriver;
use kernc_utils::config::{CompileOptions, DriverMode, OptLevel};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use super::external::ensure_external_tool_built;
use super::fingerprint::build_fingerprint;

pub(super) fn execute_staged_actions(
    root_ids: &[usize],
    build_nodes: &[StagedAction],
    required_path: Option<&Path>,
    session: &mut ExecutionSession<'_>,
) -> Result<()> {
    let action_index = build_nodes
        .iter()
        .map(|action| (action.id, action))
        .collect::<BTreeMap<_, _>>();
    let mut active = BTreeSet::new();
    for root_id in root_ids {
        let action = action_index
            .get(root_id)
            .ok_or_else(|| Error::Execution(format!("missing build node `{root_id}`")))?;
        execute_staged_action(action, &action_index, &mut active, session)?;
    }
    if let Some(required_path) = required_path
        && !root_ids.is_empty()
        && !required_path.is_file()
    {
        return Err(Error::Execution(format!(
            "staged actions did not materialize source `{}`",
            required_path.display()
        )));
    }
    Ok(())
}

fn format_captured_child_stream(label: &str, bytes: &[u8]) -> Option<String> {
    const MAX_LEN: usize = 8192;

    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let rendered = if trimmed.len() > MAX_LEN {
        format!("{}\n...<truncated>...", &trimmed[..MAX_LEN])
    } else {
        trimmed.to_string()
    };

    Some(format!("{label}:\n{rendered}"))
}

fn format_run_tool_failure(
    tool_path: &Path,
    status: ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> String {
    let mut message = format!(
        "tool `{}` exited with status {}",
        tool_path.display(),
        status
    );
    if let Some(stderr_text) = format_captured_child_stream("stderr", stderr) {
        message.push('\n');
        message.push_str(&stderr_text);
    }
    if let Some(stdout_text) = format_captured_child_stream("stdout", stdout) {
        message.push('\n');
        message.push_str(&stdout_text);
    }
    message
}

pub(super) fn compile_progress_label(action: &CompileAction) -> String {
    format!(
        "{}:{} {}",
        action.package_id.name,
        action.artifact_name,
        format_progress_tags(action.target_kind.as_str(), action.domain)
    )
}

pub(super) fn link_progress_label(action: &LinkAction) -> String {
    format!(
        "{}:{} {}",
        action.package_id.name,
        action.artifact_name,
        format_progress_tags(action.target_kind.as_str(), action.domain)
    )
}

fn format_progress_tags(target_kind: &str, domain: BuildDomain) -> String {
    match domain {
        BuildDomain::Target => format!("[{target_kind}]"),
        BuildDomain::Host => format!("[{target_kind},host]"),
    }
}

fn stage_action_label(action: &StagedAction, output_path: &Path) -> String {
    let output = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&action.output);
    match &action.kind {
        StagedActionKind::WriteFile { .. } => format!("write {output}"),
        StagedActionKind::CcCompile { source, .. } => {
            let input = Path::new(source)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source);
            format!("cc {input} -> {output}")
        }
        StagedActionKind::RunTool { tool, .. } => {
            let tool_name = Path::new(&tool.executable_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&tool.executable_path);
            format!("run-tool {tool_name} -> {output}")
        }
        StagedActionKind::CopyFile { source } => {
            let input = Path::new(source)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source);
            format!("copy {input} -> {output}")
        }
        StagedActionKind::CopyDirectory { source } => {
            let input = Path::new(source)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(source);
            format!("copy-dir {input} -> {output}")
        }
    }
}

fn execute_staged_action(
    action: &StagedAction,
    action_index: &BTreeMap<usize, &StagedAction>,
    active: &mut BTreeSet<usize>,
    session: &mut ExecutionSession<'_>,
) -> Result<bool> {
    let output_path = PathBuf::from(&action.output);
    if let Some(progress) = &session.state.progress {
        progress.set_phase(ExecutionPhase::Stage);
        progress.set_detail(stage_action_label(action, &output_path));
    }
    if session.state.staged_outputs.contains(&output_path) {
        return Ok(false);
    }
    if !active.insert(action.id) {
        return Err(Error::Execution(format!(
            "cyclic staged action dependency detected at `{}`",
            action.output
        )));
    }
    for dependency_id in &action.depends_on {
        let dependency = action_index.get(dependency_id).ok_or_else(|| {
            Error::Execution(format!(
                "missing staged action dependency `{dependency_id}` for `{}`",
                action.output
            ))
        })?;
        execute_staged_action(dependency, action_index, active, session)?;
    }
    active.remove(&action.id);
    let _staged_lock = OutputOperationLock::acquire(&output_path, "staged-action")?;
    let toolchain_digest = build_state::current_process_digest()?;
    let mut input_paths = action
        .depends_on
        .iter()
        .map(|dependency_id| {
            action_index
                .get(dependency_id)
                .map(|dependency| PathBuf::from(&dependency.output))
                .ok_or_else(|| {
                    Error::Execution(format!(
                        "missing build node `{dependency_id}` while hashing `{}`",
                        action.output
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    let fingerprint = match &action.kind {
        StagedActionKind::WriteFile { contents } => build_fingerprint(&[
            "kind=write".to_string(),
            format!("output={}", output_path.display()),
            format!("contents={}", build_state::hash_string(contents)),
        ]),
        StagedActionKind::CcCompile {
            source,
            include_dirs,
            defines,
            args,
            opt,
            debug,
        } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            input_paths.extend(
                include_dirs
                    .iter()
                    .map(PathBuf::from)
                    .filter(|path| path.is_dir() && !output_path.starts_with(path)),
            );
            let mut lines = vec![
                "kind=cc-compile".to_string(),
                format!("toolchain={toolchain_digest}"),
                format!("source={}", input_path.display()),
                format!("output={}", output_path.display()),
                format!("opt={opt}"),
                format!("debug={debug}"),
            ];
            lines.extend(
                include_dirs
                    .iter()
                    .map(|path| format!("include_dir={path}")),
            );
            lines.extend(defines.iter().map(|define| format!("define={define}")));
            lines.extend(args.iter().map(|arg| format!("arg={arg}")));
            build_fingerprint(&lines)
        }
        StagedActionKind::RunTool { tool, args } => {
            let tool_path = PathBuf::from(&tool.executable_path);
            match &tool.origin {
                crate::script::BuildScriptToolOrigin::LocalPackage { .. } => {
                    if let Some(link_action) = session.indexes.link_action_index.get(&tool_path) {
                        ensure_link_action_built(link_action, session)?;
                    }
                }
                crate::script::BuildScriptToolOrigin::ExternalPackage { .. } => {
                    ensure_external_tool_built(
                        tool,
                        session.config,
                        &mut session.external,
                        session.state.execution_summary,
                    )?;
                }
            }
            input_paths.push(tool_path.clone());
            let mut lines = vec![
                "kind=run-tool".to_string(),
                format!("toolchain={toolchain_digest}"),
                format!("tool={}", tool_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(
                input_paths
                    .iter()
                    .map(|path| format!("dep={}", path.display())),
            );
            lines.extend(args.iter().map(|arg| format!("arg={arg}")));
            build_fingerprint(&lines)
        }
        StagedActionKind::CopyFile { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            let mut lines = vec![
                "kind=copy-file".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(
                action
                    .depends_on
                    .iter()
                    .map(|dependency_id| format!("dep={dependency_id}")),
            );
            build_fingerprint(&lines)
        }
        StagedActionKind::CopyDirectory { source } => {
            let input_path = PathBuf::from(source);
            input_paths.push(input_path.clone());
            let mut lines = vec![
                "kind=copy-dir".to_string(),
                format!("input={}", input_path.display()),
                format!("output={}", output_path.display()),
            ];
            lines.extend(
                action
                    .depends_on
                    .iter()
                    .map(|dependency_id| format!("dep={dependency_id}")),
            );
            build_fingerprint(&lines)
        }
    };

    if build_state::action_state_is_current(&output_path, &fingerprint)? {
        session.state.execution_summary.record_staged_cache_hit();
        session.state.staged_outputs.insert(output_path);
        if let Some(progress) = &session.state.progress {
            progress.record_staged_action();
        }
        return Ok(false);
    }

    ensure_parent_dir(&output_path)?;

    match &action.kind {
        StagedActionKind::WriteFile { contents } => {
            prepare_output_path(&output_path, false)?;
            fs::write(&output_path, contents).map_err(|err| Error::from_io(&output_path, err))?;
        }
        StagedActionKind::CcCompile {
            source,
            include_dirs,
            defines,
            args,
            opt,
            debug,
        } => {
            prepare_output_path(&output_path, false)?;
            let mut options = CompileOptions {
                input_file: Some(source.clone()),
                output_file: output_path.to_string_lossy().to_string(),
                driver_mode: DriverMode::CcCompile,
                opt_level: cc_opt_level(*opt),
                debug_info: *debug,
                cc_args: cc_compile_args(include_dirs, defines, args),
                report_progress: false,
                ..CompileOptions::default()
            };
            super::options::apply_host_linker_env(&mut options);
            if CompilerDriver::new(options).compile_with_report().is_none() {
                return Err(Error::Execution(format!(
                    "C compile failed for `{}`",
                    source
                )));
            }
        }
        StagedActionKind::RunTool { tool, args } => {
            prepare_output_path(&output_path, false)?;
            let tool_path = PathBuf::from(&tool.executable_path);
            let output = Command::new(&tool_path)
                .args(args)
                .output()
                .map_err(Error::from_io_plain)?;
            if !output.status.success() {
                return Err(Error::Execution(format_run_tool_failure(
                    &tool_path,
                    output.status,
                    &output.stdout,
                    &output.stderr,
                )));
            }
            fs::write(&output_path, output.stdout)
                .map_err(|err| Error::from_io(&output_path, err))?;
        }
        StagedActionKind::CopyFile { source } => {
            prepare_output_path(&output_path, false)?;
            let input_path = PathBuf::from(source);
            fs::copy(&input_path, &output_path).map_err(|err| {
                Error::Execution(format!(
                    "failed to copy staged input `{}` to `{}`: {err}",
                    input_path.display(),
                    output_path.display(),
                ))
            })?;
        }
        StagedActionKind::CopyDirectory { source } => {
            prepare_output_path(&output_path, true)?;
            let input_path = PathBuf::from(source);
            copy_dir_all(&input_path, &output_path).map_err(|err| {
                Error::Execution(format!(
                    "failed to copy staged directory `{}` to `{}`: {err}",
                    input_path.display(),
                    output_path.display(),
                ))
            })?;
        }
    }

    #[cfg(test)]
    crate::test_support::hit(crate::test_support::FAILPOINT_AFTER_STAGED_OUTPUT_WRITE);

    build_state::record_action_state(
        &output_path,
        fingerprint,
        &input_paths,
        std::slice::from_ref(&output_path),
    )?;
    session.state.execution_summary.record_staged_cache_miss();
    session.state.staged_outputs.insert(output_path);
    if let Some(progress) = &session.state.progress {
        progress.record_staged_action();
    }
    Ok(true)
}

fn cc_opt_level(opt: u8) -> OptLevel {
    match opt {
        0 => OptLevel::O0,
        1 => OptLevel::O1,
        2 => OptLevel::O2,
        _ => OptLevel::O3,
    }
}

fn cc_compile_args(include_dirs: &[String], defines: &[String], args: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(include_dirs.len() + defines.len() + args.len());
    result.extend(include_dirs.iter().map(|path| format!("-I{path}")));
    result.extend(defines.iter().map(|define| format!("-D{define}")));
    result.extend(args.iter().cloned());
    result
}

pub(super) fn cleanup_stale_artifact_outputs(
    action: &LinkAction,
    build_nodes: &[StagedAction],
) -> Result<()> {
    cleanup_stale_staged_root(
        &action.artifact_root_path,
        action.artifact_outputs.as_slice(),
        build_nodes,
        "artifact",
    )
}

pub(super) fn cleanup_stale_compile_inputs(
    action: &CompileAction,
    build_nodes: &[StagedAction],
) -> Result<()> {
    cleanup_stale_staged_root(
        &action.generated_root_path,
        action.compile_inputs.as_slice(),
        build_nodes,
        "generated",
    )
}

fn cleanup_stale_staged_root(
    root: &Path,
    root_ids: &[usize],
    build_nodes: &[StagedAction],
    label: &str,
) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }

    let action_index = build_nodes
        .iter()
        .map(|node| (node.id, node))
        .collect::<BTreeMap<_, _>>();
    let mut keep_files = BTreeSet::new();
    let mut keep_dirs = BTreeSet::new();
    let mut keep_subtrees = BTreeSet::new();
    keep_dirs.insert(root.to_path_buf());

    for root_id in root_ids {
        let node = action_index
            .get(root_id)
            .ok_or_else(|| Error::Execution(format!("missing build node `{root_id}`")))?;
        let output_path = PathBuf::from(&node.output);
        if !output_path.starts_with(root) {
            return Err(Error::Execution(format!(
                "{label} output `{}` escapes owned root `{}`",
                output_path.display(),
                root.display()
            )));
        }
        keep_files.insert(output_path.clone());
        keep_files.insert(build_state::action_state_path(&output_path));
        if matches!(node.kind, StagedActionKind::CopyDirectory { .. }) {
            keep_subtrees.insert(output_path.clone());
        }
        let mut current = output_path.parent();
        while let Some(path) = current {
            if !path.starts_with(root) {
                break;
            }
            keep_dirs.insert(path.to_path_buf());
            if path == root {
                break;
            }
            current = path.parent();
        }
    }

    cleanup_stale_artifact_tree(root, root, &keep_files, &keep_dirs, &keep_subtrees)
}

fn cleanup_stale_artifact_tree(
    root: &Path,
    dir: &Path,
    keep_files: &BTreeSet<PathBuf>,
    keep_dirs: &BTreeSet<PathBuf>,
    keep_subtrees: &BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|err| Error::from_io(dir, err))? {
        let entry = entry.map_err(|err| Error::from_io(dir, err))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| Error::from_io(&path, err))?;
        if file_type.is_dir() {
            if keep_subtrees.contains(&path) {
                continue;
            }
            cleanup_stale_artifact_tree(root, &path, keep_files, keep_dirs, keep_subtrees)?;
            if path != root && !keep_dirs.contains(&path) && path.exists() {
                fs::remove_dir_all(&path).map_err(|err| Error::from_io(&path, err))?;
            }
            continue;
        }

        if !keep_files.contains(&path) {
            fs::remove_file(&path).map_err(|err| Error::from_io(&path, err))?;
        }
    }

    Ok(())
}

fn copy_dir_all(source: &Path, dest: &Path) -> std::result::Result<(), String> {
    fs::create_dir_all(dest).map_err(|err| err.to_string())?;
    for entry in fs::read_dir(source).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let file_type = entry.file_type().map_err(|err| err.to_string())?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &dest_path).map_err(|err| err.to_string())?;
        } else {
            return Err(format!(
                "unsupported filesystem entry `{}` while copying directory tree",
                source_path.display()
            ));
        }
    }
    Ok(())
}
