use super::*;

#[test]
fn structure_cache_reuses_loaded_frontend_modules_until_input_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert!(driver.uncached_parse_count() > parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_reuses_clean_typed_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_body_only_reuse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_structure_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_does_not_reuse_clean_typed_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_surface_change_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &clean)
            .is_some()
    );
    let reuse_count = driver.body_only_structure_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() bool { return true; }".to_string());
    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &dirty)
            .is_some()
    );
    assert_eq!(driver.body_only_structure_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_structure_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_modules_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &overrides)
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &overrides)
        .expect("parsed modules should be available from cached structure");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_only_loads_referenced_alias_roots() {
    let root = std::env::temp_dir().join(format!(
        "kern_alias_root_references_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();

    let main = root.join("main.rn");
    let foo = root.join("foo");
    let bar = root.join("bar");
    fs::create_dir_all(&foo).unwrap();
    fs::create_dir_all(&bar).unwrap();

    fs::write(&main, "use foo.answer;\nfn main() i32 { return answer; }\n").unwrap();
    fs::write(foo.join("init.rn"), "pub const answer = 7;\n").unwrap();
    fs::write(bar.join("init.rn"), "pub const unused = 9;\n").unwrap();

    let mut options = CompileOptions::default();
    options
        .module_aliases
        .insert("foo".to_string(), foo.to_string_lossy().to_string());
    options
        .module_aliases
        .insert("bar".to_string(), bar.to_string_lossy().to_string());
    let driver = CompilerDriver::new(options);

    assert!(
        driver
            .analyze_structure(main.to_str().unwrap(), &SourceOverrides::new())
            .is_some()
    );
    assert_eq!(driver.uncached_parse_count(), 2);

    let _ = fs::remove_dir_all(&root);
}
