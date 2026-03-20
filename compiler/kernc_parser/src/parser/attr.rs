use super::expr::Precedence;
use super::{ParseResult, Parser};
use kernc_ast::*;
use kernc_lexer::TokenType;

impl<'a> Parser<'a> {
    /// 判断当前是否处于属性标记的起始位置，避免与 `#arr` (取长度) 冲突
    fn is_at_attribute(&mut self) -> bool {
        if self.check(TokenType::Hash) {
            let next = self.stream.peek_nth(1).tag;
            if next == TokenType::LBracket {
                return true; // #[
            }
            if next == TokenType::Bang && self.stream.peek_nth(2).tag == TokenType::LBracket {
                return true; // #![
            }
        }
        false
    }

    /// 解析连续的属性块
    pub fn parse_attributes(&mut self, expect_module_level: bool) -> ParseResult<Vec<Attribute>> {
        let mut attrs = Vec::new();

        while self.is_at_attribute() {
            let is_bang = self.stream.peek_nth(1).tag == TokenType::Bang;

            // 如果期望解析模块级 #![...]，但遇到了 #[...]，立刻跳出循环，留给下一级去吃
            // 或者期望解析 #[...]，但遇到了 #![...]，也跳出
            if is_bang != expect_module_level {
                break;
            }

            let hash_span = self.advance().span; // 消费 `#`

            let mut is_module_level = false;
            if self.match_token(&[TokenType::Bang]) {
                is_module_level = true;
            }

            self.expect(TokenType::LBracket)?;

            let kind = if self.match_token(&[TokenType::If]) {
                // 模式 1: 条件编译 #[if(expr)]
                self.expect(TokenType::LParen)?;
                let expr = self.parse_expression(Precedence::Lowest)?;
                self.expect(TokenType::RParen)?;

                if self.match_token(&[TokenType::Comma]) {
                    self.add_error(self.stream.prev_span(), "`#[if(...)]` must be standalone and cannot be mixed with metadata in the same bracket".to_string());
                }

                AttributeKind::If(Box::new(expr))
            } else {
                // 模式 2: 元数据 #[cold, export_name("foo")]
                let mut items = Vec::new();
                while !self.check(TokenType::RBracket) && !self.check(TokenType::Eof) {
                    let ident_tok = self.expect(TokenType::Identifier)?;
                    let ident_id = self.intern_token(ident_tok);

                    if self.match_token(&[TokenType::LParen]) {
                        let expr = self.parse_expression(Precedence::Lowest)?;
                        self.expect(TokenType::RParen)?;
                        items.push(MetaItem::Call(ident_id, Box::new(expr)));
                    } else {
                        items.push(MetaItem::Marker(ident_id));
                    }

                    if !self.match_token(&[TokenType::Comma]) {
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
