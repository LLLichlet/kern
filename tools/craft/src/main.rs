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
