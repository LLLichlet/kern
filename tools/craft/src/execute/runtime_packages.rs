//! Runtime, standard-library, and base-package build helpers.
//!
//! These packages are built on demand for the selected profile/target and then
//! reused across workspace actions through shared incremental state and cache
//! roots.

use super::options::{
    apply_host_linker_env, normalize_windows_linker_input_options, profile_linker_input_flavor,
};
use super::{
    ActionTimingKind, BuiltLibraryPackage, BuiltStdPackage, ExecutionSummary, Result,
    base_compile_action_label, build_fingerprint, compile_with_shared_driver, ensure_parent_dir,
    rt_compile_action_label, rt_entry_compile_action_label, runtime_compile_detail_tags,
    runtime_profile_key, std_compile_action_label,
};
use crate::build_plan::CompileAction;
use crate::build_state;
use crate::error::Error;
use crate::operation_lock::WorkspaceOperationLock;
use kernc_driver::{CompilerDriver, IncrementalDriverKey, KMETA_MANIFEST_FILE};
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, LtoMode, OptLevel, inject_driver_condition_defines,
    resolve_library_workspace_path,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static TEST_RUNTIME_CACHE_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn with_test_runtime_cache_root<T>(root: PathBuf, f: impl FnOnce() -> T) -> T {
    TEST_RUNTIME_CACHE_ROOT.with(|slot| {
        let previous = slot.replace(Some(root));
        let result = f();
        slot.replace(previous);
        result
    })
}

#[cfg(not(test))]
fn sanitize_cache_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn runtime_cache_root(_workspace_root: &Path) -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_RUNTIME_CACHE_ROOT.with(|slot| slot.borrow().clone()) {
        return Ok(root);
    }

    #[cfg(test)]
    {
        Ok(_workspace_root.join(".craft").join("runtime-cache"))
    }

    #[cfg(not(test))]
    {
        let toolchain_digest = build_state::current_process_digest()?;
        Ok(std::env::temp_dir()
            .join("kern")
            .join("craft-runtime-cache")
            .join(sanitize_cache_component(&toolchain_digest)))
    }
}

fn runtime_profile_root(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
) -> Result<PathBuf> {
    Ok(runtime_cache_root(workspace_root)?.join(runtime_profile_key(profile)))
}

fn runtime_profile_label(profile: &crate::script::ScriptProfile) -> String {
    format!(
        "{} (opt={}, debug={}, cgu={}, lto={}, code-model={})",
        profile.name,
        profile.opt,
        profile.debug,
        profile.codegen_units,
        profile.lto_mode.as_str(),
        profile.code_model.as_str()
    )
}

fn runtime_emit_multi_linker_input_dir(profile: &crate::script::ScriptProfile) -> bool {
    profile.codegen_units > 1 && profile.lto_mode != LtoMode::Full
}

fn runtime_driver_mode(command: crate::script::ScriptCommand) -> DriverMode {
    match command {
        crate::script::ScriptCommand::Check => DriverMode::CompileOnly,
        _ => DriverMode::CompileOnly,
    }
}

fn normalize_runtime_codegen_options_for_driver_mode(options: &mut CompileOptions) {
    if options.driver_mode != DriverMode::AnalyzeOnly {
        return;
    }

    options.codegen_units = 1;
    options.lto_mode = LtoMode::None;
    options.linker_input_flavor = kernc_utils::config::LinkerInputFlavor::Object;
    options.emit_multi_linker_input_dir = false;
}

fn runtime_compile_outputs(
    object_path: &Path,
    metadata_root_path: Option<&Path>,
    emits_linker_input: bool,
) -> Vec<PathBuf> {
    let mut outputs = Vec::new();
    if emits_linker_input {
        outputs.push(object_path.to_path_buf());
        let multi_linker_input_dir = super::multi_linker_input_dir(object_path);
        if multi_linker_input_dir.is_dir() {
            outputs.push(multi_linker_input_dir);
        }
    }
    if let Some(metadata_root_path) = metadata_root_path {
        outputs.push(metadata_root_path.to_path_buf());
    }
    outputs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RtEntryFlavor {
    Hosted,
    Freestanding,
}

impl RtEntryFlavor {
    fn object_stem(self) -> &'static str {
        match self {
            Self::Hosted => "rt_entry_hosted.o",
            Self::Freestanding => "rt_entry_freestanding.o",
        }
    }

    fn fingerprint_tag(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Freestanding => "freestanding",
        }
    }

    fn action_label_suffix(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::Freestanding => "freestanding",
        }
    }
}

fn runtime_opt_level(profile: &crate::script::ScriptProfile) -> OptLevel {
    match profile.opt {
        0 => OptLevel::O0,
        1 => OptLevel::O1,
        2 => OptLevel::O2,
        _ => OptLevel::O3,
    }
}

fn rt_entry_linker_input_flavor(
    profile: &crate::script::ScriptProfile,
) -> kernc_utils::config::LinkerInputFlavor {
    if matches!(
        crate::script::host_target().os,
        crate::script::ScriptOs::Windows | crate::script::ScriptOs::Darwin
    ) {
        // Platform startup shims define linker-contract symbols such as
        // `mainCRTStartup`, `__chkstk`, `_fltused`, or Darwin entry symbols.
        // Preserve them as concrete objects so the final link sees ordinary
        // object definitions instead of ThinLTO-internalizable bitcode.
        kernc_utils::config::LinkerInputFlavor::Object
    } else {
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target)
    }
}

fn official_library_workspace_root() -> PathBuf {
    resolve_library_workspace_path()
}

fn official_library_package_root(workspace_root: &Path, package: &str) -> PathBuf {
    workspace_root.join(package)
}

fn official_library_package_manifest(workspace_root: &Path, package: &str) -> PathBuf {
    official_library_package_root(workspace_root, package).join("Craft.toml")
}

pub(super) fn ensure_std_packages_for_actions(
    workspace_root: &Path,
    actions: &[CompileAction],
    command: crate::script::ScriptCommand,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    let mut profiles = BTreeMap::new();
    for action in actions {
        profiles
            .entry(runtime_profile_key(&action.profile))
            .or_insert_with(|| action.profile.clone());
    }
    for profile in profiles.values() {
        build_std_package(
            workspace_root,
            profile,
            command,
            built_std_packages,
            driver_families,
            execution_summary,
        )?;
    }
    Ok(())
}

pub(super) fn build_std_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    let profile_key = runtime_profile_key(profile);
    if built_std_packages.contains_key(&profile_key) {
        return Ok(());
    }

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let _runtime_lock = WorkspaceOperationLock::acquire(&profile_root, "build-runtime")?;

    let library_workspace_root = official_library_workspace_root();
    let library_workspace_manifest = library_workspace_root.join("Craft.toml");
    let std_root = official_library_package_root(&library_workspace_root, "std");
    let std_manifest = official_library_package_manifest(&library_workspace_root, "std");
    let source_path = std_root.join("mod.kn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "standard library root `{}` is missing",
            source_path.display()
        )));
    }
    let built_base = build_base_package(
        workspace_root,
        profile,
        command,
        driver_families,
        execution_summary,
    )?;
    let built_rt = Some(build_rt_package(
        workspace_root,
        profile,
        command,
        driver_families,
        execution_summary,
    )?);
    let hosted_rt_entry_object_path = build_rt_entry_package(
        workspace_root,
        profile,
        command,
        driver_families,
        execution_summary,
        RtEntryFlavor::Hosted,
    )?;
    let freestanding_rt_entry_object_path = build_rt_entry_package(
        workspace_root,
        profile,
        command,
        driver_families,
        execution_summary,
        RtEntryFlavor::Freestanding,
    )?;

    let object_path = profile_root
        .join("obj")
        .join("std")
        .join("lib")
        .join("std.o");
    let metadata_root_path = profile_root.join("meta").join("std");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let emit_multi_linker_input_dir = runtime_emit_multi_linker_input_dir(profile);
    let linker_input_flavor =
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target);
    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("std".to_string()),
        metadata_package_version: None,
        root_module_name: Some("std".to_string()),
        driver_mode: runtime_driver_mode(command),
        report_progress: false,
        opt_level: runtime_opt_level(profile),
        debug_info: profile.debug,
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        code_model: profile.code_model,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        library_bundle: LibraryBundle::Std,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("std".to_string(), std_root.to_string_lossy().to_string());
    options.module_interface_aliases.insert(
        "base".to_string(),
        built_base.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    normalize_runtime_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let mut std_fingerprint_lines = vec![
        "std_runtime_layout=v6".to_string(),
        "kind=compile-std".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("profile={}", profile.name),
        format!("opt={}", profile.opt),
        format!("debug={}", profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!("code_model={}", options.code_model.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("library_workspace={}", library_workspace_root.display()),
        format!(
            "library_workspace_manifest={}",
            library_workspace_manifest.display()
        ),
        format!("package_manifest={}", std_manifest.display()),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("base_meta={}", built_base.metadata_root_path.display()),
        format!("base_obj={}", built_base.object_path.display()),
        "split_sections_for_gc=true".to_string(),
    ];
    if let Some(built_rt) = &built_rt {
        std_fingerprint_lines.push(format!("rt_meta={}", built_rt.metadata_root_path.display()));
        std_fingerprint_lines.push(format!("rt_obj={}", built_rt.object_path.display()));
        std_fingerprint_lines.push(format!(
            "rt_entry_hosted_obj={}",
            hosted_rt_entry_object_path.display()
        ));
        std_fingerprint_lines.push(format!(
            "rt_entry_freestanding_obj={}",
            freestanding_rt_entry_object_path.display()
        ));
    }
    let std_fingerprint = build_fingerprint(&std_fingerprint_lines);
    let std_label = std_compile_action_label(&runtime_profile_label(profile), &options);
    let std_tags = runtime_compile_detail_tags(&options);
    let emits_linker_input = options.driver_mode.emits_linker_input();

    if !build_state::action_state_is_current(&object_path, &std_fingerprint)? {
        let Some(report) = compile_with_shared_driver(driver_families, options) else {
            return Err(Error::Execution(format!(
                "compile failed for standard library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        let outputs =
            runtime_compile_outputs(&object_path, Some(&metadata_root_path), emits_linker_input);
        build_state::record_action_state(&object_path, std_fingerprint, &inputs, &outputs)?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            std_label,
            std_tags,
            report.phase_timings,
            report.cache_stats,
            report.codegen_plan,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    built_std_packages.insert(
        profile_key,
        BuiltStdPackage {
            metadata_root_path,
            base_object_path: built_base.object_path.clone(),
            rt_object_path: built_rt
                .as_ref()
                .map(|built_rt| built_rt.object_path.clone()),
            common_link_objects: if let Some(built_rt) = &built_rt {
                vec![
                    object_path,
                    built_rt.object_path.clone(),
                    built_base.object_path.clone(),
                ]
            } else {
                Vec::new()
            },
            hosted_entry_object_path: hosted_rt_entry_object_path,
            freestanding_entry_object_path: freestanding_rt_entry_object_path,
            interface_aliases: BTreeMap::from([(
                "base".to_string(),
                built_base.metadata_root_path,
            )]),
        },
    );
    Ok(())
}

pub(super) fn build_rt_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let library_workspace_root = official_library_workspace_root();
    let library_workspace_manifest = library_workspace_root.join("Craft.toml");
    let rt_root = official_library_package_root(&library_workspace_root, "rt");
    let rt_manifest = official_library_package_manifest(&library_workspace_root, "rt");
    let source_path = rt_root.join("mod.kn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "rt library root `{}` is missing",
            source_path.display()
        )));
    }

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let object_path = profile_root.join("obj").join("rt").join("lib").join("rt.o");
    let metadata_root_path = profile_root.join("meta").join("rt");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let emit_multi_linker_input_dir = runtime_emit_multi_linker_input_dir(profile);
    let linker_input_flavor =
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target);
    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("rt".to_string()),
        metadata_package_version: None,
        root_module_name: Some("rt".to_string()),
        driver_mode: runtime_driver_mode(command),
        report_progress: false,
        opt_level: runtime_opt_level(profile),
        debug_info: profile.debug,
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        code_model: profile.code_model,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("rt".to_string(), rt_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    normalize_runtime_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let rt_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v2".to_string(),
        "kind=compile-rt".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("profile={}", profile.name),
        format!("opt={}", profile.opt),
        format!("debug={}", profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!("code_model={}", options.code_model.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("library_workspace={}", library_workspace_root.display()),
        format!(
            "library_workspace_manifest={}",
            library_workspace_manifest.display()
        ),
        format!("package_manifest={}", rt_manifest.display()),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);
    let rt_label = rt_compile_action_label(&runtime_profile_label(profile), &options);
    let rt_tags = runtime_compile_detail_tags(&options);
    let emits_linker_input = options.driver_mode.emits_linker_input();

    if !build_state::action_state_is_current(&object_path, &rt_fingerprint)? {
        let Some(report) = compile_with_shared_driver(driver_families, options) else {
            return Err(Error::Execution(format!(
                "compile failed for rt library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        let outputs =
            runtime_compile_outputs(&object_path, Some(&metadata_root_path), emits_linker_input);
        build_state::record_action_state(&object_path, rt_fingerprint, &inputs, &outputs)?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            rt_label,
            rt_tags,
            report.phase_timings,
            report.cache_stats,
            report.codegen_plan,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases: BTreeMap::new(),
    })
}

pub(super) fn build_base_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let library_workspace_root = official_library_workspace_root();
    let library_workspace_manifest = library_workspace_root.join("Craft.toml");
    let base_root = official_library_package_root(&library_workspace_root, "base");
    let base_manifest = official_library_package_manifest(&library_workspace_root, "base");
    let source_path = base_root.join("mod.kn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "base library root `{}` is missing",
            source_path.display()
        )));
    }

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let object_path = profile_root
        .join("obj")
        .join("base")
        .join("lib")
        .join("base.o");
    let metadata_root_path = profile_root.join("meta").join("base");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let emit_multi_linker_input_dir = runtime_emit_multi_linker_input_dir(profile);
    let linker_input_flavor =
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target);
    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("base".to_string()),
        metadata_package_version: None,
        root_module_name: Some("base".to_string()),
        driver_mode: runtime_driver_mode(command),
        report_progress: false,
        opt_level: runtime_opt_level(profile),
        debug_info: profile.debug,
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        code_model: profile.code_model,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        library_bundle: LibraryBundle::Base,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("base".to_string(), base_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    normalize_runtime_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let base_fingerprint = build_fingerprint(&[
        "base_runtime_layout=v1".to_string(),
        "kind=compile-base".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("profile={}", profile.name),
        format!("opt={}", profile.opt),
        format!("debug={}", profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!("code_model={}", options.code_model.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("library_workspace={}", library_workspace_root.display()),
        format!(
            "library_workspace_manifest={}",
            library_workspace_manifest.display()
        ),
        format!("package_manifest={}", base_manifest.display()),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);
    let base_label = base_compile_action_label(&runtime_profile_label(profile), &options);
    let base_tags = runtime_compile_detail_tags(&options);
    let emits_linker_input = options.driver_mode.emits_linker_input();

    if !build_state::action_state_is_current(&object_path, &base_fingerprint)? {
        let Some(report) = compile_with_shared_driver(driver_families, options) else {
            return Err(Error::Execution(format!(
                "compile failed for base library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        let outputs =
            runtime_compile_outputs(&object_path, Some(&metadata_root_path), emits_linker_input);
        build_state::record_action_state(&object_path, base_fingerprint, &inputs, &outputs)?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            base_label,
            base_tags,
            report.phase_timings,
            report.cache_stats,
            report.codegen_plan,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases: BTreeMap::new(),
    })
}

pub(super) fn build_rt_entry_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
    flavor: RtEntryFlavor,
) -> Result<PathBuf> {
    let library_workspace_root = official_library_workspace_root();
    let library_workspace_manifest = library_workspace_root.join("Craft.toml");
    let rt_manifest = official_library_package_manifest(&library_workspace_root, "rt");
    let source_path = official_library_package_root(&library_workspace_root, "rt").join("entry.kn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "rt entry source `{}` is missing",
            source_path.display()
        )));
    }

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let object_path = profile_root
        .join("obj")
        .join("rt")
        .join("entry")
        .join(flavor.object_stem());

    ensure_parent_dir(&object_path)?;

    let emit_multi_linker_input_dir = runtime_emit_multi_linker_input_dir(profile);
    let linker_input_flavor = rt_entry_linker_input_flavor(profile);
    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        root_module_name: Some("rt_entry".to_string()),
        driver_mode: runtime_driver_mode(command),
        report_progress: false,
        opt_level: runtime_opt_level(profile),
        debug_info: profile.debug,
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        code_model: profile.code_model,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    inject_driver_condition_defines(&mut options);
    match flavor {
        RtEntryFlavor::Hosted => {
            options
                .custom_defines
                .insert("rt_role".to_string(), "entry".to_string());
        }
        RtEntryFlavor::Freestanding => {
            options
                .custom_defines
                .insert("runtime_entry".to_string(), "rt".to_string());
            options
                .custom_defines
                .insert("libc".to_string(), "false".to_string());
            options
                .custom_defines
                .insert("crt_startup".to_string(), "false".to_string());
        }
    }
    normalize_runtime_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let entry_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v1".to_string(),
        "kind=compile-rt-entry".to_string(),
        format!("flavor={}", flavor.fingerprint_tag()),
        format!("toolchain={toolchain_digest}"),
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("profile={}", profile.name),
        format!("opt={}", profile.opt),
        format!("debug={}", profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!("code_model={}", options.code_model.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("library_workspace={}", library_workspace_root.display()),
        format!(
            "library_workspace_manifest={}",
            library_workspace_manifest.display()
        ),
        format!("package_manifest={}", rt_manifest.display()),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);
    let entry_label = format!(
        "{} [{}]",
        rt_entry_compile_action_label(&runtime_profile_label(profile), &options),
        flavor.action_label_suffix()
    );
    let entry_tags = runtime_compile_detail_tags(&options);
    let emits_linker_input = options.driver_mode.emits_linker_input();

    if !build_state::action_state_is_current(&object_path, &entry_fingerprint)? {
        let Some(report) = compile_with_shared_driver(driver_families, options) else {
            return Err(Error::Execution(format!(
                "compile failed for rt hosted entry `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        let outputs = runtime_compile_outputs(&object_path, None, emits_linker_input);
        build_state::record_action_state(&object_path, entry_fingerprint, &inputs, &outputs)?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            entry_label,
            entry_tags,
            report.phase_timings,
            report.cache_stats,
            report.codegen_plan,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    Ok(object_path)
}
