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
//! * Double-quoted string interpolation is **not** implemented yet; a warning
//!   is emitted and the text is kept literal rather than silently mis-compiled.

use pxr_diag::{Diagnostic, Diagnostics, Span};

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // Literals and names.
    Ident(String),
    Variable(String),
    Int(i64),
    Float(f64),
    Str(String),

    // Keywords.
    Function,
    Return,
    If,
    Else,
    ElseIf,
    While,
    Echo,
    Print,
    True,
    False,
    Null,

    // Type keywords.
    KwInt,
    KwFloat,
    KwString,
    KwBool,
    KwVoid,

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
    DoubleColon,
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
    AndAnd,
    OrOr,
    Bang,

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
            TokenKind::Eof => "end of file".to_string(),
            other => format!("`{}`", other.symbol()),
        }
    }

    /// The literal source spelling for punctuation/keywords.
    pub fn symbol(&self) -> &'static str {
        match self {
            TokenKind::Function => "function",
            TokenKind::Return => "return",
            TokenKind::If => "if",
            TokenKind::Else => "else",
            TokenKind::ElseIf => "elseif",
            TokenKind::While => "while",
            TokenKind::Echo => "echo",
            TokenKind::Print => "print",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::Null => "null",
            TokenKind::KwInt => "int",
            TokenKind::KwFloat => "float",
            TokenKind::KwString => "string",
            TokenKind::KwBool => "bool",
            TokenKind::KwVoid => "void",
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
            TokenKind::DoubleColon => "::",
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
            TokenKind::AndAnd => "&&",
            TokenKind::OrOr => "||",
            TokenKind::Bang => "!",
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
                    while matches!(self.peek(), Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')) {
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
                b'?' => TokenKind::Question,
                b'+' => {
                    if self.peek() == Some(b'+') {
                        self.pos += 1;
                        TokenKind::PlusPlus
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
                    _ => TokenKind::Minus,
                },
                b'*' => TokenKind::Star,
                b'/' => TokenKind::Slash,
                b'%' => TokenKind::Percent,
                b'.' => TokenKind::Dot,
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
                        TokenKind::LtEq
                    }
                    _ => TokenKind::Lt,
                },
                b'>' => match self.peek() {
                    Some(b'=') => {
                        self.pos += 1;
                        TokenKind::GtEq
                    }
                    _ => TokenKind::Gt,
                },
                b'&' => {
                    if self.peek() == Some(b'&') {
                        self.pos += 1;
                        TokenKind::AndAnd
                    } else {
                        self.diags.emit(
                            Diagnostic::error("unexpected `&`; bitwise operators are not implemented yet")
                                .with_code("E0003")
                                .with_span(Span::new(start, self.pos)),
                        );
                        return Err(());
                    }
                }
                b'|' => {
                    if self.peek() == Some(b'|') {
                        self.pos += 1;
                        TokenKind::OrOr
                    } else {
                        self.diags.emit(
                            Diagnostic::error("unexpected `|`; bitwise operators are not implemented yet")
                                .with_code("E0003")
                                .with_span(Span::new(start, self.pos)),
                        );
                        return Err(());
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
                Some(b) if b == quote => {
                    self.pos += 1;
                    break;
                }
                Some(b'\\') => {
                    let esc_start = self.pos;
                    self.pos += 1;
                    let Some(e) = self.bump() else { break };
                    if quote == b'\'' {
                        // Single quotes only recognize `\\` and `\'`.
                        match e {
                            b'\\' => out.push(b'\\'),
                            b'\'' => out.push(b'\''),
                            other => {
                                out.push(b'\\');
                                out.push(other);
                            }
                        }
                    } else {
                        match e {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            b'r' => out.push(b'\r'),
                            b'v' => out.push(0x0b),
                            b'f' => out.push(0x0c),
                            b'e' => out.push(0x1b),
                            b'0' => out.push(0),
                            b'\\' => out.push(b'\\'),
                            b'"' => out.push(b'"'),
                            b'$' => out.push(b'$'),
                            b'x' | b'X' => {
                                let h1 = self.peek().filter(u8::is_ascii_hexdigit);
                                let h2 = h1.and_then(|_| self.peek_at(1)).filter(u8::is_ascii_hexdigit);
                                if let (Some(h1), Some(h2)) = (h1, h2) {
                                    self.pos += 2;
                                    let hex = [h1, h2];
                                    let s = std::str::from_utf8(&hex).unwrap();
                                    out.push(u8::from_str_radix(s, 16).unwrap());
                                } else {
                                    out.push(b'\\');
                                    out.push(e);
                                }
                            }
                            other => {
                                out.push(b'\\');
                                out.push(other);
                            }
                        }
                    }
                    let _ = esc_start;
                }
                Some(b'$') if quote == b'"' => {
                    self.diags.emit(
                        Diagnostic::warning(
                            "double-quoted string interpolation is not implemented yet; the `$` is kept literally",
                        )
                        .with_code("W0002")
                        .with_span(Span::new(self.pos, self.pos + 1)),
                    );
                    out.push(b'$');
                    self.pos += 1;
                }
                Some(b) => {
                    out.push(b);
                    self.pos += 1;
                }
            }
        }
        TokenKind::Str(String::from_utf8_lossy(&out).into_owned())
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
        "return" => TokenKind::Return,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "elseif" => TokenKind::ElseIf,
        "while" => TokenKind::While,
        "echo" => TokenKind::Echo,
        "print" => TokenKind::Print,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        "int" => TokenKind::KwInt,
        "float" => TokenKind::KwFloat,
        "string" => TokenKind::KwString,
        "bool" => TokenKind::KwBool,
        "void" => TokenKind::KwVoid,
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
        assert!(!diags.has_errors(), "unexpected lex errors: {:?}", diags.items());
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
        assert_eq!(k[0], TokenKind::Str("a\nb".to_string()));
        let k = kinds(r"'it\'s'");
        assert_eq!(k[0], TokenKind::Str("it's".to_string()));
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
    fn interpolation_warns() {
        let (_, diags) = lex(r#""hi $name""#);
        assert!(!diags.has_errors());
        assert!(diags.items().iter().any(|d| d.code == Some("W0002")));
    }
}
