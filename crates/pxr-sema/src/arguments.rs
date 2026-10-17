//! Native optional arguments. Presence travels with the call so virtual methods
//! choose their own defaults, including a default different from a base method.
use super::*;
pub(super) const PROVIDED: &str = OPTIONAL_PRESENCE_PARAMETER;
pub(super) fn has_defaults(params: &[ast::Param]) -> bool {
    params.iter().any(|p| p.default.is_some())
}
pub(super) fn flags(total: usize, supplied: usize) -> TExpr {
    TExpr::ArrayLit {
        elems: (0..total)
            .map(|i| (None, TExpr::Bool(i < supplied)))
            .collect(),
        ety: Ty::Bool,
    }
}
pub(super) fn append_presence(
    params: &[ast::Param],
    supplied: usize,
    values: &mut Vec<TExpr>,
    types: &mut Vec<Ty>,
) {
    if has_defaults(params) {
        values.push(flags(params.len(), supplied));
        types.push(Ty::array(Ty::Bool));
    }
}
impl Signature {
    pub(super) fn abi_params(&self) -> Vec<Ty> {
        let mut types = self.params.clone();
        if has_defaults(&self.declarations) {
            types.push(Ty::array(Ty::Bool));
        }
        types
    }
}
impl Analyzer {
    pub(super) fn check_optional_override(
        &mut self,
        child: &[ast::Param],
        parent: &[ast::Param],
        span: Span,
    ) {
        if child
            .iter()
            .zip(parent)
            .any(|(c, p)| p.default.is_some() && c.default.is_none())
        {
            self.diags.emit(
                Diagnostic::error("an override cannot make an optional parameter required")
                    .with_code("E0311")
                    .with_span(span),
            );
        }
    }
    pub(super) fn complete_arguments(
        &mut self,
        args: &[ast::Expr],
        params: &[ast::Param],
        span: Span,
        code: &'static str,
    ) -> Option<Vec<ast::Expr>> {
        if args.len() > params.len() || params.iter().skip(args.len()).any(|p| p.default.is_none())
        {
            self.diags.emit(
                Diagnostic::error(format!(
                    "expected {} to {} arguments, found {}",
                    params.iter().take_while(|p| p.default.is_none()).count(),
                    params.len(),
                    args.len()
                ))
                .with_code(code)
                .with_span(span),
            );
            return None;
        }
        let mut values = args.to_vec();
        values.extend(
            params
                .iter()
                .skip(args.len())
                .map(|p| p.default.clone().unwrap()),
        );
        Some(values)
    }
    pub(super) fn validate_defaults(
        &mut self,
        params: &[ast::Param],
        types: &[Ty],
        bindings: &HashMap<String, Ty>,
    ) {
        let mut optional = false;
        for (p, ty) in params.iter().zip(types) {
            let Some(default) = &p.default else {
                if optional {
                    self.diags.emit(
                        Diagnostic::error("required parameters cannot follow optional parameters")
                            .with_code("E0311")
                            .with_span(p.span),
                    );
                }
                continue;
            };
            optional = true;
            if !static_initializer(default) {
                self.diags.emit(Diagnostic::error("native optional defaults currently require constant scalar/array expressions").with_code("E0311").with_span(default.span()));
                continue;
            }
            let mut context = FnCtx::new(Ty::Void, false);
            context.type_bindings = bindings.clone();
            if let Some(value) = self.infer_for_type(default, &mut context, ty) {
                if !self.is_assignable(&value.ty(), ty) || !self.default_literals_fit(default, ty) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "default for `${}` must satisfy `{ty}`, found `{}`",
                            p.name,
                            value.ty()
                        ))
                        .with_code("E0311")
                        .with_span(default.span()),
                    );
                }
            }
        }
    }
    fn default_literals_fit(&mut self, expr: &ast::Expr, ty: &Ty) -> bool {
        if *ty == Ty::Mixed {
            return true;
        }
        if let ast::Expr::Paren(e, _) = expr {
            return self.default_literals_fit(e, ty);
        }
        if let ast::Expr::ArrayLit { items, .. } = expr {
            return match ty {
                Ty::Array(element) => items.iter().all(|item| self.default_literals_fit(&item.value,element)),
                Ty::Union(members) => members.iter().any(|m| matches!(m,pxr_types::UnionMember::Value(t @ Ty::Array(_)) if self.default_literals_fit(expr,t))),
                _ => false,
            };
        }
        let literal = match expr {
            ast::Expr::NullLit(_) => Some(Ty::Union(vec![pxr_types::UnionMember::Null])),
            ast::Expr::BoolLit(false, _) => Some(Ty::Union(vec![pxr_types::UnionMember::False])),
            ast::Expr::BoolLit(true, _) => Some(Ty::Union(vec![pxr_types::UnionMember::True])),
            _ => None,
        };
        if let Some(value) = literal {
            return self.union_guarantees(&value, ty);
        }
        let mut context = FnCtx::new(Ty::Void, false);
        self.infer(expr, &mut context)
            .is_some_and(|value| self.union_guarantees(&value.ty(), ty))
    }

    pub(super) fn defaults_prologue(
        &mut self,
        params: &[ast::Param],
        ctx: &mut FnCtx,
    ) -> Vec<TStmt> {
        if !has_defaults(params) {
            return Vec::new();
        }
        let mask_ty = Ty::array(Ty::Bool);
        let mask = ctx.declare(PROVIDED, mask_ty.clone());
        let mut statements = Vec::new();
        for (index, p) in params.iter().enumerate() {
            if let Some(default) = &p.default {
                let slot = ctx.slot_of(&p.name).unwrap();
                let ty = ctx.slot_ty(slot);
                let Some(value) = self.infer_for_type(default, ctx, &ty) else {
                    continue;
                };
                let supplied = TExpr::Index {
                    base: Box::new(TExpr::Var {
                        name: PROVIDED.into(),
                        slot: mask,
                        ty: mask_ty.clone(),
                    }),
                    base_ty: mask_ty.clone(),
                    index: Box::new(TExpr::Int(index as i64)),
                    index_ty: Ty::Int,
                    ety: Ty::Bool,
                };
                statements.push(TStmt::If {
                    cond: TExpr::Un {
                        op: AUnOp::Not,
                        operand_ty: Ty::Bool,
                        ty: Ty::Bool,
                        expr: Box::new(supplied),
                    },
                    cond_ty: Ty::Bool,
                    then_branch: vec![TStmt::Assign {
                        target: TTarget::Var {
                            name: p.name.clone(),
                            slot,
                            ty: ty.clone(),
                        },
                        ty,
                        value,
                    }],
                    elifs: vec![],
                    else_branch: None,
                });
            }
        }
        statements
    }
}
