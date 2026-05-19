//! Standard tree collection tests.

use super::*;

#[test]
fn rejects_tree_key_without_ord() {
    let output = compile_source_with_std(
        r#"
use base.coll.{Tree, tree};

struct Key {
    raw: i32,
};

fn main() i32 {
    let map = tree[Key, i32]()..&;
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
        stderr.contains("Ord[Key]") || stderr.contains("Tree[Key, i32]"),
        "unexpected stderr:\n{}",
        stderr
    );
}
