//! Documentation-quality analysis and Markdown rendering.
//!
//! Craft documentation checks inspect public API metadata, summarize missing or
//! weak docs, and render structured docs into package-facing Markdown output.

use crate::build_plan::{ActionPlan, BuildPlan};
use crate::error::{Error, Result};
use crate::local_state;
use kernc_driver::{KernDocSectionKind, KmetaDocItem, load_kmeta_docs, load_kmeta_manifest};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedDoc {
    pub package_label: String,
    pub metadata_root: PathBuf,
    pub markdown_path: PathBuf,
    pub item_count: usize,
    pub quality: DocQualitySummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocQualitySummary {
    pub public_items: usize,
    pub documented_public_items: usize,
    pub undocumented_public_items: usize,
    pub warning_count: usize,
}

impl DocQualitySummary {
    pub fn merge(&mut self, other: &Self) {
        self.public_items += other.public_items;
        self.documented_public_items += other.documented_public_items;
        self.undocumented_public_items += other.undocumented_public_items;
        self.warning_count += other.warning_count;
    }

    pub fn coverage(&self) -> f64 {
        if self.public_items == 0 {
            return 0.0;
        }
        self.documented_public_items as f64 * 100.0 / self.public_items as f64
    }
}

pub fn sync_workspace_docs(
    build_plan: &BuildPlan,
    action_plan: &ActionPlan,
) -> Result<Vec<RenderedDoc>> {
    let docs_root = docs_output_root(&build_plan.workspace_root);
    local_state::ensure_dir(&docs_root)?;

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
            .map_err(|err| {
                Error::Execution(format!(
                    "failed to read `{}`: {err}",
                    metadata_root.display()
                ))
            })?
            .ok_or_else(|| {
                Error::Execution(format!(
                    "metadata root `{}` is missing package manifest",
                    metadata_root.display()
                ))
            })?;
        let docs = load_kmeta_docs(metadata_root)
            .map_err(|err| {
                Error::Execution(format!(
                    "failed to read `{}`: {err}",
                    metadata_root.display()
                ))
            })?
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
        let markdown = render_package_markdown(
            &manifest.package_name,
            manifest.package_version.as_deref(),
            &docs,
        );
        fs::write(&markdown_path, markdown).map_err(|err| Error::from_io(&markdown_path, err))?;

        outputs.push(RenderedDoc {
            package_label,
            metadata_root: metadata_root.clone(),
            markdown_path,
            item_count: docs.len(),
            quality: summarize_doc_quality(&docs),
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

pub fn summarize_doc_quality(items: &[KmetaDocItem]) -> DocQualitySummary {
    let mut summary = DocQualitySummary::default();
    for item in items {
        if !item.is_public {
            continue;
        }
        summary.public_items += 1;
        if item.docs.raw_text.trim().is_empty() && item.docs.summary.trim().is_empty() {
            summary.undocumented_public_items += 1;
            summary.warning_count += 1;
        } else {
            summary.documented_public_items += 1;
        }
    }
    summary
}

pub fn render_package_markdown(
    package_name: &str,
    package_version: Option<&str>,
    items: &[KmetaDocItem],
) -> String {
    let modules = collect_module_groups(package_name, items);
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(package_name);
    if let Some(version) = package_version {
        out.push_str("\n\n> Version: `");
        out.push_str(version);
        out.push('`');
    }

    out.push_str("\n\n<a id=\"navigation\"></a>");
    out.push_str("\n\n## Navigation\n");
    for module in &modules {
        out.push_str("\n- [Module `");
        out.push_str(&module.path);
        out.push_str("`](#");
        out.push_str(&module.anchor);
        out.push(')');
    }

    for module in &modules {
        render_module_markdown(&mut out, module);
    }

    out.push('\n');
    out
}

fn render_index_markdown(outputs: &[RenderedDoc]) -> String {
    let mut out = String::from(
        "# Workspace Docs\n\nThis index links package-level docs across the workspace.\n",
    );
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

#[derive(Default)]
struct ModuleDocGroup<'a> {
    path: String,
    anchor: String,
    module_item: Option<&'a KmetaDocItem>,
    types: Vec<&'a KmetaDocItem>,
    values: Vec<&'a KmetaDocItem>,
    impls: BTreeMap<String, Vec<&'a KmetaDocItem>>,
    capabilities: BTreeMap<String, Vec<&'a KmetaDocItem>>,
    members: BTreeMap<String, Vec<&'a KmetaDocItem>>,
}

fn collect_module_groups<'a>(
    package_name: &str,
    items: &'a [KmetaDocItem],
) -> Vec<ModuleDocGroup<'a>> {
    let mut modules = BTreeMap::<String, ModuleDocGroup<'a>>::new();

    for item in items {
        let module_path = item_module_path(package_name, item);
        let module = modules
            .entry(module_path.clone())
            .or_insert_with(|| ModuleDocGroup {
                anchor: anchor_for("module", &module_path),
                path: module_path.clone(),
                ..ModuleDocGroup::default()
            });

        match item.kind.as_str() {
            "module" => module.module_item = Some(item),
            "struct" | "union" | "enum" | "trait" | "type" => module.types.push(item),
            "function" | "const" | "static" => module.values.push(item),
            "method" => {
                let Some(owner_path) = owner_path(&item.path) else {
                    continue;
                };
                if item.impl_trait_external {
                    module
                        .capabilities
                        .entry(owner_path)
                        .or_default()
                        .push(item);
                } else {
                    module.impls.entry(owner_path).or_default().push(item);
                }
            }
            "field" | "variant" | "trait_method" => {
                let Some(owner_path) = owner_path(&item.path) else {
                    continue;
                };
                module.members.entry(owner_path).or_default().push(item);
            }
            _ => module.values.push(item),
        }
    }

    for module in modules.values_mut() {
        module.types.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
        module.values.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
        for items in module.impls.values_mut() {
            items.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
        }
        for items in module.capabilities.values_mut() {
            items.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
        }
        for items in module.members.values_mut() {
            items.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path));
        }
    }

    modules.into_values().collect()
}

fn render_module_markdown(out: &mut String, module: &ModuleDocGroup<'_>) {
    out.push_str("\n\n<a id=\"");
    out.push_str(&module.anchor);
    out.push_str("\"></a>");
    out.push_str("\n\n## Module `");
    out.push_str(&module.path);
    out.push_str("`\n");
    out.push_str("\n[Back to navigation](#navigation)");

    if let Some(item) = module.module_item {
        if let Some(signature) = &item.signature {
            out.push_str("\n\n```kern\n");
            out.push_str(signature);
            out.push_str("\n```");
        }
        render_doc_body(out, item, 3);
    }

    render_module_quick_links(out, module);

    for item in &module.types {
        render_item_markdown(
            out,
            item,
            3,
            &format!("{} `{}`", kind_heading(&item.kind), short_name(&item.path)),
        );
        if let Some(members) = module.members.get(&item.path) {
            out.push_str("\n\n#### Members");
            for member in members {
                render_item_markdown(
                    out,
                    member,
                    4,
                    &format!(
                        "{} `{}`",
                        kind_heading(&member.kind),
                        short_name(&member.path)
                    ),
                );
            }
        }
    }

    for item in &module.values {
        render_item_markdown(
            out,
            item,
            3,
            &format!("{} `{}`", kind_heading(&item.kind), short_name(&item.path)),
        );
    }

    for (owner_path, methods) in &module.impls {
        render_impl_markdown(out, module, owner_path, methods, false);
    }

    for (owner_path, methods) in &module.capabilities {
        render_impl_markdown(out, module, owner_path, methods, true);
    }
}

fn render_module_quick_links(out: &mut String, module: &ModuleDocGroup<'_>) {
    if module.types.is_empty()
        && module.values.is_empty()
        && module.impls.is_empty()
        && module.capabilities.is_empty()
    {
        return;
    }

    out.push_str("\n\n### Quick Links");
    if !module.types.is_empty() {
        out.push_str("\n\n#### Types");
        render_anchor_links(
            out,
            module.types.iter().map(|item| {
                (
                    short_name(&item.path).to_string(),
                    anchor_for("item", &item.path),
                )
            }),
        );
    }
    if !module.values.is_empty() {
        out.push_str("\n\n#### Values");
        render_anchor_links(
            out,
            module.values.iter().map(|item| {
                (
                    short_name(&item.path).to_string(),
                    anchor_for("item", &item.path),
                )
            }),
        );
    }
    if !module.impls.is_empty() {
        out.push_str("\n\n#### Impls");
        render_anchor_links(
            out,
            module.impls.keys().map(|owner_path| {
                (
                    impl_display_name(&module.path, owner_path),
                    anchor_for("impl", owner_path),
                )
            }),
        );
    }
    if !module.capabilities.is_empty() {
        out.push_str("\n\n#### Capabilities");
        render_anchor_links(
            out,
            module.capabilities.keys().map(|owner_path| {
                (
                    capability_display_name(&module.path, owner_path),
                    anchor_for("capability", owner_path),
                )
            }),
        );
    }
}

fn render_impl_markdown(
    out: &mut String,
    module: &ModuleDocGroup<'_>,
    owner_path: &str,
    methods: &[&KmetaDocItem],
    capability: bool,
) {
    let (impl_kind, impl_target) = impl_heading_parts(&module.path, owner_path, capability);
    out.push_str("\n\n<a id=\"");
    out.push_str(&anchor_for(
        if capability { "capability" } else { "impl" },
        owner_path,
    ));
    out.push_str("\"></a>");
    out.push_str("\n\n### ");
    out.push_str(impl_kind);
    out.push_str(" `");
    out.push_str(&impl_target);
    out.push_str("`\n");

    if !methods.is_empty() {
        out.push_str("\n\n#### Methods");
        render_anchor_links(
            out,
            methods.iter().map(|method| {
                (
                    short_name(&method.path).to_string(),
                    anchor_for("item", &method.path),
                )
            }),
        );
    }

    for method in methods {
        render_item_markdown(out, method, 4, &format!("`{}`", short_name(&method.path)));
    }
}

fn render_anchor_links(out: &mut String, links: impl Iterator<Item = (String, String)>) {
    for (label, anchor) in links {
        out.push_str("\n- [`");
        out.push_str(&label);
        out.push_str("`](#");
        out.push_str(&anchor);
        out.push(')');
    }
}

fn render_item_markdown(out: &mut String, item: &KmetaDocItem, level: usize, title: &str) {
    out.push_str("\n\n<a id=\"");
    out.push_str(&anchor_for("item", &item.path));
    out.push_str("\"></a>");
    out.push_str("\n\n");
    out.push_str(&"#".repeat(level));
    out.push(' ');
    out.push_str(title);

    if let Some(signature) = &item.signature {
        out.push_str("\n\n```kern\n");
        out.push_str(signature);
        out.push_str("\n```");
    }

    render_doc_body(out, item, level + 1);
}

fn render_doc_body(out: &mut String, item: &KmetaDocItem, level: usize) {
    if !item.docs.summary.is_empty() {
        out.push_str("\n\n");
        out.push_str(&item.docs.summary);
    }
    if !item.docs.details.is_empty() {
        out.push_str("\n\n");
        out.push_str(&item.docs.details);
    }

    for section in &item.docs.sections {
        out.push_str("\n\n");
        out.push_str(&"#".repeat(level));
        out.push(' ');
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

fn item_module_path(package_name: &str, item: &KmetaDocItem) -> String {
    match item.kind.as_str() {
        "module" => item.path.clone(),
        "method" | "field" | "variant" | "trait_method" => owner_path(&item.path)
            .and_then(|path| owner_path(&path))
            .unwrap_or_else(|| package_name.to_string()),
        _ => owner_path(&item.path).unwrap_or_else(|| package_name.to_string()),
    }
}

fn owner_path(path: &str) -> Option<String> {
    let (owner, _) = path.rsplit_once('.')?;
    Some(owner.to_string())
}

fn short_name(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

fn relative_name(module_path: &str, path: &str) -> String {
    if let Some(rest) = path.strip_prefix(module_path)
        && let Some(rest) = rest.strip_prefix('.')
    {
        return rest.to_string();
    }
    path.to_string()
}

fn kind_heading(kind: &str) -> &'static str {
    match kind {
        "module" => "Module",
        "struct" => "Struct",
        "union" => "Union",
        "enum" => "Enum",
        "trait" => "Trait",
        "type" => "Type",
        "function" => "Function",
        "method" => "Method",
        "field" => "Field",
        "variant" => "Variant",
        "trait_method" => "Trait Method",
        "const" => "Const",
        "static" => "Static",
        _ => "Item",
    }
}

fn impl_display_name(module_path: &str, owner_path: &str) -> String {
    let (kind, label) = impl_heading_parts(module_path, owner_path, false);
    format!("{kind}: {label}")
}

fn capability_display_name(module_path: &str, owner_path: &str) -> String {
    let (_, label) = impl_heading_parts(module_path, owner_path, true);
    label
}

fn impl_heading_parts(
    module_path: &str,
    owner_path: &str,
    capability: bool,
) -> (&'static str, String) {
    let label = relative_name(module_path, owner_path);
    if capability {
        return ("Capability", label);
    }
    if label.contains(" as ") {
        return ("Trait Impl", label);
    }
    ("Inherent Impl", label)
}

fn anchor_for(prefix: &str, value: &str) -> String {
    let mut out = String::from(prefix);
    out.push('-');
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::{render_package_markdown, summarize_doc_quality};
    use kernc_driver::{KernDoc, KernDocEntry, KernDocSection, KernDocSectionKind, KmetaDocItem};

    #[test]
    fn renders_markdown_from_structured_docs() {
        let markdown = render_package_markdown(
            "uart",
            Some("0.1.0"),
            &[
                KmetaDocItem {
                    path: "uart".to_string(),
                    kind: "module".to_string(),
                    signature: Some("module uart".to_string()),
                    impl_trait_path: None,
                    impl_trait_external: false,
                    is_public: true,
                    docs: KernDoc {
                        summary: "UART package.".to_string(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                },
                KmetaDocItem {
                    path: "uart.io".to_string(),
                    kind: "module".to_string(),
                    signature: Some("module io".to_string()),
                    impl_trait_path: None,
                    impl_trait_external: false,
                    is_public: true,
                    docs: KernDoc {
                        summary: "I/O utilities.".to_string(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                },
                KmetaDocItem {
                    path: "uart.io.Uart".to_string(),
                    kind: "struct".to_string(),
                    signature: Some("struct Uart {\n    pub base: u16,\n}".to_string()),
                    impl_trait_path: None,
                    impl_trait_external: false,
                    is_public: true,
                    docs: KernDoc {
                        summary: "Typed UART handle.".to_string(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                },
                KmetaDocItem {
                    path: "uart.io.Uart.read".to_string(),
                    kind: "method".to_string(),
                    signature: Some("fn read(self: Uart, port: u16) u8".to_string()),
                    impl_trait_path: None,
                    impl_trait_external: false,
                    is_public: true,
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
                },
                KmetaDocItem {
                    path: "uart.io.Uart as Reader.read".to_string(),
                    kind: "method".to_string(),
                    signature: Some("fn read(self: Uart) u8".to_string()),
                    impl_trait_path: Some("uart.io.Reader".to_string()),
                    impl_trait_external: false,
                    is_public: true,
                    docs: KernDoc {
                        summary: "Read through the reader trait.".to_string(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                },
                KmetaDocItem {
                    path: "uart.io.Uart as Formatable.write_to".to_string(),
                    kind: "method".to_string(),
                    signature: Some(
                        "fn write_to(self: Uart, writer: &mut dyn Writer) void".to_string(),
                    ),
                    impl_trait_path: Some("core.fmt.Formatable".to_string()),
                    impl_trait_external: true,
                    is_public: true,
                    docs: KernDoc {
                        summary: "Write through the formatting trait.".to_string(),
                        details: String::new(),
                        sections: Vec::new(),
                        raw_text: String::new(),
                    },
                },
            ],
        );

        assert!(markdown.contains("# uart"));
        assert!(markdown.contains("## Navigation"));
        assert!(markdown.contains("[Module `uart.io`](#module-uart-io)"));
        assert!(markdown.contains("## Module `uart.io`"));
        assert!(markdown.contains("### Quick Links"));
        assert!(markdown.contains("#### Types"));
        assert!(markdown.contains("#### Impls"));
        assert!(markdown.contains("#### Capabilities"));
        assert!(markdown.contains("### Struct `Uart`"));
        assert!(markdown.contains("### Inherent Impl `Uart`"));
        assert!(markdown.contains("### Trait Impl `Uart as Reader`"));
        assert!(markdown.contains("### Capability `Uart as Formatable`"));
        assert!(markdown.contains("#### Methods"));
        assert!(markdown.contains("#### `read`"));
        assert!(markdown.contains("#### `write_to`"));
        assert!(!markdown.contains("Path: `uart.io.Uart.read`"));
        assert!(markdown.contains("```kern\nfn read(self: Uart, port: u16) u8\n```"));
        assert!(markdown.contains("### Safety"));
        assert!(markdown.contains("- `self`: must point to a mapped UART object."));
    }

    #[test]
    fn summarizes_public_doc_quality() {
        let items = vec![
            KmetaDocItem {
                path: "pkg.Documented".to_string(),
                kind: "struct".to_string(),
                signature: None,
                impl_trait_path: None,
                impl_trait_external: false,
                is_public: true,
                docs: KernDoc {
                    summary: "Documented item.".to_string(),
                    details: String::new(),
                    sections: Vec::new(),
                    raw_text: "Documented item.".to_string(),
                },
            },
            KmetaDocItem {
                path: "pkg.Missing".to_string(),
                kind: "struct".to_string(),
                signature: None,
                impl_trait_path: None,
                impl_trait_external: false,
                is_public: true,
                docs: KernDoc {
                    summary: String::new(),
                    details: String::new(),
                    sections: Vec::new(),
                    raw_text: String::new(),
                },
            },
            KmetaDocItem {
                path: "pkg.Private".to_string(),
                kind: "function".to_string(),
                signature: None,
                impl_trait_path: None,
                impl_trait_external: false,
                is_public: false,
                docs: KernDoc {
                    summary: String::new(),
                    details: String::new(),
                    sections: Vec::new(),
                    raw_text: String::new(),
                },
            },
        ];

        let summary = summarize_doc_quality(&items);
        assert_eq!(summary.public_items, 2);
        assert_eq!(summary.documented_public_items, 1);
        assert_eq!(summary.undocumented_public_items, 1);
        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.coverage(), 50.0);
    }
}
