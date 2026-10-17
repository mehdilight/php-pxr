//! Value types for the PXR compiler.
//!
//! These types are shared by the semantic analyzer, the IR and the code
//! generator so that every stage agrees on the same representation.
//!
//! PXR starts from a small, *statically typed* core that mirrors the
//! scalar types of PHP. Opt-in `mixed` values use tagged value boxes.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UnionMember {
    Value(Ty),
    Null,
    False,
    True,
}

/// A PXR value type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    /// Internal bottom type while return inference reaches a fixed point.
    Infer,
    /// No value; only valid as a function return type.
    Void,
    Bool,
    Int,
    Float,
    String,
    /// Opt-in tagged PHP value (null, scalar, array, or object).
    Mixed,
    /// A checked tagged value, sharing mixed's GC-safe representation.
    Union(Vec<UnionMember>),
    /// Internal element-type sentinel for empty array literals.
    /// User-facing null literals have type `Mixed`.
    Null,
    /// An array with a statically-known value type and dynamically-typed
    /// (`int` or `string`) keys, mirroring PHP's ordered maps. `Array(Null)` is
    /// the element type of an empty literal `[]` before it is refined.
    Array(Box<Ty>),
    /// A named type (class, struct or future generic instantiation).
    Named(String),
    /// A single-consumer structured asynchronous computation.
    Task(Box<Ty>),
    /// Owned, single-extraction outcome of a structured task.
    TaskResult(Box<Ty>),
}

impl Ty {
    pub fn union(members: Vec<UnionMember>) -> Ty {
        let mut flat = Vec::new();
        for member in members {
            match member {
                UnionMember::Value(Ty::Union(nested)) => flat.extend(nested),
                other => flat.push(other),
            }
        }
        if flat.contains(&UnionMember::Value(Ty::Bool))
            || (flat.contains(&UnionMember::False) && flat.contains(&UnionMember::True))
        {
            flat.retain(|m| !matches!(m, UnionMember::False | UnionMember::True));
            flat.push(UnionMember::Value(Ty::Bool));
        }
        flat.sort_by_key(|m| Ty::Union(vec![m.clone()]).to_string());
        flat.dedup();
        Ty::Union(flat)
    }
    /// Compiler-generated runtime membership descriptor. None for affine/native-only types.
    pub fn runtime_descriptor(&self, class_id: &impl Fn(&str) -> Option<u32>) -> Option<String> {
        Some(match self {
            Ty::Mixed => "m".into(),
            Ty::Int => "i".into(),
            Ty::Float => "d".into(),
            Ty::Bool => "b".into(),
            Ty::String => "s".into(),
            Ty::Array(element) => format!("[{}]", element.runtime_descriptor(class_id)?),
            Ty::Named(name) => format!("O{};", class_id(name)?),
            Ty::Union(members) => format!(
                "({})",
                members
                    .iter()
                    .map(|m| match m {
                        UnionMember::Value(t) => t.runtime_descriptor(class_id),
                        UnionMember::Null => Some("n".into()),
                        UnionMember::False => Some("f".into()),
                        UnionMember::True => Some("t".into()),
                    })
                    .collect::<Option<Vec<_>>>()?
                    .join("|")
            ),
            _ => return None,
        })
    }
    pub fn contains_infer(&self) -> bool {
        matches!(self, Ty::Infer)
            || matches!(self, Ty::Array(element) | Ty::Task(element) | Ty::TaskResult(element) if element.contains_infer())
            || matches!(self, Ty::Union(members) if members.iter().any(|m| matches!(m, UnionMember::Value(t) if t.contains_infer())))
    }
    pub fn is_boxed(&self) -> bool {
        matches!(self, Self::Mixed | Self::Union(_))
    }
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
        if let Ty::Union(targets) = to {
            let accepts = |m: &UnionMember| {
                targets.iter().any(|target| match (m, target) {
                    (UnionMember::Value(a), UnionMember::Value(b)) => a.assignable_to(b),
                    (UnionMember::False | UnionMember::True, UnionMember::Value(Ty::Bool)) => true,
                    _ => m == target,
                })
            };
            return match self {
                Ty::Mixed => true,
                Ty::Union(members) => members.iter().all(accepts),
                Ty::Bool
                    if targets.contains(&UnionMember::False)
                        || targets.contains(&UnionMember::True) =>
                {
                    true
                }
                other => accepts(&UnionMember::Value(other.clone())),
            };
        }
        if let Ty::Union(members) = self {
            if *to == Ty::Mixed {
                return true;
            }
            return members.iter().all(|m| match m {
                UnionMember::Value(t) => t.assignable_to(to),
                UnionMember::False | UnionMember::True => *to == Ty::Bool,
                _ => false,
            });
        }
        match (self, to) {
            (from, Ty::Mixed)
                if *from != Ty::Void && !matches!(from, Ty::Task(_) | Ty::TaskResult(_)) =>
            {
                true
            }
            (Ty::Int, Ty::Float) => true,
            // An empty array literal (element type `null`) is assignable to any
            // array type; the element type is refined at the assignment site.
            (Ty::Array(e), Ty::Array(t)) => {
                matches!(**e, Ty::Null)
                    || e == t
                    || **t == Ty::Mixed
                    || matches!(**t, Ty::Union(_)) && e.assignable_to(t)
            }
            _ => false,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Ty::Infer => "<unresolved return>",
            Ty::Void => "void",
            Ty::Bool => "bool",
            Ty::Int => "int",
            Ty::Float => "float",
            Ty::String => "string",
            Ty::Mixed => "mixed",
            Ty::Union(_) => "union",
            Ty::Null => "null",
            Ty::Array(_) => "array",
            Ty::Named(n) => n.as_str(),
            Ty::Task(_) => "Task",
            Ty::TaskResult(_) => "TaskResult",
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Array(e) => write!(f, "array<{e}>"),
            Ty::Task(e) => write!(f, "Task<{e}>"),
            Ty::TaskResult(e) => write!(f, "TaskResult<{e}>"),
            Ty::Union(members) => {
                for (i, member) in members.iter().enumerate() {
                    if i != 0 {
                        f.write_str("|")?;
                    }
                    match member {
                        UnionMember::Value(t) => write!(f, "{t}")?,
                        UnionMember::Null => f.write_str("null")?,
                        UnionMember::False => f.write_str("false")?,
                        UnionMember::True => f.write_str("true")?,
                    }
                }
                Ok(())
            }
            other => f.write_str(other.name()),
        }
    }
}
