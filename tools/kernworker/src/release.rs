use crate::args::{ReleaseChecksumsArgs, ReleasePackageArgs, ReleaseToolchainPackageArgs};
use shared_ops::{
    ArtifactRecord, BundledToolchain, HOST_TOOL_BINARIES, OFFICIAL_LIBRARY_LAYERS, OpsError,
    OpsResult, artifact_record_json, copy_dir_recursive, copy_path, detect_host_target, file_size,
    load_workspace_version, remove_path_if_exists, repo_root, resolve_bundled_toolchain,
    resolve_official_library_root, run_command, run_command_capture, run_command_with_env,
    sdk_manifest_json, sha256_directory, sha256_file, toolchain_manifest_json, write_json_value,
};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

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
    let root = repo_root()?;
    let artifacts = resolve_checksum_inputs(&root, &args.paths)?;
    if artifacts.is_empty() {
        return Err(OpsError::new(
            "no release artifacts matched for checksum generation",
        ));
    }

    let mut records = Vec::new();
    for artifact in &artifacts {
        let digest = sha256_file(artifact)?;
        let name = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| OpsError::new("release artifact has an invalid file name"))?;
        let sidecar = artifact.with_file_name(format!("{name}.sha256"));
        fs::write(&sidecar, format!("{digest}  {name}\n"))?;
        records.push(serde_json::json!({
            "name": name,
            "path": name,
            "sha256": digest,
            "size": file_size(artifact)?,
            "sha256_sidecar": sidecar.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
        }));
    }

    if let Some(path) = args.manifest_path {
        let manifest_path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        write_json_value(
            &manifest_path,
            &serde_json::json!({
                "schema_version": 1,
                "channel": args.channel,
                "release_tag": args.release_tag,
                "assets": records,
            }),
        )?;
    }

    println!(
        "Generated checksums for {} release artifact(s)",
        artifacts.len()
    );
    Ok(())
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

fn create_archive(
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

fn bundle_host_toolchain(
    dist_dir: &Path,
    host: &shared_ops::HostTarget,
    bundled_toolchain: &BundledToolchain,
) -> OpsResult<serde_json::Map<String, serde_json::Value>> {
    let host_root = dist_dir.join("toolchain").join("host");
    let bin_dir = host_root.join("bin");
    let lib_dir = host_root.join("lib");
    let sysroot_dir = host_root.join("sysroot");
    let mut records = serde_json::Map::new();
    println!(
        "Bundling host LLVM toolchain {} from {}: {}",
        bundled_toolchain.version,
        bundled_toolchain.source_label,
        bundled_toolchain.prefix.display()
    );

    let copied_bin_dir = host_root.join(relative_under_prefix(
        &bundled_toolchain.bindir,
        &bundled_toolchain.prefix,
    )?);
    let copied_lib_dir = host_root.join(relative_under_prefix(
        &bundled_toolchain.libdir,
        &bundled_toolchain.prefix,
    )?);
    let copied_include_dir = host_root.join(relative_under_prefix(
        &bundled_toolchain.includedir,
        &bundled_toolchain.prefix,
    )?);

    copy_dir_recursive(&bundled_toolchain.bindir, &copied_bin_dir)?;
    copy_dir_recursive(&bundled_toolchain.libdir, &copied_lib_dir)?;
    copy_dir_recursive(&bundled_toolchain.includedir, &copied_include_dir)?;

    insert_record(
        &mut records,
        "bin_dir",
        ArtifactRecord {
            path: path_relative_to(&copied_bin_dir, dist_dir)?,
            kind: "directory".into(),
            sha256: None,
            size: None,
        },
    );
    insert_record(
        &mut records,
        "lib_dir",
        ArtifactRecord {
            path: path_relative_to(&copied_lib_dir, dist_dir)?,
            kind: "directory".into(),
            sha256: None,
            size: None,
        },
    );
    insert_record(
        &mut records,
        "include_dir",
        ArtifactRecord {
            path: path_relative_to(&copied_include_dir, dist_dir)?,
            kind: "directory".into(),
            sha256: None,
            size: None,
        },
    );

    for (component, source) in tool_paths(bundled_toolchain)? {
        let target = if source.starts_with(&bundled_toolchain.prefix) {
            host_root.join(relative_under_prefix(&source, &bundled_toolchain.prefix)?)
        } else {
            bin_dir.join(canonical_toolchain_component_name(
                host, &component, &source,
            ))
        };
        if !target.exists() {
            copy_path(&source, &target)?;
        }
        if !target.is_file() {
            return Err(OpsError::new(format!(
                "bundled toolchain component `{component}` is missing at `{}`",
                target.display()
            )));
        }
        insert_file_record(&mut records, &component, &target, dist_dir)?;
    }

    let extra_runtime_lib_dirs = external_runtime_libdirs_for_bundled_tools(
        bundled_toolchain,
        &tool_paths(bundled_toolchain)?,
    )?;
    if host.archive_target.ends_with("apple-darwin") {
        for lib_dir_source in &extra_runtime_lib_dirs {
            for dylib in files_with_extension(lib_dir_source, "dylib")? {
                copy_path(&dylib, &lib_dir.join(dylib.file_name().unwrap()))?;
            }
        }
        let mut roots = direct_files(&host_root.join("bin"))?;
        roots.extend(files_with_extension(&lib_dir, "dylib")?);
        let extra_runtime_libs =
            macos_collect_external_runtime_libs(&roots, &bundled_toolchain.prefix)?;
        for dylib in &extra_runtime_libs {
            copy_path(dylib, &lib_dir.join(dylib.file_name().unwrap()))?;
        }
        let mut original_libdirs = extra_runtime_lib_dirs;
        original_libdirs.push(bundled_toolchain.libdir.clone());
        original_libdirs.extend(
            extra_runtime_libs
                .iter()
                .filter_map(|path| path.parent().map(Path::to_path_buf)),
        );
        rewrite_macos_toolchain_load_commands(&host_root, &original_libdirs)?;
    } else {
        for extra_lib_dir in extra_runtime_lib_dirs {
            copy_dir_recursive(&extra_lib_dir, &lib_dir)?;
        }
    }

    if let Some(resource_dir) = &bundled_toolchain.resource_dir
        && resource_dir.exists()
    {
        let resource_dest = dist_dir.join(bundled_resource_dir_path(bundled_toolchain)?);
        if !resource_dest.exists() {
            copy_dir_recursive(resource_dir, &resource_dest)?;
        }
        insert_record(
            &mut records,
            "clang_resource_dir",
            ArtifactRecord {
                path: path_relative_to(&resource_dest, dist_dir)?,
                kind: "directory".into(),
                sha256: None,
                size: None,
            },
        );
    }

    if let Some(sysroot_dir_source) = &bundled_toolchain.sysroot_dir {
        fs::write(
            sysroot_dir.join("README.txt"),
            format!(
                "The packaging environment exposed a host SDK path, but Kern does not redistribute platform sysroots from that location.\nObserved host SDK path: {}\n",
                sysroot_dir_source.display()
            ),
        )?;
    } else if is_empty_dir(&sysroot_dir)? {
        fs::write(
            sysroot_dir.join(".empty"),
            "Host OS sysroot contents are not bundled in this SDK.\n",
        )?;
    }

    fs::write(
        dist_dir.join("toolchain").join("README.md"),
        format!(
            "# Bundled Host Toolchain\n\nThis SDK bundles the host LLVM/Clang toolchain used for release validation.\n\n- Source: {}\n- Version: {}\n- Bundled bindir: {}\n- Bundled libdir: {}\n- Bundled includedir: {}\n\nThe SDK keeps user installs pointed at this bundled toolchain first.\nThe packaged toolchain preserves a relocatable LLVM development prefix for source builds.\nHost OS SDK/libc pieces may still remain platform responsibilities.\n",
            bundled_toolchain.source_label,
            bundled_toolchain.version,
            path_relative_to(&copied_bin_dir, dist_dir)?,
            path_relative_to(&copied_lib_dir, dist_dir)?,
            path_relative_to(&copied_include_dir, dist_dir)?,
        ),
    )?;

    for (component, value) in records.clone() {
        if value
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("file")
            != "file"
        {
            continue;
        }
        let path = value
            .get("path")
            .and_then(|path| path.as_str())
            .ok_or_else(|| OpsError::new("toolchain component record has no path"))?;
        insert_file_record(&mut records, &component, &dist_dir.join(path), dist_dir)?;
    }

    Ok(records)
}

fn bundle_sdk_runtime_toolchain(
    dist_dir: &Path,
    host: &shared_ops::HostTarget,
    bundled_toolchain: &BundledToolchain,
) -> OpsResult<serde_json::Map<String, serde_json::Value>> {
    let host_root = dist_dir.join("toolchain").join("host");
    let bin_dir = host_root.join("bin");
    let lib_dir = host_root.join("lib");
    let mut records = serde_json::Map::new();
    println!(
        "Bundling runtime host toolchain subset {} from {}: {}",
        bundled_toolchain.version,
        bundled_toolchain.source_label,
        bundled_toolchain.prefix.display()
    );

    let runtime_tools = sdk_runtime_tool_paths(host, bundled_toolchain)?;
    let mut copied_tools = Vec::new();
    for (component, source) in runtime_tools {
        let destination = bin_dir.join(source.file_name().unwrap());
        copy_path(&source, &destination)?;
        insert_file_record(&mut records, &component, &destination, dist_dir)?;
        copied_tools.push((component, destination));
    }

    let roots = copied_tools
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>();
    let runtime_libs = if host.archive_target.ends_with("linux-gnu") {
        linux_collect_bundled_runtime_libs(&roots, &bundled_toolchain.prefix)?
    } else if host.archive_target.ends_with("apple-darwin") {
        macos_collect_runtime_libs(&roots)?
    } else {
        Vec::new()
    };
    for library in &runtime_libs {
        copy_path(library, &lib_dir.join(library.file_name().unwrap()))?;
    }
    if host.archive_target.ends_with("apple-darwin") && !is_empty_dir(&lib_dir)? {
        let mut original_libdirs = vec![bundled_toolchain.libdir.clone()];
        original_libdirs.extend(
            runtime_libs
                .iter()
                .filter_map(|path| path.parent().map(Path::to_path_buf)),
        );
        rewrite_macos_toolchain_load_commands(&host_root, &original_libdirs)?;
    }

    for (component, path) in &copied_tools {
        verify_sdk_runtime_tool_starts(component, path)?;
    }
    if !is_empty_dir(&lib_dir)? {
        insert_record(
            &mut records,
            "runtime_lib_dir",
            ArtifactRecord {
                path: path_relative_to(&lib_dir, dist_dir)?,
                kind: "directory".into(),
                sha256: Some(sha256_directory(&lib_dir)?),
                size: None,
            },
        );
    }

    fs::write(
        dist_dir.join("toolchain").join("README.md"),
        format!(
            "# Bundled Host Toolchain\n\nThis SDK bundles the minimal host LLVM/Clang runtime needed by installed Kern tools.\n\n- Source: {}\n- Version: {}\n- Bundled runtime tools: {}\n\nThis is intentionally smaller than the standalone toolchain artifact.\nEnd-user SDKs omit the Clang resource dir because Kern only uses Clang as a linker driver here.\nHeaders, llvm-config, and the full LLVM development prefix are not part of the end-user SDK.\nClone the repository and configure the host environment directly for source builds.\n",
            bundled_toolchain.source_label,
            bundled_toolchain.version,
            roots
                .iter()
                .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )?;
    Ok(records)
}

fn tool_paths(bundled_toolchain: &BundledToolchain) -> OpsResult<Vec<(String, PathBuf)>> {
    let mut tools = Vec::new();
    for (component, value) in &bundled_toolchain.tools {
        let Some(path) = value.as_str() else {
            return Err(OpsError::new(format!(
                "bundled toolchain component `{component}` has an invalid path"
            )));
        };
        tools.push((component.clone(), PathBuf::from(path)));
    }
    tools.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(tools)
}

fn sdk_runtime_tool_paths(
    host: &shared_ops::HostTarget,
    bundled_toolchain: &BundledToolchain,
) -> OpsResult<Vec<(String, PathBuf)>> {
    let components = if host.archive_target.ends_with("windows-msvc") {
        vec!["clang", "lld", "llvm_lib"]
    } else {
        vec!["clang", "lld"]
    };
    let tools = tool_paths(bundled_toolchain)?;
    components
        .into_iter()
        .map(|component| {
            tools
                .iter()
                .find(|(name, _)| name == component)
                .map(|(_, path)| (component.to_string(), path.clone()))
                .ok_or_else(|| {
                    OpsError::new(format!(
                        "bundled toolchain is missing runtime component `{component}`"
                    ))
                })
        })
        .collect()
}

fn verify_sdk_runtime_tool_starts(component: &str, path: &Path) -> OpsResult<()> {
    let result = if component == "llvm_lib" {
        let probe = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("__kern_llvm_lib_probe.lib");
        let result = run_command_capture(
            &[
                path.as_os_str().to_owned(),
                OsString::from("/llvmlibempty"),
                OsString::from(format!("/out:{}", probe.display())),
            ],
            path.parent(),
        );
        let _ = remove_path_if_exists(&probe);
        result?
    } else {
        run_command_capture(
            &[path.as_os_str().to_owned(), OsString::from("--version")],
            path.parent(),
        )?
    };
    if result.status_code == Some(0) {
        Ok(())
    } else {
        Err(OpsError::new(format!(
            "bundled runtime tool `{}` failed to start while packaging; the SDK runtime subset is missing a required dependency",
            path.display()
        )))
    }
}

fn linux_collect_bundled_runtime_libs(
    roots: &[PathBuf],
    bundled_prefix: &Path,
) -> OpsResult<Vec<PathBuf>> {
    let mut queued = roots
        .iter()
        .filter(|path| path.is_file())
        .map(|path| canonical_or_self(path))
        .collect::<Vec<_>>();
    let mut visited = Vec::<PathBuf>::new();
    let mut libs = Vec::<PathBuf>::new();
    let bundled_prefix = canonical_or_self(bundled_prefix);
    while let Some(current) = queued.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.push(current.clone());
        for dependency in linux_load_dependencies(&current)? {
            if !is_linux_bundled_runtime_lib(&dependency, &bundled_prefix) {
                continue;
            }
            if !libs.contains(&dependency) {
                libs.push(dependency.clone());
                queued.push(dependency);
            }
        }
    }
    libs.sort();
    Ok(libs)
}

fn linux_load_dependencies(path: &Path) -> OpsResult<Vec<PathBuf>> {
    let result = run_command_capture(&[OsString::from("ldd"), path.as_os_str().to_owned()], None)?;
    if result.status_code != Some(0) {
        return Err(OpsError::new(format!(
            "failed to inspect ELF dependencies for `{}`",
            path.display()
        )));
    }
    let mut dependencies = Vec::new();
    for line in result.stdout.lines() {
        let stripped = line.trim();
        if stripped.is_empty()
            || stripped.contains("statically linked")
            || stripped.contains("not a dynamic executable")
            || stripped.starts_with("linux-vdso")
        {
            continue;
        }
        let candidate = if let Some((_, rhs)) = stripped.split_once("=>") {
            rhs.trim().split(' ').next().unwrap_or_default()
        } else {
            stripped.split(' ').next().unwrap_or_default()
        };
        if candidate.starts_with('/') {
            let path = PathBuf::from(candidate);
            if path.is_file() {
                dependencies.push(canonical_or_self(&path));
            }
        }
    }
    Ok(dependencies)
}

fn is_linux_bundled_runtime_lib(dependency: &Path, bundled_prefix: &Path) -> bool {
    dependency.starts_with(bundled_prefix)
        || dependency
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with("libLLVM")
                    || name.starts_with("libclang")
                    || name.starts_with("libLTO")
            })
}

fn external_runtime_libdirs_for_bundled_tools(
    bundled_toolchain: &BundledToolchain,
    tools: &[(String, PathBuf)],
) -> OpsResult<Vec<PathBuf>> {
    let prefix = canonical_or_self(&bundled_toolchain.prefix);
    let mut dirs = Vec::new();
    for (_, tool) in tools {
        let tool = canonical_or_self(tool);
        if tool.starts_with(&prefix) {
            continue;
        }
        let Some(tool_prefix) = tool.parent().and_then(Path::parent) else {
            continue;
        };
        for candidate in [tool_prefix.join("lib"), tool_prefix.join("lib64")] {
            if candidate.is_dir() {
                let candidate = canonical_or_self(&candidate);
                if !dirs.contains(&candidate) {
                    dirs.push(candidate);
                }
            }
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn macos_collect_external_runtime_libs(
    roots: &[PathBuf],
    bundled_prefix: &Path,
) -> OpsResult<Vec<PathBuf>> {
    let bundled_prefix = canonical_or_self(bundled_prefix);
    let mut queued = roots
        .iter()
        .filter(|path| path.is_file())
        .map(|path| canonical_or_self(path))
        .collect::<Vec<_>>();
    let mut visited = Vec::<PathBuf>::new();
    let mut libs = Vec::<PathBuf>::new();
    while let Some(current) = queued.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.push(current.clone());
        for dependency in macos_load_dependencies(&current)? {
            for candidate in macos_dependency_candidates(&current, &dependency)? {
                if !candidate.is_file() {
                    continue;
                }
                let resolved = canonical_or_self(&candidate);
                if resolved.starts_with(&bundled_prefix) {
                    queued.push(resolved);
                    continue;
                }
                if is_macos_system_library(&resolved) {
                    continue;
                }
                push_unique(&mut libs, candidate);
                push_unique(&mut libs, resolved.clone());
                queued.push(resolved);
            }
        }
    }
    libs.sort();
    Ok(libs)
}

fn macos_collect_runtime_libs(roots: &[PathBuf]) -> OpsResult<Vec<PathBuf>> {
    let mut queued = roots
        .iter()
        .filter(|path| path.is_file())
        .map(|path| canonical_or_self(path))
        .collect::<Vec<_>>();
    let mut visited = Vec::<PathBuf>::new();
    let mut libs = Vec::<PathBuf>::new();
    while let Some(current) = queued.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.push(current.clone());
        for dependency in macos_load_dependencies(&current)? {
            for candidate in macos_dependency_candidates(&current, &dependency)? {
                if !candidate.is_file() || is_macos_system_library(&candidate) {
                    continue;
                }
                let resolved = canonical_or_self(&candidate);
                if is_macos_system_library(&resolved) {
                    continue;
                }
                push_unique(&mut libs, candidate);
                push_unique(&mut libs, resolved.clone());
                queued.push(resolved);
            }
        }
    }
    libs.sort();
    Ok(libs)
}

fn macos_load_dependencies(path: &Path) -> OpsResult<Vec<String>> {
    let result = run_command_capture(
        &[
            OsString::from("otool"),
            OsString::from("-L"),
            path.as_os_str().to_owned(),
        ],
        None,
    )?;
    if result.status_code != Some(0) {
        return Err(OpsError::new(format!(
            "failed to inspect Mach-O load commands for `{}`",
            path.display()
        )));
    }
    Ok(result
        .stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.trim().split(" (compatibility version").next())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn macos_dependency_candidates(path: &Path, dependency: &str) -> OpsResult<Vec<PathBuf>> {
    if dependency.starts_with('/') {
        return Ok(vec![PathBuf::from(dependency)]);
    }
    if let Some(suffix) = dependency.strip_prefix("@loader_path/") {
        return Ok(vec![
            path.parent().unwrap_or_else(|| Path::new(".")).join(suffix),
        ]);
    }
    if let Some(suffix) = dependency.strip_prefix("@executable_path/") {
        return Ok(vec![
            path.parent().unwrap_or_else(|| Path::new(".")).join(suffix),
        ]);
    }
    if let Some(suffix) = dependency.strip_prefix("@rpath/") {
        return Ok(macos_load_rpaths(path)?
            .into_iter()
            .map(|rpath| rpath.join(suffix))
            .collect());
    }
    Ok(Vec::new())
}

fn macos_load_rpaths(path: &Path) -> OpsResult<Vec<PathBuf>> {
    let result = run_command_capture(
        &[
            OsString::from("otool"),
            OsString::from("-l"),
            path.as_os_str().to_owned(),
        ],
        None,
    )?;
    if result.status_code != Some(0) {
        return Err(OpsError::new(format!(
            "failed to inspect Mach-O rpaths for `{}`",
            path.display()
        )));
    }
    let mut rpaths = Vec::new();
    let mut expect_path = false;
    for line in result.stdout.lines() {
        let stripped = line.trim();
        if stripped == "cmd LC_RPATH" {
            expect_path = true;
            continue;
        }
        if !expect_path || !stripped.starts_with("path ") {
            continue;
        }
        let raw_path = stripped
            .split(" (offset")
            .next()
            .unwrap_or_default()
            .trim_start_matches("path ")
            .trim();
        expect_path = false;
        if let Some(suffix) = raw_path.strip_prefix("@loader_path/") {
            rpaths.push(canonical_or_self(
                &path.parent().unwrap_or_else(|| Path::new(".")).join(suffix),
            ));
        } else if let Some(suffix) = raw_path.strip_prefix("@executable_path/") {
            rpaths.push(canonical_or_self(
                &path.parent().unwrap_or_else(|| Path::new(".")).join(suffix),
            ));
        } else if raw_path.starts_with('/') {
            rpaths.push(canonical_or_self(Path::new(raw_path)));
        }
    }
    Ok(rpaths)
}

fn rewrite_macos_toolchain_load_commands(
    host_root: &Path,
    original_libdirs: &[PathBuf],
) -> OpsResult<()> {
    if !host_root.exists() {
        return Ok(());
    }
    let lib_dir = canonical_or_self(&host_root.join("lib"));
    let original_libdirs = original_libdirs
        .iter()
        .map(|path| canonical_or_self(path))
        .collect::<Vec<_>>();
    let mut targets = direct_files(&host_root.join("bin"))?;
    targets.extend(files_with_extension(&lib_dir, "dylib")?);
    let mut modified = Vec::new();
    for target in targets {
        for dependency in macos_load_dependencies(&target)? {
            let dependency_path = PathBuf::from(&dependency);
            if !dependency_path.is_absolute() {
                continue;
            }
            let dependency_resolved = canonical_or_self(&dependency_path);
            if !original_libdirs.iter().any(|libdir| {
                dependency_path.starts_with(libdir) || dependency_resolved.starts_with(libdir)
            }) {
                continue;
            }
            let Some(name) = dependency_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let replacement = macos_local_dylib_reference(&target, name, &lib_dir);
            run_command(
                &[
                    OsString::from("install_name_tool"),
                    OsString::from("-change"),
                    OsString::from(dependency),
                    OsString::from(replacement),
                    target.as_os_str().to_owned(),
                ],
                None,
            )?;
            push_unique(&mut modified, target.clone());
        }
        if target.parent().map(canonical_or_self) == Some(lib_dir.clone())
            && target.extension().and_then(|ext| ext.to_str()) == Some("dylib")
        {
            let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            run_command(
                &[
                    OsString::from("install_name_tool"),
                    OsString::from("-id"),
                    OsString::from(macos_local_dylib_reference(&target, name, &lib_dir)),
                    target.as_os_str().to_owned(),
                ],
                None,
            )?;
            push_unique(&mut modified, target.clone());
        }
    }
    if find_program_local("codesign").is_some() {
        for target in modified {
            run_command(
                &[
                    OsString::from("codesign"),
                    OsString::from("--force"),
                    OsString::from("--sign"),
                    OsString::from("-"),
                    target.as_os_str().to_owned(),
                ],
                None,
            )?;
        }
    }
    Ok(())
}

fn is_macos_system_library(path: &Path) -> bool {
    let raw = path.to_string_lossy();
    raw.starts_with("/usr/lib/") || raw.starts_with("/System/Library/")
}

fn macos_local_dylib_reference(path: &Path, dylib_name: &str, lib_dir: &Path) -> String {
    if path.parent().map(canonical_or_self) == Some(lib_dir.to_path_buf()) {
        format!("@loader_path/{dylib_name}")
    } else {
        format!("@loader_path/../lib/{dylib_name}")
    }
}

fn resolve_checksum_inputs(root: &Path, patterns: &[String]) -> OpsResult<Vec<PathBuf>> {
    let mut matched = Vec::new();
    for pattern in patterns {
        if has_wildcard(pattern) {
            let mut files = Vec::new();
            collect_all_files(root, &mut files)?;
            files.sort();
            for file in files {
                let relative = path_relative_to(&file, root)?;
                if wildcard_match(pattern, &relative) {
                    push_unique(&mut matched, canonical_or_self(&file));
                }
            }
            continue;
        }
        let candidate = if Path::new(pattern).is_absolute() {
            PathBuf::from(pattern)
        } else {
            root.join(pattern)
        };
        if candidate.is_file() {
            push_unique(&mut matched, canonical_or_self(&candidate));
        }
    }
    Ok(matched)
}

fn has_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let mut star = None;
    let mut star_match = 0;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            star_match = t;
            p += 1;
        } else if let Some(star_index) = star {
            if text[star_match] == b'/' {
                return false;
            }
            p = star_index + 1;
            star_match += 1;
            t = star_match;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

fn collect_all_files(root: &Path, out: &mut Vec<PathBuf>) -> OpsResult<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_all_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

fn insert_file_record(
    records: &mut serde_json::Map<String, serde_json::Value>,
    component: &str,
    path: &Path,
    dist_dir: &Path,
) -> OpsResult<()> {
    insert_record(
        records,
        component,
        ArtifactRecord {
            path: path_relative_to(path, dist_dir)?,
            kind: "file".into(),
            sha256: Some(sha256_file(path)?),
            size: Some(file_size(path)?),
        },
    );
    Ok(())
}

fn insert_record(
    records: &mut serde_json::Map<String, serde_json::Value>,
    component: &str,
    record: ArtifactRecord,
) {
    records.insert(component.into(), artifact_record_json(&record));
}

fn bundled_resource_dir_path(bundled_toolchain: &BundledToolchain) -> OpsResult<String> {
    let resource_dir = bundled_toolchain
        .resource_dir
        .as_ref()
        .ok_or_else(|| OpsError::new("bundled toolchain has no clang resource dir"))?;
    let name = resource_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OpsError::new("clang resource dir has an invalid final path component"))?;
    Ok(format!("toolchain/host/lib/clang/{name}"))
}

fn relative_under_prefix(path: &Path, prefix: &Path) -> OpsResult<PathBuf> {
    path.strip_prefix(prefix)
        .map(Path::to_path_buf)
        .map_err(|_| {
            OpsError::new(format!(
                "toolchain path `{}` does not live under prefix `{}`",
                path.display(),
                prefix.display()
            ))
        })
}

fn path_relative_to(path: &Path, root: &Path) -> OpsResult<String> {
    Ok(path
        .strip_prefix(root)
        .map_err(|err| OpsError::new(err.to_string()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn canonical_toolchain_component_name(
    host: &shared_ops::HostTarget,
    component: &str,
    source: &Path,
) -> OsString {
    let exe_suffix = if host.archive_target.ends_with("windows-msvc") {
        ".exe"
    } else {
        ""
    };
    let name = match component {
        "clang" => format!("clang{exe_suffix}"),
        "clangxx" => format!("clang++{exe_suffix}"),
        "lld" if host.archive_target.ends_with("windows-msvc") => "lld-link.exe".into(),
        "lld" if host.archive_target.ends_with("apple-darwin") => "ld64.lld".into(),
        "lld" => "ld.lld".into(),
        "llvm_ar" => format!("llvm-ar{exe_suffix}"),
        "llvm_config" => format!("llvm-config{exe_suffix}"),
        "llvm_lib" => "llvm-lib.exe".into(),
        _ => return source.file_name().unwrap_or_default().to_owned(),
    };
    OsString::from(name)
}

fn files_with_extension(root: &Path, extension: &str) -> OpsResult<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn direct_files(root: &Path) -> OpsResult<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn is_empty_dir(path: &Path) -> OpsResult<bool> {
    Ok(path.is_dir() && fs::read_dir(path)?.next().is_none())
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn push_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}

fn find_program_local(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
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
    None
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
