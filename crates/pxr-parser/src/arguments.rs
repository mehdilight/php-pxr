//! Shared call grammar for functions, methods, constructors and invokable values.
use super::*;
impl Parser {
    // The opening parenthesis has already been consumed.
    pub(super) fn parse_call_arguments(&mut self) -> Result<(Vec<Expr>, Token), ()> {
        let mut args = Vec::new();
        let mut named = false;
        while !self.at(&TokenKind::RParen) {
            if self.peek_at(1).kind == TokenKind::Colon {
                let token = self.advance();
                let name = match &token.kind {
                    TokenKind::Ident(name) => name.clone(),
                    kind if kind
                        .symbol()
                        .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') =>
                    {
                        self.source[token.span.start - self.source_offset
                            ..token.span.end - self.source_offset]
                            .to_owned()
                    }
                    _ => {
                        self.error(token.span, "expected an argument name before `:`");
                        return Err(());
                    }
                };
                self.advance();
                let value = self.parse_expr()?;
                let span = token.span.merge(value.span());
                args.push(Expr::NamedArgument {
                    name,
                    value: Box::new(value),
                    span,
                });
                named = true;
            } else {
                if named {
                    self.error(
                        self.peek().span,
                        "positional arguments cannot follow named arguments",
                    );
                    return Err(());
                }
                args.push(self.parse_expr()?);
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let close = self.expect(&TokenKind::RParen, "`)` after arguments")?;
        Ok((args, close))
    }
}
