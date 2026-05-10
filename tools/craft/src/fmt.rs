use crate::error::{Error, Result};
use crate::manifest::Manifest;
use crate::workspace::WorkspaceMember;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    Write,
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatSummary {
    pub packages: usize,
    pub files: usize,
    pub changed_files: usize,
    pub diagnostics: usize,
}

impl FormatSummary {
    pub fn merge(&mut self, other: &Self) {
        self.packages += other.packages;
        self.files += other.files;
        self.changed_files += other.changed_files;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFormatSummary {
    pub label: String,
    pub root: PathBuf,
    pub summary: FormatSummary,
    pub changed_paths: Vec<PathBuf>,
    pub diagnostics: Vec<FormatDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatDiagnostic {
    pub path: PathBuf,
    pub line: usize,
    pub width: usize,
    pub limit: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatConfig {
    pub line_width: usize,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self { line_width: 100 }
    }
}

impl FormatConfig {
    fn from_manifest(manifest: &Manifest) -> Self {
        let mut config = Self::default();
        if let Some(line_width) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.line_width)
        {
            config.line_width = line_width;
        }
        config
    }
}

pub fn format_workspace_sources(
    manifest_path: &Path,
    manifest: &Manifest,
    members: &[WorkspaceMember],
    mode: FormatMode,
) -> Result<Vec<PackageFormatSummary>> {
    let mut summaries = Vec::new();
    summaries.push(format_package_sources(manifest_path, manifest, mode)?);
    for member in members {
        summaries.push(format_package_sources(
            &member.manifest_path,
            &member.manifest,
            mode,
        )?);
    }
    summaries.sort_by(|lhs, rhs| lhs.label.cmp(&rhs.label).then(lhs.root.cmp(&rhs.root)));
    Ok(summaries)
}

fn format_package_sources(
    manifest_path: &Path,
    manifest: &Manifest,
    mode: FormatMode,
) -> Result<PackageFormatSummary> {
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut summary = FormatSummary {
        packages: 1,
        ..FormatSummary::default()
    };
    let config = FormatConfig::from_manifest(manifest);
    let mut changed_paths = Vec::new();
    let mut diagnostics = Vec::new();

    for path in kern_source_files(root)? {
        let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
        let formatted = format_source_text_with_config(&source, config);
        summary.files += 1;
        if formatted != source {
            summary.changed_files += 1;
            changed_paths.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            if mode == FormatMode::Write {
                fs::write(&path, &formatted).map_err(|err| Error::from_io(&path, err))?;
            }
        }

        for diagnostic in collect_format_diagnostics(root, &path, &formatted, config) {
            diagnostics.push(diagnostic);
            summary.diagnostics += 1;
        }
    }

    Ok(PackageFormatSummary {
        label: package_label(manifest),
        root: root.to_path_buf(),
        summary,
        changed_paths,
        diagnostics,
    })
}

fn kern_source_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_kern_source_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_kern_source_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|err| Error::from_io(dir, err))?;
    for entry in entries {
        let entry = entry.map_err(Error::from_io_plain)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(Error::from_io_plain)?;
        if file_type.is_dir() {
            if is_skipped_dir(&path) {
                continue;
            }
            collect_kern_source_files(&path, files)?;
            continue;
        }
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rn") {
            files.push(path);
        }
    }
    Ok(())
}

fn is_skipped_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".craft" | ".git" | "target" | ".idea" | ".vscode")
    )
}

fn collect_format_diagnostics(
    root: &Path,
    path: &Path,
    source: &str,
    config: FormatConfig,
) -> Vec<FormatDiagnostic> {
    let mut diagnostics = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let width = line_width(line);
        if width <= config.line_width {
            continue;
        }
        diagnostics.push(FormatDiagnostic {
            path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
            line: line_index + 1,
            width,
            limit: config.line_width,
            message: "line exceeds [craft.fmt].line-width and craft fmt could not split it automatically; split the expression or raise [craft.fmt].line-width".to_string(),
        });
    }
    diagnostics
}

pub fn format_source_text(source: &str) -> String {
    format_source_text_with_config(source, FormatConfig::default())
}

fn format_source_text_with_config(source: &str, config: FormatConfig) -> String {
    format_source_text_with_config_inner(source, config)
}

fn format_source_text_with_config_inner(source: &str, config: FormatConfig) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed_end = line.trim_end_matches([' ', '\t']);
        let formatted_line = format_line(trimmed_end, config);
        out.push_str(&formatted_line);
        out.push('\n');
    }
    if source.is_empty() {
        return String::new();
    }
    out
}

fn format_line(line: &str, config: FormatConfig) -> String {
    format_postfix_chain(
        &format_boolean_chain(&format_grouped_use(line, config), config),
        config,
    )
}

fn format_grouped_use(line: &str, config: FormatConfig) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if !trimmed.starts_with("use ") || !trimmed.ends_with(';') {
        return line.to_string();
    }
    let Some(open) = trimmed.find('{') else {
        return line.to_string();
    };
    let Some(close) = trimmed.rfind('}') else {
        return line.to_string();
    };
    if close < open || trimmed[open + 1..close].contains('{') {
        return line.to_string();
    }
    let items = trimmed[open + 1..close]
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if items.len() < 5 && line_width(line) <= config.line_width {
        return line.to_string();
    }

    let prefix = trimmed[..open + 1].trim_end();
    let suffix = trimmed[close..].trim_start();
    let mut out = String::new();
    out.push_str(indent);
    out.push_str(prefix);
    out.push('\n');
    for item in items {
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(item);
        out.push_str(",\n");
    }
    out.push_str(indent);
    out.push_str(suffix);
    out
}

fn format_boolean_chain(line: &str, config: FormatConfig) -> String {
    if line_width(line) <= config.line_width {
        return line.to_string();
    }
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if trimmed.contains("//") || trimmed.contains("/*") {
        return line.to_string();
    }
    let Some((head, operator)) = boolean_chain_head(trimmed) else {
        return line.to_string();
    };
    if !trimmed.ends_with(';') {
        return line.to_string();
    }
    let body = trimmed[head.len()..trimmed.len() - 1].trim();
    let parts = split_boolean_parts(body, operator);
    if parts.len() < 3 {
        return line.to_string();
    }

    let mut out = String::new();
    out.push_str(indent);
    out.push_str(head);
    out.push_str(parts[0]);
    for part in parts.iter().skip(1) {
        out.push('\n');
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(operator);
        out.push(' ');
        out.push_str(part);
    }
    out.push(';');
    out
}

fn format_postfix_chain(line: &str, config: FormatConfig) -> String {
    if line_width(line) <= config.line_width
        || line.contains('\n')
        || line.contains("//")
        || line.contains("/*")
    {
        return line.to_string();
    }

    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if !trimmed.ends_with(';') {
        return line.to_string();
    }

    let statement = trimmed[..trimmed.len() - 1].trim_end();
    let (prefix, expression) = split_statement_prefix(statement);
    let Some(chain) = parse_postfix_chain(expression) else {
        return line.to_string();
    };
    if chain.segments.len() < 2 {
        return line.to_string();
    }

    let mut out = String::new();
    out.push_str(indent);
    out.push_str(prefix);
    out.push_str(chain.receiver);
    for segment in chain.segments {
        out.push('\n');
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(segment);
    }
    out.push(';');
    out
}

fn boolean_chain_head(trimmed: &str) -> Option<(&str, &str)> {
    if trimmed.starts_with("return ") {
        if trimmed.contains(" or ") {
            return Some(("return ", "or"));
        }
        if trimmed.contains(" and ") {
            return Some(("return ", "and"));
        }
    }
    None
}

fn split_boolean_parts<'a>(body: &'a str, operator: &str) -> Vec<&'a str> {
    let delimiter = format!(" {operator} ");
    body.split(&delimiter)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

struct PostfixChain<'a> {
    receiver: &'a str,
    segments: Vec<&'a str>,
}

fn split_statement_prefix(statement: &str) -> (&str, &str) {
    if let Some(expression) = statement.strip_prefix("return ") {
        return ("return ", expression.trim_start());
    }

    if (statement.starts_with("let ") || statement.starts_with("const "))
        && let Some(eq) = find_top_level_assignment(statement)
    {
        let mut expression_start = eq + 1;
        while expression_start < statement.len()
            && statement.as_bytes()[expression_start].is_ascii_whitespace()
        {
            expression_start += 1;
        }
        return (
            &statement[..expression_start],
            &statement[expression_start..],
        );
    }

    ("", statement)
}

fn find_top_level_assignment(input: &str) -> Option<usize> {
    let mut scanner = Scanner::default();
    for (index, ch) in input.char_indices() {
        if !scanner.scan(index, ch) || !scanner.is_top_level() || ch != '=' {
            continue;
        }

        let prev = previous_byte(input, index);
        let next = input[index + 1..].as_bytes().first().copied();
        if matches!(prev, Some(b'=' | b'!' | b'<' | b'>' | b'-')) || next == Some(b'=') {
            continue;
        }
        return Some(index);
    }
    None
}

fn parse_postfix_chain(input: &str) -> Option<PostfixChain<'_>> {
    let mut segments = Vec::new();
    let mut scanner = Scanner::default();
    let mut first_segment_start = None;
    let mut index = 0usize;

    while index < input.len() {
        let ch = input[index..].chars().next()?;
        if !scanner.scan(index, ch) {
            index += ch.len_utf8();
            continue;
        }

        if scanner.is_top_level()
            && let Some(operator_len) = postfix_operator_len(&input[index..])
            && let Some(segment_end) = parse_postfix_segment(input, index, operator_len)
        {
            first_segment_start.get_or_insert(index);
            segments.push(input[index..segment_end].trim());
            index = segment_end;
            continue;
        }

        index += ch.len_utf8();
    }

    let first_segment_start = first_segment_start?;
    let receiver = input[..first_segment_start].trim_end();
    if receiver.is_empty() || segments.is_empty() {
        return None;
    }

    let tail_start = segments
        .last()
        .and_then(|segment| input.rfind(segment).map(|start| start + segment.len()))?;
    if !input[tail_start..].trim().is_empty() {
        return None;
    }

    Some(PostfixChain { receiver, segments })
}

fn postfix_operator_len(input: &str) -> Option<usize> {
    for operator in ["..&.", ".&.", "."] {
        if input.starts_with(operator) {
            return Some(operator.len());
        }
    }
    None
}

fn parse_postfix_segment(input: &str, operator_start: usize, operator_len: usize) -> Option<usize> {
    let ident_start = operator_start + operator_len;
    let ident_end = parse_ident(input, ident_start)?;
    if input[ident_end..].as_bytes().first().copied() != Some(b'(') {
        return None;
    }
    find_matching_call_end(input, ident_end)
}

fn parse_ident(input: &str, start: usize) -> Option<usize> {
    let mut chars = input[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }

    let mut end = start + first.len_utf8();
    for (offset, ch) in chars {
        if !is_ident_continue(ch) {
            return Some(start + offset);
        }
        end = start + offset + ch.len_utf8();
    }
    Some(end)
}

fn find_matching_call_end(input: &str, open: usize) -> Option<usize> {
    let mut scanner = Scanner::default();
    for (index, ch) in input[open..].char_indices() {
        let absolute = open + index;
        scanner.scan(absolute, ch);
        if absolute > open && scanner.paren_depth == 0 {
            return Some(absolute + ch.len_utf8());
        }
    }
    None
}

fn previous_byte(input: &str, index: usize) -> Option<u8> {
    input[..index]
        .as_bytes()
        .iter()
        .rev()
        .find(|byte| !byte.is_ascii_whitespace())
        .copied()
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn line_width(line: &str) -> usize {
    line.chars().count()
}

#[derive(Debug, Default)]
struct Scanner {
    paren_depth: usize,
    brace_depth: usize,
    bracket_depth: usize,
    in_string: bool,
    in_char: bool,
    escape: bool,
}

impl Scanner {
    fn is_top_level(&self) -> bool {
        self.paren_depth == 0
            && self.brace_depth == 0
            && self.bracket_depth == 0
            && !self.in_string
            && !self.in_char
            && !self.escape
    }

    fn scan(&mut self, index: usize, ch: char) -> bool {
        if self.escape {
            self.escape = false;
            return false;
        }

        if self.in_string {
            match ch {
                '\\' => self.escape = true,
                '"' => self.in_string = false,
                _ => {}
            }
            return false;
        }

        if self.in_char {
            match ch {
                '\\' => self.escape = true,
                '\'' => self.in_char = false,
                _ => {}
            }
            return false;
        }

        match ch {
            '"' => self.in_string = true,
            '\'' => self.in_char = true,
            '(' => self.paren_depth += 1,
            ')' => self.paren_depth = self.paren_depth.saturating_sub(1),
            '{' => self.brace_depth += 1,
            '}' => self.brace_depth = self.brace_depth.saturating_sub(1),
            '[' => self.bracket_depth += 1,
            ']' => self.bracket_depth = self.bracket_depth.saturating_sub(1),
            _ => return self.is_top_level_at(index),
        }
        false
    }

    fn is_top_level_at(&self, _index: usize) -> bool {
        self.is_top_level()
    }
}

fn package_label(manifest: &Manifest) -> String {
    manifest
        .package
        .as_ref()
        .map(|package| format!("{} {}", package.name, package.version))
        .unwrap_or_else(|| "<workspace>".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        FormatConfig, collect_format_diagnostics, format_source_text,
        format_source_text_with_config,
    };
    use std::path::Path;

    #[test]
    fn formats_trailing_whitespace_and_eof_newline() {
        assert_eq!(
            format_source_text("fn main() void {  \n}\t"),
            "fn main() void {\n}\n"
        );
    }

    #[test]
    fn preserves_empty_files() {
        assert_eq!(format_source_text(""), "");
    }

    #[test]
    fn expands_long_grouped_use_lists() {
        assert_eq!(
            format_source_text(
                "use .xml.{AlphaName, BetaName, GammaName, DeltaName, EpsilonName, ZetaName};\n"
            ),
            "use .xml.{\n    AlphaName,\n    BetaName,\n    GammaName,\n    DeltaName,\n    EpsilonName,\n    ZetaName,\n};\n"
        );
    }

    #[test]
    fn splits_long_boolean_return_chains() {
        assert_eq!(
            format_source_text(
                "pub/ fn is_name_start(byte: u8) bool {\n    return byte == b':' or byte == b'_' or (byte >= b'A' and byte <= b'Z') or (byte >= b'a' and byte <= b'z') or byte >= 0x80;\n}\n"
            ),
            "pub/ fn is_name_start(byte: u8) bool {\n    return byte == b':'\n        or byte == b'_'\n        or (byte >= b'A' and byte <= b'Z')\n        or (byte >= b'a' and byte <= b'z')\n        or byte >= 0x80;\n}\n"
        );
    }

    #[test]
    fn splits_long_postfix_chain_assignments() {
        assert_eq!(
            format_source_text_with_config(
                "fn test(t: &mut Test) void {\n    let duplicate_decl_attr_err = duplicate_decl_attr..&.next().should_err().sum(@loc(), t);\n}\n",
                FormatConfig { line_width: 80 },
            ),
            "fn test(t: &mut Test) void {\n    let duplicate_decl_attr_err = duplicate_decl_attr\n        ..&.next()\n        .should_err()\n        .sum(@loc(), t);\n}\n"
        );
    }

    #[test]
    fn splits_long_postfix_chain_expressions() {
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    \"hello, {}!\".fmt(.{\"kern\"}).println();\n}\n",
                FormatConfig { line_width: 40 },
            ),
            "fn main() void {\n    \"hello, {}!\"\n        .fmt(.{\"kern\"})\n        .println();\n}\n"
        );
    }

    #[test]
    fn leaves_short_postfix_chain_on_one_line() {
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    value.foo().bar();\n}\n",
                FormatConfig { line_width: 100 },
            ),
            "fn main() void {\n    value.foo().bar();\n}\n"
        );
    }

    #[test]
    fn leaves_commented_postfix_chain_on_one_line() {
        let source = "fn main() void {\n    value_with_a_long_name.foo().bar().baz(); // keep\n}\n";
        assert_eq!(
            format_source_text_with_config(source, FormatConfig { line_width: 40 }),
            source
        );
    }

    #[test]
    fn reports_unresolved_long_lines_after_formatting() {
        let formatted = format_source_text_with_config(
            "fn main() void {\n    let very_long_name = this_expression_is_still_too_long_even_after_the_formatter_runs;\n}\n",
            FormatConfig { line_width: 60 },
        );
        let diagnostics = collect_format_diagnostics(
            Path::new("."),
            Path::new("src/lib.rn"),
            &formatted,
            FormatConfig { line_width: 60 },
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, Path::new("src/lib.rn"));
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].limit, 60);
    }
}
