mod analysis;
mod defaults;
mod protocol;
mod server;
mod transport;

use crate::defaults::default_analysis_compile_options;
use kernc_utils::config::{CompileOptions, LibraryBundle};

#[derive(Debug)]
enum CliAction {
    Run(Box<CompileOptions>),
    Help,
    Version,
}

fn main() {
    let action = match parse_args(std::env::args().skip(1)) {
        Ok(action) => action,
        Err(message) => {
            eprintln!("error: {message}");
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
                compile_options: *options,
            });

            if let Err(err) = server::run_with_analysis(analysis) {
                eprintln!("error: {err}");
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

        match arg.as_str() {
            "--help" | "-h" => {
                return Ok(CliAction::Help);
            }
            "--version" | "-V" | "-v" => return Ok(CliAction::Version),
            "--no-default-features" => options.craft_default_features = false,
            _ => {
                return Err(format!("unsupported argument `{arg}`\n\n{}", usage()));
            }
        }
    }

    Ok(CliAction::Run(Box::new(options)))
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

fn version_text() -> String {
    format!("Kern Language Server v{}", env!("CARGO_PKG_VERSION"))
}

fn usage() -> &'static str {
    concat!(
        "Kern Language Server v",
        env!("CARGO_PKG_VERSION"),
        "\n",
        "Usage: kern-lsp [OPTIONS]\n",
        "\n",
        "Analysis Options:\n",
        "  --library-bundle <B>       Select official library root aliases for analysis: none, base, std (default: std)\n",
        "  --features <a,b>           Enable explicit `craft` features for project analysis\n",
        "  --no-default-features      Disable default `craft` features for project analysis\n",
        "  --module-path <name=path>  Add a source module alias for analysis\n",
        "  --module-interface-path <name=path>\n",
        "                             Add an imported metadata module alias for analysis\n",
        "\n",
        "Information:\n",
        "  -v, -V, --version          Print version information and exit\n",
        "  -h, --help                 Print this help text and exit\n",
    )
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
        assert_eq!(options.library_bundle, LibraryBundle::Base);
        assert!(!options.craft_default_features);
        assert_eq!(
            options.craft_features,
            vec!["tls".to_string(), "simd".to_string()]
        );
        assert_eq!(
            options.module_aliases.get("toml").map(String::as_str),
            Some("./src")
        );
        assert_eq!(
            options
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
        assert_eq!(options.library_bundle, LibraryBundle::Std);
    }

    #[test]
    fn rejects_empty_feature_names() {
        let err = parse_args(["--features".to_string(), "tls,".to_string()]).unwrap_err();
        assert!(
            err.contains("empty feature name"),
            "unexpected error: {err}"
        );
    }
}
