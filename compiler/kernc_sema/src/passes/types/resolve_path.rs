use super::*;

impl<'a, 'ctx> TypeResolver<'a, 'ctx> {
    fn resolve_type_anchor_scope(
        &mut self,
        anchor: ast::PathAnchor,
        env_scope: ScopeId,
        span: Span,
    ) -> Option<ScopeId> {
        // Anchored type paths (`..Foo`, `package.Foo`) are resolved relative to the
        // module that owns the caller's scope, not relative to lexical blocks nested
        // inside that module.
        let Some(current_module) = self.ctx.module_for_scope(env_scope) else {
            self.ctx.emit_ice(
                span,
                "Kern ICE (Types): could not determine current module for anchored type path",
            );
            return None;
        };

        match anchor {
            ast::PathAnchor::Parent => {
                let Some(parent) = self.ctx.module_parent(current_module) else {
                    self.ctx
                        .struct_error(span, "Cannot use `..` in a root module type path")
                        .emit();
                    return None;
                };
                match &self.ctx.defs[parent.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => {
                        self.ctx.emit_ice(
                            span,
                            "Kern ICE (Types): parent module def is not a module while resolving anchored type path",
                        );
                        None
                    }
                }
            }
            ast::PathAnchor::Package => {
                let root = self.ctx.module_root(current_module);
                match &self.ctx.defs[root.0 as usize] {
                    Def::Module(module) => Some(module.scope_id),
                    _ => {
                        self.ctx.emit_ice(
                            span,
                            "Kern ICE (Types): root module def is not a module while resolving anchored type path",
                        );
                        None
                    }
                }
            }
        }
    }

    pub(super) fn resolve_path_type(
        &mut self,
        anchor: Option<ast::PathAnchor>,
        segments: &[ast::TypePathSegment],
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        if segments.is_empty() {
            return TypeId::ERROR;
        }

        let mut curr_scope = match anchor {
            Some(anchor) => match self.resolve_type_anchor_scope(anchor, env_scope, span) {
                Some(scope) => scope,
                None => return TypeId::ERROR,
            },
            None => env_scope,
        };
        let mut current_ty = None;
        // After a trait segment (`Receiver.Iter[...]`) we need to delay materializing the
        // projection until we see the following associated type segment (`.Item`).
        let mut pending_trait_projection: Option<PendingTraitProjection> = None;

        for (index, segment) in segments.iter().enumerate() {
            if let Some(PendingTraitProjection {
                trait_def_id,
                trait_args,
                assoc_bindings,
            }) = pending_trait_projection.take()
            {
                // Trait qualification consumes the next segment as the associated type name.
                current_ty = Some(self.resolve_projected_associated_type(
                    current_ty.unwrap_or(TypeId::ERROR),
                    trait_def_id,
                    trait_args,
                    assoc_bindings,
                    segment,
                    env_scope,
                ));
                continue;
            }

            if current_ty.is_none() {
                let (target_symbol, skipped_hidden_assoc) = if index == 0 {
                    if segments.len() == 1 {
                        let name_str = self.ctx.resolve(segment.name).to_string();
                        if let Some(prim_id) = self.resolve_builtin_primitive(&name_str) {
                            if !segment.args.is_empty() {
                                self.ctx.emit_error(
                                    span,
                                    "Primitive types do not take generic arguments",
                                );
                            }
                            return prim_id;
                        }
                    }
                    if anchor.is_some() {
                        (
                            self.ctx
                                .scopes
                                .resolve_namespace_in(curr_scope, segment.name)
                                .cloned(),
                            false,
                        )
                    } else {
                        self.resolve_head_type_symbol(curr_scope, segment.name)
                    }
                } else {
                    (
                        self.ctx
                            .scopes
                            .resolve_namespace_in(curr_scope, segment.name)
                            .cloned(),
                        false,
                    )
                };
                let Some(sym) = target_symbol else {
                    let name = self.ctx.resolve(segment.name).to_string();
                    if index == 0 && skipped_hidden_assoc {
                        self.ctx
                            .struct_error(
                                segment.name_span,
                                format!(
                                    "impl-associated type targets must resolve to a concrete type, but `{}` resolves to the impl's associated type placeholder here",
                                    name
                                ),
                            )
                            .with_hint(
                                "inside `type Name = ...;`, bare associated type names are not available as concrete aliases",
                            )
                            .with_hint(
                                "use a distinct concrete type name, a generic parameter, or an explicit projected type outside the impl-associated type definition",
                            )
                            .emit();
                    } else if index == 0 {
                        self.ctx
                            .struct_error(
                                segment.name_span,
                                format!("Cannot find type `{}` in this scope", name),
                            )
                            .with_code(kernc_utils::DiagnosticCode::UnresolvedType)
                            .emit();
                    } else {
                        self.ctx.emit_error(
                            span,
                            format!("Cannot find `{}` in the target module", name),
                        );
                    }
                    return TypeId::ERROR;
                };

                if index < segments.len() - 1 && sym.kind == SymbolKind::Module {
                    if !segment.args.is_empty() {
                        self.ctx.emit_error(
                            segment.name_span,
                            "module path segments cannot take type arguments",
                        );
                        return TypeId::ERROR;
                    }
                    let Some(mod_def_id) =
                        self.required_def_id(&sym, span, "module path segment", segment.name)
                    else {
                        return TypeId::ERROR;
                    };
                    let Some(module_scope) =
                        self.module_scope_from_def(mod_def_id, span, segment.name)
                    else {
                        return TypeId::ERROR;
                    };
                    curr_scope = module_scope;
                    continue;
                }

                current_ty = Some(self.resolve_named_type_symbol(&sym, segment, env_scope, span));
                continue;
            }

            let current = current_ty.unwrap_or(TypeId::ERROR);
            // Once we already have a concrete receiver type, the remaining path syntax is
            // interpreted as trait qualification followed by associated-type projection.
            let trait_symbol = self.lookup_trait_projection_symbol(segment.name, env_scope);
            let Some((trait_def_id, _trait_symbol)) = trait_symbol else {
                self.ctx.emit_error(
                    segment.name_span,
                    format!(
                        "`{}` is not a trait projection on `{}`",
                        self.ctx.resolve(segment.name),
                        self.ctx.ty_to_string(current)
                    ),
                );
                return TypeId::ERROR;
            };

            let (trait_args, assoc_bindings) =
                self.resolve_trait_segment_args(trait_def_id, &segment.args, env_scope, span);
            if !trait_args.is_empty()
                && trait_args.iter().all(|arg| {
                    matches!(
                        arg,
                        GenericArg::Type(TypeId::ERROR) | GenericArg::Const(ConstGeneric::Error)
                    )
                })
            {
                return TypeId::ERROR;
            }

            if index == segments.len() - 1 {
                self.ctx
                    .struct_error(
                        segment.name_span,
                        format!(
                            "trait qualification `{}` must be followed by an associated type name",
                            self.ctx.resolve(segment.name)
                        ),
                    )
                    .emit();
                return TypeId::ERROR;
            }

            pending_trait_projection = Some(PendingTraitProjection {
                trait_def_id,
                trait_args,
                assoc_bindings,
            });
        }

        if pending_trait_projection.is_some() {
            self.ctx
                .emit_error(span, "expected associated type after trait qualification");
            return TypeId::ERROR;
        }

        current_ty.unwrap_or(TypeId::ERROR)
    }

    fn resolve_head_type_symbol(
        &mut self,
        scope_id: ScopeId,
        name: SymbolId,
    ) -> (Option<crate::scope::SymbolInfo>, bool) {
        let mut curr = Some(scope_id);
        let mut skipped_hidden_assoc = false;

        while let Some(scope_id) = curr {
            if let Some(info) = self
                .ctx
                .scopes
                .resolve_namespace_in(scope_id, name)
                .cloned()
            {
                // While resolving `type Assoc = ...` inside an impl, bare references to the
                // impl's own associated type placeholders would create self-referential aliases.
                // Skip them so callers diagnose that case explicitly.
                if self.suppress_unqualified_impl_assoc_types
                    && info.kind == SymbolKind::AssociatedType
                {
                    skipped_hidden_assoc = true;
                } else {
                    return (Some(info), skipped_hidden_assoc);
                }
            }
            curr = self.ctx.scopes.parent_scope(scope_id);
        }

        (None, skipped_hidden_assoc)
    }

    fn resolve_named_type_symbol(
        &mut self,
        final_sym: &crate::scope::SymbolInfo,
        segment: &ast::TypePathSegment,
        env_scope: ScopeId,
        span: Span,
    ) -> TypeId {
        self.record_type_symbol_reference(segment, final_sym);

        let (resolved_generics, resolved_assoc_bindings) = if let Some(def_id) = final_sym.def_id {
            self.resolve_generic_args_for_def(def_id, &segment.args, env_scope, span)
        } else {
            self.resolve_type_args(&segment.args, env_scope)
        };

        match final_sym.kind {
            SymbolKind::Struct | SymbolKind::Union => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "named types do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) = self.required_def_id(final_sym, span, "type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Def(def_id, resolved_generics))
            }
            SymbolKind::Enum => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "enum types do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) = self.required_def_id(final_sym, span, "enum type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }
                self.ctx
                    .type_registry
                    .intern(TypeKind::Enum(def_id, resolved_generics))
            }
            SymbolKind::Trait => {
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "trait object type", segment.name)
                else {
                    return TypeId::ERROR;
                };
                let (trait_args, assoc_bindings) =
                    self.resolve_trait_segment_args(def_id, &segment.args, env_scope, span);
                self.ctx.type_registry.intern(TypeKind::TraitObject(
                    def_id,
                    trait_args,
                    assoc_bindings,
                ))
            }
            SymbolKind::TypeParam => {
                if !segment.args.is_empty() {
                    self.ctx
                        .emit_error(span, "Type parameters cannot take type arguments");
                }
                final_sym.type_id
            }
            SymbolKind::ConstParam => {
                self.ctx
                    .struct_error(
                        span,
                        format!(
                            "`{}` is a const generic parameter, not a type",
                            self.ctx.resolve(segment.name)
                        ),
                    )
                    .with_hint(
                        "const generic parameters can only appear in constant positions such as `[N]T` or `Type[T, N]`",
                    )
                    .emit();
                TypeId::ERROR
            }
            SymbolKind::AssociatedType => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "associated types do not accept nested associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "associated type", segment.name)
                else {
                    return TypeId::ERROR;
                };

                let Some(assoc_def) =
                    self.ctx
                        .defs
                        .get(def_id.0 as usize)
                        .and_then(|def| match def {
                            Def::AssociatedType(assoc) => Some(assoc.clone()),
                            _ => None,
                        })
                else {
                    self.ctx.emit_ice(
                        span,
                        "associated type symbol does not point to an associated type def",
                    );
                    return TypeId::ERROR;
                };

                if assoc_def.generics.len() != resolved_generics.len() {
                    self.ctx.emit_error(
                        span,
                        format!(
                            "associated type `{}` expects {} generic arguments, but {} were provided",
                            self.last_segment_name(std::slice::from_ref(segment)),
                            assoc_def.generics.len(),
                            resolved_generics.len()
                        ),
                    );
                    return TypeId::ERROR;
                }

                if let Some(target) = assoc_def.target.as_ref() {
                    let target_ty = self.ctx.node_type(target.id).unwrap_or(final_sym.type_id);
                    if resolved_generics.is_empty() {
                        return target_ty;
                    }
                    let mut map = std::collections::HashMap::new();
                    for (param, arg) in assoc_def.generics.iter().zip(resolved_generics.iter()) {
                        map.insert(param.name, *arg);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    return subst.substitute(target_ty);
                }

                self.ctx
                    .type_registry
                    .intern(TypeKind::Associated(def_id, resolved_generics))
            }
            SymbolKind::TypeAlias => {
                if !resolved_assoc_bindings.is_empty() {
                    self.ctx.emit_error(
                        segment.name_span,
                        "type aliases do not accept associated type bindings",
                    );
                    return TypeId::ERROR;
                }
                if final_sym.def_id.is_none() {
                    return final_sym.type_id;
                }
                let Some(def_id) =
                    self.required_def_id(final_sym, span, "type alias", segment.name)
                else {
                    return TypeId::ERROR;
                };
                if !self.check_type_generic_bounds(span, def_id, &resolved_generics) {
                    return TypeId::ERROR;
                }

                let target_ty = if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    final_sym
                        .type_id
                        .ne(&TypeId::ERROR)
                        .then_some(final_sym.type_id)
                        .or_else(|| self.ctx.node_type(t_def.target.id))
                        .unwrap_or(TypeId::ERROR)
                } else {
                    TypeId::ERROR
                };

                if target_ty == TypeId::ERROR {
                    let name = self.last_segment_name(std::slice::from_ref(segment));
                    self.ctx
                        .struct_error(span, format!("type alias `{}` could not be resolved", name))
                        .with_hint("this might be caused by an invalid circular alias dependency or use before resolution")
                        .emit();
                    return TypeId::ERROR;
                }

                if resolved_generics.is_empty() {
                    target_ty
                } else if let Def::TypeAlias(t_def) = &self.ctx.defs[def_id.0 as usize] {
                    if t_def.generics.len() != resolved_generics.len() {
                        self.ctx.emit_error(
                            span,
                            format!(
                                "Type alias `{}` expects {} generic arguments, but {} were provided",
                                self.last_segment_name(std::slice::from_ref(segment)),
                                t_def.generics.len(),
                                resolved_generics.len()
                            ),
                        );
                        return TypeId::ERROR;
                    }
                    let mut map = std::collections::HashMap::new();
                    for (i, param) in t_def.generics.iter().enumerate() {
                        map.insert(param.name, resolved_generics[i]);
                    }
                    let mut subst = Substituter::new(&mut self.ctx.type_registry, &map);
                    subst.substitute(target_ty)
                } else {
                    self.ctx.emit_ice(
                        span,
                        format!(
                            "Type alias symbol `{}` resolved to non-alias def {:?}",
                            self.last_segment_name(std::slice::from_ref(segment)),
                            def_id
                        ),
                    );
                    TypeId::ERROR
                }
            }
            _ => {
                let name = self.last_segment_name(std::slice::from_ref(segment));
                self.ctx.emit_error(
                    span,
                    format!(
                        "`{}` is a {}, not a type",
                        name,
                        self.kind_to_string(final_sym.kind)
                    ),
                );
                TypeId::ERROR
            }
        }
    }

    fn record_type_symbol_reference(
        &mut self,
        segment: &ast::TypePathSegment,
        symbol: &crate::scope::SymbolInfo,
    ) {
        if symbol.span.end <= symbol.span.start
            || self
                .ctx
                .sess
                .source_manager
                .get_file(symbol.span.file)
                .is_none()
        {
            return;
        }

        self.ctx
            .record_identifier_reference(segment.name_span, symbol.span);
    }

    fn resolve_type_args(
        &mut self,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        let mut positional = Vec::new();
        let mut assoc_bindings = Vec::new();
        for arg in args {
            match arg {
                ast::GenericArg::Type(ty) => {
                    positional.push(GenericArg::Type(self.resolve_type(ty, env_scope)))
                }
                ast::GenericArg::AssocBinding { name, value, .. } => {
                    assoc_bindings.push((*name, self.resolve_type(value, env_scope)));
                }
                ast::GenericArg::ConstExpr(expr) => {
                    positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                        expr,
                        TypeId::USIZE,
                        env_scope,
                        "const generic argument",
                    )));
                }
            }
        }
        (positional, assoc_bindings)
    }

    pub(super) fn reinterpret_type_arg_as_const_expr(
        &mut self,
        ty_node: &ast::TypeNode,
    ) -> Option<ast::Expr> {
        // The parser keeps generic arguments structurally typed, so `Array[T, N]` reaches
        // type resolution with `N` represented as a path-like type. For const generic
        // parameters we rewrite the simple path back into an expression and let the const
        // evaluator resolve it with the expected integer type.
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return None;
        };
        if segments.is_empty() || segments.iter().any(|segment| !segment.args.is_empty()) {
            return None;
        }

        let first = &segments[0];
        let mut expr = if let Some(anchor) = *anchor {
            ast::Expr {
                id: self.ctx.next_node_id(),
                span: first.name_span,
                kind: ast::ExprKind::AnchoredPath {
                    anchor,
                    name: first.name,
                    name_span: first.name_span,
                },
            }
        } else {
            ast::Expr {
                id: self.ctx.next_node_id(),
                span: first.name_span,
                kind: ast::ExprKind::Identifier(first.name),
            }
        };

        for segment in &segments[1..] {
            expr = ast::Expr {
                id: self.ctx.next_node_id(),
                span: ty_node.span,
                kind: ast::ExprKind::FieldAccess {
                    lhs: Box::new(expr),
                    field: segment.name,
                    field_span: segment.name_span,
                },
            };
        }

        Some(expr)
    }

    pub(super) fn type_arg_is_payloadless_enum_value_ref(
        &mut self,
        ty_node: &ast::TypeNode,
        env_scope: ScopeId,
    ) -> bool {
        // This fast path distinguishes `Foo.Bar` as a payloadless enum value reference when
        // parsing generic arguments for expressions. It intentionally mirrors only the subset
        // of path resolution that can denote enum variants without carrying full type syntax.
        let ast::TypeKind::Path { anchor, segments } = &ty_node.kind else {
            return false;
        };
        if segments.len() < 2 || segments.iter().any(|segment| !segment.args.is_empty()) {
            return false;
        }

        let last_segment = segments.last().unwrap();
        let mut current_scope = match anchor {
            Some(anchor) => {
                let current_scope = self.ctx.scopes.current_scope_id().unwrap_or(env_scope);
                let Some(current_module) = self.ctx.module_for_scope(current_scope) else {
                    return false;
                };
                let target_module = match anchor {
                    ast::PathAnchor::Parent => {
                        let Some(parent) = self.ctx.module_parent(current_module) else {
                            return false;
                        };
                        parent
                    }
                    ast::PathAnchor::Package => self.ctx.module_root(current_module),
                };
                let Some(module_scope) =
                    self.module_scope_from_def(target_module, ty_node.span, last_segment.name)
                else {
                    return false;
                };
                module_scope
            }
            None => env_scope,
        };

        for (index, segment) in segments[..segments.len() - 1].iter().enumerate() {
            let symbol = if index == 0 && anchor.is_none() {
                self.ctx
                    .scopes
                    .resolve_namespace_from(current_scope, segment.name)
            } else {
                self.ctx
                    .scopes
                    .resolve_namespace_in(current_scope, segment.name)
            };
            let Some(symbol) = symbol.cloned() else {
                return false;
            };

            match symbol.kind {
                SymbolKind::Module => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(module_scope) =
                        self.module_scope_from_def(def_id, segment.name_span, segment.name)
                    else {
                        return false;
                    };
                    current_scope = module_scope;
                }
                SymbolKind::Enum if index == segments.len() - 2 => {
                    let Some(def_id) = symbol.def_id else {
                        return false;
                    };
                    let Some(Def::Enum(enum_def)) = self.ctx.defs.get(def_id.0 as usize) else {
                        return false;
                    };
                    return enum_def.variants.iter().any(|variant| {
                        variant.name == last_segment.name && variant.payload_type.is_none()
                    });
                }
                SymbolKind::TypeAlias if index == segments.len() - 2 => {
                    let alias_ty = self.ctx.type_registry.normalize(symbol.type_id);
                    return match self.ctx.type_registry.get(alias_ty) {
                        TypeKind::Enum(def_id, _) => self
                            .ctx
                            .defs
                            .get(def_id.0 as usize)
                            .and_then(|def| match def {
                                Def::Enum(enum_def) => Some(enum_def),
                                _ => None,
                            })
                            .is_some_and(|enum_def| {
                                enum_def.variants.iter().any(|variant| {
                                    variant.name == last_segment.name
                                        && variant.payload_type.is_none()
                                })
                            }),
                        TypeKind::AnonymousEnum(enum_def) => {
                            enum_def.variants.iter().any(|variant| {
                                variant.name == last_segment.name && variant.payload_ty.is_none()
                            })
                        }
                        _ => false,
                    };
                }
                _ => return false,
            }
        }

        false
    }

    pub(crate) fn resolve_generic_args_for_params(
        &mut self,
        params: &[ast::GenericParam],
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        self.resolve_generic_args_for_params_in_scopes(params, args, env_scope, env_scope, span)
    }

    pub(crate) fn resolve_generic_args_for_params_in_scopes(
        &mut self,
        params: &[ast::GenericParam],
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        param_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        // Positional arguments are matched against the declared parameter order while
        // associated type bindings are collected separately and validated later against the
        // trait's associated-type namespace.
        let positional_count = args
            .iter()
            .filter(|arg| !matches!(arg, ast::GenericArg::AssocBinding { .. }))
            .count();
        let mut positional = Vec::with_capacity(positional_count);
        let mut assoc_bindings = Vec::new();
        let mut positional_index = 0usize;

        for arg in args {
            match arg {
                ast::GenericArg::AssocBinding { name, value, .. } => {
                    assoc_bindings.push((*name, self.resolve_type(value, env_scope)));
                }
                ast::GenericArg::Type(ty_node) => {
                    let expected = params.get(positional_index).map(|param| &param.kind);
                    match expected {
                        Some(ast::GenericParamKind::Type) | None => {
                            let resolved_ty = self.resolve_type(ty_node, env_scope);
                            positional.push(GenericArg::Type(resolved_ty));
                        }
                        Some(ast::GenericParamKind::Const { ty }) => {
                            if let Some(expr) = self.reinterpret_type_arg_as_const_expr(ty_node) {
                                let expected_ty = self
                                    .resolve_const_generic_param_type_in_param_scope(
                                        ty,
                                        env_scope,
                                        param_scope,
                                        expr.span,
                                    );
                                positional.push(GenericArg::Const(
                                    self.resolve_const_generic_expr(
                                        &expr,
                                        expected_ty,
                                        env_scope,
                                        "const generic argument",
                                    ),
                                ));
                            } else {
                                self.ctx
                                    .struct_error(
                                        ty_node.span,
                                        "expected a const generic argument here, but found a type",
                                    )
                                    .with_span_label(
                                        span,
                                        "while resolving this generic instantiation",
                                    )
                                    .emit();
                                positional.push(GenericArg::Const(ConstGeneric::Error));
                            }
                        }
                    }
                    positional_index += 1;
                }
                ast::GenericArg::ConstExpr(expr) => {
                    let expected = params.get(positional_index).map(|param| &param.kind);
                    match expected {
                        Some(ast::GenericParamKind::Const { ty }) => {
                            let expected_ty = self.resolve_const_generic_param_type_in_param_scope(
                                ty,
                                env_scope,
                                param_scope,
                                expr.span,
                            );
                            positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                                expr,
                                expected_ty,
                                env_scope,
                                "const generic argument",
                            )));
                        }
                        Some(ast::GenericParamKind::Type) => {
                            self.ctx
                                .struct_error(
                                    expr.span,
                                    "expected a type generic argument here, but found a constant",
                                )
                                .with_hint(
                                    "type parameters must be instantiated with a type, such as `i32` or `Array[u8, 4]`",
                                )
                                .emit();
                            positional.push(GenericArg::Type(TypeId::ERROR));
                        }
                        None => {
                            positional.push(GenericArg::Const(self.resolve_const_generic_expr(
                                expr,
                                TypeId::USIZE,
                                env_scope,
                                "const generic argument",
                            )));
                        }
                    }
                    positional_index += 1;
                }
            }
        }

        (positional, assoc_bindings)
    }

    fn resolve_generic_args_for_def(
        &mut self,
        def_id: DefId,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(SymbolId, TypeId)>) {
        let generics = match &self.ctx.defs[def_id.0 as usize] {
            Def::Function(f) => f.generics.clone(),
            Def::Struct(s) => s.generics.clone(),
            Def::Union(u) => u.generics.clone(),
            Def::Enum(e) => e.generics.clone(),
            Def::Trait(t) => t.generics.clone(),
            Def::TypeAlias(t) => t.generics.clone(),
            Def::AssociatedType(a) => a.generics.clone(),
            _ => Vec::new(),
        };
        self.resolve_generic_args_for_params(&generics, args, env_scope, span)
    }

    fn resolve_trait_segment_args(
        &mut self,
        trait_def_id: DefId,
        args: &[ast::GenericArg],
        env_scope: ScopeId,
        span: Span,
    ) -> (Vec<GenericArg>, Vec<(DefId, TypeId)>) {
        let (resolved_generics, resolved_assoc_bindings) =
            self.resolve_generic_args_for_def(trait_def_id, args, env_scope, span);
        let trait_assoc_ids = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.clone(),
            _ => Vec::new(),
        };
        if !self.check_type_generic_bounds(span, trait_def_id, &resolved_generics) {
            return (vec![GenericArg::Type(TypeId::ERROR)], Vec::new());
        }
        let mut bindings = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (assoc_name, ty) in resolved_assoc_bindings {
            // Normalize textual `Assoc = T` bindings into def-id keyed bindings so later
            // projection resolution is insensitive to declaration order or shadowing.
            let Some(assoc_def_id) = trait_assoc_ids.iter().copied().find(|assoc_id| {
                self.ctx.defs[assoc_id.0 as usize]
                    .name()
                    .is_some_and(|name| name == assoc_name)
            }) else {
                self.ctx.emit_error(
                    span,
                    format!(
                        "trait `{}` does not declare associated type `{}`",
                        self.ctx.defs[trait_def_id.0 as usize]
                            .name()
                            .map(|sym| self.ctx.resolve(sym))
                            .unwrap_or("<trait>"),
                        self.ctx.resolve(assoc_name)
                    ),
                );
                continue;
            };
            if !seen.insert(assoc_def_id) {
                self.ctx.emit_error(
                    span,
                    format!(
                        "duplicate associated type binding `{}`",
                        self.ctx.resolve(assoc_name)
                    ),
                );
                continue;
            }
            bindings.push((assoc_def_id, ty));
        }
        bindings.sort_by_key(|(assoc_id, _)| assoc_id.0);
        (resolved_generics, bindings)
    }

    fn lookup_trait_projection_symbol(
        &mut self,
        name: SymbolId,
        env_scope: ScopeId,
    ) -> Option<(DefId, crate::scope::SymbolInfo)> {
        // Trait qualification after a receiver type still resolves in the caller's lexical
        // scope chain, not inside the receiver's module namespace.
        self.ctx.scopes.set_current_scope(env_scope);
        let symbol = self.ctx.scopes.resolve_type_symbol(name).cloned()?;
        if symbol.kind != SymbolKind::Trait {
            return None;
        }
        let def_id = symbol.def_id?;
        Some((def_id, symbol))
    }

    fn resolve_projected_associated_type(
        &mut self,
        target_ty: TypeId,
        trait_def_id: DefId,
        trait_args: Vec<GenericArg>,
        assoc_bindings: Vec<(DefId, TypeId)>,
        segment: &ast::TypePathSegment,
        env_scope: ScopeId,
    ) -> TypeId {
        let assoc_def_id = match self.ctx.defs.get(trait_def_id.0 as usize) {
            Some(Def::Trait(trait_def)) => trait_def.assoc_types.iter().copied().find(|assoc_id| {
                self.ctx.defs[assoc_id.0 as usize]
                    .name()
                    .is_some_and(|name| name == segment.name)
            }),
            _ => None,
        };
        let Some(assoc_def_id) = assoc_def_id else {
            self.ctx.emit_error(
                segment.name_span,
                format!(
                    "trait `{}` has no associated type `{}`",
                    self.ctx.defs[trait_def_id.0 as usize]
                        .name()
                        .map(|sym| self.ctx.resolve(sym))
                        .unwrap_or("<trait>"),
                    self.ctx.resolve(segment.name)
                ),
            );
            return TypeId::ERROR;
        };

        if let Some((_, ty)) = assoc_bindings
            .iter()
            .find(|(bound_assoc_id, _)| *bound_assoc_id == assoc_def_id)
        {
            // An explicit binding such as `Iterator[Item = i32].Item` is resolved immediately
            // to the bound concrete type instead of materializing a projection node.
            if !segment.args.is_empty() {
                self.ctx.emit_error(
                    segment.name_span,
                    "bound associated type projections cannot take extra generic arguments",
                );
                return TypeId::ERROR;
            }
            return *ty;
        }

        let (assoc_args, nested_assoc_bindings) = self.resolve_generic_args_for_def(
            assoc_def_id,
            &segment.args,
            env_scope,
            segment.name_span,
        );
        if !nested_assoc_bindings.is_empty() {
            self.ctx.emit_error(
                segment.name_span,
                "associated type projections do not accept nested associated bindings",
            );
            return TypeId::ERROR;
        }
        self.ctx.type_registry.intern(TypeKind::Projection {
            target: target_ty,
            trait_def_id,
            trait_args,
            assoc_def_id,
            assoc_args,
        })
    }
}
