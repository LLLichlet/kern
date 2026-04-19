use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, FunctionDef, ImplDef};
use kernc_utils::Span;
use std::collections::BTreeSet;

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
    let raw_lines = block
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>();
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

pub fn lint_docs(ctx: &mut SemaContext<'_>) {
    let mut warnings = Vec::new();

    for def in &ctx.defs {
        match def {
            Def::Module(module) if !module.is_imported => {
                if let Some(docs) = &module.docs {
                    lint_doc_block(
                        docs,
                        &format!("module `{}`", module_path(ctx, module.id)),
                        None,
                        &mut warnings,
                    );
                }
            }
            Def::Function(function) if !function.is_imported => {
                if let Some(docs) = &function.docs {
                    let valid_args = function
                        .params
                        .iter()
                        .map(|param| ctx.resolve(param.pattern.name).to_string())
                        .collect::<BTreeSet<_>>();
                    let target = format!("function `{}`", function_path(ctx, function));
                    lint_doc_block(docs, &target, Some(&valid_args), &mut warnings);
                }
            }
            Def::Struct(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    lint_doc_block(
                        docs,
                        &format!("struct `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
                for field in &def.fields {
                    if let Some(docs) = &field.docs {
                        lint_doc_block(
                            docs,
                            &format!(
                                "field `{}::{}`",
                                def_path(ctx, def.id),
                                ctx.resolve(field.name)
                            ),
                            None,
                            &mut warnings,
                        );
                    }
                }
            }
            Def::Union(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    lint_doc_block(
                        docs,
                        &format!("union `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
                for field in &def.fields {
                    if let Some(docs) = &field.docs {
                        lint_doc_block(
                            docs,
                            &format!(
                                "field `{}::{}`",
                                def_path(ctx, def.id),
                                ctx.resolve(field.name)
                            ),
                            None,
                            &mut warnings,
                        );
                    }
                }
            }
            Def::Enum(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    lint_doc_block(
                        docs,
                        &format!("enum `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
                for variant in &def.variants {
                    if let Some(docs) = &variant.docs {
                        lint_doc_block(
                            docs,
                            &format!(
                                "variant `{}::{}`",
                                def_path(ctx, def.id),
                                ctx.resolve(variant.name)
                            ),
                            None,
                            &mut warnings,
                        );
                    }
                }
            }
            Def::Trait(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    lint_doc_block(
                        docs,
                        &format!("trait `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
                for method in &def.methods {
                    if let Some(docs) = &method.docs {
                        lint_doc_block(
                            docs,
                            &format!(
                                "trait method `{}::{}`",
                                def_path(ctx, def.id),
                                ctx.resolve(method.name)
                            ),
                            None,
                            &mut warnings,
                        );
                    }
                }
            }
            Def::Global(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    let kind = if def.is_static { "static" } else { "const" };
                    lint_doc_block(
                        docs,
                        &format!("{kind} `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
            }
            Def::TypeAlias(def) if !def.is_imported => {
                if let Some(docs) = &def.docs {
                    lint_doc_block(
                        docs,
                        &format!("type `{}`", def_path(ctx, def.id)),
                        None,
                        &mut warnings,
                    );
                }
            }
            _ => {}
        }
    }

    for warning in warnings {
        let mut builder = ctx.struct_warning(warning.span, warning.message);
        if let Some(hint) = warning.hint {
            builder = builder.with_hint(hint);
        }
        builder.emit();
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
            out.push('\n');
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
                let is_method = function_receiver_impl(ctx, function).is_some();
                push_item(
                    &mut items,
                    def_path(ctx, function.id),
                    if is_method { "method" } else { "function" },
                    function_signature(ctx, function),
                    function.docs.as_ref(),
                );
            }
            Def::Struct(def) if !def.is_imported => {
                push_item(
                    &mut items,
                    def_path(ctx, def.id),
                    "struct",
                    Some(type_signature(
                        ctx,
                        "struct",
                        ctx.resolve(def.name),
                        &def.generics,
                        def.fields.iter(),
                    )),
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
                    Some(type_signature(
                        ctx,
                        "union",
                        ctx.resolve(def.name),
                        &def.generics,
                        def.fields.iter(),
                    )),
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
                        trait_method_signature(ctx, method),
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
            out.push_str(&format!("kind = {}\n", toml_quote(section.kind.as_str())));
            out.push_str(&format!("title = {}\n", toml_quote(&section.title)));
            out.push_str(&format!("body = {}\n", toml_quote(&section.body)));
            out.push('\n');

            for entry in &section.entries {
                out.push_str("[[item.section.entry]]\n");
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
    if title.is_empty()
        || !title
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == ' ')
    {
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
        if let Some(last) = section.entries.last_mut()
            && !last.body.is_empty()
        {
            last.body.push('\n');
            last.body.push_str(trimmed);
            return;
        }
        if let Some(last) = section.entries.last_mut() {
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

#[derive(Debug, Clone)]
struct DocLint {
    span: Span,
    message: String,
    hint: Option<String>,
}

fn lint_doc_block(
    block: &ast::DocBlock,
    target: &str,
    valid_args: Option<&BTreeSet<String>>,
    warnings: &mut Vec<DocLint>,
) {
    let doc = normalize_doc(block);
    if doc.summary.trim().is_empty() {
        warnings.push(DocLint {
            span: block.span,
            message: format!("doc block for {target} is missing a summary paragraph"),
            hint: Some(
                "start the doc block with a short first paragraph that states what the item means or guarantees"
                    .to_string(),
            ),
        });
    }

    let mut current_section = None::<String>;
    for line in &block.lines {
        if let Some(title) = parse_section_title(&line.text) {
            if classify_section(&title) == KernDocSectionKind::Custom {
                warnings.push(DocLint {
                    span: line.span,
                    message: format!("unknown doc section `{title}` in {target}"),
                    hint: Some(
                        "supported sections: Args, Returns, Errors, Safety, Effects, Requires, Ensures, State, Boundary, Design, Rationale, Example, See, Note, Warning".to_string(),
                    ),
                });
            }
            current_section = Some(title);
            continue;
        }

        if current_section.as_deref() == Some("Args") {
            lint_args_line(line, target, valid_args, warnings);
        }
    }
}

fn lint_args_line(
    line: &ast::DocLine,
    target: &str,
    valid_args: Option<&BTreeSet<String>>,
    warnings: &mut Vec<DocLint>,
) {
    let trimmed = line.text.trim();
    if trimmed.is_empty() || !trimmed.starts_with("- ") {
        return;
    }

    let entry = trimmed.trim_start_matches("- ").trim();
    let Some((name, body)) = entry.split_once(':') else {
        warnings.push(DocLint {
            span: line.span,
            message: format!("malformed `Args` entry in {target}"),
            hint: Some("write argument docs as `- name: description`".to_string()),
        });
        return;
    };

    let name = name.trim();
    let body = body.trim();
    if name.is_empty() || body.is_empty() {
        warnings.push(DocLint {
            span: line.span,
            message: format!("malformed `Args` entry in {target}"),
            hint: Some("write argument docs as `- name: description`".to_string()),
        });
        return;
    }

    if let Some(valid_args) = valid_args
        && !valid_args.contains(name)
    {
        warnings.push(DocLint {
            span: line.span,
            message: format!("unknown documented argument `{name}` in {target}"),
            hint: Some(format!(
                "documented arguments must match the real parameter list: {}",
                valid_args.iter().cloned().collect::<Vec<_>>().join(", ")
            )),
        });
    }
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
        path: format!("{}.{}", def_path(ctx, parent), name),
        kind: kind.to_string(),
        signature,
        docs: normalize_doc(docs),
    });
}

fn def_path(ctx: &SemaContext<'_>, def_id: DefId) -> String {
    match &ctx.defs[def_id.0 as usize] {
        Def::Module(module) => module_path(ctx, module.id),
        Def::Function(function) => function_path(ctx, function),
        Def::Struct(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::Union(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::Enum(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::Trait(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::AssociatedType(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::TypeAlias(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::Global(def) => {
            module_owned_path(ctx, def.name, module_parent_for_named_def(ctx, def_id))
        }
        Def::Impl(def) => impl_path(ctx, def),
    }
}

fn function_path(ctx: &SemaContext<'_>, function: &FunctionDef) -> String {
    if let Some(parent) = function.parent {
        match &ctx.defs[parent.0 as usize] {
            Def::Impl(impl_def) => format!(
                "{}.{}",
                impl_path(ctx, impl_def),
                ctx.resolve(function.name)
            ),
            Def::Module(module) => module_owned_path(ctx, function.name, Some(module.id)),
            _ => ctx.resolve(function.name).to_string(),
        }
    } else {
        ctx.resolve(function.name).to_string()
    }
}

fn impl_path(ctx: &SemaContext<'_>, impl_def: &ImplDef) -> String {
    let mut target = type_node_label(ctx, &impl_def.target_type);
    if let Some(trait_type) = &impl_def.trait_type {
        target.push_str(" as ");
        target.push_str(&type_node_label(ctx, trait_type));
    }
    if let Some(module_id) = impl_def.parent_module {
        format!("{}.{}", module_path(ctx, module_id), target)
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
        format!("{}.{}", module_path(ctx, module_id), ctx.resolve(name))
    } else {
        ctx.resolve(name).to_string()
    }
}

fn module_parent_for_named_def(ctx: &SemaContext<'_>, def_id: DefId) -> Option<DefId> {
    match &ctx.defs[def_id.0 as usize] {
        Def::Struct(def) => ctx.def_parent_module(def.id),
        Def::Union(def) => ctx.def_parent_module(def.id),
        Def::Enum(def) => ctx.def_parent_module(def.id),
        Def::Trait(def) => ctx.def_parent_module(def.id),
        Def::TypeAlias(def) => ctx.def_parent_module(def.id),
        Def::Global(def) => ctx.def_parent_module(def.id),
        _ => None,
    }
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
    names.join(".")
}

fn function_signature(ctx: &SemaContext<'_>, function: &FunctionDef) -> Option<String> {
    let mut out = String::new();
    if function.is_extern {
        out.push_str("extern ");
    }
    if function.is_const {
        out.push_str("const ");
    }
    out.push_str("fn ");
    out.push_str(ctx.resolve(function.name));
    out.push_str(&generic_params_label(ctx, &function.generics));
    out.push('(');

    let mut params = Vec::new();
    for param in &function.params {
        params.push(format!(
            "{}: {}",
            ctx.resolve(param.pattern.name),
            type_node_label(ctx, &param.type_node)
        ));
    }
    if function.is_variadic {
        params.push("...".to_string());
    }
    out.push_str(&params.join(", "));
    out.push(')');
    out.push(' ');
    out.push_str(&type_node_label(ctx, &function.ret_type));
    Some(out)
}

fn function_receiver_impl<'a>(
    ctx: &'a SemaContext<'_>,
    function: &FunctionDef,
) -> Option<&'a ImplDef> {
    let parent = function.parent?;
    let Def::Impl(impl_def) = &ctx.defs[parent.0 as usize] else {
        return None;
    };
    Some(impl_def)
}

fn generic_params_label(ctx: &SemaContext<'_>, generics: &[ast::GenericParam]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    let names = generics
        .iter()
        .map(|param| match &param.kind {
            ast::GenericParamKind::Type => ctx.resolve(param.name).to_string(),
            ast::GenericParamKind::Const { ty } => {
                format!("{}: {}", ctx.resolve(param.name), type_node_label(ctx, ty))
            }
        })
        .collect::<Vec<_>>();
    format!("[{}]", names.join(", "))
}

fn trait_method_signature(ctx: &SemaContext<'_>, method: &ast::StructFieldDef) -> Option<String> {
    let ast::TypeKind::Function {
        params,
        ret,
        is_variadic,
    } = &method.type_node.kind
    else {
        return Some(format!(
            "fn {}: {}",
            ctx.resolve(method.name),
            type_node_label(ctx, &method.type_node)
        ));
    };

    let mut out = String::new();
    out.push_str("fn ");
    out.push_str(ctx.resolve(method.name));
    out.push('(');
    let mut rendered_params = Vec::new();
    for param in params {
        rendered_params.push(type_node_label(ctx, param));
    }
    if *is_variadic {
        rendered_params.push("...".to_string());
    }
    out.push_str(&rendered_params.join(", "));
    out.push(')');
    out.push(' ');
    if let Some(ret) = ret {
        out.push_str(&type_node_label(ctx, ret));
    } else {
        out.push_str("void");
    }
    Some(out)
}

fn type_signature<'a>(
    ctx: &SemaContext<'_>,
    kind: &str,
    name: &str,
    generics: &[ast::GenericParam],
    fields: impl Iterator<Item = &'a ast::StructFieldDef>,
) -> String {
    let public_fields = fields.filter(|field| field.is_pub).collect::<Vec<_>>();
    let mut out = format!("{kind} {name}{}", generic_params_label(ctx, generics));
    if public_fields.is_empty() {
        return out;
    }

    out.push_str(" {\n");
    for field in public_fields {
        out.push_str("    pub ");
        out.push_str(ctx.resolve(field.name));
        out.push_str(": ");
        out.push_str(&type_node_label(ctx, &field.type_node));
        out.push_str(",\n");
    }
    out.push('}');
    out
}

fn type_node_label(ctx: &SemaContext<'_>, type_node: &ast::TypeNode) -> String {
    if let Some(ty) = ctx.node_types.get(&type_node.id).copied() {
        return ctx.ty_to_string(ty);
    }
    ctx.sess
        .source_manager
        .slice_source(type_node.span)
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::collect_kmeta_doc_items;
    use kernc_ast as ast;
    use kernc_sema::SemaContext;
    use kernc_sema::def::{Def, DefId, FunctionDef, ImplDef, ModuleDef, StructDef, Visibility};
    use kernc_sema::scope::ScopeId;
    use kernc_utils::{NodeId, Session, Span};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn collect_kmeta_doc_items_distinguishes_trait_impl_methods() {
        let mut session = Session::new();
        let source = "Device Service";
        let file_id = session
            .source_manager
            .add_file("doc_test.rn".to_string(), source.to_string());
        let mut ctx = SemaContext::new(&mut session);

        let root_name = ctx.intern("root");
        let read_name = ctx.intern("read");

        let module_id = ctx.add_def(Def::Module(ModuleDef {
            id: DefId(0),
            name: root_name,
            parent: None,
            is_imported: false,
            scope_id: ScopeId(0),
            dir_path: PathBuf::new(),
            file_id,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
            is_init: true,
            docs: None,
        }));

        let target_type = path_type(file_id, 0, 6, ctx.intern("Device"));
        let trait_type = path_type(file_id, 7, 14, ctx.intern("Service"));

        let inherent_impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: DefId(1),
            parent_module: Some(module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            target_type: target_type.clone(),
            trait_type: None,
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let trait_impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: DefId(2),
            parent_module: Some(module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            target_type: target_type.clone(),
            trait_type: Some(trait_type),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        ctx.add_def(Def::Function(FunctionDef {
            id: DefId(3),
            name: read_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(inherent_impl_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: void_type(),
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: Some(doc_block("Read from the inherent implementation.")),
            attributes: Vec::new(),
        }));

        ctx.add_def(Def::Function(FunctionDef {
            id: DefId(4),
            name: read_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(trait_impl_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: void_type(),
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: Some(doc_block("Read from the trait implementation.")),
            attributes: Vec::new(),
        }));

        let items = collect_kmeta_doc_items(&ctx);
        let paths = items
            .iter()
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"root.Device.read"));
        assert!(paths.contains(&"root.Device as Service.read"));
    }

    #[test]
    fn collect_kmeta_doc_items_treats_module_functions_as_functions() {
        let mut session = Session::new();
        let file_id = session
            .source_manager
            .add_file("doc_test.rn".to_string(), "Result".to_string());
        let mut ctx = SemaContext::new(&mut session);

        let root_name = ctx.intern("toml");
        let parse_name = ctx.intern("parse");

        let module_id = ctx.add_def(Def::Module(ModuleDef {
            id: DefId(0),
            name: root_name,
            parent: None,
            is_imported: false,
            scope_id: ScopeId(0),
            dir_path: PathBuf::new(),
            file_id,
            submodules: HashMap::new(),
            items: vec![DefId(1)],
            imports: Vec::new(),
            is_init: true,
            docs: None,
        }));

        let result_type = path_type(file_id, 0, 6, ctx.intern("Result"));
        ctx.add_def(Def::Function(FunctionDef {
            id: DefId(1),
            name: parse_name,
            name_span: Span::default(),
            vis: Visibility::Public,
            parent: Some(module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: result_type,
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: Some(doc_block("Parse a TOML document.")),
            attributes: Vec::new(),
        }));

        let items = collect_kmeta_doc_items(&ctx);
        let parse = items.iter().find(|item| item.path == "toml.parse").unwrap();
        assert_eq!(parse.kind, "function");
        assert_eq!(parse.signature.as_deref(), Some("fn parse() Result"));
    }

    #[test]
    fn collect_kmeta_doc_items_keep_pointer_impl_targets_distinct() {
        let mut session = Session::new();
        let source = "i32 *i32 Marker";
        let file_id = session
            .source_manager
            .add_file("doc_test.rn".to_string(), source.to_string());
        let mut ctx = SemaContext::new(&mut session);

        let root_name = ctx.intern("root");
        let marker_name = ctx.intern("Marker");
        let tag_name = ctx.intern("tag");
        let i32_name = ctx.intern("i32");

        let module_id = ctx.add_def(Def::Module(ModuleDef {
            id: DefId(0),
            name: root_name,
            parent: None,
            is_imported: false,
            scope_id: ScopeId(0),
            dir_path: PathBuf::new(),
            file_id,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
            is_init: true,
            docs: None,
        }));

        let value_target = path_type(file_id, 0, 3, i32_name);
        let pointer_target = pointer_type(file_id, 4, 8, path_type(file_id, 5, 8, i32_name));
        let trait_type = path_type(file_id, 9, 15, marker_name);

        let value_impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: DefId(1),
            parent_module: Some(module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            target_type: value_target,
            trait_type: Some(trait_type.clone()),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        let pointer_impl_id = ctx.add_def(Def::Impl(ImplDef {
            id: DefId(2),
            parent_module: Some(module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            target_type: pointer_target,
            trait_type: Some(trait_type),
            assoc_types: Vec::new(),
            methods: Vec::new(),
            span: Span::default(),
        }));

        ctx.add_def(Def::Function(FunctionDef {
            id: DefId(3),
            name: tag_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(value_impl_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: void_type(),
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: Some(doc_block("Value tag.")),
            attributes: Vec::new(),
        }));

        ctx.add_def(Def::Function(FunctionDef {
            id: DefId(4),
            name: tag_name,
            name_span: Span::default(),
            vis: Visibility::Private,
            parent: Some(pointer_impl_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: Vec::new(),
            ret_type: void_type(),
            body: None,
            is_const: false,
            is_extern: false,
            is_variadic: false,
            is_intrinsic: false,
            span: Span::default(),
            resolved_sig: None,
            docs: Some(doc_block("Pointer tag.")),
            attributes: Vec::new(),
        }));

        let items = collect_kmeta_doc_items(&ctx);
        let paths = items
            .iter()
            .map(|item| item.path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"root.i32 as Marker.tag"));
        assert!(paths.contains(&"root.*i32 as Marker.tag"));
    }

    #[test]
    fn collect_kmeta_doc_items_include_public_struct_fields_in_signature() {
        let mut session = Session::new();
        let file_id = session
            .source_manager
            .add_file("doc_test.rn".to_string(), "Config bool i64".to_string());
        let mut ctx = SemaContext::new(&mut session);

        let root_name = ctx.intern("toml");
        let config_name = ctx.intern("Config");
        let enabled_name = ctx.intern("enabled");
        let hidden_name = ctx.intern("hidden");
        let bool_name = ctx.intern("bool");
        let i64_name = ctx.intern("i64");

        ctx.add_def(Def::Module(ModuleDef {
            id: DefId(0),
            name: root_name,
            parent: None,
            is_imported: false,
            scope_id: ScopeId(0),
            dir_path: PathBuf::new(),
            file_id,
            submodules: HashMap::new(),
            items: vec![DefId(1)],
            imports: Vec::new(),
            is_init: true,
            docs: None,
        }));

        let enabled_type = path_type(file_id, 7, 11, bool_name);
        let hidden_type = path_type(file_id, 12, 15, i64_name);
        ctx.add_def(Def::Struct(StructDef {
            id: DefId(1),
            name: config_name,
            vis: Visibility::Public,
            parent_module: Some(DefId(0)),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            fields: vec![
                ast::StructFieldDef {
                    name: enabled_name,
                    name_span: Span::default(),
                    is_pub: true,
                    docs: None,
                    type_node: enabled_type,
                    default_value: None,
                    span: Span::default(),
                },
                ast::StructFieldDef {
                    name: hidden_name,
                    name_span: Span::default(),
                    is_pub: false,
                    docs: None,
                    type_node: hidden_type,
                    default_value: None,
                    span: Span::default(),
                },
            ],
            is_extern: false,
            span: Span::default(),
            docs: Some(doc_block("Public configuration shape.")),
            attributes: Vec::new(),
        }));

        let items = collect_kmeta_doc_items(&ctx);
        let config = items
            .iter()
            .find(|item| item.path == "toml.Config")
            .unwrap();
        let signature = config.signature.as_deref().unwrap();
        assert!(signature.contains("struct Config {"));
        assert!(signature.contains("pub enabled: bool,"));
        assert!(!signature.contains("hidden"));
    }

    fn doc_block(text: &str) -> ast::DocBlock {
        ast::DocBlock {
            span: Span::default(),
            lines: vec![ast::DocLine {
                span: Span::default(),
                text: text.to_string(),
            }],
        }
    }

    fn path_type(
        file_id: kernc_utils::FileId,
        start: usize,
        end: usize,
        segment: kernc_utils::SymbolId,
    ) -> ast::TypeNode {
        ast::TypeNode {
            id: NodeId(0),
            span: Span {
                file: file_id,
                start,
                end,
            },
            kind: ast::TypeKind::Path {
                anchor: None,
                segments: vec![ast::TypePathSegment {
                    name: segment,
                    name_span: Span {
                        file: file_id,
                        start,
                        end,
                    },
                    args: Vec::new(),
                }],
            },
        }
    }

    fn void_type() -> ast::TypeNode {
        ast::TypeNode {
            id: NodeId(0),
            span: Span::default(),
            kind: ast::TypeKind::Void,
        }
    }

    fn pointer_type(
        file_id: kernc_utils::FileId,
        start: usize,
        end: usize,
        elem: ast::TypeNode,
    ) -> ast::TypeNode {
        ast::TypeNode {
            id: NodeId(0),
            span: Span {
                file: file_id,
                start,
                end,
            },
            kind: ast::TypeKind::Pointer {
                is_mut: false,
                elem: Box::new(elem),
            },
        }
    }
}
