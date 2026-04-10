use super::*;

impl CompilerDriver {
    pub(super) fn configure_program_entry(&self, ctx: &mut SemaContext<'_>) -> bool {
        if !ctx.program_entry_enabled() {
            return true;
        }

        self.synthesize_program_main_adapter(ctx);
        Self::report_diagnostics_if_errors(ctx)
    }

    fn synthesize_program_main_adapter(&self, ctx: &mut SemaContext<'_>) {
        let Some(root_module_id) = ctx.root_module else {
            return;
        };
        let Some((root_items, root_scope_id)) =
            ctx.defs
                .get(root_module_id.0 as usize)
                .and_then(|def| match def {
                    Def::Module(module) => Some((module.items.clone(), module.scope_id)),
                    _ => None,
                })
        else {
            return;
        };

        let main_name = ctx.intern("main");
        let main_argv_ty = ctx.main_argv_ptr_ty();

        let entry_main =
            root_items
                .iter()
                .find_map(|item_id| match &ctx.defs[item_id.0 as usize] {
                    Def::Function(function)
                        if function.parent == Some(root_module_id)
                            && function.name == main_name =>
                    {
                        Some(function.clone())
                    }
                    _ => None,
                });

        let Some(entry_main) = entry_main else {
            ctx.struct_error(Span::default(), "program entry mode requires a root `main` function")
                .with_hint("declare either `fn main() i32` or `fn main(argc: i32, argv: **u8) i32` in the root module")
                .emit();
            return;
        };

        let Some(main_arity_uses_args) =
            Self::validate_program_main(ctx, &entry_main, main_argv_ty)
        else {
            return;
        };

        let adapter_id = DefId(ctx.defs.len() as u32);
        let adapter_name = ctx.intern("__kern_main_adapter");
        let argc_name = ctx.intern("argc");
        let argv_name = ctx.intern("argv");
        let span = entry_main.name_span;
        let argc_pattern = ast::BindingPattern {
            name: argc_name,
            name_span: span,
            is_mut: false,
            span,
        };
        let argv_pattern = ast::BindingPattern {
            name: argv_name,
            name_span: span,
            is_mut: false,
            span,
        };
        let argc_type_node = Self::i32_type_node(ctx, span);
        let argv_type_node = Self::main_argv_type_node(ctx, span);
        let ret_type_node = Self::i32_type_node(ctx, span);
        let ptr_u8_ty = ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        ctx.node_types.insert(argc_type_node.id, TypeId::I32);
        Self::record_main_argv_type_nodes(ctx, &argv_type_node, main_argv_ty, ptr_u8_ty);
        ctx.node_types.insert(ret_type_node.id, TypeId::I32);
        let call_args = if main_arity_uses_args {
            vec![
                ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(argc_name),
                },
                ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(argv_name),
                },
            ]
        } else {
            Vec::new()
        };
        let body = ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Call {
                callee: Box::new(ast::Expr {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::ExprKind::Identifier(main_name),
                }),
                args: call_args,
            },
        };
        let adapter_sig = ctx.type_registry.intern(TypeKind::Function {
            params: vec![TypeId::I32, main_argv_ty],
            ret: TypeId::I32,
            is_variadic: false,
        });

        ctx.add_def(Def::Function(FunctionDef {
            id: adapter_id,
            name: adapter_name,
            name_span: span,
            vis: Visibility::Private,
            parent: Some(root_module_id),
            is_imported: false,
            generics: Vec::new(),
            where_clauses: Vec::new(),
            params: vec![
                ast::FuncParam {
                    pattern: argc_pattern,
                    type_node: argc_type_node,
                    span,
                },
                ast::FuncParam {
                    pattern: argv_pattern,
                    type_node: argv_type_node,
                    span,
                },
            ],
            ret_type: ret_type_node,
            body: Some(Box::new(body)),
            is_const: false,
            is_extern: true,
            is_variadic: false,
            is_intrinsic: false,
            span,
            resolved_sig: Some(adapter_sig),
            docs: None,
            attributes: Vec::new(),
        }));
        ctx.register_def_owner(adapter_id, Some(root_module_id), Some(root_scope_id));

        if let Def::Module(module) = &mut ctx.defs[root_module_id.0 as usize] {
            module.items.push(adapter_id);
        }
    }

    fn validate_program_main(
        ctx: &mut SemaContext<'_>,
        main: &FunctionDef,
        main_argv_ty: TypeId,
    ) -> Option<bool> {
        if main.is_extern {
            ctx.struct_error(
                main.name_span,
                "program `main` must not be declared `extern`",
            )
            .with_hint("`main` is a language-level entry function when `runtime_entry != none`")
            .emit();
            return None;
        }

        if main.is_const {
            ctx.emit_error(main.name_span, "program `main` cannot be `const`");
            return None;
        }

        if !main.generics.is_empty() {
            ctx.emit_error(main.name_span, "program `main` cannot be generic");
            return None;
        }

        if main.body.is_none() {
            ctx.emit_error(main.name_span, "program `main` must have a body");
            return None;
        }

        let sig_ty = main.resolved_sig.unwrap_or(TypeId::ERROR);
        if sig_ty == TypeId::ERROR {
            return None;
        }

        let TypeKind::Function {
            params,
            ret,
            is_variadic,
        } = ctx.type_registry.get(sig_ty).clone()
        else {
            return None;
        };

        if is_variadic {
            ctx.emit_error(main.name_span, "program `main` cannot be variadic");
            return None;
        }

        if ret != TypeId::I32 {
            ctx.struct_error(main.ret_type.span, "program `main` must return `i32`")
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
            return None;
        }

        match params.as_slice() {
            [] => Some(false),
            [argc_ty, argv_ty] if *argc_ty == TypeId::I32 && *argv_ty == main_argv_ty => Some(true),
            [_, _] => {
                ctx.struct_error(
                    main.params[0].type_node.span,
                    "program `main` accepts only `(i32, **u8)` when it has parameters",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
                None
            }
            _ => {
                ctx.struct_error(
                    main.name_span,
                    "program `main` accepts either zero parameters or exactly `(i32, **u8)`",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: **u8) i32`")
                .emit();
                None
            }
        }
    }

    fn i32_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                segments: vec![ctx.intern("i32")],
                segment_spans: vec![span],
                generics: Vec::new(),
            },
        }
    }

    fn u8_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                segments: vec![ctx.intern("u8")],
                segment_spans: vec![span],
                generics: Vec::new(),
            },
        }
    }

    fn main_argv_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Pointer {
                is_mut: false,
                elem: Box::new(ast::TypeNode {
                    id: ctx.next_node_id(),
                    span,
                    kind: ast::TypeKind::Pointer {
                        is_mut: false,
                        elem: Box::new(Self::u8_type_node(ctx, span)),
                    },
                }),
            },
        }
    }

    fn record_main_argv_type_nodes(
        ctx: &mut SemaContext<'_>,
        type_node: &ast::TypeNode,
        argv_ty: TypeId,
        ptr_u8_ty: TypeId,
    ) {
        ctx.node_types.insert(type_node.id, argv_ty);

        let ast::TypeKind::Pointer { elem, .. } = &type_node.kind else {
            return;
        };
        ctx.node_types.insert(elem.id, ptr_u8_ty);

        if let ast::TypeKind::Pointer { elem: inner, .. } = &elem.kind {
            ctx.node_types.insert(inner.id, TypeId::U8);
        }
    }
}
