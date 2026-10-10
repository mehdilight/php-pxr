//! The PXR abstract syntax tree.
//!
//! The AST is the untyped, source-faithful representation produced by the
//! parser. Type information is attached later, during semantic analysis, and
//! lives in the typed AST owned by `pxr-sema`.

use pxr_diag::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub items: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    /// `echo a, b, c;` and `print e;`
    Echo(Vec<Expr>, Span),
    Print(Expr, Span),
    /// An expression evaluated for its side effects, e.g. a function call.
    Expr(Expr, Span),
    /// `$x = value;` optionally with a PXR type annotation `$x: int = value;`
    Assign {
        name: String,
        ty: Option<TypeRef>,
        value: Expr,
        span: Span,
    },
    FnDecl(Box<FnDecl>),
    Return(Option<Expr>, Span),
    If {
        cond: Expr,
        then_branch: Vec<Stmt>,
        elifs: Vec<(Expr, Vec<Stmt>)>,
        else_branch: Option<Vec<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    Block(Vec<Stmt>, Span),
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub name_span: Span,
    pub ty: Option<TypeRef>,
    pub span: Span,
}

/// A syntactic type reference, before resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    Int,
    Float,
    String,
    Bool,
    Void,
    Named(String),
    Span(Span),
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Span(s) => *s,
            _ => Span::synthetic(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
    Eq,
    NotEq,
    Identical,
    NotIdentical,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Concat => ".",
            BinOp::Eq => "==",
            BinOp::NotEq => "!=",
            BinOp::Identical => "===",
            BinOp::NotIdentical => "!==",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StrLit(String, Span),
    BoolLit(bool, Span),
    NullLit(Span),
    Var(String, Span),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Call {
        callee: String,
        callee_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    Paren(Box<Expr>, Span),
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::IntLit(_, s)
            | Expr::FloatLit(_, s)
            | Expr::StrLit(_, s)
            | Expr::BoolLit(_, s)
            | Expr::NullLit(s)
            | Expr::Var(_, s)
            | Expr::Paren(_, s) => *s,
            Expr::Unary { span, .. } | Expr::Binary { span, .. } | Expr::Call { span, .. } => *span,
        }
    }
}
