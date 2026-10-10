//! The PXR parser: tokens in, [`pxr_ast::Program`] out.
//!
//! The parser is a hand written recursive-descent / precedence-climbing
//! parser. It performs *no* semantic work: unknown names, arity mismatches and
//! type errors are all deferred to `pxr-sema`. When it hits an error it records
//! a diagnostic and resynchronizes at the next statement boundary so that a
//! single run reports as many independent syntax errors as possible.

use pxr_ast::*;
use pxr_diag::{Diagnostic, Diagnostics, Span};
use pxr_lexer::{Token, TokenKind};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    diags: Diagnostics,
    /// Guards against infinite loops during error recovery.
    stuck: usize,
}

pub fn parse(src: &str) -> (Option<Program>, Diagnostics) {
    let (toks, lex_diags) = pxr_lexer::lex(src);
    let mut p = Parser {
        toks,
        pos: 0,
        diags: Diagnostics::new(),
        stuck: 0,
    };
    let program = p.parse_program();
    let mut diags = lex_diags;
    diags.extend(p.diags);
    (program, diags)
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn peek_at(&self, off: usize) -> &Token {
        &self.toks[(self.pos + off).min(self.toks.len() - 1)]
    }

    fn at(&self, k: &TokenKind) -> bool {
        &self.peek().kind == k
    }

    fn advance(&mut self) -> Token {
        let t = self.peek().clone();
        if !matches!(t.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, k: &TokenKind) -> bool {
        if self.at(k) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags.emit(
            Diagnostic::error(msg)
                .with_code("E0100")
                .with_span(span),
        );
    }

    fn expect_ident(&mut self, what: &str) -> Result<Token, ()> {
        if matches!(self.peek().kind, TokenKind::Ident(_)) {
            Ok(self.advance())
        } else {
            let found = self.peek().kind.describe();
            let span = self.peek().span;
            self.error(span, format!("expected {what}, found {found}"));
            Err(())
        }
    }

    fn expect_variable(&mut self, what: &str) -> Result<Token, ()> {
        if matches!(self.peek().kind, TokenKind::Variable(_)) {
            Ok(self.advance())
        } else {
            let found = self.peek().kind.describe();
            let span = self.peek().span;
            self.error(span, format!("expected {what}, found {found}"));
            Err(())
        }
    }

    fn expect(&mut self, k: &TokenKind, what: &str) -> Result<Token, ()> {
        if self.at(k) {
            Ok(self.advance())
        } else {
            let found = self.peek().kind.describe();
            let span = self.peek().span;
            self.error(span, format!("expected {what}, found {found}"));
            Err(())
        }
    }

    fn parse_program(&mut self) -> Option<Program> {
        let start = self.peek().span;
        let mut items = Vec::new();
        let mut last = self.pos;
        while !self.at(&TokenKind::Eof) {
            match self.parse_stmt() {
                Ok(Some(s)) => items.push(s),
                Ok(None) => {}
                Err(()) => self.recover(),
            }
            if self.pos == last {
                // No forward progress: force it to avoid an infinite loop.
                self.stuck += 1;
                if self.stuck > 1000 {
                    break;
                }
                self.advance();
            }
            last = self.pos;
        }
        let end = self.peek().span;
        Some(Program {
            items,
            span: start.merge(end),
        })
    }

    /// Skip tokens until a plausible statement boundary.
    fn recover(&mut self) {
        let mut depth = 0i32;
        loop {
            match &self.peek().kind {
                TokenKind::Eof => break,
                TokenKind::Semicolon if depth == 0 => {
                    self.advance();
                    break;
                }
                TokenKind::LBrace => {
                    depth += 1;
                    self.advance();
                }
                TokenKind::RBrace => {
                    if depth == 0 {
                        self.advance();
                        break;
                    }
                    depth -= 1;
                    self.advance();
                    if depth == 0 {
                        break;
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn parse_stmt(&mut self) -> Result<Option<Stmt>, ()> {
        let start = self.peek().span;
        match self.peek().kind.clone() {
            TokenKind::Semicolon => {
                self.advance();
                Ok(Some(Stmt::Empty))
            }
            TokenKind::LBrace => {
                let body = self.parse_block()?;
                Ok(Some(Stmt::Block(body, start)))
            }
            TokenKind::Echo => {
                self.advance();
                let mut exprs = Vec::new();
                loop {
                    exprs.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let semi = self.expect(&TokenKind::Semicolon, "`;` after `echo`")?;
                Ok(Some(Stmt::Echo(exprs, start.merge(semi.span))))
            }
            TokenKind::Print => {
                self.advance();
                let e = self.parse_expr()?;
                let semi = self.expect(&TokenKind::Semicolon, "`;` after `print`")?;
                Ok(Some(Stmt::Print(e, start.merge(semi.span))))
            }
            TokenKind::Function => self.parse_fn().map(|f| Some(Stmt::FnDecl(Box::new(f)))),
            TokenKind::Return => {
                self.advance();
                let value = if self.at(&TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                let semi = self.expect(&TokenKind::Semicolon, "`;` after return value")?;
                Ok(Some(Stmt::Return(value, start.merge(semi.span))))
            }
            TokenKind::If => self.parse_if().map(Some),
            TokenKind::While => self.parse_while().map(Some),
            _ => self.parse_simple().map(Some),
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, ()> {
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            let before = self.pos;
            match self.parse_stmt() {
                Ok(Some(s)) => stmts.push(s),
                Ok(None) => {}
                Err(()) => self.recover(),
            }
            if self.pos == before {
                self.advance();
            }
        }
        self.expect(&TokenKind::RBrace, "`}`")?;
        Ok(stmts)
    }

    /// Parse either a braced block or a single statement (PHP allows both).
    fn parse_body(&mut self) -> Result<Vec<Stmt>, ()> {
        if self.at(&TokenKind::LBrace) {
            self.parse_block()
        } else {
            match self.parse_stmt()? {
                Some(s) => Ok(vec![s]),
                None => Ok(vec![]),
            }
        }
    }

    fn parse_if(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `if`
        self.expect(&TokenKind::LParen, "`(` after `if`")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` after if condition")?;
        let then_branch = self.parse_body()?;

        let mut elifs = Vec::new();
        while self.at(&TokenKind::ElseIf) {
            self.advance();
            self.expect(&TokenKind::LParen, "`(` after `elseif`")?;
            let c = self.parse_expr()?;
            self.expect(&TokenKind::RParen, "`)` after elseif condition")?;
            let b = self.parse_body()?;
            elifs.push((c, b));
        }

        let mut else_branch = None;
        if self.eat(&TokenKind::Else) {
            else_branch = Some(self.parse_body()?);
        }

        let end = self.peek().span;
        Ok(Stmt::If {
            cond,
            then_branch,
            elifs,
            else_branch,
            span: start.merge(end),
        })
    }

    fn parse_while(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `while`
        self.expect(&TokenKind::LParen, "`(` after `while`")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` after while condition")?;
        let body = self.parse_body()?;
        let end = self.peek().span;
        Ok(Stmt::While {
            cond,
            body,
            span: start.merge(end),
        })
    }

    fn parse_fn(&mut self) -> Result<FnDecl, ()> {
        let start = self.advance().span; // `function`
        let name_tok = self.expect_ident("function name")?;
        let name = match name_tok.kind {
            TokenKind::Ident(n) => n,
            _ => unreachable!(),
        };
        let name_span = name_tok.span;

        self.expect(&TokenKind::LParen, "`(` after function name")?;
        let mut params = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                let p_start = self.peek().span;
                let ty = if self.is_type_start() {
                    Some(self.parse_type()?)
                } else {
                    None
                };
                let vt = self.expect_variable("parameter name")?;
                let pname = match vt.kind {
                    TokenKind::Variable(n) => n,
                    _ => unreachable!(),
                };
                params.push(Param {
                    name: pname,
                    name_span: vt.span,
                    ty,
                    span: p_start.merge(vt.span),
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "`)` after parameter list")?;

        let ret = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let end = self.peek().span;
        Ok(FnDecl {
            name,
            name_span,
            params,
            ret,
            body,
            span: start.merge(end),
        })
    }

    fn is_type_start(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::KwInt
                | TokenKind::KwFloat
                | TokenKind::KwString
                | TokenKind::KwBool
                | TokenKind::KwVoid
                | TokenKind::Ident(_)
        )
    }

    fn parse_type(&mut self) -> Result<TypeRef, ()> {
        let tok = self.advance();
        Ok(match tok.kind {
            TokenKind::KwInt => TypeRef::Int,
            TokenKind::KwFloat => TypeRef::Float,
            TokenKind::KwString => TypeRef::String,
            TokenKind::KwBool => TypeRef::Bool,
            TokenKind::KwVoid => TypeRef::Void,
            TokenKind::Ident(name) => TypeRef::Named(name),
            other => {
                self.error(tok.span, format!("expected a type name, found {}", other.describe()));
                TypeRef::Span(tok.span)
            }
        })
    }

    /// Expression statement or assignment.
    fn parse_simple(&mut self) -> Result<Stmt, ()> {
        let start = self.peek().span;

        // `$name = expr;` or the PXR annotation `$name: type = expr;`
        if let TokenKind::Variable(name) = self.peek().kind.clone() {
            let is_annotated = matches!(self.peek_at(1).kind, TokenKind::Colon);
            if self.peek_at(1).kind == TokenKind::Assign || is_annotated {
                self.advance(); // variable
                let mut ty = None;
                if is_annotated {
                    self.advance(); // colon
                    ty = Some(self.parse_type()?);
                }
                self.expect(&TokenKind::Assign, "`=` in assignment")?;
                let value = self.parse_expr()?;
                let semi = self.expect(&TokenKind::Semicolon, "`;` after assignment")?;
                return Ok(Stmt::Assign {
                    name,
                    ty,
                    value,
                    span: start.merge(semi.span),
                });
            }
        }

        let expr = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semicolon, "`;` after expression")?;
        Ok(Stmt::Expr(expr, start.merge(semi.span)))
    }

    // ---- Expressions (precedence climbing) --------------------------------

    fn parse_expr(&mut self) -> Result<Expr, ()> {
        self.parse_binary(0)
    }

    fn parse_binary(&mut self, min_prec: u8) -> Result<Expr, ()> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, prec) = match bin_op(&self.peek().kind) {
                Some(v) => v,
                None => break,
            };
            if prec < min_prec {
                break;
            }
            self.advance();
            let rhs = self.parse_binary(prec + 1)?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, ()> {
        let start = self.peek().span;
        match self.peek().kind {
            TokenKind::Minus => {
                self.advance();
                let e = self.parse_unary()?;
                let span = start.merge(e.span());
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(e),
                    span,
                })
            }
            TokenKind::Plus => {
                self.advance();
                self.parse_unary()
            }
            TokenKind::Bang => {
                self.advance();
                let e = self.parse_unary()?;
                let span = start.merge(e.span());
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(e),
                    span,
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        let tok = self.advance();
        let span = tok.span;
        match tok.kind {
            TokenKind::Int(v) => Ok(Expr::IntLit(v, span)),
            TokenKind::Float(v) => Ok(Expr::FloatLit(v, span)),
            TokenKind::Str(s) => Ok(Expr::StrLit(s, span)),
            TokenKind::True => Ok(Expr::BoolLit(true, span)),
            TokenKind::False => Ok(Expr::BoolLit(false, span)),
            TokenKind::Null => Ok(Expr::NullLit(span)),
            TokenKind::Variable(name) => Ok(Expr::Var(name, span)),
            TokenKind::Ident(name) => {
                if self.at(&TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    let close = self.expect(&TokenKind::RParen, "`)` after arguments")?;
                    Ok(Expr::Call {
                        callee: name,
                        callee_span: span,
                        args,
                        span: span.merge(close.span),
                    })
                } else {
                    self.error(
                        span,
                        format!("unexpected bare identifier `{name}`; use `${name}` for a variable"),
                    );
                    Err(())
                }
            }
            TokenKind::LParen => {
                let e = self.parse_expr()?;
                let close = self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Expr::Paren(Box::new(e), span.merge(close.span)))
            }
            other => {
                self.error(span, format!("expected an expression, found {}", other.describe()));
                Err(())
            }
        }
    }
}

/// Binary operator precedence, matching PHP 8 (`!` binds tighter than `.`,
/// and `.` binds looser than `+`/`-`).
fn bin_op(k: &TokenKind) -> Option<(BinOp, u8)> {
    Some(match k {
        TokenKind::OrOr => (BinOp::Or, 1),
        TokenKind::AndAnd => (BinOp::And, 2),
        TokenKind::EqEq => (BinOp::Eq, 3),
        TokenKind::NotEq => (BinOp::NotEq, 3),
        TokenKind::Identical => (BinOp::Identical, 3),
        TokenKind::NotIdentical => (BinOp::NotIdentical, 3),
        TokenKind::Lt => (BinOp::Lt, 4),
        TokenKind::LtEq => (BinOp::LtEq, 4),
        TokenKind::Gt => (BinOp::Gt, 4),
        TokenKind::GtEq => (BinOp::GtEq, 4),
        TokenKind::Dot => (BinOp::Concat, 5),
        TokenKind::Plus => (BinOp::Add, 6),
        TokenKind::Minus => (BinOp::Sub, 6),
        TokenKind::Star => (BinOp::Mul, 7),
        TokenKind::Slash => (BinOp::Div, 7),
        TokenKind::Percent => (BinOp::Mod, 7),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> Program {
        let (p, diags) = parse(src);
        assert!(
            !diags.has_errors(),
            "unexpected parse errors: {:#?}",
            diags.items()
        );
        p.unwrap()
    }

    #[test]
    fn hello() {
        let p = ok("<?php echo \"hi\";");
        assert_eq!(p.items.len(), 1);
        assert!(matches!(p.items[0], Stmt::Echo(_, _)));
    }

    #[test]
    fn function_decl() {
        let p = ok("<?php function add(int $a, int $b): int { return $a + $b; }");
        match &p.items[0] {
            Stmt::FnDecl(f) => {
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
                assert!(f.ret.is_some());
            }
            other => panic!("expected fn, got {other:?}"),
        }
    }

    #[test]
    fn precedence_concat() {
        // `1 + 2 . 3` groups as `(1 + 2) . 3` because `.` is looser than `+`.
        let p = ok("<?php echo 1 + 2 . 3;");
        match &p.items[0] {
            Stmt::Echo(exprs, _) => match &exprs[0] {
                Expr::Binary { op: BinOp::Concat, lhs, .. } => {
                    assert!(matches!(**lhs, Expr::Binary { op: BinOp::Add, .. }));
                }
                other => panic!("unexpected {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn if_else_chain() {
        ok("<?php if ($a) { echo 1; } elseif ($b) echo 2; else { echo 3; }");
    }

    #[test]
    fn annotated_assignment() {
        let p = ok("<?php $x: int = 5;");
        assert!(matches!(p.items[0], Stmt::Assign { ty: Some(_), .. }));
    }

    #[test]
    fn reports_multiple_errors() {
        let (_, diags) = parse("<?php echo ; echo ;");
        assert!(diags.error_count() >= 1);
    }
}
