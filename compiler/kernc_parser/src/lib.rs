mod parser;
mod stream;

pub use parser::{ParseError, ParseResult, Parser};
pub use stream::TokenStream;

#[cfg(test)]
mod tests {
    use super::Parser;
    use kernc_ast as ast;
    use kernc_utils::Session;

    fn parse_module(source: &str) -> (Session, ast::Module) {
        let mut session = Session::new();
        let file_id = session
            .source_manager
            .add_file("parser_test.rn".to_string(), source.to_string());

        let mut parser = Parser::new(source, file_id, &mut session);
        let module = parser.parse_module().unwrap();
        (session, module)
    }

    #[test]
    fn return_expression_span_covers_return_value() {
        let source = "fn main() i32 { return 1 + 2; }";
        let (_session, module) = parse_module(source);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block {
            result: None,
            stmts,
        } = &body.kind
        else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(expr) = &stmts[0].kind else {
            panic!("expected return statement");
        };
        let ast::ExprKind::Return(Some(value)) = &expr.kind else {
            panic!("expected return expression");
        };

        assert_eq!(&source[expr.span.start..expr.span.end], "return 1 + 2");
        assert_eq!(&source[value.span.start..value.span.end], "1 + 2");
    }

    #[test]
    fn parses_zig_style_multiline_string_literals() {
        let source = r#"
fn main() void {
    let msg =
        \\line one
        \\line "two"
        \\line three
    ;
}
"#;
        let (_session, module) = parse_module(source);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &body.kind
        else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(expr) = &stmts[0].kind else {
            panic!("expected let statement");
        };
        let ast::ExprKind::Let { init, .. } = &expr.kind else {
            panic!("expected let expression");
        };
        let ast::ExprKind::String(value) = &init.kind else {
            panic!("expected parsed string literal");
        };

        assert_eq!(value, "line one\nline \"two\"\nline three");
    }

    #[test]
    fn parses_native_doc_comments_on_modules_items_and_members() {
        let source = r#"
//! UART support.
//!
//! Design:
//! keep the hardware boundary explicit.

/// A typed UART handle.
type Uart = struct {
    /// Base MMIO register address.
    ///
    /// Safety:
    /// - Must point to a mapped UART register block.
    base: ^mut u8,
};
"#;
        let (_session, module) = parse_module(source);
        let module_docs = module.docs.as_ref().expect("expected module docs");
        assert_eq!(module_docs.lines.len(), 4);
        assert_eq!(module_docs.lines[0].text, "UART support.");

        let decl = &module.decls[0];
        let decl_docs = decl.docs.as_ref().expect("expected item docs");
        assert_eq!(decl_docs.lines[0].text, "A typed UART handle.");

        let ast::DeclKind::TypeAlias { target, .. } = &decl.kind else {
            panic!("expected type alias");
        };
        let ast::TypeKind::Struct { fields, .. } = &target.kind else {
            panic!("expected struct type");
        };
        let field_docs = fields[0].docs.as_ref().expect("expected field docs");
        assert_eq!(field_docs.lines[0].text, "Base MMIO register address.");
        assert_eq!(field_docs.lines[2].text, "Safety:");
    }

    #[test]
    fn parses_interleaved_doc_comments_for_modules_impls_and_extern_members() {
        let source = r#"
#![if(true)]
//! Runtime entrypoints.
#![if(true)]
//!
//! Design:
//! keep the call boundary explicit.

#[if(true)]
/// A typed counter.
#[if(true)]
type Counter = struct {
    value: i32,
};

impl Counter {
    #[if(true)]
    /// Read the current value.
    /// Returns:
    /// - a stable snapshot of the counter.
    fn get() i32 { return self.value; }
}

extern {
    #[if(true)]
    /// Yield control to the scheduler.
    fn yield_now() void;
}
"#;
        let (_session, module) = parse_module(source);
        let module_docs = module.docs.as_ref().expect("expected module docs");
        assert_eq!(module_docs.lines[0].text, "Runtime entrypoints.");
        assert_eq!(
            module_docs.lines[3].text,
            "keep the call boundary explicit."
        );

        let counter_docs = module.decls[0].docs.as_ref().expect("expected type docs");
        assert_eq!(counter_docs.lines[0].text, "A typed counter.");

        let ast::DeclKind::Impl { decls, .. } = &module.decls[1].kind else {
            panic!("expected impl block");
        };
        let method_docs = decls[0].docs.as_ref().expect("expected method docs");
        assert_eq!(method_docs.lines[0].text, "Read the current value.");
        assert_eq!(method_docs.lines[1].text, "Returns:");

        let ast::DeclKind::ExternBlock { decls, .. } = &module.decls[2].kind else {
            panic!("expected extern block");
        };
        let extern_docs = decls[0].docs.as_ref().expect("expected extern docs");
        assert_eq!(extern_docs.lines[0].text, "Yield control to the scheduler.");
    }

    #[test]
    fn inner_doc_comments_inside_impls_produce_targeted_hints() {
        let source = r#"
type Counter = struct { value: i32 };

impl Counter {
    //! wrong level
    fn get() i32 { return self.value; }
}
"#;
        let (session, module) = parse_module(source);

        assert_eq!(module.decls.len(), 2);
        assert_eq!(session.diagnostics.len(), 1);
        assert!(
            session.diagnostics[0]
                .message
                .contains("inner doc comments (`//!`) are only allowed at module scope")
        );
        assert!(
            session.diagnostics[0]
                .hints
                .iter()
                .any(|hint| { hint.contains("use `///` to document this impl item") })
        );
    }

    #[test]
    fn parses_trailing_commas_in_common_lists() {
        let source = r#"
use base.coll.{List,};

type Pair[T,] = struct {
    left: T,
    right: T,
};

type Choice = enum {
    A,
    B: i32,
};

type Ops = trait {
    run: fn(i32, i32,) i32,
};

#[cold,]
fn add(a: i32, b: i32,) i32 {
    return a + b;
}

fn main() i32 {
    let pair = Pair[i32,].{ left: 1, right: 2, };
    let values = [2]i32.{ pair.left, pair.right, };
    match (values.[0]) {
        1, => return add(values.[0], values.[1],),
        _ => return 0,
    }
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
        assert_eq!(module.decls.len(), 6);

        let ast::DeclKind::Use { target, .. } = &module.decls[0].kind else {
            panic!("expected use declaration");
        };
        let ast::UseTarget::Members(members) = target else {
            panic!("expected grouped use members");
        };
        assert_eq!(members.len(), 1);

        let ast::DeclKind::TypeAlias {
            generics, target, ..
        } = &module.decls[1].kind
        else {
            panic!("expected generic type alias");
        };
        assert_eq!(generics.len(), 1);
        let ast::TypeKind::Struct { fields, .. } = &target.kind else {
            panic!("expected struct type");
        };
        assert_eq!(fields.len(), 2);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[2].kind else {
            panic!("expected enum type alias");
        };
        let ast::TypeKind::Enum { variants, .. } = &target.kind else {
            panic!("expected enum type");
        };
        assert_eq!(variants.len(), 2);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[3].kind else {
            panic!("expected trait type alias");
        };
        let ast::TypeKind::Trait {
            assoc_types,
            methods,
        } = &target.kind
        else {
            panic!("expected trait type");
        };
        assert!(assoc_types.is_empty());
        assert_eq!(methods.len(), 1);
        let ast::TypeKind::Function { params, .. } = &methods[0].type_node.kind else {
            panic!("expected trait method signature");
        };
        assert_eq!(params.len(), 3);

        let add_decl = &module.decls[4];
        assert_eq!(add_decl.attributes.len(), 1);
        let ast::DeclKind::Function { params, .. } = &add_decl.kind else {
            panic!("expected function declaration");
        };
        assert_eq!(params.len(), 2);

        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[5].kind
        else {
            panic!("expected main body");
        };
        let ast::ExprKind::Block {
            stmts,
            result: Some(result),
        } = &body.kind
        else {
            panic!("expected block body with trailing match result");
        };
        assert_eq!(stmts.len(), 2);

        let ast::StmtKind::ExprStmt(first_stmt) = &stmts[0].kind else {
            panic!("expected let statement");
        };
        let ast::ExprKind::Let { init, .. } = &first_stmt.kind else {
            panic!("expected first let");
        };
        let ast::ExprKind::DataInit {
            type_node: Some(type_node),
            literal: ast::DataLiteralKind::Struct(fields),
        } = &init.kind
        else {
            panic!("expected generic struct data init");
        };
        let ast::TypeKind::Path { segments } = &type_node.kind else {
            panic!("expected instantiated path type");
        };
        assert_eq!(segments.last().map(|segment| segment.args.len()), Some(1));
        assert_eq!(fields.len(), 2);

        let ast::StmtKind::ExprStmt(second_stmt) = &stmts[1].kind else {
            panic!("expected second let statement");
        };
        let ast::ExprKind::Let { init, .. } = &second_stmt.kind else {
            panic!("expected second let");
        };
        let ast::ExprKind::DataInit {
            literal: ast::DataLiteralKind::Array(elems),
            ..
        } = &init.kind
        else {
            panic!("expected array data init");
        };
        assert_eq!(elems.len(), 2);

        let ast::ExprKind::Match { arms, .. } = &result.kind else {
            panic!("expected match expression");
        };
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0].patterns.len(), 1);

        let ast::ExprKind::Return(Some(call_expr)) = &arms[0].body.kind else {
            panic!("expected return call");
        };
        let ast::ExprKind::Call { args, .. } = &call_expr.kind else {
            panic!("expected call expression");
        };
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn parses_let_else_pattern_unpack_clause() {
        let source = r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn unwrap_or(err: Result[i32, i32]) i32 {
    let .{ Ok: value } = err else .{ Err: code } => return code;
    return value;
}
"#;

        let (_session, module) = parse_module(source);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[1].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &body.kind
        else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(expr) = &stmts[0].kind else {
            panic!("expected let statement");
        };
        let ast::ExprKind::Let {
            else_pattern,
            else_branch,
            ..
        } = &expr.kind
        else {
            panic!("expected let expression");
        };

        let Some(else_pattern) = else_pattern.as_ref() else {
            panic!("expected explicit else pattern");
        };
        let ast::PatternKind::Destructure(destructure) = &else_pattern.kind else {
            panic!("expected destructure else pattern");
        };
        assert_eq!(destructure.fields.len(), 1);

        let Some(else_branch) = else_branch.as_ref() else {
            panic!("expected else branch");
        };
        let ast::ExprKind::Return(Some(returned)) = &else_branch.kind else {
            panic!("expected return else branch");
        };
        let ast::ExprKind::Identifier(_) = returned.kind else {
            panic!("expected identifier return payload");
        };
    }

    #[test]
    fn parses_nested_let_else_inside_explicit_else_pattern_block() {
        let source = r#"
type Result[T, E] = enum {
    Ok: T,
    Err: E,
};

fn pick(value: Result[Result[i32, i32], i32]) i32 {
    let .{ Ok: inner } = value else .{ Err: outer_err } => {
        let .{ Ok: fallback } = Result[i32, i32].{ Ok: 9 } else .{ Err: inner_err } => return inner_err;
        return fallback;
    };
    return 0;
}
"#;

        let (_session, module) = parse_module(source);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[1].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &body.kind
        else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(expr) = &stmts[0].kind else {
            panic!("expected outer let statement");
        };
        let ast::ExprKind::Let {
            else_pattern,
            else_branch,
            ..
        } = &expr.kind
        else {
            panic!("expected outer let expression");
        };

        assert!(
            else_pattern.is_some(),
            "expected explicit outer else pattern"
        );
        let Some(else_branch) = else_branch.as_ref() else {
            panic!("expected outer else branch");
        };
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &else_branch.kind
        else {
            panic!("expected block else branch");
        };
        let ast::StmtKind::ExprStmt(nested) = &stmts[0].kind else {
            panic!("expected nested let statement");
        };
        let ast::ExprKind::Let {
            else_pattern,
            else_branch,
            ..
        } = &nested.kind
        else {
            panic!("expected nested let expression");
        };

        assert!(
            else_pattern.is_some(),
            "expected explicit nested else pattern"
        );
        let Some(else_branch) = else_branch.as_ref() else {
            panic!("expected nested else branch");
        };
        let ast::ExprKind::Return(Some(_)) = &else_branch.kind else {
            panic!("expected nested return branch");
        };
    }

    #[test]
    fn parses_builtin_optional_and_result_type_forms() {
        let source = r#"
fn main(value: ?i32, status: i32![]u8) ?i32![]u8 {
    let a = ?i32.None;
    let b = ?i32.{ Some: 7 };
    let c = value.?;
    let d = status.!;
    return ?i32![]u8.{ Some: i32![]u8.{ Ok: c + d } };
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );

        let ast::DeclKind::Function {
            params,
            ret_type,
            body: Some(body),
            ..
        } = &module.decls[0].kind
        else {
            panic!("expected function");
        };

        match &params[0].type_node.kind {
            ast::TypeKind::Optional { inner } => {
                assert!(matches!(inner.kind, ast::TypeKind::Path { .. }));
            }
            _ => panic!("expected builtin optional parameter"),
        }

        match &params[1].type_node.kind {
            ast::TypeKind::Result { ok, err } => {
                assert!(matches!(ok.kind, ast::TypeKind::Path { .. }));
                assert!(matches!(err.kind, ast::TypeKind::Slice { .. }));
            }
            _ => panic!("expected builtin result parameter"),
        }

        match &ret_type.kind {
            ast::TypeKind::Result { ok, err } => {
                assert!(matches!(ok.kind, ast::TypeKind::Optional { .. }));
                assert!(matches!(err.kind, ast::TypeKind::Slice { .. }));
            }
            _ => panic!("expected result with optional ok return type"),
        }

        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(first_stmt) = &stmts[0].kind else {
            panic!("expected let statement");
        };
        let ast::ExprKind::Let { init, .. } = &first_stmt.kind else {
            panic!("expected let binding");
        };
        let ast::ExprKind::FieldAccess { lhs, field, .. } = &init.kind else {
            panic!("expected builtin optional field access");
        };
        assert_eq!(session.resolve(*field), "None");
        let ast::ExprKind::TypeNode(type_node) = &lhs.kind else {
            panic!("expected type namespace lhs");
        };
        assert!(matches!(type_node.kind, ast::TypeKind::Optional { .. }));
    }

    #[test]
    fn parses_builtin_propagation_expressions() {
        let source = r#"
fn main(value: ?i32, status: i32![]u8) i32 {
    return value.? + status.!;
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );

        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &body.kind
        else {
            panic!("expected block body");
        };
        let ast::StmtKind::ExprStmt(expr) = &stmts[0].kind else {
            panic!("expected return statement");
        };
        let ast::ExprKind::Return(Some(value)) = &expr.kind else {
            panic!("expected return expression");
        };
        let ast::ExprKind::Binary { lhs, rhs, .. } = &value.kind else {
            panic!("expected propagated sum");
        };
        assert!(matches!(
            lhs.kind,
            ast::ExprKind::Propagate {
                kind: ast::PropagateKind::Option,
                ..
            }
        ));
        assert!(matches!(
            rhs.kind,
            ast::ExprKind::Propagate {
                kind: ast::PropagateKind::Result,
                ..
            }
        ));
    }

    #[test]
    fn parses_associated_types_in_traits_and_impls() {
        let source = r#"
type Add[Rhs] = trait {
    type Out;
    add: fn(Rhs) Out,
};

impl Vec2: Add[i32] {
    type Out = Vec2;
    fn add(rhs: i32) Out { return self; }
}
"#;

        let (_session, module) = parse_module(source);
        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[0].kind else {
            panic!("expected trait type alias");
        };
        let ast::TypeKind::Trait {
            assoc_types,
            methods,
        } = &target.kind
        else {
            panic!("expected trait type");
        };
        assert_eq!(assoc_types.len(), 1);
        assert_eq!(methods.len(), 1);
        assert_eq!(assoc_types[0].generics.len(), 0);

        let ast::DeclKind::Impl { decls, .. } = &module.decls[1].kind else {
            panic!("expected impl block");
        };
        assert_eq!(decls.len(), 2);
        assert!(matches!(decls[0].kind, ast::DeclKind::TypeAlias { .. }));
        assert!(matches!(decls[1].kind, ast::DeclKind::Function { .. }));
    }

    #[test]
    fn optional_binds_tighter_than_result_and_grouping_overrides_it() {
        let source = r#"
type A = ?i32![]u8;
type B = ?(i32![]u8);
"#;

        let (_session, module) = parse_module(source);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[0].kind else {
            panic!("expected first type alias");
        };
        let ast::TypeKind::Result { ok, err } = &target.kind else {
            panic!("expected result type");
        };
        assert!(matches!(ok.kind, ast::TypeKind::Optional { .. }));
        assert!(matches!(err.kind, ast::TypeKind::Slice { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[1].kind else {
            panic!("expected second type alias");
        };
        let ast::TypeKind::Optional { inner } = &target.kind else {
            panic!("expected optional type");
        };
        assert!(matches!(inner.kind, ast::TypeKind::Result { .. }));
    }

    #[test]
    fn pointer_and_array_like_types_bind_tighter_than_result() {
        let source = r#"
type Ptr = *mut i32![]u8;
type Slice = []u8!i32;
type Array = [4]i32!bool;
type Grouped = *mut (i32![]u8);
"#;

        let (_session, module) = parse_module(source);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[0].kind else {
            panic!("expected pointer type alias");
        };
        let ast::TypeKind::Result { ok, err } = &target.kind else {
            panic!("expected pointer result type");
        };
        assert!(matches!(ok.kind, ast::TypeKind::Pointer { .. }));
        assert!(matches!(err.kind, ast::TypeKind::Slice { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[1].kind else {
            panic!("expected slice type alias");
        };
        let ast::TypeKind::Result { ok, err } = &target.kind else {
            panic!("expected slice result type");
        };
        assert!(matches!(ok.kind, ast::TypeKind::Slice { .. }));
        assert!(matches!(err.kind, ast::TypeKind::Path { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[2].kind else {
            panic!("expected array type alias");
        };
        let ast::TypeKind::Result { ok, err } = &target.kind else {
            panic!("expected array result type");
        };
        assert!(matches!(ok.kind, ast::TypeKind::Array { .. }));
        assert!(matches!(err.kind, ast::TypeKind::Path { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[3].kind else {
            panic!("expected grouped type alias");
        };
        let ast::TypeKind::Pointer { elem, .. } = &target.kind else {
            panic!("expected grouped pointer type");
        };
        assert!(matches!(elem.kind, ast::TypeKind::Result { .. }));
    }
}
