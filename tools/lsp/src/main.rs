//! Kern language-server binary entry point.
//!
//! The binary parses command-line options, initializes the JSON-RPC transport,
//! and runs the LSP server loop over stdin/stdout.

mod analysis;
mod defaults;
mod help;
mod protocol;
mod server;
mod transport;

use crate::defaults::default_analysis_compile_options;
use crate::help::{render_help, version_text};
use kernc_utils::config::{CompileOptions, LibraryBundle};
use shared_cli::{ColorChoice, ErrorReport};

#[derive(Debug)]
enum CliAction {
    Run(Box<RunOptions>),
    Help,
    Version,
}

#[derive(Debug)]
struct RunOptions {
    compile_options: CompileOptions,
    server_options: server::ServerOptions,
}

fn main() {
    let action = match parse_args(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(message) => {
            eprint!(
                "{}",
                ErrorReport::new("kern-lsp error", message)
                    .hint("Run `kern-lsp --help` to see the supported options.")
                    .render(ColorChoice::Auto)
            );
            std::process::exit(2);
        }
    };

    match action {
        CliAction::Help => {
            print_usage();
        }
        CliAction::Version => {
            println!("{}", version_text());
        }
        CliAction::Run(options) => {
            let analysis = analysis::AnalysisEngine::new(analysis::AnalysisSettings {
                compile_options: options.compile_options,
            });

            if let Err(err) = server::run_with_analysis_options(analysis, options.server_options) {
                eprint!(
                    "{}",
                    ErrorReport::new("kern-lsp error", err.to_string()).render(ColorChoice::Auto)
                );
                std::process::exit(1);
            }
        }
    }
}

fn parse_args<I>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = String>,
{
    let mut options = default_analysis_compile_options();
    let mut server_options = server::ServerOptions::default();
    let args: Vec<String> = args.into_iter().collect();
    if args.first().is_some_and(|arg| arg == "help") {
        if args.len() > 1 {
            return Err(format!("unsupported help topic `{}`", args[1]));
        }
        return Ok(CliAction::Help);
    }

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if let Some(value) =
            consume_long_option_value(&arg, "--library-bundle", &mut args, "<none|base|std>")?
        {
            options.library_bundle = match value.as_str() {
                "none" => LibraryBundle::None,
                "base" => LibraryBundle::Base,
                "std" => LibraryBundle::Std,
                _ => {
                    return Err(format!(
                        "invalid library bundle `{value}`; expected `none`, `base`, or `std`"
                    ));
                }
            };
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--features", &mut args, "<a,b,c>")? {
            for feature in value.split(',') {
                let feature = feature.trim();
                if feature.is_empty() {
                    return Err("empty feature name in `--features`".to_string());
                }
                if !options.craft_features.iter().any(|item| item == feature) {
                    options.craft_features.push(feature.to_string());
                }
            }
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--module-path", &mut args, "<name=path>")?
        {
            let (name, path) = parse_key_value(&value, "--module-path")?;
            options.module_aliases.insert(name, path);
            continue;
        }
        if let Some(value) =
            consume_long_option_value(&arg, "--module-interface-path", &mut args, "<name=path>")?
        {
            let (name, path) = parse_key_value(&value, "--module-interface-path")?;
            options.module_interface_aliases.insert(name, path);
            continue;
        }
        if let Some(value) = consume_long_option_value(&arg, "--worker-threads", &mut args, "<n>")?
        {
            server_options.worker_threads = parse_worker_threads(&value)?;
            continue;
        }

        match arg.as_str() {
            "--help" | "-h" => {
                return Ok(CliAction::Help);
            }
            "--version" | "-V" | "-v" => return Ok(CliAction::Version),
            "--no-default-features" => options.craft_default_features = false,
            _ => {
                return Err(format!("unsupported argument `{arg}`"));
            }
        }
    }

    Ok(CliAction::Run(Box::new(RunOptions {
        compile_options: options,
        server_options,
    })))
}

fn parse_key_value(value: &str, flag: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((name, path)) if !name.is_empty() && !path.is_empty() => {
            Ok((name.to_string(), path.to_string()))
        }
        _ => Err(format!("expected `name=path` after `{flag}`")),
    }
}

fn consume_long_option_value(
    arg: &str,
    flag: &str,
    args: &mut impl Iterator<Item = String>,
    value_name: &str,
) -> Result<Option<String>, String> {
    if arg == flag {
        return args
            .next()
            .map(Some)
            .ok_or_else(|| format!("expected `{}` after `{}`", value_name, flag));
    }

    let prefix = format!("{flag}=");
    Ok(arg.strip_prefix(&prefix).map(|value| value.to_string()))
}

fn parse_worker_threads(value: &str) -> Result<usize, String> {
    let worker_threads = value.parse::<usize>().map_err(|_| {
        format!("invalid worker thread count `{value}`; expected a positive integer")
    })?;
    if worker_threads == 0 {
        return Err("worker thread count must be greater than zero".to_string());
    }
    Ok(worker_threads)
}

fn print_usage() {
    println!("{}", render_help(ColorChoice::Auto));
}

#[cfg(test)]
mod tests {
    use super::{CliAction, parse_args};
    use kernc_utils::config::LibraryBundle;

    #[test]
    fn parses_help_and_version_flags() {
        assert!(matches!(
            parse_args(["--help".to_string()]).unwrap(),
            CliAction::Help
        ));
        assert!(matches!(
            parse_args(["help".to_string()]).unwrap(),
            CliAction::Help
        ));
        assert!(matches!(
            parse_args(["--version".to_string()]).unwrap(),
            CliAction::Version
        ));
        assert!(matches!(
            parse_args(["-v".to_string()]).unwrap(),
            CliAction::Version
        ));
    }

    #[test]
    fn parses_analysis_options() {
        let action = parse_args([
            "--library-bundle".to_string(),
            "base".to_string(),
            "--features=tls,simd".to_string(),
            "--no-default-features".to_string(),
            "--module-path".to_string(),
            "toml=./src".to_string(),
            "--module-interface-path=std=./meta/std".to_string(),
        ])
        .unwrap();

        let CliAction::Run(options) = action else {
            panic!("expected run action");
        };
        assert_eq!(options.compile_options.library_bundle, LibraryBundle::Base);
        assert!(!options.compile_options.craft_default_features);
        assert_eq!(
            options.compile_options.craft_features,
            vec!["tls".to_string(), "simd".to_string()]
        );
        assert_eq!(
            options
                .compile_options
                .module_aliases
                .get("toml")
                .map(String::as_str),
            Some("./src")
        );
        assert_eq!(
            options
                .compile_options
                .module_interface_aliases
                .get("std")
                .map(String::as_str),
            Some("./meta/std")
        );
    }

    #[test]
    fn defaults_to_std_analysis_bundle() {
        let action = parse_args(Vec::<String>::new()).unwrap();

        let CliAction::Run(options) = action else {
            panic!("expected run action");
        };
        assert_eq!(options.compile_options.library_bundle, LibraryBundle::Std);
        assert_eq!(options.server_options.worker_threads, 4);
    }

    #[test]
    fn rejects_empty_feature_names() {
        let err = parse_args(["--features".to_string(), "tls,".to_string()]).unwrap_err();
        assert!(
            err.contains("empty feature name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parses_worker_thread_count() {
        let action = parse_args(["--worker-threads=2".to_string()]).unwrap();

        let CliAction::Run(options) = action else {
            panic!("expected run action");
        };
        assert_eq!(options.server_options.worker_threads, 2);
    }

    #[test]
    fn rejects_invalid_worker_thread_count() {
        let err = parse_args(["--worker-threads".to_string(), "0".to_string()]).unwrap_err();
        assert!(err.contains("greater than zero"), "unexpected error: {err}");
    }
}
