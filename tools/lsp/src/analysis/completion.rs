//! Completion item construction and ranking.
//!
//! This module maps compiler completion entries and language keywords into IDE
//! completion items with stable sort keys and snippets.

use super::ide::{IdeCompletionItem, IdeCompletionKind};
use super::text::CompletionContext;
use kernc_driver::{AnalysisCompletionItem, AnalysisCompletionKind};

pub(super) fn completion_sort_key(
    item: &AnalysisCompletionItem,
    prefix: &str,
    context: CompletionContext,
) -> (u8, u8, usize, String) {
    let exact = (!prefix.is_empty() && item.label == prefix) as u8;
    (
        completion_context_rank(item.kind, context),
        1_u8.saturating_sub(exact),
        item.label.len(),
        item.label.to_ascii_lowercase(),
    )
}

fn completion_context_rank(kind: AnalysisCompletionKind, context: CompletionContext) -> u8 {
    match context {
        CompletionContext::Type => {
            (!matches!(
                kind,
                AnalysisCompletionKind::Struct
                    | AnalysisCompletionKind::Union
                    | AnalysisCompletionKind::Enum
                    | AnalysisCompletionKind::Trait
                    | AnalysisCompletionKind::TypeAlias
                    | AnalysisCompletionKind::TypeParameter
            )) as u8
        }
        CompletionContext::Value => {
            (!matches!(
                kind,
                AnalysisCompletionKind::Variable
                    | AnalysisCompletionKind::Function
                    | AnalysisCompletionKind::Constant
                    | AnalysisCompletionKind::Static
            )) as u8
        }
    }
}

pub(super) fn keyword_completion_item(label: &str) -> IdeCompletionItem {
    let insert_text = keyword_completion_insert_text(label);

    IdeCompletionItem {
        label: label.to_string(),
        kind: IdeCompletionKind::Keyword,
        detail: Some("keyword".to_string()),
        insert_text,
        documentation: None,
        resolve_data: None,
    }
}

fn keyword_completion_insert_text(label: &str) -> Option<String> {
    match label {
        "extern" => Some("extern fn ${1:name}(${2:args}) ${3:i32} {\n    $0\n}".to_string()),
        "fn" => Some("fn ${1:name}(${2:args}) ${3:void} {\n    $0\n}".to_string()),
        "let" => Some("let ${1:name} = ${0};".to_string()),
        "const" => Some("const ${1:name}: ${2:Type} = ${0};".to_string()),
        "static" => Some("static ${1:name}: ${2:Type} = ${0};".to_string()),
        "type" => Some("type ${1:Name} = ${0};".to_string()),
        "if" => Some("if ${1:cond} {\n    $0\n}".to_string()),
        "for" => Some("for ${1:item} in ${2:iter} {\n    $0\n}".to_string()),
        "while" => Some("while ${1:cond} {\n    $0\n}".to_string()),
        "match" => Some("match ${1:value} {\n    $0\n}".to_string()),
        "use" => Some("use ${1:path};".to_string()),
        "impl" => Some("impl ${1:Type} {\n    $0\n}".to_string()),
        "mod" => Some("mod ${1:name};".to_string()),
        "defer" => Some("defer {\n    $0\n}".to_string()),
        "struct" => Some("struct {\n    $0\n}".to_string()),
        "union" => Some("union {\n    $0\n}".to_string()),
        "enum" => Some("enum {\n    $0\n}".to_string()),
        "trait" => Some("trait {\n    $0\n}".to_string()),
        _ => None,
    }
}
