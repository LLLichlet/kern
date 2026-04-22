use shared_cli::{ColorChoice, ErrorReport};

fn main() {
    if let Err(err) = craft::cli::run() {
        let mut report = ErrorReport::new("craft error", err.to_string());
        if let Some(hint) = err.hint() {
            report = report.hint(hint);
        }
        eprint!("{}", report.render(ColorChoice::Auto));
        std::process::exit(err.exit_code());
    }
}
