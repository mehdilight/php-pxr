//! Closure environments specialize through the existing generic object pipeline.
//! By-value captures are immutable environment fields, copied into invocation
//! locals so mutation has PHP's per-invocation behavior.
use super::*;

pub(super) struct ClassScope {
    pub name: String,
    pub generic: bool,
    pub is_trait: bool,
    pub parent: Option<ClassRef>,
    pub methods: Vec<MethodDecl>,
}

const BOUND_RECEIVER: &str = "$bound$this";

fn variable(name: &str, span: Span) -> Expr {
    Expr::Var(name.into(), span)
}
fn local(name: &str, span: Span) -> LValue {
    LValue::Var {
        name: name.into(),
        span,
    }
}
fn field(name: &str, span: Span) -> Expr {
    Expr::Property {
        base: Box::new(variable("this", span)),
        name: name.into(),
        name_span: span,
        span,
    }
}
fn parameter(name: &str, ty: TypeRef, span: Span) -> Param {
    Param {
        name: name.into(),
        name_span: span,
        ty: Some(ty),
        span,
    }
}
fn superglobal(name: &str) -> bool {
    matches!(
        name,
        "GLOBALS"
            | "_GET"
            | "_POST"
            | "_COOKIE"
            | "_SERVER"
            | "_REQUEST"
            | "_FILES"
            | "_ENV"
            | "_SESSION"
    )
}
fn capture(name: &str, params: &[Param], captures: &mut Vec<String>) {
    if !superglobal(name)
        && !params.iter().any(|p| p.name == name)
        && !captures.iter().any(|n| n == name)
    {
        captures.push(name.into());
    }
}
fn target_reads(target: &LValue, read: bool, params: &[Param], captures: &mut Vec<String>) {
    match target {
        LValue::Var { name, .. } if read => capture(name, params, captures),
        LValue::Index { base, .. } => target_reads(base, true, params, captures),
        _ => {}
    }
}
fn free_variables(e: &Expr, params: &[Param], captures: &mut Vec<String>) {
    if let Expr::Var(name, _) = e {
        capture(name, params, captures);
    }
    if let Expr::Effect { statement, .. } = e {
        match &**statement {
            Stmt::Assign { target, .. } => target_reads(target, false, params, captures),
            Stmt::AssignOp { target, .. } | Stmt::IncDec { target, .. } => {
                target_reads(target, true, params, captures)
            }
            _ => {}
        }
    }
    for child in e.children() {
        free_variables(child, params, captures);
    }
}
fn bind_annotation(t: &mut TypeRef, scope: Option<&ClassScope>) -> Result<(), &'static str> {
    match t {
        TypeRef::Param(..) => {
            return Err(
                "closure annotations referring to enclosing generic types are not implemented yet",
            );
        }
        TypeRef::Named(n) if matches!(n.to_ascii_lowercase().as_str(), "self" | "parent") => {
            let scope = scope.ok_or("lexical closure annotations require a class scope")?;
            if scope.is_trait {
                return Err("lexical closure annotations in trait scope are not implemented yet");
            }
            *t = if n.eq_ignore_ascii_case("self") {
                TypeRef::Named(scope.name.clone())
            } else {
                let parent = scope
                    .parent
                    .as_ref()
                    .ok_or("`parent` closure annotation requires a parent class")?;
                if parent.type_args.is_empty() {
                    TypeRef::Named(parent.name.clone())
                } else {
                    TypeRef::Applied(parent.name.clone(), parent.type_args.clone(), parent.span)
                }
            };
        }
        TypeRef::Named(n) if n.eq_ignore_ascii_case("static") => {
            return Err("late static binding in closure annotations is not implemented yet");
        }
        TypeRef::Applied(n, args, _) => {
            if matches!(
                n.to_ascii_lowercase().as_str(),
                "self" | "parent" | "static"
            ) {
                return Err(
                    "lexical class names in closure annotations do not accept type arguments",
                );
            }
            for argument in args {
                bind_annotation(argument, scope)?;
            }
        }
        TypeRef::Array(t) => bind_annotation(t, scope)?,
        _ => {}
    }
    Ok(())
}
fn lexical_receiver(e: &Expr) -> bool {
    let direct = match e {
        Expr::Var(name, _) => name == "this",
        Expr::StaticCall { class_name, .. }
        | Expr::StaticProperty { class_name, .. }
        | Expr::New { class_name, .. }
        | Expr::Instanceof { class_name, .. } => matches!(
            class_name.to_ascii_lowercase().as_str(),
            "self" | "parent" | "static"
        ),
        Expr::ParentMethodCall { .. } => true,
        _ => false,
    };
    direct || e.children().iter().any(|e| lexical_receiver(e))
}
fn body_has_lexical_receiver(body: &[Stmt]) -> bool {
    body.iter().any(|s| {
        s.expressions().iter().any(|e| lexical_receiver(e))
            || s.bodies().iter().any(|b| body_has_lexical_receiver(b))
    })
}

fn forward_argument(parameter: &Param, span: Span) -> Expr {
    let value = variable(&parameter.name, span);
    let intrinsic = match &parameter.ty {
        Some(TypeRef::Applied(name, _, _)) if name == "$pxr_owned" => "$pxr_move",
        Some(TypeRef::Applied(name, _, _)) if name == "$pxr_borrow" => "$pxr_borrow",
        Some(TypeRef::Applied(name, _, _)) if name == "$pxr_borrow_mut" => "$pxr_borrow_mut",
        _ => return value,
    };
    Expr::Call {
        fallback: None,
        callee: intrinsic.into(),
        callee_span: span,
        type_args: vec![],
        args: vec![value],
        span,
    }
}

impl Parser {
    pub(super) fn parse_closure(&mut self, is_static: bool, is_async: bool) -> Result<Expr, ()> {
        let token = self.advance();
        let arrow = token.kind == TokenKind::Fn;
        let span = token.span;
        let scope = if self.in_method {
            self.closure_class
                .as_ref()
                .map(|scope| (scope.name.clone(), scope.generic, scope.is_trait))
        } else {
            None
        };
        if scope.as_ref().is_some_and(|(_, generic, _)| *generic)
            || self.in_method && self.method_generic
        {
            self.error(span, "closures in generic class/method scope require lexical type binding and are not implemented yet");
            return Err(());
        }
        let bound = scope.is_some() && !is_static && !self.method_static;
        if scope.as_ref().is_some_and(|(_, _, is_trait)| *is_trait) && !bound {
            self.error(span, "static trait closure scope is not implemented yet");
            return Err(());
        }
        self.expect(&TokenKind::LParen, "`(` after anonymous function keyword")?;
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) {
            let ty = if self.is_type_start() {
                Some(self.parse_type()?)
            } else {
                None
            };
            let t = self.expect_variable("closure parameter")?;
            let TokenKind::Variable(name) = t.kind else {
                unreachable!()
            };
            params.push(Param {
                name,
                name_span: t.span,
                ty,
                span: t.span,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)` after closure parameters")?;
        let mut captures = Vec::new();
        if !arrow && self.eat(&TokenKind::Use) {
            self.expect(&TokenKind::LParen, "`(` after closure use")?;
            if self.at(&TokenKind::RParen) {
                self.error(
                    self.peek().span,
                    "closure use requires at least one captured variable",
                );
                return Err(());
            }
            while !self.at(&TokenKind::RParen) {
                if self.at(&TokenKind::Amp) {
                    self.error(self.peek().span, "reference captures require an alias lifetime contract and are not implemented yet");
                    return Err(());
                }
                let t = self.expect_variable("captured variable")?;
                let TokenKind::Variable(name) = t.kind else {
                    unreachable!()
                };
                if name == "this"
                    || superglobal(&name)
                    || params.iter().any(|p| p.name == name)
                    || captures.contains(&name)
                {
                    self.error(t.span, "invalid or duplicate closure capture");
                    return Err(());
                }
                captures.push(name);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "`)` after closure captures")?;
        }
        let mut ret = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        // Resolve in the source scope before moving annotations into the hidden
        // environment class: its `self` is not the user's declaring class.
        for annotation in params
            .iter_mut()
            .filter_map(|p| p.ty.as_mut())
            .chain(ret.iter_mut())
        {
            if let Err(message) = bind_annotation(
                annotation,
                if self.in_method {
                    self.closure_class.as_ref()
                } else {
                    None
                },
            ) {
                self.error(span, message);
                return Err(());
            }
        }
        let previous_static = self.method_static;
        self.method_static |= is_static;
        let body_result = (|| {
            if arrow {
                self.expect(&TokenKind::DoubleArrow, "`=>` in arrow function")?;
                let expression = self.parse_expr()?;
                free_variables(&expression, &params, &mut captures);
                Ok(vec![Stmt::Return(Some(expression), span)])
            } else {
                self.parse_block()
            }
        })();
        self.method_static = previous_static;
        let mut body = body_result?;
        if scope.is_none() && body_has_lexical_receiver(&body) {
            self.error(
                span,
                "bound closure receivers and lexical class access require a class scope",
            );
            return Err(());
        }
        // `$this` is implicit, never a by-value local capture. Instance bodies
        // execute on the retained receiver; static bodies have no `$this` slot.
        captures.retain(|name| name != "this");
        if bound {
            captures.push(BOUND_RECEIVER.into());
        }
        // The source grammar cannot spell these symbols, preventing collisions.
        let id = format!("{}${}", span.start, self.generated_items.len());
        let class = format!("$closure${id}");
        let factory = format!("$closure_factory${id}");
        let types: Vec<_> = captures
            .iter()
            .enumerate()
            .map(|(i, _)| TypeRef::Param(format!("$capture{i}"), span))
            .collect();
        let type_params: Vec<_> = types
            .iter()
            .map(|t| match t {
                TypeRef::Param(n, _) => (n.clone(), span),
                _ => unreachable!(),
            })
            .collect();
        let capture_params: Vec<_> = captures
            .iter()
            .zip(&types)
            .map(|(n, t)| parameter(n, t.clone(), span))
            .collect();
        if let Some((owner, _, _)) = &scope {
            let bridge = format!("$closure_body${id}");
            // The receiver also supplies its concrete specialization type.
            // Keep bound bridges lazy: a class can override a trait factory
            // without instantiating the discarded trait closure's body.
            let mut bridge_params = capture_params.clone();
            let bridge_types: Vec<_> = bridge_params
                .iter()
                .filter_map(|p| match &p.ty {
                    Some(TypeRef::Param(name, span)) => Some((name.clone(), *span)),
                    _ => None,
                })
                .collect();
            bridge_params.extend(params.clone());
            self.closure_class
                .as_mut()
                .unwrap()
                .methods
                .push(MethodDecl {
                    is_async,
                    type_params: bridge_types,
                    bounds: vec![],
                    is_static: !bound,
                    is_abstract: false,
                    // The source grammar cannot spell this internal method name.
                    visibility: Visibility::Public,
                    name: bridge.clone(),
                    name_span: span,
                    params: bridge_params,
                    ret: ret.clone(),
                    body,
                    span,
                });
            let mut args: Vec<_> = captures.iter().map(|n| variable(n, span)).collect();
            args.extend(params.iter().map(|p| forward_argument(p, span)));
            let call = if bound {
                Expr::MethodCall {
                    base: Box::new(variable(BOUND_RECEIVER, span)),
                    name: bridge,
                    name_span: span,
                    type_args: vec![],
                    args,
                    span,
                }
            } else {
                Expr::StaticCall {
                    class_name: owner.clone(),
                    type_args: vec![],
                    method: bridge,
                    method_type_args: vec![],
                    args,
                    span,
                }
            };
            let result = if is_async {
                Expr::Call {
                    fallback: None,
                    callee: "$pxr_await".into(),
                    callee_span: span,
                    type_args: vec![],
                    args: vec![call],
                    span,
                }
            } else {
                call
            };
            body = vec![Stmt::ForwardReturn(result, span)];
        }
        let mut members: Vec<_> = captures
            .iter()
            .zip(&types)
            .map(|(n, t)| {
                ClassMember::Property(PropertyDecl {
                    is_static: false,
                    visibility: Visibility::Private,
                    ty: Some(t.clone()),
                    name: n.clone(),
                    name_span: span,
                    default: None,
                    span,
                })
            })
            .collect();
        let constructor_body = captures
            .iter()
            .map(|n| Stmt::Assign {
                target: LValue::Property {
                    base: Box::new(variable("this", span)),
                    name: n.clone(),
                    name_span: span,
                    span,
                },
                ty: None,
                value: variable(n, span),
                span,
            })
            .collect();
        members.push(ClassMember::Method(MethodDecl {
            is_async: false,
            type_params: vec![],
            bounds: vec![],
            is_static: false,
            is_abstract: false,
            visibility: Visibility::Public,
            name: "__construct".into(),
            name_span: span,
            params: capture_params.clone(),
            ret: None,
            body: constructor_body,
            span,
        }));
        let mut prologue: Vec<_> = captures
            .iter()
            .map(|n| Stmt::Assign {
                target: local(n, span),
                ty: None,
                value: field(n, span),
                span,
            })
            .collect();
        prologue.append(&mut body);
        members.push(ClassMember::Method(MethodDecl {
            is_async,
            type_params: vec![],
            bounds: vec![],
            is_static: false,
            is_abstract: false,
            visibility: Visibility::Public,
            name: "__invoke".into(),
            name_span: span,
            params,
            ret,
            body: prologue,
            span,
        }));
        self.generated_items
            .push(Stmt::ClassDecl(Box::new(ClassDecl {
                type_params: type_params.clone(),
                bounds: vec![],
                is_trait: false,
                trait_uses: vec![],
                is_interface: false,
                interfaces: vec![],
                is_abstract: false,
                name: class.clone(),
                name_span: span,
                extends: None,
                members,
                span,
            })));
        let class_ty = if types.is_empty() {
            TypeRef::Named(class.clone())
        } else {
            TypeRef::Applied(class.clone(), types.clone(), span)
        };
        self.generated_items.push(Stmt::FnDecl(Box::new(FnDecl {
            is_async: false,
            type_params,
            bounds: vec![],
            name: factory.clone(),
            name_span: span,
            params: capture_params,
            ret: Some(class_ty),
            body: vec![Stmt::Return(
                Some(Expr::New {
                    class_name: class,
                    type_args: types,
                    class_span: span,
                    args: captures.iter().map(|n| variable(n, span)).collect(),
                    span,
                }),
                span,
            )],
            span,
        })));
        Ok(Expr::Call {
            fallback: None,
            callee: factory,
            callee_span: span,
            type_args: vec![],
            args: captures
                .iter()
                .map(|n| variable(if n == BOUND_RECEIVER { "this" } else { n }, span))
                .collect(),
            span,
        })
    }
}
