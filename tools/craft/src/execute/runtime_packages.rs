use super::{
    ActionTimingKind, BuiltLibraryPackage, BuiltStdPackage, ExecutionSummary, Result,
    apply_host_linker_env, base_compile_action_label, build_fingerprint,
    compile_with_shared_driver, ensure_parent_dir, rt_compile_action_label,
    rt_entry_compile_action_label, std_compile_action_label, sys_compile_action_label,
};
use crate::build_plan::CompileAction;
use crate::build_state;
use crate::error::Error;
use crate::operation_lock::WorkspaceOperationLock;
use kernc_driver::{CompilerDriver, IncrementalDriverKey, KMETA_MANIFEST_FILE};
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, OptLevel, inject_driver_condition_defines,
    resolve_base_path, resolve_rt_path, resolve_std_path, resolve_sys_path,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
        return Ok(_workspace_root.join(".craft").join("runtime-cache"));
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

fn runtime_profile_root(workspace_root: &Path, profile: &str) -> Result<PathBuf> {
    Ok(runtime_cache_root(workspace_root)?.join(profile))
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
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    let profiles = actions
        .iter()
        .map(|action| action.profile.name.clone())
        .collect::<BTreeSet<_>>();
    for profile in profiles {
        build_std_package(
            workspace_root,
            &profile,
            built_std_packages,
            driver_families,
            execution_summary,
        )?;
    }
    Ok(())
}

pub(super) fn build_std_package(
    workspace_root: &Path,
    profile: &str,
    built_std_packages: &mut BTreeMap<String, BuiltStdPackage>,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
) -> Result<()> {
    if built_std_packages.contains_key(profile) {
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
    let built_rt = build_rt_package(workspace_root, profile, driver_families, execution_summary)?;
    let built_sys = build_sys_package(workspace_root, profile, driver_families, execution_summary)?;
    let rt_entry_object_path = build_rt_entry_package(
        workspace_root,
        profile,
        driver_families,
        execution_summary,
        &built_sys,
    )?;

    let object_path = profile_root
        .join("obj")
        .join("std")
        .join("lib")
        .join("std.o");
    let metadata_root_path = profile_root.join("meta").join("std");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("std".to_string()),
        metadata_package_version: None,
        root_module_name: Some("std".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
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
    let toolchain_digest = build_state::current_process_digest()?;
    let std_fingerprint = build_fingerprint(&[
        "std_runtime_layout=v6".to_string(),
        "kind=compile-std".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("rt_meta={}", built_rt.metadata_root_path.display()),
        format!("rt_obj={}", built_rt.object_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
        format!("sys_obj={}", built_sys.object_path.display()),
        format!("rt_entry_obj={}", rt_entry_object_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);

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
        build_state::record_action_state(
            &object_path,
            std_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            std_compile_action_label(profile),
            report.phase_timings,
            report.cache_stats,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    built_std_packages.insert(
        profile.to_string(),
        BuiltStdPackage {
            metadata_root_path,
            link_objects: vec![
                object_path,
                built_rt.object_path.clone(),
                built_sys.object_path.clone(),
                profile_root
                    .join("obj")
                    .join("base")
                    .join("lib")
                    .join("base.o"),
                rt_entry_object_path,
            ],
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
    profile: &str,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
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

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("rt".to_string()),
        metadata_package_version: None,
        root_module_name: Some("rt".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("rt".to_string(), rt_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let rt_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v1".to_string(),
        "kind=compile-rt".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);

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
        build_state::record_action_state(
            &object_path,
            rt_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            rt_compile_action_label(profile),
            report.phase_timings,
            report.cache_stats,
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
    profile: &str,
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

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("base".to_string()),
        metadata_package_version: None,
        root_module_name: Some("base".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        library_bundle: LibraryBundle::Base,
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    apply_host_linker_env(&mut options);
    options
        .module_aliases
        .insert("base".to_string(), base_root.to_string_lossy().to_string());
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let base_fingerprint = build_fingerprint(&[
        "base_runtime_layout=v1".to_string(),
        "kind=compile-base".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);

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
        build_state::record_action_state(
            &object_path,
            base_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            base_compile_action_label(profile),
            report.phase_timings,
            report.cache_stats,
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
    profile: &str,
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
    let built_base =
        build_base_package(workspace_root, profile, driver_families, execution_summary)?;

    let profile_root = runtime_profile_root(workspace_root, profile)?;
    let object_path = profile_root
        .join("obj")
        .join("sys")
        .join("lib")
        .join("sys.o");
    let metadata_root_path = profile_root.join("meta").join("sys");

    ensure_parent_dir(&object_path)?;
    ensure_parent_dir(&metadata_root_path.join(KMETA_MANIFEST_FILE))?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        metadata_output: Some(metadata_root_path.to_string_lossy().to_string()),
        metadata_package_name: Some("sys".to_string()),
        metadata_package_version: None,
        root_module_name: Some("sys".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
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
    let toolchain_digest = build_state::current_process_digest()?;
    let sys_fingerprint = build_fingerprint(&[
        "sys_runtime_layout=v1".to_string(),
        "kind=compile-sys".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("metadata={}", metadata_root_path.display()),
        format!("base_meta={}", built_base.metadata_root_path.display()),
        format!("base_obj={}", built_base.object_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);

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
        build_state::record_action_state(
            &object_path,
            sys_fingerprint,
            &inputs,
            &[object_path.clone(), metadata_root_path.clone()],
        )?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            sys_compile_action_label(profile),
            report.phase_timings,
            report.cache_stats,
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
    profile: &str,
    driver_families: &mut BTreeMap<IncrementalDriverKey, CompilerDriver>,
    execution_summary: &mut ExecutionSummary,
    built_sys: &BuiltLibraryPackage,
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
        .join("rt_entry.o");

    ensure_parent_dir(&object_path)?;

    let mut options = CompileOptions {
        input_file: Some(source_path.to_string_lossy().to_string()),
        output_file: object_path.to_string_lossy().to_string(),
        root_module_name: Some("rt_entry".to_string()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: if profile == "release" {
            OptLevel::O3
        } else {
            OptLevel::O0
        },
        split_sections_for_gc: true,
        ..CompileOptions::default()
    };
    options
        .custom_defines
        .insert("rt_role".to_string(), "entry".to_string());
    extend_interface_aliases(&mut options, &built_sys.interface_aliases);
    options.module_interface_aliases.insert(
        "sys".to_string(),
        built_sys.metadata_root_path.to_string_lossy().to_string(),
    );
    inject_driver_condition_defines(&mut options);
    let toolchain_digest = build_state::current_process_digest()?;
    let entry_fingerprint = build_fingerprint(&[
        "rt_runtime_layout=v1".to_string(),
        "kind=compile-rt-entry".to_string(),
        format!("toolchain={toolchain_digest}"),
        format!("profile={profile}"),
        format!("source={}", source_path.display()),
        format!("object={}", object_path.display()),
        format!("sys_meta={}", built_sys.metadata_root_path.display()),
        "split_sections_for_gc=true".to_string(),
    ]);

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
        build_state::record_action_state(
            &object_path,
            entry_fingerprint,
            &inputs,
            std::slice::from_ref(&object_path),
        )?;
        execution_summary.record_compile_cache_miss();
        execution_summary.record_action(
            ActionTimingKind::Compile,
            rt_entry_compile_action_label(profile),
            report.phase_timings,
            report.cache_stats,
        );
    } else {
        execution_summary.record_compile_cache_hit();
    }

    Ok(object_path)
}
