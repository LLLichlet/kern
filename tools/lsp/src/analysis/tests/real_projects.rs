use super::*;

fn write_lsp_navigation_fixture() -> PathBuf {
    let root = unique_temp_dir("analysis_lsp_navigation_fixture");
    fs::create_dir_all(root.join("src/editor")).unwrap();

    fs::write(
        root.join("Craft.toml"),
        format!(
            r#"
[package]
name = "lsp-fixture"
version = "0.1.0"
kern = "{CURRENT_KERN_VERSION}"

[lib]
root = "src/lib.rn"
"#
        ),
    )
    .unwrap();
    fs::write(
        root.join("src/lib.rn"),
        "\
mod bitio_like;
mod buffer;
pub mod editor;
mod document;
mod owned;

pub use .bitio_like.BitWriter;
pub use .buffer.TextBuffer;
pub use .document.Document;
pub use .owned.Value;
",
    )
    .unwrap();
    fs::write(
        root.join("src/bitio_like.rn"),
        "\
pub enum BitIoError {
    BufferTooSmall,
}

pub struct BitReader {
    ptr: ?&u8 = .None,
}

pub struct BitWriter {
    ptr: ?&mut u8 = .None,
    len: usize = 0,
}

impl &mut BitWriter {
    pub fn write_bit(value: bool) usize!BitIoError {
        if (!value and self.len == 0) {
            return .{ Err: BitIoError.BufferTooSmall };
        }
        return .{ Ok: 1 };
    }

    pub fn write_pair(value: bool) usize!BitIoError {
        _ = self.write_bit(value).?;
        return self.write_bit(false);
    }
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/buffer.rn"),
        "\
pub struct TextBuffer {
    lines: usize = 0,
}

impl &TextBuffer {
    pub fn line_count() usize {
        return self.lines;
    }
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/editor/init.rn"),
        "\
mod window_storage;
mod window_view;
mod render;

pub/ use ..TextBuffer;

pub struct BufferSlot {
    pub text: TextBuffer = TextBuffer.{},
    pub ref_count: usize = 0,
}

pub struct Editor {
    first: BufferSlot = BufferSlot.{},
    second: BufferSlot = BufferSlot.{},
}

pub use .render.rendered_rows;
",
    )
    .unwrap();
    fs::write(
        root.join("src/editor/render.rn"),
        "\
use ..TextBuffer;

pub fn rendered_rows(text_buffer: &TextBuffer) usize {
    return text_buffer.line_count();
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/editor/window_storage.rn"),
        "\
use ..{BufferSlot, Editor};

impl &mut Editor {
    fn buffer_slot_mut(index: usize) &mut BufferSlot {
        if (index == 0) {
            return self.first..&;
        }
        return self.second..&;
    }

    pub fn retain_buffer_slot(index: usize) void {
        let slot = self.buffer_slot_mut(index);
        slot.ref_count += 1;
    }

    pub fn release_buffer_slot(index: usize) void {
        let slot = self.buffer_slot_mut(index);
        if (slot.ref_count > 0) {
            slot.ref_count -= 1;
        }
    }
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/editor/window_view.rn"),
        "\
use ..Editor;

impl &mut Editor {
    pub fn replace_buffer(index: usize) void {
        let slot = self.buffer_slot_mut(index);
        slot.ref_count = 1;
    }
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/owned.rn"),
        "\
pub enum CloneError {
    Empty,
}

pub struct Value {
    raw: &[u8] = \"\",
}

pub fn clone_owned_value_in_arena(value: Value) Value!CloneError {
    if (#value.raw == 0) {
        return .{ Err: CloneError.Empty };
    }
    return .{ Ok: value };
}
",
    )
    .unwrap();
    fs::write(
        root.join("src/document.rn"),
        "\
use ..owned.{CloneError, Value, clone_owned_value_in_arena};

pub struct Document {
    value: Value = Value.{},
}

pub fn document_from_value(value: Value) Document!CloneError {
    let .{ Ok: root } = clone_owned_value_in_arena(value) else {
        return .{ Err: CloneError.Empty };
    };
    let .{ Ok: cloned } = clone_owned_value_in_arena(root) else {
        return .{ Err: CloneError.Empty };
    };
    return .{ Ok: .{ value: cloned } };
}
",
    )
    .unwrap();

    root
}

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

    assert_eq!(impl_symbol.name, "impl &mut File : Write");
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
pub struct Vector2 {
    x: f32,
    y: f32,
}

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
fn limine_smoke_resolves_real_freestanding_project_runtime() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("examples/limine-smoke/src/main.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    assert_eq!(normalize_path(&resolved.input_file), normalize_path(&path));
    assert_eq!(resolved.compile_options.runtime_entry, RuntimeEntry::None);
    assert_eq!(resolved.compile_options.library_bundle, LibraryBundle::Base);
    assert!(resolved.compile_options.module_aliases.contains_key("base"));
    assert!(!resolved.compile_options.module_aliases.contains_key("std"));

    let diagnostics = analysis.analyze_document_uri(&uri);
    let target_bundle = diagnostics
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("diagnostic bundle for limine-smoke kernel");
    assert!(
        target_bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        target_bundle.diagnostics
    );

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "serial_write", 1, 2))
        .unwrap()
        .unwrap();
    assert!(
        hover.contents.value.contains("fn serial_write:"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn limine_mkiso_resolves_real_hosted_build_tool_runtime() {
    let mut analysis = AnalysisEngine::default();
    let path = workspace_root().join("examples/limine-mkiso/src/main.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let resolved = analysis.resolve_analysis(&uri).unwrap();
    assert_eq!(normalize_path(&resolved.input_file), normalize_path(&path));
    assert_eq!(resolved.compile_options.runtime_entry, RuntimeEntry::Rt);
    assert_eq!(resolved.compile_options.library_bundle, LibraryBundle::Std);
    assert!(resolved.compile_options.module_aliases.contains_key("base"));
    assert!(resolved.compile_options.module_aliases.contains_key("std"));

    let diagnostics = analysis.analyze_document_uri(&uri);
    let target_bundle = diagnostics
        .bundles
        .iter()
        .find(|bundle| bundle.uri == uri)
        .expect("diagnostic bundle for limine-mkiso tool");
    assert!(
        target_bundle.diagnostics.is_empty(),
        "unexpected diagnostics: {:?}",
        target_bundle.diagnostics
    );

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "push_shell_arg", 1, 2))
        .unwrap()
        .unwrap();
    assert!(
        hover.contents.value.contains("fn push_shell_arg:"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn hover_renders_optional_mut_pointer_field() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/bitio_like.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let hover = analysis
        .hover(&uri, position_of_nth(&source, "ptr", 1, 1))
        .unwrap()
        .unwrap();

    assert!(
        hover.contents.value.contains("field ptr: ?&mut u8"),
        "{}",
        hover.contents.value
    );
}

#[test]
fn goto_definition_resolves_impl_method_call() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/bitio_like.rn");
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
fn hover_on_impl_method_call_uses_method_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/bitio_like.rn");
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
fn goto_definition_resolves_cross_module_method_call() {
    let mut analysis = AnalysisEngine::default();
    let root = write_lsp_navigation_fixture();
    let path = root.join("src/editor/render.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let definition = analysis
        .goto_definition(&uri, position_of_nth(&source, "line_count", 0, 2))
        .unwrap()
        .unwrap();

    assert_eq!(
        normalize_path(&uri_to_file_path(&definition.uri).unwrap()),
        normalize_path(&root.join("src/buffer.rn"))
    );
}

#[test]
fn references_include_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let root = write_lsp_navigation_fixture();
    let path = root.join("src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);

    let references = analysis
        .references(
            &uri,
            position_of_nth(&source, "buffer_slot_mut", 1, 2),
            true,
        )
        .unwrap();

    let window_view_path = root.join("src/editor/window_view.rn");

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
fn document_highlights_include_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/editor/window_storage.rn");
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
fn hover_on_private_method_call_uses_method_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/editor/window_storage.rn");
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
fn rename_updates_private_method_definition_and_uses() {
    let mut analysis = AnalysisEngine::default();
    let root = write_lsp_navigation_fixture();
    let path = root.join("src/editor/window_storage.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);
    let window_view_path = root.join("src/editor/window_view.rn");
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
fn hover_resolves_imported_function_signature() {
    let mut analysis = AnalysisEngine::default();
    let path = write_lsp_navigation_fixture().join("src/document.rn");
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
fn goto_definition_resolves_imported_function_call() {
    let mut analysis = AnalysisEngine::default();
    let root = write_lsp_navigation_fixture();
    let path = root.join("src/document.rn");
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
        normalize_path(&root.join("src/owned.rn"))
    );
}

#[test]
fn rename_updates_imported_function_definition_import_and_calls() {
    let mut analysis = AnalysisEngine::default();
    let root = write_lsp_navigation_fixture();
    let path = root.join("src/document.rn");
    let (uri, source) = open_workspace_document(&mut analysis, &path);
    let owned_path = root.join("src/owned.rn");
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
