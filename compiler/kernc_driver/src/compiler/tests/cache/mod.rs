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
        .map(|bytes| bytes.starts_with(b"BC\xc0\xde"))
        .unwrap_or(false)
}

fn nm_reports_object(paths: &[String]) -> bool {
    Command::new("nm")
        .arg("-A")
        .args(paths)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
