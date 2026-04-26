use super::{DependencySpec, Manifest, ReleaseSourcePolicy};
use crate::plan::TargetKind;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};

#[test]
fn parses_package_manifest() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"
description = "Demo package"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/demo"

[lib]
root = "src/lib.rn"

[[bin]]
name = "demo"
root = "src/main.rn"

[test]
roots = ["tests/smoke.rn", "tests/env.rn"]

[example]
roots = ["examples/hello.rn"]

[dependencies]
alloc = { path = "../alloc", features = ["arena"] }
toml = { git = "https://example.com/toml.git", tag = "v0.1.0" }

[features]
default = []
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let package = manifest.package.as_ref().unwrap();
    assert_eq!(package.name, "demo");
    assert_eq!(package.description.as_deref(), Some("Demo package"));
    assert!(manifest.lib.is_some());
    assert_eq!(manifest.bin.len(), 1);
    assert_eq!(manifest.test.len(), 2);
    assert_eq!(manifest.example.len(), 1);
    assert_eq!(manifest.test[0].name, "smoke");
    assert_eq!(manifest.test[1].name, "env");
    assert_eq!(manifest.example[0].name, "hello");
    assert_eq!(manifest.dependencies.len(), 2);
}

#[test]
fn parses_workspace_inherited_dependency() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[dependencies]
shared = { workspace = true, features = ["simd"] }
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let dep = manifest.dependencies.get("shared").unwrap();
    let DependencySpec::Detailed(dep) = dep else {
        panic!("expected detailed dependency");
    };

    assert_eq!(dep.workspace, Some(true));
    assert_eq!(dep.features, vec!["simd"]);
}

#[test]
fn parses_package_resources() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[resources]
limine = { git = "https://example.com/limine.git", branch = "main" }
assets = { path = "vendor/assets" }
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let limine = manifest.resources.get("limine").unwrap();
    assert_eq!(
        limine.git.as_deref(),
        Some("https://example.com/limine.git")
    );
    assert_eq!(limine.branch.as_deref(), Some("main"));
    let assets = manifest.resources.get("assets").unwrap();
    assert_eq!(assets.path.as_deref(), Some("vendor/assets"));
}

#[test]
fn rejects_invalid_resource_source_combinations() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[resources]
limine = { path = "vendor/limine", git = "https://example.com/limine.git" }
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(err.to_string().contains("cannot combine `path` and `git`"));
}

#[test]
fn rejects_plain_version_dependencies() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[dependencies]
log = "1"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("plain version strings are unsupported")
    );
}

#[test]
fn rejects_unsupported_source_tables() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[source.default]
git = "https://example.com/default.git"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported table `[source.default]`")
    );
}

#[test]
fn parses_craft_release_source_policy_overrides() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[craft]
release-source-policy = "warn"
allow-floating-git = ["default"]
allow-insecure-source = ["mirror"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let craft = manifest.craft.as_ref().unwrap();
    assert_eq!(craft.release_source_policy, Some(ReleaseSourcePolicy::Warn));
    assert_eq!(craft.allow_floating_git, vec!["default"]);
    assert_eq!(craft.allow_insecure_source, vec!["mirror"]);
}

#[test]
fn rejects_invalid_release_source_policy_value() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[craft]
release-source-policy = "strict"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("release-source-policy has unsupported value")
    );
}

#[test]
fn parses_runtime_section() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[runtime]
entry = "crt"
libc = true
bundle = "std"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let runtime = manifest.runtime.as_ref().expect("expected runtime section");
    assert_eq!(runtime.entry, Some(RuntimeEntry::Crt));
    assert_eq!(runtime.libc, Some(true));
    assert_eq!(runtime.bundle, Some(LibraryBundle::Std));
}

#[test]
fn rejects_unknown_runtime_provider_key() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[runtime]
provider = "toolchain"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported [runtime] key `provider`")
    );
}

#[test]
fn runtime_section_applies_to_compile_options() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[runtime]
entry = "rt"
libc = false
bundle = "std"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let mut options = CompileOptions::default();
    manifest.apply_runtime_options(&mut options);

    assert_eq!(options.runtime_entry, RuntimeEntry::Rt);
    assert!(!options.runtime_libc);
    assert_eq!(options.library_bundle, LibraryBundle::Std);
}

#[test]
fn runtime_entry_does_not_override_lib_target_defaults() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[runtime]
entry = "rt"
libc = false
bundle = "base"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let mut options = CompileOptions {
        runtime_entry: RuntimeEntry::None,
        runtime_libc: false,
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    };

    manifest.apply_runtime_options_for_target(TargetKind::Lib, &mut options);

    assert_eq!(options.runtime_entry, RuntimeEntry::None);
    assert!(!options.runtime_libc);
    assert_eq!(options.library_bundle, LibraryBundle::Base);
}

#[test]
fn runtime_entry_overrides_test_target_defaults() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[runtime]
entry = "rt"
libc = false
bundle = "base"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let mut options = CompileOptions {
        runtime_entry: RuntimeEntry::Rt,
        runtime_libc: false,
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    };

    manifest.apply_runtime_options_for_target(TargetKind::Test, &mut options);

    assert_eq!(options.runtime_entry, RuntimeEntry::Rt);
    assert!(!options.runtime_libc);
    assert_eq!(options.library_bundle, LibraryBundle::Base);
}

#[test]
fn profile_section_parses_codegen_units() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[profile.release]
opt = 3
debug = false
codegen-units = 4
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let profile = manifest
        .profile
        .as_ref()
        .and_then(|profiles| profiles.release.as_ref())
        .expect("expected release profile");
    assert_eq!(profile.codegen_units, Some(4));
}

#[test]
fn profile_section_parses_lto_mode() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[profile.release]
lto = "thin"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let profile = manifest
        .profile
        .as_ref()
        .and_then(|profiles| profiles.release.as_ref())
        .expect("expected release profile");
    assert_eq!(profile.lto.as_deref(), Some("thin"));
}

#[test]
fn rejects_zero_profile_codegen_units() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[profile.dev]
codegen-units = 0
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(format!("{err}").contains("[profile.dev].codegen-units must be greater than zero"));
}

#[test]
fn rejects_invalid_profile_lto_mode() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[profile.release]
lto = "turbo"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("invalid LTO mode `turbo`"));
}

#[test]
fn rejects_package_edition_field() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"
edition = "2027"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported [package] key `edition`")
    );
}

#[test]
fn rejects_workspace_package_edition_field() {
    let err = Manifest::parse(
        r#"
[workspace]
members = ["app"]

[workspace.package]
edition = "2027"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported [workspace.package] key `edition`")
    );
}

#[test]
fn rejects_mismatched_kern_version() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("must match the current toolchain version")
    );
}

#[test]
fn rejects_duplicate_test_file_stems() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[test]
roots = ["tests/smoke.rn", "alt/smoke.rn"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("duplicate file stem `smoke` in [test].roots")
    );
}

#[test]
fn rejects_glob_patterns_in_test_roots() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[test]
roots = ["tests/*"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(err.to_string().contains("does not support glob patterns"));
}

#[test]
fn rejects_array_table_test_targets() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[test]]
name = "smoke"
root = "tests/smoke.rn"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported array table `[[test]]`")
    );
}

#[test]
fn rejects_array_table_example_targets() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.7.2"

[[example]]
name = "hello"
root = "examples/hello.rn"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported array table `[[example]]`")
    );
}
