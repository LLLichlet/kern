//! Standard map collection tests.

use super::*;

#[test]
fn rejects_map_key_without_eq_and_hash() {
    let output = compile_source_with_std(
        r#"
use base.coll.{Map, map};

struct Key {
    raw: i32,
};

fn main() i32 {
    let map = map[Key, i32]()..&;
    let _ = map;
    return 0;
}
"#,
    );

    assert!(
        !output.status.success(),
        "kernc unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Eq[Key]")
            || stderr.contains("Hash[Key]")
            || stderr.contains("Map[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}
