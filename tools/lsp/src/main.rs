mod analysis;
mod protocol;
mod server;
mod transport;

use kernc_utils::config::{CompileOptions, LibraryBundle};

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
    let mut options = CompileOptions {
        library_bundle: LibraryBundle::Std,
        ..CompileOptions::default()
    };

    let mut args = std::env::args().skip(1);
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

        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--no-default-features" => options.craft_default_features = false,
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

fn print_usage() {
    println!("{}", usage());
}

fn usage() -> &'static str {
    "\
kern-lsp - Kern language server

USAGE:
    kern-lsp [--library-bundle <none|base|std>] [--features <a,b>] [--no-default-features] [--module-path <name=path>]... [--module-interface-path <name=path>]...

OPTIONS:
    --library-bundle Select the injected library bundle for analysis (default: std)
    --features <a,b> Enable explicit `craft` features for project analysis
    --no-default-features
                     Disable default `craft` features for project analysis
    --module-path <name=path>
                     Add a source module alias for analysis
    --module-interface-path <name=path>
                     Add an imported metadata module alias for analysis
"
}
