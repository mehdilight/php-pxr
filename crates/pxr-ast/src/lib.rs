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
        target: LValue,
        ty: Option<TypeRef>,
        value: Expr,
        span: Span,
    },
    /// `$x += value;` and the other compound forms (`-=`, `*=`, `/=`, `%=`, `.=`, ...).
    AssignOp {
        target: LValue,
        op: BinOp,
        value: Expr,
        span: Span,
    },
    /// `$x++;` / `$x--;` (prefix and postfix are equivalent in statement position).
    IncDec {
        target: LValue,
        delta: i32,
        span: Span,
    },
    FnDecl(Box<FnDecl>),
    ClassDecl(Box<ClassDecl>),
    /// File-loader-resolved interpreter boundary, not parsed from source directly.
    PhpInclude {
        path: String,
        mode: u8,
        span: Span,
    },
    Require {
        path: String,
        once: bool,
        required: bool,
        dir_relative: bool,
        span: Span,
    },
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
    /// `for (init; cond; update) body` — each clause may contain several
    /// comma-separated parts, matching PHP.
    For {
        init: Vec<Stmt>,
        cond: Vec<Expr>,
        update: Vec<Stmt>,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `do body while (cond);`
    DoWhile {
        body: Vec<Stmt>,
        cond: Expr,
        span: Span,
    },
    /// `foreach ($subject as $value)` / `foreach ($subject as $key => $value)`.
    Foreach {
        subject: Expr,
        key: Option<String>,
        key_span: Span,
        value: String,
        value_span: Span,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `switch ($subject) { case ...: ... default: ... }` (with fall-through).
    Switch {
        subject: Expr,
        cases: Vec<SwitchCase>,
        span: Span,
    },
    /// `break;` / `break N;`
    Break(Option<u32>, Span),
    /// `continue;` / `continue N;`
    Continue(Option<u32>, Span),
    Block(Vec<Stmt>, Span),
    Empty,
}

impl Stmt {
    /// Statement lists owned by this node, including declaration bodies.
    pub fn bodies(&self) -> Vec<&[Stmt]> {
        match self {
            Stmt::FnDecl(f) => vec![&f.body],
            Stmt::ClassDecl(c) => c
                .members
                .iter()
                .filter_map(|m| match m {
                    ClassMember::Method(m) => Some(m.body.as_slice()),
                    _ => None,
                })
                .collect(),
            Stmt::Block(body, _)
            | Stmt::While { body, .. }
            | Stmt::DoWhile { body, .. }
            | Stmt::Foreach { body, .. } => vec![body],
            Stmt::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                let mut bodies = vec![then_branch.as_slice()];
                bodies.extend(elifs.iter().map(|(_, body)| body.as_slice()));
                if let Some(body) = else_branch {
                    bodies.push(body);
                }
                bodies
            }
            Stmt::For {
                init, update, body, ..
            } => vec![init, update, body],
            Stmt::Switch { cases, .. } => cases.iter().map(|case| case.body.as_slice()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn bodies_mut(&mut self) -> Vec<&mut Vec<Stmt>> {
        match self {
            Stmt::FnDecl(f) => vec![&mut f.body],
            Stmt::ClassDecl(c) => c
                .members
                .iter_mut()
                .filter_map(|m| match m {
                    ClassMember::Method(m) => Some(&mut m.body),
                    _ => None,
                })
                .collect(),
            Stmt::Block(body, _)
            | Stmt::While { body, .. }
            | Stmt::DoWhile { body, .. }
            | Stmt::Foreach { body, .. } => vec![body],
            Stmt::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                let mut bodies = vec![then_branch];
                bodies.extend(elifs.iter_mut().map(|(_, body)| body));
                if let Some(body) = else_branch {
                    bodies.push(body);
                }
                bodies
            }
            Stmt::For {
                init, update, body, ..
            } => vec![init, update, body],
            Stmt::Switch { cases, .. } => cases.iter_mut().map(|case| &mut case.body).collect(),
            _ => Vec::new(),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Stmt::Echo(_, sp)
            | Stmt::Print(_, sp)
            | Stmt::Expr(_, sp)
            | Stmt::Return(_, sp)
            | Stmt::Break(_, sp)
            | Stmt::Continue(_, sp) => *sp,
            Stmt::PhpInclude { span, .. }
            | Stmt::Require { span, .. }
            | Stmt::Assign { span, .. }
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
            Stmt::ClassDecl(c) => c.span,
            Stmt::Empty => Span::synthetic(),
        }
    }
}

/// One `case <expr>:` or `default:` arm of a `switch`.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchCase {
    /// `None` for `default`.
    pub test: Option<Expr>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// A syntactic assignment target.
#[derive(Clone, Debug, PartialEq)]
pub enum LValue {
    Var {
        name: String,
        span: Span,
    },
    /// `$base[index]`; `index` is `None` for the push form `$base[]`.
    Index {
        base: Box<LValue>,
        index: Option<Box<Expr>>,
        span: Span,
    },
    StaticProperty {
        class_name: String,
        type_args: Vec<TypeRef>,
        name: String,
        span: Span,
    },
    /// `$base->prop`.
    Property {
        base: Box<Expr>,
        name: String,
        name_span: Span,
        span: Span,
    },
}

impl LValue {
    pub fn span(&self) -> Span {
        match self {
            LValue::Var { span, .. } => *span,
            LValue::Index { span, .. } => *span,
            LValue::Property { span, .. } | LValue::StaticProperty { span, .. } => *span,
        }
    }

    /// The root variable name of this target (`$a[0][1]` -> `a`).
    pub fn root_name(&self) -> &str {
        match self {
            LValue::Var { name, .. } => name,
            LValue::Index { base, .. } => base.root_name(),
            LValue::Property { name, .. } | LValue::StaticProperty { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub type_params: Vec<(String, Span)>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassDecl {
    pub type_params: Vec<(String, Span)>,
    pub is_interface: bool,
    /// Implemented interfaces, or extended interfaces for an interface declaration.
    pub interfaces: Vec<ClassRef>,
    pub is_abstract: bool,
    pub name: String,
    pub name_span: Span,
    pub extends: Option<ClassRef>,
    pub members: Vec<ClassMember>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassRef {
    pub name: String,
    pub type_args: Vec<TypeRef>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClassMember {
    Property(PropertyDecl),
    Method(MethodDecl),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDecl {
    pub is_static: bool,
    pub visibility: Visibility,
    pub ty: Option<TypeRef>,
    pub name: String,
    pub name_span: Span,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    pub type_params: Vec<(String, Span)>,
    pub is_static: bool,
    pub is_abstract: bool,
    pub visibility: Visibility,
    pub name: String,
    pub name_span: Span,
    pub params: Vec<Param>,
    pub ret: Option<TypeRef>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// A syntactic type reference, before resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum TypeRef {
    /// A lexically scoped generic parameter.
    Param(String, Span),
    Applied(String, Vec<TypeRef>, Span),
    /// Internal element marker retained when specializing an inferred empty array.
    EmptyElement,
    Mixed,
    Int,
    Float,
    String,
    Bool,
    Void,
    /// `array<T>` — a PXR array whose values are all of type `T`. Keys are
    /// dynamically `int` or `string`, mirroring PHP's ordered maps.
    Array(Box<TypeRef>),
    Named(String),
    Span(Span),
}

impl TypeRef {
    pub fn span(&self) -> Span {
        match self {
            TypeRef::Span(s) | TypeRef::Param(_, s) | TypeRef::Applied(_, _, s) => *s,
            _ => Span::synthetic(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `~` — bitwise NOT.
    BitNot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    Eq,
    NotEq,
    Identical,
    NotIdentical,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Spaceship,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    NullCoalesce,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
            BinOp::Concat => ".",
            BinOp::Eq => "==",
            BinOp::NotEq => "!=",
            BinOp::Identical => "===",
            BinOp::NotIdentical => "!==",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::Spaceship => "<=>",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::NullCoalesce => "??",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    IntLit(i64, Span),
    FloatLit(f64, Span),
    StrLit(Vec<u8>, Span),
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
    /// `cond ? then : else`; `then` is `None` for the short form `cond ?: else`.
    Ternary {
        cond: Box<Expr>,
        then: Option<Box<Expr>>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `base[index]` with `None` for the push form `base[]`.
    Index {
        base: Box<Expr>,
        index: Option<Box<Expr>>,
        span: Span,
    },
    /// `[k => v, v2, ...]`.
    ArrayLit {
        items: Vec<ArrayItem>,
        span: Span,
    },
    /// `match (subject) { conds => result, default => result }`.
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Call {
        /// Global fallback for an unqualified call in a namespace.
        fallback: Option<String>,
        callee: String,
        callee_span: Span,
        type_args: Vec<TypeRef>,
        args: Vec<Expr>,
        span: Span,
    },
    Property {
        base: Box<Expr>,
        name: String,
        name_span: Span,
        span: Span,
    },
    MethodCall {
        base: Box<Expr>,
        name: String,
        type_args: Vec<TypeRef>,
        name_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    New {
        class_name: String,
        type_args: Vec<TypeRef>,
        class_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    Instanceof {
        expr: Box<Expr>,
        class_name: String,
        type_args: Vec<TypeRef>,
        class_span: Span,
        span: Span,
    },
    StaticProperty {
        class_name: String,
        type_args: Vec<TypeRef>,
        name: String,
        span: Span,
    },
    StaticCall {
        class_name: String,
        type_args: Vec<TypeRef>,
        method: String,
        method_type_args: Vec<TypeRef>,
        args: Vec<Expr>,
        span: Span,
    },
    ParentMethodCall {
        method: String,
        type_args: Vec<TypeRef>,
        method_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    Paren(Box<Expr>, Span),
}

/// One element of an array literal.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayItem {
    /// `Some` for `key => value`, `None` for an implicit integer key.
    pub key: Option<Expr>,
    pub value: Expr,
    pub span: Span,
}

/// One arm of a `match` expression.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    /// The match conditions; `is_default` is true for `default`.
    pub conds: Vec<Expr>,
    pub is_default: bool,
    pub result: Expr,
    pub span: Span,
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
            Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Ternary { span, .. }
            | Expr::Index { span, .. }
            | Expr::ArrayLit { span, .. }
            | Expr::Match { span, .. }
            | Expr::Call { span, .. }
            | Expr::Property { span, .. }
            | Expr::MethodCall { span, .. }
            | Expr::New { span, .. }
            | Expr::Instanceof { span, .. }
            | Expr::ParentMethodCall { span, .. }
            | Expr::StaticProperty { span, .. }
            | Expr::StaticCall { span, .. } => *span,
        }
    }
}
