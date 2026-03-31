use crate::SemaContext;
use crate::checker::{ExprChecker, Substituter};
use crate::def::{Def, DefId};
use crate::scope::SymbolKind;
use crate::ty::{TypeId, TypeKind};
use kernc_ast as ast;
use kernc_utils::{Span, SymbolId};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct MemberCandidate {
    pub name: SymbolId,
    pub kind: SymbolKind,
    pub type_id: TypeId,
    pub def_id: Option<DefId>,
    pub is_mut: bool,
}

#[derive(Debug, Clone, Default)]
pub struct MemberQueryEnv {
    active_bounds: Vec<(TypeId, Vec<TypeId>)>,
}

impl MemberQueryEnv {
    pub fn from_active_bounds(bounds: &[(TypeId, Vec<TypeId>)]) -> Self {
        Self {
            active_bounds: bounds.to_vec(),
        }
    }

    pub fn extend_with_where_clauses(
        &mut self,
        ctx: &SemaContext<'_>,
        where_clauses: &[ast::WhereClause],
    ) {
        for clause in where_clauses {
            let target_ty = ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let bounds = clause
                .bounds
                .iter()
                .filter_map(|bound| ctx.node_types.get(&bound.id).copied())
                .collect();
            self.active_bounds.push((target_ty, bounds));
        }
    }

    pub fn len(&self) -> usize {
        self.active_bounds.len()
    }

    pub fn truncate(&mut self, len: usize) {
        self.active_bounds.truncate(len);
    }

    fn active_bounds(&self) -> &[(TypeId, Vec<TypeId>)] {
        &self.active_bounds
    }
}

#[derive(Debug, Clone)]
pub struct MemberResolution {
    pub candidate: MemberCandidate,
    pub owner_trait_ty: Option<TypeId>,
}

pub struct MemberQuery<'a, 'ctx> {
    ctx: &'a mut SemaContext<'ctx>,
}

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    pub fn new(ctx: &'a mut SemaContext<'ctx>) -> Self {
        Self { ctx }
    }

    pub fn context(&self) -> &SemaContext<'ctx> {
        self.ctx
    }

    pub fn member_candidates(
        &mut self,
        current_module_id: DefId,
        receiver_ty: TypeId,
    ) -> Vec<MemberCandidate> {
        let env = MemberQueryEnv::default();
        self.member_candidates_in_env(Some(current_module_id), receiver_ty, &env)
    }

    pub fn member_candidates_in_env(
        &mut self,
        current_module_id: Option<DefId>,
        receiver_ty: TypeId,
        env: &MemberQueryEnv,
    ) -> Vec<MemberCandidate> {
        let mut candidates = Vec::new();
        let base_norm = base_type(self.ctx, receiver_ty);

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            self.collect_module_candidates(current_module_id, module_def_id, &mut candidates);
            return candidates;
        }

        for search_norm in self.search_types(receiver_ty) {
            match self.ctx.type_registry.get(search_norm).clone() {
                TypeKind::Def(def_id, generic_args) => {
                    self.collect_named_type_field_candidates(
                        current_module_id,
                        def_id,
                        &generic_args,
                        &mut candidates,
                    );
                }
                TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) => {
                    for field in fields {
                        push_member_candidate(
                            &mut candidates,
                            MemberCandidate {
                                name: field.name,
                                kind: SymbolKind::Var,
                                type_id: field.ty,
                                def_id: None,
                                is_mut: false,
                            },
                        );
                    }
                }
                TypeKind::TraitObject(trait_def_id, trait_args) => {
                    self.collect_trait_object_method_candidates(
                        trait_def_id,
                        &trait_args,
                        receiver_ty,
                        &mut candidates,
                    );
                }
                _ => {}
            }

            self.collect_bound_method_candidates(search_norm, receiver_ty, env, &mut candidates);
            self.collect_impl_method_candidates(search_norm, &mut candidates);
        }

        candidates
    }

    pub fn resolve_named_member(
        &mut self,
        current_module_id: Option<DefId>,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let base_norm = base_type(self.ctx, receiver_ty);

        if let TypeKind::Module(module_def_id) = self.ctx.type_registry.get(base_norm).clone() {
            return self.resolve_module_member(current_module_id, module_def_id, member_name);
        }

        for search_norm in self.search_types(receiver_ty) {
            if let Some(resolution) = self.resolve_named_member_in_type(
                current_module_id,
                search_norm,
                receiver_ty,
                member_name,
                env,
                access_span,
            ) {
                return Some(resolution);
            }
        }

        None
    }

    fn search_types(&mut self, receiver_ty: TypeId) -> Vec<TypeId> {
        let receiver_norm = self.ctx.type_registry.normalize(receiver_ty);
        let base_norm = base_type(self.ctx, receiver_ty);
        let mut search_tys = vec![receiver_norm];

        match self.ctx.type_registry.get(receiver_norm).clone() {
            TypeKind::Pointer { is_mut: true, elem } => {
                search_tys.push(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Pointer { is_mut: false, elem }),
                );
            }
            TypeKind::VolatilePtr { is_mut: true, elem } => {
                search_tys.push(self.ctx.type_registry.intern(TypeKind::VolatilePtr {
                    is_mut: false,
                    elem,
                }));
            }
            TypeKind::Slice { is_mut: true, elem } => {
                search_tys.push(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::Slice { is_mut: false, elem }),
                );
            }
            _ => {}
        }

        if !search_tys.contains(&base_norm) {
            search_tys.push(base_norm);
        }

        search_tys
    }

    fn collect_module_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        module_def_id: DefId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let Def::Module(module_def) = &self.ctx.defs[module_def_id.0 as usize] else {
            return;
        };

        for (name, info) in self.ctx.scopes.symbols_in_scope(module_def.scope_id) {
            if !info.is_pub && current_module_id != Some(module_def_id) {
                continue;
            }

            let type_id = if info.kind == SymbolKind::Function {
                info.def_id
                    .map(|def_id| self.ctx.type_registry.intern(TypeKind::FnDef(def_id, vec![])))
                    .unwrap_or(info.type_id)
            } else if info.kind == SymbolKind::Module {
                info.def_id
                    .map(|def_id| self.ctx.type_registry.intern(TypeKind::Module(def_id)))
                    .unwrap_or(info.type_id)
            } else {
                info.type_id
            };

            push_member_candidate(
                candidates,
                MemberCandidate {
                    name,
                    kind: info.kind,
                    type_id,
                    def_id: info.def_id,
                    is_mut: info.is_mut,
                },
            );
        }
    }

    fn resolve_module_member(
        &mut self,
        current_module_id: Option<DefId>,
        module_def_id: DefId,
        member_name: SymbolId,
    ) -> Option<MemberResolution> {
        let Def::Module(module_def) = &self.ctx.defs[module_def_id.0 as usize] else {
            return None;
        };
        let info = self
            .ctx
            .scopes
            .resolve_in(module_def.scope_id, member_name)
            .cloned()?;
        if !info.is_pub && current_module_id != Some(module_def_id) {
            return None;
        }

        let type_id = if info.kind == SymbolKind::Function {
            info.def_id
                .map(|def_id| self.ctx.type_registry.intern(TypeKind::FnDef(def_id, vec![])))
                .unwrap_or(info.type_id)
        } else if info.kind == SymbolKind::Module {
            info.def_id
                .map(|def_id| self.ctx.type_registry.intern(TypeKind::Module(def_id)))
                .unwrap_or(info.type_id)
        } else {
            info.type_id
        };

        Some(MemberResolution {
            candidate: MemberCandidate {
                name: member_name,
                kind: info.kind,
                type_id,
                def_id: info.def_id,
                is_mut: info.is_mut,
            },
            owner_trait_ty: None,
        })
    }

    fn collect_named_type_field_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[TypeId],
        candidates: &mut Vec<MemberCandidate>,
    ) {
        match self.ctx.defs[def_id.0 as usize].clone() {
            Def::Struct(struct_def) => {
                for field in &struct_def.fields {
                    if !field.is_pub
                        && def_owner_module_id(self.ctx, def_id) != current_module_id
                    {
                        continue;
                    }

                    let ty = self.apply_generics_to_field(
                        &struct_def.generics,
                        generic_args,
                        field.type_node.id,
                    );
                    push_member_candidate(
                        candidates,
                        MemberCandidate {
                            name: field.name,
                            kind: SymbolKind::Var,
                            type_id: ty,
                            def_id: None,
                            is_mut: false,
                        },
                    );
                }
            }
            Def::Union(union_def) => {
                for field in &union_def.fields {
                    if !field.is_pub
                        && def_owner_module_id(self.ctx, def_id) != current_module_id
                    {
                        continue;
                    }

                    let ty = self.apply_generics_to_field(
                        &union_def.generics,
                        generic_args,
                        field.type_node.id,
                    );
                    push_member_candidate(
                        candidates,
                        MemberCandidate {
                            name: field.name,
                            kind: SymbolKind::Var,
                            type_id: ty,
                            def_id: None,
                            is_mut: false,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn resolve_named_type_field(
        &mut self,
        current_module_id: Option<DefId>,
        def_id: DefId,
        generic_args: &[TypeId],
        member_name: SymbolId,
        access_span: Span,
    ) -> Option<MemberCandidate> {
        match self.ctx.defs[def_id.0 as usize].clone() {
            Def::Struct(struct_def) => {
                let field = struct_def.fields.iter().find(|field| field.name == member_name)?;
                if !field.is_pub && def_owner_module_id(self.ctx, def_id) != current_module_id {
                    self.ctx
                        .struct_error(
                            access_span,
                            format!(
                                "field `{}` of type `{}` is private",
                                self.ctx.resolve(member_name),
                                self.ctx.resolve(struct_def.name)
                            ),
                        )
                        .with_hint(
                            "mark the field `pub`, or access it from within the defining module",
                        )
                        .emit();
                    return Some(MemberCandidate {
                        name: member_name,
                        kind: SymbolKind::Var,
                        type_id: TypeId::ERROR,
                        def_id: None,
                        is_mut: false,
                    });
                }

                Some(MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Var,
                    type_id: self.apply_generics_to_field(
                        &struct_def.generics,
                        generic_args,
                        field.type_node.id,
                    ),
                    def_id: None,
                    is_mut: false,
                })
            }
            Def::Union(union_def) => {
                let field = union_def.fields.iter().find(|field| field.name == member_name)?;
                if !field.is_pub && def_owner_module_id(self.ctx, def_id) != current_module_id {
                    self.ctx
                        .struct_error(
                            access_span,
                            format!(
                                "field `{}` of type `{}` is private",
                                self.ctx.resolve(member_name),
                                self.ctx.resolve(union_def.name)
                            ),
                        )
                        .with_hint(
                            "mark the field `pub`, or access it from within the defining module",
                        )
                        .emit();
                    return Some(MemberCandidate {
                        name: member_name,
                        kind: SymbolKind::Var,
                        type_id: TypeId::ERROR,
                        def_id: None,
                        is_mut: false,
                    });
                }

                Some(MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Var,
                    type_id: self.apply_generics_to_field(
                        &union_def.generics,
                        generic_args,
                        field.type_node.id,
                    ),
                    def_id: None,
                    is_mut: false,
                })
            }
            _ => None,
        }
    }

    fn resolve_named_member_in_type(
        &mut self,
        current_module_id: Option<DefId>,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv,
        access_span: Span,
    ) -> Option<MemberResolution> {
        if let TypeKind::TraitObject(trait_def_id, trait_args) =
            self.ctx.type_registry.get(search_norm).clone()
            && let Some(resolution) = self.resolve_trait_object_method_named(
                trait_def_id,
                &trait_args,
                member_name,
                receiver_ty,
                Some(access_span),
            )
        {
            return Some(resolution);
        }

        if let TypeKind::Def(def_id, generic_args) = self.ctx.type_registry.get(search_norm).clone()
            && let Some(candidate) = self.resolve_named_type_field(
                current_module_id,
                def_id,
                &generic_args,
                member_name,
                access_span,
            )
        {
            return Some(MemberResolution {
                candidate,
                owner_trait_ty: None,
            });
        }

        if let TypeKind::AnonymousStruct(_, fields) | TypeKind::AnonymousUnion(_, fields) =
            self.ctx.type_registry.get(search_norm).clone()
            && let Some(field) = fields.iter().find(|field| field.name == member_name)
        {
            return Some(MemberResolution {
                candidate: MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Var,
                    type_id: field.ty,
                    def_id: None,
                    is_mut: false,
                },
                owner_trait_ty: None,
            });
        }

        if let Some(resolution) =
            self.resolve_bound_member(search_norm, receiver_ty, member_name, env, access_span)
        {
            return Some(resolution);
        }

        self.resolve_named_impl_method(search_norm, member_name)
            .map(|candidate| MemberResolution {
                candidate,
                owner_trait_ty: None,
            })
    }

    fn collect_bound_method_candidates(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        env: &MemberQueryEnv,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let bounds = env.active_bounds().to_vec();
        let mut checker = ExprChecker::new(self.ctx, None);

        for (env_target, bound_tys) in bounds {
            let mut map = HashMap::new();
            if !checker.unify(env_target, search_norm, &mut map) {
                continue;
            }

            let instantiated_bounds = {
                let mut subst = Substituter::new(&mut checker.ctx.type_registry, &map);
                bound_tys
                    .into_iter()
                    .map(|bound| subst.substitute(bound))
                    .collect::<Vec<_>>()
            };

            drop(checker);

            for bound_ty in instantiated_bounds {
                let bound_norm = self.ctx.type_registry.normalize(bound_ty);
                if let TypeKind::TraitObject(trait_def_id, trait_args) =
                    self.ctx.type_registry.get(bound_norm).clone()
                {
                    self.collect_trait_object_method_candidates(
                        trait_def_id,
                        &trait_args,
                        receiver_ty,
                        candidates,
                    );
                }
            }

            checker = ExprChecker::new(self.ctx, None);
        }
    }

    fn resolve_bound_member(
        &mut self,
        search_norm: TypeId,
        receiver_ty: TypeId,
        member_name: SymbolId,
        env: &MemberQueryEnv,
        access_span: Span,
    ) -> Option<MemberResolution> {
        let bounds = env.active_bounds().to_vec();
        let mut checker = ExprChecker::new(self.ctx, None);

        for (env_target, bound_tys) in bounds {
            let mut map = HashMap::new();
            if !checker.unify(env_target, search_norm, &mut map) {
                continue;
            }

            let instantiated_bounds = {
                let mut subst = Substituter::new(&mut checker.ctx.type_registry, &map);
                bound_tys
                    .into_iter()
                    .map(|bound| subst.substitute(bound))
                    .collect::<Vec<_>>()
            };

            drop(checker);

            for bound_ty in instantiated_bounds {
                let bound_norm = self.ctx.type_registry.normalize(bound_ty);
                if let TypeKind::TraitObject(trait_def_id, trait_args) =
                    self.ctx.type_registry.get(bound_norm).clone()
                    && let Some(resolution) = self.resolve_trait_object_method_named(
                        trait_def_id,
                        &trait_args,
                        member_name,
                        receiver_ty,
                        Some(access_span),
                    )
                {
                    return Some(resolution);
                }
            }

            checker = ExprChecker::new(self.ctx, None);
        }

        None
    }

    fn collect_impl_method_candidates(
        &mut self,
        receiver_norm: TypeId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let impl_blocks: Vec<_> = self
            .ctx
            .global_impls
            .iter()
            .filter_map(|&id| match &self.ctx.defs[id.0 as usize] {
                Def::Impl(impl_def) => Some(impl_def.clone()),
                _ => None,
            })
            .collect();

        let mut checker = ExprChecker::new(self.ctx, None);
        for impl_def in impl_blocks {
            let impl_target_ty = checker
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut map = HashMap::new();

            if !checker.unify(impl_target_ty, receiver_norm, &mut map) {
                continue;
            }
            if !impl_bounds_satisfied(&mut checker, &impl_def.where_clauses, &map) {
                continue;
            }

            let resolved_impl_args = impl_def
                .generics
                .iter()
                .map(|param| map.get(&param.name).copied().unwrap_or(TypeId::ERROR))
                .collect::<Vec<_>>();

            for method_id in &impl_def.methods {
                let Def::Function(function) = &checker.ctx.defs[method_id.0 as usize] else {
                    continue;
                };
                let type_id = checker
                    .ctx
                    .type_registry
                    .intern(TypeKind::FnDef(*method_id, resolved_impl_args.clone()));
                push_member_candidate(
                    candidates,
                    MemberCandidate {
                        name: function.name,
                        kind: SymbolKind::Function,
                        type_id,
                        def_id: Some(*method_id),
                        is_mut: false,
                    },
                );
            }
        }
    }

    fn resolve_named_impl_method(
        &mut self,
        receiver_norm: TypeId,
        member_name: SymbolId,
    ) -> Option<MemberCandidate> {
        let impl_blocks: Vec<_> = self
            .ctx
            .global_impls
            .iter()
            .filter_map(|&id| match &self.ctx.defs[id.0 as usize] {
                Def::Impl(impl_def) => Some(impl_def.clone()),
                _ => None,
            })
            .collect();

        let mut checker = ExprChecker::new(self.ctx, None);
        for impl_def in impl_blocks {
            let impl_target_ty = checker
                .ctx
                .node_types
                .get(&impl_def.target_type.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let mut map = HashMap::new();

            if !checker.unify(impl_target_ty, receiver_norm, &mut map) {
                continue;
            }
            if !impl_bounds_satisfied(&mut checker, &impl_def.where_clauses, &map) {
                continue;
            }

            let resolved_impl_args = impl_def
                .generics
                .iter()
                .map(|param| map.get(&param.name).copied().unwrap_or(TypeId::ERROR))
                .collect::<Vec<_>>();

            for method_id in &impl_def.methods {
                let Def::Function(function) = &checker.ctx.defs[method_id.0 as usize] else {
                    continue;
                };
                if function.name != member_name {
                    continue;
                }

                return Some(MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Function,
                    type_id: checker
                        .ctx
                        .type_registry
                        .intern(TypeKind::FnDef(*method_id, resolved_impl_args)),
                    def_id: Some(*method_id),
                    is_mut: false,
                });
            }
        }

        None
    }

    fn collect_trait_object_method_candidates(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        receiver_ty: TypeId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let mut visited = HashSet::new();
        self.collect_trait_methods_in_hierarchy(
            trait_def_id,
            trait_args,
            receiver_ty,
            &mut visited,
            candidates,
        );
    }

    fn resolve_trait_object_method_named(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        member_name: SymbolId,
        receiver_ty: TypeId,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        let mut visited = HashSet::new();
        self.resolve_trait_method_in_hierarchy(
            trait_def_id,
            trait_args,
            member_name,
            receiver_ty,
            &mut visited,
            diagnostic_span,
        )
    }

    fn collect_trait_methods_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        receiver_ty: TypeId,
        visited: &mut HashSet<DefId>,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        if !visited.insert(trait_def_id) {
            return;
        }

        let Def::Trait(trait_def) = self.ctx.defs[trait_def_id.0 as usize].clone() else {
            return;
        };
        let trait_arg_map: HashMap<SymbolId, TypeId> = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();

        for (method_name, method_ty) in &trait_def.resolved_methods {
            let mut method_ty = *method_ty;
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params;
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
                });
            }

            if !trait_arg_map.is_empty() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                method_ty = subst.substitute(method_ty);
            }

            push_member_candidate(
                candidates,
                MemberCandidate {
                    name: *method_name,
                    kind: SymbolKind::Function,
                    type_id: method_ty,
                    def_id: None,
                    is_mut: false,
                },
            );
        }

        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_ty);

            if let TypeKind::TraitObject(super_def_id, super_args) =
                self.ctx.type_registry.get(inst_super_norm).clone()
            {
                self.collect_trait_methods_in_hierarchy(
                    super_def_id,
                    &super_args,
                    receiver_ty,
                    visited,
                    candidates,
                );
            }
        }
    }

    fn resolve_trait_method_in_hierarchy(
        &mut self,
        trait_def_id: DefId,
        trait_args: &[TypeId],
        member_name: SymbolId,
        receiver_ty: TypeId,
        visited: &mut HashSet<DefId>,
        diagnostic_span: Option<Span>,
    ) -> Option<MemberResolution> {
        if !visited.insert(trait_def_id) {
            return None;
        }

        let Def::Trait(trait_def) = self.ctx.defs[trait_def_id.0 as usize].clone() else {
            return None;
        };
        let trait_arg_map: HashMap<SymbolId, TypeId> = trait_def
            .generics
            .iter()
            .zip(trait_args.iter())
            .map(|(param, arg)| (param.name, *arg))
            .collect();

        if let Some((_, method_ty)) = trait_def
            .resolved_methods
            .iter()
            .find(|(name, _)| *name == member_name)
        {
            let mut method_ty = *method_ty;
            if let TypeKind::Function {
                params,
                ret,
                is_variadic,
            } = self.ctx.type_registry.get(method_ty).clone()
            {
                let mut new_params = params;
                if !new_params.is_empty() {
                    new_params[0] = receiver_ty;
                }
                method_ty = self.ctx.type_registry.intern(TypeKind::Function {
                    params: new_params,
                    ret,
                    is_variadic,
                });
            }

            if !trait_arg_map.is_empty() {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                method_ty = subst.substitute(method_ty);
            }

            return Some(MemberResolution {
                candidate: MemberCandidate {
                    name: member_name,
                    kind: SymbolKind::Function,
                    type_id: method_ty,
                    def_id: None,
                    is_mut: false,
                },
                owner_trait_ty: Some(
                    self.ctx
                        .type_registry
                        .intern(TypeKind::TraitObject(trait_def_id, trait_args.to_vec())),
                ),
            });
        }

        let mut matches = Vec::new();
        for &super_ty in &trait_def.resolved_supertraits {
            let inst_super_ty = if trait_arg_map.is_empty() {
                super_ty
            } else {
                let mut subst = Substituter::new(&mut self.ctx.type_registry, &trait_arg_map);
                subst.substitute(super_ty)
            };
            let inst_super_norm = self.ctx.type_registry.normalize(inst_super_ty);

            if let TypeKind::TraitObject(super_def_id, super_args) =
                self.ctx.type_registry.get(inst_super_norm).clone()
                && let Some(resolution) = self.resolve_trait_method_in_hierarchy(
                    super_def_id,
                    &super_args,
                    member_name,
                    receiver_ty,
                    visited,
                    diagnostic_span,
                )
            {
                matches.push(resolution);
            }
        }

        if matches.len() > 1 {
            if let Some(span) = diagnostic_span {
                let owners = matches
                    .iter()
                    .filter_map(|resolution| resolution.owner_trait_ty)
                    .map(|owner| self.ctx.ty_to_string(owner))
                    .collect::<Vec<_>>();
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "ambiguous inherited trait method `{}`",
                            self.ctx.resolve(member_name)
                        ),
                    )
                    .with_hint(format!(
                        "the method is inherited from multiple parent traits: {}",
                        owners.join(", ")
                    ))
                    .emit();
            }
            return None;
        }

        matches.into_iter().next()
    }

    fn apply_generics_to_field(
        &mut self,
        generics: &[ast::GenericParam],
        args: &[TypeId],
        node_id: kernc_utils::NodeId,
    ) -> TypeId {
        let mut field_ty = self
            .ctx
            .node_types
            .get(&node_id)
            .copied()
            .unwrap_or(TypeId::ERROR);

        if !generics.is_empty() && !args.is_empty() {
            let mut map = HashMap::new();
            for (index, param) in generics.iter().enumerate() {
                if let Some(arg) = args.get(index).copied() {
                    map.insert(param.name, arg);
                }
            }
            let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
            field_ty = subst.substitute(field_ty);
        }

        field_ty
    }
}

fn base_type(ctx: &SemaContext<'_>, mut ty: TypeId) -> TypeId {
    loop {
        let norm = ctx.type_registry.normalize(ty);
        match ctx.type_registry.get(norm).clone() {
            TypeKind::Pointer { elem, .. } | TypeKind::VolatilePtr { elem, .. } => ty = elem,
            _ => return norm,
        }
    }
}

fn def_owner_module_id(ctx: &SemaContext<'_>, def_id: DefId) -> Option<DefId> {
    ctx.defs.iter().find_map(|def| {
        let Def::Module(module) = def else {
            return None;
        };
        if module.items.contains(&def_id) {
            Some(module.id)
        } else {
            None
        }
    })
}

fn impl_bounds_satisfied(
    checker: &mut ExprChecker<'_, '_>,
    where_clauses: &[ast::WhereClause],
    map: &HashMap<SymbolId, TypeId>,
) -> bool {
    let mut pairs_to_check = Vec::new();

    {
        let mut subst = Substituter::new(&mut checker.ctx.type_registry, map);
        for clause in where_clauses {
            let original_target = checker
                .ctx
                .node_types
                .get(&clause.target_ty.id)
                .copied()
                .unwrap_or(TypeId::ERROR);
            let sub_target = subst.substitute(original_target);

            for bound_ast in &clause.bounds {
                let original_bound = checker
                    .ctx
                    .node_types
                    .get(&bound_ast.id)
                    .copied()
                    .unwrap_or(TypeId::ERROR);
                let sub_bound = subst.substitute(original_bound);
                pairs_to_check.push((sub_target, sub_bound));
            }
        }
    }

    for (sub_target, sub_bound) in pairs_to_check {
        if sub_target != TypeId::ERROR
            && sub_bound != TypeId::ERROR
            && !checker.check_trait_impl(sub_target, sub_bound)
        {
            return false;
        }
    }

    true
}

fn push_member_candidate(candidates: &mut Vec<MemberCandidate>, candidate: MemberCandidate) {
    if let Some(index) = candidates
        .iter()
        .position(|existing| existing.name == candidate.name)
    {
        candidates.remove(index);
    }
    candidates.push(candidate);
}
