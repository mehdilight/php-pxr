//! A final check against concrete, specialized types: a borrow must never be
//! silently boxed, widened, or copied by ordinary PHP argument coercion.
use crate::{Callee, TExpr, TProgram, TStmt, TTarget};
use pxr_diag::{Diagnostic, Diagnostics, Span};
use pxr_types::Ty;

pub(crate) fn check(program: &TProgram, diagnostics: &mut Diagnostics) {
    for f in program.funcs.iter().chain(std::iter::once(&program.main)) {
        statements(&f.body, diagnostics);
    }
}
fn payload(e: &TExpr) -> Option<(Ty, Span)> {
    match e {
        TExpr::Pass {
            expr,
            borrowed: true,
            span,
        } => Some((expr.ty(), *span)),
        TExpr::Coerce { expr, .. } => payload(expr),
        _ => None,
    }
}
fn argument(e: &TExpr, want: &Ty, d: &mut Diagnostics) {
    if let Some((actual, span)) = payload(e) {
        // Semantic assignability has already checked nominal ancestry. All
        // nominal upcasts retain the same object pointer/layout. Everything
        // else, including arrays, must agree exactly at a borrow boundary.
        if actual != *want && !matches!((&actual, want), (Ty::Named(_), Ty::Named(_))) {
            d.emit(Diagnostic::error(format!("borrow cannot change representation from `{actual}` to `{want}`; annotate the owner with the exact borrowed type"))
                .with_code("E0290").with_span(span));
        }
    }
}
fn expression(e: &TExpr, d: &mut Diagnostics) {
    match e {
        TExpr::Effect { statement, .. } => statements(std::slice::from_ref(statement), d),
        TExpr::Ordered { expr, .. }
        | TExpr::Pass { expr, .. }
        | TExpr::Coerce { expr, .. }
        | TExpr::Un { expr, .. }
        | TExpr::Instanceof { expr, .. } => expression(expr, d),
        TExpr::Bin { lhs, rhs, .. } => {
            expression(lhs, d);
            expression(rhs, d);
        }
        TExpr::Ternary {
            cond,
            then,
            else_branch,
            ..
        } => {
            expression(cond, d);
            if let Some(e) = then {
                expression(e, d);
            }
            expression(else_branch, d);
        }
        TExpr::Match { subject, arms, .. } => {
            expression(subject, d);
            for a in arms {
                for e in &a.conds {
                    expression(e, d);
                }
                expression(&a.result, d);
            }
        }
        TExpr::ArrayLit { elems, .. } | TExpr::ArgumentPack { elems, .. } => {
            for (key, value) in elems {
                if let Some(e) = key {
                    expression(e, d);
                }
                expression(value, d);
            }
        }
        TExpr::Index { base, index, .. } => {
            expression(base, d);
            expression(index, d);
        }
        TExpr::Property { base, .. } => expression(base, d),
        TExpr::Call { callee, args, .. } => {
            if let Callee::User { params, .. } = callee {
                for (e, want) in args.iter().zip(params) {
                    argument(e, want, d);
                }
            }
            for e in args {
                expression(e, d);
            }
        }
        TExpr::New {
            prop_defaults,
            ctor,
            ..
        } => {
            for (_, e) in prop_defaults {
                expression(e, d);
            }
            if let Some((_, args, params)) = ctor {
                for (e, want) in args.iter().zip(params) {
                    argument(e, want, d);
                    expression(e, d);
                }
            }
        }
        TExpr::Deferred
        | TExpr::Int(_)
        | TExpr::Float(_)
        | TExpr::Str(_)
        | TExpr::Bool(_)
        | TExpr::Null
        | TExpr::Var { .. }
        | TExpr::Missing(_) => {}
    }
}
fn target(t: &TTarget, d: &mut Diagnostics) {
    match t {
        TTarget::Index { base, key, .. } => {
            expression(base, d);
            expression(key, d);
        }
        TTarget::Property { base, .. } | TTarget::Push { base, .. } => expression(base, d),
        TTarget::Nested { parent, key, .. } => {
            target(parent, d);
            if let Some((key, _)) = key {
                expression(key, d);
            }
        }
        TTarget::Var { .. } | TTarget::MixedVar { .. } => {}
    }
}
fn statements(stmts: &[TStmt], d: &mut Diagnostics) {
    for s in stmts {
        match s {
            TStmt::Block(body) => statements(body, d),
            TStmt::Echo(es) => {
                for e in es {
                    expression(e, d);
                }
            }
            TStmt::Print(e) | TStmt::Expr(e) => expression(e, d),
            TStmt::Assign {
                target: t, value, ..
            }
            | TStmt::AssignOp {
                target: t, value, ..
            } => {
                target(t, d);
                expression(value, d);
            }
            TStmt::IncDec { target: t, .. } => target(t, d),
            TStmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                expression(cond, d);
                statements(then_branch, d);
                for (e, _, body) in elifs {
                    expression(e, d);
                    statements(body, d);
                }
                if let Some(body) = else_branch {
                    statements(body, d);
                }
            }
            TStmt::While { cond, body, .. } | TStmt::DoWhile { cond, body, .. } => {
                expression(cond, d);
                statements(body, d);
            }
            TStmt::For {
                init,
                cond,
                update,
                body,
            } => {
                statements(init, d);
                if let Some((e, _)) = cond {
                    expression(e, d);
                }
                statements(update, d);
                statements(body, d);
            }
            TStmt::Foreach { base, body, .. } => {
                expression(base, d);
                statements(body, d);
            }
            TStmt::Switch { subject, cases, .. } => {
                expression(subject, d);
                for c in cases {
                    if let Some(e) = &c.test {
                        expression(e, d);
                    }
                    statements(&c.body, d);
                }
            }
            TStmt::Return { value, .. } => {
                if let Some(e) = value {
                    expression(e, d);
                }
            }
            TStmt::PhpInclude { .. } | TStmt::Break(_) | TStmt::Continue(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concrete_borrows_reject_reboxing_but_allow_nominal_upcasts() {
        for (actual, want, valid) in [
            (Ty::array(Ty::Int), Ty::array(Ty::Mixed), false),
            (Ty::Int, Ty::Float, false),
            (Ty::Named("Child".into()), Ty::Named("Parent".into()), true),
        ] {
            let value = TExpr::Coerce {
                expr: Box::new(TExpr::Pass {
                    expr: Box::new(TExpr::Var {
                        name: "x".into(),
                        slot: 0,
                        ty: actual,
                    }),
                    borrowed: true,
                    span: Span::new(1, 2),
                }),
                ty: want.clone(),
            };
            let mut diagnostics = Diagnostics::new();
            argument(&value, &want, &mut diagnostics);
            assert_eq!(!diagnostics.has_errors(), valid);
        }
    }
}
