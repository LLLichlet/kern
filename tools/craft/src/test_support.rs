use std::fs;
use std::thread;
use std::time::Duration;

pub(crate) const FAILPOINT_AFTER_WORKSPACE_LOCK: &str = "after-workspace-lock";
pub(crate) const FAILPOINT_AFTER_STAGED_OUTPUT_WRITE: &str = "after-staged-output-write";
pub(crate) const FAILPOINT_AFTER_COMPILE_STATE_WRITE: &str = "after-compile-state-write";
pub(crate) const FAILPOINT_AFTER_LINK_STATE_WRITE: &str = "after-link-state-write";
pub(crate) const FAILPOINT_AFTER_ANALYSIS_CONTEXT_SYNC: &str = "after-analysis-context-sync";

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
