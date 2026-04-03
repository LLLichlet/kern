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
                .any(|hint| { hint.contains("use `///` to document this impl method") })
        );
    }
}
