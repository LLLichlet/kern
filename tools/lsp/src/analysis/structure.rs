use super::*;
use kernc_lexer::{LexemeType, TokenType, Tokenizer};
use kernc_utils::{FileId, SourceFile};

#[derive(Debug, Clone, Copy)]
struct DelimiterSpan {
    start: usize,
    end: usize,
}

impl AnalysisEngine {
    #[cfg(test)]
    pub fn folding_ranges(&self, uri: &str) -> Result<Vec<IdeFoldingRange>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.folding_ranges_in_snapshot(&snapshot, uri)
    }

    pub fn folding_ranges_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeFoldingRange>, String> {
        snapshot.check_canceled()?;
        let file = snapshot.document_source_file(uri).ok_or_else(|| {
            "requested folding ranges for a document that is not open".to_string()
        })?;

        let mut ranges = Vec::new();
        let mut stack = Vec::new();
        let mut tokenizer = Tokenizer::new(&file.src, FileId(0));

        loop {
            let lexeme = tokenizer.next_lexeme();
            match lexeme.tag {
                LexemeType::BlockComment => {
                    if let Some(range) = folding_range_for_span(
                        &file,
                        lexeme.span.start,
                        lexeme.span.end,
                        Some(IdeFoldingRangeKind::Comment),
                    ) {
                        ranges.push(range);
                    }
                }
                LexemeType::Token(TokenType::LBrace | TokenType::DotLBrace) => {
                    stack.push(lexeme.span.start);
                }
                LexemeType::Token(TokenType::RBrace) => {
                    if let Some(start) = stack.pop()
                        && let Some(range) =
                            folding_range_for_span(&file, start, lexeme.span.end, None)
                    {
                        ranges.push(range);
                    }
                }
                LexemeType::Token(TokenType::Eof) => break,
                _ => {}
            }
        }

        ranges.sort_by_key(|range| (range.start_line, range.start_character.unwrap_or(0)));
        Ok(ranges)
    }

    #[cfg(test)]
    pub fn selection_ranges(
        &self,
        uri: &str,
        positions: Vec<impl IntoIdePosition>,
    ) -> Result<Vec<IdeSelectionRange>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.selection_ranges_in_snapshot(&snapshot, uri, positions)
    }

    pub fn selection_ranges_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        positions: Vec<impl IntoIdePosition>,
    ) -> Result<Vec<IdeSelectionRange>, String> {
        let positions = positions
            .into_iter()
            .map(IntoIdePosition::into_ide_position)
            .collect::<Vec<IdePosition>>();
        snapshot.check_canceled()?;
        let file = snapshot.document_source_file(uri).ok_or_else(|| {
            "requested selection ranges for a document that is not open".to_string()
        })?;
        let delimiter_spans = delimiter_spans(&file);

        positions
            .iter()
            .map(|position| selection_range_for_position(&file, &delimiter_spans, position))
            .collect()
    }

    #[cfg(test)]
    pub fn document_links(&self, uri: &str) -> Result<Vec<IdeDocumentLink>, String> {
        let snapshot = self.snapshot(Vec::new(), CancellationToken::new());
        self.document_links_in_snapshot(&snapshot, uri)
    }

    pub fn document_links_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeDocumentLink>, String> {
        snapshot.check_canceled()?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested document links for a document that is not open".to_string());
        };
        if target_doc.path.file_name().and_then(|name| name.to_str()) == Some("Craft.toml") {
            return self.manifest_document_links(snapshot, target_doc);
        }

        let context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let target_path = normalize_path(&target_doc.path);

        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                Arc::clone(structure)
            } else {
                let Some(structure) = context
                    .driver
                    .analyze_structure_cancelable(
                        &context.resolved.input_file.to_string_lossy(),
                        &context.dirty_documents.overrides,
                        &snapshot.cancellation,
                    )
                    .map_err(|_| "request was canceled".to_string())?
                else {
                    return Ok(Vec::new());
                };
                let structure = Arc::new(structure);
                self.prune_cache_family_for_insert(&context.cache_key);
                self.structure_cache
                    .lock()
                    .unwrap()
                    .insert(context.cache_key.clone(), Arc::clone(&structure));
                structure
            };

        let mut links = Vec::new();
        for link in structure.document_links(&target_path) {
            let Some(target_uri) = file_path_to_uri(&link.target_path).ok() else {
                continue;
            };
            links.push(IdeDocumentLink {
                range: span_to_range(structure.session(), link.origin_span).into(),
                target: target_uri,
            });
        }
        links.sort_by_key(|link| (link.range.start.line, link.range.start.character));
        Ok(links)
    }

    fn manifest_document_links(
        &self,
        snapshot: &AnalysisSnapshot,
        document: &OpenDocument,
    ) -> Result<Vec<IdeDocumentLink>, String> {
        snapshot.check_canceled()?;
        let manifest_path = normalize_path(&document.path);
        let workspace_manifest_path = match resolve_project_manifest_path(Some(&manifest_path)) {
            Ok(path) => normalize_path(&path),
            Err(CraftError::ManifestNotFound { .. }) => manifest_path.clone(),
            Err(err) => {
                return Err(format!(
                    "failed to resolve Craft project for manifest links: {err}"
                ));
            }
        };
        let workspace_manifest_source = if workspace_manifest_path == manifest_path {
            Some(document.text.as_str())
        } else {
            None
        };
        let workspace_dependencies = manifest_dependency_entries_for_path(
            &workspace_manifest_path,
            workspace_manifest_source,
        );

        let mut links = Vec::new();
        for entry in manifest_dependency_entries(&document.text) {
            snapshot.check_canceled()?;
            let Some(target_path) = manifest_dependency_target(
                &manifest_path,
                &workspace_manifest_path,
                &workspace_dependencies,
                &entry,
            ) else {
                continue;
            };
            let Some(target_uri) = file_path_to_uri(&target_path).ok() else {
                continue;
            };
            links.push(IdeDocumentLink {
                range: entry.range.into(),
                target: target_uri,
            });
        }
        links.sort_by_key(|link| (link.range.start.line, link.range.start.character));
        links.dedup();
        Ok(links)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestDependencySection {
    Dependencies,
    DevDependencies,
    BuildDependencies,
    WorkspaceDependencies,
}

#[derive(Debug, Clone)]
struct ManifestDependencyEntry {
    section: ManifestDependencySection,
    name: String,
    raw_value: String,
    range: IdeRange,
}

#[derive(Debug)]
struct PendingManifestDependencyEntry {
    section: ManifestDependencySection,
    name: String,
    raw_value: String,
    range: IdeRange,
    balance: ManifestValueBalance,
}

#[derive(Debug, Default)]
struct ManifestValueBalance {
    brace_depth: usize,
    bracket_depth: usize,
    in_string: bool,
    escape: bool,
    invalid: bool,
}

impl ManifestValueBalance {
    fn scan(&mut self, input: &str) {
        for ch in input.chars() {
            if self.escape {
                self.escape = false;
                continue;
            }
            if self.in_string {
                match ch {
                    '\\' => self.escape = true,
                    '"' => self.in_string = false,
                    _ => {}
                }
                continue;
            }
            match ch {
                '"' => self.in_string = true,
                '{' => self.brace_depth += 1,
                '}' => {
                    if let Some(depth) = self.brace_depth.checked_sub(1) {
                        self.brace_depth = depth;
                    } else {
                        self.invalid = true;
                    }
                }
                '[' => self.bracket_depth += 1,
                ']' => {
                    if let Some(depth) = self.bracket_depth.checked_sub(1) {
                        self.bracket_depth = depth;
                    } else {
                        self.invalid = true;
                    }
                }
                _ => {}
            }
        }
    }

    fn is_complete(&self) -> bool {
        !self.invalid && !self.in_string && self.brace_depth == 0 && self.bracket_depth == 0
    }
}

fn manifest_dependency_entries_for_path(
    manifest_path: &Path,
    open_source: Option<&str>,
) -> Vec<ManifestDependencyEntry> {
    let source = match open_source {
        Some(source) => source.to_string(),
        None => match fs::read_to_string(manifest_path) {
            Ok(source) => source,
            Err(_) => return Vec::new(),
        },
    };
    manifest_dependency_entries(&source)
}

fn manifest_dependency_entries(source: &str) -> Vec<ManifestDependencyEntry> {
    let mut entries = Vec::new();
    let mut section = None;
    let mut pending = None::<PendingManifestDependencyEntry>;

    for (line_index, raw_line) in source.lines().enumerate() {
        let Some(stripped) = strip_manifest_comment(raw_line) else {
            pending = None;
            continue;
        };
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(current) = pending.as_mut() {
            current.raw_value.push(' ');
            current.raw_value.push_str(trimmed);
            current.balance.scan(trimmed);
            if current.balance.is_complete()
                && let Some(completed) = pending.take()
            {
                entries.push(completed.into_entry());
            }
            continue;
        }

        if let Some(next_section) = manifest_section(trimmed) {
            section = next_section;
            continue;
        }

        let Some(section) = section else {
            continue;
        };
        let Some(eq_index) = top_level_equals(&stripped) else {
            continue;
        };
        let Some((name, start_character, end_character)) = manifest_key_range(&stripped, eq_index)
        else {
            continue;
        };
        let raw_value = stripped[eq_index + 1..].trim().to_string();
        let mut balance = ManifestValueBalance::default();
        balance.scan(&raw_value);
        let pending_entry = PendingManifestDependencyEntry {
            section,
            name,
            raw_value,
            range: IdeRange {
                start: IdePosition {
                    line: line_index as u32,
                    character: start_character as u32,
                },
                end: IdePosition {
                    line: line_index as u32,
                    character: end_character as u32,
                },
            },
            balance,
        };
        if pending_entry.balance.is_complete() {
            entries.push(pending_entry.into_entry());
        } else {
            pending = Some(pending_entry);
        }
    }

    entries
}

impl PendingManifestDependencyEntry {
    fn into_entry(self) -> ManifestDependencyEntry {
        ManifestDependencyEntry {
            section: self.section,
            name: self.name,
            raw_value: self.raw_value,
            range: self.range,
        }
    }
}

fn manifest_section(trimmed_line: &str) -> Option<Option<ManifestDependencySection>> {
    match trimmed_line {
        "[dependencies]" => Some(Some(ManifestDependencySection::Dependencies)),
        "[dev-dependencies]" => Some(Some(ManifestDependencySection::DevDependencies)),
        "[build-dependencies]" => Some(Some(ManifestDependencySection::BuildDependencies)),
        "[workspace.dependencies]" => Some(Some(ManifestDependencySection::WorkspaceDependencies)),
        line if line.starts_with('[') => Some(None),
        _ => None,
    }
}

fn strip_manifest_comment(line: &str) -> Option<String> {
    let mut out = String::new();
    let mut in_string = false;
    let mut escape = false;

    for ch in line.chars() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                out.push(ch);
                escape = true;
            }
            '"' => {
                out.push(ch);
                in_string = !in_string;
            }
            '#' if !in_string => break,
            _ => out.push(ch),
        }
    }

    (!in_string).then_some(out)
}

fn top_level_equals(line: &str) -> Option<usize> {
    let mut balance = ManifestValueBalance::default();
    for (index, ch) in line.char_indices() {
        if balance.escape {
            balance.escape = false;
            continue;
        }
        if balance.in_string {
            match ch {
                '\\' => balance.escape = true,
                '"' => balance.in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => balance.in_string = true,
            '{' => balance.brace_depth += 1,
            '}' => balance.brace_depth = balance.brace_depth.saturating_sub(1),
            '[' => balance.bracket_depth += 1,
            ']' => balance.bracket_depth = balance.bracket_depth.saturating_sub(1),
            '=' if balance.brace_depth == 0 && balance.bracket_depth == 0 => return Some(index),
            _ => {}
        }
    }
    None
}

fn manifest_key_range(line: &str, eq_index: usize) -> Option<(String, usize, usize)> {
    let raw_key = &line[..eq_index];
    let start = raw_key.find(|ch: char| !ch.is_whitespace())?;
    let end = raw_key.rfind(|ch: char| !ch.is_whitespace())? + 1;
    let key = raw_key[start..end].trim();
    if key.is_empty() || key.contains('.') {
        return None;
    }
    let name = key
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(key)
        .to_string();
    Some((name, start, end))
}

fn manifest_dependency_target(
    manifest_path: &Path,
    workspace_manifest_path: &Path,
    workspace_dependencies: &[ManifestDependencyEntry],
    entry: &ManifestDependencyEntry,
) -> Option<PathBuf> {
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_dir = workspace_manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let target_dir = if let Some(path) = dependency_path_value(&entry.raw_value) {
        let base = if entry.section == ManifestDependencySection::WorkspaceDependencies {
            workspace_dir
        } else {
            manifest_dir
        };
        base.join(path)
    } else if dependency_workspace_value(&entry.raw_value) == Some(true) {
        let workspace_entry = workspace_dependencies.iter().find(|candidate| {
            candidate.section == ManifestDependencySection::WorkspaceDependencies
                && candidate.name == entry.name
        })?;
        workspace_dir.join(dependency_path_value(&workspace_entry.raw_value)?)
    } else {
        return None;
    };

    let target_manifest = normalize_path(&target_dir.join("Craft.toml"));
    if !target_manifest.is_file() {
        return None;
    }
    if craft::manifest::Manifest::load(&target_manifest).is_err() {
        return None;
    }
    Some(target_manifest)
}

fn dependency_path_value(raw_value: &str) -> Option<PathBuf> {
    let fields = inline_table_fields(raw_value)?;
    parse_manifest_string(fields.get("path")?).map(PathBuf::from)
}

fn dependency_workspace_value(raw_value: &str) -> Option<bool> {
    let fields = inline_table_fields(raw_value)?;
    match fields.get("workspace")?.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn inline_table_fields(raw_value: &str) -> Option<BTreeMap<String, String>> {
    let trimmed = raw_value.trim();
    let inner = trimmed.strip_prefix('{')?.strip_suffix('}')?;
    let mut fields = BTreeMap::new();
    for field in split_top_level_commas(inner) {
        let eq_index = top_level_equals(&field)?;
        let key = field[..eq_index].trim();
        if key.is_empty() {
            continue;
        }
        fields.insert(key.to_string(), field[eq_index + 1..].trim().to_string());
    }
    Some(fields)
}

fn split_top_level_commas(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut balance = ManifestValueBalance::default();

    for (index, ch) in input.char_indices() {
        if balance.escape {
            balance.escape = false;
            continue;
        }
        if balance.in_string {
            match ch {
                '\\' => balance.escape = true,
                '"' => balance.in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => balance.in_string = true,
            '{' => balance.brace_depth += 1,
            '}' => balance.brace_depth = balance.brace_depth.saturating_sub(1),
            '[' => balance.bracket_depth += 1,
            ']' => balance.bracket_depth = balance.bracket_depth.saturating_sub(1),
            ',' if balance.brace_depth == 0 && balance.bracket_depth == 0 => {
                parts.push(input[start..index].trim().to_string());
                start = index + 1;
            }
            _ => {}
        }
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail.to_string());
    }
    parts
}

fn parse_manifest_string(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            _ => return None,
        }
    }
    Some(out)
}

fn folding_range_for_span(
    file: &SourceFile,
    start: usize,
    end: usize,
    kind: Option<IdeFoldingRangeKind>,
) -> Option<IdeFoldingRange> {
    if start >= end {
        return None;
    }
    let start_position = byte_offset_to_position(file, start);
    let end_position = byte_offset_to_position(file, end);
    if start_position.line >= end_position.line {
        return None;
    }

    Some(IdeFoldingRange {
        start_line: start_position.line,
        start_character: Some(start_position.character),
        end_line: end_position.line,
        end_character: Some(end_position.character),
        kind,
    })
}

fn selection_range_for_position(
    file: &SourceFile,
    delimiter_spans: &[DelimiterSpan],
    position: &IdePosition,
) -> Result<IdeSelectionRange, String> {
    let Some(offset) = position_to_byte_offset(file, position) else {
        return Err(format!(
            "requested selection range for invalid position {}:{}",
            position.line, position.character
        ));
    };

    let mut ranges = Vec::new();
    if let Some(range) = token_range_at_offset(file, offset) {
        ranges.push(range);
    }
    if let Some(range) = line_range_at_offset(file, offset) {
        ranges.push(range);
    }
    for span in delimiter_spans
        .iter()
        .filter(|span| offset >= span.start && offset <= span.end)
    {
        ranges.push(IdeRange {
            start: byte_offset_to_position(file, span.start),
            end: byte_offset_to_position(file, span.end),
        });
    }
    if !file.src.is_empty() {
        ranges.push(IdeRange {
            start: IdePosition {
                line: 0,
                character: 0,
            },
            end: byte_offset_to_position(file, file.src.len()),
        });
    }

    ranges.sort_by_key(range_extent_key);
    ranges.dedup();
    let mut parent = None;
    for range in ranges.into_iter().rev() {
        parent = Some(Box::new(IdeSelectionRange { range, parent }));
    }
    parent
        .map(|range| *range)
        .ok_or_else(|| "selection range could not be constructed".to_string())
}

fn delimiter_spans(file: &SourceFile) -> Vec<DelimiterSpan> {
    let mut spans = Vec::new();
    let mut stack = Vec::new();
    let mut tokenizer = Tokenizer::new(&file.src, FileId(0));

    loop {
        let lexeme = tokenizer.next_lexeme();
        match lexeme.tag {
            LexemeType::Token(
                tag @ (TokenType::LBrace
                | TokenType::DotLBrace
                | TokenType::LParen
                | TokenType::LBracket),
            ) => stack.push((tag, lexeme.span.start)),
            LexemeType::Token(TokenType::RBrace | TokenType::RParen | TokenType::RBracket) => {
                let Some(open_index) = stack
                    .iter()
                    .rposition(|(open, _)| delimiters_match(*open, lexeme.tag))
                else {
                    continue;
                };
                let (_, start) = stack.remove(open_index);
                spans.push(DelimiterSpan {
                    start,
                    end: lexeme.span.end,
                });
                stack.truncate(open_index);
            }
            LexemeType::Token(TokenType::Eof) => break,
            _ => {}
        }
    }

    spans.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
    spans
}

fn delimiters_match(open: TokenType, close: LexemeType) -> bool {
    matches!(
        (open, close),
        (
            TokenType::LBrace | TokenType::DotLBrace,
            LexemeType::Token(TokenType::RBrace)
        ) | (TokenType::LParen, LexemeType::Token(TokenType::RParen))
            | (TokenType::LBracket, LexemeType::Token(TokenType::RBracket))
    )
}

fn token_range_at_offset(file: &SourceFile, offset: usize) -> Option<IdeRange> {
    let mut tokenizer = Tokenizer::new(&file.src, FileId(0));
    loop {
        let lexeme = tokenizer.next_lexeme();
        match lexeme.tag {
            LexemeType::Token(TokenType::Eof) => return None,
            LexemeType::Whitespace => {}
            _ => {
                if offset >= lexeme.span.start && offset < lexeme.span.end {
                    return Some(IdeRange {
                        start: byte_offset_to_position(file, lexeme.span.start),
                        end: byte_offset_to_position(file, lexeme.span.end),
                    });
                }
            }
        }
    }
}

fn line_range_at_offset(file: &SourceFile, offset: usize) -> Option<IdeRange> {
    let line = file.lookup_line(offset).saturating_sub(1);
    let line_start = *file.line_starts.get(line)?;
    let next_line_start = file
        .line_starts
        .get(line + 1)
        .copied()
        .unwrap_or(file.src.len());
    let line_end = trim_line_ending(&file.src, line_start, next_line_start);
    (line_start < line_end).then(|| IdeRange {
        start: byte_offset_to_position(file, line_start),
        end: byte_offset_to_position(file, line_end),
    })
}

fn range_extent_key(range: &IdeRange) -> (u32, u32, u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
        range.start.line,
        range.start.character,
    )
}
