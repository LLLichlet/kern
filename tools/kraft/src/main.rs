mod cli;
mod discover;
mod error;
mod manifest;
mod workspace;

fn main() {
    if let Err(err) = cli::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
