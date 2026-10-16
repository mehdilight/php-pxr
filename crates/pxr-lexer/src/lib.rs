//! The PXR lexer.
//!
//! PXR source files use the `.php` extension and the familiar PHP lexical
//! grammar. The lexer is byte oriented: every [`Span`] it produces is a byte
//! range into the original source, so diagnostics can be rendered exactly.
//!
//! Milestone notes (see `docs/LANGUAGE.md`):
//! * `<?php` opening tags are accepted and skipped; files without a tag are
//!   lexed as bare code so that PXR fragments are convenient to test.
//! * `#!` shebang lines are skipped.
//! * Double-quoted string interpolation supports the simple `$name` and
//!   `{$name}` forms. The more complex forms PHP also allows (`$a[0]`,
//!   `$a[k]`, `{$a[0]}`, `$obj->p`, the deprecated `${name}`) are rejected with
//!   a diagnostic (`E0008`) rather than being silently mis-lexed.

use pxr_diag::{Diagnostic, Diagnostics, Span};

/// One piece of an interpolated double-quoted string.
#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    /// A run of literal bytes (escapes already applied).
    Lit(Vec<u8>),
    /// A `$name` / `{$name}` variable reference.
    Var(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // Literals and names.
    Ident(String),
    Variable(String),
    Int(i64),
    Float(f64),
    /// Raw bytes: PHP strings are byte strings, so a literal may hold invalid
    /// UTF-8 (e.g. `"\xA9"` is the single byte `0xA9`).
    Str(Vec<u8>),
    /// A double-quoted string that contains at least one `$name` / `{$name}`
    /// interpolation. Plain strings (including ones where every `$` is literal)
    /// stay [`TokenKind::Str`] so downstream code is unchanged.
    InterpStr(Vec<StrPart>),

    // Keywords.
    Function,
    Async,
    Await,
    Spawn,
    Return,
    If,
    Else,
    ElseIf,
    While,
    For,
    Foreach,
    As,
    Do,
    Break,
    Continue,
    Switch,
    Case,
    Default,
    Match,
    Echo,
    Print,
    True,
    False,
    Null,
    Class,
    Abstract,
    Interface,
    Trait,
    Implements,
    Static,
    Namespace,
    Use,
    Require,
    RequireOnce,
    Include,
    IncludeOnce,
    New,
    Public,
    Private,
    Protected,
    Extends,
    Instanceof,
    Parent,

    // Type keywords.
    KwInt,
    KwFloat,
    KwString,
    KwBool,
    KwVoid,
    KwMixed,
    KwArray,

    // Punctuation.
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Arrow,
    DoubleArrow,
    DoubleColon,
    Backslash,
    Question,

    // Operators.
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Dot,
    PlusPlus,
    MinusMinus,
    EqEq,
    NotEq,
    Identical,
    NotIdentical,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Spaceship,
    AndAnd,
    OrOr,
    Bang,
    Coalesce,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,

    // Compound assignment.
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    DotEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,
    CoalesceEq,

    /// End of input.
    Eof,
}

impl TokenKind {
    /// A short, human readable description used in parser errors.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(s) => format!("identifier `{s}`"),
            TokenKind::Variable(s) => format!("variable `${s}`"),
            TokenKind::Int(v) => format!("integer `{v}`"),
            TokenKind::Float(v) => format!("float `{v}`"),
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::InterpStr(_) => "interpolated string literal".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{}`", other.symbol()),
        }
    }

    /// The literal source spelling for punctuation/keywords.
    pub fn symbol(&self) -> &'static str {
        match self {
            TokenKind::Function => "function",
            TokenKind::Async => "async",
            TokenKind::Await => "await",
            TokenKind::Spawn => "spawn",
            TokenKind::Return => "return",
            TokenKind::If => "if",
            TokenKind::Else => "else",
            TokenKind::ElseIf => "elseif",
            TokenKind::While => "while",
            TokenKind::For => "for",
            TokenKind::Foreach => "foreach",
            TokenKind::As => "as",
            TokenKind::Do => "do",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::Switch => "switch",
            TokenKind::Case => "case",
            TokenKind::Default => "default",
            TokenKind::Match => "match",
            TokenKind::Echo => "echo",
            TokenKind::Print => "print",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::Null => "null",
            TokenKind::Class => "class",
            TokenKind::Abstract => "abstract",
            TokenKind::Interface => "interface",
            TokenKind::Trait => "trait",
            TokenKind::Implements => "implements",
            TokenKind::Static => "static",
            TokenKind::Namespace => "namespace",
            TokenKind::Use => "use",
            TokenKind::Require => "require",
            TokenKind::RequireOnce => "require_once",
            TokenKind::Include => "include",
            TokenKind::IncludeOnce => "include_once",
            TokenKind::New => "new",
            TokenKind::Public => "public",
            TokenKind::Private => "private",
            TokenKind::Protected => "protected",
            TokenKind::Extends => "extends",
            TokenKind::Instanceof => "instanceof",
            TokenKind::Parent => "parent",
            TokenKind::KwInt => "int",
            TokenKind::KwFloat => "float",
            TokenKind::KwString => "string",
            TokenKind::KwBool => "bool",
            TokenKind::KwVoid => "void",
            TokenKind::KwMixed => "mixed",
            TokenKind::KwArray => "array",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Semicolon => ";",
            TokenKind::Colon => ":",
            TokenKind::Arrow => "->",
            TokenKind::DoubleArrow => "=>",
            TokenKind::DoubleColon => "::",
            TokenKind::Backslash => "\\",
            TokenKind::Question => "?",
            TokenKind::Assign => "=",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Dot => ".",
            TokenKind::PlusPlus => "++",
            TokenKind::MinusMinus => "--",
            TokenKind::EqEq => "==",
            TokenKind::NotEq => "!=",
            TokenKind::Identical => "===",
            TokenKind::NotIdentical => "!==",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::Spaceship => "<=>",
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::Bang => "!",
            TokenKind::Coalesce => "??",
            TokenKind::Amp => "&",
            TokenKind::Pipe => "|",
            TokenKind::Caret => "^",
            TokenKind::Tilde => "~",
            TokenKind::Shl => "<<",
            TokenKind::Shr => ">>",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::PercentEq => "%=",
            TokenKind::DotEq => ".=",
            TokenKind::AmpEq => "&=",
            TokenKind::PipeEq => "|=",
            TokenKind::CaretEq => "^=",
            TokenKind::ShlEq => "<<=",
            TokenKind::ShrEq => ">>=",
            TokenKind::CoalesceEq => "??=",
            _ => "token",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    diags: Diagnostics,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
            diags: Diagnostics::new(),
        }
    }

    /// Tokenize the whole input, returning the tokens (always terminated by
    /// [`TokenKind::Eof`]) and any lexical diagnostics.
    pub fn tokenize(mut self) -> (Vec<Token>, Diagnostics) {
        self.skip_prefix();
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            let start = self.pos;
            match self.next_token(&mut tokens) {
                Ok(()) => {}
                Err(()) => {}
            }
            let end = self.pos;
            let _ = (start, end);
            if matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)) {
                break;
            }
        }
        (tokens, self.diags)
    }

    fn skip_prefix(&mut self) {
        // Shebang line.
        if self.src.starts_with(b"#!") {
            while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                self.pos += 1;
            }
        }
        // Leading whitespace then an optional `<?php` / `<?` opening tag.
        let mut i = self.pos;
        while i < self.src.len() && (self.src[i] as char).is_whitespace() {
            i += 1;
        }
        if self.src[i..].starts_with(b"<?php") {
            self.pos = i + 5;
        } else if self.src[i..].starts_with(b"<?") {
            self.pos = i + 2;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(0x0b) | Some(0x0c) => {
                    self.pos += 1;
                }
                Some(b'#') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    let start = self.pos;
                    self.pos += 2;
                    let mut closed = false;
                    while self.pos < self.src.len() {
                        if self.src[self.pos] == b'*' && self.peek_at(1) == Some(b'/') {
                            self.pos += 2;
                            closed = true;
                            break;
                        }
                        self.pos += 1;
                    }
                    if !closed {
                        self.diags.emit(
                            Diagnostic::error("unterminated block comment")
                                .with_code("E0001")
                                .with_span(Span::new(start, self.src.len())),
                        );
                    }
                }
                Some(b'?') if self.peek_at(1) == Some(b'>') => {
                    // Closing tag: treat the remainder as literal text and stop
                    // tokenizing, mirroring how PHP ignores trailing output.
                    self.pos += 2;
                    // Skip any trailing whitespace/newline.
                    while matches!(
                        self.peek(),
                        Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
                    ) {
                        self.pos += 1;
                    }
                    if self.pos < self.src.len() {
                        self.diags.emit(
                            Diagnostic::warning("content after `?>` is ignored by PXR")
                                .with_span(Span::new(self.pos, self.src.len())),
                        );
                    }
                    self.pos = self.src.len();
                }
                _ => break,
            }
        }
    }

    fn next_token(&mut self, out: &mut Vec<Token>) -> Result<(), ()> {
        let start = self.pos;
        let Some(b) = self.peek() else {
            out.push(Token {
                kind: TokenKind::Eof,
                span: Span::new(self.pos, self.pos),
            });
            return Ok(());
        };

        let kind = if b == b'$' {
            self.pos += 1;
            let name = self.read_ident_body();
            if name.is_empty() {
                self.diags.emit(
                    Diagnostic::error("`$` must be followed by a variable name")
                        .with_code("E0002")
                        .with_span(Span::new(start, self.pos)),
                );
                return Err(());
            }
            TokenKind::Variable(name)
        } else if b.is_ascii_digit() {
            self.read_number(start)?
        } else if b == b'\'' || b == b'"' {
            self.read_string(b, start)
        } else if is_ident_start(b) {
            let word = self.read_ident_body();
            keyword(&word).unwrap_or(TokenKind::Ident(word))
        } else {
            self.pos += 1;
            match b {
                b'\\' => TokenKind::Backslash,
                b'(' => TokenKind::LParen,
                b')' => TokenKind::RParen,
                b'{' => TokenKind::LBrace,
                b'}' => TokenKind::RBrace,
                b'[' => TokenKind::LBracket,
                b']' => TokenKind::RBracket,
                b',' => TokenKind::Comma,
                b';' => TokenKind::Semicolon,
                b':' => {
                    if self.peek() == Some(b':') {
                        self.pos += 1;
                        TokenKind::DoubleColon
                    } else {
                        TokenKind::Colon
                    }
                }
                b'?' => {
                    if self.peek() == Some(b'?') {
                        self.pos += 1;
                        if self.peek() == Some(b'=') {
                            self.pos += 1;
                            TokenKind::CoalesceEq
                        } else {
                            TokenKind::Coalesce
                        }
                    } else {
                        TokenKind::Question
                    }
                }
                b'+' => {
                    if self.peek() == Some(b'+') {
                        self.pos += 1;
                        TokenKind::PlusPlus
                    } else if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::PlusEq
                    } else {
                        TokenKind::Plus
                    }
                }
                b'-' => match self.peek() {
                    Some(b'-') => {
                        self.pos += 1;
                        TokenKind::MinusMinus
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        TokenKind::Arrow
                    }
                    Some(b'=') => {
                        self.pos += 1;
                        TokenKind::MinusEq
                    }
                    _ => TokenKind::Minus,
                },
                b'*' => {
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::StarEq
                    } else {
                        TokenKind::Star
                    }
                }
                b'/' => {
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::SlashEq
                    } else {
                        TokenKind::Slash
                    }
                }
                b'%' => {
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::PercentEq
                    } else {
                        TokenKind::Percent
                    }
                }
                b'.' => {
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::DotEq
                    } else {
                        TokenKind::Dot
                    }
                }
                b'~' => TokenKind::Tilde,
                b'^' => {
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::CaretEq
                    } else {
                        TokenKind::Caret
                    }
                }
                b'=' => match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        if self.peek() == Some(b'=') {
                            self.pos += 1;
                            TokenKind::Identical
                        } else {
                            TokenKind::EqEq
                        }
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        TokenKind::DoubleArrow
                    }
                    _ => TokenKind::Assign,
                },
                b'!' => match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        if self.peek() == Some(b'=') {
                            self.pos += 1;
                            TokenKind::NotIdentical
                        } else {
                            TokenKind::NotEq
                        }
                    }
                    _ => TokenKind::Bang,
                },
                b'<' => match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        if self.peek() == Some(b'>') {
                            self.pos += 1;
                            TokenKind::Spaceship
                        } else {
                            TokenKind::LtEq
                        }
                    }
                    Some(b'<') => {
                        self.pos += 1;
                        if self.peek() == Some(b'=') {
                            self.pos += 1;
                            TokenKind::ShlEq
                        } else {
                            TokenKind::Shl
                        }
                    }
                    _ => TokenKind::Lt,
                },
                b'>' => match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        TokenKind::GtEq
                    }
                    Some(b'>') => {
                        self.pos += 1;
                        if self.peek() == Some(b'=') {
                            self.pos += 1;
                            TokenKind::ShrEq
                        } else {
                            TokenKind::Shr
                        }
                    }
                    _ => TokenKind::Gt,
                },
                b'&' => {
                    if self.peek() == Some(b'&') {
                        self.pos += 1;
                        TokenKind::AndAnd
                    } else if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::AmpEq
                    } else {
                        TokenKind::Amp
                    }
                }
                b'|' => {
                    if self.peek() == Some(b'|') {
                        self.pos += 1;
                        TokenKind::OrOr
                    } else if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::PipeEq
                    } else {
                        TokenKind::Pipe
                    }
                }
                other => {
                    let shown = if other.is_ascii_graphic() {
                        (other as char).to_string()
                    } else {
                        format!("\\x{other:02x}")
                    };
                    self.diags.emit(
                        Diagnostic::error(format!("unexpected character `{shown}`"))
                            .with_code("E0004")
                            .with_span(Span::new(start, self.pos)),
                    );
                    return Err(());
                }
            }
        };

        out.push(Token {
            kind,
            span: Span::new(start, self.pos),
        });
        Ok(())
    }

    fn read_ident_body(&mut self) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if is_ident_continue(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn read_number(&mut self, start: usize) -> Result<TokenKind, ()> {
        // Hexadecimal / binary / octal (0o) literal forms.
        if self.peek() == Some(b'0') {
            match self.peek_at(1) {
                Some(b'x') | Some(b'X') => {
                    self.pos += 2;
                    let s = self.read_while(|b| b.is_ascii_hexdigit() || b == b'_');
                    return match i64::from_str_radix(&s.replace('_', ""), 16) {
                        Ok(v) => Ok(TokenKind::Int(v)),
                        Err(_) => {
                            self.diags.emit(
                                Diagnostic::error("invalid hexadecimal integer literal")
                                    .with_code("E0005")
                                    .with_span(Span::new(start, self.pos)),
                            );
                            Err(())
                        }
                    };
                }
                Some(b'b') | Some(b'B') => {
                    self.pos += 2;
                    let s = self.read_while(|b| b == b'0' || b == b'1' || b == b'_');
                    return match i64::from_str_radix(&s.replace('_', ""), 2) {
                        Ok(v) => Ok(TokenKind::Int(v)),
                        Err(_) => {
                            self.diags.emit(
                                Diagnostic::error("invalid binary integer literal")
                                    .with_code("E0005")
                                    .with_span(Span::new(start, self.pos)),
                            );
                            Err(())
                        }
                    };
                }
                Some(b'o') | Some(b'O') => {
                    self.pos += 2;
                    let s = self.read_while(|b| (b'0'..=b'7').contains(&b) || b == b'_');
                    return match i64::from_str_radix(&s.replace('_', ""), 8) {
                        Ok(v) => Ok(TokenKind::Int(v)),
                        Err(_) => {
                            self.diags.emit(
                                Diagnostic::error("invalid octal integer literal")
                                    .with_code("E0005")
                                    .with_span(Span::new(start, self.pos)),
                            );
                            Err(())
                        }
                    };
                }
                _ => {}
            }
        }

        let digits_start = self.pos;
        self.read_while(|b| b.is_ascii_digit() || b == b'_');

        let mut is_float = false;
        // A `.` only starts a fractional part when directly followed by a digit,
        // otherwise it is the concatenation operator.
        if self.peek() == Some(b'.') && self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) {
            is_float = true;
            self.pos += 1;
            self.read_while(|b| b.is_ascii_digit() || b == b'_');
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            let save = self.pos;
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if self.peek().is_some_and(|d| d.is_ascii_digit()) {
                is_float = true;
                self.read_while(|b| b.is_ascii_digit());
            } else {
                self.pos = save;
            }
        }

        let raw = String::from_utf8_lossy(&self.src[digits_start..self.pos]).into_owned();
        let cleaned = raw.replace('_', "");

        if is_float {
            match cleaned.parse::<f64>() {
                Ok(v) => Ok(TokenKind::Float(v)),
                Err(_) => {
                    self.diags.emit(
                        Diagnostic::error("invalid float literal")
                            .with_code("E0005")
                            .with_span(Span::new(start, self.pos)),
                    );
                    Err(())
                }
            }
        } else {
            if cleaned.len() > 1 && cleaned.starts_with('0') {
                self.diags.emit(
                    Diagnostic::warning(
                        "leading-zero octal literals are not implemented; read as decimal",
                    )
                    .with_code("W0001")
                    .with_span(Span::new(start, self.pos)),
                );
            }
            match cleaned.parse::<i64>() {
                Ok(v) => Ok(TokenKind::Int(v)),
                Err(_) => {
                    self.diags.emit(
                        Diagnostic::error("integer literal out of range for `int` (64-bit)")
                            .with_code("E0006")
                            .with_span(Span::new(start, self.pos)),
                    );
                    Err(())
                }
            }
        }
    }

    fn read_string(&mut self, quote: u8, start: usize) -> TokenKind {
        self.pos += 1; // opening quote
        if quote == b'\'' {
            // Single quotes only recognize `\\` and `\'`; no interpolation.
            let mut out: Vec<u8> = Vec::new();
            loop {
                match self.peek() {
                    None => {
                        self.diags.emit(
                            Diagnostic::error("unterminated string literal")
                                .with_code("E0007")
                                .with_span(Span::new(start, self.pos)),
                        );
                        break;
                    }
                    Some(b'\'') => {
                        self.pos += 1;
                        break;
                    }
                    Some(b'\\') => {
                        self.pos += 1;
                        match self.bump() {
                            Some(b'\\') => out.push(b'\\'),
                            Some(b'\'') => out.push(b'\''),
                            Some(other) => {
                                out.push(b'\\');
                                out.push(other);
                            }
                            None => break,
                        }
                    }
                    Some(b) => {
                        out.push(b);
                        self.pos += 1;
                    }
                }
            }
            return TokenKind::Str(out);
        }

        // Double-quoted: escapes plus `$name` / `{$name}` interpolation.
        let mut parts: Vec<StrPart> = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut has_var = false;
        loop {
            match self.peek() {
                None => {
                    self.diags.emit(
                        Diagnostic::error("unterminated string literal")
                            .with_code("E0007")
                            .with_span(Span::new(start, self.pos)),
                    );
                    break;
                }
                Some(b'"') => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let Some(e) = self.bump() else { break };
                    match e {
                        b'n' => cur.push(b'\n'),
                        b't' => cur.push(b'\t'),
                        b'r' => cur.push(b'\r'),
                        b'v' => cur.push(0x0b),
                        b'f' => cur.push(0x0c),
                        b'e' => cur.push(0x1b),
                        b'0' => cur.push(0),
                        b'\\' => cur.push(b'\\'),
                        b'"' => cur.push(b'"'),
                        b'$' => cur.push(b'$'),
                        b'x' | b'X' => {
                            let h1 = self.peek().filter(u8::is_ascii_hexdigit);
                            let h2 = h1
                                .and_then(|_| self.peek_at(1))
                                .filter(u8::is_ascii_hexdigit);
                            if let (Some(h1), Some(h2)) = (h1, h2) {
                                self.pos += 2;
                                let hex = [h1, h2];
                                let s = std::str::from_utf8(&hex).unwrap();
                                cur.push(u8::from_str_radix(s, 16).unwrap());
                            } else {
                                cur.push(b'\\');
                                cur.push(e);
                            }
                        }
                        other => {
                            cur.push(b'\\');
                            cur.push(other);
                        }
                    }
                }
                Some(b'$') => match self.peek_at(1) {
                    Some(b'{') => {
                        // Deprecated `${name}` / `${expr}` syntax.
                        self.diags.emit(
                            Diagnostic::error(
                                "`${...}` string interpolation is not implemented yet; write `{$name}` instead",
                            )
                            .with_code("E0008")
                            .with_span(Span::new(self.pos, self.pos + 2)),
                        );
                        cur.push(b'$');
                        self.pos += 1;
                    }
                    Some(n) if is_ident_start(n) => {
                        self.pos += 1; // consume `$`
                        let name = self.read_ident_body();
                        if !cur.is_empty() {
                            parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                        }
                        has_var = true;
                        parts.push(StrPart::Var(name));
                        // PHP's "simple" syntax also allows `$a[0]` / `$a[$k]`
                        // and `$obj->prop`; reject these rather than mis-lex.
                        if self.peek() == Some(b'[') {
                            self.diags.emit(
                                Diagnostic::error(
                                    "array interpolation (`$a[...]`) is not implemented yet; write a concatenation instead",
                                )
                                .with_code("E0008")
                                .with_span(Span::new(self.pos, self.pos + 1)),
                            );
                        } else if self.peek() == Some(b'-') && self.peek_at(1) == Some(b'>') {
                            self.diags.emit(
                                Diagnostic::error(
                                    "property interpolation (`$obj->prop`) is not implemented yet; write a concatenation instead",
                                )
                                .with_code("E0008")
                                .with_span(Span::new(self.pos, self.pos + 2)),
                            );
                        }
                    }
                    _ => {
                        // `$` not followed by a name start is a literal `$`
                        // (`"$"`, `"$1"`, `"$$x"`), matching PHP.
                        cur.push(b'$');
                        self.pos += 1;
                    }
                },
                Some(b'{') if self.peek_at(1) == Some(b'$') => {
                    // `{$name}` (simple) is supported; anything more complex is
                    // rejected so it is never silently treated as literal.
                    let save = self.pos;
                    self.pos += 2; // `{` `$`
                    let name = self.read_ident_body();
                    if !name.is_empty() && self.peek() == Some(b'}') {
                        self.pos += 1;
                        if !cur.is_empty() {
                            parts.push(StrPart::Lit(std::mem::take(&mut cur)));
                        }
                        has_var = true;
                        parts.push(StrPart::Var(name));
                    } else {
                        self.pos = save;
                        self.diags.emit(
                            Diagnostic::error(
                                "complex string interpolation (`{$...}`) is not implemented yet; only `{$name}` is supported",
                            )
                            .with_code("E0008")
                            .with_span(Span::new(self.pos, self.pos + 1)),
                        );
                        cur.push(b'{');
                        self.pos += 1;
                    }
                }
                Some(b) => {
                    cur.push(b);
                    self.pos += 1;
                }
            }
        }

        if !cur.is_empty() {
            parts.push(StrPart::Lit(cur));
        }
        if has_var {
            TokenKind::InterpStr(parts)
        } else {
            // No interpolation: fold back into a plain byte string.
            let mut buf = Vec::new();
            for p in parts {
                if let StrPart::Lit(b) = p {
                    buf.extend_from_slice(&b);
                }
            }
            TokenKind::Str(buf)
        }
    }

    fn read_while(&mut self, pred: impl Fn(u8) -> bool) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if pred(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn keyword(word: &str) -> Option<TokenKind> {
    // PHP keywords are case-insensitive; PXR preserves that behaviour.
    Some(match word.to_ascii_lowercase().as_str() {
        "function" => TokenKind::Function,
        "async" => TokenKind::Async,
        "await" => TokenKind::Await,
        "spawn" => TokenKind::Spawn,
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "elseif" => TokenKind::ElseIf,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "foreach" => TokenKind::Foreach,
        "as" => TokenKind::As,
        "do" => TokenKind::Do,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "switch" => TokenKind::Switch,
        "case" => TokenKind::Case,
        "default" => TokenKind::Default,
        "match" => TokenKind::Match,
        "echo" => TokenKind::Echo,
        "print" => TokenKind::Print,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        "class" => TokenKind::Class,
        "abstract" => TokenKind::Abstract,
        "interface" => TokenKind::Interface,
        "trait" => TokenKind::Trait,
        "implements" => TokenKind::Implements,
        "static" => TokenKind::Static,
        "namespace" => TokenKind::Namespace,
        "use" => TokenKind::Use,
        "require" => TokenKind::Require,
        "require_once" => TokenKind::RequireOnce,
        "include" => TokenKind::Include,
        "include_once" => TokenKind::IncludeOnce,
        "new" => TokenKind::New,
        "public" => TokenKind::Public,
        "private" => TokenKind::Private,
        "protected" => TokenKind::Protected,
        "extends" => TokenKind::Extends,
        "instanceof" => TokenKind::Instanceof,
        "parent" => TokenKind::Parent,
        "int" => TokenKind::KwInt,
        "float" => TokenKind::KwFloat,
        "string" => TokenKind::KwString,
        "bool" => TokenKind::KwBool,
        "void" => TokenKind::KwVoid,
        "mixed" => TokenKind::KwMixed,
        "array" => TokenKind::KwArray,
        _ => return None,
    })
}

/// Convenience entry point.
pub fn lex(src: &str) -> (Vec<Token>, Diagnostics) {
    Lexer::new(src).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, diags) = lex(src);
        assert!(
            !diags.has_errors(),
            "unexpected lex errors: {:?}",
            diags.items()
        );
        toks.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn skips_open_tag() {
        let k = kinds("<?php echo 1;");
        assert_eq!(k[0], TokenKind::Echo);
    }

    #[test]
    fn numbers() {
        assert_eq!(kinds("0x10")[0], TokenKind::Int(16));
        assert_eq!(kinds("0b101")[0], TokenKind::Int(5));
        assert_eq!(kinds("1_000")[0], TokenKind::Int(1000));
        assert_eq!(kinds("1.5e3")[0], TokenKind::Float(1500.0));
    }

    #[test]
    fn concat_vs_float() {
        // `1.5` is a float, `1 . 5` is concatenation.
        assert_eq!(kinds("1.5")[0], TokenKind::Float(1.5));
        let k = kinds("1 . 5");
        assert_eq!(k[0], TokenKind::Int(1));
        assert_eq!(k[1], TokenKind::Dot);
        assert_eq!(k[2], TokenKind::Int(5));
    }

    #[test]
    fn strings_and_escapes() {
        let k = kinds(r#""a\nb""#);
        assert_eq!(k[0], TokenKind::Str(b"a\nb".to_vec()));
        let k = kinds(r"'it\'s'");
        assert_eq!(k[0], TokenKind::Str(b"it's".to_vec()));
    }

    #[test]
    fn string_hex_escapes_are_raw_bytes() {
        // `\xHH` must yield the raw byte even when it is not valid UTF-8; the
        // payload is a byte string, not a Rust `String`.
        let k = kinds(r#""\xA9\xC3""#);
        assert_eq!(k[0], TokenKind::Str(vec![0xA9, 0xC3]));
        let k = kinds(r#""\x00\x01\xFE\xFF""#);
        assert_eq!(k[0], TokenKind::Str(vec![0x00, 0x01, 0xFE, 0xFF]));
    }

    #[test]
    fn variables_and_keywords_case_insensitive() {
        let k = kinds("$x ECHO Function");
        assert_eq!(k[0], TokenKind::Variable("x".into()));
        assert_eq!(k[1], TokenKind::Echo);
        assert_eq!(k[2], TokenKind::Function);
    }

    #[test]
    fn comments() {
        let k = kinds("// a\n# b\necho /* c */ 1;");
        assert_eq!(k[0], TokenKind::Echo);
    }

    #[test]
    fn interpolation_simple_variable() {
        let k = kinds(r#""hi $name""#);
        assert_eq!(
            k[0],
            TokenKind::InterpStr(vec![
                StrPart::Lit(b"hi ".to_vec()),
                StrPart::Var("name".into()),
            ])
        );
    }

    #[test]
    fn interpolation_braced_variable() {
        let k = kinds(r#""a{$x}b""#);
        assert_eq!(
            k[0],
            TokenKind::InterpStr(vec![
                StrPart::Lit(b"a".to_vec()),
                StrPart::Var("x".into()),
                StrPart::Lit(b"b".to_vec()),
            ])
        );
        // Escaped dollar and literal dollars that are not followed by a name
        // start stay part of the literal text (no interpolation token).
        assert_eq!(kinds(r#""\$x""#)[0], TokenKind::Str(b"$x".to_vec()));
        assert_eq!(kinds(r#""$1 $ ""#)[0], TokenKind::Str(b"$1 $ ".to_vec()));
        assert_eq!(kinds(r#""foo$""#)[0], TokenKind::Str(b"foo$".to_vec()));
    }

    #[test]
    fn interpolation_complex_forms_are_rejected() {
        for src in [
            r#""$a[0]""#,
            r#""$a[k]""#,
            r#""$a[$j]""#,
            r#""{$a[0]}""#,
            r#""$obj->prop""#,
            r#""${x}""#,
        ] {
            let (_, diags) = lex(src);
            assert!(diags.has_errors(), "expected a diagnostic for {src}");
            assert!(
                diags.items().iter().any(|d| d.code == Some("E0008")),
                "expected E0008 for {src}"
            );
        }
    }

    #[test]
    fn class_keywords() {
        let k =
            kinds("class Point { public int $x; private string $y; protected $z; new Point(); }");
        assert_eq!(
            k,
            vec![
                TokenKind::Class,
                TokenKind::Ident("Point".into()),
                TokenKind::LBrace,
                TokenKind::Public,
                TokenKind::KwInt,
                TokenKind::Variable("x".into()),
                TokenKind::Semicolon,
                TokenKind::Private,
                TokenKind::KwString,
                TokenKind::Variable("y".into()),
                TokenKind::Semicolon,
                TokenKind::Protected,
                TokenKind::Variable("z".into()),
                TokenKind::Semicolon,
                TokenKind::New,
                TokenKind::Ident("Point".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn inheritance_keywords() {
        let k = kinds("class Dog extends Animal { $x instanceof Animal; parent::foo(); }");
        assert_eq!(
            k,
            vec![
                TokenKind::Class,
                TokenKind::Ident("Dog".into()),
                TokenKind::Extends,
                TokenKind::Ident("Animal".into()),
                TokenKind::LBrace,
                TokenKind::Variable("x".into()),
                TokenKind::Instanceof,
                TokenKind::Ident("Animal".into()),
                TokenKind::Semicolon,
                TokenKind::Parent,
                TokenKind::DoubleColon,
                TokenKind::Ident("foo".into()),
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }
}
