//! Diagnostics: source spans, severities, and human friendly rendering.
//!
//! All spans are **byte offsets** into the original source file, which keeps
//! them cheap to store and trivially comparable across compiler stages.

use std::fmt::Write as _;

/// A half-open byte range `[start, end)` into a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    pub const fn empty(at: usize) -> Self {
        Span { start: at, end: at }
    }

    /// Smallest span covering both `self` and `other`.
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// A synthetic span used for compiler-generated nodes.
    pub const fn synthetic() -> Self {
        Span { start: 0, end: 0 }
    }

    pub fn is_synthetic(&self) -> bool {
        self.start == 0 && self.end == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// Optional source context for a diagnostic originating in another module.
    pub source: Option<(String, String)>,
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
    pub notes: Vec<String>,
    /// Optional stable identifier such as `E0042`, useful for documentation
    /// and for tests that assert on a specific class of error.
    pub code: Option<&'static str>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Diagnostic {
            source: None,
            severity: Severity::Error,
            message: message.into(),
            span: None,
            notes: Vec::new(),
            code: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Diagnostic {
            source: None,
            severity: Severity::Warning,
            message: message.into(),
            span: None,
            notes: Vec::new(),
            code: None,
        }
    }

    pub fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_opt_span(mut self, span: Option<Span>) -> Self {
        self.span = span;
        self
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A collection of diagnostics emitted while compiling a single unit.
#[derive(Clone, Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Diagnostics { items: Vec::new() }
    }

    pub fn emit(&mut self, diag: Diagnostic) {
        self.items.push(diag);
    }

    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        self.emit(Diagnostic::error(message).with_span(span));
    }

    pub fn warning(&mut self, span: Span, message: impl Into<String>) {
        self.emit(Diagnostic::warning(message).with_span(span));
    }

    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(Diagnostic::is_error)
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.is_error()).count()
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    /// Render all diagnostics against `source`, one block per diagnostic.
    pub fn render(&self, source: &str, filename: &str) -> String {
        let mut out = String::new();
        for diag in &self.items {
            render_one(&mut out, diag, source, filename);
        }
        out
    }
}

fn render_one(out: &mut String, diag: &Diagnostic, source: &str, filename: &str) {
    let (source, filename) = diag
        .source
        .as_ref()
        .map(|(name, text)| (text.as_str(), name.as_str()))
        .unwrap_or((source, filename));
    let code = diag.code.map(|c| format!("[{c}] ")).unwrap_or_default();
    let _ = writeln!(
        out,
        "{}: {}: {}{}",
        filename,
        diag.severity.label(),
        code,
        diag.message
    );

    if let Some(span) = diag.span {
        if !span.is_synthetic() {
            if let Some((line_no, col, line_text, caret_len)) = locate(source, span) {
                let gutter = format!("{:>4} | ", line_no);
                let pad = " ".repeat(gutter.len());
                let _ = writeln!(out, "{pad}");
                let _ = writeln!(out, "{gutter}{line_text}");
                let _ = writeln!(
                    out,
                    "{pad}{}{}",
                    " ".repeat(col),
                    "^".repeat(caret_len.max(1))
                );
            }
        }
    }

    for note in &diag.notes {
        let _ = writeln!(out, "  = note: {note}");
    }
}

/// Returns `(1-based line number, 0-based caret column, line text, caret len)`.
fn locate(source: &str, span: Span) -> Option<(usize, usize, String, usize)> {
    let bytes = source.as_bytes();
    let start = span.start.min(bytes.len());

    // Line number and the offset of the start of the line containing `start`.
    let mut line_no = 1usize;
    let mut line_start = 0usize;
    for (i, b) in bytes.iter().enumerate().take(start) {
        if *b == b'\n' {
            line_no += 1;
            line_start = i + 1;
        }
    }

    // End of that line.
    let line_end = bytes[line_start..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|p| line_start + p)
        .unwrap_or(bytes.len());

    let line_text = String::from_utf8_lossy(&bytes[line_start..line_end]).into_owned();

    // Column measured in characters, not bytes, so the caret lines up in
    // terminals even when the line contains multi-byte UTF-8.
    let col = source[line_start..start].chars().count();

    let end = span.end.min(line_end).max(start);
    let caret_len = source[start..end].chars().count();

    Some((line_no, col, line_text, caret_len))
}

/// Convenience alias used across compiler stages.
pub type Result<T> = std::result::Result<T, CompileError>;

/// A top level compile error carrying already-collected diagnostics.
#[derive(Clone, Debug)]
pub struct CompileError {
    pub diagnostics: Diagnostics,
}

impl CompileError {
    pub fn new(diagnostics: Diagnostics) -> Self {
        CompileError { diagnostics }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} compile error(s)", self.diagnostics.error_count())
    }
}

impl std::error::Error for CompileError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_merge() {
        let a = Span::new(2, 5);
        let b = Span::new(10, 12);
        assert_eq!(a.merge(b), Span::new(2, 12));
    }

    #[test]
    fn renders_caret() {
        let src = "echo $x;\n";
        let mut d = Diagnostics::new();
        d.error(Span::new(5, 7), "undefined variable `$x`");
        let rendered = d.render(src, "main.php");
        assert!(rendered.contains("error"));
        assert!(rendered.contains("undefined variable"));
        assert!(rendered.contains("^"));
    }
}
