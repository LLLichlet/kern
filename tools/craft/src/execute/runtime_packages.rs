use super::options::{
    apply_host_linker_env, normalize_windows_linker_input_options, profile_linker_input_flavor,
};
use super::{
    ActionTimingKind, BuiltLibraryPackage, BuiltStdPackage, ExecutionSummary, Result,
    base_compile_action_label, build_fingerprint, compile_with_shared_driver, ensure_parent_dir,
    rt_compile_action_label, rt_entry_compile_action_label, runtime_compile_detail_tags,
    runtime_profile_key, std_compile_action_label, sys_compile_action_label,
};
use crate::build_plan::CompileAction;
use crate::build_state;
use crate::error::Error;
use crate::operation_lock::WorkspaceOperationLock;
use kernc_driver::{CompilerDriver, IncrementalDriverKey, KMETA_MANIFEST_FILE};
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, LtoMode, OptLevel, inject_driver_condition_defines,
    resolve_base_path, resolve_rt_path, resolve_std_path, resolve_sys_path,
};
use std::collections::{BTreeMap, HashMap};
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
        "{} (opt={}, debug={}, cgu={}, lto={})",
        profile.name,
        profile.opt,
        profile.debug,
        profile.codegen_units,
        profile.lto_mode.as_str()
    )
}

fn runtime_emit_multi_linker_input_dir(profile: &crate::script::ScriptProfile) -> bool {
    profile.codegen_units > 1 && profile.lto_mode != LtoMode::Full
}

fn runtime_driver_mode(command: crate::script::ScriptCommand) -> DriverMode {
    match command {
        crate::script::ScriptCommand::Check => DriverMode::AnalyzeOnly,
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
    if crate::script::host_target().os == crate::script::ScriptOs::Windows {
        // Windows startup shims define linker-contract symbols such as
        // `mainCRTStartup`, `__chkstk`, and `_fltused`. Preserve them as a
        // concrete COFF object so the final link sees them as ordinary object
        // definitions instead of ThinLTO-internalizable bitcode.
        kernc_utils::config::LinkerInputFlavor::Object
    } else {
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target)
    }
}

pub(super) fn interface_alias_strings(
    aliases: &BTreeMap<String, PathBuf>,
) -> HashMap<String, String> {
    aliases
        .iter()
        .map(|(name, path)| (name.clone(), path.to_string_lossy().to_string()))
        .collect()
}

pub(super) fn extend_interface_aliases(
    options: &mut CompileOptions,
    aliases: &BTreeMap<String, PathBuf>,
) {
    options
        .module_interface_aliases
        .extend(interface_alias_strings(aliases));
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

    let std_root = resolve_std_path();
    let source_path = std_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "standard library root `{}` is missing",
            source_path.display()
        )));
    }
    let built_sys = build_sys_package(
        workspace_root,
        profile,
        command,
        driver_families,
        execution_summary,
    )?;
    let built_rt = if command == crate::script::ScriptCommand::Check {
        None
    } else {
        Some(build_rt_package(
            workspace_root,
            profile,
            command,
            driver_families,
            execution_summary,
            &built_sys,
        )?)
    };
    let hosted_rt_entry_object_path = if command == crate::script::ScriptCommand::Check {
        PathBuf::new()
    } else {
        build_rt_entry_package(
            workspace_root,
            profile,
            command,
            driver_families,
            execution_summary,
            &built_sys,
            RtEntryFlavor::Hosted,
        )?
    };
    let freestanding_rt_entry_object_path = if command == crate::script::ScriptCommand::Check {
        PathBuf::new()
    } else {
        build_rt_entry_package(
            workspace_root,
            profile,
            command,
            driver_families,
            execution_summary,
            &built_sys,
            RtEntryFlavor::Freestanding,
        )?
    };

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
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
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
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
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
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
        format!("sys_obj={}", built_sys.object_path.display()),
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
            common_link_objects: if let Some(built_rt) = &built_rt {
                vec![
                    object_path,
                    built_rt.object_path.clone(),
                    built_sys.object_path.clone(),
                    profile_root
                        .join("obj")
                        .join("base")
                        .join("lib")
                        .join("base.o"),
                ]
            } else {
                Vec::new()
            },
            hosted_entry_object_path: hosted_rt_entry_object_path,
            freestanding_entry_object_path: freestanding_rt_entry_object_path,
            interface_aliases: {
                let mut aliases = built_sys.interface_aliases.clone();
                aliases.insert("sys".to_string(), built_sys.metadata_root_path);
                aliases
            },
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
    built_sys: &BuiltLibraryPackage,
) -> Result<BuiltLibraryPackage> {
    let rt_root = resolve_rt_path();
    let source_path = rt_root.join("init.rn");
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
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("rt".to_string(), rt_root.to_string_lossy().to_string());
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
    );
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
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
        format!("sys_obj={}", built_sys.object_path.display()),
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
    let base_root = resolve_base_path();
    let source_path = base_root.join("init.rn");
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
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
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
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
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

pub(super) fn build_sys_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<BuiltLibraryPackage> {
    let sys_root = resolve_sys_path();
    let source_path = sys_root.join("init.rn");
    if !source_path.is_file() {
        return Err(Error::Execution(format!(
            "sys library root `{}` is missing",
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

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let object_path = profile_root
        .join("obj")
        .join("sys")
        .join("lib")
        .join("sys.o");
    let metadata_root_path = profile_root.join("meta").join("sys");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let emit_multi_linker_input_dir = runtime_emit_multi_linker_input_dir(profile);
    let linker_input_flavor =
        profile_linker_input_flavor(profile, crate::graph::BuildDomain::Target);
    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("sys".to_string()),
        metadata_package_version: None,
        root_module_name: Some("sys".to_string()),
        driver_mode: runtime_driver_mode(command),
        report_progress: false,
        opt_level: runtime_opt_level(profile),
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        library_bundle: LibraryBundle::Base,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("sys".to_string(), sys_root.to_string_lossy().to_string());
    extend_interface_aliases(&mut options, &built_base.interface_aliases);
    options.module_interface_aliases.insert(
        "base".to_string(),
        built_base.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    normalize_runtime_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let sys_fingerprint = build_fingerprint(&[
        "sys_runtime_layout=v1".to_string(),
        "kind=compile-sys".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("driver_mode={}", options.driver_mode.as_str()),
        format!("profile={}", profile.name),
        format!("opt={}", profile.opt),
        format!("debug={}", profile.debug),
        format!("codegen_units={}", options.codegen_units),
        format!("lto={}", options.lto_mode.as_str()),
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("base_meta={}", built_base.metadata_root_path.display()),
        format!("base_obj={}", built_base.object_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);
    let sys_label = sys_compile_action_label(&runtime_profile_label(profile), &options);
    let sys_tags = runtime_compile_detail_tags(&options);
    let emits_linker_input = options.driver_mode.emits_linker_input();

    if !build_state::action_state_is_current(&object_path, &sys_fingerprint)? {
        let Some(report) = compile_with_shared_driver(driver_families, options) else {
            return Err(Error::Execution(format!(
                "compile failed for sys library `{}`",
                source_path.display()
            )));
        };

        let mut inputs = report.loaded_sources;
        inputs.sort();
        inputs.dedup();
        let outputs =
            runtime_compile_outputs(&object_path, Some(&metadata_root_path), emits_linker_input);
        build_state::record_action_state(&object_path, sys_fingerprint, &inputs, &outputs)?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            sys_label,
            sys_tags,
            report.phase_timings,
            report.cache_stats,
            report.codegen_plan,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    let mut interface_aliases = built_base.interface_aliases.clone();
    interface_aliases.insert("base".to_string(), built_base.metadata_root_path);
    Ok(BuiltLibraryPackage {
        metadata_root_path,
        object_path,
        interface_aliases,
    })
}

pub(super) fn build_rt_entry_package(
    workspace_root: &Path,
    profile: &crate::script::ScriptProfile,
    command: crate::script::ScriptCommand,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
    built_sys: &BuiltLibraryPackage,
    flavor: RtEntryFlavor,
) -> Result<PathBuf> {
    let source_path = resolve_rt_path().join("entry.rn");
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
        codegen_units: profile.codegen_units,
        lto_mode: profile.lto_mode,
        linker_input_flavor,
        emit_multi_linker_input_dir,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
    );
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
        format!(
            "linker_input_flavor={}",
            options.linker_input_flavor.as_str()
        ),
        format!(
            "emit_multi_linker_input_dir={}",
            options.emit_multi_linker_input_dir
        ),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
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
