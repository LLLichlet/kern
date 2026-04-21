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
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "root={}",
            options.root_module_name.as_deref().unwrap_or_default()
        ),
        format!("runtime_entry={}", options.runtime_entry.as_str()),
        format!("runtime_libc={}", options.runtime_libc),
        format!("library_bundle={}", options.library_bundle.as_str()),
        format!("split_sections_for_gc={}", options.split_sections_for_gc),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
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
    link_input_paths: &[PathBuf],
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
        link_input_paths
            .iter()
            .map(|path| format!("link-input={}", path.display())),
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
    emits_linker_input: bool,
    emit_multi_linker_input_dir: bool,
    report: &CompileReport,
    fingerprint: String,
) -> Result<()> {
    let mut inputs = report.loaded_sources.clone();
    inputs.sort();
    inputs.dedup();

    let mut outputs = Vec::new();
    if emits_linker_input {
        outputs.push(action.object_path.clone());
    }
    if emits_linker_input && emit_multi_linker_input_dir {
        let multi_linker_input_dir = super::multi_linker_input_dir(&action.object_path);
        if multi_linker_input_dir.is_dir() {
            outputs.push(multi_linker_input_dir);
        }
    }
    if let Some(metadata_path) = &action.metadata_path {
        outputs.push(metadata_path.clone());
    }

    build_state::record_action_state(&action.object_path, fingerprint, &inputs, &outputs)
}

pub(super) fn compile_action_label(action: &CompileAction, options: &CompileOptions) -> String {
    format!(
        "{}:{} -> {} [{}]",
        action.package_id.name,
        action.source_path().display(),
        action.object_path.display(),
        compile_pipeline_label(options),
    )
}

pub(super) fn compile_action_detail_tags(options: &CompileOptions) -> Vec<String> {
    let mut tags = vec![
        format!("pipeline={}", compile_pipeline_label(options)),
        format!("lto={}", options.lto_mode.as_str()),
        format!("cgu={}", options.codegen_units),
        format!("linker-input={}", options.linker_input_flavor.as_str()),
    ];
    if options.emit_multi_linker_input_dir {
        tags.push("preserved-inputs=dir".to_string());
    }
    tags
}

pub(super) fn link_action_label(action: &LinkAction, options: &CompileOptions) -> String {
    format!(
        "{}:{} [{}]",
        action.package_id.name,
        action.artifact_path.display(),
        link_pipeline_label(options),
    )
}

pub(super) fn link_action_detail_tags(
    action: &LinkAction,
    options: &CompileOptions,
    linker_inputs: &[PathBuf],
) -> Vec<String> {
    let dependency_count = action.local_library_objects.len() + action.external_dependencies.len();
    let mut tags = vec![
        format!("pipeline={}", link_pipeline_label(options)),
        format!("inputs={}", linker_inputs.len()),
        format!("local-libs={}", action.local_library_objects.len()),
        format!("external-deps={}", action.external_dependencies.len()),
        format!("cross-package={}", dependency_count > 0),
    ];
    if !matches!(
        options.runtime_entry,
        kernc_utils::config::RuntimeEntry::None
    ) {
        tags.push(format!("runtime-entry={}", options.runtime_entry.as_str()));
    }
    tags
}

pub(super) fn std_compile_action_label(profile: &str, options: &CompileOptions) -> String {
    format!("std ({profile}; {})", compile_pipeline_label(options))
}

pub(super) fn runtime_compile_detail_tags(options: &CompileOptions) -> Vec<String> {
    compile_action_detail_tags(options)
}

pub(super) fn rt_compile_action_label(profile: &str, options: &CompileOptions) -> String {
    format!("rt ({profile}; {})", compile_pipeline_label(options))
}

pub(super) fn base_compile_action_label(profile: &str, options: &CompileOptions) -> String {
    format!("base ({profile}; {})", compile_pipeline_label(options))
}

pub(super) fn sys_compile_action_label(profile: &str, options: &CompileOptions) -> String {
    format!("sys ({profile}; {})", compile_pipeline_label(options))
}

pub(super) fn rt_entry_compile_action_label(profile: &str, options: &CompileOptions) -> String {
    format!("rt-entry ({profile}; {})", compile_pipeline_label(options))
}

fn compile_pipeline_label(options: &CompileOptions) -> &'static str {
    if options.driver_mode == kernc_utils::config::DriverMode::AnalyzeOnly {
        return "semantic-check";
    }

    match (
        options.linker_input_flavor.as_str(),
        options.lto_mode.as_str(),
        options.driver_mode.emits_linker_input(),
    ) {
        ("thinlto-bitcode", _, true) => "thinlto-bitcode",
        (_, "full", true) => "full-lto-object",
        (_, "thin", true) => "thinlto-object",
        _ => "object",
    }
}

fn link_pipeline_label(options: &CompileOptions) -> &'static str {
    if options.linker_args.iter().any(|arg| arg == "-flto=thin") {
        "thinlto-final-link"
    } else {
        "native-link"
    }
}

#[cfg(test)]
mod tests {
    use super::{compile_action_label, link_action_label};
    use crate::build_plan::{CompileAction, CompileSourceInput, LinkAction, LinkPlan};
    use crate::graph::{BuildDomain, PackageId, SourceId};
    use crate::script::ScriptProfile;
    use kernc_utils::config::{CompileOptions, DriverMode, LinkerInputFlavor, LtoMode};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn package_id(name: &str) -> PackageId {
        PackageId {
            name: name.to_string(),
            version: "0.1.0".to_string(),
            source: SourceId::Root,
        }
    }

    fn profile() -> ScriptProfile {
        ScriptProfile {
            name: "release".to_string(),
            opt: 3,
            debug: false,
            codegen_units: 2,
            lto_mode: LtoMode::Thin,
        }
    }

    #[test]
    fn action_labels_include_pipeline_kind() {
        let compile_action = CompileAction {
            domain: BuildDomain::Target,
            package_id: package_id("demo"),
            manifest_path: PathBuf::from("Craft.toml"),
            target_kind: crate::plan::TargetKind::Lib,
            target_name: None,
            artifact_name: "demo".to_string(),
            generated_root_path: PathBuf::from("build/gen/demo"),
            source_input: CompileSourceInput::AbsolutePath(PathBuf::from("src/lib.rn")),
            metadata_path: None,
            object_path: PathBuf::from("build/demo.o"),
            artifact_path: PathBuf::from("build/libdemo.a"),
            profile: profile(),
            cfg: BTreeMap::new(),
            define: BTreeMap::new(),
            compile_inputs: Vec::new(),
            local_dependencies: Vec::new(),
            external_dependencies: Vec::new(),
        };
        let link_action = LinkAction {
            domain: BuildDomain::Target,
            package_id: package_id("demo"),
            manifest_path: PathBuf::from("Craft.toml"),
            package_root_path: PathBuf::from("."),
            artifact_root_path: PathBuf::from("build/stage/demo"),
            target_kind: crate::plan::TargetKind::Bin,
            target_name: Some("demo".to_string()),
            artifact_name: "demo".to_string(),
            artifact_path: PathBuf::from("build/demo"),
            primary_object: PathBuf::from("build/demo.o"),
            local_library_objects: Vec::new(),
            artifact_outputs: Vec::new(),
            external_dependencies: Vec::new(),
            link: LinkPlan::default(),
        };

        let compile_options = CompileOptions {
            driver_mode: DriverMode::CompileOnly,
            lto_mode: LtoMode::Thin,
            linker_input_flavor: LinkerInputFlavor::ThinLtoBitcode,
            ..CompileOptions::default()
        };
        let link_options = CompileOptions {
            driver_mode: DriverMode::LinkOnly,
            linker_args: vec!["-flto=thin".to_string()],
            ..CompileOptions::default()
        };

        assert!(
            compile_action_label(&compile_action, &compile_options).contains("thinlto-bitcode")
        );
        assert!(link_action_label(&link_action, &link_options).contains("thinlto-final-link"));
    }
}
