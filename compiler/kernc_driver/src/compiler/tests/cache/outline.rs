use super::*;

#[test]
fn analyze_outline_reuses_collected_cache_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_collected_cache_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    let parsed = driver
        .parse_modules(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("parsed modules should be available");
    assert!(!parsed.modules.is_empty());
    let parse_count = driver.uncached_parse_count();

    let outline = driver
        .analyze_outline(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap();
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_reuses_clean_collected_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_body_only_reuse_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &clean, &CancellationToken::new())
        .unwrap()
        .expect("parsed modules should warm clean collected cache");
    assert!(!parsed.modules.is_empty());
    let reuse_count = driver.body_only_collected_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    let outline = driver
        .analyze_outline(main.to_str().unwrap(), &dirty, &CancellationToken::new())
        .unwrap();
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    let outline = driver
        .analyze_outline(main.to_str().unwrap(), &dirty, &CancellationToken::new())
        .unwrap();
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_outline_does_not_reuse_clean_collected_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_outline_surface_change_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    fs::write(&main, "fn main() i32 { return 1; }").unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let clean = SourceOverrides::new();

    let parsed = driver
        .parse_modules(main.to_str().unwrap(), &clean, &CancellationToken::new())
        .unwrap()
        .expect("parsed modules should warm clean collected cache");
    assert!(!parsed.modules.is_empty());
    let reuse_count = driver.body_only_collected_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(
        main.clone(),
        "fn main(value: i32) i32 { return value; }".to_string(),
    );
    let outline = driver
        .analyze_outline(main.to_str().unwrap(), &dirty, &CancellationToken::new())
        .unwrap();
    assert!(!outline.symbols.is_empty());
    assert_eq!(driver.body_only_collected_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}
