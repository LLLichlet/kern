mod cli;
mod discover;
mod error;
mod graph;
mod manifest;
mod workspace;

fn main() {
    if let Err(err) = cli::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
