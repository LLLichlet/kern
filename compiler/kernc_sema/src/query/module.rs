use super::*;

impl<'a, 'ctx> MemberQuery<'a, 'ctx> {
    pub(super) fn collect_module_candidates(
        &mut self,
        current_module_id: Option<DefId>,
        module_def_id: DefId,
        candidates: &mut Vec<MemberCandidate>,
    ) {
        let Def::Module(module_def) = &self.ctx.defs[module_def_id.0 as usize] else {
            return;
        };

        for (name, info) in self.ctx.scopes.symbols_in_scope(module_def.scope_id) {
            if !self
                .ctx
                .visibility_allows_access(info.vis, module_def_id, current_module_id)
            {
                continue;
            }

            let type_id = if info.kind == SymbolKind::Function {
                info.def_id
                    .map(|def_id| {
                        self.ctx
                            .type_registry
                            .intern(TypeKind::FnDef(def_id, vec![]))
                    })
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
                    definition_span: info.span,
                    is_mut: info.is_mut,
                },
            );
        }
    }

    pub(super) fn resolve_module_member(
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
        if !self
            .ctx
            .visibility_allows_access(info.vis, module_def_id, current_module_id)
        {
            return None;
        }

        let type_id = if info.kind == SymbolKind::Function {
            info.def_id
                .map(|def_id| {
                    self.ctx
                        .type_registry
                        .intern(TypeKind::FnDef(def_id, vec![]))
                })
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
                definition_span: info.span,
                is_mut: info.is_mut,
            },
            owner_trait_ty: None,
        })
    }
}
