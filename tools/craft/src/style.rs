use crate::error::{Error, Result};
use crate::manifest::Manifest;
use crate::workspace::WorkspaceMember;
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
}

pub fn collect_workspace_style_metrics(
    manifest_path: &Path,
    manifest: &Manifest,
    members: &[WorkspaceMember],
) -> Result<Vec<PackageStyleSummary>> {
    let mut summaries = Vec::new();
    summaries.push(collect_package_style_metrics(manifest_path, manifest)?);
    for member in members {
        summaries.push(collect_package_style_metrics(
            &member.manifest_path,
            &member.manifest,
        )?);
    }
    summaries.sort_by(|lhs, rhs| lhs.label.cmp(&rhs.label).then(lhs.root.cmp(&rhs.root)));
    Ok(summaries)
}

fn collect_package_style_metrics(
    manifest_path: &Path,
    manifest: &Manifest,
) -> Result<PackageStyleSummary> {
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let mut metrics = StyleSummary {
        packages: 1,
        ..StyleSummary::default()
    };

    for path in kern_source_files(root)? {
        let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
        metrics.merge(&count_source_metrics(&source));
        metrics.files += 1;
    }

    Ok(PackageStyleSummary {
        label: package_label(manifest),
        root: root.to_path_buf(),
        metrics,
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
        StyleSummary, count_source_metrics, find_token_outside_string, is_public_declaration_line,
    };

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
}
