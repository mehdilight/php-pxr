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
        self.diags
            .emit(Diagnostic::error(msg).with_code("E0100").with_span(span));
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
            TokenKind::For => self.parse_for().map(Some),
            TokenKind::Do => self.parse_do().map(Some),
            TokenKind::Foreach => self.parse_foreach().map(Some),
            TokenKind::Switch => self.parse_switch().map(Some),
            TokenKind::Break => {
                self.advance();
                let level = self.optional_level();
                let semi = self.expect(&TokenKind::Semicolon, "`;` after `break`")?;
                Ok(Some(Stmt::Break(level, start.merge(semi.span))))
            }
            TokenKind::Continue => {
                self.advance();
                let level = self.optional_level();
                let semi = self.expect(&TokenKind::Semicolon, "`;` after `continue`")?;
                Ok(Some(Stmt::Continue(level, start.merge(semi.span))))
            }
            _ => self.parse_simple().map(Some),
        }
    }

    /// Parse an optional integer literal level (`break 2;`). Only literal
    /// levels are accepted in PXR, which is enough for all real programs and
    /// keeps the control-flow analysis simple.
    fn optional_level(&mut self) -> Option<u32> {
        if let TokenKind::Int(v) = self.peek().kind {
            if v > 0 {
                self.advance();
                return Some(v as u32);
            }
        }
        None
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
                | TokenKind::KwArray
                | TokenKind::Ident(_)
        )
    }

    fn parse_type(&mut self) -> Result<TypeRef, ()> {
        if self.at(&TokenKind::KwArray) {
            let start = self.advance().span;
            self.expect(&TokenKind::Lt, "`<` after `array` (write `array<T>`)")?;
            let inner = self.parse_type()?;
            let close = self.expect(&TokenKind::Gt, "`>` closing the array element type")?;
            let _ = start.merge(close.span);
            return Ok(TypeRef::Array(Box::new(inner)));
        }
        let tok = self.advance();
        Ok(match tok.kind {
            TokenKind::KwInt => TypeRef::Int,
            TokenKind::KwFloat => TypeRef::Float,
            TokenKind::KwString => TypeRef::String,
            TokenKind::KwBool => TypeRef::Bool,
            TokenKind::KwVoid => TypeRef::Void,
            TokenKind::Ident(name) => TypeRef::Named(name),
            other => {
                self.error(
                    tok.span,
                    format!("expected a type name, found {}", other.describe()),
                );
                TypeRef::Span(tok.span)
            }
        })
    }

    /// Expression statement or assignment.
    fn parse_simple(&mut self) -> Result<Stmt, ()> {
        let mut s = self.parse_simple_core()?;
        let semi = self.expect(&TokenKind::Semicolon, "`;` after expression")?;
        let span = stmt_span(&s).merge(semi.span);
        set_stmt_span(&mut s, span);
        Ok(s)
    }

    /// Parse a statement body that is one of: an annotated declaration, an
    /// assignment, a compound assignment, `++`/`--`, or an expression — without
    /// consuming a trailing `;`. This is reused by `for` clauses, where the
    /// separator is the comma or the enclosing `)`/`;` instead.
    fn parse_simple_core(&mut self) -> Result<Stmt, ()> {
        let start = self.peek().span;

        // `$name: type = expr` — the PXR type-annotation form.
        if let TokenKind::Variable(name) = self.peek().kind.clone() {
            if matches!(self.peek_at(1).kind, TokenKind::Colon) {
                self.advance(); // variable
                self.advance(); // colon
                let ty = self.parse_type()?;
                self.expect(&TokenKind::Assign, "`=` in annotated assignment")?;
                let value = self.parse_expr()?;
                let span = start.merge(value.span());
                return Ok(Stmt::Assign {
                    target: LValue::Var { name, span: start },
                    ty: Some(ty),
                    value,
                    span,
                });
            }
        }

        let lhs = self.parse_expr()?;
        match self.peek().kind.clone() {
            TokenKind::Assign => {
                self.advance();
                let value = self.parse_expr()?;
                let span = start.merge(value.span());
                let target = self.expr_to_lvalue(lhs)?;
                Ok(Stmt::Assign {
                    target,
                    ty: None,
                    value,
                    span,
                })
            }
            TokenKind::PlusEq
            | TokenKind::MinusEq
            | TokenKind::StarEq
            | TokenKind::SlashEq
            | TokenKind::PercentEq
            | TokenKind::DotEq
            | TokenKind::AmpEq
            | TokenKind::PipeEq
            | TokenKind::CaretEq
            | TokenKind::ShlEq
            | TokenKind::ShrEq
            | TokenKind::CoalesceEq => {
                let op = match self.peek().kind {
                    TokenKind::PlusEq => BinOp::Add,
                    TokenKind::MinusEq => BinOp::Sub,
                    TokenKind::StarEq => BinOp::Mul,
                    TokenKind::SlashEq => BinOp::Div,
                    TokenKind::PercentEq => BinOp::Mod,
                    TokenKind::DotEq => BinOp::Concat,
                    TokenKind::AmpEq => BinOp::BitAnd,
                    TokenKind::PipeEq => BinOp::BitOr,
                    TokenKind::CaretEq => BinOp::BitXor,
                    TokenKind::ShlEq => BinOp::Shl,
                    TokenKind::ShrEq => BinOp::Shr,
                    TokenKind::CoalesceEq => BinOp::NullCoalesce,
                    _ => unreachable!(),
                };
                self.advance();
                let value = self.parse_expr()?;
                let span = start.merge(value.span());
                let target = self.expr_to_lvalue(lhs)?;
                Ok(Stmt::AssignOp {
                    target,
                    op,
                    value,
                    span,
                })
            }
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let delta = if self.peek().kind == TokenKind::PlusPlus {
                    1
                } else {
                    -1
                };
                let op_tok = self.advance();
                let span = start.merge(op_tok.span);
                let target = self.expr_to_lvalue(lhs)?;
                Ok(Stmt::IncDec {
                    target,
                    delta,
                    span,
                })
            }
            _ => Ok(Stmt::Expr(lhs, start)),
        }
    }

    /// Convert a parsed expression to an assignment target, reporting an error
    /// for non-lvalues (e.g. `1 = 2`).
    fn expr_to_lvalue(&mut self, e: Expr) -> Result<LValue, ()> {
        match e {
            Expr::Var(name, span) => Ok(LValue::Var { name, span }),
            Expr::Index { base, index, span } => {
                let base = self.expr_to_lvalue(*base)?;
                Ok(LValue::Index {
                    base: Box::new(base),
                    index,
                    span,
                })
            }
            other => {
                let span = other.span();
                self.error(
                    span,
                    "the left-hand side of an assignment must be a variable or array element",
                );
                Err(())
            }
        }
    }

    fn parse_for(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `for`
        self.expect(&TokenKind::LParen, "`(` after `for`")?;
        let init = self.parse_for_clause(&TokenKind::Semicolon)?;
        self.expect(&TokenKind::Semicolon, "`;` after `for` initializer")?;
        let mut cond = Vec::new();
        if !self.at(&TokenKind::Semicolon) {
            loop {
                cond.push(self.parse_expr()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::Semicolon, "`;` after `for` condition")?;
        let update = self.parse_for_clause(&TokenKind::RParen)?;
        self.expect(&TokenKind::RParen, "`)` after `for` clauses")?;
        let body = self.parse_body()?;
        let end = self.peek().span;
        Ok(Stmt::For {
            init,
            cond,
            update,
            body,
            span: start.merge(end),
        })
    }

    fn parse_for_clause(&mut self, term: &TokenKind) -> Result<Vec<Stmt>, ()> {
        let mut parts = Vec::new();
        if self.at(term) {
            return Ok(parts);
        }
        loop {
            parts.push(self.parse_simple_core()?);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(parts)
    }

    fn parse_do(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `do`
        let body = self.parse_body()?;
        self.expect(&TokenKind::While, "`while` after `do` body")?;
        self.expect(&TokenKind::LParen, "`(` after `while`")?;
        let cond = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` after do-while condition")?;
        let semi = self.expect(&TokenKind::Semicolon, "`;` after do-while")?;
        Ok(Stmt::DoWhile {
            body,
            cond,
            span: start.merge(semi.span),
        })
    }

    fn parse_foreach(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `foreach`
        self.expect(&TokenKind::LParen, "`(` after `foreach`")?;
        let subject = self.parse_expr()?;
        self.expect(&TokenKind::As, "`as` in `foreach`")?;
        let first = self.expect_variable("loop variable in `foreach`")?;
        let first_name = match first.kind {
            TokenKind::Variable(n) => n,
            _ => unreachable!(),
        };
        let (key, key_span, value, value_span) = if self.eat(&TokenKind::DoubleArrow) {
            let vt = self.expect_variable("value variable in `foreach`")?;
            let vname = match vt.kind {
                TokenKind::Variable(n) => n,
                _ => unreachable!(),
            };
            (Some(first_name), first.span, vname, vt.span)
        } else {
            (None, Span::synthetic(), first_name, first.span)
        };
        self.expect(&TokenKind::RParen, "`)` after `foreach` header")?;
        let body = self.parse_body()?;
        let end = self.peek().span;
        Ok(Stmt::Foreach {
            subject,
            key,
            key_span,
            value,
            value_span,
            body,
            span: start.merge(end),
        })
    }

    fn parse_switch(&mut self) -> Result<Stmt, ()> {
        let start = self.advance().span; // `switch`
        self.expect(&TokenKind::LParen, "`(` after `switch`")?;
        let subject = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` after switch subject")?;
        self.expect(&TokenKind::LBrace, "`{` after switch subject")?;
        let mut cases = Vec::new();
        while matches!(self.peek().kind, TokenKind::Case | TokenKind::Default) {
            let cstart = self.peek().span;
            let test = if self.eat(&TokenKind::Case) {
                Some(self.parse_expr()?)
            } else {
                self.advance(); // `default`
                None
            };
            // PHP accepts `:` (and historically `;`) after a case label.
            if !self.eat(&TokenKind::Colon) {
                self.expect(&TokenKind::Semicolon, "`:` after `case`")?;
            }
            let mut body = Vec::new();
            while !matches!(
                self.peek().kind,
                TokenKind::Case | TokenKind::Default | TokenKind::RBrace | TokenKind::Eof
            ) {
                let before = self.pos;
                match self.parse_stmt() {
                    Ok(Some(s)) => body.push(s),
                    Ok(None) => {}
                    Err(()) => self.recover(),
                }
                if self.pos == before {
                    self.advance();
                }
            }
            let end = self.peek().span;
            cases.push(SwitchCase {
                test,
                body,
                span: cstart.merge(end),
            });
        }
        self.expect(&TokenKind::RBrace, "`}` after switch cases")?;
        Ok(Stmt::Switch {
            subject,
            cases,
            span: start.merge(self.peek().span),
        })
    }

    // ---- Expressions (precedence climbing) --------------------------------

    fn parse_expr(&mut self) -> Result<Expr, ()> {
        self.parse_ternary()
    }

    /// `cond ? then : else` and the short form `cond ?: else`.
    fn parse_ternary(&mut self) -> Result<Expr, ()> {
        let cond = self.parse_coalesce()?;
        if self.at(&TokenKind::Question) {
            let start = cond.span();
            self.advance();
            let then = if self.eat(&TokenKind::Colon) {
                None
            } else {
                let t = self.parse_expr()?;
                self.expect(&TokenKind::Colon, "`:` in ternary expression")?;
                Some(Box::new(t))
            };
            let else_branch = self.parse_ternary()?;
            let span = start.merge(else_branch.span());
            return Ok(Expr::Ternary {
                cond: Box::new(cond),
                then,
                else_branch: Box::new(else_branch),
                span,
            });
        }
        Ok(cond)
    }

    /// `a ?? b` is right-associative and binds tighter than the ternary.
    fn parse_coalesce(&mut self) -> Result<Expr, ()> {
        let lhs = self.parse_binary(1)?;
        if self.at(&TokenKind::Coalesce) {
            let start = lhs.span();
            self.advance();
            let rhs = self.parse_coalesce()?;
            let span = start.merge(rhs.span());
            return Ok(Expr::Binary {
                op: BinOp::NullCoalesce,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            });
        }
        Ok(lhs)
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
            TokenKind::Tilde => {
                self.advance();
                let e = self.parse_unary()?;
                let span = start.merge(e.span());
                Ok(Expr::Unary {
                    op: UnOp::BitNot,
                    expr: Box::new(e),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    /// A primary expression followed by any number of `[...]` index accesses.
    fn parse_postfix(&mut self) -> Result<Expr, ()> {
        let mut base = self.parse_primary()?;
        while self.at(&TokenKind::LBracket) {
            self.advance();
            let index = if self.at(&TokenKind::RBracket) {
                None
            } else {
                Some(Box::new(self.parse_expr()?))
            };
            let close = self.expect(&TokenKind::RBracket, "`]` after index")?;
            let span = base.span().merge(close.span);
            base = Expr::Index {
                base: Box::new(base),
                index,
                span,
            };
        }
        Ok(base)
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        let tok = self.peek().clone();
        let span = tok.span;
        match tok.kind {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr::IntLit(v, span))
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Expr::FloatLit(v, span))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::StrLit(s, span))
            }
            TokenKind::True => {
                self.advance();
                Ok(Expr::BoolLit(true, span))
            }
            TokenKind::False => {
                self.advance();
                Ok(Expr::BoolLit(false, span))
            }
            TokenKind::Null => {
                self.advance();
                Ok(Expr::NullLit(span))
            }
            TokenKind::Variable(name) => {
                self.advance();
                Ok(Expr::Var(name, span))
            }
            TokenKind::LBracket => {
                self.parse_array_literal(TokenKind::LBracket, TokenKind::RBracket)
            }
            TokenKind::KwArray if matches!(self.peek_at(1).kind, TokenKind::LParen) => {
                self.parse_array_literal(TokenKind::LParen, TokenKind::RParen)
            }
            TokenKind::Match => self.parse_match(),
            TokenKind::Ident(name) => {
                self.advance();
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
                        format!(
                            "unexpected bare identifier `{name}`; use `${name}` for a variable"
                        ),
                    );
                    Err(())
                }
            }
            TokenKind::LParen => {
                self.advance();
                let e = self.parse_expr()?;
                let close = self.expect(&TokenKind::RParen, "`)`")?;
                Ok(Expr::Paren(Box::new(e), span.merge(close.span)))
            }
            other => {
                self.error(
                    span,
                    format!("expected an expression, found {}", other.describe()),
                );
                Err(())
            }
        }
    }

    /// Parse an array literal, assuming the opening delimiter is the current
    /// token (`[` or `array(`). Consumes through the matching close token.
    fn parse_array_literal(&mut self, open: TokenKind, close: TokenKind) -> Result<Expr, ()> {
        let start = self.expect(&open, "`[`")?.span;
        let mut items = Vec::new();
        while !self.at(&close) && !self.at(&TokenKind::Eof) {
            let item_start = self.peek().span;
            let first = self.parse_expr()?;
            if self.eat(&TokenKind::DoubleArrow) {
                let value = self.parse_expr()?;
                let span = item_start.merge(value.span());
                items.push(ArrayItem {
                    key: Some(first),
                    value,
                    span,
                });
            } else {
                let span = item_start.merge(first.span());
                items.push(ArrayItem {
                    key: None,
                    value: first,
                    span,
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&close, "closing bracket of array literal")?;
        Ok(Expr::ArrayLit {
            items,
            span: start.merge(end.span),
        })
    }

    fn parse_match(&mut self) -> Result<Expr, ()> {
        let start = self.advance().span; // `match`
        self.expect(&TokenKind::LParen, "`(` after `match`")?;
        let subject = self.parse_expr()?;
        self.expect(&TokenKind::RParen, "`)` after match subject")?;
        self.expect(&TokenKind::LBrace, "`{` after match subject")?;
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            let arm_start = self.peek().span;
            if self.eat(&TokenKind::Default) {
                self.expect(&TokenKind::DoubleArrow, "`=>` after `default`")?;
                let result = self.parse_expr()?;
                let span = arm_start.merge(result.span());
                arms.push(MatchArm {
                    conds: Vec::new(),
                    is_default: true,
                    result,
                    span,
                });
            } else {
                let mut conds = Vec::new();
                loop {
                    conds.push(self.parse_expr()?);
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    // A `=>` after the comma would mean the comma separated two
                    // arms, which is malformed; the loop below stops on `=>`.
                    if self.at(&TokenKind::DoubleArrow) {
                        break;
                    }
                }
                self.expect(&TokenKind::DoubleArrow, "`=>` in match arm")?;
                let result = self.parse_expr()?;
                let span = arm_start.merge(result.span());
                arms.push(MatchArm {
                    conds,
                    is_default: false,
                    result,
                    span,
                });
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(&TokenKind::RBrace, "`}` ending the match expression")?;
        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
            span: start.merge(end.span),
        })
    }
}

fn stmt_span(s: &Stmt) -> Span {
    match s {
        Stmt::Echo(_, sp)
        | Stmt::Print(_, sp)
        | Stmt::Expr(_, sp)
        | Stmt::Return(_, sp)
        | Stmt::Break(_, sp)
        | Stmt::Continue(_, sp) => *sp,
        Stmt::Assign { span, .. }
        | Stmt::AssignOp { span, .. }
        | Stmt::IncDec { span, .. }
        | Stmt::If { span, .. }
        | Stmt::While { span, .. }
        | Stmt::For { span, .. }
        | Stmt::DoWhile { span, .. }
        | Stmt::Foreach { span, .. }
        | Stmt::Switch { span, .. }
        | Stmt::Block(_, span) => *span,
        Stmt::FnDecl(f) => f.span,
        Stmt::Empty => Span::synthetic(),
    }
}

fn set_stmt_span(s: &mut Stmt, span: Span) {
    match s {
        Stmt::Echo(_, sp)
        | Stmt::Print(_, sp)
        | Stmt::Expr(_, sp)
        | Stmt::Return(_, sp)
        | Stmt::Break(_, sp)
        | Stmt::Continue(_, sp) => *sp = span,
        Stmt::Assign { span: sp, .. }
        | Stmt::AssignOp { span: sp, .. }
        | Stmt::IncDec { span: sp, .. }
        | Stmt::If { span: sp, .. }
        | Stmt::While { span: sp, .. }
        | Stmt::For { span: sp, .. }
        | Stmt::DoWhile { span: sp, .. }
        | Stmt::Foreach { span: sp, .. }
        | Stmt::Switch { span: sp, .. }
        | Stmt::Block(_, sp) => *sp = span,
        Stmt::FnDecl(_) | Stmt::Empty => {}
    }
}

/// Binary operator precedence, matching PHP 8 (`!` binds tighter than `.`,
/// and `.` binds looser than `+`/`-` and the shifts).
fn bin_op(k: &TokenKind) -> Option<(BinOp, u8)> {
    Some(match k {
        TokenKind::OrOr => (BinOp::Or, 1),
        TokenKind::AndAnd => (BinOp::And, 2),
        TokenKind::Pipe => (BinOp::BitOr, 3),
        TokenKind::Caret => (BinOp::BitXor, 4),
        TokenKind::Amp => (BinOp::BitAnd, 5),
        TokenKind::EqEq => (BinOp::Eq, 6),
        TokenKind::NotEq => (BinOp::NotEq, 6),
        TokenKind::Identical => (BinOp::Identical, 6),
        TokenKind::NotIdentical => (BinOp::NotIdentical, 6),
        TokenKind::Lt => (BinOp::Lt, 7),
        TokenKind::LtEq => (BinOp::LtEq, 7),
        TokenKind::Gt => (BinOp::Gt, 7),
        TokenKind::GtEq => (BinOp::GtEq, 7),
        TokenKind::Spaceship => (BinOp::Spaceship, 7),
        TokenKind::Dot => (BinOp::Concat, 8),
        TokenKind::Shl => (BinOp::Shl, 9),
        TokenKind::Shr => (BinOp::Shr, 9),
        TokenKind::Plus => (BinOp::Add, 10),
        TokenKind::Minus => (BinOp::Sub, 10),
        TokenKind::Star => (BinOp::Mul, 11),
        TokenKind::Slash => (BinOp::Div, 11),
        TokenKind::Percent => (BinOp::Mod, 11),
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
                Expr::Binary {
                    op: BinOp::Concat,
                    lhs,
                    ..
                } => {
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
