//! Closure environments specialize through the existing generic object pipeline.
//! By-value captures are immutable environment fields, copied into invocation
//! locals so mutation has PHP's per-invocation behavior.
use super::*;

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
fn lexical_type(t: &TypeRef) -> bool {
    match t {
        TypeRef::Param(..) => true,
        TypeRef::Named(n) => matches!(
            n.to_ascii_lowercase().as_str(),
            "self" | "parent" | "static"
        ),
        TypeRef::Applied(n, args, _) => {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "self" | "parent" | "static"
            ) || args.iter().any(lexical_type)
        }
        TypeRef::Array(t) => lexical_type(t),
        _ => false,
    }
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

impl Parser {
    pub(super) fn parse_closure(&mut self) -> Result<Expr, ()> {
        let token = self.advance();
        let arrow = token.kind == TokenKind::Fn;
        let span = token.span;
        if self.in_method {
            self.error(span, "closures in class scope require lexical receiver binding and are not implemented yet");
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
        let ret = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        if params
            .iter()
            .filter_map(|p| p.ty.as_ref())
            .any(lexical_type)
            || ret.as_ref().is_some_and(lexical_type)
        {
            self.error(span, "closure annotations referring to enclosing generic or lexical class types are not implemented yet");
            return Err(());
        }
        let mut body = if arrow {
            self.expect(&TokenKind::DoubleArrow, "`=>` in arrow function")?;
            let expression = self.parse_expr()?;
            free_variables(&expression, &params, &mut captures);
            vec![Stmt::Return(Some(expression), span)]
        } else {
            self.parse_block()?
        };
        if body_has_lexical_receiver(&body) {
            self.error(
                span,
                "bound closure receivers and lexical class access are not implemented yet",
            );
            return Err(());
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
            is_async: false,
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
            args: captures.iter().map(|n| variable(n, span)).collect(),
            span,
        })
    }
}
