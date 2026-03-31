mod analysis;
mod protocol;
mod server;
mod transport;

fn main() {
    if let Err(err) = server::run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
