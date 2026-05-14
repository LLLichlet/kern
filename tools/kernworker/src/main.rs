mod args;
mod ci;
mod release;

use args::{CiCommand, Command, ReleaseCommand};
use shared_cli::{ColorChoice, ErrorReport};
use shared_ops::OpsResult;
use std::env;

fn main() {
    if let Err(err) = run() {
        eprint!(
            "{}",
            ErrorReport::new("kernworker error", err.to_string()).render(ColorChoice::Auto)
        );
        std::process::exit(1);
    }
}

fn run() -> OpsResult<()> {
    match args::parse_args(env::args().skip(1).collect())? {
        Command::Ci(CiCommand::KerncTests { mode }) => ci::run_kernc_tests(mode),
        Command::Ci(CiCommand::CraftPolicy) => ci::run_craft_policy_checks(),
        Command::Ci(CiCommand::ActivateToolchain(args)) => ci::activate_toolchain(args),
        Command::Ci(CiCommand::ToolchainInfo) => ci::print_toolchain_info(),
        Command::Ci(CiCommand::ToolchainHealth) => ci::assert_toolchain_health(),
        Command::Ci(CiCommand::ToolchainSpec(args)) => ci::print_toolchain_spec(args),
        Command::Ci(CiCommand::VerifyToolchainArchive(args)) => ci::verify_toolchain_archive(args),
        Command::Ci(CiCommand::VerifyPackagedToolchain(args)) => {
            ci::verify_packaged_toolchain(args)
        }
        Command::Ci(CiCommand::InstallPackagedToolchain(args)) => {
            ci::install_packaged_toolchain(args)
        }
        Command::Ci(CiCommand::VerifyVsix(args)) => ci::verify_vscode_extension_archive(args),
        Command::Ci(CiCommand::Help) => {
            print!("{}", args::ci_help().render(ColorChoice::Auto));
            Ok(())
        }
        Command::Release(ReleaseCommand::Package(args)) => release::package_release(args),
        Command::Release(ReleaseCommand::PackageToolchain(args)) => {
            release::package_toolchain_release(args)
        }
        Command::Release(ReleaseCommand::WriteChecksums(args)) => {
            release::write_release_checksums(args)
        }
        Command::Release(ReleaseCommand::Help) => {
            print!("{}", args::release_help().render(ColorChoice::Auto));
            Ok(())
        }
        Command::Help => {
            print!("{}", args::help().render(ColorChoice::Auto));
            Ok(())
        }
    }
}
