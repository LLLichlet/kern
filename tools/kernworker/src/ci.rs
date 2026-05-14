use crate::args::{
    ActivateToolchainArgs, PackagedToolchainInstallArgs, PackagedToolchainVerifyArgs, TestMode,
    ToolchainArchiveArgs, ToolchainSpecArgs, VsixVerifyArgs,
};
use shared_ops::{
    OpsError, OpsResult, archive_kind_from_path, copy_dir_recursive, copy_path, detect_host_target,
    expected_archive_sha256, extract_archive_with_system_tool, first_non_empty_line,
    format_policy_value, load_workspace_version, make_temp_dir, remove_path_if_exists, repo_root,
    resolve_bundled_toolchain, resolve_ci_toolchain_policy, run_command, run_command_capture,
    runner_os_for_host, runner_os_for_target, validate_toolchain_root, verify_archive_checksum,
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
const HOSTED_TESTS: &[&str] = &["collections"];
pub fn run_kernc_tests(mode: TestMode) -> OpsResult<()> {
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

pub fn run_craft_policy_checks() -> OpsResult<()> {
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

fn ci_toolchains_manifest() -> OpsResult<PathBuf> {
    Ok(repo_root()?.join("manifest").join("ci-toolchains.json"))
}

fn resolve_policy_from_args(
    runner_os: Option<&str>,
    mode: &str,
    host_target: Option<&str>,
) -> OpsResult<shared_ops::CiToolchainPolicy> {
    let host = detect_host_target()?;
    let runner = runner_os.unwrap_or_else(|| runner_os_for_host(&host));
    resolve_ci_toolchain_policy(&ci_toolchains_manifest()?, runner, mode, host_target)
}

pub fn activate_toolchain(args: ActivateToolchainArgs) -> OpsResult<()> {
    let prefix = args
        .prefix
        .or_else(find_active_toolchain_prefix)
        .ok_or_else(|| {
            OpsError::new(
                "failed to resolve active LLVM toolchain prefix; pass `--prefix` or set KERN_TOOLCHAIN_ROOT/LLVM_SYS_*_PREFIX",
            )
        })?;
    if !prefix.is_dir() {
        return Err(OpsError::new(format!(
            "toolchain prefix `{}` is not a directory",
            prefix.display()
        )));
    }
    let prefix = canonical_or_original(&prefix);
    if args.format == "github-env" {
        println!("KERN_TOOLCHAIN_ROOT={}", prefix.display());
        println!("KERN_DEBUG_LTO_LINK=1");
    } else {
        println!("toolchain.root: {}", prefix.display());
        println!("toolchain.debug_lto_link: 1");
    }
    Ok(())
}

pub fn print_toolchain_info() -> OpsResult<()> {
    let host = detect_host_target()?;
    println!("runner_target: {}", host.archive_target);
    let kern_toolchain_root = env::var("KERN_TOOLCHAIN_ROOT").ok();
    println!(
        "KERN_TOOLCHAIN_ROOT: {}",
        kern_toolchain_root.as_deref().unwrap_or("<unset>")
    );
    println!(
        "CC: {}",
        env::var("CC").unwrap_or_else(|_| "<unset>".into())
    );
    println!(
        "CXX: {}",
        env::var("CXX").unwrap_or_else(|_| "<unset>".into())
    );
    print_configured_toolchain_roots(kern_toolchain_root.as_deref());
    for name in [
        "cc",
        "clang",
        "clang++",
        "ld",
        "ld.lld",
        "ld64.lld",
        "lld-link",
        "llvm-lib",
        "llvm-config",
        "llvm-config-21",
    ] {
        let result = run_command_capture(
            &[
                OsString::from(if cfg!(windows) { "where" } else { "which" }),
                OsString::from(name),
            ],
            None,
        );
        match result {
            Ok(result) if result.status_code == Some(0) => {
                let path = result.stdout.lines().next().unwrap_or("").trim();
                println!("{name}: {path}");
                println!("{name} --version: {}", tool_version_line(Path::new(path)));
            }
            _ => println!("{name}: <missing>"),
        }
    }
    match resolve_bundled_toolchain(&host, None) {
        Ok(toolchain) => {
            println!("resolved_toolchain.prefix: {}", toolchain.prefix.display());
            println!("resolved_toolchain.bindir: {}", toolchain.bindir.display());
            println!("resolved_toolchain.libdir: {}", toolchain.libdir.display());
            println!(
                "resolved_toolchain.includedir: {}",
                toolchain.includedir.display()
            );
            println!("resolved_toolchain.version: {}", toolchain.version);
            for (name, path) in &toolchain.tools {
                if let Some(path) = path.as_str() {
                    println!("resolved_toolchain.tool.{name}: {path}");
                }
            }
            if let Some(resource_dir) = toolchain.resource_dir {
                println!(
                    "resolved_toolchain.resource_dir: {}",
                    resource_dir.display()
                );
            }
            if let Some(sysroot_dir) = toolchain.sysroot_dir {
                println!("resolved_toolchain.sysroot_dir: {}", sysroot_dir.display());
            }
        }
        Err(err) => println!("resolved_toolchain: <unavailable> ({err})"),
    }
    Ok(())
}

pub fn print_toolchain_spec(args: ToolchainSpecArgs) -> OpsResult<()> {
    let policy = resolve_policy_from_args(
        args.runner_os.as_deref(),
        if args.mode.is_empty() {
            "current"
        } else {
            &args.mode
        },
        args.host_target.as_deref(),
    )?;
    if args.format == "github-env" {
        print!("{}", render_policy_github_env(&policy)?);
        return Ok(());
    }
    println!("toolchain_policy.runner_os: {}", policy.runner_os);
    println!("toolchain_policy.mode: {}", policy.mode);
    println!(
        "toolchain_policy.host_target: {}",
        policy.host_target.as_deref().unwrap_or("<unset>")
    );
    println!("toolchain_policy.provider_kind: {}", policy.provider_kind);
    println!("toolchain_policy.provider: {}", policy.provider);
    println!(
        "toolchain_policy.target_provider_kind: {}",
        policy.target_provider_kind
    );
    println!(
        "toolchain_policy.target_provider: {}",
        policy.target_provider
    );
    println!("toolchain_policy.llvm_version: {}", policy.llvm_version);
    println!("toolchain_policy.llvm_major: {}", policy.llvm_major);
    println!("toolchain_policy.prefix_env: {}", policy.prefix_env);
    println!(
        "toolchain_policy.required_tools: {}",
        policy.required_tools.join(" ")
    );
    for key in policy.raw.keys() {
        if [
            "llvm_version",
            "llvm_major",
            "prefix_env",
            "provider_kind",
            "provider",
            "target_provider_kind",
            "target_provider",
            "required_tools",
        ]
        .contains(&key.as_str())
        {
            continue;
        }
        println!("toolchain_policy.{key}: {}", policy.raw[key]);
    }
    Ok(())
}

fn render_policy_github_env(policy: &shared_ops::CiToolchainPolicy) -> OpsResult<String> {
    let mut lines = vec![
        format!("KERN_CI_RUNNER_OS={}", policy.runner_os),
        format!("KERN_CI_MODE={}", policy.mode),
        format!(
            "KERN_CI_HOST_TARGET={}",
            policy.host_target.as_deref().unwrap_or("")
        ),
        format!("KERN_CI_LLVM_VERSION={}", policy.llvm_version),
        format!("KERN_CI_LLVM_MAJOR={}", policy.llvm_major),
        format!("KERN_CI_LLVM_PREFIX_ENV={}", policy.prefix_env),
        format!("KERN_CI_PROVIDER_KIND={}", policy.provider_kind),
        format!("KERN_CI_TOOLCHAIN_PROVIDER={}", policy.provider),
        format!(
            "KERN_CI_TARGET_PROVIDER_KIND={}",
            policy.target_provider_kind
        ),
        format!(
            "KERN_CI_TARGET_TOOLCHAIN_PROVIDER={}",
            policy.target_provider
        ),
        format!("KERN_CI_REQUIRED_TOOLS={}", policy.required_tools.join(" ")),
    ];
    for (key, env_name) in [
        ("archive_url", "KERN_CI_ARCHIVE_URL"),
        ("archive_sha256", "KERN_CI_ARCHIVE_SHA256"),
        ("archive_root", "KERN_CI_ARCHIVE_ROOT"),
        ("install_dir", "KERN_CI_INSTALL_DIR"),
        ("archive_prefix_subdir", "KERN_CI_ARCHIVE_PREFIX_SUBDIR"),
        ("apt_packages", "KERN_CI_APT_PACKAGES"),
        ("primary_formula", "KERN_CI_BREW_PRIMARY_FORMULA"),
        ("fallback_formula", "KERN_CI_BREW_FALLBACK_FORMULA"),
        ("vcpkg_package", "KERN_CI_WINDOWS_VCPKG_PACKAGE"),
        ("vcpkg_cache_key", "KERN_CI_WINDOWS_VCPKG_CACHE_KEY"),
    ] {
        if let Some(value) = policy.raw.get(key) {
            let rendered = if let Some(text) = value.as_str() {
                format_policy_value(policy, text)
            } else if let Some(items) = value.as_array() {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                continue;
            };
            lines.push(format!("{env_name}={rendered}"));
        }
    }
    if let Some(items) = policy
        .raw
        .get("extra_formulas")
        .and_then(|value| value.as_array())
    {
        lines.push(format!(
            "KERN_CI_BREW_EXTRA_FORMULAS={}",
            items
                .iter()
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    Ok(lines.join("\n") + "\n")
}

pub fn verify_toolchain_archive(args: ToolchainArchiveArgs) -> OpsResult<()> {
    let archive = args
        .archive_path
        .ok_or_else(|| OpsError::new("`--archive-path` is required"))?;
    let policy = resolve_policy_from_args(
        args.runner_os.as_deref(),
        if args.mode.is_empty() {
            "current"
        } else {
            &args.mode
        },
        args.host_target.as_deref(),
    )?;
    if policy.provider_kind != "archive" {
        return Err(OpsError::new(format!(
            "runner OS `{}` does not use an archive-based {} provider",
            policy.runner_os, policy.mode
        )));
    }
    let expected = expected_archive_sha256(&policy)?;
    println!(
        "{}",
        verify_archive_checksum(&archive, expected.as_deref())?
    );
    Ok(())
}

pub fn verify_packaged_toolchain(args: PackagedToolchainVerifyArgs) -> OpsResult<()> {
    let archive = args
        .archive_path
        .ok_or_else(|| OpsError::new("`--archive-path` is required"))?;
    let host = detect_host_target()?;
    let target = args.target.unwrap_or(host.archive_target);
    let temp = make_temp_dir("kern-toolchain-verify-")?;
    let result = (|| -> OpsResult<()> {
        let root = extract_archive_with_system_tool(
            &archive,
            &temp.join("extract"),
            archive_kind_from_path(&archive)?,
        )?;
        validate_toolchain_root(&root, &target)?;
        println!("packaged toolchain archive verified: {}", archive.display());
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp);
    result
}

pub fn install_packaged_toolchain(args: PackagedToolchainInstallArgs) -> OpsResult<()> {
    let archive = args
        .archive_path
        .ok_or_else(|| OpsError::new("`--archive-path` is required"))?;
    let dest = args
        .dest
        .ok_or_else(|| OpsError::new("`--dest` is required"))?;
    let host = detect_host_target()?;
    let target = args.target.unwrap_or(host.archive_target);
    let temp = make_temp_dir("kern-toolchain-install-")?;
    let result = (|| -> OpsResult<()> {
        let root = extract_archive_with_system_tool(
            &archive,
            &temp.join("extract"),
            archive_kind_from_path(&archive)?,
        )?;
        validate_toolchain_root(&root, &target)?;
        remove_path_if_exists(&dest)?;
        copy_path(&root, &dest)?;
        let prefix = dest.join("toolchain").join("host");
        let policy = resolve_ci_toolchain_policy(
            &ci_toolchains_manifest()?,
            runner_os_for_target(&target)?,
            "current",
            Some(&target),
        )?;
        if args.format == "github-env" {
            println!("KERN_CI_PACKAGED_TOOLCHAIN_ROOT={}", prefix.display());
            println!("KERN_TOOLCHAIN_ROOT={}", prefix.display());
            println!("{}={}", policy.prefix_env, prefix.display());
        } else {
            println!("packaged_toolchain.install_root: {}", dest.display());
            println!("packaged_toolchain.prefix: {}", prefix.display());
        }
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp);
    result
}

pub fn verify_vscode_extension_archive(args: VsixVerifyArgs) -> OpsResult<()> {
    let package_json = args
        .package_json
        .unwrap_or_else(|| PathBuf::from("package.json"));
    let package = serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&package_json)?)?;
    let version = package
        .get("version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| OpsError::new("VS Code package.json has no string `version`"))?;
    let vsix_path = args
        .vsix_path
        .unwrap_or_else(|| PathBuf::from(format!("kern-vscode-{version}-linux-x64.vsix")));
    if !vsix_path.is_file() {
        return Err(OpsError::new(format!(
            "VSIX archive `{}` is missing",
            vsix_path.display()
        )));
    }
    let result = run_command_capture(
        &[
            OsString::from("unzip"),
            OsString::from("-Z1"),
            vsix_path.as_os_str().to_owned(),
        ],
        None,
    )?;
    if result.status_code != Some(0) {
        return Err(OpsError::new(format!(
            "failed to inspect VSIX archive `{}`: {}{}",
            vsix_path.display(),
            result.stdout,
            result.stderr
        )));
    }
    let names = result.stdout.lines().collect::<Vec<_>>();
    for required in ["extension/extension.js", "extension/out/extension.js"] {
        if !names.contains(&required) {
            return Err(OpsError::new(format!(
                "missing VSIX entry `{required}` in `{}`",
                vsix_path.display()
            )));
        }
    }
    if let Some(entry) = names
        .iter()
        .find(|name| name.starts_with("extension/server/"))
    {
        return Err(OpsError::new(format!(
            "unexpected embedded server entry in VSIX: `{entry}`"
        )));
    }
    if let Some(entry) = names.iter().find(|name| name.contains("node_modules/")) {
        return Err(OpsError::new(format!(
            "unexpected node_modules in VSIX: `{entry}`"
        )));
    }
    println!("VSIX archive verified: {}", vsix_path.display());
    Ok(())
}
pub fn assert_toolchain_health() -> OpsResult<()> {
    let host = detect_host_target()?;
    let policy = resolve_ci_toolchain_policy(
        &ci_toolchains_manifest()?,
        runner_os_for_host(&host),
        "current",
        Some(&host.archive_target),
    )?;
    println!("toolchain_health.target: {}", host.archive_target);
    let toolchain = resolve_bundled_toolchain(&host, None).map_err(|err| {
        OpsError::new(format!(
            "release-grade host toolchain is incomplete for {}; expected a controlled LLVM toolchain with `{}`: {err}",
            host.archive_target,
            policy.required_tools.join("`, `")
        ))
    })?;
    println!("toolchain_health.source: {}", toolchain.source_label);
    println!("toolchain_health.version: {}", toolchain.version);
    for tool in &policy.required_tools {
        let component = component_key_from_tool(tool)?;
        let Some(path) = toolchain
            .tools
            .get(component)
            .and_then(|value| value.as_str())
        else {
            return Err(OpsError::new(format!(
                "resolved toolchain is missing required component `{component}` for {}",
                host.archive_target
            )));
        };
        let path = Path::new(path);
        if !path.is_file() {
            return Err(OpsError::new(format!(
                "required tool `{component}` is missing at `{}`",
                path.display()
            )));
        }
        let result = run_command_capture(
            &[path.as_os_str().to_owned(), OsString::from("--version")],
            None,
        )?;
        if result.status_code != Some(0) {
            return Err(OpsError::new(format!(
                "required tool `{component}` did not answer `--version`: {}{}",
                result.stdout, result.stderr
            )));
        }
        let output = format!("{}{}", result.stdout, result.stderr);
        let version = first_non_empty_line(&output).unwrap_or("<no version output>");
        println!(
            "toolchain_health.{component}: {} :: {version}",
            path.display()
        );
    }
    if !toolchain.libdir.is_dir() {
        return Err(OpsError::new(format!(
            "toolchain libdir `{}` is missing",
            toolchain.libdir.display()
        )));
    }
    if !toolchain.includedir.is_dir() {
        return Err(OpsError::new(format!(
            "toolchain includedir `{}` is missing",
            toolchain.includedir.display()
        )));
    }
    if let Some(resource_dir) = &toolchain.resource_dir
        && !resource_dir.is_dir()
    {
        return Err(OpsError::new(format!(
            "clang resource dir `{}` is missing",
            resource_dir.display()
        )));
    }
    println!("toolchain_health.status: ok");
    Ok(())
}

fn print_configured_toolchain_roots(kern_toolchain_root: Option<&str>) {
    let configured = kern_toolchain_root
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_dir());
    if let Some(path) = configured {
        println!(
            "KERN_TOOLCHAIN_ROOT: {}",
            canonical_or_original(&path).display()
        );
    } else {
        println!("configured_toolchain_root: <unset>");
    }

    let mut matches = env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            if key.starts_with("LLVM_SYS_") && key.ends_with("_PREFIX") && !value.is_empty() {
                Some((key, PathBuf::from(value)))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some((key, path)) = matches.into_iter().find(|(_, path)| path.is_dir()) {
        println!("{key}: {}", canonical_or_original(&path).display());
    } else {
        println!("LLVM_SYS prefix: <unset>");
    }
}

fn find_active_toolchain_prefix() -> Option<PathBuf> {
    if let Some(root) = env::var_os("KERN_TOOLCHAIN_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    {
        return Some(root);
    }

    let mut matches = env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            if key.starts_with("LLVM_SYS_") && key.ends_with("_PREFIX") && !value.is_empty() {
                Some((key, PathBuf::from(value)))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches
        .into_iter()
        .find_map(|(_, path)| path.is_dir().then_some(path))
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn tool_version_line(path: &Path) -> String {
    let Ok(result) = run_command_capture(
        &[path.as_os_str().to_owned(), OsString::from("--version")],
        None,
    ) else {
        return "<unavailable>".into();
    };
    if result.status_code != Some(0) {
        return "<unavailable>".into();
    }
    let output = format!("{}{}", result.stdout, result.stderr);
    first_non_empty_line(&output)
        .unwrap_or("<unavailable>")
        .to_string()
}

fn component_key_from_tool(tool: &str) -> OpsResult<&'static str> {
    match tool {
        "clang" => Ok("clang"),
        "clang++" => Ok("clangxx"),
        "llvm-ar" => Ok("llvm_ar"),
        "llvm-config" => Ok("llvm_config"),
        "ld.lld" | "ld64.lld" | "lld-link" => Ok("lld"),
        "llvm-lib" => Ok("llvm_lib"),
        other => Err(OpsError::new(format!(
            "unknown required tool `{other}` in CI toolchain policy"
        ))),
    }
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

pub(crate) fn rewrite_kern_versions(root: &Path, version: &str) -> OpsResult<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
