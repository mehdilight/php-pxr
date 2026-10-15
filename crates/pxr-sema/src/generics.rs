//! Generic function templates specialize into ordinary concrete typed functions.
//! Queueing bodies permits recursion without recursively running the analyzer.
use super::*;

fn pattern_is_bound(pattern: &ast::TypeRef, bindings: &HashMap<String, Ty>) -> bool {
    match pattern {
        ast::TypeRef::Param(name, _) => bindings.contains_key(name),
        ast::TypeRef::Array(element) => pattern_is_bound(element, bindings),
        ast::TypeRef::Applied(_, args, _) => args
            .iter()
            .all(|argument| pattern_is_bound(argument, bindings)),
        _ => true,
    }
}

fn infer_bindings(
    pattern: &ast::TypeRef,
    actual: &Ty,
    bindings: &mut HashMap<String, Ty>,
    classes: &class_generics::ClassExpander,
    class_infos: &HashMap<String, ClassInfo>,
) -> Result<(), String> {
    match pattern {
        ast::TypeRef::Param(name, _) => {
            if let Some(previous) = bindings.get(name) {
                if previous != actual {
                    return Err(format!("conflicting types for `{name}`: `{previous}` and `{actual}`; specify type arguments explicitly"));
                }
            } else {
                bindings.insert(name.clone(), actual.clone());
            }
        }
        ast::TypeRef::Array(element) => {
            let Ty::Array(actual) = actual else {
                return Err(format!("expected an array argument, found `{actual}`"));
            };
            // An empty array contains no evidence for an element parameter.
            if **actual != Ty::Null {
                infer_bindings(element, actual, bindings, classes, class_infos)?;
            }
        }
        ast::TypeRef::Applied(base, patterns, _) => {
            let Ty::Named(name) = actual else {
                return Err(format!("expected `{base}<...>`, found `{actual}`"));
            };
            let mut pending = vec![name.to_ascii_lowercase()];
            let mut visited = std::collections::HashSet::new();
            let mut found = false;
            while let Some(name) = pending.pop() {
                if !visited.insert(name.clone()) {
                    continue;
                }
                if let Some(arguments) = classes.arguments(&name, base) {
                    found = true;
                    for (pattern, argument) in patterns.iter().zip(arguments) {
                        infer_bindings(
                            pattern,
                            &class_generics::concrete_type(argument),
                            bindings,
                            classes,
                            class_infos,
                        )?;
                    }
                }
                if let Some(info) = class_infos.get(&name) {
                    pending.extend(info.parent.iter().chain(&info.interfaces).cloned());
                }
            }
            if !found {
                return Err(format!(
                    "expected an instance of `{base}<...>`, found `{actual}`"
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn encode_type(ty: &Ty) -> String {
    match ty {
        Ty::Void => "v".into(),
        Ty::Int => "i".into(),
        Ty::Float => "f".into(),
        Ty::Bool => "b".into(),
        Ty::String => "s".into(),
        Ty::Mixed => "m".into(),
        Ty::Null => "z".into(),
        Ty::Array(element) => format!("a{}", encode_type(element)),
        Ty::Named(name) => format!("n{}", hex_name(name)),
    }
}

pub(super) fn hex_name(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl Analyzer {
    pub(super) fn is_compiled_function(&self, name: &str) -> bool {
        self.funcs.contains_key(name) || self.generics.contains_key(name)
    }

    pub(super) fn infer_generic_call(
        &mut self,
        declaration: &ast::FnDecl,
        type_args: &[ast::TypeRef],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        if args.len() != declaration.params.len() {
            self.diags.emit(
                Diagnostic::error(format!(
                    "function `{}` expects {} argument(s), but {} were given",
                    declaration.name,
                    declaration.params.len(),
                    args.len()
                ))
                .with_code("E0230")
                .with_span(span),
            );
            return None;
        }
        if !type_args.is_empty() && type_args.len() != declaration.type_params.len() {
            self.diags.emit(
                Diagnostic::error(format!(
                    "generic function `{}` expects {} type argument(s), found {}",
                    declaration.name,
                    declaration.type_params.len(),
                    type_args.len()
                ))
                .with_code("E0280")
                .with_span(span),
            );
            return None;
        }

        let mut bindings = HashMap::new();
        let before = self.diags.items().len();
        for ((name, _), argument) in declaration.type_params.iter().zip(type_args) {
            let ty = self.resolve_type_with(argument, &ctx.type_bindings);
            if ty == Ty::Void {
                self.diags.emit(
                    Diagnostic::error("void cannot be a generic type argument")
                        .with_code("E0280")
                        .with_span(span),
                );
            }
            bindings.insert(name.clone(), ty);
        }
        if self.diags.items().len() != before {
            return None;
        }

        let mut values = Vec::new();
        for (parameter, argument) in declaration.params.iter().zip(args) {
            let pattern = parameter.ty.as_ref()?;
            let value = if pattern_is_bound(pattern, &bindings) {
                let expected = self.resolve_type_with(pattern, &bindings);
                self.infer_for_type(argument, ctx, &expected)?
            } else {
                self.infer(argument, ctx)?
            };
            if value.ty() == Ty::Void {
                self.diags.emit(
                    Diagnostic::error("a generic argument must produce a value")
                        .with_code("E0231")
                        .with_span(argument.span()),
                );
                return None;
            }
            if type_args.is_empty() {
                if let Err(message) = infer_bindings(
                    pattern,
                    &value.ty(),
                    &mut bindings,
                    &self.class_expander,
                    &self.classes,
                ) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "cannot infer `{}`: {message}",
                            declaration.name
                        ))
                        .with_code("E0281")
                        .with_span(argument.span()),
                    );
                    return None;
                }
            }
            values.push(value);
        }
        let mut concrete = Vec::new();
        for (name, _) in &declaration.type_params {
            let Some(ty) = bindings.get(name) else {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "cannot infer `{name}` for `{}`; provide explicit type arguments",
                        declaration.name
                    ))
                    .with_code("E0281")
                    .with_span(span),
                );
                return None;
            };
            concrete.push(ty.clone());
        }

        let key = (declaration.name.to_ascii_lowercase(), concrete.clone());
        let signature = if let Some(signature) = self.specializations.get(&key) {
            signature.clone()
        } else {
            // Bound polymorphic recursion and specialization explosions with a
            // diagnostic rather than an analyzer stack overflow or endless queue.
            if self.specializations.len() >= 256
                || self
                    .specializations
                    .keys()
                    .filter(|(name, _)| name == &key.0)
                    .count()
                    >= 64
            {
                self.diags.emit(Diagnostic::error(format!(
                    "generic specialization limit exceeded for `{}`; recursive calls must reuse concrete type arguments",
                    declaration.name
                )).with_code("E0283").with_span(span));
                return None;
            }
            let params = declaration
                .params
                .iter()
                .map(|parameter| self.resolve_type_with(parameter.ty.as_ref().unwrap(), &bindings))
                .collect();
            let ret = self.resolve_type_with(declaration.ret.as_ref()?, &bindings);
            Signature {
                mangled: format!(
                    "pxr$g${}${}",
                    hex_name(&key.0),
                    concrete
                        .iter()
                        .map(encode_type)
                        .collect::<Vec<_>>()
                        .join("$")
                ),
                display: format!(
                    "{}<{}>",
                    declaration.name,
                    concrete
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                params,
                param_names: declaration
                    .params
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect(),
                ret,
                span: declaration.span,
            }
        };
        for ((argument, value), expected) in args.iter().zip(&values).zip(&signature.params) {
            if !self.is_assignable(&value.ty(), expected) {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "argument of `{}` expects `{expected}`, found `{}`",
                        signature.display,
                        value.ty()
                    ))
                    .with_code("E0231")
                    .with_span(argument.span()),
                );
                return None;
            }
        }
        if !self.specializations.contains_key(&key) {
            self.specializations.insert(key, signature.clone());
            self.pending_specializations.push_back((
                declaration.clone(),
                signature.clone(),
                bindings,
            ));
        }
        let arg_tys = values.iter().map(TExpr::ty).collect();
        Some(TExpr::Call {
            callee: Callee::User {
                mangled: signature.mangled,
                name: signature.display,
                params: signature.params,
            },
            ret: signature.ret,
            args: values,
            arg_tys,
            span,
        })
    }
}
