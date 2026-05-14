use shared_cli::{ColorChoice, ErrorReport, HelpDoc, HelpSection};
use shared_ops::{
    OpsError, OpsResult, copy_dir_recursive, load_workspace_version, make_temp_dir,
    remove_path_if_exists, repo_root, run_command, run_command_capture,
};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const SMOKE_TESTS: &[&str] = &[
    "anonymous_aggregates",
    "atomics",
    "regressions",
    "stdlib",
    "traits",
];
const HOSTED_TESTS: &[&str] = &["collections", "filesystem"];

#[derive(Debug)]
enum Command {
    Ci(CiCommand),
    Help,
}

#[derive(Debug)]
enum CiCommand {
    KerncTests { mode: TestMode },
    CraftPolicy,
    Help,
}

#[derive(Debug, Clone, Copy)]
enum TestMode {
    Smoke,
    Hosted,
    All,
}

fn main() {
    if let Err(err) = run() {
        eprint!(
            "{}",
            ErrorReport::new("kernworker error", err.to_string()).render(ColorChoice::Auto)
        );
        std::process::exit(1);
    }
}

fn run() -> OpsResult<()> {
    match parse_args(env::args().skip(1).collect())? {
        Command::Ci(CiCommand::KerncTests { mode }) => run_kernc_tests(mode),
        Command::Ci(CiCommand::CraftPolicy) => run_craft_policy_checks(),
        Command::Ci(CiCommand::Help) => {
            print!("{}", ci_help().render(ColorChoice::Auto));
            Ok(())
        }
        Command::Help => {
            print!("{}", help().render(ColorChoice::Auto));
            Ok(())
        }
    }
}

fn parse_args(args: Vec<String>) -> OpsResult<Command> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match command {
        "ci" => parse_ci_args(&args[1..]).map(Command::Ci),
        "help" | "--help" | "-h" => Ok(Command::Help),
        other => Err(OpsError::new(format!(
            "unknown command `{other}`; run `kernworker help`"
        ))),
    }
}

fn parse_ci_args(args: &[String]) -> OpsResult<CiCommand> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(CiCommand::Help);
    };

    match command {
        "kernc-tests" => parse_kernc_tests_args(&args[1..]),
        "craft-policy" => Ok(CiCommand::CraftPolicy),
        "help" | "--help" | "-h" => Ok(CiCommand::Help),
        other => Err(OpsError::new(format!(
            "unknown ci command `{other}`; run `kernworker ci help`"
        ))),
    }
}

fn parse_kernc_tests_args(args: &[String]) -> OpsResult<CiCommand> {
    let mut mode = TestMode::All;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--mode` requires a value"));
                };
                mode = match value.as_str() {
                    "smoke" => TestMode::Smoke,
                    "hosted" => TestMode::Hosted,
                    "all" => TestMode::All,
                    other => {
                        return Err(OpsError::new(format!(
                            "unsupported kernc test mode `{other}`"
                        )));
                    }
                };
            }
            "--help" | "-h" => {
                print!("{}", kernc_tests_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected kernc-tests argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(CiCommand::KerncTests { mode })
}

fn run_kernc_tests(mode: TestMode) -> OpsResult<()> {
    let suites: Vec<(&str, &[&str])> = match mode {
        TestMode::Smoke => vec![("smoke", SMOKE_TESTS)],
        TestMode::Hosted => vec![("hosted", HOSTED_TESTS)],
        TestMode::All => vec![("smoke", SMOKE_TESTS), ("hosted", HOSTED_TESTS)],
    };

    for (label, tests) in suites {
        println!("Running {label} suite...");
        for test in tests {
            run_command(
                &[
                    OsString::from("cargo"),
                    OsString::from("test"),
                    OsString::from("-p"),
                    OsString::from("kernc_cli"),
                    OsString::from("--test"),
                    OsString::from(test),
                ],
                None,
            )?;
        }
    }

    Ok(())
}

fn run_craft_policy_checks() -> OpsResult<()> {
    let root = repo_root()?;
    let version = load_workspace_version(&root)?;
    let fixtures_root = root.join("tools/craft/fixtures/release-policy");
    let temp_root = make_temp_dir("craft-policy-")?;
    let result = (|| -> OpsResult<()> {
        let allowed = prepare_fixture(&fixtures_root.join("allowed"), &temp_root, &version)?;
        let allowed_exception = prepare_fixture(
            &fixtures_root.join("allowed-exception"),
            &temp_root,
            &version,
        )?;
        let blocked = prepare_fixture(&fixtures_root.join("blocked"), &temp_root, &version)?;

        println!("Running craft release policy allow fixture...");
        run_craft_check(&allowed)?;

        println!("Running craft release policy allow-exception fixture...");
        run_craft_check(&allowed_exception)?;

        println!("Running craft release policy block fixture...");
        let blocked_result = run_command_capture(&craft_check_command(&blocked), None)?;
        if blocked_result.status_code == Some(0) {
            return Err(OpsError::new(format!(
                "craft release policy fixture unexpectedly passed: {}",
                blocked.display()
            )));
        }
        let output = format!("{}{}", blocked_result.stdout, blocked_result.stderr);
        if !output.contains("release source policy rejected") {
            return Err(OpsError::new(
                "craft release policy fixture failed for an unexpected reason",
            ));
        }

        println!("craft release policy fixtures passed");
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp_root);
    result
}

fn prepare_fixture(source: &Path, temp_root: &Path, version: &str) -> OpsResult<PathBuf> {
    let dest = temp_root.join(
        source
            .file_name()
            .ok_or_else(|| OpsError::new("fixture path has no final component"))?,
    );
    copy_dir_recursive(source, &dest)?;
    rewrite_kern_versions(&dest, version)?;
    Ok(dest)
}

fn rewrite_kern_versions(root: &Path, version: &str) -> OpsResult<()> {
    for entry in walk_files(root)? {
        if entry.file_name().and_then(|name| name.to_str()) != Some("Craft.toml") {
            continue;
        }
        let source = fs::read_to_string(&entry)?;
        let rewritten = source
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("kern = ") {
                    let indent_len = line.len() - line.trim_start().len();
                    format!("{}kern = \"{}\"", &line[..indent_len], version)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&entry, rewritten)?;
    }
    Ok(())
}

fn walk_files(root: &Path) -> OpsResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_files(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

fn run_craft_check(project_path: &Path) -> OpsResult<()> {
    run_command(&craft_check_command(project_path), None)
}

fn craft_check_command(project_path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("cargo"),
        OsString::from("run"),
        OsString::from("-p"),
        OsString::from("craft"),
        OsString::from("--"),
        OsString::from("check"),
        OsString::from("--project-path"),
        project_path.as_os_str().to_owned(),
        OsString::from("--profile"),
        OsString::from("release"),
    ]
}

fn help() -> HelpDoc {
    HelpDoc::new("kernworker")
        .summary("Kern repository maintenance and CI worker.")
        .usage("kernworker <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("ci", "Run CI-oriented repository checks")
                .entry("help", "Show this help text"),
        )
        .example(
            "kernworker ci kernc-tests --mode smoke",
            "run the smoke integration tests",
        )
        .example(
            "kernworker ci craft-policy",
            "run craft release policy fixtures",
        )
        .note("Release packaging migration will move here after the shared SDK operations settle.")
}

fn ci_help() -> HelpDoc {
    HelpDoc::new("kernworker ci")
        .summary("CI-oriented repository checks.")
        .usage("kernworker ci <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("kernc-tests", "Run grouped kernc integration tests")
                .entry("craft-policy", "Run craft release policy fixtures"),
        )
}

fn kernc_tests_help() -> HelpDoc {
    HelpDoc::new("kernworker ci kernc-tests")
        .summary("Run grouped kernc integration tests.")
        .usage("kernworker ci kernc-tests [--mode smoke|hosted|all]")
        .section(
            HelpSection::new("Options")
                .entry("--mode smoke", "run smoke integration tests")
                .entry("--mode hosted", "run hosted integration tests")
                .entry("--mode all", "run all grouped integration tests"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kernc_test_modes() {
        assert!(matches!(
            parse_args(vec![
                "ci".to_string(),
                "kernc-tests".to_string(),
                "--mode".to_string(),
                "smoke".to_string()
            ])
            .unwrap(),
            Command::Ci(CiCommand::KerncTests {
                mode: TestMode::Smoke
            })
        ));
        assert!(
            parse_args(vec![
                "ci".to_string(),
                "kernc-tests".to_string(),
                "--mode".to_string(),
                "bad".to_string()
            ])
            .is_err()
        );
    }

    #[test]
    fn rewrites_nested_fixture_kern_versions() {
        let root = make_temp_dir("kernworker-fixture-test-").unwrap();
        let package = root.join("package");
        fs::create_dir_all(&package).unwrap();
        fs::write(
            root.join("Craft.toml"),
            "[package]\nname = \"root\"\nkern = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(
            package.join("Craft.toml"),
            "[package]\nname = \"package\"\n    kern = \"0.0.0\"\n",
        )
        .unwrap();

        rewrite_kern_versions(&root, "0.7.6").unwrap();

        assert!(
            fs::read_to_string(root.join("Craft.toml"))
                .unwrap()
                .contains("kern = \"0.7.6\"")
        );
        assert!(
            fs::read_to_string(package.join("Craft.toml"))
                .unwrap()
                .contains("    kern = \"0.7.6\"")
        );
        remove_path_if_exists(&root).unwrap();
    }
}
