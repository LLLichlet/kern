use super::*;

#[test]
fn analysis_artifact_exposes_unused_private_items() {
    let root = std::env::temp_dir().join(format!(
        "kern_unused_items_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "const dead_const = 1;\n",
        "fn dead_fn() i32 { return dead_const; }\n",
        "extern fn main() i32 { return 0; }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let unused = artifact.unused_private_items();

    assert_eq!(unused.len(), 2);
    assert!(unused.iter().any(|item| {
        item.kind == AnalysisUnusedItemKind::Constant && item.name == "dead_const"
    }));
    assert!(
        unused
            .iter()
            .any(|item| item.kind == AnalysisUnusedItemKind::Function && item.name == "dead_fn")
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_unused_bindings() {
    let root = std::env::temp_dir().join(format!(
        "kern_unused_bindings_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(_: i32, unused_param: i32, used_param: i32) i32 {\n",
        "    let unused_local = used_param;\n",
        "    return used_param;\n",
        "}\n",
        "extern fn main() i32 { return helper(1, 2, 3); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let unused = artifact.unused_bindings();

    assert_eq!(unused.len(), 2);
    assert!(unused.iter().any(|binding| {
        binding.kind == AnalysisUnusedBindingKind::Parameter && binding.name == "unused_param"
    }));
    assert!(unused.iter().any(|binding| {
        binding.kind == AnalysisUnusedBindingKind::Variable && binding.name == "unused_local"
    }));
    assert!(!unused.iter().any(|binding| binding.name == "_"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analysis_artifact_exposes_dead_stores() {
    let root = std::env::temp_dir().join(format!(
        "kern_dead_store_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "fn helper(seed: i32) i32 {\n",
        "    let mut value = seed;\n",
        "    value = seed + 1;\n",
        "    value = seed + 2;\n",
        "    return value;\n",
        "}\n",
        "extern fn main() i32 { return helper(1); }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let dead_stores = artifact.dead_stores();

    assert_eq!(dead_stores.len(), 2);
    assert!(dead_stores.iter().all(|store| store.name == "value"));
    assert!(
        dead_stores
            .iter()
            .any(|store| { store.kind == AnalysisDeadStoreKind::Initializer })
    );
    assert!(
        dead_stores
            .iter()
            .any(|store| { store.kind == AnalysisDeadStoreKind::Assignment })
    );
    for dead_store in &dead_stores {
        assert!(artifact.flow_owners().iter().any(|owner| {
            owner
                .bindings
                .iter()
                .any(|binding| binding.id == dead_store.binding_id)
        }));
    }

    let _ = fs::remove_dir_all(&root);
}
