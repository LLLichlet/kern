use super::{
    BuiltExternalPackage, BuiltStdPackage, ManifestRuntimeOptions, PackageInstanceKey,
    runtime_profile_key,
};
use crate::build_plan::{CompileAction, LinkAction};
use crate::error::{Error, Result};
use crate::graph::BuildDomain;
use crate::manifest::Manifest;
use crate::resolver::ExternalPackageId;
use crate::target_defaults::apply_target_runtime_defaults;
use kernc_utils::config::{
    CompileOptions, DriverMode, LinkerInputFlavor, LtoMode, OptLevel,
    inject_driver_condition_defines, maybe_add_base_alias, maybe_add_std_alias,
    maybe_add_sys_alias,
};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::external::{compile_module_aliases, link_objects_for_compile_action};

fn default_target_compile_options(target_kind: crate::plan::TargetKind) -> CompileOptions {
    let mut options = CompileOptions::default();
    apply_target_runtime_defaults(&mut options, target_kind);
    options
}

fn inject_target_library_aliases(options: &mut CompileOptions) {
    if options.module_interface_aliases.contains_key("std") {
        return;
    }
    maybe_add_base_alias(options);
    maybe_add_sys_alias(options);
    if !options.module_interface_aliases.contains_key("std") {
        maybe_add_std_alias(options);
    }
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
        if let Some(existing) = values.get(name)
            && existing != &value
        {
            return Err(Error::Execution(format!(
                "compile-time key `{name}` has conflicting cfg/define values for `{}`",
                source_path.display()
            )));
        }
        values.insert(name.clone(), value);
    }

    Ok(values)
}

fn apply_manifest_runtime_options(
    manifest_path: &Path,
    manifest_runtime_options: &mut BTreeMap<std::path::PathBuf, ManifestRuntimeOptions>,
    target_kind: crate::plan::TargetKind,
    options: &mut CompileOptions,
) -> Result<()> {
    if let Some(cached) = manifest_runtime_options.get(manifest_path) {
        cached.apply_for_target(target_kind, options);
        return Ok(());
    }

    let manifest = Manifest::load(manifest_path)?;
    manifest.validate(manifest_path)?;
    let cached = ManifestRuntimeOptions {
        entry: manifest.runtime.as_ref().and_then(|runtime| runtime.entry),
        libc: manifest.runtime.as_ref().and_then(|runtime| runtime.libc),
        bundle: manifest.runtime.as_ref().and_then(|runtime| runtime.bundle),
    };
    cached.apply_for_target(target_kind, options);
    manifest_runtime_options.insert(manifest_path.to_path_buf(), cached);
    Ok(())
}

fn plan_value_string(value: &crate::plan::PlanValue) -> String {
    match value {
        crate::plan::PlanValue::Bool(value) => value.to_string(),
        crate::plan::PlanValue::String(value) => value.clone(),
    }
}

fn profile_opt_level(profile: &crate::script::ScriptProfile) -> OptLevel {
    match profile.opt {
        0 => OptLevel::O0,
        1 => OptLevel::O1,
        2 => OptLevel::O2,
        _ => OptLevel::O3,
    }
}

fn profile_emit_multi_linker_input_dir(
    profile: &crate::script::ScriptProfile,
    domain: BuildDomain,
) -> bool {
    domain == BuildDomain::Target && profile.codegen_units > 1 && profile.lto_mode != LtoMode::Full
}

pub(super) fn profile_linker_input_flavor(
    profile: &crate::script::ScriptProfile,
    domain: BuildDomain,
) -> LinkerInputFlavor {
    if domain == BuildDomain::Target && profile.lto_mode == LtoMode::Thin {
        LinkerInputFlavor::ThinLtoBitcode
    } else {
        LinkerInputFlavor::Object
    }
}

pub(super) fn profile_uses_cross_package_thin_lto(
    profile: &crate::script::ScriptProfile,
    domain: BuildDomain,
) -> bool {
    domain == BuildDomain::Target && profile.lto_mode == LtoMode::Thin
}

pub(super) fn compile_action_options(
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    manifest_runtime_options: &mut BTreeMap<std::path::PathBuf, ManifestRuntimeOptions>,
) -> Result<CompileOptions> {
    let mut options = CompileOptions {
        input_file: Some(action.source_path().to_string_lossy().to_string()),
        output_file: action.object_path.to_string_lossy().to_string(),
        metadata_output: action
            .metadata_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        metadata_package_name: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.name.clone()),
        metadata_package_version: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.version.clone()),
        root_module_name: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.name.clone()),
        driver_mode: DriverMode::CompileOnly,
        report_progress: false,
        opt_level: profile_opt_level(&action.profile),
        codegen_units: action.profile.codegen_units,
        lto_mode: action.profile.lto_mode,
        linker_input_flavor: profile_linker_input_flavor(&action.profile, action.domain),
        emit_multi_linker_input_dir: profile_emit_multi_linker_input_dir(
            &action.profile,
            action.domain,
        ),
        split_sections_for_gc: true,
        ..default_target_compile_options(action.target_kind)
    };
    apply_manifest_runtime_options(
        &action.manifest_path,
        manifest_runtime_options,
        action.target_kind,
        &mut options,
    )?;
    apply_host_linker_env(&mut options);
    options.module_interface_aliases = compile_module_aliases(
        action,
        local_library_actions,
        built_std_packages.get(&runtime_profile_key(&action.profile)),
        built_external_packages,
    )?;
    inject_target_library_aliases(&mut options);
    inject_driver_condition_defines(&mut options);
    options.custom_defines.extend(compile_time_defines(
        &action.cfg,
        &action.define,
        action.source_path(),
    )?);
    Ok(options)
}

pub(super) fn link_action_options(
    action: &LinkAction,
    compile_action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    manifest_runtime_options: &mut BTreeMap<std::path::PathBuf, ManifestRuntimeOptions>,
) -> Result<(CompileOptions, Vec<std::path::PathBuf>)> {
    let mut options = CompileOptions {
        output_file: action.artifact_path.to_string_lossy().to_string(),
        driver_mode: DriverMode::LinkOnly,
        report_progress: false,
        dead_strip_sections: true,
        ..default_target_compile_options(action.target_kind)
    };
    apply_manifest_runtime_options(
        &action.manifest_path,
        manifest_runtime_options,
        action.target_kind,
        &mut options,
    )?;
    apply_host_linker_env(&mut options);
    let linker_inputs = link_objects_for_compile_action(
        compile_action,
        &options,
        local_library_actions,
        built_std_packages,
        built_external_packages,
    )?;
    options.linker_inputs = linker_inputs
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
    options.linker_libraries = action.link.system_libs.clone();
    options.linker_search_paths = action.link.search_paths.clone();
    options.linker_args = action.link.args.clone();
    if profile_uses_cross_package_thin_lto(&compile_action.profile, compile_action.domain)
        && !options.linker_args.iter().any(|arg| arg == "-flto=thin")
    {
        options.linker_args.push("-flto=thin".to_string());
    }
    for framework in &action.link.frameworks {
        options.linker_args.push("-framework".to_string());
        options.linker_args.push(framework.clone());
    }
    Ok((options, linker_inputs))
}

pub(super) fn apply_host_linker_env(options: &mut CompileOptions) {
    if let Ok(cc_env) = std::env::var("CC") {
        options.linker_cmd = cc_env;
    }
}

#[cfg(test)]
mod tests {
    use super::{profile_emit_multi_linker_input_dir, profile_linker_input_flavor};
    use crate::graph::BuildDomain;
    use crate::script::ScriptProfile;
    use kernc_utils::config::{LinkerInputFlavor, LtoMode};

    fn profile(codegen_units: usize, lto_mode: LtoMode) -> ScriptProfile {
        ScriptProfile {
            name: "release".to_string(),
            opt: 3,
            debug: false,
            codegen_units,
            lto_mode,
        }
    }

    #[test]
    fn target_builds_keep_multi_object_outputs_without_full_lto() {
        assert!(profile_emit_multi_linker_input_dir(
            &profile(2, LtoMode::None),
            BuildDomain::Target,
        ));
        assert!(profile_emit_multi_linker_input_dir(
            &profile(2, LtoMode::Thin),
            BuildDomain::Target,
        ));
    }

    #[test]
    fn full_lto_or_non_target_domains_disable_multi_object_outputs() {
        assert!(!profile_emit_multi_linker_input_dir(
            &profile(2, LtoMode::Full),
            BuildDomain::Target,
        ));
        assert!(!profile_emit_multi_linker_input_dir(
            &profile(2, LtoMode::Thin),
            BuildDomain::Host,
        ));
        assert!(!profile_emit_multi_linker_input_dir(
            &profile(1, LtoMode::Thin),
            BuildDomain::Target,
        ));
    }

    #[test]
    fn target_thin_profiles_emit_thinlto_bitcode_linker_inputs() {
        assert_eq!(
            profile_linker_input_flavor(&profile(2, LtoMode::Thin), BuildDomain::Target),
            LinkerInputFlavor::ThinLtoBitcode,
        );
        assert_eq!(
            profile_linker_input_flavor(&profile(1, LtoMode::None), BuildDomain::Target),
            LinkerInputFlavor::Object,
        );
        assert_eq!(
            profile_linker_input_flavor(&profile(2, LtoMode::Thin), BuildDomain::Host),
            LinkerInputFlavor::Object,
        );
    }
}
