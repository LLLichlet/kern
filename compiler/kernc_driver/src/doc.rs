use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, FunctionDef, ImplDef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernDoc {
    pub summary: String,
    pub details: String,
    pub sections: Vec<KernDocSection>,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernDocSection {
    pub kind: KernDocSectionKind,
    pub title: String,
    pub body: String,
    pub entries: Vec<KernDocEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernDocEntry {
    pub name: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernDocSectionKind {
    Args,
    Returns,
    Errors,
    Safety,
    Effects,
    Requires,
    Ensures,
    State,
    Boundary,
    Design,
    Rationale,
    Example,
    See,
    Note,
    Warning,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KmetaDocItem {
    pub path: String,
    pub kind: String,
    pub signature: Option<String>,
    pub docs: KernDoc,
}

pub fn normalize_doc(block: &ast::DocBlock) -> KernDoc {
    let raw_lines = block.lines.iter().map(|line| line.text.clone()).collect::<Vec<_>>();
    let raw_text = raw_lines.join("\n");
    let mut summary = String::new();
    let mut details = String::new();
    let mut sections = Vec::new();
    let mut body_lines = Vec::new();
    let mut current: Option<KernDocSection> = None;

    for line in raw_lines {
        if let Some(title) = parse_section_title(&line) {
            if let Some(section) = current.take() {
                sections.push(finalize_section(section));
            }
            current = Some(KernDocSection {
                kind: classify_section(&title),
                title,
                body: String::new(),
                entries: Vec::new(),
            });
            continue;
        }

        if let Some(section) = current.as_mut() {
            push_section_line(section, &line);
        } else {
            body_lines.push(line);
        }
    }

    if let Some(section) = current.take() {
        sections.push(finalize_section(section));
    }

    let paragraphs = split_paragraphs(&body_lines);
    if let Some(first) = paragraphs.first() {
        summary = first.clone();
    }
    if paragraphs.len() > 1 {
        details = paragraphs[1..].join("\n\n");
    }

    KernDoc {
        summary,
        details,
        sections,
        raw_text,
    }
}

pub fn render_hover_markdown(code: &str, docs: Option<&ast::DocBlock>) -> String {
    let mut out = format!("```kern\n{}\n```", code);
    let Some(block) = docs else {
        return out;
    };
    let doc = normalize_doc(block);
    if !doc.summary.is_empty() {
        out.push_str("\n\n");
        out.push_str(&doc.summary);
    }
    if !doc.details.is_empty() {
        out.push_str("\n\n");
        out.push_str(&doc.details);
    }
    for section in &doc.sections {
        out.push_str("\n\n**");
        out.push_str(&section.title);
        out.push_str("**");
        if !section.body.is_empty() {
            out.push_str("\n");
            out.push_str(&section.body);
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
    out
}

pub fn collect_kmeta_doc_items(ctx: &SemaContext<'_>) -> Vec<KmetaDocItem> {
    let mut items = Vec::new();

    for def in &ctx.defs {
        match def {
            Def::Module(module) if !module.is_imported => {
                push_item(
                    &mut items,
                    module_path(ctx, module.id),
                    "module",
                    Some(format!("module {}", ctx.resolve(module.name))),
                    module.docs.as_ref(),
                );
            }
            Def::Function(function) if !function.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, function.id),
                    if function.parent.is_some() { "method" } else { "function" },
                    function_signature(ctx, function),
                    function.docs.as_ref(),
                );
            }
            Def::Struct(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "struct",
                    Some(format!("struct {}", ctx.resolve(def.name))),
                    def.docs.as_ref(),
                );
                for field in &def.fields {
                    push_member_item(
                        &mut items,
                        ctx,
                        def.id,
                        "field",
                        ctx.resolve(field.name),
                        field.docs.as_ref(),
                        Some(format!(
                            "field {}: {}",
                            ctx.resolve(field.name),
                            type_node_label(ctx, &field.type_node)
                        )),
                    );
                }
            }
            Def::Union(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "union",
                    Some(format!("union {}", ctx.resolve(def.name))),
                    def.docs.as_ref(),
                );
                for field in &def.fields {
                    push_member_item(
                        &mut items,
                        ctx,
                        def.id,
                        "field",
                        ctx.resolve(field.name),
                        field.docs.as_ref(),
                        Some(format!(
                            "field {}: {}",
                            ctx.resolve(field.name),
                            type_node_label(ctx, &field.type_node)
                        )),
                    );
                }
            }
            Def::Enum(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "enum",
                    Some(format!("enum {}", ctx.resolve(def.name))),
                    def.docs.as_ref(),
                );
                for variant in &def.variants {
                    let signature = if let Some(payload) = &variant.payload_type {
                        Some(format!(
                            "variant {}: {}",
                            ctx.resolve(variant.name),
                            type_node_label(ctx, payload)
                        ))
                    } else {
                        Some(format!("variant {}", ctx.resolve(variant.name)))
                    };
                    push_member_item(
                        &mut items,
                        ctx,
                        def.id,
                        "variant",
                        ctx.resolve(variant.name),
                        variant.docs.as_ref(),
                        signature,
                    );
                }
            }
            Def::Trait(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "trait",
                    Some(format!("trait {}", ctx.resolve(def.name))),
                    def.docs.as_ref(),
                );
                for method in &def.methods {
                    push_member_item(
                        &mut items,
                        ctx,
                        def.id,
                        "trait_method",
                        ctx.resolve(method.name),
                        method.docs.as_ref(),
                        Some(format!(
                            "fn {}: {}",
                            ctx.resolve(method.name),
                            type_node_label(ctx, &method.type_node)
                        )),
                    );
                }
            }
            Def::Global(def) if !def.is_imported => {
                let kind = if def.is_static { "static" } else { "const" };
                let signature = if let Some(ty) = ctx.node_types.get(&def.value.id).copied() {
                    Some(format!(
                        "{} {}: {}",
                        kind,
                        ctx.resolve(def.name),
                        ctx.ty_to_string(ty)
                    ))
                } else {
                    Some(format!("{} {}", kind, ctx.resolve(def.name)))
                };
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    kind,
                    signature,
                    def.docs.as_ref(),
                );
            }
            Def::TypeAlias(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "type",
                    Some(format!(
                        "type {} = {}",
                        ctx.resolve(def.name),
                        type_node_label(ctx, &def.target)
                    )),
                    def.docs.as_ref(),
                );
            }
            _ => {}
        }
    }

    items.sort_by(|lhs, rhs| lhs.path.cmp(&rhs.path).then(lhs.kind.cmp(&rhs.kind)));
    items
}

pub fn render_kmeta_docs_toml(items: &[KmetaDocItem]) -> String {
    let mut out = String::new();
    out.push_str("format_version = 1\n\n");

    for item in items {
        out.push_str("[[item]]\n");
        out.push_str(&format!("path = {}\n", toml_quote(&item.path)));
        out.push_str(&format!("kind = {}\n", toml_quote(&item.kind)));
        if let Some(signature) = &item.signature {
            out.push_str(&format!("signature = {}\n", toml_quote(signature)));
        }
        out.push_str(&format!("summary = {}\n", toml_quote(&item.docs.summary)));
        out.push_str(&format!("details = {}\n", toml_quote(&item.docs.details)));
        out.push_str(&format!("raw = {}\n", toml_quote(&item.docs.raw_text)));
        out.push('\n');

        for section in &item.docs.sections {
            out.push_str("[[item.section]]\n");
            out.push_str(&format!("path = {}\n", toml_quote(&item.path)));
            out.push_str(&format!("kind = {}\n", toml_quote(section.kind.as_str())));
            out.push_str(&format!("title = {}\n", toml_quote(&section.title)));
            out.push_str(&format!("body = {}\n", toml_quote(&section.body)));
            out.push('\n');

            for entry in &section.entries {
                out.push_str("[[item.section.entry]]\n");
                out.push_str(&format!("path = {}\n", toml_quote(&item.path)));
                out.push_str(&format!("section = {}\n", toml_quote(&section.title)));
                if let Some(name) = &entry.name {
                    out.push_str(&format!("name = {}\n", toml_quote(name)));
                }
                out.push_str(&format!("body = {}\n", toml_quote(&entry.body)));
                out.push('\n');
            }
        }
    }

    out
}

fn parse_section_title(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.ends_with(':') || trimmed.len() < 2 {
        return None;
    }
    let title = trimmed[..trimmed.len() - 1].trim();
    if title.is_empty() || !title.chars().all(|ch| ch.is_ascii_alphabetic() || ch == ' ') {
        return None;
    }
    Some(title.to_string())
}

fn classify_section(title: &str) -> KernDocSectionKind {
    match title.to_ascii_lowercase().as_str() {
        "args" => KernDocSectionKind::Args,
        "returns" => KernDocSectionKind::Returns,
        "errors" | "fails" => KernDocSectionKind::Errors,
        "safety" => KernDocSectionKind::Safety,
        "effects" => KernDocSectionKind::Effects,
        "requires" => KernDocSectionKind::Requires,
        "ensures" => KernDocSectionKind::Ensures,
        "state" => KernDocSectionKind::State,
        "boundary" => KernDocSectionKind::Boundary,
        "design" => KernDocSectionKind::Design,
        "rationale" => KernDocSectionKind::Rationale,
        "example" => KernDocSectionKind::Example,
        "see" => KernDocSectionKind::See,
        "note" => KernDocSectionKind::Note,
        "warning" => KernDocSectionKind::Warning,
        _ => KernDocSectionKind::Custom,
    }
}

fn split_paragraphs(lines: &[String]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();

    for line in lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        current.push(line.trim_end().to_string());
    }

    if !current.is_empty() {
        paragraphs.push(current.join("\n"));
    }

    paragraphs
}

fn push_section_line(section: &mut KernDocSection, line: &str) {
    let trimmed = line.trim();
    if trimmed.starts_with("- ") {
        let entry_text = trimmed.trim_start_matches("- ").trim();
        let (name, body) = if let Some((name, body)) = entry_text.split_once(':') {
            let name = name.trim();
            let body = body.trim();
            if !name.is_empty() && !body.is_empty() {
                (Some(name.to_string()), body.to_string())
            } else {
                (None, entry_text.to_string())
            }
        } else {
            (None, entry_text.to_string())
        };
        section.entries.push(KernDocEntry { name, body });
        return;
    }

    if !section.entries.is_empty() && (line.starts_with(' ') || line.starts_with('\t')) {
        if let Some(last) = section.entries.last_mut() {
            if !last.body.is_empty() {
                last.body.push('\n');
            }
            last.body.push_str(trimmed);
            return;
        }
    }

    if !section.body.is_empty() {
        section.body.push('\n');
    }
    section.body.push_str(trimmed);
}

fn finalize_section(mut section: KernDocSection) -> KernDocSection {
    section.body = section.body.trim().to_string();
    for entry in &mut section.entries {
        entry.body = entry.body.trim().to_string();
    }
    section
}

fn push_item(
    items: &mut Vec<KmetaDocItem>,
    path: String,
    kind: &str,
    signature: Option<String>,
    docs: Option<&ast::DocBlock>,
) {
    let Some(docs) = docs else {
        return;
    };
    items.push(KmetaDocItem {
        path,
        kind: kind.to_string(),
        signature,
        docs: normalize_doc(docs),
    });
}

fn push_member_item(
    items: &mut Vec<KmetaDocItem>,
    ctx: &SemaContext<'_>,
    parent: DefId,
    kind: &str,
    name: &str,
    docs: Option<&ast::DocBlock>,
    signature: Option<String>,
) {
    let Some(docs) = docs else {
        return;
    };
    items.push(KmetaDocItem {
        path: format!("{}::{}", def_path(ctx, parent), name),
        kind: kind.to_string(),
        signature,
        docs: normalize_doc(docs),
    });
}

fn def_path(ctx: &SemaContext<'_>, def_id: DefId) -> String {
    match &ctx.defs[def_id.0 as usize] {
        Def::Module(module) => module_path(ctx, module.id),
        Def::Function(function) => function_path(ctx, function),
        Def::Struct(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::Union(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::Enum(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::Trait(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::TypeAlias(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::Global(def) => module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id)),
        Def::Impl(def) => impl_path(ctx, def),
    }
}

fn function_path(ctx: &SemaContext<'_>, function: &FunctionDef) -> String {
    if let Some(parent) = function.parent {
        match &ctx.defs[parent.0 as usize] {
            Def::Impl(impl_def) => format!("{}::{}", impl_path(ctx, impl_def), ctx.resolve(function.name)),
            Def::Module(module) => module_owned_path(ctx, function.name, Some(module.id)),
            _ => ctx.resolve(function.name).to_string(),
        }
    } else {
        ctx.resolve(function.name).to_string()
    }
}

fn impl_path(ctx: &SemaContext<'_>, impl_def: &ImplDef) -> String {
    let target = type_node_label(ctx, &impl_def.target_type);
    if let Some(module_id) = impl_def.parent_module {
        format!("{}::{}", module_path(ctx, module_id), target)
    } else {
        target
    }
}

fn module_owned_path(
    ctx: &SemaContext<'_>,
    name: kernc_utils::SymbolId,
    module_id: Option<DefId>,
) -> String {
    if let Some(module_id) = module_id {
        format!("{}::{}", module_path(ctx, module_id), ctx.resolve(name))
    } else {
        ctx.resolve(name).to_string()
    }
}

fn module_parent_for_named_def(ctx: &SemaContext<'_>, def_id: DefId) -> Option<DefId> {
    match &ctx.defs[def_id.0 as usize] {
        Def::Struct(def) => find_parent_module(ctx, def.id),
        Def::Union(def) => find_parent_module(ctx, def.id),
        Def::Enum(def) => find_parent_module(ctx, def.id),
        Def::Trait(def) => find_parent_module(ctx, def.id),
        Def::TypeAlias(def) => find_parent_module(ctx, def.id),
        Def::Global(def) => find_parent_module(ctx, def.id),
        _ => None,
    }
}

fn find_parent_module(ctx: &SemaContext<'_>, target: DefId) -> Option<DefId> {
    for def in &ctx.defs {
        let Def::Module(module) = def else {
            continue;
        };
        if module.items.contains(&target) {
            return Some(module.id);
        }
    }
    None
}

fn module_path(ctx: &SemaContext<'_>, module_id: DefId) -> String {
    let mut names = Vec::new();
    let mut current = Some(module_id);
    while let Some(id) = current {
        let Def::Module(module) = &ctx.defs[id.0 as usize] else {
            break;
        };
        names.push(ctx.resolve(module.name).to_string());
        current = module.parent;
    }
    names.reverse();
    names.join("::")
}

fn function_signature(ctx: &SemaContext<'_>, function: &FunctionDef) -> Option<String> {
    let sig = function.resolved_sig?;
    Some(format!(
        "fn {}: {}",
        ctx.resolve(function.name),
        ctx.ty_to_string(sig)
    ))
}

fn type_node_label(ctx: &SemaContext<'_>, type_node: &ast::TypeNode) -> String {
    if let Some(ty) = ctx.node_types.get(&type_node.id).copied() {
        return ctx.ty_to_string(ty);
    }
    ctx.sess.source_manager.slice_source(type_node.span).to_string()
}

impl KernDocSectionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            KernDocSectionKind::Args => "args",
            KernDocSectionKind::Returns => "returns",
            KernDocSectionKind::Errors => "errors",
            KernDocSectionKind::Safety => "safety",
            KernDocSectionKind::Effects => "effects",
            KernDocSectionKind::Requires => "requires",
            KernDocSectionKind::Ensures => "ensures",
            KernDocSectionKind::State => "state",
            KernDocSectionKind::Boundary => "boundary",
            KernDocSectionKind::Design => "design",
            KernDocSectionKind::Rationale => "rationale",
            KernDocSectionKind::Example => "example",
            KernDocSectionKind::See => "see",
            KernDocSectionKind::Note => "note",
            KernDocSectionKind::Warning => "warning",
            KernDocSectionKind::Custom => "custom",
        }
    }
}

fn toml_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}
