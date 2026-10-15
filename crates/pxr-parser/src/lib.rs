//! The PXR parser: tokens in, [`pxr_ast::Program`] out.
//!
//! The parser is a hand written recursive-descent / precedence-climbing
//! parser. It performs *no* semantic work: unknown names, arity mismatches and
//! type errors are all deferred to `pxr-sema`. When it hits an error it records
//! a diagnostic and resynchronizes at the next statement boundary so that a
//! single run reports as many independent syntax errors as possible.

use pxr_ast::*;
use pxr_diag::{Diagnostic, Diagnostics, Span};
use pxr_lexer::{StrPart, Token, TokenKind};
use std::collections::HashMap;

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    diags: Diagnostics,
    /// Guards against infinite loops during error recovery.
    stuck: usize,
    namespace: String,
    aliases: HashMap<String, String>,
    function_aliases: HashMap<String, String>,
    namespace_style: Option<bool>,
    in_namespace_block: bool,
    global_imports: bool,
    declared_types: Vec<String>,
    declared_functions: Vec<String>,
}

pub fn parse(src: &str) -> (Option<Program>, Diagnostics) {
    parse_at(src, 0)
}

/// Parse with byte offsets in a driver's combined source map.
pub fn parse_at(src: &str, offset: usize) -> (Option<Program>, Diagnostics) {
    let (mut toks, lex_diags) = pxr_lexer::lex(src);
    for token in &mut toks {
        token.span.start += offset;
        token.span.end += offset;
    }
    let mut shifted = Diagnostics::new();
    for diag in lex_diags.items() {
        let mut diag = diag.clone();
        if let Some(span) = &mut diag.span {
            span.start += offset;
            span.end += offset;
        }
        shifted.emit(diag);
    }
    let mut p = Parser {
        toks,
        pos: 0,
        diags: Diagnostics::new(),
        stuck: 0,
        namespace: String::new(),
        aliases: HashMap::new(),
        function_aliases: HashMap::new(),
        namespace_style: None,
        in_namespace_block: false,
        global_imports: false,
        declared_types: Vec::new(),
        declared_functions: Vec::new(),
    };
    let program = p.parse_program();
    let mut diags = shifted;
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
            match self.parse_top_item(&mut items) {
                Ok(()) => {}
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

    fn declared_name(&self, name: &str) -> String {
        if self.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}\\{name}", self.namespace)
        }
    }

    fn resolve_name(&self, name: &str, function: bool) -> (String, Option<String>) {
        if let Some(name) = name.strip_prefix('\\') {
            return (name.to_string(), None);
        }
        if let Some(name) = name.strip_prefix("namespace\\") {
            return (self.declared_name(name), None);
        }
        let (head, tail) = name.split_once('\\').unwrap_or((name, ""));
        if !function
            && tail.is_empty()
            && matches!(
                head.to_ascii_lowercase().as_str(),
                "self" | "parent" | "static"
            )
        {
            return (name.to_string(), None);
        }
        let aliases = if function && tail.is_empty() {
            &self.function_aliases
        } else {
            &self.aliases
        };
        if let Some(alias) = aliases.get(&head.to_ascii_lowercase()) {
            return (
                if tail.is_empty() {
                    alias.clone()
                } else {
                    format!("{alias}\\{tail}")
                },
                None,
            );
        }
        (
            self.declared_name(name),
            if function && tail.is_empty() && !self.namespace.is_empty() {
                Some(name.to_string())
            } else {
                None
            },
        )
    }

    fn parse_name(&mut self) -> Result<(String, Span), ()> {
        let start = self.peek().span;
        let mut name = String::new();
        if self.eat(&TokenKind::Backslash) {
            name.push('\\');
        }
        if self.eat(&TokenKind::Namespace) {
            name.push_str("namespace");
            self.expect(&TokenKind::Backslash, "`\\` after `namespace`")?;
            name.push('\\');
        }
        loop {
            let token = self.expect_ident("name")?;
            let TokenKind::Ident(part) = token.kind else {
                unreachable!()
            };
            name.push_str(&part);
            if !self.eat(&TokenKind::Backslash) {
                return Ok((name, start.merge(token.span)));
            }
            name.push('\\');
        }
    }

    fn parse_top_item(&mut self, items: &mut Vec<Stmt>) -> Result<(), ()> {
        if self.at(&TokenKind::Namespace) && !matches!(self.peek_at(1).kind, TokenKind::Backslash) {
            if self.in_namespace_block
                || (self.namespace_style.is_none() && (!items.is_empty() || self.global_imports))
            {
                self.error(
                    self.peek().span,
                    "namespace declarations must be top-level and precede code",
                );
                return Err(());
            }
            let start = self.advance().span;
            let name = if self.at(&TokenKind::LBrace) {
                String::new()
            } else {
                self.parse_name()?.0
            };
            if name.starts_with('\\') || name.starts_with("namespace\\") {
                self.error(start, "namespace declarations require a relative name");
                return Err(());
            }
            let braced = self.at(&TokenKind::LBrace);
            if self.namespace_style.is_some_and(|style| style != braced) {
                self.error(start, "cannot mix braced and unbraced namespaces");
                return Err(());
            }
            self.namespace_style = Some(braced);
            self.namespace = name;
            self.aliases.clear();
            self.function_aliases.clear();
            self.declared_types.clear();
            self.declared_functions.clear();
            if braced {
                self.advance();
                self.in_namespace_block = true;
                while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
                    let before = self.pos;
                    if self.parse_top_item(items).is_err() {
                        self.recover();
                    }
                    if self.pos == before {
                        self.advance();
                    }
                }
                self.expect(&TokenKind::RBrace, "`}` after namespace body")?;
                self.in_namespace_block = false;
                self.namespace.clear();
                self.aliases.clear();
                self.function_aliases.clear();
            } else {
                self.expect(&TokenKind::Semicolon, "`;` after namespace name")?;
            }
            return Ok(());
        }
        if self.namespace_style == Some(true) && !self.in_namespace_block {
            self.error(self.peek().span, "code must be inside a namespace block");
            return Err(());
        }
        if self.eat(&TokenKind::Use) {
            if self.namespace_style.is_none() {
                self.global_imports = true;
            }
            let function = self.eat(&TokenKind::Function);
            loop {
                let (name, span) = self.parse_name()?;
                if name.starts_with("namespace\\") {
                    self.error(span, "imports must name an absolute symbol");
                    return Err(());
                }
                let name = name.trim_start_matches('\\').to_string();
                let alias = if self.eat(&TokenKind::As) {
                    let token = self.expect_ident("import alias")?;
                    let TokenKind::Ident(alias) = token.kind else {
                        unreachable!()
                    };
                    alias
                } else {
                    name.rsplit('\\').next().unwrap().to_string()
                };
                let key = alias.to_ascii_lowercase();
                let declared = if function {
                    &self.declared_functions
                } else {
                    &self.declared_types
                };
                let aliases = if function {
                    &mut self.function_aliases
                } else {
                    &mut self.aliases
                };
                if declared.contains(&key) || aliases.insert(key, name).is_some() {
                    self.error(span, format!("import alias `{alias}` is already in use"));
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::Semicolon, "`;` after import")?;
            return Ok(());
        }
        if let Some(stmt) = self.parse_stmt()? {
            items.push(stmt);
        }
        Ok(())
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
            TokenKind::Class | TokenKind::Abstract | TokenKind::Interface => self
                .parse_class()
                .map(|c| Some(Stmt::ClassDecl(Box::new(c)))),
            TokenKind::Require | TokenKind::RequireOnce => {
                let once = self.advance().kind == TokenKind::RequireOnce;
                let paren = self.eat(&TokenKind::LParen);
                let dir_relative = matches!(&self.peek().kind, TokenKind::Ident(name) if name.eq_ignore_ascii_case("__DIR__"));
                if dir_relative {
                    self.advance();
                    self.expect(&TokenKind::Dot, "`.` after `__DIR__`")?;
                }
                let token = self.advance();
                let TokenKind::Str(bytes) = token.kind else {
                    self.error(
                        token.span,
                        "module paths must be literal strings, optionally prefixed by `__DIR__ .`",
                    );
                    return Err(());
                };
                let path = String::from_utf8(bytes)
                    .map_err(|_| self.error(token.span, "module paths must be UTF-8"))?;
                if paren {
                    self.expect(&TokenKind::RParen, "`)` after module path")?;
                }
                let end = self.expect(&TokenKind::Semicolon, "`;` after module dependency")?;
                Ok(Some(Stmt::Require {
                    path,
                    once,
                    dir_relative,
                    span: start.merge(end.span),
                }))
            }
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
        if self
            .function_aliases
            .contains_key(&name.to_ascii_lowercase())
        {
            self.error(name_span, "function name conflicts with an import");
        }
        self.declared_functions.push(name.to_ascii_lowercase());
        let name = self.declared_name(&name);

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

    fn parse_class(&mut self) -> Result<ClassDecl, ()> {
        let start = self.peek().span;
        let explicit_abstract = self.eat(&TokenKind::Abstract);
        let is_interface = self.eat(&TokenKind::Interface);
        if is_interface && explicit_abstract {
            self.error(start, "interfaces cannot have an `abstract` modifier");
            return Err(());
        }
        let is_abstract = explicit_abstract || is_interface;
        if !is_interface {
            self.expect(&TokenKind::Class, "`class` after `abstract`")?;
        }
        let name_tok = self.expect_ident("class name")?;
        let name = match name_tok.kind {
            TokenKind::Ident(n) => n,
            _ => unreachable!(),
        };
        let name_span = name_tok.span;
        if self.aliases.contains_key(&name.to_ascii_lowercase()) {
            self.error(name_span, "type name conflicts with an import");
        }
        self.declared_types.push(name.to_ascii_lowercase());
        let name = self.declared_name(&name);
        let mut extends = None;
        let mut extends_span = None;
        let mut interfaces = Vec::new();
        if is_interface {
            if self.eat(&TokenKind::Extends) {
                interfaces = self.parse_interface_names()?;
            }
        } else if self.eat(&TokenKind::Extends) {
            let (parent_name, span) = self.parse_name()?;
            extends = Some(self.resolve_name(&parent_name, false).0);
            extends_span = Some(span);
        }
        if !is_interface && self.eat(&TokenKind::Implements) {
            interfaces = self.parse_interface_names()?;
        }
        self.expect(&TokenKind::LBrace, "`{` after class header")?;
        let mut members = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            let before = self.pos;
            match self.parse_class_member(is_interface) {
                Ok(m) => members.push(m),
                Err(()) => self.recover(),
            }
            if self.pos == before {
                self.advance();
            }
        }
        let close = self.expect(&TokenKind::RBrace, "`}` after class body")?;
        Ok(ClassDecl {
            is_interface,
            interfaces,
            is_abstract,
            name,
            name_span,
            extends,
            extends_span,
            members,
            span: start.merge(close.span),
        })
    }

    fn parse_interface_names(&mut self) -> Result<Vec<(String, Span)>, ()> {
        let mut names = Vec::new();
        loop {
            let (name, span) = self.parse_name()?;
            names.push((self.resolve_name(&name, false).0, span));
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Ok(names)
    }

    fn parse_class_member(&mut self, is_interface: bool) -> Result<ClassMember, ()> {
        let start = self.peek().span;
        let mut is_abstract = false;
        let mut is_static = false;
        let mut vis = Visibility::Public;
        let mut has_vis = false;
        loop {
            match self.peek().kind {
                TokenKind::Abstract if !is_abstract => {
                    is_abstract = true;
                    self.advance();
                }
                TokenKind::Static if !is_static => {
                    is_static = true;
                    self.advance();
                }
                TokenKind::Public | TokenKind::Protected | TokenKind::Private if !has_vis => {
                    vis = match self.advance().kind {
                        TokenKind::Protected => Visibility::Protected,
                        TokenKind::Private => Visibility::Private,
                        _ => Visibility::Public,
                    };
                    has_vis = true;
                }
                _ => break,
            }
        }
        if is_interface && is_abstract {
            self.error(start, "interface methods are implicitly abstract");
            return Err(());
        }
        if self.at(&TokenKind::Function) {
            let m = self.parse_method(start, vis, is_abstract || is_interface, is_static)?;
            return Ok(ClassMember::Method(m));
        }

        if is_abstract || is_interface {
            self.error(
                start,
                "expected an interface method signature or class method",
            );
            return Err(());
        }
        if !has_vis && !is_static {
            let span = self.peek().span;
            self.error(
                span,
                "expected visibility (`public`, `protected`, or `private`) or `function`",
            );
            return Err(());
        }

        // Property declaration:
        // Optional type, then $var, then optional = expr, then ;
        let ty = if !matches!(self.peek().kind, TokenKind::Variable(_)) && self.is_type_start() {
            Some(self.parse_type()?)
        } else {
            None
        };

        let vt = self.expect_variable("property name")?;
        let name = match vt.kind {
            TokenKind::Variable(n) => n,
            _ => unreachable!(),
        };
        let name_span = vt.span;

        let default = if self.eat(&TokenKind::Assign) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let semi = self.expect(&TokenKind::Semicolon, "`;` after property declaration")?;
        Ok(ClassMember::Property(PropertyDecl {
            is_static,
            visibility: vis,
            ty,
            name,
            name_span,
            default,
            span: start.merge(semi.span),
        }))
    }

    fn parse_method(
        &mut self,
        start: Span,
        visibility: Visibility,
        is_abstract: bool,
        is_static: bool,
    ) -> Result<MethodDecl, ()> {
        self.expect(&TokenKind::Function, "`function`")?;
        let name_tok = self.expect_ident("method name")?;
        let name = match name_tok.kind {
            TokenKind::Ident(n) => n,
            _ => unreachable!(),
        };
        let name_span = name_tok.span;

        self.expect(&TokenKind::LParen, "`(` after method name")?;
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

        let body = if is_abstract {
            self.expect(&TokenKind::Semicolon, "`;` after abstract method signature")?;
            Vec::new()
        } else {
            self.parse_block()?
        };
        let end = self.peek().span;
        Ok(MethodDecl {
            is_static,
            is_abstract,
            visibility,
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
                | TokenKind::KwMixed
                | TokenKind::KwArray
                | TokenKind::Ident(_)
                | TokenKind::Backslash
                | TokenKind::Namespace
        )
    }

    fn parse_type(&mut self) -> Result<TypeRef, ()> {
        if matches!(
            self.peek().kind,
            TokenKind::Ident(_) | TokenKind::Backslash | TokenKind::Namespace
        ) {
            let (name, _) = self.parse_name()?;
            return Ok(TypeRef::Named(self.resolve_name(&name, false).0));
        }

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
            TokenKind::KwMixed => TypeRef::Mixed,
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
            Expr::StaticProperty {
                class_name,
                name,
                span,
            } => Ok(LValue::StaticProperty {
                class_name,
                name,
                span,
            }),
            Expr::Var(name, span) => Ok(LValue::Var { name, span }),
            Expr::Index { base, index, span } => {
                let base = self.expr_to_lvalue(*base)?;
                Ok(LValue::Index {
                    base: Box::new(base),
                    index,
                    span,
                })
            }
            Expr::Property {
                base,
                name,
                name_span,
                span,
            } => Ok(LValue::Property {
                base,
                name,
                name_span,
                span,
            }),
            other => {
                let span = other.span();
                self.error(
                    span,
                    "the left-hand side of an assignment must be a variable, array element, or property",
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
            if 7 >= min_prec && self.at(&TokenKind::Instanceof) {
                self.advance();
                let (raw_name, class_span) = self.parse_name()?;
                let class_name = self.resolve_name(&raw_name, false).0;
                let span = lhs.span().merge(class_span);
                lhs = Expr::Instanceof {
                    expr: Box::new(lhs),
                    class_name,
                    class_span,
                    span,
                };
                continue;
            }
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

    /// A primary expression followed by any number of `[...]` index accesses and `->` member accesses.
    fn parse_postfix(&mut self) -> Result<Expr, ()> {
        let mut base = self.parse_primary()?;
        loop {
            if self.at(&TokenKind::LBracket) {
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
            } else if self.at(&TokenKind::Arrow) {
                self.advance();
                let name_tok = self.expect_ident("property or method name after `->`")?;
                let name = match name_tok.kind {
                    TokenKind::Ident(n) => n,
                    _ => unreachable!(),
                };
                let name_span = name_tok.span;
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    let close = self.expect(&TokenKind::RParen, "`)` after method arguments")?;
                    let span = base.span().merge(close.span);
                    base = Expr::MethodCall {
                        base: Box::new(base),
                        name,
                        name_span,
                        args,
                        span,
                    };
                } else {
                    let span = base.span().merge(name_span);
                    base = Expr::Property {
                        base: Box::new(base),
                        name,
                        name_span,
                        span,
                    };
                }
            } else {
                break;
            }
        }
        Ok(base)
    }

    fn parse_qualified_member(&mut self, class_name: String, start: Span) -> Result<Expr, ()> {
        self.expect(&TokenKind::DoubleColon, "`::` after class name")?;
        if matches!(self.peek().kind, TokenKind::Variable(_)) {
            let token = self.advance();
            let TokenKind::Variable(name) = token.kind else {
                unreachable!()
            };
            return Ok(Expr::StaticProperty {
                class_name,
                name,
                span: start.merge(token.span),
            });
        }
        let token = self.expect_ident("method name after `::`")?;
        let TokenKind::Ident(method) = token.kind else {
            unreachable!()
        };
        self.expect(&TokenKind::LParen, "`(` after method name")?;
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
        // Keep the existing AST shape for parent calls for downstream consumers.
        if class_name == "parent" {
            Ok(Expr::ParentMethodCall {
                method,
                method_span: token.span,
                args,
                span: start.merge(close.span),
            })
        } else {
            Ok(Expr::StaticCall {
                class_name,
                method,
                args,
                span: start.merge(close.span),
            })
        }
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
            TokenKind::InterpStr(parts) => {
                self.advance();
                Ok(interpolated_string(parts, span))
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
            TokenKind::New => {
                self.advance();
                let (raw_name, class_span) = self.parse_name()?;
                let class_name = self.resolve_name(&raw_name, false).0;
                let (args, end_span) = if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    let close =
                        self.expect(&TokenKind::RParen, "`)` after constructor arguments")?;
                    (args, close.span)
                } else {
                    (Vec::new(), class_span)
                };
                Ok(Expr::New {
                    class_name,
                    class_span,
                    args,
                    span: span.merge(end_span),
                })
            }
            TokenKind::LBracket => {
                self.parse_array_literal(TokenKind::LBracket, TokenKind::RBracket)
            }
            TokenKind::KwArray if matches!(self.peek_at(1).kind, TokenKind::LParen) => {
                self.parse_array_literal(TokenKind::LParen, TokenKind::RParen)
            }
            TokenKind::Match => self.parse_match(),
            TokenKind::Parent | TokenKind::Static => {
                let class_name = if self.at(&TokenKind::Parent) {
                    "parent"
                } else {
                    "static"
                }
                .to_string();
                self.advance();
                self.parse_qualified_member(class_name, span)
            }
            TokenKind::Ident(_) | TokenKind::Backslash | TokenKind::Namespace => {
                let (name, _) = self.parse_name()?;
                if self.at(&TokenKind::DoubleColon) {
                    let name = self.resolve_name(&name, false).0;
                    return self.parse_qualified_member(name, span);
                }
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
                    let (callee, fallback) = self.resolve_name(&name, true);
                    Ok(Expr::Call {
                        fallback,
                        callee,
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
    s.span()
}

fn set_stmt_span(s: &mut Stmt, span: Span) {
    match s {
        Stmt::Echo(_, sp)
        | Stmt::Print(_, sp)
        | Stmt::Expr(_, sp)
        | Stmt::Return(_, sp)
        | Stmt::Break(_, sp)
        | Stmt::Continue(_, sp) => *sp = span,
        Stmt::Require { span: sp, .. }
        | Stmt::Assign { span: sp, .. }
        | Stmt::AssignOp { span: sp, .. }
        | Stmt::IncDec { span: sp, .. }
        | Stmt::If { span: sp, .. }
        | Stmt::While { span: sp, .. }
        | Stmt::For { span: sp, .. }
        | Stmt::DoWhile { span: sp, .. }
        | Stmt::Foreach { span: sp, .. }
        | Stmt::Switch { span: sp, .. }
        | Stmt::Block(_, sp) => *sp = span,
        Stmt::FnDecl(_) | Stmt::ClassDecl(_) | Stmt::Empty => {}
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

/// Desugar a `"a$x b"`-style literal into a left-associative concatenation.
///
/// The chain always begins from an empty string literal so the expression is
/// string-typed and the first dynamic part undergoes exactly the same
/// conversion as PHP's `"$x"` (e.g. `true` -> `"1"`, `5` -> `"5"`).
fn interpolated_string(parts: Vec<StrPart>, span: Span) -> Expr {
    let mut acc = Expr::StrLit(Vec::new(), span);
    for part in parts {
        let rhs = match part {
            StrPart::Lit(bytes) => Expr::StrLit(bytes, span),
            StrPart::Var(name) => Expr::Var(name, span),
        };
        acc = Expr::Binary {
            op: BinOp::Concat,
            lhs: Box::new(acc),
            rhs: Box::new(rhs),
            span,
        };
    }
    acc
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

    #[test]
    fn class_declaration_and_objects() {
        let p = ok("<?php
            class Counter {
                public int $val;
                private string $name = \"test\";

                public function __construct(int $initial) {
                    $this->val = $initial;
                }

                public function inc(): int {
                    $this->val += 1;
                    return $this->val;
                }
            }
            $c = new Counter(10);
            $c->inc();
            echo $c->val;
        ");
        assert_eq!(p.items.len(), 4);
        match &p.items[0] {
            Stmt::ClassDecl(c) => {
                assert_eq!(c.name, "Counter");
                assert_eq!(c.members.len(), 4);
                match &c.members[0] {
                    ClassMember::Property(prop) => {
                        assert_eq!(prop.name, "val");
                        assert_eq!(prop.visibility, Visibility::Public);
                        assert_eq!(prop.ty, Some(TypeRef::Int));
                        assert!(prop.default.is_none());
                    }
                    _ => panic!("expected property"),
                }
                match &c.members[1] {
                    ClassMember::Property(prop) => {
                        assert_eq!(prop.name, "name");
                        assert_eq!(prop.visibility, Visibility::Private);
                        assert_eq!(prop.ty, Some(TypeRef::String));
                        assert!(prop.default.is_some());
                    }
                    _ => panic!("expected property"),
                }
                match &c.members[2] {
                    ClassMember::Method(m) => {
                        assert_eq!(m.name, "__construct");
                        assert_eq!(m.params.len(), 1);
                        assert_eq!(m.visibility, Visibility::Public);
                    }
                    _ => panic!("expected method"),
                }
                match &c.members[3] {
                    ClassMember::Method(m) => {
                        assert_eq!(m.name, "inc");
                        assert_eq!(m.visibility, Visibility::Public);
                        assert_eq!(m.ret, Some(TypeRef::Int));
                    }
                    _ => panic!("expected method"),
                }
            }
            _ => panic!("expected class decl"),
        }
    }

    #[test]
    fn class_inheritance_and_instanceof() {
        let p = ok("<?php
            class Dog extends Animal {
                public function speak(): string {
                    return parent::speak() . \" woof\";
                }
            }
            $d = new Dog();
            $ok = $d instanceof Animal;
        ");
        assert_eq!(p.items.len(), 3);
        match &p.items[0] {
            Stmt::ClassDecl(c) => {
                assert_eq!(c.name, "Dog");
                assert_eq!(c.extends, Some("Animal".into()));
                match &c.members[0] {
                    ClassMember::Method(m) => {
                        assert_eq!(m.name, "speak");
                        assert_eq!(m.body.len(), 1);
                    }
                    _ => panic!("expected method"),
                }
            }
            _ => panic!("expected class decl"),
        }
    }
}
