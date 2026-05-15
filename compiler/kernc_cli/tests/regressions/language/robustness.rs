use super::*;

#[test]
fn rejects_malformed_sources_without_ice_or_backend_failure() {
    let cases = [
        (
            "unterminated_block_comment",
            "fn main() i32 {\n    return 0;\n}\n/* unterminated",
        ),
        (
            "broken_attribute",
            "#[test\nfn main() i32 {\n    return 0;\n}\n",
        ),
        (
            "bad_string_escape",
            "fn main() i32 {\n    let text = \"bad \\x\";\n    return 0;\n}\n",
        ),
        (
            "bad_char_literal",
            "fn main() i32 {\n    let ch = 'xy';\n    return 0;\n}\n",
        ),
        (
            "broken_expression_tree",
            "fn main() i32 {\n    let value = (1 + [2, .{, if (true) {;\n    return value;\n}\n",
        ),
        (
            "bad_generic_decl",
            "struct Box[T: ] {\n    value: T,\n}\nfn main() i32 { return 0; }\n",
        ),
        (
            "bad_trait_impl_header",
            "trait Need { fn value() i32; }\nimpl[N: usize &Need[N] {\n    fn value() i32 { return 0; }\n}\n",
        ),
        (
            "nested_unclosed_groups",
            "fn main() i32 {\n    return foo(bar([.{ .a = (1 + 2 };\n}\n",
        ),
        (
            "control_flow_fragments",
            "fn main() i32 {\n    while (let mut x = ) { break continue return; }\n}\n",
        ),
        (
            "module_fragments",
            "mod inner {\n    use base::{self, , missing::};\n    extern { fn ; }\n",
        ),
        (
            "nul_and_invalid_tokens",
            "fn main() i32 {\n    let x = @@@\0###;\n    return 0;\n}\n",
        ),
        (
            "deeply_repeated_prefixes",
            "fn main() i32 {\n    let x = !!!!!!!!!!!!!&&&&&.....?????;\n    return x;\n}\n",
        ),
    ];

    for (name, source) in cases {
        let output = compile_source(source);
        assert!(
            !output.status.success(),
            "malformed source case {name} unexpectedly compiled:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("panicked at")
                && !stderr.contains("Kern Compiler Internal Error")
                && !stderr.contains("LLVM IR Verification Failed"),
            "malformed source case {name} triggered an internal failure:\n{}",
            stderr
        );
    }
}
