use super::*;
use kernc_sema::scope::{SymbolInfo, SymbolKind};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
struct TestCase {
    def_id: DefId,
    name: String,
    dispatch_name: kernc_utils::SymbolId,
    uses_args: bool,
    name_span: Span,
}

#[derive(Debug, Clone, Copy)]
struct TestAdapterSymbols {
    argc: kernc_utils::SymbolId,
    argv: kernc_utils::SymbolId,
    case_index: kernc_utils::SymbolId,
    user_argc: kernc_utils::SymbolId,
    user_argv: kernc_utils::SymbolId,
}

impl CompilerDriver {
    pub(super) fn configure_program_entry(&self, ctx: &mut SemaContext<'_>) -> bool {
        if !ctx.program_entry_enabled() {
            if ctx.test_mode_enabled() {
                self.emit_test_metadata(ctx, &[]);
            }
            return true;
        }

        if ctx.test_mode_enabled() {
            self.synthesize_test_main_adapter(ctx);
        } else {
            self.synthesize_program_main_adapter(ctx);
        }
        Self::report_diagnostics_if_errors(ctx)
    }

    fn synthesize_test_main_adapter(&self, ctx: &mut SemaContext<'_>) {
        let Some(root_module_id) = ctx.root_module() else {
            return;
        };
        let Some((root_scope_id, span)) =
            ctx.defs
                .get(root_module_id.0 as usize)
                .and_then(|def| match def {
                    Def::Module(module) => Some((module.scope_id, Span::default())),
                    _ => None,
                })
        else {
            return;
        };

        let main_argv_ty = ctx.main_argv_ptr_ty();
        let tests = self.collect_test_cases(ctx, root_scope_id, main_argv_ty);
        self.emit_test_metadata(ctx, &tests);

        if tests.is_empty() {
            ctx.emit_error(span, "test mode requires at least one `#[test]` function");
            return;
        }

        let adapter_name = ctx.intern("__kern_main_adapter");
        let argc_name = ctx.intern("argc");
        let argv_name = ctx.intern("argv");
        let case_index_name = ctx.intern("__kern_test_case_index");
        let user_argc_name = ctx.intern("__kern_test_user_argc");
        let user_argv_name = ctx.intern("__kern_test_user_argv");
        let private_arg_count = 3usize;

        let argc_type_node = Self::i32_type_node(ctx, span);
        let argv_type_node = Self::main_argv_type_node(ctx, span);
        let ret_type_node = Self::i32_type_node(ctx, span);
        let ptr_u8_ty = ctx.type_registry.intern(TypeKind::Pointer {
            is_mut: false,
            elem: TypeId::U8,
        });
        ctx.set_node_type(argc_type_node.id, TypeId::I32);
        Self::record_main_argv_type_nodes(ctx, &argv_type_node, main_argv_ty, ptr_u8_ty);
        ctx.set_node_type(ret_type_node.id, TypeId::I32);

        let body = self.test_adapter_body(
            ctx,
            &tests,
            span,
            TestAdapterSymbols {
                argc: argc_name,
                argv: argv_name,
                case_index: case_index_name,
                user_argc: user_argc_name,
                user_argv: user_argv_name,
            },
            private_arg_count,
        );
        let body_node_id = body.id;
        let adapter_sig = ctx.type_registry.intern(TypeKind::Function {
            params: vec![TypeId::I32, main_argv_ty],
            ret: TypeId::I32,
            is_variadic: false,
        });

        let adapter_id = ctx.add_def_with(|adapter_id| {
            Def::Function(FunctionDef {
                id: adapter_id,
                name: adapter_name,
                name_span: span,
                vis: Visibility::Private,
                parent: Some(root_module_id),
                default_trait_method: None,
                is_imported: false,
                generics: Vec::new(),
                where_clauses: Vec::new(),
                params: vec![
                    ast::FuncParam {
                        pattern: ast::BindingPattern {
                            name: argc_name,
                            name_span: span,
                            is_mut: false,
                            span,
                        },
                        type_node: argc_type_node,
                        span,
                    },
                    ast::FuncParam {
                        pattern: ast::BindingPattern {
                            name: argv_name,
                            name_span: span,
                            is_mut: false,
                            span,
                        },
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
            })
        });
        ctx.register_def_owner(adapter_id, Some(root_module_id), Some(root_scope_id));

        if let Def::Module(module) = &mut ctx.defs[root_module_id.0 as usize] {
            module.items.push(adapter_id);
        }
        ctx.scopes.set_current_scope(root_scope_id);
        let _ = ctx.scopes.define(
            adapter_name,
            SymbolInfo {
                kind: SymbolKind::Function,
                node_id: body_node_id,
                type_id: ctx
                    .type_registry
                    .intern(TypeKind::FnDef(adapter_id, Vec::new())),
                def_id: Some(adapter_id),
                span,
                vis: Visibility::Private,
                is_mut: false,
            },
        );
    }

    fn synthesize_program_main_adapter(&self, ctx: &mut SemaContext<'_>) {
        let Some(root_module_id) = ctx.root_module() else {
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
                .with_hint("declare either `fn main() i32` or `fn main(argc: i32, argv: &&u8) i32` in the root module")
                .emit();
            return;
        };

        let Some(main_arity_uses_args) =
            Self::validate_program_main(ctx, &entry_main, main_argv_ty)
        else {
            return;
        };

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
        ctx.set_node_type(argc_type_node.id, TypeId::I32);
        Self::record_main_argv_type_nodes(ctx, &argv_type_node, main_argv_ty, ptr_u8_ty);
        ctx.set_node_type(ret_type_node.id, TypeId::I32);
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

        let adapter_id = ctx.add_def_with(|adapter_id| {
            Def::Function(FunctionDef {
                id: adapter_id,
                name: adapter_name,
                name_span: span,
                vis: Visibility::Private,
                parent: Some(root_module_id),
                default_trait_method: None,
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
            })
        });
        ctx.register_def_owner(adapter_id, Some(root_module_id), Some(root_scope_id));

        if let Def::Module(module) = &mut ctx.defs[root_module_id.0 as usize] {
            module.items.push(adapter_id);
        }
    }

    fn collect_test_cases(
        &self,
        ctx: &mut SemaContext<'_>,
        root_scope_id: kernc_sema::scope::ScopeId,
        main_argv_ty: TypeId,
    ) -> Vec<TestCase> {
        let mut tests = Vec::new();
        let test_name = ctx.intern("test");
        let def_ids = ctx.defs.ids().collect::<Vec<_>>();

        for def_id in def_ids {
            let Some(Def::Function(function)) = ctx.defs.get(def_id.0 as usize).cloned() else {
                continue;
            };
            if !Self::function_has_marker_attr(ctx, &function, test_name) {
                continue;
            }

            let Some(uses_args) = Self::validate_test_function(ctx, &function, main_argv_ty) else {
                continue;
            };
            tests.push(TestCase {
                def_id,
                name: Self::test_case_name(ctx, &function),
                dispatch_name: ctx.intern("__kern_test_case_pending"),
                uses_args,
                name_span: function.name_span,
            });
        }

        tests.sort_by(|lhs, rhs| {
            lhs.name
                .cmp(&rhs.name)
                .then(lhs.def_id.0.cmp(&rhs.def_id.0))
        });
        self.register_test_dispatch_symbols(ctx, root_scope_id, &mut tests);
        tests
    }

    fn register_test_dispatch_symbols(
        &self,
        ctx: &mut SemaContext<'_>,
        root_scope_id: kernc_sema::scope::ScopeId,
        tests: &mut [TestCase],
    ) {
        let previous_scope = ctx.scopes.current_scope_id();
        ctx.scopes.set_current_scope(root_scope_id);
        for (index, test) in tests.iter_mut().enumerate() {
            let dispatch_name = ctx.intern(&format!("__kern_test_case_{index}"));
            let node_id = ctx.next_node_id();
            let type_id = ctx
                .type_registry
                .intern(TypeKind::FnDef(test.def_id, Vec::new()));
            test.dispatch_name = dispatch_name;
            let _ = ctx.scopes.define(
                dispatch_name,
                SymbolInfo {
                    kind: SymbolKind::Function,
                    node_id,
                    type_id,
                    def_id: Some(test.def_id),
                    span: test.name_span,
                    vis: Visibility::Private,
                    is_mut: false,
                },
            );
        }
        if let Some(previous_scope) = previous_scope {
            ctx.scopes.set_current_scope(previous_scope);
        }
    }

    fn function_has_marker_attr(
        _ctx: &SemaContext<'_>,
        function: &FunctionDef,
        expected: kernc_utils::SymbolId,
    ) -> bool {
        function.attributes.iter().any(|attribute| {
            let ast::AttributeKind::Meta(items) = &attribute.kind else {
                return false;
            };
            items
                .iter()
                .any(|item| matches!(item, ast::MetaItem::Marker(name) if *name == expected))
        })
    }

    fn validate_test_function(
        ctx: &mut SemaContext<'_>,
        function: &FunctionDef,
        main_argv_ty: TypeId,
    ) -> Option<bool> {
        if function.is_extern {
            ctx.struct_error(
                function.name_span,
                "`#[test]` function must not be `extern`",
            )
            .with_hint("test cases are language-level entry functions invoked by the test adapter")
            .emit();
            return None;
        }

        if function.is_const {
            ctx.emit_error(function.name_span, "`#[test]` function cannot be `const`");
            return None;
        }

        if !function.generics.is_empty() {
            ctx.emit_error(function.name_span, "`#[test]` function cannot be generic");
            return None;
        }

        if function.body.is_none() {
            ctx.emit_error(function.name_span, "`#[test]` function must have a body");
            return None;
        }

        let sig_ty = function.resolved_sig.unwrap_or(TypeId::ERROR);
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
            ctx.emit_error(function.name_span, "`#[test]` function cannot be variadic");
            return None;
        }

        if ret != TypeId::I32 {
            ctx.struct_error(
                function.ret_type.span,
                "`#[test]` function must return `i32`",
            )
            .with_hint(
                "legal test forms are `fn name() i32` and `fn name(argc: i32, argv: &&u8) i32`",
            )
            .emit();
            return None;
        }

        match params.as_slice() {
            [] => Some(false),
            [argc_ty, argv_ty] if *argc_ty == TypeId::I32 && *argv_ty == main_argv_ty => Some(true),
            [_, _] => {
                ctx.struct_error(
                    function.params[0].type_node.span,
                    "`#[test]` function accepts only `(i32, &&u8)` when it has parameters",
                )
                .with_hint(
                    "legal test forms are `fn name() i32` and `fn name(argc: i32, argv: &&u8) i32`",
                )
                .emit();
                None
            }
            _ => {
                ctx.struct_error(
                    function.name_span,
                    "`#[test]` function accepts either zero parameters or exactly `(i32, &&u8)`",
                )
                .with_hint(
                    "legal test forms are `fn name() i32` and `fn name(argc: i32, argv: &&u8) i32`",
                )
                .emit();
                None
            }
        }
    }

    fn test_case_name(ctx: &SemaContext<'_>, function: &FunctionDef) -> String {
        let mut components = Vec::new();
        let mut current = function.parent;
        while let Some(parent_id) = current {
            match &ctx.defs[parent_id.0 as usize] {
                Def::Module(module) => {
                    components.push(ctx.resolve(module.name).to_string());
                    current = module.parent;
                }
                Def::Impl(_) => break,
                _ => break,
            }
        }
        components.reverse();
        if !components.is_empty() {
            if let Some(root_module_id) = ctx.root_module()
                && matches!(
                    ctx.defs.get(root_module_id.0 as usize),
                    Some(Def::Module(root)) if components.first().is_some_and(|name| name == ctx.resolve(root.name))
                )
            {
                components.remove(0);
            }
        }
        components.push(ctx.resolve(function.name).to_string());
        components.join("::")
    }

    fn emit_test_metadata(&self, ctx: &mut SemaContext<'_>, tests: &[TestCase]) {
        let Some(output) = self.options.test_metadata_output.as_deref() else {
            return;
        };

        let mut contents = String::from("version=1\n");
        for (index, test) in tests.iter().enumerate() {
            contents.push_str(&format!("case={index}\t{}\n", test.name));
        }

        let path = Path::new(output);
        if let Some(parent) = path.parent()
            && let Err(err) = fs::create_dir_all(parent)
        {
            ctx.struct_error(
                Span::default(),
                format!(
                    "failed to create test metadata directory `{}`",
                    parent.display()
                ),
            )
            .with_hint(err.to_string())
            .emit();
            return;
        }
        if let Err(err) = fs::write(path, contents) {
            ctx.struct_error(
                Span::default(),
                format!("failed to write test metadata `{}`", path.display()),
            )
            .with_hint(err.to_string())
            .emit();
        }
    }

    fn test_adapter_body(
        &self,
        ctx: &mut SemaContext<'_>,
        tests: &[TestCase],
        span: Span,
        symbols: TestAdapterSymbols,
        private_arg_count: usize,
    ) -> ast::Expr {
        let mut stmts = Vec::new();

        let argc_expr = Self::ident(ctx, span, symbols.argc);
        let min_argc = Self::int(ctx, span, private_arg_count as u128);
        let too_few_args = Self::binary(
            ctx,
            span,
            argc_expr,
            ast::BinaryOperator::LessThan,
            min_argc,
        );
        let too_few_args_return = Self::if_return(ctx, span, too_few_args, 101);
        stmts.push(Self::expr_stmt(ctx, span, too_few_args_return));

        let protocol_arg = Self::argv_at(ctx, span, symbols.argv, 1);
        let protocol_matches = Self::cstr_eq_literal(ctx, span, protocol_arg, b"--kern-test-case");
        let protocol_missing =
            Self::unary(ctx, span, ast::UnaryOperator::LogicalNot, protocol_matches);
        let protocol_return = Self::if_return(ctx, span, protocol_missing, 102);
        stmts.push(Self::expr_stmt(ctx, span, protocol_return));

        let case_arg = Self::argv_at(ctx, span, symbols.argv, 2);
        let case_index = Self::parse_case_index(ctx, span, case_arg);
        let case_binding = Self::let_binding(ctx, span, symbols.case_index, case_index, false);
        stmts.push(Self::expr_stmt(ctx, span, case_binding));

        let argc_expr = Self::ident(ctx, span, symbols.argc);
        let private_arg_count_expr = Self::int(ctx, span, private_arg_count as u128);
        let user_argc_value = Self::binary(
            ctx,
            span,
            argc_expr,
            ast::BinaryOperator::Subtract,
            private_arg_count_expr,
        );
        let user_argc_binding =
            Self::let_binding(ctx, span, symbols.user_argc, user_argc_value, false);
        stmts.push(Self::expr_stmt(ctx, span, user_argc_binding));

        let argv_expr = Self::ident(ctx, span, symbols.argv);
        let private_arg_count_expr = Self::usize_int(ctx, span, private_arg_count as u128);
        let user_argv_value = Self::binary(
            ctx,
            span,
            argv_expr,
            ast::BinaryOperator::Add,
            private_arg_count_expr,
        );
        let user_argv_binding =
            Self::let_binding(ctx, span, symbols.user_argv, user_argv_value, false);
        stmts.push(Self::expr_stmt(ctx, span, user_argv_binding));

        for (index, test) in tests.iter().enumerate() {
            let case_index_expr = Self::ident(ctx, test.name_span, symbols.case_index);
            let expected_index = Self::usize_int(ctx, test.name_span, index as u128);
            let cond = Self::binary(
                ctx,
                test.name_span,
                case_index_expr,
                ast::BinaryOperator::Equal,
                expected_index,
            );
            let call = Self::test_call(ctx, test, symbols);
            let ret = Self::return_expr(ctx, test.name_span, Some(call));
            let then_branch = Self::block_with_result(ctx, test.name_span, Vec::new(), ret);
            let case_if = Self::if_expr(ctx, test.name_span, cond, then_branch, None);
            stmts.push(Self::expr_stmt(ctx, test.name_span, case_if));
        }

        let error_code = Self::int(ctx, span, 103);
        let result = Self::return_expr(ctx, span, Some(error_code));
        Self::block_with_result(ctx, span, stmts, result)
    }

    fn cstr_eq_literal(
        ctx: &mut SemaContext<'_>,
        span: Span,
        ptr: ast::Expr,
        expected: &[u8],
    ) -> ast::Expr {
        let mut result = Self::bool_expr(ctx, span, true);
        for (offset, &byte) in expected.iter().enumerate() {
            let index = Self::usize_int(ctx, span, offset as u128);
            let byte_ptr = Self::binary(ctx, span, ptr.clone(), ast::BinaryOperator::Add, index);
            let actual = Self::deref(ctx, span, byte_ptr);
            let expected_byte = Self::byte(ctx, span, byte);
            let matches =
                Self::binary(ctx, span, actual, ast::BinaryOperator::Equal, expected_byte);
            result = Self::binary(ctx, span, result, ast::BinaryOperator::LogicalAnd, matches);
        }
        let index = Self::usize_int(ctx, span, expected.len() as u128);
        let byte_ptr = Self::binary(ctx, span, ptr, ast::BinaryOperator::Add, index);
        let actual = Self::deref(ctx, span, byte_ptr);
        let nul = Self::byte(ctx, span, 0);
        let nul_matches = Self::binary(ctx, span, actual, ast::BinaryOperator::Equal, nul);
        Self::binary(
            ctx,
            span,
            result,
            ast::BinaryOperator::LogicalAnd,
            nul_matches,
        )
    }

    fn parse_case_index(ctx: &mut SemaContext<'_>, span: Span, ptr: ast::Expr) -> ast::Expr {
        let index_name = ctx.intern("__kern_test_parse_i");
        let value_name = ctx.intern("__kern_test_parse_value");
        let byte_name = ctx.intern("__kern_test_parse_byte");

        let mut stmts = Vec::new();
        let zero = Self::usize_int(ctx, span, 0);
        let index_binding = Self::let_binding(ctx, span, index_name, zero, true);
        stmts.push(Self::expr_stmt(ctx, span, index_binding));

        let zero = Self::usize_int(ctx, span, 0);
        let value_binding = Self::let_binding(ctx, span, value_name, zero, true);
        stmts.push(Self::expr_stmt(ctx, span, value_binding));

        let current = Self::ptr_byte_at_index(ctx, span, ptr.clone(), index_name);
        let nul = Self::byte(ctx, span, 0);
        let empty_arg = Self::binary(ctx, span, current, ast::BinaryOperator::Equal, nul);
        let empty_arg_return = Self::if_return(ctx, span, empty_arg, 104);
        stmts.push(Self::expr_stmt(ctx, span, empty_arg_return));

        let loop_body = {
            let mut loop_stmts = Vec::new();
            let current = Self::ptr_byte_at_index(ctx, span, ptr.clone(), index_name);
            let byte_binding = Self::let_binding(ctx, span, byte_name, current, false);
            loop_stmts.push(Self::expr_stmt(ctx, span, byte_binding));

            let byte_expr = Self::ident(ctx, span, byte_name);
            let nul = Self::byte(ctx, span, 0);
            let at_end = Self::binary(ctx, span, byte_expr, ast::BinaryOperator::Equal, nul);
            let break_expr = Self::break_expr(ctx, span);
            let break_block = Self::block_with_result(ctx, span, Vec::new(), break_expr);
            let break_if = Self::if_expr(ctx, span, at_end, break_block, None);
            loop_stmts.push(Self::expr_stmt(ctx, span, break_if));

            let byte_expr = Self::ident(ctx, span, byte_name);
            let zero_byte = Self::byte(ctx, span, b'0');
            let below_zero = Self::binary(
                ctx,
                span,
                byte_expr,
                ast::BinaryOperator::LessThan,
                zero_byte,
            );
            let byte_expr = Self::ident(ctx, span, byte_name);
            let nine_byte = Self::byte(ctx, span, b'9');
            let above_nine = Self::binary(
                ctx,
                span,
                byte_expr,
                ast::BinaryOperator::GreaterThan,
                nine_byte,
            );
            let invalid_digit = Self::binary(
                ctx,
                span,
                below_zero,
                ast::BinaryOperator::LogicalOr,
                above_nine,
            );
            let invalid_digit_return = Self::if_return(ctx, span, invalid_digit, 105);
            loop_stmts.push(Self::expr_stmt(ctx, span, invalid_digit_return));

            let current_value = Self::ident(ctx, span, value_name);
            let ten = Self::usize_int(ctx, span, 10);
            let scaled_value =
                Self::binary(ctx, span, current_value, ast::BinaryOperator::Multiply, ten);
            let byte_expr = Self::ident(ctx, span, byte_name);
            let zero_byte = Self::byte(ctx, span, b'0');
            let digit_u8 = Self::binary(
                ctx,
                span,
                byte_expr,
                ast::BinaryOperator::Subtract,
                zero_byte,
            );
            let usize_ty = Self::usize_type_node(ctx, span);
            let digit = Self::as_expr(ctx, span, digit_u8, usize_ty);
            let next_value = Self::binary(ctx, span, scaled_value, ast::BinaryOperator::Add, digit);
            let lhs = Self::ident(ctx, span, value_name);
            let update_value =
                Self::assign(ctx, span, lhs, ast::AssignmentOperator::Assign, next_value);
            loop_stmts.push(Self::expr_stmt(ctx, span, update_value));

            let lhs = Self::ident(ctx, span, index_name);
            let one = Self::usize_int(ctx, span, 1);
            let bump_index = Self::assign(ctx, span, lhs, ast::AssignmentOperator::AddAssign, one);
            loop_stmts.push(Self::expr_stmt(ctx, span, bump_index));

            Self::block(ctx, span, loop_stmts, None)
        };
        let cond = Self::bool_expr(ctx, span, true);
        let loop_expr = ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::While {
                cond: Box::new(cond),
                body: Box::new(loop_body),
            },
        };
        stmts.push(Self::expr_stmt(ctx, span, loop_expr));

        let result = Self::ident(ctx, span, value_name);
        Self::block_with_result(ctx, span, stmts, result)
    }

    fn ptr_byte_at_index(
        ctx: &mut SemaContext<'_>,
        span: Span,
        ptr: ast::Expr,
        index_name: kernc_utils::SymbolId,
    ) -> ast::Expr {
        let index = Self::ident(ctx, span, index_name);
        let offset = Self::binary(ctx, span, ptr, ast::BinaryOperator::Add, index);
        Self::deref(ctx, span, offset)
    }

    fn test_call(
        ctx: &mut SemaContext<'_>,
        test: &TestCase,
        symbols: TestAdapterSymbols,
    ) -> ast::Expr {
        let args = if test.uses_args {
            vec![
                Self::ident(ctx, test.name_span, symbols.user_argc),
                Self::ident(ctx, test.name_span, symbols.user_argv),
            ]
        } else {
            Vec::new()
        };
        let callee = Self::ident(ctx, test.name_span, test.dispatch_name);
        ast::Expr {
            id: ctx.next_node_id(),
            span: test.name_span,
            kind: ast::ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
        }
    }

    fn expr_stmt(ctx: &mut SemaContext<'_>, span: Span, expr: ast::Expr) -> ast::Stmt {
        ast::Stmt {
            id: ctx.next_node_id(),
            span,
            attributes: Vec::new(),
            kind: ast::StmtKind::ExprStmt(expr),
        }
    }

    fn ident(ctx: &mut SemaContext<'_>, span: Span, name: kernc_utils::SymbolId) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Identifier(name),
        }
    }

    fn int(ctx: &mut SemaContext<'_>, span: Span, value: u128) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Integer {
                value,
                suffix: None,
            },
        }
    }

    fn usize_int(ctx: &mut SemaContext<'_>, span: Span, value: u128) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Integer {
                value,
                suffix: Some(ast::NumericLiteralSuffix::USize),
            },
        }
    }

    fn byte(ctx: &mut SemaContext<'_>, span: Span, value: u8) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::ByteChar(value),
        }
    }

    fn bool_expr(ctx: &mut SemaContext<'_>, span: Span, value: bool) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Bool(value),
        }
    }

    fn binary(
        ctx: &mut SemaContext<'_>,
        span: Span,
        lhs: ast::Expr,
        op: ast::BinaryOperator,
        rhs: ast::Expr,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Binary {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        }
    }

    fn unary(
        ctx: &mut SemaContext<'_>,
        span: Span,
        op: ast::UnaryOperator,
        operand: ast::Expr,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
        }
    }

    fn deref(ctx: &mut SemaContext<'_>, span: Span, operand: ast::Expr) -> ast::Expr {
        Self::unary(ctx, span, ast::UnaryOperator::PointerDeRef, operand)
    }

    fn assign(
        ctx: &mut SemaContext<'_>,
        span: Span,
        lhs: ast::Expr,
        op: ast::AssignmentOperator,
        rhs: ast::Expr,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Assign {
                lhs: Box::new(lhs),
                op,
                rhs: Box::new(rhs),
            },
        }
    }

    fn as_expr(
        ctx: &mut SemaContext<'_>,
        span: Span,
        lhs: ast::Expr,
        target: ast::TypeNode,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::As {
                lhs: Box::new(lhs),
                target: Box::new(target),
            },
        }
    }

    fn argv_at(
        ctx: &mut SemaContext<'_>,
        span: Span,
        argv_name: kernc_utils::SymbolId,
        index: u128,
    ) -> ast::Expr {
        let argv = Self::ident(ctx, span, argv_name);
        let index = Self::usize_int(ctx, span, index);
        let ptr = Self::binary(ctx, span, argv, ast::BinaryOperator::Add, index);
        Self::deref(ctx, span, ptr)
    }

    fn let_binding(
        ctx: &mut SemaContext<'_>,
        span: Span,
        name: kernc_utils::SymbolId,
        init: ast::Expr,
        is_mut: bool,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Let {
                pattern: ast::LetPattern {
                    pattern: ast::Pattern {
                        kind: ast::PatternKind::Binding(ast::BindingPattern {
                            name,
                            name_span: span,
                            is_mut,
                            span,
                        }),
                        span,
                    },
                    span,
                },
                type_node: None,
                init: Box::new(init),
                else_clause: None,
            },
        }
    }

    fn if_return(ctx: &mut SemaContext<'_>, span: Span, cond: ast::Expr, code: u128) -> ast::Expr {
        let code = Self::int(ctx, span, code);
        let ret = Self::return_expr(ctx, span, Some(code));
        let then_branch = Self::block_with_result(ctx, span, Vec::new(), ret);
        Self::if_expr(ctx, span, cond, then_branch, None)
    }

    fn if_expr(
        ctx: &mut SemaContext<'_>,
        span: Span,
        cond: ast::Expr,
        then_branch: ast::Expr,
        else_branch: Option<ast::Expr>,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: else_branch.map(Box::new),
            },
        }
    }

    fn return_expr(ctx: &mut SemaContext<'_>, span: Span, value: Option<ast::Expr>) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Return(value.map(Box::new)),
        }
    }

    fn break_expr(ctx: &mut SemaContext<'_>, span: Span) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Break,
        }
    }

    fn block_with_result(
        ctx: &mut SemaContext<'_>,
        span: Span,
        stmts: Vec<ast::Stmt>,
        result: ast::Expr,
    ) -> ast::Expr {
        Self::block(ctx, span, stmts, Some(result))
    }

    fn block(
        ctx: &mut SemaContext<'_>,
        span: Span,
        stmts: Vec<ast::Stmt>,
        result: Option<ast::Expr>,
    ) -> ast::Expr {
        ast::Expr {
            id: ctx.next_node_id(),
            span,
            kind: ast::ExprKind::Block {
                stmts,
                result: result.map(Box::new),
            },
        }
    }

    fn path_type_node(ctx: &mut SemaContext<'_>, span: Span, name: &str) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                anchor: None,
                segments: vec![ast::TypePathSegment {
                    name: ctx.intern(name),
                    name_span: span,
                    args: Vec::new(),
                }],
            },
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
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: &&u8) i32`")
                .emit();
            return None;
        }

        match params.as_slice() {
            [] => Some(false),
            [argc_ty, argv_ty] if *argc_ty == TypeId::I32 && *argv_ty == main_argv_ty => Some(true),
            [_, _] => {
                ctx.struct_error(
                    main.params[0].type_node.span,
                    "program `main` accepts only `(i32, &&u8)` when it has parameters",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: &&u8) i32`")
                .emit();
                None
            }
            _ => {
                ctx.struct_error(
                    main.name_span,
                    "program `main` accepts either zero parameters or exactly `(i32, &&u8)`",
                )
                .with_hint("legal entry forms are `fn main() i32` and `fn main(argc: i32, argv: &&u8) i32`")
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
                anchor: None,
                segments: vec![ast::TypePathSegment {
                    name: ctx.intern("i32"),
                    name_span: span,
                    args: Vec::new(),
                }],
            },
        }
    }

    fn u8_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        ast::TypeNode {
            id: ctx.next_node_id(),
            span,
            kind: ast::TypeKind::Path {
                anchor: None,
                segments: vec![ast::TypePathSegment {
                    name: ctx.intern("u8"),
                    name_span: span,
                    args: Vec::new(),
                }],
            },
        }
    }

    fn usize_type_node(ctx: &mut SemaContext<'_>, span: Span) -> ast::TypeNode {
        let node = Self::path_type_node(ctx, span, "usize");
        ctx.set_node_type(node.id, TypeId::USIZE);
        node
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
        ctx.set_node_type(type_node.id, argv_ty);

        let ast::TypeKind::Pointer { elem, .. } = &type_node.kind else {
            return;
        };
        ctx.set_node_type(elem.id, ptr_u8_ty);

        if let ast::TypeKind::Pointer { elem: inner, .. } = &elem.kind {
            ctx.set_node_type(inner.id, TypeId::U8);
        }
    }
}
