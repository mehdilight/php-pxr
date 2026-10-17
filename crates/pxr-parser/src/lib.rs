//! The PXR parser: tokens in, [`pxr_ast::Program`] out.
//!
//! The parser is a hand written recursive-descent / precedence-climbing
//! parser. It performs *no* semantic work: unknown names, arity mismatches and
//! type errors are all deferred to `pxr-sema`. When it hits an error it records
//! a diagnostic and resynchronizes at the next statement boundary so that a
//! single run reports as many independent syntax errors as possible.

mod closures;

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
    type_params: Vec<String>,
    future_type_params: Vec<String>,
    generated_items: Vec<Stmt>,
    in_method: bool,
    method_static: bool,
    method_generic: bool,
    closure_class: Option<closures::ClassScope>,
}

pub fn parse(src: &str) -> (Option<Program>, Diagnostics) {
    parse_at(src, 0)
}

/// Parse with byte offsets in a driver's combined source map.
pub fn parse_at(src: &str, offset: usize) -> (Option<Program>, Diagnostics) {
    let (mut toks, lex_diags) = pxr_lexer::lex(src);
    for token in &mut toks {
        token.shift(offset);
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
        type_params: Vec::new(),
        future_type_params: Vec::new(),
        generated_items: Vec::new(),
        in_method: false,
        method_static: false,
        method_generic: false,
        closure_class: None,
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
        items.append(&mut self.generated_items);
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
            TokenKind::Function if self.peek_at(1).kind == TokenKind::LParen => {
                self.parse_simple().map(Some)
            }
            TokenKind::Function | TokenKind::Async => {
                self.parse_fn().map(|f| Some(Stmt::FnDecl(Box::new(f))))
            }
            TokenKind::Class | TokenKind::Abstract | TokenKind::Interface | TokenKind::Trait => {
                self.parse_class()
                    .map(|c| Some(Stmt::ClassDecl(Box::new(c))))
            }
            TokenKind::Require
            | TokenKind::RequireOnce
            | TokenKind::Include
            | TokenKind::IncludeOnce => {
                let kind = self.advance().kind;
                let once = matches!(kind, TokenKind::RequireOnce | TokenKind::IncludeOnce);
                let required = matches!(kind, TokenKind::Require | TokenKind::RequireOnce);
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
                    required,
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
        let previous = std::mem::take(&mut self.type_params);
        let is_async = self.eat(&TokenKind::Async);
        let result = self.parse_fn_inner().map(|mut f| {
            f.is_async = is_async;
            f
        });
        self.type_params = previous;
        result
    }

    fn parse_fn_inner(&mut self) -> Result<FnDecl, ()> {
        let start = self
            .expect(&TokenKind::Function, "`function` after async")?
            .span;
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
        let (type_params, bounds) = self.parse_type_parameters()?;

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
            is_async: false,
            type_params,
            bounds,
            name,
            name_span,
            params,
            ret,
            body,
            span: start.merge(end),
        })
    }

    fn parse_class(&mut self) -> Result<ClassDecl, ()> {
        let previous = std::mem::take(&mut self.type_params);
        let previous_scope = self.closure_class.take();
        let result = self.parse_class_inner();
        self.type_params = previous;
        self.closure_class = previous_scope;
        result
    }

    fn parse_class_inner(&mut self) -> Result<ClassDecl, ()> {
        let start = self.peek().span;
        let explicit_abstract = self.eat(&TokenKind::Abstract);
        let is_interface = self.eat(&TokenKind::Interface);
        let is_trait = self.eat(&TokenKind::Trait);
        if is_interface && explicit_abstract {
            self.error(start, "interfaces cannot have an `abstract` modifier");
            return Err(());
        }
        let is_abstract = explicit_abstract || is_interface;
        if !is_interface && !is_trait {
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
        let (type_params, bounds) = self.parse_type_parameters()?;

        let mut extends = None;
        let mut interfaces = Vec::new();
        if is_interface {
            if self.eat(&TokenKind::Extends) {
                interfaces = self.parse_interface_names()?;
            }
        } else if self.eat(&TokenKind::Extends) {
            extends = Some(self.parse_class_ref()?);
        }
        if !is_interface && self.eat(&TokenKind::Implements) {
            interfaces = self.parse_interface_names()?;
        }
        self.closure_class = Some(closures::ClassScope {
            name: name.clone(),
            generic: !type_params.is_empty(),
            is_trait,
            parent: extends.clone(),
            methods: Vec::new(),
        });
        self.expect(&TokenKind::LBrace, "`{` after class header")?;
        let mut members = Vec::new();
        let mut trait_uses = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.eat(&TokenKind::Use) {
                trait_uses.extend(self.parse_interface_names()?);
                self.expect(&TokenKind::Semicolon, "`;` after trait use")?;
                continue;
            }
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
        if let Some(scope) = &mut self.closure_class {
            members.extend(
                std::mem::take(&mut scope.methods)
                    .into_iter()
                    .map(ClassMember::Method),
            );
        }
        Ok(ClassDecl {
            is_trait,
            trait_uses,
            type_params,
            bounds,
            is_interface,
            interfaces,
            is_abstract,
            name,
            name_span,
            extends,
            members,
            span: start.merge(close.span),
        })
    }

    fn parse_class_ref(&mut self) -> Result<ClassRef, ()> {
        let (name, span) = self.parse_name()?;
        let name = self.resolve_name(&name, false).0;
        let type_args = self.parse_type_arguments()?;
        Ok(ClassRef {
            name,
            type_args,
            span,
        })
    }

    fn parse_interface_names(&mut self) -> Result<Vec<ClassRef>, ()> {
        let mut names = Vec::new();
        loop {
            names.push(self.parse_class_ref()?);
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
        let mut is_async = false;
        let mut vis = Visibility::Public;
        let mut has_vis = false;
        loop {
            match self.peek().kind {
                TokenKind::Async if !is_async => {
                    is_async = true;
                    self.advance();
                }
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
            let mut m = self.parse_method(start, vis, is_abstract || is_interface, is_static)?;
            m.is_async = is_async;
            return Ok(ClassMember::Method(m));
        }

        if is_abstract || is_interface || is_async {
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
        let previous = self.type_params.clone();
        let result = self.parse_method_inner(start, visibility, is_abstract, is_static);
        self.type_params = previous;
        result
    }

    fn parse_method_inner(
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

        let (type_params, bounds) = self.parse_type_parameters()?;

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
            let previous = std::mem::replace(&mut self.in_method, true);
            let previous_static = std::mem::replace(&mut self.method_static, is_static);
            let previous_generic =
                std::mem::replace(&mut self.method_generic, !type_params.is_empty());
            let result = self.parse_block();
            self.in_method = previous;
            self.method_static = previous_static;
            self.method_generic = previous_generic;
            result?
        };
        let end = self.peek().span;
        Ok(MethodDecl {
            is_async: false,
            type_params,
            bounds,
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
            TokenKind::Amp
                | TokenKind::KwInt
                | TokenKind::KwFloat
                | TokenKind::KwString
                | TokenKind::KwBool
                | TokenKind::KwVoid
                | TokenKind::KwMixed
                | TokenKind::KwArray
                | TokenKind::Ident(_)
                | TokenKind::Backslash
                | TokenKind::Namespace
                | TokenKind::Parent
                | TokenKind::Static
        )
    }

    /// Split adjacent closers in type contexts without changing expression shifts.
    fn expect_type_close(&mut self) -> Result<Token, ()> {
        let token = self.peek().clone();
        let remainder = match token.kind {
            TokenKind::Shr => Some(TokenKind::Gt),
            TokenKind::GtEq => Some(TokenKind::Assign),
            TokenKind::ShrEq => Some(TokenKind::GtEq),
            _ => None,
        };
        if let Some(kind) = remainder {
            self.toks[self.pos] = Token {
                kind,
                span: Span::new(token.span.start + 1, token.span.end),
            };
            return Ok(Token {
                kind: TokenKind::Gt,
                span: Span::new(token.span.start, token.span.start + 1),
            });
        }
        self.expect(&TokenKind::Gt, "`>` closing type arguments")
    }

    fn parse_type(&mut self) -> Result<TypeRef, ()> {
        if matches!(self.peek().kind, TokenKind::Parent | TokenKind::Static) {
            let token = self.advance();
            let name = if token.kind == TokenKind::Parent {
                "parent"
            } else {
                "static"
            };
            let args = self.parse_type_arguments()?;
            return Ok(if args.is_empty() {
                TypeRef::Named(name.into())
            } else {
                TypeRef::Applied(name.into(), args, token.span)
            });
        }
        if self.at(&TokenKind::Amp) {
            let start = self.advance().span;
            let mutable = matches!(&self.peek().kind, TokenKind::Ident(n) if n == "mut");
            if mutable {
                self.advance();
            }
            let inner = self.parse_type()?;
            return Ok(TypeRef::Applied(
                if mutable {
                    "$pxr_borrow_mut"
                } else {
                    "$pxr_borrow"
                }
                .into(),
                vec![inner],
                start,
            ));
        }
        if matches!(
            self.peek().kind,
            TokenKind::Ident(_) | TokenKind::Backslash | TokenKind::Namespace
        ) {
            let (name, span) = self.parse_name()?;
            if self.type_params.contains(&name) || self.future_type_params.contains(&name) {
                return Ok(TypeRef::Param(name, span));
            }
            let name = if name.eq_ignore_ascii_case("owned") {
                "$pxr_owned".into()
            } else {
                self.resolve_name(&name, false).0
            };
            let args = self.parse_type_arguments()?;
            return Ok(if args.is_empty() {
                TypeRef::Named(name)
            } else {
                TypeRef::Applied(name, args, span)
            });
        }

        if self.at(&TokenKind::KwArray) {
            let start = self.advance().span;
            self.expect(&TokenKind::Lt, "`<` after `array` (write `array<T>`)")?;
            let inner = self.parse_type()?;
            let close = self.expect_type_close()?;
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

    fn parse_type_arguments(&mut self) -> Result<Vec<TypeRef>, ()> {
        let mut arguments = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                arguments.push(self.parse_type()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect_type_close()?;
        }
        Ok(arguments)
    }

    fn parse_type_parameters(
        &mut self,
    ) -> Result<(Vec<(String, Span)>, Vec<(String, Vec<TypeRef>)>), ()> {
        let previous = std::mem::take(&mut self.future_type_params);
        if self.at(&TokenKind::Lt) {
            let mut depth = 1i32;
            let mut parameter = true;
            for token in self.toks.iter().skip(self.pos + 1) {
                match &token.kind {
                    TokenKind::Ident(name) if depth == 1 && parameter => {
                        self.future_type_params.push(name.clone());
                        parameter = false;
                    }
                    TokenKind::Lt => depth += 1,
                    TokenKind::Gt | TokenKind::GtEq => depth -= 1,
                    TokenKind::Shr | TokenKind::ShrEq => depth -= 2,
                    TokenKind::Comma if depth == 1 => parameter = true,
                    _ => {}
                }
                if depth <= 0 {
                    break;
                }
            }
        }
        let result = self.parse_type_parameters_inner();
        self.future_type_params = previous;
        result
    }

    fn parse_type_parameters_inner(
        &mut self,
    ) -> Result<(Vec<(String, Span)>, Vec<(String, Vec<TypeRef>)>), ()> {
        let mut parameters = Vec::new();
        let mut bounds = Vec::new();
        if self.eat(&TokenKind::Lt) {
            loop {
                let token = self.expect_ident("type parameter")?;
                let TokenKind::Ident(name) = token.kind else {
                    unreachable!()
                };
                if self.type_params.contains(&name) {
                    self.error(
                        token.span,
                        format!("duplicate or shadowed type parameter `{name}`"),
                    );
                }
                self.type_params.push(name.clone());
                parameters.push((name.clone(), token.span));
                if self.eat(&TokenKind::Colon) {
                    let mut requirements = vec![self.parse_type()?];
                    while self.eat(&TokenKind::Amp) {
                        requirements.push(self.parse_type()?);
                    }
                    bounds.push((name, requirements));
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect_type_close()?;
        }
        Ok((parameters, bounds))
    }

    // `<` after an instanceof target or property can begin a comparison.
    // Restore split `>>` tokens as well as diagnostics when speculation fails.
    fn try_parse_type_arguments(&mut self) -> Vec<TypeRef> {
        if !self.at(&TokenKind::Lt) {
            return Vec::new();
        }
        let previous = (self.pos, self.toks.clone(), self.diags.clone());
        let arguments = self.parse_type_arguments();
        if self.diags.items().len() == previous.2.items().len() {
            if let Ok(arguments) = arguments {
                return arguments;
            }
        }
        (self.pos, self.toks, self.diags) = previous;
        Vec::new()
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

        let expression = self.parse_expr()?;
        Ok(match expression {
            Expr::Effect { statement, .. } => *statement,
            other => Stmt::Expr(other, start),
        })
    }

    /// Convert a parsed expression to an assignment target, reporting an error
    /// for non-lvalues (e.g. `1 = 2`).
    fn expr_to_lvalue(&mut self, e: Expr) -> Result<LValue, ()> {
        match e {
            Expr::StaticProperty {
                class_name,
                type_args,
                name,
                span,
            } => Ok(LValue::StaticProperty {
                class_name,
                type_args,
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
        let lhs = self.parse_ternary()?;
        self.parse_assignment_tail(lhs)
    }

    // PHP permits a writable operand to start an assignment even when it is
    // the RHS of a tighter-binding operator (`1 + $x = 2`, `$a ?? $b = 3`).
    fn parse_assignment_tail(&mut self, lhs: Expr) -> Result<Expr, ()> {
        let op = match self.peek().kind {
            TokenKind::Assign => None,
            TokenKind::PlusEq => Some(BinOp::Add),
            TokenKind::MinusEq => Some(BinOp::Sub),
            TokenKind::StarEq => Some(BinOp::Mul),
            TokenKind::SlashEq => Some(BinOp::Div),
            TokenKind::PercentEq => Some(BinOp::Mod),
            TokenKind::DotEq => Some(BinOp::Concat),
            TokenKind::AmpEq => Some(BinOp::BitAnd),
            TokenKind::PipeEq => Some(BinOp::BitOr),
            TokenKind::CaretEq => Some(BinOp::BitXor),
            TokenKind::ShlEq => Some(BinOp::Shl),
            TokenKind::ShrEq => Some(BinOp::Shr),
            TokenKind::CoalesceEq => Some(BinOp::NullCoalesce),
            _ => return Ok(lhs),
        };
        self.advance();
        let rhs = self.parse_expr()?;
        let span = lhs.span().merge(rhs.span());
        let target = self.expr_to_lvalue(lhs)?;
        let statement = match op {
            None => Stmt::Assign {
                target,
                ty: None,
                value: rhs,
                span,
            },
            Some(op) => Stmt::AssignOp {
                target,
                op,
                value: rhs,
                span,
            },
        };
        Ok(Expr::Effect {
            statement: Box::new(statement),
            postfix: false,
            span,
        })
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
                let type_args = self.try_parse_type_arguments();
                let span = lhs.span().merge(class_span);
                lhs = Expr::Instanceof {
                    expr: Box::new(lhs),
                    class_name,
                    type_args,
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
            TokenKind::PlusPlus | TokenKind::MinusMinus => {
                let delta = if self.advance().kind == TokenKind::PlusPlus {
                    1
                } else {
                    -1
                };
                let expr = self.parse_unary()?;
                let span = start.merge(expr.span());
                let target = self.expr_to_lvalue(expr)?;
                Ok(Expr::Effect {
                    statement: Box::new(Stmt::IncDec {
                        target,
                        delta,
                        span,
                    }),
                    postfix: false,
                    span,
                })
            }
            TokenKind::Await | TokenKind::Spawn => {
                let op = self.advance();
                let value = self.parse_unary()?;
                let span = start.merge(value.span());
                Ok(Expr::Call {
                    callee: if op.kind == TokenKind::Await {
                        "$pxr_await"
                    } else {
                        "$pxr_spawn"
                    }
                    .into(),
                    fallback: None,
                    callee_span: start,
                    type_args: vec![],
                    args: vec![value],
                    span,
                })
            }
            TokenKind::Amp => {
                self.advance();
                let mutable = matches!(&self.peek().kind, TokenKind::Ident(n) if n == "mut");
                if mutable {
                    self.advance();
                }
                let value = self.parse_unary()?;
                let span = start.merge(value.span());
                Ok(Expr::Call {
                    fallback: None,
                    callee: if mutable {
                        "$pxr_borrow_mut"
                    } else {
                        "$pxr_borrow"
                    }
                    .into(),
                    callee_span: start,
                    type_args: vec![],
                    args: vec![value],
                    span,
                })
            }
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
            if self.eat(&TokenKind::LParen) {
                let name_span = base.span();
                let mut args = Vec::new();
                if !self.at(&TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let close = self.expect(&TokenKind::RParen, "`)` after callable arguments")?;
                let span = base.span().merge(close.span);
                // Invokable objects share ordinary checked method dispatch,
                // receiver evaluation, ownership and GC handling.
                base = Expr::MethodCall {
                    base: Box::new(base),
                    name: "__invoke".into(),
                    name_span,
                    type_args: Vec::new(),
                    args,
                    span,
                };
            } else if self.at(&TokenKind::LBracket) {
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
                let type_args = if self.at(&TokenKind::Lt) {
                    self.try_parse_type_arguments()
                } else {
                    Vec::new()
                };
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
                        type_args,
                        name_span,
                        args,
                        span,
                    };
                } else {
                    if !type_args.is_empty() {
                        self.error(name_span, "type arguments require a method call");
                        return Err(());
                    }
                    let span = base.span().merge(name_span);
                    base = Expr::Property {
                        base: Box::new(base),
                        name,
                        name_span,
                        span,
                    };
                }
            } else if matches!(
                self.peek().kind,
                TokenKind::PlusPlus | TokenKind::MinusMinus
            ) {
                let token = self.advance();
                let delta = if token.kind == TokenKind::PlusPlus {
                    1
                } else {
                    -1
                };
                let span = base.span().merge(token.span);
                let target = self.expr_to_lvalue(base)?;
                return Ok(Expr::Effect {
                    statement: Box::new(Stmt::IncDec {
                        target,
                        delta,
                        span,
                    }),
                    postfix: true,
                    span,
                });
            } else {
                break;
            }
        }
        self.parse_assignment_tail(base)
    }

    fn parse_qualified_member(
        &mut self,
        class_name: String,
        type_args: Vec<TypeRef>,
        start: Span,
    ) -> Result<Expr, ()> {
        self.expect(&TokenKind::DoubleColon, "`::` after class name")?;
        if matches!(self.peek().kind, TokenKind::Variable(_)) {
            let token = self.advance();
            let TokenKind::Variable(name) = token.kind else {
                unreachable!()
            };
            return Ok(Expr::StaticProperty {
                class_name,
                type_args,
                name,
                span: start.merge(token.span),
            });
        }
        let token = self.expect_ident("method name after `::`")?;
        let TokenKind::Ident(method) = token.kind else {
            unreachable!()
        };
        let method_type_args = self.parse_type_arguments()?;
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
                type_args: method_type_args,
                method_span: token.span,
                args,
                span: start.merge(close.span),
            })
        } else {
            Ok(Expr::StaticCall {
                class_name,
                type_args,
                method,
                method_type_args,
                args,
                span: start.merge(close.span),
            })
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        let tok = self.peek().clone();
        let span = tok.span;
        match tok.kind {
            TokenKind::Function | TokenKind::Fn => self.parse_closure(false),
            TokenKind::Static
                if matches!(self.peek_at(1).kind, TokenKind::Function | TokenKind::Fn) =>
            {
                self.advance();
                self.parse_closure(true)
            }
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
                self.interpolated_string(parts, span)
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
                let type_args = self.parse_type_arguments()?;
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
                    type_args,
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
                self.parse_qualified_member(class_name, Vec::new(), span)
            }
            TokenKind::Ident(_) | TokenKind::Backslash | TokenKind::Namespace => {
                let (name, _) = self.parse_name()?;
                if self.at(&TokenKind::DoubleColon) {
                    let name = self.resolve_name(&name, false).0;
                    return self.parse_qualified_member(name, Vec::new(), span);
                }
                let type_args = self.parse_type_arguments()?;
                if self.at(&TokenKind::DoubleColon) {
                    let name = self.resolve_name(&name, false).0;
                    return self.parse_qualified_member(name, type_args, span);
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
                    let (callee, fallback) =
                        if name.eq_ignore_ascii_case("move") || name.eq_ignore_ascii_case("drop") {
                            (format!("$pxr_{}", name.to_ascii_lowercase()), None)
                        } else {
                            self.resolve_name(&name, true)
                        };
                    Ok(Expr::Call {
                        fallback,
                        callee,
                        callee_span: span,
                        type_args,
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
        | Stmt::ForwardReturn(_, sp)
        | Stmt::Break(_, sp)
        | Stmt::Continue(_, sp) => *sp = span,
        Stmt::PhpInclude { span: sp, .. }
        | Stmt::Require { span: sp, .. }
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
impl Parser {
    fn interpolated_string(&mut self, parts: Vec<StrPart>, span: Span) -> Result<Expr, ()> {
        let mut acc = Expr::StrLit(Vec::new(), span);
        for part in parts {
            let rhs = match part {
                StrPart::Lit(bytes) => Expr::StrLit(bytes, span),
                StrPart::Var(name) => Expr::Var(name, span),
                StrPart::Expression(tokens) => {
                    let outer_tokens = std::mem::replace(&mut self.toks, tokens);
                    let outer_pos = std::mem::replace(&mut self.pos, 0);
                    let result = self.parse_postfix();
                    let complete = self.at(&TokenKind::Eof);
                    self.toks = outer_tokens;
                    self.pos = outer_pos;
                    let expression = result?;
                    if !complete
                        || !matches!(
                            expression,
                            Expr::Var(..)
                                | Expr::Index { .. }
                                | Expr::Property { .. }
                                | Expr::MethodCall { .. }
                        )
                    {
                        self.diags.emit(Diagnostic::error("braced interpolation requires a variable, array access or property/method access").with_code("E0008").with_span(expression.span()));
                        return Err(());
                    }
                    expression
                }
            };
            acc = Expr::Binary {
                op: BinOp::Concat,
                lhs: Box::new(acc),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(acc)
    }
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
    fn instanceof_generic_arguments_preserve_comparisons_and_shifts() {
        let p = ok("<?php echo $x instanceof C < (8 >> 1); echo $x instanceof Box<array<int>>;");
        let Stmt::Echo(values, _) = &p.items[0] else {
            panic!("expected echo")
        };
        assert!(matches!(&values[0], Expr::Binary { op: BinOp::Lt, lhs, .. }
            if matches!(lhs.as_ref(), Expr::Instanceof { type_args, .. } if type_args.is_empty())));
        let Stmt::Echo(values, _) = &p.items[1] else {
            panic!("expected echo")
        };
        assert!(matches!(&values[0], Expr::Instanceof { type_args, .. }
            if type_args == &[TypeRef::Array(Box::new(TypeRef::Int))]));
    }

    #[test]
    fn generic_method_arguments_preserve_property_comparisons() {
        let p = ok("<?php echo $x->value < (8 >> 1); echo $x->id<array<int>>([1]); echo Box<int>::id<string>('s');");
        let Stmt::Echo(values, _) = &p.items[0] else {
            panic!("expected echo")
        };
        assert!(
            matches!(&values[0], Expr::Binary {op: BinOp::Lt, lhs, ..} if matches!(lhs.as_ref(), Expr::Property {..}))
        );
        let Stmt::Echo(values, _) = &p.items[1] else {
            panic!("expected echo")
        };
        assert!(
            matches!(&values[0], Expr::MethodCall {type_args, ..} if type_args == &[TypeRef::Array(Box::new(TypeRef::Int))])
        );
        let Stmt::Echo(values, _) = &p.items[2] else {
            panic!("expected echo")
        };
        assert!(
            matches!(&values[0], Expr::StaticCall {type_args, method_type_args, ..} if type_args == &[TypeRef::Int] && method_type_args == &[TypeRef::String])
        );
    }

    #[test]
    fn generic_bounds_scope_over_the_full_parameter_list() {
        let p = ok("<?php namespace N; function f<T: Reader<U> & Named, U>(T $v) {return $v;}");
        let Stmt::FnDecl(function) = &p.items[0] else {
            panic!("expected function")
        };
        assert_eq!(function.bounds.len(), 1);
        assert!(
            matches!(&function.bounds[0].1[0], TypeRef::Applied(name, args, _) if name == "N\\Reader" && matches!(&args[0], TypeRef::Param(name, _) if name == "U"))
        );
        assert!(function.ret.is_none());
    }

    #[test]
    fn trait_members_and_class_generic_scopes_parse_independently() {
        let p = ok("<?php trait Identity {public function id<U>(U $v) {return $v;}} class Box<T> {use Identity; public function get(): T {return $this->value;}}");
        let Stmt::ClassDecl(trait_decl) = &p.items[0] else {
            panic!("expected trait")
        };
        assert!(trait_decl.is_trait);
        let Stmt::ClassDecl(class) = &p.items[1] else {
            panic!("expected class")
        };
        assert_eq!(class.trait_uses[0].name, "Identity");
        let ClassMember::Method(method) = &class.members[0] else {
            panic!("expected method")
        };
        assert!(matches!(&method.ret, Some(TypeRef::Param(name, _)) if name == "T"));
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
                assert_eq!(
                    c.extends.as_ref().map(|parent| parent.name.as_str()),
                    Some("Animal")
                );
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
