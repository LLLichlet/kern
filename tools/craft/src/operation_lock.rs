use crate::error::{Error, Result};
use crate::local_state;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct WorkspaceOperationLock {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug)]
struct LockOwner {
    pid: u32,
    #[cfg(unix)]
    start_ticks: Option<u64>,
}

impl WorkspaceOperationLock {
    pub(crate) fn acquire(workspace_root: &Path, operation: &str) -> Result<Self> {
        let path = workspace_lock_path(workspace_root);
        local_state::ensure_parent_dir(&path)?;

        loop {
            match try_acquire(&path, operation) {
                Ok(lock) => return Ok(lock),
                Err(err) if is_lock_contention_error(&path, &err) => {
                    if reclaim_stale_lock(&path)? {
                        continue;
                    }
                    thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(err) => return Err(Error::from_io(&path, err)),
            }
        }
    }
}

fn is_lock_contention_error(path: &Path, err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::AlreadyExists
        || (cfg!(windows) && err.kind() == ErrorKind::PermissionDenied && path.exists())
}

impl Drop for WorkspaceOperationLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            let _ = err;
        }
    }
}

fn try_acquire(path: &Path, operation: &str) -> std::io::Result<WorkspaceOperationLock> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(lock_contents(operation).as_bytes())?;
    file.sync_all()?;
    Ok(WorkspaceOperationLock {
        path: path.to_path_buf(),
    })
}

fn lock_contents(operation: &str) -> String {
    let created_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let mut contents = format!(
        "pid={}\noperation={}\ncreated_unix_ms={}\n",
        pid, operation, created_ms
    );
    #[cfg(unix)]
    if let Some(start_ticks) = read_process_start_ticks(pid) {
        contents.push_str(&format!("start_ticks={start_ticks}\n"));
    }
    contents
}

fn workspace_lock_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".craft")
        .join("lock")
        .join("workspace.lock")
}

fn reclaim_stale_lock(path: &Path) -> Result<bool> {
    let Some(owner) = read_lock_owner(path)? else {
        return Ok(false);
    };

    if lock_owner_is_alive(owner) {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(err) => Err(Error::from_io(path, err)),
    }
}

fn read_lock_owner(path: &Path) -> Result<Option<LockOwner>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(Error::from_io(path, err)),
    };

    let mut pid = None;
    #[cfg(unix)]
    let mut start_ticks = None;
    for line in contents.lines() {
        if let Some(raw_pid) = line.strip_prefix("pid=") {
            pid = raw_pid.parse::<u32>().ok();
            continue;
        }
        #[cfg(unix)]
        if let Some(raw_start_ticks) = line.strip_prefix("start_ticks=") {
            start_ticks = raw_start_ticks.parse::<u64>().ok();
        }
    }

    Ok(pid.map(|pid| LockOwner {
        pid,
        #[cfg(unix)]
        start_ticks,
    }))
}

#[cfg(unix)]
fn lock_owner_is_alive(owner: LockOwner) -> bool {
    let Some(current_start_ticks) = read_process_start_ticks(owner.pid) else {
        return false;
    };

    match owner.start_ticks {
        Some(lock_start_ticks) => current_start_ticks == lock_start_ticks,
        None => owner.pid != std::process::id(),
    }
}

#[cfg(not(unix))]
fn lock_owner_is_alive(_owner: LockOwner) -> bool {
    true
}

#[cfg(unix)]
fn read_process_start_ticks(pid: u32) -> Option<u64> {
    let path = Path::new("/proc").join(pid.to_string()).join("stat");
    let contents = fs::read_to_string(path).ok()?;
    let end = contents.rfind(") ")?;
    let fields = contents[end + 2..].split_whitespace().collect::<Vec<_>>();
    fields.get(19)?.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::read_process_start_ticks;
    use super::{WorkspaceOperationLock, workspace_lock_path};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn removes_lock_file_when_guard_drops() {
        let root = temp_dir("craft-workspace-lock-drop");
        let lock_path = workspace_lock_path(&root);

        {
            let _lock = WorkspaceOperationLock::acquire(&root, "build").unwrap();
            assert!(lock_path.is_file());
        }

        assert!(!lock_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn reclaims_stale_lock_from_dead_process() {
        let root = temp_dir("craft-workspace-lock-stale");
        let lock_path = workspace_lock_path(&root);
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        fs::write(&lock_path, "pid=999999\noperation=test\n").unwrap();

        let _lock = WorkspaceOperationLock::acquire(&root, "build").unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.contains(&format!("pid={}", std::process::id())));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn reclaims_lock_when_pid_matches_but_start_time_differs() {
        let root = temp_dir("craft-workspace-lock-pid-reuse");
        let lock_path = workspace_lock_path(&root);
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        let pid = std::process::id();
        let current_start_ticks = read_process_start_ticks(pid).unwrap();
        fs::write(
            &lock_path,
            format!(
                "pid={pid}\noperation=test\nstart_ticks={}\n",
                current_start_ticks.saturating_sub(1)
            ),
        )
        .unwrap();

        let _lock = WorkspaceOperationLock::acquire(&root, "build").unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.contains(&format!("pid={pid}")));
        assert!(contents.contains(&format!("start_ticks={current_start_ticks}")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn waits_until_existing_lock_is_released() {
        let root = temp_dir("craft-workspace-lock-wait");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let root_for_worker = root.clone();

        let worker = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_worker, "build").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let root_for_waiter = root.clone();
        let start = Instant::now();
        let waiter = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_waiter, "test").unwrap();
            start.elapsed()
        });

        thread::sleep(Duration::from_millis(200));
        release_tx.send(()).unwrap();

        worker.join().unwrap();
        let waited = waiter.join().unwrap();
        assert!(waited >= Duration::from_millis(150));

        let _ = fs::remove_dir_all(root);
    }
}
