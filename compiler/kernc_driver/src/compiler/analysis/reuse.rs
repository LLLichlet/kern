//! Incremental analysis reuse helpers.
//!
//! Reuse compares normalized module paths, source contents, structure snapshots,
//! and body-only changes so analysis can preserve cached semantic artifacts when
//! edits do not invalidate them.

use super::*;
use kernc_utils::{Interner, SymbolId};

pub(super) fn module_file_id(
    defs: &[kernc_sema::def::Def],
    module_id: DefId,
) -> kernc_utils::FileId {
    match &defs[module_id.0 as usize] {
        kernc_sema::def::Def::Module(module) => module.file_id,
        _ => kernc_utils::FileId(0),
    }
}

pub(super) fn module_analysis_path(
    session: &Session,
    defs: &[kernc_sema::def::Def],
    module_id: DefId,
    module_ast: &ast::Module,
) -> PathBuf {
    let file_id = module_file_id(defs, module_id);
    session
        .source_manager
        .get_file_path(file_id)
        .map(|path| module_analysis_path_from_source(path, module_ast))
        .unwrap_or_default()
}

pub(in crate::compiler) fn module_analysis_path_from_source(
    source_path: &Path,
    module_ast: &ast::Module,
) -> PathBuf {
    let source_path = normalize_driver_path(source_path);
    if module_ast.path == source_path.to_string_lossy() {
        source_path
    } else {
        PathBuf::from(module_ast.path.as_str())
    }
}

pub(super) fn normalize_driver_path(path: &Path) -> PathBuf {
    normalize_platform_path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
}

fn normalize_platform_path(path: PathBuf) -> PathBuf {
    let path = strip_windows_verbatim_prefix(path);
    strip_macos_private_var_prefix(path)
}

#[cfg(windows)]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("\\\\?\\UNC\\") {
        return PathBuf::from(format!("\\\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix("\\\\?\\") {
        return PathBuf::from(stripped);
    }
    path
}

#[cfg(not(windows))]
fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    path
}

#[cfg(target_os = "macos")]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix("/private/var/") {
        return PathBuf::from(format!("/var/{stripped}"));
    }
    if raw == "/private/var" {
        return PathBuf::from("/var");
    }
    path
}

#[cfg(not(target_os = "macos"))]
fn strip_macos_private_var_prefix(path: PathBuf) -> PathBuf {
    path
}

pub(super) fn module_source_changed(
    clean_session: &Session,
    clean_file_id: kernc_utils::FileId,
    parsed_session: &Session,
    parsed_file_id: kernc_utils::FileId,
) -> bool {
    let clean_source = clean_session
        .source_manager
        .get_file(clean_file_id)
        .map(|file| file.src.as_ref());
    let parsed_source = parsed_session
        .source_manager
        .get_file(parsed_file_id)
        .map(|file| file.src.as_ref());
    clean_source != parsed_source
}

pub(super) fn classify_function_body_decl_changes<'a>(
    clean_session: &Session,
    clean_module: &ast::Module,
    dirty_session: &Session,
    dirty_module: &ast::Module,
    item_ids: &mut std::slice::Iter<'a, DefId>,
    module_scope: ScopeId,
    worklist: &mut Vec<(DefId, ScopeId)>,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    if clean_module.decls.len() != dirty_module.decls.len() {
        return false;
    }

    for (clean_decl, dirty_decl) in clean_module.decls.iter().zip(&dirty_module.decls) {
        match (&clean_decl.kind, &dirty_decl.kind) {
            (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                if decls_equal_ignoring_ids_and_spans(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    continue;
                }
                if decls_equal_ignoring_body_only(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    worklist.push((def_id, module_scope));
                    replaced_spans.push(AnalysisSpanReplacement {
                        clean: clean_decl.span,
                        dirty: dirty_decl.span,
                    });
                    continue;
                }
                return false;
            }
            (ast::DeclKind::Impl { .. }, ast::DeclKind::Impl { .. }) => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                if decls_equal_ignoring_ids_and_spans(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    continue;
                }
                if decls_equal_ignoring_body_only(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    worklist.push((def_id, module_scope));
                    if !collect_impl_method_replacements(clean_decl, dirty_decl, replaced_spans) {
                        return false;
                    }
                    continue;
                }
                return false;
            }
            (
                ast::DeclKind::ExternBlock { decls: clean, .. },
                ast::DeclKind::ExternBlock { decls: dirty, .. },
            ) => {
                if !classify_function_body_decls(
                    clean_session,
                    clean,
                    dirty_session,
                    dirty,
                    item_ids,
                    module_scope,
                    worklist,
                    replaced_spans,
                ) {
                    return false;
                }
            }
            _ => {
                if matches!(
                    clean_decl.kind,
                    ast::DeclKind::Var { .. } | ast::DeclKind::TypeAlias { .. }
                ) {
                    let Some(_def_id) = item_ids.next().copied() else {
                        return false;
                    };
                }
                if !decls_equal_ignoring_ids_and_spans(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    return false;
                }
            }
        }
    }

    true
}

fn classify_function_body_decls<'a>(
    clean_session: &Session,
    clean_decls: &[ast::Decl],
    dirty_session: &Session,
    dirty_decls: &[ast::Decl],
    item_ids: &mut std::slice::Iter<'a, DefId>,
    module_scope: ScopeId,
    worklist: &mut Vec<(DefId, ScopeId)>,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    if clean_decls.len() != dirty_decls.len() {
        return false;
    }

    for (clean_decl, dirty_decl) in clean_decls.iter().zip(dirty_decls) {
        match (&clean_decl.kind, &dirty_decl.kind) {
            (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) => {
                let Some(_def_id) = item_ids.next().copied() else {
                    return false;
                };
                if !decls_equal_ignoring_ids_and_spans(
                    clean_session,
                    clean_decl,
                    dirty_session,
                    dirty_decl,
                ) {
                    return false;
                }
            }
            _ => return false,
        }
    }

    let _ = module_scope;
    let _ = worklist;
    let _ = replaced_spans;
    true
}

fn collect_impl_method_replacements(
    clean_decl: &ast::Decl,
    dirty_decl: &ast::Decl,
    replaced_spans: &mut Vec<AnalysisSpanReplacement>,
) -> bool {
    let (
        ast::DeclKind::Impl {
            decls: clean_methods,
            ..
        },
        ast::DeclKind::Impl {
            decls: dirty_methods,
            ..
        },
    ) = (&clean_decl.kind, &dirty_decl.kind)
    else {
        return false;
    };

    if clean_methods.len() != dirty_methods.len() {
        return false;
    }

    for (clean_method, dirty_method) in clean_methods.iter().zip(dirty_methods) {
        let (ast::DeclKind::Function { .. }, ast::DeclKind::Function { .. }) =
            (&clean_method.kind, &dirty_method.kind)
        else {
            return false;
        };
        replaced_spans.push(AnalysisSpanReplacement {
            clean: clean_method.span,
            dirty: dirty_method.span,
        });
    }

    true
}

fn decls_equal_ignoring_ids_and_spans(
    left_session: &Session,
    left: &ast::Decl,
    right_session: &Session,
    right: &ast::Decl,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_reuse_comparison(&mut left);
    normalize_decl_for_reuse_comparison(&mut right);
    canonicalize_decl_symbols_for_comparison(left_session, &mut left, right_session, &mut right);
    left == right
}

fn decls_equal_ignoring_body_only(
    left_session: &Session,
    left: &ast::Decl,
    right_session: &Session,
    right: &ast::Decl,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_body_only_comparison(&mut left);
    normalize_decl_for_body_only_comparison(&mut right);
    canonicalize_decl_symbols_for_comparison(left_session, &mut left, right_session, &mut right);
    left == right
}

fn normalize_decl_for_reuse_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
    if let Some(docs) = &mut decl.docs {
        normalize_doc_block_for_comparison(docs);
    }
    normalize_attributes_for_body_only_comparison(&mut decl.attributes);

    match &mut decl.kind {
        ast::DeclKind::Function {
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            if let Some(body) = body {
                normalize_expr_for_body_only_comparison(body);
            }
        }
        ast::DeclKind::Var {
            type_node, value, ..
        } => {
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            if let Some(value) = value {
                normalize_expr_for_body_only_comparison(value);
            }
        }
        ast::DeclKind::TypeAlias {
            generics,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
        }
        ast::DeclKind::Struct {
            generics,
            where_clauses,
            fields,
            ..
        }
        | ast::DeclKind::Union {
            generics,
            where_clauses,
            fields,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for field in fields {
                normalize_struct_field_for_body_only_comparison(field);
            }
        }
        ast::DeclKind::Enum {
            generics,
            where_clauses,
            backing_type,
            variants,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            if let Some(backing_type) = backing_type {
                normalize_type_for_body_only_comparison(backing_type);
            }
            for variant in variants {
                normalize_enum_variant_for_body_only_comparison(variant);
            }
        }
        ast::DeclKind::Trait {
            generics,
            where_clauses,
            supertraits,
            assoc_types,
            methods,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for supertrait in supertraits {
                normalize_type_for_body_only_comparison(supertrait);
            }
            normalize_trait_items_for_body_only_comparison(assoc_types, methods);
        }
        ast::DeclKind::Use { target, .. } => normalize_use_target_for_body_only_comparison(target),
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                normalize_decl_for_reuse_comparison(child);
            }
        }
        ast::DeclKind::Impl {
            generics,
            where_clauses,
            target_type,
            trait_type,
            decls,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target_type);
            if let Some(trait_type) = trait_type {
                normalize_type_for_body_only_comparison(trait_type);
            }
            for child in decls {
                normalize_decl_for_reuse_comparison(child);
            }
        }
        ast::DeclKind::Mod { decls } => {
            if let Some(decls) = decls {
                for decl in decls {
                    normalize_decl_for_reuse_comparison(decl);
                }
            }
        }
    }
}

pub(super) fn rebind_module_defs(
    ctx: &mut SemaContext<'_>,
    module_id: DefId,
    parsed_module: &ParsedModule,
) -> bool {
    let (module_scope, item_ids) = match &mut ctx.defs[module_id.0 as usize] {
        kernc_sema::def::Def::Module(module) => {
            module.file_id = parsed_module.file_id;
            (module.scope_id, module.items.clone())
        }
        _ => return false,
    };

    let mut iter = item_ids.iter();
    if !rebind_decl_sequence(ctx, module_scope, &mut iter, &parsed_module.ast.decls) {
        return false;
    }

    iter.next().is_none()
}

fn rebind_decl_sequence<'a>(
    ctx: &mut SemaContext<'_>,
    module_scope: ScopeId,
    item_ids: &mut std::slice::Iter<'a, DefId>,
    decls: &[ast::Decl],
) -> bool {
    for decl in decls {
        match &decl.kind {
            ast::DeclKind::Function { body, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let kernc_sema::def::Def::Function(function) = &mut ctx.defs[def_id.0 as usize]
                else {
                    return false;
                };
                let name = function.name;
                function.span = decl.span;
                function.name_span = decl.name_span;
                function.body = body.clone();
                let _ = ctx
                    .scopes
                    .update_span_in_scope(module_scope, name, decl.name_span);
            }
            ast::DeclKind::Var { value, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let kernc_sema::def::Def::Global(global) = &mut ctx.defs[def_id.0 as usize] else {
                    return false;
                };
                let name = global.name;
                global.span = decl.span;
                global.value = value.clone();
                let _ = ctx
                    .scopes
                    .update_span_in_scope(module_scope, name, decl.name_span);
            }
            ast::DeclKind::TypeAlias { target, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let name = match &ctx.defs[def_id.0 as usize] {
                    kernc_sema::def::Def::TypeAlias(alias_def) => alias_def.name,
                    _ => return false,
                };
                match (&mut ctx.defs[def_id.0 as usize], &target.kind) {
                    (kernc_sema::def::Def::TypeAlias(alias_def), _) => {
                        alias_def.span = decl.span;
                    }
                    _ => {
                        return false;
                    }
                }
                let _ = ctx
                    .scopes
                    .update_span_in_scope(module_scope, name, decl.name_span);
            }
            ast::DeclKind::Struct { .. }
            | ast::DeclKind::Union { .. }
            | ast::DeclKind::Enum { .. }
            | ast::DeclKind::Trait { .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let name = match &mut ctx.defs[def_id.0 as usize] {
                    kernc_sema::def::Def::Struct(def) => {
                        def.span = decl.span;
                        def.name
                    }
                    kernc_sema::def::Def::Union(def) => {
                        def.span = decl.span;
                        def.name
                    }
                    kernc_sema::def::Def::Enum(def) => {
                        def.span = decl.span;
                        def.name
                    }
                    kernc_sema::def::Def::Trait(def) => {
                        def.span = decl.span;
                        def.name
                    }
                    _ => return false,
                };
                let _ = ctx
                    .scopes
                    .update_span_in_scope(module_scope, name, decl.name_span);
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                if !rebind_decl_sequence(ctx, module_scope, item_ids, decls) {
                    return false;
                }
            }
            ast::DeclKind::Impl { decls, .. } => {
                let Some(def_id) = item_ids.next().copied() else {
                    return false;
                };
                let method_ids = match &mut ctx.defs[def_id.0 as usize] {
                    kernc_sema::def::Def::Impl(impl_def) => {
                        impl_def.span = decl.span;
                        impl_def.methods.clone()
                    }
                    _ => return false,
                };
                let mut method_iter = method_ids.iter();
                if !rebind_impl_methods(ctx, &mut method_iter, decls) {
                    return false;
                }
                if method_iter.next().is_some() {
                    return false;
                }
            }
            ast::DeclKind::Use { .. } | ast::DeclKind::Mod { .. } => {}
        }
    }

    true
}

fn rebind_impl_methods<'a>(
    ctx: &mut SemaContext<'_>,
    method_ids: &mut std::slice::Iter<'a, DefId>,
    decls: &[ast::Decl],
) -> bool {
    for decl in decls {
        let ast::DeclKind::Function { body, .. } = &decl.kind else {
            return false;
        };
        let Some(def_id) = method_ids.next().copied() else {
            return false;
        };
        let kernc_sema::def::Def::Function(function) = &mut ctx.defs[def_id.0 as usize] else {
            return false;
        };
        function.span = decl.span;
        function.name_span = decl.name_span;
        function.body = body.clone();
    }

    true
}

pub(super) fn modules_match_ignoring_body_only(
    left_session: &Session,
    left: &ast::Module,
    right_session: &Session,
    right: &ast::Module,
) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_module_for_body_only_comparison(&mut left);
    normalize_module_for_body_only_comparison(&mut right);
    canonicalize_module_symbols_for_comparison(left_session, &mut left, right_session, &mut right);
    left == right
}

fn normalize_module_for_body_only_comparison(module: &mut ast::Module) {
    if let Some(docs) = &mut module.docs {
        normalize_doc_block_for_comparison(docs);
    }
    normalize_attributes_for_body_only_comparison(&mut module.attributes);
    for decl in &mut module.decls {
        normalize_decl_for_body_only_comparison(decl);
    }
}

fn normalize_doc_block_for_comparison(docs: &mut ast::DocBlock) {
    docs.span = Span::default();
    for line in &mut docs.lines {
        line.span = Span::default();
    }
}

fn canonicalize_module_symbols_for_comparison(
    left_session: &Session,
    left: &mut ast::Module,
    right_session: &Session,
    right: &mut ast::Module,
) {
    let mut canonical = Interner::new();
    canonicalize_module_symbols(left_session, &mut canonical, left);
    canonicalize_module_symbols(right_session, &mut canonical, right);
}

fn canonicalize_decl_symbols_for_comparison(
    left_session: &Session,
    left: &mut ast::Decl,
    right_session: &Session,
    right: &mut ast::Decl,
) {
    let mut canonical = Interner::new();
    canonicalize_decl_symbols(left_session, &mut canonical, left);
    canonicalize_decl_symbols(right_session, &mut canonical, right);
}

fn canonicalize_module_symbols(
    session: &Session,
    canonical: &mut Interner,
    module: &mut ast::Module,
) {
    for attribute in &mut module.attributes {
        canonicalize_attribute_symbols(session, canonical, attribute);
    }
    for decl in &mut module.decls {
        canonicalize_decl_symbols(session, canonical, decl);
    }
}

fn canonicalize_decl_symbols(session: &Session, canonical: &mut Interner, decl: &mut ast::Decl) {
    canonicalize_symbol(session, canonical, &mut decl.name);
    for attribute in &mut decl.attributes {
        canonicalize_attribute_symbols(session, canonical, attribute);
    }

    match &mut decl.kind {
        ast::DeclKind::Function {
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            ..
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            for param in params {
                canonicalize_func_param_symbols(session, canonical, param);
            }
            canonicalize_type_symbols(session, canonical, ret_type);
            if let Some(body) = body {
                canonicalize_expr_symbols(session, canonical, body);
            }
        }
        ast::DeclKind::Var {
            type_node, value, ..
        } => {
            if let Some(type_node) = type_node {
                canonicalize_type_symbols(session, canonical, type_node);
            }
            if let Some(value) = value {
                canonicalize_expr_symbols(session, canonical, value);
            }
        }
        ast::DeclKind::TypeAlias {
            generics,
            where_clauses,
            target,
            ..
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            canonicalize_type_symbols(session, canonical, target);
        }
        ast::DeclKind::Struct {
            generics,
            where_clauses,
            fields,
            ..
        }
        | ast::DeclKind::Union {
            generics,
            where_clauses,
            fields,
            ..
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            for field in fields {
                canonicalize_struct_field_symbols(session, canonical, field);
            }
        }
        ast::DeclKind::Enum {
            generics,
            where_clauses,
            backing_type,
            variants,
            ..
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            if let Some(backing_type) = backing_type {
                canonicalize_type_symbols(session, canonical, backing_type);
            }
            for variant in variants {
                canonicalize_enum_variant_symbols(session, canonical, variant);
            }
        }
        ast::DeclKind::Trait {
            generics,
            where_clauses,
            supertraits,
            assoc_types,
            methods,
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            for supertrait in supertraits {
                canonicalize_type_symbols(session, canonical, supertrait);
            }
            for assoc in assoc_types {
                canonicalize_assoc_type_symbols(session, canonical, assoc);
            }
            for method in methods {
                canonicalize_trait_method_symbols(session, canonical, method);
            }
        }
        ast::DeclKind::Mod { decls } => {
            if let Some(decls) = decls {
                for child in decls {
                    canonicalize_decl_symbols(session, canonical, child);
                }
            }
        }
        ast::DeclKind::Use { path, target, .. } => {
            canonicalize_symbol_slice(session, canonical, path);
            canonicalize_use_target_symbols(session, canonical, target);
        }
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                canonicalize_decl_symbols(session, canonical, child);
            }
        }
        ast::DeclKind::Impl {
            generics,
            where_clauses,
            target_type,
            trait_type,
            decls,
        } => {
            canonicalize_generics_symbols(session, canonical, generics);
            canonicalize_where_clause_symbols(session, canonical, where_clauses);
            canonicalize_type_symbols(session, canonical, target_type);
            if let Some(trait_type) = trait_type {
                canonicalize_type_symbols(session, canonical, trait_type);
            }
            for child in decls {
                canonicalize_decl_symbols(session, canonical, child);
            }
        }
    }
}

fn canonicalize_attribute_symbols(
    session: &Session,
    canonical: &mut Interner,
    attribute: &mut ast::Attribute,
) {
    match &mut attribute.kind {
        ast::AttributeKind::If(expr) => canonicalize_expr_symbols(session, canonical, expr),
        ast::AttributeKind::Meta(items) => {
            for item in items {
                match item {
                    ast::MetaItem::Marker(name) => canonicalize_symbol(session, canonical, name),
                    ast::MetaItem::Call(name, expr) => {
                        canonicalize_symbol(session, canonical, name);
                        canonicalize_expr_symbols(session, canonical, expr);
                    }
                }
            }
        }
    }
}

fn canonicalize_generics_symbols(
    session: &Session,
    canonical: &mut Interner,
    generics: &mut [ast::GenericParam],
) {
    for generic in generics {
        canonicalize_symbol(session, canonical, &mut generic.name);
        match &mut generic.kind {
            ast::GenericParamKind::Type => {}
            ast::GenericParamKind::Const { ty } => {
                canonicalize_type_symbols(session, canonical, ty)
            }
        }
    }
}

fn canonicalize_where_clause_symbols(
    session: &Session,
    canonical: &mut Interner,
    clauses: &mut [ast::WhereClause],
) {
    for clause in clauses {
        canonicalize_type_symbols(session, canonical, &mut clause.target_ty);
        for bound in &mut clause.bounds {
            canonicalize_type_symbols(session, canonical, bound);
        }
    }
}

fn canonicalize_func_param_symbols(
    session: &Session,
    canonical: &mut Interner,
    param: &mut ast::FuncParam,
) {
    canonicalize_binding_pattern_symbols(session, canonical, &mut param.pattern);
    canonicalize_type_symbols(session, canonical, &mut param.type_node);
}

fn canonicalize_binding_pattern_symbols(
    session: &Session,
    canonical: &mut Interner,
    pattern: &mut ast::BindingPattern,
) {
    canonicalize_symbol(session, canonical, &mut pattern.name);
}

fn canonicalize_use_target_symbols(
    session: &Session,
    canonical: &mut Interner,
    target: &mut ast::UseTarget,
) {
    match target {
        ast::UseTarget::Module(alias) => {
            if let Some(alias) = alias {
                canonicalize_symbol(session, canonical, alias);
            }
        }
        ast::UseTarget::Tree(items) => {
            for item in items {
                canonicalize_use_tree_symbols(session, canonical, item);
            }
        }
    }
}

fn canonicalize_use_tree_symbols(
    session: &Session,
    canonical: &mut Interner,
    tree: &mut ast::UseTree,
) {
    match tree {
        ast::UseTree::SelfModule { alias, .. } => {
            if let Some(alias) = alias {
                canonicalize_symbol(session, canonical, alias);
            }
        }
        ast::UseTree::Path {
            path,
            alias,
            nested,
            ..
        } => {
            canonicalize_symbol_slice(session, canonical, path);
            if let Some(alias) = alias {
                canonicalize_symbol(session, canonical, alias);
            }
            if let Some(nested) = nested {
                for item in nested {
                    canonicalize_use_tree_symbols(session, canonical, item);
                }
            }
        }
    }
}

fn canonicalize_type_symbols(session: &Session, canonical: &mut Interner, ty: &mut ast::TypeNode) {
    match &mut ty.kind {
        ast::TypeKind::Path { segments, .. } => {
            for segment in segments {
                canonicalize_symbol(session, canonical, &mut segment.name);
                for arg in &mut segment.args {
                    canonicalize_generic_arg_symbols(session, canonical, arg);
                }
            }
        }
        ast::TypeKind::Optional { inner } => canonicalize_type_symbols(session, canonical, inner),
        ast::TypeKind::Result { ok, err } => {
            canonicalize_type_symbols(session, canonical, ok);
            canonicalize_type_symbols(session, canonical, err);
        }
        ast::TypeKind::Range { start, end, .. } => {
            if let Some(start) = start {
                canonicalize_type_symbols(session, canonical, start);
            }
            if let Some(end) = end {
                canonicalize_type_symbols(session, canonical, end);
            }
        }
        ast::TypeKind::Pointer { elem, .. }
        | ast::TypeKind::VolatilePtr { elem, .. }
        | ast::TypeKind::ArrayInfer { elem, .. }
        | ast::TypeKind::Slice { elem, .. } => canonicalize_type_symbols(session, canonical, elem),
        ast::TypeKind::Array { elem, len } => {
            canonicalize_type_symbols(session, canonical, elem);
            canonicalize_expr_symbols(session, canonical, len);
        }
        ast::TypeKind::Function { params, ret, .. }
        | ast::TypeKind::ClosureInterface { params, ret } => {
            for param in params {
                canonicalize_type_symbols(session, canonical, param);
            }
            if let Some(ret) = ret {
                canonicalize_type_symbols(session, canonical, ret);
            }
        }
        ast::TypeKind::Struct { fields, .. } | ast::TypeKind::Union { fields, .. } => {
            for field in fields {
                canonicalize_struct_field_symbols(session, canonical, field);
            }
        }
        ast::TypeKind::Enum {
            backing_type,
            variants,
        } => {
            if let Some(backing_type) = backing_type {
                canonicalize_type_symbols(session, canonical, backing_type);
            }
            for variant in variants {
                canonicalize_enum_variant_symbols(session, canonical, variant);
            }
        }
        ast::TypeKind::Trait {
            assoc_types,
            methods,
        } => {
            for assoc in assoc_types {
                canonicalize_assoc_type_symbols(session, canonical, assoc);
            }
            for method in methods {
                canonicalize_trait_method_symbols(session, canonical, method);
            }
        }
        ast::TypeKind::TypeOf(expr) => canonicalize_expr_symbols(session, canonical, expr),
        ast::TypeKind::Error
        | ast::TypeKind::Infer
        | ast::TypeKind::SelfType
        | ast::TypeKind::Never
        | ast::TypeKind::Void => {}
    }
}

fn canonicalize_generic_arg_symbols(
    session: &Session,
    canonical: &mut Interner,
    arg: &mut ast::GenericArg,
) {
    match arg {
        ast::GenericArg::Type(ty) => canonicalize_type_symbols(session, canonical, ty),
        ast::GenericArg::ConstExpr(expr) => canonicalize_expr_symbols(session, canonical, expr),
        ast::GenericArg::AssocBinding { name, value, .. } => {
            canonicalize_symbol(session, canonical, name);
            canonicalize_type_symbols(session, canonical, value);
        }
    }
}

fn canonicalize_struct_field_symbols(
    session: &Session,
    canonical: &mut Interner,
    field: &mut ast::StructFieldDef,
) {
    canonicalize_symbol(session, canonical, &mut field.name);
    canonicalize_type_symbols(session, canonical, &mut field.type_node);
    if let Some(value) = &mut field.default_value {
        canonicalize_expr_symbols(session, canonical, value);
    }
}

fn canonicalize_enum_variant_symbols(
    session: &Session,
    canonical: &mut Interner,
    variant: &mut ast::EnumVariant,
) {
    canonicalize_symbol(session, canonical, &mut variant.name);
    if let Some(payload_type) = &mut variant.payload_type {
        canonicalize_type_symbols(session, canonical, payload_type);
    }
    if let Some(value) = &mut variant.value {
        canonicalize_expr_symbols(session, canonical, value);
    }
}

fn canonicalize_assoc_type_symbols(
    session: &Session,
    canonical: &mut Interner,
    assoc: &mut ast::AssociatedTypeDecl,
) {
    canonicalize_symbol(session, canonical, &mut assoc.name);
    canonicalize_generics_symbols(session, canonical, &mut assoc.generics);
    for bound in &mut assoc.bounds {
        canonicalize_type_symbols(session, canonical, bound);
    }
    canonicalize_where_clause_symbols(session, canonical, &mut assoc.where_clauses);
}

fn canonicalize_trait_method_symbols(
    session: &Session,
    canonical: &mut Interner,
    method: &mut ast::TraitMethodDef,
) {
    canonicalize_struct_field_symbols(session, canonical, &mut method.signature);
    for param in &mut method.params {
        canonicalize_func_param_symbols(session, canonical, param);
    }
    if let Some(body) = &mut method.body {
        canonicalize_expr_symbols(session, canonical, body);
    }
}

fn canonicalize_expr_symbols(session: &Session, canonical: &mut Interner, expr: &mut ast::Expr) {
    match &mut expr.kind {
        ast::ExprKind::Let {
            pattern,
            type_node,
            init,
            else_clause,
            ..
        } => {
            canonicalize_let_pattern_symbols(session, canonical, pattern);
            if let Some(type_node) = type_node {
                canonicalize_type_symbols(session, canonical, type_node);
            }
            canonicalize_expr_symbols(session, canonical, init);
            if let Some(else_clause) = else_clause {
                canonicalize_let_else_clause_symbols(session, canonical, else_clause);
            }
        }
        ast::ExprKind::Static {
            pattern,
            type_node,
            init,
            ..
        } => {
            canonicalize_binding_pattern_symbols(session, canonical, pattern);
            if let Some(type_node) = type_node {
                canonicalize_type_symbols(session, canonical, type_node);
            }
            if let Some(init) = init {
                canonicalize_expr_symbols(session, canonical, init);
            }
        }
        ast::ExprKind::Identifier(name) => canonicalize_symbol(session, canonical, name),
        ast::ExprKind::AnchoredPath { name, .. } => {
            canonicalize_symbol(session, canonical, name);
        }
        ast::ExprKind::TypeNode(type_node) => {
            canonicalize_type_symbols(session, canonical, type_node);
        }
        ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
            canonicalize_expr_symbols(session, canonical, lhs);
            canonicalize_expr_symbols(session, canonical, rhs);
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                canonicalize_expr_symbols(session, canonical, start);
            }
            if let Some(end) = end {
                canonicalize_expr_symbols(session, canonical, end);
            }
        }
        ast::ExprKind::Unary { operand, .. } => {
            canonicalize_expr_symbols(session, canonical, operand);
        }
        ast::ExprKind::Grouped { expr } => canonicalize_expr_symbols(session, canonical, expr),
        ast::ExprKind::FieldAccess { lhs, field, .. } => {
            canonicalize_expr_symbols(session, canonical, lhs);
            canonicalize_symbol(session, canonical, field);
        }
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            canonicalize_expr_symbols(session, canonical, lhs);
            canonicalize_expr_symbols(session, canonical, index);
        }
        ast::ExprKind::Call { callee, args } => {
            canonicalize_expr_symbols(session, canonical, callee);
            for arg in args {
                canonicalize_expr_symbols(session, canonical, arg);
            }
        }
        ast::ExprKind::DataInit { type_node, literal } => {
            if let Some(type_node) = type_node {
                canonicalize_type_symbols(session, canonical, type_node);
            }
            canonicalize_data_literal_symbols(session, canonical, literal);
        }
        ast::ExprKind::EnumLiteral { variant, .. } => {
            canonicalize_symbol(session, canonical, variant);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            canonicalize_expr_symbols(session, canonical, cond);
            canonicalize_expr_symbols(session, canonical, then_branch);
            if let Some(else_branch) = else_branch {
                canonicalize_expr_symbols(session, canonical, else_branch);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            canonicalize_expr_symbols(session, canonical, target);
            for arm in arms {
                for pattern in &mut arm.patterns {
                    canonicalize_match_pattern_symbols(session, canonical, pattern);
                }
                canonicalize_expr_symbols(session, canonical, &mut arm.body);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                for attribute in &mut stmt.attributes {
                    canonicalize_attribute_symbols(session, canonical, attribute);
                }
                match &mut stmt.kind {
                    ast::StmtKind::Use(use_stmt) => {
                        canonicalize_symbol_slice(session, canonical, &mut use_stmt.path);
                        canonicalize_use_target_symbols(session, canonical, &mut use_stmt.target);
                    }
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        canonicalize_expr_symbols(session, canonical, expr);
                    }
                }
            }
            if let Some(result) = result {
                canonicalize_expr_symbols(session, canonical, result);
            }
        }
        ast::ExprKind::While { cond, body } => {
            canonicalize_expr_symbols(session, canonical, cond);
            canonicalize_expr_symbols(session, canonical, body);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            canonicalize_expr_symbols(session, canonical, lhs);
            if let Some(start) = start {
                canonicalize_expr_symbols(session, canonical, start);
            }
            if let Some(end) = end {
                canonicalize_expr_symbols(session, canonical, end);
            }
        }
        ast::ExprKind::Defer { expr } => canonicalize_expr_symbols(session, canonical, expr),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                canonicalize_expr_symbols(session, canonical, value);
            }
        }
        ast::ExprKind::As { lhs, target } => {
            canonicalize_expr_symbols(session, canonical, lhs);
            canonicalize_type_symbols(session, canonical, target);
        }
        ast::ExprKind::Propagate { operand } => {
            canonicalize_expr_symbols(session, canonical, operand);
        }
        ast::ExprKind::GenericInstantiation { target, args } => {
            canonicalize_expr_symbols(session, canonical, target);
            for arg in args {
                canonicalize_generic_arg_symbols(session, canonical, arg);
            }
        }
        ast::ExprKind::Closure {
            captures,
            params,
            ret_type,
            body,
        } => {
            for capture in captures {
                canonicalize_symbol(session, canonical, &mut capture.name);
                canonicalize_expr_symbols(session, canonical, &mut capture.value);
            }
            for param in params {
                canonicalize_func_param_symbols(session, canonical, param);
            }
            canonicalize_type_symbols(session, canonical, ret_type);
            canonicalize_expr_symbols(session, canonical, body);
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
    }
}

fn canonicalize_let_else_clause_symbols(
    session: &Session,
    canonical: &mut Interner,
    else_clause: &mut ast::LetElseClause,
) {
    match else_clause {
        ast::LetElseClause::Expr(expr) => canonicalize_expr_symbols(session, canonical, expr),
        ast::LetElseClause::Arms(arms) => {
            for arm in arms {
                canonicalize_pattern_symbols(session, canonical, &mut arm.pattern);
                canonicalize_expr_symbols(session, canonical, &mut arm.body);
            }
        }
    }
}

fn canonicalize_data_literal_symbols(
    session: &Session,
    canonical: &mut Interner,
    literal: &mut ast::DataLiteralKind,
) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                canonicalize_symbol(session, canonical, &mut field.name);
                canonicalize_expr_symbols(session, canonical, &mut field.value);
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                canonicalize_expr_symbols(session, canonical, item);
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            canonicalize_expr_symbols(session, canonical, value);
            canonicalize_expr_symbols(session, canonical, count);
        }
        ast::DataLiteralKind::Scalar(value) => canonicalize_expr_symbols(session, canonical, value),
    }
}

fn canonicalize_let_pattern_symbols(
    session: &Session,
    canonical: &mut Interner,
    pattern: &mut ast::LetPattern,
) {
    canonicalize_pattern_symbols(session, canonical, &mut pattern.pattern);
}

fn canonicalize_pattern_symbols(
    session: &Session,
    canonical: &mut Interner,
    pattern: &mut ast::Pattern,
) {
    match &mut pattern.kind {
        ast::PatternKind::Binding(binding) => {
            canonicalize_binding_pattern_symbols(session, canonical, binding);
        }
        ast::PatternKind::Ignore => {}
        ast::PatternKind::Value(value) => canonicalize_expr_symbols(session, canonical, value),
        ast::PatternKind::Variant(variant) => {
            canonicalize_symbol(session, canonical, &mut variant.variant_name);
            if let Some(target_type) = &mut variant.target_type {
                canonicalize_type_symbols(session, canonical, target_type);
            }
        }
        ast::PatternKind::Destructure(destructure) => {
            if let Some(target_type) = &mut destructure.target_type {
                canonicalize_type_symbols(session, canonical, target_type);
            }
            for field in &mut destructure.fields {
                canonicalize_symbol(session, canonical, &mut field.name);
                canonicalize_pattern_symbols(session, canonical, &mut field.pattern);
            }
        }
    }
}

fn canonicalize_match_pattern_symbols(
    session: &Session,
    canonical: &mut Interner,
    pattern: &mut ast::MatchPattern,
) {
    match &mut pattern.kind {
        ast::MatchPatternKind::Value(value) => canonicalize_expr_symbols(session, canonical, value),
        ast::MatchPatternKind::Pattern(inner) => {
            canonicalize_pattern_symbols(session, canonical, inner);
        }
    }
}

fn canonicalize_symbol_slice(
    session: &Session,
    canonical: &mut Interner,
    symbols: &mut [SymbolId],
) {
    for symbol in symbols {
        canonicalize_symbol(session, canonical, symbol);
    }
}

fn canonicalize_symbol(session: &Session, canonical: &mut Interner, symbol: &mut SymbolId) {
    let canonical_name;
    let name = match session.interner.resolve(*symbol) {
        Some(name) => name,
        None => {
            canonical_name = format!("\0unknown_symbol_{}", symbol.0);
            canonical_name.as_str()
        }
    };
    *symbol = canonical.intern(name);
}

fn normalize_decl_for_body_only_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
    if let Some(docs) = &mut decl.docs {
        normalize_doc_block_for_comparison(docs);
    }
    normalize_attributes_for_body_only_comparison(&mut decl.attributes);

    match &mut decl.kind {
        ast::DeclKind::Function {
            generics,
            where_clauses,
            params,
            ret_type,
            body,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            *body = None;
        }
        ast::DeclKind::Var {
            type_node, value, ..
        } => {
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            *value = value.as_ref().map(|_| placeholder_expr());
        }
        ast::DeclKind::TypeAlias {
            generics,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
        }
        ast::DeclKind::Struct {
            generics,
            where_clauses,
            fields,
            ..
        }
        | ast::DeclKind::Union {
            generics,
            where_clauses,
            fields,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for field in fields {
                normalize_struct_field_for_body_only_comparison(field);
            }
        }
        ast::DeclKind::Enum {
            generics,
            where_clauses,
            backing_type,
            variants,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            if let Some(backing_type) = backing_type {
                normalize_type_for_body_only_comparison(backing_type);
            }
            for variant in variants {
                normalize_enum_variant_for_body_only_comparison(variant);
            }
        }
        ast::DeclKind::Trait {
            generics,
            where_clauses,
            supertraits,
            assoc_types,
            methods,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            for supertrait in supertraits {
                normalize_type_for_body_only_comparison(supertrait);
            }
            normalize_trait_items_for_body_only_comparison(assoc_types, methods);
        }
        ast::DeclKind::Use { target, .. } => normalize_use_target_for_body_only_comparison(target),
        ast::DeclKind::ExternBlock { decls, .. } => {
            for child in decls {
                normalize_decl_for_body_only_comparison(child);
            }
        }
        ast::DeclKind::Impl {
            generics,
            where_clauses,
            target_type,
            trait_type,
            decls,
        } => {
            normalize_generics_for_body_only_comparison(generics);
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target_type);
            if let Some(trait_type) = trait_type {
                normalize_type_for_body_only_comparison(trait_type);
            }
            for child in decls {
                normalize_decl_for_body_only_comparison(child);
            }
        }
        ast::DeclKind::Mod { decls } => {
            if let Some(decls) = decls {
                for decl in decls {
                    normalize_decl_for_body_only_comparison(decl);
                }
            }
        }
    }
}

fn normalize_attributes_for_body_only_comparison(attributes: &mut [ast::Attribute]) {
    for attribute in attributes {
        attribute.span = Span::default();
        match &mut attribute.kind {
            ast::AttributeKind::If(expr) => normalize_expr_for_body_only_comparison(expr),
            ast::AttributeKind::Meta(items) => {
                for item in items {
                    if let ast::MetaItem::Call(_, expr) = item {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                }
            }
        }
    }
}

fn normalize_generics_for_body_only_comparison(generics: &mut [ast::GenericParam]) {
    for generic in generics {
        generic.span = Span::default();
    }
}

fn normalize_where_clauses_for_body_only_comparison(where_clauses: &mut [ast::WhereClause]) {
    for clause in where_clauses {
        clause.span = Span::default();
        normalize_type_for_body_only_comparison(&mut clause.target_ty);
        for bound in &mut clause.bounds {
            normalize_type_for_body_only_comparison(bound);
        }
    }
}

fn normalize_func_param_for_body_only_comparison(param: &mut ast::FuncParam) {
    param.span = Span::default();
    normalize_binding_pattern_for_body_only_comparison(&mut param.pattern);
    normalize_type_for_body_only_comparison(&mut param.type_node);
}

fn normalize_binding_pattern_for_body_only_comparison(pattern: &mut ast::BindingPattern) {
    pattern.name_span = Span::default();
    pattern.span = Span::default();
}

fn normalize_use_target_for_body_only_comparison(target: &mut ast::UseTarget) {
    if let ast::UseTarget::Tree(items) = target {
        for item in items {
            normalize_use_tree_for_body_only_comparison(item);
        }
    }
}

fn normalize_use_tree_for_body_only_comparison(tree: &mut ast::UseTree) {
    match tree {
        ast::UseTree::SelfModule {
            span, binding_span, ..
        } => {
            *span = Span::default();
            *binding_span = Span::default();
        }
        ast::UseTree::Path {
            nested,
            span,
            binding_span,
            ..
        } => {
            *span = Span::default();
            *binding_span = Span::default();
            if let Some(nested) = nested {
                for item in nested {
                    normalize_use_tree_for_body_only_comparison(item);
                }
            }
        }
    }
}

fn normalize_type_for_body_only_comparison(ty: &mut ast::TypeNode) {
    ty.id = NodeId(0);
    ty.span = Span::default();
    match &mut ty.kind {
        ast::TypeKind::Path { segments, .. } => {
            for segment in segments {
                segment.name_span = Span::default();
                for arg in &mut segment.args {
                    match arg {
                        ast::GenericArg::Type(generic) => {
                            normalize_type_for_body_only_comparison(generic);
                        }
                        ast::GenericArg::ConstExpr(expr) => {
                            normalize_expr_for_body_only_comparison(expr);
                        }
                        ast::GenericArg::AssocBinding {
                            name_span, value, ..
                        } => {
                            *name_span = Span::default();
                            normalize_type_for_body_only_comparison(value);
                        }
                    }
                }
            }
        }
        ast::TypeKind::Optional { inner } => normalize_type_for_body_only_comparison(inner),
        ast::TypeKind::Result { ok, err } => {
            normalize_type_for_body_only_comparison(ok);
            normalize_type_for_body_only_comparison(err);
        }
        ast::TypeKind::Range { start, end, .. } => {
            if let Some(start) = start {
                normalize_type_for_body_only_comparison(start);
            }
            if let Some(end) = end {
                normalize_type_for_body_only_comparison(end);
            }
        }
        ast::TypeKind::Pointer { elem, .. }
        | ast::TypeKind::VolatilePtr { elem, .. }
        | ast::TypeKind::ArrayInfer { elem, .. }
        | ast::TypeKind::Slice { elem, .. } => normalize_type_for_body_only_comparison(elem),
        ast::TypeKind::Array { elem, len, .. } => {
            normalize_type_for_body_only_comparison(elem);
            normalize_expr_for_body_only_comparison(len);
        }
        ast::TypeKind::Function { params, ret, .. }
        | ast::TypeKind::ClosureInterface { params, ret } => {
            for param in params {
                normalize_type_for_body_only_comparison(param);
            }
            if let Some(ret) = ret {
                normalize_type_for_body_only_comparison(ret);
            }
        }
        ast::TypeKind::Struct { fields, .. } | ast::TypeKind::Union { fields, .. } => {
            for field in fields {
                normalize_struct_field_for_body_only_comparison(field);
            }
        }
        ast::TypeKind::Trait {
            assoc_types,
            methods,
        } => {
            for assoc in assoc_types {
                if let Some(docs) = &mut assoc.docs {
                    normalize_doc_block_for_comparison(docs);
                }
                assoc.name_span = Span::default();
                assoc.span = Span::default();
                for bound in &mut assoc.bounds {
                    normalize_type_for_body_only_comparison(bound);
                }
                for clause in &mut assoc.where_clauses {
                    clause.span = Span::default();
                    normalize_type_for_body_only_comparison(&mut clause.target_ty);
                    for bound in &mut clause.bounds {
                        normalize_type_for_body_only_comparison(bound);
                    }
                }
            }
            for method in methods {
                normalize_trait_method_for_body_only_comparison(method);
            }
        }
        ast::TypeKind::Enum {
            backing_type,
            variants,
        } => {
            if let Some(backing_type) = backing_type {
                normalize_type_for_body_only_comparison(backing_type);
            }
            for variant in variants {
                if let Some(docs) = &mut variant.docs {
                    normalize_doc_block_for_comparison(docs);
                }
                variant.span = Span::default();
                variant.name_span = Span::default();
                if let Some(payload_type) = &mut variant.payload_type {
                    normalize_type_for_body_only_comparison(payload_type);
                }
                if let Some(value) = &mut variant.value {
                    normalize_expr_for_body_only_comparison(value);
                }
            }
        }
        ast::TypeKind::TypeOf(expr) => normalize_expr_for_body_only_comparison(expr),
        ast::TypeKind::Error
        | ast::TypeKind::Infer
        | ast::TypeKind::SelfType
        | ast::TypeKind::Never
        | ast::TypeKind::Void => {}
    }
}

fn normalize_struct_field_for_body_only_comparison(field: &mut ast::StructFieldDef) {
    field.span = Span::default();
    field.name_span = Span::default();
    if let Some(docs) = &mut field.docs {
        normalize_doc_block_for_comparison(docs);
    }
    normalize_type_for_body_only_comparison(&mut field.type_node);
    field.default_value = None;
}

fn normalize_enum_variant_for_body_only_comparison(variant: &mut ast::EnumVariant) {
    variant.span = Span::default();
    variant.name_span = Span::default();
    if let Some(docs) = &mut variant.docs {
        normalize_doc_block_for_comparison(docs);
    }
    if let Some(payload_type) = &mut variant.payload_type {
        normalize_type_for_body_only_comparison(payload_type);
    }
    if let Some(value) = &mut variant.value {
        normalize_expr_for_body_only_comparison(value);
    }
}

fn normalize_trait_items_for_body_only_comparison(
    assoc_types: &mut [ast::AssociatedTypeDecl],
    methods: &mut [ast::TraitMethodDef],
) {
    for assoc in assoc_types {
        assoc.name_span = Span::default();
        assoc.span = Span::default();
        if let Some(docs) = &mut assoc.docs {
            normalize_doc_block_for_comparison(docs);
        }
        normalize_generics_for_body_only_comparison(&mut assoc.generics);
        for bound in &mut assoc.bounds {
            normalize_type_for_body_only_comparison(bound);
        }
        normalize_where_clauses_for_body_only_comparison(&mut assoc.where_clauses);
    }
    for method in methods {
        normalize_trait_method_for_body_only_comparison(method);
    }
}

fn normalize_trait_method_for_body_only_comparison(method: &mut ast::TraitMethodDef) {
    method.span = Span::default();
    normalize_struct_field_for_body_only_comparison(&mut method.signature);
    for param in &mut method.params {
        normalize_func_param_for_body_only_comparison(param);
    }
    if let Some(body) = &mut method.body {
        normalize_expr_for_body_only_comparison(body);
    }
}

fn normalize_expr_for_body_only_comparison(expr: &mut ast::Expr) {
    expr.id = NodeId(0);
    expr.span = Span::default();
    match &mut expr.kind {
        ast::ExprKind::Let {
            pattern,
            type_node,
            init,
            else_clause,
            ..
        } => {
            normalize_let_pattern_for_body_only_comparison(pattern);
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            normalize_expr_for_body_only_comparison(init);
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        normalize_expr_for_body_only_comparison(else_expr);
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            normalize_pattern_for_body_only_comparison(&mut arm.pattern);
                            normalize_expr_for_body_only_comparison(&mut arm.body);
                            arm.span = Span::default();
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static {
            pattern,
            type_node,
            init,
            ..
        } => {
            normalize_binding_pattern_for_body_only_comparison(pattern);
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            if let Some(init) = init {
                normalize_expr_for_body_only_comparison(init);
            }
        }
        ast::ExprKind::Error
        | ast::ExprKind::Integer { .. }
        | ast::ExprKind::Float { .. }
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
        | ast::ExprKind::AnchoredPath { .. }
        | ast::ExprKind::Break
        | ast::ExprKind::Continue
        | ast::ExprKind::Undef
        | ast::ExprKind::Infer
        | ast::ExprKind::SelfValue => {}
        ast::ExprKind::EnumLiteral { variant_span, .. } => {
            *variant_span = Span::default();
        }
        ast::ExprKind::TypeNode(type_node) => {
            normalize_type_for_body_only_comparison(type_node);
        }
        ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_expr_for_body_only_comparison(rhs);
        }
        ast::ExprKind::Range { start, end, .. } => {
            if let Some(start) = start {
                normalize_expr_for_body_only_comparison(start);
            }
            if let Some(end) = end {
                normalize_expr_for_body_only_comparison(end);
            }
        }
        ast::ExprKind::Unary { operand, .. } => normalize_expr_for_body_only_comparison(operand),
        ast::ExprKind::Grouped { expr: inner } => normalize_expr_for_body_only_comparison(inner),
        ast::ExprKind::FieldAccess { lhs, .. } => normalize_expr_for_body_only_comparison(lhs),
        ast::ExprKind::IndexAccess { lhs, index, .. } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_expr_for_body_only_comparison(index);
        }
        ast::ExprKind::Call { callee, args } => {
            normalize_expr_for_body_only_comparison(callee);
            for arg in args {
                normalize_expr_for_body_only_comparison(arg);
            }
        }
        ast::ExprKind::DataInit { type_node, literal } => {
            if let Some(type_node) = type_node {
                normalize_type_for_body_only_comparison(type_node);
            }
            normalize_data_literal_for_body_only_comparison(literal);
        }
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            normalize_expr_for_body_only_comparison(cond);
            normalize_expr_for_body_only_comparison(then_branch);
            if let Some(else_branch) = else_branch {
                normalize_expr_for_body_only_comparison(else_branch);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            normalize_expr_for_body_only_comparison(target);
            for arm in arms {
                arm.span = Span::default();
                for pattern in &mut arm.patterns {
                    normalize_match_pattern_for_body_only_comparison(pattern);
                }
                normalize_expr_for_body_only_comparison(&mut arm.body);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                stmt.id = NodeId(0);
                stmt.span = Span::default();
                normalize_attributes_for_body_only_comparison(&mut stmt.attributes);
                match &mut stmt.kind {
                    ast::StmtKind::Use(use_stmt) => {
                        use_stmt.binding_span = Span::default();
                        normalize_use_target_for_body_only_comparison(&mut use_stmt.target);
                    }
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                }
            }
            if let Some(result) = result {
                normalize_expr_for_body_only_comparison(result);
            }
        }
        ast::ExprKind::While { cond, body } => {
            normalize_expr_for_body_only_comparison(cond);
            normalize_expr_for_body_only_comparison(body);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            normalize_expr_for_body_only_comparison(lhs);
            if let Some(start) = start {
                normalize_expr_for_body_only_comparison(start);
            }
            if let Some(end) = end {
                normalize_expr_for_body_only_comparison(end);
            }
        }
        ast::ExprKind::Defer { expr: inner } => normalize_expr_for_body_only_comparison(inner),
        ast::ExprKind::Return(value) => {
            if let Some(value) = value {
                normalize_expr_for_body_only_comparison(value);
            }
        }
        ast::ExprKind::As { lhs, target } => {
            normalize_expr_for_body_only_comparison(lhs);
            normalize_type_for_body_only_comparison(target);
        }
        ast::ExprKind::Propagate { operand, .. } => {
            normalize_expr_for_body_only_comparison(operand);
        }
        ast::ExprKind::GenericInstantiation { target, args } => {
            normalize_expr_for_body_only_comparison(target);
            for arg in args {
                match arg {
                    ast::GenericArg::Type(ty) => normalize_type_for_body_only_comparison(ty),
                    ast::GenericArg::ConstExpr(expr) => {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                    ast::GenericArg::AssocBinding {
                        name_span, value, ..
                    } => {
                        *name_span = Span::default();
                        normalize_type_for_body_only_comparison(value);
                    }
                }
            }
        }
        ast::ExprKind::Closure {
            captures,
            params,
            ret_type,
            body,
        } => {
            for capture in captures {
                capture.name_span = Span::default();
                capture.span = Span::default();
                normalize_expr_for_body_only_comparison(&mut capture.value);
            }
            for param in params {
                normalize_func_param_for_body_only_comparison(param);
            }
            normalize_type_for_body_only_comparison(ret_type);
            normalize_expr_for_body_only_comparison(body);
        }
    }
}

fn normalize_data_literal_for_body_only_comparison(literal: &mut ast::DataLiteralKind) {
    match literal {
        ast::DataLiteralKind::Struct(fields) => {
            for field in fields {
                field.span = Span::default();
                field.name_span = Span::default();
                normalize_expr_for_body_only_comparison(&mut field.value);
            }
        }
        ast::DataLiteralKind::Array(items) => {
            for item in items {
                normalize_expr_for_body_only_comparison(item);
            }
        }
        ast::DataLiteralKind::Repeat { value, count } => {
            normalize_expr_for_body_only_comparison(value);
            normalize_expr_for_body_only_comparison(count);
        }
        ast::DataLiteralKind::Scalar(value) => normalize_expr_for_body_only_comparison(value),
    }
}

fn normalize_let_pattern_for_body_only_comparison(pattern: &mut ast::LetPattern) {
    pattern.span = Span::default();
    normalize_pattern_for_body_only_comparison(&mut pattern.pattern);
}

fn normalize_pattern_for_body_only_comparison(pattern: &mut ast::Pattern) {
    pattern.span = Span::default();
    match &mut pattern.kind {
        ast::PatternKind::Binding(binding) => {
            normalize_binding_pattern_for_body_only_comparison(binding);
        }
        ast::PatternKind::Ignore => {}
        ast::PatternKind::Value(value) => normalize_expr_for_body_only_comparison(value),
        ast::PatternKind::Variant(variant) => {
            variant.variant_span = Span::default();
            if let Some(target_type) = &mut variant.target_type {
                normalize_type_for_body_only_comparison(target_type);
            }
        }
        ast::PatternKind::Destructure(destructure) => {
            if let Some(target_type) = &mut destructure.target_type {
                normalize_type_for_body_only_comparison(target_type);
            }
            for field in &mut destructure.fields {
                field.span = Span::default();
                field.name_span = Span::default();
                normalize_pattern_for_body_only_comparison(&mut field.pattern);
            }
        }
    }
}

fn normalize_match_pattern_for_body_only_comparison(pattern: &mut ast::MatchPattern) {
    pattern.span = Span::default();
    match &mut pattern.kind {
        ast::MatchPatternKind::Value(value) => normalize_expr_for_body_only_comparison(value),
        ast::MatchPatternKind::Pattern(inner) => {
            normalize_pattern_for_body_only_comparison(inner);
        }
    }
}

fn placeholder_expr() -> ast::Expr {
    ast::Expr {
        id: NodeId(0),
        span: Span::default(),
        kind: ast::ExprKind::Infer,
    }
}
