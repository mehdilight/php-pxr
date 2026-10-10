//! The PXR intermediate representation.
//!
//! PXR uses a small, **typed, three-address** IR with an explicit control-flow
//! graph. Every instruction that produces a result writes to a fresh *value*
//! (`%vN`), mirroring SSA style while still allowing mutable local variables
//! through dedicated `slot` instructions (`%sN` stack slots). This keeps the
//! representation simple enough to reason about, and lets LLVM's own
//! `mem2reg`/`SROA` passes promote the slots to SSA registers when we hand it
//! the generated `.ll` file at `-O2`.
//!
//! The module also owns a textual printer used by `pxr emit-ir`, which is
//! invaluable when debugging the optimizer.

use pxr_types::Ty;
use std::fmt::Write as _;

pub type Value = u32;
pub type Slot = u32;
pub type Label = u32;

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Literal {
    pub fn ty(&self) -> Ty {
        match self {
            Literal::Int(_) => Ty::Int,
            Literal::Float(_) => Ty::Float,
            Literal::Bool(_) => Ty::Bool,
            Literal::Str(_) => Ty::String,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Mod => "mod",
            BinOp::Concat => "concat",
            BinOp::Eq => "eq",
            BinOp::NotEq => "ne",
            BinOp::Lt => "lt",
            BinOp::LtEq => "le",
            BinOp::Gt => "gt",
            BinOp::GtEq => "ge",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

/// Explicit type conversions. String conversions require runtime support;
/// numeric conversions map directly onto LLVM instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastOp {
    IntToFloat,
    FloatToInt,
    BoolToInt,
    BoolToFloat,
    IntToBool,
    FloatToBool,
    StrToBool,
    IntToStr,
    FloatToStr,
    BoolToStr,
    StrToInt,
    StrToFloat,
}

impl CastOp {
    pub fn name(self) -> &'static str {
        match self {
            CastOp::IntToFloat => "int_to_float",
            CastOp::FloatToInt => "float_to_int",
            CastOp::BoolToInt => "bool_to_int",
            CastOp::BoolToFloat => "bool_to_float",
            CastOp::IntToBool => "int_to_bool",
            CastOp::FloatToBool => "float_to_bool",
            CastOp::StrToBool => "str_to_bool",
            CastOp::IntToStr => "int_to_str",
            CastOp::FloatToStr => "float_to_str",
            CastOp::BoolToStr => "bool_to_str",
            CastOp::StrToInt => "str_to_int",
            CastOp::StrToFloat => "str_to_float",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inst {
    /// Start of a basic block.
    Label(Label),
    Const {
        dst: Value,
        lit: Literal,
    },
    /// Binary operation. Operand and result types are recorded explicitly so
    /// that later stages never have to re-derive them.
    Bin {
        dst: Value,
        op: BinOp,
        lhs_ty: Ty,
        rhs_ty: Ty,
        result_ty: Ty,
        a: Value,
        b: Value,
    },
    Un {
        dst: Value,
        op: UnOp,
        operand_ty: Ty,
        result_ty: Ty,
        a: Value,
    },
    Cast {
        dst: Value,
        op: CastOp,
        from: Ty,
        to: Ty,
        src: Value,
    },
    Call {
        dst: Option<Value>,
        callee: String,
        args: Vec<Value>,
        ret: Ty,
    },
    SlotAlloc {
        slot: Slot,
        ty: Ty,
    },
    SlotGet {
        dst: Value,
        slot: Slot,
        ty: Ty,
    },
    SlotSet {
        slot: Slot,
        src: Value,
        ty: Ty,
    },
    /// Print a value followed by no separator (matching PHP `echo`).
    Print {
        ty: Ty,
        src: Value,
    },
    Jump(Label),
    Branch {
        cond: Value,
        then_label: Label,
        else_label: Label,
    },
    Ret {
        ty: Ty,
        src: Option<Value>,
    },
}

impl Inst {
    /// Is this instruction free of side effects and therefore removable when
    /// its result is unused?
    pub fn is_pure(&self) -> bool {
        matches!(
            self,
            Inst::Const { .. }
                | Inst::Bin { .. }
                | Inst::Un { .. }
                | Inst::Cast { .. }
                | Inst::SlotGet { .. }
        )
    }

    /// The value this instruction defines, if any.
    pub fn defined(&self) -> Option<Value> {
        match self {
            Inst::Const { dst, .. }
            | Inst::Bin { dst, .. }
            | Inst::Un { dst, .. }
            | Inst::Cast { dst, .. }
            | Inst::SlotGet { dst, .. } => Some(*dst),
            Inst::Call { dst, .. } => *dst,
            _ => None,
        }
    }

    pub fn used(&self) -> Vec<Value> {
        match self {
            Inst::Bin { a, b, .. } => vec![*a, *b],
            Inst::Un { a, .. } => vec![*a],
            Inst::Cast { src, .. } => vec![*src],
            Inst::Call { args, .. } => args.clone(),
            Inst::SlotSet { src, .. } => vec![*src],
            Inst::Print { src, .. } => vec![*src],
            Inst::Branch { cond, .. } => vec![*cond],
            Inst::Ret { src: Some(s), .. } => vec![*s],
            _ => vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    /// Mangled, link-time name (e.g. `pxr$main`).
    pub name: String,
    /// Original source name for diagnostics (e.g. `add`).
    pub display: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub slots: Vec<Ty>,
    pub body: Vec<Inst>,
    next_value: Value,
    next_label: Label,
}

impl Function {
    pub fn new(name: impl Into<String>, display: impl Into<String>, params: Vec<Ty>, ret: Ty) -> Self {
        let next_value = params.len() as Value;
        Function {
            name: name.into(),
            display: display.into(),
            params,
            ret,
            slots: Vec::new(),
            body: Vec::new(),
            next_value,
            next_label: 0,
        }
    }

    pub fn fresh_value(&mut self) -> Value {
        let v = self.next_value;
        self.next_value += 1;
        v
    }

    /// One past the highest value id allocated so far.
    pub fn value_count(&self) -> Value {
        self.next_value
    }

    pub fn fresh_label(&mut self) -> Label {
        let l = self.next_label;
        self.next_label += 1;
        l
    }

    pub fn alloc_slot(&mut self, ty: Ty) -> Slot {
        let s = self.slots.len() as Slot;
        self.slots.push(ty);
        s
    }

    pub fn push(&mut self, inst: Inst) {
        self.body.push(inst);
    }

    /// Ensure the value/label counters are ahead of anything already present.
    /// Called after passes that create fresh ids.
    pub fn sync_counters(&mut self) {
        self.next_value = self.next_value.max(self.params.len() as Value);
        for inst in &self.body {
            if let Some(v) = inst.defined() {
                self.next_value = self.next_value.max(v + 1);
            }
            match inst {
                Inst::Label(l) => self.next_label = self.next_label.max(l + 1),
                Inst::Jump(l) => self.next_label = self.next_label.max(l + 1),
                Inst::Branch {
                    then_label,
                    else_label,
                    ..
                } => {
                    self.next_label = self.next_label.max(then_label + 1);
                    self.next_label = self.next_label.max(else_label + 1);
                }
                _ => {}
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let params = self
            .params
            .iter()
            .enumerate()
            .map(|(i, t)| format!("%v{i}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "func {}({}) -> {} {{", self.name, params, self.ret);
        for (i, ty) in self.slots.iter().enumerate() {
            let _ = writeln!(out, "  slot %s{i}: {ty}");
        }
        for inst in &self.body {
            let _ = writeln!(out, "  {}", fmt_inst(inst));
        }
        let _ = writeln!(out, "}}");
        out
    }
}

fn fmt_inst(inst: &Inst) -> String {
    match inst {
        Inst::Label(l) => format!("bb{l}:"),
        Inst::Const { dst, lit } => {
            let v = match lit {
                Literal::Int(i) => i.to_string(),
                Literal::Float(f) => format_float(*f),
                Literal::Bool(b) => b.to_string(),
                Literal::Str(s) => format!("{s:?}"),
            };
            format!("%v{dst} = const {} {v}", lit.ty())
        }
        Inst::Bin {
            dst,
            op,
            lhs_ty,
            rhs_ty,
            result_ty,
            a,
            b,
        } => format!(
            "%v{dst} = {} {lhs_ty} %v{a}, {rhs_ty} %v{b} -> {result_ty}",
            op.name()
        ),
        Inst::Un {
            dst,
            op,
            operand_ty,
            result_ty,
            a,
        } => {
            let name = match op {
                UnOp::Neg => "neg",
                UnOp::Not => "not",
            };
            format!("%v{dst} = {name} {operand_ty} %v{a} -> {result_ty}")
        }
        Inst::Cast {
            dst,
            op,
            from,
            to,
            src,
        } => format!("%v{dst} = cast {} {from} %v{src} -> {to}", op.name()),
        Inst::Call {
            dst,
            callee,
            args,
            ret,
        } => {
            let args = args
                .iter()
                .map(|a| format!("%v{a}"))
                .collect::<Vec<_>>()
                .join(", ");
            let head = match dst {
                Some(d) => format!("%v{d} = "),
                None => String::new(),
            };
            format!("{head}call @{callee}({args}) -> {ret}")
        }
        Inst::SlotAlloc { slot, ty } => format!("slotsetup %s{slot}: {ty}"),
        Inst::SlotGet { dst, slot, ty } => format!("%v{dst} = slotget %s{slot}: {ty}"),
        Inst::SlotSet { slot, src, ty } => format!("slotset %s{slot}: {ty} = %v{src}"),
        Inst::Print { ty, src } => format!("print {ty} %v{src}"),
        Inst::Jump(l) => format!("jump bb{l}"),
        Inst::Branch {
            cond,
            then_label,
            else_label,
        } => format!("branch %v{cond}, bb{then_label}, bb{else_label}"),
        Inst::Ret { ty, src } => match src {
            Some(s) => format!("ret {ty} %v{s}"),
            None => "ret void".to_string(),
        },
    }
}

pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let s = format!("{f:?}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Module {
    pub functions: Vec<Function>,
}

impl Module {
    pub fn new() -> Self {
        Module {
            functions: Vec::new(),
        }
    }

    pub fn function(&mut self, f: Function) {
        self.functions.push(f);
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for f in &self.functions {
            out.push_str(&f.to_text());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_round_trip_textually() {
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(1.5), "1.5");
        assert!(format_float(0.1).starts_with("0.1"));
    }

    #[test]
    fn builder_allocates_unique_ids() {
        let mut f = Function::new("f", "f", vec![], Ty::Void);
        let a = f.fresh_value();
        let b = f.fresh_value();
        assert_ne!(a, b);
        let s = f.alloc_slot(Ty::Int);
        assert_eq!(s, 0);
        assert_eq!(f.slots[0], Ty::Int);
    }
}
