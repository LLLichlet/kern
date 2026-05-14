use shared_cli::{ColorChoice, ErrorReport, HelpDoc, HelpSection};
use shared_ops::{
    OpsError, OpsResult, archive_kind_from_path, copy_sdk_contents, default_install_root,
    detect_host_target, extract_archive_with_system_tool, make_temp_dir, remove_path_if_exists,
    validate_sdk_root, verify_installed_tools,
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
    archive: Option<PathBuf>,
    dest: Option<PathBuf>,
    target: Option<String>,
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
    let mut parsed = InstallArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
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
    let target = args.target.unwrap_or_else(|| host.archive_target.clone());
    if target != host.archive_target {
        return Err(OpsError::new(format!(
            "target `{target}` does not match the current host `{}`",
            host.archive_target
        )));
    }

    let Some(archive) = args.archive else {
        return Err(OpsError::new(
            "`kernup install` currently requires `--archive`; release downloads and source installs are the next migration step",
        ));
    };
    if !archive.is_file() {
        return Err(OpsError::new(format!(
            "archive `{}` does not exist",
            archive.display()
        )));
    }
    let install_root = args.dest.unwrap_or(default_install_root(&host)?);
    let temp_root = make_temp_dir("kernup-install-")?;
    let result = (|| -> OpsResult<()> {
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
            println!(
                "=> PATH configuration is not migrated yet. Add `{}` to PATH if needed.",
                install_root.join("bin").display()
            );
        }
        println!(
            "Kern SDK installed successfully into {}",
            install_root.display()
        );
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp_root);
    result
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
        .summary("Kern SDK installer and toolchain manager.")
        .usage("kernup <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("install", "Install a Kern SDK archive")
                .entry("doctor", "Validate the active SDK installation")
                .entry("target", "Print the current host archive target")
                .entry("help", "Show this help text"),
        )
        .example(
            "kernup install --archive ./kern-v0.7.6-x86_64-linux-gnu.tar.gz",
            "install a local SDK archive",
        )
        .example("kernup doctor", "verify the default installation")
        .note("Release downloads, source installs, shims, and PATH mutation are planned next.")
}

fn install_help() -> HelpDoc {
    HelpDoc::new("kernup install")
        .summary("Install a Kern SDK archive.")
        .usage("kernup install --archive <path> [--dest <path>] [--target <target>] [--no-path]")
        .section(
            HelpSection::new("Options")
                .entry("--archive <path>", "local SDK archive to install")
                .entry(
                    "--dest <path>",
                    "installation directory; defaults to ~/.kern",
                )
                .entry(
                    "--target <target>",
                    "host target label; defaults to the current host",
                )
                .entry("--no-path", "skip PATH guidance"),
        )
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
