mod archive;
mod bundle;
mod checksum;
mod deps;
mod util;

use crate::args::{ReleaseChecksumsArgs, ReleasePackageArgs, ReleaseToolchainPackageArgs};
use archive::create_archive;
use bundle::{bundle_host_toolchain, bundle_sdk_runtime_toolchain};
use checksum::write_checksums;
use shared_ops::{
    BundledToolchain, HOST_TOOL_BINARIES, OFFICIAL_LIBRARY_LAYERS, OpsError, OpsResult,
    copy_dir_recursive, copy_path, detect_host_target, load_workspace_version,
    remove_path_if_exists, repo_root, resolve_bundled_toolchain, resolve_official_library_root,
    run_command, run_command_with_env, sdk_manifest_json, toolchain_manifest_json,
    write_json_value,
};
use std::ffi::OsString;
use std::fs;
use std::path::Path;

pub fn package_release(args: ReleasePackageArgs) -> OpsResult<()> {
    let root = repo_root()?;
    let host = detect_host_target()?;
    let version = args
        .version
        .unwrap_or_else(|| format!("v{}", load_workspace_version(&root).unwrap_or_default()));
    if version == "v" {
        return Err(OpsError::new("failed to resolve workspace version"));
    }
    let archive_target = args.target.unwrap_or_else(|| host.archive_target.clone());
    ensure_host_native_target(&archive_target, &host)?;
    let bundled_toolchain = resolve_bundled_toolchain(&host, args.toolchain_prefix.as_deref())?;

    if !args.skip_build {
        build_release_binaries(&host)?;
    }

    let dist_name = format!("kern-{version}-{}", host.archive_target);
    let dist_dir = root.join(&dist_name);
    let archive_path = root.join(format!("{dist_name}.{}", host.archive_extension));
    prepare_dist_dir(
        &root,
        &dist_dir,
        &host,
        version.as_str(),
        &bundled_toolchain,
    )?;
    remove_path_if_exists(&archive_path)?;
    create_archive(&root, &dist_dir, &archive_path, &host)?;
    println!("Successfully packaged: {}", archive_path.display());
    Ok(())
}

pub fn package_toolchain_release(args: ReleaseToolchainPackageArgs) -> OpsResult<()> {
    let root = repo_root()?;
    let host = detect_host_target()?;
    let archive_target = args.target.unwrap_or_else(|| host.archive_target.clone());
    ensure_host_native_target(&archive_target, &host)?;
    let bundled_toolchain = resolve_bundled_toolchain(&host, args.toolchain_prefix.as_deref())?;
    let version = args
        .version
        .unwrap_or_else(|| format!("llvm-{}", bundled_toolchain.version));
    let dist_name = format!("kern-toolchain-{version}-{}", host.archive_target);
    let dist_dir = root.join(&dist_name);
    let archive_path = root.join(format!("{dist_name}.{}", host.archive_extension));
    prepare_toolchain_dist_dir(
        &root,
        &dist_dir,
        &host,
        version.as_str(),
        &bundled_toolchain,
    )?;
    remove_path_if_exists(&archive_path)?;
    create_archive(&root, &dist_dir, &archive_path, &host)?;
    println!("Successfully packaged: {}", archive_path.display());
    Ok(())
}

pub fn write_release_checksums(args: ReleaseChecksumsArgs) -> OpsResult<()> {
    write_checksums(args)
}

fn ensure_host_native_target(target: &str, host: &shared_ops::HostTarget) -> OpsResult<()> {
    if target == host.archive_target {
        return Ok(());
    }
    Err(OpsError::new(format!(
        "target label `{target}` does not match the current host `{}`; release packaging is host-native",
        host.archive_target
    )))
}

fn build_release_binaries(host: &shared_ops::HostTarget) -> OpsResult<()> {
    println!("Building release binaries...");
    for (package, bin) in [
        ("kernc_cli", Some("kernc")),
        ("craft", None),
        ("kern-lsp", None),
    ] {
        let mut cmd = vec![
            OsString::from("cargo"),
            OsString::from("build"),
            OsString::from("--release"),
        ];
        if let Some(target) = &host.cargo_target {
            cmd.push(OsString::from("--target"));
            cmd.push(OsString::from(target));
        }
        cmd.push(OsString::from("-p"));
        cmd.push(OsString::from(package));
        if let Some(bin) = bin {
            cmd.push(OsString::from("--bin"));
            cmd.push(OsString::from(bin));
        }
        if host.is_windows {
            run_command_with_env(
                &cmd,
                None,
                &[(
                    "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS",
                    "-C target-feature=+crt-static",
                )],
            )?;
        } else {
            run_command(&cmd, None)?;
        }
    }
    Ok(())
}

fn prepare_dist_dir(
    root: &Path,
    dist_dir: &Path,
    host: &shared_ops::HostTarget,
    version: &str,
    bundled_toolchain: &BundledToolchain,
) -> OpsResult<()> {
    remove_path_if_exists(dist_dir)?;
    fs::create_dir_all(dist_dir.join("bin"))?;
    fs::create_dir_all(dist_dir.join("lib").join("kern"))?;
    fs::create_dir_all(dist_dir.join("manifest"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("bin"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("lib"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("sysroot"))?;

    let binary_dir = if let Some(target) = &host.cargo_target {
        root.join("target").join(target).join("release")
    } else {
        root.join("target").join("release")
    };
    for binary in HOST_TOOL_BINARIES {
        let source = binary_dir.join(format!("{binary}{}", host.exe_suffix));
        if !source.is_file() {
            return Err(OpsError::new(format!(
                "expected release binary `{}`",
                source.display()
            )));
        }
        copy_path(
            &source,
            &dist_dir.join("bin").join(source.file_name().unwrap()),
        )?;
    }

    let library_root = resolve_official_library_root(root)?;
    for workspace_file in ["Craft.toml", "Craft.lock", "README.md"] {
        let source = library_root.join(workspace_file);
        if source.is_file() {
            copy_path(
                &source,
                &dist_dir.join("lib").join("kern").join(workspace_file),
            )?;
        }
    }
    for layer in OFFICIAL_LIBRARY_LAYERS {
        let source = library_root.join(layer);
        if !source.is_dir() {
            return Err(OpsError::new(format!(
                "expected library layer `{}`",
                source.display()
            )));
        }
        copy_dir_recursive(&source, &dist_dir.join("lib").join("kern").join(layer))?;
    }
    let craft_sdk = root.join("tools").join("craft").join("sdk");
    if !craft_sdk.join("init.rn").is_file() {
        return Err(OpsError::new(format!(
            "expected craft SDK `{}`",
            craft_sdk.display()
        )));
    }
    copy_dir_recursive(&craft_sdk, &dist_dir.join("lib").join("kern").join("craft"))?;

    for text_file in ["README.md", "LICENSE"] {
        copy_path(&root.join(text_file), &dist_dir.join(text_file))?;
    }

    let records = bundle_sdk_runtime_toolchain(dist_dir, host, bundled_toolchain)?;
    write_json_value(
        &dist_dir.join("manifest").join("sdk.json"),
        &sdk_manifest_json(
            version,
            &host.archive_target,
            Some(bundled_toolchain),
            Some(&records),
        ),
    )?;
    Ok(())
}

fn prepare_toolchain_dist_dir(
    root: &Path,
    dist_dir: &Path,
    host: &shared_ops::HostTarget,
    version: &str,
    bundled_toolchain: &BundledToolchain,
) -> OpsResult<()> {
    remove_path_if_exists(dist_dir)?;
    fs::create_dir_all(dist_dir.join("manifest"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("bin"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("lib"))?;
    fs::create_dir_all(dist_dir.join("toolchain").join("host").join("sysroot"))?;
    copy_path(&root.join("LICENSE"), &dist_dir.join("LICENSE"))?;

    let records = bundle_host_toolchain(dist_dir, host, bundled_toolchain)?;
    write_json_value(
        &dist_dir.join("manifest").join("toolchain.json"),
        &toolchain_manifest_json(version, &host.archive_target, bundled_toolchain, &records),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::checksum::wildcard_match;

    #[test]
    fn wildcard_matching_covers_release_globs() {
        assert!(wildcard_match(
            "toolchain-dist/*",
            "toolchain-dist/kern.tar.gz"
        ));
        assert!(wildcard_match("a/b/*.rn", "a/b/test.rn"));
        assert!(!wildcard_match("a/b/*.rn", "a/c/test.rn"));
        assert!(!wildcard_match(
            "toolchain-dist/*",
            "toolchain-dist/nested/kern.tar.gz"
        ));
    }
}
