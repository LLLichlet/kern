use super::*;
use crate::CodegenPlanFallback;
use kernc_utils::config::{LibraryBundle, RuntimeEntry};

mod codegen;
mod imported;
mod outline;
mod reporting;
mod structure;

fn manifest_object_paths(manifest: &std::path::Path) -> Vec<String> {
    fs::read_to_string(manifest)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("linker_input=").map(ToOwned::to_owned))
        .collect()
}

fn has_llvm_bitcode_magic(path: &std::path::Path) -> bool {
    fs::read(path)
        .map(|bytes| {
            bytes.starts_with(b"BC\xc0\xde")
                || bytes.starts_with(&[0xDE, 0xC0, 0x17, 0x0B])
        })
        .unwrap_or(false)
}

fn symbol_dump_tool() -> String {
    for candidate in if cfg!(windows) {
        vec!["llvm-nm.exe", "nm.exe"]
    } else {
        vec!["llvm-nm", "nm"]
    } {
        if let Some(path) = find_tool_in_path(candidate) {
            return path;
        }
    }

    if cfg!(windows) {
        for candidate in [
            r"C:\Program Files\LLVM\bin\llvm-nm.exe",
            r"C:\LLVM-21\bin\llvm-nm.exe",
        ] {
            if std::path::Path::new(candidate).is_file() {
                return candidate.to_string();
            }
        }
    }

    panic!("failed to locate `llvm-nm` or `nm` in PATH");
}

fn find_tool_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    None
}

fn nm_reports_object(paths: &[String]) -> bool {
    Command::new(symbol_dump_tool())
        .arg("-A")
        .args(paths)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
