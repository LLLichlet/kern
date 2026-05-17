use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub(crate) const FAILPOINT_AFTER_WORKSPACE_LOCK: &str = "after-workspace-lock";
pub(crate) const FAILPOINT_AFTER_STAGED_OUTPUT_WRITE: &str = "after-staged-output-write";
pub(crate) const FAILPOINT_AFTER_COMPILE_STATE_WRITE: &str = "after-compile-state-write";
pub(crate) const FAILPOINT_AFTER_LINK_STATE_WRITE: &str = "after-link-state-write";
pub(crate) const FAILPOINT_AFTER_ANALYSIS_CONTEXT_SYNC: &str = "after-analysis-context-sync";

pub(crate) struct TestCommandSlot {
    _lock: Option<crate::operation_lock::TestResourceLock>,
}

pub(crate) fn acquire_command_slot() -> TestCommandSlot {
    if std::env::var_os("CRAFT_TEST_FAILPOINT").is_some() {
        return TestCommandSlot { _lock: None };
    }

    let limit = configured_command_limit();
    loop {
        for slot in 0..limit {
            let path = command_slot_path(slot);
            let lock =
                crate::operation_lock::TestResourceLock::try_acquire(&path, "craft-test-command")
                    .unwrap_or_else(|err| {
                        panic!(
                            "failed to acquire craft test command slot `{}`: {err}",
                            path.display()
                        )
                    });
            if let Some(lock) = lock {
                return TestCommandSlot { _lock: Some(lock) };
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
}

pub(crate) fn test_parallel_worker_count(job_count: usize) -> usize {
    if job_count < 2 {
        return 1;
    }
    let limit = read_positive_usize_env("CRAFT_TEST_PARALLEL_WORKERS").unwrap_or(2);
    limit.min(job_count)
}

fn configured_command_limit() -> usize {
    if let Some(limit) = read_positive_usize_env("CRAFT_TEST_COMMAND_LIMIT") {
        return limit;
    }

    let available = thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    if available <= 4 {
        1
    } else {
        (available / 4).clamp(1, 8)
    }
}

fn command_slot_path(slot: usize) -> PathBuf {
    command_slot_dir().join(format!("command-{slot}.lock"))
}

fn command_slot_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("CRAFT_TEST_RESOURCE_DIR") {
        return PathBuf::from(path);
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let package_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(manifest_dir);
    package_root.join("target").join("craft-test-resources")
}

fn read_positive_usize_env(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
}

pub(crate) fn hit(name: &str) {
    let Ok(active) = std::env::var("CRAFT_TEST_FAILPOINT") else {
        return;
    };
    if active != name {
        return;
    }

    if let Ok(path) = std::env::var("CRAFT_TEST_FAILPOINT_READY_FILE") {
        let _ = fs::write(path, name);
    }

    loop {
        thread::sleep(Duration::from_millis(50));
    }
}
