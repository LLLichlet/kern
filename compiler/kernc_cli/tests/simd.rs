use kernc_cli::test_support::{
    build_and_run, compile_source_with_args as compile_with_args,
    emit_llvm_ir_with_args as emit_ir_with_args,
};
use kernc_utils::config::resolve_base_path;

mod simd {
    use super::*;

    mod core;
    mod diagnostics;
    mod ir;
    mod memory;

    fn compile_source(source: &str) -> std::process::Output {
        compile_with_args("kernc_simd_test", source, &[])
    }

    fn emit_llvm_ir(source: &str) -> std::process::Output {
        emit_ir_with_args("kernc_simd_test", source, &[])
    }

    fn build_and_run_source(source: &str) -> std::process::Output {
        build_and_run("kernc_simd_test", source, &[])
    }
}
