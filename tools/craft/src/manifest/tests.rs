//! Manifest parsing and validation tests.

use super::{CraftStyleSuggestionLevel, DependencySpec, Manifest, ReleaseSourcePolicy};
use crate::plan::TargetKind;
use kernc_utils::config::{CompileOptions, LibraryBundle, RuntimeEntry};

#[test]
fn parses_package_manifest() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"
description = "Demo package"
license = "MIT"
authors = ["Demo <demo@example.com>"]
readme = "README.md"
repository = "https://example.com/demo"

[lib]
root = "src/lib.kn"

[[bin]]
name = "demo"
root = "src/main.kn"

[test]
roots = ["tests/smoke.kn", "tests/env.kn"]

[example]
roots = ["examples/hello.kn"]

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
kern = "0.8.0"

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
fn parses_workspace_namespace_exports() {
    let manifest = Manifest::parse(
        r#"
[workspace]
name = "json-kern"
members = ["json", "json-test"]

[workspace.exports]
json = { member = "json" }
schema = { member = "json-schema" }
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let workspace = manifest.workspace.as_ref().unwrap();
    assert_eq!(workspace.name, "json-kern");
    assert_eq!(workspace.exports["json"].member, "json");
    assert_eq!(workspace.exports["schema"].member, "json-schema");
}

#[test]
fn rejects_package_and_workspace_in_same_manifest() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[workspace]
name = "demo"
members = []
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot declare both `[package]` and `[workspace]`")
    );
}

#[test]
fn parses_package_resources() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
fn parses_craft_style_config() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.style]
suggestions = "warn"
disabled-rules = ["index-while"]
exclude = ["src/generated/**"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let style = manifest.craft.as_ref().unwrap().style.as_ref().unwrap();
    assert_eq!(style.suggestions, Some(CraftStyleSuggestionLevel::Warn));
    assert_eq!(style.disabled_rules, vec!["index-while"]);
    assert_eq!(style.exclude, vec!["src/generated/**"]);
    manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap();
}

#[test]
fn parses_craft_fmt_config() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.fmt]
line-width = 88
postfix-chain-threshold = 4
boolean-chain-threshold = 2
function-parameter-threshold = 5
call-argument-threshold = 6
exclude = ["src/generated/**"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let fmt = manifest.craft.as_ref().unwrap().fmt.as_ref().unwrap();
    assert_eq!(fmt.line_width, Some(88));
    assert_eq!(fmt.postfix_chain_threshold, Some(4));
    assert_eq!(fmt.boolean_chain_threshold, Some(2));
    assert_eq!(fmt.function_parameter_threshold, Some(5));
    assert_eq!(fmt.call_argument_threshold, Some(6));
    assert_eq!(fmt.exclude, vec!["src/generated/**"]);
    manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap();
}

#[test]
fn rejects_tiny_craft_fmt_line_width() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.fmt]
line-width = 20
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(err.to_string().contains("line-width must be at least 40"));
}

#[test]
fn rejects_tiny_craft_fmt_threshold() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.fmt]
postfix-chain-threshold = 1
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("postfix-chain-threshold must be at least 2")
    );
}

#[test]
fn rejects_unknown_craft_style_rule() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.style]
disabled-rules = ["unknown-rule"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(err.to_string().contains("unknown style rule"));
}

#[test]
fn rejects_invalid_release_source_policy_value() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

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
fn rejects_invalid_craft_style_suggestion_level() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[craft.style]
suggestions = "strict"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("[craft.style].suggestions has unsupported value")
    );
}

#[test]
fn parses_runtime_section() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
kern = "0.8.0"

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
fn profile_section_parses_code_model() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[profile.release]
code-model = "kernel"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let profile = manifest
        .profile
        .as_ref()
        .and_then(|profiles| profiles.release.as_ref())
        .expect("expected release profile");
    assert_eq!(profile.code_model.as_deref(), Some("kernel"));
}

#[test]
fn rejects_zero_profile_codegen_units() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

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
kern = "0.8.0"

[profile.release]
lto = "turbo"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("invalid LTO mode `turbo`"));
}

#[test]
fn rejects_invalid_profile_code_model() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[profile.release]
code-model = "huge"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();
    assert!(format!("{err}").contains("invalid code model `huge`"));
}

#[test]
fn rejects_package_edition_field() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"
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
name = "workspace"
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
fn accepts_current_kern_minor_line() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap();
}

#[test]
fn rejects_mismatched_kern_minor_line() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.9"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    let err = manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("must match the current Kern minor line")
    );
}

#[test]
fn rejects_duplicate_test_file_stems() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[test]
roots = ["tests/smoke.kn", "alt/smoke.kn"]
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
fn parses_glob_patterns_in_test_roots() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[test]
roots = ["tests/*"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    assert!(manifest.test_roots_explicit);
    assert_eq!(manifest.test.len(), 1);
    assert_eq!(manifest.test[0].name, "*");
    assert_eq!(manifest.test[0].root, "tests/*");
}

#[test]
fn accepts_multiple_glob_patterns_in_test_roots() {
    let manifest = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[test]
roots = ["tests/*.kn", "integration/*.kn"]
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap();

    manifest
        .validate(std::path::Path::new("Craft.toml"))
        .unwrap();
    assert_eq!(manifest.test.len(), 2);
}

#[test]
fn rejects_glob_patterns_in_example_roots() {
    let err = Manifest::parse(
        r#"
[package]
name = "demo"
version = "0.1.0"
kern = "0.8.0"

[example]
roots = ["examples/*.kn"]
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
kern = "0.8.0"

[[test]]
name = "smoke"
root = "tests/smoke.kn"
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
kern = "0.8.0"

[[example]]
name = "hello"
root = "examples/hello.kn"
"#,
        std::path::Path::new("Craft.toml"),
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("unsupported array table `[[example]]`")
    );
}

#[test]
fn deterministic_manifest_parser_fuzz_smoke_does_not_panic() {
    for seed in 0..384u64 {
        let source = fuzz_manifest(seed);
        let result = std::panic::catch_unwind(|| {
            if let Ok(manifest) = Manifest::parse(&source, std::path::Path::new("Craft.toml")) {
                let _ = manifest.validate(std::path::Path::new("Craft.toml"));
            }
        });
        assert!(
            result.is_ok(),
            "manifest parser fuzz seed {seed} panicked with source:\n{source}"
        );
    }
}

fn fuzz_manifest(seed: u64) -> String {
    const FRAGMENTS: &[&str] = &[
        "[package]\n",
        "[workspace]\n",
        "[dependencies]\n",
        "[dev-dependencies]\n",
        "[build-dependencies]\n",
        "[resources]\n",
        "[features]\n",
        "[runtime]\n",
        "[craft]\n",
        "[craft.fmt]\n",
        "[craft.style]\n",
        "[lib]\n",
        "[test]\n",
        "[example]\n",
        "[profile.dev]\n",
        "[profile.release]\n",
        "[[bin]]\n",
        "[[unknown]]\n",
        "name = \"demo\"\n",
        "version = \"0.1.0\"\n",
        "kern = \"0.8.0\"\n",
        "root = \"src/main.kn\"\n",
        "roots = [\"tests/a.kn\", \"tests/**/*.kn\"]\n",
        "members = [\"a\", \"b\"]\n",
        "default = []\n",
        "feature = [\"a\", \"b\"]\n",
        "libc = true\n",
        "code-model = \"kernel\"\n",
        "library-bundle = \"base\"\n",
        "entry = \"rt\"\n",
        "line-width = 100\n",
        "suggestions = \"warn\"\n",
        "disabled-rules = [\"a\", \"b\"]\n",
        "dep = { path = \"../dep\", features = [\"x\"] }\n",
        "dep = { git = \"https://example.com/dep.git\", tag = \"v1\" }\n",
        "dep = { workspace = true }\n",
        "bad = \"unterminated\n",
        "bad = [\"unterminated]\n",
        "bad = { path = \"x\", git = \"y\" }\n",
        "bad = \n",
        "= \"missing-key\"\n",
        "# comment\n",
        "unknown = \"value\"\n",
        "\n",
    ];

    let mut rng = FuzzRng::new(seed ^ 0x632b_d7d1_2f81_b35b);
    let mut source = String::new();
    let target_len = 8 + rng.range(64) as usize;
    for _ in 0..target_len {
        source.push_str(FRAGMENTS[rng.range(FRAGMENTS.len() as u64) as usize]);
    }
    source
}

struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn range(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}
