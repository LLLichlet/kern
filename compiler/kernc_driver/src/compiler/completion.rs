mod facts;
mod member;
mod model;

pub(super) use self::facts::module_body_completion_regions;
pub(super) use self::model::parsed_requires_body_completion;

use self::facts::{
    collect_module_binding_completion_facts, collect_module_block_completion_facts,
    collect_module_closure_completion_facts, collect_module_if_completion_facts,
    collect_module_match_completion_facts, module_surface_decls,
};
use self::model::push_completion_item;
use super::{AnalysisCompletionItem, AnalysisCompletionKind, CompilerDriver};
use crate::compiler::analysis::module_analysis_path_from_source;
use crate::language::is_language_builtin_def_id;
use kernc_ast as ast;
use kernc_sema::def::DefId;
use kernc_sema::{MemberCandidate, MemberQuery, MemberQueryEnv, SemaContext};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct CompletionModule {
    pub(super) path: PathBuf,
    pub(super) source_path: PathBuf,
    pub(super) ast: ast::Module,
    pub(super) body_regions: Vec<kernc_utils::Span>,
    pub(super) surface_decls: Vec<CompletionSurfaceDecl>,
    pub(super) top_level_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionSurfaceDecl {
    pub(super) span: kernc_utils::Span,
    pub(super) function_items: Vec<AnalysisCompletionItem>,
    pub(super) children: Vec<CompletionSurfaceDecl>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionBlockStmtFacts {
    pub(super) span: kernc_utils::Span,
    pub(super) prefix_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionBlockFacts {
    pub(super) stmt_facts: Vec<CompletionBlockStmtFacts>,
    pub(super) tail_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionMatchArmFacts {
    pub(super) span: kernc_utils::Span,
    pub(super) body_span: kernc_utils::Span,
    pub(super) binding_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionMatchFacts {
    pub(super) arms: Vec<CompletionMatchArmFacts>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionClosureFacts {
    pub(super) body_span: kernc_utils::Span,
    pub(super) binding_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionLetElseFacts {
    pub(super) binding_items: Vec<AnalysisCompletionItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CompletionIfFacts {
    pub(super) then_span: kernc_utils::Span,
    pub(super) else_span: Option<kernc_utils::Span>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompletionModel {
    pub(super) root_items: Vec<AnalysisCompletionItem>,
    pub(super) function_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    pub(super) member_items_by_span: BTreeMap<kernc_utils::Span, Vec<AnalysisCompletionItem>>,
    pub(super) block_facts_by_span: BTreeMap<kernc_utils::Span, CompletionBlockFacts>,
    pub(super) match_facts_by_span: BTreeMap<kernc_utils::Span, CompletionMatchFacts>,
    pub(super) closure_facts_by_span: BTreeMap<kernc_utils::Span, CompletionClosureFacts>,
    pub(super) let_else_facts_by_span: BTreeMap<kernc_utils::Span, CompletionLetElseFacts>,
    pub(super) if_facts_by_span: BTreeMap<kernc_utils::Span, CompletionIfFacts>,
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

        let mut expr_binding_items_by_span = BTreeMap::new();
        let mut match_arm_binding_items_by_span = BTreeMap::new();
        let mut closure_binding_items_by_body_span = BTreeMap::new();
        let mut let_else_facts_by_span = BTreeMap::new();
        for (_mod_id, ast) in asts {
            collect_module_binding_completion_facts(
                ast,
                &items_by_span,
                &mut expr_binding_items_by_span,
                &mut match_arm_binding_items_by_span,
                &mut closure_binding_items_by_body_span,
                &mut let_else_facts_by_span,
            );
        }

        let mut block_facts_by_span = BTreeMap::new();
        for (_mod_id, ast) in asts {
            collect_module_block_completion_facts(
                ast,
                &expr_binding_items_by_span,
                &mut block_facts_by_span,
            );
        }

        let mut match_facts_by_span = BTreeMap::new();
        for (_mod_id, ast) in asts {
            collect_module_match_completion_facts(
                ast,
                &match_arm_binding_items_by_span,
                &mut match_facts_by_span,
            );
        }

        let mut closure_facts_by_span = BTreeMap::new();
        for (_mod_id, ast) in asts {
            collect_module_closure_completion_facts(
                ast,
                &closure_binding_items_by_body_span,
                &mut closure_facts_by_span,
            );
        }

        let mut if_facts_by_span = BTreeMap::new();
        for (_mod_id, ast) in asts {
            collect_module_if_completion_facts(ast, &mut if_facts_by_span);
        }

        let modules = asts
            .iter()
            .filter_map(|(mod_id, ast)| {
                let kernc_sema::def::Def::Module(module_def) = &ctx.defs[mod_id.0 as usize] else {
                    return None;
                };
                let module_file_id = module_def.file_id;
                let module_scope_id = module_def.scope_id;
                let source_path = ctx
                    .sess
                    .source_manager
                    .get_file_path(module_file_id)
                    .map(|path| normalize_analysis_path(path))?;
                let module_path = module_analysis_path_from_source(&source_path, ast);

                let top_level_items = ctx
                    .scopes
                    .symbols_in_scope(module_scope_id)
                    .filter_map(|(name, info)| self.completion_item_for_symbol(ctx, name, info))
                    .collect();

                Some(CompletionModule {
                    path: module_path,
                    source_path,
                    ast: ast.clone(),
                    body_regions: module_body_completion_regions(ast),
                    surface_decls: module_surface_decls(ast, &function_items_by_span),
                    top_level_items,
                })
            })
            .collect();

        CompletionModel {
            root_items,
            function_items_by_span,
            block_facts_by_span,
            match_facts_by_span,
            closure_facts_by_span,
            let_else_facts_by_span,
            if_facts_by_span,
            member_items_by_span: BTreeMap::new(),
            modules,
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

        let param_types =
            function
                .resolved_sig
                .and_then(|sig| match ctx.type_registry.get(sig).clone() {
                    kernc_sema::ty::TypeKind::Function { params, .. } => Some(params),
                    _ => None,
                });

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
                    detail: param_types
                        .as_ref()
                        .and_then(|params| params.get(index).copied())
                        .map(|ty| ctx.ty_to_string(ty)),
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
        if info
            .def_id
            .is_some_and(|def_id| is_language_builtin_def_id(ctx, def_id))
        {
            return None;
        }

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
                (
                    AnalysisCompletionKind::Function,
                    function
                        .resolved_sig
                        .map(|sig| ctx.ty_to_string(sig))
                        .or_else(|| Some("fn".to_string())),
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
            kernc_sema::scope::SymbolKind::ConstParam => (
                AnalysisCompletionKind::Constant,
                Some(format!("const {}", ctx.ty_to_string(info.type_id))),
            ),
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
            kernc_sema::scope::SymbolKind::TypeAlias
            | kernc_sema::scope::SymbolKind::AssociatedType => {
                let detail = if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                {
                    ctx.node_type(alias.target.id)
                        .map(|target_ty| format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if let Some(def_id) = info.def_id
                    && let kernc_sema::def::Def::AssociatedType(assoc) =
                        &ctx.defs[def_id.0 as usize]
                    && let Some(target) = assoc.target.as_ref()
                {
                    ctx.node_type(target.id)
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
        if candidate
            .def_id
            .is_some_and(|def_id| is_language_builtin_def_id(ctx, def_id))
        {
            return None;
        }

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
            kernc_sema::scope::SymbolKind::ConstParam => {
                Some(format!("const {}", ctx.ty_to_string(candidate.type_id)))
            }
            kernc_sema::scope::SymbolKind::Module => {
                Some(format!("module {}", ctx.resolve(candidate.name)))
            }
            kernc_sema::scope::SymbolKind::Struct => Some("struct".to_string()),
            kernc_sema::scope::SymbolKind::Union => Some("union".to_string()),
            kernc_sema::scope::SymbolKind::Enum => Some("enum".to_string()),
            kernc_sema::scope::SymbolKind::Trait => Some("trait".to_string()),
            kernc_sema::scope::SymbolKind::TypeParam => Some("type".to_string()),
            kernc_sema::scope::SymbolKind::TypeAlias
            | kernc_sema::scope::SymbolKind::AssociatedType => {
                if let Some(def_id) = candidate.def_id
                    && let kernc_sema::def::Def::TypeAlias(alias) = &ctx.defs[def_id.0 as usize]
                    && let Some(target_ty) = ctx.node_type(alias.target.id)
                {
                    Some(format!("type = {}", ctx.ty_to_string(target_ty)))
                } else if let Some(def_id) = candidate.def_id
                    && let kernc_sema::def::Def::AssociatedType(assoc) =
                        &ctx.defs[def_id.0 as usize]
                    && let Some(target) = assoc.target.as_ref()
                    && let Some(target_ty) = ctx.node_type(target.id)
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

fn completion_kind_from_symbol_kind(kind: kernc_sema::scope::SymbolKind) -> AnalysisCompletionKind {
    match kind {
        kernc_sema::scope::SymbolKind::Var => AnalysisCompletionKind::Variable,
        kernc_sema::scope::SymbolKind::Const => AnalysisCompletionKind::Constant,
        kernc_sema::scope::SymbolKind::ConstParam => AnalysisCompletionKind::Constant,
        kernc_sema::scope::SymbolKind::Static => AnalysisCompletionKind::Static,
        kernc_sema::scope::SymbolKind::Function => AnalysisCompletionKind::Function,
        kernc_sema::scope::SymbolKind::Struct => AnalysisCompletionKind::Struct,
        kernc_sema::scope::SymbolKind::Union => AnalysisCompletionKind::Union,
        kernc_sema::scope::SymbolKind::Enum => AnalysisCompletionKind::Enum,
        kernc_sema::scope::SymbolKind::Trait => AnalysisCompletionKind::Trait,
        kernc_sema::scope::SymbolKind::Module => AnalysisCompletionKind::Module,
        kernc_sema::scope::SymbolKind::TypeAlias
        | kernc_sema::scope::SymbolKind::AssociatedType => AnalysisCompletionKind::TypeAlias,
        kernc_sema::scope::SymbolKind::TypeParam => AnalysisCompletionKind::TypeParameter,
    }
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
