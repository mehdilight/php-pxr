//! Affine source contracts, checked before generic specialization/type erasure.
//! Borrows are call-scoped: no reference-valued locals, fields, or returns.
mod representation;
use pxr_ast::{self as a, Expr, LValue, Stmt, TypeRef};
use pxr_diag::{Diagnostic, Diagnostics, Span};
pub(super) use representation::check as check_representation;
use std::collections::{BTreeMap, BTreeSet};

const TASK: &str = "$pxr_task";
const JOIN: &str = "$pxr_join";
const ANY_TASK: &str = "$pxr_any_task";
fn task_result(t: Option<&TypeRef>) -> Option<&TypeRef> {
    match t {
        Some(TypeRef::Applied(name, args, _))
            if [TASK, JOIN, ANY_TASK].contains(&name.as_str()) && args.len() == 1 =>
        {
            Some(&args[0])
        }
        _ => None,
    }
}
fn task_type(result: TypeRef, span: Span, spawned: bool) -> TypeRef {
    TypeRef::Applied(if spawned { JOIN } else { TASK }.into(), vec![result], span)
}
const OWN: &str = "$pxr_owned";
const READ: &str = "$pxr_borrow";
const WRITE: &str = "$pxr_borrow_mut";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Shared,
    Owned,
    Read,
    Write,
    Transfer,
    Conflict,
}
fn mode(t: Option<&TypeRef>) -> Mode {
    match t {
        Some(TypeRef::Applied(n, _, _)) if n == OWN => Mode::Owned,
        Some(TypeRef::Applied(n, _, _)) if n == READ => Mode::Read,
        Some(TypeRef::Applied(n, _, _)) if n == WRITE => Mode::Write,
        _ => Mode::Shared,
    }
}
fn inner(t: &TypeRef) -> TypeRef {
    match t {
        TypeRef::Applied(n, ts, _) if [OWN, READ, WRITE].contains(&n.as_str()) && ts.len() == 1 => {
            inner(&ts[0])
        }
        _ => t.clone(),
    }
}
fn nominal(t: Option<&TypeRef>) -> Option<String> {
    match t.map(inner) {
        Some(TypeRef::Named(n) | TypeRef::Applied(n, _, _)) => Some(n.to_ascii_lowercase()),
        _ => None,
    }
}
fn result_ownership_compatible(expected: &TypeRef, actual: &TypeRef) -> bool {
    let expected = inner(expected);
    let actual = inner(actual);
    match (&expected, &actual) {
        (TypeRef::Applied(a, xs, _), TypeRef::Applied(b, ys, _))
            if a.rsplit('\\')
                .next()
                .unwrap_or(a)
                .eq_ignore_ascii_case("TaskResult")
                && b.rsplit('\\')
                    .next()
                    .unwrap_or(b)
                    .eq_ignore_ascii_case("TaskResult")
                && xs.len() == 1
                && ys.len() == 1 =>
        {
            mode(Some(&xs[0])) == mode(Some(&ys[0])) && result_ownership_compatible(&xs[0], &ys[0])
        }
        _ => true,
    }
}
fn copy_safe(t: Option<&TypeRef>) -> bool {
    if let Some(TypeRef::Union(members, _)) = t.map(inner) {
        return members.iter().all(|t| copy_safe(Some(t)));
    }
    if matches!(
        t,
        Some(TypeRef::LiteralNull | TypeRef::LiteralFalse | TypeRef::LiteralTrue)
    ) {
        return true;
    }
    matches!(
        t.map(inner),
        Some(TypeRef::Int | TypeRef::Float | TypeRef::Bool | TypeRef::String | TypeRef::Void)
    )
}
#[derive(Clone, PartialEq)]
struct Signature {
    param_names: Vec<String>,
    is_static: bool,
    is_async: bool,
    type_params: Vec<String>,
    params: Vec<Option<TypeRef>>,
    ret: Option<TypeRef>,
}
#[derive(Clone, Debug, PartialEq)]
struct Binding {
    mode: Mode,
    ty: Option<TypeRef>,
    alive: bool,
}
type Env = BTreeMap<String, Binding>;
#[derive(Clone)]
struct Value {
    mode: Mode,
    ty: Option<TypeRef>,
    fresh: bool,
    origins: BTreeSet<String>,
}
fn call_result(s: Signature, bindings: &BTreeMap<String, TypeRef>, span: Span) -> Value {
    let ret = s.ret.as_ref().map(|t| substitute(t, bindings));
    if s.is_async {
        Value {
            mode: Mode::Transfer,
            ty: Some(task_type(ret.unwrap_or(TypeRef::Mixed), span, false)),
            fresh: false,
            origins: BTreeSet::new(),
        }
    } else {
        Value {
            mode: if mode(ret.as_ref()) == Mode::Owned {
                Mode::Transfer
            } else {
                Mode::Shared
            },
            ty: ret.as_ref().map(inner),
            fresh: false,
            origins: BTreeSet::new(),
        }
    }
}
impl Value {
    fn plain(ty: Option<TypeRef>) -> Self {
        Self {
            mode: Mode::Shared,
            ty,
            fresh: false,
            origins: BTreeSet::new(),
        }
    }
}
#[derive(Clone, Copy)]
enum Exit {
    Break(u32),
    Continue(u32),
}
struct Checker {
    signatures: BTreeMap<String, Signature>,
    classes: BTreeMap<String, a::ClassDecl>,
    audited: BTreeSet<String>,
    diags: Diagnostics,
    env: Env,
    loans: Vec<(String, Mode)>,
    exits: Vec<(Exit, Env)>,
    ret: Option<TypeRef>,
    class: Option<String>,
    borrow_argument: Option<Span>,
    task_drops: BTreeSet<Span>,
    return_types: Vec<Option<TypeRef>>,
}

pub fn check_and_erase(program: &a::Program) -> (a::Program, Diagnostics) {
    let mut c = Checker {
        signatures: BTreeMap::new(),
        classes: BTreeMap::new(),
        audited: BTreeSet::new(),
        diags: Diagnostics::new(),
        env: Env::new(),
        loans: vec![],
        exits: vec![],
        ret: None,
        class: None,
        borrow_argument: None,
        task_drops: BTreeSet::new(),
        return_types: Vec::new(),
    };
    for s in &program.items {
        match s {
            Stmt::FnDecl(f) => {
                if [
                    "move",
                    "drop",
                    "sleepasync",
                    "tcpexchangeasync",
                    "tcpserveasync",
                    "httpserveasync",
                    "http_response_code",
                    "header",
                    "httprequestbody",
                    "canceltask",
                    "stopserver",
                    "tcpconnectasync",
                    "tcplistenasync",
                    "tcpacceptasync",
                    "tcpreadasync",
                    "tcpwriteasync",
                    "closesocket",
                    "tcpshutdownwrite",
                    "taskresultasync",
                    "resultvalueasync",
                    "resultok",
                    "resulterror",
                    "resultstatus",
                    "parallelasync",
                    "channelasync",
                    "channelpeerasync",
                    "channelcloneasync",
                    "channelsendasync",
                    "channelreceiveasync",
                    "closechannel",
                ]
                .contains(
                    &f.name
                        .rsplit('\\')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                ) {
                    c.error(
                        f.name_span,
                        "move/drop and native async/HTTP intrinsics are reserved",
                    );
                }
                c.signatures.insert(
                    f.name.to_ascii_lowercase(),
                    Signature {
                        param_names: f.params.iter().map(|p| p.name.clone()).collect(),
                        is_static: true,
                        is_async: f.is_async,
                        type_params: f.type_params.iter().map(|(n, _)| n.clone()).collect(),
                        params: f.params.iter().map(|p| p.ty.clone()).collect(),
                        ret: f.ret.clone(),
                    },
                );
            }
            Stmt::ClassDecl(class) => {
                c.classes
                    .insert(class.name.to_ascii_lowercase(), *class.clone());
            }
            _ => {}
        }
    }
    for class in c.classes.values() {
        for member in &class.members {
            if let a::ClassMember::Method(m) = member {
                c.signatures.insert(
                    format!(
                        "{}::{}",
                        class.name.to_ascii_lowercase(),
                        m.name.to_ascii_lowercase()
                    ),
                    Signature {
                        param_names: m.params.iter().map(|p| p.name.clone()).collect(),
                        is_static: m.is_static,
                        is_async: m.is_async,
                        type_params: m.type_params.iter().map(|(n, _)| n.clone()).collect(),
                        params: m.params.iter().map(|p| p.ty.clone()).collect(),
                        ret: m.ret.clone(),
                    },
                );
            }
        }
    }
    // Erasing qualifiers must not erase a virtual ownership contract, including
    // inherited implementations of interfaces and multiple interface parents.
    for class in c.classes.values().cloned().collect::<Vec<_>>() {
        let mut queue = vec![class.name.to_ascii_lowercase()];
        let mut seen = BTreeSet::new();
        let mut contracts: BTreeMap<String, Vec<Mode>> = BTreeMap::new();
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(decl) = c.classes.get(&name).cloned() else {
                continue;
            };
            queue.extend(
                decl.extends
                    .iter()
                    .chain(decl.interfaces.iter())
                    .map(|p| p.name.to_ascii_lowercase()),
            );
            for member in &decl.members {
                if let a::ClassMember::Method(m) = member {
                    if m.visibility == a::Visibility::Private
                        || m.name.eq_ignore_ascii_case("__construct")
                    {
                        continue;
                    }
                    let mut modes: Vec<_> = m.params.iter().map(|p| mode(p.ty.as_ref())).collect();
                    modes.push(mode(m.ret.as_ref()));
                    let key = m.name.to_ascii_lowercase();
                    if let Some(previous) = contracts.get(&key) {
                        if previous != &modes
                            && previous.iter().chain(&modes).any(|m| *m != Mode::Shared)
                        {
                            c.error(class.span, "override/interface implementation must preserve parameter and return ownership modes");
                        }
                    } else {
                        contracts.insert(key, modes);
                    }
                }
            }
        }
    }
    let mut inference = Vec::new();
    for item in &program.items {
        match item {
            Stmt::FnDecl(f) if f.ret.is_none() => inference.push((
                f.name.to_ascii_lowercase(),
                f.params.clone(),
                f.body.clone(),
                None,
            )),
            Stmt::ClassDecl(class) => {
                for member in &class.members {
                    if let a::ClassMember::Method(m) = member {
                        if m.ret.is_none() {
                            inference.push((
                                format!(
                                    "{}::{}",
                                    class.name.to_ascii_lowercase(),
                                    m.name.to_ascii_lowercase()
                                ),
                                m.params.clone(),
                                m.body.clone(),
                                if m.is_static {
                                    None
                                } else {
                                    Some(class.name.clone())
                                },
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let saved_diags = std::mem::take(&mut c.diags);
    for _ in 0..64 {
        let mut changed = false;
        for (key, params, body, class) in &inference {
            c.function(params, None, body, class.as_deref(), false);
            let result = if c.return_types.is_empty() {
                Some(TypeRef::Void)
            } else if c.return_types.iter().any(Option::is_none) {
                None
            } else {
                let first = c.return_types[0].clone().unwrap();
                Some(
                    if c.return_types.iter().all(|t| t.as_ref() == Some(&first)) {
                        first
                    } else {
                        TypeRef::Mixed
                    },
                )
            };
            if c.signatures[key].ret != result {
                c.signatures.get_mut(key).unwrap().ret = result;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    c.diags = saved_diags;
    c.audited.clear();
    c.task_drops.clear();
    // Validate qualifier positions even in unused declarations/templates.
    let mut output = program.clone();
    for s in &mut output.items {
        c.validate_stmt_types(s);
    }
    // Check free functions and top level. Fresh owned constructions discover
    // classes whose constructors/methods must preserve their receiver identity.
    for s in &program.items {
        if let Stmt::FnDecl(f) = s {
            if f.is_async {
                for param in &f.params {
                    if matches!(mode(param.ty.as_ref()), Mode::Read | Mode::Write) {
                        c.error(
                            param.span,
                            "call-scoped borrows cannot be captured by lazy async tasks",
                        );
                    }
                }
            }
            c.function(&f.params, f.ret.as_ref(), &f.body, None, false);
        }
    }
    c.env.clear();
    c.ret = None;
    c.class = None;
    c.block(&program.items);
    // Every method checks its explicit parameter contracts, even when unused.
    for class in c.classes.values().cloned().collect::<Vec<_>>() {
        for member in &class.members {
            if let a::ClassMember::Method(m) = member {
                c.function(
                    &m.params,
                    m.ret.as_ref(),
                    &m.body,
                    if m.is_static { None } else { Some(&class.name) },
                    false,
                );
            }
        }
    }
    // Audit receiver identity to a fixed point after all fresh constructions
    // have been discovered, including constructions inside static methods.
    let mut checked = BTreeSet::new();
    loop {
        let next = c.audited.iter().find(|n| !checked.contains(*n)).cloned();
        let Some(name) = next else { break };
        checked.insert(name.clone());
        if let Some(class) = c.classes.get(&name).cloned() {
            if let Some(parent) = &class.extends {
                c.audited.insert(parent.name.to_ascii_lowercase());
            }
            for member in &class.members {
                if let a::ClassMember::Method(m) = member {
                    if !m.is_static {
                        c.function(&m.params, m.ret.as_ref(), &m.body, Some(&class.name), true);
                    }
                }
            }
        }
    }
    for s in &mut output.items {
        rewrite_task_drops(s, &c.task_drops);
        erase_stmt(s);
    }
    (output, c.diags)
}

impl Checker {
    fn error(&mut self, span: Span, msg: impl Into<String>) {
        self.diags
            .emit(Diagnostic::error(msg).with_code("E0290").with_span(span));
    }
    fn validate_type(&mut self, t: &TypeRef, allowed: &[Mode], span: Span) {
        let m = mode(Some(t));
        if m != Mode::Shared {
            if !allowed.contains(&m) {
                self.error(span, "ownership qualifier is not allowed here; borrows are call-scoped parameters and owned values are local/parameter/return bindings");
            }
            if let TypeRef::Applied(_, args, _) = t {
                if args.len() != 1 {
                    self.error(
                        span,
                        "ownership qualifiers require exactly one type argument",
                    );
                }
                for arg in args {
                    self.validate_type(arg, &[], span);
                    if matches!(arg, TypeRef::Void) {
                        self.error(span, "ownership qualifiers require a value type, not void");
                    }
                }
                if m == Mode::Write
                    && args.first().is_some_and(|t| {
                        !matches!(
                            t,
                            TypeRef::Array(_) | TypeRef::Named(_) | TypeRef::Applied(_, _, _)
                        )
                    })
                {
                    self.error(span, "exclusive borrows require a concrete object or array type; borrowed bindings cannot be rebound");
                }
            }
        } else {
            match t {
                TypeRef::Array(t) => self.validate_type(t, &[], span),
                TypeRef::Union(members, _) => {
                    for member in members {
                        self.validate_type(member, &[], span);
                    }
                }
                TypeRef::Applied(n, ts, _) => {
                    for t in ts {
                        self.validate_type(
                            t,
                            if n.rsplit('\\')
                                .next()
                                .unwrap_or(n)
                                .eq_ignore_ascii_case("TaskResult")
                            {
                                &[Mode::Owned]
                            } else {
                                &[]
                            },
                            span,
                        );
                    }
                }
                _ => {}
            }
        }
    }
    fn validate_stmt_types(&mut self, s: &Stmt) {
        match s {
            Stmt::FnDecl(f) => {
                for p in &f.params {
                    if p.default.is_some() && mode(p.ty.as_ref()) != Mode::Shared {
                        self.error(
                            p.span,
                            "owned/borrowed parameters cannot have optional defaults",
                        );
                    }
                    if let Some(t) = &p.ty {
                        self.validate_type(t, &[Mode::Owned, Mode::Read, Mode::Write], p.span);
                    }
                }
                if let Some(t) = &f.ret {
                    self.validate_type(t, &[Mode::Owned], f.span);
                }
                for (_, bs) in &f.bounds {
                    for b in bs {
                        self.validate_type(b, &[], f.span);
                    }
                }
            }
            Stmt::ClassDecl(c) => {
                for member in &c.members {
                    match member {
                        a::ClassMember::Property(p) => {
                            if let Some(t) = &p.ty {
                                self.validate_type(t, &[], p.span);
                            }
                        }
                        a::ClassMember::Method(m) => {
                            if m.name.eq_ignore_ascii_case("__destruct") {
                                self.error(m.span, "native destructors are not supported; use explicit cleanup functions (M6 reclaims memory without user destructor callbacks)");
                            }
                            for p in &m.params {
                                if p.default.is_some() && mode(p.ty.as_ref()) != Mode::Shared {
                                    self.error(
                                        p.span,
                                        "owned/borrowed parameters cannot have optional defaults",
                                    );
                                }
                                if m.is_async
                                    && matches!(mode(p.ty.as_ref()), Mode::Read | Mode::Write)
                                {
                                    self.error(p.span, "call-scoped borrows cannot be captured by lazy async methods");
                                }
                                if let Some(t) = &p.ty {
                                    self.validate_type(
                                        t,
                                        &[Mode::Owned, Mode::Read, Mode::Write],
                                        p.span,
                                    );
                                }
                            }
                            if let Some(t) = &m.ret {
                                self.validate_type(t, &[Mode::Owned], m.span);
                            }
                        }
                    }
                }
            }
            Stmt::Assign {
                ty: Some(t), span, ..
            } => self.validate_type(t, &[Mode::Owned], *span),
            _ => {}
        }
        for body in s.bodies() {
            for child in body {
                self.validate_stmt_types(child);
            }
        }
    }
    fn function(
        &mut self,
        params: &[a::Param],
        ret: Option<&TypeRef>,
        body: &[Stmt],
        class: Option<&str>,
        audited: bool,
    ) {
        self.return_types.clear();
        self.env.clear();
        self.loans.clear();
        self.exits.clear();
        self.ret = ret.cloned();
        self.class = class.map(str::to_owned);
        if let Some(class) = class {
            self.env.insert(
                "this".into(),
                Binding {
                    mode: if audited { Mode::Write } else { Mode::Shared },
                    ty: Some(TypeRef::Named(class.into())),
                    alive: true,
                },
            );
        }
        for p in params {
            if super::request_global(&p.name).is_some()
                || ["_SESSION", "_ENV", "GLOBALS"].contains(&p.name.as_str())
            {
                self.error(p.span, "request globals cannot be shadowed by parameters");
            }
            self.env.insert(
                p.name.clone(),
                Binding {
                    mode: mode(p.ty.as_ref()),
                    ty: p.ty.as_ref().map(inner),
                    alive: true,
                },
            );
        }
        self.block(body);
    }
    fn read(&mut self, name: &str, span: Span) -> Value {
        if super::request_global(name).is_some() {
            return Value::plain(Some(TypeRef::Array(Box::new(TypeRef::Mixed))));
        }
        if let Some(b) = self.env.get(name).cloned() {
            if b.mode == Mode::Conflict {
                self.error(
                    span,
                    format!("`${name}` has incompatible ownership modes across control-flow paths"),
                );
            }
            if !b.alive && b.mode != Mode::Shared {
                self.error(
                    span,
                    format!("use of moved/dropped value `${name}`; reinitialize it before use"),
                );
            }
            if self
                .loans
                .iter()
                .any(|(n, m)| n == name && *m == Mode::Write)
            {
                self.error(
                    span,
                    format!("`${name}` is exclusively borrowed by this call"),
                );
            }
            Value {
                mode: b.mode,
                ty: b.ty,
                fresh: false,
                origins: BTreeSet::from([name.into()]),
            }
        } else {
            Value::plain(None)
        }
    }
    fn consume(&mut self, name: &str, span: Span) -> Value {
        let v = self.read(name, span);
        if v.mode != Mode::Owned {
            self.error(span, "move/drop requires an owned local or parameter");
        }
        if self.loans.iter().any(|(n, _)| n == name) {
            self.error(span, format!("cannot move/drop `${name}` while borrowed"));
        }
        if let Some(b) = self.env.get_mut(name) {
            b.alive = false;
        }
        Value {
            mode: Mode::Transfer,
            ..v
        }
    }
    fn escape(&mut self, v: &Value, span: Span) {
        if task_result(v.ty.as_ref()).is_some() {
            self.error(
                span,
                "task handles cannot escape their creating async scope",
            );
        }
        if matches!(v.mode, Mode::Owned | Mode::Transfer)
            || (matches!(v.mode, Mode::Read | Mode::Write) && !copy_safe(v.ty.as_ref()))
        {
            self.error(span, "owned/borrowed value cannot escape into shared storage; move into an owned binding or use a call-scoped borrow");
        }
    }
    fn join(envs: Vec<Env>) -> Option<Env> {
        let mut it = envs.into_iter();
        let mut result = it.next()?;
        for env in it {
            for (n, b) in &mut result {
                b.alive &= env.get(n).is_some_and(|b| b.alive);
                if let Some(other) = env.get(n) {
                    if task_result(b.ty.as_ref()).is_some()
                        || task_result(other.ty.as_ref()).is_some()
                    {
                        b.ty = join_type(b.ty.as_ref(), other.ty.as_ref());
                    }
                }
                if env.get(n).is_some_and(|other| other.mode != b.mode) {
                    b.mode = Mode::Conflict;
                }
            }
            for (n, mut b) in env {
                if !result.contains_key(&n) {
                    b.alive = false;
                    result.insert(n, b);
                }
            }
        }
        Some(result)
    }
    fn block(&mut self, stmts: &[Stmt]) -> bool {
        for s in stmts {
            if !self.stmt(s) {
                return false;
            }
        }
        true
    }
    fn branch(&mut self, body: &[Stmt], start: &Env) -> Option<Env> {
        self.env = start.clone();
        if self.block(body) {
            Some(self.env.clone())
        } else {
            None
        }
    }
    fn stmt(&mut self, s: &Stmt) -> bool {
        match s {
            Stmt::FnDecl(_)
            | Stmt::ClassDecl(_)
            | Stmt::Empty
            | Stmt::Require { .. }
            | Stmt::PhpInclude { .. } => {}
            Stmt::Echo(es, _) => {
                for e in es {
                    let v = self.expr(e);
                    self.check_stringification(&v, e.span());
                }
            }
            Stmt::Print(e, _) => {
                let v = self.expr(e);
                self.check_stringification(&v, e.span());
            }
            Stmt::Expr(
                Expr::Call {
                    callee,
                    args,
                    span,
                    type_args,
                    ..
                },
                _,
            ) if callee == "$pxr_drop" => {
                if !type_args.is_empty() {
                    self.error(*span, "drop takes no type arguments");
                }
                if args.len() == 1 {
                    if let Expr::Var(n, sp) = unparen(&args[0]) {
                        let v = self.consume(n, *sp);
                        if task_result(v.ty.as_ref()).is_some() {
                            self.task_drops.insert(*span);
                        }
                    } else {
                        self.error(*span, "drop requires one owned variable");
                    }
                } else {
                    self.error(*span, "drop requires one owned variable");
                }
            }
            Stmt::Expr(e, _) => {
                let v = self.expr(e);
                if task_result(v.ty.as_ref()).is_some() && v.mode != Mode::Transfer {
                    self.error(
                        e.span(),
                        "discarding a task variable requires drop($task) or move($task)",
                    );
                }
                if matches!(v.mode, Mode::Read | Mode::Write) {
                    self.error(
                        e.span(),
                        "borrow expressions are only valid as call arguments",
                    );
                }
            }
            Stmt::Assign {
                target,
                ty,
                value,
                span,
            } => {
                let v = self.expr(value);
                if let LValue::Var { name, .. } = target {
                    let old = self.env.get(name).cloned();
                    if old
                        .as_ref()
                        .is_some_and(|b| b.alive && task_result(b.ty.as_ref()).is_some())
                    {
                        self.error(
                            *span,
                            "consume or drop the existing task before reinitializing its binding",
                        );
                    }
                    if task_result(v.ty.as_ref()).is_some() && ty.is_some() {
                        self.error(*span, "task bindings currently use inferred types; explicit task annotations are not supported yet");
                    }
                    if old
                        .as_ref()
                        .is_some_and(|b| task_result(b.ty.as_ref()).is_some())
                        != task_result(v.ty.as_ref()).is_some()
                        && old.is_some()
                    {
                        self.error(
                            *span,
                            "a task binding cannot change between task and ordinary value types",
                        );
                    }
                    if old
                        .as_ref()
                        .is_some_and(|b| matches!(b.mode, Mode::Read | Mode::Write))
                    {
                        self.error(*span, "borrowed parameters cannot be rebound");
                    }
                    if self.loans.iter().any(|(n, _)| n == name) {
                        self.error(*span, "cannot reassign a borrowed owner");
                    }
                    let want = ty.as_ref().map(|t| mode(Some(t))).unwrap_or_else(|| {
                        old.as_ref()
                            .map(|b| b.mode)
                            .unwrap_or(if v.mode == Mode::Transfer {
                                Mode::Owned
                            } else {
                                Mode::Shared
                            })
                    });
                    if let (Some(expected), Some(actual)) = (
                        ty.as_ref()
                            .map(inner)
                            .or_else(|| old.as_ref().and_then(|b| b.ty.clone())),
                        v.ty.as_ref(),
                    ) {
                        if !result_ownership_compatible(&expected, actual) {
                            self.error(*span, "TaskResult payload ownership must be preserved");
                        }
                    }
                    if want == Mode::Owned {
                        if v.mode != Mode::Transfer && !(v.fresh && v.mode == Mode::Shared) {
                            self.error(*span, "owned initialization requires a fresh value or explicit move($owner)");
                        }
                        self.audit_new(value);
                    } else {
                        self.escape(&v, value.span());
                    }
                    self.env.insert(
                        name.clone(),
                        Binding {
                            mode: want,
                            ty: if task_result(v.ty.as_ref()).is_some() {
                                v.ty
                            } else {
                                ty.as_ref()
                                    .map(inner)
                                    .or_else(|| old.as_ref().and_then(|b| b.ty.clone()))
                                    .or(v.ty)
                            },
                            alive: true,
                        },
                    );
                } else {
                    self.target(target, true);
                    self.escape(&v, value.span());
                }
            }
            Stmt::AssignOp {
                target,
                value,
                span,
                ..
            } => {
                self.target(target, true);
                let v = self.expr(value);
                self.escape(&v, *span);
            }
            Stmt::IncDec { target, .. } => {
                self.target(target, true);
            }
            Stmt::ForwardReturn(value, span) => {
                return self.stmt(&Stmt::Return(Some(value.clone()), *span));
            }
            Stmt::Return(value, span) => {
                let want = mode(self.ret.as_ref());
                if let Some(e) = value {
                    let v = self.expr(e);
                    self.return_types.push(v.ty.clone());
                    if let (Some(expected), Some(actual)) = (self.ret.as_ref(), v.ty.as_ref()) {
                        if !result_ownership_compatible(expected, actual) {
                            self.error(*span, "TaskResult payload ownership must be preserved");
                        }
                    }
                    if want == Mode::Owned {
                        if v.mode != Mode::Transfer && !(v.fresh && v.mode == Mode::Shared) {
                            self.error(
                                *span,
                                "owned return requires a fresh value or move($owner)",
                            );
                        }
                        self.audit_new(e);
                    } else {
                        self.escape(&v, *span);
                    }
                } else {
                    self.return_types.push(Some(TypeRef::Void));
                }
                return false;
            }
            Stmt::Block(body, _) => return self.block(body),
            Stmt::If {
                cond,
                then_branch,
                elifs,
                else_branch,
                ..
            } => {
                self.expr(cond);
                let mut next = self.env.clone();
                let mut ends = vec![];
                if let Some(e) = self.branch(then_branch, &next) {
                    ends.push(e);
                }
                for (cond, body) in elifs {
                    self.env = next;
                    self.expr(cond);
                    next = self.env.clone();
                    if let Some(e) = self.branch(body, &next) {
                        ends.push(e);
                    }
                }
                if let Some(body) = else_branch {
                    if let Some(e) = self.branch(body, &next) {
                        ends.push(e);
                    }
                } else {
                    ends.push(next);
                }
                if let Some(env) = Self::join(ends) {
                    self.env = env;
                } else {
                    return false;
                }
            }
            Stmt::While { cond, body, .. } => {
                return self.loop_body(&[], std::slice::from_ref(cond), &[], body, false)
            }
            Stmt::DoWhile { cond, body, .. } => {
                return self.loop_body(&[], std::slice::from_ref(cond), &[], body, true)
            }
            Stmt::For {
                init,
                cond,
                update,
                body,
                ..
            } => return self.loop_body(init, cond, update, body, false),
            Stmt::Foreach {
                subject,
                key,
                value,
                body,
                ..
            } => {
                let v = self.expr(subject);
                if matches!(v.mode, Mode::Read | Mode::Write | Mode::Owned)
                    && !matches!(v.ty, Some(TypeRef::Array(_)))
                {
                    self.error(
                        subject.span(),
                        "cannot iterate an unresolved borrowed value",
                    );
                }
                let ty = match v.ty {
                    Some(TypeRef::Array(t)) => Some(*t),
                    _ => None,
                };
                if self.env.get(value).is_some_and(|b| b.mode != Mode::Shared)
                    || key
                        .as_ref()
                        .is_some_and(|k| self.env.get(k).is_some_and(|b| b.mode != Mode::Shared))
                {
                    self.error(
                        subject.span(),
                        "foreach cannot implicitly rebind an owned or borrowed destination",
                    );
                }
                self.env.insert(
                    value.clone(),
                    Binding {
                        mode: Mode::Shared,
                        ty,
                        alive: true,
                    },
                );
                if let Some(key) = key {
                    self.env.insert(
                        key.clone(),
                        Binding {
                            mode: Mode::Shared,
                            ty: None,
                            alive: true,
                        },
                    );
                }
                return self.loop_body(&[], &[], &[], body, false);
            }
            Stmt::Break(level, _) => {
                self.exits
                    .push((Exit::Break(level.unwrap_or(1)), self.env.clone()));
                return false;
            }
            Stmt::Continue(level, _) => {
                self.exits
                    .push((Exit::Continue(level.unwrap_or(1)), self.env.clone()));
                return false;
            }
            Stmt::Switch { subject, cases, .. } => {
                let v = self.expr(subject);
                self.check_stringification(&v, subject.span());
                let start = self.env.clone();
                let exit_start = self.exits.len();
                let mut fall = None;
                let mut ends = vec![];
                let mut default = false;
                for case in cases {
                    let mut inputs = vec![start.clone()];
                    if let Some(e) = fall.take() {
                        inputs.push(e);
                    }
                    self.env = Self::join(inputs).unwrap();
                    if let Some(test) = &case.test {
                        let v = self.test_without_transfer(test);
                        self.check_stringification(&v, test.span());
                    } else {
                        default = true;
                    }
                    fall = if self.block(&case.body) {
                        Some(self.env.clone())
                    } else {
                        None
                    };
                }
                if let Some(e) = fall {
                    ends.push(e);
                }
                if !default {
                    ends.push(start);
                }
                for (exit, env) in self.exits.split_off(exit_start) {
                    match exit {
                        Exit::Break(1) | Exit::Continue(1) => ends.push(env),
                        Exit::Break(n) => self.exits.push((Exit::Break(n - 1), env)),
                        Exit::Continue(n) => self.exits.push((Exit::Continue(n - 1), env)),
                    }
                }
                if let Some(e) = Self::join(ends) {
                    self.env = e;
                } else {
                    return false;
                }
            }
        }
        true
    }
    fn loop_body(
        &mut self,
        init: &[Stmt],
        cond: &[Expr],
        update: &[Stmt],
        body: &[Stmt],
        once: bool,
    ) -> bool {
        if !self.block(init) {
            return false;
        }
        let start = self.env.clone();
        let exits_at = self.exits.len();
        if !once {
            for e in cond {
                self.expr(e);
            }
        }
        let condition_end = self.env.clone();
        let normal = self.block(body);
        let mut back = vec![];
        if normal {
            back.push(self.env.clone());
        }
        let mut ends = if once { vec![] } else { vec![condition_end] };
        for (exit, env) in self.exits.split_off(exits_at) {
            match exit {
                Exit::Break(1) => ends.push(env),
                Exit::Continue(1) => back.push(env),
                Exit::Break(n) => self.exits.push((Exit::Break(n - 1), env)),
                Exit::Continue(n) => self.exits.push((Exit::Continue(n - 1), env)),
            }
        }
        if let Some(back) = Self::join(back) {
            self.env = back;
            self.block(update);
            for e in cond {
                self.expr(e);
            }
            let after_condition = self.env.clone();
            ends.push(after_condition.clone());
            // Recheck a subsequent iteration: a moved value cannot become
            // available merely because source analysis visited the body once.
            let replay_exits = self.exits.len();
            self.env = after_condition;
            let normal = self.block(body);
            let mut again = if normal {
                vec![self.env.clone()]
            } else {
                vec![]
            };
            for (exit, env) in self.exits.split_off(replay_exits) {
                if matches!(exit, Exit::Continue(1)) {
                    again.push(env);
                }
            }
            if let Some(env) = Self::join(again) {
                self.env = env;
                self.block(update);
                for e in cond {
                    self.expr(e);
                }
            }
        }
        if let Some(env) = Self::join(ends) {
            self.env = env;
            true
        } else {
            self.env = start;
            false
        }
    }
    fn target(&mut self, target: &LValue, write: bool) -> Value {
        match target {
            LValue::Var { name, span } => {
                let v = self.read(name, *span);
                if write && matches!(v.mode, Mode::Read | Mode::Write) {
                    self.error(*span, "borrowed parameter bindings cannot be rebound; mutate array/object contents instead");
                }
                v
            }
            LValue::Index { base, index, span } => {
                let v = self.target(base, false);
                if v.mode == Mode::Read && write {
                    self.error(*span, "cannot mutate through a shared borrow");
                }
                if let Some(e) = index {
                    self.expr(e);
                }
                Value::plain(match v.ty {
                    Some(TypeRef::Array(t)) => Some(*t),
                    _ => None,
                })
            }
            LValue::Property {
                base, name, span, ..
            } => {
                let v = self.expr(base);
                if write && v.mode == Mode::Read {
                    self.error(*span, "cannot mutate through a shared borrow");
                }
                let ty = self.property_type(v.ty.as_ref(), name);
                Value::plain(ty)
            }
            LValue::StaticProperty {
                class_name, name, ..
            } => Value::plain(
                self.property_type(Some(&TypeRef::Named(self.class_name(class_name))), name),
            ),
        }
    }
    fn class_name(&self, name: &str) -> String {
        match name.to_ascii_lowercase().as_str() {
            "self" | "static" => self.class.clone().unwrap_or_else(|| name.into()),
            "parent" => self
                .class
                .as_ref()
                .and_then(|n| self.classes.get(&n.to_ascii_lowercase()))
                .and_then(|c| c.extends.as_ref())
                .map(|c| c.name.clone())
                .unwrap_or_else(|| name.into()),
            _ => name.into(),
        }
    }
    fn class_views(&self, ty: &TypeRef) -> Vec<(a::ClassDecl, BTreeMap<String, TypeRef>)> {
        let mut result = vec![];
        let mut queue = vec![inner(ty)];
        let mut seen = BTreeSet::new();
        while let Some(view) = queue.pop() {
            let Some(name) = nominal(Some(&view)) else {
                continue;
            };
            let name = self.class_name(&name).to_ascii_lowercase();
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(decl) = self.classes.get(&name) else {
                continue;
            };
            let args = match &view {
                TypeRef::Applied(_, args, _) => args.clone(),
                _ => vec![],
            };
            let bindings: BTreeMap<_, _> = decl
                .type_params
                .iter()
                .map(|(n, _)| n.clone())
                .zip(args)
                .collect();
            let mut parents: Vec<_> = decl.extends.iter().chain(decl.interfaces.iter()).collect();
            parents.reverse();
            for parent in parents {
                queue.push(if parent.type_args.is_empty() {
                    TypeRef::Named(parent.name.clone())
                } else {
                    TypeRef::Applied(
                        parent.name.clone(),
                        parent
                            .type_args
                            .iter()
                            .map(|t| substitute(t, &bindings))
                            .collect(),
                        parent.span,
                    )
                });
            }
            result.push((decl.clone(), bindings));
        }
        result
    }
    fn property_type(&self, ty: Option<&TypeRef>, property: &str) -> Option<TypeRef> {
        for (decl, bindings) in self.class_views(ty?) {
            for m in &decl.members {
                if let a::ClassMember::Property(p) = m {
                    if p.name == property {
                        return p.ty.as_ref().map(|t| substitute(t, &bindings));
                    }
                }
            }
        }
        None
    }
    fn class_type(&self, class: &str, args: &[TypeRef], span: Span) -> TypeRef {
        if args.is_empty() {
            if class.eq_ignore_ascii_case("parent") {
                if let Some(parent) = self
                    .class
                    .as_ref()
                    .and_then(|n| self.classes.get(&n.to_ascii_lowercase()))
                    .and_then(|c| c.extends.as_ref())
                {
                    return if parent.type_args.is_empty() {
                        TypeRef::Named(parent.name.clone())
                    } else {
                        TypeRef::Applied(parent.name.clone(), parent.type_args.clone(), parent.span)
                    };
                }
            }
            if class.eq_ignore_ascii_case("self") || class.eq_ignore_ascii_case("static") {
                if let Some(decl) = self
                    .class
                    .as_ref()
                    .and_then(|n| self.classes.get(&n.to_ascii_lowercase()))
                {
                    return view_type(decl, &BTreeMap::new());
                }
            }
            TypeRef::Named(self.class_name(class))
        } else {
            TypeRef::Applied(self.class_name(class), args.to_vec(), span)
        }
    }
    fn method(&self, class: &str, name: &str) -> Option<Signature> {
        self.method_for_type(&self.class_type(class, &[], Span::synthetic()), name)
    }
    fn method_for_type(&self, ty: &TypeRef, name: &str) -> Option<Signature> {
        for (decl, mut bindings) in self.class_views(ty) {
            if let Some(sig) = self.signatures.get(&format!(
                "{}::{}",
                decl.name.to_ascii_lowercase(),
                name.to_ascii_lowercase()
            )) {
                let mut sig = sig.clone();
                // Return inference can carry lexical trait witnesses through
                // factories and nested environments. Resolve them in the source
                // consumer, or in the environment's retained owner type, before
                // checking invariant borrow representations. Keep caller type
                // arguments separate from the callee's witness substitution.
                let owner = bindings
                    .get(a::CLOSURE_TRAIT_SELF)
                    .cloned()
                    .unwrap_or_else(|| view_type(&decl, &bindings));
                let parent = bindings
                    .get(a::CLOSURE_TRAIT_PARENT)
                    .cloned()
                    .or_else(|| {
                        self.class_views(&owner).into_iter().next().and_then(
                            |(owner, owner_bindings)| {
                                owner.extends.as_ref().map(|parent| {
                                    let ty = if parent.type_args.is_empty() {
                                        TypeRef::Named(parent.name.clone())
                                    } else {
                                        TypeRef::Applied(
                                            parent.name.clone(),
                                            parent.type_args.clone(),
                                            parent.span,
                                        )
                                    };
                                    substitute(&ty, &owner_bindings)
                                })
                            },
                        )
                    })
                    .unwrap_or_else(|| {
                        TypeRef::Param(a::CLOSURE_CONSUMER_PARENT.into(), Span::synthetic())
                    });
                let witnesses = BTreeMap::from([
                    (a::CLOSURE_CONSUMER_SELF.into(), owner),
                    (a::CLOSURE_CONSUMER_PARENT.into(), parent),
                ]);
                bindings.retain(|n, _| !sig.type_params.contains(n));
                for t in sig.params.iter_mut().flatten() {
                    *t = substitute(t, &witnesses);
                    *t = substitute(t, &bindings);
                }
                if let Some(t) = &mut sig.ret {
                    *t = substitute(t, &witnesses);
                    *t = substitute(t, &bindings);
                }
                return Some(sig);
            }
        }
        None
    }
    fn closure_bridge_signature(&self, name: &str, is_static: bool) -> Option<Signature> {
        // Before specialization, the environment receiver is a type parameter.
        // These unspellable names identify one generated source body. Trait
        // composition can copy it to several classes: require identical full
        // contracts rather than guessing an ownership mode from the receiver.
        if !name.starts_with("$closure_body$") {
            return None;
        }
        let suffix = format!("::{}", name.to_ascii_lowercase());
        let mut candidates = self
            .signatures
            .iter()
            .filter(|(key, _)| key.ends_with(&suffix))
            .map(|(_, signature)| signature);
        let first = candidates.next()?;
        if first.is_static != is_static || candidates.any(|other| other != first) {
            return None;
        }
        // Semantic analysis still checks the concrete target after expansion.
        Some(first.clone())
    }
    fn audit_new(&mut self, expr: &Expr) {
        match unparen(expr) {
            Expr::New { class_name, .. } => {
                self.audited
                    .insert(self.class_name(class_name).to_ascii_lowercase());
            }
            Expr::Ternary {
                then, else_branch, ..
            } => {
                if let Some(t) = then {
                    self.audit_new(t);
                }
                self.audit_new(else_branch);
            }
            Expr::Match { arms, .. } => {
                for arm in arms {
                    self.audit_new(&arm.result);
                }
            }
            _ => {}
        }
    }
    fn arguments(
        &mut self,
        args: &[Expr],
        signature: Option<&Signature>,
        builtin: bool,
        span: Span,
        stringify: bool,
    ) -> BTreeMap<String, TypeRef> {
        let base = self.loans.len();
        let mut bindings = BTreeMap::new();
        for (position, original) in args.iter().enumerate() {
            let (i, arg) = if let Expr::NamedArgument { name, value, .. } = original {
                (
                    signature
                        .and_then(|s| s.param_names.iter().position(|n| n == name))
                        .unwrap_or(usize::MAX),
                    value.as_ref(),
                )
            } else {
                (position, original)
            };
            let allowed = self.borrow_argument;
            self.borrow_argument = Some(unparen(arg).span());
            let v = self.expr(arg);
            self.borrow_argument = allowed;
            let expected = signature
                .and_then(|s| s.params.get(i))
                .and_then(Option::as_ref);
            if let (Some(expected), Some(actual)) = (expected, v.ty.as_ref()) {
                let pattern = inner(expected);
                let actual = if let Some(target) = nominal(Some(&pattern)) {
                    self.class_views(actual)
                        .into_iter()
                        .find(|(c, _)| c.name.eq_ignore_ascii_case(&target))
                        .map(|(c, b)| view_type(&c, &b))
                        .unwrap_or_else(|| actual.clone())
                } else {
                    actual.clone()
                };
                let mut inferred = BTreeMap::new();
                infer_bindings(&pattern, &actual, &mut inferred);
                for (n, t) in inferred {
                    if signature.is_some_and(|s| s.type_params.contains(&n)) {
                        bindings.entry(n).or_insert(t);
                    }
                }
            }
            let want = mode(expected);
            if let (Some(expected), Some(actual)) = (expected, v.ty.as_ref()) {
                let expected = substitute(expected, &bindings);
                if !result_ownership_compatible(&expected, actual) {
                    self.error(arg.span(), "TaskResult payload ownership must be preserved");
                }
            }
            match want {
                Mode::Owned => {
                    if v.mode != Mode::Transfer && !(v.mode == Mode::Shared && v.fresh) {
                        self.error(
                            arg.span(),
                            "owned parameter requires a fresh value or move($owner)",
                        );
                    }
                    self.audit_new(arg);
                }
                Mode::Read | Mode::Write => {
                    if let Some(expected) = expected {
                        let expected = substitute(&inner(expected), &bindings);
                        if let Some(actual) = v.ty.as_ref() {
                            if !self.borrow_compatible(&expected, actual) {
                                self.error(arg.span(), "borrow types must preserve representation; arrays are invariant and borrows cannot rebox or widen elements");
                            }
                        } else {
                            self.error(
                                arg.span(),
                                "borrowed value needs an explicit concrete type annotation",
                            );
                        }
                    }
                    // A bare borrowed variable is not a new loan. Prefix syntax
                    // is required so aliases stay visible at every call site.
                    let marked = matches!(unparen(arg), Expr::Call { callee, .. } if callee == READ || callee == WRITE);
                    if !marked
                        || (want == Mode::Write && v.mode != Mode::Write)
                        || !matches!(v.mode, Mode::Read | Mode::Write)
                    {
                        self.error(arg.span(), "borrowed parameter requires an explicit &$owner or &mut $owner argument");
                    }
                }
                _ if builtin => {
                    if stringify {
                        self.check_stringification(&v, arg.span());
                    }
                    if matches!(v.mode, Mode::Transfer) {
                        self.error(span, "cannot move an owner into an ordinary builtin");
                    }
                    if matches!(unparen(arg), Expr::Call { callee, .. } if callee == READ || callee == WRITE)
                    {
                        self.error(
                            arg.span(),
                            "explicit borrows require a typed borrowed parameter",
                        );
                    }
                }
                _ => {
                    if matches!(unparen(arg), Expr::Call { callee, .. } if callee == READ || callee == WRITE)
                    {
                        self.error(arg.span(), "explicit borrow arguments require a matching borrowed parameter contract");
                    }
                    self.escape(&v, arg.span());
                }
            }
        }
        self.loans.truncate(base);
        bindings
    }
    fn test_without_transfer(&mut self, e: &Expr) -> Value {
        let before = self.env.clone();
        let value = self.expr(e);
        if before.iter().any(|(n, b)| {
            self.env
                .get(n)
                .is_some_and(|after| b.alive != after.alive || b.mode != after.mode)
        }) {
            self.error(
                e.span(),
                "ownership transfers in switch/match conditions require explicit control flow",
            );
        }
        value
    }
    fn check_stringification(&mut self, value: &Value, span: Span) {
        if task_result(value.ty.as_ref()).is_some() {
            self.error(
                span,
                "task handles cannot be printed or converted to ordinary values",
            );
        }
        let object_possible = matches!(
            value.ty.as_ref(),
            None | Some(
                TypeRef::Named(_)
                    | TypeRef::Applied(_, _, _)
                    | TypeRef::Mixed
                    | TypeRef::Param(_, _)
            )
        );
        if object_possible
            && (value.mode == Mode::Read
                || value
                    .origins
                    .iter()
                    .any(|n| self.loans.iter().any(|(borrowed, _)| n == borrowed)))
        {
            self.error(span, "implicit object stringification/comparison may mutate a borrowed receiver; use an exclusive borrow or read a scalar field");
        }
    }
    fn borrow_compatible(&self, expected: &TypeRef, actual: &TypeRef) -> bool {
        if type_shape(expected) == type_shape(actual) {
            return true;
        }
        // Upcasts preserve the same stable object layout. Generic ancestor
        // arguments must agree as well; array views are strictly invariant.
        if nominal(Some(expected)).is_none() || nominal(Some(actual)).is_none() {
            return false;
        }
        self.class_views(actual)
            .into_iter()
            .any(|(decl, bindings)| {
                let view = if decl.type_params.is_empty() {
                    TypeRef::Named(decl.name)
                } else {
                    TypeRef::Applied(
                        decl.name,
                        decl.type_params
                            .iter()
                            .map(|(n, sp)| {
                                bindings
                                    .get(n)
                                    .cloned()
                                    .unwrap_or_else(|| TypeRef::Param(n.clone(), *sp))
                            })
                            .collect(),
                        decl.span,
                    )
                };
                type_shape(expected) == type_shape(&view)
            })
    }
    fn send_type(&self, ty: &TypeRef, root: bool) -> bool {
        if matches!(mode(Some(ty)), Mode::Read | Mode::Write) {
            return false;
        }
        let owned = mode(Some(ty)) == Mode::Owned;
        match inner(ty) {
            TypeRef::Int
            | TypeRef::Float
            | TypeRef::Bool
            | TypeRef::String
            | TypeRef::Void
            | TypeRef::Mixed
            | TypeRef::Union(_, _)
            | TypeRef::LiteralNull
            | TypeRef::LiteralFalse
            | TypeRef::LiteralTrue => true,
            TypeRef::Array(t) => self.send_type(&t, false),
            TypeRef::Param(_, _) if !root => true,
            TypeRef::Named(ref n) if root && owned && n == "Channel" => true,
            t @ (TypeRef::Named(_) | TypeRef::Applied(_, _, _)) if root && owned => {
                let views = self.class_views(&t);
                !views.is_empty()
                    && views.iter().all(|(class, bindings)| {
                        class.name != "TcpStream"
                            && class.name != "TcpListener"
                            && class.members.iter().all(|m| {
                                if let a::ClassMember::Property(p) = m {
                                    p.is_static
                                        || p.ty.as_ref().is_some_and(|t| {
                                            self.send_type(&substitute(t, bindings), false)
                                        })
                                } else {
                                    true
                                }
                            })
                    })
            }
            _ => false,
        }
    }
    fn expr(&mut self, e: &Expr) -> Value {
        match e {
            Expr::Effect {
                statement, span, ..
            } => {
                self.stmt(statement);
                if let Stmt::Assign {
                    target: LValue::Var { name, .. },
                    ..
                }
                | Stmt::AssignOp {
                    target: LValue::Var { name, .. },
                    ..
                }
                | Stmt::IncDec {
                    target: LValue::Var { name, .. },
                    ..
                } = &**statement
                {
                    if self.env.get(name).is_some_and(|b| b.mode != Mode::Shared) {
                        self.error(*span, "an assignment expression cannot duplicate an owned or borrowed binding");
                    }
                    return Value::plain(self.env.get(name).and_then(|b| b.ty.clone()));
                }
                Value::plain(None)
            }
            Expr::IntLit(..) => Value {
                fresh: true,
                ..Value::plain(Some(TypeRef::Int))
            },
            Expr::FloatLit(..) => Value {
                fresh: true,
                ..Value::plain(Some(TypeRef::Float))
            },
            Expr::BoolLit(..) => Value {
                fresh: true,
                ..Value::plain(Some(TypeRef::Bool))
            },
            Expr::StrLit(..) => Value {
                fresh: true,
                ..Value::plain(Some(TypeRef::String))
            },
            Expr::NullLit(..) => Value {
                fresh: true,
                ..Value::plain(Some(TypeRef::Mixed))
            },
            Expr::Var(n, sp) => self.read(n, *sp),
            Expr::NamedArgument { value: e, .. } | Expr::Paren(e, _) => self.expr(e),
            Expr::Call {
                callee,
                fallback,
                args,
                span,
                ..
            } if fallback
                .as_deref()
                .unwrap_or(callee)
                .eq_ignore_ascii_case("parallelAsync") =>
            {
                let signature = args.first().and_then(|e| match unparen(e) {
                    Expr::StrLit(n, _) => std::str::from_utf8(n)
                        .ok()
                        .and_then(|n| {
                            self.signatures
                                .get(&n.trim_start_matches('\\').to_ascii_lowercase())
                        })
                        .cloned(),
                    _ => None,
                });
                let Some(signature) = signature
                    .filter(|s| s.is_async && s.params.len() == 1 && s.type_params.is_empty())
                else {
                    self.error(*span, "parallelAsync requires a literal non-generic native async function with one Send parameter");
                    return Value::plain(None);
                };
                self.arguments(&args[1..], Some(&signature), false, *span, false);
                if !signature.params[0]
                    .as_ref()
                    .is_some_and(|t| self.send_type(t, true))
                    || !signature
                        .ret
                        .as_ref()
                        .is_some_and(|t| self.send_type(t, true))
                {
                    self.error(*span, "parallelAsync input and result must be Send: values, owned Channel, or moved owned objects containing value fields");
                }
                Value {
                    mode: Mode::Transfer,
                    ty: Some(task_type(
                        signature.ret.unwrap_or(TypeRef::Void),
                        *span,
                        false,
                    )),
                    fresh: false,
                    origins: BTreeSet::new(),
                }
            }
            Expr::Call {
                callee,
                fallback,
                args,
                span,
                ..
            } if [
                "channelasync",
                "channelpeerasync",
                "channelcloneasync",
                "channelsendasync",
                "channelreceiveasync",
                "closechannel",
            ]
            .iter()
            .any(|n| {
                fallback
                    .as_deref()
                    .unwrap_or(callee)
                    .eq_ignore_ascii_case(n)
            }) =>
            {
                let name = fallback.as_deref().unwrap_or(callee).to_ascii_lowercase();
                if name != "channelasync" {
                    if let Some(argument) = args.first() {
                        let value = if name == "closechannel" {
                            if let Expr::Var(n, sp) = unparen(argument) {
                                self.consume(n, *sp)
                            } else {
                                self.expr(argument)
                            }
                        } else {
                            self.expr(argument)
                        };
                        if name == "closechannel" {
                            if value.mode != Mode::Transfer {
                                self.error(*span, "closeChannel consumes a channel owner");
                            }
                        } else if value.mode != Mode::Owned || !self.loans.is_empty() {
                            self.error(*span, "channel operations require an owned channel local; borrows cannot enter tasks");
                        }
                    }
                }
                for argument in args.iter().skip(if name == "channelasync" { 0 } else { 1 }) {
                    let value = self.expr(argument);
                    self.escape(&value, argument.span());
                    if name == "channelsendasync"
                        && !value.ty.as_ref().is_some_and(|t| self.send_type(t, false))
                    {
                        self.error(
                            argument.span(),
                            "channel messages must contain only scalar, string and array values",
                        );
                    }
                }
                if name == "closechannel" {
                    return Value::plain(Some(TypeRef::Void));
                }
                let ret = match name.as_str() {
                    "channelasync" | "channelpeerasync" | "channelcloneasync" => {
                        TypeRef::Applied(OWN.into(), vec![TypeRef::Named("Channel".into())], *span)
                    }
                    "channelsendasync" => TypeRef::Void,
                    _ => TypeRef::Mixed,
                };
                Value {
                    mode: Mode::Transfer,
                    ty: Some(task_type(ret, *span, false)),
                    fresh: false,
                    origins: BTreeSet::new(),
                }
            }
            Expr::Call {
                callee,
                fallback,
                args,
                span,
                ..
            } if [
                "tcpconnectasync",
                "tcplistenasync",
                "tcpacceptasync",
                "tcpreadasync",
                "tcpwriteasync",
                "closesocket",
                "tcpshutdownwrite",
            ]
            .iter()
            .any(|n| {
                fallback
                    .as_deref()
                    .unwrap_or(callee)
                    .eq_ignore_ascii_case(n)
            }) =>
            {
                let name = fallback.as_deref().unwrap_or(callee).to_ascii_lowercase();
                if ["tcpconnectasync", "tcplistenasync"].contains(&name.as_str()) {
                    self.arguments(args, None, false, *span, false);
                } else if let Some(argument) = args.first() {
                    let value = if name == "closesocket" {
                        if let Expr::Var(n, sp) = unparen(argument) {
                            self.consume(n, *sp)
                        } else {
                            self.expr(argument)
                        }
                    } else {
                        self.expr(argument)
                    };
                    if name == "closesocket" {
                        if value.mode != Mode::Transfer {
                            self.error(*span, "closeSocket consumes a socket owner");
                        }
                    } else if value.mode != Mode::Owned || !self.loans.is_empty() {
                        self.error(*span,"async socket operations require an owned socket local; borrowed handles cannot enter tasks");
                    }
                    for argument in args.iter().skip(1) {
                        let value = self.expr(argument);
                        self.escape(&value, argument.span());
                    }
                }
                if ["closesocket", "tcpshutdownwrite"].contains(&name.as_str()) {
                    return Value::plain(Some(TypeRef::Void));
                }
                let ret = match name.as_str() {
                    "tcpconnectasync" | "tcpacceptasync" => TypeRef::Applied(
                        OWN.into(),
                        vec![TypeRef::Named("TcpStream".into())],
                        *span,
                    ),
                    "tcplistenasync" => TypeRef::Applied(
                        OWN.into(),
                        vec![TypeRef::Named("TcpListener".into())],
                        *span,
                    ),
                    "tcpwriteasync" => TypeRef::Int,
                    _ => TypeRef::String,
                };
                Value {
                    mode: Mode::Transfer,
                    ty: Some(task_type(ret, *span, false)),
                    fresh: false,
                    origins: BTreeSet::new(),
                }
            }
            Expr::Call {
                callee,
                fallback,
                args,
                span,
                ..
            } if [
                "taskresultasync",
                "resultvalueasync",
                "resultok",
                "resulterror",
                "resultstatus",
            ]
            .iter()
            .any(|n| {
                fallback
                    .as_deref()
                    .unwrap_or(callee)
                    .eq_ignore_ascii_case(n)
            }) =>
            {
                let name = fallback.as_deref().unwrap_or(callee).to_ascii_lowercase();
                let Some(argument) = args.first() else {
                    return Value::plain(None);
                };
                let consume = name == "taskresultasync" || name == "resultvalueasync";
                let value = if consume {
                    if let Expr::Var(n, sp) = unparen(argument) {
                        self.consume(n, *sp)
                    } else {
                        self.expr(argument)
                    }
                } else {
                    self.expr(argument)
                };
                if consume && value.mode != Mode::Transfer {
                    self.error(
                        *span,
                        "result operations consume a fresh expression or moved owner",
                    );
                }
                if name == "taskresultasync" {
                    let Some(result) = task_result(value.ty.as_ref()).cloned() else {
                        self.error(*span, "taskResultAsync requires a task handle");
                        return Value::plain(None);
                    };
                    let result = TypeRef::Applied("TaskResult".into(), vec![result], *span);
                    return Value {
                        mode: Mode::Transfer,
                        ty: Some(task_type(
                            TypeRef::Applied(OWN.into(), vec![result], *span),
                            *span,
                            false,
                        )),
                        fresh: false,
                        origins: BTreeSet::new(),
                    };
                }
                let payload = match value.ty.as_ref() {
                    Some(TypeRef::Applied(n, ts, _))
                        if n.rsplit('\\')
                            .next()
                            .unwrap_or(n)
                            .eq_ignore_ascii_case("TaskResult")
                            && ts.len() == 1 =>
                    {
                        Some(ts[0].clone())
                    }
                    _ => None,
                };
                let Some(payload) = payload else {
                    self.error(*span, "result operations require a TaskResult");
                    return Value::plain(None);
                };
                if name == "resultvalueasync" {
                    Value {
                        mode: Mode::Transfer,
                        ty: Some(task_type(payload, *span, false)),
                        fresh: false,
                        origins: BTreeSet::new(),
                    }
                } else {
                    Value::plain(Some(if name == "resultok" {
                        TypeRef::Bool
                    } else {
                        TypeRef::Int
                    }))
                }
            }
            Expr::Call {
                callee,
                fallback,
                args,
                span,
                ..
            } if callee == "$pxr_await"
                || callee == "$pxr_spawn"
                || callee.eq_ignore_ascii_case("canceltask")
                || callee.eq_ignore_ascii_case("stopserver")
                || fallback.as_ref().is_some_and(|n| {
                    n.eq_ignore_ascii_case("canceltask") || n.eq_ignore_ascii_case("stopserver")
                }) =>
            {
                if !self.loans.is_empty() {
                    self.error(
                        *span,
                        "await/spawn/cancelTask/stopServer cannot cross an active call-scoped borrow",
                    );
                }
                let Some(arg) = args.first() else {
                    return Value::plain(None);
                };
                let value = if let Expr::Var(name, sp) = unparen(arg) {
                    self.consume(name, *sp)
                } else {
                    self.expr(arg)
                };
                let Some(result) = task_result(value.ty.as_ref()).cloned() else {
                    self.error(
                        *span,
                        "task control requires a task with a known local lifetime",
                    );
                    return Value::plain(None);
                };
                if value.mode != Mode::Transfer {
                    self.error(*span, "task control requires a fresh task expression or an explicit move of a handle selected by an expression");
                }
                for argument in args.iter().skip(1) {
                    self.expr(argument);
                }
                if callee.eq_ignore_ascii_case("canceltask")
                    || callee.eq_ignore_ascii_case("stopserver")
                    || fallback.as_ref().is_some_and(|n| {
                        n.eq_ignore_ascii_case("canceltask") || n.eq_ignore_ascii_case("stopserver")
                    })
                {
                    Value::plain(Some(TypeRef::Void))
                } else if callee == "$pxr_spawn" {
                    if matches!(value.ty, Some(TypeRef::Applied(ref name, _, _)) if name != TASK) {
                        self.error(*span, "spawn requires an unstarted task; a spawned or conditionally started handle cannot be spawned again");
                    }
                    Value {
                        mode: Mode::Transfer,
                        ty: Some(task_type(result, *span, true)),
                        fresh: false,
                        origins: BTreeSet::new(),
                    }
                } else {
                    Value {
                        mode: if mode(Some(&result)) == Mode::Owned {
                            Mode::Transfer
                        } else {
                            Mode::Shared
                        },
                        ty: Some(inner(&result)),
                        fresh: false,
                        origins: BTreeSet::new(),
                    }
                }
            }
            Expr::Call {
                callee,
                args,
                span,
                type_args,
                ..
            } if callee == "$pxr_move" => {
                if !type_args.is_empty() || args.len() != 1 {
                    self.error(
                        *span,
                        "move requires exactly one owned variable and no type arguments",
                    );
                    return Value::plain(None);
                }
                if let Expr::Var(n, sp) = unparen(&args[0]) {
                    self.consume(n, *sp)
                } else {
                    self.error(*span, "move requires an owned variable");
                    Value::plain(None)
                }
            }
            Expr::Call {
                callee, args, span, ..
            } if callee == READ || callee == WRITE => {
                if self.borrow_argument != Some(*span) {
                    self.error(*span, "a borrow must be a direct call argument; borrowed values cannot be stored or returned");
                }
                let m = if callee == READ {
                    Mode::Read
                } else {
                    Mode::Write
                };
                if let [arg] = args.as_slice() {
                    if let Expr::Var(n, sp) = unparen(arg) {
                        let v = self.read(n, *sp);
                        if task_result(v.ty.as_ref()).is_some() {
                            self.error(*span, "task handles cannot be borrowed");
                        }
                        if !matches!(v.mode, Mode::Owned | Mode::Read | Mode::Write) {
                            self.error(
                                *span,
                                "only owned values and borrowed parameters can be borrowed",
                            );
                        }
                        if m == Mode::Write && v.mode == Mode::Read {
                            self.error(*span, "a shared borrow cannot become exclusive");
                        }
                        if self.loans.iter().any(|(other, mode)| {
                            other == n && (m == Mode::Write || *mode == Mode::Write)
                        }) {
                            self.error(*span, "overlapping exclusive/shared borrows in one call");
                        }
                        self.loans.push((n.clone(), m));
                        return Value { mode: m, ..v };
                    }
                }
                self.error(*span, "borrows require a local/parameter variable");
                Value::plain(None)
            }
            Expr::Call {
                callee,
                args,
                span,
                fallback,
                type_args,
                ..
            } => {
                if callee == "$pxr_drop" {
                    self.error(*span, "drop is only valid as a standalone statement");
                    return Value::plain(None);
                }
                for t in type_args {
                    self.validate_type(t, &[], *span);
                }
                let sig = self
                    .signatures
                    .get(&callee.to_ascii_lowercase())
                    .or_else(|| {
                        fallback
                            .as_ref()
                            .and_then(|n| self.signatures.get(&n.to_ascii_lowercase()))
                    })
                    .cloned()
                    .map(|s| specialize_signature(s, type_args));
                let name = fallback.as_deref().unwrap_or(callee).to_ascii_lowercase();
                if [
                    "sleepasync",
                    "tcpexchangeasync",
                    "tcpserveasync",
                    "httpserveasync",
                ]
                .contains(&name.as_str())
                    && sig.is_none()
                {
                    self.arguments(args, None, false, *span, false);
                    return Value {
                        mode: Mode::Transfer,
                        ty: Some(task_type(
                            if name != "tcpexchangeasync" {
                                TypeRef::Void
                            } else {
                                TypeRef::String
                            },
                            *span,
                            false,
                        )),
                        fresh: false,
                        origins: BTreeSet::new(),
                    };
                }
                let builtin = sig.is_none() && super::Builtin::from_name(&name).is_some();
                // Builtins which can retain/return object payloads require normal
                // escape checks. Scalar/string readers cannot retain handles.
                let reading = builtin
                    && matches!(
                        name.as_str(),
                        "strlen"
                            | "count"
                            | "intval"
                            | "floatval"
                            | "boolval"
                            | "strval"
                            | "is_int"
                            | "is_float"
                            | "is_bool"
                            | "is_string"
                            | "is_array"
                            | "is_object"
                            | "is_scalar"
                            | "is_null"
                            | "strtoupper"
                            | "strtolower"
                            | "trim"
                            | "ltrim"
                            | "rtrim"
                            | "substr"
                            | "str_repeat"
                            | "str_replace"
                            | "str_contains"
                            | "str_starts_with"
                            | "str_ends_with"
                            | "strrev"
                            | "ucfirst"
                            | "lcfirst"
                            | "ucwords"
                            | "str_pad"
                            | "substr_count"
                            | "addslashes"
                            | "stripslashes"
                            | "bin2hex"
                            | "base64_encode"
                            | "urlencode"
                            | "rawurlencode"
                            | "urldecode"
                            | "rawurldecode"
                            | "abs"
                    );
                let bindings = self.arguments(args, sig.as_ref(), reading, *span, name == "strval");
                if let Some(s) = sig {
                    call_result(s, &bindings, *span)
                } else {
                    let ty = match name.as_str() {
                        "strlen" | "count" | "intval" | "substr_count" => Some(TypeRef::Int),
                        "floatval" => Some(TypeRef::Float),
                        _ if reading => Some(TypeRef::String),
                        _ => None,
                    };
                    Value {
                        fresh: reading && matches!(ty, Some(TypeRef::String)),
                        ..Value::plain(ty)
                    }
                }
            }
            Expr::New {
                class_name,
                args,
                type_args,
                span,
                ..
            } => {
                for t in type_args {
                    self.validate_type(t, &[], *span);
                }
                let ty = self.class_type(class_name, type_args, *span);
                let sig = self.method_for_type(&ty, "__construct");
                self.arguments(args, sig.as_ref(), false, *span, false);
                Value {
                    fresh: true,
                    ..Value::plain(Some(ty))
                }
            }
            Expr::MethodCall {
                base,
                name,
                args,
                span,
                type_args,
                ..
            } => {
                for t in type_args {
                    self.validate_type(t, &[], *span);
                }
                let receiver = self.expr(base);
                let async_receiver = receiver
                    .ty
                    .as_ref()
                    .and_then(|t| self.method_for_type(t, name))
                    .is_some_and(|s| s.is_async);
                if async_receiver
                    && (matches!(receiver.mode, Mode::Read | Mode::Write | Mode::Owned)
                        || !self.loans.is_empty())
                {
                    self.error(*span, "async receivers must be shared values or moved owners; call-scoped receiver borrows cannot escape into a task");
                }
                if async_receiver && receiver.mode == Mode::Transfer {
                    if let Some(class) = nominal(receiver.ty.as_ref()) {
                        self.audited.insert(class);
                    }
                }
                let base_loans = self.loans.len();
                if receiver.mode == Mode::Read {
                    self.error(*span, "method calls require an exclusive receiver borrow; use a free function for shared reads");
                }
                if matches!(receiver.mode, Mode::Owned | Mode::Write) {
                    if let Some(class) = nominal(receiver.ty.as_ref()) {
                        self.audited.insert(class);
                    } else {
                        self.error(
                            *span,
                            "ownership-aware method calls require a concrete receiver class",
                        );
                    }
                    if let Expr::Var(n, _) = unparen(base) {
                        if self.loans.iter().any(|(o, _)| o == n) {
                            self.error(*span, "receiver is already borrowed by this call");
                        }
                        self.loans.push((n.clone(), Mode::Write));
                    } else {
                        self.error(
                            *span,
                            "ownership-aware method receivers must be local/parameter variables",
                        );
                    }
                }
                let sig = receiver
                    .ty
                    .as_ref()
                    .and_then(|t| self.method_for_type(t, name))
                    .or_else(|| self.closure_bridge_signature(name, false))
                    .map(|s| specialize_signature(s, type_args));
                let bindings = self.arguments(args, sig.as_ref(), false, *span, false);
                self.loans.truncate(base_loans);
                if let Some(s) = sig {
                    call_result(s, &bindings, *span)
                } else {
                    Value::plain(None)
                }
            }
            Expr::StaticCall {
                class_name,
                method,
                args,
                span,
                type_args,
                method_type_args,
                ..
            } => {
                for t in type_args.iter().chain(method_type_args) {
                    self.validate_type(t, &[], *span);
                }
                let sig = self
                    .method_for_type(&self.class_type(class_name, type_args, *span), method)
                    .or_else(|| {
                        if class_name == a::CLOSURE_TRAIT_SELF {
                            self.closure_bridge_signature(method, true)
                        } else {
                            None
                        }
                    })
                    .map(|s| specialize_signature(s, method_type_args));
                if sig.as_ref().is_some_and(|s| s.is_async && !s.is_static)
                    && self.env.get("this").is_some_and(|b| b.mode != Mode::Shared)
                {
                    self.error(
                        *span,
                        "class-qualified async calls cannot capture an owned receiver borrow",
                    );
                }
                let bindings = self.arguments(args, sig.as_ref(), false, *span, false);
                if let Some(s) = sig {
                    call_result(s, &bindings, *span)
                } else {
                    Value::plain(None)
                }
            }
            Expr::ParentMethodCall {
                method,
                args,
                span,
                type_args,
                ..
            } => {
                let sig = self
                    .method("parent", method)
                    .map(|s| specialize_signature(s, type_args));
                if sig.as_ref().is_some_and(|s| s.is_async && !s.is_static)
                    && self.env.get("this").is_some_and(|b| b.mode != Mode::Shared)
                {
                    self.error(
                        *span,
                        "class-qualified async calls cannot capture an owned receiver borrow",
                    );
                }
                let bindings = self.arguments(args, sig.as_ref(), false, *span, false);
                if let Some(s) = sig {
                    call_result(s, &bindings, *span)
                } else {
                    Value::plain(None)
                }
            }
            Expr::Property { base, name, .. } => {
                let v = self.expr(base);
                // Property declarations use ordinary value/shared types. Their
                // contents have independent managed lifetimes; the receiver
                // handle itself is never copied by reading a property.
                Value::plain(self.property_type(v.ty.as_ref(), name))
            }
            Expr::StaticProperty {
                class_name, name, ..
            } => Value::plain(
                self.property_type(Some(&TypeRef::Named(self.class_name(class_name))), name),
            ),
            Expr::Index { base, index, .. } => {
                let v = self.expr(base);
                if let Some(i) = index {
                    self.expr(i);
                }
                Value::plain(match v.ty {
                    Some(TypeRef::Array(t)) => Some(*t),
                    _ => None,
                })
            }
            Expr::ArrayLit { items, .. } => {
                let mut ty = None;
                for item in items {
                    if let Some(k) = &item.key {
                        let v = self.expr(k);
                        self.escape(&v, k.span());
                    }
                    let v = self.expr(&item.value);
                    self.escape(&v, item.value.span());
                    ty = ty.or(v.ty);
                }
                Value {
                    fresh: true,
                    ..Value::plain(Some(TypeRef::Array(Box::new(
                        ty.unwrap_or(TypeRef::EmptyElement),
                    ))))
                }
            }
            Expr::Unary { expr, .. } => {
                let v = self.expr(expr);
                Value {
                    fresh: true,
                    ..Value::plain(v.ty)
                }
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                let left = self.expr(lhs);
                let before = self.env.clone();
                let right = self.expr(rhs);
                if !matches!(
                    op,
                    a::BinOp::Identical
                        | a::BinOp::NotIdentical
                        | a::BinOp::And
                        | a::BinOp::Or
                        | a::BinOp::NullCoalesce
                ) {
                    self.check_stringification(&left, lhs.span());
                    self.check_stringification(&right, rhs.span());
                }
                if matches!(op, a::BinOp::And | a::BinOp::Or | a::BinOp::NullCoalesce) {
                    self.env = Self::join(vec![before, self.env.clone()]).unwrap();
                }
                if matches!(op, a::BinOp::NullCoalesce)
                    && (left.mode != Mode::Shared || right.mode != Mode::Shared)
                {
                    self.error(e.span(), "ownership transfers in coalescing expressions require explicit control flow");
                }
                let ty = if *op == a::BinOp::Concat {
                    Some(TypeRef::String)
                } else {
                    left.ty
                };
                Value {
                    fresh: true,
                    ..Value::plain(ty)
                }
            }
            Expr::Ternary {
                cond,
                then,
                else_branch,
                ..
            } => {
                let condition = self.expr(cond);
                let start = self.env.clone();
                let yes = then.as_ref().map(|e| self.expr(e)).unwrap_or(condition);
                let yes_env = self.env.clone();
                self.env = start;
                let no = self.expr(else_branch);
                self.env = Self::join(vec![yes_env, self.env.clone()]).unwrap();
                if yes.mode != no.mode {
                    self.error(
                        e.span(),
                        "conditional ownership results must have the same transfer mode",
                    );
                }
                Value {
                    mode: yes.mode,
                    fresh: yes.fresh && no.fresh,
                    ty: join_type(yes.ty.as_ref(), no.ty.as_ref()),
                    origins: yes.origins.union(&no.origins).cloned().collect(),
                }
            }
            Expr::Match { subject, arms, .. } => {
                self.expr(subject);
                let start = self.env.clone();
                let mut ends = vec![];
                let mut result: Option<Value> = None;
                for arm in arms {
                    self.env = start.clone();
                    for c in &arm.conds {
                        self.test_without_transfer(c);
                    }
                    let v = self.expr(&arm.result);
                    ends.push(self.env.clone());
                    if let Some(r) = &result {
                        if r.mode != v.mode {
                            self.error(
                                e.span(),
                                "match ownership results must have the same transfer mode",
                            );
                        }
                    }
                    result = Some(if let Some(previous) = result {
                        Value {
                            ty: join_type(previous.ty.as_ref(), v.ty.as_ref()),
                            origins: previous.origins.union(&v.origins).cloned().collect(),
                            fresh: previous.fresh && v.fresh,
                            mode: previous.mode,
                        }
                    } else {
                        v
                    });
                }
                self.env = Self::join(ends).unwrap_or(start);
                result.unwrap_or(Value::plain(None))
            }
            Expr::Instanceof {
                expr,
                type_args,
                span,
                ..
            } => {
                self.expr(expr);
                for t in type_args {
                    self.validate_type(t, &[], *span);
                }
                Value::plain(Some(TypeRef::Bool))
            }
        }
    }
}
fn unparen(mut e: &Expr) -> &Expr {
    while let Expr::Paren(inner, _) = e {
        e = inner;
    }
    e
}

fn erase_type(t: &mut TypeRef) {
    if mode(Some(t)) != Mode::Shared {
        *t = inner(t);
    }
    match t {
        TypeRef::Array(t) => erase_type(t),
        TypeRef::Union(members, _) => {
            for member in members {
                erase_type(member);
            }
        }
        TypeRef::Applied(_, ts, _) => {
            for t in ts {
                erase_type(t);
            }
        }
        _ => {}
    }
}
fn erase_expr(e: &mut Expr) {
    match e {
        Expr::Effect { statement, .. } => erase_stmt(statement),
        Expr::Call {
            callee,
            args,
            type_args,
            ..
        } => {
            if callee == "$pxr_move" {
                *callee = "$pxr_pass".into();
            } else if callee == READ || callee == WRITE {
                *callee = "$pxr_borrow_pass".into();
            }
            for t in type_args {
                erase_type(t);
            }
            for a in args {
                erase_expr(a);
            }
        }
        Expr::NamedArgument { value: expr, .. }
        | Expr::Unary { expr, .. }
        | Expr::Paren(expr, _)
        | Expr::Instanceof { expr, .. } => erase_expr(expr),
        Expr::Binary { lhs, rhs, .. } => {
            erase_expr(lhs);
            erase_expr(rhs);
        }
        Expr::Ternary {
            cond,
            then,
            else_branch,
            ..
        } => {
            erase_expr(cond);
            if let Some(t) = then {
                erase_expr(t);
            }
            erase_expr(else_branch);
        }
        Expr::Index { base, index, .. } => {
            erase_expr(base);
            if let Some(i) = index {
                erase_expr(i);
            }
        }
        Expr::ArrayLit { items, .. } => {
            for item in items {
                if let Some(k) = &mut item.key {
                    erase_expr(k);
                }
                erase_expr(&mut item.value);
            }
        }
        Expr::Match { subject, arms, .. } => {
            erase_expr(subject);
            for arm in arms {
                for c in &mut arm.conds {
                    erase_expr(c);
                }
                erase_expr(&mut arm.result);
            }
        }
        Expr::Property { base, .. } => erase_expr(base),
        Expr::MethodCall {
            base,
            args,
            type_args,
            ..
        } => {
            erase_expr(base);
            for t in type_args {
                erase_type(t);
            }
            for a in args {
                erase_expr(a);
            }
        }
        Expr::New {
            args, type_args, ..
        }
        | Expr::StaticCall {
            args, type_args, ..
        }
        | Expr::ParentMethodCall {
            args, type_args, ..
        } => {
            for t in type_args {
                erase_type(t);
            }
            for a in args {
                erase_expr(a);
            }
        }
        _ => {}
    }
}
fn erase_target(t: &mut LValue) {
    match t {
        LValue::Index { base, index, .. } => {
            erase_target(base);
            if let Some(e) = index {
                erase_expr(e);
            }
        }
        LValue::Property { base, .. } => erase_expr(base),
        _ => {}
    }
}
fn rewrite_task_drops(s: &mut Stmt, spans: &BTreeSet<Span>) {
    if let Stmt::Expr(Expr::Call { callee, span, .. }, _) = s {
        if callee == "$pxr_drop" && spans.contains(span) {
            *callee = "$pxr_task_drop".into();
        }
    }
    for body in s.bodies_mut() {
        for child in body {
            rewrite_task_drops(child, spans);
        }
    }
}
fn erase_stmt(s: &mut Stmt) {
    match s {
        Stmt::Expr(Expr::Call { callee, .. }, _) if callee == "$pxr_drop" => {
            if let Stmt::Expr(Expr::Call { callee, args, .. }, _) = s {
                *callee = "$pxr_value_drop".into();
                for argument in args {
                    erase_expr(argument);
                }
            }
            return;
        }
        Stmt::FnDecl(f) => {
            for p in &mut f.params {
                if let Some(t) = &mut p.ty {
                    erase_type(t);
                }
            }
            if let Some(t) = &mut f.ret {
                erase_type(t);
            }
        }
        Stmt::ClassDecl(c) => {
            for m in &mut c.members {
                match m {
                    a::ClassMember::Method(m) => {
                        for p in &mut m.params {
                            if let Some(t) = &mut p.ty {
                                erase_type(t);
                            }
                        }
                        if let Some(t) = &mut m.ret {
                            erase_type(t);
                        }
                    }
                    a::ClassMember::Property(p) => {
                        if let Some(e) = &mut p.default {
                            erase_expr(e);
                        }
                    }
                }
            }
        }
        Stmt::Assign {
            target, ty, value, ..
        } => {
            erase_target(target);
            if let Some(t) = ty {
                erase_type(t);
            }
            erase_expr(value);
        }
        Stmt::AssignOp { target, value, .. } => {
            erase_target(target);
            erase_expr(value);
        }
        Stmt::IncDec { target, .. } => erase_target(target),
        Stmt::Echo(es, _) => {
            for e in es {
                erase_expr(e);
            }
        }
        Stmt::Print(e, _)
        | Stmt::Expr(e, _)
        | Stmt::Return(Some(e), _)
        | Stmt::ForwardReturn(e, _) => erase_expr(e),
        Stmt::If { cond, elifs, .. } => {
            erase_expr(cond);
            for (c, _) in elifs {
                erase_expr(c);
            }
        }
        Stmt::While { cond, .. } | Stmt::DoWhile { cond, .. } => erase_expr(cond),
        Stmt::For { cond, .. } => {
            for e in cond {
                erase_expr(e);
            }
        }
        Stmt::Foreach { subject, .. } => erase_expr(subject),
        Stmt::Switch { subject, cases, .. } => {
            erase_expr(subject);
            for c in cases {
                if let Some(e) = &mut c.test {
                    erase_expr(e);
                }
            }
        }
        _ => {}
    }
    for body in s.bodies_mut() {
        for s in body {
            erase_stmt(s);
        }
    }
}

fn substitute(ty: &TypeRef, bindings: &BTreeMap<String, TypeRef>) -> TypeRef {
    match ty {
        TypeRef::Param(n, _) => bindings.get(n).cloned().unwrap_or_else(|| ty.clone()),
        TypeRef::Array(t) => TypeRef::Array(Box::new(substitute(t, bindings))),
        TypeRef::Union(members, span) => TypeRef::Union(
            members.iter().map(|t| substitute(t, bindings)).collect(),
            *span,
        ),
        TypeRef::Applied(n, ts, sp) => TypeRef::Applied(
            n.clone(),
            ts.iter().map(|t| substitute(t, bindings)).collect(),
            *sp,
        ),
        _ => ty.clone(),
    }
}
fn infer_bindings(pattern: &TypeRef, actual: &TypeRef, bindings: &mut BTreeMap<String, TypeRef>) {
    match (pattern, actual) {
        (TypeRef::Param(n, _), actual) => {
            bindings.entry(n.clone()).or_insert_with(|| actual.clone());
        }
        (TypeRef::Array(p), TypeRef::Array(a)) => infer_bindings(p, a, bindings),
        (TypeRef::Applied(p, ps, _), TypeRef::Applied(a, as_, _)) if p.eq_ignore_ascii_case(a) => {
            for (p, a) in ps.iter().zip(as_) {
                infer_bindings(p, a, bindings);
            }
        }
        _ => {}
    }
}

pub(super) fn type_shape(t: &TypeRef) -> String {
    match t {
        TypeRef::Param(n, _) => format!("param:{n}"),
        TypeRef::Named(n) => n.to_ascii_lowercase(),
        TypeRef::Applied(n, ts, _) => format!(
            "{}<{}>",
            n.to_ascii_lowercase(),
            ts.iter().map(type_shape).collect::<Vec<_>>().join(",")
        ),
        TypeRef::Array(t) => format!("array<{}>", type_shape(t)),
        TypeRef::Union(members, _) => {
            let mut shapes: Vec<_> = members.iter().map(type_shape).collect();
            shapes.sort();
            shapes.join("|")
        }
        _ => format!("{t:?}"),
    }
}

fn view_type(decl: &a::ClassDecl, bindings: &BTreeMap<String, TypeRef>) -> TypeRef {
    if decl.type_params.is_empty() {
        TypeRef::Named(decl.name.clone())
    } else {
        TypeRef::Applied(
            decl.name.clone(),
            decl.type_params
                .iter()
                .map(|(n, sp)| {
                    bindings
                        .get(n)
                        .cloned()
                        .unwrap_or_else(|| TypeRef::Param(n.clone(), *sp))
                })
                .collect(),
            decl.span,
        )
    }
}
fn specialize_signature(mut sig: Signature, types: &[TypeRef]) -> Signature {
    let bindings: BTreeMap<_, _> = sig
        .type_params
        .iter()
        .cloned()
        .zip(types.iter().cloned())
        .collect();
    for t in sig.params.iter_mut().flatten() {
        *t = substitute(t, &bindings);
    }
    if let Some(t) = &mut sig.ret {
        *t = substitute(t, &bindings);
    }
    sig.type_params
        .drain(..types.len().min(sig.type_params.len()));
    sig
}

fn join_type(a: Option<&TypeRef>, b: Option<&TypeRef>) -> Option<TypeRef> {
    match (a, b) {
        (Some(a), Some(b)) if type_shape(a) == type_shape(b) => Some(a.clone()),
        (Some(a), Some(b))
            if task_result(Some(a))
                .zip(task_result(Some(b)))
                .is_some_and(|(a, b)| type_shape(a) == type_shape(b)) =>
        {
            Some(TypeRef::Applied(
                ANY_TASK.into(),
                vec![task_result(Some(a)).unwrap().clone()],
                Span::synthetic(),
            ))
        }
        (Some(_), Some(_)) => Some(TypeRef::Mixed),
        _ => None,
    }
}
