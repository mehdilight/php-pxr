//! Value types for the PXR compiler.
//!
//! These types are shared by the semantic analyzer, the IR and the code
//! generator so that every stage agrees on the same representation.
//!
//! PXR starts from a small, *statically typed* core that mirrors the
//! scalar types of PHP. Dynamic / `mixed` values are a deliberate roadmap
//! item rather than something the current milestone pretends to support.

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
    /// The type of the `null` literal. Currently not fully implemented.
    Null,
    /// A named type (class, struct or future generic instantiation).
    Named(String),
}

impl Ty {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float)
    }

    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Bool | Ty::Int | Ty::Float | Ty::String)
    }

    /// Can a value of `self` be implicitly used where `to` is expected?
    ///
    /// This captures the only implicit coercion in the PXR core: a lossless
    /// widening from `int` to `float`.
    pub fn assignable_to(&self, to: &Ty) -> bool {
        if self == to {
            return true;
        }
        matches!((self, to), (Ty::Int, Ty::Float))
    }

    pub fn name(&self) -> &str {
        match self {
            Ty::Void => "void",
            Ty::Bool => "bool",
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::String => "string",
            Ty::Null => "null",
            Ty::Named(n) => n.as_str(),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
