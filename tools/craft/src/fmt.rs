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
    let mut changed_paths = Vec::new();

    for path in kern_source_files(root)? {
        let source = fs::read_to_string(&path).map_err(|err| Error::from_io(&path, err))?;
        let formatted = format_source_text(&source);
        summary.files += 1;
        if formatted == source {
            continue;
        }
        summary.changed_files += 1;
        changed_paths.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        if mode == FormatMode::Write {
            fs::write(&path, formatted).map_err(|err| Error::from_io(&path, err))?;
        }
    }

    Ok(PackageFormatSummary {
        label: package_label(manifest),
        root: root.to_path_buf(),
        summary,
        changed_paths,
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

pub fn format_source_text(source: &str) -> String {
    let mut out = String::new();
    for line in source.lines() {
        let trimmed_end = line.trim_end_matches([' ', '\t']);
        let formatted_line = format_line(trimmed_end);
        out.push_str(&formatted_line);
        out.push('\n');
    }
    if source.is_empty() {
        return String::new();
    }
    out
}

fn format_line(line: &str) -> String {
    format_boolean_chain(&format_grouped_use(line))
}

fn format_grouped_use(line: &str) -> String {
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
    if items.len() < 5 {
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

fn format_boolean_chain(line: &str) -> String {
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

fn package_label(manifest: &Manifest) -> String {
    manifest
        .package
        .as_ref()
        .map(|package| format!("{} {}", package.name, package.version))
        .unwrap_or_else(|| "<workspace>".to_string())
}

#[cfg(test)]
mod tests {
    use super::format_source_text;

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
}
