fn main() {
    if let Err(err) = craft::cli::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
