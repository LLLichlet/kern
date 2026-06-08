//! Formatting query integration for LSP requests.
//!
//! Formatting delegates to Craft's formatter and converts whole-document or
//! range-limited edits into IDE text edits.

use super::*;
use craft::fmt::{FormatConfig, format_source_text_with_config};
use craft::manifest::Manifest;

impl AnalysisEngine {
    pub fn formatting_edits_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
    ) -> Result<Vec<IdeTextEdit>, String> {
        snapshot.check_canceled()?;
        let document = snapshot
            .document(uri)
            .ok_or_else(|| "requested formatting for a document that is not open".to_string())?;
        let config = self.format_config_for_document(&document.path)?;
        Ok(format_text_edits(&document.path, &document.text, &config))
    }

    pub fn range_formatting_edits_in_snapshot(
        &self,
        snapshot: &AnalysisSnapshot,
        uri: &str,
        requested_range: impl IntoIdeRange,
    ) -> Result<Vec<IdeTextEdit>, String> {
        let requested_range = requested_range.into_ide_range();
        snapshot.check_canceled()?;
        let document = snapshot.document(uri).ok_or_else(|| {
            "requested range formatting for a document that is not open".to_string()
        })?;
        let file = SourceFile::new(document.path.clone(), document.text.clone());
        let Some(range_start) = position_to_byte_offset(&file, &requested_range.start) else {
            return Err(format!(
                "requested range formatting with invalid start position {}:{}",
                requested_range.start.line, requested_range.start.character
            ));
        };
        let Some(range_end) = position_to_byte_offset(&file, &requested_range.end) else {
            return Err(format!(
                "requested range formatting with invalid end position {}:{}",
                requested_range.end.line, requested_range.end.character
            ));
        };
        if range_start > range_end {
            return Err("requested range formatting with a reversed range".to_string());
        }

        let config = self.format_config_for_document(&document.path)?;
        Ok(format_text_edits(&document.path, &document.text, &config)
            .into_iter()
            .filter(|edit| {
                edit_range_intersects_requested_range(&file, edit, range_start, range_end)
            })
            .collect())
    }

    fn format_config_for_document(&self, path: &Path) -> Result<FormatConfig, String> {
        let Some(project) = self.project_for_path(path)? else {
            return Ok(FormatConfig::default());
        };
        let manifest = Manifest::load(project.manifest_path()).map_err(|err| {
            format!(
                "failed to load Craft manifest `{}` for LSP formatting: {err}",
                project.manifest_path().display()
            )
        })?;
        Ok(FormatConfig::from_manifest(&manifest))
    }
}

fn format_text_edits(path: &Path, source: &str, config: &FormatConfig) -> Vec<IdeTextEdit> {
    if source.is_empty() {
        return Vec::new();
    }

    let file = SourceFile::new(path.to_path_buf(), source.to_string());
    source_lines_for_formatter(source)
        .into_iter()
        .filter_map(|line| {
            let formatted = format_source_text_with_config(&format!("{}\n", line.body), config);
            if line.text == formatted {
                return None;
            }

            let prefix_len = common_prefix_len(line.text, &formatted);
            let source_suffix_start = common_suffix_start(line.text, &formatted, prefix_len);
            let formatted_suffix_start = common_suffix_start(&formatted, line.text, prefix_len);
            Some(IdeTextEdit {
                range: IdeRange {
                    start: byte_offset_to_position(&file, line.start + prefix_len),
                    end: byte_offset_to_position(&file, line.start + source_suffix_start),
                },
                new_text: formatted[prefix_len..formatted_suffix_start].to_string(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct SourceFormatLine<'a> {
    start: usize,
    text: &'a str,
    body: &'a str,
}

fn source_lines_for_formatter(source: &str) -> Vec<SourceFormatLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0;
    for text in source.split_inclusive('\n') {
        lines.push(SourceFormatLine {
            start,
            text,
            body: line_body(text),
        });
        start += text.len();
    }
    if start < source.len() {
        let text = &source[start..];
        lines.push(SourceFormatLine {
            start,
            text,
            body: line_body(text),
        });
    }
    lines
}

fn line_body(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    let mut prefix = 0;
    for ((left_index, left_char), (_, right_char)) in left.char_indices().zip(right.char_indices())
    {
        if left_char != right_char {
            break;
        }
        prefix = left_index + left_char.len_utf8();
    }
    prefix
}

fn common_suffix_start(source: &str, other: &str, prefix_len: usize) -> usize {
    let mut source_suffix_start = source.len();
    let mut source_chars = source[prefix_len..].char_indices().rev();
    let mut other_chars = other[prefix_len..].char_indices().rev();

    while let (Some((source_index, source_char)), Some((_, other_char))) =
        (source_chars.next(), other_chars.next())
    {
        if source_char != other_char {
            break;
        }
        source_suffix_start = prefix_len + source_index;
    }

    source_suffix_start
}

fn edit_range_intersects_requested_range(
    file: &SourceFile,
    edit: &IdeTextEdit,
    requested_start: usize,
    requested_end: usize,
) -> bool {
    let Some(edit_start) = position_to_byte_offset(file, &edit.range.start) else {
        return false;
    };
    let Some(edit_end) = position_to_byte_offset(file, &edit.range.end) else {
        return false;
    };

    edit_start <= requested_end && requested_start <= edit_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_document_formatting_returns_minimal_edit() {
        let edits = format_text_edits(
            Path::new("/tmp/main.kn"),
            "fn main() void {  \n}\t",
            &FormatConfig::default(),
        );

        assert_eq!(edits.len(), 2);
        assert_eq!(
            edits[0].range,
            IdeRange {
                start: IdePosition {
                    line: 0,
                    character: 16,
                },
                end: IdePosition {
                    line: 0,
                    character: 18,
                },
            }
        );
        assert_eq!(edits[0].new_text, "");
        assert_eq!(
            edits[1].range,
            IdeRange {
                start: IdePosition {
                    line: 1,
                    character: 1,
                },
                end: IdePosition {
                    line: 1,
                    character: 2,
                },
            }
        );
        assert_eq!(edits[1].new_text, "\n");
    }

    #[test]
    fn format_edits_keep_separate_line_hunks() {
        let edits = format_text_edits(
            Path::new("/tmp/main.kn"),
            "fn first() void {  \n}\nfn second() void {  \n}\n",
            &FormatConfig::default(),
        );

        assert_eq!(edits.len(), 2);
        assert_eq!(
            edits[0].range.start,
            IdePosition {
                line: 0,
                character: 17,
            }
        );
        assert_eq!(
            edits[1].range.start,
            IdePosition {
                line: 2,
                character: 18,
            }
        );
    }
}
