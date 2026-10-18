//! Generic function templates specialize into ordinary concrete typed functions.
//! Queueing bodies permits recursion without recursively running the analyzer.
use super::*;

fn pattern_is_bound(pattern: &ast::TypeRef, bindings: &HashMap<String, Ty>) -> bool {
    match pattern {
        ast::TypeRef::Param(name, _) => bindings.contains_key(name),
        ast::TypeRef::Array(element) => pattern_is_bound(element, bindings),
        ast::TypeRef::Union(members, _) => members.iter().all(|t| pattern_is_bound(t, bindings)),
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
    if matches!(actual, Ty::Task(_)) {
        return Err("task handles cannot enter generic functions yet".into());
    }
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
        ast::TypeRef::Union(members, _) => {
            let fixed = |m: &ast::TypeRef| match m {
                ast::TypeRef::Int => *actual == Ty::Int,
                ast::TypeRef::Float => *actual == Ty::Float,
                ast::TypeRef::String => *actual == Ty::String,
                ast::TypeRef::Bool => *actual == Ty::Bool,
                ast::TypeRef::LiteralFalse | ast::TypeRef::LiteralTrue => *actual == Ty::Bool,
                _ => false,
            };
            if *actual == Ty::Mixed || members.iter().any(fixed) {
                return Ok(());
            }
            let actual = if let Ty::Union(parts) = actual {
                let rest: Vec<_> = parts
                    .iter()
                    .filter(|part| {
                        !members.iter().any(|m| {
                            matches!(
                                (m, *part),
                                (ast::TypeRef::LiteralNull, pxr_types::UnionMember::Null)
                                    | (ast::TypeRef::LiteralFalse, pxr_types::UnionMember::False)
                                    | (ast::TypeRef::LiteralTrue, pxr_types::UnionMember::True)
                            )
                        })
                    })
                    .cloned()
                    .collect();
                if rest.is_empty() {
                    return Ok(());
                }
                if rest.len() == 1 {
                    if let pxr_types::UnionMember::Value(t) = &rest[0] {
                        t.clone()
                    } else {
                        Ty::union(rest)
                    }
                } else {
                    Ty::union(rest)
                }
            } else {
                actual.clone()
            };
            let unbound: Vec<_> = members
                .iter()
                .filter(|m| !pattern_is_bound(m, bindings))
                .collect();
            if unbound.len() == 1 {
                infer_bindings(unbound[0], &actual, bindings, classes, class_infos)?;
            } else if unbound.len() > 1 {
                return Err("ambiguous union inference; supply explicit type arguments".into());
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
            if base.eq_ignore_ascii_case("TaskResult") {
                if let (Some(pattern), Ty::TaskResult(value)) = (patterns.first(), actual) {
                    return infer_bindings(pattern, value, bindings, classes, class_infos);
                }
                return Err("expected a TaskResult argument".into());
            }
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
        Ty::Infer => unreachable!("unresolved arguments cannot be specialized"),
        Ty::Void => "v".into(),
        Ty::Int => "i".into(),
        Ty::Float => "f".into(),
        Ty::Bool => "b".into(),
        Ty::String => "s".into(),
        Ty::Mixed => "m".into(),
        Ty::Union(_) => format!("u{}", hex_name(&ty.to_string())),
        Ty::Null => "z".into(),
        Ty::Array(element) => format!("a{}", encode_type(element)),
        Ty::Named(name) => format!("n{}", hex_name(name)),
        Ty::Task(result) => format!("task{}", encode_type(result)),
        Ty::TaskResult(result) => format!("result{}", encode_type(result)),
    }
}

pub(super) fn hex_name(name: &str) -> String {
    name.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct GenericCall {
    order: Vec<ArgumentIndex>,
    deferred: bool,
    key: (String, Vec<Ty>),
    signature: Signature,
    bindings: HashMap<String, Ty>,
    values: Vec<TExpr>,
}

impl GenericCall {
    fn deferred(span: Span) -> Self {
        Self {
            order: Vec::new(),
            deferred: true,
            key: (String::new(), Vec::new()),
            signature: Signature {
                declarations: Vec::new(),
                is_async: false,
                mangled: String::new(),
                display: String::new(),
                params: Vec::new(),
                param_names: Vec::new(),
                ret: Ty::Infer,
                span,
            },
            bindings: HashMap::new(),
            values: Vec::new(),
        }
    }
    fn expression(self, span: Span) -> TExpr {
        let abi_params = self.signature.abi_params();
        let arg_tys = self.values.iter().map(TExpr::ty).collect();
        arguments::ordered(
            TExpr::Call {
                callee: Callee::User {
                    mangled: self.signature.mangled,
                    name: self.signature.display,
                    params: abi_params,
                },
                ret: if self.signature.is_async {
                    Ty::Task(Box::new(self.signature.ret))
                } else {
                    self.signature.ret
                },
                args: self.values,
                arg_tys,
                span,
            },
            self.order,
        )
    }
}

impl Analyzer {
    pub(super) fn generic_method_compatible(
        &self,
        method: &ast::MethodDecl,
        inherited: &ast::MethodDecl,
    ) -> bool {
        fn shape(ty: &ast::TypeRef, parameters: &[(String, Span)]) -> String {
            match ty {
                ast::TypeRef::Param(name, _) => format!(
                    "p{}",
                    parameters
                        .iter()
                        .position(|(parameter, _)| parameter == name)
                        .unwrap_or(usize::MAX)
                ),
                ast::TypeRef::Array(element) => format!("a({})", shape(element, parameters)),
                ast::TypeRef::Union(members, _) => {
                    let mut parts: Vec<_> = members.iter().map(|m| shape(m, parameters)).collect();
                    parts.sort();
                    parts.join("|")
                }
                ast::TypeRef::Applied(name, args, _) => format!(
                    "{}<{}>",
                    name.to_ascii_lowercase(),
                    args.iter()
                        .map(|argument| shape(argument, parameters))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                ast::TypeRef::Named(name) => format!("n({})", name.to_ascii_lowercase()),
                _ => format!("{ty:?}"),
            }
        }
        if method.type_params.len() != inherited.type_params.len()
            || method.params.len() != inherited.params.len()
        {
            return false;
        }
        let normalized_bounds = |method: &ast::MethodDecl| {
            method
                .type_params
                .iter()
                .map(|(name, _)| {
                    let mut bounds: Vec<_> = method
                        .bounds
                        .iter()
                        .find(|(parameter, _)| parameter == name)
                        .into_iter()
                        .flat_map(|(_, bounds)| bounds)
                        .map(|bound| shape(bound, &method.type_params))
                        .collect();
                    bounds.sort();
                    bounds
                })
                .collect::<Vec<_>>()
        };
        if normalized_bounds(method) != normalized_bounds(inherited) {
            return false;
        }
        method
            .params
            .iter()
            .zip(&inherited.params)
            .all(|(left, right)| match (&left.ty, &right.ty) {
                (Some(left), Some(right)) => {
                    shape(left, &method.type_params) == shape(right, &inherited.type_params)
                }
                _ => false,
            })
            && match (&method.ret, &inherited.ret) {
                (Some(left), Some(right)) => {
                    shape(left, &method.type_params) == shape(right, &inherited.type_params)
                }
                _ => true,
            }
    }

    pub(super) fn infer_generic_method_call(
        &mut self,
        class: &ClassInfo,
        method: &MethodInfo,
        receiver: Option<TExpr>,
        type_args: &[ast::TypeRef],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
        virtual_call: bool,
    ) -> Option<TExpr> {
        let declaration = ast::FnDecl {
            is_async: method.ast_method.is_async,
            bounds: method.ast_method.bounds.clone(),
            type_params: method.ast_method.type_params.clone(),
            name: format!("{}::{}", method.declaring_class, method.name),
            name_span: method.name_span,
            params: method.ast_method.params.clone(),
            ret: method.ast_method.ret.clone(),
            body: Vec::new(),
            span: method.ast_method.span,
        };
        let mut prepared = self.prepare_generic_call(&declaration, type_args, args, span, ctx)?;
        if prepared.deferred {
            return Some(TExpr::Deferred);
        }
        let types = &prepared.key.1;
        let member = format!(
            "$g${}${}",
            method.name.to_ascii_lowercase(),
            types.iter().map(encode_type).collect::<Vec<_>>().join("$")
        );
        if method.ast_method.ret.is_none() {
            prepared.signature.ret = class
                .methods
                .get(&member)
                .map(|method| method.ret.clone())
                .unwrap_or(Ty::Infer);
        }
        // Materialize the same concrete member in every applicable implementation
        // and contract. The ordinary inheritance checker and dispatcher then
        // operate on concrete types, including interface and abstract calls.
        let mut targets = vec![method.clone()];
        if virtual_call {
            self.class_expander
                .request_virtual_method(&class.name, &member, &method.name, types);
            for candidate in self.classes.values() {
                if candidate.name.eq_ignore_ascii_case(&class.name)
                    || self.is_subclass_of(&candidate.name, &class.name)
                {
                    if let Some(implementation) =
                        candidate.methods.get(&method.name.to_ascii_lowercase())
                    {
                        targets.push(implementation.clone());
                    }
                }
            }
        }
        targets.sort_by(|left, right| left.declaring_class.cmp(&right.declaring_class));
        targets.dedup_by(|left, right| {
            left.declaring_class
                .eq_ignore_ascii_case(&right.declaring_class)
        });
        for target in targets {
            self.class_expander.specialize_method(
                &target.declaring_class,
                &target.ast_method,
                types,
                &member,
            );
        }
        self.specializations
            .insert(prepared.key.clone(), prepared.signature.clone());
        let concrete = class.methods.get(&member);
        let mangled = if virtual_call
            && concrete.is_some()
            && self.method_has_overrides(&class.name, &member)
        {
            mangle_dispatcher(&class.name, &member)
        } else {
            concrete
                .map(|method| method.mangled.clone())
                .unwrap_or_else(|| mangle_method(&method.declaring_class, &member))
        };
        let mut values = prepared.values;
        let mut order = prepared.order;
        let mut params = prepared.signature.abi_params();
        if let Some(receiver) = receiver {
            params.insert(0, Ty::Named(class.name.clone()));
            values.insert(0, receiver);
            order = std::iter::once(ArgumentIndex::value(0))
                .chain(order.into_iter().map(|i| i.shifted(1)))
                .collect();
        }
        let arg_tys = values.iter().map(TExpr::ty).collect();
        Some(arguments::ordered(
            TExpr::Call {
                callee: Callee::User {
                    mangled,
                    name: prepared.signature.display,
                    params,
                },
                ret: if prepared.signature.is_async {
                    Ty::Task(Box::new(prepared.signature.ret))
                } else {
                    prepared.signature.ret
                },
                args: values,
                arg_tys,
                span,
            },
            order,
        ))
    }

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
        if declaration.is_async && !ctx.is_async {
            self.diags.emit(
                Diagnostic::error("async functions can only be called from an async context")
                    .with_code("E0300")
                    .with_span(span),
            );
            return None;
        }
        let prepared = self.prepare_generic_call(declaration, type_args, args, span, ctx)?;
        if prepared.deferred {
            return Some(TExpr::Deferred);
        }
        if !self.specializations.contains_key(&prepared.key) {
            self.specializations
                .insert(prepared.key.clone(), prepared.signature.clone());
            self.pending_specializations.push_back((
                declaration.clone(),
                prepared.signature.clone(),
                prepared.bindings.clone(),
            ));
        }
        Some(prepared.expression(span))
    }

    fn prepare_generic_call(
        &mut self,
        declaration: &ast::FnDecl,
        type_args: &[ast::TypeRef],
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<GenericCall> {
        let binding = self.bind_arguments(args, &declaration.params, span, "E0230")?;
        let args = binding.values.as_slice();
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

        let mut values = vec![None; args.len()];
        let mut elements = Vec::new();
        for (step, argument) in binding.steps() {
            let index = step.parameter;
            let parameter = &declaration.params[index];
            let pattern = parameter.ty.as_ref()?;
            let pattern = if step.element.is_some() {
                let ast::TypeRef::Array(element) = pattern else {
                    unreachable!()
                };
                element.as_ref()
            } else {
                pattern
            };
            let value = if pattern_is_bound(pattern, &bindings) {
                let expected = self.resolve_type_with(pattern, &bindings);
                self.infer_for_type(argument, ctx, &expected)?
            } else {
                self.infer(argument, ctx)?
            };
            if value.ty().contains_infer() {
                return Some(GenericCall::deferred(span));
            }
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
            if step.element.is_some() {
                elements.push((argument.span(), value));
            } else {
                values[index] = Some(value);
            }
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
        if !self.check_bounds(&declaration.name, &declaration.bounds, &bindings, span) {
            return None;
        }
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
            let mangled = format!(
                "pxr$g${}${}",
                hex_name(&key.0),
                concrete
                    .iter()
                    .map(encode_type)
                    .collect::<Vec<_>>()
                    .join("$")
            );
            let ret = match &declaration.ret {
                Some(ret) => self.resolve_type_with(ret, &bindings),
                None => self.class_expander.inferred_return(&mangled),
            };
            Signature {
                declarations: declaration.params.clone(),
                is_async: declaration.is_async,
                mangled,
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
        if let Some(index) = binding.variadic {
            let Ty::Array(element) = &signature.params[index] else {
                unreachable!()
            };
            for (span, value) in &elements {
                if !self.is_assignable(&value.ty(), element) {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "variadic element expects `{element}`, found `{}`",
                            value.ty()
                        ))
                        .with_code("E0231")
                        .with_span(*span),
                    );
                    return None;
                }
            }
            values[index] = Some(binding.pack(
                elements.into_iter().map(|(_, value)| value).collect(),
                element.as_ref().clone(),
            ));
        }
        let mut values: Vec<_> = values.into_iter().map(Option::unwrap).collect();
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
        for (index, present) in binding.present.iter().enumerate() {
            if !present {
                values[index] = TExpr::Missing(signature.params[index].clone());
            }
        }
        let mut actual_types = values.iter().map(TExpr::ty).collect();
        binding.append_presence(&declaration.params, &mut values, &mut actual_types);
        let order = binding.evaluation_order(0, arguments::has_defaults(&declaration.params));
        Some(GenericCall {
            order,
            deferred: false,
            key,
            signature,
            bindings,
            values,
        })
    }
}
