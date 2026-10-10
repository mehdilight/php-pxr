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
    /// `count($array)` — number of elements.
    Count,
    /// `strtoupper($str)` — ASCII case conversion (bytes only, like PHP).
    Strtoupper,
    Strtolower,
    /// `trim($str[, $charlist])`.
    Trim,
    Ltrim,
    Rtrim,
    /// `substr($str, $offset[, $length])`.
    Substr,
    /// `str_repeat($str, $times)`.
    StrRepeat,
    /// `str_replace($search, $replace, $subject)` — string arguments only.
    StrReplace,
    /// `implode($glue, $pieces)` / `implode($pieces)`.
    Implode,
    /// `explode($separator, $str[, $limit])` → `array<string>`.
    Explode,
    /// `str_contains($haystack, $needle): bool` and friends (case-sensitive).
    StrContains,
    StrStartsWith,
    StrEndsWith,
    /// ASCII-only, byte-oriented case helpers.
    Strrev,
    Ucfirst,
    Lcfirst,
    Ucwords,
    /// `str_split($str[, $length])` → `array<string>`.
    StrSplit,
    /// `array_values($array)` — reindexed, keys dropped.
    ArrayValues,
    /// `array_reverse($array[, $preserve_keys])`.
    ArrayReverse,
    /// `array_merge($array, ...$arrays)` — later values win on string keys,
    /// integer keys are renumbered; all arrays must share one element type.
    ArrayMerge,
    /// `array_slice($array, $offset[, $length[, $preserve_keys]])`.
    ArraySlice,
    /// `array_pad($array, $size, $value)`.
    ArrayPad,
    /// `array_fill($start, $count, $value)`.
    ArrayFill,
    /// `array_key_exists($key, $array)`.
    ArrayKeyExists,
    /// `str_pad($str, $length[, $pad[, $type]])`.
    StrPad,
    /// `substr_count($haystack, $needle[, $offset[, $length]])`.
    SubstrCount,
    /// `(string) -> string` escaping & encoding helpers.
    Addslashes,
    Stripslashes,
    Bin2hex,
    Base64Encode,
    Urlencode,
    Rawurlencode,
    Urldecode,
    Rawurldecode,
    /// `sprintf(string $format, ...$values): string`.
    Sprintf,
    /// `printf(string $format, ...$values): int`.
    Printf,
}

impl Builtin {
    fn from_name(name: &str) -> Option<Builtin> {
        Some(match name {
            "strlen" => Builtin::Strlen,
            "intval" => Builtin::Intval,
            "floatval" => Builtin::Floatval,
            "strval" => Builtin::Strval,
            "abs" => Builtin::Abs,
            "count" => Builtin::Count,
            "strtoupper" => Builtin::Strtoupper,
            "strtolower" => Builtin::Strtolower,
            "trim" => Builtin::Trim,
            "ltrim" => Builtin::Ltrim,
            "rtrim" => Builtin::Rtrim,
            "substr" => Builtin::Substr,
            "str_repeat" => Builtin::StrRepeat,
            "str_replace" => Builtin::StrReplace,
            "implode" => Builtin::Implode,
            "explode" => Builtin::Explode,
            "str_contains" => Builtin::StrContains,
            "str_starts_with" => Builtin::StrStartsWith,
            "str_ends_with" => Builtin::StrEndsWith,
            "strrev" => Builtin::Strrev,
            "ucfirst" => Builtin::Ucfirst,
            "lcfirst" => Builtin::Lcfirst,
            "ucwords" => Builtin::Ucwords,
            "str_split" => Builtin::StrSplit,
            "array_values" => Builtin::ArrayValues,
            "array_reverse" => Builtin::ArrayReverse,
            "array_merge" => Builtin::ArrayMerge,
            "array_slice" => Builtin::ArraySlice,
            "array_pad" => Builtin::ArrayPad,
            "array_fill" => Builtin::ArrayFill,
            "array_key_exists" => Builtin::ArrayKeyExists,
            "str_pad" => Builtin::StrPad,
            "substr_count" => Builtin::SubstrCount,
            "addslashes" => Builtin::Addslashes,
            "stripslashes" => Builtin::Stripslashes,
            "bin2hex" => Builtin::Bin2hex,
            "base64_encode" => Builtin::Base64Encode,
            "urlencode" => Builtin::Urlencode,
            "rawurlencode" => Builtin::Rawurlencode,
            "urldecode" => Builtin::Urldecode,
            "rawurldecode" => Builtin::Rawurldecode,
            "sprintf" => Builtin::Sprintf,
            "printf" => Builtin::Printf,
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
            Builtin::Count => "count",
            Builtin::Strtoupper => "strtoupper",
            Builtin::Strtolower => "strtolower",
            Builtin::Trim => "trim",
            Builtin::Ltrim => "ltrim",
            Builtin::Rtrim => "rtrim",
            Builtin::Substr => "substr",
            Builtin::StrRepeat => "str_repeat",
            Builtin::StrReplace => "str_replace",
            Builtin::Implode => "implode",
            Builtin::Explode => "explode",
            Builtin::StrContains => "str_contains",
            Builtin::StrStartsWith => "str_starts_with",
            Builtin::StrEndsWith => "str_ends_with",
            Builtin::Strrev => "strrev",
            Builtin::Ucfirst => "ucfirst",
            Builtin::Lcfirst => "lcfirst",
            Builtin::Ucwords => "ucwords",
            Builtin::StrSplit => "str_split",
            Builtin::ArrayValues => "array_values",
            Builtin::ArrayReverse => "array_reverse",
            Builtin::ArrayMerge => "array_merge",
            Builtin::ArraySlice => "array_slice",
            Builtin::ArrayPad => "array_pad",
            Builtin::ArrayFill => "array_fill",
            Builtin::ArrayKeyExists => "array_key_exists",
            Builtin::StrPad => "str_pad",
            Builtin::SubstrCount => "substr_count",
            Builtin::Addslashes => "addslashes",
            Builtin::Stripslashes => "stripslashes",
            Builtin::Bin2hex => "bin2hex",
            Builtin::Base64Encode => "base64_encode",
            Builtin::Urlencode => "urlencode",
            Builtin::Rawurlencode => "rawurlencode",
            Builtin::Urldecode => "urldecode",
            Builtin::Rawurldecode => "rawurldecode",
            Builtin::Sprintf => "sprintf",
            Builtin::Printf => "printf",
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
        /// The argument types, after semantic resolution.
        arg_tys: Vec<Ty>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TExpr {
    Int(i64),
    Float(f64),
    /// Raw byte-string literal (PHP strings are byte strings).
    Str(Vec<u8>),
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
    /// `cond ? then : else`; `then = None` is the short form `cond ?: else`.
    Ternary {
        cond: Box<TExpr>,
        cond_ty: Ty,
        then: Option<Box<TExpr>>,
        else_branch: Box<TExpr>,
        ty: Ty,
    },
    /// `match (subject) { ... }`.
    Match {
        subject: Box<TExpr>,
        subject_ty: Ty,
        arms: Vec<TArm>,
        ty: Ty,
    },
    /// `[k => v, ...]` — `ety` is the (already unified) element type.
    ArrayLit {
        elems: Vec<(Option<TExpr>, TExpr)>,
        ety: Ty,
    },
    /// `base[index]` read; `ety` is the element type.
    Index {
        base: Box<TExpr>,
        base_ty: Ty,
        index: Box<TExpr>,
        index_ty: Ty,
        ety: Ty,
    },
    Call {
        callee: Callee,
        ret: Ty,
        args: Vec<TExpr>,
        /// Actual argument types, aligned with `args`.
        arg_tys: Vec<Ty>,
        span: Span,
    },
    Property {
        base: Box<TExpr>,
        class_name: String,
        prop_name: String,
        prop_idx: u32,
        ty: Ty,
    },
    New {
        class_name: String,
        num_props: u32,
        prop_defaults: Vec<(u32, TExpr)>,
        ctor: Option<(String, Vec<TExpr>, Vec<Ty>)>,
        ty: Ty,
    },
}

/// One arm of a typed `match`.
#[derive(Clone, Debug, PartialEq)]
pub struct TArm {
    pub conds: Vec<TExpr>,
    pub is_default: bool,
    pub result: TExpr,
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
            TExpr::Ternary { ty, .. } => ty.clone(),
            TExpr::Match { ty, .. } => ty.clone(),
            TExpr::ArrayLit { ety, .. } => Ty::array(ety.clone()),
            TExpr::Index { ety, .. } => ety.clone(),
            TExpr::Call { ret, .. } => ret.clone(),
            TExpr::Property { ty, .. } => ty.clone(),
            TExpr::New { ty, .. } => ty.clone(),
        }
    }
}

/// A typed assignment target.
#[derive(Clone, Debug, PartialEq)]
pub enum TTarget {
    Var {
        name: String,
        slot: u32,
        ty: Ty,
    },
    /// `base[key] = ...`; evaluating `base` yields the array pointer.
    Index {
        base: Box<TExpr>,
        elem_ty: Ty,
        key: Box<TExpr>,
        key_ty: Ty,
    },
    /// `base[] = ...` (push).
    Push {
        base: Box<TExpr>,
        elem_ty: Ty,
    },
    Property {
        base: Box<TExpr>,
        class_name: String,
        prop_name: String,
        prop_idx: u32,
        ty: Ty,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TStmt {
    Block(Vec<TStmt>),
    Echo(Vec<TExpr>),
    Print(TExpr),
    Expr(TExpr),
    Assign {
        target: TTarget,
        ty: Ty,
        value: TExpr,
    },
    /// `target op= value`.
    AssignOp {
        target: TTarget,
        ty: Ty,
        op: ABinOp,
        value: TExpr,
    },
    /// `target++` / `target--`.
    IncDec {
        target: TTarget,
        ty: Ty,
        delta: i64,
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
    For {
        init: Vec<TStmt>,
        cond: Option<(TExpr, Ty)>,
        update: Vec<TStmt>,
        body: Vec<TStmt>,
    },
    DoWhile {
        body: Vec<TStmt>,
        cond: TExpr,
        cond_ty: Ty,
    },
    Foreach {
        base: TExpr,
        base_ty: Ty,
        value_slot: u32,
        value_ty: Ty,
        body: Vec<TStmt>,
    },
    Switch {
        subject: TExpr,
        subject_ty: Ty,
        cases: Vec<TCase>,
    },
    Break(u32),
    Continue(u32),
    Return {
        value: Option<TExpr>,
        ret: Ty,
    },
}

/// One arm of a typed `switch` (fall-through preserved).
#[derive(Clone, Debug, PartialEq)]
pub struct TCase {
    pub test: Option<TExpr>,
    pub test_ty: Ty,
    pub body: Vec<TStmt>,
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

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub name: String,
    pub name_span: Span,
    pub properties: Vec<PropertyInfo>,
    pub prop_indices: HashMap<String, usize>,
    pub methods: HashMap<String, MethodInfo>,
}

#[derive(Clone, Debug)]
pub struct PropertyInfo {
    pub name: String,
    pub name_span: Span,
    pub ty: Ty,
    pub visibility: ast::Visibility,
    pub default: Option<ast::Expr>,
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub name: String,
    pub name_span: Span,
    pub mangled: String,
    pub visibility: ast::Visibility,
    pub params: Vec<Ty>,
    pub param_names: Vec<String>,
    pub ret: Ty,
    pub ast_method: ast::MethodDecl,
}

pub struct Analyzer {
    diags: Diagnostics,
    /// lowercased name -> signature
    funcs: HashMap<String, Signature>,
    /// lowercased name -> class info
    classes: HashMap<String, ClassInfo>,
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
    /// Enclosing loop nesting depth (for `break`/`continue` validation).
    loop_depth: u32,
    /// Enclosing switch nesting depth.
    switch_depth: u32,
    /// Enclosing class (if currently inside a method).
    current_class: Option<String>,
}

impl FnCtx {
    fn new(ret: Ty, is_main: bool) -> Self {
        FnCtx {
            slots: Vec::new(),
            vars: HashMap::new(),
            ret,
            is_main,
            loop_depth: 0,
            switch_depth: 0,
            current_class: None,
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

    /// Refine the element type of an array slot created from an empty literal
    /// `[]`, once its element type becomes known.
    fn refine_array_slot(&mut self, slot: u32, want_elem: &Ty) -> bool {
        match &self.slots[slot as usize] {
            Ty::Array(cur) if matches!(**cur, Ty::Null) => {
                self.slots[slot as usize] = Ty::array(want_elem.clone());
                true
            }
            _ => false,
        }
    }
}

pub fn analyze(program: &ast::Program) -> (Option<TProgram>, Diagnostics) {
    let mut a = Analyzer {
        diags: Diagnostics::new(),
        funcs: HashMap::new(),
        classes: HashMap::new(),
    };
    let result = a.run(program);
    (result, a.diags)
}

impl Analyzer {
    fn run(&mut self, program: &ast::Program) -> Option<TProgram> {
        self.collect_classes(program);
        self.collect_signatures(program);

        let mut funcs = Vec::new();

        // Analyze methods of classes
        for class_key in self.classes.keys().cloned().collect::<Vec<_>>() {
            let class = self.classes.get(&class_key).unwrap().clone();
            for method in class.methods.values() {
                if let Some(tf) = self.analyze_method(&class.name, method) {
                    funcs.push(tf);
                }
            }
        }

        // Analyze user functions
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
            .filter(|s| !matches!(s, ast::Stmt::FnDecl(_) | ast::Stmt::ClassDecl(_)))
            .cloned()
            .collect();
        let main = self.analyze_main(&main_stmts);

        if self.diags.has_errors() {
            return None;
        }
        Some(TProgram { funcs, main: main? })
    }

    fn collect_classes(&mut self, program: &ast::Program) {
        // Step 1: register all class names so forward references work
        for item in &program.items {
            let ast::Stmt::ClassDecl(c) = item else {
                continue;
            };
            let key = c.name.to_ascii_lowercase();
            if self.classes.contains_key(&key) {
                self.diags.emit(
                    Diagnostic::error(format!("class `{}` is already defined", c.name))
                        .with_code("E0201")
                        .with_span(c.name_span),
                );
                continue;
            }
            self.classes.insert(
                key,
                ClassInfo {
                    name: c.name.clone(),
                    name_span: c.name_span,
                    properties: Vec::new(),
                    prop_indices: HashMap::new(),
                    methods: HashMap::new(),
                },
            );
        }

        // Step 2: resolve properties and methods
        for item in &program.items {
            let ast::Stmt::ClassDecl(c) = item else {
                continue;
            };
            let key = c.name.to_ascii_lowercase();
            let mut properties = Vec::new();
            let mut prop_indices = HashMap::new();
            let mut methods = HashMap::new();

            for member in &c.members {
                match member {
                    ast::ClassMember::Property(p) => {
                        if prop_indices.contains_key(&p.name) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "property `${}` is already defined in class `{}`",
                                    p.name, c.name
                                ))
                                .with_code("E0201")
                                .with_span(p.name_span),
                            );
                            continue;
                        }
                        let ty = match &p.ty {
                            Some(t) => self.resolve_type(t),
                            None => {
                                if let Some(def) = &p.default {
                                    let mut dummy_ctx = FnCtx::new(Ty::Void, false);
                                    if let Some(t_def) = self.infer(def, &mut dummy_ctx) {
                                        t_def.ty()
                                    } else {
                                        Ty::Int
                                    }
                                } else {
                                    self.diags.emit(
                                        Diagnostic::error(format!(
                                            "property `${}` needs an explicit type",
                                            p.name
                                        ))
                                        .with_code("E0202")
                                        .with_span(p.name_span)
                                        .with_note("write e.g. `public int $x;`"),
                                    );
                                    Ty::Int
                                }
                            }
                        };
                        if ty == Ty::Void {
                            self.diags.emit(
                                Diagnostic::error("a property cannot have type `void`")
                                    .with_code("E0203")
                                    .with_span(p.span),
                            );
                        }
                        let idx = properties.len();
                        prop_indices.insert(p.name.clone(), idx);
                        properties.push(PropertyInfo {
                            name: p.name.clone(),
                            name_span: p.name_span,
                            ty,
                            visibility: p.visibility,
                            default: p.default.clone(),
                        });
                    }
                    ast::ClassMember::Method(m) => {
                        let mkey = m.name.to_ascii_lowercase();
                        if methods.contains_key(&mkey) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "method `{}` is already defined in class `{}`",
                                    m.name, c.name
                                ))
                                .with_code("E0201")
                                .with_span(m.name_span),
                            );
                            continue;
                        }
                        let mut params = Vec::new();
                        let mut param_names = Vec::new();
                        for param in &m.params {
                            let pty = match &param.ty {
                                Some(t) => self.resolve_type(t),
                                None => {
                                    self.diags.emit(
                                        Diagnostic::error(format!(
                                            "parameter `${}` needs an explicit type",
                                            param.name
                                        ))
                                        .with_code("E0202")
                                        .with_span(param.name_span),
                                    );
                                    Ty::Int
                                }
                            };
                            if pty == Ty::Void {
                                self.diags.emit(
                                    Diagnostic::error("a parameter cannot have type `void`")
                                        .with_code("E0203")
                                        .with_span(param.span),
                                );
                            }
                            params.push(pty);
                            param_names.push(param.name.clone());
                        }

                        let ret = if mkey == "__construct" {
                            if let Some(t) = &m.ret {
                                let r = self.resolve_type(t);
                                if r != Ty::Void {
                                    self.diags.emit(
                                        Diagnostic::error(
                                            "constructor cannot declare a non-void return type",
                                        )
                                        .with_code("E0207")
                                        .with_span(m.name_span),
                                    );
                                }
                            }
                            Ty::Void
                        } else {
                            match &m.ret {
                                Some(t) => self.resolve_type(t),
                                None => {
                                    self.diags.emit(
                                        Diagnostic::error(format!(
                                            "method `{}` needs an explicit return type",
                                            m.name
                                        ))
                                        .with_code("E0204")
                                        .with_span(m.name_span)
                                        .with_note("write e.g. `public function f(): int`"),
                                    );
                                    Ty::Int
                                }
                            }
                        };

                        let mangled = mangle_method(&c.name, &m.name);
                        methods.insert(
                            mkey,
                            MethodInfo {
                                name: m.name.clone(),
                                name_span: m.name_span,
                                mangled,
                                visibility: m.visibility,
                                params,
                                param_names,
                                ret,
                                ast_method: m.clone(),
                            },
                        );
                    }
                }
            }

            if let Some(info) = self.classes.get_mut(&key) {
                info.properties = properties;
                info.prop_indices = prop_indices;
                info.methods = methods;
            }
        }
    }

    fn collect_signatures(&mut self, program: &ast::Program) {
        for item in &program.items {
            let ast::Stmt::FnDecl(f) = item else { continue };
            let key = f.name.to_ascii_lowercase();
            if let Some(prev) = self.funcs.get(&key) {
                self.diags.emit(
                    Diagnostic::error(format!("function `{}` is already defined", f.name))
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
            ast::TypeRef::Array(inner) => {
                let elem = self.resolve_type(inner);
                if elem == Ty::Void {
                    self.diags.emit(
                        Diagnostic::error("an array cannot have element type `void`")
                            .with_code("E0205")
                            .with_span(inner.span()),
                    );
                    return Ty::array(Ty::Int);
                }
                Ty::array(elem)
            }
            ast::TypeRef::Named(name) => {
                if let Some(c) = self.classes.get(&name.to_ascii_lowercase()) {
                    Ty::Named(c.name.clone())
                } else {
                    self.diags.emit(
                        Diagnostic::error(format!("unknown type `{name}`"))
                            .with_code("E0205")
                            .with_span(t.span()),
                    );
                    Ty::Int
                }
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

    fn analyze_method(&mut self, class_name: &str, method: &MethodInfo) -> Option<TFunc> {
        let mut ctx = FnCtx::new(method.ret.clone(), false);
        ctx.current_class = Some(class_name.to_string());
        ctx.declare("this", Ty::Named(class_name.to_string()));
        for (name, ty) in method.param_names.iter().zip(method.params.iter()) {
            ctx.declare(name, ty.clone());
        }
        let body = self.analyze_stmts(&method.ast_method.body, &mut ctx);
        if method.name != "__construct" && method.ret != Ty::Void {
            let sig = Signature {
                mangled: method.mangled.clone(),
                display: format!("{class_name}::{}", method.name),
                params: method.params.clone(),
                param_names: method.param_names.clone(),
                ret: method.ret.clone(),
                span: method.name_span,
            };
            self.check_termination(&method.ast_method.body, &sig, method.name_span);
        }

        let mut all_params = vec![("this".to_string(), Ty::Named(class_name.to_string()))];
        all_params.extend(
            method
                .param_names
                .iter()
                .cloned()
                .zip(method.params.iter().cloned()),
        );

        Some(TFunc {
            name: method.mangled.clone(),
            display: format!("{class_name}::{}", method.name),
            params: all_params,
            ret: method.ret.clone(),
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
            ast::Stmt::Block(body, _) => Some(TStmt::Block(self.analyze_stmts(body, ctx))),
            ast::Stmt::Echo(exprs, _) => {
                let mut texprs = Vec::new();
                for e in exprs {
                    if let Some(t) = self.infer(e, ctx) {
                        if t.ty().is_array() {
                            self.diags.emit(
                                Diagnostic::error(
                                    "cannot echo an array; index it or use `implode()` (not yet implemented)",
                                )
                                .with_code("E0240")
                                .with_span(e.span()),
                            );
                            continue;
                        }
                        texprs.push(t);
                    }
                }
                Some(TStmt::Echo(texprs))
            }
            ast::Stmt::Print(e, _) => {
                let t = self.infer(e, ctx)?;
                if t.ty().is_array() {
                    self.diags.emit(
                        Diagnostic::error(
                            "cannot print an array; index it or use `implode()` (not yet implemented)",
                        )
                        .with_code("E0240")
                        .with_span(e.span()),
                    );
                    return None;
                }
                Some(TStmt::Print(t))
            }
            ast::Stmt::Expr(e, _) => {
                let t = self.infer(e, ctx)?;
                Some(TStmt::Expr(t))
            }
            ast::Stmt::Assign {
                target,
                ty,
                value,
                span,
            } => self.analyze_assign(target, ty.as_ref(), value, *span, ctx),
            ast::Stmt::AssignOp {
                target,
                op,
                value,
                span,
            } => self.analyze_assign_op(target, *op, value, *span, ctx),
            ast::Stmt::IncDec {
                target,
                delta,
                span,
            } => self.analyze_incdec(target, *delta, *span, ctx),
            ast::Stmt::FnDecl(f) => {
                self.diags.emit(
                    Diagnostic::error("nested function declarations are not supported yet")
                        .with_code("E0207")
                        .with_span(f.span),
                );
                None
            }
            ast::Stmt::ClassDecl(c) => {
                self.diags.emit(
                    Diagnostic::error("nested class declarations are not supported yet")
                        .with_code("E0207")
                        .with_span(c.span),
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
                self.expect_truthy(&cond_ty, cond.span());
                let then_t = self.analyze_stmts(then_branch, ctx);
                let mut elifs_t = Vec::new();
                for (c, b) in elifs {
                    let ct = self.infer(c, ctx)?;
                    let cty = ct.ty();
                    self.expect_truthy(&cty, c.span());
                    let bt = self.analyze_stmts(b, ctx);
                    elifs_t.push((ct, cty, bt));
                }
                let else_t = else_branch.as_ref().map(|b| self.analyze_stmts(b, ctx));
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
                self.expect_truthy(&cond_ty, cond.span());
                ctx.loop_depth += 1;
                let body_t = self.analyze_stmts(body, ctx);
                ctx.loop_depth -= 1;
                Some(TStmt::While {
                    cond: cond_t,
                    cond_ty,
                    body: body_t,
                })
            }
            ast::Stmt::For {
                init,
                cond,
                update,
                body,
                ..
            } => self.analyze_for(init, cond, update, body, ctx),
            ast::Stmt::DoWhile { body, cond, .. } => {
                ctx.loop_depth += 1;
                let body_t = self.analyze_stmts(body, ctx);
                ctx.loop_depth -= 1;
                let cond_t = self.infer(cond, ctx)?;
                let cond_ty = cond_t.ty();
                self.expect_truthy(&cond_ty, cond.span());
                Some(TStmt::DoWhile {
                    body: body_t,
                    cond: cond_t,
                    cond_ty,
                })
            }
            ast::Stmt::Foreach {
                subject,
                key,
                key_span,
                value,
                value_span,
                body,
                ..
            } => self.analyze_foreach(
                subject,
                key.as_deref(),
                *key_span,
                value,
                *value_span,
                body,
                ctx,
            ),
            ast::Stmt::Switch { subject, cases, .. } => self.analyze_switch(subject, cases, ctx),
            ast::Stmt::Break(level, span) => self.analyze_jump(*level, *span, ctx, false),
            ast::Stmt::Continue(level, span) => self.analyze_jump(*level, *span, ctx, true),
        }
    }

    fn expect_truthy(&mut self, ty: &Ty, span: Span) {
        if !is_truthy(ty) {
            self.diags.emit(
                Diagnostic::error(format!("condition must be a scalar value, found `{ty}`"))
                    .with_code("E0208")
                    .with_span(span),
            );
        }
    }

    fn analyze_for(
        &mut self,
        init: &[ast::Stmt],
        cond: &[ast::Expr],
        update: &[ast::Stmt],
        body: &[ast::Stmt],
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let init_t = self.analyze_stmts(init, ctx);
        let mut cond_t = None;
        if !cond.is_empty() {
            if cond.len() > 1 {
                self.diags.emit(
                    Diagnostic::error("the `for` condition must be a single expression")
                        .with_code("E0241")
                        .with_span(cond[1].span()),
                );
            }
            let c = self.infer(&cond[0], ctx)?;
            let cty = c.ty();
            self.expect_truthy(&cty, cond[0].span());
            cond_t = Some((c, cty));
        }
        ctx.loop_depth += 1;
        let body_t = self.analyze_stmts(body, ctx);
        ctx.loop_depth -= 1;
        let update_t = self.analyze_stmts(update, ctx);
        Some(TStmt::For {
            init: init_t,
            cond: cond_t,
            update: update_t,
            body: body_t,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn analyze_foreach(
        &mut self,
        subject: &ast::Expr,
        key: Option<&str>,
        key_span: Span,
        value: &str,
        value_span: Span,
        body: &[ast::Stmt],
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        if key.is_some() {
            self.diags.emit(
                Diagnostic::error(
                    "`foreach` with a key variable is not implemented yet; iterate with an index and `count()` instead",
                )
                .with_code("E0242")
                .with_span(key_span),
            );
            return None;
        }
        let subj = self.infer(subject, ctx)?;
        let sty = subj.ty();
        let elem = match sty.array_elem() {
            Some(e) if !matches!(e, Ty::Null) => e.clone(),
            Some(_) => {
                self.diags.emit(
                    Diagnostic::error(
                        "cannot iterate an array whose element type is still unknown (an empty `[]`); assign an element or annotate the type",
                    )
                    .with_code("E0243")
                    .with_span(subject.span()),
                );
                return None;
            }
            None => {
                self.diags.emit(
                    Diagnostic::error(format!("`foreach` expects an array, found `{sty}`"))
                        .with_code("E0244")
                        .with_span(subject.span()),
                );
                return None;
            }
        };
        let vslot = match ctx.slot_of(value) {
            Some(s) => {
                let existing = ctx.slot_ty(s);
                if !elem.assignable_to(&existing) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`foreach` value variable `${value}` is already of type `{existing}`, but the array elements are `{elem}`"
                        ))
                        .with_code("E0245")
                        .with_span(value_span),
                    );
                }
                s
            }
            None => ctx.declare(value, elem.clone()),
        };
        ctx.loop_depth += 1;
        let body_t = self.analyze_stmts(body, ctx);
        ctx.loop_depth -= 1;
        Some(TStmt::Foreach {
            base: subj,
            base_ty: sty,
            value_slot: vslot,
            value_ty: elem,
            body: body_t,
        })
    }

    fn analyze_switch(
        &mut self,
        subject: &ast::Expr,
        cases: &[ast::SwitchCase],
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let subj = self.infer(subject, ctx)?;
        let sty = subj.ty();
        let mut tcases = Vec::new();
        ctx.switch_depth += 1;
        for c in cases {
            let (test_t, test_ty) = match &c.test {
                Some(t) => {
                    let te = self.infer(t, ctx)?;
                    let tt = te.ty();
                    if !comparable_eq(&sty, &tt) {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot compare `case` value of type `{tt}` with switch subject of type `{sty}`"
                            ))
                            .with_code("E0224")
                            .with_span(t.span()),
                        );
                    }
                    (Some(te), tt)
                }
                None => (None, Ty::Int),
            };
            let body = self.analyze_stmts(&c.body, ctx);
            tcases.push(TCase {
                test: test_t,
                test_ty,
                body,
            });
        }
        ctx.switch_depth -= 1;
        Some(TStmt::Switch {
            subject: subj,
            subject_ty: sty,
            cases: tcases,
        })
    }

    fn analyze_jump(
        &mut self,
        level: Option<u32>,
        span: Span,
        ctx: &FnCtx,
        is_continue: bool,
    ) -> Option<TStmt> {
        let level = level.unwrap_or(1);
        if is_continue {
            if level > ctx.loop_depth {
                self.diags.emit(
                    Diagnostic::error("`continue` is not inside a loop (or its level is too deep)")
                        .with_code("E0246")
                        .with_span(span),
                );
                return None;
            }
            Some(TStmt::Continue(level))
        } else {
            if level > ctx.loop_depth + ctx.switch_depth {
                self.diags.emit(
                    Diagnostic::error(
                        "`break` is not inside a loop or `switch` (or its level is too deep)",
                    )
                    .with_code("E0247")
                    .with_span(span),
                );
                return None;
            }
            Some(TStmt::Break(level))
        }
    }

    fn analyze_assign(
        &mut self,
        target: &ast::LValue,
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

        match target {
            ast::LValue::Var { name, .. } => {
                if name == "this" {
                    self.diags.emit(
                        Diagnostic::error("cannot re-assign `$this`")
                            .with_code("E0267")
                            .with_span(span),
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
                    target: TTarget::Var {
                        name: name.clone(),
                        slot,
                        ty: decl_ty.clone(),
                    },
                    ty: decl_ty,
                    value: val,
                })
            }
            ast::LValue::Index { base, index, .. } => {
                if annotation.is_some() {
                    self.diags.emit(
                        Diagnostic::error("an array element cannot carry a type annotation")
                            .with_code("E0251")
                            .with_span(span),
                    );
                }
                let target = self.infer_index_target(base, index.as_deref(), span, ctx)?;
                let elem_ty = self.elem_ty_maybe_refine(&target, &val_ty, ctx);
                if !val_ty.assignable_to(&elem_ty) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot assign value of type `{val_ty}` into an array of element type `{elem_ty}`"
                        ))
                        .with_code("E0212")
                        .with_span(span),
                    );
                    return None;
                }
                Some(TStmt::Assign {
                    target,
                    ty: elem_ty,
                    value: val,
                })
            }
            ast::LValue::Property {
                base,
                name,
                name_span,
                ..
            } => {
                if annotation.is_some() {
                    self.diags.emit(
                        Diagnostic::error(
                            "a property cannot carry an inline type annotation on assignment",
                        )
                        .with_code("E0251")
                        .with_span(span),
                    );
                }
                let base_expr = self.infer(base, ctx)?;
                let base_ty = base_expr.ty();
                let class_name = match &base_ty {
                    Ty::Named(cn) => cn.clone(),
                    other => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot access property on non-object of type `{other}`"
                            ))
                            .with_code("E0260")
                            .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let class = match self.classes.get(&class_name.to_ascii_lowercase()) {
                    Some(c) => c.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("unknown class `{class_name}`"))
                                .with_code("E0205")
                                .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let prop_idx = match class.prop_indices.get(name) {
                    Some(&idx) => idx,
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "class `{}` has no property `${name}`",
                                class.name
                            ))
                            .with_code("E0261")
                            .with_span(*name_span),
                        );
                        return None;
                    }
                };

                let prop = &class.properties[prop_idx];
                if prop.visibility == ast::Visibility::Private
                    && ctx.current_class.as_deref().map(|s| s.to_ascii_lowercase())
                        != Some(class.name.to_ascii_lowercase())
                {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access private property {}::${name} from outside class",
                            class.name
                        ))
                        .with_code("E0265")
                        .with_span(*name_span),
                    );
                    return None;
                }

                if !val_ty.assignable_to(&prop.ty) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot assign value of type `{val_ty}` to property `{}::${name}` of type `{}`",
                            class.name, prop.ty
                        ))
                        .with_code("E0212")
                        .with_span(span),
                    );
                    return None;
                }

                Some(TStmt::Assign {
                    target: TTarget::Property {
                        base: Box::new(base_expr),
                        class_name: class.name.clone(),
                        prop_name: name.clone(),
                        prop_idx: prop_idx as u32,
                        ty: prop.ty.clone(),
                    },
                    ty: prop.ty.clone(),
                    value: val,
                })
            }
        }
    }

    /// The element type of a target, refining an `array<null>` slot when the
    /// assigned value reveals the element type.
    fn elem_ty_maybe_refine(&mut self, target: &TTarget, val_ty: &Ty, ctx: &mut FnCtx) -> Ty {
        let (elem, base) = match target {
            TTarget::Index { elem_ty, base, .. } => (elem_ty.clone(), base),
            TTarget::Push { elem_ty, base } => (elem_ty.clone(), base),
            TTarget::Var { ty, .. } | TTarget::Property { ty, .. } => return ty.clone(),
        };
        if elem == Ty::Null {
            if let TExpr::Var { slot, .. } = &**base {
                if ctx.refine_array_slot(*slot, val_ty) {
                    return val_ty.clone();
                }
            }
        }
        elem
    }

    fn analyze_assign_op(
        &mut self,
        target: &ast::LValue,
        op: ABinOp,
        value: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let (ttarget, tty) = self.infer_target_read(target, span, ctx)?;
        let val = self.infer(value, ctx)?;
        let vty = val.ty();
        let result_ty = self.binary_type(op, &tty, &vty, span)?;
        if !result_ty.assignable_to(&tty) {
            self.diags.emit(
                Diagnostic::error(format!(
                    "result of `{}=` has type `{result_ty}`, which cannot be stored back into a target of type `{tty}`",
                    op.symbol()
                ))
                .with_code("E0212")
                .with_span(span),
            );
            return None;
        }
        Some(TStmt::AssignOp {
            target: ttarget,
            ty: tty,
            op,
            value: val,
        })
    }

    fn analyze_incdec(
        &mut self,
        target: &ast::LValue,
        delta: i32,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TStmt> {
        let (ttarget, tty) = self.infer_target_read(target, span, ctx)?;
        if !tty.is_numeric() {
            self.diags.emit(
                Diagnostic::error(format!(
                    "`++`/`--` requires a numeric operand, found `{tty}`"
                ))
                .with_code("E0252")
                .with_span(span),
            );
            return None;
        }
        Some(TStmt::IncDec {
            target: ttarget,
            ty: tty,
            delta: delta as i64,
        })
    }

    /// Resolve an assignment target to a typed target together with its own
    /// (read) type.
    fn infer_target_read(
        &mut self,
        target: &ast::LValue,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<(TTarget, Ty)> {
        match target {
            ast::LValue::Var { name, .. } => {
                if name == "this" {
                    self.diags.emit(
                        Diagnostic::error("cannot re-assign `$this`")
                            .with_code("E0267")
                            .with_span(span),
                    );
                    return None;
                }
                let slot = match ctx.slot_of(name) {
                    Some(s) => s,
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("undefined variable `${name}`"))
                                .with_code("E0220")
                                .with_span(span),
                        );
                        return None;
                    }
                };
                let ty = ctx.slot_ty(slot);
                Some((
                    TTarget::Var {
                        name: name.clone(),
                        slot,
                        ty: ty.clone(),
                    },
                    ty,
                ))
            }
            ast::LValue::Index { base, index, .. } => {
                let t = self.infer_index_target(base, index.as_deref(), span, ctx)?;
                let ty = match &t {
                    TTarget::Index { elem_ty, .. } => elem_ty.clone(),
                    TTarget::Push { elem_ty, .. } => elem_ty.clone(),
                    TTarget::Var { ty, .. } | TTarget::Property { ty, .. } => ty.clone(),
                };
                if ty == Ty::Null {
                    self.diags.emit(
                        Diagnostic::error(
                            "the element type of this array is still unknown (an empty `[]`)",
                        )
                        .with_code("E0243")
                        .with_span(span),
                    );
                    return None;
                }
                Some((t, ty))
            }
            ast::LValue::Property {
                base,
                name,
                name_span,
                ..
            } => {
                let base_expr = self.infer(base, ctx)?;
                let base_ty = base_expr.ty();
                let class_name = match &base_ty {
                    Ty::Named(cn) => cn.clone(),
                    other => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot access property on non-object of type `{other}`"
                            ))
                            .with_code("E0260")
                            .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let class = match self.classes.get(&class_name.to_ascii_lowercase()) {
                    Some(c) => c.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("unknown class `{class_name}`"))
                                .with_code("E0205")
                                .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let prop_idx = match class.prop_indices.get(name) {
                    Some(&idx) => idx,
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "class `{}` has no property `${name}`",
                                class.name
                            ))
                            .with_code("E0261")
                            .with_span(*name_span),
                        );
                        return None;
                    }
                };

                let prop = &class.properties[prop_idx];
                if prop.visibility == ast::Visibility::Private
                    && ctx.current_class.as_deref().map(|s| s.to_ascii_lowercase())
                        != Some(class.name.to_ascii_lowercase())
                {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access private property {}::${name} from outside class",
                            class.name
                        ))
                        .with_code("E0265")
                        .with_span(*name_span),
                    );
                    return None;
                }

                let ty = prop.ty.clone();
                Some((
                    TTarget::Property {
                        base: Box::new(base_expr),
                        class_name: class.name.clone(),
                        prop_name: name.clone(),
                        prop_idx: prop_idx as u32,
                        ty: ty.clone(),
                    },
                    ty,
                ))
            }
        }
    }

    fn infer_index_target(
        &mut self,
        base: &ast::LValue,
        index: Option<&ast::Expr>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TTarget> {
        let ast::LValue::Var { name, .. } = base else {
            self.diags.emit(
                Diagnostic::error("nested array assignment is not supported yet")
                    .with_code("E0248")
                    .with_span(span),
            );
            return None;
        };
        let slot = match ctx.slot_of(name) {
            Some(s) => s,
            None => {
                self.diags.emit(
                    Diagnostic::error(format!("undefined variable `${name}`"))
                        .with_code("E0220")
                        .with_span(span),
                );
                return None;
            }
        };
        let bty = ctx.slot_ty(slot);
        let Ty::Array(elem) = bty.clone() else {
            self.diags.emit(
                Diagnostic::error(format!("cannot index a value of type `{bty}` with `[]`"))
                    .with_code("E0249")
                    .with_span(span),
            );
            return None;
        };
        let elem = (*elem).clone();
        let base_expr = TExpr::Var {
            name: name.clone(),
            slot,
            ty: bty.clone(),
        };
        match index {
            None => Some(TTarget::Push {
                base: Box::new(base_expr),
                elem_ty: elem,
            }),
            Some(ie) => {
                let key = self.infer(ie, ctx)?;
                let kty = key.ty();
                if !matches!(kty, Ty::Int | Ty::String) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "array keys must be `int` or `string`, found `{kty}`"
                        ))
                        .with_code("E0250")
                        .with_span(ie.span()),
                    );
                    return None;
                }
                Some(TTarget::Index {
                    base: Box::new(base_expr),
                    elem_ty: elem,
                    key: Box::new(key),
                    key_ty: kty,
                })
            }
        }
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
                    Diagnostic::warning(
                        "`return` with a value in the top level of a script is ignored",
                    )
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
                    Diagnostic::error(format!("function must return a value of type `{ret}`"))
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
            ast::Expr::Var(name, span) => {
                if name == "this" && ctx.current_class.is_none() {
                    self.diags.emit(
                        Diagnostic::error("using `$this` when not in object context")
                            .with_code("E0268")
                            .with_span(*span),
                    );
                    return None;
                }
                match ctx.slot_of(name) {
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
                }
            }
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
                    AUnOp::BitNot => {
                        if oty != Ty::Int {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "`~` requires an `int` operand, found `{oty}`"
                                ))
                                .with_code("E0221")
                                .with_span(*span),
                            );
                            return None;
                        }
                        Some(TExpr::Un {
                            op: AUnOp::BitNot,
                            operand_ty: oty,
                            ty: Ty::Int,
                            expr: Box::new(operand),
                        })
                    }
                }
            }
            ast::Expr::Binary { op, lhs, rhs, span } => {
                self.infer_binary(*op, lhs, rhs, *span, ctx)
            }
            ast::Expr::Call {
                callee,
                callee_span,
                args,
                span,
            } => self.infer_call(callee, *callee_span, args, *span, ctx),
            ast::Expr::Ternary {
                cond,
                then,
                else_branch,
                span,
            } => self.infer_ternary(cond, then.as_deref(), else_branch, *span, ctx),
            ast::Expr::Index { base, index, span } => {
                self.infer_index(base, index.as_deref(), *span, ctx)
            }
            ast::Expr::ArrayLit { items, span } => self.infer_array_lit(items, *span, ctx),
            ast::Expr::Match {
                subject,
                arms,
                span,
            } => self.infer_match(subject, arms, *span, ctx),
            ast::Expr::Property {
                base,
                name,
                name_span,
                span: _,
            } => {
                let base_expr = self.infer(base, ctx)?;
                let base_ty = base_expr.ty();
                let class_name = match &base_ty {
                    Ty::Named(cn) => cn.clone(),
                    other => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot access property on non-object of type `{other}`"
                            ))
                            .with_code("E0260")
                            .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let class = match self.classes.get(&class_name.to_ascii_lowercase()) {
                    Some(c) => c.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("unknown class `{class_name}`"))
                                .with_code("E0205")
                                .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let prop_idx = match class.prop_indices.get(name) {
                    Some(&idx) => idx,
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "class `{}` has no property `${name}`",
                                class.name
                            ))
                            .with_code("E0261")
                            .with_span(*name_span),
                        );
                        return None;
                    }
                };

                let prop = &class.properties[prop_idx];
                if prop.visibility == ast::Visibility::Private
                    && ctx.current_class.as_deref().map(|s| s.to_ascii_lowercase())
                        != Some(class.name.to_ascii_lowercase())
                {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access private property {}::${name} from outside class",
                            class.name
                        ))
                        .with_code("E0265")
                        .with_span(*name_span),
                    );
                    return None;
                }

                Some(TExpr::Property {
                    base: Box::new(base_expr),
                    class_name: class.name.clone(),
                    prop_name: name.clone(),
                    prop_idx: prop_idx as u32,
                    ty: prop.ty.clone(),
                })
            }
            ast::Expr::MethodCall {
                base,
                name,
                name_span,
                args,
                span,
            } => {
                let base_expr = self.infer(base, ctx)?;
                let base_ty = base_expr.ty();
                let class_name = match &base_ty {
                    Ty::Named(cn) => cn.clone(),
                    other => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot call method on non-object of type `{other}`"
                            ))
                            .with_code("E0260")
                            .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let class = match self.classes.get(&class_name.to_ascii_lowercase()) {
                    Some(c) => c.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("unknown class `{class_name}`"))
                                .with_code("E0205")
                                .with_span(base.span()),
                        );
                        return None;
                    }
                };

                let method = match class.methods.get(&name.to_ascii_lowercase()) {
                    Some(m) => m.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "class `{}` has no method `{name}`",
                                class.name
                            ))
                            .with_code("E0262")
                            .with_span(*name_span),
                        );
                        return None;
                    }
                };

                if method.visibility == ast::Visibility::Private
                    && ctx.current_class.as_deref().map(|s| s.to_ascii_lowercase())
                        != Some(class.name.to_ascii_lowercase())
                {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot call private method {}::{name} from outside class",
                            class.name
                        ))
                        .with_code("E0265")
                        .with_span(*name_span),
                    );
                    return None;
                }

                if args.len() != method.params.len() {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "method `{}::{name}` expects {} argument{}, found {}",
                            class.name,
                            method.params.len(),
                            if method.params.len() == 1 { "" } else { "s" },
                            args.len()
                        ))
                        .with_code("E0207")
                        .with_span(*span),
                    );
                    return None;
                }

                let mut typed_args = vec![base_expr];
                let mut actual_tys = vec![Ty::Named(class.name.clone())];
                let mut formal_params = vec![Ty::Named(class.name.clone())];
                formal_params.extend(method.params.iter().cloned());

                for (arg, formal_ty) in args.iter().zip(method.params.iter()) {
                    let t_arg = self.infer(arg, ctx)?;
                    let aty = t_arg.ty();
                    if !aty.assignable_to(formal_ty) {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "argument type mismatch in call to `{}::{name}`: expected `{formal_ty}`, found `{aty}`",
                                class.name
                            ))
                            .with_code("E0208")
                            .with_span(arg.span()),
                        );
                        return None;
                    }
                    actual_tys.push(aty);
                    typed_args.push(t_arg);
                }

                Some(TExpr::Call {
                    callee: Callee::User {
                        mangled: method.mangled,
                        name: format!("{}::{name}", class.name),
                        params: formal_params,
                    },
                    ret: method.ret,
                    args: typed_args,
                    arg_tys: actual_tys,
                    span: *span,
                })
            }
            ast::Expr::New {
                class_name,
                class_span,
                args,
                span,
            } => {
                let class = match self.classes.get(&class_name.to_ascii_lowercase()) {
                    Some(c) => c.clone(),
                    None => {
                        self.diags.emit(
                            Diagnostic::error(format!("unknown class `{class_name}`"))
                                .with_code("E0205")
                                .with_span(*class_span),
                        );
                        return None;
                    }
                };

                let ctor = class.methods.get("__construct").cloned();
                let ctor_info = if let Some(ctor) = &ctor {
                    if ctor.visibility == ast::Visibility::Private
                        && ctx.current_class.as_deref().map(|s| s.to_ascii_lowercase())
                            != Some(class.name.to_ascii_lowercase())
                    {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "cannot call private constructor of class `{}` from outside class",
                                class.name
                            ))
                            .with_code("E0265")
                            .with_span(*span),
                        );
                        return None;
                    }

                    if args.len() != ctor.params.len() {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "constructor `{}::__construct` expects {} argument{}, found {}",
                                class.name,
                                ctor.params.len(),
                                if ctor.params.len() == 1 { "" } else { "s" },
                                args.len()
                            ))
                            .with_code("E0207")
                            .with_span(*span),
                        );
                        return None;
                    }

                    let mut typed_args = Vec::new();
                    let mut actual_tys = Vec::new();
                    for (arg, formal_ty) in args.iter().zip(ctor.params.iter()) {
                        let t_arg = self.infer(arg, ctx)?;
                        let aty = t_arg.ty();
                        if !aty.assignable_to(formal_ty) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "argument type mismatch in call to `{}::__construct`: expected `{formal_ty}`, found `{aty}`",
                                    class.name
                                ))
                                .with_code("E0208")
                                .with_span(arg.span()),
                            );
                            return None;
                        }
                        actual_tys.push(aty);
                        typed_args.push(t_arg);
                    }
                    Some((ctor.mangled.clone(), typed_args, actual_tys))
                } else {
                    if !args.is_empty() {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "class `{}` has no constructor and accepts 0 arguments, found {}",
                                class.name,
                                args.len()
                            ))
                            .with_code("E0207")
                            .with_span(*span),
                        );
                        return None;
                    }
                    None
                };

                // Infer default expressions for properties
                let mut prop_defaults = Vec::new();
                for (idx, p) in class.properties.iter().enumerate() {
                    if let Some(def_expr) = &p.default {
                        let mut dummy_ctx = FnCtx::new(Ty::Void, false);
                        if let Some(t_def) = self.infer(def_expr, &mut dummy_ctx) {
                            if !t_def.ty().assignable_to(&p.ty) {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "default value of property `{}::${}` is of type `{}`, expected `{}`",
                                        class.name, p.name, t_def.ty(), p.ty
                                    ))
                                    .with_code("E0212")
                                    .with_span(def_expr.span()),
                                );
                            } else {
                                prop_defaults.push((idx as u32, t_def));
                            }
                        }
                    }
                }

                Some(TExpr::New {
                    class_name: class.name.clone(),
                    num_props: class.properties.len() as u32,
                    prop_defaults,
                    ctor: ctor_info,
                    ty: Ty::Named(class.name.clone()),
                })
            }
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
        let ty = self.binary_type(op, &l, &r, span)?;
        Some(TExpr::Bin {
            op,
            lhs_ty: l,
            rhs_ty: r,
            ty,
            lhs: Box::new(lt),
            rhs: Box::new(rt),
        })
    }

    /// Validate the operands of a binary operator and compute its result type.
    /// Shared by expression inference and compound assignment.
    fn binary_type(&mut self, op: ABinOp, l: &Ty, r: &Ty, span: Span) -> Option<Ty> {
        match op {
            ABinOp::Add | ABinOp::Sub | ABinOp::Mul => {
                if !l.is_numeric() || !r.is_numeric() {
                    return self.bad_operands(op, l, r, span);
                }
                Some(numeric_result(l, r))
            }
            ABinOp::Div => {
                if !l.is_numeric() || !r.is_numeric() {
                    return self.bad_operands(op, l, r, span);
                }
                // PHP `/` always produces a float.
                Some(Ty::Float)
            }
            ABinOp::Mod => {
                if *l != Ty::Int || *r != Ty::Int {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`%` requires two `int` operands, found `{l}` and `{r}`"
                        ))
                        .with_code("E0222")
                        .with_span(span)
                        .with_note(
                            "use `fmod()` for floating point remainders (not yet implemented)",
                        ),
                    );
                    return None;
                }
                Some(Ty::Int)
            }
            ABinOp::Pow => {
                self.diags.emit(
                    Diagnostic::error("the `**` operator is not implemented yet")
                        .with_code("E0257")
                        .with_span(span),
                );
                None
            }
            ABinOp::Concat => {
                for t in [l, r] {
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
                Some(Ty::String)
            }
            ABinOp::Eq | ABinOp::NotEq => {
                if !comparable_eq(l, r) {
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
                Some(Ty::Bool)
            }
            ABinOp::Identical | ABinOp::NotIdentical => {
                let ok = l == r && l.is_scalar();
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
                Some(Ty::Bool)
            }
            ABinOp::Lt | ABinOp::LtEq | ABinOp::Gt | ABinOp::GtEq => {
                let ok =
                    (l.is_numeric() && r.is_numeric()) || (*l == Ty::String && *r == Ty::String);
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
                Some(Ty::Bool)
            }
            ABinOp::Spaceship => {
                let ok =
                    (l.is_numeric() && r.is_numeric()) || (*l == Ty::String && *r == Ty::String);
                if !ok {
                    self.diags.emit(
                        Diagnostic::error(format!("`<=>` cannot compare `{l}` and `{r}`"))
                            .with_code("E0226")
                            .with_span(span)
                            .with_note("operands must both be numeric or both `string`"),
                    );
                    return None;
                }
                Some(Ty::Int)
            }
            ABinOp::And | ABinOp::Or => {
                if !is_truthy(l) || !is_truthy(r) {
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
                Some(Ty::Bool)
            }
            ABinOp::BitAnd | ABinOp::BitOr | ABinOp::BitXor | ABinOp::Shl | ABinOp::Shr => {
                if *l != Ty::Int || *r != Ty::Int {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`{}` requires two `int` operands, found `{l}` and `{r}`",
                            op.symbol()
                        ))
                        .with_code("E0222")
                        .with_span(span),
                    );
                    return None;
                }
                Some(Ty::Int)
            }
            ABinOp::NullCoalesce => self.unify_types(l, r, span),
        }
    }

    /// The common type of two branch results (`?:`, `??`, `match`, array
    /// literals). Allows the same `int`/`float` widening as assignment.
    fn unify_types(&mut self, a: &Ty, b: &Ty, span: Span) -> Option<Ty> {
        if a == b {
            return Some(a.clone());
        }
        match (a, b) {
            (Ty::Null, other) | (other, Ty::Null) => return Some(other.clone()),
            (x, y) if x.is_numeric() && y.is_numeric() => return Some(numeric_result(x, y)),
            _ => {}
        }
        self.diags.emit(
            Diagnostic::error(format!("cannot combine values of type `{a}` and `{b}`"))
                .with_code("E0253")
                .with_span(span),
        );
        None
    }

    fn infer_ternary(
        &mut self,
        cond: &ast::Expr,
        then: Option<&ast::Expr>,
        else_branch: &ast::Expr,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let c = self.infer(cond, ctx)?;
        let cty = c.ty();
        self.expect_truthy(&cty, cond.span());
        let eb = self.infer(else_branch, ctx)?;
        match then {
            Some(t) => {
                let tt = self.infer(t, ctx)?;
                let ty = self.unify_types(&tt.ty(), &eb.ty(), span)?;
                Some(TExpr::Ternary {
                    cond: Box::new(c),
                    cond_ty: cty,
                    then: Some(Box::new(tt)),
                    else_branch: Box::new(eb),
                    ty,
                })
            }
            None => {
                let ty = self.unify_types(&cty, &eb.ty(), span)?;
                Some(TExpr::Ternary {
                    cond: Box::new(c),
                    cond_ty: cty,
                    then: None,
                    else_branch: Box::new(eb),
                    ty,
                })
            }
        }
    }

    fn infer_index(
        &mut self,
        base: &ast::Expr,
        index: Option<&ast::Expr>,
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let b = self.infer(base, ctx)?;
        let bty = b.ty();
        let elem = match bty.array_elem() {
            Some(e) if !matches!(e, Ty::Null) => e.clone(),
            Some(_) => {
                self.diags.emit(
                    Diagnostic::error(
                        "this array's element type is still unknown (an empty `[]`); assign an element or annotate the type",
                    )
                    .with_code("E0243")
                    .with_span(span),
                );
                return None;
            }
            None => {
                self.diags.emit(
                    Diagnostic::error(format!("cannot index a value of type `{bty}`"))
                        .with_code("E0249")
                        .with_span(span),
                );
                return None;
            }
        };
        let Some(ie) = index else {
            self.diags.emit(
                Diagnostic::error("`[]` (append) is only valid as an assignment target")
                    .with_code("E0254")
                    .with_span(span),
            );
            return None;
        };
        let key = self.infer(ie, ctx)?;
        let kty = key.ty();
        if !matches!(kty, Ty::Int | Ty::String) {
            self.diags.emit(
                Diagnostic::error(format!(
                    "array keys must be `int` or `string`, found `{kty}`"
                ))
                .with_code("E0250")
                .with_span(ie.span()),
            );
            return None;
        }
        Some(TExpr::Index {
            base: Box::new(b),
            base_ty: bty,
            index: Box::new(key),
            index_ty: kty,
            ety: elem,
        })
    }

    fn infer_array_lit(
        &mut self,
        items: &[ast::ArrayItem],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let _ = span;
        let mut elems = Vec::new();
        let mut ety: Option<Ty> = None;
        for item in items {
            let key = match &item.key {
                Some(k) => {
                    let kt = self.infer(k, ctx)?;
                    let kty = kt.ty();
                    if !matches!(kty, Ty::Int | Ty::String) {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "array keys must be `int` or `string`, found `{kty}`"
                            ))
                            .with_code("E0250")
                            .with_span(k.span()),
                        );
                        return None;
                    }
                    Some(kt)
                }
                None => None,
            };
            let v = self.infer(&item.value, ctx)?;
            let vt = v.ty();
            if vt == Ty::Null {
                self.diags.emit(
                    Diagnostic::error("`null` is not supported yet")
                        .with_code("E0209")
                        .with_span(item.value.span()),
                );
                return None;
            }
            ety = Some(match &ety {
                None => vt,
                Some(e) => self.unify_types(e, &vt, item.value.span())?,
            });
            elems.push((key, v));
        }
        let ety = ety.unwrap_or(Ty::Null);
        Some(TExpr::ArrayLit { elems, ety })
    }

    fn infer_match(
        &mut self,
        subject: &ast::Expr,
        arms: &[ast::MatchArm],
        _span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let s = self.infer(subject, ctx)?;
        let st = s.ty();
        let mut tarms = Vec::new();
        let mut has_default = false;
        let mut ty: Option<Ty> = None;
        for arm in arms {
            if arm.is_default {
                has_default = true;
            }
            let mut conds = Vec::new();
            for c in &arm.conds {
                let ct = self.infer(c, ctx)?;
                let cty = ct.ty();
                if cty != st {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "match arm condition has type `{cty}` but the subject is `{st}`; `match` uses strict `===` comparison"
                        ))
                        .with_code("E0255")
                        .with_span(c.span()),
                    );
                }
                conds.push(ct);
            }
            let r = self.infer(&arm.result, ctx)?;
            let rt = r.ty();
            ty = Some(match &ty {
                None => rt,
                Some(t) => self.unify_types(t, &rt, arm.result.span())?,
            });
            tarms.push(TArm {
                conds,
                is_default: arm.is_default,
                result: r,
            });
        }
        if !has_default {
            self.diags.emit(
                Diagnostic::error(
                    "`match` must have a `default` arm in PXR (there is no runtime exception mechanism yet)",
                )
                .with_code("E0256")
                .with_span(subject.span()),
            );
        }
        let ty = ty.unwrap_or(Ty::Int);
        Some(TExpr::Match {
            subject: Box::new(s),
            subject_ty: st,
            arms: tarms,
            ty,
        })
    }

    fn bad_operands<T>(&mut self, op: ABinOp, l: &Ty, r: &Ty, span: Span) -> Option<T> {
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

    /// Emit an arity error for a builtin call and abort the call type.
    fn builtin_arity_err(
        &mut self,
        name: &str,
        min: usize,
        max: usize,
        got: usize,
        span: Span,
    ) -> Option<TExpr> {
        let want = if min == max {
            min.to_string()
        } else {
            format!("{min} to {max}")
        };
        self.diags.emit(
            Diagnostic::error(format!(
                "builtin `{name}` expects {want} argument(s), but {got} were given"
            ))
            .with_code("E0233")
            .with_span(span),
        );
        None
    }

    /// Emit a type error for a builtin argument and abort the call type.
    fn builtin_ty_err(&mut self, name: &str, desc: &str, found: &Ty, span: Span) -> Option<TExpr> {
        self.diags.emit(
            Diagnostic::error(format!("`{name}` expects {desc}, found `{found}`"))
                .with_code("E0234")
                .with_span(span),
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
        let name = kind.name();
        let mut targs = Vec::new();
        for a in args {
            targs.push(self.infer(a, ctx)?);
        }
        let atys: Vec<Ty> = targs.iter().map(|t| t.ty()).collect();
        let n = args.len();

        let ret = match kind {
            Builtin::Strlen => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                Ty::Int
            }
            Builtin::Intval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_scalar() {
                    return self.builtin_ty_err(name, "a scalar", &atys[0], args[0].span());
                }
                Ty::Int
            }
            Builtin::Floatval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_scalar() {
                    return self.builtin_ty_err(name, "a scalar", &atys[0], args[0].span());
                }
                Ty::Float
            }
            Builtin::Strval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_scalar() {
                    return self.builtin_ty_err(name, "a scalar", &atys[0], args[0].span());
                }
                Ty::String
            }
            Builtin::Abs => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_numeric() {
                    return self.builtin_ty_err(name, "`int` or `float`", &atys[0], args[0].span());
                }
                atys[0].clone()
            }
            Builtin::Count => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[0], args[0].span());
                }
                Ty::Int
            }
            Builtin::Strtoupper | Builtin::Strtolower => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                Ty::String
            }
            Builtin::Trim | Builtin::Ltrim | Builtin::Rtrim => {
                if !(1..=2).contains(&n) {
                    return self.builtin_arity_err(name, 1, 2, n, span);
                }
                for (i, ty) in atys.iter().enumerate() {
                    if *ty != Ty::String {
                        return self.builtin_ty_err(name, "`string` arguments", ty, args[i].span());
                    }
                }
                Ty::String
            }
            Builtin::Substr => {
                if !(2..=3).contains(&n) {
                    return self.builtin_arity_err(name, 2, 3, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                for (i, ty) in atys.iter().enumerate().skip(1) {
                    if *ty != Ty::Int {
                        return self.builtin_ty_err(
                            name,
                            "an `int` offset/length",
                            ty,
                            args[i].span(),
                        );
                    }
                }
                Ty::String
            }
            Builtin::StrRepeat => {
                if n != 2 {
                    return self.builtin_arity_err(name, 2, 2, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                if atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` count", &atys[1], args[1].span());
                }
                Ty::String
            }
            Builtin::StrReplace => {
                if n != 3 {
                    return self.builtin_arity_err(name, 3, 3, n, span);
                }
                for (i, ty) in atys.iter().enumerate() {
                    if *ty != Ty::String {
                        return self.builtin_ty_err(name, "`string` arguments", ty, args[i].span());
                    }
                }
                Ty::String
            }
            Builtin::Implode => {
                if !(1..=2).contains(&n) {
                    return self.builtin_arity_err(name, 1, 2, n, span);
                }
                if n == 2 && atys[0] != Ty::String {
                    return self.builtin_ty_err(
                        name,
                        "a `string` separator",
                        &atys[0],
                        args[0].span(),
                    );
                }
                let arr_ty = if n == 1 { &atys[0] } else { &atys[1] };
                let elem = match arr_ty {
                    Ty::Array(e) => &**e,
                    _ => {
                        return self.builtin_ty_err(
                            name,
                            "an array of scalar values",
                            arr_ty,
                            args[n - 1].span(),
                        )
                    }
                };
                if !elem.is_scalar() {
                    return self.builtin_ty_err(
                        name,
                        "an array of `string|int|float|bool` elements",
                        elem,
                        args[n - 1].span(),
                    );
                }
                Ty::String
            }
            Builtin::Explode => {
                if !(2..=3).contains(&n) {
                    return self.builtin_arity_err(name, 2, 3, n, span);
                }
                for (i, ty) in atys.iter().enumerate().take(2) {
                    if *ty != Ty::String {
                        return self.builtin_ty_err(name, "`string` arguments", ty, args[i].span());
                    }
                }
                if n == 3 && atys[2] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` limit", &atys[2], args[2].span());
                }
                Ty::array(Ty::String)
            }
            Builtin::StrContains | Builtin::StrStartsWith | Builtin::StrEndsWith => {
                if n != 2 {
                    return self.builtin_arity_err(name, 2, 2, n, span);
                }
                for (i, ty) in atys.iter().enumerate() {
                    if *ty != Ty::String {
                        return self.builtin_ty_err(name, "`string` arguments", ty, args[i].span());
                    }
                }
                Ty::Bool
            }
            Builtin::Strrev
            | Builtin::Ucfirst
            | Builtin::Lcfirst
            | Builtin::Ucwords
            | Builtin::Addslashes
            | Builtin::Stripslashes
            | Builtin::Bin2hex
            | Builtin::Base64Encode
            | Builtin::Urlencode
            | Builtin::Rawurlencode
            | Builtin::Urldecode
            | Builtin::Rawurldecode => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                Ty::String
            }
            Builtin::StrSplit => {
                if !(1..=2).contains(&n) {
                    return self.builtin_arity_err(name, 1, 2, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                if n == 2 && atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` length", &atys[1], args[1].span());
                }
                Ty::array(Ty::String)
            }
            Builtin::ArrayValues => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if !atys[0].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[0], args[0].span());
                }
                atys[0].clone()
            }
            Builtin::ArrayReverse => {
                if !(1..=2).contains(&n) {
                    return self.builtin_arity_err(name, 1, 2, n, span);
                }
                if !atys[0].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[0], args[0].span());
                }
                if n == 2 && atys[1] != Ty::Bool {
                    return self.builtin_ty_err(name, "a `bool` flag", &atys[1], args[1].span());
                }
                atys[0].clone()
            }
            Builtin::ArrayMerge => {
                if n < 1 {
                    self.diags.emit(
                        Diagnostic::error(format!("builtin `{name}` expects at least 1 argument"))
                            .with_code("E0233")
                            .with_span(span),
                    );
                    return None;
                }
                // All arrays must share one element type (an empty `[]` literal
                // contributes `null`, which unifies with anything).
                let mut elem: Option<Ty> = None;
                for (i, t) in atys.iter().enumerate() {
                    if !t.is_array() {
                        return self.builtin_ty_err(name, "an array", t, args[i].span());
                    }
                    let e = t.array_elem().unwrap().clone();
                    if e == Ty::Null {
                        continue;
                    }
                    match &elem {
                        None => elem = Some(e),
                        Some(prev) if *prev == e => {}
                        Some(prev) => {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "`{name}` requires arrays with the same element type, \
                                     found `array<{prev}>` and `array<{e}>`"
                                ))
                                .with_code("E0253")
                                .with_span(args[i].span()),
                            );
                            return None;
                        }
                    }
                }
                Ty::array(elem.unwrap_or(Ty::Null))
            }
            Builtin::ArraySlice => {
                if !(2..=4).contains(&n) {
                    return self.builtin_arity_err(name, 2, 4, n, span);
                }
                if !atys[0].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[0], args[0].span());
                }
                if atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` offset", &atys[1], args[1].span());
                }
                if n >= 3 && atys[2] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` length", &atys[2], args[2].span());
                }
                if n >= 4 && atys[3] != Ty::Bool {
                    return self.builtin_ty_err(name, "a `bool` flag", &atys[3], args[3].span());
                }
                atys[0].clone()
            }
            Builtin::ArrayPad => {
                if n != 3 {
                    return self.builtin_arity_err(name, 3, 3, n, span);
                }
                if !atys[0].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[0], args[0].span());
                }
                if atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` size", &atys[1], args[1].span());
                }
                let elem = atys[0].array_elem().unwrap().clone();
                if !atys[2].assignable_to(&elem) {
                    return self.builtin_ty_err(
                        name,
                        &format!("a value assignable to `{elem}`"),
                        &atys[2],
                        args[2].span(),
                    );
                }
                Ty::array(elem)
            }
            Builtin::ArrayFill => {
                if n != 3 {
                    return self.builtin_arity_err(name, 3, 3, n, span);
                }
                if atys[0] != Ty::Int {
                    return self.builtin_ty_err(
                        name,
                        "an `int` start index",
                        &atys[0],
                        args[0].span(),
                    );
                }
                if atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` count", &atys[1], args[1].span());
                }
                Ty::array(atys[2].clone())
            }
            Builtin::ArrayKeyExists => {
                if n != 2 {
                    return self.builtin_arity_err(name, 2, 2, n, span);
                }
                if atys[0] != Ty::Int && atys[0] != Ty::String {
                    return self.builtin_ty_err(
                        name,
                        "an `int` or `string` key",
                        &atys[0],
                        args[0].span(),
                    );
                }
                if !atys[1].is_array() {
                    return self.builtin_ty_err(name, "an array", &atys[1], args[1].span());
                }
                Ty::Bool
            }
            Builtin::StrPad => {
                if !(2..=4).contains(&n) {
                    return self.builtin_arity_err(name, 2, 4, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(name, "a `string`", &atys[0], args[0].span());
                }
                if atys[1] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` length", &atys[1], args[1].span());
                }
                if n >= 3 && atys[2] != Ty::String {
                    return self.builtin_ty_err(name, "a `string` pad", &atys[2], args[2].span());
                }
                if n >= 4 && atys[3] != Ty::Int {
                    return self.builtin_ty_err(
                        name,
                        "an `int` pad type",
                        &atys[3],
                        args[3].span(),
                    );
                }
                Ty::String
            }
            Builtin::SubstrCount => {
                if !(2..=4).contains(&n) {
                    return self.builtin_arity_err(name, 2, 4, n, span);
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(
                        name,
                        "a `string` haystack",
                        &atys[0],
                        args[0].span(),
                    );
                }
                if atys[1] != Ty::String {
                    return self.builtin_ty_err(
                        name,
                        "a `string` needle",
                        &atys[1],
                        args[1].span(),
                    );
                }
                if n >= 3 && atys[2] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` offset", &atys[2], args[2].span());
                }
                if n >= 4 && atys[3] != Ty::Int {
                    return self.builtin_ty_err(name, "an `int` length", &atys[3], args[3].span());
                }
                Ty::Int
            }
            Builtin::Sprintf | Builtin::Printf => {
                if n < 1 {
                    self.diags.emit(
                        Diagnostic::error(format!("builtin `{name}` expects at least 1 argument"))
                            .with_code("E0233")
                            .with_span(span),
                    );
                    return None;
                }
                if atys[0] != Ty::String {
                    return self.builtin_ty_err(
                        name,
                        "a `string` format",
                        &atys[0],
                        args[0].span(),
                    );
                }
                for i in 1..n {
                    match &atys[i] {
                        Ty::Int | Ty::Float | Ty::Bool | Ty::String => {}
                        other => {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "builtin `{name}` argument {} must be a scalar, got `{other}`",
                                    i + 1
                                ))
                                .with_code("E0234")
                                .with_span(args[i].span()),
                            );
                            return None;
                        }
                    }
                }
                if kind == Builtin::Sprintf {
                    Ty::String
                } else {
                    Ty::Int
                }
            }
        };

        let _ = callee_span;
        let _ = callee;
        Some(TExpr::Call {
            callee: Callee::Builtin {
                kind,
                arg_tys: atys.clone(),
            },
            ret,
            arg_tys: atys,
            args: targs,
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
    matches!(
        ty,
        Ty::Bool | Ty::Int | Ty::Float | Ty::String | Ty::Named(_)
    )
}

/// Types that may be compared with `==` / `!=` (and `switch` case matching).
fn comparable_eq(a: &Ty, b: &Ty) -> bool {
    (a.is_numeric() && b.is_numeric())
        || (a == b && matches!(a, Ty::String | Ty::Bool | Ty::Named(_)))
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

pub fn mangle_method(class: &str, method: &str) -> String {
    format!(
        "pxr$m${}${}",
        class.to_ascii_lowercase(),
        method.to_ascii_lowercase()
    )
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
        let (_, d) =
            run("<?php function add(int $a, int $b): int { return $a + $b; } echo add(1,2);");
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

    #[test]
    fn valid_class_and_object_usage() {
        let (prog, d) = run("<?php
            class Point {
                public int $x;
                public int $y;

                public function __construct(int $x, int $y) {
                    $this->x = $x;
                    $this->y = $y;
                }

                public function sum(): int {
                    return $this->x + $this->y;
                }
            }
            $p = new Point(10, 20);
            $p->x = 15;
            echo $p->sum();
        ");
        assert!(!d.has_errors(), "{:#?}", d.items());
        assert!(prog.is_some());
    }

    #[test]
    fn reject_private_member_access() {
        let (_, d) = run("<?php
            class Secret {
                private int $val;
                public function __construct(int $v) { $this->val = $v; }
                private function hidden(): int { return $this->val; }
            }
            $s = new Secret(42);
            echo $s->val;
        ");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0265")));

        let (_, d2) = run("<?php
            class Secret {
                private function hidden(): int { return 42; }
            }
            $s = new Secret();
            echo $s->hidden();
        ");
        assert!(d2.has_errors());
        assert!(d2.items().iter().any(|x| x.code == Some("E0265")));
    }

    #[test]
    fn reject_assigning_to_this() {
        let (_, d) = run("<?php
            class Foo {
                public function test(): void {
                    $this = 1;
                }
            }
        ");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0267")));
    }

    #[test]
    fn reject_this_outside_class() {
        let (_, d) = run("<?php
            echo $this->x;
        ");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0268")));
    }

    #[test]
    fn reject_unknown_property_and_method() {
        let (_, d) = run("<?php
            class A { public int $x; }
            $a = new A();
            echo $a->nonexistent;
        ");
        assert!(d.has_errors());
        assert!(d.items().iter().any(|x| x.code == Some("E0261")));

        let (_, d2) = run("<?php
            class A { public int $x; }
            $a = new A();
            $a->nope();
        ");
        assert!(d2.has_errors());
        assert!(d2.items().iter().any(|x| x.code == Some("E0262")));
    }
}
