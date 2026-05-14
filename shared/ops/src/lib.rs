use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

pub const HOST_TOOL_BINARIES: &[&str] = &["kernc", "craft", "kern-lsp"];
pub const OFFICIAL_LIBRARY_LAYERS: &[&str] = &["base", "rt", "std"];

#[derive(Debug)]
pub struct OpsError {
    message: String,
}

impl OpsError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for OpsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OpsError {}

impl From<io::Error> for OpsError {
    fn from(value: io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<serde_json::Error> for OpsError {
    fn from(value: serde_json::Error) -> Self {
        Self::new(value.to_string())
    }
}

pub type OpsResult<T> = Result<T, OpsError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTarget {
    pub archive_target: String,
    pub cargo_target: Option<String>,
    pub exe_suffix: &'static str,
    pub archive_extension: &'static str,
    pub is_windows: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, Deserialize)]
pub struct SdkManifest {
    pub sdk_version: Option<String>,
    pub host_target: String,
    pub binaries: Option<Vec<String>>,
    pub libraries: Option<Vec<String>>,
    pub toolchain: Option<ToolchainManifestSection>,
}

#[derive(Debug, Deserialize)]
pub struct ToolchainManifestSection {
    pub bundled: Option<bool>,
    pub components: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn repo_root() -> OpsResult<PathBuf> {
    Ok(env::current_dir()?)
}

pub fn detect_host_target() -> OpsResult<HostTarget> {
    let arch = match env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(OpsError::new(format!("unsupported architecture: {other}"))),
    };

    match env::consts::OS {
        "linux" => Ok(HostTarget {
            archive_target: format!("{arch}-linux-gnu"),
            cargo_target: None,
            exe_suffix: "",
            archive_extension: "tar.gz",
            is_windows: false,
        }),
        "macos" => Ok(HostTarget {
            archive_target: format!("{arch}-apple-darwin"),
            cargo_target: None,
            exe_suffix: "",
            archive_extension: "tar.gz",
            is_windows: false,
        }),
        "windows" => {
            if arch != "x86_64" {
                return Err(OpsError::new(
                    "Windows packaging currently only supports x86_64-windows-msvc",
                ));
            }
            Ok(HostTarget {
                archive_target: "x86_64-windows-msvc".to_string(),
                cargo_target: Some("x86_64-pc-windows-msvc".to_string()),
                exe_suffix: ".exe",
                archive_extension: "zip",
                is_windows: true,
            })
        }
        other => Err(OpsError::new(format!(
            "unsupported operating system: {other}"
        ))),
    }
}

pub fn default_install_root(host: &HostTarget) -> OpsResult<PathBuf> {
    if host.is_windows {
        let Some(profile) = env::var_os("USERPROFILE") else {
            return Err(OpsError::new("USERPROFILE is not set"));
        };
        return Ok(PathBuf::from(profile).join(".kern"));
    }
    Ok(home_dir()?.join(".kern"))
}

pub fn read_sdk_manifest(sdk_root: &Path) -> OpsResult<SdkManifest> {
    let manifest_path = sdk_root.join("manifest").join("sdk.json");
    if !manifest_path.is_file() {
        return Err(OpsError::new(format!(
            "SDK manifest `{}` is missing",
            manifest_path.display()
        )));
    }
    let source = fs::read_to_string(&manifest_path)?;
    Ok(serde_json::from_str(&source)?)
}

pub fn validate_sdk_root(sdk_root: &Path, expected_target: &str) -> OpsResult<SdkManifest> {
    let manifest = read_sdk_manifest(sdk_root)?;
    if manifest.host_target != expected_target {
        return Err(OpsError::new(format!(
            "SDK host target mismatch in `{}`: expected `{expected_target}`, found `{}`",
            sdk_root.join("manifest").join("sdk.json").display(),
            manifest.host_target
        )));
    }

    let bin_dir = sdk_root.join("bin");
    for binary in HOST_TOOL_BINARIES {
        let unix = bin_dir.join(binary);
        let windows = bin_dir.join(format!("{binary}.exe"));
        if !unix.is_file() && !windows.is_file() {
            return Err(OpsError::new(format!(
                "SDK binary `{binary}` is missing from `{}`",
                sdk_root.display()
            )));
        }
    }

    let library_root = sdk_root.join("lib").join("kern");
    if !library_root.join("Craft.toml").is_file() {
        return Err(OpsError::new(
            "SDK official library workspace manifest is missing",
        ));
    }
    for layer in OFFICIAL_LIBRARY_LAYERS {
        if !library_root.join(layer).join("init.rn").is_file() {
            return Err(OpsError::new(format!(
                "SDK official library `{layer}` is missing"
            )));
        }
    }
    if !library_root.join("craft").join("init.rn").is_file() {
        return Err(OpsError::new("SDK craft script modules are missing"));
    }
    if !sdk_root.join("toolchain").join("host").join("bin").is_dir() {
        return Err(OpsError::new("SDK toolchain layout is incomplete"));
    }

    Ok(manifest)
}

pub fn copy_sdk_contents(sdk_root: &Path, install_root: &Path) -> OpsResult<()> {
    let install_root = absolute_path(install_root)?;
    let Some(install_parent) = install_root.parent() else {
        return Err(OpsError::new(format!(
            "installation root `{}` has no parent directory",
            install_root.display()
        )));
    };
    let install_name = install_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OpsError::new("installation root has an invalid final path component"))?;
    fs::create_dir_all(install_parent)?;

    let unique = unique_suffix();
    let staging_root = install_parent.join(format!(".{install_name}.installing-{unique}"));
    let backup_root = install_parent.join(format!(".{install_name}.previous-{unique}"));
    remove_path_if_exists(&staging_root)?;
    remove_path_if_exists(&backup_root)?;
    fs::create_dir_all(&staging_root)?;

    let result = (|| -> OpsResult<()> {
        for entry in fs::read_dir(sdk_root)? {
            let entry = entry?;
            let source = entry.path();
            let dest = staging_root.join(entry.file_name());
            copy_path(&source, &dest)?;
        }

        let moved_existing = if install_root.exists() {
            fs::rename(&install_root, &backup_root)?;
            true
        } else {
            false
        };

        if let Err(err) = fs::rename(&staging_root, &install_root) {
            if moved_existing && backup_root.exists() && !install_root.exists() {
                let _ = fs::rename(&backup_root, &install_root);
            }
            return Err(OpsError::new(format!(
                "failed to replace existing installation at `{}`: {err}",
                install_root.display()
            )));
        }

        Ok(())
    })();

    let _ = remove_path_if_exists(&staging_root);
    if result.is_ok() {
        let _ = remove_path_if_exists(&backup_root);
    }
    result
}

pub fn verify_installed_tools(install_root: &Path, host: &HostTarget) -> OpsResult<()> {
    let bin_dir = install_root.join("bin");
    for binary in HOST_TOOL_BINARIES {
        let binary_path = bin_dir.join(format!("{binary}{}", host.exe_suffix));
        verify_binary_starts(&binary_path)?;
    }
    Ok(())
}

pub fn verify_binary_starts(binary_path: &Path) -> OpsResult<CommandResult> {
    if !binary_path.is_file() {
        return Err(OpsError::new(format!(
            "installed binary `{}` is missing",
            binary_path.display()
        )));
    }
    let output = Command::new(binary_path)
        .arg("--version")
        .output()
        .map_err(|err| {
            OpsError::new(format!(
                "failed to start `{}`: {err}",
                binary_path.display()
            ))
        })?;
    let result = command_result(output);
    if result.status_code == Some(0) {
        let first = first_non_empty_line(&result.stdout)
            .or_else(|| first_non_empty_line(&result.stderr))
            .unwrap_or("<no version output>");
        println!(
            "=> Verified {}: {}",
            binary_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>"),
            first
        );
        Ok(result)
    } else {
        Err(OpsError::new(format!(
            "failed to start `{}` after installation:\n{}{}",
            binary_path.display(),
            result.stdout,
            result.stderr
        )))
    }
}

pub fn run_command(cmd: &[OsString], cwd: Option<&Path>) -> OpsResult<()> {
    if cmd.is_empty() {
        return Err(OpsError::new("cannot run an empty command"));
    }
    println!(
        "=> Running: {}",
        cmd.iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command.status().map_err(|err| {
        OpsError::new(format!(
            "failed to start `{}`: {err}",
            cmd[0].to_string_lossy()
        ))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(OpsError::new(format!(
            "command failed with exit code {:?}: {}",
            status.code(),
            cmd.iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        )))
    }
}

pub fn run_command_capture(cmd: &[OsString], cwd: Option<&Path>) -> OpsResult<CommandResult> {
    if cmd.is_empty() {
        return Err(OpsError::new("cannot run an empty command"));
    }
    println!(
        "=> Running: {}",
        cmd.iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut command = Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|err| {
        OpsError::new(format!(
            "failed to start `{}`: {err}",
            cmd[0].to_string_lossy()
        ))
    })?;
    Ok(command_result(output))
}

pub fn load_workspace_version(root: &Path) -> OpsResult<String> {
    let source = fs::read_to_string(root.join("Cargo.toml"))?;
    let mut in_workspace_package = false;
    for line in source.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if in_workspace_package && line.starts_with("version = ") {
            let value = line
                .split_once('=')
                .map(|(_, value)| value.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            if value.is_empty() {
                return Err(OpsError::new("workspace version is empty"));
            }
            return Ok(value);
        }
    }
    Err(OpsError::new(format!(
        "failed to resolve workspace version from `{}`",
        root.join("Cargo.toml").display()
    )))
}

pub fn extract_archive_with_system_tool(
    archive_path: &Path,
    extract_root: &Path,
    kind: ArchiveKind,
) -> OpsResult<PathBuf> {
    fs::create_dir_all(extract_root)?;
    match kind {
        ArchiveKind::TarGz => run_command(
            &[
                OsString::from("tar"),
                OsString::from("-xf"),
                archive_path.as_os_str().to_owned(),
                OsString::from("-C"),
                extract_root.as_os_str().to_owned(),
            ],
            None,
        )?,
        ArchiveKind::Zip => {
            if cfg!(windows) {
                let script = format!(
                    "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
                    archive_path.display(),
                    extract_root.display()
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
                )?;
            } else {
                run_command(
                    &[
                        OsString::from("unzip"),
                        OsString::from("-q"),
                        archive_path.as_os_str().to_owned(),
                        OsString::from("-d"),
                        extract_root.as_os_str().to_owned(),
                    ],
                    None,
                )?;
            }
        }
    }

    single_directory_child(extract_root)
}

pub fn archive_kind_from_path(path: &Path) -> OpsResult<ArchiveKind> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OpsError::new("archive path has an invalid file name"))?;
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(ArchiveKind::TarGz)
    } else if name.ends_with(".zip") {
        Ok(ArchiveKind::Zip)
    } else {
        Err(OpsError::new(format!(
            "unsupported archive extension for `{}`",
            path.display()
        )))
    }
}

pub fn make_temp_dir(prefix: &str) -> OpsResult<PathBuf> {
    let mut root = env::var_os("KERN_OPS_TEMP_ROOT")
        .or_else(|| env::var_os("RUNNER_TEMP"))
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    fs::create_dir_all(&root)?;
    root.push(format!("{prefix}{}", unique_suffix()));
    fs::create_dir_all(&root)?;
    Ok(root)
}

pub fn remove_path_if_exists(path: &Path) -> OpsResult<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn copy_path(source: &Path, dest: &Path) -> OpsResult<()> {
    if source.is_dir() {
        copy_dir_recursive(source, dest)
    } else {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, dest)?;
        Ok(())
    }
}

pub fn copy_dir_recursive(source: &Path, dest: &Path) -> OpsResult<()> {
    if !source.is_dir() {
        return Err(OpsError::new(format!(
            "directory `{}` does not exist",
            source.display()
        )));
    }
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        copy_path(&source_path, &dest_path)?;
    }
    Ok(())
}

pub fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn home_dir() -> OpsResult<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| OpsError::new("HOME is not set"))
}

fn absolute_path(path: &Path) -> OpsResult<PathBuf> {
    if path.exists() {
        Ok(path.canonicalize()?)
    } else if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

fn single_directory_child(root: &Path) -> OpsResult<PathBuf> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        }
    }
    if directories.len() != 1 {
        return Err(OpsError::new(format!(
            "expected exactly one directory in `{}`",
            root.display()
        )));
    }
    Ok(directories.remove(0))
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

fn command_result(output: Output) -> CommandResult {
    CommandResult {
        status_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_host_target_uses_kern_archive_labels() {
        let host = detect_host_target().unwrap();
        assert!(
            host.archive_target.ends_with("-linux-gnu")
                || host.archive_target.ends_with("-apple-darwin")
                || host.archive_target.ends_with("-windows-msvc")
        );
        assert_eq!(
            host.archive_extension,
            if host.is_windows { "zip" } else { "tar.gz" }
        );
    }

    #[test]
    fn archive_kind_accepts_release_archive_extensions() {
        assert_eq!(
            archive_kind_from_path(Path::new("kern-v0.7.6-x86_64-linux-gnu.tar.gz")).unwrap(),
            ArchiveKind::TarGz
        );
        assert_eq!(
            archive_kind_from_path(Path::new("kern-v0.7.6-x86_64-windows-msvc.zip")).unwrap(),
            ArchiveKind::Zip
        );
        assert!(archive_kind_from_path(Path::new("kern.tar")).is_err());
    }

    #[test]
    fn load_workspace_version_reads_workspace_package_section() {
        let root = make_temp_dir("shared-ops-version-test-").unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nversion = \"9.9.9\"\n\n[workspace.package]\nversion = \"0.7.6\"\n",
        )
        .unwrap();

        assert_eq!(load_workspace_version(&root).unwrap(), "0.7.6");
        remove_path_if_exists(&root).unwrap();
    }
}
