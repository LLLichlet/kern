//! CLI integration tests for bundled standard-library behavior.

use std::fs;
use std::process::Command;

use kernc_cli::test_support::{
    assert_not_textual_llvm_ir, assert_success, build_and_run, build_temp_program,
    compile_source_with_args, repo_root, run_kernc, run_kernc_with_env, unique_temp_path,
};

// These are toolchain integration tests for consuming the official library
// workspace. Library-behavior coverage belongs in Kern test packages under
// `library/`, such as `kernlib-test`.

const HOSTED_HELLO_WORLD_SOURCE: &str = r#"
use std.io;

fn main() i32 {
    "hello, {}!".fmt(.{"world"}).println();
    return 0;
}
"#;

#[path = "stdlib/bundle.rs"]
mod bundle;
#[path = "stdlib/runtime.rs"]
mod runtime;
#[path = "stdlib/support.rs"]
mod support;
