use std::fs;
use std::process::Command;

use kernc_cli::test_support::{
    assert_not_textual_llvm_ir, assert_success, build_and_run, build_temp_program,
    compile_source_with_args, repo_root, run_kernc, unique_temp_path,
};

#[path = "stdlib/alloc.rs"]
mod alloc;
#[path = "stdlib/bundle.rs"]
mod bundle;
#[path = "stdlib/runtime.rs"]
mod runtime;
#[path = "stdlib/support.rs"]
mod support;
