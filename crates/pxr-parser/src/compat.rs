//! Source-only normalization. Generated closure adapters retain their own contracts.
use super::*;

pub(super) fn complete_returns(body: &mut Vec<Stmt>, span: Span) {
    fn visit(statements: &mut [Stmt]) {
        for statement in statements {
            if let Stmt::Return(value @ None, span) = statement {
                *value = Some(Expr::NullLit(*span));
            }
            if matches!(statement, Stmt::FnDecl(_) | Stmt::ClassDecl(_)) {
                continue;
            }
            for body in statement.bodies_mut() {
                visit(body);
            }
        }
    }
    visit(body);
    body.push(Stmt::Return(Some(Expr::NullLit(span)), span));
}
