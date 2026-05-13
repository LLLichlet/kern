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
struct Uart {
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

        let ast::DeclKind::Struct { fields, .. } = &decl.kind else {
            panic!("expected struct declaration");
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
struct Counter {
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
struct Counter { value: i32 }

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

struct Pair[T,] {
    left: T,
    right: T,
}

enum Choice {
    A,
    B: i32,
}

trait Ops {
    fn run(lhs: i32, rhs: i32,) i32;
}

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
        let ast::UseTarget::Tree(items) = target else {
            panic!("expected grouped use tree");
        };
        assert_eq!(items.len(), 1);

        let ast::DeclKind::Struct {
            generics, fields, ..
        } = &module.decls[1].kind
        else {
            panic!("expected generic struct declaration");
        };
        assert_eq!(generics.len(), 1);
        assert_eq!(fields.len(), 2);

        let ast::DeclKind::Enum { variants, .. } = &module.decls[2].kind else {
            panic!("expected enum declaration");
        };
        assert_eq!(variants.len(), 2);

        let ast::DeclKind::Trait {
            assoc_types,
            methods,
            ..
        } = &module.decls[3].kind
        else {
            panic!("expected trait declaration");
        };
        assert!(assoc_types.is_empty());
        assert_eq!(methods.len(), 1);
        let ast::TypeKind::Function { params, .. } = &methods[0].signature.type_node.kind else {
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
        let ast::TypeKind::Path { segments, .. } = &type_node.kind else {
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
    fn parses_inline_and_nested_module_declarations() {
        let source = r#"
pub mod api {
    pub fn answer() i32 {
        return 42;
    }

    mod detail {
        fn hidden() i32 {
            return 7;
        }
    }

    mod file_backed;
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
        assert_eq!(module.decls.len(), 1);

        let ast::DeclKind::Mod { decls: Some(api) } = &module.decls[0].kind else {
            panic!("expected inline module");
        };
        assert_eq!(api.len(), 3);

        let ast::DeclKind::Function { .. } = &api[0].kind else {
            panic!("expected function inside inline module");
        };
        let ast::DeclKind::Mod {
            decls: Some(detail),
        } = &api[1].kind
        else {
            panic!("expected nested inline module");
        };
        assert_eq!(detail.len(), 1);
        let ast::DeclKind::Mod { decls: None } = &api[2].kind else {
            panic!("expected file-backed nested module declaration");
        };
    }

    #[test]
    fn parses_generic_enum_variant_match_arm_patterns() {
        let source = r#"
enum Mode {
    Off,
    On,
};

enum Box[T] {
    Empty,
    Full: T,
};

fn pick(value: Box[Mode]) i32 {
    return match (value) {
        Box[Mode].Empty => 1,
        Box[Mode].{ Full: Mode.On } => 2,
    };
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
        } = &module.decls[2].kind
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
        let ast::ExprKind::Return(Some(returned)) = &expr.kind else {
            panic!("expected return expression");
        };
        let ast::ExprKind::Match { arms, .. } = &returned.kind else {
            panic!("expected match expression");
        };

        let ast::MatchPatternKind::Value(value) = &arms[0].patterns[0].kind else {
            panic!("expected generic payloadless variant to parse as a value pattern");
        };
        let ast::ExprKind::FieldAccess { lhs, .. } = &value.kind else {
            panic!("expected variant value field access");
        };
        let ast::ExprKind::GenericInstantiation { .. } = &lhs.kind else {
            panic!("expected generic enum namespace in variant value pattern");
        };

        let ast::MatchPatternKind::Pattern(pattern) = &arms[1].patterns[0].kind else {
            panic!("expected generic payload variant to parse as a pattern");
        };
        let ast::PatternKind::Destructure(destructure) = &pattern.kind else {
            panic!("expected typed enum payload destructuring pattern");
        };
        assert!(destructure.target_type.is_some());
    }

    #[test]
    fn parses_let_else_arm_block() {
        let source = r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn unwrap_or(err: Result[i32, i32]) i32 {
    let .{ Ok: value } = err else {
        .{ Err: code } => return code,
    };
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
        let ast::ExprKind::Let { else_clause, .. } = &expr.kind else {
            panic!("expected let expression");
        };

        let Some(ast::LetElseClause::Arms(arms)) = else_clause.as_ref() else {
            panic!("expected let-else arm block");
        };
        assert_eq!(arms.len(), 1);
        let ast::PatternKind::Destructure(destructure) = &arms[0].pattern.kind else {
            panic!("expected destructure else pattern");
        };
        assert_eq!(destructure.fields.len(), 1);

        let ast::ExprKind::Return(Some(returned)) = &arms[0].body.kind else {
            panic!("expected return else branch");
        };
        let ast::ExprKind::Identifier(_) = returned.kind else {
            panic!("expected identifier return payload");
        };
    }

    #[test]
    fn parses_nested_let_else_inside_arm_block() {
        let source = r#"
enum Result[T, E] {
    Ok: T,
    Err: E,
};

fn pick(value: Result[Result[i32, i32], i32]) i32 {
    let .{ Ok: inner } = value else {
        .{ Err: outer_err } => {
            let .{ Ok: fallback } = Result[i32, i32].{ Ok: 9 } else {
                .{ Err: inner_err } => return inner_err,
            };
            return fallback;
        },
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
        let ast::ExprKind::Let { else_clause, .. } = &expr.kind else {
            panic!("expected outer let expression");
        };

        let Some(ast::LetElseClause::Arms(outer_arms)) = else_clause.as_ref() else {
            panic!("expected outer let-else arm block");
        };
        assert_eq!(outer_arms.len(), 1);
        let ast::ExprKind::Block {
            stmts,
            result: None,
        } = &outer_arms[0].body.kind
        else {
            panic!("expected block else branch");
        };
        let ast::StmtKind::ExprStmt(nested) = &stmts[0].kind else {
            panic!("expected nested let statement");
        };
        let ast::ExprKind::Let { else_clause, .. } = &nested.kind else {
            panic!("expected nested let expression");
        };

        let Some(ast::LetElseClause::Arms(nested_arms)) = else_clause.as_ref() else {
            panic!("expected nested let-else arm block");
        };
        assert_eq!(nested_arms.len(), 1);
        let ast::ExprKind::Return(Some(_)) = &nested_arms[0].body.kind else {
            panic!("expected nested return branch");
        };
    }

    #[test]
    fn parses_builtin_optional_and_result_type_forms() {
        let source = r#"
fn main(value: ?i32, status: i32!&[u8]) ?i32!&[u8] {
    let a = ?i32.None;
    let b = ?i32.{ Some: 7 };
    let c = value.?;
    let d = status.?;
    return ?i32!&[u8].{ Some: i32!&[u8].{ Ok: c + d } };
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
fn main(value: ?i32, status: i32!&[u8]) i32 {
    return value.? + status.?;
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
        assert!(matches!(lhs.kind, ast::ExprKind::Propagate { .. }));
        assert!(matches!(rhs.kind, ast::ExprKind::Propagate { .. }));
    }

    #[test]
    fn parses_associated_types_in_traits_and_impls() {
        let source = r#"
trait Add[Rhs] {
    type Out;
    fn add(rhs: Rhs) Out;
}

impl Vec2: Add[i32] {
    type Out = Vec2;
    fn add(rhs: i32) Out { return self; }
}
"#;

        let (_session, module) = parse_module(source);
        let ast::DeclKind::Trait {
            assoc_types,
            methods,
            ..
        } = &module.decls[0].kind
        else {
            panic!("expected trait declaration");
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
type A = ?i32!&[u8];
type B = ?(i32!&[u8]);
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
type Ptr = &mut i32!&[u8];
type Slice = &[u8]!i32;
type Array = [4]i32!bool;
type Grouped = &mut (i32!&[u8]);
type PtrArray = &[4]u8;
type MutPtrArray = &mut [4]u8;
type PtrInferArray = &[_]u8;
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

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[4].kind else {
            panic!("expected pointer-to-array type alias");
        };
        let ast::TypeKind::Pointer { elem, is_mut } = &target.kind else {
            panic!("expected pointer-to-array type");
        };
        assert!(!is_mut);
        assert!(matches!(elem.kind, ast::TypeKind::Array { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[5].kind else {
            panic!("expected mutable pointer-to-array type alias");
        };
        let ast::TypeKind::Pointer { elem, is_mut } = &target.kind else {
            panic!("expected mutable pointer-to-array type");
        };
        assert!(*is_mut);
        assert!(matches!(elem.kind, ast::TypeKind::Array { .. }));

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[6].kind else {
            panic!("expected pointer-to-inferred-array type alias");
        };
        let ast::TypeKind::Pointer { elem, .. } = &target.kind else {
            panic!("expected pointer-to-inferred-array type");
        };
        assert!(matches!(elem.kind, ast::TypeKind::ArrayInfer { .. }));
    }

    #[test]
    fn parses_bare_lbracket_closures_and_type_namespace_exprs() {
        let source = r#"
fn main() void {
    let base = i32.{40};
    let add = [base](value: i32) i32 {
        return base + value;
    };
    let stateless = [](value: i32) i32 {
        return value + 1;
    };
    let bytes = [4]u8.{ 1, 2, 3, 4 };
}
"#;

        let (_session, module) = parse_module(source);
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };

        let ast::StmtKind::ExprStmt(add_stmt) = &stmts[1].kind else {
            panic!("expected add let statement");
        };
        let ast::ExprKind::Let { init, .. } = &add_stmt.kind else {
            panic!("expected add let expression");
        };
        let ast::ExprKind::Closure { captures, .. } = &init.kind else {
            panic!("expected captured closure literal");
        };
        assert_eq!(captures.len(), 1);

        let ast::StmtKind::ExprStmt(stateless_stmt) = &stmts[2].kind else {
            panic!("expected stateless let statement");
        };
        let ast::ExprKind::Let { init, .. } = &stateless_stmt.kind else {
            panic!("expected stateless let expression");
        };
        let ast::ExprKind::Closure { captures, .. } = &init.kind else {
            panic!("expected stateless closure literal");
        };
        assert!(captures.is_empty());

        let ast::StmtKind::ExprStmt(bytes_stmt) = &stmts[3].kind else {
            panic!("expected bytes let statement");
        };
        let ast::ExprKind::Let { init, .. } = &bytes_stmt.kind else {
            panic!("expected bytes let expression");
        };
        let ast::ExprKind::DataInit {
            type_node: Some(type_node),
            ..
        } = &init.kind
        else {
            panic!("expected typed array data init");
        };
        assert!(matches!(type_node.kind, ast::TypeKind::Array { .. }));
    }

    #[test]
    fn parses_super_visibility_with_and_without_space() {
        let source = r#"
pub..fn add(a: i32, b: i32) i32 {
    return a + b;
}

pub .. use .helper as parent_helper;

fn helper() i32 {
    return 0;
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
        assert_eq!(module.decls[0].vis, ast::Visibility::Super);
        assert_eq!(module.decls[1].vis, ast::Visibility::Super);
        assert_eq!(module.decls[2].vis, ast::Visibility::Private);
    }

    #[test]
    fn parses_package_visibility_with_and_without_space() {
        let source = r#"
pub/fn add(a: i32, b: i32) i32 {
    return a + b;
}

pub / use .helper as pkg_helper;

fn helper() i32 {
    return 0;
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
        assert_eq!(module.decls[0].vis, ast::Visibility::Package);
        assert_eq!(module.decls[1].vis, ast::Visibility::Package);
        assert_eq!(module.decls[2].vis, ast::Visibility::Private);
    }

    #[test]
    fn parses_struct_field_visibility_with_and_without_space() {
        let source = r#"
struct Config {
    pub visible: i32,
    pub.. parent_visible: i32,
    pub .. spaced_parent_visible: i32,
    pub/ package_visible: i32,
    pub / spaced_package_visible: i32,
    private: i32,
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
        let ast::DeclKind::Struct { fields, .. } = &module.decls[0].kind else {
            panic!("expected struct declaration");
        };
        assert_eq!(fields[0].vis, ast::Visibility::Public);
        assert_eq!(fields[1].vis, ast::Visibility::Super);
        assert_eq!(fields[2].vis, ast::Visibility::Super);
        assert_eq!(fields[3].vis, ast::Visibility::Package);
        assert_eq!(fields[4].vis, ast::Visibility::Package);
        assert_eq!(fields[5].vis, ast::Visibility::Private);
    }

    #[test]
    fn parses_package_root_and_anchored_parent_paths() {
        let source = r#"
use /util.answer;

type CurrentKind = /util.Kind;
type ParentKind = ..shared.Kind;

fn main() i32 {
    let value = /util.kind();
    return ..shared.answer();
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );

        let ast::DeclKind::Use { kind, path, .. } = &module.decls[0].kind else {
            panic!("expected use decl");
        };
        assert_eq!(*kind, ast::UsePathKind::Package);
        assert_eq!(path.len(), 2);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[1].kind else {
            panic!("expected first type alias");
        };
        let ast::TypeKind::Path { anchor, segments } = &target.kind else {
            panic!("expected path type");
        };
        assert_eq!(*anchor, Some(ast::PathAnchor::Package));
        assert_eq!(segments.len(), 2);

        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[2].kind else {
            panic!("expected second type alias");
        };
        let ast::TypeKind::Path { anchor, segments } = &target.kind else {
            panic!("expected path type");
        };
        assert_eq!(*anchor, Some(ast::PathAnchor::Parent));
        assert_eq!(segments.len(), 2);

        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[3].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };

        let ast::StmtKind::ExprStmt(first_stmt) = &stmts[0].kind else {
            panic!("expected first stmt");
        };
        let ast::ExprKind::Let { init, .. } = &first_stmt.kind else {
            panic!("expected let");
        };
        let ast::ExprKind::Call { callee, .. } = &init.kind else {
            panic!("expected call");
        };
        let ast::ExprKind::FieldAccess { lhs, field, .. } = &callee.kind else {
            panic!("expected field access");
        };
        assert_eq!(session.resolve(*field), "kind");
        let ast::ExprKind::AnchoredPath { anchor, name, .. } = &lhs.kind else {
            panic!("expected anchored path");
        };
        assert_eq!(*anchor, ast::PathAnchor::Package);
        assert_eq!(session.resolve(*name), "util");

        let ast::StmtKind::ExprStmt(second_stmt) = &stmts[1].kind else {
            panic!("expected second stmt");
        };
        let ast::ExprKind::Return(Some(ret)) = &second_stmt.kind else {
            panic!("expected return");
        };
        let ast::ExprKind::Call { callee, .. } = &ret.kind else {
            panic!("expected call");
        };
        let ast::ExprKind::FieldAccess { lhs, field, .. } = &callee.kind else {
            panic!("expected field access");
        };
        assert_eq!(session.resolve(*field), "answer");
        let ast::ExprKind::AnchoredPath { anchor, name, .. } = &lhs.kind else {
            panic!("expected anchored parent path");
        };
        assert_eq!(*anchor, ast::PathAnchor::Parent);
        assert_eq!(session.resolve(*name), "shared");
    }

    #[test]
    fn parses_local_use_statements_inside_blocks() {
        let source = r#"
fn helper() i32 { return 1; }

fn main() i32 {
    use .{helper as answer_fn};
    return answer_fn();
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
        assert_eq!(stmts.len(), 2);

        let ast::StmtKind::Use(use_stmt) = &stmts[0].kind else {
            panic!("expected local use stmt");
        };
        assert_eq!(use_stmt.kind, ast::UsePathKind::Current);
        assert!(use_stmt.path.is_empty());

        let ast::UseTarget::Tree(items) = &use_stmt.target else {
            panic!("expected local import tree");
        };
        let ast::UseTree::Path {
            path,
            alias: Some(alias),
            ..
        } = &items[0]
        else {
            panic!("expected aliased tree entry");
        };
        assert_eq!(path.len(), 1);
        assert_eq!(session.resolve(*alias), "answer_fn");

        let ast::StmtKind::ExprStmt(expr) = &stmts[1].kind else {
            panic!("expected return stmt");
        };
        let ast::ExprKind::Return(Some(value)) = &expr.kind else {
            panic!("expected return expression");
        };
        let ast::ExprKind::Call { callee, .. } = &value.kind else {
            panic!("expected call");
        };
        let ast::ExprKind::Identifier(name) = &callee.kind else {
            panic!("expected identifier callee");
        };
        assert_eq!(session.resolve(*name), "answer_fn");
    }

    #[test]
    fn rejects_current_module_anchored_type_paths() {
        let source = r#"
type Bad = .util.Kind;
"#;

        let (session, _module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected parser diagnostics for current-module anchored type path"
        );

        let rendered = format!("{:?}", session.diagnostics);
        assert!(
            rendered.contains("Expected type definition, found '.'"),
            "unexpected diagnostics: {:?}",
            session.diagnostics
        );
    }

    #[test]
    fn recovers_defer_statement_missing_semicolon_inside_block() {
        let source = r#"
fn main() void {
    defer cleanup()
    run();
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected missing semicolon diagnostic"
        );
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };
        assert!(stmts.len() >= 2);
        let ast::StmtKind::ExprStmt(first) = &stmts[0].kind else {
            panic!("expected defer statement");
        };
        assert!(matches!(first.kind, ast::ExprKind::Defer { .. }));
        assert!(
            stmts.iter().skip(1).any(|stmt| matches!(
                &stmt.kind,
                ast::StmtKind::ExprStmt(expr) if matches!(expr.kind, ast::ExprKind::Call { .. })
            )),
            "expected following expression statement"
        );
    }

    #[test]
    fn parses_defer_method_call_statement() {
        let source = r#"
fn main() void {
    defer self.release();
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
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };
        assert_eq!(stmts.len(), 1);
        let ast::StmtKind::ExprStmt(stmt) = &stmts[0].kind else {
            panic!("expected defer statement");
        };
        let ast::ExprKind::Defer { expr } = &stmt.kind else {
            panic!("expected defer expression");
        };
        assert!(matches!(expr.kind, ast::ExprKind::Call { .. }));
    }

    #[test]
    fn recovers_missing_block_close_at_eof() {
        let source = r#"
fn main() void {
    run();
"#;

        let (session, module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected unclosed block diagnostic"
        );
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected recovered block body");
        };
        assert_eq!(stmts.len(), 1);
    }

    #[test]
    fn recovers_missing_expression_in_let_initializer() {
        let source = r#"
fn main() void {
    let value = ;
    run();
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected missing expression diagnostic"
        );
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };
        assert_eq!(stmts.len(), 2);
        let ast::StmtKind::ExprStmt(first) = &stmts[0].kind else {
            panic!("expected let statement");
        };
        let ast::ExprKind::Let { init, .. } = &first.kind else {
            panic!("expected let expression");
        };
        assert!(matches!(init.kind, ast::ExprKind::Error));
        let ast::StmtKind::ExprStmt(second) = &stmts[1].kind else {
            panic!("expected following expression statement");
        };
        assert!(matches!(second.kind, ast::ExprKind::Call { .. }));
    }

    #[test]
    fn recovers_missing_defer_expression() {
        let source = r#"
fn main() void {
    defer ;
    run();
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected missing expression diagnostic"
        );
        let ast::DeclKind::Function {
            body: Some(body), ..
        } = &module.decls[0].kind
        else {
            panic!("expected function body");
        };
        let ast::ExprKind::Block { stmts, .. } = &body.kind else {
            panic!("expected block body");
        };
        assert_eq!(stmts.len(), 2);
        let ast::StmtKind::ExprStmt(first) = &stmts[0].kind else {
            panic!("expected defer statement");
        };
        let ast::ExprKind::Defer { expr } = &first.kind else {
            panic!("expected defer expression");
        };
        assert!(matches!(expr.kind, ast::ExprKind::Error));
    }

    #[test]
    fn recovers_missing_type_after_prefix() {
        let source = r#"
type Bad = &;
type Good = i32;
"#;

        let (session, module) = parse_module(source);
        assert!(
            !session.diagnostics.is_empty(),
            "expected missing type diagnostic"
        );
        assert_eq!(module.decls.len(), 2);
        let ast::DeclKind::TypeAlias { target, .. } = &module.decls[0].kind else {
            panic!("expected type alias");
        };
        let ast::TypeKind::Pointer { elem, .. } = &target.kind else {
            panic!("expected recovered pointer type");
        };
        assert!(matches!(elem.kind, ast::TypeKind::Error));
    }

    #[test]
    fn casts_bind_after_prefix_unary_operators() {
        let source = "fn main() i32 { return #array as i32 - 1; }";
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
        let ast::ExprKind::Binary { lhs, op, rhs } = &value.kind else {
            panic!("expected subtraction after cast");
        };
        assert_eq!(*op, ast::BinaryOperator::Subtract);
        assert!(matches!(
            rhs.kind,
            ast::ExprKind::Integer {
                value: 1,
                suffix: None
            }
        ));

        let ast::ExprKind::As { lhs: cast_lhs, .. } = &lhs.kind else {
            panic!("expected cast on the unary prefix result");
        };
        let ast::ExprKind::Unary { op, operand } = &cast_lhs.kind else {
            panic!("expected unary operand inside cast");
        };
        assert_eq!(*op, ast::UnaryOperator::MetaOf);
        assert!(matches!(operand.kind, ast::ExprKind::Identifier(_)));
    }

    #[test]
    fn recovers_unclosed_call_before_statement_boundary() {
        let source = r#"
struct Point { x: i32, y: i32 }
enum Shape { Dot: Point, Empty }

fn main() i32 {
    let point = make_point(1, 2)
    let shape = Shape.Dot(point;
    match shape {
        Shape.Empty => return 0;
    }
}
"#;

        let (session, module) = parse_module(source);
        assert!(
            session.diagnostics.iter().any(|diagnostic| diagnostic
                .hints
                .iter()
                .any(|hint| hint == "unclosed parenthesis")),
            "expected unclosed call diagnostic"
        );
        assert_eq!(module.decls.len(), 3);
    }

    #[test]
    fn parses_match_arm_call_shaped_value_pattern_without_hanging() {
        let source = r#"
struct Point { x: i32, y: i32 }
enum Shape { Dot: Point, Empty }
fn make_point(x: i32, y: i32) Point {
    return Point.{ x: x, y: y };
}
fn main(flag: bool) i32 {
    let point = make_point(1, 2);
    let shape = Shape.Dot(point);
    match shape {
        Shape.Dot(p) => return p.x;
        Shape.Empty => return 0;
    }
}
"#;

        let (_session, module) = parse_module(source);
        assert_eq!(module.decls.len(), 4);
    }
}
