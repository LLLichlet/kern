use crate::discover;
use crate::error::{Error, Result};
use crate::manifest::Manifest;
use std::env;
use std::path::{Path, PathBuf};

pub enum Command {
    Help,
    Check { path: Option<PathBuf> },
}

pub fn run() -> Result<()> {
    match parse_args(env::args().skip(1))? {
        Command::Help => {
            print!("{}", usage());
            Ok(())
        }
        Command::Check { path } => {
            let manifest_path = discover::resolve_manifest_path(path.as_deref())?;
            let manifest = Manifest::load(&manifest_path)?;
            manifest.validate(&manifest_path)?;

            let package_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
            let kraft_script = package_root.join("kraft.kr");
            let build_script = package_root.join("build.kr");

            println!("checked {}", manifest_path.display());
            if let Some(package) = &manifest.package {
                println!("package: {} {}", package.name, package.version);
            } else {
                println!("package: <none>");
            }
            if let Some(workspace) = &manifest.workspace {
                println!("workspace members: {}", workspace.members.len());
            } else {
                println!("workspace members: 0");
            }
            println!(
                "targets: lib={} bin={} test={} example={}",
                usize::from(manifest.lib.is_some()),
                manifest.bin.len(),
                manifest.test.len(),
                manifest.example.len()
            );
            println!(
                "dependencies: normal={} dev={} build={}",
                manifest.dependencies.len(),
                manifest.dev_dependencies.len(),
                manifest.build_dependencies.len()
            );
            println!(
                "scripts: kraft.kr={} build.kr={}",
                if kraft_script.is_file() { "yes" } else { "no" },
                if build_script.is_file() { "yes" } else { "no" }
            );

            Ok(())
        }
    }
}

fn parse_args<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Command::Help),
        [cmd] if cmd == "help" || cmd == "--help" || cmd == "-h" => Ok(Command::Help),
        [cmd] if cmd == "check" => Ok(Command::Check { path: None }),
        [cmd, path] if cmd == "check" => Ok(Command::Check {
            path: Some(PathBuf::from(path)),
        }),
        _ => Err(Error::Usage(format!(
            "unsupported command line: {}\n\n{}",
            args.join(" "),
            usage()
        ))),
    }
}

fn usage() -> &'static str {
    "\
kraft - Kern package manager and builder

USAGE:
    kraft help
    kraft check [PATH]

COMMANDS:
    help         Show this help text
    check        Discover, parse, and validate Kraft.toml
"
}
