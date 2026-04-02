use super::{AnalysisCompletionItem, AnalysisCompletionKind, CompilerDriver};
use kernc_ast as ast;
use kernc_sema::def::DefId;
use kernc_sema::{MemberCandidate, MemberQuery, MemberQueryEnv, SemaContext};
use kernc_utils::{FileId, Session};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct CompletionModule {
    pub(super) file_id: FileId,
    pub(super) ast: ast::Module,
    pub(super) top_level_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompletionModel {
    pub(super) root_items: Vec<AnalysisCompletionItem>,
    pub(super) items_by_span: BTreeMap<kernc_utils::Span, AnalysisCompletionItem>,
    pub(super) function_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    pub(super) member_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    pub(super) modules: Vec<CompletionModule>,
}

impl CompilerDriver {
    pub(super) fn collect_completion_model(
        &self,
        ctx: &mut SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> CompletionModel {
        let mut model = self.collect_structure_completion_model(ctx, asts);
        let mut member_items_by_span = BTreeMap::new();
        let mut member_query = MemberQuery::new(ctx);
        let mut member_env = MemberQueryEnv::default();
        for (mod_id, ast) in asts {
            self.collect_member_completion_items_in_module(
                &mut member_query,
                *mod_id,
                ast,
                &mut member_env,
                &mut member_items_by_span,
            );
        }
        model.member_items_by_span = member_items_by_span;
        model
    }

    pub(super) fn collect_structure_completion_model(
        &self,
        ctx: &SemaContext<'_>,
        asts: &[(DefId, ast::Module)],
    ) -> CompletionModel {
        let mut items_by_span = BTreeMap::new();
        for (name, info) in ctx.scopes.all_symbols() {
            let Some(item) = self.completion_item_for_symbol(ctx, name, info) else {
                continue;
            };

            items_by_span.entry(info.span).or_insert(item);
        }

        let root_items = ctx
            .scopes
            .symbols_in_scope(kernc_sema::scope::ScopeId(0))
            .filter_map(|(name, info)| self.completion_item_for_symbol(ctx, name, info))
            .collect();

        let mut function_items_by_span = BTreeMap::new();
        for def in &ctx.defs {
            let kernc_sema::def::Def::Function(function) = def else {
                continue;
            };

            function_items_by_span.insert(
                function.span,
                self.initial_completion_items_for_function(ctx, function),
            );
        }

        let modules = asts
            .iter()
            .filter_map(|(mod_id, ast)| {
                let kernc_sema::def::Def::Module(module_def) = &ctx.defs[mod_id.0 as usize] else {
                    return None;
                };
                let module_file_id = module_def.file_id;
                let module_scope_id = module_def.scope_id;

                let top_level_items = ctx
                    .scopes
                    .symbols_in_scope(module_scope_id)
                    .filter_map(|(name, info)| self.completion_item_for_symbol(ctx, name, info))
                    .collect();

                Some(CompletionModule {
                    file_id: module_file_id,
                    ast: ast.clone(),
                    top_level_items,
                })
            })
            .collect();

        CompletionModel {
            root_items,
            items_by_span,
            function_items_by_span,
            member_items_by_span: BTreeMap::new(),
            modules,
        }
    }

    fn collect_member_completion_items_in_module(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        module: &ast::Module,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        for decl in &module.decls {
            self.collect_member_completion_items_in_decl(
                member_query,
                module_id,
                decl,
                member_env,
                member_items_by_span,
            );
        }
    }

    fn collect_member_completion_items_in_decl(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        decl: &ast::Decl,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &decl.kind {
            ast::DeclKind::Function {
                where_clauses,
                body,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                if let Some(body) = body {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        body,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    value,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::DeclKind::Impl {
                where_clauses,
                decls,
                ..
            } => {
                let previous_env_len = member_env.len();
                member_env.extend_with_where_clauses(member_query.context(), where_clauses);
                for child in decls {
                    self.collect_member_completion_items_in_decl(
                        member_query,
                        module_id,
                        child,
                        member_env,
                        member_items_by_span,
                    );
                }
                member_env.truncate(previous_env_len);
            }
            _ => {}
        }
    }

    fn collect_member_completion_items_in_expr(
        &self,
        member_query: &mut MemberQuery<'_, '_>,
        module_id: DefId,
        expr: &ast::Expr,
        member_env: &mut MemberQueryEnv,
        member_items_by_span: &mut BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    ) {
        match &expr.kind {
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if let Some(lhs_ty) = member_query.context().node_types.get(&lhs.id).copied() {
                    let items = member_query
                        .member_candidates_in_env(Some(module_id), lhs_ty, member_env)
                        .into_iter()
                        .filter_map(|candidate| {
                            self.completion_item_for_member_candidate(
                                member_query.context(),
                                candidate,
                            )
                        })
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        member_items_by_span.insert(expr.span, items);
                    }
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    match &stmt.kind {
                        ast::StmtKind::ExprStmt(inner) | ast::StmtKind::ExprValue(inner) => {
                            self.collect_member_completion_items_in_expr(
                                member_query,
                                module_id,
                                inner,
                                member_env,
                                member_items_by_span,
                            );
                        }
                    }
                }
                if let Some(result) = result {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        result,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Let {
                init, else_branch, ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_branch) = else_branch {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        else_branch,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Static { init, .. }
            | ast::ExprKind::Unary { operand: init, .. }
            | ast::ExprKind::Defer { expr: init } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    init,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    rhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    index,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Call { callee, args } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    callee,
                    member_env,
                    member_items_by_span,
                );
                for arg in args {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        arg,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::DataInit { literal, .. } => match literal {
                ast::DataLiteralKind::Struct(fields) => {
                    for field in fields {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            &field.value,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Array(items) => {
                    for item in items {
                        self.collect_member_completion_items_in_expr(
                            member_query,
                            module_id,
                            item,
                            member_env,
                            member_items_by_span,
                        );
                    }
                }
                ast::DataLiteralKind::Repeat { value, count } => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        count,
                        member_env,
                        member_items_by_span,
                    );
                }
                ast::DataLiteralKind::Scalar(value) => {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        value,
                        member_env,
                        member_items_by_span,
                    );
                }
            },
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    cond,
                    member_env,
                    member_items_by_span,
                );
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    then_branch,
                    member_env,
                    member_items_by_span,
                );
                if let Some(else_branch) = else_branch {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        else_branch,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Match { target, arms } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    target,
                    member_env,
                    member_items_by_span,
                );
                for arm in arms {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &arm.body,
                        member_env,
                        member_items_by_span,
                    );
                    for pattern in &arm.patterns {
                        match &pattern.kind {
                            ast::MatchPatternKind::Value(value) => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    value,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            ast::MatchPatternKind::Range { start, end, .. } => {
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    start,
                                    member_env,
                                    member_items_by_span,
                                );
                                self.collect_member_completion_items_in_expr(
                                    member_query,
                                    module_id,
                                    end,
                                    member_env,
                                    member_items_by_span,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                if let Some(init) = init {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        init,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(cond) = cond {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        cond,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(post) = post {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        post,
                        member_env,
                        member_items_by_span,
                    );
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
                if let Some(start) = start {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        start,
                        member_env,
                        member_items_by_span,
                    );
                }
                if let Some(end) = end {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        end,
                        member_env,
                        member_items_by_span,
                    );
                }
            }
            ast::ExprKind::Return(Some(value)) => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    value,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Return(None) => {}
            ast::ExprKind::As { lhs, .. }
            | ast::ExprKind::GenericInstantiation { target: lhs, .. } => {
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    lhs,
                    member_env,
                    member_items_by_span,
                );
            }
            ast::ExprKind::Closure { captures, body, .. } => {
                for capture in captures {
                    self.collect_member_completion_items_in_expr(
                        member_query,
                        module_id,
                        &capture.value,
                        member_env,
                        member_items_by_span,
                    );
                }
                self.collect_member_completion_items_in_expr(
                    member_query,
                    module_id,
                    body,
                    member_env,
                    member_items_by_span,
                );
            }
            _ => {}
        }
    }

    fn initial_completion_items_for_function(
        &self,
        ctx: &SemaContext<'_>,
        function: &kernc_sema::def::FunctionDef,
    ) -> Vec<AnalysisCompletionItem> {
        let mut items = Vec::new();

        for generic in &function.generics {
            push_completion_item(
                &mut items,
                AnalysisCompletionItem {
                    label: ctx.resolve(generic.name).to_string(),
                    kind: AnalysisCompletionKind::TypeParameter,
                    detail: Some("type".to_string()),
                    insert_text: None,
                },
            );
        }

        let Some(sig) = function.resolved_sig else {
            return items;
        };
        let kernc_sema::ty::TypeKind::Function { params, .. } = ctx.type_registry.get(sig).clone()
        else {
            return items;
        };

        for (index, param) in function.params.iter().enumerate() {
            let name = ctx.resolve(param.pattern.name);
            if name == "_" {
                continue;
            }

            push_completion_item(
                &mut items,
                AnalysisCompletionItem {
                    label: name.to_string(),
                    kind: AnalysisCompletionKind::Variable,
                    detail: params.get(index).copied().map(|ty| ctx.ty_to_string(ty)),
                    insert_text: None,
                },
            );
        }

        items
    }

    fn completion_item_for_symbol(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        info: &kernc_sema::scope::SymbolInfo,
    ) -> Option<AnalysisCompletionItem> {
        let label = ctx.resolve(name);
        if label == "_" {
            return None;
        }

        let (kind, detail) = self.symbol_completion_presentation(ctx, name, info)?;
        Some(AnalysisCompletionItem {
            label: label.to_string(),
            kind,
            detail,
            insert_text: symbol_completion_insert_text(ctx, label, info),
        })
    }

    fn symbol_completion_presentation(
        &self,
        ctx: &SemaContext<'_>,
        name: kernc_utils::SymbolId,
        info: &kernc_sema::scope::SymbolInfo,
    ) -> Option<(AnalysisCompletionKind, Option<String>)> {
        let name = ctx.resolve(name);

        let presentation = match info.kind {
            kernc_sema::scope::SymbolKind::Function => {
                let def_id = info.def_id?;
                let kernc_sema::def::Def::Function(function) = &ctx.defs[def_id.0 as usize] else {
                    return None;
                };
                let sig = function.resolved_sig?;
                (
                    AnalysisCompletionKind::Function,
                    Some(ctx.ty_to_string(sig)),
                )
            }
            kernc_sema::scope::SymbolKind::Const => {
                let mut detail = String::from("const");
                if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(info.type_id));
                }
                (AnalysisCompletionKind::Constant, Some(detail))
            }
            kernc_sema::scope::SymbolKind::Static => {
                let mut detail = String::from("static");
                if info.is_mut {
                    detail.push_str(" mut");
                }
                if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(info.type_id));
                }
                (AnalysisCompletionKind::Static, Some(detail))
            }
            kernc_sema::scope::SymbolKind::Var => (
                AnalysisCompletionKind::Variable,
                Some(ctx.ty_to_string(info.type_id)),
            ),
            kernc_sema::scope::SymbolKind::Struct => {
                (AnalysisCompletionKind::Struct, Some("struct".to_string()))
            }
            kernc_sema::scope::SymbolKind::Union => {
                (AnalysisCompletionKind::Union, Some("union".to_string()))
            }
            kernc_sema::scope::SymbolKind::Enum => {
                (AnalysisCompletionKind::Enum, Some("enum".to_string()))
            }
            kernc_sema::scope::SymbolKind::Trait => {
                (AnalysisCompletionKind::Trait, Some("trait".to_string()))
            }
            kernc_sema::scope::SymbolKind::Module => (
                AnalysisCompletionKind::Module,
                Some(format!("module {}", name)),
            ),
            kernc_sema::scope::SymbolKind::TypeAlias => {
                let detail = if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                {
                    ctx.node_types
                        .get(&alias.target.id)
                        .copied()
                        .map(|target_ty| format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if info.type_id != kernc_sema::ty::TypeId::ERROR {
                    Some(format!("type = {}", ctx.ty_to_string(info.type_id)))
                } else {
                    Some("type".to_string())
                };

                (AnalysisCompletionKind::TypeAlias, detail)
            }
            kernc_sema::scope::SymbolKind::TypeParam => (
                AnalysisCompletionKind::TypeParameter,
                Some("type".to_string()),
            ),
        };

        Some(presentation)
    }

    fn completion_item_for_member_candidate(
        &self,
        ctx: &SemaContext<'_>,
        candidate: MemberCandidate,
    ) -> Option<AnalysisCompletionItem> {
        let detail = match candidate.kind {
            kernc_sema::scope::SymbolKind::Function => Some(ctx.ty_to_string(candidate.type_id)),
            kernc_sema::scope::SymbolKind::Var => Some(ctx.ty_to_string(candidate.type_id)),
            kernc_sema::scope::SymbolKind::Static => {
                let mut detail = String::from("static");
                if candidate.is_mut {
                    detail.push_str(" mut");
                }
                if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(candidate.type_id));
                }
                Some(detail)
            }
            kernc_sema::scope::SymbolKind::Const => {
                let mut detail = String::from("const");
                if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    detail.push(' ');
                    detail.push_str(&ctx.ty_to_string(candidate.type_id));
                }
                Some(detail)
            }
            kernc_sema::scope::SymbolKind::Module => {
                Some(format!("module {}", ctx.resolve(candidate.name)))
            }
            kernc_sema::scope::SymbolKind::Struct => Some("struct".to_string()),
            kernc_sema::scope::SymbolKind::Union => Some("union".to_string()),
            kernc_sema::scope::SymbolKind::Enum => Some("enum".to_string()),
            kernc_sema::scope::SymbolKind::Trait => Some("trait".to_string()),
            kernc_sema::scope::SymbolKind::TypeParam => Some("type".to_string()),
            kernc_sema::scope::SymbolKind::TypeAlias => {
                if let Some(def_id) = candidate.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                    && let Some(target_ty) = ctx.node_types.get(&alias.target.id).copied()
                {
                    Some(format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if candidate.type_id != kernc_sema::ty::TypeId::ERROR {
                    Some(format!("type = {}", ctx.ty_to_string(candidate.type_id)))
                } else {
                    Some("type".to_string())
                }
            }
        };

        Some(AnalysisCompletionItem {
            label: ctx.resolve(candidate.name).to_string(),
            kind: completion_kind_from_symbol_kind(candidate.kind),
            detail,
            insert_text: candidate_completion_insert_text(ctx, &candidate),
        })
    }
}

pub(super) fn parsed_requires_body_completion(
    session: &Session,
    modules: &[super::ParsedModule],
    target_path: &Path,
    offset: usize,
) -> bool {
    let Some(module) = modules.iter().find(|module| {
        session
            .source_manager
            .get_file_path(module.file_id)
            .map(|path| normalize_analysis_path(path) == target_path)
            .unwrap_or(false)
    }) else {
        return true;
    };

    module
        .ast
        .decls
        .iter()
        .any(|decl| decl_requires_body_completion(decl, offset))
}

impl CompletionModel {
    pub(super) fn completion_items(
        &self,
        session: &Session,
        target_path: &Path,
        offset: usize,
    ) -> Vec<AnalysisCompletionItem> {
        let Some(module) = self.modules.iter().find(|module| {
            session
                .source_manager
                .get_file_path(module.file_id)
                .map(|path| normalize_analysis_path(path) == target_path)
                .unwrap_or(false)
        }) else {
            return Vec::new();
        };

        let mut visible = Vec::new();
        for item in &self.root_items {
            push_completion_item(&mut visible, item.clone());
        }
        for item in &module.top_level_items {
            push_completion_item(&mut visible, item.clone());
        }

        for decl in &module.ast.decls {
            if self.collect_in_decl(decl, &mut visible, offset) {
                break;
            }
        }

        visible
    }

    fn collect_in_decl(
        &self,
        decl: &ast::Decl,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(decl.span, offset) {
            return false;
        }

        match &decl.kind {
            ast::DeclKind::Function { body, .. } => {
                if let Some(items) = self.function_items_by_span.get(&decl.span) {
                    for item in items {
                        push_completion_item(visible, item.clone());
                    }
                }

                if let Some(body) = body
                    && span_contains_offset(body.span, offset)
                {
                    self.collect_in_expr(body, visible, offset);
                }
                true
            }
            ast::DeclKind::Var { value, .. } => {
                self.collect_in_expr(value, visible, offset);
                true
            }
            ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => {
                for child in decls {
                    if self.collect_in_decl(child, visible, offset) {
                        return true;
                    }
                }
                true
            }
            _ => true,
        }
    }

    fn collect_in_expr(
        &self,
        expr: &ast::Expr,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(query_span_for_expr(expr), offset) {
            return false;
        }

        match &expr.kind {
            ast::ExprKind::Block { stmts, result } => {
                let mut block_visible = visible.clone();

                for stmt in stmts {
                    let stmt_span = query_span_for_stmt(stmt);
                    if stmt_span.start > offset {
                        break;
                    }

                    if span_contains_offset(stmt_span, offset) {
                        self.collect_in_stmt(stmt, &mut block_visible, offset);
                        *visible = block_visible;
                        return true;
                    }

                    if stmt_span.end <= offset {
                        self.record_stmt_bindings(stmt, &mut block_visible);
                    }
                }

                if let Some(result) = result
                    && span_contains_offset(result.span, offset)
                {
                    self.collect_in_expr(result, &mut block_visible, offset);
                }

                *visible = block_visible;
                true
            }
            ast::ExprKind::Let {
                pattern: _,
                init,
                else_branch,
            } => {
                if self.collect_in_expr(init, visible, offset) {
                    return true;
                }
                if let Some(else_branch) = else_branch {
                    return self.collect_in_expr(else_branch, visible, offset);
                }
                true
            }
            ast::ExprKind::Static { init, .. } => self.collect_in_expr(init, visible, offset),
            ast::ExprKind::Binary { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::Unary { operand, .. } => self.collect_in_expr(operand, visible, offset),
            ast::ExprKind::FieldAccess { lhs, .. } => {
                if span_contains_offset(lhs.span, offset) {
                    self.collect_in_expr(lhs, visible, offset)
                } else if let Some(items) = self.member_items_by_span.get(&expr.span) {
                    *visible = items.clone();
                    true
                } else {
                    true
                }
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(index, visible, offset)
            }
            ast::ExprKind::Call { callee, args } => {
                if self.collect_in_expr(callee, visible, offset) {
                    return true;
                }
                for arg in args {
                    if self.collect_in_expr(arg, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::ExprKind::DataInit { literal, .. } => {
                self.collect_in_data_literal(literal, visible, offset)
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                if self.collect_in_expr(cond, visible, offset) {
                    return true;
                }

                if span_contains_offset(then_branch.span, offset) {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(then_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                if let Some(else_branch) = else_branch
                    && span_contains_offset(else_branch.span, offset)
                {
                    let mut branch_visible = visible.clone();
                    self.collect_in_expr(else_branch, &mut branch_visible, offset);
                    *visible = branch_visible;
                    return true;
                }

                true
            }
            ast::ExprKind::Match { target, arms } => {
                if self.collect_in_expr(target, visible, offset) {
                    return true;
                }

                for arm in arms {
                    if !span_contains_offset(arm.span, offset) {
                        continue;
                    }

                    if span_contains_offset(arm.body.span, offset) {
                        let mut arm_visible = visible.clone();
                        self.record_match_arm_bindings(arm, &mut arm_visible);
                        self.collect_in_expr(&arm.body, &mut arm_visible, offset);
                        *visible = arm_visible;
                        return true;
                    }

                    for pattern in &arm.patterns {
                        if self.collect_in_match_pattern(pattern, visible, offset) {
                            return true;
                        }
                    }

                    return true;
                }

                true
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                let mut loop_visible = visible.clone();

                if let Some(init) = init {
                    if self.collect_in_expr(init, &mut loop_visible, offset) {
                        *visible = loop_visible;
                        return true;
                    }
                    if init.span.end <= offset {
                        self.record_expr_bindings(init, &mut loop_visible);
                    }
                }

                if let Some(cond) = cond
                    && self.collect_in_expr(cond, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if let Some(post) = post
                    && self.collect_in_expr(post, &mut loop_visible, offset)
                {
                    *visible = loop_visible;
                    return true;
                }

                if span_contains_offset(body.span, offset) {
                    self.collect_in_expr(body, &mut loop_visible, offset);
                    *visible = loop_visible;
                }

                true
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                if self.collect_in_expr(lhs, visible, offset) {
                    return true;
                }
                if let Some(start) = start
                    && self.collect_in_expr(start, visible, offset)
                {
                    return true;
                }
                if let Some(end) = end
                    && self.collect_in_expr(end, visible, offset)
                {
                    return true;
                }
                true
            }
            ast::ExprKind::Defer { expr } => self.collect_in_expr(expr, visible, offset),
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    return self.collect_in_expr(value, visible, offset);
                }
                true
            }
            ast::ExprKind::Assign { lhs, rhs, .. } => {
                self.collect_in_expr(lhs, visible, offset)
                    || self.collect_in_expr(rhs, visible, offset)
            }
            ast::ExprKind::As { lhs, .. } => self.collect_in_expr(lhs, visible, offset),
            ast::ExprKind::GenericInstantiation { target, .. } => {
                self.collect_in_expr(target, visible, offset)
            }
            ast::ExprKind::Closure {
                captures,
                params,
                body,
                ..
            } => {
                for capture in captures {
                    if self.collect_in_expr(&capture.value, visible, offset) {
                        return true;
                    }
                }

                if span_contains_offset(body.span, offset) {
                    let mut closure_visible = visible.clone();
                    for capture in captures {
                        self.record_span_binding(capture.span, &mut closure_visible);
                    }
                    for param in params {
                        self.record_span_binding(param.span, &mut closure_visible);
                    }
                    self.collect_in_expr(body, &mut closure_visible, offset);
                    *visible = closure_visible;
                }

                true
            }
            _ => true,
        }
    }

    fn collect_in_stmt(
        &self,
        stmt: &ast::Stmt,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match &stmt.kind {
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.collect_in_expr(expr, visible, offset)
            }
        }
    }

    fn collect_in_data_literal(
        &self,
        literal: &ast::DataLiteralKind,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    if self.collect_in_expr(&field.value, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    if self.collect_in_expr(item, visible, offset) {
                        return true;
                    }
                }
                true
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                self.collect_in_expr(value, visible, offset)
                    || self.collect_in_expr(count, visible, offset)
            }
            ast::DataLiteralKind::Scalar(value) => self.collect_in_expr(value, visible, offset),
        }
    }

    fn collect_in_match_pattern(
        &self,
        pattern: &ast::MatchPattern,
        visible: &mut Vec<AnalysisCompletionItem>,
        offset: usize,
    ) -> bool {
        if !span_contains_offset(pattern.span, offset) {
            return false;
        }

        match &pattern.kind {
            ast::MatchPatternKind::Value(value) => self.collect_in_expr(value, visible, offset),
            ast::MatchPatternKind::Range { start, end, .. } => {
                self.collect_in_expr(start, visible, offset)
                    || self.collect_in_expr(end, visible, offset)
            }
            _ => true,
        }
    }

    fn record_stmt_bindings(&self, stmt: &ast::Stmt, visible: &mut Vec<AnalysisCompletionItem>) {
        match &stmt.kind {
            ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                self.record_expr_bindings(expr, visible);
            }
        }
    }

    fn record_expr_bindings(&self, expr: &ast::Expr, visible: &mut Vec<AnalysisCompletionItem>) {
        match &expr.kind {
            ast::ExprKind::Let { pattern, .. } => {
                self.record_let_pattern_bindings(pattern, visible)
            }
            ast::ExprKind::Static { pattern, .. } => {
                self.record_span_binding(pattern.span, visible)
            }
            _ => {}
        }
    }

    fn record_let_pattern_bindings(
        &self,
        pattern: &ast::LetPattern,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        match &pattern.kind {
            ast::LetPatternKind::Binding(binding) => {
                self.record_span_binding(binding.span, visible);
            }
            ast::LetPatternKind::Variant(variant) => {
                if let Some(binding) = &variant.binding {
                    self.record_span_binding(binding.span, visible);
                }
            }
        }
    }

    fn record_match_arm_bindings(
        &self,
        arm: &ast::MatchArm,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        for pattern in &arm.patterns {
            let ast::MatchPatternKind::Variant(variant) = &pattern.kind else {
                continue;
            };
            if let Some(binding) = &variant.binding {
                self.record_span_binding(binding.span, visible);
            }
        }
    }

    fn record_span_binding(
        &self,
        span: kernc_utils::Span,
        visible: &mut Vec<AnalysisCompletionItem>,
    ) {
        let Some(item) = self.items_by_span.get(&span) else {
            return;
        };
        push_completion_item(visible, item.clone());
    }
}

fn push_completion_item(items: &mut Vec<AnalysisCompletionItem>, item: AnalysisCompletionItem) {
    if let Some(index) = items
        .iter()
        .position(|existing| existing.label == item.label)
    {
        items.remove(index);
    }
    items.push(item);
}

fn decl_requires_body_completion(decl: &ast::Decl, offset: usize) -> bool {
    if !span_contains_offset(decl.span, offset) {
        return false;
    }

    match &decl.kind {
        ast::DeclKind::Function { body, .. } => body
            .as_ref()
            .map(|body| span_contains_offset(query_span_for_expr(body), offset))
            .unwrap_or(false),
        ast::DeclKind::Var { value, .. } => {
            span_contains_offset(query_span_for_expr(value), offset)
        }
        ast::DeclKind::ExternBlock { decls, .. } | ast::DeclKind::Impl { decls, .. } => decls
            .iter()
            .any(|child| decl_requires_body_completion(child, offset)),
        _ => false,
    }
}

fn symbol_completion_insert_text(
    ctx: &SemaContext<'_>,
    label: &str,
    info: &kernc_sema::scope::SymbolInfo,
) -> Option<String> {
    if info.kind != kernc_sema::scope::SymbolKind::Function {
        return None;
    }

    let def_id = info.def_id?;
    let kernc_sema::def::Def::Function(function) = &ctx.defs[def_id.0 as usize] else {
        return None;
    };
    let sig = function.resolved_sig?;
    Some(function_completion_snippet(ctx, label, sig))
}

fn candidate_completion_insert_text(
    ctx: &SemaContext<'_>,
    candidate: &MemberCandidate,
) -> Option<String> {
    if candidate.kind != kernc_sema::scope::SymbolKind::Function {
        return None;
    }

    Some(function_completion_snippet(
        ctx,
        ctx.resolve(candidate.name),
        candidate.type_id,
    ))
}

fn function_completion_snippet(
    ctx: &SemaContext<'_>,
    label: &str,
    ty: kernc_sema::ty::TypeId,
) -> String {
    let normalized = ctx.type_registry.normalize(ty);
    let has_parameters = match ctx.type_registry.get(normalized) {
        kernc_sema::ty::TypeKind::Function {
            params,
            is_variadic,
            ..
        } => !params.is_empty() || *is_variadic,
        _ => false,
    };

    if has_parameters {
        format!("{label}($0)")
    } else {
        format!("{label}()$0")
    }
}

fn query_span_for_stmt(stmt: &ast::Stmt) -> kernc_utils::Span {
    match &stmt.kind {
        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => query_span_for_expr(expr),
    }
}

fn completion_kind_from_symbol_kind(kind: kernc_sema::scope::SymbolKind) -> AnalysisCompletionKind {
    match kind {
        kernc_sema::scope::SymbolKind::Var => AnalysisCompletionKind::Variable,
        kernc_sema::scope::SymbolKind::Const => AnalysisCompletionKind::Constant,
        kernc_sema::scope::SymbolKind::Static => AnalysisCompletionKind::Static,
        kernc_sema::scope::SymbolKind::Function => AnalysisCompletionKind::Function,
        kernc_sema::scope::SymbolKind::Struct => AnalysisCompletionKind::Struct,
        kernc_sema::scope::SymbolKind::Union => AnalysisCompletionKind::Union,
        kernc_sema::scope::SymbolKind::Enum => AnalysisCompletionKind::Enum,
        kernc_sema::scope::SymbolKind::Trait => AnalysisCompletionKind::Trait,
        kernc_sema::scope::SymbolKind::Module => AnalysisCompletionKind::Module,
        kernc_sema::scope::SymbolKind::TypeAlias => AnalysisCompletionKind::TypeAlias,
        kernc_sema::scope::SymbolKind::TypeParam => AnalysisCompletionKind::TypeParameter,
    }
}

fn query_span_for_expr(expr: &ast::Expr) -> kernc_utils::Span {
    match &expr.kind {
        ast::ExprKind::Return(Some(value)) => expr.span.to(query_span_for_expr(value)),
        _ => expr.span,
    }
}

fn span_contains_offset(span: kernc_utils::Span, offset: usize) -> bool {
    let end = if span.end > span.start {
        span.end
    } else {
        span.start.saturating_add(1)
    };
    offset >= span.start && offset < end
}

fn normalize_analysis_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
