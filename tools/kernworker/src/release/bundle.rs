//! Filesystem bundling logic for release directories.
//!
//! This module copies host tools, official libraries, metadata manifests, and
//! runtime dependencies into the staging layout that archive creation consumes.

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
    remove_path_if_exists, run_command_capture_with_env, sha256_directory, sha256_file,
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
        let mut bundled_prefixes = vec![bundled_toolchain.prefix.clone(), host_root.clone()];
        bundled_prefixes.extend(extra_runtime_lib_dirs.clone());
        for lib_dir_source in &extra_runtime_lib_dirs {
            for dylib in files_with_extension(lib_dir_source, "dylib")? {
                copy_runtime_library(&dylib, &lib_dir)?;
            }
        }
        let mut roots = direct_files(&host_root.join("bin"))?;
        roots.extend(files_with_extension(&lib_dir, "dylib")?);
        let extra_runtime_libs = macos_collect_external_runtime_libs(&roots, &bundled_prefixes)?;
        for dylib in &extra_runtime_libs {
            copy_runtime_library(dylib, &lib_dir)?;
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
            if let Some(parent) = resource_dest.parent() {
                fs::create_dir_all(parent)?;
            }
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
    fs::create_dir_all(&lib_dir)?;

    let runtime_tools = sdk_runtime_tool_paths(host, bundled_toolchain)?;
    let runtime_tool_roots = runtime_tool_source_roots(&runtime_tools);
    let mut copied_tools = Vec::new();
    for (component, source) in runtime_tools {
        let destination = bin_dir.join(path_file_name(&source, "runtime tool")?);
        copy_path(&source, &destination)?;
        insert_file_record(&mut records, &component, &destination, dist_dir)?;
        copied_tools.push((component, destination));
    }

    let runtime_libs = if host.archive_target.ends_with("linux-gnu") {
        linux_collect_bundled_runtime_libs(&runtime_tool_roots, &bundled_toolchain.prefix)?
    } else if host.archive_target.ends_with("apple-darwin") {
        macos_collect_runtime_libs(&runtime_tool_roots)?
    } else {
        Vec::new()
    };
    if !runtime_libs.is_empty() {
        println!("Bundling runtime libraries:");
        for library in &runtime_libs {
            println!("  {}", library.display());
        }
    }
    for library in &runtime_libs {
        if host.archive_target.ends_with("apple-darwin") {
            copy_runtime_library(library, &lib_dir)?;
        } else {
            copy_path(
                library,
                &lib_dir.join(path_file_name(library, "runtime library")?),
            )?;
        }
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

    let should_record_runtime_lib_dir = !runtime_libs.is_empty();
    for (component, path) in &copied_tools {
        verify_sdk_runtime_tool_starts(component, path, &lib_dir, host)?;
    }

    if let Some(resource_dir) = &bundled_toolchain.resource_dir
        && resource_dir.exists()
    {
        let resource_dest = dist_dir.join(bundled_resource_dir_path(bundled_toolchain)?);
        if !resource_dest.exists() {
            if let Some(parent) = resource_dest.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_dir_recursive(resource_dir, &resource_dest)?;
        }
        insert_record(
            &mut records,
            "clang_resource_dir",
            ArtifactRecord {
                path: path_relative_to(&resource_dest, dist_dir)?,
                kind: "directory".into(),
                sha256: Some(sha256_directory(&resource_dest)?),
                size: None,
            },
        );
    }

    if should_record_runtime_lib_dir {
        insert_runtime_lib_dir_record(&mut records, dist_dir, &lib_dir)?;
    }

    fs::write(
        dist_dir.join("toolchain").join("README.md"),
        format!(
            "# Bundled Host Toolchain\n\nThis SDK bundles the minimal host LLVM/Clang runtime needed by installed Kern tools.\n\n- Source: {}\n- Version: {}\n- Bundled runtime tools: {}\n\nThis is intentionally smaller than the standalone toolchain artifact.\nThe SDK includes Clang's resource headers so package `build.kn` C-family compilation can use the bundled SDK clang.\nllvm-config, C++ compiler tools, LLVM libraries for source builds, and the full LLVM development prefix are not part of the end-user SDK.\nClone the repository and configure the host environment directly for source builds.\n",
            bundled_toolchain.source_label,
            bundled_toolchain.version,
            copied_tools
                .iter()
                .filter_map(|(_, path)| path.file_name().and_then(|name| name.to_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )?;
    Ok(records)
}

fn insert_runtime_lib_dir_record(
    records: &mut serde_json::Map<String, serde_json::Value>,
    dist_dir: &Path,
    lib_dir: &Path,
) -> OpsResult<()> {
    insert_record(
        records,
        "runtime_lib_dir",
        ArtifactRecord {
            path: path_relative_to(lib_dir, dist_dir)?,
            kind: "directory".into(),
            sha256: Some(sha256_directory(lib_dir)?),
            size: None,
        },
    );
    Ok(())
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

fn path_file_name<'a>(path: &'a Path, label: &str) -> OpsResult<&'a std::ffi::OsStr> {
    path.file_name().ok_or_else(|| {
        OpsError::new(format!(
            "{label} path `{}` has no file name",
            path.display()
        ))
    })
}

fn copy_runtime_library(source: &Path, lib_dir: &Path) -> OpsResult<()> {
    let Some(name) = source.file_name() else {
        return Err(OpsError::new(format!(
            "runtime library path `{}` has no file name",
            source.display()
        )));
    };
    let destination = lib_dir.join(name);
    if destination.exists() {
        if source.canonicalize().ok() == destination.canonicalize().ok() {
            return Ok(());
        }
        let source_hash = sha256_file(source)?;
        let destination_hash = sha256_file(&destination)?;
        if source_hash == destination_hash {
            return Ok(());
        }
        return Err(OpsError::new(format!(
            "refusing to overwrite bundled runtime library `{}` with different contents from `{}`",
            destination.display(),
            source.display()
        )));
    }
    copy_path(source, &destination)
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

fn runtime_tool_source_roots(runtime_tools: &[(String, PathBuf)]) -> Vec<PathBuf> {
    runtime_tools
        .iter()
        .map(|(_, path)| path.clone())
        .collect::<Vec<_>>()
}

fn verify_sdk_runtime_tool_starts(
    component: &str,
    path: &Path,
    lib_dir: &Path,
    host: &shared_ops::HostTarget,
) -> OpsResult<()> {
    let runtime_env_name = if host.archive_target.ends_with("apple-darwin") {
        Some("DYLD_LIBRARY_PATH")
    } else if host.archive_target.ends_with("linux-gnu") && lib_dir.is_dir() {
        Some("LD_LIBRARY_PATH")
    } else {
        None
    };
    let runtime_env_value = runtime_env_name.map(|_| lib_dir.to_string_lossy().to_string());
    let runtime_env = match (runtime_env_name, runtime_env_value.as_deref()) {
        (Some(name), Some(value)) => vec![(name, value)],
        _ => Vec::new(),
    };
    let result = if component == "llvm_lib" {
        let probe = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("__kern_llvm_lib_probe.lib");
        let result = run_command_capture_with_env(
            &[
                path.as_os_str().to_owned(),
                OsString::from("/llvmlibempty"),
                OsString::from(format!("/out:{}", probe.display())),
            ],
            path.parent(),
            &runtime_env,
        );
        let _ = remove_path_if_exists(&probe);
        result?
    } else {
        run_command_capture_with_env(
            &[path.as_os_str().to_owned(), OsString::from("--version")],
            path.parent(),
            &runtime_env,
        )?
    };
    if result.status_code == Some(0) {
        Ok(())
    } else {
        let stdout = result.stdout.trim();
        let stderr = result.stderr.trim();
        Err(OpsError::new(format!(
            "bundled runtime tool `{}` failed to start while packaging; the SDK runtime subset is missing a required dependency\n  status: {}\n  stdout: {}\n  stderr: {}",
            path.display(),
            result
                .status_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated by signal".to_string()),
            if stdout.is_empty() { "<empty>" } else { stdout },
            if stderr.is_empty() { "<empty>" } else { stderr },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared_ops::{BundledToolchain, make_temp_dir};

    #[test]
    fn copy_runtime_library_skips_existing_library_with_same_contents() {
        let root = make_temp_dir("kernworker-runtime-dylib-same-").unwrap();
        let source_dir = root.join("source");
        let lib_dir = root.join("lib");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        let source = source_dir.join("libduplicate.dylib");
        let destination = lib_dir.join("libduplicate.dylib");
        fs::write(&source, "same").unwrap();
        fs::write(&destination, "same").unwrap();

        copy_runtime_library(&source, &lib_dir).unwrap();

        assert_eq!(fs::read_to_string(destination).unwrap(), "same");
        remove_path_if_exists(&root).unwrap();
    }

    #[test]
    fn copy_runtime_library_rejects_existing_library_with_different_contents() {
        let root = make_temp_dir("kernworker-runtime-dylib-different-").unwrap();
        let source_dir = root.join("source");
        let lib_dir = root.join("lib");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();
        let source = source_dir.join("libduplicate.dylib");
        let destination = lib_dir.join("libduplicate.dylib");
        fs::write(&source, "source").unwrap();
        fs::write(&destination, "destination").unwrap();

        let error = copy_runtime_library(&source, &lib_dir).unwrap_err();

        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(destination).unwrap(), "destination");
        remove_path_if_exists(&root).unwrap();
    }

    #[test]
    fn sdk_runtime_dependency_roots_use_source_tool_paths() {
        let roots = runtime_tool_source_roots(&[
            (
                "clang".to_string(),
                PathBuf::from("/source/toolchain/bin/clang"),
            ),
            (
                "lld".to_string(),
                PathBuf::from("/source/toolchain/bin/ld64.lld"),
            ),
        ]);

        assert_eq!(
            roots,
            vec![
                PathBuf::from("/source/toolchain/bin/clang"),
                PathBuf::from("/source/toolchain/bin/ld64.lld"),
            ]
        );
    }

    #[test]
    fn runtime_lib_dir_record_hashes_final_lib_tree_after_resource_headers() {
        let root = make_temp_dir("kernworker-sdk-runtime-lib-hash-").unwrap();
        let dist = root.join("dist");
        let lib_dir = dist.join("toolchain").join("host").join("lib");
        fs::create_dir_all(lib_dir.join("clang").join("21").join("include")).unwrap();
        fs::write(lib_dir.join("libclang.so.21.1"), "runtime lib\n").unwrap();
        fs::write(
            lib_dir
                .join("clang")
                .join("21")
                .join("include")
                .join("stdarg.h"),
            "/* resource header */\n",
        )
        .unwrap();

        let mut records = serde_json::Map::new();
        insert_runtime_lib_dir_record(&mut records, &dist, &lib_dir).unwrap();

        let runtime_lib_dir = records
            .get("runtime_lib_dir")
            .and_then(|value| value.as_object())
            .expect("expected runtime_lib_dir record");
        assert_eq!(
            runtime_lib_dir.get("path").and_then(|value| value.as_str()),
            Some("toolchain/host/lib")
        );
        let expected = sha256_directory(&lib_dir).unwrap();
        assert_eq!(
            runtime_lib_dir
                .get("sha256")
                .and_then(|value| value.as_str()),
            Some(expected.as_str())
        );
        remove_path_if_exists(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn sdk_runtime_bundle_includes_clang_resource_dir() {
        let root = make_temp_dir("kernworker-sdk-resource-dir-").unwrap();
        let source = root.join("source");
        let dist = root.join("dist");
        let bin = source.join("bin");
        let lib = source.join("lib");
        let include = source.join("include");
        let resource = lib.join("clang").join("21");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&include).unwrap();
        fs::create_dir_all(resource.join("include")).unwrap();
        fs::write(resource.join("include").join("stdarg.h"), "/* builtin */\n").unwrap();

        let clang = PathBuf::from("/bin/true");
        let lld = PathBuf::from("/bin/echo");

        let mut tools = serde_json::Map::new();
        tools.insert(
            "clang".into(),
            serde_json::Value::String(clang.display().to_string()),
        );
        tools.insert(
            "lld".into(),
            serde_json::Value::String(lld.display().to_string()),
        );

        let bundled = BundledToolchain {
            source_label: "test".into(),
            prefix: source,
            bindir: bin,
            libdir: lib,
            includedir: include,
            version: "21.1.8".into(),
            tools,
            resource_dir: Some(resource),
            sysroot_dir: None,
        };
        let host = shared_ops::HostTarget {
            archive_target: "x86_64-linux-gnu".into(),
            cargo_target: None,
            exe_suffix: "",
            archive_extension: "tar.gz".into(),
            is_windows: false,
        };

        let records = bundle_sdk_runtime_toolchain(&dist, &host, &bundled).unwrap();

        assert!(
            dist.join("toolchain/host/lib/clang/21/include/stdarg.h")
                .is_file()
        );
        assert!(records.contains_key("clang_resource_dir"));
        remove_path_if_exists(&root).unwrap();
    }
}
