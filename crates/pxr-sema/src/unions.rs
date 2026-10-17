use super::*;
use pxr_types::UnionMember as M;

pub(super) fn has_parameter(ty: &ast::TypeRef) -> bool {
    match ty {
        ast::TypeRef::Param(..) => true,
        ast::TypeRef::Union(ts, _) | ast::TypeRef::Applied(_, ts, _) => {
            ts.iter().any(has_parameter)
        }
        ast::TypeRef::Array(t) => has_parameter(t),
        _ => false,
    }
}

pub(super) fn raw_union_error(members: &[ast::TypeRef]) -> Option<&'static str> {
    use ast::TypeRef as R;
    let mut seen = std::collections::HashSet::new();
    if members
        .iter()
        .any(|t| !seen.insert(ownership::type_shape(t)))
    {
        return Some("duplicate union member");
    }
    let has = |predicate: fn(&R) -> bool| members.iter().any(predicate);
    let bool_ = has(|t| matches!(t, R::Bool));
    let false_ = has(|t| matches!(t, R::LiteralFalse));
    let true_ = has(|t| matches!(t, R::LiteralTrue));
    if (bool_ && (false_ || true_)) || (false_ && true_) {
        return Some("redundant boolean union: use bool");
    }
    if has(|t| matches!(t, R::Mixed | R::Void)) {
        return Some("mixed and void cannot be union members");
    }
    None
}

impl Analyzer {
    pub(super) fn resolve_union(
        &mut self,
        members: &[ast::TypeRef],
        span: Span,
        bindings: &HashMap<String, Ty>,
    ) -> Ty {
        if let Some(message) = raw_union_error(members) {
            self.diags.emit(
                Diagnostic::error(message)
                    .with_code("E0310")
                    .with_span(span),
            );
            return Ty::Mixed;
        }
        let substituted = members.iter().any(has_parameter);
        let mut result = Vec::new();
        let mut deferred = false;
        for member in members {
            let resolved = self.resolve_type_with(member, bindings);
            let parts = match resolved {
                Ty::Union(parts) => parts,
                Ty::Mixed if self.validating_template && !matches!(member, ast::TypeRef::Mixed) => {
                    deferred = true;
                    vec![]
                }
                Ty::Mixed | Ty::Void | Ty::Task(_) | Ty::TaskResult(_) => {
                    self.diags.emit(
                        Diagnostic::error(
                            "mixed, void and affine task/result types cannot be union members",
                        )
                        .with_code("E0310")
                        .with_span(span),
                    );
                    return Ty::Mixed;
                }
                value if unsafe_union_value(&value) => {
                    self.diags.emit(
                        Diagnostic::error(
                            "affine native resources cannot enter tagged union storage",
                        )
                        .with_code("E0310")
                        .with_span(span),
                    );
                    return Ty::Mixed;
                }
                value => vec![M::Value(value)],
            };
            for part in parts {
                if result.contains(&part) && !substituted {
                    self.diags.emit(
                        Diagnostic::error("duplicate union member")
                            .with_code("E0310")
                            .with_span(span),
                    );
                } else if !result.contains(&part) {
                    result.push(part);
                }
            }
        }
        if !substituted
            && ((result.contains(&M::False) && result.contains(&M::True))
                || (result.contains(&M::Value(Ty::Bool))
                    && (result.contains(&M::False) || result.contains(&M::True))))
        {
            self.diags.emit(
                Diagnostic::error(
                    "redundant boolean union: use bool instead of true|false or bool|true/false",
                )
                .with_code("E0310")
                .with_span(span),
            );
        }
        result.sort_by_key(|m| match m {
            M::Value(t) => t.to_string(),
            M::Null => "null".into(),
            M::False => "false".into(),
            M::True => "true".into(),
        });
        if deferred {
            Ty::Mixed
        } else {
            Ty::union(result)
        }
    }

    pub(super) fn union_accepts(&self, from: &Ty, members: &[M]) -> bool {
        match from {
            Ty::Mixed => true, // checked at the destination, never unchecked unboxing
            Ty::Union(parts) => parts.iter().all(|part| {
                members.iter().any(|target| match (part, target) {
                    (M::Value(a), M::Value(b)) => self.is_assignable(a, b),
                    (M::False | M::True, M::Value(Ty::Bool)) => true,
                    _ => part == target,
                })
            }),
            value => members.iter().any(|m| match m {
                M::Value(target) => self.is_assignable(value, target),
                M::False | M::True => *value == Ty::Bool,
                M::Null => false,
            }),
        }
    }
}

pub(super) fn union_mask(ty: &Ty) -> u8 {
    match ty {
        Ty::Int => 1,
        Ty::Float => 2,
        Ty::Bool => 4,
        Ty::String => 8,
        Ty::Array(_) => 32,
        Ty::Named(_) => 64,
        Ty::Union(parts) => parts.iter().fold(0, |mask, m| {
            mask | match m {
                M::Value(t) => union_mask(t),
                M::Null => 16,
                M::False | M::True => 4,
            }
        }),
        _ => VALUE_MASK,
    }
}

pub(super) fn narrowed_union(ty: &Ty, mask: u8) -> Option<Ty> {
    if mask == VALUE_MASK {
        return None;
    }
    let Ty::Union(members) = ty else {
        return None;
    };
    let remaining: Vec<_> = members
        .iter()
        .filter(|m| union_mask(&Ty::Union(vec![(*m).clone()])) & mask != 0)
        .collect();
    if remaining.len() != 1 {
        return None;
    }
    match remaining[0] {
        M::Value(t) => Some(t.clone()),
        M::False | M::True => Some(Ty::Bool),
        _ => None,
    }
}

fn unsafe_union_value(ty: &Ty) -> bool {
    match ty {
        Ty::Task(_) | Ty::TaskResult(_) => true,
        Ty::Named(name) => ["TcpStream", "TcpListener", "Channel"]
            .iter()
            .any(|resource| name.eq_ignore_ascii_case(resource)),
        Ty::Array(element) => unsafe_union_value(element),
        _ => false,
    }
}

impl Analyzer {
    pub(super) fn infer_union_array(
        &mut self,
        items: &[ast::ArrayItem],
        span: Span,
        ctx: &mut FnCtx,
        element: &Ty,
    ) -> Option<TExpr> {
        let mut elems = Vec::new();
        for item in items {
            let key = match &item.key {
                Some(key) => {
                    let value = self.infer(key, ctx)?;
                    if !matches!(value.ty(), Ty::Int | Ty::String | Ty::Mixed | Ty::Union(_)) {
                        self.diags.emit(
                            Diagnostic::error("array keys must be int or string")
                                .with_code("E0250")
                                .with_span(key.span()),
                        );
                        return None;
                    }
                    Some(value)
                }
                None => None,
            };
            let value = self.infer_for_type(&item.value, ctx, element)?;
            if !self.is_assignable(&value.ty(), element) {
                self.diags.emit(
                    Diagnostic::error("array element does not satisfy the declared union")
                        .with_code("E0242")
                        .with_span(span),
                );
                return None;
            }
            elems.push((key, value));
        }
        Some(TExpr::ArrayLit {
            elems,
            ety: element.clone(),
        })
    }
}

impl Analyzer {
    // Assignability may request a checked narrowing from mixed or bool to a
    // literal. A branch join must instead be valid for every possible value.
    pub(super) fn union_guarantees(&self, from: &Ty, to: &Ty) -> bool {
        if from == to || *to == Ty::Mixed {
            return true;
        }
        if *from == Ty::Mixed {
            return false;
        }
        if let Ty::Union(targets) = to {
            let accepts = |part: &M| {
                targets.iter().any(|target| match (part, target) {
                    (M::Value(a), M::Value(b)) => self.union_guarantees(a, b),
                    (M::False | M::True, M::Value(Ty::Bool)) => true,
                    _ => part == target,
                })
            };
            return match from {
                Ty::Union(parts) => parts.iter().all(accepts),
                other => accepts(&M::Value(other.clone())),
            };
        }
        if let Ty::Union(parts) = from {
            return parts.iter().all(|part| match part {
                M::Value(t) => self.union_guarantees(t, to),
                M::False | M::True => *to == Ty::Bool,
                _ => false,
            });
        }
        if let (Ty::Array(a), Ty::Array(b)) = (from, to) {
            return **a == Ty::Null || self.union_guarantees(a, b);
        }
        self.is_assignable(from, to)
    }
}
