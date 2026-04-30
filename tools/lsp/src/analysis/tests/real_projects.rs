use super::*;

#[test]
fn stdlib_document_symbols_render_real_impl_target_types() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("library/std/io/init.rn");
    let (uri, _source) = open_workspace_document(&mut analysis, &path);

    let symbols = analysis.document_symbols(&uri).unwrap();
    let impl_symbol = symbols
        .iter()
        .find(|symbol| symbol.detail.as_deref() == Some("impl"))
        .expect("expected std.io impl symbol");

    assert_eq!(impl_symbol.name, "impl *mut File : Writer");
}

#[test]
fn package_api_compile_survives_discard_assignment_queries() {
    let root = unique_temp_dir("analysis_package_api_compile");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "graphics"
version = "0.1.0"
kern = "{CURRENT_KERN_VERSION}"

[lib]
root = "src/lib.rn"

[test]
roots = ["tests/api_compile.rn"]
"#
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rn"),
        "\
pub type Vector2 = struct {
    x: f32,
    y: f32,
};

pub fn vector2(x: f32, y: f32) Vector2 {
    return Vector2.{ x: x, y: y };
}

pub fn window_should_close() bool {
    return false;
}

pub fn get_time() f64 {
    return 0.0;
}
",
    )
    .unwrap();
    let test_source = "\
use graphics;

fn use_window_api() void {
    let _ = graphics.window_should_close();
    _ = graphics.get_time();
    let _ = graphics.vector2(10.0, 20.0);
}

fn main() i32 {
    if (false) {
        use_window_api();
    }
    return 0;
}
";
    let test_path = root.join("tests/api_compile.rn");
    fs::write(&test_path, test_source).unwrap();

    let mut analysis = AnalysisEngine::default();
    let (uri, source) = open_workspace_document(&mut analysis, &test_path);

    let diagnostics = analysis.analyze_document_uri(&uri);
    let semantic_tokens = analysis.semantic_tokens(&uri).unwrap();
    let hover = analysis
        .hover(&uri, position_of_nth(&source, "window_should_close", 0, 1))
        .unwrap();
    let completion = analysis
        .completion(&uri, position_of_nth(&source, "graphics.get_time", 0, 9))
        .unwrap();

    assert!(!diagnostics.bundles.is_empty());
    assert!(!semantic_tokens.data.is_empty());
    assert!(hover.is_some());
    assert!(!completion.is_empty());
}

#[test]
fn bitio_hover_renders_real_optional_mut_pointer_field() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/bitio/src/lib.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "ptr", 1, 1))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.value.contains("field ptr: ?*mut u8"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn bitio_goto_definition_resolves_real_impl_method_call() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/bitio/src/lib.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let definition = analysis
        .goto_definition(&uri, position_of_nth(&source, "self.write_bit", 0, 7))
        .unwrap()
        .unwrap();

    assert_eq!(definition.uri, uri);
    assert_eq!(
        definition.range.start,
        position_of_nth(&source, "write_bit", 0, 0)
    );
}

#[test]
fn bitio_hover_on_real_impl_method_call_uses_method_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/bitio/src/lib.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "self.write_bit", 0, 7))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.value.contains("fn write_bit:"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn bed_goto_definition_resolves_real_internal_method_call() {
    let mut analysis = AnalysisEngine::default();
    let root = workspace_root();
    let path = root.join("incubator/bed/src/editor/render.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let definition = analysis
        .goto_definition(&uri, position_of_nth(&source, "line_count", 0, 2))
        .unwrap()
        .unwrap();

    assert_eq!(
        normalize_path(&uri_to_file_path(&definition.uri).unwrap()),
        normalize_path(&root.join("incubator/bed/src/buffer.rn"))
    );
}

#[test]
fn bed_references_include_real_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let root = workspace_root();
    let path = root.join("incubator/bed/src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let references = analysis
        .references(
            &uri,
            position_of_nth(&source, "buffer_slot_mut", 1, 2),
            true,
        )
        .unwrap();

    let window_view_path = root.join("incubator/bed/src/editor/window_view.rn");

    assert_eq!(references.len(), 4, "{references:#?}");
    assert!(references[..3].iter().all(|location| location.uri == uri));
    assert_eq!(
        references[0].range.start,
        position_of_nth(&source, "buffer_slot_mut", 0, 0)
    );
    assert_eq!(
        references[1].range.start,
        position_of_nth(&source, "buffer_slot_mut", 1, 0)
    );
    assert_eq!(
        references[2].range.start,
        position_of_nth(&source, "buffer_slot_mut", 2, 0)
    );
    assert_eq!(
        normalize_path(&uri_to_file_path(&references[3].uri).unwrap()),
        normalize_path(&window_view_path)
    );
}

#[test]
fn bed_document_highlights_include_real_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/bed/src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let highlights = analysis
        .document_highlights(&uri, position_of_nth(&source, "buffer_slot_mut", 1, 2))
        .unwrap();

    assert_eq!(highlights.len(), 3);
    assert_eq!(
        highlights[0].range.start,
        position_of_nth(&source, "buffer_slot_mut", 0, 0)
    );
    assert_eq!(
        highlights[1].range.start,
        position_of_nth(&source, "buffer_slot_mut", 1, 0)
    );
    assert_eq!(
        highlights[2].range.start,
        position_of_nth(&source, "buffer_slot_mut", 2, 0)
    );
}

#[test]
fn bed_hover_on_real_private_method_call_uses_method_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/bed/src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "buffer_slot_mut", 1, 2))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.value.contains("fn buffer_slot_mut:"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn bed_rename_updates_real_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let root = workspace_root();
    let path = root.join("incubator/bed/src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);
    let window_view_path = root.join("incubator/bed/src/editor/window_view.rn");
    let window_view_uri = file_path_to_uri(&window_view_path).unwrap();
    let window_view_source = fs::read_to_string(&window_view_path).unwrap();

    let edit = analysis
        .rename(
            &uri,
            position_of_nth(&source, "buffer_slot_mut", 1, 2),
            "shared_buffer_slot_mut",
        )
        .unwrap();
    let edits = edit
        .changes
        .get(&uri)
        .expect("rename edits for source file");
    let window_view_edits = edit
        .changes
        .get(&window_view_uri)
        .expect("rename edits for dependent source file");

    assert_eq!(edits.len(), 3);
    assert!(
        edits
            .iter()
            .all(|edit| edit.new_text == "shared_buffer_slot_mut")
    );
    assert_eq!(
        edits[0].range.start,
        position_of_nth(&source, "buffer_slot_mut", 0, 0)
    );
    assert_eq!(
        edits[1].range.start,
        position_of_nth(&source, "buffer_slot_mut", 1, 0)
    );
    assert_eq!(
        edits[2].range.start,
        position_of_nth(&source, "buffer_slot_mut", 2, 0)
    );
    assert_eq!(window_view_edits.len(), 1);
    assert_eq!(window_view_edits[0].new_text, "shared_buffer_slot_mut");
    assert_eq!(
        window_view_edits[0].range.start,
        position_of_nth(&window_view_source, "buffer_slot_mut", 0, 0)
    );
}

#[test]
fn json_hover_resolves_real_imported_function_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("incubator/json/src/document.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let hover = analysis
        .hover(
            &uri,
            position_of_nth(&source, "clone_owned_value_in_arena", 1, 2),
        )
        .unwrap()
        .unwrap();

    assert!(
        hover
            .contents
            .value
            .contains("fn clone_owned_value_in_arena:"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn json_goto_definition_resolves_real_imported_function_call() {
    let mut analysis = AnalysisEngine::default();
    let root = workspace_root();
    let path = root.join("incubator/json/src/document.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let definition = analysis
        .goto_definition(
            &uri,
            position_of_nth(&source, "clone_owned_value_in_arena", 1, 2),
        )
        .unwrap()
        .unwrap();

    assert_eq!(
        normalize_path(&uri_to_file_path(&definition.uri).unwrap()),
        normalize_path(&root.join("incubator/json/src/owned.rn"))
    );
}

#[test]
fn json_rename_updates_real_imported_function_definition_import_and_calls() {
    let mut analysis = AnalysisEngine::default();
    let root = workspace_root();
    let path = root.join("incubator/json/src/document.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);
    let owned_path = root.join("incubator/json/src/owned.rn");
    let owned_uri = file_path_to_uri(&owned_path).unwrap();
    let owned_source = fs::read_to_string(&owned_path).unwrap();

    let edit = analysis
        .rename(
            &uri,
            position_of_nth(&source, "clone_owned_value_in_arena", 1, 2),
            "clone_owned_value_into_document_arena",
        )
        .unwrap();
    let document_edits = edit
        .changes
        .get(&uri)
        .expect("rename edits for document source file");
    let owned_edits = edit
        .changes
        .get(&owned_uri)
        .expect("rename edits for imported definition file");

    assert_eq!(document_edits.len(), 3, "{document_edits:#?}");
    assert!(
        document_edits
            .iter()
            .all(|edit| edit.new_text == "clone_owned_value_into_document_arena")
    );
    assert_eq!(
        document_edits[0].range.start,
        position_of_nth(&source, "clone_owned_value_in_arena", 0, 0)
    );
    assert_eq!(
        document_edits[1].range.start,
        position_of_nth(&source, "clone_owned_value_in_arena", 1, 0)
    );
    assert_eq!(
        document_edits[2].range.start,
        position_of_nth(&source, "clone_owned_value_in_arena", 2, 0)
    );

    assert_eq!(owned_edits.len(), 1, "{owned_edits:#?}");
    assert_eq!(
        owned_edits[0].new_text,
        "clone_owned_value_into_document_arena"
    );
    assert_eq!(
        owned_edits[0].range.start,
        position_of_nth(&owned_source, "clone_owned_value_in_arena", 0, 0)
    );
}
