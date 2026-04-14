use super::*;

pub(super) fn module_file_id(
    defs: &[kernc_sema::def::Def],
    module_id: DefId,
) -> kernc_utils::FileId {
    match &defs[module_id.0 as usize] {
        kernc_sema::def::Def::Module(module) => module.file_id,
        _ => kernc_utils::FileId(0),
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
    clean_module: &ast::Module,
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
                if decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    continue;
                }
                if decls_equal_ignoring_body_only(clean_decl, dirty_decl) {
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
                if decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    continue;
                }
                if decls_equal_ignoring_body_only(clean_decl, dirty_decl) {
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
                    clean,
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
                if !decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
                    return false;
                }
            }
        }
    }

    true
}

fn classify_function_body_decls<'a>(
    clean_decls: &[ast::Decl],
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
                if !decls_equal_ignoring_ids_and_spans(clean_decl, dirty_decl) {
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

fn decls_equal_ignoring_ids_and_spans(left: &ast::Decl, right: &ast::Decl) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_reuse_comparison(&mut left);
    normalize_decl_for_reuse_comparison(&mut right);
    left == right
}

fn decls_equal_ignoring_body_only(left: &ast::Decl, right: &ast::Decl) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_decl_for_body_only_comparison(&mut left);
    normalize_decl_for_body_only_comparison(&mut right);
    left == right
}

fn normalize_decl_for_reuse_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
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
        ast::DeclKind::Var { value, .. } => normalize_expr_for_body_only_comparison(value),
        ast::DeclKind::TypeAlias {
            generics,
            bounds,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            for bound in bounds {
                normalize_type_for_body_only_comparison(bound);
            }
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
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
        ast::DeclKind::ModDecl { .. } => {}
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
                    kernc_sema::def::Def::Struct(struct_def) => struct_def.name,
                    kernc_sema::def::Def::Union(union_def) => union_def.name,
                    kernc_sema::def::Def::Enum(enum_def) => enum_def.name,
                    kernc_sema::def::Def::Trait(trait_def) => trait_def.name,
                    kernc_sema::def::Def::TypeAlias(alias_def) => alias_def.name,
                    _ => return false,
                };
                match (&mut ctx.defs[def_id.0 as usize], &target.kind) {
                    (kernc_sema::def::Def::Struct(struct_def), ast::TypeKind::Struct { .. }) => {
                        struct_def.span = decl.span;
                    }
                    (kernc_sema::def::Def::Union(union_def), ast::TypeKind::Union { .. }) => {
                        union_def.span = decl.span;
                    }
                    (kernc_sema::def::Def::Enum(enum_def), ast::TypeKind::Enum { .. }) => {
                        enum_def.span = decl.span;
                    }
                    (kernc_sema::def::Def::Trait(trait_def), ast::TypeKind::Trait { .. }) => {
                        trait_def.span = decl.span;
                    }
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
            ast::DeclKind::Use { .. } | ast::DeclKind::ModDecl { .. } => {}
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

pub(super) fn modules_match_ignoring_body_only(left: &ast::Module, right: &ast::Module) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    normalize_module_for_body_only_comparison(&mut left);
    normalize_module_for_body_only_comparison(&mut right);
    left == right
}

fn normalize_module_for_body_only_comparison(module: &mut ast::Module) {
    for decl in &mut module.decls {
        normalize_decl_for_body_only_comparison(decl);
    }
}

fn normalize_decl_for_body_only_comparison(decl: &mut ast::Decl) {
    decl.id = NodeId(0);
    decl.span = Span::default();
    decl.name_span = Span::default();
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
        ast::DeclKind::Var { value, .. } => {
            *value = placeholder_expr();
        }
        ast::DeclKind::TypeAlias {
            generics,
            bounds,
            where_clauses,
            target,
            ..
        } => {
            normalize_generics_for_body_only_comparison(generics);
            for bound in bounds {
                normalize_type_for_body_only_comparison(bound);
            }
            normalize_where_clauses_for_body_only_comparison(where_clauses);
            normalize_type_for_body_only_comparison(target);
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
        ast::DeclKind::ModDecl { .. } => {}
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
    if let ast::UseTarget::Members(members) = target {
        for member in members {
            member.span = Span::default();
        }
    }
}

fn normalize_type_for_body_only_comparison(ty: &mut ast::TypeNode) {
    ty.id = NodeId(0);
    ty.span = Span::default();
    match &mut ty.kind {
        ast::TypeKind::Path { segments } => {
            for segment in segments {
                segment.name_span = Span::default();
                for arg in &mut segment.args {
                    match arg {
                        ast::TypeArg::Positional(generic) => {
                            normalize_type_for_body_only_comparison(generic);
                        }
                        ast::TypeArg::AssocBinding {
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
                normalize_struct_field_for_body_only_comparison(method);
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
        ast::TypeKind::Infer
        | ast::TypeKind::SelfType
        | ast::TypeKind::Never
        | ast::TypeKind::Void => {}
    }
}

fn normalize_struct_field_for_body_only_comparison(field: &mut ast::StructFieldDef) {
    field.span = Span::default();
    field.name_span = Span::default();
    normalize_type_for_body_only_comparison(&mut field.type_node);
    field.default_value = None;
}

fn normalize_expr_for_body_only_comparison(expr: &mut ast::Expr) {
    expr.id = NodeId(0);
    expr.span = Span::default();
    match &mut expr.kind {
        ast::ExprKind::Let {
            pattern,
            init,
            else_pattern,
            else_branch,
        } => {
            normalize_let_pattern_for_body_only_comparison(pattern);
            if let Some(else_pattern) = else_pattern {
                normalize_pattern_for_body_only_comparison(else_pattern);
            }
            normalize_expr_for_body_only_comparison(init);
            if let Some(else_branch) = else_branch {
                normalize_expr_for_body_only_comparison(else_branch);
            }
        }
        ast::ExprKind::Static { pattern, init } => {
            normalize_binding_pattern_for_body_only_comparison(pattern);
            normalize_expr_for_body_only_comparison(init);
        }
        ast::ExprKind::Integer(_)
        | ast::ExprKind::Float(_)
        | ast::ExprKind::Bool(_)
        | ast::ExprKind::Char(_)
        | ast::ExprKind::ByteChar(_)
        | ast::ExprKind::String(_)
        | ast::ExprKind::Identifier(_)
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
        ast::ExprKind::Unary { operand, .. } => normalize_expr_for_body_only_comparison(operand),
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
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        normalize_expr_for_body_only_comparison(expr);
                    }
                }
            }
            if let Some(result) = result {
                normalize_expr_for_body_only_comparison(result);
            }
        }
        ast::ExprKind::For {
            init,
            cond,
            post,
            body,
        } => {
            if let Some(init) = init {
                normalize_expr_for_body_only_comparison(init);
            }
            if let Some(cond) = cond {
                normalize_expr_for_body_only_comparison(cond);
            }
            if let Some(post) = post {
                normalize_expr_for_body_only_comparison(post);
            }
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
        ast::ExprKind::GenericInstantiation { target, types } => {
            normalize_expr_for_body_only_comparison(target);
            for ty in types {
                normalize_type_for_body_only_comparison(ty);
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
        ast::MatchPatternKind::Range { start, end, .. } => {
            normalize_expr_for_body_only_comparison(start);
            normalize_expr_for_body_only_comparison(end);
        }
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
