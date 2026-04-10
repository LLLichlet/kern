use super::*;

#[test]
fn rejects_kmeta_package_with_mismatched_declared_identity() {
    let root = temp_dir("craft-exec-kmeta-identity");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Kmeta.toml"),
        r#"
format_version = 2
kind = "source_snapshot"
package_name = "other"
package_version = "2.0.0"
root_module_name = "other"
entry_module_path = "src/init.rn"
"#,
    )
    .unwrap();

    let err = validate_package_metadata_root(&root, "util", Some("1.0.0")).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("declares package `other` but `util` was required"),
        "unexpected error: {message}"
    );

    let _ = fs::remove_dir_all(root);
}
