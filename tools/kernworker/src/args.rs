//! Argument parsing and help text for `kernworker`.
//!
//! Parsing is intentionally hand-written so CI/release commands can reject
//! ambiguous options with precise messages and keep the shipped binary light.

use shared_cli::{ColorChoice, HelpDoc, HelpSection};
use shared_ops::{OpsError, OpsResult};
use std::path::PathBuf;

pub enum Command {
    Ci(CiCommand),
    Release(ReleaseCommand),
    Help,
}

#[derive(Debug)]
pub enum CiCommand {
    KerncTests { mode: TestMode },
    CraftPolicy,
    ActivateToolchain(ActivateToolchainArgs),
    ToolchainInfo,
    ToolchainHealth,
    ToolchainSpec(ToolchainSpecArgs),
    VerifyToolchainArchive(ToolchainArchiveArgs),
    VerifyPackagedToolchain(PackagedToolchainVerifyArgs),
    InstallPackagedToolchain(PackagedToolchainInstallArgs),
    VerifyVsix(VsixVerifyArgs),
    Help,
}

#[derive(Debug)]
pub enum ReleaseCommand {
    Package(ReleasePackageArgs),
    PackageToolchain(ReleaseToolchainPackageArgs),
    WriteChecksums(ReleaseChecksumsArgs),
    Help,
}

#[derive(Debug, Clone, Copy)]
pub enum TestMode {
    Smoke,
    Hosted,
    All,
}

#[derive(Debug, Default)]
pub struct ToolchainSpecArgs {
    pub runner_os: Option<String>,
    pub mode: String,
    pub host_target: Option<String>,
    pub format: String,
}

#[derive(Debug, Default)]
pub struct ActivateToolchainArgs {
    pub prefix: Option<PathBuf>,
    pub format: String,
}

#[derive(Debug, Default)]
pub struct ToolchainArchiveArgs {
    pub runner_os: Option<String>,
    pub mode: String,
    pub host_target: Option<String>,
    pub archive_path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct PackagedToolchainVerifyArgs {
    pub archive_path: Option<PathBuf>,
    pub target: Option<String>,
}

#[derive(Debug, Default)]
pub struct PackagedToolchainInstallArgs {
    pub archive_path: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub target: Option<String>,
    pub format: String,
}

#[derive(Debug, Default)]
pub struct VsixVerifyArgs {
    pub package_json: Option<PathBuf>,
    pub vsix_path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct ReleasePackageArgs {
    pub version: Option<String>,
    pub target: Option<String>,
    pub skip_build: bool,
    pub skip_kernup: bool,
    pub toolchain_prefix: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct ReleaseToolchainPackageArgs {
    pub version: Option<String>,
    pub target: Option<String>,
    pub toolchain_prefix: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct ReleaseChecksumsArgs {
    pub paths: Vec<String>,
    pub manifest_path: Option<PathBuf>,
    pub channel: String,
    pub release_tag: Option<String>,
}
pub fn parse_args(args: Vec<String>) -> OpsResult<Command> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match command {
        "ci" => parse_ci_args(&args[1..]).map(Command::Ci),
        "release" => parse_release_args(&args[1..]).map(Command::Release),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(OpsError::new(format!(
            "unknown command `{other}`; run `kernworker help`"
        ))),
    }
}

fn parse_ci_args(args: &[String]) -> OpsResult<CiCommand> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(CiCommand::Help);
    };

    match command {
        "kernc-tests" => parse_kernc_tests_args(&args[1..]),
        "craft-policy" => Ok(CiCommand::CraftPolicy),
        "activate-toolchain" => {
            parse_activate_toolchain_args(&args[1..]).map(CiCommand::ActivateToolchain)
        }
        "toolchain-info" => Ok(CiCommand::ToolchainInfo),
        "toolchain-health" => Ok(CiCommand::ToolchainHealth),
        "toolchain-spec" => parse_toolchain_spec_args(&args[1..]).map(CiCommand::ToolchainSpec),
        "verify-toolchain-archive" => {
            parse_toolchain_archive_args(&args[1..]).map(CiCommand::VerifyToolchainArchive)
        }
        "verify-packaged-toolchain" => {
            parse_packaged_toolchain_verify_args(&args[1..]).map(CiCommand::VerifyPackagedToolchain)
        }
        "install-packaged-toolchain" => parse_packaged_toolchain_install_args(&args[1..])
            .map(CiCommand::InstallPackagedToolchain),
        "verify-vsix" => parse_verify_vsix_args(&args[1..]).map(CiCommand::VerifyVsix),
        "help" | "--help" | "-h" => Ok(CiCommand::Help),
        other => Err(OpsError::new(format!(
            "unknown ci command `{other}`; run `kernworker ci help`"
        ))),
    }
}

fn parse_release_args(args: &[String]) -> OpsResult<ReleaseCommand> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(ReleaseCommand::Help);
    };

    match command {
        "package" => parse_release_package_args(&args[1..]).map(ReleaseCommand::Package),
        "package-toolchain" => {
            parse_release_toolchain_package_args(&args[1..]).map(ReleaseCommand::PackageToolchain)
        }
        "write-checksums" => {
            parse_release_checksums_args(&args[1..]).map(ReleaseCommand::WriteChecksums)
        }
        "help" | "--help" | "-h" => Ok(ReleaseCommand::Help),
        other => Err(OpsError::new(format!(
            "unknown release command `{other}`; run `kernworker release help`"
        ))),
    }
}

fn parse_release_package_args(args: &[String]) -> OpsResult<ReleasePackageArgs> {
    let mut parsed = ReleasePackageArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--version" => {
                index += 1;
                parsed.version = Some(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--version` requires a value"))?
                        .to_string(),
                );
            }
            "--target" => {
                index += 1;
                parsed.target = Some(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--target` requires a value"))?
                        .to_string(),
                );
            }
            "--skip-build" => parsed.skip_build = true,
            "--skip-kernup" => parsed.skip_kernup = true,
            "--toolchain-prefix" => {
                index += 1;
                parsed.toolchain_prefix =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        OpsError::new("`--toolchain-prefix` requires a value")
                    })?));
            }
            "--help" | "-h" => {
                print!("{}", release_package_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected release package argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_release_toolchain_package_args(args: &[String]) -> OpsResult<ReleaseToolchainPackageArgs> {
    let mut parsed = ReleaseToolchainPackageArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--version" => {
                index += 1;
                parsed.version = Some(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--version` requires a value"))?
                        .to_string(),
                );
            }
            "--target" => {
                index += 1;
                parsed.target = Some(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--target` requires a value"))?
                        .to_string(),
                );
            }
            "--toolchain-prefix" => {
                index += 1;
                parsed.toolchain_prefix =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        OpsError::new("`--toolchain-prefix` requires a value")
                    })?));
            }
            "--help" | "-h" => {
                print!(
                    "{}",
                    release_package_toolchain_help().render(ColorChoice::Auto)
                );
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected release package-toolchain argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_release_checksums_args(args: &[String]) -> OpsResult<ReleaseChecksumsArgs> {
    let mut parsed = ReleaseChecksumsArgs {
        channel: "release".into(),
        ..ReleaseChecksumsArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--manifest-path" => {
                index += 1;
                parsed.manifest_path =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        OpsError::new("`--manifest-path` requires a value")
                    })?));
            }
            "--channel" => {
                index += 1;
                parsed.channel = args
                    .get(index)
                    .ok_or_else(|| OpsError::new("`--channel` requires a value"))?
                    .to_string();
            }
            "--release-tag" => {
                index += 1;
                parsed.release_tag = Some(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--release-tag` requires a value"))?
                        .to_string(),
                );
            }
            "--help" | "-h" => {
                print!("{}", release_checksums_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(OpsError::new(format!(
                    "unexpected write-checksums argument `{value}`"
                )));
            }
            value => parsed.paths.push(value.to_string()),
        }
        index += 1;
    }
    if parsed.paths.is_empty() {
        return Err(OpsError::new(
            "`kernworker release write-checksums` requires at least one path or glob pattern",
        ));
    }
    Ok(parsed)
}

fn parse_kernc_tests_args(args: &[String]) -> OpsResult<CiCommand> {
    let mut mode = TestMode::All;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--mode` requires a value"));
                };
                mode = match value.as_str() {
                    "smoke" => TestMode::Smoke,
                    "hosted" => TestMode::Hosted,
                    "all" => TestMode::All,
                    other => {
                        return Err(OpsError::new(format!(
                            "unsupported kernc test mode `{other}`"
                        )));
                    }
                };
            }
            "--help" | "-h" => {
                print!("{}", kernc_tests_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected kernc-tests argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(CiCommand::KerncTests { mode })
}

fn parse_toolchain_spec_args(args: &[String]) -> OpsResult<ToolchainSpecArgs> {
    let mut parsed = ToolchainSpecArgs {
        mode: "current".into(),
        format: "text".into(),
        ..ToolchainSpecArgs::default()
    };
    parse_common_toolchain_args(args, |key, value| {
        match key {
            "--runner-os" => parsed.runner_os = Some(value.to_string()),
            "--mode" => parsed.mode = value.to_string(),
            "--host-target" => parsed.host_target = Some(value.to_string()),
            "--format" => parsed.format = value.to_string(),
            other => {
                return Err(OpsError::new(format!(
                    "unexpected toolchain-spec argument `{other}`"
                )));
            }
        }
        Ok(())
    })?;
    Ok(parsed)
}

fn parse_activate_toolchain_args(args: &[String]) -> OpsResult<ActivateToolchainArgs> {
    let mut parsed = ActivateToolchainArgs {
        format: "text".into(),
        ..ActivateToolchainArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--prefix" => {
                index += 1;
                parsed.prefix =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        OpsError::new("`--prefix` requires a value")
                    })?));
            }
            "--format" => {
                index += 1;
                parsed.format = args
                    .get(index)
                    .ok_or_else(|| OpsError::new("`--format` requires a value"))?
                    .to_string();
            }
            "--help" | "-h" => {
                print!("{}", activate_toolchain_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected activate-toolchain argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_toolchain_archive_args(args: &[String]) -> OpsResult<ToolchainArchiveArgs> {
    let mut parsed = ToolchainArchiveArgs {
        mode: "current".into(),
        ..ToolchainArchiveArgs::default()
    };
    parse_common_toolchain_args(args, |key, value| {
        match key {
            "--runner-os" => parsed.runner_os = Some(value.to_string()),
            "--mode" => parsed.mode = value.to_string(),
            "--host-target" => parsed.host_target = Some(value.to_string()),
            "--archive-path" => parsed.archive_path = Some(PathBuf::from(value)),
            other => {
                return Err(OpsError::new(format!(
                    "unexpected verify-toolchain-archive argument `{other}`"
                )));
            }
        }
        Ok(())
    })?;
    Ok(parsed)
}

fn parse_packaged_toolchain_verify_args(args: &[String]) -> OpsResult<PackagedToolchainVerifyArgs> {
    let mut parsed = PackagedToolchainVerifyArgs::default();
    parse_common_toolchain_args(args, |key, value| {
        match key {
            "--archive-path" => parsed.archive_path = Some(PathBuf::from(value)),
            "--target" => parsed.target = Some(value.to_string()),
            other => {
                return Err(OpsError::new(format!(
                    "unexpected verify-packaged-toolchain argument `{other}`"
                )));
            }
        }
        Ok(())
    })?;
    Ok(parsed)
}

fn parse_packaged_toolchain_install_args(
    args: &[String],
) -> OpsResult<PackagedToolchainInstallArgs> {
    let mut parsed = PackagedToolchainInstallArgs {
        format: "text".into(),
        ..PackagedToolchainInstallArgs::default()
    };
    parse_common_toolchain_args(args, |key, value| {
        match key {
            "--archive-path" => parsed.archive_path = Some(PathBuf::from(value)),
            "--dest" => parsed.dest = Some(PathBuf::from(value)),
            "--target" => parsed.target = Some(value.to_string()),
            "--format" => parsed.format = value.to_string(),
            other => {
                return Err(OpsError::new(format!(
                    "unexpected install-packaged-toolchain argument `{other}`"
                )));
            }
        }
        Ok(())
    })?;
    Ok(parsed)
}

fn parse_verify_vsix_args(args: &[String]) -> OpsResult<VsixVerifyArgs> {
    let mut parsed = VsixVerifyArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--package-json" => {
                index += 1;
                parsed.package_json =
                    Some(PathBuf::from(args.get(index).ok_or_else(|| {
                        OpsError::new("`--package-json` requires a value")
                    })?));
            }
            "--vsix" => {
                index += 1;
                parsed.vsix_path = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| OpsError::new("`--vsix` requires a value"))?,
                ));
            }
            "--help" | "-h" => {
                print!("{}", ci_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected verify-vsix argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_common_toolchain_args(
    args: &[String],
    mut set: impl FnMut(&str, &str) -> OpsResult<()>,
) -> OpsResult<()> {
    let mut index = 0;
    while index < args.len() {
        let key = args[index].as_str();
        if key == "--help" || key == "-h" {
            print!("{}", ci_help().render(ColorChoice::Auto));
            std::process::exit(0);
        }
        index += 1;
        let Some(value) = args.get(index) else {
            return Err(OpsError::new(format!("`{key}` requires a value")));
        };
        set(key, value)?;
        index += 1;
    }
    Ok(())
}
pub fn help() -> HelpDoc {
    HelpDoc::new("kernworker")
        .summary("Kern repository maintenance and CI worker.")
        .usage("kernworker <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("ci", "Run CI-oriented repository checks")
                .entry("release", "Build and verify release artifacts")
                .entry("help", "Show this help text"),
        )
        .example(
            "kernworker ci kernc-tests --mode smoke",
            "run the smoke integration tests",
        )
        .example(
            "kernworker ci craft-policy",
            "run craft release policy fixtures",
        )
        .example(
            "kernworker release package --version v0.7.7",
            "build a host-native SDK archive",
        )
}

pub fn ci_help() -> HelpDoc {
    HelpDoc::new("kernworker ci")
        .summary("CI-oriented repository checks.")
        .usage("kernworker ci <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("kernc-tests", "Run grouped kernc integration tests")
                .entry("craft-policy", "Run craft release policy fixtures")
                .entry(
                    "activate-toolchain",
                    "Emit environment entries for the active CI toolchain",
                )
                .entry("toolchain-info", "Print CI toolchain diagnostics")
                .entry(
                    "toolchain-health",
                    "Fail if the current host toolchain is incomplete",
                )
                .entry("toolchain-spec", "Print checked-in CI toolchain policy")
                .entry(
                    "verify-toolchain-archive",
                    "Verify a downloaded CI toolchain archive checksum",
                )
                .entry(
                    "verify-packaged-toolchain",
                    "Verify a packaged Kern toolchain archive",
                )
                .entry(
                    "install-packaged-toolchain",
                    "Extract a packaged Kern toolchain for local CI use",
                )
                .entry("verify-vsix", "Verify a packaged VS Code extension archive"),
        )
}

pub fn release_help() -> HelpDoc {
    HelpDoc::new("kernworker release")
        .summary("Release engineering commands.")
        .usage("kernworker release <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("package", "Build and package a host-native SDK")
                .entry(
                    "package-toolchain",
                    "Package the controlled host LLVM toolchain artifact",
                )
                .entry(
                    "write-checksums",
                    "Generate sha256 sidecars and optional release manifest",
                ),
        )
}

fn release_package_help() -> HelpDoc {
    HelpDoc::new("kernworker release package")
        .summary("Build and package a host-native SDK.")
        .usage("kernworker release package [options]")
        .section(
            HelpSection::new("Options")
                .entry(
                    "--version <tag>",
                    "archive version label; defaults to v<workspace>",
                )
                .entry(
                    "--target <target>",
                    "archive target label; defaults to current host",
                )
                .entry("--skip-build", "reuse existing release binaries")
                .entry("--skip-kernup", "do not package the kernup bootstrapper")
                .entry(
                    "--toolchain-prefix <path>",
                    "LLVM toolchain prefix to bundle",
                ),
        )
}

fn release_package_toolchain_help() -> HelpDoc {
    HelpDoc::new("kernworker release package-toolchain")
        .summary("Package the controlled host LLVM toolchain artifact.")
        .usage("kernworker release package-toolchain [options]")
        .section(
            HelpSection::new("Options")
                .entry(
                    "--version <tag>",
                    "artifact version label; defaults to llvm-<version>",
                )
                .entry(
                    "--target <target>",
                    "archive target label; defaults to current host",
                )
                .entry(
                    "--toolchain-prefix <path>",
                    "LLVM toolchain prefix to package",
                ),
        )
}

fn release_checksums_help() -> HelpDoc {
    HelpDoc::new("kernworker release write-checksums")
        .summary("Generate sha256 sidecars and optional release manifest.")
        .usage("kernworker release write-checksums <paths...> [options]")
        .section(
            HelpSection::new("Options")
                .entry(
                    "--manifest-path <path>",
                    "optional manifest JSON output path",
                )
                .entry("--channel <name>", "logical artifact channel label")
                .entry(
                    "--release-tag <tag>",
                    "release tag recorded in the manifest",
                ),
        )
}

fn kernc_tests_help() -> HelpDoc {
    HelpDoc::new("kernworker ci kernc-tests")
        .summary("Run grouped kernc integration tests.")
        .usage("kernworker ci kernc-tests [--mode smoke|hosted|all]")
        .section(
            HelpSection::new("Options")
                .entry("--mode smoke", "run smoke integration tests")
                .entry("--mode hosted", "run hosted integration tests")
                .entry("--mode all", "run all grouped integration tests"),
        )
}

fn activate_toolchain_help() -> HelpDoc {
    HelpDoc::new("kernworker ci activate-toolchain")
        .summary("Emit environment entries for the active CI toolchain.")
        .usage("kernworker ci activate-toolchain [--prefix <path>] [--format text|github-env]")
        .section(
            HelpSection::new("Options")
                .entry(
                    "--prefix <path>",
                    "LLVM toolchain prefix; defaults to KERN_TOOLCHAIN_ROOT or LLVM_SYS_*_PREFIX",
                )
                .entry(
                    "--format github-env",
                    "print GitHub environment file entries",
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn parses_kernc_test_modes() {
        assert!(matches!(
            parse_args(vec![
                "ci".to_string(),
                "kernc-tests".to_string(),
                "--mode".to_string(),
                "smoke".to_string()
            ])
            .unwrap(),
            Command::Ci(CiCommand::KerncTests {
                mode: TestMode::Smoke
            })
        ));
        assert!(
            parse_args(vec![
                "ci".to_string(),
                "kernc-tests".to_string(),
                "--mode".to_string(),
                "bad".to_string()
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_release_package_options() {
        let command = parse_args(vec![
            "release".to_string(),
            "package".to_string(),
            "--version".to_string(),
            "v0.7.7".to_string(),
            "--target".to_string(),
            "x86_64-linux-gnu".to_string(),
            "--skip-build".to_string(),
            "--toolchain-prefix".to_string(),
            "/opt/llvm".to_string(),
        ])
        .unwrap();
        let Command::Release(ReleaseCommand::Package(args)) = command else {
            panic!("expected release package command");
        };
        assert_eq!(args.version.as_deref(), Some("v0.7.7"));
        assert_eq!(args.target.as_deref(), Some("x86_64-linux-gnu"));
        assert!(args.skip_build);
        assert_eq!(
            args.toolchain_prefix.as_deref(),
            Some(Path::new("/opt/llvm"))
        );
    }

    #[test]
    fn parses_activate_toolchain_options() {
        let command = parse_args(vec![
            "ci".to_string(),
            "activate-toolchain".to_string(),
            "--prefix".to_string(),
            "/opt/llvm".to_string(),
            "--format".to_string(),
            "github-env".to_string(),
        ])
        .unwrap();
        let Command::Ci(CiCommand::ActivateToolchain(args)) = command else {
            panic!("expected activate-toolchain command");
        };
        assert_eq!(args.prefix.as_deref(), Some(Path::new("/opt/llvm")));
        assert_eq!(args.format, "github-env");
    }

    #[test]
    fn parses_release_checksum_inputs() {
        let command = parse_args(vec![
            "release".to_string(),
            "write-checksums".to_string(),
            "dist/*".to_string(),
            "--manifest-path".to_string(),
            "dist/manifest.json".to_string(),
            "--channel".to_string(),
            "toolchain".to_string(),
            "--release-tag".to_string(),
            "toolchain-llvm-21.1.7".to_string(),
        ])
        .unwrap();
        let Command::Release(ReleaseCommand::WriteChecksums(args)) = command else {
            panic!("expected release write-checksums command");
        };
        assert_eq!(args.paths, vec!["dist/*"]);
        assert_eq!(
            args.manifest_path.as_deref(),
            Some(Path::new("dist/manifest.json"))
        );
        assert_eq!(args.channel, "toolchain");
        assert_eq!(args.release_tag.as_deref(), Some("toolchain-llvm-21.1.7"));
    }
}
