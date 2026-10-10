//! PXR semantic analysis and static type checking.
//!
//! The analyzer consumes the untyped [`pxr_ast::Program`] and produces a
//! **typed program** ([`TProgram`]) in which every expression carries a fully
//! resolved type, every variable reference is bound to a stack slot, and every
//! call is bound to a concrete callee (a user function or a runtime builtin).
//!
//! Everything the language guarantees is enforced here, before any machine
//! code is produced:
//!
//! * undefined variables and functions are rejected,
//! * argument counts and argument types are checked,
//! * return types are checked and non-void functions must return on every path,
//! * operand types of every operator are validated, with the single implicit
//!   coercion of the PXR core: lossless `int` → `float` widening.

use pxr_ast::{self as ast, BinOp as ABinOp, UnOp as AUnOp};
use pxr_diag::{Diagnostic, Diagnostics, Span};
use pxr_types::Ty;
use std::collections::HashMap;

/// Aliases re-exported so downstream stages can refer to the operator enums
/// through the semantic layer.
pub use pxr_ast::{BinOp as TBinOp, UnOp as TUnOp};

// ---------------------------------------------------------------------------
// Typed AST
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Strlen,
    Intval,
    Floatval,
    Strval,
    Abs,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "strlen" => Builtin::Strlen,
            "intval" => Builtin::Intval,
            "floatval" => Builtin::Floatval,
            "strval" => Builtin::Strval,
            "abs" => Builtin::Abs,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Strlen => "strlen",
            Builtin::Intval => "intval",
            Builtin::Floatval => "floatval",
            Builtin::Strval => "strval",
            Builtin::Abs => "abs",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Callee {
    User {
        /// Link-time (mangled) name.
        mangled: String,
        /// Display name.
        name: String,
        params: Vec<Ty>,
    },
    Builtin {
        kind: Builtin,
        /// The single argument's type, after semantic resolution.
        arg_ty: Ty,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TExpr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Var {
        name: String,
        slot: u32,
        ty: Ty,
    },
    Un {
        op: AUnOp,
        operand_ty: Ty,
        ty: Ty,
        expr: Box<TExpr>,
    },
    Bin {
        op: ABinOp,
        lhs_ty: Ty,
        rhs_ty: Ty,
        ty: Ty,
        lhs: Box<TExpr>,
        rhs: Box<TExpr>,
    },
    Call {
        callee: Callee,
        ret: Ty,
        args: Vec<TExpr>,
        /// Actual argument types, aligned with `args`.
        arg_tys: Vec<Ty>,
        span: Span,
    },
}

impl TExpr {
    pub fn ty(&self) -> Ty {
        match self {
            TExpr::Int(_) => Ty::Int,
            TExpr::Float(_) => Ty::Float,
            TExpr::Str(_) => Ty::String,
            TExpr::Bool(_) => Ty::Bool,
            TExpr::Var { ty, .. } => ty.clone(),
            TExpr::Un { ty, .. } => ty.clone(),
            TExpr::Bin { ty, .. } => ty.clone(),
            TExpr::Call { ret, .. } => ret.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TStmt {
    Block(Vec<TStmt>),
    Echo(Vec<TExpr>),
    Print(TExpr),
    Expr(TExpr),
    Assign {
        slot: u32,
        ty: Ty,
        value: TExpr,
    },
    If {
        cond: TExpr,
        cond_ty: Ty,
        then_branch: Vec<TStmt>,
        elifs: Vec<(TExpr, Ty, Vec<TStmt>)>,
        else_branch: Option<Vec<TStmt>>,
    },
    While {
        cond: TExpr,
        cond_ty: Ty,
        body: Vec<TStmt>,
    },
    Return {
        value: Option<TExpr>,
        ret: Ty,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct TFunc {
    /// Link-time name.
    pub name: String,
    /// Display name.
    pub display: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    pub slots: Vec<Ty>,
    pub body: Vec<TStmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TProgram {
    pub funcs: Vec<TFunc>,
    /// The synthesized entry point running top-level statements.
    pub main: TFunc,
}

// ---------------------------------------------------------------------------
// Analyzer
// ---------------------------------------------------------------------------

pub struct Analyzer {
    diags: Diagnostics,
    /// lowercased name -> signature
    funcs: HashMap<String, Signature>,
}

#[derive(Clone, Debug)]
struct Signature {
    mangled: String,
    display: String,
    params: Vec<Ty>,
    param_names: Vec<String>,
    ret: Ty,
    span: Span,
}

/// Result of analysing one function body.
struct FnCtx {
    slots: Vec<Ty>,
    vars: HashMap<String, u32>,
    ret: Ty,
    is_main: bool,
}

impl FnCtx {
    fn new(ret: Ty, is_main: bool) -> Self {
        FnCtx {
            slots: Vec::new(),
            vars: HashMap::new(),
            ret,
            is_main,
        }
    }

    fn slot_of(&self, name: &str) -> Option<u32> {
        self.vars.get(name).copied()
    }

    fn declare(&mut self, name: &str, ty: Ty) -> u32 {
        let slot = self.slots.len() as u32;
        self.slots.push(ty);
        self.vars.insert(name.to_string(), slot);
        slot
    }

    fn slot_ty(&self, slot: u32) -> Ty {
        self.slots[slot as usize].clone()
    }
}

pub fn analyze(program: &ast::Program) -> (Option<TProgram>, Diagnostics) {
    let mut a = Analyzer {
        diags: Diagnostics::new(),
        funcs: HashMap::new(),
    };
    let result = a.run(program);
    (result, a.diags)
}

impl Analyzer {
    fn run(&mut self, program: &ast::Program) -> Option<TProgram> {
        self.collect_signatures(program);

        let mut funcs = Vec::new();
        for item in &program.items {
            if let ast::Stmt::FnDecl(f) = item {
                if let Some(tf) = self.analyze_function(f) {
                    funcs.push(tf);
                }
            }
        }

        // Remaining top-level statements form `main`.
        let main_stmts: Vec<ast::Stmt> = program
            .items
            .iter()
            .filter(|s| !matches!(s, ast::Stmt::FnDecl(_)))
            .cloned()
            .collect();
        let main = self.analyze_main(&main_stmts);

        if self.diags.has_errors() {
            return None;
        }
        Some(TProgram {
            funcs,
            main: main?,
        })
    }

    fn collect_signatures(&mut self, program: &ast::Program) {
        for item in &program.items {
            let ast::Stmt::FnDecl(f) = item else { continue };
            let key = f.name.to_ascii_lowercase();
            if let Some(prev) = self.funcs.get(&key) {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "function `{}` is already defined",
                        f.name
                    ))
                    .with_code("E0201")
                    .with_span(f.name_span)
                    .with_note(format!(
                        "previous definition is at byte offset {}",
                        prev.span.start
                    )),
                );
                continue;
            }

            let mut params = Vec::new();
            let mut param_names = Vec::new();
            for p in &f.params {
                let ty = match &p.ty {
                    Some(t) => self.resolve_type(t),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "parameter `${}` needs an explicit type; PXR does not infer parameter types",
                                p.name
                            ))
                            .with_code("E0202")
                            .with_span(p.name_span)
                            .with_note("write e.g. `int $x` (this is valid PHP 7+ syntax)"),
                        );
                        Ty::Int
                    }
                };
                if ty == Ty::Void {
                    self.diags.emit(
                        Diagnostic::error("a parameter cannot have type `void`")
                            .with_code("E0203")
                            .with_span(p.span),
                    );
                }
                params.push(ty);
                param_names.push(p.name.clone());
            }

            let ret = match &f.ret {
                Some(t) => self.resolve_type(t),
                None => {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "function `{}` needs an explicit return type",
                            f.name
                        ))
                        .with_code("E0204")
                        .with_span(f.name_span)
                        .with_note("write e.g. `function f(): int` or `: void`"),
                    );
                    Ty::Int
                }
            };

            self.funcs.insert(
                key,
                Signature {
                    mangled: mangle_fn(&f.name),
                    display: f.name.clone(),
                    params,
                    param_names,
                    ret,
                    span: f.span,
                },
            );
        }
    }

    fn resolve_type(&mut self, t: &ast::TypeRef) -> Ty {
        match t {
            ast::TypeRef::Int => Ty::Int,
            ast::TypeRef::Float => Ty::Float,
            ast::TypeRef::String => Ty::String,
            ast::TypeRef::Bool => Ty::Bool,
            ast::TypeRef::Void => Ty::Void,
            ast::TypeRef::Named(name) => {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "unknown type `{name}`; classes and user types are not implemented yet"
                    ))
                    .with_code("E0205")
                    .with_span(t.span()),
                );
                Ty::Int
            }
            ast::TypeRef::Span(_s) => Ty::Int,
        }
    }

    fn analyze_function(&mut self, f: &ast::FnDecl) -> Option<TFunc> {
        let sig = self.funcs.get(&f.name.to_ascii_lowercase()).cloned()?;
        let mut ctx = FnCtx::new(sig.ret.clone(), false);
        for (name, ty) in sig.param_names.iter().zip(sig.params.iter()) {
            ctx.declare(name, ty.clone());
        }
        let body = self.analyze_stmts(&f.body, &mut ctx);
        self.check_termination(&f.body, &sig, f.name_span);

        Some(TFunc {
            name: sig.mangled,
            display: sig.display,
            params: sig.param_names.iter().cloned().zip(sig.params).collect(),
            ret: sig.ret,
            slots: ctx.slots,
            body,
        })
    }

    fn analyze_main(&mut self, stmts: &[ast::Stmt]) -> Option<TFunc> {
        let mut ctx = FnCtx::new(Ty::Void, true);
        let body = self.analyze_stmts(stmts, &mut ctx);
        Some(TFunc {
            name: "main".to_string(),
            display: "main".to_string(),
            params: Vec::new(),
            ret: Ty::Void,
            slots: ctx.slots,
            body,
        })
    }

    /// A non-void function must end with a `return` on every path.
    fn check_termination(&mut self, body: &[ast::Stmt], sig: &Signature, span: Span) {
        if sig.ret == Ty::Void {
            return;
        }
        let terminates = body.last().is_some_and(|s| stmt_terminates(s));
        if !terminates {
            self.diags.emit(
                Diagnostic::error(format!(
                    "function `{}` must return a value of type `{}` on all paths",
                    sig.display, sig.ret
                ))
                .with_code("E0206")
                .with_span(span)
                .with_note("add a `return` statement as the final statement"),
            );
        }
    }

    fn analyze_stmts(&mut self, stmts: &[ast::Stmt], ctx: &mut FnCtx) -> Vec<TStmt> {
        let mut out = Vec::new();
        for s in stmts {
            if let Some(ts) = self.analyze_stmt(s, ctx) {
                out.push(ts);
            }
        }
        out
    }

    fn analyze_stmt(&mut self, stmt: &ast::Stmt, ctx: &mut FnCtx) -> Option<TStmt> {
        match stmt {
            ast::Stmt::Empty => None,
            ast::Stmt::Block(body, _) => {
                let inner = self.analyze_stmts(body, ctx);
                Some(TStmt::Block(inner))
            }
            ast::Stmt::Echo(exprs, _) => {
                let mut texprs = Vec::new();
                for e in exprs {
                    if let Some(t) = self.infer(e, ctx) {
                        texprs.push(t);
                    }
                }
                Some(TStmt::Echo(texprs))
            }
            ast::Stmt::Print(e, _) => {
                let t = self.infer(e, ctx)?;
                Some(TStmt::Print(t))
            }
            ast::Stmt::Expr(e, _) => {
                let t = self.infer(e, ctx)?;
                Some(TStmt::Expr(t))
            }
            ast::Stmt::Assign {
                name,
                ty,
                value,
                span,
            } => self.analyze_assign(name, ty.as_ref(), value, *span, ctx),
            ast::Stmt::FnDecl(f) => {
                self.diags.emit(
                    Diagnostic::error("nested function declarations are not supported yet")
                        .with_code("E0207")
                        .with_span(f.span),
                );
                None
            }
            ast::Stmt::Return(value, span) => self.analyze_return(value.as_ref(), *span, ctx),
            ast::Stmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                let cond_t = self.infer(cond, ctx)?;
                let cond_ty = cond_t.ty();
                if !is_truthy(&cond_ty) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "condition must be a scalar value, found `{cond_ty}`"
                        ))
                        .with_code("E0208")
                        .with_span(cond.span()),
                    );
                }
                let then_t = self.analyze_stmts(then_branch, ctx);
                let mut elifs_t = Vec::new();
                for (c, b) in elifs {
                    let ct = self.infer(c, ctx)?;
                    let cty = ct.ty();
                    if !is_truthy(&cty) {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "condition must be a scalar value, found `{cty}`"
                            ))
                            .with_code("E0208")
                            .with_span(c.span()),
                        );
                    }
                    let bt = self.analyze_stmts(b, ctx);
                    elifs_t.push((ct, cty, bt));
                }
                let else_t = else_branch
                    .as_ref()
                    .map(|b| self.analyze_stmts(b, ctx));
                Some(TStmt::If {
                    cond: cond_t,
                    cond_ty,
                    then_branch: then_t,
                    elifs: elifs_t,
                    else_branch: else_t,
                })
            }
            ast::Stmt::While { cond, body, .. } => {
                let cond_t = self.infer(cond, ctx)?;
                let cond_ty = cond_t.ty();
                if !is_truthy(&cond_ty) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "condition must be a scalar value, found `{cond_ty}`"
                        ))
                        .with_code("E0208")
                        .with_span(cond.span()),
                    );
                }
                let body_t = self.analyze_stmts(body, ctx);
                Some(TStmt::While {
                    cond: cond_t,
                    cond_ty,
                    body: body_t,
                })
            }
        }
    }

    fn analyze_assign(
        &mut self,
        name: &str,
        annotation: Option<&ast::TypeRef>,
        value: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let val = self.infer(value, ctx)?;
        let val_ty = val.ty();

        if val_ty == Ty::Null {
            self.diags.emit(
                Diagnostic::error("`null` is not supported yet; use a concrete value")
                    .with_code("E0209")
                    .with_span(value.span()),
            );
            return None;
        }

        let annotated = annotation.map(|t| self.resolve_type(t));

        let (slot, decl_ty) = match ctx.slot_of(name) {
            Some(slot) => {
                let existing = ctx.slot_ty(slot);
                if let Some(a) = &annotated {
                    if *a != existing {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "variable `${name}` is already of type `{existing}`, cannot redeclare as `{a}`"
                            ))
                            .with_code("E0210")
                            .with_span(span),
                        );
                        return None;
                    }
                }
                (slot, existing)
            }
            None => {
                let ty = annotated.clone().unwrap_or_else(|| val_ty.clone());
                if ty == Ty::Void {
                    self.diags.emit(
                        Diagnostic::error("a variable cannot have type `void`")
                            .with_code("E0211")
                            .with_span(span),
                    );
                    return None;
                }
                let slot = ctx.declare(name, ty.clone());
                (slot, ty)
            }
        };

        if !val_ty.assignable_to(&decl_ty) {
            self.diags.emit(
                Diagnostic::error(format!(
                    "cannot assign value of type `{val_ty}` to variable `${name}` of type `{decl_ty}`"
                ))
                .with_code("E0212")
                .with_span(span)
                .with_note("PXR only implicitly widens `int` to `float`"),
            );
            return None;
        }

        Some(TStmt::Assign {
            slot,
            ty: decl_ty,
            value: val,
        })
    }

    fn analyze_return(
        &mut self,
        value: Option<&ast::Expr>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let ret = ctx.ret.clone();
        if ctx.is_main {
            if let Some(v) = value {
                self.diags.emit(
                    Diagnostic::warning("`return` with a value in the top level of a script is ignored")
                        .with_code("W0202")
                        .with_span(v.span()),
                );
            }
            return Some(TStmt::Return { value: None, ret });
        }

        match (value, &ret) {
            (None, Ty::Void) => Some(TStmt::Return { value: None, ret }),
            (None, _) => {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "function must return a value of type `{ret}`"
                    ))
                    .with_code("E0213")
                    .with_span(span),
                );
                None
            }
            (Some(v), Ty::Void) => {
                self.diags.emit(
                    Diagnostic::error("void function must not return a value")
                        .with_code("E0214")
                        .with_span(v.span()),
                );
                None
            }
            (Some(v), _) => {
                let t = self.infer(v, ctx)?;
                let vt = t.ty();
                if !vt.assignable_to(&ret) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "returning `{vt}` from function declared to return `{ret}`"
                        ))
                        .with_code("E0215")
                        .with_span(v.span()),
                    );
                    return None;
                }
                Some(TStmt::Return {
                    value: Some(t),
                    ret,
                })
            }
        }
    }

    // ---- Expressions ------------------------------------------------------

    fn infer(&mut self, expr: &ast::Expr, ctx: &mut FnCtx) -> Option<TExpr> {
        match expr {
            ast::Expr::IntLit(v, _) => Some(TExpr::Int(*v)),
            ast::Expr::FloatLit(v, _) => Some(TExpr::Float(*v)),
            ast::Expr::StrLit(s, _) => Some(TExpr::Str(s.clone())),
            ast::Expr::BoolLit(b, _) => Some(TExpr::Bool(*b)),
            ast::Expr::NullLit(span) => {
                self.diags.emit(
                    Diagnostic::error("`null` is not supported yet")
                        .with_code("E0209")
                        .with_span(*span),
                );
                None
            }
            ast::Expr::Paren(inner, _) => self.infer(inner, ctx),
            ast::Expr::Var(name, span) => match ctx.slot_of(name) {
                Some(slot) => {
                    let ty = ctx.slot_ty(slot);
                    Some(TExpr::Var {
                        name: name.clone(),
                        slot,
                        ty,
                    })
                }
                None => {
                    self.diags.emit(
                        Diagnostic::error(format!("undefined variable `${name}`"))
                            .with_code("E0220")
                            .with_span(*span)
                            .with_note("variables must be assigned before they are used"),
                    );
                    None
                }
            },
            ast::Expr::Unary { op, expr, span } => {
                let operand = self.infer(expr, ctx)?;
                let oty = operand.ty();
                match op {
                    AUnOp::Neg => {
                        if !oty.is_numeric() {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "unary `-` requires a numeric operand, found `{oty}`"
                                ))
                                .with_code("E0221")
                                .with_span(*span),
                            );
                            return None;
                        }
                        Some(TExpr::Un {
                            op: AUnOp::Neg,
                            operand_ty: oty.clone(),
                            ty: oty,
                            expr: Box::new(operand),
                        })
                    }
                    AUnOp::Not => {
                        if !is_truthy(&oty) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "`!` requires a scalar operand, found `{oty}`"
                                ))
                                .with_code("E0221")
                                .with_span(*span),
                            );
                            return None;
                        }
                        Some(TExpr::Un {
                            op: AUnOp::Not,
                            operand_ty: oty,
                            ty: Ty::Bool,
                            expr: Box::new(operand),
                        })
                    }
                }
            }
            ast::Expr::Binary {
                op,
                lhs,
                rhs,
                span,
            } => self.infer_binary(*op, lhs, rhs, *span, ctx),
            ast::Expr::Call {
                callee,
                callee_span,
                args,
                span,
            } => self.infer_call(callee, *callee_span, args, *span, ctx),
        }
    }

    fn infer_binary(
        &mut self,
        op: ABinOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let lt = self.infer(lhs, ctx)?;
        let rt = self.infer(rhs, ctx)?;
        let l = lt.ty();
        let r = rt.ty();

        let mk = |op, lty: Ty, rty: Ty, ty: Ty, l: TExpr, r: TExpr| TExpr::Bin {
            op,
            lhs_ty: lty,
            rhs_ty: rty,
            ty,
            lhs: Box::new(l),
            rhs: Box::new(r),
        };

        match op {
            ABinOp::Add | ABinOp::Sub | ABinOp::Mul => {
                if !l.is_numeric() || !r.is_numeric() {
                    return self.bad_operands(op, &l, &r, span);
                }
                let ty = numeric_result(&l, &r);
                Some(mk(op, l, r, ty, lt, rt))
            }
            ABinOp::Div => {
                if !l.is_numeric() || !r.is_numeric() {
                    return self.bad_operands(op, &l, &r, span);
                }
                // PHP `/` always produces a float.
                Some(mk(op, l, r, Ty::Float, lt, rt))
            }
            ABinOp::Mod => {
                if l != Ty::Int || r != Ty::Int {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`%` requires two `int` operands, found `{l}` and `{r}`"
                        ))
                        .with_code("E0222")
                        .with_span(span)
                        .with_note("use `fmod()` for floating point remainders (not yet implemented)"),
                    );
                    return None;
                }
                Some(mk(op, l, r, Ty::Int, lt, rt))
            }
            ABinOp::Concat => {
                for t in [&l, &r] {
                    if !matches!(t, Ty::String | Ty::Int | Ty::Float | Ty::Bool) {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "`.` cannot concatenate a value of type `{t}`"
                            ))
                            .with_code("E0223")
                            .with_span(span),
                        );
                        return None;
                    }
                }
                Some(mk(op, l, r, Ty::String, lt, rt))
            }
            ABinOp::Eq | ABinOp::NotEq => {
                let ok = (l.is_numeric() && r.is_numeric())
                    || (l == Ty::String && r == Ty::String)
                    || (l == Ty::Bool && r == Ty::Bool);
                if !ok {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot compare `{l}` and `{r}` with `{}`",
                            op.symbol()
                        ))
                        .with_code("E0224")
                        .with_span(span)
                        .with_note("operands must both be numeric, both `string`, or both `bool`"),
                    );
                    return None;
                }
                Some(mk(op, l, r, Ty::Bool, lt, rt))
            }
            ABinOp::Identical | ABinOp::NotIdentical => {
                let ok = (l == r) && l.is_scalar();
                if !ok {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`{}` requires operands of the same type, found `{l}` and `{r}`",
                            op.symbol()
                        ))
                        .with_code("E0225")
                        .with_span(span)
                        .with_note("use `==` for numeric comparison across `int`/`float`"),
                    );
                    return None;
                }
                Some(mk(op, l, r, Ty::Bool, lt, rt))
            }
            ABinOp::Lt | ABinOp::LtEq | ABinOp::Gt | ABinOp::GtEq => {
                let ok = (l.is_numeric() && r.is_numeric()) || (l == Ty::String && r == Ty::String);
                if !ok {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot order `{l}` and `{r}` with `{}`",
                            op.symbol()
                        ))
                        .with_code("E0226")
                        .with_span(span)
                        .with_note("operands must both be numeric or both `string`"),
                    );
                    return None;
                }
                Some(mk(op, l, r, Ty::Bool, lt, rt))
            }
            ABinOp::And | ABinOp::Or => {
                if !is_truthy(&l) || !is_truthy(&r) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`{}` requires scalar operands, found `{l}` and `{r}`",
                            op.symbol()
                        ))
                        .with_code("E0227")
                        .with_span(span),
                    );
                    return None;
                }
                Some(mk(op, l, r, Ty::Bool, lt, rt))
            }
        }
    }

    fn bad_operands<T>(
        &mut self,
        op: ABinOp,
        l: &Ty,
        r: &Ty,
        span: Span,
    ) -> Option<T> {
        self.diags.emit(
            Diagnostic::error(format!(
                "operator `{}` cannot be applied to `{l}` and `{r}`",
                op.symbol()
            ))
            .with_code("E0228")
            .with_span(span)
            .with_note("arithmetic operators require numeric operands"),
        );
        None
    }

    fn infer_call(
        &mut self,
        callee: &str,
        callee_span: Span,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let key = callee.to_ascii_lowercase();

        // User-defined function?
        if let Some(sig) = self.funcs.get(&key).cloned() {
            if args.len() != sig.params.len() {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "function `{}` expects {} argument(s), but {} were given",
                        sig.display,
                        sig.params.len(),
                        args.len()
                    ))
                    .with_code("E0230")
                    .with_span(span),
                );
                return None;
            }
            let mut targs = Vec::new();
            let mut arg_tys = Vec::new();
            let mut ok = true;
            for (i, (a, pty)) in args.iter().zip(sig.params.iter()).enumerate() {
                match self.infer(a, ctx) {
                    Some(t) => {
                        let at = t.ty();
                        if !at.assignable_to(pty) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "argument {} of `{}` expects `{pty}`, found `{at}`",
                                    i + 1,
                                    sig.display
                                ))
                                .with_code("E0231")
                                .with_span(a.span()),
                            );
                            ok = false;
                        }
                        arg_tys.push(at);
                        targs.push(t);
                    }
                    None => ok = false,
                }
            }
            if !ok {
                return None;
            }
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: sig.mangled,
                    name: sig.display,
                    params: sig.params,
                },
                ret: sig.ret,
                args: targs,
                arg_tys,
                span,
            });
        }

        // Builtin?
        if let Some(kind) = Builtin::from_name(&key) {
            return self.infer_builtin(kind, callee, callee_span, args, span, ctx);
        }

        self.diags.emit(
            Diagnostic::error(format!("call to undefined function `{callee}`"))
                .with_code("E0232")
                .with_span(callee_span),
        );
        None
    }

    fn infer_builtin(
        &mut self,
        kind: Builtin,
        callee: &str,
        callee_span: Span,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        if args.len() != 1 {
            self.diags.emit(
                Diagnostic::error(format!(
                    "builtin `{}` expects 1 argument, but {} were given",
                    kind.name(),
                    args.len()
                ))
                .with_code("E0233")
                .with_span(span),
            );
            return None;
        }
        let arg = self.infer(&args[0], ctx)?;
        let at = arg.ty();

        let ret = match kind {
            Builtin::Strlen => {
                if at != Ty::String {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`strlen` expects a `string`, found `{at}`"
                        ))
                        .with_code("E0234")
                        .with_span(args[0].span()),
                    );
                    return None;
                }
                Ty::Int
            }
            Builtin::Intval => {
                if !matches!(at, Ty::Int | Ty::Float | Ty::Bool | Ty::String) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`intval` expects a scalar, found `{at}`"
                        ))
                        .with_code("E0234")
                        .with_span(args[0].span()),
                    );
                    return None;
                }
                Ty::Int
            }
            Builtin::Floatval => {
                if !matches!(at, Ty::Int | Ty::Float | Ty::Bool | Ty::String) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`floatval` expects a scalar, found `{at}`"
                        ))
                        .with_code("E0234")
                        .with_span(args[0].span()),
                    );
                    return None;
                }
                Ty::Float
            }
            Builtin::Strval => {
                if !matches!(at, Ty::Int | Ty::Float | Ty::Bool | Ty::String) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`strval` expects a scalar, found `{at}`"
                        ))
                        .with_code("E0234")
                        .with_span(args[0].span()),
                    );
                    return None;
                }
                Ty::String
            }
            Builtin::Abs => {
                if !at.is_numeric() {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`abs` expects `int` or `float`, found `{at}`"
                        ))
                        .with_code("E0234")
                        .with_span(args[0].span()),
                    );
                    return None;
                }
                at.clone()
            }
        };

        let _ = callee_span;
        let _ = callee;
        Some(TExpr::Call {
            callee: Callee::Builtin {
                kind,
                arg_ty: at.clone(),
            },
            ret,
            arg_tys: vec![at],
            args: vec![arg],
            span,
        })
    }
}

fn numeric_result(l: &Ty, r: &Ty) -> Ty {
    if *l == Ty::Float || *r == Ty::Float {
        Ty::Float
    } else {
        Ty::Int
    }
}

/// Types that can be used in boolean context (`if`, `while`, `&&`, `||`, `!`).
pub fn is_truthy(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool | Ty::Int | Ty::Float | Ty::String)
}

/// Does this statement provably end with a `return` on all paths?
fn stmt_terminates(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Return(..) => true,
        ast::Stmt::Block(b, _) => b.last().is_some_and(stmt_terminates),
        ast::Stmt::If {
            then_branch,
            elifs,
            else_branch,
            ..
        } => {
            let all_then = then_branch.last().is_some_and(stmt_terminates);
            let all_elif = elifs
                .iter()
                .all(|(_, b)| b.last().is_some_and(stmt_terminates));
            let all_else = else_branch
                .as_ref()
                .is_some_and(|b| b.last().is_some_and(stmt_terminates));
            all_then && all_elif && all_else
        }
        _ => false,
    }
}

pub fn mangle_fn(name: &str) -> String {
    format!("pxr$fn${}", name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Option<TProgram>, Diagnostics) {
        let (prog, pdiags) = pxr_parser::parse(src);
        assert!(!pdiags.has_errors(), "parse errors: {:#?}", pdiags.items());
        let mut result = analyze(&prog.unwrap());
        result.1.extend(pdiags);
        result
    }

    #[test]
    fn hello_ok() {
        let (p, d) = run("<?php echo \"hi\";");
        assert!(!d.has_errors(), "{:#?}", d.items());
        assert!(p.is_some());
    }

    #[test]
    fn typed_arithmetic() {
        let (_, d) = run("<?php function add(int $a, int $b): int { return $a + $b; } echo add(1,2);");
        assert!(!d.has_errors(), "{:#?}", d.items());
    }

    #[test]
    fn int_to_float_widening() {
        let (_, d) = run("<?php $x: float = 1; echo $x;");
        assert!(!d.has_errors(), "{:#?}", d.items());
    }

    #[test]
    fn undefined_variable() {
        let (_, d) = run("<?php echo $x;");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0220")));
    }

    #[test]
    fn type_mismatch_argument() {
        let (_, d) = run("<?php function f(int $a): int { return $a; } echo f(\"x\");");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0231")));
    }

    #[test]
    fn missing_return() {
        let (_, d) = run("<?php function f(): int { $x = 1; }");
        assert!(d.items().iter().any(|x| x.code == Some("E0206")));
    }

    #[test]
    fn string_int_add_is_error() {
        let (_, d) = run("<?php echo 1 + \"a\";");
        assert!(d.has_errors());
    }

    #[test]
    fn undefined_function() {
        let (_, d) = run("<?php nope();");
        assert!(d.items().iter().any(|x| x.code == Some("E0232")));
    }

    #[test]
    fn concat_ok_with_numbers() {
        let (_, d) = run("<?php echo \"n=\" . 42;");
        assert!(!d.has_errors(), "{:#?}", d.items());
    }
}
