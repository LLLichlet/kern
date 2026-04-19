use std::fs;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use kernc_cli::test_support::{
    assert_success, build_and_run, compile_source_tree_with_args, compile_source_with_args,
    emit_llvm_ir_stage_with_args, emit_llvm_ir_with_args, repo_root, run_kernc, unique_temp_path,
};

#[path = "regressions/early.rs"]
mod early;
#[path = "regressions/language.rs"]
mod language;
#[path = "regressions/match_exhaustiveness.rs"]
mod match_exhaustiveness;
#[path = "regressions/mutability.rs"]
mod mutability;
#[path = "regressions/packages.rs"]
mod packages;
#[path = "regressions/paterson.rs"]
mod paterson;
#[path = "regressions/supertraits.rs"]
mod supertraits;

fn compile_source(source: &str) -> std::process::Output {
    compile_source_with_args("kernc_regression_test", source, &[])
}

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args(
        "kernc_regression_std_test",
        source,
        &["--library-bundle", "std"],
    )
}

fn compile_source_tree(entry: &str, files: &[(&str, &str)]) -> std::process::Output {
    compile_source_tree_with_args("kernc_regression_tree", entry, files, &["-c"])
}

fn build_and_run_source(source: &str) -> std::process::Output {
    build_and_run("kernc_regression_run", source, &["--runtime-libc", "yes"])
}

fn build_and_run_source_with_std(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_regression_std_run",
        source,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    )
}
