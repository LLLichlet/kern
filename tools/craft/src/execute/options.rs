//! Translation from Craft action plans to compiler/linker options.
//!
//! Option builders combine manifest profiles, runtime defaults, target kind,
//! generated roots, dependency aliases, and linker input policy into concrete
//! `CompileOptions` and link flags.

use super::{
    BuiltExternalPackage, BuiltStdPackage, ManifestRuntimeOptions, PackageInstanceKey,
    runtime_profile_key,
};
use crate::build_plan::{CompileAction, LinkAction};
use crate::error::{Error, Result};
use crate::graph::BuildDomain;
use crate::resolver::ExternalPackageId;
use crate::target_defaults::apply_target_runtime_defaults;
use kernc_utils::config::{
    CompileOptions, DriverMode, LibraryBundle, LinkerInputFlavor, LtoMode, OptLevel,
    inject_driver_condition_defines, maybe_add_base_alias, maybe_add_std_alias,
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
    if !options.module_interface_aliases.contains_key("base") {
        maybe_add_base_alias(options);
    }
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

pub(super) fn apply_manifest_runtime_options(
    manifest_path: &Path,
    manifest_runtime_options: &mut BTreeMap<std::path::PathBuf, ManifestRuntimeOptions>,
    target_kind: crate::plan::TargetKind,
    options: &mut CompileOptions,
) -> Result<()> {
    if let Some(cached) = manifest_runtime_options.get(manifest_path) {
        cached.apply_for_target(target_kind, options);
        return Ok(());
    }

    let manifest = crate::workspace::load_manifest_with_project_defaults(manifest_path)?;
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

fn resolve_link_search_paths(package_root: &Path, search_paths: &[String]) -> Vec<String> {
    search_paths
        .iter()
        .map(|path| {
            let candidate = Path::new(path);
            if candidate.is_absolute() {
                path.clone()
            } else {
                package_root.join(candidate).to_string_lossy().to_string()
            }
        })
        .collect()
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

fn compile_action_driver_mode(command: crate::script::ScriptCommand) -> DriverMode {
    match command {
        crate::script::ScriptCommand::Check => DriverMode::AnalyzeOnly,
        _ => DriverMode::CompileOnly,
    }
}

fn normalize_codegen_options_for_driver_mode(options: &mut CompileOptions) {
    if options.driver_mode != DriverMode::AnalyzeOnly {
        return;
    }

    options.codegen_units = 1;
    options.lto_mode = LtoMode::None;
    options.linker_input_flavor = LinkerInputFlavor::Object;
    options.emit_multi_linker_input_dir = false;
}

pub(super) fn normalize_windows_linker_input_options(options: &mut CompileOptions) {
    let is_windows_target = options.target.triple.to_string().contains("windows");
    if !is_windows_target || !options.driver_mode.emits_linker_input() || options.codegen_units <= 1
    {
        return;
    }

    // Preserving multiple COFF linker inputs for downstream package links is
    // not reliable yet: exported generic/runtime definitions can be left
    // undefined across the per-CGU object or ThinLTO bitcode outputs. Emit a
    // single linker input instead while keeping the selected linker-input
    // flavor (for example ThinLTO bitcode) intact.
    options.codegen_units = 1;
    options.emit_multi_linker_input_dir = false;
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
    command: crate::script::ScriptCommand,
    action: &CompileAction,
    local_library_actions: &BTreeMap<PackageInstanceKey, CompileAction>,
    built_std_packages: &BTreeMap<String, BuiltStdPackage>,
    built_external_packages: &BTreeMap<ExternalPackageId, BuiltExternalPackage>,
    manifest_runtime_options: &mut BTreeMap<std::path::PathBuf, ManifestRuntimeOptions>,
    report_timings: bool,
) -> Result<CompileOptions> {
    let mut options = CompileOptions {
        input_file: Some(action.source_path().to_string_lossy().to_string()),
        output_file: action.object_path.to_string_lossy().to_string(),
        metadata_output: action
            .metadata_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        test_mode: action.target_kind == crate::plan::TargetKind::Test,
        test_metadata_output: action
            .test_metadata_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        metadata_package_name: Some(action.package_id.name.clone()),
        metadata_package_version: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.version.clone()),
        root_module_name: (action.target_kind == crate::plan::TargetKind::Lib)
            .then(|| action.package_id.name.clone()),
        driver_mode: compile_action_driver_mode(command),
        report_progress: false,
        opt_level: profile_opt_level(&action.profile),
        debug_info: action.profile.debug,
        codegen_units: action.profile.codegen_units,
        lto_mode: action.profile.lto_mode,
        code_model: action.profile.code_model,
        linker_input_flavor: profile_linker_input_flavor(&action.profile, action.domain),
        emit_multi_linker_input_dir: profile_emit_multi_linker_input_dir(
            &action.profile,
            action.domain,
        ),
        report_timings,
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
    let runtime_package = built_std_packages.get(&runtime_profile_key(&action.profile));
    let std_package = matches!(options.library_bundle, LibraryBundle::Std)
        .then_some(runtime_package)
        .flatten();
    options.module_interface_aliases = compile_module_aliases(
        action,
        local_library_actions,
        std_package,
        built_external_packages,
    )?;
    if matches!(options.library_bundle, LibraryBundle::Base)
        && action.package_id.name != "base"
        && !options.module_interface_aliases.contains_key("base")
        && let Some(runtime_package) = runtime_package
        && let Some(base_metadata) = runtime_package.interface_aliases.get("base")
    {
        options.module_interface_aliases.insert(
            "base".to_string(),
            base_metadata.to_string_lossy().to_string(),
        );
    }
    if action.target_kind == crate::plan::TargetKind::Lib {
        options.module_aliases.insert(
            action.package_id.name.clone(),
            action.source_path().to_string_lossy().to_string(),
        );
    }
    inject_target_library_aliases(&mut options);
    inject_driver_condition_defines(&mut options);
    options.custom_defines.extend(compile_time_defines(
        &action.cfg,
        &action.define,
        action.source_path(),
    )?);
    normalize_codegen_options_for_driver_mode(&mut options);
    normalize_windows_linker_input_options(&mut options);
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
    options.linker_search_paths =
        resolve_link_search_paths(&action.package_root_path, &action.link.search_paths);
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
    if let Ok(toolchain_root) = std::env::var("KERN_TOOLCHAIN_ROOT")
        && !toolchain_root.is_empty()
    {
        options.toolchain_root = Some(toolchain_root);
    }
    if let Ok(cc_env) = std::env::var("CC")
        && !cc_env.is_empty()
    {
        options.linker_cmd = cc_env;
        options.linker_cmd_explicit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BuiltStdPackage, runtime_profile_key};
    use super::{
        compile_action_options, profile_emit_multi_linker_input_dir, profile_linker_input_flavor,
    };
    use crate::build_plan::{CompileAction, CompileSourceInput};
    use crate::graph::{BuildDomain, PackageId, SourceId};
    use crate::plan::TargetKind;
    use crate::script::ScriptProfile;
    use kernc_utils::config::{CodeModel, LibraryBundle, LinkerInputFlavor, LtoMode};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn profile(codegen_units: usize, lto_mode: LtoMode) -> ScriptProfile {
        ScriptProfile {
            name: "release".to_string(),
            opt: 3,
            debug: false,
            codegen_units,
            lto_mode,
            code_model: CodeModel::Default,
        }
    }

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let unique = format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn lib_action(root: &Path, package_name: &str, manifest_path: PathBuf) -> CompileAction {
        let source_path = root.join("src/lib.kn");
        CompileAction {
            domain: BuildDomain::Target,
            package_id: PackageId {
                name: package_name.to_string(),
                version: "0.1.0".to_string(),
                source: SourceId::Root,
            },
            manifest_path,
            target_kind: TargetKind::Lib,
            target_name: None,
            artifact_name: package_name.to_string(),
            generated_root_path: root.join("gen"),
            source_input: CompileSourceInput::AbsolutePath(source_path),
            metadata_path: Some(root.join("meta")),
            test_metadata_path: None,
            object_path: root.join(format!("{package_name}.o")),
            artifact_path: root.join(format!("lib{package_name}.a")),
            profile: ScriptProfile {
                name: "dev".to_string(),
                opt: 0,
                debug: true,
                codegen_units: 1,
                lto_mode: LtoMode::None,
                code_model: CodeModel::Default,
            },
            cfg: BTreeMap::new(),
            define: BTreeMap::new(),
            compile_inputs: Vec::new(),
            local_dependencies: Vec::new(),
            external_dependencies: Vec::new(),
        }
    }

    fn built_std_package(
        root: &Path,
        profile: &ScriptProfile,
    ) -> BTreeMap<String, BuiltStdPackage> {
        BTreeMap::from([(
            runtime_profile_key(profile),
            BuiltStdPackage {
                metadata_root_path: root.join("prebuilt-std"),
                base_object_path: root.join("base.o"),
                rt_object_path: Some(root.join("rt.o")),
                common_link_objects: Vec::new(),
                hosted_entry_object_path: root.join("hosted-entry.o"),
                freestanding_entry_object_path: root.join("freestanding-entry.o"),
                interface_aliases: BTreeMap::from([(
                    "base".to_string(),
                    root.join("prebuilt-base"),
                )]),
            },
        )])
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

    #[test]
    fn compile_action_options_thread_profile_debug_into_compile_options() {
        let root = temp_dir("craft-debug-options");
        let manifest_path = root.join("Craft.toml");
        fs::write(
            &manifest_path,
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nkern = \"0.7.8\"\n",
        )
        .unwrap();
        let source_path = root.join("src/main.kn");
        let action = CompileAction {
            domain: BuildDomain::Target,
            package_id: PackageId {
                name: "demo".to_string(),
                version: "0.1.0".to_string(),
                source: SourceId::Root,
            },
            manifest_path: manifest_path.clone(),
            target_kind: TargetKind::Bin,
            target_name: Some("demo".to_string()),
            artifact_name: "demo".to_string(),
            generated_root_path: root.join("gen"),
            source_input: CompileSourceInput::AbsolutePath(source_path.clone()),
            metadata_path: None,
            test_metadata_path: None,
            object_path: root.join("demo.o"),
            artifact_path: root.join("demo"),
            profile: ScriptProfile {
                name: "dev".to_string(),
                opt: 0,
                debug: true,
                codegen_units: 1,
                lto_mode: LtoMode::None,
                code_model: CodeModel::Default,
            },
            cfg: BTreeMap::new(),
            define: BTreeMap::new(),
            compile_inputs: Vec::new(),
            local_dependencies: Vec::new(),
            external_dependencies: Vec::new(),
        };
        let mut manifest_runtime_options = BTreeMap::new();
        let options = compile_action_options(
            crate::script::ScriptCommand::Build,
            &action,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        assert!(options.debug_info);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn base_bundle_lib_targets_import_prebuilt_base_interface_only() {
        let root = temp_dir("craft-base-bundle-aliases");
        let manifest_path = root.join("Craft.toml");
        fs::write(
            &manifest_path,
            r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.8"

[runtime]
bundle = "base"

[lib]
root = "src/lib.kn"
"#,
        )
        .unwrap();
        let action = lib_action(&root, "demo", manifest_path);
        let built_std_packages = built_std_package(&root, &action.profile);
        let mut manifest_runtime_options = BTreeMap::new();

        let options = compile_action_options(
            crate::script::ScriptCommand::Build,
            &action,
            &BTreeMap::new(),
            &built_std_packages,
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        assert_eq!(options.library_bundle, LibraryBundle::Base);
        assert!(!options.module_interface_aliases.contains_key("std"));
        assert_eq!(
            options
                .module_interface_aliases
                .get("base")
                .map(String::as_str),
            Some(root.join("prebuilt-base").to_string_lossy().as_ref())
        );
        assert_eq!(
            options.module_aliases.get("demo").map(String::as_str),
            Some(action.source_path().to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn base_package_lib_targets_keep_base_alias_for_their_own_source() {
        let root = temp_dir("craft-base-self-alias");
        let manifest_path = root.join("Craft.toml");
        fs::write(
            &manifest_path,
            r#"
[package]
name = "base"
version = "0.1.0"
kern = "0.7.8"

[runtime]
bundle = "base"

[lib]
root = "src/lib.kn"
"#,
        )
        .unwrap();
        let action = lib_action(&root, "base", manifest_path);
        let built_std_packages = built_std_package(&root, &action.profile);
        let mut manifest_runtime_options = BTreeMap::new();

        let options = compile_action_options(
            crate::script::ScriptCommand::Build,
            &action,
            &BTreeMap::new(),
            &built_std_packages,
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        assert_eq!(options.library_bundle, LibraryBundle::Base);
        assert!(!options.module_interface_aliases.contains_key("base"));
        assert_eq!(
            options.module_aliases.get("base").map(String::as_str),
            Some(action.source_path().to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rt_lib_targets_keep_official_library_aliases_absent() {
        let root = temp_dir("craft-rt-no-library-aliases");
        let manifest_path = root.join("Craft.toml");
        fs::write(
            &manifest_path,
            r#"
[package]
name = "rt"
version = "0.1.0"
kern = "0.7.8"

[runtime]
bundle = "none"

[lib]
root = "src/lib.kn"
"#,
        )
        .unwrap();
        let action = lib_action(&root, "rt", manifest_path);
        let built_std_packages = built_std_package(&root, &action.profile);
        let mut manifest_runtime_options = BTreeMap::new();

        let options = compile_action_options(
            crate::script::ScriptCommand::Build,
            &action,
            &BTreeMap::new(),
            &built_std_packages,
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        assert_eq!(options.library_bundle, LibraryBundle::None);
        assert!(options.module_interface_aliases.is_empty());
        assert_eq!(
            options.module_aliases.get("rt").map(String::as_str),
            Some(action.source_path().to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn std_named_lib_targets_keep_the_std_alias_for_their_own_source() {
        let root = temp_dir("craft-std-self-alias");
        let manifest_path = root.join("Craft.toml");
        fs::write(
            &manifest_path,
            r#"
[package]
name = "std"
version = "0.1.0"
kern = "0.7.8"

[runtime]
bundle = "std"

[lib]
root = "src/lib.kn"
"#,
        )
        .unwrap();
        let action = lib_action(&root, "std", manifest_path);
        let built_std_packages = built_std_package(&root, &action.profile);
        let mut manifest_runtime_options = BTreeMap::new();

        let options = compile_action_options(
            crate::script::ScriptCommand::Build,
            &action,
            &BTreeMap::new(),
            &built_std_packages,
            &BTreeMap::new(),
            &mut manifest_runtime_options,
            false,
        )
        .unwrap();

        assert_eq!(options.library_bundle, LibraryBundle::Std);
        assert!(!options.module_interface_aliases.contains_key("std"));
        assert!(options.module_interface_aliases.contains_key("base"));
        assert_eq!(
            options.module_aliases.get("std").map(String::as_str),
            Some(action.source_path().to_string_lossy().as_ref())
        );

        let _ = fs::remove_dir_all(root);
    }
}
