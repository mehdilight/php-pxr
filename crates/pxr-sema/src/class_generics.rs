//! Demand-driven AST specialization for generic classes. Concrete declarations
//! enter the ordinary class checker, preserving its layout and dispatch rules.
use super::*;

type Bindings = HashMap<String, ast::TypeRef>;

pub(super) struct ClassExpander {
    templates: HashMap<String, ast::ClassDecl>,
    names: HashMap<String, String>,
    instances: HashMap<String, (String, Vec<ast::TypeRef>)>,
    pending: VecDeque<(ast::ClassDecl, Bindings)>,
    declarations: Vec<ast::ClassDecl>,
    pub diags: Diagnostics,
}

pub(super) fn type_ref(ty: &Ty) -> ast::TypeRef {
    use ast::TypeRef as R;
    match ty {
        Ty::Void => R::Void,
        Ty::Int => R::Int,
        Ty::Float => R::Float,
        Ty::Bool => R::Bool,
        Ty::String => R::String,
        Ty::Mixed => R::Mixed,
        Ty::Null => R::EmptyElement,
        Ty::Array(element) => R::Array(Box::new(type_ref(element))),
        Ty::Named(name) => R::Named(name.clone()),
    }
}

pub(super) fn concrete_type(ty: &ast::TypeRef) -> Ty {
    use ast::TypeRef as R;
    match ty {
        R::Void => Ty::Void,
        R::Int => Ty::Int,
        R::Float => Ty::Float,
        R::Bool => Ty::Bool,
        R::String => Ty::String,
        R::Mixed => Ty::Mixed,
        R::EmptyElement => Ty::Null,
        R::Array(element) => Ty::array(concrete_type(element)),
        R::Named(name) => Ty::Named(name.clone()),
        _ => unreachable!("class instance arguments are concrete"),
    }
}

fn closed(ty: &ast::TypeRef) -> bool {
    match ty {
        ast::TypeRef::Param(..) => false,
        ast::TypeRef::Array(element) => closed(element),
        ast::TypeRef::Applied(_, args, _) => args.iter().all(closed),
        _ => true,
    }
}

impl ClassExpander {
    pub fn new(program: &ast::Program) -> Self {
        let mut this = Self {
            templates: HashMap::new(),
            names: HashMap::new(),
            instances: HashMap::new(),
            pending: VecDeque::new(),
            declarations: Vec::new(),
            diags: Diagnostics::new(),
        };
        for item in &program.items {
            if let ast::Stmt::ClassDecl(class) = item {
                let key = class.name.to_ascii_lowercase();
                if this.names.insert(key.clone(), class.name.clone()).is_some() {
                    this.error(
                        "E0201",
                        class.name_span,
                        format!("class `{}` is already defined", class.name),
                    );
                }
                if !class.type_params.is_empty() {
                    this.templates.insert(key, (**class).clone());
                }
            }
        }
        this
    }

    fn error(&mut self, code: &'static str, span: Span, message: String) {
        self.diags
            .emit(Diagnostic::error(message).with_code(code).with_span(span));
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn arguments(&self, name: &str, template: &str) -> Option<&[ast::TypeRef]> {
        let (base, args) = self.instances.get(&name.to_ascii_lowercase())?;
        (base == &template.to_ascii_lowercase()).then_some(args.as_slice())
    }

    /// Validate template signatures without inventing a mixed instantiation.
    pub fn validate_type(&mut self, ty: &ast::TypeRef) {
        match ty {
            ast::TypeRef::Applied(name, args, span) => {
                match self.templates.get(&name.to_ascii_lowercase()) {
                    Some(template) if template.type_params.len() == args.len() => {}
                    Some(template) => self.error(
                        "E0280",
                        *span,
                        format!(
                            "generic class `{name}` expects {} type argument(s), found {}",
                            template.type_params.len(),
                            args.len()
                        ),
                    ),
                    None => self.error("E0280", *span, format!("`{name}` is not a generic class")),
                }
                for argument in args {
                    if matches!(argument, ast::TypeRef::Void) {
                        self.error(
                            "E0280",
                            *span,
                            "void cannot be a generic class type argument".into(),
                        );
                    }
                    self.validate_type(argument);
                }
            }
            ast::TypeRef::Array(element) => {
                if matches!(**element, ast::TypeRef::Void) {
                    self.error(
                        "E0205",
                        element.span(),
                        "an array cannot have element type `void`".into(),
                    );
                }
                self.validate_type(element);
            }
            ast::TypeRef::Named(name) if !self.names.contains_key(&name.to_ascii_lowercase()) => {
                self.error("E0205", ty.span(), format!("unknown type `{name}`"));
            }
            ast::TypeRef::Named(name)
                if self.templates.contains_key(&name.to_ascii_lowercase()) =>
            {
                self.error(
                    "E0280",
                    ty.span(),
                    format!("generic class `{name}` requires explicit type arguments"),
                );
            }
            _ => {}
        }
    }

    pub fn rewrite_type(&mut self, ty: &mut ast::TypeRef, bindings: &Bindings) {
        use ast::TypeRef as R;
        match ty {
            R::Param(name, _) => {
                if let Some(value) = bindings.get(name) {
                    *ty = value.clone();
                }
            }
            R::Array(element) => self.rewrite_type(element, bindings),
            R::Applied(name, args, span) => {
                for argument in args.iter_mut() {
                    self.rewrite_type(argument, bindings);
                }
                if args.iter().all(closed) {
                    let name = name.clone();
                    let args = args.clone();
                    let span = *span;
                    if let Some(instance) = self.instantiate(&name, args, span) {
                        *ty = R::Named(instance);
                    }
                }
            }
            R::Named(name) => {
                let key = name.to_ascii_lowercase();
                if self.templates.contains_key(&key) {
                    self.error(
                        "E0280",
                        Span::synthetic(),
                        format!("generic class `{name}` requires explicit type arguments"),
                    );
                } else if let Some(canonical) = self.names.get(&key) {
                    *name = canonical.clone();
                }
            }
            _ => {}
        }
    }

    fn instantiate(&mut self, name: &str, args: Vec<ast::TypeRef>, span: Span) -> Option<String> {
        if self.diags.has_errors() {
            return None;
        }
        let template = match self.templates.get(&name.to_ascii_lowercase()).cloned() {
            Some(template) => template,
            None => {
                self.error("E0280", span, format!("`{name}` is not a generic class"));
                return None;
            }
        };
        let before = self.diags.items().len();
        self.validate_type(&ast::TypeRef::Applied(name.into(), args.clone(), span));
        if self.diags.items().len() != before {
            return None;
        }
        let display = format!(
            "{}<{}>",
            template.name,
            args.iter()
                .map(|argument| concrete_type(argument).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        // A recursion such as Grow<Pair<T,T>> doubles its type name each
        // step; a count limit alone cannot bound its memory consumption.
        if display.len() > 16_384 {
            self.error(
                "E0283",
                span,
                "generic class type name exceeds the 16384-byte expansion limit".into(),
            );
            return None;
        }
        let key = display.to_ascii_lowercase();
        if self.instances.contains_key(&key) {
            return Some(display);
        }
        if self.instances.len() >= 256
            || self
                .instances
                .values()
                .filter(|(base, _)| base == &name.to_ascii_lowercase())
                .count()
                >= 64
        {
            self.error(
                "E0283",
                span,
                format!("generic class specialization limit exceeded for `{name}`"),
            );
            return None;
        }
        let bindings = template
            .type_params
            .iter()
            .zip(&args)
            .map(|((name, _), argument)| (name.clone(), argument.clone()))
            .collect();
        self.instances
            .insert(key.clone(), (template.name.to_ascii_lowercase(), args));
        self.names.insert(key, display.clone());
        let mut declaration = template;
        declaration.name = display.clone();
        declaration.type_params.clear();
        self.pending.push_back((declaration, bindings));
        Some(display)
    }

    pub fn program(&mut self, original: &ast::Program) -> ast::Program {
        let mut program = original.clone();
        program.items.retain(
            |item| !matches!(item, ast::Stmt::ClassDecl(class) if !class.type_params.is_empty()),
        );
        for item in &mut program.items {
            self.statement(item, &Bindings::new());
        }
        while let Some((mut declaration, bindings)) = self.pending.pop_front() {
            self.class(&mut declaration, &bindings);
            self.declarations.push(declaration);
            if self.diags.has_errors() {
                break;
            }
        }
        program.items.extend(
            self.declarations
                .iter()
                .cloned()
                .map(|class| ast::Stmt::ClassDecl(Box::new(class))),
        );
        program
    }

    pub fn function(&mut self, function: &mut ast::FnDecl, bindings: &Bindings) {
        for parameter in &mut function.params {
            self.annotation(&mut parameter.ty, bindings);
        }
        self.annotation(&mut function.ret, bindings);
        for statement in &mut function.body {
            self.statement(statement, bindings);
        }
    }

    fn annotation(&mut self, ty: &mut Option<ast::TypeRef>, bindings: &Bindings) {
        if let Some(ty) = ty {
            self.rewrite_type(ty, bindings);
        }
    }

    fn class(&mut self, class: &mut ast::ClassDecl, bindings: &Bindings) {
        for reference in class.extends.iter_mut().chain(class.interfaces.iter_mut()) {
            self.class_reference(
                &mut reference.name,
                &mut reference.type_args,
                reference.span,
                bindings,
            );
        }
        for member in &mut class.members {
            match member {
                ast::ClassMember::Property(property) => {
                    self.annotation(&mut property.ty, bindings);
                    if let Some(default) = &mut property.default {
                        self.expression(default, bindings);
                    }
                }
                ast::ClassMember::Method(method) => {
                    for parameter in &mut method.params {
                        self.annotation(&mut parameter.ty, bindings);
                    }
                    self.annotation(&mut method.ret, bindings);
                    for statement in &mut method.body {
                        self.statement(statement, bindings);
                    }
                }
            }
        }
    }

    fn class_reference(
        &mut self,
        name: &mut String,
        args: &mut Vec<ast::TypeRef>,
        span: Span,
        bindings: &Bindings,
    ) {
        let mut reference = if args.is_empty() {
            ast::TypeRef::Named(name.clone())
        } else {
            ast::TypeRef::Applied(name.clone(), args.clone(), span)
        };
        self.rewrite_type(&mut reference, bindings);
        match reference {
            ast::TypeRef::Named(concrete) => {
                *name = concrete;
                args.clear();
            }
            ast::TypeRef::Applied(_, arguments, _) => {
                *args = arguments;
            }
            _ => unreachable!(),
        }
    }

    fn target(&mut self, target: &mut ast::LValue, bindings: &Bindings) {
        match target {
            ast::LValue::Var { .. } => {}
            ast::LValue::Index { base, index, .. } => {
                self.target(base, bindings);
                if let Some(index) = index {
                    self.expression(index, bindings);
                }
            }
            ast::LValue::Property { base, .. } => self.expression(base, bindings),
            ast::LValue::StaticProperty {
                class_name,
                type_args,
                span,
                ..
            } => self.class_reference(class_name, type_args, *span, bindings),
        }
    }

    fn statement(&mut self, statement: &mut ast::Stmt, bindings: &Bindings) {
        use ast::Stmt as S;
        match statement {
            S::FnDecl(function) => {
                // Function templates are rewritten with concrete bindings only
                // when instantiated, matching their lazy body-checking model.
                if !function.type_params.is_empty() {
                    return;
                }
                let mut scope = bindings.clone();
                for (name, _) in &function.type_params {
                    scope.remove(name);
                }
                self.function(function, &scope);
                return;
            }
            S::ClassDecl(class) => {
                self.class(class, bindings);
                return;
            }
            S::Assign {
                target, ty, value, ..
            } => {
                self.target(target, bindings);
                self.annotation(ty, bindings);
                self.expression(value, bindings);
            }
            S::AssignOp { target, value, .. } => {
                self.target(target, bindings);
                self.expression(value, bindings);
            }
            S::IncDec { target, .. } => self.target(target, bindings),
            S::Echo(values, _) => {
                for value in values {
                    self.expression(value, bindings);
                }
            }
            S::Print(value, _) | S::Expr(value, _) => self.expression(value, bindings),
            S::Return(value, _) => {
                if let Some(value) = value {
                    self.expression(value, bindings);
                }
            }
            S::If { cond, elifs, .. } => {
                self.expression(cond, bindings);
                for (cond, _) in elifs {
                    self.expression(cond, bindings);
                }
            }
            S::While { cond, .. } | S::DoWhile { cond, .. } => self.expression(cond, bindings),
            S::For { cond, .. } => {
                for cond in cond {
                    self.expression(cond, bindings);
                }
            }
            S::Foreach { subject, .. } => self.expression(subject, bindings),
            S::Switch { subject, cases, .. } => {
                self.expression(subject, bindings);
                for case in cases {
                    if let Some(value) = &mut case.test {
                        self.expression(value, bindings);
                    }
                }
            }
            S::Require { .. }
            | S::PhpInclude { .. }
            | S::Break(..)
            | S::Continue(..)
            | S::Block(..)
            | S::Empty => {}
        }
        for body in statement.bodies_mut() {
            for child in body {
                self.statement(child, bindings);
            }
        }
    }

    fn expression(&mut self, expression: &mut ast::Expr, bindings: &Bindings) {
        use ast::Expr as E;
        match expression {
            E::Unary { expr, .. } | E::Paren(expr, _) | E::Property { base: expr, .. } => {
                self.expression(expr, bindings)
            }
            E::Binary { lhs, rhs, .. } => {
                self.expression(lhs, bindings);
                self.expression(rhs, bindings);
            }
            E::Ternary {
                cond,
                then,
                else_branch,
                ..
            } => {
                self.expression(cond, bindings);
                if let Some(then) = then {
                    self.expression(then, bindings);
                }
                self.expression(else_branch, bindings);
            }
            E::Index { base, index, .. } => {
                self.expression(base, bindings);
                if let Some(index) = index {
                    self.expression(index, bindings);
                }
            }
            E::ArrayLit { items, .. } => {
                for item in items {
                    if let Some(key) = &mut item.key {
                        self.expression(key, bindings);
                    }
                    self.expression(&mut item.value, bindings);
                }
            }
            E::Match { subject, arms, .. } => {
                self.expression(subject, bindings);
                for arm in arms {
                    for cond in &mut arm.conds {
                        self.expression(cond, bindings);
                    }
                    self.expression(&mut arm.result, bindings);
                }
            }
            E::Call {
                type_args, args, ..
            } => {
                for argument in type_args {
                    self.rewrite_type(argument, bindings);
                }
                for argument in args {
                    self.expression(argument, bindings);
                }
            }
            E::New {
                class_name,
                type_args,
                args,
                class_span: span,
                ..
            }
            | E::StaticCall {
                class_name,
                type_args,
                args,
                span,
                ..
            } => {
                self.class_reference(class_name, type_args, *span, bindings);
                for argument in args {
                    self.expression(argument, bindings);
                }
            }
            E::StaticProperty {
                class_name,
                type_args,
                span,
                ..
            } => self.class_reference(class_name, type_args, *span, bindings),
            E::Instanceof {
                expr,
                class_name,
                type_args,
                class_span,
                ..
            } => {
                self.expression(expr, bindings);
                self.class_reference(class_name, type_args, *class_span, bindings);
            }
            E::MethodCall { base, args, .. } => {
                self.expression(base, bindings);
                for argument in args {
                    self.expression(argument, bindings);
                }
            }
            E::ParentMethodCall { args, .. } => {
                for argument in args {
                    self.expression(argument, bindings);
                }
            }
            E::IntLit(..)
            | E::FloatLit(..)
            | E::StrLit(..)
            | E::BoolLit(..)
            | E::NullLit(..)
            | E::Var(..) => {}
        }
    }
}
