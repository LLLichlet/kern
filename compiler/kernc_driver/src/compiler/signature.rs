use super::{
    AnalysisParameterInformation, AnalysisSignatureHelp, AnalysisSignatureInformation,
    CompilerDriver,
};
use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId};
use kernc_sema::ty::{Substituter, TypeId, TypeKind};
use kernc_utils::{FileId, Session, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct SignatureCallSite {
    file_id: FileId,
    span: Span,
    callee_span: Span,
    arg_spans: Vec<Span>,
    signatures: Vec<AnalysisSignatureInformation>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct SignatureModel {
    call_sites: Vec<SignatureCallSite>,
}

impl CompilerDriver {
    pub(super) fn collect_signature_model(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> SignatureModel {
        let mut call_sites = Vec::new();

        for (mod_id, module) in asts {
            let Def::Module(module_def) = &ctx.defs[mod_id.0 as usize] else {
                continue;
            };
            let file_id = module_def.file_id;

            for decl in &module.decls {
                collect_call_sites_in_decl(ctx, file_id, decl, &mut call_sites);
            }
        }

        SignatureModel { call_sites }
    }
}

impl SignatureModel {
    pub(super) fn signature_help(
        &self,
        session: &Session,
        target_path: &Path,
        offset: usize,
    ) -> Option<AnalysisSignatureHelp> {
        let site = self
            .call_sites
            .iter()
            .filter(|site| {
                session
                    .source_manager
                    .get_file_path(site.file_id)
                    .map(|path| normalize_analysis_path(path) == target_path)
                    .unwrap_or(false)
                    && call_span_contains_offset(site.span, offset)
            })
            .min_by_key(|site| site.span.end.saturating_sub(site.span.start))?;

        let parameter_count = site
            .signatures
            .first()
            .map(|signature| signature.parameters.len())
            .unwrap_or(0);
        let active_parameter = session
            .source_manager
            .get_file(site.file_id)
            .and_then(|file| {
                active_parameter_from_source(
                    &file.src,
                    site.span,
                    site.callee_span,
                    offset,
                    parameter_count,
                )
            })
            .unwrap_or_else(|| active_parameter_index(&site.arg_spans, offset, parameter_count));

        Some(AnalysisSignatureHelp {
            signatures: site.signatures.clone(),
            active_signature: 0,
            active_parameter,
        })
    }
}

fn collect_call_sites_in_decl(
    ctx: &mut SemaContext<'_>,
    file_id: FileId,
    decl: &ast::Decl,
    call_sites: &mut Vec<SignatureCallSite>,
) {
    match &decl.kind {
        ast::DeclKind::Function {
            body: Some(body), ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, body, call_sites);
        }
        ast::DeclKind::Function { body: None, .. } => {}
        ast::DeclKind::Var {
            value: Some(value), ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, value, call_sites);
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
            for child in decls {
                collect_call_sites_in_decl(ctx, file_id, child, call_sites);
            }
        }
        _ => {}
    }
}

fn collect_call_sites_in_expr(
    ctx: &mut SemaContext<'_>,
    file_id: FileId,
    expr: &ast::Expr,
    call_sites: &mut Vec<SignatureCallSite>,
) {
    match &expr.kind {
        ast::ExprKind::Let {
            init, else_clause, ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, init, call_sites);
            if let Some(else_clause) = else_clause {
                match else_clause {
                    ast::LetElseClause::Expr(else_expr) => {
                        collect_call_sites_in_expr(ctx, file_id, else_expr, call_sites);
                    }
                    ast::LetElseClause::Arms(arms) => {
                        for arm in arms {
                            collect_call_sites_in_expr(ctx, file_id, &arm.body, call_sites);
                        }
                    }
                }
            }
        }
        ast::ExprKind::Static {
            init: Some(init), ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, init, call_sites);
        }
        ast::ExprKind::Unary { operand: init, .. } | ast::ExprKind::Defer { expr: init } => {
            collect_call_sites_in_expr(ctx, file_id, init, call_sites);
        }
        ast::ExprKind::Binary { lhs, rhs, .. }
        | ast::ExprKind::Assign { lhs, rhs, .. }
        | ast::ExprKind::IndexAccess {
            lhs, index: rhs, ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, lhs, call_sites);
            collect_call_sites_in_expr(ctx, file_id, rhs, call_sites);
        }
        ast::ExprKind::FieldAccess { lhs, .. }
        | ast::ExprKind::As { lhs, .. }
        | ast::ExprKind::GenericInstantiation { target: lhs, .. } => {
            collect_call_sites_in_expr(ctx, file_id, lhs, call_sites);
        }
        ast::ExprKind::Call { callee, args } => {
            collect_call_sites_in_expr(ctx, file_id, callee, call_sites);
            for arg in args {
                collect_call_sites_in_expr(ctx, file_id, arg, call_sites);
            }

            if let Some(signatures) = signatures_for_call(ctx, callee) {
                call_sites.push(SignatureCallSite {
                    file_id,
                    span: expr.span,
                    callee_span: callee.span,
                    arg_spans: args.iter().map(|arg| arg.span).collect(),
                    signatures,
                });
            }
        }
        ast::ExprKind::DataInit { literal, .. } => match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    collect_call_sites_in_expr(ctx, file_id, &field.value, call_sites);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    collect_call_sites_in_expr(ctx, file_id, item, call_sites);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                collect_call_sites_in_expr(ctx, file_id, value, call_sites);
                collect_call_sites_in_expr(ctx, file_id, count, call_sites);
            }
            ast::DataLiteralKind::Scalar(value) => {
                collect_call_sites_in_expr(ctx, file_id, value, call_sites);
            }
        },
        ast::ExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_call_sites_in_expr(ctx, file_id, cond, call_sites);
            collect_call_sites_in_expr(ctx, file_id, then_branch, call_sites);
            if let Some(else_branch) = else_branch {
                collect_call_sites_in_expr(ctx, file_id, else_branch, call_sites);
            }
        }
        ast::ExprKind::Match { target, arms } => {
            collect_call_sites_in_expr(ctx, file_id, target, call_sites);
            for arm in arms {
                for pattern in &arm.patterns {
                    collect_call_sites_in_pattern_exprs(ctx, file_id, pattern, call_sites);
                }
                collect_call_sites_in_expr(ctx, file_id, &arm.body, call_sites);
            }
        }
        ast::ExprKind::Block { stmts, result } => {
            for stmt in stmts {
                match &stmt.kind {
                    ast::StmtKind::Use(_) => {}
                    ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                        collect_call_sites_in_expr(ctx, file_id, expr, call_sites);
                    }
                }
            }
            if let Some(result) = result {
                collect_call_sites_in_expr(ctx, file_id, result, call_sites);
            }
        }
        ast::ExprKind::While { cond, body } => {
            collect_call_sites_in_expr(ctx, file_id, cond, call_sites);
            collect_call_sites_in_expr(ctx, file_id, body, call_sites);
        }
        ast::ExprKind::SliceOp {
            lhs, start, end, ..
        } => {
            collect_call_sites_in_expr(ctx, file_id, lhs, call_sites);
            if let Some(start) = start {
                collect_call_sites_in_expr(ctx, file_id, start, call_sites);
            }
            if let Some(end) = end {
                collect_call_sites_in_expr(ctx, file_id, end, call_sites);
            }
        }
        ast::ExprKind::Return(Some(value)) => {
            collect_call_sites_in_expr(ctx, file_id, value, call_sites);
        }
        ast::ExprKind::Return(None) => {}
        ast::ExprKind::Closure { captures, body, .. } => {
            for capture in captures {
                collect_call_sites_in_expr(ctx, file_id, &capture.value, call_sites);
            }
            collect_call_sites_in_expr(ctx, file_id, body, call_sites);
        }
        _ => {}
    }
}

fn collect_call_sites_in_pattern_exprs(
    ctx: &mut SemaContext<'_>,
    file_id: FileId,
    pattern: &ast::MatchPattern,
    call_sites: &mut Vec<SignatureCallSite>,
) {
    match &pattern.kind {
        ast::MatchPatternKind::Value(value) => {
            collect_call_sites_in_expr(ctx, file_id, value, call_sites);
        }
        _ => {}
    }
}

fn signatures_for_call(
    ctx: &mut SemaContext<'_>,
    callee: &ast::Expr,
) -> Option<Vec<AnalysisSignatureInformation>> {
    let callee_ty = ctx.node_type(callee.id)?;
    let normalized = ctx.type_registry.normalize(callee_ty);
    let signature = match ctx.type_registry.get(normalized).clone() {
        TypeKind::FnDef(def_id, args) => function_signature_information(
            ctx,
            def_id,
            &kernc_sema::ty::erase_non_type_generic_args(&args),
        )?,
        TypeKind::Function {
            params,
            ret,
            is_variadic,
        } => callable_signature_information(ctx, None, &params, ret, is_variadic),
        TypeKind::ClosureInterface { params, ret } => {
            callable_signature_information(ctx, None, &params, ret, false)
        }
        TypeKind::Pointer { elem, .. } => {
            let pointee = ctx.type_registry.normalize(elem);
            match ctx.type_registry.get(pointee).clone() {
                TypeKind::Function {
                    params,
                    ret,
                    is_variadic,
                } => callable_signature_information(ctx, None, &params, ret, is_variadic),
                TypeKind::ClosureInterface { params, ret } => {
                    callable_signature_information(ctx, None, &params, ret, false)
                }
                _ => return None,
            }
        }
        _ => return None,
    };

    Some(vec![signature])
}

fn function_signature_information(
    ctx: &mut SemaContext<'_>,
    def_id: DefId,
    args: &[TypeId],
) -> Option<AnalysisSignatureInformation> {
    let Def::Function(function) = ctx.defs[def_id.0 as usize].clone() else {
        return None;
    };
    let sig_ty = function.resolved_sig?;
    let normalized_sig = ctx.type_registry.normalize(sig_ty);
    let TypeKind::Function {
        mut params,
        mut ret,
        is_variadic,
    } = ctx.type_registry.get(normalized_sig).clone()
    else {
        return None;
    };

    if !function.generics.is_empty() && !args.is_empty() {
        let mut map = HashMap::new();
        for (index, generic) in function.generics.iter().enumerate() {
            if let Some(&arg) = args.get(index) {
                map.insert(generic.name, arg);
            }
        }
        if !map.is_empty() {
            let mut substituter = Substituter::new(&mut ctx.type_registry, &map);
            params = params
                .into_iter()
                .map(|param| substituter.substitute(param))
                .collect();
            ret = substituter.substitute(ret);
        }
    }

    let parameter_labels = function
        .params
        .iter()
        .enumerate()
        .map(|(index, param)| {
            let ty = params.get(index).copied().unwrap_or(TypeId::ERROR);
            let ty_label = ctx.ty_to_string(ty);
            let name = ctx.resolve(param.pattern.name);
            if name == "_" {
                ty_label
            } else {
                format!("{name}: {ty_label}")
            }
        })
        .collect::<Vec<_>>();

    let mut display_parameters = parameter_labels.clone();
    if is_variadic {
        display_parameters.push("...".to_string());
    }

    Some(AnalysisSignatureInformation {
        label: format!(
            "{}({}) {}",
            ctx.resolve(function.name),
            display_parameters.join(", "),
            ctx.ty_to_string(ret)
        ),
        parameters: display_parameters
            .into_iter()
            .map(|label| AnalysisParameterInformation { label })
            .collect(),
    })
}

fn callable_signature_information(
    ctx: &SemaContext<'_>,
    name: Option<&str>,
    params: &[TypeId],
    ret: TypeId,
    is_variadic: bool,
) -> AnalysisSignatureInformation {
    let mut parameter_labels = params
        .iter()
        .enumerate()
        .map(|(index, ty)| format!("arg{}: {}", index + 1, ctx.ty_to_string(*ty)))
        .collect::<Vec<_>>();
    if is_variadic {
        parameter_labels.push("...".to_string());
    }

    let callee = name.unwrap_or("fn");
    AnalysisSignatureInformation {
        label: format!(
            "{}({}) {}",
            callee,
            parameter_labels.join(", "),
            ctx.ty_to_string(ret)
        ),
        parameters: parameter_labels
            .into_iter()
            .map(|label| AnalysisParameterInformation { label })
            .collect(),
    }
}

fn active_parameter_index(arg_spans: &[Span], offset: usize, parameter_count: usize) -> usize {
    if parameter_count == 0 || arg_spans.is_empty() {
        return 0;
    }

    let raw_index = arg_spans
        .iter()
        .position(|span| offset <= span.end)
        .unwrap_or(arg_spans.len().saturating_sub(1));
    raw_index.min(parameter_count.saturating_sub(1))
}

fn active_parameter_from_source(
    source: &str,
    call_span: Span,
    callee_span: Span,
    offset: usize,
    parameter_count: usize,
) -> Option<usize> {
    if parameter_count == 0 {
        return Some(0);
    }

    let scan_start = callee_span.end.min(source.len());
    let scan_end = offset.min(call_span.end).min(source.len());
    if scan_start >= scan_end {
        return None;
    }

    let bytes = &source.as_bytes()[scan_start..scan_end];
    let mut index = 0usize;
    let mut found_call_opener = false;
    let mut nested_depth = 0usize;
    let mut comma_count = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index = skip_block_comment(bytes, index + 2);
            }
            b'"' | b'\'' => {
                index = skip_quoted_literal(bytes, index);
            }
            b'(' if !found_call_opener => {
                found_call_opener = true;
                index += 1;
            }
            b'(' | b'[' | b'{' if found_call_opener => {
                nested_depth += 1;
                index += 1;
            }
            b')' | b']' | b'}' if found_call_opener => {
                if nested_depth == 0 {
                    break;
                }
                nested_depth -= 1;
                index += 1;
            }
            b',' if found_call_opener && nested_depth == 0 => {
                comma_count += 1;
                index += 1;
            }
            _ => {
                index += 1;
            }
        }
    }

    found_call_opener.then_some(comma_count.min(parameter_count.saturating_sub(1)))
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> usize {
    let mut depth = 1usize;

    while index < bytes.len() && depth > 0 {
        match (bytes[index], bytes.get(index + 1).copied()) {
            (b'/', Some(b'*')) => {
                depth += 1;
                index += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                index += 2;
            }
            _ => {
                index += 1;
            }
        }
    }

    index
}

fn skip_quoted_literal(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut index = start + 1;

    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                index = (index + 2).min(bytes.len());
            }
            byte if byte == quote => {
                return index + 1;
            }
            _ => {
                index += 1;
            }
        }
    }

    index
}

fn call_span_contains_offset(span: Span, offset: usize) -> bool {
    offset >= span.start && offset <= span.end
}

fn normalize_analysis_path(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::active_parameter_from_source;
    use kernc_utils::{FileId, Span};

    #[test]
    fn source_tracking_advances_after_trailing_comma_without_next_argument() {
        let source = "helper(1, )";
        let file_id = FileId(0);
        let active_parameter = active_parameter_from_source(
            source,
            Span {
                file: file_id,
                start: 0,
                end: source.len(),
            },
            Span {
                file: file_id,
                start: 0,
                end: "helper".len(),
            },
            "helper(1, ".len(),
            2,
        );

        assert_eq!(active_parameter, Some(1));
    }

    #[test]
    fn source_tracking_ignores_nested_commas_inside_arguments() {
        let source = "helper(nested(1, 2), )";
        let file_id = FileId(0);
        let active_parameter = active_parameter_from_source(
            source,
            Span {
                file: file_id,
                start: 0,
                end: source.len(),
            },
            Span {
                file: file_id,
                start: 0,
                end: "helper".len(),
            },
            "helper(nested(1, 2), ".len(),
            2,
        );

        assert_eq!(active_parameter, Some(1));
    }
}
