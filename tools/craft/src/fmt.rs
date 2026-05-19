//! Formatter for Kern source files managed by Craft.
//!
//! The formatter performs conservative whitespace, argument-list, postfix-chain,
//! boolean-chain, and line-width adjustments without needing a full semantic
//! analysis pass.

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
        self.diagnostics += other.diagnostics;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatConfig {
    pub line_width: usize,
    pub postfix_chain_threshold: usize,
    pub boolean_chain_threshold: usize,
    pub function_parameter_threshold: usize,
    pub call_argument_threshold: usize,
    pub exclude: Vec<String>,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            line_width: 100,
            postfix_chain_threshold: 3,
            boolean_chain_threshold: 3,
            function_parameter_threshold: 3,
            call_argument_threshold: 4,
            exclude: Vec::new(),
        }
    }
}

impl FormatConfig {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let mut config = Self::default();
        if let Some(line_width) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.line_width)
        {
            config.line_width = line_width;
        }
        if let Some(threshold) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.postfix_chain_threshold)
        {
            config.postfix_chain_threshold = threshold;
        }
        if let Some(threshold) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.boolean_chain_threshold)
        {
            config.boolean_chain_threshold = threshold;
        }
        if let Some(threshold) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.function_parameter_threshold)
        {
            config.function_parameter_threshold = threshold;
        }
        if let Some(threshold) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.fmt.as_ref())
            .and_then(|fmt| fmt.call_argument_threshold)
        {
            config.call_argument_threshold = threshold;
        }
        if let Some(fmt) = manifest.craft.as_ref().and_then(|craft| craft.fmt.as_ref()) {
            config.exclude = fmt.exclude.clone();
        }
        config
    }

    fn path_in_scope(&self, path: &Path) -> bool {
        let text = path.to_string_lossy();
        !self
            .exclude
            .iter()
            .any(|pattern| path_matches(&text, pattern))
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
        let display_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if !config.path_in_scope(&display_path) {
            continue;
        }
        let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
        let formatted = format_source_text_with_config(&source, &config);
        summary.files += 1;
        if formatted != source {
            summary.changed_files += 1;
            changed_paths.push(display_path);
            if mode == FormatMode::Write {
                fs::write(&path, &formatted).map_err(|err| Error::from_io(&path, err))?;
            }
        }

        for diagnostic in collect_format_diagnostics(root, &path, &formatted, &config) {
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
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("kn") {
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

fn path_matches(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn collect_format_diagnostics(
    root: &Path,
    path: &Path,
    source: &str,
    config: &FormatConfig,
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
    format_source_text_with_config(source, &FormatConfig::default())
}

pub fn format_source_text_with_config(source: &str, config: &FormatConfig) -> String {
    format_source_text_with_config_inner(source, config)
}

fn format_source_text_with_config_inner(source: &str, config: &FormatConfig) -> String {
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

fn format_line(line: &str, config: &FormatConfig) -> String {
    format_postfix_chain(
        &format_call_arguments(
            &format_function_parameters(
                &format_boolean_chain(&format_grouped_use(line, config), config),
                config,
            ),
            config,
        ),
        config,
    )
}

fn format_grouped_use(line: &str, config: &FormatConfig) -> String {
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

fn format_boolean_chain(line: &str, config: &FormatConfig) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if trimmed.contains("//") || trimmed.contains("/*") || !trimmed.ends_with(';') {
        return line.to_string();
    };
    let statement = trimmed[..trimmed.len() - 1].trim_end();
    let (prefix, expression) = split_statement_prefix(statement);
    let Some(chain) = parse_boolean_chain(expression) else {
        return line.to_string();
    };
    let segment_count = chain.parts.len();
    if segment_count < config.boolean_chain_threshold && line_width(line) <= config.line_width {
        return line.to_string();
    }

    let mut out = String::new();
    out.push_str(indent);
    out.push_str(prefix);
    out.push_str(chain.parts[0]);
    for part in chain.parts.iter().skip(1) {
        out.push('\n');
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(chain.operator);
        out.push(' ');
        out.push_str(part);
    }
    out.push(';');
    out
}

fn format_function_parameters(line: &str, config: &FormatConfig) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if line.contains('\n')
        || trimmed.contains("//")
        || trimmed.contains("/*")
        || !(trimmed.starts_with("fn ") || trimmed.starts_with("pub/ fn "))
    {
        return line.to_string();
    }

    let Some(open) = find_top_level_char(trimmed, '(') else {
        return line.to_string();
    };
    let Some(close) = find_matching_delimiter(trimmed, open, '(', ')') else {
        return line.to_string();
    };
    let params = split_top_level(trimmed[open + 1..close].trim(), ',');
    if params.len() < config.function_parameter_threshold && line_width(line) <= config.line_width {
        return line.to_string();
    }
    if params.is_empty() {
        return line.to_string();
    }

    let mut out = String::new();
    out.push_str(indent);
    out.push_str(trimmed[..open + 1].trim_end());
    out.push('\n');
    for param in params {
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(param);
        out.push_str(",\n");
    }
    out.push_str(indent);
    out.push(')');
    let suffix = trimmed[close + 1..].trim_start();
    if !suffix.is_empty() {
        out.push(' ');
        out.push_str(suffix);
    }
    out
}

fn format_call_arguments(line: &str, config: &FormatConfig) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();
    if line.contains('\n')
        || trimmed.contains("//")
        || trimmed.contains("/*")
        || !trimmed.ends_with(';')
        || trimmed.starts_with("fn ")
        || trimmed.starts_with("pub/ fn ")
    {
        return line.to_string();
    }

    let statement = trimmed[..trimmed.len() - 1].trim_end();
    let Some(call) = parse_call_arguments(statement) else {
        return line.to_string();
    };
    if call.arguments.len() < config.call_argument_threshold
        && line_width(line) <= config.line_width
    {
        return line.to_string();
    }

    let mut out = String::new();
    out.push_str(indent);
    out.push_str(call.head.trim_end());
    out.push('(');
    out.push('\n');
    for argument in call.arguments {
        out.push_str(indent);
        out.push_str("    ");
        out.push_str(argument);
        out.push_str(",\n");
    }
    out.push_str(indent);
    out.push(')');
    out.push_str(call.tail.trim_start());
    out.push(';');
    out
}

fn format_postfix_chain(line: &str, config: &FormatConfig) -> String {
    if line.contains('\n') || line.contains("//") || line.contains("/*") {
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
    let segment_count = chain.receiver_segments + chain.segments.len();
    if segment_count < config.postfix_chain_threshold && line_width(line) <= config.line_width {
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

struct PostfixChain<'a> {
    receiver: &'a str,
    segments: Vec<&'a str>,
    receiver_segments: usize,
}

struct BooleanChain<'a> {
    operator: &'static str,
    parts: Vec<&'a str>,
}

struct CallArguments<'a> {
    head: &'a str,
    arguments: Vec<&'a str>,
    tail: &'a str,
}

fn parse_call_arguments(input: &str) -> Option<CallArguments<'_>> {
    let (open, close) = find_last_top_level_call(input)?;
    let args = input[open + 1..close].trim();
    if args.is_empty() {
        return None;
    }
    let arguments = split_top_level(args, ',');
    if arguments.is_empty() {
        return None;
    }
    Some(CallArguments {
        head: &input[..open],
        arguments,
        tail: &input[close + 1..],
    })
}

fn find_last_top_level_call(input: &str) -> Option<(usize, usize)> {
    let mut scanner = Scanner::default();
    let mut candidate = None;
    let mut index = 0usize;
    while index < input.len() {
        let ch = input[index..].chars().next()?;
        let is_top_level = scanner.is_top_level();
        if is_top_level
            && ch == '('
            && let Some(close) = find_matching_delimiter(input, index, '(', ')')
            && is_call_head(input, index)
        {
            candidate = Some((index, close));
            scanner = Scanner::default();
            for (replay_index, replay_ch) in input[..=close].char_indices() {
                scanner.scan(replay_index, replay_ch);
            }
            index = close + 1;
            continue;
        }

        if !scanner.scan(index, ch) {
            index += ch.len_utf8();
            continue;
        }

        index += ch.len_utf8();
    }
    candidate
}

fn is_call_head(input: &str, open: usize) -> bool {
    let before = input[..open].trim_end();
    let Some(last) = before.chars().next_back() else {
        return false;
    };
    if !(last == ']' || last == ')' || is_ident_continue(last)) {
        return false;
    }
    !before.ends_with("if")
        && !before.ends_with("while")
        && !before.ends_with("for")
        && !before.ends_with("match")
}

fn parse_boolean_chain(input: &str) -> Option<BooleanChain<'_>> {
    let operators = top_level_boolean_operators(input);
    if operators.is_empty() {
        return None;
    }
    let first_operator = operators[0].1;
    if !operators
        .iter()
        .all(|(_, operator, _)| *operator == first_operator)
    {
        return None;
    }

    let mut parts = Vec::new();
    let mut start = 0usize;
    for (index, _, len) in operators {
        let part = input[start..index].trim();
        if part.is_empty() {
            return None;
        }
        parts.push(part);
        start = index + len;
    }
    let tail = input[start..].trim();
    if tail.is_empty() {
        return None;
    }
    parts.push(tail);

    Some(BooleanChain {
        operator: first_operator,
        parts,
    })
}

fn top_level_boolean_operators(input: &str) -> Vec<(usize, &'static str, usize)> {
    let mut operators = Vec::new();
    let mut scanner = Scanner::default();
    let mut index = 0usize;
    while index < input.len() {
        let ch = input[index..].chars().next().expect("valid char boundary");
        if !scanner.scan(index, ch) {
            index += ch.len_utf8();
            continue;
        }

        if scanner.is_top_level()
            && let Some((operator, len)) = boolean_operator_at(input, index)
        {
            operators.push((index, operator, len));
            index += len;
            continue;
        }

        index += ch.len_utf8();
    }
    operators
}

fn boolean_operator_at(input: &str, index: usize) -> Option<(&'static str, usize)> {
    for operator in ["or", "and"] {
        let end = index + operator.len();
        if input.get(index..end) == Some(operator) && is_word_boundary(input, index, end) {
            return Some((operator, operator.len()));
        }
    }
    None
}

fn is_word_boundary(input: &str, start: usize, end: usize) -> bool {
    let before = input[..start].chars().next_back();
    let after = input[end..].chars().next();
    !matches!(before, Some(ch) if is_ident_continue(ch))
        && !matches!(after, Some(ch) if is_ident_continue(ch))
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
        let next = input.as_bytes()[index + 1..].first().copied();
        if matches!(prev, Some(b'=' | b'!' | b'<' | b'>' | b'-')) || next == Some(b'=') {
            continue;
        }
        return Some(index);
    }
    None
}

fn find_top_level_char(input: &str, needle: char) -> Option<usize> {
    let mut scanner = Scanner::default();
    for (index, ch) in input.char_indices() {
        if scanner.is_top_level() && ch == needle {
            return Some(index);
        }
        scanner.scan(index, ch);
    }
    None
}

fn parse_postfix_chain(input: &str) -> Option<PostfixChain<'_>> {
    let mut segments = Vec::new();
    let mut receiver_segments = 0usize;
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
            && input[index..].starts_with(".&.")
            && first_segment_start.is_none()
            && let Some(segment_end) = parse_postfix_segment(input, index, 3)
        {
            receiver_segments += 1;
            index = segment_end;
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

    Some(PostfixChain {
        receiver,
        segments,
        receiver_segments,
    })
}

fn postfix_operator_len(input: &str) -> Option<usize> {
    for operator in ["..&.", "."] {
        if input.starts_with(operator) {
            return Some(operator.len());
        }
    }
    None
}

fn parse_postfix_segment(input: &str, operator_start: usize, operator_len: usize) -> Option<usize> {
    let ident_start = operator_start + operator_len;
    let ident_end = parse_ident(input, ident_start)?;
    if input.as_bytes()[ident_end..].first().copied() != Some(b'(') {
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
    find_matching_delimiter(input, open, '(', ')').map(|close| {
        close
            + input[close..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0)
    })
}

fn previous_byte(input: &str, index: usize) -> Option<u8> {
    input.as_bytes()[..index]
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

fn find_matching_delimiter(
    input: &str,
    open: usize,
    open_ch: char,
    close_ch: char,
) -> Option<usize> {
    let mut in_string = false;
    let mut in_char = false;
    let mut escape = false;
    let mut depth = 0usize;
    for (offset, ch) in input[open..].char_indices() {
        let index = open + offset;
        if escape {
            escape = false;
            continue;
        }

        if in_string {
            match ch {
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        if in_char {
            match ch {
                '\\' => escape = true,
                '\'' => in_char = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '\'' => in_char = true,
            _ if ch == open_ch => depth += 1,
            _ if ch == close_ch => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(input: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut scanner = Scanner::default();
    let mut start = 0usize;
    for (index, ch) in input.char_indices() {
        if scanner.scan(index, ch) && scanner.is_top_level() && ch == separator {
            let part = input[start..index].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = index + ch.len_utf8();
        }
    }

    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
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
    fn splits_boolean_chains_in_assignments() {
        assert_eq!(
            format_source_text(
                "fn main(byte: u8) void {\n    let valid = byte == b':' or byte == b'_' or byte >= 0x80;\n}\n"
            ),
            "fn main(byte: u8) void {\n    let valid = byte == b':'\n        or byte == b'_'\n        or byte >= 0x80;\n}\n"
        );
    }

    #[test]
    fn leaves_short_boolean_chain_when_under_threshold() {
        let config = FormatConfig {
            boolean_chain_threshold: 4,
            ..FormatConfig::default()
        };
        assert_eq!(
            format_source_text_with_config(
                "fn main(byte: u8) void {\n    let valid = byte == b':' or byte == b'_' or byte >= 0x80;\n}\n",
                &config,
            ),
            "fn main(byte: u8) void {\n    let valid = byte == b':' or byte == b'_' or byte >= 0x80;\n}\n"
        );
    }

    #[test]
    fn splits_postfix_chain_assignments_at_threshold() {
        assert_eq!(
            format_source_text_with_config(
                "fn test(t: &mut Test) void {\n    let duplicate_decl_attr_err = duplicate_decl_attr..&.next().should_err().sum(@loc(), t);\n}\n",
                &FormatConfig::default(),
            ),
            "fn test(t: &mut Test) void {\n    let duplicate_decl_attr_err = duplicate_decl_attr\n        ..&.next()\n        .should_err()\n        .sum(@loc(), t);\n}\n"
        );
    }

    #[test]
    fn splits_long_postfix_chain_expressions() {
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    \"hello, {}!\".fmt(.{\"kern\"}).println();\n}\n",
                &FormatConfig {
                    line_width: 40,
                    ..FormatConfig::default()
                },
            ),
            "fn main() void {\n    \"hello, {}!\"\n        .fmt(.{\"kern\"})\n        .println();\n}\n"
        );
    }

    #[test]
    fn leaves_short_postfix_chain_on_one_line() {
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    value.foo().bar();\n}\n",
                &FormatConfig::default(),
            ),
            "fn main() void {\n    value.foo().bar();\n}\n"
        );
    }

    #[test]
    fn leaves_postfix_chain_when_under_configured_threshold() {
        let config = FormatConfig {
            postfix_chain_threshold: 4,
            ..FormatConfig::default()
        };
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    value.foo().bar().baz();\n}\n",
                &config,
            ),
            "fn main() void {\n    value.foo().bar().baz();\n}\n"
        );
    }

    #[test]
    fn keeps_borrow_operator_with_receiver() {
        assert_eq!(
            format_source_text(
                "fn main(t: &mut Test) void {\n    plain.&.qualified_name().should_none().sum(@loc(), t);\n}\n"
            ),
            "fn main(t: &mut Test) void {\n    plain.&.qualified_name()\n        .should_none()\n        .sum(@loc(), t);\n}\n"
        );
    }

    #[test]
    fn splits_function_parameters_at_threshold() {
        assert_eq!(
            format_source_text(
                "fn write_all(writer: &mut Write, text: &[u8], offset: usize) void!RenderError {\n}\n"
            ),
            "fn write_all(\n    writer: &mut Write,\n    text: &[u8],\n    offset: usize,\n) void!RenderError {\n}\n"
        );
    }

    #[test]
    fn leaves_function_parameters_under_configured_threshold() {
        let config = FormatConfig {
            function_parameter_threshold: 4,
            ..FormatConfig::default()
        };
        assert_eq!(
            format_source_text_with_config(
                "fn write_all(writer: &mut Write, text: &[u8], offset: usize) void!RenderError {\n}\n",
                &config,
            ),
            "fn write_all(writer: &mut Write, text: &[u8], offset: usize) void!RenderError {\n}\n"
        );
    }

    #[test]
    fn splits_call_arguments_at_threshold() {
        assert_eq!(
            format_source_text(
                "fn main() void {\n    print_stats(\"parse\", input_label, text.@len(), iterations);\n}\n"
            ),
            "fn main() void {\n    print_stats(\n        \"parse\",\n        input_label,\n        text.@len(),\n        iterations,\n    );\n}\n"
        );
    }

    #[test]
    fn splits_call_arguments_when_over_line_width() {
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    write_all(writer, very_long_text_slice_name_that_pushes_the_call_over_the_configured_width, offset);\n}\n",
                &FormatConfig {
                    line_width: 80,
                    ..FormatConfig::default()
                },
            ),
            "fn main() void {\n    write_all(\n        writer,\n        very_long_text_slice_name_that_pushes_the_call_over_the_configured_width,\n        offset,\n    );\n}\n"
        );
    }

    #[test]
    fn leaves_call_arguments_under_configured_threshold() {
        let config = FormatConfig {
            call_argument_threshold: 4,
            ..FormatConfig::default()
        };
        assert_eq!(
            format_source_text_with_config(
                "fn main() void {\n    write_all(writer, text, offset);\n}\n",
                &config,
            ),
            "fn main() void {\n    write_all(writer, text, offset);\n}\n"
        );
    }

    #[test]
    fn leaves_commented_postfix_chain_on_one_line() {
        let source = "fn main() void {\n    value_with_a_long_name.foo().bar().baz(); // keep\n}\n";
        assert_eq!(
            format_source_text_with_config(
                source,
                &FormatConfig {
                    line_width: 40,
                    ..FormatConfig::default()
                },
            ),
            source
        );
    }

    #[test]
    fn reports_unresolved_long_lines_after_formatting() {
        let formatted = format_source_text_with_config(
            "fn main() void {\n    let very_long_name = this_expression_is_still_too_long_even_after_the_formatter_runs;\n}\n",
            &FormatConfig {
                line_width: 60,
                ..FormatConfig::default()
            },
        );
        let diagnostics = collect_format_diagnostics(
            Path::new("."),
            Path::new("src/lib.kn"),
            &formatted,
            &FormatConfig {
                line_width: 60,
                ..FormatConfig::default()
            },
        );

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, Path::new("src/lib.kn"));
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(diagnostics[0].limit, 60);
    }

    #[test]
    fn excludes_configured_paths_from_formatting_scope() {
        let config = FormatConfig {
            exclude: vec!["src/generated/**".to_string()],
            ..FormatConfig::default()
        };

        assert!(!config.path_in_scope(Path::new("src/generated/bindings.kn")));
        assert!(config.path_in_scope(Path::new("src/lib.kn")));
    }
}
