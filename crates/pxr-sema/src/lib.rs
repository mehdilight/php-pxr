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
use std::collections::{HashMap, VecDeque};

mod class_generics;
mod constraints;
mod generics;
mod ownership;
mod return_inference;
mod traits;

/// Aliases re-exported so downstream stages can refer to the operator enums
/// through the semantic layer.
pub use pxr_ast::{BinOp as TBinOp, UnOp as TUnOp};

// ---------------------------------------------------------------------------
// Typed AST
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    IsInt,
    IsFloat,
    IsBool,
    IsString,
    IsArray,
    IsObject,
    IsNull,
    IsScalar,
    Boolval,

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
            "is_int" => Builtin::IsInt,
            "is_float" => Builtin::IsFloat,
            "is_bool" => Builtin::IsBool,
            "is_string" => Builtin::IsString,
            "is_array" => Builtin::IsArray,
            "is_object" => Builtin::IsObject,
            "is_null" => Builtin::IsNull,
            "is_scalar" => Builtin::IsScalar,
            "boolval" => Builtin::Boolval,

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
            Builtin::IsInt => "is_int",
            Builtin::IsFloat => "is_float",
            Builtin::IsBool => "is_bool",
            Builtin::IsString => "is_string",
            Builtin::IsArray => "is_array",
            Builtin::IsObject => "is_object",
            Builtin::IsNull => "is_null",
            Builtin::IsScalar => "is_scalar",
            Builtin::Boolval => "boolval",
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
    /// A statically checked async string handler, passed as a native constructor.
    AsyncServer {
        handler: String,
        http: bool,
    },
    Parallel {
        handler: String,
    },
    Php {
        name: String,
        fallback: Option<String>,
    },
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
    Effect {
        statement: Box<TStmt>,
        postfix: bool,
        ty: Ty,
    },
    /// Internal inference placeholder; never reaches IR generation.
    Deferred,
    /// Ownership-checked argument: transfer/borrow without PHP array copying.
    Pass {
        expr: Box<TExpr>,
        borrowed: bool,
        span: Span,
    },
    Coerce {
        expr: Box<TExpr>,
        ty: Ty,
    },
    Int(i64),
    Float(f64),
    /// Raw byte-string literal (PHP strings are byte strings).
    Str(Vec<u8>),
    Bool(bool),
    /// Null is a boxed mixed value, distinct from the empty-array sentinel.
    Null,
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
        class_id: u32,
        class_name: String,
        num_props: u32,
        prop_defaults: Vec<(u32, TExpr)>,
        ctor: Option<(String, Vec<TExpr>, Vec<Ty>)>,
        ty: Ty,
    },
    Instanceof {
        expr: Box<TExpr>,
        target_class_id: u32,
        target_class_name: String,
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
            TExpr::Effect { ty, .. } => ty.clone(),
            TExpr::Deferred => Ty::Infer,
            TExpr::Pass { expr, .. } => expr.ty(),
            TExpr::Coerce { ty, .. } => ty.clone(),
            TExpr::Int(_) => Ty::Int,
            TExpr::Float(_) => Ty::Float,
            TExpr::Str(_) => Ty::String,
            TExpr::Bool(_) => Ty::Bool,
            TExpr::Null => Ty::Mixed,
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
            TExpr::Instanceof { .. } => Ty::Bool,
        }
    }
}

/// A typed assignment target.
#[derive(Clone, Debug, PartialEq)]
pub enum TTarget {
    /// A mixed slot read through a proven scalar refinement; stores remain boxed.
    MixedVar {
        name: String,
        slot: u32,
        ty: Ty,
    },
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
    /// An array element reached through another writable array/property.
    Nested {
        parent: Box<TTarget>,
        array_ty: Ty,
        elem_ty: Ty,
        key: Option<(Box<TExpr>, Ty)>,
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

impl TTarget {
    pub fn storage_ty(&self) -> Ty {
        match self {
            Self::MixedVar { .. } => Ty::Mixed,
            Self::Var { ty, .. } | Self::Property { ty, .. } => ty.clone(),
            Self::Index { elem_ty, .. }
            | Self::Push { elem_ty, .. }
            | Self::Nested { elem_ty, .. } => elem_ty.clone(),
        }
    }
}

// Value possibility sets for local mixed slots. Facts describe values, never
// alter slot storage types, and are joined only across reachable paths.
const SCALAR_MASK: u8 = 15;
const VALUE_MASK: u8 = 127; // Scalars, null, arrays, and objects.
#[derive(Clone, Debug)]
struct FlowState {
    initialized: std::collections::HashSet<u32>,
    types: HashMap<u32, u8>,
    named: HashMap<u32, String>,
    reachable: bool,
}
impl Default for FlowState {
    fn default() -> Self {
        Self {
            initialized: std::collections::HashSet::new(),
            types: HashMap::new(),
            named: HashMap::new(),
            reachable: true,
        }
    }
}
impl FlowState {
    fn forget(&mut self, slot: u32) {
        self.types.remove(&slot);
        self.named.remove(&slot);
    }
    fn mask(&self, slot: u32) -> u8 {
        self.types.get(&slot).copied().unwrap_or(VALUE_MASK)
    }
    fn restrict(&mut self, slot: u32, mask: u8) {
        let mask = self.mask(slot) & mask;
        self.types.insert(slot, mask);
        if mask == 0 {
            self.reachable = false;
        }
    }
    fn narrowed(&self, slot: u32) -> Option<Ty> {
        match self.mask(slot) {
            1 => Some(Ty::Int),
            2 => Some(Ty::Float),
            4 => Some(Ty::Bool),
            8 => Some(Ty::String),
            32 => Some(Ty::array(Ty::Mixed)),
            64 => self.named.get(&slot).cloned().map(Ty::Named),
            _ => None,
        }
    }
    fn merge(paths: &[Self]) -> Self {
        let paths: Vec<_> = paths.iter().filter(|p| p.reachable).collect();
        let mut result = Self::default();
        result.reachable = !paths.is_empty();
        if let Some(first) = paths.first() {
            result.initialized = first
                .initialized
                .iter()
                .copied()
                .filter(|slot| paths.iter().all(|p| p.initialized.contains(slot)))
                .collect();
        }
        for slot in paths.iter().flat_map(|p| p.types.keys()) {
            result
                .types
                .insert(*slot, paths.iter().fold(0, |mask, p| mask | p.mask(*slot)));
        }
        for slot in paths.iter().flat_map(|p| p.named.keys()) {
            if let Some(name) = paths[0].named.get(slot) {
                if paths.iter().all(|p| p.named.get(slot) == Some(name)) {
                    result.named.insert(*slot, name.clone());
                }
            }
        }
        result
    }
    fn condition(&self, expression: &TExpr, truth: bool) -> Self {
        match expression {
            TExpr::Un {
                op: AUnOp::Not,
                expr,
                ..
            } => self.condition(expr, !truth),
            TExpr::Bin {
                op: ABinOp::And,
                lhs,
                rhs,
                ..
            } => {
                if truth {
                    self.condition(lhs, true).condition(rhs, true)
                } else {
                    Self::merge(&[
                        self.condition(lhs, false),
                        self.condition(lhs, true).condition(rhs, false),
                    ])
                }
            }
            TExpr::Bin {
                op: ABinOp::Or,
                lhs,
                rhs,
                ..
            } => {
                if !truth {
                    self.condition(lhs, false).condition(rhs, false)
                } else {
                    Self::merge(&[
                        self.condition(lhs, true),
                        self.condition(lhs, false).condition(rhs, true),
                    ])
                }
            }
            TExpr::Instanceof {
                expr,
                target_class_name,
                ..
            } if truth => {
                let mut result = self.clone();
                if let Some(slot) = mixed_slot(expr) {
                    result.restrict(slot, 64);
                    result.named.insert(slot, target_class_name.clone());
                }
                result
            }
            TExpr::Bool(value) => {
                let mut result = self.clone();
                result.reachable &= *value == truth;
                result
            }
            TExpr::Bin { op, lhs, rhs, .. }
                if matches!(op, ABinOp::Identical | ABinOp::NotIdentical) =>
            {
                let equal = truth == (*op == ABinOp::Identical);
                if let TExpr::Bool(value) = &**rhs {
                    if lhs.ty() == Ty::Bool {
                        return self.condition(lhs, equal == *value);
                    }
                }
                if let TExpr::Bool(value) = &**lhs {
                    if rhs.ty() == Ty::Bool {
                        return self.condition(rhs, equal == *value);
                    }
                }
                let mut result = self.clone();
                for (candidate, other) in [(lhs, rhs), (rhs, lhs)] {
                    if matches!(&**other, TExpr::Null) {
                        if let Some(slot) = mixed_slot(candidate) {
                            result.restrict(slot, if equal { 16 } else { VALUE_MASK & !16 });
                        }
                    }
                }
                if equal {
                    if let Some(slot) = mixed_slot(lhs) {
                        if rhs.ty().is_scalar() {
                            result.restrict(slot, scalar_mask(&rhs.ty()));
                        }
                    }
                    if let Some(slot) = mixed_slot(rhs) {
                        if lhs.ty().is_scalar() {
                            result.restrict(slot, scalar_mask(&lhs.ty()));
                        }
                    }
                }
                result
            }
            TExpr::Call {
                callee: Callee::Builtin { kind, .. },
                args,
                ..
            } => {
                let mask = match kind {
                    Builtin::IsInt => 1,
                    Builtin::IsFloat => 2,
                    Builtin::IsBool => 4,
                    Builtin::IsString => 8,
                    Builtin::IsScalar => SCALAR_MASK,
                    Builtin::IsNull => 16,
                    Builtin::IsArray => 32,
                    Builtin::IsObject => 64,
                    _ => return self.clone(),
                };
                let mut result = self.clone();
                if let Some(slot) = args.first().and_then(mixed_slot) {
                    result.restrict(slot, if truth { mask } else { VALUE_MASK & !mask });
                }
                result
            }
            _ => self.clone(),
        }
    }
}
fn mixed_slot(expression: &TExpr) -> Option<u32> {
    match expression {
        TExpr::Var {
            slot,
            ty: Ty::Mixed,
            ..
        } => Some(*slot),
        TExpr::Coerce { expr, .. } => mixed_slot(expr),
        _ => None,
    }
}
fn scalar_mask(ty: &Ty) -> u8 {
    match ty {
        Ty::Int => 1,
        Ty::Float => 2,
        Ty::Bool => 4,
        Ty::String => 8,
        Ty::Array(_) => 32,
        Ty::Named(_) => 64,
        _ => VALUE_MASK,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TStmt {
    PhpInclude {
        path: String,
        mode: u8,
    },
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
        key_slot: Option<u32>,
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
    pub is_async: bool,
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
    pub class_registrations: Vec<(u32, u32)>,
    pub stringifiers: Vec<(u32, String)>,
    pub worker_bootstrap: Vec<TStmt>,
    pub parallel_handlers: std::collections::HashSet<String>,
}

// ---------------------------------------------------------------------------
// Analyzer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ClassInfo {
    pub is_interface: bool,
    pub interfaces: Vec<String>,
    pub is_abstract: bool,
    pub class_id: u32,
    pub name: String,
    pub name_span: Span,
    pub parent: Option<String>,
    pub parent_span: Option<Span>,
    pub properties: Vec<PropertyInfo>,
    pub prop_indices: HashMap<String, usize>,
    pub methods: HashMap<String, MethodInfo>,
}

#[derive(Clone, Debug)]
pub struct PropertyInfo {
    pub is_static: bool,
    pub declaring_class: String,
    pub name: String,
    pub name_span: Span,
    pub ty: Ty,
    pub visibility: ast::Visibility,
    pub default: Option<ast::Expr>,
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub declaring_class: String,
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
    parallel_handlers: std::collections::HashSet<String>,
    worker_bootstrap: Vec<TStmt>,
    unresolved_returns: Vec<(String, Span)>,
    return_contract_errors: Vec<Diagnostic>,
    class_expander: class_generics::ClassExpander,
    validating_template: bool,
    php_interop: bool,
    diags: Diagnostics,
    /// lowercased name -> signature
    funcs: HashMap<String, Signature>,
    generics: HashMap<String, ast::FnDecl>,
    specializations: HashMap<(String, Vec<Ty>), Signature>,
    pending_specializations: VecDeque<(ast::FnDecl, Signature, HashMap<String, Ty>)>,
    /// lowercased name -> class info
    classes: HashMap<String, ClassInfo>,
}

#[derive(Clone, Debug)]
struct Signature {
    is_async: bool,
    mangled: String,
    display: String,
    params: Vec<Ty>,
    param_names: Vec<String>,
    ret: Ty,
    span: Span,
}

/// Result of analysing one function body.
struct FnCtx {
    is_async: bool,
    infer_return: bool,
    inference_failed: bool,
    return_types: Vec<(Ty, Span)>,
    flow: FlowState,
    type_bindings: HashMap<String, Ty>,
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
            is_async: false,
            infer_return: false,
            inference_failed: false,
            return_types: Vec::new(),
            flow: FlowState::default(),
            type_bindings: HashMap::new(),
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
        self.flow.initialized.insert(slot);
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

fn uses_php(statement: &ast::Stmt) -> bool {
    matches!(statement, ast::Stmt::PhpInclude { .. })
        || statement
            .bodies()
            .iter()
            .any(|body| body.iter().any(uses_php))
}

pub fn analyze(program: &ast::Program) -> (Option<TProgram>, Diagnostics) {
    let (mut expanded_traits, diagnostics) = traits::expand(program);
    if diagnostics.has_errors() {
        return (None, diagnostics);
    }
    for item in &expanded_traits.items {
        if let ast::Stmt::ClassDecl(class) = item {
            if class
                .name
                .rsplit('\\')
                .next()
                .unwrap_or(&class.name)
                .eq_ignore_ascii_case("TaskResult")
            {
                let mut diagnostics = Diagnostics::new();
                diagnostics.emit(
                    Diagnostic::error("TaskResult is a reserved native result type")
                        .with_code("E0300")
                        .with_span(class.name_span),
                );
                return (None, diagnostics);
            }
        }
    }
    for name in ["TcpStream", "TcpListener", "Channel"] {
        for item in &expanded_traits.items {
            if let ast::Stmt::ClassDecl(class) = item {
                if class.name.eq_ignore_ascii_case(name)
                    || class
                        .extends
                        .as_ref()
                        .is_some_and(|parent| parent.name.eq_ignore_ascii_case(name))
                {
                    let mut diagnostics = Diagnostics::new();
                    diagnostics.emit(
                        Diagnostic::error("native resource types cannot be declared or extended")
                            .with_code("E0300")
                            .with_span(class.name_span),
                    );
                    return (None, diagnostics);
                }
            }
        }
        expanded_traits
            .items
            .push(ast::Stmt::ClassDecl(Box::new(ast::ClassDecl {
                type_params: vec![],
                bounds: vec![],
                is_trait: false,
                trait_uses: vec![],
                is_interface: false,
                interfaces: vec![],
                is_abstract: true,
                name: name.into(),
                name_span: Span::synthetic(),
                extends: None,
                members: vec![],
                span: Span::synthetic(),
            })));
    }
    let (owned_program, diagnostics) = ownership::check_and_erase(&expanded_traits);
    if diagnostics.has_errors() {
        return (None, diagnostics);
    }
    let program = &owned_program;
    let mut class_expander = class_generics::ClassExpander::new(program);
    for _ in 0..2048 {
        let expanded = class_expander.program(program);
        if class_expander.diags.has_errors() {
            return (None, class_expander.diags);
        }
        let mut a = Analyzer {
            parallel_handlers: std::collections::HashSet::new(),
            worker_bootstrap: vec![],
            unresolved_returns: Vec::new(),
            return_contract_errors: Vec::new(),
            class_expander,
            validating_template: false,
            php_interop: program.items.iter().any(uses_php),
            diags: Diagnostics::new(),
            funcs: HashMap::new(),
            generics: HashMap::new(),
            specializations: HashMap::new(),
            pending_specializations: VecDeque::new(),
            classes: HashMap::new(),
        };
        let result = a.run(&expanded);
        if let Some(next) = a.class_expander.next_inference_epoch(program) {
            class_expander = next;
            continue;
        }
        if a.class_expander.diags.has_errors() {
            return (None, a.class_expander.diags);
        }
        if !a.class_expander.has_pending() {
            if let Some(program) = &result {
                ownership::check_representation(program, &mut a.diags);
            }
            return (if a.diags.has_errors() { None } else { result }, a.diags);
        }
        // A generic function can discover a class specialization through its
        // inferred type bindings. Recheck against the expanded declaration graph;
        // only diagnostics from the complete graph are externally visible.
        class_expander = a.class_expander;
    }
    let mut diagnostics = Diagnostics::new();
    diagnostics.emit(
        Diagnostic::error(
            "return/class inference did not converge; add explicit return annotations",
        )
        .with_code("E0284")
        .with_span(program.span),
    );
    (None, diagnostics)
}

impl Analyzer {
    pub fn is_subclass_of(&self, child: &str, parent: &str) -> bool {
        let mut pending = vec![child.to_ascii_lowercase()];
        let target = parent.to_ascii_lowercase();
        let mut visited = std::collections::HashSet::new();
        while let Some(key) = pending.pop() {
            if !visited.insert(key.clone()) {
                continue;
            }
            if let Some(c) = self.classes.get(&key) {
                for ancestor in c.parent.iter().chain(c.interfaces.iter()) {
                    if *ancestor == target {
                        return true;
                    }
                    pending.push(ancestor.clone());
                }
            }
        }
        false
    }

    pub fn is_assignable(&self, from: &Ty, to: &Ty) -> bool {
        if from.contains_infer() || to.contains_infer() {
            return true;
        }
        if from == to {
            return true;
        }
        match (from, to) {
            (from, Ty::Mixed)
                if *from != Ty::Void && !matches!(from, Ty::Task(_) | Ty::TaskResult(_)) =>
            {
                true
            }
            (Ty::Int, Ty::Float) => true,
            (Ty::Array(e), Ty::Array(t)) => {
                matches!(**e, Ty::Null)
                    || e == t
                    || **t == Ty::Mixed
                    || self.array_layout_assignable(e, t)
            }
            (Ty::Named(child), Ty::Named(parent)) => self.is_subclass_of(child, parent),
            _ => false,
        }
    }

    fn array_layout_assignable(&self, from: &Ty, to: &Ty) -> bool {
        if from == to || *from == Ty::Null {
            return true;
        }
        match (from, to) {
            (Ty::Named(child), Ty::Named(parent)) => self.is_subclass_of(child, parent),
            (Ty::Array(from), Ty::Array(to)) => self.array_layout_assignable(from, to),
            _ => false,
        }
    }

    // Numeric widening is an expression coercion, not signature covariance:
    // dispatch must preserve the implementation's ABI.
    fn compatible_return(&self, from: &Ty, to: &Ty) -> bool {
        if from.contains_infer() || to.contains_infer() {
            return true;
        }
        from == to
            || matches!((from, to), (Ty::Named(_), Ty::Named(_))) && self.is_assignable(from, to)
    }

    fn check_member_visibility(
        &mut self,
        declaring_class: &str,
        vis: ast::Visibility,
        span: Span,
        ctx: &FnCtx,
    ) -> bool {
        match vis {
            ast::Visibility::Public => true,
            ast::Visibility::Protected => {
                let Some(current) = &ctx.current_class else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access protected member of class `{declaring_class}` outside class context"
                        ))
                        .with_code("E0265")
                        .with_span(span),
                    );
                    return false;
                };
                if current.eq_ignore_ascii_case(declaring_class)
                    || self.is_subclass_of(current, declaring_class)
                    || self.is_subclass_of(declaring_class, current)
                {
                    true
                } else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access protected member of class `{declaring_class}` from class `{current}`"
                        ))
                        .with_code("E0265")
                        .with_span(span),
                    );
                    false
                }
            }
            ast::Visibility::Private => {
                let Some(current) = &ctx.current_class else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access private member of class `{declaring_class}` outside class context"
                        ))
                        .with_code("E0265")
                        .with_span(span),
                    );
                    return false;
                };
                if current.eq_ignore_ascii_case(declaring_class) {
                    true
                } else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot access private member of class `{declaring_class}` from class `{current}`"
                        ))
                        .with_code("E0265")
                        .with_span(span),
                    );
                    false
                }
            }
        }
    }

    fn method_has_overrides(&self, root_class: &str, method_name: &str) -> bool {
        let mkey = method_name.to_ascii_lowercase();
        let root_info = match self.classes.get(&root_class.to_ascii_lowercase()) {
            Some(c) => c,
            None => return false,
        };
        let root_mangled = match root_info.methods.get(&mkey) {
            Some(m) => &m.mangled,
            None => return false,
        };
        for class in self.classes.values() {
            if self.is_subclass_of(&class.name, root_class) {
                if let Some(sub_m) = class.methods.get(&mkey) {
                    if sub_m.mangled != *root_mangled {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn synthesize_dispatcher(&self, root_class: &ClassInfo, method: &MethodInfo) -> TFunc {
        let result = if method.ast_method.is_async {
            Ty::Task(Box::new(method.ret.clone()))
        } else {
            method.ret.clone()
        };
        let mkey = method.name.to_ascii_lowercase();
        let disp_mangled = mangle_dispatcher(&root_class.name, &method.name);
        let disp_display = format!("{}::{}$dispatch", root_class.name, method.name);

        let mut slots = vec![Ty::Named(root_class.name.clone())];
        let mut param_vars = Vec::new();
        let mut param_tys = vec![("$this".to_string(), Ty::Named(root_class.name.clone()))];
        for (i, pty) in method.params.iter().enumerate() {
            slots.push(pty.clone());
            let pname = format!("$arg{i}");
            param_tys.push((pname.clone(), pty.clone()));
            param_vars.push(TExpr::Var {
                name: pname,
                slot: (i + 1) as u32,
                ty: pty.clone(),
            });
        }
        let cid_slot = slots.len() as u32;
        slots.push(Ty::Int); // $cid

        let get_cid = TExpr::Call {
            callee: Callee::User {
                mangled: "pxr_object_get_class_id".to_string(),
                name: "pxr_object_get_class_id".to_string(),
                params: vec![Ty::Named(root_class.name.clone())],
            },
            ret: Ty::Int,
            args: vec![TExpr::Var {
                name: "$this".to_string(),
                slot: 0,
                ty: Ty::Named(root_class.name.clone()),
            }],
            arg_tys: vec![Ty::Named(root_class.name.clone())],
            span: Span::synthetic(),
        };
        let init_cid = TStmt::Assign {
            target: TTarget::Var {
                name: "$cid".to_string(),
                slot: cid_slot,
                ty: Ty::Int,
            },
            ty: Ty::Int,
            value: get_cid,
        };

        let mut groups: HashMap<String, Vec<u32>> = HashMap::new();
        for sub in self.classes.values() {
            if self.is_subclass_of(&sub.name, &root_class.name) {
                if let Some(sub_m) = sub.methods.get(&mkey) {
                    if sub_m.mangled != method.mangled {
                        groups
                            .entry(sub_m.mangled.clone())
                            .or_default()
                            .push(sub.class_id);
                    }
                }
            }
        }

        let mut body = vec![init_cid];

        let make_call = |target_mangled: &str| -> TExpr {
            let mut call_args = vec![TExpr::Var {
                name: "$this".to_string(),
                slot: 0,
                ty: Ty::Named(root_class.name.clone()),
            }];
            call_args.extend(param_vars.clone());
            let mut p = vec![Ty::Named(root_class.name.clone())];
            p.extend(method.params.clone());
            TExpr::Call {
                callee: Callee::User {
                    mangled: target_mangled.to_string(),
                    name: target_mangled.to_string(),
                    params: p.clone(),
                },
                ret: result.clone(),
                args: call_args,
                arg_tys: p,
                span: Span::synthetic(),
            }
        };

        let make_return_stmts = |target_mangled: &str| -> Vec<TStmt> {
            let call = make_call(target_mangled);
            if result == Ty::Void {
                vec![
                    TStmt::Expr(call),
                    TStmt::Return {
                        value: None,
                        ret: Ty::Void,
                    },
                ]
            } else {
                vec![TStmt::Return {
                    value: Some(call),
                    ret: result.clone(),
                }]
            }
        };

        for (target_mangled, ids) in groups {
            let mut cond = TExpr::Bin {
                op: TBinOp::Eq,
                lhs: Box::new(TExpr::Var {
                    name: "$cid".to_string(),
                    slot: cid_slot,
                    ty: Ty::Int,
                }),
                rhs: Box::new(TExpr::Int(ids[0] as i64)),
                lhs_ty: Ty::Int,
                rhs_ty: Ty::Int,
                ty: Ty::Bool,
            };
            for id in &ids[1..] {
                let eq = TExpr::Bin {
                    op: TBinOp::Eq,
                    lhs: Box::new(TExpr::Var {
                        name: "$cid".to_string(),
                        slot: cid_slot,
                        ty: Ty::Int,
                    }),
                    rhs: Box::new(TExpr::Int(*id as i64)),
                    lhs_ty: Ty::Int,
                    rhs_ty: Ty::Int,
                    ty: Ty::Bool,
                };
                cond = TExpr::Bin {
                    op: TBinOp::Or,
                    lhs: Box::new(cond),
                    rhs: Box::new(eq),
                    lhs_ty: Ty::Bool,
                    rhs_ty: Ty::Bool,
                    ty: Ty::Bool,
                };
            }
            body.push(TStmt::If {
                cond,
                cond_ty: Ty::Bool,
                then_branch: make_return_stmts(&target_mangled),
                elifs: Vec::new(),
                else_branch: None,
            });
        }

        body.extend(make_return_stmts(&method.mangled));

        TFunc {
            is_async: false,
            name: disp_mangled,
            display: disp_display,
            params: param_tys,
            ret: result.clone(),
            slots,
            body,
        }
    }

    fn run(&mut self, program: &ast::Program) -> Option<TProgram> {
        self.collect_classes(program);
        for (name, bounds, bindings, span) in self.class_expander.class_constraints() {
            self.validate_bounds(&bounds);
            self.check_bounds(&name, &bounds, &bindings, span);
        }
        if self.diags.has_errors() {
            return None;
        }
        self.collect_signatures(program);
        let mut stringifiers = Vec::new();
        for class in self.classes.values() {
            if let Some(method) = class.methods.get("__tostring") {
                if method.ret != Ty::String
                    || !method.params.is_empty()
                    || method.ast_method.is_static
                    || method.visibility != ast::Visibility::Public
                {
                    self.diags.emit(Diagnostic::error("__toString must be public, non-static, take no arguments, and return string").with_code("E0275").with_span(method.name_span));
                } else if !class.is_abstract && !class.is_interface {
                    stringifiers.push((class.class_id, method.mangled.clone()));
                }
            }
        }
        stringifiers.sort();

        let mut funcs = Vec::new();

        // Analyze methods declared in classes
        for class_key in self.classes.keys().cloned().collect::<Vec<_>>() {
            let class = self.classes.get(&class_key).unwrap().clone();
            for method in class.methods.values() {
                if !method.ast_method.type_params.is_empty() {
                    continue;
                }
                if method.declaring_class.eq_ignore_ascii_case(&class.name) {
                    if let Some(tf) = self.analyze_method(&class.name, method) {
                        funcs.push(tf);
                    }
                }
            }
        }

        // Synthesize dynamic dispatchers for overridden virtual methods
        for class_key in self.classes.keys().cloned().collect::<Vec<_>>() {
            let class = self.classes.get(&class_key).unwrap().clone();
            for method in class.methods.values() {
                if !method.ast_method.type_params.is_empty() {
                    continue;
                }
                if !method.ast_method.is_static
                    && (method.name != "__construct" || method.ast_method.is_abstract)
                    && self.method_has_overrides(&class.name, &method.name)
                {
                    let disp_tf = self.synthesize_dispatcher(&class, method);
                    funcs.push(disp_tf);
                }
            }
        }

        // Analyze user functions
        for item in &program.items {
            if let ast::Stmt::FnDecl(f) = item {
                if !f.type_params.is_empty() {
                    continue;
                }
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
        while let Some((declaration, signature, bindings)) =
            self.pending_specializations.pop_front()
        {
            let previous = std::mem::take(&mut self.diags);
            let function = self.analyze_function_with_signature(&declaration, &signature, bindings);
            let specialized_diags = std::mem::take(&mut self.diags);
            self.diags = previous;
            for diagnostic in specialized_diags.items() {
                self.diags.emit(
                    diagnostic
                        .clone()
                        .with_note(format!("while specializing `{}`", signature.display)),
                );
            }
            funcs.push(function);
        }

        if !self.class_expander.has_pending() {
            for diagnostic in std::mem::take(&mut self.return_contract_errors) {
                self.diags.emit(diagnostic);
            }
        }
        if self.diags.has_errors() {
            return None;
        }

        if !self.class_expander.has_pending() && !self.unresolved_returns.is_empty() {
            for (name, span) in &self.unresolved_returns {
                self.diags.emit(Diagnostic::error(format!("cannot resolve the recursive return type of `{name}`; add an explicit return annotation")).with_code("E0284").with_span(*span));
            }
            return None;
        }
        if !self.class_expander.has_pending()
            && (funcs.iter().any(|function| {
                function.ret.contains_infer() || function.slots.iter().any(Ty::contains_infer)
            }) || main
                .as_ref()
                .is_some_and(|function| function.slots.iter().any(Ty::contains_infer)))
        {
            self.diags.emit(Diagnostic::error("return inference left unresolved value types; add an explicit return annotation").with_code("E0284").with_span(program.span));
            return None;
        }
        // Register the transitive closure, including interface membership.
        let mut class_registrations = Vec::new();
        for class in self.classes.values() {
            for ancestor in self.classes.values() {
                if self.is_subclass_of(&class.name, &ancestor.name) {
                    class_registrations.push((class.class_id, ancestor.class_id));
                }
            }
        }
        class_registrations.sort_unstable();

        Some(TProgram {
            funcs,
            main: main?,
            class_registrations,
            stringifiers,
            worker_bootstrap: self.worker_bootstrap.clone(),
            parallel_handlers: self.parallel_handlers.clone(),
        })
    }

    fn collect_classes(&mut self, program: &ast::Program) {
        // Step 1: register all class names and assign unique class IDs
        let mut class_id_counter = 1u32;
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
            let class_id = class_id_counter;
            class_id_counter += 1;
            self.classes.insert(
                key,
                ClassInfo {
                    is_interface: c.is_interface,
                    interfaces: c
                        .interfaces
                        .iter()
                        .map(|reference| reference.name.to_ascii_lowercase())
                        .collect(),
                    is_abstract: c.is_abstract,
                    class_id,
                    name: c.name.clone(),
                    name_span: c.name_span,
                    parent: c
                        .extends
                        .as_ref()
                        .map(|reference| reference.name.to_ascii_lowercase()),
                    parent_span: c.extends.as_ref().map(|reference| reference.span),
                    properties: Vec::new(),
                    prop_indices: HashMap::new(),
                    methods: HashMap::new(),
                },
            );
        }

        // Validate the class/interface graph before resolving any members.
        for item in &program.items {
            let ast::Stmt::ClassDecl(c) = item else {
                continue;
            };
            let mut seen = std::collections::HashSet::new();
            let edges = c
                .extends
                .iter()
                .map(|reference| (&reference.name, reference.span, false))
                .chain(
                    c.interfaces
                        .iter()
                        .map(|reference| (&reference.name, reference.span, true)),
                );
            for (name, span, want_interface) in edges {
                let key = name.to_ascii_lowercase();
                let Some(target) = self.classes.get(&key) else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "unknown {} `{name}`",
                            if want_interface { "interface" } else { "class" }
                        ))
                        .with_code("E0205")
                        .with_span(span),
                    );
                    continue;
                };
                if !seen.insert(key.clone()) {
                    self.diags.emit(
                        Diagnostic::error(format!("duplicate supertype `{name}`"))
                            .with_code("E0270")
                            .with_span(span),
                    );
                }
                if target.is_interface != want_interface {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`{name}` must be {} here",
                            if want_interface {
                                "an interface"
                            } else {
                                "a class"
                            }
                        ))
                        .with_code("E0270")
                        .with_span(span),
                    );
                }
                if key == c.name.to_ascii_lowercase() || self.is_subclass_of(name, &c.name) {
                    self.diags.emit(
                        Diagnostic::error(format!("circular inheritance involving `{}`", c.name))
                            .with_code("E0201")
                            .with_span(span),
                    );
                }
            }
        }
        if self.diags.has_errors() {
            return;
        }

        // Step 3: topological order: ancestors before children
        let mut order = Vec::new();
        let mut visited = std::collections::HashSet::new();
        fn visit(
            key: &str,
            classes: &HashMap<String, ClassInfo>,
            visited: &mut std::collections::HashSet<String>,
            order: &mut Vec<String>,
        ) {
            if visited.contains(key) {
                return;
            }
            visited.insert(key.to_string());
            if let Some(info) = classes.get(key) {
                for parent in info.parent.iter().chain(info.interfaces.iter()) {
                    visit(parent, classes, visited, order);
                }
            }
            order.push(key.to_string());
        }
        for k in self.classes.keys().cloned().collect::<Vec<_>>() {
            visit(&k, &self.classes, &mut visited, &mut order);
        }

        // Map AST class decls by lowercased name
        let mut decls_by_name = HashMap::new();
        for item in &program.items {
            if let ast::Stmt::ClassDecl(c) = item {
                decls_by_name.insert(c.name.to_ascii_lowercase(), c.clone());
            }
        }

        // Step 4: resolve properties and methods in topological order
        for class_key in order {
            let Some(c) = decls_by_name.get(&class_key) else {
                continue;
            };
            let mut properties = Vec::new();
            let mut prop_indices = HashMap::new();
            let mut methods = HashMap::new();

            // Inherit from parent if present
            if let Some(parent_key) = &self.classes.get(&class_key).unwrap().parent {
                if let Some(parent_info) = self.classes.get(parent_key) {
                    properties = parent_info.properties.clone();
                    prop_indices = parent_info.prop_indices.clone();
                    methods = parent_info.methods.clone();
                }
            }

            for member in &c.members {
                match member {
                    ast::ClassMember::Property(p) => {
                        if p.is_static && p.default.as_ref().is_none_or(|e| !static_initializer(e))
                        {
                            self.diags.emit(Diagnostic::error("static properties require a constant default value in this slice")
                                .with_code("E0271").with_span(p.name_span));
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
                        if ty == Ty::Mixed {
                            if let Some(default) = &p.default {
                                if !static_initializer(default) {
                                    self.diags.emit(
                                        Diagnostic::error(
                                            "mixed property defaults require constant expressions",
                                        )
                                        .with_code("E0274")
                                        .with_span(p.name_span),
                                    );
                                } else {
                                    let mut context = FnCtx::new(Ty::Void, false);
                                    if let Some(value) =
                                        self.infer_for_type(default, &mut context, &Ty::Mixed)
                                    {
                                        if value.ty() == Ty::Void {
                                            self.diags.emit(
                                                Diagnostic::error("void cannot be boxed as mixed")
                                                    .with_code("E0274")
                                                    .with_span(p.name_span),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        if ty == Ty::Void {
                            self.diags.emit(
                                Diagnostic::error("a property cannot have type `void`")
                                    .with_code("E0203")
                                    .with_span(p.span),
                            );
                        }

                        if let Some(&existing_idx) = prop_indices.get(&p.name) {
                            let existing = &properties[existing_idx];
                            if existing.declaring_class.eq_ignore_ascii_case(&c.name) {
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
                            if p.is_static != existing.is_static {
                                self.diags.emit(
                                    Diagnostic::error(
                                        "cannot change whether an inherited property is static",
                                    )
                                    .with_code("E0271")
                                    .with_span(p.name_span),
                                );
                            }
                            // Overriding inherited property
                            if existing.visibility == ast::Visibility::Public
                                && p.visibility != ast::Visibility::Public
                            {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "cannot narrow visibility of public property `${}` in class `{}`",
                                        p.name, c.name
                                    ))
                                    .with_code("E0265")
                                    .with_span(p.name_span),
                                );
                            } else if existing.visibility == ast::Visibility::Protected
                                && p.visibility == ast::Visibility::Private
                            {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "cannot narrow visibility of protected property `${}` in class `{}`",
                                        p.name, c.name
                                    ))
                                    .with_code("E0265")
                                    .with_span(p.name_span),
                                );
                            }
                            if existing.ty != ty {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "type of property `{}::${}` (`{ty}`) must match inherited property type `{}`",
                                        c.name, p.name, existing.ty
                                    ))
                                    .with_code("E0208")
                                    .with_span(p.span),
                                );
                            }
                            properties[existing_idx] = PropertyInfo {
                                is_static: p.is_static,
                                declaring_class: c.name.clone(),
                                name: p.name.clone(),
                                name_span: p.name_span,
                                ty,
                                visibility: p.visibility,
                                default: p.default.clone().or_else(|| existing.default.clone()),
                            };
                        } else {
                            let idx = properties.len();
                            prop_indices.insert(p.name.clone(), idx);
                            properties.push(PropertyInfo {
                                is_static: p.is_static,
                                declaring_class: c.name.clone(),
                                name: p.name.clone(),
                                name_span: p.name_span,
                                ty,
                                visibility: p.visibility,
                                default: p.default.clone(),
                            });
                        }
                    }
                    ast::ClassMember::Method(m) => {
                        if c.is_interface && m.visibility != ast::Visibility::Public {
                            self.diags.emit(
                                Diagnostic::error("interface methods must be public")
                                    .with_code("E0270")
                                    .with_span(m.name_span),
                            );
                        }
                        if m.is_abstract && m.visibility == ast::Visibility::Private {
                            self.diags.emit(
                                Diagnostic::error("abstract methods cannot be private")
                                    .with_code("E0269")
                                    .with_span(m.name_span),
                            );
                        }
                        if m.is_static && m.name.eq_ignore_ascii_case("__construct") {
                            self.diags.emit(
                                Diagnostic::error("constructors cannot be static")
                                    .with_code("E0271")
                                    .with_span(m.name_span),
                            );
                        }
                        let mkey = m.name.to_ascii_lowercase();
                        if m.is_async && mkey.starts_with("__") {
                            self.diags.emit(
                                Diagnostic::error(
                                    "constructors and magic methods must be synchronous",
                                )
                                .with_code("E0300")
                                .with_span(m.name_span),
                            );
                        }
                        if !m.type_params.is_empty() && mkey.starts_with("__") {
                            self.diags.emit(
                                Diagnostic::error(
                                    "magic methods and constructors cannot declare type parameters",
                                )
                                .with_code("E0282")
                                .with_span(m.name_span),
                            );
                        }
                        self.validating_template = !m.type_params.is_empty();
                        self.validate_bounds(&m.bounds);
                        let bindings: HashMap<_, _> = m
                            .type_params
                            .iter()
                            .map(|(name, _)| (name.clone(), Ty::Mixed))
                            .collect();
                        let mut params = Vec::new();
                        let mut param_names = Vec::new();
                        for param in &m.params {
                            if param_names.contains(&param.name) {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "duplicate parameter `${}`",
                                        param.name
                                    ))
                                    .with_code("E0201")
                                    .with_span(param.name_span),
                                );
                            }
                            if param.name == "this" {
                                self.diags.emit(
                                    Diagnostic::error("a method parameter cannot replace `$this`")
                                        .with_code("E0267")
                                        .with_span(param.name_span),
                                );
                            }
                            let pty = match &param.ty {
                                Some(t) => self.resolve_type_with(t, &bindings),
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
                                let r = self.resolve_type_with(t, &bindings);
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
                                Some(t) => self.resolve_type_with(t, &bindings),
                                None if m.is_abstract => {
                                    self.diags.emit(Diagnostic::error("abstract/interface methods require a return annotation").with_code("E0204").with_span(m.name_span));
                                    Ty::Infer
                                }
                                None => self
                                    .class_expander
                                    .inferred_return(&mangle_method(&c.name, &m.name)),
                            }
                        };

                        self.validating_template = false;
                        if let Some(existing) = methods.get(&mkey) {
                            if existing.declaring_class.eq_ignore_ascii_case(&c.name) {
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
                            if m.is_static != existing.ast_method.is_static {
                                self.diags.emit(
                                    Diagnostic::error(
                                        "cannot change whether an inherited method is static",
                                    )
                                    .with_code("E0271")
                                    .with_span(m.name_span),
                                );
                            }
                            if m.is_abstract && !existing.ast_method.is_abstract {
                                self.diags.emit(
                                    Diagnostic::error(
                                        "cannot make an inherited concrete method abstract",
                                    )
                                    .with_code("E0269")
                                    .with_span(m.name_span),
                                );
                            }
                            if m.is_async != existing.ast_method.is_async {
                                self.diags.emit(
                                    Diagnostic::error("overrides must preserve the async modifier")
                                        .with_code("E0300")
                                        .with_span(m.name_span),
                                );
                            }
                            // Overriding inherited method
                            if (!m.type_params.is_empty()
                                || !existing.ast_method.type_params.is_empty())
                                && !self.generic_method_compatible(m, &existing.ast_method)
                            {
                                self.diags.emit(Diagnostic::error(format!("generic method `{}` must preserve its inherited type-parameter contract", m.name)).with_code("E0282").with_span(m.name_span));
                            }
                            if existing.visibility == ast::Visibility::Public
                                && m.visibility != ast::Visibility::Public
                            {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "cannot narrow visibility of public method `{}::{}` to protected or private",
                                        existing.declaring_class, m.name
                                    ))
                                    .with_code("E0265")
                                    .with_span(m.name_span),
                                );
                            } else if existing.visibility == ast::Visibility::Protected
                                && m.visibility == ast::Visibility::Private
                            {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "cannot narrow visibility of protected method `{}::{}` to private",
                                        existing.declaring_class, m.name
                                    ))
                                    .with_code("E0265")
                                    .with_span(m.name_span),
                                );
                            }
                            if mkey != "__construct" || existing.ast_method.is_abstract {
                                if params.len() != existing.params.len() {
                                    self.diags.emit(
                                        Diagnostic::error(format!(
                                            "declaration of `{}::{}` must be compatible with `{}::{}` (expected {} argument{}, found {})",
                                            c.name,
                                            m.name,
                                            existing.declaring_class,
                                            m.name,
                                            existing.params.len(),
                                            if existing.params.len() == 1 { "" } else { "s" },
                                            params.len()
                                        ))
                                        .with_code("E0207")
                                        .with_span(m.name_span),
                                    );
                                } else if params != existing.params {
                                    self.diags.emit(
                                        Diagnostic::error(format!(
                                            "parameter types in `{}::{}` must be compatible with `{}::{}`",
                                            c.name, m.name, existing.declaring_class, m.name
                                        ))
                                        .with_code("E0208")
                                        .with_span(m.name_span),
                                    );
                                }
                                if ret != existing.ret
                                    && !self.compatible_return(&ret, &existing.ret)
                                {
                                    let diagnostic = Diagnostic::error(format!(
                                            "return type `{ret}` in `{}::{}` must be compatible with `{}` in `{}::{}`",
                                            c.name, m.name, existing.ret, existing.declaring_class, m.name
                                        ))
                                        .with_code("E0208")
                                        .with_span(m.name_span);
                                    if m.ret.is_none() || existing.ast_method.ret.is_none() {
                                        self.return_contract_errors.push(diagnostic);
                                    } else {
                                        self.diags.emit(diagnostic);
                                    }
                                }
                            }
                        }

                        let mangled = mangle_method(&c.name, &m.name);
                        methods.insert(
                            mkey,
                            MethodInfo {
                                declaring_class: c.name.clone(),
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

            // Check every contract, including interfaces inherited by a parent class.
            // This also validates overrides in descendants against those contracts.
            // Resolve contracts in declaration order, with parent-class contracts
            // first. This preserves PHP's method selection when abstract contracts
            // with covariant return types are inherited from multiple interfaces.
            let mut lineage = vec![class_key.clone()];
            while let Some(parent) = self
                .classes
                .get(lineage.last().unwrap())
                .and_then(|i| i.parent.clone())
            {
                lineage.push(parent);
            }
            lineage.reverse();
            let contracts: Vec<_> = lineage
                .iter()
                .flat_map(|name| self.classes[name].interfaces.iter())
                .filter_map(|name| self.classes.get(name))
                .flat_map(|i| i.methods.iter().map(|(key, m)| (key.clone(), m.clone())))
                .collect();
            for (key, requirement) in contracts {
                if let Some(implementation) = methods.get(&key) {
                    if implementation.ast_method.is_async != requirement.ast_method.is_async {
                        self.diags.emit(
                            Diagnostic::error(
                                "interface implementations must preserve the async modifier",
                            )
                            .with_code("E0300")
                            .with_span(implementation.name_span),
                        );
                    }
                    if (!implementation.ast_method.type_params.is_empty()
                        || !requirement.ast_method.type_params.is_empty())
                        && !self.generic_method_compatible(
                            &implementation.ast_method,
                            &requirement.ast_method,
                        )
                    {
                        self.diags.emit(Diagnostic::error(format!("generic method `{}` is incompatible with its interface type-parameter contract", implementation.name)).with_code("E0282").with_span(implementation.name_span));
                    }
                    if implementation.ast_method.is_static != requirement.ast_method.is_static {
                        self.diags.emit(
                            Diagnostic::error(
                                "interface method static modifier must match its implementation",
                            )
                            .with_code("E0271")
                            .with_span(implementation.name_span),
                        );
                    }
                    if implementation.visibility != ast::Visibility::Public {
                        self.diags.emit(
                            Diagnostic::error(format!(
                                "implementation of `{}::{}` must be public",
                                requirement.declaring_class, requirement.name
                            ))
                            .with_code("E0265")
                            .with_span(implementation.name_span),
                        );
                    }
                    // Parameter types are invariant in this initial typed subset.
                    if implementation.params != requirement.params
                        || (implementation.ret != requirement.ret
                            && !self.compatible_return(&implementation.ret, &requirement.ret))
                    {
                        let diagnostic = Diagnostic::error(format!(
                            "method `{}::{}` is incompatible with interface `{}::{}`",
                            c.name,
                            implementation.name,
                            requirement.declaring_class,
                            requirement.name
                        ))
                        .with_code("E0208")
                        .with_span(c.name_span);
                        if implementation.params == requirement.params
                            && (implementation.ast_method.ret.is_none()
                                || requirement.ast_method.ret.is_none())
                        {
                            self.return_contract_errors.push(diagnostic);
                        } else {
                            self.diags.emit(diagnostic);
                        }
                    }
                } else {
                    methods.insert(key, requirement);
                }
            }

            if !c.is_abstract {
                for method in methods.values().filter(|m| m.ast_method.is_abstract) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "concrete class `{}` must implement abstract method `{}`",
                            c.name, method.name
                        ))
                        .with_code("E0269")
                        .with_span(c.name_span),
                    );
                }
            }
            if let Some(info) = self.classes.get_mut(&class_key) {
                info.properties = properties;
                info.prop_indices = prop_indices;
                info.methods = methods;
            }
        }
        // A class discovered by a later generic instantiation must implement
        // concrete method contracts requested on an earlier pass, including
        // overrides of an inherited specialization and interface requirements.
        let mut requests = Vec::new();
        for (root, name, original, types) in self.class_expander.virtual_method_requests() {
            for class in self.classes.values() {
                if class.name.eq_ignore_ascii_case(&root) || self.is_subclass_of(&class.name, &root)
                {
                    if let Some(implementation) = class.methods.get(&original) {
                        if !implementation.ast_method.type_params.is_empty() {
                            requests.push((implementation.clone(), types.clone(), name.clone()));
                        }
                    }
                }
            }
        }
        for (implementation, types, name) in requests {
            self.class_expander.specialize_method(
                &implementation.declaring_class,
                &implementation.ast_method,
                &types,
                &name,
            );
        }
    }

    fn collect_signatures(&mut self, program: &ast::Program) {
        for item in &program.items {
            let ast::Stmt::FnDecl(f) = item else { continue };
            let key = f.name.to_ascii_lowercase();
            if self.generics.contains_key(&key) || self.funcs.contains_key(&key) {
                let previous_span = self
                    .generics
                    .get(&key)
                    .map(|f| f.span)
                    .or_else(|| self.funcs.get(&key).map(|sig| sig.span))
                    .unwrap();
                self.diags.emit(
                    Diagnostic::error(format!("function `{}` is already defined", f.name))
                        .with_code("E0201")
                        .with_span(f.name_span)
                        .with_note(format!(
                            "previous definition is at byte offset {}",
                            previous_span.start
                        )),
                );
                continue;
            }

            if f.is_async && key == "main" && !f.type_params.is_empty() {
                self.diags.emit(
                    Diagnostic::error("async main cannot be generic")
                        .with_code("E0300")
                        .with_span(f.name_span),
                );
            }
            let bindings: HashMap<_, _> = f
                .type_params
                .iter()
                .map(|(name, _)| (name.clone(), Ty::Mixed))
                .collect();
            self.validating_template = !f.type_params.is_empty();
            self.validate_bounds(&f.bounds);
            let mut params = Vec::new();
            let mut param_names = Vec::new();
            for p in &f.params {
                if param_names.contains(&p.name) {
                    self.diags.emit(
                        Diagnostic::error(format!("duplicate parameter `${}`", p.name))
                            .with_code("E0201")
                            .with_span(p.name_span),
                    );
                }

                let ty = match &p.ty {
                    Some(t) => self.resolve_type_with(t, &bindings),
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
                Some(t) => self.resolve_type_with(t, &bindings),
                None => self.class_expander.inferred_return(&mangle_fn(&f.name)),
            };

            self.validating_template = false;
            if !f.type_params.is_empty() {
                self.generics.insert(key, (**f).clone());
                continue;
            }
            self.funcs.insert(
                key,
                Signature {
                    is_async: f.is_async,
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
        self.resolve_type_with(t, &HashMap::new())
    }

    fn resolve_type_with(&mut self, t: &ast::TypeRef, bindings: &HashMap<String, Ty>) -> Ty {
        match t {
            ast::TypeRef::EmptyElement => Ty::Null,
            ast::TypeRef::Applied(name, args, _)
                if name
                    .rsplit('\\')
                    .next()
                    .unwrap_or(name)
                    .eq_ignore_ascii_case("TaskResult")
                    && args.len() == 1 =>
            {
                Ty::TaskResult(Box::new(self.resolve_type_with(&args[0], bindings)))
            }
            ast::TypeRef::Applied(..) => {
                if self.validating_template {
                    self.class_expander.validate_type(t);
                    return Ty::Mixed;
                }
                let mut concrete = t.clone();
                let bindings = bindings
                    .iter()
                    .map(|(name, ty)| (name.clone(), class_generics::type_ref(ty)))
                    .collect();
                self.class_expander.rewrite_type(&mut concrete, &bindings);
                match concrete {
                    ast::TypeRef::Named(name) => Ty::Named(name),
                    _ => Ty::Mixed,
                }
            }
            ast::TypeRef::Param(name, span) => bindings.get(name).cloned().unwrap_or_else(|| {
                self.diags.emit(
                    Diagnostic::error(format!("unbound generic type parameter `{name}`"))
                        .with_code("E0280")
                        .with_span(*span),
                );
                Ty::Mixed
            }),
            ast::TypeRef::Mixed => Ty::Mixed,
            ast::TypeRef::Int => Ty::Int,
            ast::TypeRef::Float => Ty::Float,
            ast::TypeRef::String => Ty::String,
            ast::TypeRef::Bool => Ty::Bool,
            ast::TypeRef::Void => Ty::Void,
            ast::TypeRef::Array(inner) => {
                let elem = self.resolve_type_with(inner, bindings);
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
        Some(self.analyze_function_with_signature(f, &sig, HashMap::new()))
    }

    fn analyze_function_with_signature(
        &mut self,
        f: &ast::FnDecl,
        sig: &Signature,
        bindings: HashMap<String, Ty>,
    ) -> TFunc {
        let mut declaration = f.clone();
        let substitutions = bindings
            .iter()
            .map(|(name, ty)| (name.clone(), class_generics::type_ref(ty)))
            .collect();
        self.class_expander
            .function(&mut declaration, &substitutions);
        let f = &declaration;
        let mut ctx = FnCtx::new(sig.ret.clone(), false);
        ctx.is_async = f.is_async;
        ctx.infer_return = f.ret.is_none();
        ctx.type_bindings = bindings;
        for (name, ty) in sig.param_names.iter().zip(sig.params.iter()) {
            ctx.declare(name, ty.clone());
        }
        let before_errors = self.diags.error_count();
        let body = self.analyze_stmts(&f.body, &mut ctx);
        ctx.inference_failed = self.diags.error_count() != before_errors;
        self.finish_return_inference(&sig.mangled, &sig.display, f.name_span, &ctx);
        if ctx.flow.reachable {
            self.check_termination(&f.body, &sig, f.name_span);
        }

        TFunc {
            is_async: f.is_async,
            name: sig.mangled.clone(),
            display: sig.display.clone(),
            params: sig
                .param_names
                .iter()
                .cloned()
                .zip(sig.params.iter().cloned())
                .collect(),
            ret: sig.ret.clone(),
            slots: ctx.slots,
            body,
        }
    }

    fn analyze_method(&mut self, class_name: &str, method: &MethodInfo) -> Option<TFunc> {
        let display = self
            .class_expander
            .method_display(class_name, &method.name)
            .map(str::to_owned);
        let Some(display) = display else {
            return self.analyze_method_inner(class_name, method);
        };
        let previous = std::mem::take(&mut self.diags);
        let mut result = self.analyze_method_inner(class_name, method);
        let specialized = std::mem::take(&mut self.diags);
        self.diags = previous;
        for diagnostic in specialized.items() {
            self.diags.emit(
                diagnostic
                    .clone()
                    .with_note(format!("while specializing `{display}`")),
            );
        }
        if let Some(function) = &mut result {
            function.display = display;
        }
        result
    }

    fn analyze_method_inner(&mut self, class_name: &str, method: &MethodInfo) -> Option<TFunc> {
        let mut ctx = FnCtx::new(method.ret.clone(), false);
        ctx.is_async = method.ast_method.is_async;
        ctx.infer_return = method.ast_method.ret.is_none()
            && method.name != "__construct"
            && !method.ast_method.is_abstract;
        ctx.current_class = Some(class_name.to_string());
        if !method.ast_method.is_static {
            ctx.declare("this", Ty::Named(class_name.to_string()));
        }
        for (name, ty) in method.param_names.iter().zip(method.params.iter()) {
            ctx.declare(name, ty.clone());
        }
        // Abstract declarations still need a linkable fallback for dispatchers in
        // hierarchies with no concrete descendants. No valid object can reach it.
        let before_errors = self.diags.error_count();
        let body = if method.ast_method.is_abstract {
            vec![TStmt::Expr(TExpr::Call {
                callee: Callee::User {
                    mangled: "abort".into(),
                    name: "abort".into(),
                    params: vec![],
                },
                ret: Ty::Void,
                args: vec![],
                arg_tys: vec![],
                span: method.name_span,
            })]
        } else {
            self.analyze_stmts(&method.ast_method.body, &mut ctx)
        };
        ctx.inference_failed = self.diags.error_count() != before_errors;
        self.finish_return_inference(
            &method.mangled,
            &format!("{class_name}::{}", method.name),
            method.name_span,
            &ctx,
        );
        if !method.ast_method.is_abstract && method.name != "__construct" && method.ret != Ty::Void
        {
            let sig = Signature {
                is_async: method.ast_method.is_async,
                mangled: method.mangled.clone(),
                display: format!("{class_name}::{}", method.name),
                params: method.params.clone(),
                param_names: method.param_names.clone(),
                ret: method.ret.clone(),
                span: method.name_span,
            };
            if ctx.flow.reachable {
                self.check_termination(&method.ast_method.body, &sig, method.name_span);
            }
        }

        let mut all_params = if method.ast_method.is_static {
            Vec::new()
        } else {
            vec![("this".to_string(), Ty::Named(class_name.to_string()))]
        };
        all_params.extend(
            method
                .param_names
                .iter()
                .cloned()
                .zip(method.params.iter().cloned()),
        );

        Some(TFunc {
            is_async: method.ast_method.is_async,
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
        let mut body = Vec::new();
        let mut classes: Vec<_> = self.classes.values().cloned().collect();
        classes.sort_by_key(|c| c.class_id);
        for class in classes {
            for (index, property) in class.properties.iter().enumerate() {
                if !property.is_static
                    || !property.declaring_class.eq_ignore_ascii_case(&class.name)
                {
                    continue;
                }
                if let Some(default) = &property.default {
                    let value = self.infer_for_type(default, &mut ctx, &property.ty)?;
                    if !self.is_assignable(&value.ty(), &property.ty) {
                        self.diags.emit(
                            Diagnostic::error("static property default has the wrong type")
                                .with_code("E0212")
                                .with_span(property.name_span),
                        );
                        continue;
                    }
                    body.push(TStmt::Assign {
                        target: TTarget::Property {
                            base: Box::new(self.static_storage(&class, property.name_span)),
                            class_name: class.name.clone(),
                            prop_name: property.name.clone(),
                            prop_idx: index as u32,
                            ty: property.ty.clone(),
                        },
                        ty: property.ty.clone(),
                        value,
                    });
                }
            }
        }
        self.worker_bootstrap = body.clone();
        if let Some(entry) = self.funcs.get("main").filter(|f| f.is_async).cloned() {
            if !entry.params.is_empty()
                || (entry.ret != Ty::Void && entry.ret != Ty::Infer)
                || stmts.iter().any(|s| {
                    !matches!(
                        s,
                        ast::Stmt::FnDecl(_) | ast::Stmt::ClassDecl(_) | ast::Stmt::Empty
                    )
                })
            {
                self.diags.emit(Diagnostic::error("async main must have no parameters, return void, and cannot coexist with executable top-level statements").with_code("E0301").with_span(entry.span));
            } else {
                let task_ty = Ty::Task(Box::new(Ty::Void));
                let task = TExpr::Call {
                    callee: Callee::User {
                        mangled: entry.mangled,
                        name: entry.display,
                        params: vec![],
                    },
                    ret: task_ty.clone(),
                    args: vec![],
                    arg_tys: vec![],
                    span: entry.span,
                };
                body.push(TStmt::Expr(TExpr::Call {
                    callee: Callee::User {
                        mangled: "pxr_async_run".into(),
                        name: "async entry".into(),
                        params: vec![task_ty.clone()],
                    },
                    ret: Ty::Void,
                    args: vec![task],
                    arg_tys: vec![task_ty],
                    span: entry.span,
                }));
            }
        } else {
            body.extend(self.analyze_stmts(stmts, &mut ctx));
        }
        Some(TFunc {
            is_async: false,
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
            if !ctx.flow.reachable {
                break;
            }
            if let Some(ts) = self.analyze_stmt(s, ctx) {
                out.push(ts);
            }
        }
        out
    }

    fn analyze_stmt(&mut self, stmt: &ast::Stmt, ctx: &mut FnCtx) -> Option<TStmt> {
        match stmt {
            ast::Stmt::PhpInclude { path, mode, .. } => Some(TStmt::PhpInclude {
                path: path.clone(),
                mode: *mode,
            }),
            ast::Stmt::Require { span, .. } => {
                self.diags.emit(
                    Diagnostic::error(
                        "module dependencies require a file-based, top-level compilation context",
                    )
                    .with_code("E0273")
                    .with_span(*span),
                );
                None
            }
            ast::Stmt::Empty => None,
            ast::Stmt::Block(body, _) => Some(TStmt::Block(self.analyze_stmts(body, ctx))),
            ast::Stmt::Echo(exprs, _) => {
                let mut texprs = Vec::new();
                for e in exprs {
                    if let Some(t) = self.infer(e, ctx) {
                        if matches!(t.ty(), Ty::Void | Ty::Task(_) | Ty::TaskResult(_)) {
                            self.diags.emit(
                                Diagnostic::error(
                                    "echo requires a completed value, not void or a task handle",
                                )
                                .with_code("E0240")
                                .with_span(e.span()),
                            );
                            continue;
                        }
                        if t.ty().is_array() {
                            self.diags.emit(
                                Diagnostic::error(
                                    "cannot echo an array; index it or use `implode()`",
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
                if matches!(t.ty(), Ty::Void | Ty::Task(_) | Ty::TaskResult(_)) {
                    self.diags.emit(
                        Diagnostic::error(
                            "print requires a completed value, not void or a task handle",
                        )
                        .with_code("E0240")
                        .with_span(e.span()),
                    );
                    return None;
                }
                if t.ty().is_array() {
                    self.diags.emit(
                        Diagnostic::error("cannot print an array; index it or use `implode()`")
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
            ast::Stmt::ForwardReturn(value, span) => {
                let ret = ctx.ret.clone();
                let value = self.infer_for_type(value, ctx, &ret)?;
                let actual = value.ty();
                if ctx.infer_return {
                    ctx.return_types.push((actual.clone(), *span));
                } else if !self.is_assignable(&actual, &ret) {
                    self.diags.emit(
                        Diagnostic::error(format!("cannot forward `{actual}` as `{ret}`"))
                            .with_code("E0215")
                            .with_span(*span),
                    );
                    return None;
                }
                ctx.flow.reachable = false;
                Some(if actual == Ty::Void {
                    TStmt::Block(vec![
                        TStmt::Expr(value),
                        TStmt::Return {
                            value: None,
                            ret: Ty::Void,
                        },
                    ])
                } else {
                    TStmt::Return {
                        value: Some(value),
                        ret,
                    }
                })
            }
            ast::Stmt::Return(value, span) => {
                let result = self.analyze_return(value.as_ref(), *span, ctx);
                ctx.flow.reachable = false;
                result
            }
            ast::Stmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                let cond_t = self.infer(cond, ctx)?;
                let incoming = ctx.flow.clone();
                let cond_ty = cond_t.ty();
                self.expect_truthy(&cond_ty, cond.span());
                ctx.flow = incoming.condition(&cond_t, true);
                invalidate_expression_writes(cond, ctx);
                let then_t = self.analyze_stmts(then_branch, ctx);
                let mut paths = vec![ctx.flow.clone()];
                ctx.flow = incoming.condition(&cond_t, false);
                invalidate_expression_writes(cond, ctx);
                let mut pending = ctx.flow.clone();
                let mut elifs_t = Vec::new();
                for (c, b) in elifs {
                    ctx.flow = pending.clone();
                    let ct = self.infer(c, ctx)?;
                    let cty = ct.ty();
                    self.expect_truthy(&cty, c.span());
                    pending = ctx.flow.clone();
                    ctx.flow = pending.condition(&ct, true);
                    invalidate_expression_writes(c, ctx);
                    let bt = self.analyze_stmts(b, ctx);
                    paths.push(ctx.flow.clone());
                    ctx.flow = pending.condition(&ct, false);
                    invalidate_expression_writes(c, ctx);
                    pending = ctx.flow.clone();
                    elifs_t.push((ct, cty, bt));
                }
                ctx.flow = pending;
                let else_t = else_branch.as_ref().map(|b| self.analyze_stmts(b, ctx));
                paths.push(ctx.flow.clone());
                ctx.flow = FlowState::merge(&paths);
                Some(TStmt::If {
                    cond: cond_t,
                    cond_ty,
                    then_branch: then_t,
                    elifs: elifs_t,
                    else_branch: else_t,
                })
            }
            ast::Stmt::While { cond, body, .. } => {
                invalidate_writes(body, ctx);
                invalidate_expression_writes(cond, ctx);
                let cond_t = self.infer(cond, ctx)?;
                let incoming = ctx.flow.clone();
                let cond_ty = cond_t.ty();
                self.expect_truthy(&cond_ty, cond.span());
                ctx.flow = incoming.condition(&cond_t, true);
                invalidate_expression_writes(cond, ctx);
                ctx.loop_depth += 1;
                let body_t = self.analyze_stmts(body, ctx);
                ctx.loop_depth -= 1;
                ctx.flow = incoming;
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
                invalidate_writes(body, ctx);
                invalidate_expression_writes(cond, ctx);
                let incoming = ctx.flow.clone();
                ctx.loop_depth += 1;
                let body_t = self.analyze_stmts(body, ctx);
                ctx.loop_depth -= 1;
                ctx.flow = incoming.clone();
                let cond_t = self.infer(cond, ctx)?;
                let cond_ty = cond_t.ty();
                self.expect_truthy(&cond_ty, cond.span());
                ctx.flow = incoming;
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
            ast::Stmt::Break(level, span) => {
                let result = self.analyze_jump(*level, *span, ctx, false);
                if result.is_some() {
                    ctx.flow.reachable = false;
                }
                result
            }
            ast::Stmt::Continue(level, span) => {
                let result = self.analyze_jump(*level, *span, ctx, true);
                if result.is_some() {
                    ctx.flow.reachable = false;
                }
                result
            }
        }
    }

    fn expect_truthy(&mut self, ty: &Ty, span: Span) {
        if ty.contains_infer() {
            return;
        }
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
        invalidate_writes(body, ctx);
        invalidate_writes(update, ctx);
        for c in cond {
            invalidate_expression_writes(c, ctx);
        }
        let mut incoming = ctx.flow.clone();
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
        incoming.initialized = ctx.flow.initialized.clone();
        if let Some((condition, _)) = &cond_t {
            ctx.flow = incoming.condition(condition, true);
            for c in cond {
                invalidate_expression_writes(c, ctx);
            }
        }
        ctx.loop_depth += 1;
        let body_t = self.analyze_stmts(body, ctx);
        ctx.loop_depth -= 1;
        ctx.flow = if let Some((condition, _)) = &cond_t {
            incoming.condition(condition, true)
        } else {
            incoming.clone()
        };
        invalidate_writes(body, ctx);
        for c in cond {
            invalidate_expression_writes(c, ctx);
        }
        let update_t = self.analyze_stmts(update, ctx);
        ctx.flow = incoming;
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
        if request_global_name(value) || key.is_some_and(request_global_name) {
            self.diags.emit(
                Diagnostic::error("foreach cannot replace a request-global binding")
                    .with_code("E0300")
                    .with_span(value_span),
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
        let before_bindings = ctx.flow.initialized.clone();
        let key_slot = key.map(|name| match ctx.slot_of(name) {
            Some(slot) => {
                if ctx.slot_ty(slot) != Ty::Mixed {
                    self.diags.emit(Diagnostic::error(format!(
                        "foreach key variable `${name}` must use mixed storage for integer/string keys"
                    )).with_code("E0242").with_span(key_span));
                }
                slot
            }
            None => ctx.declare(name, Ty::Mixed),
        });
        let vslot = match ctx.slot_of(value) {
            Some(s) => {
                let existing = ctx.slot_ty(s);
                if !self.is_assignable(&elem, &existing) {
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
        invalidate_writes(body, ctx);
        ctx.flow.forget(vslot);
        if let Some(slot) = key_slot {
            ctx.flow.forget(slot);
        }
        let mut incoming = ctx.flow.clone();
        incoming.initialized = before_bindings;
        ctx.flow.initialized.insert(vslot);
        if let Some(slot) = key_slot {
            ctx.flow.initialized.insert(slot);
        }
        ctx.loop_depth += 1;
        let body_t = self.analyze_stmts(body, ctx);
        ctx.loop_depth -= 1;
        ctx.flow = incoming;
        Some(TStmt::Foreach {
            base: subj,
            base_ty: sty,
            key_slot,
            value_slot: vslot,
            value_ty: ctx.slot_ty(vslot),
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
        for case in cases {
            invalidate_writes(&case.body, ctx);
        }
        let incoming = ctx.flow.clone();
        let mut tcases = Vec::new();
        ctx.switch_depth += 1;
        for c in cases {
            ctx.flow = incoming.clone();
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
        ctx.flow = incoming;
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
        if let ast::LValue::Var { name, .. } = target {
            if request_global_name(name) {
                self.diags.emit(Diagnostic::error("whole request-global replacement is not supported yet; assign array elements instead").with_code("E0300").with_span(span));
                return None;
            }
        }
        let expected = if let Some(annotation) = annotation {
            Some(self.resolve_type_with(annotation, &ctx.type_bindings))
        } else if let ast::LValue::Var { name, .. } = target {
            ctx.slot_of(name).map(|slot| ctx.slot_ty(slot))
        } else {
            Some(match target {
                ast::LValue::Index { base, index, .. } => self
                    .infer_index_target(base, index.as_deref(), span, ctx)?
                    .storage_ty(),
                _ => self.infer_target_read(target, span, ctx)?.0.storage_ty(),
            })
        };
        let val = if let Some(expected) = expected {
            self.infer_for_type(value, ctx, &expected)?
        } else {
            self.infer(value, ctx)?
        };
        let val_ty = val.ty();

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
                let annotated = annotation.map(|t| self.resolve_type_with(t, &ctx.type_bindings));
                let (slot, decl_ty) = match ctx.slot_of(name) {
                    Some(slot) => {
                        let existing = ctx.slot_ty(slot);
                        if existing == Ty::Infer && !val_ty.contains_infer() && annotated.is_none()
                        {
                            ctx.slots[slot as usize] = val_ty.clone();
                            return Some(TStmt::Assign {
                                target: TTarget::Var {
                                    name: name.clone(),
                                    slot,
                                    ty: val_ty.clone(),
                                },
                                ty: val_ty,
                                value: val,
                            });
                        }
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

                if !self.is_assignable(&val_ty, &decl_ty) {
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

                ctx.flow.forget(slot);
                ctx.flow.initialized.insert(slot);
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
                if !self.is_assignable(&val_ty, &elem_ty) {
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
            ast::LValue::StaticProperty {
                class_name, name, ..
            } => {
                let (target, ty) = self.static_property(class_name, name, span, ctx)?;
                if annotation.is_some() || !self.is_assignable(&val_ty, &ty) {
                    self.diags.emit(
                        Diagnostic::error("invalid type in static property assignment")
                            .with_code("E0212")
                            .with_span(span),
                    );
                    return None;
                }
                Some(TStmt::Assign {
                    target,
                    ty,
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
                if prop.is_static {
                    self.diags.emit(
                        Diagnostic::error("static properties require `Class::$property` access")
                            .with_code("E0271")
                            .with_span(*name_span),
                    );
                    return None;
                }
                if !self.check_member_visibility(
                    &prop.declaring_class,
                    prop.visibility,
                    *name_span,
                    ctx,
                ) {
                    return None;
                }

                if !self.is_assignable(&val_ty, &prop.ty) {
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
        if let TTarget::Nested { elem_ty, .. } = target {
            if *elem_ty != Ty::Null {
                return elem_ty.clone();
            }
            fn root(target: &TTarget, depth: usize) -> Option<(u32, usize)> {
                match target {
                    TTarget::Nested { parent, .. } => root(parent, depth + 1),
                    TTarget::Index { base, .. } | TTarget::Push { base, .. } => {
                        if let TExpr::Var { slot, .. } = &**base {
                            Some((*slot, depth + 1))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            fn refine(current: &Ty, depth: usize, value: &Ty) -> Option<Ty> {
                if depth == 0 {
                    return (*current == Ty::Null).then(|| value.clone());
                }
                let inner = match current {
                    Ty::Array(inner) => &**inner,
                    Ty::Null => &Ty::Null,
                    _ => return None,
                };
                refine(inner, depth - 1, value).map(Ty::array)
            }
            if let Some((slot, depth)) = root(target, 0) {
                if let Some(ty) = refine(&ctx.slot_ty(slot), depth, val_ty) {
                    ctx.slots[slot as usize] = ty;
                    return val_ty.clone();
                }
            }
            return elem_ty.clone();
        }
        let (elem, base) = match target {
            TTarget::Nested { .. } => unreachable!(),
            TTarget::Index { elem_ty, base, .. } => (elem_ty.clone(), base),
            TTarget::Push { elem_ty, base } => (elem_ty.clone(), base),
            TTarget::MixedVar { ty, .. }
            | TTarget::Var { ty, .. }
            | TTarget::Property { ty, .. } => return ty.clone(),
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
        let before_rhs = ctx.flow.clone();
        let val = self.infer(value, ctx)?;
        if op == ABinOp::NullCoalesce {
            ctx.flow = FlowState::merge(&[before_rhs, ctx.flow.clone()]);
        }
        let vty = val.ty();
        let result_ty = self.binary_type(op, &tty, &vty, span)?;
        if !self.is_assignable(&result_ty, &ttarget.storage_ty()) {
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
        if let ast::LValue::Var { name, .. } = target {
            if let Some(slot) = ctx.slot_of(name) {
                ctx.flow.types.insert(slot, scalar_mask(&result_ty));
            }
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
        if let ast::LValue::Var { name, .. } = target {
            if let Some(slot) = ctx.slot_of(name) {
                ctx.flow.types.insert(slot, scalar_mask(&tty));
            }
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
                if ctx.flow.reachable && !ctx.flow.initialized.contains(&slot) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "variable `${name}` may be uninitialized on this path"
                        ))
                        .with_code("E0220")
                        .with_span(span),
                    );
                    return None;
                }
                let ty = ctx.slot_ty(slot);
                if ty == Ty::Mixed {
                    if let Some(narrowed) = ctx.flow.narrowed(slot) {
                        return Some((
                            TTarget::MixedVar {
                                name: name.clone(),
                                slot,
                                ty: narrowed.clone(),
                            },
                            narrowed,
                        ));
                    }
                }
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
                if index.is_none() {
                    self.diags.emit(
                        Diagnostic::error("cannot read [] in a compound assignment or increment")
                            .with_code("E0249")
                            .with_span(span),
                    );
                    return None;
                }
                let t = self.infer_index_target(base, index.as_deref(), span, ctx)?;
                let ty = match &t {
                    TTarget::Index { elem_ty, .. } => elem_ty.clone(),
                    TTarget::Push { elem_ty, .. } | TTarget::Nested { elem_ty, .. } => {
                        elem_ty.clone()
                    }
                    TTarget::MixedVar { ty, .. }
                    | TTarget::Var { ty, .. }
                    | TTarget::Property { ty, .. } => ty.clone(),
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
            ast::LValue::StaticProperty {
                class_name, name, ..
            } => self.static_property(class_name, name, span, ctx),
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
                if prop.is_static {
                    self.diags.emit(
                        Diagnostic::error("static properties require `Class::$property` access")
                            .with_code("E0271")
                            .with_span(*name_span),
                    );
                    return None;
                }
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
            let mut parent = match base {
                ast::LValue::Index { base, index, .. } => {
                    self.infer_index_target(base, index.as_deref(), span, ctx)?
                }
                _ => self.infer_target_read(base, span, ctx)?.0,
            };
            if parent.storage_ty() == Ty::Null {
                match &mut parent {
                    TTarget::Index { elem_ty, .. }
                    | TTarget::Push { elem_ty, .. }
                    | TTarget::Nested { elem_ty, .. } => *elem_ty = Ty::array(Ty::Null),
                    _ => {}
                }
            }
            let array_ty = match parent.storage_ty() {
                Ty::Mixed => Ty::array(Ty::Mixed),
                ty @ Ty::Array(_) => ty,
                _ => {
                    self.diags.emit(
                        Diagnostic::error("nested write requires an array-valued parent")
                            .with_code("E0249")
                            .with_span(span),
                    );
                    return None;
                }
            };
            let elem = array_ty.array_elem().unwrap().clone();
            let key = match index {
                None => None,
                Some(index) => {
                    let key = self.infer(index, ctx)?;
                    let ty = key.ty();
                    if !matches!(ty, Ty::Int | Ty::String | Ty::Mixed) {
                        self.diags.emit(
                            Diagnostic::error("array keys must be int or string")
                                .with_code("E0250")
                                .with_span(index.span()),
                        );
                        return None;
                    }
                    Some((Box::new(key), ty))
                }
            };
            return Some(TTarget::Nested {
                parent: Box::new(parent),
                array_ty,
                elem_ty: elem,
                key,
            });
        };
        let base_expr = self.infer(&ast::Expr::Var(name.clone(), span), ctx)?;
        let bty = base_expr.ty();
        if bty == Ty::Mixed {
            let parent = self.infer_target_read(base, span, ctx)?.0;
            let key = match index {
                None => None,
                Some(index) => {
                    let key = self.infer(index, ctx)?;
                    let ty = key.ty();
                    if !matches!(ty, Ty::Int | Ty::String | Ty::Mixed) {
                        self.diags.emit(
                            Diagnostic::error("array keys must be int or string")
                                .with_code("E0250")
                                .with_span(index.span()),
                        );
                        return None;
                    }
                    Some((Box::new(key), ty))
                }
            };
            return Some(TTarget::Nested {
                parent: Box::new(parent),
                array_ty: Ty::array(Ty::Mixed),
                elem_ty: Ty::Mixed,
                key,
            });
        }
        let Ty::Array(elem) = bty.clone() else {
            self.diags.emit(
                Diagnostic::error(format!("cannot index a value of type `{bty}` with `[]`"))
                    .with_code("E0249")
                    .with_span(span),
            );
            return None;
        };
        let elem = (*elem).clone();
        match index {
            None => Some(TTarget::Push {
                base: Box::new(base_expr),
                elem_ty: elem,
            }),
            Some(ie) => {
                let key = self.infer(ie, ctx)?;
                let kty = key.ty();
                if !matches!(kty, Ty::Int | Ty::String | Ty::Mixed) {
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

        if ctx.infer_return {
            let typed = match value {
                Some(value) => Some(self.infer_for_type(value, ctx, &ret)?),
                None => None,
            };
            let ty = typed.as_ref().map(TExpr::ty).unwrap_or(Ty::Void);
            if typed.is_some() && ty == Ty::Void {
                self.diags.emit(
                    Diagnostic::error(
                        "cannot return a void expression; call it before a bare return",
                    )
                    .with_code("E0214")
                    .with_span(span),
                );
                return None;
            }
            ctx.return_types.push((ty, span));
            return Some(TStmt::Return { value: typed, ret });
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
                let t = self.infer_for_type(v, ctx, &ret)?;
                let vt = t.ty();
                if !self.is_assignable(&vt, &ret) {
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
            ast::Expr::Effect {
                statement, postfix, ..
            } => {
                let statement = self.analyze_stmt(statement, ctx)?;
                let ty = match &statement {
                    TStmt::Assign { ty, .. } => ty.clone(),
                    TStmt::AssignOp { target, .. } => target.storage_ty(),
                    TStmt::IncDec { ty, .. } => ty.clone(),
                    _ => unreachable!("effect expressions contain assignments or updates"),
                };
                Some(TExpr::Effect {
                    statement: Box::new(statement),
                    postfix: *postfix,
                    ty,
                })
            }
            ast::Expr::IntLit(v, _) => Some(TExpr::Int(*v)),
            ast::Expr::FloatLit(v, _) => Some(TExpr::Float(*v)),
            ast::Expr::StrLit(s, _) => Some(TExpr::Str(s.clone())),
            ast::Expr::BoolLit(b, _) => Some(TExpr::Bool(*b)),
            ast::Expr::NullLit(_) => Some(TExpr::Null),
            ast::Expr::Paren(inner, _) => self.infer(inner, ctx),
            ast::Expr::Var(name, span) => {
                if let Some(id) = request_global(name) {
                    return Some(TExpr::Call {
                        callee: Callee::User {
                            mangled: "pxr_http_global".into(),
                            name: format!("${name}"),
                            params: vec![Ty::Int],
                        },
                        ret: Ty::array(Ty::Mixed),
                        args: vec![TExpr::Int(id)],
                        arg_tys: vec![Ty::Int],
                        span: *span,
                    });
                }
                if ["_SESSION", "_ENV", "GLOBALS"].contains(&name.as_str()) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "`${name}` is not implemented by the native request runtime"
                        ))
                        .with_code("E0300")
                        .with_span(*span),
                    );
                    return None;
                }
                if name == "this" && ctx.slot_of("this").is_none() {
                    self.diags.emit(
                        Diagnostic::error("using `$this` when not in object context")
                            .with_code("E0268")
                            .with_span(*span),
                    );
                    return None;
                }
                match ctx.slot_of(name) {
                    Some(slot) => {
                        if ctx.flow.reachable && !ctx.flow.initialized.contains(&slot) {
                            self.diags.emit(
                                Diagnostic::error(format!(
                                    "variable `${name}` may be uninitialized on this path"
                                ))
                                .with_code("E0220")
                                .with_span(*span),
                            );
                            return None;
                        }
                        let ty = ctx.slot_ty(slot);
                        let expression = TExpr::Var {
                            name: name.clone(),
                            slot,
                            ty: ty.clone(),
                        };
                        Some(if ty == Ty::Mixed {
                            if let Some(narrowed) = ctx.flow.narrowed(slot) {
                                TExpr::Coerce {
                                    expr: Box::new(expression),
                                    ty: narrowed,
                                }
                            } else {
                                expression
                            }
                        } else {
                            expression
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
                if oty.contains_infer() {
                    return Some(TExpr::Deferred);
                }
                match op {
                    AUnOp::Neg => {
                        if !oty.is_numeric() && oty != Ty::Mixed {
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
                        if oty != Ty::Int && oty != Ty::Mixed {
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
                            operand_ty: oty.clone(),
                            ty: oty,
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
                type_args,
                args,
                span,
                fallback,
            } => {
                if callee == "$pxr_collect" {
                    return Some(TExpr::Call {
                        callee: Callee::User {
                            mangled: "pxr_gc_collect".into(),
                            name: "drop".into(),
                            params: vec![],
                        },
                        ret: Ty::Void,
                        args: vec![],
                        arg_tys: vec![],
                        span: *span,
                    });
                }
                if (callee == "$pxr_pass" || callee == "$pxr_borrow_pass") && args.len() == 1 {
                    return self.infer(&args[0], ctx).map(|e| TExpr::Pass {
                        expr: Box::new(e),
                        borrowed: callee == "$pxr_borrow_pass",
                        span: *span,
                    });
                }
                let name = if self.is_compiled_function(&callee.to_ascii_lowercase()) {
                    callee.as_str()
                } else {
                    fallback.as_deref().unwrap_or(callee)
                };
                if self.php_interop
                    && ![
                        "sleepasync",
                        "tcpexchangeasync",
                        "tcpserveasync",
                        "httpserveasync",
                        "http_response_code",
                        "header",
                        "httprequestbody",
                        "canceltask",
                        "stopserver",
                        "tcpconnectasync",
                        "tcplistenasync",
                        "tcpacceptasync",
                        "tcpreadasync",
                        "tcpwriteasync",
                        "closesocket",
                        "tcpshutdownwrite",
                        "taskresultasync",
                        "resultvalueasync",
                        "resultok",
                        "resulterror",
                        "resultstatus",
                        "parallelasync",
                        "channelasync",
                        "channelpeerasync",
                        "channelcloneasync",
                        "channelsendasync",
                        "channelreceiveasync",
                        "closechannel",
                    ]
                    .iter()
                    .any(|n| name.eq_ignore_ascii_case(n))
                    && !self.is_compiled_function(&callee.to_ascii_lowercase())
                    && fallback
                        .as_ref()
                        .is_some_and(|f| !self.is_compiled_function(&f.to_ascii_lowercase()))
                {
                    let mut expression =
                        self.infer_call(callee, *callee_span, type_args, args, *span, ctx)?;
                    if let TExpr::Call {
                        callee:
                            Callee::Php {
                                fallback: target, ..
                            },
                        ..
                    } = &mut expression
                    {
                        *target = fallback.clone();
                    }
                    return Some(expression);
                }
                self.infer_call(name, *callee_span, type_args, args, *span, ctx)
            }
            ast::Expr::Ternary {
                cond,
                then,
                else_branch,
                span,
            } => self.infer_ternary(cond, then.as_deref(), else_branch, *span, ctx),
            ast::Expr::Index { base, index, span } => {
                self.infer_index(base, index.as_deref(), *span, ctx)
            }
            ast::Expr::ArrayLit { items, span } => self.infer_array_lit(items, *span, ctx, false),
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
                if base_ty.contains_infer() {
                    return Some(TExpr::Deferred);
                }
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
                if prop.is_static {
                    self.diags.emit(
                        Diagnostic::error("static properties require `Class::$property` access")
                            .with_code("E0271")
                            .with_span(*name_span),
                    );
                    return None;
                }
                if !self.check_member_visibility(
                    &prop.declaring_class,
                    prop.visibility,
                    *name_span,
                    ctx,
                ) {
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
                type_args,
                name_span,
                args,
                span,
            } => {
                let base_expr = self.infer(base, ctx)?;
                let base_ty = base_expr.ty();
                if base_ty.contains_infer() {
                    return Some(TExpr::Deferred);
                }
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

                if method.ast_method.is_async && !ctx.is_async {
                    self.diags.emit(
                        Diagnostic::error("async methods require an async context")
                            .with_code("E0300")
                            .with_span(*span),
                    );
                    return None;
                }
                if !self.check_member_visibility(
                    &method.declaring_class,
                    method.visibility,
                    *name_span,
                    ctx,
                ) {
                    return None;
                }

                if !method.ast_method.type_params.is_empty() {
                    if method.ast_method.is_static {
                        self.diags.emit(
                            Diagnostic::error("static methods require class-qualified access")
                                .with_code("E0271")
                                .with_span(*span),
                        );
                        return None;
                    }
                    return self.infer_generic_method_call(
                        &class,
                        &method,
                        Some(base_expr),
                        type_args,
                        args,
                        *span,
                        ctx,
                        true,
                    );
                }
                if !type_args.is_empty() {
                    self.diags.emit(
                        Diagnostic::error("method does not declare type parameters")
                            .with_code("E0280")
                            .with_span(*span),
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

                if method.ast_method.is_static {
                    self.diags.emit(
                        Diagnostic::error(
                            "static methods require `Class::method()` access in this slice",
                        )
                        .with_code("E0271")
                        .with_span(*span),
                    );
                    return None;
                }
                let target_mangled = if self.method_has_overrides(&class.name, &name) {
                    mangle_dispatcher(&class.name, &name)
                } else {
                    method.mangled.clone()
                };

                let mut typed_args = vec![base_expr];
                let mut actual_tys = vec![Ty::Named(class.name.clone())];
                let mut formal_params = vec![Ty::Named(class.name.clone())];
                formal_params.extend(method.params.iter().cloned());

                for (arg, formal_ty) in args.iter().zip(method.params.iter()) {
                    let t_arg = self.infer_for_type(arg, ctx, formal_ty)?;
                    let aty = t_arg.ty();
                    if !self.is_assignable(&aty, formal_ty) {
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
                        mangled: target_mangled,
                        name: format!("{}::{name}", class.name),
                        params: formal_params,
                    },
                    ret: if method.ast_method.is_async {
                        Ty::Task(Box::new(method.ret))
                    } else {
                        method.ret
                    },
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
                ..
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

                if class.is_abstract {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot instantiate abstract class `{}`",
                            class.name
                        ))
                        .with_code("E0269")
                        .with_span(*class_span),
                    );
                    return None;
                }
                let ctor = class.methods.get("__construct").cloned();
                let ctor_info = if let Some(ctor) = &ctor {
                    if !self.check_member_visibility(
                        &ctor.declaring_class,
                        ctor.visibility,
                        *span,
                        ctx,
                    ) {
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
                        let t_arg = self.infer_for_type(arg, ctx, formal_ty)?;
                        let aty = t_arg.ty();
                        if !self.is_assignable(&aty, formal_ty) {
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
                        actual_tys.push(formal_ty.clone());
                        typed_args.push(TExpr::Coerce {
                            expr: Box::new(t_arg),
                            ty: formal_ty.clone(),
                        });
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
                    if p.is_static {
                        continue;
                    }
                    if let Some(def_expr) = &p.default {
                        let mut dummy_ctx = FnCtx::new(Ty::Void, false);
                        if let Some(t_def) = self.infer_for_type(def_expr, &mut dummy_ctx, &p.ty) {
                            if !self.is_assignable(&t_def.ty(), &p.ty) {
                                self.diags.emit(
                                    Diagnostic::error(format!(
                                        "default value of property `{}::${}` is of type `{}`, expected `{}`",
                                        class.name, p.name, t_def.ty(), p.ty
                                    ))
                                    .with_code("E0212")
                                    .with_span(def_expr.span()),
                                );
                            } else {
                                prop_defaults.push((
                                    idx as u32,
                                    TExpr::Coerce {
                                        expr: Box::new(t_def),
                                        ty: p.ty.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }

                Some(TExpr::New {
                    class_id: class.class_id,
                    class_name: class.name.clone(),
                    num_props: class.properties.len() as u32,
                    prop_defaults,
                    ctor: ctor_info,
                    ty: Ty::Named(class.name.clone()),
                })
            }
            ast::Expr::Instanceof {
                expr,
                class_name,
                class_span,
                span: _,
                ..
            } => {
                let t_expr = self.infer(expr, ctx)?;
                let expr_ty = t_expr.ty();
                if !matches!(expr_ty, Ty::Named(_) | Ty::Mixed) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot use `instanceof` on non-object of type `{expr_ty}`"
                        ))
                        .with_code("E0260")
                        .with_span(expr.span()),
                    );
                    return None;
                }
                let target_class = match self.classes.get(&class_name.to_ascii_lowercase()) {
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
                Some(TExpr::Instanceof {
                    expr: Box::new(t_expr),
                    target_class_id: target_class.class_id,
                    target_class_name: target_class.name,
                })
            }
            ast::Expr::ParentMethodCall {
                method,
                type_args,
                args,
                span,
                ..
            } => self.qualified_call("parent", method, type_args, args, *span, ctx),
            ast::Expr::StaticCall {
                class_name,
                method,
                method_type_args,
                args,
                span,
                ..
            } => self.qualified_call(class_name, method, method_type_args, args, *span, ctx),
            ast::Expr::StaticProperty {
                class_name,
                name,
                span,
                ..
            } => {
                let (target, _) = self.static_property(class_name, name, *span, ctx)?;
                let TTarget::Property {
                    base,
                    class_name,
                    prop_name,
                    prop_idx,
                    ty,
                } = target
                else {
                    unreachable!()
                };
                Some(TExpr::Property {
                    base,
                    class_name,
                    prop_name,
                    prop_idx,
                    ty,
                })
            }
        }
    }

    fn qualified_class(&mut self, name: &str, span: Span, ctx: &FnCtx) -> Option<ClassInfo> {
        let key = match name.to_ascii_lowercase().as_str() {
            "static" => {
                self.diags.emit(
                    Diagnostic::error("late static binding (`static::`) is not implemented yet")
                        .with_code("E0271")
                        .with_span(span),
                );
                return None;
            }
            "self" | "parent" => {
                let Some(current) = ctx
                    .current_class
                    .as_ref()
                    .and_then(|n| self.classes.get(&n.to_ascii_lowercase()))
                else {
                    self.diags.emit(
                        Diagnostic::error("`self::` and `parent::` require a class context")
                            .with_code("E0268")
                            .with_span(span),
                    );
                    return None;
                };
                if name.eq_ignore_ascii_case("parent") {
                    let Some(parent) = &current.parent else {
                        self.diags.emit(
                            Diagnostic::error("`parent::` requires a parent class")
                                .with_code("E0268")
                                .with_span(span),
                        );
                        return None;
                    };
                    parent.clone()
                } else {
                    current.name.to_ascii_lowercase()
                }
            }
            _ => name.to_ascii_lowercase(),
        };
        let result = self.classes.get(&key).cloned();
        if result.is_none() {
            self.diags.emit(
                Diagnostic::error(format!("unknown class `{name}`"))
                    .with_code("E0205")
                    .with_span(span),
            );
        }
        result
    }

    fn static_storage(&self, class: &ClassInfo, span: Span) -> TExpr {
        TExpr::Call {
            callee: Callee::User {
                mangled: "pxr_static_storage".into(),
                name: "pxr_static_storage".into(),
                params: vec![Ty::Int, Ty::Int],
            },
            ret: Ty::Named(class.name.clone()),
            args: vec![
                TExpr::Int(class.class_id as i64),
                TExpr::Int(class.properties.len() as i64),
            ],
            arg_tys: vec![Ty::Int, Ty::Int],
            span,
        }
    }

    fn static_property(
        &mut self,
        class_name: &str,
        name: &str,
        span: Span,
        ctx: &FnCtx,
    ) -> Option<(TTarget, Ty)> {
        let class = self.qualified_class(class_name, span, ctx)?;
        let Some(&index) = class.prop_indices.get(name) else {
            self.diags.emit(
                Diagnostic::error(format!("unknown static property `{}::${name}`", class.name))
                    .with_code("E0261")
                    .with_span(span),
            );
            return None;
        };
        let property = &class.properties[index];
        if !property.is_static {
            self.diags.emit(
                Diagnostic::error("instance properties require an object")
                    .with_code("E0271")
                    .with_span(span),
            );
            return None;
        }
        if !self.check_member_visibility(&property.declaring_class, property.visibility, span, ctx)
        {
            return None;
        }
        let owner = &self.classes[&property.declaring_class.to_ascii_lowercase()];
        let owner_index = owner.prop_indices[name];
        Some((
            TTarget::Property {
                base: Box::new(self.static_storage(owner, span)),
                class_name: owner.name.clone(),
                prop_name: name.to_string(),
                prop_idx: owner_index as u32,
                ty: property.ty.clone(),
            },
            property.ty.clone(),
        ))
    }

    fn qualified_call(
        &mut self,
        class_name: &str,
        name: &str,
        type_args: &[ast::TypeRef],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let class = self.qualified_class(class_name, span, ctx)?;
        let Some(method) = class.methods.get(&name.to_ascii_lowercase()).cloned() else {
            self.diags.emit(
                Diagnostic::error(format!("unknown method `{}::{name}`", class.name))
                    .with_code("E0262")
                    .with_span(span),
            );
            return None;
        };
        if method.ast_method.is_async && !ctx.is_async {
            self.diags.emit(
                Diagnostic::error("async methods require an async context")
                    .with_code("E0300")
                    .with_span(span),
            );
            return None;
        }
        if method.ast_method.is_abstract {
            self.diags.emit(
                Diagnostic::error("cannot call an abstract method using class-qualified access")
                    .with_code("E0269")
                    .with_span(span),
            );
            return None;
        }
        if !self.check_member_visibility(&method.declaring_class, method.visibility, span, ctx) {
            return None;
        }
        if method.ast_method.type_params.is_empty() && !type_args.is_empty() {
            self.diags.emit(
                Diagnostic::error("method does not declare type parameters")
                    .with_code("E0280")
                    .with_span(span),
            );
            return None;
        }
        if args.len() != method.params.len() {
            self.diags.emit(
                Diagnostic::error("incorrect argument count in class-qualified call")
                    .with_code("E0207")
                    .with_span(span),
            );
            return None;
        }
        let mut typed = Vec::new();
        let mut actual = Vec::new();
        let mut formal = Vec::new();
        if !method.ast_method.is_static {
            let Some(slot) = ctx.slot_of("this") else {
                self.diags.emit(
                    Diagnostic::error("cannot call an instance method without `$this`")
                        .with_code("E0271")
                        .with_span(span),
                );
                return None;
            };
            let this_ty = ctx.slot_ty(slot);
            if !self.is_assignable(&this_ty, &Ty::Named(class.name.clone())) {
                self.diags.emit(
                    Diagnostic::error(
                        "class-qualified instance call requires a compatible `$this`",
                    )
                    .with_code("E0271")
                    .with_span(span),
                );
                return None;
            }
            typed.push(TExpr::Var {
                name: "this".into(),
                slot,
                ty: this_ty.clone(),
            });
            actual.push(this_ty);
            formal.push(Ty::Named(class.name.clone()));
        }
        if !method.ast_method.type_params.is_empty() {
            return self.infer_generic_method_call(
                &class,
                &method,
                typed.into_iter().next(),
                type_args,
                args,
                span,
                ctx,
                false,
            );
        }
        formal.extend(method.params.clone());
        for (arg, want) in args.iter().zip(&method.params) {
            let value = self.infer(arg, ctx)?;
            let ty = value.ty();
            if !self.is_assignable(&ty, want) {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "expected `{want}`, found `{ty}` in class-qualified call"
                    ))
                    .with_code("E0208")
                    .with_span(arg.span()),
                );
                return None;
            }
            typed.push(value);
            actual.push(ty);
        }
        Some(TExpr::Call {
            callee: Callee::User {
                mangled: method.mangled,
                name: format!("{}::{name}", class.name),
                params: formal,
            },
            ret: if method.ast_method.is_async {
                Ty::Task(Box::new(method.ret))
            } else {
                method.ret
            },
            args: typed,
            arg_tys: actual,
            span,
        })
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
        let incoming = ctx.flow.clone();
        if matches!(op, ABinOp::And | ABinOp::Or) {
            ctx.flow = incoming.condition(&lt, op == ABinOp::And);
            invalidate_expression_writes(lhs, ctx);
        }
        let result = self.infer(rhs, ctx);
        if matches!(op, ABinOp::And | ABinOp::Or | ABinOp::NullCoalesce) {
            ctx.flow = FlowState::merge(&[incoming, ctx.flow.clone()]);
        }
        let rt = result?;
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
        if l.contains_infer() || r.contains_infer() {
            return Some(Ty::Infer);
        }
        if matches!(l, Ty::Task(_) | Ty::TaskResult(_))
            || matches!(r, Ty::Task(_) | Ty::TaskResult(_))
        {
            return self.bad_operands(op, l, r, span);
        }
        if *l == Ty::Mixed || *r == Ty::Mixed {
            if *l == Ty::Void || *r == Ty::Void {
                return self.bad_operands(op, l, r, span);
            }
            match op {
                ABinOp::Add
                | ABinOp::Sub
                | ABinOp::Mul
                | ABinOp::Div
                | ABinOp::Mod
                | ABinOp::BitAnd
                | ABinOp::BitOr
                | ABinOp::BitXor
                | ABinOp::Shl
                | ABinOp::Shr => return Some(Ty::Mixed),
                ABinOp::Eq
                | ABinOp::NotEq
                | ABinOp::Lt
                | ABinOp::LtEq
                | ABinOp::Gt
                | ABinOp::GtEq => return Some(Ty::Bool),
                ABinOp::Spaceship => return Some(Ty::Int),
                _ => {}
            }
        }
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
                    if !matches!(t, Ty::String | Ty::Int | Ty::Float | Ty::Bool | Ty::Mixed) {
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
                let mixed =
                    (*l == Ty::Mixed && (*r != Ty::Void)) || (*r == Ty::Mixed && *l != Ty::Void);
                let ok = l == r && l.is_scalar()
                    || mixed
                    || (l.is_array() && r.is_array())
                    || matches!((l, r), (Ty::Named(_), Ty::Named(_)));
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
        if *a == Ty::Infer {
            return Some(b.clone());
        }
        if *b == Ty::Infer {
            return Some(a.clone());
        }
        if a == b {
            return Some(a.clone());
        }
        match (a, b) {
            (Ty::Null, other) | (other, Ty::Null) => return Some(other.clone()),
            (Ty::Mixed, other) | (other, Ty::Mixed) if *other != Ty::Void => {
                return Some(Ty::Mixed)
            }
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
        let incoming = ctx.flow.clone();
        ctx.flow = incoming.condition(&c, false);
        invalidate_expression_writes(cond, ctx);
        let eb_result = self.infer(else_branch, ctx);
        let else_flow = ctx.flow.clone();
        ctx.flow = incoming.condition(&c, true);
        invalidate_expression_writes(cond, ctx);
        let tt_result = then.map(|t| self.infer(t, ctx));
        ctx.flow = FlowState::merge(&[else_flow, ctx.flow.clone()]);
        let eb = eb_result?;
        match then {
            Some(_) => {
                let tt = tt_result.flatten()?;
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
        let mut b = self.infer(base, ctx)?;
        if b.ty() == Ty::Mixed {
            b = TExpr::Coerce {
                expr: Box::new(b),
                ty: Ty::array(Ty::Mixed),
            };
        }
        let bty = b.ty();
        if bty == Ty::Infer {
            return Some(TExpr::Deferred);
        }
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
        if !matches!(kty, Ty::Int | Ty::String | Ty::Mixed) {
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

    fn infer_for_type(
        &mut self,
        expr: &ast::Expr,
        ctx: &mut FnCtx,
        expected: &Ty,
    ) -> Option<TExpr> {
        if *expected == Ty::Mixed || *expected == Ty::array(Ty::Mixed) {
            match expr {
                ast::Expr::ArrayLit { items, span } => {
                    return self.infer_array_lit(items, *span, ctx, true)
                }
                ast::Expr::Paren(expr, _) => return self.infer_for_type(expr, ctx, expected),
                _ => {}
            }
        }
        self.infer(expr, ctx)
    }

    fn infer_array_lit(
        &mut self,
        items: &[ast::ArrayItem],
        span: Span,
        ctx: &mut FnCtx,
        mixed_elements: bool,
    ) -> Option<TExpr> {
        let _ = span;
        let mut elems = Vec::new();
        let mut ety: Option<Ty> = None;
        for item in items {
            let key = match &item.key {
                Some(k) => {
                    let kt = self.infer(k, ctx)?;
                    let kty = kt.ty();
                    if !matches!(kty, Ty::Int | Ty::String | Ty::Mixed) {
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
            let v = if mixed_elements {
                self.infer_for_type(&item.value, ctx, &Ty::Mixed)?
            } else {
                self.infer(&item.value, ctx)?
            };
            let vt = v.ty();
            if vt == Ty::Void {
                self.diags.emit(
                    Diagnostic::error("array elements cannot be void")
                        .with_code("E0203")
                        .with_span(item.value.span()),
                );
                return None;
            }
            ety = Some(if mixed_elements {
                Ty::Mixed
            } else {
                match &ety {
                    None => vt,
                    Some(e) => self.unify_types(e, &vt, item.value.span())?,
                }
            });
            elems.push((key, v));
        }
        let ety = ety.unwrap_or(if mixed_elements { Ty::Mixed } else { Ty::Null });
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
                if cty != st && !(st == Ty::Mixed && cty != Ty::Void) {
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
        type_args: &[ast::TypeRef],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let key = callee.to_ascii_lowercase();
        if ["http_response_code", "header", "httprequestbody"].contains(&key.as_str()) {
            let (mut params, defaults, minimum, ret, runtime) = match key.as_str() {
                "http_response_code" => (
                    vec![Ty::Int],
                    vec![TExpr::Int(0)],
                    0,
                    Ty::Int,
                    "pxr_http_status",
                ),
                "header" => (
                    vec![Ty::String, Ty::Bool, Ty::Int],
                    vec![TExpr::Bool(true), TExpr::Int(0)],
                    1,
                    Ty::Void,
                    "pxr_http_header",
                ),
                _ => (vec![], vec![], 0, Ty::String, "pxr_http_body"),
            };
            if !type_args.is_empty() || args.len() < minimum || args.len() > params.len() {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "invalid argument count/type arguments for `{callee}`"
                    ))
                    .with_code("E0300")
                    .with_span(span),
                );
                return None;
            }
            let mut values = Vec::new();
            for (arg, expected) in args.iter().zip(&params) {
                let value = self.infer(arg, ctx)?;
                if value.ty() != *expected {
                    return self.builtin_ty_err(
                        callee,
                        &format!("argument of type `{expected}`"),
                        &value.ty(),
                        arg.span(),
                    );
                }
                values.push(value);
            }
            while values.len() < params.len() {
                values.push(defaults[values.len() - minimum].clone());
            }
            if key == "header" {
                values[1] = TExpr::Coerce {
                    expr: Box::new(values[1].clone()),
                    ty: Ty::Int,
                };
                params[1] = Ty::Int;
            }
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: runtime.into(),
                    name: callee.into(),
                    params: params.clone(),
                },
                ret,
                args: values,
                arg_tys: params,
                span,
            });
        }
        if key == "parallelasync" {
            if self.php_interop {
                self.diags.emit(Diagnostic::error("parallel workers cannot be combined with PHP module interop; use native functions").with_code("E0300").with_span(span));
                return None;
            }
            if !ctx.is_async || args.len() != 2 || !type_args.is_empty() {
                self.diags.emit(Diagnostic::error("parallelAsync requires (literal native async function name, Send input) in an async context").with_code("E0300").with_span(span));
                return None;
            }
            let mut name = &args[0];
            while let ast::Expr::Paren(inner, _) = name {
                name = inner;
            }
            let ast::Expr::StrLit(bytes, _) = name else {
                self.diags.emit(
                    Diagnostic::error(
                        "parallelAsync callback must be a literal native async function name",
                    )
                    .with_code("E0300")
                    .with_span(span),
                );
                return None;
            };
            let Some(handler) = std::str::from_utf8(bytes)
                .ok()
                .and_then(|n| {
                    self.funcs
                        .get(&n.trim_start_matches('\\').to_ascii_lowercase())
                })
                .filter(|f| f.is_async && f.params.len() == 1)
                .cloned()
            else {
                self.diags.emit(Diagnostic::error("parallelAsync callback must be a non-generic native async function with one Send parameter").with_code("E0300").with_span(span));
                return None;
            };
            let value = self.infer_for_type(&args[1], ctx, &handler.params[0])?;
            if !self.is_assignable(&value.ty(), &handler.params[0]) {
                return self.builtin_ty_err(callee, "the callback input type", &value.ty(), span);
            }
            self.parallel_handlers.insert(handler.mangled.clone());
            return Some(TExpr::Call {
                callee: Callee::Parallel {
                    handler: handler.mangled,
                },
                ret: Ty::Task(Box::new(handler.ret)),
                args: vec![value],
                arg_tys: handler.params,
                span,
            });
        }
        if [
            "channelasync",
            "channelpeerasync",
            "channelcloneasync",
            "channelsendasync",
            "channelreceiveasync",
            "closechannel",
        ]
        .contains(&key.as_str())
        {
            let channel = Ty::Named("Channel".into());
            let (op, minimum, maximum, ret) = match key.as_str() {
                "channelasync" => (0, 0, 1, Ty::Task(Box::new(channel.clone()))),
                "channelpeerasync" => (1, 1, 1, Ty::Task(Box::new(channel.clone()))),
                "channelcloneasync" => (2, 1, 1, Ty::Task(Box::new(channel.clone()))),
                "channelsendasync" => (3, 2, 3, Ty::Task(Box::new(Ty::Void))),
                "channelreceiveasync" => (4, 1, 2, Ty::Task(Box::new(Ty::Mixed))),
                _ => (5, 1, 1, Ty::Void),
            };
            if (op != 5 && !ctx.is_async)
                || !(minimum..=maximum).contains(&args.len())
                || !type_args.is_empty()
            {
                self.diags.emit(
                    Diagnostic::error(
                        "channel operation requires its declared operands in an async context",
                    )
                    .with_code("E0300")
                    .with_span(span),
                );
                return None;
            }
            let object = if op == 0 {
                TExpr::Null
            } else {
                let object = self.infer(&args[0], ctx)?;
                if object.ty() != channel {
                    return self.builtin_ty_err(callee, "an owned Channel", &object.ty(), span);
                }
                object
            };
            if op == 5 {
                return Some(TExpr::Call {
                    callee: Callee::User {
                        mangled: "pxr_socket_close".into(),
                        name: callee.into(),
                        params: vec![channel.clone()],
                    },
                    ret,
                    args: vec![object],
                    arg_tys: vec![channel],
                    span,
                });
            }
            let value = if op == 3 {
                let v = self.infer_for_type(&args[1], ctx, &Ty::Mixed)?;
                if !send_value_type(&v.ty()) {
                    return self.builtin_ty_err(callee, "Send scalar/array data", &v.ty(), span);
                }
                TExpr::Coerce {
                    expr: Box::new(v),
                    ty: Ty::Mixed,
                }
            } else {
                TExpr::Null
            };
            let number_arg = if op == 0 {
                args.first()
            } else if op == 3 {
                args.get(2)
            } else if op == 4 {
                args.get(1)
            } else {
                None
            };
            let number = if let Some(n) = number_arg {
                let n = self.infer(n, ctx)?;
                if n.ty() != Ty::Int {
                    return self.builtin_ty_err(callee, "int capacity or timeout", &n.ty(), span);
                }
                n
            } else {
                TExpr::Int(if op == 0 { 64 } else { 30000 })
            };
            let values = vec![
                TExpr::Int(op),
                object,
                value,
                number,
                TExpr::Int(self.classes["channel"].class_id as i64),
            ];
            let params = values.iter().map(TExpr::ty).collect::<Vec<_>>();
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: "pxr_async_channel".into(),
                    name: callee.into(),
                    params: params.clone(),
                },
                ret,
                args: values,
                arg_tys: params,
                span,
            });
        }
        if key == "tcpserveasync" || key == "httpserveasync" {
            let http = key == "httpserveasync";
            if !ctx.is_async || !(5..=7).contains(&args.len()) || !type_args.is_empty() {
                self.diags.emit(Diagnostic::error(format!("{callee} requires an async context and (string address, int port, literal string handler, int maxRequests, int maxBytes, optional int timeoutMs, optional int maxConcurrent)")).with_code("E0300").with_span(span));
                return None;
            }
            let mut handler_arg = &args[2];
            while let ast::Expr::Paren(inner, _) = handler_arg {
                handler_arg = inner;
            }
            let ast::Expr::StrLit(handler_name, _) = handler_arg else {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "{callee} handler must be a literal native async function name"
                    ))
                    .with_code("E0300")
                    .with_span(args[2].span()),
                );
                return None;
            };
            let Some(handler) = std::str::from_utf8(handler_name)
                .ok()
                .and_then(|name| {
                    self.funcs
                        .get(&name.strip_prefix('\\').unwrap_or(name).to_ascii_lowercase())
                })
                .filter(|s| {
                    s.is_async
                        && (if http {
                            s.params.is_empty()
                        } else {
                            s.params == [Ty::String]
                        })
                        && (s.ret == Ty::String || (http && s.ret == Ty::Void))
                })
                .cloned()
            else {
                self.diags.emit(Diagnostic::error(if http { "httpServeAsync handler must name a native non-generic async function(): string or void" } else { "tcpServeAsync handler must name a native non-generic async function(string): string" }).with_code("E0300").with_span(args[2].span()));
                return None;
            };
            let mut values = Vec::new();
            let mut params = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                if i == 2 {
                    continue;
                }
                let expected = if i == 0 { Ty::String } else { Ty::Int };
                let value = self.infer(arg, ctx)?;
                if value.ty() != expected {
                    return self.builtin_ty_err(
                        callee,
                        &format!("argument of type `{expected}`"),
                        &value.ty(),
                        arg.span(),
                    );
                }
                values.push(value);
                params.push(expected);
            }
            if args.len() == 5 {
                values.push(TExpr::Int(30000));
                params.push(Ty::Int);
            }
            if args.len() < 7 {
                values.push(TExpr::Int(64));
                params.push(Ty::Int);
            }
            return Some(TExpr::Call {
                callee: Callee::AsyncServer {
                    handler: handler.mangled,
                    http,
                },
                ret: Ty::Task(Box::new(Ty::Void)),
                args: values,
                arg_tys: params,
                span,
            });
        }
        if key == "tcpexchangeasync" {
            if !ctx.is_async || !(4..=5).contains(&args.len()) || !type_args.is_empty() {
                self.diags.emit(Diagnostic::error("tcpExchangeAsync requires an async context and (string address, int port, string request, int maxBytes, optional int timeoutMs)").with_code("E0300").with_span(span));
                return None;
            }
            let params = vec![Ty::String, Ty::Int, Ty::String, Ty::Int, Ty::Int];
            let mut values = Vec::new();
            for (arg, expected) in args.iter().zip(&params) {
                let value = self.infer(arg, ctx)?;
                if value.ty() != *expected {
                    return self.builtin_ty_err(
                        "tcpExchangeAsync",
                        &format!("argument of type `{expected}`"),
                        &value.ty(),
                        arg.span(),
                    );
                }
                values.push(value);
            }
            if values.len() == 4 {
                values.push(TExpr::Int(30000));
            }
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: "pxr_async_tcp_exchange".into(),
                    name: callee.into(),
                    params: params.clone(),
                },
                ret: Ty::Task(Box::new(Ty::String)),
                args: values,
                arg_tys: params,
                span,
            });
        }
        if [
            "taskresultasync",
            "resultvalueasync",
            "resultok",
            "resulterror",
            "resultstatus",
        ]
        .contains(&key.as_str())
        {
            if args.len() != 1 || !type_args.is_empty() || (!ctx.is_async && key.ends_with("async"))
            {
                self.diags.emit(Diagnostic::error("task result operations require one operand; async operations require an async context").with_code("E0300").with_span(span));
                return None;
            }
            let value = self.infer(&args[0], ctx)?;
            let input = value.ty();
            let (runtime, ret) = match (&*key, &input) {
                ("taskresultasync", Ty::Task(result)) => (
                    "pxr_async_result_task",
                    Ty::Task(Box::new(Ty::TaskResult(result.clone()))),
                ),
                ("resultvalueasync", Ty::TaskResult(result)) => {
                    ("pxr_async_result_value_task", Ty::Task(result.clone()))
                }
                ("resultok", Ty::TaskResult(_)) => ("pxr_async_result_ok", Ty::Bool),
                ("resulterror", Ty::TaskResult(_)) => ("pxr_async_result_error", Ty::Int),
                ("resultstatus", Ty::TaskResult(_)) => ("pxr_async_result_status", Ty::Int),
                _ => {
                    return self.builtin_ty_err(
                        callee,
                        "a task or TaskResult operand",
                        &input,
                        span,
                    );
                }
            };
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: runtime.into(),
                    name: callee.into(),
                    params: vec![input.clone()],
                },
                ret,
                args: vec![value],
                arg_tys: vec![input],
                span,
            });
        }
        if [
            "tcpconnectasync",
            "tcplistenasync",
            "tcpacceptasync",
            "tcpreadasync",
            "tcpwriteasync",
            "closesocket",
            "tcpshutdownwrite",
        ]
        .contains(&key.as_str())
        {
            let stream = Ty::Named("TcpStream".into());
            let listener = Ty::Named("TcpListener".into());
            let (params, minimum, ret, runtime, class_name) = match key.as_str() {
                "tcpconnectasync" => (
                    vec![Ty::String, Ty::Int, Ty::Int],
                    2,
                    Ty::Task(Box::new(stream.clone())),
                    "pxr_async_stream_connect",
                    Some("tcpstream"),
                ),
                "tcplistenasync" => (
                    vec![Ty::String, Ty::Int, Ty::Int],
                    2,
                    Ty::Task(Box::new(listener.clone())),
                    "pxr_async_stream_listen",
                    Some("tcplistener"),
                ),
                "tcpacceptasync" => (
                    vec![listener, Ty::Int],
                    1,
                    Ty::Task(Box::new(stream.clone())),
                    "pxr_async_stream_accept",
                    Some("tcpstream"),
                ),
                "tcpreadasync" => (
                    vec![stream, Ty::Int, Ty::Int],
                    2,
                    Ty::Task(Box::new(Ty::String)),
                    "pxr_async_stream_read",
                    None,
                ),
                "tcpwriteasync" => (
                    vec![stream, Ty::String, Ty::Int],
                    2,
                    Ty::Task(Box::new(Ty::Int)),
                    "pxr_async_stream_write",
                    None,
                ),
                "tcpshutdownwrite" => {
                    (vec![stream], 1, Ty::Void, "pxr_socket_shutdown_write", None)
                }
                _ => {
                    if args.len() != 1 || !type_args.is_empty() {
                        self.diags.emit(
                            Diagnostic::error("closeSocket requires one owned socket")
                                .with_code("E0300")
                                .with_span(span),
                        );
                        return None;
                    }
                    let value = self.infer(&args[0], ctx)?;
                    let ty = value.ty();
                    if args.len() != 1
                        || !type_args.is_empty()
                        || !matches!(&ty, Ty::Named(n) if n == "TcpStream" || n == "TcpListener")
                    {
                        self.diags.emit(
                            Diagnostic::error("closeSocket requires an owned native socket")
                                .with_code("E0300")
                                .with_span(span),
                        );
                        return None;
                    }
                    return Some(TExpr::Call {
                        callee: Callee::User {
                            mangled: "pxr_socket_close".into(),
                            name: callee.into(),
                            params: vec![ty.clone()],
                        },
                        ret: Ty::Void,
                        args: vec![value],
                        arg_tys: vec![ty],
                        span,
                    });
                }
            };
            if (key.ends_with("async") && !ctx.is_async)
                || !(minimum..=params.len()).contains(&args.len())
                || !type_args.is_empty()
            {
                self.diags.emit(
                    Diagnostic::error(
                        "socket operations require an async context and their declared operands",
                    )
                    .with_code("E0300")
                    .with_span(span),
                );
                return None;
            }
            let mut values = Vec::new();
            for (argument, expected) in args.iter().zip(&params) {
                let value = self.infer(argument, ctx)?;
                if value.ty() != *expected {
                    return self.builtin_ty_err(
                        callee,
                        "the declared socket operand types",
                        &value.ty(),
                        span,
                    );
                }
                values.push(value);
            }
            if values.len() < params.len() {
                values.push(TExpr::Int(if key == "tcplistenasync" { 64 } else { 30000 }));
            }
            let mut abi = params;
            if let Some(class_name) = class_name {
                values.push(TExpr::Int(self.classes[class_name].class_id as i64));
                abi.push(Ty::Int);
            }
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: runtime.into(),
                    name: callee.into(),
                    params: abi.clone(),
                },
                ret,
                args: values,
                arg_tys: abi,
                span,
            });
        }
        if key == "$pxr_value_drop" && args.len() == 1 {
            let value = self.infer(&args[0], ctx)?;
            let ty = value.ty();
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: key.clone(),
                    name: key.clone(),
                    params: vec![ty.clone()],
                },
                ret: Ty::Void,
                args: vec![value],
                arg_tys: vec![ty],
                span,
            });
        }
        if key == "stopserver" {
            if !ctx.is_async || !(1..=2).contains(&args.len()) || !type_args.is_empty() {
                self.diags.emit(
                    Diagnostic::error("stopServer requires an async context, a server task and optional int grace milliseconds")
                        .with_code("E0300").with_span(span),
                );
                return None;
            }
            let server = self.infer(&args[0], ctx)?;
            let task = Ty::Task(Box::new(Ty::Void));
            if server.ty() != task {
                return self.builtin_ty_err("stopServer", "Task<void> server", &server.ty(), span);
            }
            let grace = if args.len() == 2 {
                self.infer(&args[1], ctx)?
            } else {
                TExpr::Int(30000)
            };
            if grace.ty() != Ty::Int {
                return self.builtin_ty_err(
                    "stopServer",
                    "int grace milliseconds",
                    &grace.ty(),
                    span,
                );
            }
            let params = vec![task, Ty::Int];
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: "pxr_async_stop_server".into(),
                    name: callee.into(),
                    params: params.clone(),
                },
                ret: Ty::Void,
                args: vec![server, grace],
                arg_tys: params,
                span,
            });
        }
        if [
            "$pxr_await",
            "$pxr_spawn",
            "$pxr_task_drop",
            "sleepasync",
            "canceltask",
        ]
        .contains(&key.as_str())
        {
            if !ctx.is_async || args.len() != 1 || !type_args.is_empty() {
                self.diags.emit(
                    Diagnostic::error(
                        "task control and sleepAsync require an async context and one operand",
                    )
                    .with_code("E0300")
                    .with_span(span),
                );
                return None;
            }
            let value = self.infer(&args[0], ctx)?;
            let input = value.ty();
            let (mangled, ret) = if key == "sleepasync" {
                if input != Ty::Int {
                    return self.builtin_ty_err("sleepAsync", "int milliseconds", &input, span);
                }
                ("pxr_async_sleep_task", Ty::Task(Box::new(Ty::Void)))
            } else if let Ty::Task(result) = &input {
                if key == "canceltask" {
                    ("pxr_async_cancel", Ty::Void)
                } else if key == "$pxr_task_drop" {
                    ("pxr_async_drop", Ty::Void)
                } else if key == "$pxr_await" {
                    ("$pxr_await", (**result).clone())
                } else {
                    ("pxr_async_spawn", input.clone())
                }
            } else {
                self.diags.emit(
                    Diagnostic::error(format!("task control requires a Task, found `{input}`"))
                        .with_code("E0300")
                        .with_span(span),
                );
                return None;
            };
            return Some(TExpr::Call {
                callee: Callee::User {
                    mangled: mangled.into(),
                    name: callee.into(),
                    params: vec![input.clone()],
                },
                ret,
                args: vec![value],
                arg_tys: vec![input],
                span,
            });
        }

        if let Some(declaration) = self.generics.get(&key).cloned() {
            return self.infer_generic_call(&declaration, type_args, args, span, ctx);
        }
        if !type_args.is_empty() {
            self.diags.emit(
                Diagnostic::error(format!(
                    "`{callee}` does not declare generic type parameters"
                ))
                .with_code("E0280")
                .with_span(callee_span),
            );
            return None;
        }

        // User-defined function?
        if let Some(sig) = self.funcs.get(&key).cloned() {
            if sig.is_async && !ctx.is_async {
                self.diags.emit(
                    Diagnostic::error("async calls require an async function context")
                        .with_code("E0300")
                        .with_span(span),
                );
                return None;
            }
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
                match self.infer_for_type(a, ctx, pty) {
                    Some(t) => {
                        let at = t.ty();
                        if !self.is_assignable(&at, pty) {
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
                ret: if sig.is_async {
                    Ty::Task(Box::new(sig.ret))
                } else {
                    sig.ret
                },
                args: targs,
                arg_tys,
                span,
            });
        }

        // Builtin?
        if let Some(kind) = Builtin::from_name(&key) {
            return self.infer_builtin(kind, callee, callee_span, args, span, ctx);
        }

        if self.php_interop {
            let mut values = Vec::new();
            for argument in args {
                let value = self.infer_for_type(argument, ctx, &Ty::Mixed)?;
                if value.ty() == Ty::Void {
                    return self.builtin_ty_err(callee, "a value", &Ty::Void, argument.span());
                }
                values.push(value);
            }
            let arg_tys = values.iter().map(TExpr::ty).collect();
            return Some(TExpr::Call {
                callee: Callee::Php {
                    name: callee.to_string(),
                    fallback: None,
                },
                ret: Ty::Mixed,
                args: values,
                arg_tys,
                span,
            });
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
        if let Some(task) = atys
            .iter()
            .find(|t| matches!(t, Ty::Task(_) | Ty::TaskResult(_)))
        {
            return self.builtin_ty_err(
                name,
                "an ordinary value rather than a task handle",
                task,
                span,
            );
        }
        let n = args.len();

        let ret = match kind {
            Builtin::IsInt
            | Builtin::IsFloat
            | Builtin::IsBool
            | Builtin::IsString
            | Builtin::IsArray
            | Builtin::IsObject
            | Builtin::IsNull
            | Builtin::IsScalar
            | Builtin::Boolval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] == Ty::Void {
                    return self.builtin_ty_err(name, "a value", &atys[0], span);
                }
                Ty::Bool
            }

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
                if atys[0] == Ty::Void {
                    return self.builtin_ty_err(name, "a value", &atys[0], args[0].span());
                }
                Ty::Int
            }
            Builtin::Floatval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] == Ty::Void {
                    return self.builtin_ty_err(name, "a value", &atys[0], args[0].span());
                }
                Ty::Float
            }
            Builtin::Strval => {
                if n != 1 {
                    return self.builtin_arity_err(name, 1, 1, n, span);
                }
                if atys[0] == Ty::Void {
                    return self.builtin_ty_err(name, "a value", &atys[0], args[0].span());
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
                let mixed_elements = atys.iter().any(|ty| ty.array_elem() == Some(&Ty::Mixed));
                let mut elem: Option<Ty> = None;
                for (i, t) in atys.iter().enumerate() {
                    if !t.is_array() {
                        return self.builtin_ty_err(name, "an array", t, args[i].span());
                    }
                    if mixed_elements {
                        elem = Some(Ty::Mixed);
                        continue;
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
                if !self.is_assignable(&atys[2], &elem) {
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
                if !matches!(atys[0], Ty::Int | Ty::String | Ty::Mixed) {
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
        Ty::Bool | Ty::Int | Ty::Float | Ty::String | Ty::Mixed | Ty::Named(_) | Ty::Array(_)
    )
}

/// Types that may be compared with `==` / `!=` (and `switch` case matching).
pub(crate) fn request_global_name(name: &str) -> bool {
    request_global(name).is_some() || ["_SESSION", "_ENV", "GLOBALS"].contains(&name)
}
pub(crate) fn request_global(name: &str) -> Option<i64> {
    ["_GET", "_POST", "_COOKIE", "_SERVER", "_REQUEST", "_FILES"]
        .iter()
        .position(|n| *n == name)
        .map(|id| id as i64)
}

fn comparable_eq(a: &Ty, b: &Ty) -> bool {
    if (*a == Ty::Mixed || *b == Ty::Mixed) && *a != Ty::Void && *b != Ty::Void {
        return true;
    }
    (a.is_numeric() && b.is_numeric())
        || (a == b && matches!(a, Ty::String | Ty::Bool | Ty::Named(_)))
}

/// Does this statement provably end with a `return` on all paths?
fn stmt_terminates(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Return(..) | ast::Stmt::ForwardReturn(..) => true,
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
    format!("pxr$fn${}", name.to_ascii_lowercase().replace('\\', "$"))
}

pub fn mangle_method(class: &str, method: &str) -> String {
    format!(
        "pxr$m${}${}",
        mangle_class_name(class),
        method.to_ascii_lowercase()
    )
}

pub fn mangle_dispatcher(class: &str, method: &str) -> String {
    format!(
        "pxr$disp${}${}",
        mangle_class_name(class),
        method.to_ascii_lowercase()
    )
}

fn mangle_class_name(class: &str) -> String {
    if class.contains('<') {
        format!(
            "$generic${}",
            generics::hex_name(&class.to_ascii_lowercase())
        )
    } else {
        class.to_ascii_lowercase().replace('\\', "$")
    }
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

// Static defaults are restricted to scalar/array constant expressions. Reject
// calls, variable reads, and member access rather than running them at startup.
fn static_initializer(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::IntLit(..)
        | ast::Expr::FloatLit(..)
        | ast::Expr::StrLit(..)
        | ast::Expr::BoolLit(..)
        | ast::Expr::NullLit(..) => true,
        ast::Expr::Paren(e, _) | ast::Expr::Unary { expr: e, .. } => static_initializer(e),
        ast::Expr::Binary { lhs, rhs, .. } => static_initializer(lhs) && static_initializer(rhs),
        ast::Expr::ArrayLit { items, .. } => items.iter().all(|item| {
            item.key.as_ref().is_none_or(static_initializer) && static_initializer(&item.value)
        }),
        _ => false,
    }
}

// Invalidate values written on any loop/fallthrough path before typing repeated
// code. Function calls cannot rebind locals: references are not part of PXR yet.
fn invalidate_expression_writes(expression: &ast::Expr, ctx: &mut FnCtx) {
    if let ast::Expr::Effect { statement, .. } = expression {
        invalidate_writes(std::slice::from_ref(statement), ctx);
    } else {
        for child in expression.children() {
            invalidate_expression_writes(child, ctx);
        }
    }
}

fn invalidate_writes(statements: &[ast::Stmt], ctx: &mut FnCtx) {
    for statement in statements {
        for expression in statement.expressions() {
            invalidate_expression_writes(expression, ctx);
        }
        match statement {
            ast::Stmt::Assign { target, .. }
            | ast::Stmt::AssignOp { target, .. }
            | ast::Stmt::IncDec { target, .. } => {
                if let Some(slot) = ctx.slot_of(target.root_name()) {
                    ctx.flow.forget(slot);
                }
            }
            ast::Stmt::Block(body, _)
            | ast::Stmt::While { body, .. }
            | ast::Stmt::DoWhile { body, .. } => invalidate_writes(body, ctx),
            ast::Stmt::If {
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                invalidate_writes(then_branch, ctx);
                for (_, body) in elifs {
                    invalidate_writes(body, ctx);
                }
                if let Some(body) = else_branch {
                    invalidate_writes(body, ctx);
                }
            }
            ast::Stmt::For {
                init, body, update, ..
            } => {
                invalidate_writes(init, ctx);
                invalidate_writes(body, ctx);
                invalidate_writes(update, ctx);
            }
            ast::Stmt::Foreach {
                key, value, body, ..
            } => {
                if let Some(slot) = key.as_ref().and_then(|key| ctx.slot_of(key)) {
                    ctx.flow.forget(slot);
                }
                if let Some(slot) = ctx.slot_of(value) {
                    ctx.flow.forget(slot);
                }
                invalidate_writes(body, ctx);
            }
            ast::Stmt::Switch { cases, .. } => {
                for case in cases {
                    invalidate_writes(&case.body, ctx);
                }
            }
            _ => {}
        }
    }
}

fn send_value_type(ty: &Ty) -> bool {
    match ty {
        Ty::Null | Ty::Bool | Ty::Int | Ty::Float | Ty::String | Ty::Mixed => true,
        Ty::Array(t) => send_value_type(t),
        _ => false,
    }
}
