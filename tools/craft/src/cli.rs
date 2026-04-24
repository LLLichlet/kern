use crate::elaborate;
use crate::error::{Error, Result};
use shared_cli::ColorChoice as HelpColorChoice;
use std::env;
use std::path::PathBuf;

mod commands;
mod help;
mod policy;
mod render;

use self::commands::run_command;

#[cfg(test)]
use self::policy::{
    summarize_check_sources, summarize_source_security, validate_check_source_policy,
};

#[derive(Debug)]
pub enum Command {
    Help {
        topic: HelpTopic,
        color: HelpColorChoice,
    },
    Version,
    Init {
        path: Option<PathBuf>,
        ui: UiOptions,
    },
    Clean {
        path: Option<PathBuf>,
        ui: UiOptions,
    },
    Check {
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
    Install {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
        selection: InstallSelection,
        root: Option<PathBuf>,
    },
    Uninstall {
        path: Option<PathBuf>,
        ui: UiOptions,
        selection: InstallSelection,
        root: Option<PathBuf>,
    },
    Run {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
        selection: RunSelection,
    },
    Test {
        path: Option<PathBuf>,
        feature_selection: elaborate::FeatureSelection,
        ui: UiOptions,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpTopic {
    Overview,
    Command(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSelection {
    DefaultBin,
    Bin(String),
    Example(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallSelection {
    AllBins,
    Bin(String),
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

fn help_text(topic: &HelpTopic, color: HelpColorChoice) -> Result<String> {
    help::render_help(topic, color)
}

fn usage_text(topic: &HelpTopic) -> String {
    help::render_help(topic, HelpColorChoice::Never).unwrap_or_else(|_| {
        help::render_help(&HelpTopic::Overview, HelpColorChoice::Never).unwrap()
    })
}

fn known_command(name: &str) -> bool {
    matches!(
        name,
        "init"
            | "clean"
            | "check"
            | "fetch"
            | "publish"
            | "doc"
            | "build"
            | "install"
            | "uninstall"
            | "run"
            | "test"
    )
}

fn parse_help_color(args: &[String]) -> Result<HelpColorChoice> {
    let mut color = HelpColorChoice::Auto;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg == "--no-color" {
            color = HelpColorChoice::Never;
            idx += 1;
            continue;
        }
        if arg == "--color" {
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--color` requires one of: auto, always, never".to_string(),
                ));
            };
            color = match value.as_str() {
                "auto" => HelpColorChoice::Auto,
                "always" => HelpColorChoice::Always,
                "never" => HelpColorChoice::Never,
                other => {
                    return Err(Error::Usage(format!(
                        "unsupported `--color` value `{other}`; expected auto, always, or never"
                    )));
                }
            };
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--color=") {
            color = match value {
                "auto" => HelpColorChoice::Auto,
                "always" => HelpColorChoice::Always,
                "never" => HelpColorChoice::Never,
                other => {
                    return Err(Error::Usage(format!(
                        "unsupported `--color` value `{other}`; expected auto, always, or never"
                    )));
                }
            };
        }
        idx += 1;
    }
    Ok(color)
}

fn parse_args<I>(args: I) -> Result<Command>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let Some((cmd, rest)) = args.split_first() else {
        return Ok(Command::Help {
            topic: HelpTopic::Overview,
            color: parse_help_color(&args)?,
        });
    };
    if cmd == "--version" || cmd == "-V" || (cmd == "-v" && rest.is_empty()) {
        return Ok(Command::Version);
    }
    if rest.iter().any(|arg| arg == "--version" || arg == "-V") {
        return Ok(Command::Version);
    }
    let help_color = parse_help_color(&args)?;
    if cmd == "help" {
        return match rest {
            [] => Ok(Command::Help {
                topic: HelpTopic::Overview,
                color: help_color,
            }),
            [topic] => Ok(Command::Help {
                topic: HelpTopic::Command(topic.clone()),
                color: help_color,
            }),
            _ => Err(Error::Usage(
                "too many help topics provided; use `craft help <command>`".to_string(),
            )),
        };
    }
    if cmd == "--help" || cmd == "-h" {
        return Ok(Command::Help {
            topic: HelpTopic::Overview,
            color: help_color,
        });
    }
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        if known_command(cmd) {
            return Ok(Command::Help {
                topic: HelpTopic::Command(cmd.clone()),
                color: help_color,
            });
        }
    }

    match cmd.as_str() {
        "init" => {
            let options = parse_command_options(rest, init_option_mode())?;
            Ok(Command::Init {
                path: options.path,
                ui: options.ui,
            })
        }
        "clean" => {
            let options = parse_command_options(rest, clean_option_mode())?;
            Ok(Command::Clean {
                path: options.path,
                ui: options.ui,
            })
        }
        "check" => {
            let options = parse_command_options(rest, default_option_mode("check"))?;
            Ok(Command::Check {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
            })
        }
        "fetch" => {
            let options = parse_command_options(rest, default_option_mode("fetch"))?;
            Ok(Command::Fetch {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
            })
        }
        "publish" => {
            let options = parse_command_options(rest, default_option_mode("publish"))?;
            let mut feature_selection = options.feature_selection;
            feature_selection.profile = crate::script::ProfileSelection::Release;
            Ok(Command::Publish {
                path: options.path,
                feature_selection,
                ui: options.ui,
            })
        }
        "doc" => {
            let options = parse_command_options(rest, default_option_mode("doc"))?;
            Ok(Command::Doc {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
            })
        }
        "build" => {
            let options = parse_command_options(rest, build_option_mode())?;
            Ok(Command::Build {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
                include_examples: options.include_examples,
            })
        }
        "install" => {
            let options = parse_command_options(rest, install_option_mode())?;
            Ok(Command::Install {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
                selection: install_selection_from_bin_name(options.bin_name)?,
                root: options.install_root,
            })
        }
        "uninstall" => {
            let options = parse_command_options(rest, uninstall_option_mode())?;
            Ok(Command::Uninstall {
                path: options.path,
                ui: options.ui,
                selection: install_selection_from_bin_name(options.bin_name)?,
                root: options.install_root,
            })
        }
        "run" => {
            let options = parse_command_options(rest, run_option_mode())?;
            Ok(Command::Run {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
                selection: parse_run_selection(options.bin_name, options.example_name)?,
            })
        }
        "test" => {
            let options = parse_command_options(rest, default_option_mode("test"))?;
            Ok(Command::Test {
                path: options.path,
                feature_selection: options.feature_selection,
                ui: options.ui,
            })
        }
        _ => Err(Error::Usage(format!(
            "unsupported command line: {}",
            args.join(" ")
        ))),
    }
}

#[derive(Clone, Copy)]
struct CommandOptionMode {
    command_name: &'static str,
    allow_feature_selection: bool,
    allow_examples: bool,
    allow_bin_selection: bool,
    allow_example_selection: bool,
    allow_install_root: bool,
}

struct ParsedCommandOptions {
    path: Option<PathBuf>,
    feature_selection: elaborate::FeatureSelection,
    ui: UiOptions,
    include_examples: bool,
    bin_name: Option<String>,
    example_name: Option<String>,
    install_root: Option<PathBuf>,
}

fn init_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "init",
        allow_feature_selection: false,
        allow_examples: false,
        allow_bin_selection: false,
        allow_example_selection: false,
        allow_install_root: false,
    }
}

fn clean_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "clean",
        allow_feature_selection: false,
        allow_examples: false,
        allow_bin_selection: false,
        allow_example_selection: false,
        allow_install_root: false,
    }
}

fn default_option_mode(command_name: &'static str) -> CommandOptionMode {
    CommandOptionMode {
        command_name,
        allow_feature_selection: true,
        allow_examples: false,
        allow_bin_selection: false,
        allow_example_selection: false,
        allow_install_root: false,
    }
}

fn build_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "build",
        allow_feature_selection: true,
        allow_examples: true,
        allow_bin_selection: false,
        allow_example_selection: false,
        allow_install_root: false,
    }
}

fn install_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "install",
        allow_feature_selection: true,
        allow_examples: false,
        allow_bin_selection: true,
        allow_example_selection: false,
        allow_install_root: true,
    }
}

fn uninstall_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "uninstall",
        allow_feature_selection: false,
        allow_examples: false,
        allow_bin_selection: true,
        allow_example_selection: false,
        allow_install_root: true,
    }
}

fn run_option_mode() -> CommandOptionMode {
    CommandOptionMode {
        command_name: "run",
        allow_feature_selection: true,
        allow_examples: false,
        allow_bin_selection: true,
        allow_example_selection: true,
        allow_install_root: false,
    }
}

fn mode_usage_text(mode: CommandOptionMode) -> String {
    let topic = HelpTopic::Command(mode.command_name.to_string());
    usage_text(&topic)
}

fn parse_command_options(args: &[String], mode: CommandOptionMode) -> Result<ParsedCommandOptions> {
    let mut path: Option<PathBuf> = None;
    let mut feature_selection = elaborate::FeatureSelection::default();
    let mut ui = UiOptions::default();
    let mut include_examples = false;
    let mut bin_name: Option<String> = None;
    let mut example_name: Option<String> = None;
    let mut install_root: Option<PathBuf> = None;
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
            if !mode.allow_examples {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            include_examples = true;
            idx += 1;
            continue;
        }
        if arg == "--bin" {
            if !mode.allow_bin_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage("`--bin` requires a target name".to_string()));
            };
            set_named_target(&mut bin_name, value, "--bin")?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--bin=") {
            if !mode.allow_bin_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `--bin`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            set_named_target(&mut bin_name, value, "--bin")?;
            idx += 1;
            continue;
        }
        if arg == "--example" {
            if !mode.allow_example_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--example` requires a target name".to_string(),
                ));
            };
            set_named_target(&mut example_name, value, "--example")?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--example=") {
            if !mode.allow_example_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `--example`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            set_named_target(&mut example_name, value, "--example")?;
            idx += 1;
            continue;
        }
        if arg == "--root" || arg == "-r" {
            if !mode.allow_install_root {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            let Some(value) = args.get(idx + 1) else {
                return Err(Error::Usage(
                    "`--root` requires an installation root path".to_string(),
                ));
            };
            set_optional_path(&mut install_root, value, "--root")?;
            idx += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--root=") {
            if !mode.allow_install_root {
                return Err(Error::Usage(format!(
                    "unsupported option `--root`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            set_optional_path(&mut install_root, value, "--root")?;
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
            if !mode.allow_feature_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            feature_selection.enable_default = false;
            idx += 1;
            continue;
        }
        if arg == "--project-path" || arg == "-p" {
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
            if !mode.allow_feature_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
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
            if !mode.allow_feature_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `--profile`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            feature_selection.profile = parse_profile_selection(value)?;
            idx += 1;
            continue;
        }
        if arg == "--features" {
            if !mode.allow_feature_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `{arg}`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
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
            if !mode.allow_feature_selection {
                return Err(Error::Usage(format!(
                    "unsupported option `--features`\n\n{}",
                    mode_usage_text(mode)
                )));
            }
            extend_feature_selection(&mut feature_selection, value)?;
            idx += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(Error::Usage(format!(
                "unsupported option `{arg}`\n\n{}",
                mode_usage_text(mode)
            )));
        }
        return Err(Error::Usage(format!(
            "unexpected positional argument `{arg}`; use `--project-path <PATH>`\n\n{}",
            mode_usage_text(mode)
        )));
    }

    Ok(ParsedCommandOptions {
        path,
        feature_selection,
        ui,
        include_examples,
        bin_name,
        example_name,
        install_root,
    })
}

fn parse_run_selection(
    bin_name: Option<String>,
    example_name: Option<String>,
) -> Result<RunSelection> {
    match (bin_name, example_name) {
        (Some(_), Some(_)) => Err(Error::Usage(
            "`craft run` accepts at most one of `--bin <NAME>` or `--example <NAME>`".to_string(),
        )),
        (Some(name), None) => Ok(RunSelection::Bin(name)),
        (None, Some(name)) => Ok(RunSelection::Example(name)),
        (None, None) => Ok(RunSelection::DefaultBin),
    }
}

fn install_selection_from_bin_name(bin_name: Option<String>) -> Result<InstallSelection> {
    match bin_name {
        Some(name) => Ok(InstallSelection::Bin(name)),
        None => Ok(InstallSelection::AllBins),
    }
}

fn set_named_target(slot: &mut Option<String>, raw: &str, flag: &str) -> Result<()> {
    if slot.is_some() {
        return Err(Error::Usage(format!("`{flag}` may only be provided once")));
    }
    if raw.trim().is_empty() {
        return Err(Error::Usage("target names must not be empty".to_string()));
    }
    *slot = Some(raw.to_string());
    Ok(())
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
    set_optional_path(slot, raw, "--project-path")
}

fn set_optional_path(slot: &mut Option<PathBuf>, raw: &str, flag: &str) -> Result<()> {
    if let Some(existing_path) = slot {
        return Err(Error::Usage(format!(
            "multiple `{flag}` values provided: `{}` and `{raw}`",
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

#[cfg(test)]
mod tests;
