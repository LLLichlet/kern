mod build_plan;
mod cli;
mod discover;
mod elaborate;
mod error;
mod graph;
mod lockfile;
mod manifest;
mod plan;
mod resolver;
mod script;
mod workspace;

fn main() {
    if let Err(err) = cli::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
