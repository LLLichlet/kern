mod analysis;
mod protocol;
mod server;
mod transport;

use kernc_utils::config::CompileOptions;

fn main() {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(2);
        }
    };

    let analysis = analysis::AnalysisEngine::new(analysis::AnalysisSettings {
        compile_options: options,
    });

    if let Err(err) = server::run_with_analysis(analysis) {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<CompileOptions, String> {
    let mut options = CompileOptions::default();
    options.use_std = true;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--use-std" => options.use_std = true,
            "--no-use-std" => options.use_std = false,
            "-M" => {
                let value = args
                    .next()
                    .ok_or_else(|| "expected `name=path` after `-M`".to_string())?;
                let (name, path) = parse_key_value(&value, "-M")?;
                options.module_aliases.insert(name, path);
            }
            "-I" => {
                let value = args
                    .next()
                    .ok_or_else(|| "expected `name=path` after `-I`".to_string())?;
                let (name, path) = parse_key_value(&value, "-I")?;
                options.module_interface_aliases.insert(name, path);
            }
            _ => {
                return Err(format!("unsupported argument `{arg}`\n\n{}", usage()));
            }
        }
    }

    Ok(options)
}

fn parse_key_value(value: &str, flag: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((name, path)) if !name.is_empty() && !path.is_empty() => {
            Ok((name.to_string(), path.to_string()))
        }
        _ => Err(format!("expected `name=path` after `{flag}`")),
    }
}

fn print_usage() {
    println!("{}", usage());
}

fn usage() -> &'static str {
    "\
kern-lsp - Kern language server

USAGE:
    kern-lsp [--use-std|--no-use-std] [-M <name=path>]... [-I <name=path>]...

OPTIONS:
    --use-std        Enable std injection for analysis (default)
    --no-use-std     Disable std injection for analysis
    -M <name=path>   Add a source module alias for analysis
    -I <name=path>   Add an imported kmeta module alias for analysis
"
}
