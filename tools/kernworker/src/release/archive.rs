//! Archive creation helpers for release distributions.
//!
//! The implementation selects the platform archive format and delegates to
//! system tools with deterministic paths so produced SDK archives are easy to
//! verify and publish.

use super::util::powershell_quote;
use shared_ops::{OpsError, OpsResult, run_command};
use std::ffi::OsString;
use std::path::Path;

pub fn create_archive(
    root: &Path,
    dist_dir: &Path,
    archive_path: &Path,
    host: &shared_ops::HostTarget,
) -> OpsResult<()> {
    let dist_name = dist_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OpsError::new("distribution directory has an invalid file name"))?;
    println!("Packaging {dist_name}...");
    if host.is_windows {
        let script = format!(
            "Compress-Archive -LiteralPath {} -DestinationPath {} -Force",
            powershell_quote(&dist_dir.display().to_string()),
            powershell_quote(&archive_path.display().to_string())
        );
        run_command(
            &[
                OsString::from("powershell"),
                OsString::from("-NoProfile"),
                OsString::from("-ExecutionPolicy"),
                OsString::from("Bypass"),
                OsString::from("-Command"),
                OsString::from(script),
            ],
            None,
        )
    } else {
        run_command(
            &[
                OsString::from("tar"),
                OsString::from("-czf"),
                archive_path.as_os_str().to_owned(),
                OsString::from(dist_name),
            ],
            Some(root),
        )
    }
}
