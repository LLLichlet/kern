fn main() {
    if let Err(err) = craft::cli::run() {
        eprintln!("craft failed");
        eprintln!("error: {err}");
        if let Some(hint) = err.hint() {
            eprintln!("help: {hint}");
        }
        std::process::exit(err.exit_code());
    }
}
