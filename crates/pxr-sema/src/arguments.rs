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
pub(super) fn ordered(expr: TExpr, order: Vec<ArgumentIndex>) -> TExpr {
    if order
        .iter()
        .copied()
        .eq((0..order.len()).map(ArgumentIndex::value))
    {
        expr
    } else {
        TExpr::Ordered {
            expr: Box::new(expr),
            order,
        }
    }
}
pub(super) struct BoundArguments {
    pub values: Vec<ast::Expr>,
    pub present: Vec<bool>,
    pub order: Vec<usize>,
    pub evaluation: Vec<ArgumentIndex>,
    pub variadic: Option<usize>,
}
impl BoundArguments {
    pub fn append_presence(
        &self,
        declarations: &[ast::Param],
        values: &mut Vec<TExpr>,
        types: &mut Vec<Ty>,
    ) {
        if has_defaults(declarations) {
            values.push(TExpr::ArrayLit {
                elems: self
                    .present
                    .iter()
                    .map(|p| (None, TExpr::Bool(*p)))
                    .collect(),
                ety: Ty::Bool,
            });
            types.push(Ty::array(Ty::Bool));
        }
    }
    pub fn evaluation_order(&self, offset: usize, optional: bool) -> Vec<ArgumentIndex> {
        let mut order: Vec<_> = (0..offset).map(ArgumentIndex::value).collect();
        order.extend(self.evaluation.iter().map(|i| i.shifted(offset)));
        if optional {
            order.push(ArgumentIndex::value(offset + self.values.len()));
        }
        order
    }
    pub fn steps(&self) -> Vec<(ArgumentIndex, &ast::Expr)> {
        self.evaluation
            .iter()
            .map(|&index| {
                let expr = &self.values[index.parameter];
                let expr = if let Some(element) = index.element {
                    let ast::Expr::ArrayLit { items, .. } = expr else {
                        unreachable!()
                    };
                    &items[element].value
                } else {
                    expr
                };
                (index, expr)
            })
            .collect()
    }
    pub fn pack(&self, values: Vec<TExpr>, ety: Ty) -> TExpr {
        let ast::Expr::ArrayLit { items, .. } = &self.values[self.variadic.unwrap()] else {
            unreachable!()
        };
        let elems = items
            .iter()
            .zip(values)
            .map(|(item, value)| {
                let key = item.key.as_ref().map(|key| {
                    let ast::Expr::StrLit(value, _) = key else {
                        unreachable!()
                    };
                    TExpr::Str(value.clone())
                });
                (key, value)
            })
            .collect();
        TExpr::ArgumentPack { elems, ety }
    }
    pub fn infer_values(
        &self,
        analyzer: &mut Analyzer,
        ctx: &mut FnCtx,
        types: &[Ty],
        code: &'static str,
    ) -> Option<Vec<TExpr>> {
        let mut values = vec![None; self.values.len()];
        let mut elements = Vec::new();
        for (index, argument) in self.steps() {
            let expected = if index.element.is_some() {
                let Ty::Array(element) = &types[index.parameter] else {
                    unreachable!()
                };
                element.as_ref()
            } else {
                &types[index.parameter]
            };
            let value = analyzer.infer_for_type(argument, ctx, expected)?;
            if !analyzer.is_assignable(&value.ty(), expected) {
                analyzer.diags.emit(
                    Diagnostic::error(format!(
                        "argument expects `{expected}`, found `{}`",
                        value.ty()
                    ))
                    .with_code(code)
                    .with_span(argument.span()),
                );
                return None;
            }
            if index.element.is_some() {
                elements.push(value);
            } else {
                values[index.parameter] = Some(if self.present[index.parameter] {
                    value
                } else {
                    TExpr::Missing(expected.clone())
                });
            }
        }
        if let Some(index) = self.variadic {
            let Ty::Array(element) = &types[index] else {
                unreachable!()
            };
            values[index] = Some(self.pack(elements, element.as_ref().clone()));
        }
        Some(values.into_iter().map(Option::unwrap).collect())
    }
    pub fn wrap(&self, expr: TExpr, offset: usize, optional: bool) -> TExpr {
        let order = self.evaluation_order(offset, optional);
        ordered(expr, order)
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
    pub(super) fn check_named_dispatch(
        &mut self,
        class: &ClassInfo,
        method: &MethodInfo,
        args: &[ast::Expr],
        span: Span,
    ) -> bool {
        if !args
            .iter()
            .any(|a| matches!(a, ast::Expr::NamedArgument { .. }))
        {
            return true;
        }
        let different = self
            .classes
            .values()
            .filter(|c| self.is_subclass_of(&c.name, &class.name))
            .filter_map(|c| c.methods.get(&method.name.to_ascii_lowercase()))
            .any(|target| target.param_names != method.param_names);
        if different {
            self.diags.emit(Diagnostic::error("named virtual calls currently require matching parameter names across all possible overrides").with_code("E0312").with_span(span));
            return false;
        }
        true
    }
    pub(super) fn check_optional_override(
        &mut self,
        child: &[ast::Param],
        parent: &[ast::Param],
        span: Span,
    ) {
        if child
            .iter()
            .zip(parent)
            .any(|(c, p)| c.variadic != p.variadic)
        {
            self.diags.emit(
                Diagnostic::error("overrides must preserve the variadic parameter contract")
                    .with_code("E0313")
                    .with_span(span),
            );
        }
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
    pub(super) fn bind_arguments(
        &mut self,
        args: &[ast::Expr],
        params: &[ast::Param],
        span: Span,
        arity_code: &'static str,
    ) -> Option<BoundArguments> {
        let mut values: Vec<Option<ast::Expr>> = vec![None; params.len()];
        let mut order = Vec::new();
        let mut positional = 0;
        let rest = params.iter().position(|p| p.variadic);
        let mut rest_items = Vec::new();
        let mut rest_started = false;
        let mut evaluation = Vec::new();
        for argument in args {
            let (index, value) = if let ast::Expr::NamedArgument { name, value, span } = argument {
                let index = params.iter().position(|p| p.name == *name && !p.variadic);
                if index.is_none() && rest.is_some() {
                    if rest_items.iter().any(|item: &ast::ArrayItem| matches!(&item.key,Some(ast::Expr::StrLit(key,_)) if key == name.as_bytes())) {
                        self.diags.emit(Diagnostic::error(format!("argument `{name}` was supplied more than once")).with_code("E0312").with_span(*span));
                        return None;
                    }
                    if !rest_started {
                        order.push(rest.unwrap());
                        rest_started = true;
                    }
                    evaluation.push(ArgumentIndex {
                        parameter: rest.unwrap(),
                        element: Some(rest_items.len()),
                    });
                    rest_items.push(ast::ArrayItem {
                        key: Some(ast::Expr::StrLit(name.as_bytes().to_vec(), *span)),
                        value: value.as_ref().clone(),
                        span: *span,
                    });
                    continue;
                }
                let Some(index) = index else {
                    self.diags.emit(
                        Diagnostic::error(format!("unknown named argument `{name}`"))
                            .with_code("E0312")
                            .with_span(*span),
                    );
                    return None;
                };
                (index, value.as_ref())
            } else {
                let index = positional;
                positional += 1;
                (index, argument)
            };
            if rest.is_some_and(|r| index >= r) {
                if !rest_started {
                    order.push(rest.unwrap());
                    rest_started = true;
                }
                evaluation.push(ArgumentIndex {
                    parameter: rest.unwrap(),
                    element: Some(rest_items.len()),
                });
                rest_items.push(ast::ArrayItem {
                    key: None,
                    value: value.clone(),
                    span: value.span(),
                });
                continue;
            }
            if index >= values.len() {
                self.diags.emit(
                    Diagnostic::error("too many positional arguments")
                        .with_code(arity_code)
                        .with_span(span),
                );
                return None;
            }
            if values[index].is_some() {
                self.diags.emit(
                    Diagnostic::error(format!(
                        "argument `${}` was supplied more than once",
                        params[index].name
                    ))
                    .with_code("E0312")
                    .with_span(argument.span()),
                );
                return None;
            }
            values[index] = Some(value.clone());
            order.push(index);
            evaluation.push(ArgumentIndex::value(index));
        }
        if let Some(index) = rest {
            values[index] = Some(ast::Expr::ArrayLit {
                items: rest_items,
                span,
            });
            if !rest_started {
                order.push(index);
            }
        }
        let present = values.iter().map(Option::is_some).collect();
        for (index, parameter) in params.iter().enumerate() {
            if values[index].is_none() {
                let Some(default) = &parameter.default else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "missing required argument `${}`",
                            parameter.name
                        ))
                        .with_code(arity_code)
                        .with_span(span),
                    );
                    return None;
                };
                values[index] = Some(default.clone());
                order.push(index);
                evaluation.push(ArgumentIndex::value(index));
            }
        }
        Some(BoundArguments {
            values: values.into_iter().map(Option::unwrap).collect(),
            present,
            order,
            evaluation,
            variadic: rest,
        })
    }
    pub(super) fn validate_defaults(
        &mut self,
        params: &[ast::Param],
        types: &[Ty],
        bindings: &HashMap<String, Ty>,
    ) {
        let mut optional = false;
        for (index, (p, ty)) in params.iter().zip(types).enumerate() {
            if p.variadic {
                if index + 1 != params.len() || p.default.is_some() {
                    self.diags.emit(
                        Diagnostic::error(
                            "a variadic parameter must be last and cannot have a default",
                        )
                        .with_code("E0313")
                        .with_span(p.span),
                    );
                }
                continue;
            }
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

impl Builtin {
    // Names follow the PHP 8.4 internal signatures. Only operands implemented by
    // the native overload are listed; unsupported API options remain errors.
    fn parameter_names(self) -> &'static [&'static str] {
        use Builtin::*;
        match self {
            IsInt | IsFloat | IsBool | IsString | IsArray | IsObject | IsNull | IsScalar
            | Boolval | Intval | Floatval | Strval | Count => &["value"],
            Abs => &["num"],
            Strlen | Strtoupper | Strtolower | Strrev | Ucfirst | Lcfirst | Ucwords
            | Addslashes | Stripslashes | Bin2hex | Base64Encode | Urlencode | Rawurlencode
            | Urldecode | Rawurldecode => &["string"],
            Trim | Ltrim | Rtrim => &["string", "characters"],
            Substr => &["string", "offset", "length"],
            StrRepeat => &["string", "times"],
            StrReplace => &["search", "replace", "subject"],
            Implode => &["separator", "array"],
            Explode => &["separator", "string", "limit"],
            StrContains | StrStartsWith | StrEndsWith => &["haystack", "needle"],
            StrSplit => &["string", "length"],
            ArrayValues => &["array"],
            ArrayReverse => &["array", "preserve_keys"],
            ArrayMerge => &[],
            ArraySlice => &["array", "offset", "length", "preserve_keys"],
            ArrayPad => &["array", "length", "value"],
            ArrayFill => &["start_index", "count", "value"],
            ArrayKeyExists => &["key", "array"],
            StrPad => &["string", "length", "pad_string", "pad_type"],
            SubstrCount => &["haystack", "needle", "offset", "length"],
            Sprintf | Printf => &["format"],
        }
    }
    fn parameter_default(self, index: usize, span: Span) -> Option<ast::Expr> {
        use Builtin::*;
        Some(match (self, index) {
            (Trim | Ltrim | Rtrim, 1) => ast::Expr::StrLit(b" \n\r\t\x0b\0".to_vec(), span),
            (Substr, 2) | (ArraySlice, 2) | (SubstrCount, 3) => ast::Expr::IntLit(0, span),
            (ArrayReverse, 1) | (ArraySlice, 3) => ast::Expr::BoolLit(false, span),
            (StrPad, 2) => ast::Expr::StrLit(b" ".to_vec(), span),
            (StrPad, 3) | (StrSplit, 1) => ast::Expr::IntLit(1, span),
            (Explode, 2) => ast::Expr::IntLit(i64::MAX, span),
            (SubstrCount, 2) => ast::Expr::IntLit(0, span),
            (Implode, 1) => ast::Expr::NullLit(span),
            _ => return None,
        })
    }
}
impl Analyzer {
    pub(super) fn infer_named_builtin(
        &mut self,
        kind: Builtin,
        callee: &str,
        callee_span: Span,
        args: &[ast::Expr],
        span: Span,
        ctx: &mut FnCtx,
    ) -> Option<TExpr> {
        let names = kind.parameter_names();
        let mut last = 0;
        let mut position = 0;
        for argument in args {
            let index = if let ast::Expr::NamedArgument { name, span, .. } = argument {
                let Some(index) = names.iter().position(|n| *n == name) else {
                    self.diags.emit(
                        Diagnostic::error(format!(
                            "unknown or unsupported named argument `{name}` for `{callee}`"
                        ))
                        .with_code("E0312")
                        .with_span(*span),
                    );
                    return None;
                };
                index
            } else {
                let index = position;
                position += 1;
                index
            };
            last = last.max(index + 1);
        }
        let declarations: Vec<_> = names
            .iter()
            .take(last)
            .enumerate()
            .map(|(index, name)| ast::Param {
                variadic: false,
                name: (*name).into(),
                name_span: span,
                span,
                ty: None,
                default: kind.parameter_default(index, span),
            })
            .collect();
        let binding = self.bind_arguments(args, &declarations, span, "E0233")?;
        let mut expression = self.infer_builtin_order(
            kind,
            callee,
            callee_span,
            &binding.values,
            span,
            ctx,
            Some(&binding.order),
        )?;
        if kind == Builtin::ArraySlice && binding.present.len() > 2 && !binding.present[2] {
            if let TExpr::Call { args, .. } = &mut expression {
                args[2] = TExpr::Missing(Ty::Int);
            }
        }
        Some(binding.wrap(expression, 0, false))
    }
}
