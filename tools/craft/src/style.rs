use crate::error::{Error, Result};
use crate::manifest::{CraftStyleConfig, CraftStyleSuggestionLevel, Manifest};
use crate::workspace::WorkspaceMember;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleSummary {
    pub packages: usize,
    pub files: usize,
    pub total_lines: usize,
    pub code_lines: usize,
    pub blank_lines: usize,
    pub inline_comment_lines: usize,
    pub block_comment_lines: usize,
    pub doc_comment_lines: usize,
    pub public_items: usize,
    pub documented_public_items: usize,
    pub undocumented_public_items: usize,
}

impl StyleSummary {
    pub fn comment_lines(&self) -> usize {
        self.inline_comment_lines + self.block_comment_lines + self.doc_comment_lines
    }

    pub fn comment_ratio(&self) -> f64 {
        ratio(self.comment_lines(), self.code_lines)
    }

    pub fn doc_ratio(&self) -> f64 {
        ratio(self.doc_comment_lines, self.code_lines)
    }

    pub fn public_doc_coverage(&self) -> f64 {
        ratio(self.documented_public_items, self.public_items)
    }

    pub fn merge(&mut self, other: &Self) {
        self.packages += other.packages;
        self.files += other.files;
        self.total_lines += other.total_lines;
        self.code_lines += other.code_lines;
        self.blank_lines += other.blank_lines;
        self.inline_comment_lines += other.inline_comment_lines;
        self.block_comment_lines += other.block_comment_lines;
        self.doc_comment_lines += other.doc_comment_lines;
        self.public_items += other.public_items;
        self.documented_public_items += other.documented_public_items;
        self.undocumented_public_items += other.undocumented_public_items;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageStyleSummary {
    pub label: String,
    pub root: PathBuf,
    pub metrics: StyleSummary,
    pub suggestions: Vec<StyleSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleSuggestion {
    pub path: PathBuf,
    pub line: usize,
    pub severity: SuggestionSeverity,
    pub rule: StyleRule,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleRule {
    IndexWhile,
    LongPostfixChain,
    RepeatedBorrowReceiver,
    MissingModuleDoc,
    UndocumentedPrivateHelper,
}

impl StyleRule {
    pub fn code(self) -> &'static str {
        match self {
            Self::IndexWhile => "index-while",
            Self::LongPostfixChain => "long-postfix-chain",
            Self::RepeatedBorrowReceiver => "repeated-borrow-receiver",
            Self::MissingModuleDoc => "missing-module-doc",
            Self::UndocumentedPrivateHelper => "undocumented-private-helper",
        }
    }

    pub fn is_known_code(code: &str) -> bool {
        matches!(
            code,
            "index-while"
                | "long-postfix-chain"
                | "repeated-borrow-receiver"
                | "missing-module-doc"
                | "undocumented-private-helper"
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionSeverity {
    Info,
    Warn,
}

impl SuggestionSeverity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleConfig {
    pub suggestions_enabled: bool,
    pub suggestion_severity: SuggestionSeverity,
    disabled_rules: BTreeSet<&'static str>,
    exclude: Vec<String>,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            suggestions_enabled: true,
            suggestion_severity: SuggestionSeverity::Info,
            disabled_rules: BTreeSet::new(),
            exclude: Vec::new(),
        }
    }
}

impl StyleConfig {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let mut config = Self::default();
        let Some(style) = manifest
            .craft
            .as_ref()
            .and_then(|craft| craft.style.as_ref())
        else {
            return config;
        };
        config.apply(style);
        config
    }

    fn apply(&mut self, style: &CraftStyleConfig) {
        match style.suggestions.unwrap_or(CraftStyleSuggestionLevel::Info) {
            CraftStyleSuggestionLevel::Off => self.suggestions_enabled = false,
            CraftStyleSuggestionLevel::Info => {
                self.suggestions_enabled = true;
                self.suggestion_severity = SuggestionSeverity::Info;
            }
            CraftStyleSuggestionLevel::Warn => {
                self.suggestions_enabled = true;
                self.suggestion_severity = SuggestionSeverity::Warn;
            }
        }
        self.disabled_rules = style
            .disabled_rules
            .iter()
            .filter_map(|rule| known_rule_code(rule))
            .collect();
        self.exclude = style.exclude.clone();
    }

    fn rule_enabled(&self, rule: StyleRule) -> bool {
        !self.disabled_rules.contains(rule.code())
    }

    fn path_in_scope(&self, path: &Path) -> bool {
        let text = path.to_string_lossy();
        !self
            .exclude
            .iter()
            .any(|pattern| path_matches(&text, pattern))
    }
}

pub fn collect_workspace_style_metrics(
    manifest_path: &Path,
    manifest: &Manifest,
    members: &[WorkspaceMember],
) -> Result<Vec<PackageStyleSummary>> {
    let mut summaries = Vec::new();
    summaries.push(collect_package_style_metrics(
        manifest_path,
        manifest,
        &StyleConfig::from_manifest(manifest),
    )?);
    for member in members {
        summaries.push(collect_package_style_metrics(
            &member.manifest_path,
            &member.manifest,
            &StyleConfig::from_manifest(&member.manifest),
        )?);
    }
    summaries.sort_by(|lhs, rhs| lhs.label.cmp(&rhs.label).then(lhs.root.cmp(&rhs.root)));
    Ok(summaries)
}

fn collect_package_style_metrics(
    manifest_path: &Path,
    manifest: &Manifest,
    config: &StyleConfig,
) -> Result<PackageStyleSummary> {
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut metrics = StyleSummary {
        packages: 1,
        ..StyleSummary::default()
    };
    let mut suggestions = Vec::new();

    for path in kern_source_files(root)? {
        let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
        metrics.merge(&count_source_metrics(&source));
        suggestions.extend(collect_source_suggestions(root, &path, &source, config));
        metrics.files += 1;
    }

    Ok(PackageStyleSummary {
        label: package_label(manifest),
        root: root.to_path_buf(),
        metrics,
        suggestions,
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

fn count_source_metrics(source: &str) -> StyleSummary {
    let mut metrics = StyleSummary::default();
    let mut in_block_comment = false;
    let mut pending_doc = false;

    for line in source.lines() {
        metrics.total_lines += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            metrics.blank_lines += 1;
            pending_doc = false;
            continue;
        }

        if in_block_comment {
            metrics.block_comment_lines += 1;
            if find_token_outside_string(trimmed, "*/").is_some() {
                in_block_comment = false;
            }
            continue;
        }

        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            metrics.doc_comment_lines += 1;
            pending_doc = true;
            continue;
        }

        if trimmed.starts_with("//") {
            metrics.inline_comment_lines += 1;
            pending_doc = false;
            continue;
        }

        if is_public_declaration_line(trimmed) {
            metrics.public_items += 1;
            if pending_doc {
                metrics.documented_public_items += 1;
            } else {
                metrics.undocumented_public_items += 1;
            }
        }
        pending_doc = false;

        let line_comment = find_token_outside_string(line, "//");
        let block_comment = find_token_outside_string(line, "/*");
        let first_comment = match (line_comment, block_comment) {
            (Some(line_pos), Some(block_pos)) => Some((line_pos.min(block_pos), block_pos)),
            (Some(line_pos), None) => Some((line_pos, usize::MAX)),
            (None, Some(block_pos)) => Some((block_pos, block_pos)),
            (None, None) => None,
        };
        let Some((comment_pos, block_pos)) = first_comment else {
            metrics.code_lines += 1;
            continue;
        };

        if !line[..comment_pos].trim().is_empty() {
            metrics.code_lines += 1;
        }
        if block_pos == comment_pos {
            metrics.block_comment_lines += 1;
            if find_token_outside_string(&line[comment_pos + 2..], "*/").is_none() {
                in_block_comment = true;
            }
            continue;
        }

        metrics.inline_comment_lines += 1;
    }

    metrics
}

fn is_public_declaration_line(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix("pub") else {
        return false;
    };
    let rest = rest
        .strip_prefix("..")
        .or_else(|| rest.strip_prefix('/'))
        .unwrap_or(rest)
        .trim_start();
    matches!(
        rest.split(|ch: char| ch.is_ascii_whitespace() || ch == '[')
            .next(),
        Some(
            "mod"
                | "use"
                | "fn"
                | "const"
                | "static"
                | "struct"
                | "union"
                | "enum"
                | "trait"
                | "type"
        )
    )
}

fn collect_source_suggestions(
    root: &Path,
    path: &Path,
    source: &str,
    config: &StyleConfig,
) -> Vec<StyleSuggestion> {
    if !config.suggestions_enabled {
        return Vec::new();
    }
    let display_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    if !config.path_in_scope(&display_path) {
        return Vec::new();
    }
    let mut suggestions = Vec::new();
    if config.rule_enabled(StyleRule::MissingModuleDoc) && missing_module_doc(source) {
        suggestions.push(StyleSuggestion {
            path: display_path.clone(),
            line: first_significant_line(source).unwrap_or(1),
            severity: config.suggestion_severity,
            rule: StyleRule::MissingModuleDoc,
            message: "add a module doc comment (`//!`) describing the file's role".to_string(),
        });
    }
    let mut pending_doc = false;
    let mut pending_comment = false;
    let mut scope_depth = 0usize;
    for (line_index, line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = line.trim();
        let is_doc_comment = trimmed.starts_with("///") || trimmed.starts_with("//!");
        let is_plain_comment = trimmed.starts_with("//") && !is_doc_comment;
        if is_doc_comment {
            pending_doc = true;
            pending_comment = true;
            continue;
        }
        if is_plain_comment {
            pending_doc = false;
            pending_comment = true;
            continue;
        }
        if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        let in_trait_body = scope_depth > 0 && trimmed.ends_with(';');
        if config.rule_enabled(StyleRule::UndocumentedPrivateHelper)
            && is_private_helper_declaration_line(trimmed)
            && !in_trait_body
            && !pending_doc
            && !pending_comment
        {
            suggestions.push(StyleSuggestion {
                path: display_path.clone(),
                line: line_number,
                severity: config.suggestion_severity,
                rule: StyleRule::UndocumentedPrivateHelper,
                message: "add a short comment for non-public helper functions that encode parsing, validation, or ownership logic"
                    .to_string(),
            });
        }
        if config.rule_enabled(StyleRule::IndexWhile)
            && is_index_while_line(trimmed)
            && !pending_comment
        {
            suggestions.push(StyleSuggestion {
                path: display_path.clone(),
                line: line_number,
                severity: config.suggestion_severity,
                rule: StyleRule::IndexWhile,
                message:
                    "consider `for` or an iterator when a loop only walks collection positions"
                        .to_string(),
            });
        }
        let postfix_calls = postfix_call_count_outside_string(trimmed);
        if config.rule_enabled(StyleRule::LongPostfixChain) && postfix_calls >= 5 {
            suggestions.push(StyleSuggestion {
                path: display_path.clone(),
                line: line_number,
                severity: config.suggestion_severity,
                rule: StyleRule::LongPostfixChain,
                message: "split long postfix chains across lines or bind an intermediate handle"
                    .to_string(),
            });
        }
        if config.rule_enabled(StyleRule::RepeatedBorrowReceiver)
            && borrowed_receiver_count(trimmed) >= 2
        {
            suggestions.push(StyleSuggestion {
                path: display_path.clone(),
                line: line_number,
                severity: config.suggestion_severity,
                rule: StyleRule::RepeatedBorrowReceiver,
                message: "bind a stack-mode handle when repeated `..&.` access dominates the line"
                    .to_string(),
            });
        }
        pending_doc = false;
        pending_comment = false;
        scope_depth = update_scope_depth(scope_depth, line);
    }
    suggestions
}

fn known_rule_code(rule: &str) -> Option<&'static str> {
    match rule {
        "index-while" => Some("index-while"),
        "long-postfix-chain" => Some("long-postfix-chain"),
        "repeated-borrow-receiver" => Some("repeated-borrow-receiver"),
        "missing-module-doc" => Some("missing-module-doc"),
        "undocumented-private-helper" => Some("undocumented-private-helper"),
        _ => None,
    }
}

fn path_matches(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

fn is_index_while_line(trimmed: &str) -> bool {
    let Some(condition) = trimmed
        .strip_prefix("while (")
        .and_then(|rest| rest.split_once(')').map(|(condition, _)| condition.trim()))
    else {
        return false;
    };
    if !condition.contains('<') || !condition.contains('#') {
        return false;
    }
    let Some((lhs, rhs)) = condition.split_once('<') else {
        return false;
    };
    is_simple_identifier(lhs.trim())
        && rhs.trim_start().starts_with('#')
        && !condition.contains(" and ")
        && !condition.contains(" or ")
}

fn is_simple_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn missing_module_doc(source: &str) -> bool {
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        return !trimmed.starts_with("//!");
    }
    false
}

fn first_significant_line(source: &str) -> Option<usize> {
    for (index, line) in source.lines().enumerate() {
        if !line.trim().is_empty() {
            return Some(index + 1);
        }
    }
    None
}

fn is_private_helper_declaration_line(trimmed: &str) -> bool {
    trimmed.starts_with("fn ") && trimmed.contains('(')
}

fn update_scope_depth(mut depth: usize, line: &str) -> usize {
    let mut in_string = false;
    let mut escaped = false;
    for byte in line.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn postfix_call_count_outside_string(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    let mut count = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while idx + 2 < bytes.len() {
        let byte = bytes[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            idx += 1;
            continue;
        }
        if byte == b'.' && bytes[idx + 1].is_ascii_alphabetic() && bytes[idx + 1..].contains(&b'(')
        {
            count += 1;
        }
        idx += 1;
    }

    count
}

fn borrowed_receiver_count(line: &str) -> usize {
    let mut count = 0usize;
    let mut rest = line;
    while let Some(index) = rest.find("..&.") {
        count += 1;
        rest = &rest[index + 4..];
    }
    count
}

fn find_token_outside_string(line: &str, token: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let token_bytes = token.as_bytes();
    let mut idx = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while idx < bytes.len() {
        let byte = bytes[idx];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }

        if byte == b'"' {
            in_string = true;
            idx += 1;
            continue;
        }
        if bytes[idx..].starts_with(token_bytes) {
            return Some(idx);
        }
        idx += 1;
    }

    None
}

fn package_label(manifest: &Manifest) -> String {
    manifest
        .package
        .as_ref()
        .map(|package| format!("{} {}", package.name, package.version))
        .unwrap_or_else(|| "<workspace>".to_string())
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    numerator as f64 * 100.0 / denominator as f64
}

#[cfg(test)]
mod tests {
    use super::{
        StyleConfig, StyleRule, StyleSummary, SuggestionSeverity, borrowed_receiver_count,
        collect_source_suggestions, count_source_metrics, find_token_outside_string,
        is_index_while_line, is_private_helper_declaration_line, is_public_declaration_line,
        missing_module_doc, path_matches, postfix_call_count_outside_string,
    };
    use crate::manifest::{CraftConfig, CraftStyleConfig, CraftStyleSuggestionLevel, Manifest};
    use std::path::Path;

    #[test]
    fn counts_comment_and_doc_comment_lines() {
        let metrics = count_source_metrics(
            r#"
/// Documents the function.
pub fn demo() void {
    // explain a branch
    call();
    let text = "http://example.invalid";
    let value = 1; // inline note
    /*
     block detail
     */
}
pub fn undocumented() void {}
"#,
        );

        assert_eq!(
            metrics,
            StyleSummary {
                packages: 0,
                files: 0,
                total_lines: 12,
                code_lines: 6,
                blank_lines: 1,
                inline_comment_lines: 2,
                block_comment_lines: 3,
                doc_comment_lines: 1,
                public_items: 2,
                documented_public_items: 1,
                undocumented_public_items: 1,
            }
        );
        assert_eq!(metrics.comment_lines(), 6);
        assert_eq!(metrics.public_doc_coverage(), 50.0);
    }

    #[test]
    fn ignores_comment_tokens_inside_strings() {
        assert_eq!(
            find_token_outside_string(r#""// not a comment""#, "//"),
            None
        );
        assert_eq!(
            find_token_outside_string(r#"let value = "/* nope */"; // yes"#, "//"),
            Some(26)
        );
    }

    #[test]
    fn recognizes_public_declaration_lines() {
        assert!(is_public_declaration_line("pub fn run() void {"));
        assert!(is_public_declaration_line("pub.. struct Page {"));
        assert!(is_public_declaration_line("pub/ trait Write {"));
        assert!(is_public_declaration_line("pub use .parse.{parse_i32};"));
        assert!(!is_public_declaration_line("puberty = true;"));
        assert!(!is_public_declaration_line("fn private() void {}"));
    }

    #[test]
    fn recognizes_source_style_suggestions() {
        assert!(is_index_while_line("while (index < #items) {"));
        assert!(!is_index_while_line(
            "while (index < #items and keep_going) {"
        ));
        assert!(missing_module_doc("fn demo() void {}\n"));
        assert!(!missing_module_doc(
            "//! Module docs.\n\nfn demo() void {}\n"
        ));
        assert!(is_private_helper_declaration_line("fn parse() void {"));
        assert!(is_private_helper_declaration_line(
            "fn next_event() ?Event!Error;"
        ));
        assert_eq!(
            postfix_call_count_outside_string(
                r#"value.should_ok().sum(@loc(), t).name.eq("root").should()"#
            ),
            5
        );
        assert_eq!(postfix_call_count_outside_string(r#""a.b().c()""#), 0);
        assert_eq!(
            borrowed_receiver_count("source..&.next(); other..&.next();"),
            2
        );
    }

    #[test]
    fn collects_source_suggestions_with_locations() {
        let suggestions = collect_source_suggestions(
            Path::new("/pkg"),
            Path::new("/pkg/src/main.rn"),
            r#"
//! Test module.

pub trait Stream {
    fn next_event() ?Event!Error;
};

// Exercises source-level style suggestions.
fn demo() void {
    while (index < #items) {
        index += 1;
    }
    // A stateful scan keeps byte offsets explicit.
    while (offset < #text) {
        offset += 1;
    }
    value.should_ok().sum(@loc(), t).name.eq("root").should();
    source..&.next(); other..&.next();
}
"#,
            &StyleConfig::default(),
        );

        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion.path == Path::new("src/main.rn")
                    && suggestion.rule == StyleRule::IndexWhile)
        );
        assert!(!suggestions.iter().any(|suggestion| suggestion.line == 5
            && suggestion.rule == StyleRule::UndocumentedPrivateHelper));
        assert!(
            !suggestions
                .iter()
                .any(|suggestion| suggestion.line == 12
                    && suggestion.rule == StyleRule::IndexWhile)
        );
        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion.rule == StyleRule::LongPostfixChain)
        );
        assert!(
            suggestions
                .iter()
                .any(|suggestion| suggestion.rule == StyleRule::RepeatedBorrowReceiver)
        );
    }

    #[test]
    fn style_config_controls_suggestions() {
        let source = r#"
//! Test module.

// Exercises source-level style suggestions.
fn demo() void {
    while (index < #items) {
        index += 1;
    }
    value.should_ok().sum(@loc(), t).name.eq("root").should();
}
"#;
        let mut manifest = Manifest::default();
        manifest.craft = Some(CraftConfig {
            style: Some(CraftStyleConfig {
                suggestions: Some(CraftStyleSuggestionLevel::Warn),
                disabled_rules: vec!["long-postfix-chain".to_string()],
                exclude: Vec::new(),
            }),
            ..CraftConfig::default()
        });
        let config = StyleConfig::from_manifest(&manifest);
        assert_eq!(config.suggestion_severity, SuggestionSeverity::Warn);

        let suggestions = collect_source_suggestions(
            Path::new("/pkg"),
            Path::new("/pkg/src/main.rn"),
            source,
            &config,
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].rule, StyleRule::IndexWhile);

        manifest
            .craft
            .as_mut()
            .unwrap()
            .style
            .as_mut()
            .unwrap()
            .suggestions = Some(CraftStyleSuggestionLevel::Off);
        let off_config = StyleConfig::from_manifest(&manifest);
        assert!(
            collect_source_suggestions(
                Path::new("/pkg"),
                Path::new("/pkg/src/main.rn"),
                source,
                &off_config,
            )
            .is_empty()
        );

        manifest
            .craft
            .as_mut()
            .unwrap()
            .style
            .as_mut()
            .unwrap()
            .suggestions = Some(CraftStyleSuggestionLevel::Info);
        manifest
            .craft
            .as_mut()
            .unwrap()
            .style
            .as_mut()
            .unwrap()
            .exclude = vec!["src/**".to_string()];
        let excluded_config = StyleConfig::from_manifest(&manifest);
        assert!(
            collect_source_suggestions(
                Path::new("/pkg"),
                Path::new("/pkg/src/main.rn"),
                source,
                &excluded_config,
            )
            .is_empty()
        );
    }

    #[test]
    fn collects_documentation_suggestions() {
        let suggestions = collect_source_suggestions(
            Path::new("/pkg"),
            Path::new("/pkg/src/parser.rn"),
            r#"
fn parse_name(text: &[u8], start: usize) usize!Error {
    return start;
}
"#,
            &StyleConfig::default(),
        );

        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].rule, StyleRule::MissingModuleDoc);
        assert_eq!(suggestions[1].rule, StyleRule::UndocumentedPrivateHelper);
    }

    #[test]
    fn path_scope_supports_plain_prefixes_and_globs() {
        assert!(path_matches("src/generated/bindings.rn", "src/generated"));
        assert!(path_matches(
            "src/generated/bindings.rn",
            "src/generated/**"
        ));
        assert!(!path_matches("src/xml/init.rn", "src/generated/**"));
    }
}
