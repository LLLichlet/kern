//! CLI integration tests for the standard collection packages.

use kernc_cli::test_support::compile_source_with_args;

#[path = "collections/map.rs"]
mod map;
#[path = "collections/tree.rs"]
mod tree;

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args(
        "kernc_std_coll_compile",
        source,
        &["--library-bundle", "std"],
    )
}
