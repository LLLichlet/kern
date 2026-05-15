use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
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

    pub fn io(path: &Path, action: &str, source: io::Error) -> Self {
        Self::new(format!("failed to {action} `{}`: {source}", path.display()))
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
pub struct CiToolchainPolicy {
    pub runner_os: String,
    pub mode: String,
    pub host_target: Option<String>,
    pub llvm_version: String,
    pub llvm_major: u64,
    pub prefix_env: String,
    pub provider_kind: String,
    pub provider: String,
    pub target_provider_kind: String,
    pub target_provider: String,
    pub required_tools: Vec<String>,
    pub raw: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub struct BundledToolchain {
    pub source_label: String,
    pub prefix: PathBuf,
    pub bindir: PathBuf,
    pub libdir: PathBuf,
    pub includedir: PathBuf,
    pub version: String,
    pub tools: serde_json::Map<String, serde_json::Value>,
    pub resource_dir: Option<PathBuf>,
    pub sysroot_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub path: String,
    pub kind: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

pub fn repo_root() -> OpsResult<PathBuf> {
    Ok(env::current_dir()?)
}

pub fn read_json_value(path: &Path) -> OpsResult<serde_json::Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

pub fn write_json_value(path: &Path, value: &serde_json::Value) -> OpsResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = serde_json::to_string_pretty(value)?;
    out.push('\n');
    fs::write(path, out)?;
    Ok(())
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
        if !library_root.join(layer).join("mod.kn").is_file() {
            return Err(OpsError::new(format!(
                "SDK official library `{layer}` is missing"
            )));
        }
    }
    if !library_root.join("craft").join("mod.kn").is_file() {
        return Err(OpsError::new("SDK craft build modules are missing"));
    }
    if !sdk_root.join("toolchain").join("host").join("bin").is_dir() {
        return Err(OpsError::new("SDK toolchain layout is incomplete"));
    }

    validate_sdk_toolchain_manifest(sdk_root)?;

    Ok(manifest)
}

pub fn validate_sdk_toolchain_manifest(sdk_root: &Path) -> OpsResult<()> {
    let manifest_path = sdk_root.join("manifest").join("sdk.json");
    let manifest = read_json_value(&manifest_path)?;
    let Some(toolchain) = manifest
        .get("toolchain")
        .and_then(|value| value.as_object())
    else {
        return Err(OpsError::new(
            "SDK manifest is missing the `toolchain` section",
        ));
    };
    let Some(components) = toolchain
        .get("components")
        .and_then(|value| value.as_object())
    else {
        return Err(OpsError::new(
            "SDK manifest toolchain components are invalid",
        ));
    };
    if toolchain
        .get("bundled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        let host_target = manifest
            .get("host_target")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let required = manifest_required_components(
            toolchain,
            sdk_runtime_required_components(host_target),
            "SDK manifest toolchain",
        )?;
        for component in &required {
            if !components.contains_key(component) {
                return Err(OpsError::new(format!(
                    "SDK manifest is missing bundled component `{component}`"
                )));
            }
        }
        for (component, entry) in components {
            validate_component_record(sdk_root, component, entry, "SDK bundled component")?;
        }
        for check in manifest_health_checks(toolchain, &required, "SDK manifest toolchain")? {
            let entry = components.get(&check.component).ok_or_else(|| {
                OpsError::new(format!(
                    "SDK manifest is missing bundled component `{}`",
                    check.component
                ))
            })?;
            validate_manifest_health_check(sdk_root, &check.component, entry, &check.kind)?;
        }
    }
    Ok(())
}

pub fn validate_toolchain_root(toolchain_root: &Path, expected_target: &str) -> OpsResult<()> {
    let manifest_path = toolchain_root.join("manifest").join("toolchain.json");
    if !manifest_path.is_file() {
        return Err(OpsError::new(format!(
            "toolchain manifest `{}` is missing",
            manifest_path.display()
        )));
    }
    let manifest = read_json_value(&manifest_path)?;
    if manifest
        .get("host_target")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        != expected_target
    {
        return Err(OpsError::new(format!(
            "toolchain host target mismatch in `{}`",
            manifest_path.display()
        )));
    }
    if !toolchain_root.join("toolchain").join("host").is_dir() {
        return Err(OpsError::new("toolchain host layout is incomplete"));
    }
    let Some(manifest_obj) = manifest.as_object() else {
        return Err(OpsError::new("toolchain manifest is invalid"));
    };
    let Some(components) = manifest
        .get("components")
        .and_then(|value| value.as_object())
    else {
        return Err(OpsError::new("toolchain manifest components are invalid"));
    };
    let required = manifest_required_components(
        manifest_obj,
        full_toolchain_required_components(expected_target),
        "toolchain manifest",
    )?;
    for component in &required {
        let Some(entry) = components.get(component) else {
            return Err(OpsError::new(format!(
                "toolchain manifest is missing component `{component}`"
            )));
        };
        validate_component_record(toolchain_root, component, entry, "toolchain component")?;
    }
    if let Some(resource) = components.get("clang_resource_dir") {
        validate_component_record(
            toolchain_root,
            "clang_resource_dir",
            resource,
            "toolchain component",
        )?;
    }
    for check in manifest_health_checks(manifest_obj, &required, "toolchain manifest")? {
        let entry = components.get(&check.component).ok_or_else(|| {
            OpsError::new(format!(
                "toolchain manifest is missing component `{}`",
                check.component
            ))
        })?;
        validate_manifest_health_check(toolchain_root, &check.component, entry, &check.kind)?;
    }
    Ok(())
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

pub fn configure_path(install_bin: &Path, host: &HostTarget) -> OpsResult<()> {
    if host.is_windows {
        configure_windows_path(install_bin)
    } else {
        configure_unix_path(install_bin)
    }
}

pub fn download_file(url: &str, dest: &Path) -> OpsResult<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if cfg!(windows) {
        let escaped_url = powershell_quote(url);
        let escaped_dest = powershell_quote(&dest.display().to_string());
        let script = format!(
            "Invoke-WebRequest -Uri {escaped_url} -OutFile {escaped_dest} -UseBasicParsing"
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
                OsString::from("curl"),
                OsString::from("-fsSL"),
                OsString::from(url),
                OsString::from("-o"),
                dest.as_os_str().to_owned(),
            ],
            None,
        )
    }
}

pub fn fetch_latest_github_release(github_repo: &str) -> OpsResult<Option<String>> {
    if cfg!(windows) {
        let script = format!(
            "(Invoke-RestMethod -Uri {}).tag_name",
            powershell_quote(&format!(
                "https://api.github.com/repos/{github_repo}/releases/latest"
            ))
        );
        let result = run_command_capture(
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
        if result.status_code == Some(0) {
            let tag = result.stdout.trim();
            return Ok((!tag.is_empty()).then(|| tag.to_string()));
        }
        return Ok(None);
    }

    let result = run_command_capture(
        &[
            OsString::from("curl"),
            OsString::from("-fsSLI"),
            OsString::from("-o"),
            OsString::from("/dev/null"),
            OsString::from("-w"),
            OsString::from("%{url_effective}"),
            OsString::from(format!("https://github.com/{github_repo}/releases/latest")),
        ],
        None,
    )?;
    if result.status_code != Some(0) {
        return Ok(None);
    }
    let resolved = result.stdout.trim();
    Ok(resolved
        .split("/releases/tag/")
        .nth(1)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string))
}

pub fn infer_release_version_from_archive_name(name: &str, target: &str) -> Option<String> {
    let prefix = "kern-";
    let suffixes = [format!("-{target}.tar.gz"), format!("-{target}.zip")];
    suffixes.iter().find_map(|suffix| {
        name.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .map(str::to_string)
    })
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
    run_command_with_env(cmd, cwd, &[])
}

pub fn run_command_with_env(
    cmd: &[OsString],
    cwd: Option<&Path>,
    envs: &[(&str, &str)],
) -> OpsResult<()> {
    if cmd.is_empty() {
        return Err(OpsError::new("cannot run an empty command"));
    }
    eprintln!(
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
    for (key, value) in envs {
        command.env(key, value);
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
    eprintln!(
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

pub fn resolve_official_library_root(root: &Path) -> OpsResult<PathBuf> {
    let candidate = env::var_os("KERNLIB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("library"));
    let library_root = if candidate.is_absolute() {
        candidate
    } else {
        root.join(candidate)
    };
    if library_root.join("Craft.toml").is_file()
        && OFFICIAL_LIBRARY_LAYERS
            .iter()
            .all(|layer| library_root.join(layer).join("mod.kn").is_file())
    {
        return Ok(library_root);
    }
    Err(OpsError::new(format!(
        "official Kern library workspace is missing or incomplete at `{}`",
        library_root.display()
    )))
}

pub fn resolve_bundled_toolchain(
    host: &HostTarget,
    explicit_prefix: Option<&Path>,
) -> OpsResult<BundledToolchain> {
    let (source_label, prefix) = if let Some(prefix) = explicit_prefix {
        (
            "explicit-toolchain-prefix".to_string(),
            prefix.to_path_buf(),
        )
    } else if let Some(root) = env::var_os("KERN_TOOLCHAIN_ROOT").map(PathBuf::from)
        && root.is_dir()
    {
        ("KERN_TOOLCHAIN_ROOT".to_string(), root)
    } else if let Some((key, root)) = find_llvm_sys_prefix() {
        (key, root)
    } else {
        let llvm_config = find_program(&["llvm-config-21", "llvm-config"]).ok_or_else(|| {
            OpsError::new("failed to locate `llvm-config`; cannot bundle host LLVM toolchain")
        })?;
        let prefix = tool_output(&[
            llvm_config.as_os_str().to_owned(),
            OsString::from("--prefix"),
        ])?;
        ("llvm-config".to_string(), PathBuf::from(prefix.trim()))
    };
    let prefix = if prefix.exists() {
        prefix.canonicalize()?
    } else {
        prefix
    };
    if !prefix.is_dir() {
        return Err(OpsError::new(format!(
            "LLVM toolchain prefix `{}` does not exist",
            prefix.display()
        )));
    }
    let llvm_config =
        resolve_llvm_tool("llvm-config", "21", &prefix.join("bin"), host, false, false)?
            .ok_or_else(|| {
                OpsError::new(format!(
                    "failed to resolve `llvm-config` within LLVM prefix `{}`",
                    prefix.display()
                ))
            })?;
    let version = tool_output(&[
        llvm_config.as_os_str().to_owned(),
        OsString::from("--version"),
    ])?
    .trim()
    .to_string();
    let major = version.split('.').next().unwrap_or("21");
    let bindir = PathBuf::from(
        tool_output(&[
            llvm_config.as_os_str().to_owned(),
            OsString::from("--bindir"),
        ])?
        .trim(),
    )
    .canonicalize()?;
    let libdir = PathBuf::from(
        tool_output(&[
            llvm_config.as_os_str().to_owned(),
            OsString::from("--libdir"),
        ])?
        .trim(),
    )
    .canonicalize()?;
    let includedir = PathBuf::from(
        tool_output(&[
            llvm_config.as_os_str().to_owned(),
            OsString::from("--includedir"),
        ])?
        .trim(),
    )
    .canonicalize()?;
    let mut tools = serde_json::Map::new();
    for (key, name, required) in [
        ("llvm_config", "llvm-config", true),
        ("clang", "clang", true),
        ("clangxx", "clang++", true),
        ("llvm_ar", "llvm-ar", true),
    ] {
        if let Some(tool) = resolve_llvm_tool(name, major, &bindir, host, required, false)? {
            tools.insert(
                key.into(),
                serde_json::Value::String(tool.display().to_string()),
            );
        }
    }
    let lld_name = if host.archive_target.ends_with("windows-msvc") {
        "lld-link"
    } else if host.archive_target.ends_with("apple-darwin") {
        "ld64.lld"
    } else {
        "ld.lld"
    };
    let allow_lld_path_lookup = host.archive_target.ends_with("apple-darwin");
    let lld = resolve_llvm_tool(lld_name, major, &bindir, host, true, allow_lld_path_lookup)?
        .ok_or_else(|| OpsError::new(format!("failed to resolve LLVM tool `{lld_name}`")))?;
    tools.insert(
        "lld".into(),
        serde_json::Value::String(lld.display().to_string()),
    );
    if host.archive_target.ends_with("windows-msvc") {
        let llvm_lib = resolve_llvm_tool("llvm-lib", major, &bindir, host, true, false)?
            .ok_or_else(|| OpsError::new("failed to resolve LLVM tool `llvm-lib`"))?;
        tools.insert(
            "llvm_lib".into(),
            serde_json::Value::String(llvm_lib.display().to_string()),
        );
    }
    let resource_dir = tools
        .get("clang")
        .and_then(|value| value.as_str())
        .and_then(|clang| {
            tool_output(&[
                OsString::from(clang),
                OsString::from("--print-resource-dir"),
            ])
            .ok()
            .map(|out| PathBuf::from(out.trim()))
        })
        .filter(|path| path.exists());
    let sysroot_dir = if host.archive_target.ends_with("apple-darwin") {
        env::var_os("SDKROOT")
            .map(PathBuf::from)
            .filter(|path| path.exists())
    } else {
        None
    };
    Ok(BundledToolchain {
        source_label,
        prefix,
        bindir,
        libdir,
        includedir,
        version,
        tools,
        resource_dir,
        sysroot_dir,
    })
}

pub fn sdk_manifest_json(
    version: &str,
    archive_target: &str,
    bundled_toolchain: Option<&BundledToolchain>,
    records: Option<&serde_json::Map<String, serde_json::Value>>,
) -> serde_json::Value {
    let mut components = serde_json::Map::new();
    let mut bundled = false;
    let mut source = serde_json::Value::Null;
    let mut provenance = serde_json::Value::Null;
    let mut required_components = Vec::<String>::new();
    let mut health_checks = Vec::<serde_json::Value>::new();
    let layout = serde_json::json!({
        "root": "toolchain",
        "host_root": "toolchain/host",
        "bin_dir": "toolchain/host/bin",
        "lib_dir": "toolchain/host/lib",
        "include_dir": "toolchain/host/include",
        "sysroot_dir": "toolchain/host/sysroot",
    });
    let mut notes = vec![
        "The SDK prefers the bundled host toolchain when present.",
        "Ambient LLVM/PATH lookup remains a source-build fallback, not the primary install path.",
    ];
    let mut strategy = "system-fallback";
    if let Some(toolchain) = bundled_toolchain {
        bundled = true;
        strategy = "bundled-first";
        source = serde_json::json!({"label": toolchain.source_label, "version": toolchain.version});
        provenance = toolchain_provenance_json(toolchain, archive_target, "sdk-runtime-subset");
        if let Some(records) = records {
            components = records.clone();
            notes = vec![
                "The SDK bundles the minimal host LLVM/Clang runtime needed by installed Kern tools.",
                "The full LLVM development prefix is intentionally not part of the end-user SDK.",
                "Clone the repository and configure the host environment directly for source builds.",
            ];
        }
        required_components = sdk_runtime_required_components(archive_target);
        health_checks = toolchain_component_health_checks_json(&required_components);
    }
    serde_json::json!({
        "schema_version": 1,
        "sdk_version": version,
        "host_target": archive_target,
        "layout_version": 1,
        "binaries": HOST_TOOL_BINARIES,
        "libraries": OFFICIAL_LIBRARY_LAYERS,
        "toolchain": {
            "layout": layout,
            "bundled": bundled,
            "strategy": strategy,
            "resolver_order": [
                "explicit-toolchain-root",
                "sdk-relative-toolchain",
                "environment-overrides",
                "system-path"
            ],
            "source": source,
            "provenance": provenance,
            "required_components": required_components,
            "health_checks": health_checks,
            "components": components,
            "notes": notes,
        }
    })
}

pub fn toolchain_manifest_json(
    version: &str,
    archive_target: &str,
    bundled_toolchain: &BundledToolchain,
    records: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    let required = full_toolchain_required_components(archive_target);
    serde_json::json!({
        "schema_version": 1,
        "toolchain_version": version,
        "host_target": archive_target,
        "layout_version": 1,
        "provider": "bundled-host-llvm",
        "layout": toolchain_layout_paths_json(bundled_toolchain),
        "source": {
            "label": bundled_toolchain.source_label,
            "version": bundled_toolchain.version,
        },
        "provenance": toolchain_provenance_json(
            bundled_toolchain,
            archive_target,
            "standalone-development-prefix"
        ),
        "required_components": required,
        "health_checks": toolchain_component_health_checks_json(&full_toolchain_required_components(archive_target)),
        "components": records,
        "notes": [
            "This archive contains the controlled host LLVM/Clang toolchain used by Kern packaging.",
            "It is intended for CI, release engineering, and SDK assembly.",
            "The archive preserves a relocatable LLVM development prefix for source builds.",
            "Host OS SDK/libc components may still remain platform responsibilities."
        ]
    })
}

pub fn sha256_file(path: &Path) -> OpsResult<String> {
    if !path.is_file() {
        return Err(OpsError::new(format!(
            "cannot compute sha256 for non-file `{}`",
            path.display()
        )));
    }
    if cfg!(windows) {
        let script = format!(
            "(Get-FileHash -Algorithm SHA256 -LiteralPath {}).Hash.ToLowerInvariant()",
            powershell_quote(&path.display().to_string())
        );
        let result = run_command_capture(
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
        if result.status_code == Some(0) {
            return Ok(result.stdout.trim().to_ascii_lowercase());
        }
    } else {
        for command in ["sha256sum", "shasum"] {
            let args = if command == "shasum" {
                vec![
                    OsString::from(command),
                    OsString::from("-a"),
                    OsString::from("256"),
                    path.as_os_str().to_owned(),
                ]
            } else {
                vec![OsString::from(command), path.as_os_str().to_owned()]
            };
            let Ok(result) = run_command_capture(&args, None) else {
                continue;
            };
            if result.status_code == Some(0)
                && let Some(hash) = result.stdout.split_whitespace().next()
            {
                return Ok(hash.to_ascii_lowercase());
            }
        }
    }
    Err(OpsError::new(format!(
        "failed to compute sha256 for `{}`",
        path.display()
    )))
}

pub fn sha256_directory(path: &Path) -> OpsResult<String> {
    if !path.is_dir() {
        return Err(OpsError::new(format!(
            "directory `{}` does not exist",
            path.display()
        )));
    }
    let mut files = Vec::new();
    collect_files(path, &mut files)?;
    files.sort();
    let mut payload = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(path)
            .map_err(|err| OpsError::new(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        payload.extend_from_slice(relative.as_bytes());
        payload.push(0);
        payload.extend_from_slice(sha256_file(&file)?.as_bytes());
        payload.push(0);
    }
    let temp = make_temp_dir("kern-sha256-dir-")?.join("payload");
    fs::write(&temp, payload)?;
    let digest = sha256_file(&temp);
    if let Some(parent) = temp.parent() {
        let _ = remove_path_if_exists(parent);
    }
    digest
}

pub fn file_size(path: &Path) -> OpsResult<u64> {
    Ok(path
        .metadata()
        .map_err(|err| OpsError::io(path, "read metadata for", err))?
        .len())
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
                    "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
                    powershell_quote(&archive_path.display().to_string()),
                    powershell_quote(&extract_root.display().to_string())
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

pub fn resolve_ci_toolchain_policy(
    manifest_path: &Path,
    runner_os: &str,
    mode: &str,
    host_target: Option<&str>,
) -> OpsResult<CiToolchainPolicy> {
    let manifest = read_json_value(manifest_path)?;
    if manifest
        .get("schema_version")
        .and_then(|value| value.as_u64())
        != Some(1)
    {
        return Err(OpsError::new(format!(
            "unsupported CI toolchain manifest schema in `{}`",
            manifest_path.display()
        )));
    }
    let normalized_runner = normalize_runner_os(runner_os)?;
    let normalized_mode = normalize_policy_mode(mode)?;
    let toolchains = manifest
        .get("toolchains")
        .and_then(|value| value.as_object())
        .ok_or_else(|| OpsError::new("invalid CI toolchain manifest: missing `toolchains`"))?;
    let base = toolchains
        .get(&normalized_runner)
        .and_then(|value| value.as_object())
        .ok_or_else(|| {
            OpsError::new(format!(
                "missing CI toolchain policy for `{normalized_runner}`"
            ))
        })?;
    let mut effective = base.clone();
    if let Some(host_target) = host_target.filter(|value| !value.is_empty()) {
        let host_targets = base
            .get("host_targets")
            .and_then(|value| value.as_object())
            .ok_or_else(|| OpsError::new("missing CI toolchain host target overrides"))?;
        let override_policy = host_targets
            .get(host_target)
            .and_then(|value| value.as_object())
            .ok_or_else(|| {
                OpsError::new(format!("missing CI toolchain host_target `{host_target}`"))
            })?;
        effective.extend(override_policy.clone());
    }
    if normalized_mode != "current" {
        let prefix = format!("{normalized_mode}_");
        let overrides = effective
            .iter()
            .filter_map(|(key, value)| {
                key.strip_prefix(&prefix)
                    .map(|stripped| (stripped.to_string(), value.clone()))
            })
            .collect::<Vec<_>>();
        effective.extend(overrides);
    }

    Ok(CiToolchainPolicy {
        runner_os: normalized_runner,
        mode: normalized_mode,
        host_target: host_target
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        llvm_version: json_string(&effective, "llvm_version")?,
        llvm_major: json_u64(&effective, "llvm_major")?,
        prefix_env: json_string(&effective, "prefix_env")?,
        provider_kind: json_string(&effective, "provider_kind")?,
        provider: json_string(&effective, "provider")?,
        target_provider_kind: json_string(&effective, "target_provider_kind")?,
        target_provider: json_string(&effective, "target_provider")?,
        required_tools: json_string_array(&effective, "required_tools")?,
        raw: effective,
    })
}

pub fn runner_os_for_host(host: &HostTarget) -> &'static str {
    if host.archive_target.ends_with("linux-gnu") {
        "Linux"
    } else if host.archive_target.ends_with("apple-darwin") {
        "macOS"
    } else {
        "Windows"
    }
}

pub fn runner_os_for_target(target: &str) -> OpsResult<&'static str> {
    if target.ends_with("linux-gnu") {
        Ok("Linux")
    } else if target.ends_with("apple-darwin") {
        Ok("macOS")
    } else if target.ends_with("windows-msvc") {
        Ok("Windows")
    } else {
        Err(OpsError::new(format!(
            "unsupported archive target `{target}`"
        )))
    }
}

pub fn format_policy_value(policy: &CiToolchainPolicy, value: &str) -> String {
    value
        .replace("{llvm_version}", &policy.llvm_version)
        .replace("{host_target}", policy.host_target.as_deref().unwrap_or(""))
}

pub fn expected_archive_sha256(policy: &CiToolchainPolicy) -> OpsResult<Option<String>> {
    if let Some(value) = policy
        .raw
        .get("archive_sha256")
        .and_then(|value| value.as_str())
        && !value.is_empty()
    {
        return Ok(Some(value.to_ascii_lowercase()));
    }
    let Some(url) = policy
        .raw
        .get("archive_sha256_url")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let temp = make_temp_dir("kern-sha256-download-")?;
    let path = temp.join("archive.sha256");
    let result = (|| -> OpsResult<Option<String>> {
        download_file(&format_policy_value(policy, url), &path)?;
        let source = fs::read_to_string(&path)?;
        let checksum = source
            .split_whitespace()
            .next()
            .ok_or_else(|| OpsError::new("archive checksum file is empty"))?;
        if checksum.len() != 64 || !checksum.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(OpsError::new(
                "archive checksum file does not start with a sha256 digest",
            ));
        }
        Ok(Some(checksum.to_ascii_lowercase()))
    })();
    let _ = remove_path_if_exists(&temp);
    result
}

pub fn verify_archive_checksum(path: &Path, expected: Option<&str>) -> OpsResult<String> {
    let Some(expected) = expected else {
        return Ok(format!(
            "toolchain archive verification skipped for `{}`; no archive_sha256 is pinned yet",
            path.display()
        ));
    };
    let actual = sha256_file(path)?;
    if actual != expected.to_ascii_lowercase() {
        return Err(OpsError::new(format!(
            "archive checksum mismatch for `{}`: expected {}, got {}",
            path.display(),
            expected,
            actual
        )));
    }
    Ok(format!(
        "toolchain archive checksum verified for `{}`",
        path.display()
    ))
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
        fs::remove_dir_all(path).map_err(|err| OpsError::io(path, "remove directory", err))?;
    } else {
        fs::remove_file(path).map_err(|err| OpsError::io(path, "remove file", err))?;
    }
    Ok(())
}

pub fn copy_path(source: &Path, dest: &Path) -> OpsResult<()> {
    if source.is_dir() {
        copy_dir_recursive(source, dest)
    } else {
        if dest.exists() {
            let source_canonical = source.canonicalize().ok();
            let dest_canonical = dest.canonicalize().ok();
            if source_canonical.is_some() && source_canonical == dest_canonical {
                return Ok(());
            }
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| OpsError::io(parent, "create directory", err))?;
        }
        fs::copy(source, dest).map_err(|err| {
            OpsError::new(format!(
                "failed to copy `{}` to `{}`: {err}",
                source.display(),
                dest.display()
            ))
        })?;
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
    fs::create_dir_all(dest).map_err(|err| OpsError::io(dest, "create directory", err))?;
    for entry in fs::read_dir(source).map_err(|err| OpsError::io(source, "read directory", err))? {
        let entry = entry.map_err(|err| OpsError::io(source, "read directory entry in", err))?;
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

fn configure_unix_path(install_bin: &Path) -> OpsResult<()> {
    let rc_file = select_unix_rc_file()?;
    if let Some(parent) = rc_file.parent() {
        fs::create_dir_all(parent)?;
    }
    if !rc_file.exists() {
        fs::File::create(&rc_file)?;
    }
    let marker = install_bin.to_string_lossy();
    let contents = fs::read_to_string(&rc_file)?;
    if contents.contains(marker.as_ref()) {
        println!("{} is already in your PATH.", install_bin.display());
        return Ok(());
    }
    let mut file = fs::OpenOptions::new().append(true).open(&rc_file)?;
    writeln!(file)?;
    writeln!(file, "# Kern Programming Language")?;
    writeln!(file, "export PATH=\"{}:$PATH\"", install_bin.display())?;
    println!(
        "Added {} to your PATH in {}.",
        install_bin.display(),
        rc_file.display()
    );
    Ok(())
}

fn configure_windows_path(install_bin: &Path) -> OpsResult<()> {
    let path = install_bin.display().to_string();
    let script = format!(
        "$p=[Environment]::GetEnvironmentVariable('Path','User'); if (!$p) {{ $p='' }}; if (($p -split ';') -notcontains {}) {{ [Environment]::SetEnvironmentVariable('Path', ($(if ($p) {{ \"$p;{}\" }} else {{ {} }})), 'User') }}",
        powershell_quote(&path),
        path.replace('`', "``").replace('"', "`\""),
        powershell_quote(&path)
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
    println!("Added {} to your user PATH.", install_bin.display());
    Ok(())
}

fn select_unix_rc_file() -> OpsResult<PathBuf> {
    let shell = env::var_os("SHELL")
        .and_then(|value| PathBuf::from(value).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.into_string().ok())
        .unwrap_or_default();
    let home = home_dir()?;
    Ok(match shell.as_str() {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bashrc"),
        _ => home.join(".profile"),
    })
}

fn sdk_runtime_required_components(target: &str) -> Vec<String> {
    if target.ends_with("windows-msvc") {
        vec!["clang".into(), "lld".into(), "llvm_lib".into()]
    } else {
        vec!["clang".into(), "lld".into()]
    }
}

fn full_toolchain_required_components(target: &str) -> Vec<String> {
    let mut components = vec![
        "clang".into(),
        "clangxx".into(),
        "lld".into(),
        "llvm_ar".into(),
        "llvm_config".into(),
        "lib_dir".into(),
        "include_dir".into(),
    ];
    if target.ends_with("windows-msvc") {
        components.push("llvm_lib".into());
    }
    components
}

#[derive(Debug)]
struct HealthCheck {
    component: String,
    kind: String,
}

fn manifest_required_components(
    manifest: &serde_json::Map<String, serde_json::Value>,
    fallback: Vec<String>,
    label: &str,
) -> OpsResult<Vec<String>> {
    let Some(required) = manifest.get("required_components") else {
        return Ok(fallback);
    };
    let Some(items) = required.as_array() else {
        return Err(OpsError::new(format!(
            "`{label}.required_components` is invalid"
        )));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| OpsError::new(format!("`{label}.required_components` is invalid")))
        })
        .collect()
}

fn manifest_health_checks(
    manifest: &serde_json::Map<String, serde_json::Value>,
    fallback_components: &[String],
    label: &str,
) -> OpsResult<Vec<HealthCheck>> {
    let Some(checks) = manifest.get("health_checks") else {
        return Ok(fallback_components
            .iter()
            .map(|component| HealthCheck {
                component: component.clone(),
                kind: if component == "llvm_lib" {
                    "creates-empty-library".into()
                } else if component.ends_with("_dir") {
                    "exists".into()
                } else {
                    "starts-with-version".into()
                },
            })
            .collect());
    };
    let Some(items) = checks.as_array() else {
        return Err(OpsError::new(format!("`{label}.health_checks` is invalid")));
    };
    items
        .iter()
        .map(|item| {
            let Some(obj) = item.as_object() else {
                return Err(OpsError::new(format!("`{label}.health_checks` is invalid")));
            };
            Ok(HealthCheck {
                component: json_string(obj, "component")?,
                kind: json_string(obj, "kind")?,
            })
        })
        .collect()
}

fn validate_component_record(
    root: &Path,
    component: &str,
    entry: &serde_json::Value,
    label: &str,
) -> OpsResult<()> {
    let Some(obj) = entry.as_object() else {
        return Err(OpsError::new(format!("{label} `{component}` is invalid")));
    };
    let relative_path = json_string(obj, "path")?;
    let kind = obj
        .get("kind")
        .and_then(|value| value.as_str())
        .unwrap_or("file");
    let target = root.join(&relative_path);
    if kind == "directory" {
        if !target.is_dir() {
            return Err(OpsError::new(format!(
                "{label} `{component}` is missing at `{}`",
                target.display()
            )));
        }
        if let Some(expected) = obj.get("sha256").and_then(|value| value.as_str())
            && !expected.is_empty()
        {
            let actual = sha256_directory(&target)?;
            if actual != expected.to_ascii_lowercase() {
                return Err(OpsError::new(format!(
                    "{label} `{component}` checksum mismatch at `{}`",
                    target.display()
                )));
            }
        }
        return Ok(());
    }
    if !target.is_file() {
        return Err(OpsError::new(format!(
            "{label} `{component}` is missing at `{}`",
            target.display()
        )));
    }
    if let Some(expected) = obj.get("size").and_then(|value| value.as_u64())
        && file_size(&target)? != expected
    {
        return Err(OpsError::new(format!(
            "{label} `{component}` size mismatch at `{}`",
            target.display()
        )));
    }
    if let Some(expected) = obj.get("sha256").and_then(|value| value.as_str())
        && !expected.is_empty()
    {
        let actual = sha256_file(&target)?;
        if actual != expected.to_ascii_lowercase() {
            return Err(OpsError::new(format!(
                "{label} `{component}` checksum mismatch at `{}`",
                target.display()
            )));
        }
    }
    Ok(())
}

fn validate_manifest_health_check(
    root: &Path,
    component: &str,
    entry: &serde_json::Value,
    kind: &str,
) -> OpsResult<()> {
    if kind == "exists" {
        return validate_component_record(root, component, entry, "toolchain component");
    }
    let path = entry
        .as_object()
        .and_then(|obj| obj.get("path"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| OpsError::new(format!("toolchain component `{component}` has no path")))?;
    let target = root.join(path);
    if kind == "starts-with-version" {
        validate_component_record(root, component, entry, "toolchain component")?;
        let result = run_command_capture(
            &[target.as_os_str().to_owned(), OsString::from("--version")],
            None,
        )?;
        if result.status_code == Some(0) {
            return Ok(());
        }
        return Err(OpsError::new(format!(
            "toolchain component `{component}` did not answer `--version`: {}{}",
            result.stdout, result.stderr
        )));
    }
    if kind == "creates-empty-library" {
        validate_component_record(root, component, entry, "toolchain component")?;
        let temp = make_temp_dir("kern-llvm-lib-probe-")?;
        let probe = temp.join("empty.lib");
        let result = run_command_capture(
            &[
                target.as_os_str().to_owned(),
                OsString::from("/llvmlibempty"),
                OsString::from(format!("/out:{}", probe.display())),
            ],
            Some(&temp),
        );
        let _ = remove_path_if_exists(&temp);
        let result = result?;
        if result.status_code == Some(0) {
            return Ok(());
        }
        return Err(OpsError::new(format!(
            "toolchain component `{component}` failed empty library probe: {}{}",
            result.stdout, result.stderr
        )));
    }
    Err(OpsError::new(format!(
        "toolchain manifest component `{component}` has unsupported health check `{kind}`"
    )))
}

fn normalize_runner_os(value: &str) -> OpsResult<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "linux" => Ok("Linux".into()),
        "macos" | "macosx" | "darwin" => Ok("macOS".into()),
        "windows" | "win32" => Ok("Windows".into()),
        other => Err(OpsError::new(format!("unsupported runner OS `{other}`"))),
    }
}

fn normalize_policy_mode(value: &str) -> OpsResult<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "current" | "bootstrap" | "target" => Ok(value.trim().to_ascii_lowercase()),
        other => Err(OpsError::new(format!(
            "unsupported CI toolchain mode `{other}`"
        ))),
    }
}

fn json_string(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> OpsResult<String> {
    obj.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| OpsError::new(format!("missing or invalid `{key}`")))
}

fn json_u64(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> OpsResult<u64> {
    obj.get(key)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| OpsError::new(format!("missing or invalid `{key}`")))
}

fn json_string_array(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> OpsResult<Vec<String>> {
    let Some(items) = obj.get(key).and_then(|value| value.as_array()) else {
        return Err(OpsError::new(format!("missing or invalid `{key}`")));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| OpsError::new(format!("missing or invalid `{key}`")))
        })
        .collect()
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> OpsResult<()> {
    for entry in fs::read_dir(root).map_err(|err| OpsError::io(root, "read directory", err))? {
        let entry = entry.map_err(|err| OpsError::io(root, "read directory entry in", err))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn find_llvm_sys_prefix() -> Option<(String, PathBuf)> {
    let mut matches = env::vars_os()
        .filter_map(|(key, value)| {
            let key_string = key.into_string().ok()?;
            if key_string.starts_with("LLVM_SYS_")
                && key_string.ends_with("_PREFIX")
                && !value.is_empty()
            {
                Some((key_string, PathBuf::from(value)))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches.into_iter().find(|(_, path)| path.is_dir())
}

fn find_program(names: &[&str]) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            if cfg!(windows) {
                let candidate = dir.join(format!("{name}.exe"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn tool_output(cmd: &[OsString]) -> OpsResult<String> {
    let result = run_command_capture(cmd, None)?;
    if result.status_code != Some(0) {
        return Err(OpsError::new(format!(
            "command failed: {}{}",
            result.stdout, result.stderr
        )));
    }
    let output = result.stdout.trim();
    if !output.is_empty() {
        return Ok(output.to_string());
    }
    let output = result.stderr.trim();
    if !output.is_empty() {
        return Ok(output.to_string());
    }
    Err(OpsError::new("command produced no output"))
}

fn resolve_llvm_tool(
    name: &str,
    major: &str,
    bindir: &Path,
    host: &HostTarget,
    required: bool,
    allow_path_lookup: bool,
) -> OpsResult<Option<PathBuf>> {
    let suffix = if host.is_windows { ".exe" } else { "" };
    let candidates = [format!("{name}{suffix}"), format!("{name}-{major}{suffix}")];
    for candidate in &candidates {
        let path = bindir.join(candidate);
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    if allow_path_lookup {
        let candidate_refs = candidates.iter().map(String::as_str).collect::<Vec<_>>();
        if let Some(path) = find_program(&candidate_refs) {
            return Ok(Some(path));
        }
    }
    if required {
        return Err(OpsError::new(format!(
            "failed to resolve LLVM tool `{name}` for the current packaging environment"
        )));
    }
    Ok(None)
}

fn toolchain_component_health_checks_json(components: &[String]) -> Vec<serde_json::Value> {
    components
        .iter()
        .map(|component| {
            let kind = if component == "llvm_lib" {
                "creates-empty-library"
            } else if component.ends_with("_dir") {
                "exists"
            } else {
                "starts-with-version"
            };
            serde_json::json!({"component": component, "kind": kind})
        })
        .collect()
}

fn toolchain_provenance_json(
    bundled_toolchain: &BundledToolchain,
    archive_target: &str,
    package_role: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "resolved-host-llvm",
        "package_role": package_role,
        "host_target": archive_target,
        "source": {
            "label": bundled_toolchain.source_label,
            "version": bundled_toolchain.version,
        }
    })
}

fn toolchain_layout_paths_json(bundled_toolchain: &BundledToolchain) -> serde_json::Value {
    serde_json::json!({
        "root": "toolchain",
        "host_root": "toolchain/host",
        "bin_dir": bundled_component_path(bundled_toolchain, &bundled_toolchain.bindir).unwrap_or_else(|_| "toolchain/host/bin".into()),
        "lib_dir": bundled_component_path(bundled_toolchain, &bundled_toolchain.libdir).unwrap_or_else(|_| "toolchain/host/lib".into()),
        "include_dir": bundled_component_path(bundled_toolchain, &bundled_toolchain.includedir).unwrap_or_else(|_| "toolchain/host/include".into()),
        "sysroot_dir": "toolchain/host/sysroot",
    })
}

pub fn bundled_component_path(
    bundled_toolchain: &BundledToolchain,
    path: &Path,
) -> OpsResult<String> {
    let relative = path.strip_prefix(&bundled_toolchain.prefix).map_err(|_| {
        OpsError::new(format!(
            "toolchain path `{}` does not live under prefix `{}`",
            path.display(),
            bundled_toolchain.prefix.display()
        ))
    })?;
    Ok(format!(
        "toolchain/host/{}",
        relative.to_string_lossy().replace('\\', "/")
    ))
}

pub fn artifact_record_json(record: &ArtifactRecord) -> serde_json::Value {
    serde_json::json!({
        "path": record.path,
        "kind": record.kind,
        "sha256": record.sha256,
        "size": record.size,
    })
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

    #[test]
    fn copy_path_allows_copying_file_to_itself() {
        let root = make_temp_dir("shared-ops-copy-self-test-").unwrap();
        let file = root.join("artifact.txt");
        fs::write(&file, "contents").unwrap();

        copy_path(&file, &file).unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "contents");
        remove_path_if_exists(&root).unwrap();
    }
}
