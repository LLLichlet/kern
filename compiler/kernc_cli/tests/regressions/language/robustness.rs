//! Malformed-source robustness regression tests.

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
        assert_no_internal_failure(&stderr, &format!("malformed source case {name}"));
    }
}

#[test]
fn deterministic_kernc_stress_compiles_generated_valid_programs() {
    for seed in 0..32u64 {
        let source = valid_stress_source(seed);
        let output = compile_source(&source);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_no_internal_failure(&stderr, &format!("valid kernc stress seed {seed}"));
        assert!(
            output.status.success(),
            "valid kernc stress seed {seed} failed:\nstdout:\n{}\nstderr:\n{}\nsource:\n{}",
            String::from_utf8_lossy(&output.stdout),
            stderr,
            source
        );
    }
}

#[test]
fn deterministic_kernc_stress_rejects_generated_bad_programs_without_ice() {
    for seed in 0..32u64 {
        let source = invalid_stress_source(seed);
        let output = compile_source(&source);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_no_internal_failure(&stderr, &format!("invalid kernc stress seed {seed}"));
        assert!(
            !output.status.success(),
            "invalid kernc stress seed {seed} unexpectedly compiled:\nstdout:\n{}\nstderr:\n{}\nsource:\n{}",
            String::from_utf8_lossy(&output.stdout),
            stderr,
            source
        );
    }
}

fn assert_no_internal_failure(stderr: &str, context: &str) {
    assert!(
        !stderr.contains("panicked at")
            && !stderr.contains("Kern Compiler Internal Error")
            && !stderr.contains("LLVM IR Verification Failed"),
        "{context} triggered an internal failure:\n{stderr}"
    );
}

fn valid_stress_source(seed: u64) -> String {
    match seed % 8 {
        0 => format!(
            r#"
fn inc_{seed}(value: i32) i32 {{
    return value + {bias}i32;
}}

fn main() i32 {{
    let value = inc_{seed}({input}i32);
    return value - {expected}i32;
}}
"#,
            bias = (seed % 7) + 1,
            input = (seed % 11) + 3,
            expected = (seed % 7) + 1 + (seed % 11) + 3,
        ),
        1 => format!(
            r#"
struct Pair_{seed} {{
    left: i32,
    right: i32,
}}

fn main() i32 {{
    let pair = Pair_{seed}.{{ left: {left}i32, right: {right}i32 }};
    return pair.left + pair.right - {expected}i32;
}}
"#,
            left = (seed % 5) + 2,
            right = (seed % 13) + 4,
            expected = (seed % 5) + 2 + (seed % 13) + 4,
        ),
        2 => format!(
            r#"
enum Choice_{seed} {{
    Left: i32,
    Right: i32,
}}

fn score_{seed}(value: Choice_{seed}) i32 {{
    return match (value) {{
        .{{ Left: amount }} => amount,
        .{{ Right: amount }} => amount + 1i32,
    }};
}}

fn main() i32 {{
    return score_{seed}(Choice_{seed}.{{ Right: {value}i32 }}) - {expected}i32;
}}
"#,
            value = (seed % 9) + 1,
            expected = (seed % 9) + 2,
        ),
        3 => format!(
            r#"
fn main() i32 {{
    let mut value = 0i32;
    let mut index = 0i32;
    while (index < {limit}i32) {{
        value += index;
        index += 1i32;
    }}
    return value - {sum}i32;
}}
"#,
            limit = (seed % 5) + 3,
            sum = {
                let limit = (seed % 5) + 3;
                limit * (limit - 1) / 2
            },
        ),
        4 => format!(
            r#"
fn choose_{seed}(flag: bool, left: i32, right: i32) i32 {{
    if (flag) {{
        return left;
    }}
    return right;
}}

fn main() i32 {{
    return choose_{seed}({flag}, {left}i32, {right}i32) - {expected}i32;
}}
"#,
            flag = if seed % 2 == 0 { "true" } else { "false" },
            left = (seed % 17) + 1,
            right = (seed % 19) + 2,
            expected = if seed % 2 == 0 {
                (seed % 17) + 1
            } else {
                (seed % 19) + 2
            },
        ),
        5 => format!(
            r#"
fn apply_{seed}(callback: &Fn(i32) i32, value: i32) i32 {{
    return callback(value);
}}

fn main() i32 {{
    let add = [](value: i32) i32 {{
        return value + {delta}i32;
    }};
    return apply_{seed}(add, {input}i32) - {expected}i32;
}}
"#,
            delta = (seed % 6) + 1,
            input = (seed % 8) + 2,
            expected = (seed % 6) + 1 + (seed % 8) + 2,
        ),
        6 => format!(
            r#"
const VALUE_{seed}: i32 = {value}i32;

fn main() i32 {{
    return VALUE_{seed} - {value}i32;
}}
"#,
            value = (seed % 23) + 1,
        ),
        _ => format!(
            r#"
fn main() i32 {{
    let first = {a}i32;
    let second = {b}i32;
    let third = {c}i32;
    return first + second + third - {sum}i32;
}}
"#,
            a = (seed % 3) + 1,
            b = (seed % 5) + 2,
            c = (seed % 7) + 3,
            sum = (seed % 3) + 1 + (seed % 5) + 2 + (seed % 7) + 3,
        ),
    }
}

fn invalid_stress_source(seed: u64) -> String {
    let mut rng = FuzzRng::new(seed ^ 0x7a17_4c2d_932f_1b5d);
    let mut source = String::from("fn main() i32 {\n");
    let target_len = 12 + rng.range(48) as usize;

    for index in 0..target_len {
        if index % 9 == 0 {
            source.push('\n');
        } else if rng.range(4) == 0 {
            source.push(' ');
        }
        source.push_str(BAD_FRAGMENTS[rng.range(BAD_FRAGMENTS.len() as u64) as usize]);
    }

    if seed % 3 == 0 {
        source.push_str("\n}\n");
    }
    source
}

const BAD_FRAGMENTS: &[&str] = &[
    "let",
    "mut",
    "return",
    "if",
    "else",
    "match",
    "struct",
    "enum",
    "impl",
    "trait",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    ".{",
    ".[",
    "..&",
    ".?",
    "=>",
    "=",
    "==",
    "!",
    "+",
    "-",
    "*",
    "&",
    "|",
    ":",
    ";",
    ",",
    "i32",
    "void",
    "Fn",
    "Self",
    "0x",
    "0b102",
    "1e+",
    "\"unterminated",
    "\"\\x",
    "'xy'",
    "'\\x'",
    "/* unterminated",
    "/// docs\n",
    "\u{e9}",
    "\u{4e2d}",
];

struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn range(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}
