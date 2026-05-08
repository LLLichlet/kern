use super::*;

impl<'a, 'ctx> BuiltinInjector<'a, 'ctx> {
    pub(super) fn inject_custom_define_consts(&mut self) {
        let prev_scope = self.ctx.scopes.current_scope_id();
        self.ctx.scopes.set_current_scope(ScopeId(0));

        let defines = self
            .ctx
            .sess
            .custom_defines
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<Vec<_>>();

        for (name, value) in defines {
            if !is_valid_define_identifier(&name) {
                continue;
            }

            let name_id = self.ctx.intern(&name);
            let expr = self.custom_define_expr(&value);
            let def_id = self.ctx.add_def_with(|def_id| {
                Def::Global(GlobalDef {
                    id: def_id,
                    name: name_id,
                    vis: Visibility::Private,
                    parent: None,
                    is_imported: true,
                    value: expr,
                    is_static: false,
                    is_extern: false,
                    is_mut: false,
                    span: Span::default(),
                    docs: None,
                    attributes: Vec::new(),
                })
            });

            let node_id = self.ctx.next_node_id();
            let _ = self.ctx.scopes.define(
                name_id,
                SymbolInfo {
                    kind: SymbolKind::Const,
                    node_id,
                    type_id: TypeId::ERROR,
                    def_id: Some(def_id),
                    span: Span::default(),
                    vis: Visibility::Private,
                    is_mut: false,
                },
            );
        }

        if let Some(prev_scope) = prev_scope {
            self.ctx.scopes.set_current_scope(prev_scope);
        }
    }

    pub(super) fn custom_define_expr(&mut self, value: &str) -> ast::Expr {
        let kind = match value {
            "true" => ast::ExprKind::Bool(true),
            "false" => ast::ExprKind::Bool(false),
            _ => ast::ExprKind::String(value.to_string()),
        };

        ast::Expr {
            id: self.ctx.next_node_id(),
            span: Span::default(),
            kind,
        }
    }
}

fn is_valid_define_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }

    chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
