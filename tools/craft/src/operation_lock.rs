//! Cooperative filesystem locks for Craft operations.
//!
//! Workspace, output, and cache locks prevent concurrent builds from corrupting
//! shared state. Stale lock recovery uses process liveness and Linux start-time
//! checks where available.

use crate::error::{Error, Result};
use crate::local_state;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const LOCK_WAIT_REPORT_DELAY: Duration = Duration::from_secs(1);
const LOCK_WAIT_REPORT_INTERVAL: Duration = Duration::from_secs(5);
const INVALID_LOCK_METADATA_GRACE: Duration = Duration::from_millis(250);

pub(crate) struct WorkspaceOperationLock {
    path: PathBuf,
}

pub(crate) struct OutputOperationLock {
    path: PathBuf,
}

pub(crate) struct CacheOperationLock {
    path: PathBuf,
}

#[cfg(test)]
pub(crate) struct TestResourceLock {
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
        acquire_lock(&path, operation).map(|lock| Self { path: lock.path })
    }
}

impl OutputOperationLock {
    pub(crate) fn acquire(output_path: &Path, operation: &str) -> Result<Self> {
        let path = output_lock_path(output_path);
        acquire_lock(&path, operation).map(|lock| Self { path: lock.path })
    }
}

impl CacheOperationLock {
    pub(crate) fn acquire(cache_path: &Path, operation: &str) -> Result<Self> {
        let path = cache_lock_path(cache_path);
        acquire_lock(&path, operation).map(|lock| Self { path: lock.path })
    }
}

#[cfg(test)]
impl TestResourceLock {
    pub(crate) fn try_acquire(path: &Path, operation: &str) -> Result<Option<Self>> {
        local_state::ensure_parent_dir(path)?;
        match try_acquire(path, operation) {
            Ok(lock) => Ok(Some(Self { path: lock.path })),
            Err(err) if is_lock_contention_error(path, &err) => {
                if reclaim_stale_lock(path)? {
                    return Self::try_acquire(path, operation);
                }
                Ok(None)
            }
            Err(err) => Err(Error::from_io(path, err)),
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

impl Drop for OutputOperationLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            let _ = err;
        }
    }
}

impl Drop for CacheOperationLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            let _ = err;
        }
    }
}

#[cfg(test)]
impl Drop for TestResourceLock {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            let _ = err;
        }
    }
}

struct AcquiredLock {
    path: PathBuf,
}

fn acquire_lock(path: &Path, operation: &str) -> Result<AcquiredLock> {
    local_state::ensure_parent_dir(path)?;
    let mut wait_started = None;
    let mut last_report_at = None;

    loop {
        match try_acquire(path, operation) {
            Ok(lock) => return Ok(lock),
            Err(err) if is_lock_contention_error(path, &err) => {
                if reclaim_stale_lock(path)? {
                    continue;
                }
                report_lock_wait(path, operation, &mut wait_started, &mut last_report_at)?;
                thread::sleep(LOCK_POLL_INTERVAL);
            }
            Err(err) => return Err(Error::from_io(path, err)),
        }
    }
}

fn try_acquire(path: &Path, operation: &str) -> std::io::Result<AcquiredLock> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(lock_contents(operation).as_bytes())?;
    file.sync_all()?;
    Ok(AcquiredLock {
        path: path.to_path_buf(),
    })
}

fn lock_contents(operation: &str) -> String {
    #[cfg(unix)]
    {
        let created_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let pid = std::process::id();
        let mut contents = format!(
            "pid={}\noperation={}\ncreated_unix_ms={}\n",
            pid, operation, created_ms
        );
        if let Some(start_ticks) = read_process_start_ticks(pid) {
            contents.push_str(&format!("start_ticks={start_ticks}\n"));
        }
        contents
    }
    #[cfg(not(unix))]
    {
        let created_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let pid = std::process::id();
        format!(
            "pid={}\noperation={}\ncreated_unix_ms={}\n",
            pid, operation, created_ms
        )
    }
}

fn workspace_lock_path(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(".craft")
        .join("lock")
        .join("workspace.lock")
}

fn output_lock_path(output_path: &Path) -> PathBuf {
    let file_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let file_name = sanitize_lock_component(file_name);
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{file_name}.craft.lock"))
}

fn cache_lock_path(cache_path: &Path) -> PathBuf {
    let file_name = cache_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cache");
    let file_name = sanitize_lock_component(file_name);
    cache_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{file_name}.craft.lock"))
}

fn sanitize_lock_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn reclaim_stale_lock(path: &Path) -> Result<bool> {
    match read_lock_owner(path)? {
        Some(owner) if lock_owner_is_alive(owner) => {
            return Ok(false);
        }
        Some(_) => {}
        None if !invalid_lock_metadata_is_stale(path)? => {
            return Ok(false);
        }
        None => {}
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(true),
        Err(err) => Err(Error::from_io(path, err)),
    }
}

fn invalid_lock_metadata_is_stale(path: &Path) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(Error::from_io(path, err)),
    };
    let modified = metadata.modified().map_err(Error::from_io_plain)?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    Ok(age >= INVALID_LOCK_METADATA_GRACE)
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
    match (owner.start_ticks, read_process_start_ticks(owner.pid)) {
        (Some(lock_start_ticks), Some(current_start_ticks)) => {
            current_start_ticks == lock_start_ticks
        }
        _ => process_exists(owner.pid),
    }
}

#[cfg(windows)]
fn lock_owner_is_alive(owner: LockOwner) -> bool {
    process_exists(owner.pid)
}

#[cfg(all(not(unix), not(windows)))]
fn lock_owner_is_alive(owner: LockOwner) -> bool {
    let _ = owner.pid;
    true
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn CloseHandle(object: Handle) -> i32;
    }

    // SAFETY: OpenProcess is called with query-only access, no inherited handle, and a PID read
    // from lock metadata. A null handle is handled as "process is not alive".
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }

    let mut exit_code = 0u32;
    // SAFETY: handle is non-null and owned by this function. exit_code points to a valid u32
    // for the duration of the call.
    let alive =
        unsafe { GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE };
    // SAFETY: handle was returned by OpenProcess above and has not been closed yet.
    let _ = unsafe { CloseHandle(handle) };
    alive
}

fn report_lock_wait(
    path: &Path,
    operation: &str,
    wait_started: &mut Option<Instant>,
    last_report_at: &mut Option<Instant>,
) -> Result<()> {
    let now = Instant::now();
    let started = wait_started.get_or_insert(now);
    let waited = now.saturating_duration_since(*started);
    if waited < LOCK_WAIT_REPORT_DELAY {
        return Ok(());
    }
    if let Some(last) = last_report_at
        && now.saturating_duration_since(*last) < LOCK_WAIT_REPORT_INTERVAL
    {
        return Ok(());
    }

    let owner = read_lock_owner(path)?;
    let owner_text = owner
        .map(|owner| format!(" held by pid {}", owner.pid))
        .unwrap_or_default();
    eprintln!(
        "craft: waiting {}s for {operation} lock `{}`{owner_text}",
        waited.as_secs(),
        path.display()
    );
    *last_report_at = Some(now);
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_process_start_ticks(pid: u32) -> Option<u64> {
    let path = Path::new("/proc").join(pid.to_string()).join("stat");
    let contents = fs::read_to_string(path).ok()?;
    let end = contents.rfind(") ")?;
    let fields = contents[end + 2..].split_whitespace().collect::<Vec<_>>();
    fields.get(19)?.parse::<u64>().ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn read_process_start_ticks(_pid: u32) -> Option<u64> {
    None
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    use std::ffi::c_int;

    unsafe extern "C" {
        fn kill(pid: c_int, sig: c_int) -> c_int;
    }

    // SAFETY: kill(pid, 0) does not send a signal; it only asks the kernel whether the process
    // exists or is inaccessible. pid comes from lock metadata and is narrowed to libc::c_int.
    let result = unsafe { kill(pid as c_int, 0) };
    if result == 0 {
        return true;
    }

    matches!(std::io::Error::last_os_error().raw_os_error(), Some(1))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::read_process_start_ticks;
    use super::{
        CacheOperationLock, INVALID_LOCK_METADATA_GRACE, OutputOperationLock,
        WorkspaceOperationLock, cache_lock_path, output_lock_path, workspace_lock_path,
    };
    #[cfg(windows)]
    use super::{LockOwner, lock_owner_is_alive};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    #[test]
    fn reclaims_workspace_lock_with_invalid_metadata_after_grace_period() {
        let root = temp_dir("craft-workspace-lock-invalid");
        let lock_path = workspace_lock_path(&root);
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        fs::write(&lock_path, "operation=build\n").unwrap();
        thread::sleep(INVALID_LOCK_METADATA_GRACE + Duration::from_millis(50));

        let _lock = WorkspaceOperationLock::acquire(&root, "build").unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.contains(&format!("pid={}", std::process::id())));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(target_os = "linux")]
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
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let root_for_worker = root.clone();

        let worker = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_worker, "build").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let root_for_waiter = root.clone();
        let waiter = thread::spawn(move || {
            let _lock = WorkspaceOperationLock::acquire(&root_for_waiter, "test").unwrap();
            acquired_tx.send(()).unwrap();
        });

        thread::sleep(Duration::from_millis(150));
        assert!(acquired_rx.try_recv().is_err());
        release_tx.send(()).unwrap();

        worker.join().unwrap();
        waiter.join().unwrap();
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn output_lock_uses_output_parent_directory() {
        let output = Path::new("/tmp/demo/artifact.o");
        assert_eq!(
            output_lock_path(output),
            PathBuf::from("/tmp/demo/.artifact.o.craft.lock")
        );
    }

    #[test]
    fn cache_lock_uses_cache_parent_directory() {
        let cache = Path::new("/tmp/demo/.craft/git-dependencies/pkg/abcdef");
        assert_eq!(
            cache_lock_path(cache),
            PathBuf::from("/tmp/demo/.craft/git-dependencies/pkg/.abcdef.craft.lock")
        );
    }

    #[test]
    fn output_lock_waits_until_existing_lock_is_released() {
        let root = temp_dir("craft-output-lock-wait");
        let output = root.join("build").join("demo.o");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let output_for_worker = output.clone();

        let worker = thread::spawn(move || {
            let _lock = OutputOperationLock::acquire(&output_for_worker, "compile").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let output_for_waiter = output.clone();
        let waiter = thread::spawn(move || {
            let _lock = OutputOperationLock::acquire(&output_for_waiter, "compile").unwrap();
            acquired_tx.send(()).unwrap();
        });

        thread::sleep(Duration::from_millis(150));
        assert!(acquired_rx.try_recv().is_err());
        release_tx.send(()).unwrap();

        worker.join().unwrap();
        waiter.join().unwrap();
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_lock_waits_until_existing_lock_is_released() {
        let root = temp_dir("craft-cache-lock-wait");
        let cache = root.join(".craft").join("git-dependencies/pkg/abcdef");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let cache_for_worker = cache.clone();

        let worker = thread::spawn(move || {
            let _lock = CacheOperationLock::acquire(&cache_for_worker, "git-source").unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        ready_rx.recv().unwrap();
        let cache_for_waiter = cache.clone();
        let waiter = thread::spawn(move || {
            let _lock = CacheOperationLock::acquire(&cache_for_waiter, "git-source").unwrap();
            acquired_tx.send(()).unwrap();
        });

        thread::sleep(Duration::from_millis(150));
        assert!(acquired_rx.try_recv().is_err());
        release_tx.send(()).unwrap();

        worker.join().unwrap();
        waiter.join().unwrap();
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reclaims_output_lock_with_invalid_metadata_after_grace_period() {
        let root = temp_dir("craft-output-lock-invalid");
        let output = root.join("build").join("demo.o");
        let lock_path = output_lock_path(&output);
        fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        fs::write(&lock_path, "operation=compile\n").unwrap();
        thread::sleep(INVALID_LOCK_METADATA_GRACE + Duration::from_millis(50));

        let _lock = OutputOperationLock::acquire(&output, "compile").unwrap();
        let contents = fs::read_to_string(&lock_path).unwrap();
        assert!(contents.contains(&format!("pid={}", std::process::id())));

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_lock_owner_check_distinguishes_live_and_dead_processes() {
        assert!(lock_owner_is_alive(LockOwner {
            pid: std::process::id(),
        }));
        assert!(!lock_owner_is_alive(LockOwner { pid: u32::MAX }));
    }
}
