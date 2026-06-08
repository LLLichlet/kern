//! Craft binary entry point.
//!
//! The executable installs shared CLI rendering, invokes the library command
//! dispatcher, and maps structured command failures to process exit codes.

use shared_cli::{ColorChoice, ErrorReport};

fn main() {
    kernc_utils::install_compiler_panic_hook("craft");

    if let Err(err) = craft::cli::run() {
        let mut report = ErrorReport::new("craft error", err.to_string());
        if let Some(hint) = err.hint() {
            report = report.hint(hint);
        }
        eprint!("{}", report.render(ColorChoice::Auto));
        std::process::exit(err.exit_code());
    }
}
