//! Imported-cache tests.
//!
//! These tests verify analysis artifacts can be derived from imported metadata
//! caches and remain coherent across dirty source edits.

use super::*;

#[test]
fn imported_structure_cache_reuses_loaded_frontend_modules_without_type_stage() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_structure_cache_{}_{}",
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

    let imported = driver
        .analyze_imported_structure(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("imported structure should be available");
    let parse_count = driver.uncached_parse_count();

    let items = imported.completion_items(main.as_path(), 0);
    assert!(!items.is_empty());

    let imported_again = driver
        .analyze_imported_structure(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("imported structure should stay cached");
    assert!(
        !imported_again
            .completion_items(main.as_path(), 0)
            .is_empty()
    );
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_imported_structure_reuses_clean_imported_structure_for_body_only_overrides() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_body_only_reuse_{}_{}",
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

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &clean, &CancellationToken::new())
            .unwrap()
            .is_some()
    );
    let reuse_count = driver.body_only_imported_reuse_count();
    let parse_count = driver.uncached_parse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(main.clone(), "fn main() i32 { return 2; }".to_string());
    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty, &CancellationToken::new())
            .unwrap()
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count + 1);

    let dirty_parse_count = driver.uncached_parse_count();
    assert!(dirty_parse_count > parse_count);

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty, &CancellationToken::new())
            .unwrap()
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count + 1);
    assert_eq!(driver.uncached_parse_count(), dirty_parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_imported_structure_does_not_reuse_clean_imported_structure_for_surface_changes() {
    let root = std::env::temp_dir().join(format!(
        "kern_imported_surface_change_{}_{}",
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

    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &clean, &CancellationToken::new())
            .unwrap()
            .is_some()
    );
    let reuse_count = driver.body_only_imported_reuse_count();

    let mut dirty = SourceOverrides::new();
    dirty.insert(
        main.clone(),
        "fn main(value: i32) i32 { return value; }".to_string(),
    );
    assert!(
        driver
            .analyze_imported_structure(main.to_str().unwrap(), &dirty, &CancellationToken::new())
            .unwrap()
            .is_some()
    );
    assert_eq!(driver.body_only_imported_reuse_count(), reuse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn surface_artifact_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_surface_cache_{}_{}",
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

    assert!(
        driver
            .analyze_imported_structure(
                main.to_str().unwrap(),
                &overrides,
                &CancellationToken::new()
            )
            .unwrap()
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let surface = driver
        .analyze_surface(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("surface artifact should be derivable from imported cache");
    assert!(!surface.symbols.is_empty());
    assert!(!surface.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn analyze_structure_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_structure_from_imported_cache_{}_{}",
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

    assert!(
        driver
            .analyze_imported_structure(
                main.to_str().unwrap(),
                &overrides,
                &CancellationToken::new()
            )
            .unwrap()
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let structure = driver
        .analyze_structure(main.to_str().unwrap(), &overrides)
        .expect("typed structure should be derivable from imported cache");
    assert!(!structure.completion_items(main.as_path(), 0).is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parse_modules_reuses_cached_imported_stage_without_extra_frontend_parse() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_from_imported_cache_{}_{}",
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

    assert!(
        driver
            .analyze_imported_structure(
                main.to_str().unwrap(),
                &overrides,
                &CancellationToken::new()
            )
            .unwrap()
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("parsed modules should be derivable from imported cache");
    assert!(!parsed.modules.is_empty());
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn parsed_modules_cache_preserves_body_completion_regions() {
    let root = std::env::temp_dir().join(format!(
        "kern_parse_body_regions_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.kn");
    let source = "fn main() i32 {\n    return 1;\n}\n";
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let overrides = SourceOverrides::new();

    assert!(
        driver
            .analyze_imported_structure(
                main.to_str().unwrap(),
                &overrides,
                &CancellationToken::new()
            )
            .unwrap()
            .is_some()
    );
    let parse_count = driver.uncached_parse_count();

    let parsed = driver
        .parse_modules(
            main.to_str().unwrap(),
            &overrides,
            &CancellationToken::new(),
        )
        .unwrap()
        .expect("parsed modules should be derivable from imported cache");
    let body_offset = source.find("return").unwrap();
    let top_level_offset = source.find("fn").unwrap();

    assert!(parsed.requires_body_completion(main.as_path(), body_offset));
    assert!(!parsed.requires_body_completion(main.as_path(), top_level_offset));
    assert_eq!(driver.uncached_parse_count(), parse_count);

    let _ = fs::remove_dir_all(&root);
}
