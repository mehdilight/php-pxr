//! Shared call grammar for functions, methods, constructors and invokable values.
use super::*;
impl Parser {
    // The opening parenthesis has already been consumed.
    pub(super) fn parse_call_arguments(&mut self) -> Result<(Vec<Expr>, Token), ()> {
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) {
            args.push(self.parse_expr()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let close = self.expect(&TokenKind::RParen, "`)` after arguments")?;
        Ok((args, close))
    }
}
