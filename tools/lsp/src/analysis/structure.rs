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
        let snapshot = self.snapshot(None, CancellationToken::new());
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
        positions: Vec<Position>,
    ) -> Result<Vec<IdeSelectionRange>, String> {
        let snapshot = self.snapshot(None, CancellationToken::new());
        self.selection_ranges_in_snapshot(&snapshot, uri, positions)
    }

    pub fn selection_ranges_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        positions: Vec<Position>,
    ) -> Result<Vec<IdeSelectionRange>, String> {
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
        let snapshot = self.snapshot(None, CancellationToken::new());
        self.document_links_in_snapshot(&snapshot, uri)
    }

    pub fn document_links_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeDocumentLink>, String> {
        snapshot.check_canceled()?;
        let context = self.resolve_analysis_context_for_snapshot(snapshot, uri)?;
        let Some(target_doc) = snapshot.document(uri) else {
            return Err("requested document links for a document that is not open".to_string());
        };
        let target_path = normalize_path(&target_doc.path);

        let structure =
            if let Some(structure) = self.structure_cache.lock().unwrap().get(&context.cache_key) {
                Arc::clone(structure)
            } else {
                let Some(structure) = context.driver.analyze_structure(
                    &context.resolved.input_file.to_string_lossy(),
                    &context.dirty_documents.overrides,
                ) else {
                    return Err("document link analysis failed".to_string());
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
                range: span_to_range(structure.session(), link.origin_span),
                target: target_uri,
            });
        }
        links.sort_by_key(|link| (link.range.start.line, link.range.start.character));
        Ok(links)
    }
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
    position: &Position,
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
        ranges.push(Range {
            start: byte_offset_to_position(file, span.start),
            end: byte_offset_to_position(file, span.end),
        });
    }
    if !file.src.is_empty() {
        ranges.push(Range {
            start: Position {
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

fn token_range_at_offset(file: &SourceFile, offset: usize) -> Option<Range> {
    let mut tokenizer = Tokenizer::new(&file.src, FileId(0));
    loop {
        let lexeme = tokenizer.next_lexeme();
        match lexeme.tag {
            LexemeType::Token(TokenType::Eof) => return None,
            LexemeType::Whitespace => {}
            _ => {
                if offset >= lexeme.span.start && offset < lexeme.span.end {
                    return Some(Range {
                        start: byte_offset_to_position(file, lexeme.span.start),
                        end: byte_offset_to_position(file, lexeme.span.end),
                    });
                }
            }
        }
    }
}

fn line_range_at_offset(file: &SourceFile, offset: usize) -> Option<Range> {
    let line = file.lookup_line(offset).saturating_sub(1);
    let line_start = *file.line_starts.get(line)?;
    let next_line_start = file
        .line_starts
        .get(line + 1)
        .copied()
        .unwrap_or(file.src.len());
    let line_end = trim_line_ending(&file.src, line_start, next_line_start);
    (line_start < line_end).then(|| Range {
        start: byte_offset_to_position(file, line_start),
        end: byte_offset_to_position(file, line_end),
    })
}

fn range_extent_key(range: &Range) -> (u32, u32, u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range.end.character.saturating_sub(range.start.character),
        range.start.line,
        range.start.character,
    )
}
