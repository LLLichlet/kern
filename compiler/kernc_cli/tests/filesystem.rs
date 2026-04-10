use std::fs;

use kernc_cli::test_support::{build_and_run, kern_string_literal, unique_temp_path};

#[path = "filesystem/dirs.rs"]
mod dirs;
#[path = "filesystem/io.rs"]
mod io;
#[path = "filesystem/paths.rs"]
mod paths;

fn build_and_run_hosted(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_std_fs",
        source,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    )
}
