use std::env;
use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
#[ignore = "stress test; run explicitly when validating craft workspace concurrency"]
fn workspace_concurrency_stress() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("craft manifest should live under tools/craft")
        .to_path_buf();
    let project_input = env::var_os("CRAFT_STRESS_PROJECT")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join("examples"));
    let source_root = resolve_project_root(&project_input);
    assert!(
        source_root.join("Craft.toml").is_file(),
        "expected Craft.toml under {}",
        source_root.display()
    );

    let rounds = env_usize("ROUNDS", 8);
    assert!(rounds >= 1, "ROUNDS must be a positive integer");
    let jobs = env_usize("JOBS", 2);
    assert!(jobs >= 2, "JOBS must be an integer >= 2");
    let keep_success = env::var("KEEP_SUCCESS").as_deref() == Ok("1");
    let craft = PathBuf::from(env!("CARGO_BIN_EXE_craft"));

    eprintln!(
        "workspace={} rounds={} jobs={} keep_success={}",
        source_root.display(),
        rounds,
        jobs,
        keep_success
    );

    for round in 1..=rounds {
        run_round(&craft, &source_root, round, jobs, keep_success);
    }
}

fn resolve_project_root(input: &Path) -> PathBuf {
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        env::current_dir().unwrap().join(input)
    };
    if path.is_file() {
        path.parent().unwrap().to_path_buf()
    } else {
        path
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("{name} must be an integer"))
        })
        .unwrap_or(default)
}

fn run_round(craft: &Path, source_root: &Path, round: usize, jobs: usize, keep_success: bool) {
    let mut handles = Vec::new();
    for job in 1..=jobs {
        let workspace = prepare_workspace_copy(source_root, round, job);
        let log = workspace.join("craft-test.log");
        let out = fs::File::create(&log).unwrap();
        let err = out.try_clone().unwrap();
        let child = Command::new(craft)
            .arg("test")
            .arg("--project-path")
            .arg(workspace.join("Craft.toml"))
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap_or_else(|err| {
                panic!("failed to start craft for {}: {err}", workspace.display())
            });
        handles.push(JobHandle {
            job,
            workspace,
            log,
            child,
        });
    }

    let mut failures = Vec::new();
    let mut statuses = Vec::new();
    for mut handle in handles {
        let status = handle.child.wait().unwrap();
        let code = status.code().unwrap_or(-1);
        statuses.push((handle.job, code));
        if !status.success() {
            failures.push((
                handle.job,
                handle.workspace.clone(),
                handle.log.clone(),
                code,
            ));
        } else if !keep_success {
            fs::remove_dir_all(&handle.workspace).unwrap();
        }
    }

    eprint!("round={round}");
    for (job, code) in &statuses {
        eprint!(" job{job}={code}");
    }
    eprintln!();

    if failures.is_empty() {
        return;
    }

    for (job, workspace, log, code) in &failures {
        eprintln!("failure job{job} status={code}");
        eprintln!("failure workspace: {}", workspace.display());
        eprintln!("failure log: {}", log.display());
        eprintln!("{}", first_log_lines(log, 220));
    }
    panic!("craft workspace concurrency stress failed");
}

struct JobHandle {
    job: usize,
    workspace: PathBuf,
    log: PathBuf,
    child: std::process::Child,
}

fn prepare_workspace_copy(source_root: &Path, round: usize, job: usize) -> PathBuf {
    let dest = env::temp_dir().join(format!("craft-race-r{round}-j{job}-{}", unique_suffix()));
    fs::create_dir_all(&dest).unwrap();
    copy_dir_excluding_state(source_root, &dest).unwrap();
    dest
}

fn copy_dir_excluding_state(source: &Path, dest: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == ".craft" {
            continue;
        }
        let source_path = entry.path();
        let dest_path = dest.join(name);
        if source_path.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_dir_excluding_state(&source_path, &dest_path)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

fn first_log_lines(path: &Path, limit: usize) -> String {
    let source =
        fs::read_to_string(path).unwrap_or_else(|err| format!("<failed to read log: {err}>"));
    let mut out = String::new();
    for line in source.lines().take(limit) {
        let _ = writeln!(out, "{line}");
    }
    out
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}
