use super::*;

impl Analyzer {
    fn join_returns(&self, left: &Ty, right: &Ty) -> Option<Ty> {
        if left == right {
            return Some(left.clone());
        }
        if matches!(left, Ty::Union(_)) && self.union_guarantees(right, left) {
            return Some(left.clone());
        }
        if matches!(right, Ty::Union(_)) && self.union_guarantees(left, right) {
            return Some(right.clone());
        }
        if *left == Ty::Infer {
            return Some(right.clone());
        }
        if *right == Ty::Infer {
            return Some(left.clone());
        }
        if *left == Ty::Void || *right == Ty::Void {
            return None;
        }
        if *left == Ty::Mixed || *right == Ty::Mixed {
            return Some(Ty::Mixed);
        }
        if left.is_numeric() && right.is_numeric() {
            return Some(Ty::Float);
        }
        match (left, right) {
            (Ty::Array(left), Ty::Array(right)) => {
                if **left == Ty::Null {
                    return Some(Ty::array((**right).clone()));
                }
                if **right == Ty::Null {
                    return Some(Ty::array((**left).clone()));
                }
                if left.contains_infer() || right.contains_infer() {
                    return self.join_returns(left, right).map(Ty::array);
                }
                if **left == Ty::Mixed || **right == Ty::Mixed {
                    return Some(Ty::array(Ty::Mixed));
                }
                None
            }
            (Ty::Named(_), Ty::Named(_)) => {
                if self.is_assignable(left, right) {
                    return Some(right.clone());
                }
                if self.is_assignable(right, left) {
                    return Some(left.clone());
                }
                let Ty::Named(mut candidate) = left.clone() else {
                    unreachable!()
                };
                while let Some(parent) = self
                    .classes
                    .get(&candidate.to_ascii_lowercase())
                    .and_then(|class| class.parent.clone())
                {
                    let parent_type = Ty::Named(self.classes[&parent].name.clone());
                    if self.is_assignable(right, &parent_type) {
                        return Some(parent_type);
                    }
                    candidate = parent;
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn finish_return_inference(
        &mut self,
        key: &str,
        display: &str,
        span: Span,
        ctx: &FnCtx,
    ) {
        if !ctx.infer_return {
            return;
        }
        let mut candidate = if ctx.return_types.is_empty() && !ctx.inference_failed {
            Ty::Void
        } else {
            Ty::Infer
        };
        for (ty, return_span) in &ctx.return_types {
            let Some(joined) = self.join_returns(&candidate, ty) else {
                self.diags.emit(Diagnostic::error(format!("cannot infer one return type for `{display}` from `{candidate}` and `{ty}`; add an explicit annotation")).with_code("E0284").with_span(*return_span));
                return;
            };
            candidate = joined;
        }
        let previous = self.class_expander.inferred_return(key);
        // Recompute from the whole current body. A callee's widening can change
        // Box<Child> to Box<Parent>; those are invariant types, not two actual
        // return paths to union. Preserve earlier evidence only while deferred.
        let joined = if candidate.contains_infer() {
            self.join_returns(&previous, &candidate).unwrap_or(previous)
        } else {
            candidate
        };
        self.class_expander.record_return(key, joined, span);
        if ctx.ret.contains_infer() {
            self.unresolved_returns.push((display.into(), span));
        }
    }
}
