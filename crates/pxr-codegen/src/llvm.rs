//! IR -> textual LLVM IR.
//!
//! The emitted module is ordinary LLVM assembly (opaque-pointer form) which is
//! then compiled and linked by `clang`. Keeping this stage textual — rather than
//! linking `llvm-sys` — means PXR has no build-time dependency on a matching
//! LLVM development installation, while still using the real LLVM backend for
//! optimization and machine-code generation.

use pxr_ir::{BinOp, CastOp, Function, Inst, Literal, Module, UnOp, Value};
use pxr_types::Ty;
use std::collections::HashMap;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct LlvmOptions {
    /// `source_filename` recorded in the module.
    pub source_name: String,
    /// Optional target triple; when `None`, clang's default is used.
    pub triple: Option<String>,
}

/// Runtime entry points and their C signatures.
const RUNTIME_DECLS: &[(&str, &str, &[&str])] = &[
    ("abort", "void", &[]),
    ("pxr_print_i64", "void", &["i64"]),
    ("pxr_print_f64", "void", &["double"]),
    ("pxr_print_str", "void", &["ptr"]),
    ("pxr_print_bool", "void", &["i32"]),
    ("pxr_str_concat", "ptr", &["ptr", "ptr"]),
    ("pxr_i64_to_str", "ptr", &["i64"]),
    ("pxr_f64_to_str", "ptr", &["double"]),
    ("pxr_bool_to_str", "ptr", &["i32"]),
    ("pxr_strlen", "i64", &["ptr"]),
    ("pxr_str_to_i64", "i64", &["ptr"]),
    ("pxr_str_to_f64", "double", &["ptr"]),
    ("pxr_truthy_i64", "i32", &["i64"]),
    ("pxr_truthy_f64", "i32", &["double"]),
    ("pxr_truthy_str", "i32", &["ptr"]),
    ("pxr_str_eq", "i32", &["ptr", "ptr"]),
    ("pxr_str_cmp", "i32", &["ptr", "ptr"]),
    ("pxr_str_cmp_loose", "i64", &["ptr", "ptr"]),
    ("pxr_abs_i64", "i64", &["i64"]),
    ("pxr_shl", "i64", &["i64", "i64"]),
    ("pxr_shr", "i64", &["i64", "i64"]),
    ("pxr_div", "double", &["double", "double"]),
    ("pxr_mod", "i64", &["i64", "i64"]),
    ("fabs", "double", &["double"]),
    // Arrays. Values cross the boundary as a type tag plus a 64-bit payload.
    ("pxr_array_new", "ptr", &[]),
    (
        "pxr_array_set",
        "void",
        &["ptr", "i64", "i64", "ptr", "i64", "i64"],
    ),
    ("pxr_array_push", "void", &["ptr", "i64", "i64"]),
    ("pxr_array_get_bits", "i64", &["ptr", "i64", "i64", "ptr"]),
    ("pxr_array_has", "i64", &["ptr", "i64", "i64", "ptr"]),
    ("pxr_array_count", "i64", &["ptr"]),
    ("pxr_array_val_bits_at", "i64", &["ptr", "i64"]),
    ("pxr_array_val_tag_at", "i64", &["ptr", "i64"]),
    ("pxr_array_copy", "ptr", &["ptr"]),
    // String builtins (M2). All operate on `pxr_str *` byte strings.
    ("pxr_str_toupper", "ptr", &["ptr"]),
    ("pxr_str_tolower", "ptr", &["ptr"]),
    ("pxr_str_trim", "ptr", &["ptr", "ptr", "i64"]),
    ("pxr_substr", "ptr", &["ptr", "i64", "i64", "i64"]),
    ("pxr_str_repeat", "ptr", &["ptr", "i64"]),
    ("pxr_str_replace", "ptr", &["ptr", "ptr", "ptr"]),
    ("pxr_implode", "ptr", &["ptr", "ptr"]),
    ("pxr_explode", "ptr", &["ptr", "ptr", "i64"]),
    // More string builtins (M2). Predicates return i64 0/1, narrowed with
    // `IntToBool` at the call site.
    ("pxr_str_contains", "i64", &["ptr", "ptr"]),
    ("pxr_str_starts_with", "i64", &["ptr", "ptr"]),
    ("pxr_str_ends_with", "i64", &["ptr", "ptr"]),
    ("pxr_strrev", "ptr", &["ptr"]),
    ("pxr_ucfirst", "ptr", &["ptr"]),
    ("pxr_lcfirst", "ptr", &["ptr"]),
    ("pxr_ucwords", "ptr", &["ptr"]),
    ("pxr_str_split", "ptr", &["ptr", "i64"]),
    // Array transforms (M2). Both deep-copy nested arrays (value semantics).
    ("pxr_array_values", "ptr", &["ptr"]),
    ("pxr_array_reverse", "ptr", &["ptr", "i64"]),
    // More array builtins (M2). `array_merge`/`slice`/`pad`/`fill` deep-copy
    // nested arrays; values cross as a type tag plus a 64-bit payload.
    ("pxr_array_merge", "ptr", &["ptr", "ptr"]),
    (
        "pxr_array_slice",
        "ptr",
        &["ptr", "i64", "i64", "i64", "i64"],
    ),
    ("pxr_array_pad", "ptr", &["ptr", "i64", "i64", "i64"]),
    ("pxr_array_fill", "ptr", &["i64", "i64", "i64", "i64"]),
    // String builtins (M2): `str_pad`, `substr_count`.
    ("pxr_str_pad", "ptr", &["ptr", "i64", "ptr", "i64"]),
    (
        "pxr_substr_count",
        "i64",
        &["ptr", "ptr", "i64", "i64", "i64"],
    ),
    // Escaping & encoding builtins (M2). All are `(string) -> string`.
    ("pxr_addslashes", "ptr", &["ptr"]),
    ("pxr_stripslashes", "ptr", &["ptr"]),
    ("pxr_bin2hex", "ptr", &["ptr"]),
    ("pxr_base64_encode", "ptr", &["ptr"]),
    ("pxr_urlencode", "ptr", &["ptr"]),
    ("pxr_rawurlencode", "ptr", &["ptr"]),
    ("pxr_urldecode", "ptr", &["ptr"]),
    ("pxr_rawurldecode", "ptr", &["ptr"]),
    // Formatted printing builtins (M2): sprintf / printf.
    ("pxr_sprintf", "ptr", &["ptr", "ptr"]),
    ("pxr_printf", "i64", &["ptr", "ptr"]),
    // Object runtime (M3.1, M3.2).
    ("pxr_object_new", "ptr", &["i64", "ptr", "i64"]),
    ("pxr_object_get_bits", "i64", &["ptr", "i64"]),
    ("pxr_object_set_bits", "void", &["ptr", "i64", "i64", "i64"]),
    ("pxr_class_register", "void", &["i64", "i64"]),
    ("pxr_object_get_class_id", "i64", &["ptr"]),
    ("pxr_instanceof", "i64", &["ptr", "i64"]),
];

pub fn llty(ty: &Ty) -> &'static str {
    match ty {
        Ty::Void => "void",
        Ty::Bool => "i1",
        Ty::Int => "i64",
        Ty::Float => "double",
        Ty::String => "ptr",
        // A `null` value is a null pointer (`inttoptr i64 0 to ptr`); the null
        // literal itself is otherwise rejected at compile time.
        Ty::Null => "ptr",
        Ty::Array(_) => "ptr",
        Ty::Named(_) => "ptr",
    }
}

fn align_of(ty: &Ty) -> u32 {
    match ty {
        Ty::Bool | Ty::Null => 1,
        _ => 8,
    }
}

pub fn emit_llvm(module: &Module, opts: &LlvmOptions) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "; Generated by PXR (PHP Extended Runtime). Do not edit."
    );
    if !opts.source_name.is_empty() {
        let _ = writeln!(out, "source_filename = {:?}", opts.source_name);
    }
    if let Some(t) = &opts.triple {
        let _ = writeln!(out, "target triple = \"{t}\"");
    }
    out.push('\n');

    // String constant pool.
    let mut pool: Vec<Vec<u8>> = Vec::new();
    let mut pool_idx: HashMap<Vec<u8>, usize> = HashMap::new();
    for f in &module.functions {
        for inst in &f.body {
            if let Inst::Const {
                lit: Literal::Str(s),
                ..
            } = inst
            {
                if !pool_idx.contains_key(s) {
                    pool_idx.insert(s.clone(), pool.len());
                    pool.push(s.clone());
                }
            }
        }
    }
    for (i, s) in pool.iter().enumerate() {
        let bytes = s.as_slice();
        let escaped = escape_bytes(bytes);
        let n = bytes.len() + 1;
        // A `pxr_str` (runtime): `{ size_t len; char data[]; }`. The constant
        // carries the length up front so embedded NUL bytes survive.
        let _ = writeln!(
            out,
            "@.pxr.str.{i} = private unnamed_addr constant {{ i64, [{n} x i8] }} {{ i64 {len}, [{n} x i8] c\"{escaped}\\00\" }}, align 8",
            len = bytes.len(),
        );
    }
    if !pool.is_empty() {
        out.push('\n');
    }

    for (name, ret, args) in RUNTIME_DECLS {
        let params = args
            .iter()
            .map(|a| (*a).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "declare {ret} @{name}({params})");
    }
    out.push('\n');

    for f in &module.functions {
        emit_function(&mut out, f, &pool_idx);
        out.push('\n');
    }
    out
}

fn emit_function(out: &mut String, f: &Function, pool: &HashMap<Vec<u8>, usize>) {
    let mut g = FnGen {
        f,
        pool,
        map: HashMap::new(),
        next: f.value_count(),
        is_entry: f.name == "main",
        out: String::new(),
    };
    g.run();
    out.push_str(&g.out);
}

struct FnGen<'a> {
    f: &'a Function,
    pool: &'a HashMap<Vec<u8>, usize>,
    map: HashMap<Value, Ty>,
    next: Value,
    /// True for the synthesized program entry point, whose LLVM signature must
    /// be `i32 @main()` so that the process exit status is well defined.
    is_entry: bool,
    out: String,
}

impl<'a> FnGen<'a> {
    fn run(&mut self) {
        // Signature.
        let params = self
            .f
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("{} %v{i}", llty(t)))
            .collect::<Vec<_>>()
            .join(", ");
        let ret_ll = if self.is_entry {
            "i32"
        } else {
            llty(&self.f.ret)
        };
        let _ = writeln!(self.out, "define {ret_ll} @{}({}) {{", self.f.name, params);

        for (i, t) in self.f.params.iter().enumerate() {
            self.map.insert(i as Value, t.clone());
        }

        // Block bookkeeping. LLVM requires every block to be terminated
        // explicitly before the next label, so when a block is still open we
        // emit an explicit branch to the label that follows it.
        let mut block_open = false;
        let mut terminated = true;

        for inst in &self.f.body {
            if let Inst::Label(l) = inst {
                if block_open && !terminated {
                    let _ = writeln!(self.out, "  br label %bb{l}");
                }
                let _ = writeln!(self.out, "bb{l}:");
                block_open = true;
                terminated = false;
                continue;
            }
            if !block_open {
                let _ = writeln!(self.out, "entry:");
                block_open = true;
                terminated = false;
            }
            self.emit_inst(inst);
            if is_terminator(inst) {
                terminated = true;
            }
        }

        if !block_open {
            let _ = writeln!(self.out, "entry:");
            self.line(if self.is_entry {
                "ret i32 0"
            } else {
                "ret void"
            });
        } else if !terminated {
            self.line(if self.is_entry {
                "ret i32 0"
            } else {
                "ret void"
            });
        }

        let _ = writeln!(self.out, "}}");
    }

    fn temp(&mut self) -> Value {
        let v = self.next;
        self.next += 1;
        v
    }

    fn set(&mut self, v: Value, ty: Ty) {
        self.map.insert(v, ty);
    }

    fn emit_inst(&mut self, inst: &Inst) {
        match inst {
            Inst::Label(l) => {
                let _ = writeln!(self.out, "bb{l}:");
            }
            Inst::Const { dst, lit } => {
                let (line, ty) = match lit {
                    Literal::Int(i) => (format!("%v{dst} = add i64 0, {i}"), Ty::Int),
                    Literal::Float(x) => (
                        // Exact bit pattern: `fadd 0.0, -0.0` would fold to +0.0
                        // under IEEE, and NaN payloads/signs must survive.
                        format!("%v{dst} = bitcast i64 {} to double", x.to_bits()),
                        Ty::Float,
                    ),
                    Literal::Bool(b) => {
                        let v = if *b { "true" } else { "false" };
                        (format!("%v{dst} = xor i1 false, {v}"), Ty::Bool)
                    }
                    Literal::Str(s) => {
                        let idx = *self.pool.get(s).unwrap();
                        let n = s.len() + 1;
                        (
                            format!(
                                "%v{dst} = getelementptr inbounds {{ i64, [{n} x i8] }}, ptr @.pxr.str.{idx}, i64 0"
                            ),
                            Ty::String,
                        )
                    }
                    Literal::Null => (format!("%v{dst} = inttoptr i64 0 to ptr"), Ty::Null),
                };
                self.set(*dst, ty);
                self.line(&line);
            }
            Inst::Bin {
                dst,
                op,
                lhs_ty,
                rhs_ty,
                result_ty,
                a,
                b,
            } => self.emit_bin(*dst, *op, lhs_ty, rhs_ty, result_ty, *a, *b),
            Inst::Un {
                dst,
                op,
                operand_ty,
                result_ty,
                a,
            } => {
                let line = match (op, operand_ty) {
                    (UnOp::Neg, Ty::Int) => format!("%v{dst} = sub i64 0, %v{a}"),
                    (UnOp::Neg, Ty::Float) => format!("%v{dst} = fneg double %v{a}"),
                    (UnOp::Not, Ty::Bool) => format!("%v{dst} = xor i1 %v{a}, true"),
                    (UnOp::BitNot, Ty::Int) => format!("%v{dst} = xor i64 %v{a}, -1"),
                    _ => format!("%v{dst} = sub {} 0, %v{a}", llty(operand_ty)),
                };
                self.set(*dst, result_ty.clone());
                self.line(&line);
            }
            Inst::Cast {
                dst,
                op,
                from,
                to,
                src,
            } => self.emit_cast(*dst, *op, from, to, *src),
            Inst::BitCast { dst, from, to, src } => {
                let line = match (from, to) {
                    (Ty::Bool, Ty::Int) => format!("%v{dst} = zext i1 %v{src} to i64"),
                    (Ty::Int, Ty::Bool) => format!("%v{dst} = trunc i64 %v{src} to i1"),
                    (Ty::Float, Ty::Int) => format!("%v{dst} = bitcast double %v{src} to i64"),
                    (Ty::Int, Ty::Float) => format!("%v{dst} = bitcast i64 %v{src} to double"),
                    (Ty::String, Ty::Int) | (Ty::Array(_), Ty::Int) | (Ty::Named(_), Ty::Int) => {
                        format!("%v{dst} = ptrtoint ptr %v{src} to i64")
                    }
                    (Ty::Int, Ty::String) | (Ty::Int, Ty::Array(_)) | (Ty::Int, Ty::Named(_)) => {
                        format!("%v{dst} = inttoptr i64 %v{src} to ptr")
                    }
                    _ => format!("%v{dst} = bitcast i64 %v{src} to i64"),
                };
                self.set(*dst, to.clone());
                self.line(&line);
            }
            Inst::Call {
                dst,
                callee,
                args,
                ret,
            } => {
                let arglist = args
                    .iter()
                    .map(|a| {
                        let t = self.map.get(a).cloned().unwrap_or(Ty::Int);
                        format!("{} %v{a}", llty(&t))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let head = match dst {
                    Some(d) if *ret != Ty::Void => format!("%v{d} = "),
                    _ => String::new(),
                };
                let line = format!("{head}call {} @{callee}({arglist})", llty(ret));
                if let Some(d) = dst {
                    if *ret != Ty::Void {
                        self.set(*d, ret.clone());
                    }
                }
                self.line(&line);
            }
            Inst::SlotAlloc { slot, ty } => {
                self.line(&format!(
                    "%s{slot} = alloca {}, align {}",
                    llty(ty),
                    align_of(ty)
                ));
            }
            Inst::SlotGet { dst, slot, ty } => {
                self.set(*dst, ty.clone());
                self.line(&format!(
                    "%v{dst} = load {}, ptr %s{slot}, align {}",
                    llty(ty),
                    align_of(ty)
                ));
            }
            Inst::SlotSet { slot, src, ty } => {
                self.line(&format!(
                    "store {} %v{src}, ptr %s{slot}, align {}",
                    llty(ty),
                    align_of(ty)
                ));
            }
            Inst::Print { ty, src } => match ty {
                Ty::Int => self.line(&format!("call void @pxr_print_i64(i64 %v{src})")),
                Ty::Float => self.line(&format!("call void @pxr_print_f64(double %v{src})")),
                Ty::String => self.line(&format!("call void @pxr_print_str(ptr %v{src})")),
                Ty::Bool => {
                    let t = self.temp();
                    self.line(&format!("%v{t} = zext i1 %v{src} to i32"));
                    self.line(&format!("call void @pxr_print_bool(i32 %v{t})"));
                }
                _ => self.line(&format!("; unsupported print of {ty}")),
            },
            Inst::Jump(l) => self.line(&format!("br label %bb{l}")),
            Inst::Branch {
                cond,
                then_label,
                else_label,
            } => self.line(&format!(
                "br i1 %v{cond}, label %bb{then_label}, label %bb{else_label}"
            )),
            Inst::Ret { ty, src } => {
                if self.is_entry {
                    self.line("ret i32 0");
                } else {
                    match (ty, src) {
                        (Ty::Void, _) => self.line("ret void"),
                        (_, Some(s)) => self.line(&format!("ret {} %v{s}", llty(ty))),
                        (_, None) => self.line("ret void"),
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_bin(
        &mut self,
        dst: Value,
        op: BinOp,
        lhs_ty: &Ty,
        rhs_ty: &Ty,
        result_ty: &Ty,
        a: Value,
        b: Value,
    ) {
        if op == BinOp::Cmp {
            self.emit_cmp(dst, lhs_ty, a, b, result_ty);
            return;
        }
        let line = match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod => {
                if lhs_ty == &Ty::Float {
                    let ins = match op {
                        BinOp::Add => "fadd",
                        BinOp::Sub => "fsub",
                        BinOp::Mul => "fmul",
                        _ => "frem",
                    };
                    format!("%v{dst} = {ins} double %v{a}, %v{b}")
                } else {
                    let ins = match op {
                        BinOp::Add => "add",
                        BinOp::Sub => "sub",
                        BinOp::Mul => "mul",
                        _ => "srem",
                    };
                    format!("%v{dst} = {ins} i64 %v{a}, %v{b}")
                }
            }
            BinOp::Div => format!("%v{dst} = fdiv double %v{a}, %v{b}"),
            BinOp::Concat => {
                format!("%v{dst} = call ptr @pxr_str_concat(ptr %v{a}, ptr %v{b})")
            }
            BinOp::Eq | BinOp::NotEq => match lhs_ty {
                Ty::Int => {
                    let c = if op == BinOp::Eq { "eq" } else { "ne" };
                    format!("%v{dst} = icmp {c} i64 %v{a}, %v{b}")
                }
                Ty::Float => {
                    // ordered eq (false for NaN); unordered ne (true for NaN).
                    let c = if op == BinOp::Eq { "oeq" } else { "une" };
                    format!("%v{dst} = fcmp {c} double %v{a}, %v{b}")
                }
                Ty::Bool => {
                    let c = if op == BinOp::Eq { "eq" } else { "ne" };
                    format!("%v{dst} = icmp {c} i1 %v{a}, %v{b}")
                }
                Ty::String => {
                    let t = self.temp();
                    self.line(&format!(
                        "%v{t} = call i32 @pxr_str_eq(ptr %v{a}, ptr %v{b})"
                    ));
                    let c = if op == BinOp::Eq { "ne" } else { "eq" };
                    format!("%v{dst} = icmp {c} i32 %v{t}, 0")
                }
                _ => format!("%v{dst} = icmp eq i64 0, 0"),
            },
            BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => match lhs_ty {
                Ty::Int => {
                    let c = match op {
                        BinOp::Lt => "slt",
                        BinOp::LtEq => "sle",
                        BinOp::Gt => "sgt",
                        _ => "sge",
                    };
                    format!("%v{dst} = icmp {c} i64 %v{a}, %v{b}")
                }
                Ty::Float => {
                    let c = match op {
                        BinOp::Lt => "olt",
                        BinOp::LtEq => "ole",
                        BinOp::Gt => "ogt",
                        _ => "oge",
                    };
                    format!("%v{dst} = fcmp {c} double %v{a}, %v{b}")
                }
                Ty::String => {
                    let t = self.temp();
                    self.line(&format!(
                        "%v{t} = call i32 @pxr_str_cmp(ptr %v{a}, ptr %v{b})"
                    ));
                    let c = match op {
                        BinOp::Lt => "slt",
                        BinOp::LtEq => "sle",
                        BinOp::Gt => "sgt",
                        _ => "sge",
                    };
                    format!("%v{dst} = icmp {c} i32 %v{t}, 0")
                }
                _ => format!("%v{dst} = icmp eq i64 0, 0"),
            },
            BinOp::BitAnd => format!("%v{dst} = and i64 %v{a}, %v{b}"),
            BinOp::BitOr => format!("%v{dst} = or i64 %v{a}, %v{b}"),
            BinOp::BitXor => format!("%v{dst} = xor i64 %v{a}, %v{b}"),
            // `Cmp` is handled by the early `if` above.
            BinOp::Cmp => unreachable!(),
            BinOp::Shl | BinOp::Shr => {
                // Mask the shift amount so it is always in bounds (LLVM would
                // otherwise treat oversized shifts as immediate UB).
                let t = self.temp();
                self.line(&format!("%v{t} = and i64 %v{b}, 63"));
                let ins = if op == BinOp::Shl { "shl" } else { "ashr" };
                format!("%v{dst} = {ins} i64 %v{a}, %v{t}")
            }
        };
        let _ = rhs_ty;
        self.set(dst, result_ty.clone());
        self.line(&line);
    }

    /// `<=>` — a three-way comparison producing -1 / 0 / 1.
    fn emit_cmp(&mut self, dst: Value, lhs_ty: &Ty, a: Value, b: Value, result_ty: &Ty) {
        let (lt, gt) = if lhs_ty == &Ty::String {
            let tc = self.temp();
            self.line(&format!(
                "%v{tc} = call i32 @pxr_str_cmp(ptr %v{a}, ptr %v{b})"
            ));
            let t1 = self.temp();
            let t2 = self.temp();
            self.line(&format!("%v{t1} = icmp slt i32 %v{tc}, 0"));
            self.line(&format!("%v{t2} = icmp sgt i32 %v{tc}, 0"));
            (t1, t2)
        } else if lhs_ty == &Ty::Float {
            let t1 = self.temp();
            let t2 = self.temp();
            self.line(&format!("%v{t1} = fcmp olt double %v{a}, %v{b}"));
            self.line(&format!("%v{t2} = fcmp ogt double %v{a}, %v{b}"));
            (t1, t2)
        } else {
            let t1 = self.temp();
            let t2 = self.temp();
            self.line(&format!("%v{t1} = icmp slt i64 %v{a}, %v{b}"));
            self.line(&format!("%v{t2} = icmp sgt i64 %v{a}, %v{b}"));
            (t1, t2)
        };
        let t3 = self.temp();
        self.line(&format!("%v{t3} = select i1 %v{lt}, i64 -1, i64 0"));
        self.line(&format!("%v{dst} = select i1 %v{gt}, i64 1, i64 %v{t3}"));
        self.set(dst, result_ty.clone());
    }

    fn emit_cast(&mut self, dst: Value, op: CastOp, from: &Ty, to: &Ty, src: Value) {
        let line = match op {
            CastOp::IntToFloat => format!("%v{dst} = sitofp i64 %v{src} to double"),
            CastOp::FloatToInt => format!("%v{dst} = fptosi double %v{src} to i64"),
            CastOp::BoolToInt => format!("%v{dst} = zext i1 %v{src} to i64"),
            CastOp::BoolToFloat => format!("%v{dst} = uitofp i1 %v{src} to double"),
            CastOp::IntToBool => format!("%v{dst} = icmp ne i64 %v{src}, 0"),
            CastOp::FloatToBool => format!("%v{dst} = fcmp une double %v{src}, 0.0"),
            CastOp::StrToBool => {
                let t = self.temp();
                self.line(&format!("%v{t} = call i32 @pxr_truthy_str(ptr %v{src})"));
                format!("%v{dst} = icmp ne i32 %v{t}, 0")
            }
            CastOp::IntToStr => {
                format!("%v{dst} = call ptr @pxr_i64_to_str(i64 %v{src})")
            }
            CastOp::FloatToStr => {
                format!("%v{dst} = call ptr @pxr_f64_to_str(double %v{src})")
            }
            CastOp::BoolToStr => {
                let t = self.temp();
                self.line(&format!("%v{t} = zext i1 %v{src} to i32"));
                format!("%v{dst} = call ptr @pxr_bool_to_str(i32 %v{t})")
            }
            CastOp::StrToInt => format!("%v{dst} = call i64 @pxr_str_to_i64(ptr %v{src})"),
            CastOp::StrToFloat => format!("%v{dst} = call double @pxr_str_to_f64(ptr %v{src})"),
        };
        let _ = (from, to);
        self.set(dst, to.clone());
        self.line(&line);
    }

    fn line(&mut self, s: &str) {
        let _ = writeln!(self.out, "  {s}");
    }
}

fn is_terminator(inst: &Inst) -> bool {
    matches!(inst, Inst::Jump(_) | Inst::Branch { .. } | Inst::Ret { .. })
}

fn escape_bytes(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        if b == b'"' || b == b'\\' || b < 0x20 || b > 0x7e {
            let _ = write!(s, "\\{b:02X}");
        } else {
            s.push(b as char);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use pxr_ir::{Function, Literal};

    #[test]
    fn emits_a_minimal_program() {
        let mut f = Function::new("main", "main", vec![], Ty::Void);
        let c = f.fresh_value();
        f.push(Inst::Const {
            dst: c,
            lit: Literal::Int(42),
        });
        f.push(Inst::Print {
            ty: Ty::Int,
            src: c,
        });
        f.push(Inst::Ret {
            ty: Ty::Void,
            src: None,
        });
        let mut m = Module::new();
        m.function(f);
        let text = emit_llvm(&m, &LlvmOptions::default());
        // The synthesized entry point must yield a well-defined process exit
        // status, so `main` is emitted as `i32 @main()` returning 0.
        assert!(text.contains("define i32 @main()"));
        assert!(text.contains("call void @pxr_print_i64(i64"));
        assert!(text.contains("ret i32 0"));
    }

    #[test]
    fn escapes_strings() {
        assert_eq!(escape_bytes(b"a\"b\\c"), "a\\22b\\5Cc");
    }
}
