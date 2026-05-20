//! Runtime dependency discovery used while packaging SDK toolchains.
//!
//! The release bundler uses these helpers to locate platform runtime libraries
//! that must travel with the compiled Kern binaries.

use super::util::{
    canonical_or_self, direct_files, files_with_extension, find_program_local, push_unique,
};
use shared_ops::{BundledToolchain, OpsError, OpsResult, run_command, run_command_capture};

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn linux_collect_bundled_runtime_libs(
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
            .is_some_and(should_bundle_linux_runtime_library)
}

fn should_bundle_linux_runtime_library(name: &str) -> bool {
    if name.starts_with("libLLVM") || name.starts_with("libclang") || name.starts_with("libLTO") {
        return true;
    }
    if name.starts_with("ld-linux") {
        return false;
    }
    if !name.contains(".so") {
        return false;
    }
    !is_linux_host_abi_library(name)
}

fn is_linux_host_abi_library(name: &str) -> bool {
    let Some(stem) = name.split(".so").next() else {
        return false;
    };
    matches!(
        stem,
        "libBrokenLocale"
            | "libanl"
            | "libc"
            | "libdl"
            | "libm"
            | "libmvec"
            | "libnsl"
            | "libpthread"
            | "libresolv"
            | "librt"
            | "libutil"
    )
}

pub fn external_runtime_libdirs_for_bundled_tools(
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

pub fn macos_collect_external_runtime_libs(
    roots: &[PathBuf],
    bundled_prefixes: &[PathBuf],
) -> OpsResult<Vec<PathBuf>> {
    let bundled_prefixes = bundled_prefixes
        .iter()
        .map(|path| canonical_or_self(path))
        .collect::<Vec<_>>();
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
                if bundled_prefixes
                    .iter()
                    .any(|prefix| resolved.starts_with(prefix))
                {
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

pub fn macos_collect_runtime_libs(roots: &[PathBuf]) -> OpsResult<Vec<PathBuf>> {
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

pub fn rewrite_macos_toolchain_load_commands(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_runtime_filter_bundles_non_baseline_clang_dependencies() {
        assert!(should_bundle_linux_runtime_library("libedit.so.2"));
        assert!(should_bundle_linux_runtime_library("libtinfo.so.6"));
        assert!(should_bundle_linux_runtime_library("libzstd.so.1"));
        assert!(should_bundle_linux_runtime_library("libstdc++.so.6"));
    }

    #[test]
    fn linux_runtime_filter_keeps_llvm_libraries_and_excludes_host_abi() {
        assert!(should_bundle_linux_runtime_library("libLLVM.so.21.1"));
        assert!(should_bundle_linux_runtime_library("libclang-cpp.so.21.1"));
        assert!(should_bundle_linux_runtime_library("libLTO.so.21.1"));
        assert!(!should_bundle_linux_runtime_library("libc.so.6"));
        assert!(!should_bundle_linux_runtime_library("libm.so.6"));
        assert!(!should_bundle_linux_runtime_library("libpthread.so.0"));
        assert!(!should_bundle_linux_runtime_library("ld-linux-x86-64.so.2"));
    }
}
