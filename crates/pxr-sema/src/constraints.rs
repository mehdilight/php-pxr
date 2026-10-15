use super::*;

impl Analyzer {
    pub(super) fn validate_bounds(&mut self, bounds: &[(String, Vec<ast::TypeRef>)]) {
        for (_, requirements) in bounds {
            for requirement in requirements {
                if !matches!(
                    requirement,
                    ast::TypeRef::Named(_) | ast::TypeRef::Applied(..) | ast::TypeRef::Param(..)
                ) {
                    self.diags.emit(
                        Diagnostic::error("a generic bound must name a class or interface")
                            .with_code("E0286")
                            .with_span(requirement.span()),
                    );
                } else {
                    self.class_expander.validate_type(requirement);
                }
            }
        }
    }

    pub(super) fn check_bounds(
        &mut self,
        name: &str,
        bounds: &[(String, Vec<ast::TypeRef>)],
        bindings: &HashMap<String, Ty>,
        span: Span,
    ) -> bool {
        let before = self.diags.items().len();
        for (parameter, requirements) in bounds {
            let Some(actual) = bindings.get(parameter) else {
                continue;
            };
            for requirement in requirements {
                let bound = self.resolve_type_with(requirement, bindings);
                if !matches!((&actual, &bound), (Ty::Named(_), Ty::Named(_)))
                    || !self.is_assignable(actual, &bound)
                {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "type `{actual}` for `{name}` does not satisfy `{parameter}: {bound}`"
                        ))
                        .with_code("E0286")
                        .with_span(span),
                    );
                }
            }
        }
        self.diags.items().len() == before
    }
}
