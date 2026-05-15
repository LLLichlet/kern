use super::*;

#[test]
fn deterministic_analysis_stress_handles_dirty_documents_and_queries() {
    for seed in 0..64u64 {
        let clean_source = fuzz_source(seed, true);
        let dirty_source = fuzz_source(seed ^ 0x5eed_5eed_d15e_a5e5, false);
        let uri = temp_file_uri(&format!("lsp_analysis_stress_{seed}"), &clean_source);
        let mut analysis = AnalysisEngine::default();

        let open = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            open_document_for_full_diagnostics(&mut analysis, &uri, &clean_source)
        }));
        assert!(
            open.is_ok(),
            "lsp analysis open seed {seed} panicked with source:\n{clean_source}"
        );

        let change = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            change_document_for_full_diagnostics(&mut analysis, &uri, 2, &dirty_source)
        }));
        assert!(
            change.is_ok(),
            "lsp analysis change seed {seed} panicked with source:\n{dirty_source}"
        );

        for position in query_positions(&dirty_source, seed) {
            let queried_position = position.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = analysis.completion(&uri, queried_position.clone());
                let _ = analysis.hover(&uri, queried_position.clone());
                let _ = analysis.signature_help(&uri, queried_position.clone());
                let _ = analysis.goto_definition(&uri, queried_position.clone());
                let _ = analysis.document_highlights(&uri, queried_position.clone());
                let _ = analysis.references(&uri, queried_position, true);
            }));
            assert!(
                result.is_ok(),
                "lsp analysis query seed {seed} panicked at {position:?} with source:\n{dirty_source}"
            );
        }

        let symbols = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = analysis.document_symbols(&uri);
            let _ = analysis.semantic_tokens(&uri);
            analysis.refresh_workspace_targets()
        }));
        assert!(
            symbols.is_ok(),
            "lsp analysis document-level query seed {seed} panicked with source:\n{dirty_source}"
        );
    }
}

#[test]
fn incremental_content_change_fuzz_preserves_valid_utf16_positions() {
    let mut rng = FuzzRng::new(0xc0de_cafe_baad_f00d);
    let mut text = String::from("fn main() i32 {\n    return 0;\n}\n");
    let uri = temp_file_uri("lsp_incremental_change_fuzz", &text);
    let mut analysis = AnalysisEngine::default();

    let _ = analysis.open_document(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            _language_id: "kern".to_string(),
            version: 1,
            text: text.clone(),
        },
    });

    for version in 2..96 {
        let (start, end) = random_range(&text, &mut rng);
        let replacement = fuzz_replacement(&mut rng);
        let range = Range {
            start: position_for_offset(&text, start),
            end: position_for_offset(&text, end),
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = analysis.change_document(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(range),
                    text: replacement.clone(),
                }],
            });
        }));
        assert!(
            result.is_ok(),
            "lsp incremental change version {version} panicked with text:\n{text}"
        );

        text.replace_range(start..end, &replacement);
    }
}

#[test]
fn lsp_analysis_does_not_silence_infrastructure_errors() {
    let queries = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analysis")
            .join("queries.rs"),
    )
    .unwrap();
    let analysis = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("analysis.rs"),
    )
    .unwrap();

    for forbidden in [
        "Err(_) => return Ok(None)",
        "Err(_) => return Ok(Vec::new())",
        "Err(_) => Ok(None)",
        "Err(_) => Ok(Vec::new())",
        ".analyze_interactive_artifact(uri) {\n            Ok(artifact) => artifact,\n            Err(_) => return Ok(None)",
        ".analyze_interactive_navigation_artifact(uri) {\n            Ok(artifact) => artifact,\n            Err(_) => return Ok",
    ] {
        assert!(
            !queries.contains(forbidden),
            "LSP analysis query code must not silence infrastructure errors with `{forbidden}`"
        );
        assert!(
            !analysis.contains(forbidden),
            "LSP analysis core code must not silence infrastructure errors with `{forbidden}`"
        );
    }
}

fn query_positions(source: &str, seed: u64) -> Vec<Position> {
    let mut offsets = vec![0, source.len()];
    let mut rng = FuzzRng::new(seed ^ 0x9e37_79b9_7f4a_7c15);
    for _ in 0..8 {
        offsets.push(random_char_boundary(source, &mut rng));
    }
    offsets.sort_unstable();
    offsets.dedup();
    offsets
        .into_iter()
        .map(|offset| position_for_offset(source, offset))
        .collect()
}

fn position_for_offset(source: &str, offset: usize) -> Position {
    let clamped = offset.min(source.len());
    let prefix = &source[..clamped];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let character = source[line_start..clamped].encode_utf16().count() as u32;
    Position { line, character }
}

fn random_range(source: &str, rng: &mut FuzzRng) -> (usize, usize) {
    let mut start = random_char_boundary(source, rng);
    let mut end = random_char_boundary(source, rng);
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    (start, end)
}

fn random_char_boundary(source: &str, rng: &mut FuzzRng) -> usize {
    let mut boundaries = source
        .char_indices()
        .map(|(idx, _)| idx)
        .collect::<Vec<_>>();
    boundaries.push(source.len());
    boundaries[rng.range(boundaries.len() as u64) as usize]
}

fn fuzz_source(seed: u64, include_main: bool) -> String {
    const FRAGMENTS: &[&str] = &[
        "fn",
        "main",
        "helper",
        "(",
        ")",
        "{",
        "}",
        "[",
        "]",
        ".",
        "..&",
        ".{",
        ";",
        ":",
        "=",
        "return",
        "let",
        "mut",
        "if",
        "else",
        "match",
        "struct",
        "trait",
        "impl",
        "i32",
        "void",
        "true",
        "false",
        "0",
        "1",
        "\"unterminated",
        "\"ok\"",
        "'x'",
        "'xy'",
        "// comment\n",
        "/* unterminated",
        "/// docs\n",
        "\u{e9}",
        "\u{4e2d}",
    ];

    let mut rng = FuzzRng::new(seed ^ 0xa076_1d64_78bd_642f);
    let mut source = String::new();
    if include_main {
        source.push_str("fn main() i32 {\n    return 0;\n}\n");
    }

    let target_len = 18 + rng.range(96) as usize;
    for index in 0..target_len {
        if index % 13 == 0 {
            source.push('\n');
        } else if rng.range(4) == 0 {
            source.push(' ');
        }
        source.push_str(FRAGMENTS[rng.range(FRAGMENTS.len() as u64) as usize]);
    }
    source
}

fn fuzz_replacement(rng: &mut FuzzRng) -> String {
    const REPLACEMENTS: &[&str] = &[
        "",
        " ",
        "\n",
        "let value = 1;\n",
        "return value;\n",
        "helper(",
        ".",
        ".{",
        "\"bad",
        "/* comment */",
        "\u{e9}",
        "\u{4e2d}",
    ];
    REPLACEMENTS[rng.range(REPLACEMENTS.len() as u64) as usize].to_string()
}

struct FuzzRng {
    state: u64,
}

impl FuzzRng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn range(&mut self, upper: u64) -> u64 {
        self.next() % upper
    }
}
