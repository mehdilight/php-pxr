//! Value types for the PXR compiler.
//!
//! These types are shared by the semantic analyzer, the IR and the code
//! generator so that every stage agrees on the same representation.
//!
//! PXR starts from a small, *statically typed* core that mirrors the
//! scalar types of PHP. Opt-in `mixed` values use tagged value boxes.

use std::fmt;

/// A PXR value type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    /// No value; only valid as a function return type.
    Void,
    Bool,
    Int,
    Float,
    String,
    /// Opt-in tagged PHP value (null, scalar, array, or object).
    Mixed,
    /// Internal element-type sentinel for empty array literals.
    /// User-facing null literals have type `Mixed`.
    Null,
    /// An array with a statically-known value type and dynamically-typed
    /// (`int` or `string`) keys, mirroring PHP's ordered maps. `Array(Null)` is
    /// the element type of an empty literal `[]` before it is refined.
    Array(Box<Ty>),
    /// A named type (class, struct or future generic instantiation).
    Named(String),
}

impl Ty {
    /// Construct `array<elem>`.
    pub fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Bool | Ty::Int | Ty::Float | Ty::String)
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Ty::Array(_))
    }

    /// The value type of an array type, if this is an array.
    pub fn array_elem(&self) -> Option<&Ty> {
        match self {
            Ty::Array(e) => Some(e),
            _ => None,
        }
    }

    /// Can a value of `self` be implicitly used where `to` is expected?
    ///
    /// This captures the only implicit coercion in the PXR core: a lossless
    /// widening from `int` to `float`, plus boxing into mixed destinations.
    /// Arrays otherwise retain their element types.
    pub fn assignable_to(&self, to: &Ty) -> bool {
        if self == to {
            return true;
        }
        match (self, to) {
            (from, Ty::Mixed) if *from != Ty::Void => true,
            (Ty::Int, Ty::Float) => true,
            // An empty array literal (element type `null`) is assignable to any
            // array type; the element type is refined at the assignment site.
            (Ty::Array(e), Ty::Array(t)) => matches!(**e, Ty::Null) || e == t || **t == Ty::Mixed,
            _ => false,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Ty::Void => "void",
            Ty::Bool => "bool",
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::String => "string",
            Ty::Mixed => "mixed",
            Ty::Null => "null",
            Ty::Array(_) => "array",
            Ty::Named(n) => n.as_str(),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Array(e) => write!(f, "array<{e}>"),
            other => f.write_str(other.name()),
        }
    }
}
