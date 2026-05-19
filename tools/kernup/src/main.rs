//! `kernup` installer entry point.
//!
//! The binary downloads or installs a Kern SDK archive, validates the resulting
//! toolchain, and optionally wires the installed tools into the user's PATH.

use shared_cli::{ColorChoice, ErrorReport, HelpDoc, HelpSection};
use shared_ops::{
    OpsError, OpsResult, archive_kind_from_path, configure_path, copy_sdk_contents,
    default_install_root, detect_host_target, download_file, extract_archive_with_system_tool,
    fetch_latest_github_release, infer_release_version_from_archive_name, make_temp_dir,
    remove_path_if_exists, validate_sdk_root, verify_installed_tools,
};
use std::env;
use std::path::PathBuf;

#[derive(Debug)]
enum Command {
    Install(InstallArgs),
    Doctor(DoctorArgs),
    Target,
    Help,
}

#[derive(Debug, Default)]
struct InstallArgs {
    version: Option<String>,
    archive: Option<PathBuf>,
    dest: Option<PathBuf>,
    target: Option<String>,
    github_repo: String,
    no_path: bool,
}

#[derive(Debug, Default)]
struct DoctorArgs {
    dest: Option<PathBuf>,
}

fn main() {
    if let Err(err) = run() {
        eprint!(
            "{}",
            ErrorReport::new("kernup error", err.to_string()).render(ColorChoice::Auto)
        );
        std::process::exit(1);
    }
}

fn run() -> OpsResult<()> {
    match parse_args(env::args().skip(1).collect())? {
        Command::Install(args) => install(args),
        Command::Doctor(args) => doctor(args),
        Command::Target => {
            println!("{}", detect_host_target()?.archive_target);
            Ok(())
        }
        Command::Help => {
            print!("{}", help().render(ColorChoice::Auto));
            Ok(())
        }
    }
}

fn parse_args(args: Vec<String>) -> OpsResult<Command> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match command {
        "install" => parse_install_args(&args[1..]).map(Command::Install),
        "doctor" => parse_doctor_args(&args[1..]).map(Command::Doctor),
        "target" => Ok(Command::Target),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(OpsError::new(format!(
            "unknown command `{other}`; run `kernup help`"
        ))),
    }
}

fn parse_install_args(args: &[String]) -> OpsResult<InstallArgs> {
    let mut parsed = InstallArgs {
        github_repo: "kern-project/kern".into(),
        ..InstallArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--version" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--version` requires a value"));
                };
                parsed.version = Some(value.clone());
            }
            "--archive" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--archive` requires a value"));
                };
                parsed.archive = Some(PathBuf::from(value));
            }
            "--dest" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--dest` requires a value"));
                };
                parsed.dest = Some(PathBuf::from(value));
            }
            "--target" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--target` requires a value"));
                };
                parsed.target = Some(value.clone());
            }
            "--github-repo" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--github-repo` requires a value"));
                };
                parsed.github_repo = value.clone();
            }
            "--no-path" => {
                parsed.no_path = true;
            }
            "--help" | "-h" => {
                print!("{}", install_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected install argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_doctor_args(args: &[String]) -> OpsResult<DoctorArgs> {
    let mut parsed = DoctorArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dest" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--dest` requires a value"));
                };
                parsed.dest = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                print!("{}", doctor_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected doctor argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn install(args: InstallArgs) -> OpsResult<()> {
    let host = detect_host_target()?;
    let target = args
        .target
        .clone()
        .unwrap_or_else(|| host.archive_target.clone());
    if target != host.archive_target {
        return Err(OpsError::new(format!(
            "target `{target}` does not match the current host `{}`",
            host.archive_target
        )));
    }

    let install_root = args.dest.clone().unwrap_or(default_install_root(&host)?);
    let temp_root = make_temp_dir("kernup-install-")?;
    let result = (|| -> OpsResult<()> {
        let (archive, version) = resolve_install_archive(&args, &target, &host, &temp_root)?;
        let extract_root = temp_root.join("extract");
        let sdk_root = extract_archive_with_system_tool(
            &archive,
            &extract_root,
            archive_kind_from_path(&archive)?,
        )?;
        validate_sdk_root(&sdk_root, &target)?;
        copy_sdk_contents(&sdk_root, &install_root)?;
        println!("=> Verifying installed tools...");
        verify_installed_tools(&install_root, &host)?;
        if args.no_path {
            println!(
                "=> Skipped PATH configuration. Add `{}` to PATH when ready.",
                install_root.join("bin").display()
            );
        } else {
            println!("=> Configuring PATH...");
            configure_path(&install_root.join("bin"), &host)?;
        }
        println!(
            "Kern {version} SDK installed successfully into {}",
            install_root.display()
        );
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp_root);
    result
}

fn resolve_install_archive(
    args: &InstallArgs,
    target: &str,
    host: &shared_ops::HostTarget,
    temp_root: &std::path::Path,
) -> OpsResult<(PathBuf, String)> {
    if let Some(archive) = &args.archive {
        if !archive.is_file() {
            return Err(OpsError::new(format!(
                "archive `{}` does not exist",
                archive.display()
            )));
        }
        let version = args
            .version
            .clone()
            .or_else(|| {
                archive
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| infer_release_version_from_archive_name(name, target))
            })
            .unwrap_or_else(|| "<local>".to_string());
        return Ok((archive.clone(), version));
    }

    let version = args
        .version
        .clone()
        .or_else(|| {
            fetch_latest_github_release(&args.github_repo)
                .ok()
                .flatten()
        })
        .unwrap_or_else(|| "v0.7.7".to_string());
    let archive_name = format!("kern-{version}-{target}.{}", host.archive_extension);
    let archive = temp_root.join(&archive_name);
    let url = format!(
        "https://github.com/{}/releases/download/{version}/{archive_name}",
        args.github_repo
    );
    println!("=> Downloading Kern {version}...");
    download_file(&url, &archive)?;
    Ok((archive, version))
}

fn doctor(args: DoctorArgs) -> OpsResult<()> {
    let host = detect_host_target()?;
    let install_root = args.dest.unwrap_or(default_install_root(&host)?);
    validate_sdk_root(&install_root, &host.archive_target)?;
    println!("=> SDK manifest and layout are valid.");
    verify_installed_tools(&install_root, &host)?;
    println!("kernup doctor: ok");
    Ok(())
}

fn help() -> HelpDoc {
    HelpDoc::new("kernup")
        .summary("Kern SDK installer.")
        .usage("kernup <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("install", "Install a Kern SDK archive")
                .entry("doctor", "Validate the active SDK installation")
                .entry("target", "Print the current host archive target")
                .entry("help", "Show this help text"),
        )
        .example(
            "kernup install --archive ./kern-v0.7.7-x86_64-linux-gnu.tar.gz",
            "install a local SDK archive",
        )
        .example(
            "kernup install --version v0.7.7",
            "download and install a release SDK",
        )
        .example("kernup doctor", "verify the default installation")
        .note("kernup installs SDK archives only; it does not build Kern from source.")
        .note("For source builds, configure the host LLVM development environment and run Cargo directly.")
}

fn install_help() -> HelpDoc {
    HelpDoc::new("kernup install")
        .summary("Install a Kern SDK release.")
        .usage("kernup install [--version <tag>] [--archive <path>] [--dest <path>] [--target <target>] [--no-path]")
        .section(
            HelpSection::new("Options")
                .entry("--version <tag>", "release tag; defaults to the latest GitHub release")
                .entry("--archive <path>", "local SDK archive to install")
                .entry(
                    "--dest <path>",
                    "installation directory; defaults to ~/.kern",
                )
                .entry(
                    "--target <target>",
                    "host target label; defaults to the current host",
                )
                .entry("--github-repo <repo>", "GitHub repository for release downloads")
                .entry("--no-path", "skip PATH configuration"),
        )
        .note("This command installs release SDK archives; it is not a source-build command.")
}

fn doctor_help() -> HelpDoc {
    HelpDoc::new("kernup doctor")
        .summary("Validate a Kern SDK installation.")
        .usage("kernup doctor [--dest <path>]")
        .section(HelpSection::new("Options").entry(
            "--dest <path>",
            "installation directory; defaults to ~/.kern",
        ))
}
