//! Attribute parsing.
//!
//! Attribute syntax is split into module-level `#![...]`, item-level `#[...]`,
//! conditional attributes, and generic metadata lists.  Payload validation is
//! deliberately deferred to semantic analysis so the parser can preserve source
//! expressions and keep recovery local to the bracketed attribute.

use super::expr::Precedence;
use super::{ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;

impl<'a> Parser<'a> {
    pub(super) fn is_at_attribute_with_level(&mut self, expect_module_level: bool) -> bool {
        if !self.check(TokenType::Hash) {
            return false;
        }

        if expect_module_level {
            self.stream.peek_tag_nth(1) == TokenType::Bang
                && self.stream.peek_tag_nth(2) == TokenType::LBracket
        } else {
            self.stream.peek_tag_nth(1) == TokenType::LBracket
        }
    }

    /// Check whether the current lookahead starts an attribute.
    fn is_at_attribute(&mut self) -> bool {
        self.is_at_attribute_with_level(false) || self.is_at_attribute_with_level(true)
    }

    /// Parse a contiguous sequence of attribute blocks.
    pub fn parse_attributes(&mut self, expect_module_level: bool) -> ParseResult<Vec<Attribute>> {
        let mut attrs = Vec::new();

        while self.is_at_attribute() {
            self.check_canceled()?;
            let is_bang = self.stream.peek_tag_nth(1) == TokenType::Bang;

            // Stop as soon as the attribute level no longer matches the caller's expectation.
            if is_bang != expect_module_level {
                break;
            }

            let hash_span = self.advance().span; // Consume `#`.

            let mut is_module_level = false;
            if self.match_token(&[TokenType::Bang]) {
                is_module_level = true;
            }

            self.expect(TokenType::LBracket)?;

            let kind = if self.match_token(&[TokenType::If]) {
                // Form 1: conditional attributes, for example `#[if os == "linux"]`.
                let expr = self.parse_expression(Precedence::Lowest)?;

                if self.match_token(&[TokenType::Comma]) {
                    self.add_error(self.stream.prev_span(), "`#[if ...]` must be standalone and cannot be mixed with metadata in the same bracket".to_string());
                }

                AttributeKind::If(Box::new(expr))
            } else {
                // Form 2: metadata attributes such as `#[cold, export_name("foo")]`.
                let mut items = Vec::new();
                while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
                    self.check_canceled()?;
                    let ident_tok = self.expect(TokenType::Identifier)?;
                    let ident_id = self.intern_token(ident_tok);

                    if self.match_token(&[TokenType::LParen]) {
                        let expr = self.parse_expression(Precedence::Lowest)?;
                        self.expect(TokenType::RParen)?;
                        items.push(MetaItem::Call(ident_id, Box::new(expr)));
                    } else {
                        items.push(MetaItem::Marker(ident_id));
                    }

                    if !self.continue_after_comma(&[TokenType::RBracket]) {
                        break;
                    }
                }
                AttributeKind::Meta(items)
            };

            let rb = self.expect(TokenType::RBracket)?;
            attrs.push(Attribute {
                span: hash_span.to(rb.span),
                is_module_level,
                kind,
            });
        }
        Ok(attrs)
    }
}
