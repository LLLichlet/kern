use super::{CompileReport, Result};
use crate::build_plan::{CompileAction, LinkAction};
use crate::build_state;
use kernc_utils::config::CompileOptions;
use std::path::PathBuf;

pub(super) fn build_fingerprint(lines: &[String]) -> String {
    build_state::hash_string(&lines.join("\n"))
}

pub(super) fn map_fingerprint_lines(
    label: &str,
    values: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| lhs.0.cmp(rhs.0));
    entries
        .into_iter()
        .map(|(key, value)| format!("{label}:{key}={value}"))
        .collect()
}

pub(super) fn compile_action_fingerprint(
    action: &CompileAction,
    options: &CompileOptions,
    toolchain_digest: &str,
) -> String {
    let mut lines = vec![
        "kind=compile".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("target={}", options.target.triple),
        format!("source={}", action.source_path().display()),
        format!("object={}", action.object_path.display()),
        format!("profile={}", action.profile.name),
        format!("opt={}", action.profile.opt),
        format!("debug={}", action.profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!(
            "root={}",
            options.root_module_name.as_deref().unwrap_or_default()
        ),
        format!("runtime_entry={}", options.runtime_entry.as_str()),
        format!("runtime_libc={}", options.runtime_libc),
        format!("library_bundle={}", options.library_bundle.as_str()),
        format!("split_sections_for_gc={}", options.split_sections_for_gc),
        format!("emit_multi_object_dir={}", options.emit_multi_object_dir),
    ];
    if let Some(metadata_output) = options.metadata_output.as_deref() {
        lines.push(format!("metadata={metadata_output}"));
    }
    lines.extend(map_fingerprint_lines("define", &options.custom_defines));
    lines.extend(map_fingerprint_lines(
        "ifalias",
        &options.module_interface_aliases,
    ));
    build_fingerprint(&lines)
}

pub(super) fn link_action_fingerprint(
    action: &LinkAction,
    options: &CompileOptions,
    linker_inputs: &[PathBuf],
    toolchain_digest: &str,
) -> String {
    let mut lines = vec![
        "kind=link".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("artifact={}", action.artifact_path.display()),
        format!("linker={}", options.linker_cmd),
        format!("runtime_entry={}", options.runtime_entry.as_str()),
        format!("runtime_libc={}", options.runtime_libc),
        format!("library_bundle={}", options.library_bundle.as_str()),
        format!("dead_strip_sections={}", options.dead_strip_sections),
    ];
    lines.extend(
        linker_inputs
            .iter()
            .map(|path| format!("input={}", path.display())),
    );
    lines.extend(
        options
            .linker_search_paths
            .iter()
            .map(|path| format!("search={path}")),
    );
    lines.extend(
        options
            .linker_libraries
            .iter()
            .map(|library| format!("lib={library}")),
    );
    lines.extend(options.linker_args.iter().map(|arg| format!("arg={arg}")));
    build_fingerprint(&lines)
}

pub(super) fn write_compile_action_state(
    action: &CompileAction,
    emit_multi_object_dir: bool,
    report: &CompileReport,
    fingerprint: String,
) -> Result<()> {
    let mut inputs = report.loaded_sources.clone();
    inputs.sort();
    inputs.dedup();

    let mut outputs = vec![action.object_path.clone()];
    if emit_multi_object_dir {
        let multi_object_dir = super::multi_object_output_dir(&action.object_path);
        if multi_object_dir.is_dir() {
            outputs.push(multi_object_dir);
        }
    }
    if let Some(metadata_path) = &action.metadata_path {
        outputs.push(metadata_path.clone());
    }

    build_state::record_action_state(&action.object_path, fingerprint, &inputs, &outputs)
}

pub(super) fn compile_action_label(action: &CompileAction) -> String {
    format!(
        "{}:{} -> {}",
        action.package_id.name,
        action.source_path().display(),
        action.object_path.display()
    )
}

pub(super) fn link_action_label(action: &LinkAction) -> String {
    format!(
        "{}:{}",
        action.package_id.name,
        action.artifact_path.display()
    )
}

pub(super) fn std_compile_action_label(profile: &str) -> String {
    format!("std ({profile})")
}

pub(super) fn rt_compile_action_label(profile: &str) -> String {
    format!("rt ({profile})")
}

pub(super) fn base_compile_action_label(profile: &str) -> String {
    format!("base ({profile})")
}

pub(super) fn sys_compile_action_label(profile: &str) -> String {
    format!("sys ({profile})")
}

pub(super) fn rt_entry_compile_action_label(profile: &str) -> String {
    format!("rt-entry ({profile})")
}
