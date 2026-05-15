use super::deps::{
    external_runtime_libdirs_for_bundled_tools, linux_collect_bundled_runtime_libs,
    macos_collect_external_runtime_libs, macos_collect_runtime_libs,
    rewrite_macos_toolchain_load_commands,
};
use super::util::{
    bundled_resource_dir_path, canonical_toolchain_component_name, direct_files,
    files_with_extension, insert_file_record, insert_record, is_empty_dir, path_relative_to,
    relative_under_prefix,
};
use shared_ops::{
    ArtifactRecord, BundledToolchain, OpsError, OpsResult, copy_dir_recursive, copy_path,
    remove_path_if_exists, run_command_capture, sha256_directory,
};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

pub fn bundle_host_toolchain(
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
        let extra_runtime_libs = macos_collect_external_runtime_libs(
            &roots,
            &[bundled_toolchain.prefix.clone(), host_root.clone()],
        )?;
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

pub fn bundle_sdk_runtime_toolchain(
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
