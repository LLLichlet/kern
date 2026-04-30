use super::*;
use crate::AnalysisArtifact;

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
fn analysis_artifact_omits_retained_private_items_from_unused_list() {
    let root = std::env::temp_dir().join(format!(
        "kern_unused_retained_items_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    let source = concat!(
        "#[retain]\n",
        "const kept_const = 1;\n",
        "#[retain]\n",
        "fn kept_fn() i32 { return kept_const; }\n",
        "extern fn main() i32 { return 0; }\n",
    );
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let unused = artifact.unused_private_items();

    assert!(unused.is_empty(), "unexpected unused items: {unused:?}");

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

fn analyze_source_for_diagnostics(name: &str, source: &str) -> AnalysisArtifact {
    let root = std::env::temp_dir().join(format!(
        "{}_{}_{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let main = root.join("main.rn");
    fs::write(&main, source).unwrap();

    let driver = CompilerDriver::new(CompileOptions::default());
    let artifact = driver.analyze_artifact(main.to_str().unwrap(), &SourceOverrides::new());
    let _ = fs::remove_dir_all(&root);
    artifact
}

#[test]
fn rejects_temporary_address_stored_into_static() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_static",
        concat!(
            "type Holder = struct { value: bool };\n",
            "static mut sink = 0 as *mut Holder;\n",
            "fn install() void {\n",
            "    sink = Holder.{ value: true }..&;\n",
            "}\n",
            "extern fn main() i32 { install(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into static storage")
    }));
}

#[test]
fn rejects_temporary_address_return_value() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_return",
        concat!(
            "type Holder = struct { value: bool };\n",
            "fn make() *mut Holder {\n",
            "    return Holder.{ value: true }..&;\n",
            "}\n",
            "extern fn main() i32 { let _ = make(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into a return value")
    }));
}

#[test]
fn rejects_local_temporary_address_return_value() {
    let artifact = analyze_source_for_diagnostics(
        "kern_local_temp_addr_return",
        concat!(
            "type Holder = struct { value: bool };\n",
            "fn make() *mut Holder {\n",
            "    let p = Holder.{ value: true }..&;\n",
            "    return p;\n",
            "}\n",
            "extern fn main() i32 { let _ = make(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into a return value")
    }));
}

#[test]
fn rejects_local_temporary_address_stored_into_static() {
    let artifact = analyze_source_for_diagnostics(
        "kern_local_temp_addr_static",
        concat!(
            "type Holder = struct { value: bool };\n",
            "static mut sink = 0 as *mut Holder;\n",
            "fn install() void {\n",
            "    let p = Holder.{ value: true }..&;\n",
            "    sink = p;\n",
            "}\n",
            "extern fn main() i32 { install(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into static storage")
    }));
}

#[test]
fn rejects_assigned_local_temporary_address_stored_into_static() {
    let artifact = analyze_source_for_diagnostics(
        "kern_assigned_local_temp_addr_static",
        concat!(
            "type Holder = struct { value: bool };\n",
            "static mut sink = 0 as *mut Holder;\n",
            "fn install() void {\n",
            "    let p = Holder.{ value: true }..&;\n",
            "    let q = p;\n",
            "    sink = q;\n",
            "}\n",
            "extern fn main() i32 { install(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into static storage")
    }));
}

#[test]
fn rejects_temporary_address_inside_returned_aggregate() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_return_aggregate",
        concat!(
            "type Holder = struct { value: bool };\n",
            "type Wrapper = struct { ptr: *mut Holder };\n",
            "fn make() Wrapper {\n",
            "    return Wrapper.{ ptr: Holder.{ value: true }..& };\n",
            "}\n",
            "extern fn main() i32 { let _ = make(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into a return value")
    }));
}

#[test]
fn rejects_temporary_address_inside_static_aggregate() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_static_aggregate",
        concat!(
            "type Holder = struct { value: bool };\n",
            "type Wrapper = struct { ptr: *mut Holder };\n",
            "static mut sink = Wrapper.{ ptr: 0 as *mut Holder };\n",
            "fn install() void {\n",
            "    sink = Wrapper.{ ptr: Holder.{ value: true }..& };\n",
            "}\n",
            "extern fn main() i32 { install(); return 0; }\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes into static storage")
    }));
}

#[test]
fn permits_temporary_address_as_call_argument() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_arg",
        concat!(
            "type Holder = struct { value: bool };\n",
            "fn consume(_: *mut Holder) void {}\n",
            "extern fn main() i32 {\n",
            "    consume(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(
        artifact.succeeded,
        "unexpected diagnostics: {:?}",
        artifact.session.diagnostics
    );
}

#[test]
fn permits_temporary_array_address_as_extern_call_argument() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_array_addr_extern_call_arg",
        concat!(
            "extern {\n",
            "    fn write(fd: i32, buf: *mut [5]u8, len: usize) isize;\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    let _ = write(1, [5]u8.{ b'h', b'e', b'l', b'l', b'o' }..&, 5);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(
        artifact.succeeded,
        "unexpected diagnostics: {:?}",
        artifact.session.diagnostics
    );
}

#[test]
fn permits_unrelated_destructured_field_from_temporary_pointer_aggregate() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_destructure_unrelated_field",
        concat!(
            "type Holder = struct { value: bool };\n",
            "type Pair = struct { ptr: *mut Holder, value: bool };\n",
            "fn make_value() bool {\n",
            "    let .{ value: v } = Pair.{ ptr: Holder.{ value: true }..&, value: false };\n",
            "    return v;\n",
            "}\n",
            "extern fn main() i32 { let _ = make_value(); return 0; }\n",
        ),
    );

    assert!(
        artifact.succeeded,
        "unexpected diagnostics: {:?}",
        artifact.session.diagnostics
    );
}

#[test]
fn rejects_temporary_address_passed_to_storing_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_static_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "static mut sink = 0 as *mut Holder;\n",
            "fn store(p: *mut Holder) void {\n",
            "    sink = p;\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    store(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes through function call")
    }));
}

#[test]
fn rejects_temporary_address_passed_to_returning_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_return_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "fn identity(p: *mut Holder) *mut Holder {\n",
            "    return p;\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    let _ = identity(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes through function call")
    }));
}

#[test]
fn rejects_temporary_address_passed_to_aggregate_returning_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_return_aggregate_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "type Wrapper = struct { ptr: *mut Holder };\n",
            "fn wrap(p: *mut Holder) Wrapper {\n",
            "    return Wrapper.{ ptr: p };\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    let _ = wrap(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes through function call")
    }));
}

#[test]
fn rejects_temporary_address_passed_to_aggregate_storing_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_static_aggregate_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "type Wrapper = struct { ptr: *mut Holder };\n",
            "static mut sink = Wrapper.{ ptr: 0 as *mut Holder };\n",
            "fn store(p: *mut Holder) void {\n",
            "    sink = Wrapper.{ ptr: p };\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    store(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes through function call")
    }));
}

#[test]
fn rejects_local_temporary_address_passed_to_storing_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_local_temp_addr_call_static_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "static mut sink = 0 as *mut Holder;\n",
            "fn store(p: *mut Holder) void {\n",
            "    sink = p;\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    let p = Holder.{ value: true }..&;\n",
            "    store(p);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(!artifact.succeeded);
    assert!(artifact.session.diagnostics.iter().any(|diag| {
        diag.message
            .contains("address of temporary value escapes through function call")
    }));
}

#[test]
fn permits_temporary_address_passed_to_non_escaping_function() {
    let artifact = analyze_source_for_diagnostics(
        "kern_temp_addr_call_non_escape",
        concat!(
            "type Holder = struct { value: bool };\n",
            "fn consume(p: *mut Holder) bool {\n",
            "    return p.value;\n",
            "}\n",
            "extern fn main() i32 {\n",
            "    let _ = consume(Holder.{ value: true }..&);\n",
            "    return 0;\n",
            "}\n",
        ),
    );

    assert!(
        artifact.succeeded,
        "unexpected diagnostics: {:?}",
        artifact.session.diagnostics
    );
}
