use crate::build_plan::{ActionPlan, BuildPlan};
use crate::error::{Error, Result};
use kernc_driver::{KernDocSectionKind, KmetaDocItem, load_kmeta_docs, load_kmeta_manifest};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDoc {
    pub package_label: String,
    pub metadata_root: PathBuf,
    pub markdown_path: PathBuf,
    pub item_count: usize,
}

pub fn sync_workspace_docs(build_plan: &BuildPlan, action_plan: &ActionPlan) -> Result<Vec<RenderedDoc>> {
    let docs_root = docs_output_root(&build_plan.workspace_root);
    fs::create_dir_all(&docs_root).map_err(|err| Error::from_io(&docs_root, err))?;

    let mut outputs = Vec::new();
    let mut seen = BTreeSet::new();

    for action in &action_plan.compile_actions {
        let Some(metadata_root) = action.metadata_path.as_ref() else {
            continue;
        };
        if !seen.insert(metadata_root.clone()) {
            continue;
        }

        let manifest = load_kmeta_manifest(metadata_root)
            .map_err(|err| Error::Execution(format!("failed to read `{}`: {err}", metadata_root.display())))?
            .ok_or_else(|| {
                Error::Execution(format!(
                    "metadata root `{}` is missing package manifest",
                    metadata_root.display()
                ))
            })?;
        let docs = load_kmeta_docs(metadata_root)
            .map_err(|err| Error::Execution(format!("failed to read `{}`: {err}", metadata_root.display())))?
            .ok_or_else(|| {
                Error::Execution(format!(
                    "metadata root `{}` is missing native docs output",
                    metadata_root.display()
                ))
            })?;

        let package_label = if let Some(version) = &manifest.package_version {
            format!("{} {}", manifest.package_name, version)
        } else {
            manifest.package_name.clone()
        };
        let markdown_path = docs_root.join(format!(
            "{}-{}.md",
            sanitize_segment(&manifest.package_name),
            manifest
                .package_version
                .as_deref()
                .map(sanitize_segment)
                .unwrap_or_else(|| "local".to_string())
        ));
        let markdown = render_package_markdown(&manifest.package_name, manifest.package_version.as_deref(), &docs);
        fs::write(&markdown_path, markdown).map_err(|err| Error::from_io(&markdown_path, err))?;

        outputs.push(RenderedDoc {
            package_label,
            metadata_root: metadata_root.clone(),
            markdown_path,
            item_count: docs.len(),
        });
    }

    if outputs.is_empty() {
        return Err(Error::Usage(
            "`craft doc` requires at least one library target that emits native package metadata"
                .to_string(),
        ));
    }

    let index_path = docs_root.join("index.md");
    let index = render_index_markdown(&outputs);
    fs::write(&index_path, index).map_err(|err| Error::from_io(&index_path, err))?;

    Ok(outputs)
}

pub fn render_package_markdown(
    package_name: &str,
    package_version: Option<&str>,
    items: &[KmetaDocItem],
) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(package_name);
    if let Some(version) = package_version {
        out.push_str("\n\n> Version: `");
        out.push_str(version);
        out.push('`');
    }

    for item in items {
        out.push_str("\n\n## `");
        out.push_str(&item.path);
        out.push_str("`\n");
        out.push_str("\nKind: `");
        out.push_str(&item.kind);
        out.push('`');

        if let Some(signature) = &item.signature {
            out.push_str("\n\n```kern\n");
            out.push_str(signature);
            out.push_str("\n```");
        }

        if !item.docs.summary.is_empty() {
            out.push_str("\n\n");
            out.push_str(&item.docs.summary);
        }
        if !item.docs.details.is_empty() {
            out.push_str("\n\n");
            out.push_str(&item.docs.details);
        }

        for section in &item.docs.sections {
            out.push_str("\n\n### ");
            out.push_str(&section.title);
            out.push('\n');

            if !section.body.is_empty() {
                out.push('\n');
                if matches!(section.kind, KernDocSectionKind::Example) {
                    out.push_str("```kern\n");
                    out.push_str(&section.body);
                    out.push_str("\n```");
                } else {
                    out.push_str(&section.body);
                }
            }

            for entry in &section.entries {
                out.push_str("\n- ");
                if let Some(name) = &entry.name {
                    out.push('`');
                    out.push_str(name);
                    out.push_str("`: ");
                }
                out.push_str(&entry.body);
            }
        }
    }

    out.push('\n');
    out
}

fn render_index_markdown(outputs: &[RenderedDoc]) -> String {
    let mut out = String::from("# Workspace Docs\n");
    for output in outputs {
        let file = output
            .markdown_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("docs.md");
        out.push_str("\n- [");
        out.push_str(&output.package_label);
        out.push_str("](");
        out.push_str(file);
        out.push_str(") - ");
        out.push_str(&format!("{} item(s)", output.item_count));
    }
    out.push('\n');
    out
}

fn docs_output_root(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".craft").join("docs")
}

fn sanitize_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::render_package_markdown;
    use kernc_driver::{KernDoc, KernDocEntry, KernDocSection, KernDocSectionKind, KmetaDocItem};

    #[test]
    fn renders_markdown_from_structured_docs() {
        let markdown = render_package_markdown(
            "uart",
            Some("0.1.0"),
            &[KmetaDocItem {
                path: "uart::Uart::read".to_string(),
                kind: "function".to_string(),
                signature: Some("fn read: fn(u16) u8".to_string()),
                docs: KernDoc {
                    summary: "Read one byte from the receiver register.".to_string(),
                    details: String::new(),
                    sections: vec![KernDocSection {
                        kind: KernDocSectionKind::Safety,
                        title: "Safety".to_string(),
                        body: String::new(),
                        entries: vec![KernDocEntry {
                            name: Some("self".to_string()),
                            body: "must point to a mapped UART object.".to_string(),
                        }],
                    }],
                    raw_text: String::new(),
                },
            }],
        );

        assert!(markdown.contains("# uart"));
        assert!(markdown.contains("## `uart::Uart::read`"));
        assert!(markdown.contains("```kern\nfn read: fn(u16) u8\n```"));
        assert!(markdown.contains("### Safety"));
        assert!(markdown.contains("- `self`: must point to a mapped UART object."));
    }
}
