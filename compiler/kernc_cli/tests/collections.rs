use kernc_cli::test_support::{build_and_run, compile_source_with_args};

#[path = "collections/map.rs"]
mod map;
#[path = "collections/seq.rs"]
mod seq;
#[path = "collections/tree.rs"]
mod tree;

fn compile_source_with_std(source: &str) -> std::process::Output {
    compile_source_with_args(
        "kernc_std_coll_compile",
        source,
        &["--library-bundle", "std"],
    )
}

fn build_and_run_hosted(source: &str) -> std::process::Output {
    build_and_run(
        "kernc_std_coll",
        source,
        &["--library-bundle", "std", "--runtime-libc", "yes"],
    )
}
