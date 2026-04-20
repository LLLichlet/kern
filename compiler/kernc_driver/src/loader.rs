use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::compiler::PhaseTiming;
use crate::frontend::FrontendDatabase;
use crate::metadata;
use kernc_ast as ast;
use kernc_sema::SemaContext;
use kernc_sema::def::{Def, DefId, ModuleDef};
use kernc_utils::{FastHashMap, FastHashSet, SymbolId};

struct ResolvedRootModule {
    entry_path: PathBuf,
    declared_root_name: Option<SymbolId>,
    package_name: Option<SymbolId>,
}

#[derive(Default)]
struct ModuleLoadTimings {
    normalize_path: Duration,
    frontend_read_source: Duration,
    frontend_ensure_file_id: Duration,
    frontend_parse: Duration,
    frontend_prune: Duration,
    frontend_rebind: Duration,
    resolve_submodule_paths: Duration,
}

pub struct ModuleLoader<'a, 'ctx> {
    pub ctx: &'a mut SemaContext<'ctx>,
    // Prevent import cycles: physical absolute path -> module ID.
    pub loaded_files: FastHashMap<PathBuf, DefId>,
    path_exists_cache: FastHashMap<PathBuf, bool>,
    // Cache parsed ASTs until the collector extracts semantic symbols.
    pub asts: Vec<(DefId, ast::Module)>,
    known_alias_names: FastHashSet<SymbolId>,
    module_alias_references: Vec<FastHashSet<SymbolId>>,
    frontend: &'a FrontendDatabase,
    timings: ModuleLoadTimings,
    collect_docs: bool,
}

impl<'a, 'ctx> ModuleLoader<'a, 'ctx> {
    pub fn new(
        ctx: &'a mut SemaContext<'ctx>,
        frontend: &'a FrontendDatabase,
        collect_docs: bool,
    ) -> Self {
        let mut known_alias_names = FastHashSet::default();
        let module_alias_names = ctx.module_aliases.keys().cloned().collect::<Vec<_>>();
        let interface_alias_names = ctx
            .module_interface_aliases
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for alias_name in module_alias_names {
            known_alias_names.insert(ctx.intern(&alias_name));
        }
        for alias_name in interface_alias_names {
            known_alias_names.insert(ctx.intern(&alias_name));
        }
        Self {
            ctx,
            loaded_files: FastHashMap::default(),
            path_exists_cache: FastHashMap::default(),
            asts: Vec::new(),
            known_alias_names,
            module_alias_references: Vec::new(),
            frontend,
            timings: ModuleLoadTimings::default(),
            collect_docs,
        }
    }

    pub fn phase_timings(&self) -> Vec<PhaseTiming> {
        [
            ("    load_normalize_path", self.timings.normalize_path),
            ("    load_read_source", self.timings.frontend_read_source),
            (
                "    load_ensure_file_id",
                self.timings.frontend_ensure_file_id,
            ),
            ("    load_parse", self.timings.frontend_parse),
            ("    load_prune", self.timings.frontend_prune),
            ("    load_rebind", self.timings.frontend_rebind),
            (
                "    load_resolve_submodule_paths",
                self.timings.resolve_submodule_paths,
            ),
        ]
        .into_iter()
        .filter(|(_, duration)| !duration.is_zero())
        .map(|(name, duration)| PhaseTiming { name, duration })
        .collect()
    }

    pub fn load_root(&mut self, root_file: &str, root_name: SymbolId) -> Option<DefId> {
        let path = PathBuf::from(root_file);
        let root_id = self.load_module(path, None, root_name, false);
        if let (Some(root_id), Some(package_name)) = (root_id, self.ctx.current_package_name) {
            self.ctx
                .root_module_package_names
                .insert(root_id, package_name);
        }
        self.ctx.root_module = root_id;
        self.load_referenced_alias_roots(false);
        self.load_referenced_alias_roots(true);
        root_id
    }

    fn load_referenced_alias_roots(&mut self, imported: bool) {
        let aliases = if imported {
            self.ctx.module_interface_aliases.clone()
        } else {
            self.ctx.module_aliases.clone()
        };
        if aliases.is_empty() {
            return;
        }

        let mut pending = aliases
            .into_iter()
            .map(|(alias_name, alias_path)| (self.ctx.intern(&alias_name), alias_path))
            .collect::<Vec<_>>();

        loop {
            if pending.is_empty() {
                break;
            }

            let available_aliases = pending
                .iter()
                .map(|(alias_sym, _)| *alias_sym)
                .collect::<FastHashSet<_>>();
            let referenced_aliases = self.collect_referenced_aliases(&available_aliases);
            if referenced_aliases.is_empty() {
                break;
            }

            let mut progressed = false;
            let mut remaining = Vec::with_capacity(pending.len());

            for (alias_sym, alias_path) in pending {
                if !referenced_aliases.contains(&alias_sym) {
                    remaining.push((alias_sym, alias_path));
                    continue;
                }

                let Some(root) = self.resolve_root_module(&PathBuf::from(&alias_path), imported)
                else {
                    progressed = true;
                    continue;
                };

                let alias_package_name = (!imported
                    && self
                        .ctx
                        .current_package_name
                        .is_some_and(|package_name| package_name == alias_sym))
                .then_some(alias_sym);
                let module_name = root.declared_root_name.unwrap_or(alias_sym);
                if let Some(mod_id) = self.load_module(root.entry_path, None, module_name, imported)
                {
                    if let Some(package_name) = root.package_name.or(alias_package_name) {
                        self.ctx
                            .root_module_package_names
                            .insert(mod_id, package_name);
                    }
                    self.ctx.alias_roots.insert(alias_sym, mod_id);
                }
                progressed = true;
            }

            if !progressed {
                break;
            }

            pending = remaining;
        }
    }

    fn collect_referenced_aliases(
        &self,
        alias_names: &FastHashSet<SymbolId>,
    ) -> FastHashSet<SymbolId> {
        let mut referenced = FastHashSet::default();
        if self.ctx.program_entry_enabled()
            && let Some(rt) = alias_names
                .iter()
                .copied()
                .find(|alias| self.ctx.resolve(*alias) == "rt")
        {
            referenced.insert(rt);
        }
        for module_references in &self.module_alias_references {
            for alias in module_references {
                if alias_names.contains(alias) {
                    referenced.insert(*alias);
                }
            }
        }
        referenced
    }

    fn collect_module_alias_references(
        module: &ast::Module,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        for attribute in &module.attributes {
            Self::collect_attribute_alias_references(attribute, alias_names, referenced);
        }
        for decl in &module.decls {
            Self::collect_decl_alias_references(decl, alias_names, referenced);
        }
    }

    fn collect_decl_alias_references(
        decl: &ast::Decl,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        for attribute in &decl.attributes {
            Self::collect_attribute_alias_references(attribute, alias_names, referenced);
        }

        match &decl.kind {
            ast::DeclKind::Function {
                generics: _,
                where_clauses,
                params,
                ret_type,
                body,
                ..
            } => {
                for clause in where_clauses {
                    Self::collect_where_clause_alias_references(clause, alias_names, referenced);
                }
                for param in params {
                    Self::collect_func_param_alias_references(param, alias_names, referenced);
                }
                Self::collect_type_alias_references(ret_type, alias_names, referenced);
                if let Some(body) = body {
                    Self::collect_expr_alias_references(body, alias_names, referenced);
                }
            }
            ast::DeclKind::Var { value, .. } => {
                Self::collect_expr_alias_references(value, alias_names, referenced);
            }
            ast::DeclKind::TypeAlias {
                bounds,
                where_clauses,
                target,
                ..
            } => {
                for bound in bounds {
                    Self::collect_type_alias_references(bound, alias_names, referenced);
                }
                for clause in where_clauses {
                    Self::collect_where_clause_alias_references(clause, alias_names, referenced);
                }
                Self::collect_type_alias_references(target, alias_names, referenced);
            }
            ast::DeclKind::ModDecl => {}
            ast::DeclKind::Use {
                kind, path, target, ..
            } => {
                if matches!(kind, ast::UsePathKind::External)
                    && let Some(&root) = path.first()
                    && alias_names.contains(&root)
                {
                    referenced.insert(root);
                }
                if let ast::UseTarget::Tree(items) = target {
                    for item in items {
                        Self::collect_use_tree_alias_references(item, alias_names, referenced);
                    }
                }
            }
            ast::DeclKind::ExternBlock { decls, .. } => {
                for decl in decls {
                    Self::collect_decl_alias_references(decl, alias_names, referenced);
                }
            }
            ast::DeclKind::Impl {
                where_clauses,
                target_type,
                trait_type,
                decls,
                ..
            } => {
                for clause in where_clauses {
                    Self::collect_where_clause_alias_references(clause, alias_names, referenced);
                }
                Self::collect_type_alias_references(target_type, alias_names, referenced);
                if let Some(trait_type) = trait_type {
                    Self::collect_type_alias_references(trait_type, alias_names, referenced);
                }
                for decl in decls {
                    Self::collect_decl_alias_references(decl, alias_names, referenced);
                }
            }
        }
    }

    fn collect_attribute_alias_references(
        attribute: &ast::Attribute,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match &attribute.kind {
            ast::AttributeKind::If(expr) => {
                Self::collect_expr_alias_references(expr, alias_names, referenced);
            }
            ast::AttributeKind::Meta(items) => {
                for item in items {
                    if let ast::MetaItem::Call(_, expr) = item {
                        Self::collect_expr_alias_references(expr, alias_names, referenced);
                    }
                }
            }
        }
    }

    fn collect_use_tree_alias_references(
        tree: &ast::UseTree,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match tree {
            ast::UseTree::SelfModule { .. } => {}
            ast::UseTree::Path { path, nested, .. } => {
                if let Some(&root) = path.first()
                    && alias_names.contains(&root)
                {
                    referenced.insert(root);
                }
                if let Some(nested) = nested {
                    for item in nested {
                        Self::collect_use_tree_alias_references(item, alias_names, referenced);
                    }
                }
            }
        }
    }

    fn collect_where_clause_alias_references(
        clause: &ast::WhereClause,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        Self::collect_type_alias_references(&clause.target_ty, alias_names, referenced);
        for bound in &clause.bounds {
            Self::collect_type_alias_references(bound, alias_names, referenced);
        }
    }

    fn collect_func_param_alias_references(
        param: &ast::FuncParam,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        Self::collect_type_alias_references(&param.type_node, alias_names, referenced);
    }

    fn collect_type_alias_references(
        ty: &ast::TypeNode,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match &ty.kind {
            ast::TypeKind::Path {
                anchor: None,
                segments,
            } => {
                if let Some(root) = segments.first()
                    && alias_names.contains(&root.name)
                {
                    referenced.insert(root.name);
                }
                for segment in segments {
                    for arg in &segment.args {
                        match arg {
                            ast::GenericArg::Type(generic) => {
                                Self::collect_type_alias_references(
                                    generic,
                                    alias_names,
                                    referenced,
                                );
                            }
                            ast::GenericArg::ConstExpr(expr) => {
                                Self::collect_expr_alias_references(expr, alias_names, referenced);
                            }
                            ast::GenericArg::AssocBinding { value, .. } => {
                                Self::collect_type_alias_references(value, alias_names, referenced);
                            }
                        }
                    }
                }
            }
            ast::TypeKind::Path { segments, .. } => {
                for segment in segments {
                    for arg in &segment.args {
                        match arg {
                            ast::GenericArg::Type(generic) => {
                                Self::collect_type_alias_references(
                                    generic,
                                    alias_names,
                                    referenced,
                                );
                            }
                            ast::GenericArg::ConstExpr(expr) => {
                                Self::collect_expr_alias_references(expr, alias_names, referenced);
                            }
                            ast::GenericArg::AssocBinding { value, .. } => {
                                Self::collect_type_alias_references(value, alias_names, referenced);
                            }
                        }
                    }
                }
            }
            ast::TypeKind::Optional { inner } => {
                Self::collect_type_alias_references(inner, alias_names, referenced);
            }
            ast::TypeKind::Result { ok, err } => {
                Self::collect_type_alias_references(ok, alias_names, referenced);
                Self::collect_type_alias_references(err, alias_names, referenced);
            }
            ast::TypeKind::Pointer { elem, .. }
            | ast::TypeKind::VolatilePtr { elem, .. }
            | ast::TypeKind::ArrayInfer { elem, .. }
            | ast::TypeKind::Slice { elem, .. } => {
                Self::collect_type_alias_references(elem, alias_names, referenced);
            }
            ast::TypeKind::Array { elem, len, .. } => {
                Self::collect_type_alias_references(elem, alias_names, referenced);
                Self::collect_expr_alias_references(len, alias_names, referenced);
            }
            ast::TypeKind::Function { params, ret, .. }
            | ast::TypeKind::ClosureInterface { params, ret } => {
                for param in params {
                    Self::collect_type_alias_references(param, alias_names, referenced);
                }
                if let Some(ret) = ret {
                    Self::collect_type_alias_references(ret, alias_names, referenced);
                }
            }
            ast::TypeKind::Struct { fields, .. } | ast::TypeKind::Union { fields, .. } => {
                for field in fields {
                    Self::collect_struct_field_alias_references(field, alias_names, referenced);
                }
            }
            ast::TypeKind::Trait {
                assoc_types,
                methods,
            } => {
                for assoc in assoc_types {
                    for bound in &assoc.bounds {
                        Self::collect_type_alias_references(bound, alias_names, referenced);
                    }
                    for clause in &assoc.where_clauses {
                        Self::collect_type_alias_references(
                            &clause.target_ty,
                            alias_names,
                            referenced,
                        );
                        for bound in &clause.bounds {
                            Self::collect_type_alias_references(bound, alias_names, referenced);
                        }
                    }
                }
                for method in methods {
                    Self::collect_struct_field_alias_references(method, alias_names, referenced);
                }
            }
            ast::TypeKind::Enum {
                backing_type,
                variants,
            } => {
                if let Some(backing_type) = backing_type {
                    Self::collect_type_alias_references(backing_type, alias_names, referenced);
                }
                for variant in variants {
                    if let Some(payload_type) = &variant.payload_type {
                        Self::collect_type_alias_references(payload_type, alias_names, referenced);
                    }
                    if let Some(value) = &variant.value {
                        Self::collect_expr_alias_references(value, alias_names, referenced);
                    }
                }
            }
            ast::TypeKind::TypeOf(expr) => {
                Self::collect_expr_alias_references(expr, alias_names, referenced);
            }
            ast::TypeKind::Infer
            | ast::TypeKind::SelfType
            | ast::TypeKind::Never
            | ast::TypeKind::Void => {}
        }
    }

    fn collect_struct_field_alias_references(
        field: &ast::StructFieldDef,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        Self::collect_type_alias_references(&field.type_node, alias_names, referenced);
        if let Some(default_value) = &field.default_value {
            Self::collect_expr_alias_references(default_value, alias_names, referenced);
        }
    }

    fn collect_expr_alias_references(
        expr: &ast::Expr,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match &expr.kind {
            ast::ExprKind::Let {
                pattern,
                init,
                else_clause,
            } => {
                Self::collect_let_pattern_alias_references(pattern, alias_names, referenced);
                Self::collect_expr_alias_references(init, alias_names, referenced);
                if let Some(else_clause) = else_clause {
                    match else_clause {
                        ast::LetElseClause::Expr(else_expr) => {
                            Self::collect_expr_alias_references(else_expr, alias_names, referenced);
                        }
                        ast::LetElseClause::Arms(arms) => {
                            for arm in arms {
                                Self::collect_pattern_alias_references(
                                    &arm.pattern,
                                    alias_names,
                                    referenced,
                                );
                                Self::collect_expr_alias_references(
                                    &arm.body,
                                    alias_names,
                                    referenced,
                                );
                            }
                        }
                    }
                }
            }
            ast::ExprKind::Static { init, .. } => {
                Self::collect_expr_alias_references(init, alias_names, referenced);
            }
            ast::ExprKind::Identifier(name) => {
                if alias_names.contains(name) {
                    referenced.insert(*name);
                }
            }
            ast::ExprKind::AnchoredPath { .. } => {}
            ast::ExprKind::TypeNode(type_node) => {
                Self::collect_type_alias_references(type_node, alias_names, referenced);
            }
            ast::ExprKind::Binary { lhs, rhs, .. } | ast::ExprKind::Assign { lhs, rhs, .. } => {
                Self::collect_expr_alias_references(lhs, alias_names, referenced);
                Self::collect_expr_alias_references(rhs, alias_names, referenced);
            }
            ast::ExprKind::Unary { operand, .. } => {
                Self::collect_expr_alias_references(operand, alias_names, referenced);
            }
            ast::ExprKind::FieldAccess { lhs, .. } => {
                Self::collect_expr_alias_references(lhs, alias_names, referenced);
            }
            ast::ExprKind::IndexAccess { lhs, index, .. } => {
                Self::collect_expr_alias_references(lhs, alias_names, referenced);
                Self::collect_expr_alias_references(index, alias_names, referenced);
            }
            ast::ExprKind::Call { callee, args } => {
                Self::collect_expr_alias_references(callee, alias_names, referenced);
                for arg in args {
                    Self::collect_expr_alias_references(arg, alias_names, referenced);
                }
            }
            ast::ExprKind::DataInit { type_node, literal } => {
                if let Some(type_node) = type_node {
                    Self::collect_type_alias_references(type_node, alias_names, referenced);
                }
                Self::collect_data_literal_alias_references(literal, alias_names, referenced);
            }
            ast::ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                Self::collect_expr_alias_references(cond, alias_names, referenced);
                Self::collect_expr_alias_references(then_branch, alias_names, referenced);
                if let Some(else_branch) = else_branch {
                    Self::collect_expr_alias_references(else_branch, alias_names, referenced);
                }
            }
            ast::ExprKind::Match { target, arms } => {
                Self::collect_expr_alias_references(target, alias_names, referenced);
                for arm in arms {
                    for pattern in &arm.patterns {
                        Self::collect_match_pattern_alias_references(
                            pattern,
                            alias_names,
                            referenced,
                        );
                    }
                    Self::collect_expr_alias_references(&arm.body, alias_names, referenced);
                }
            }
            ast::ExprKind::Block { stmts, result } => {
                for stmt in stmts {
                    for attribute in &stmt.attributes {
                        Self::collect_attribute_alias_references(
                            attribute,
                            alias_names,
                            referenced,
                        );
                    }
                    match &stmt.kind {
                        ast::StmtKind::Use(_) => {}
                        ast::StmtKind::ExprStmt(expr) | ast::StmtKind::ExprValue(expr) => {
                            Self::collect_expr_alias_references(expr, alias_names, referenced);
                        }
                    }
                }
                if let Some(result) = result {
                    Self::collect_expr_alias_references(result, alias_names, referenced);
                }
            }
            ast::ExprKind::For {
                init,
                cond,
                post,
                body,
            } => {
                if let Some(init) = init {
                    Self::collect_expr_alias_references(init, alias_names, referenced);
                }
                if let Some(cond) = cond {
                    Self::collect_expr_alias_references(cond, alias_names, referenced);
                }
                if let Some(post) = post {
                    Self::collect_expr_alias_references(post, alias_names, referenced);
                }
                Self::collect_expr_alias_references(body, alias_names, referenced);
            }
            ast::ExprKind::SliceOp {
                lhs, start, end, ..
            } => {
                Self::collect_expr_alias_references(lhs, alias_names, referenced);
                if let Some(start) = start {
                    Self::collect_expr_alias_references(start, alias_names, referenced);
                }
                if let Some(end) = end {
                    Self::collect_expr_alias_references(end, alias_names, referenced);
                }
            }
            ast::ExprKind::Defer { expr } => {
                Self::collect_expr_alias_references(expr, alias_names, referenced);
            }
            ast::ExprKind::Return(value) => {
                if let Some(value) = value {
                    Self::collect_expr_alias_references(value, alias_names, referenced);
                }
            }
            ast::ExprKind::As { lhs, target } => {
                Self::collect_expr_alias_references(lhs, alias_names, referenced);
                Self::collect_type_alias_references(target, alias_names, referenced);
            }
            ast::ExprKind::Propagate { operand, .. } => {
                Self::collect_expr_alias_references(operand, alias_names, referenced);
            }
            ast::ExprKind::GenericInstantiation { target, args } => {
                Self::collect_expr_alias_references(target, alias_names, referenced);
                for arg in args {
                    match arg {
                        ast::GenericArg::Type(ty) => {
                            Self::collect_type_alias_references(ty, alias_names, referenced);
                        }
                        ast::GenericArg::ConstExpr(expr) => {
                            Self::collect_expr_alias_references(expr, alias_names, referenced);
                        }
                        ast::GenericArg::AssocBinding { value, .. } => {
                            Self::collect_type_alias_references(value, alias_names, referenced);
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
                    Self::collect_expr_alias_references(&capture.value, alias_names, referenced);
                }
                for param in params {
                    Self::collect_func_param_alias_references(param, alias_names, referenced);
                }
                Self::collect_type_alias_references(ret_type, alias_names, referenced);
                Self::collect_expr_alias_references(body, alias_names, referenced);
            }
            ast::ExprKind::Integer(_)
            | ast::ExprKind::Float(_)
            | ast::ExprKind::Bool(_)
            | ast::ExprKind::Char(_)
            | ast::ExprKind::ByteChar(_)
            | ast::ExprKind::String(_)
            | ast::ExprKind::EnumLiteral { .. }
            | ast::ExprKind::Break
            | ast::ExprKind::Continue
            | ast::ExprKind::Undef
            | ast::ExprKind::Infer
            | ast::ExprKind::SelfValue => {}
        }
    }

    fn collect_data_literal_alias_references(
        literal: &ast::DataLiteralKind,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match literal {
            ast::DataLiteralKind::Struct(fields) => {
                for field in fields {
                    Self::collect_expr_alias_references(&field.value, alias_names, referenced);
                }
            }
            ast::DataLiteralKind::Array(items) => {
                for item in items {
                    Self::collect_expr_alias_references(item, alias_names, referenced);
                }
            }
            ast::DataLiteralKind::Repeat { value, count } => {
                Self::collect_expr_alias_references(value, alias_names, referenced);
                Self::collect_expr_alias_references(count, alias_names, referenced);
            }
            ast::DataLiteralKind::Scalar(value) => {
                Self::collect_expr_alias_references(value, alias_names, referenced);
            }
        }
    }

    fn collect_let_pattern_alias_references(
        pattern: &ast::LetPattern,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        Self::collect_pattern_alias_references(&pattern.pattern, alias_names, referenced);
    }

    fn collect_pattern_alias_references(
        pattern: &ast::Pattern,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match &pattern.kind {
            ast::PatternKind::Binding(_) | ast::PatternKind::Ignore => {}
            ast::PatternKind::Variant(variant) => {
                if let Some(target_type) = &variant.target_type {
                    Self::collect_type_alias_references(target_type, alias_names, referenced);
                }
            }
            ast::PatternKind::Destructure(destructure) => {
                if let Some(target_type) = &destructure.target_type {
                    Self::collect_type_alias_references(target_type, alias_names, referenced);
                }
                for field in &destructure.fields {
                    Self::collect_pattern_alias_references(&field.pattern, alias_names, referenced);
                }
            }
        }
    }

    fn collect_match_pattern_alias_references(
        pattern: &ast::MatchPattern,
        alias_names: &FastHashSet<SymbolId>,
        referenced: &mut FastHashSet<SymbolId>,
    ) {
        match &pattern.kind {
            ast::MatchPatternKind::Value(value) => {
                Self::collect_expr_alias_references(value, alias_names, referenced);
            }
            ast::MatchPatternKind::Range { start, end, .. } => {
                Self::collect_expr_alias_references(start, alias_names, referenced);
                Self::collect_expr_alias_references(end, alias_names, referenced);
            }
            ast::MatchPatternKind::Pattern(pattern) => {
                Self::collect_pattern_alias_references(pattern, alias_names, referenced);
            }
        }
    }

    fn resolve_root_module(
        &mut self,
        base_path: &Path,
        require_manifest: bool,
    ) -> Option<ResolvedRootModule> {
        if base_path.is_dir() {
            match metadata::load_manifest(base_path) {
                Ok(Some(manifest)) => {
                    let entry_path = base_path.join(&manifest.entry_module_path);
                    if !entry_path.is_file() {
                        eprintln!(
                            "Error: kmeta package at `{}` points to missing entry module `{}`",
                            base_path.display(),
                            entry_path.display()
                        );
                        return None;
                    }

                    let declared_root_name = Some(self.ctx.intern(&manifest.root_module_name));
                    let package_name = Some(self.ctx.intern(&manifest.package_name));
                    return Some(ResolvedRootModule {
                        entry_path,
                        declared_root_name,
                        package_name,
                    });
                }
                Ok(None) => {
                    if require_manifest {
                        eprintln!(
                            "Error: Imported package path `{}` is missing `{}`",
                            base_path.display(),
                            metadata::KMETA_MANIFEST_FILE
                        );
                        return None;
                    }
                }
                Err(err) => {
                    eprintln!(
                        "Error: Failed to read kmeta manifest from `{}`: {}",
                        base_path.display(),
                        err
                    );
                    return None;
                }
            }
        }

        if require_manifest {
            eprintln!(
                "Error: Imported package alias expects a kmeta package root at `{}`",
                base_path.display()
            );
            return None;
        }

        let dir_init = base_path.join("init.rn");
        let file_kn = PathBuf::from(format!("{}.rn", base_path.display()));

        if self.path_exists(&dir_init) {
            Some(ResolvedRootModule {
                entry_path: dir_init,
                declared_root_name: None,
                package_name: None,
            })
        } else if self.path_exists(&file_kn) {
            Some(ResolvedRootModule {
                entry_path: file_kn,
                declared_root_name: None,
                package_name: None,
            })
        } else if self.path_exists(base_path) && !base_path.is_dir() {
            Some(ResolvedRootModule {
                entry_path: base_path.to_path_buf(),
                declared_root_name: None,
                package_name: None,
            })
        } else {
            None
        }
    }

    fn resolve_submodule_path(&mut self, dir_path: &Path, decl: &ast::Decl) -> Option<PathBuf> {
        let mod_name = self.ctx.resolve(decl.name).to_string();
        let dir_init = dir_path.join(&mod_name).join("init.rn");
        let file_kn = dir_path.join(format!("{}.rn", mod_name));

        if self.path_exists(&dir_init) {
            Some(dir_init)
        } else if self.path_exists(&file_kn) {
            Some(file_kn)
        } else {
            self.ctx
                .struct_error(
                    decl.span,
                    format!("Cannot find module file for `{}`", mod_name),
                )
                .with_hint(format!(
                    "expected to find `{}` or `{}`",
                    file_kn.display(),
                    dir_init.display()
                ))
                .emit();
            None
        }
    }

    fn load_module(
        &mut self,
        path: PathBuf,
        parent: Option<DefId>,
        name: SymbolId,
        is_imported: bool,
    ) -> Option<DefId> {
        let normalize_started = Instant::now();
        let abs_path = Self::normalize_path(&path);
        self.timings.normalize_path += normalize_started.elapsed();
        self.load_module_normalized(abs_path, parent, name, is_imported)
    }

    fn load_module_normalized(
        &mut self,
        abs_path: PathBuf,
        parent: Option<DefId>,
        name: SymbolId,
        is_imported: bool,
    ) -> Option<DefId> {
        if let Some(&mod_id) = self.loaded_files.get(&abs_path) {
            return Some(mod_id);
        }

        let parsed = match self.frontend.load_parsed_module_normalized_profiled(
            self.ctx.sess,
            &abs_path,
            self.collect_docs,
        ) {
            Ok(Some((parsed, timings))) => {
                self.timings.frontend_read_source += timings.read_source;
                self.timings.frontend_ensure_file_id += timings.ensure_file_id;
                self.timings.frontend_parse += timings.parse;
                self.timings.frontend_prune += timings.prune;
                self.timings.frontend_rebind += timings.rebind;
                parsed
            }
            Ok(None) => {
                self.ctx.sess.error_count += 1;
                eprintln!(
                    "Error: Cannot read or parse module file '{}'.",
                    abs_path.display()
                );
                return None;
            }
            Err(err) => {
                self.ctx.sess.error_count += 1;
                eprintln!(
                    "Error: Query cycle while loading module '{}': {}",
                    abs_path.display(),
                    err
                );
                return None;
            }
        };
        let Some(dir_path) = abs_path.parent().map(|p| p.to_path_buf()) else {
            self.ctx.sess.error_count += 1;
            eprintln!(
                "Error: Cannot determine parent directory for module file '{}'.",
                abs_path.display()
            );
            return None;
        };

        let mod_id = DefId(self.ctx.defs.len() as u32);
        self.loaded_files.insert(abs_path.clone(), mod_id);
        let file_id = parsed.file_id;

        let scope_id = self.ctx.scopes.enter_scope();
        self.ctx.scopes.exit_scope();

        let is_init = abs_path.file_name().and_then(|n| n.to_str()) == Some("init.rn");

        let dummy_def = ModuleDef {
            id: mod_id,
            name,
            parent,
            is_imported,
            scope_id,
            dir_path: dir_path.clone(),
            file_id,
            is_init,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
            docs: None,
        };
        self.ctx.add_def(Def::Module(dummy_def));
        self.ctx.register_module_scope(mod_id, scope_id);
        self.ctx.register_def_owner(mod_id, parent, Some(scope_id));
        let ast = parsed.ast;

        let mut submodules = HashMap::new();
        for decl in &ast.decls {
            if let ast::DeclKind::ModDecl = &decl.kind {
                let resolve_started = Instant::now();
                let resolved = self.resolve_submodule_path(&dir_path, decl);
                self.timings.resolve_submodule_paths += resolve_started.elapsed();

                if let Some(path) = resolved
                    && let Some(sub_id) =
                        self.load_module_normalized(path, Some(mod_id), decl.name, is_imported)
                {
                    submodules.insert(decl.name, sub_id);
                }
            }
        }

        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.submodules = submodules;
        }

        let mut module_alias_references = FastHashSet::default();
        Self::collect_module_alias_references(
            &ast,
            &self.known_alias_names,
            &mut module_alias_references,
        );
        self.module_alias_references.push(module_alias_references);
        self.asts.push((mod_id, ast));
        Some(mod_id)
    }

    fn normalize_path(path: &Path) -> PathBuf {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path = Self::strip_windows_verbatim_prefix(path);
        Self::strip_macos_private_var_prefix(path)
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

    fn path_exists(&mut self, path: &Path) -> bool {
        if let Some(exists) = self.path_exists_cache.get(path).copied() {
            return exists;
        }

        let exists = self.frontend.source_exists(path);
        self.path_exists_cache.insert(path.to_path_buf(), exists);
        exists
    }
}
