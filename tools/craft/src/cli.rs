use crate::elaborate;
use crate::error::{Error, Result};
use std::env;
use std::path::PathBuf;

mod commands;
mod policy;
mod render;

use self::commands::run_command;

#[cfg(test)]
use self::policy::{
    summarize_check_sources, summarize_source_security, validate_check_source_policy,
};

#[derive(Debug)]
pub enum Command {
    Help,
    Version,
    Check {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Lock {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Fetch {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Publish {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Doc {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Build {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
        include_examples: bool,
    },
    Run {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
    Test {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiOptions {
    verbose: bool,
    timings: bool,
    color: ColorChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

pub fn run() -> Result<()> {
    run_command(parse_args(env::args().skip(1))?)
}

pub(super) fn version_text() -> String {
    format!("Craft v{}", env!("CARGO_PKG_VERSION"))
}

fn parse_args<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help);
    };
    if cmd == "--version" || cmd == "-V" || (cmd == "-v" && rest.is_empty()) {
        return Ok(Command::Version);
    }
    if rest.iter().any(|arg| arg == "--version" || arg == "-V") {
        return Ok(Command::Version);
    }
    if cmd == "help" || cmd == "--help" || cmd == "-h" {
        return Ok(Command::Help);
    }
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Command::Help);
    }

    let (path, feature_selection, ui, include_examples) =
        parse_command_options(rest, cmd == "build")?;
    match cmd.as_str() {
        "check" => Ok(Command::Check {
            path,
            feature_selection,
            ui,
        }),
        "lock" => Ok(Command::Lock {
            path,
            feature_selection,
            ui,
        }),
        "fetch" => Ok(Command::Fetch {
            path,
            feature_selection,
            ui,
        }),
        "publish" => {
            let mut feature_selection = feature_selection;
            feature_selection.profile = crate::script::ProfileSelection::Release;
            Ok(Command::Publish {
                path,
                feature_selection,
                ui,
            })
        }
        "doc" => Ok(Command::Doc {
            path,
            feature_selection,
            ui,
        }),
        "build" => Ok(Command::Build {
            path,
            feature_selection,
            ui,
            include_examples,
        }),
        "run" => Ok(Command::Run {
            path,
            feature_selection,
            ui,
        }),
        "test" => Ok(Command::Test {
            path,
            feature_selection,
            ui,
        }),
        _ => Err(Error::Usage(format!(
            "unsupported command line: {}\n\n{}",
            args.join(" "),
            usage()
        ))),
    }
}

fn parse_command_options(
    args: &[String],
    allow_examples: bool,
) -> Result<(
    Option<PathBuf>,
    elaborate::FeatureSelection,
    UiOptions,
    bool,
)> {
    let mut path: Option<PathBuf> = None;
    let mut feature_selection = elaborate::FeatureSelection::default();
    let mut ui = UiOptions::default();
    let mut include_examples = false;
    let mut idx = 0;

    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--verbose" || arg == "-v" {
            ui.verbose = true;
            idx += 1;
            continue;
        }
        if arg == "--timings" {
            ui.timings = true;
            idx += 1;
            continue;
        }
        if arg == "--examples" {
            if !allow_examples {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    usage()
                )));
            }
            include_examples = true;
            idx += 1;
            continue;
        }
        if arg == "--no-color" {
            ui.color = ColorChoice::Never;
            idx += 1;
            continue;
        }
        if arg == "--color" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--color` requires one of: auto, always, never".to_string(),
                ));
            };
            ui.color = parse_color_choice(value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            ui.color = parse_color_choice(value)?;
            idx += 1;
            continue;
        }
        if arg == "--no-default-features" {
            feature_selection.enable_default = false;
            idx += 1;
            continue;
        }
        if arg == "--project-path" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--project-path` requires a package or workspace path".to_string(),
                ));
            };
            set_project_path(&mut path, value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--project-path=") {
            set_project_path(&mut path, value)?;
            idx += 1;
            continue;
        }
        if arg == "--profile" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--profile` requires one of: dev, release".to_string(),
                ));
            };
            feature_selection.profile = parse_profile_selection(value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--profile=") {
            feature_selection.profile = parse_profile_selection(value)?;
            idx += 1;
            continue;
        }
        if arg == "--features" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--features` requires a comma-separated feature list".to_string(),
                ));
            };
            extend_feature_selection(&mut feature_selection, value)?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--features=") {
            extend_feature_selection(&mut feature_selection, value)?;
            idx += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(Error::Usage(format!(
                "unsupported option `{arg}`\n\n{}",
                usage()
            )));
        }
        return Err(Error::Usage(format!(
            "unexpected positional argument `{arg}`; use `--project-path <PATH>`\n\n{}",
            usage()
        )));
    }

    Ok((path, feature_selection, ui, include_examples))
}

fn parse_color_choice(raw: &str) -> Result<ColorChoice> {
    match raw {
        "auto" => Ok(ColorChoice::Auto),
        "always" => Ok(ColorChoice::Always),
        "never" => Ok(ColorChoice::Never),
        other => Err(Error::Usage(format!(
            "unsupported `--color` value `{other}`; expected auto, always, or never"
        ))),
    }
}

fn parse_profile_selection(raw: &str) -> Result<crate::script::ProfileSelection> {
    match raw {
        "dev" => Ok(crate::script::ProfileSelection::Dev),
        "release" => Ok(crate::script::ProfileSelection::Release),
        other => Err(Error::Usage(format!(
            "unsupported `--profile` value `{other}`; expected dev or release"
        ))),
    }
}

fn set_project_path(slot: &mut Option<PathBuf>, raw: &str) -> Result<()> {
    if let Some(existing_path) = slot {
        return Err(Error::Usage(format!(
            "multiple `--project-path` values provided: `{}` and `{raw}`",
            existing_path.display()
        )));
    }

    *slot = Some(PathBuf::from(raw));
    Ok(())
}

fn extend_feature_selection(selection: &mut elaborate::FeatureSelection, raw: &str) -> Result<()> {
    for feature in raw.split(',') {
        let feature = feature.trim();
        if feature.is_empty() {
            return Err(Error::Usage(
                "feature names in `--features` must not be empty".to_string(),
            ));
        }
        selection.explicit.insert(feature.to_string());
    }
    Ok(())
}

fn usage() -> &'static str {
    concat!(
        "Craft v",
        env!("CARGO_PKG_VERSION"),
        "\n",
        "Kern package manager and builder\n",
        "\n",
        "Usage:\n",
        "  craft <command> [OPTIONS]\n",
        "  craft help\n",
        "  craft --help\n",
        "  craft --version\n",
        "\n",
        "Commands:\n",
        "  help     Show this help text\n",
        "  check    Validate `Craft.toml`, scripts, sources, and derived analysis inputs\n",
        "  lock     Write a deterministic `Craft.lock` for the current package graph\n",
        "  fetch    Materialize external package sources into the local `.craft` cache\n",
        "  publish  Run release-oriented publish readiness checks without uploading anywhere\n",
        "  doc      Build library metadata and render native package docs to Markdown\n",
        "  build    Build the selected package graph and print the derived action plan\n",
        "  run      Build and run the single runnable `bin` target in the package graph\n",
        "  test     Build and run all discovered `test` targets\n",
        "\n",
        "Global Options:\n",
        "  --project-path <PATH>    Select the package or workspace root (or `Craft.toml` path)\n",
        "  --profile <NAME>         Profile selection: dev (default) or release\n",
        "  --examples               Include `[[example]]` targets when running `craft build`\n",
        "  --no-default-features    Disable the implicit `default` feature\n",
        "  --features <FEATURES>    Enable a comma-separated feature list\n",
        "  --verbose, -v            Print detailed action logs instead of the default compact summary\n",
        "  --timings                Print aggregated compiler/linker phase timings and cache stats\n",
        "  --color <WHEN>           Color mode: auto, always, never\n",
        "  --no-color               Alias for `--color never`\n",
        "\n",
        "Information:\n",
        "  --version, -V           Print version information and exit\n",
        "  -h, --help              Print this help text and exit\n",
        "\n",
        "Examples:\n",
        "  craft check\n",
        "  craft build --project-path path/to/pkg --profile release\n",
        "  craft doc --verbose\n",
        "  craft build --timings\n",
        "  craft run --features tls,simd\n",
        "  craft build --verbose --color always\n",
    )
}

#[cfg(test)]
mod tests;
