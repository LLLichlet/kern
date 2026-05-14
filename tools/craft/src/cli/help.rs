use crate::error::{Error, Result};
use shared_cli::{ColorChoice, HelpDoc, HelpSection};

use super::{HelpTopic, version_text};

pub(super) fn render_help(topic: &HelpTopic, color: ColorChoice) -> Result<String> {
    let doc = match topic {
        HelpTopic::Overview => overview_doc(),
        HelpTopic::Command(command) => command_doc(command)?,
    };
    Ok(doc.render(color))
}

fn overview_doc() -> HelpDoc {
    HelpDoc::new(version_text())
        .summary("Kern package manager, builder, and workspace driver")
        .usage("craft <command> [OPTIONS]")
        .usage("craft help <command>")
        .usage("craft --version")
        .section(
            HelpSection::new("Popular Commands")
                .entry("init", "Create a package in the selected directory")
                .entry("clean", "Remove derived .craft state for a package")
                .entry(
                    "check",
                    "Validate manifests, scripts, sources, and analysis inputs",
                )
                .entry("build", "Compile the selected package graph")
                .entry("run", "Build and run a selected binary or example target")
                .entry("test", "Build and run discovered test targets"),
        )
        .section(
            HelpSection::new("Other Commands")
                .entry(
                    "fetch",
                    "Populate external package sources into .craft cache",
                )
                .entry("publish", "Verify release publishability")
                .entry("doc", "Render package docs to Markdown")
                .entry("fmt", "Normalize Kern source text deterministically")
                .entry("style", "Report source metrics and comment ratios")
                .entry("install", "Build bin targets and install them under a root")
                .entry(
                    "uninstall",
                    "Remove installed bin targets from an install root",
                ),
        )
        .section(
            HelpSection::new("Common Options")
                .entry(
                    "--project-path, -p <PATH>",
                    "Select the package root, workspace root, or Craft.toml manifest",
                )
                .entry("--profile <NAME>", "Build profile: dev or release")
                .entry(
                    "--no-default-features",
                    "Disable the implicit `default` feature",
                )
                .entry("--features <A,B>", "Enable a comma-separated feature list")
                .entry("--verbose, -v/-vv/-vvv", "Increase diagnostic detail")
                .entry("--timings", "Print aggregated action timings")
                .entry("--color <WHEN>", "Output color mode: auto, always, never"),
        )
        .example("craft check", "Validate the current package graph")
        .example(
            "craft build --project-path examples --profile release --examples",
            "Build a package in release mode",
        )
        .example(
            "craft run --example hello_world",
            "Run a named example target",
        )
        .note("Use `craft help <command>` for command-specific options and examples.")
}

fn command_doc(command: &str) -> Result<HelpDoc> {
    let doc = match command {
        "init" => command_template(
            "init",
            "Create a package in the selected directory without adding a parent folder",
            &["craft init [OPTIONS]"],
            HelpSection::new("Options")
                .entry("--project-path, -p <PATH>", "Directory to initialize")
                .entry("--verbose, -v/-vv/-vvv", "Increase diagnostic detail")
                .entry("--timings", "Print aggregated timing information")
                .entry("--color <WHEN>", "Color mode: auto, always, never"),
            &[
                ("craft init", "Initialize the current directory"),
                (
                    "craft init --project-path demos/http",
                    "Initialize another directory",
                ),
            ],
        ),
        "check" => feature_command_doc(
            "check",
            "Validate manifests, scripts, sources, and derived analysis inputs",
            "craft check [OPTIONS]",
            &[("craft check", "Validate the current package graph")],
        ),
        "clean" => command_template(
            "clean",
            "Remove derived .craft build, cache, and analysis state for the selected package",
            &["craft clean [OPTIONS]"],
            HelpSection::new("Options")
                .entry(
                    "--project-path, -p <PATH>",
                    "Select the package root, workspace root, or Craft.toml manifest",
                )
                .entry("--verbose, -v/-vv/-vvv", "Increase diagnostic detail")
                .entry("--timings", "Print aggregated timing information")
                .entry("--color <WHEN>", "Color mode: auto, always, never"),
            &[
                ("craft clean", "Clean the current package .craft state"),
                (
                    "craft clean --project-path examples",
                    "Clean another package",
                ),
            ],
        ),
        "fetch" => feature_command_doc(
            "fetch",
            "Fetch external package sources into the local .craft cache",
            "craft fetch [OPTIONS]",
            &[("craft fetch", "Warm the local source cache")],
        ),
        "publish" => feature_command_doc(
            "publish",
            "Verify release publishability with release-oriented defaults",
            "craft publish [OPTIONS]",
            &[("craft publish", "Validate the current committed revision")],
        ),
        "doc" => feature_command_doc(
            "doc",
            "Render package docs to Markdown",
            "craft doc [OPTIONS]",
            &[(
                "craft doc --verbose",
                "Show generated doc files and actions",
            )],
        ),
        "fmt" => command_template(
            "fmt",
            "Normalize Kern source text deterministically",
            &["craft fmt [OPTIONS]"],
            HelpSection::new("Options")
                .entry(
                    "--project-path, -p <PATH>",
                    "Select the package root, workspace root, or Craft.toml manifest",
                )
                .entry(
                    "--check",
                    "Report files that would change without writing them",
                )
                .entry("--verbose, -v/-vv/-vvv", "Show changed source files")
                .entry("--color <WHEN>", "Color mode: auto, always, never"),
            &[
                ("craft fmt", "Normalize source text in the current package"),
                (
                    "craft fmt --check",
                    "Check whether source text is already normalized",
                ),
            ],
        ),
        "style" => command_template(
            "style",
            "Report source metrics, doc coverage, and advisory style suggestions",
            &["craft style [OPTIONS]"],
            HelpSection::new("Options")
                .entry(
                    "--project-path, -p <PATH>",
                    "Select the package root, workspace root, or Craft.toml manifest",
                )
                .entry(
                    "--verbose, -v/-vv/-vvv",
                    "Show per-package metrics and suggestion locations",
                )
                .entry("--color <WHEN>", "Color mode: auto, always, never"),
            &[
                ("craft style", "Report metrics for the current package"),
                (
                    "craft style --project-path library --verbose",
                    "Report per-package metrics for a workspace",
                ),
            ],
        ),
        "build" => command_template(
            "build",
            "Compile the selected package graph and report the derived action plan",
            &["craft build [OPTIONS]"],
            feature_options_section().entry(
                "--examples",
                "Include `[example].roots` targets in the build graph",
            ),
            &[
                ("craft build", "Build the current package"),
                (
                    "craft build --project-path path/to/pkg --profile release",
                    "Build another package in release mode",
                ),
                (
                    "craft build --examples --features tls,simd",
                    "Build examples with explicit features enabled",
                ),
            ],
        ),
        "install" => command_template(
            "install",
            "Build bin targets and copy them into an installation root",
            &["craft install [OPTIONS]"],
            feature_options_section()
                .entry("--bin, -b <NAME>", "Install only the named binary target")
                .entry(
                    "--root, -r <PATH>",
                    "Installation root; binaries land in `PATH/bin`",
                ),
            &[
                ("craft install", "Install all binary targets"),
                (
                    "craft install --project-path examples/limine-mkiso --bin limine-mkiso",
                    "Install one binary from another package",
                ),
            ],
        ),
        "uninstall" => command_template(
            "uninstall",
            "Remove installed bin targets from an installation root",
            &["craft uninstall [OPTIONS]"],
            HelpSection::new("Options")
                .entry(
                    "--project-path, -p <PATH>",
                    "Select the package root, workspace root, or Craft.toml manifest",
                )
                .entry("--bin, -b <NAME>", "Remove only the named binary target")
                .entry(
                    "--root, -r <PATH>",
                    "Installation root; binaries are removed from `PATH/bin`",
                )
                .entry("--verbose, -v/-vv/-vvv", "Increase diagnostic detail")
                .entry("--timings", "Print aggregated action timings")
                .entry("--color <WHEN>", "Color mode: auto, always, never"),
            &[
                (
                    "craft uninstall",
                    "Remove all installed binaries for the package",
                ),
                (
                    "craft uninstall --bin limine-mkiso --root ~/.local",
                    "Remove one installed binary from a custom root",
                ),
            ],
        ),
        "run" => command_template(
            "run",
            "Build and execute a selected binary or example target",
            &[
                "craft run [OPTIONS] [-- <ARGS>...]",
                "craft run --example <NAME> [OPTIONS] [-- <ARGS>...]",
            ],
            feature_options_section()
                .entry("--bin, -b <NAME>", "Run the named binary target")
                .entry("--example <NAME>", "Run the named example target")
                .entry("-- <ARGS>...", "Pass remaining arguments to the target"),
            &[
                ("craft run", "Run the default binary target"),
                (
                    "craft run --example hello_world",
                    "Run a named example target",
                ),
                (
                    "craft run -- --help",
                    "Pass option-like arguments to the program",
                ),
                (
                    "craft run --example hello_world --features tracing",
                    "Run an example with explicit features enabled",
                ),
            ],
        ),
        "test" => command_template(
            "test",
            "Build and execute discovered test targets",
            &[
                "craft test [OPTIONS] [-- <ARGS>...]",
                "craft test --test <NAME> [OPTIONS] [-- <ARGS>...]",
            ],
            feature_options_section()
                .entry("--test <NAME>", "Run only the named test target")
                .entry(
                    "-- <ARGS>...",
                    "Pass remaining arguments to each selected test target",
                ),
            &[
                ("craft test", "Run the current package tests"),
                ("craft test --test fs_io", "Run one named test target"),
                ("craft test -- smoke", "Pass arguments to each test binary"),
                (
                    "craft test --project-path workspace/member --features simd",
                    "Run tests for another package with explicit features",
                ),
            ],
        ),
        other => {
            return Err(Error::Usage(format!("unknown help topic `{other}`")));
        }
    };

    Ok(doc)
}

fn command_template(
    command: &str,
    summary: &str,
    usages: &[&str],
    options: HelpSection,
    examples: &[(&str, &str)],
) -> HelpDoc {
    let mut doc = HelpDoc::new(format!("Craft {} help", command)).summary(summary);
    for usage in usages {
        doc = doc.usage(*usage);
    }
    doc = doc.section(options);
    for (command, description) in examples {
        doc = doc.example(*command, *description);
    }
    doc.note("Global flags `--help` and `--version` are accepted anywhere on the command line.")
}

fn feature_command_doc(
    command: &str,
    summary: &str,
    usage: &str,
    examples: &[(&str, &str)],
) -> HelpDoc {
    command_template(
        command,
        summary,
        &[usage],
        feature_options_section(),
        examples,
    )
}

fn feature_options_section() -> HelpSection {
    HelpSection::new("Options")
        .entry(
            "--project-path, -p <PATH>",
            "Select the package root, workspace root, or Craft.toml manifest",
        )
        .entry("--profile <NAME>", "Build profile: dev or release")
        .entry(
            "--no-default-features",
            "Disable the implicit `default` feature",
        )
        .entry("--features <A,B>", "Enable a comma-separated feature list")
        .entry("--verbose, -v/-vv/-vvv", "Increase diagnostic detail")
        .entry("--timings", "Print aggregated action timings")
        .entry("--color <WHEN>", "Output color mode: auto, always, never")
}
