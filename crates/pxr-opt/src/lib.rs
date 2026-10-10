//! The PXR IR optimizer.
//!
//! Milestone 0 implements a small but genuinely useful, *conservative* set of
//! machine-independent passes:
//!
//! * **constant folding** — arithmetic and comparisons on literal operands,
//! * **constant branch resolution** — `if (true)` / `if (false)` collapse,
//! * **unary simplification** — `!true`, `-<const>`,
//! * **dead code elimination** — pure instructions whose result is unused,
//! * **CFG cleanup** — jumps to the immediately following label.
//!
//! Passes never change observable behaviour. Where semantics could diverge
//! from the runtime (for example float→string formatting) folding is simply
//! skipped.

use pxr_ir::{BinOp, Function, Inst, Literal, Module, UnOp, Value};
use pxr_types::Ty;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptStats {
    pub constants_folded: usize,
    pub branches_folded: usize,
    pub instructions_removed: usize,
    pub jumps_removed: usize,
}

impl OptStats {
    pub fn merge(&mut self, other: OptStats) {
        self.constants_folded += other.constants_folded;
        self.branches_folded += other.branches_folded;
        self.instructions_removed += other.instructions_removed;
        self.jumps_removed += other.jumps_removed;
    }
}

/// Run the optimizer on the whole module. Returns aggregate statistics.
pub fn optimize(module: &mut Module) -> OptStats {
    let mut total = OptStats::default();
    for f in &mut module.functions {
        let s = optimize_function(f);
        total.merge(s);
        f.sync_counters();
    }
    total
}

pub fn optimize_function(f: &mut Function) -> OptStats {
    let mut stats = OptStats::default();
    // Iterate to a fixed point: folding can expose more folding and more dead
    // code, and DCE can expose jumps to the next label.
    loop {
        let before = stats;
        fold(f, &mut stats);
        dce(f, &mut stats);
        cleanup_cfg(f, &mut stats);
        if stats == before {
            break;
        }
    }
    stats
}

/// Constant folding, constant branches and unary simplification.
fn fold(f: &mut Function, stats: &mut OptStats) {
    let mut consts: HashMap<Value, Literal> = HashMap::new();
    for inst in &mut f.body {
        match inst {
            Inst::Label(_) => consts.clear(),
            Inst::Const { dst, lit } => {
                consts.insert(*dst, lit.clone());
            }
            Inst::Bin {
                dst,
                op,
                lhs_ty,
                rhs_ty,
                result_ty,
                a,
                b,
            } => {
                if let (Some(la), Some(lb)) = (consts.get(a).cloned(), consts.get(b).cloned()) {
                    if let Some(out) = eval_bin(*op, lhs_ty, rhs_ty, result_ty, &la, &lb) {
                        consts.insert(*dst, out.clone());
                        *inst = Inst::Const {
                            dst: *dst,
                            lit: out,
                        };
                        stats.constants_folded += 1;
                    }
                }
            }
            Inst::Un {
                dst,
                op,
                operand_ty,
                result_ty,
                a,
            } => {
                if let Some(la) = consts.get(a).cloned() {
                    if let Some(out) = eval_un(*op, operand_ty, result_ty, &la) {
                        consts.insert(*dst, out.clone());
                        *inst = Inst::Const {
                            dst: *dst,
                            lit: out,
                        };
                        stats.constants_folded += 1;
                    }
                }
            }
            Inst::Branch {
                cond,
                then_label,
                else_label,
            } => {
                if let Some(Literal::Bool(b)) = consts.get(cond) {
                    let target = if *b { *then_label } else { *else_label };
                    *inst = Inst::Jump(target);
                    stats.branches_folded += 1;
                }
            }
            _ => {}
        }
    }
}

/// Remove pure instructions whose results are never used.
fn dce(f: &mut Function, stats: &mut OptStats) {
    let mut used: HashSet<Value> = HashSet::new();
    for inst in &f.body {
        if !inst.is_pure() {
            for u in inst.used() {
                used.insert(u);
            }
        }
    }
    loop {
        let mut changed = false;
        for inst in &f.body {
            if let Some(d) = inst.defined() {
                if used.contains(&d) {
                    for u in inst.used() {
                        if used.insert(u) {
                            changed = true;
                        }
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let before = f.body.len();
    f.body.retain(|inst| {
        if inst.is_pure() {
            match inst.defined() {
                Some(d) => used.contains(&d),
                None => true,
            }
        } else {
            true
        }
    });
    stats.instructions_removed += before - f.body.len();
}

/// Drop jumps whose target is the label that immediately follows.
fn cleanup_cfg(f: &mut Function, stats: &mut OptStats) {
    let mut i = 0;
    while i < f.body.len() {
        let remove = matches!(
            (&f.body[i], f.body.get(i + 1)),
            (Inst::Jump(target), Some(Inst::Label(l))) if target == l
        );
        if remove {
            f.body.remove(i);
            stats.jumps_removed += 1;
        } else {
            i += 1;
        }
    }
}

fn lit_as_f64(l: &Literal) -> Option<f64> {
    match l {
        Literal::Int(i) => Some(*i as f64),
        Literal::Float(f) => Some(*f),
        _ => None,
    }
}

fn eval_bin(
    op: BinOp,
    lhs_ty: &Ty,
    rhs_ty: &Ty,
    result_ty: &Ty,
    a: &Literal,
    b: &Literal,
) -> Option<Literal> {
    use Literal::*;
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            let both_int = lhs_ty == &Ty::Int && rhs_ty == &Ty::Int;
            if both_int {
                let (Int(x), Int(y)) = (a, b) else {
                    return None;
                };
                let v = match op {
                    BinOp::Add => x.checked_add(*y)?,
                    BinOp::Sub => x.checked_sub(*y)?,
                    BinOp::Mul => x.checked_mul(*y)?,
                    BinOp::Div => return None, // `/` is float in PXR
                    BinOp::Mod => {
                        if *y == 0 {
                            return None;
                        }
                        x.checked_rem(*y)?
                    }
                    _ => unreachable!(),
                };
                return Some(Int(v));
            }
            // Floating point path (result must be float).
            if result_ty != &Ty::Float {
                return None;
            }
            let x = lit_as_f64(a)?;
            let y = lit_as_f64(b)?;
            let v = match op {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => {
                    if y == 0.0 {
                        return None;
                    }
                    x / y
                }
                _ => return None,
            };
            Some(Float(v))
        }
        BinOp::Concat => match (a, b) {
            (Str(x), Str(y)) => Some(Str(format!("{x}{y}"))),
            _ => None,
        },
        BinOp::Eq | BinOp::NotEq => {
            let equal = match (a, b) {
                (Int(x), Int(y)) => x == y,
                (Float(x), Float(y)) => x == y,
                (Int(x), Float(y)) | (Float(y), Int(x)) => (*x as f64) == *y,
                (Bool(x), Bool(y)) => x == y,
                (Str(x), Str(y)) => x == y,
                _ => return None,
            };
            Some(Bool(if op == BinOp::Eq { equal } else { !equal }))
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            let (x, y) = match (a, b) {
                (Int(x), Int(y)) => (*x as f64, *y as f64),
                (Float(x), Float(y)) => (*x, *y),
                (Int(x), Float(y)) => (*x as f64, *y),
                (Float(x), Int(y)) => (*x, *y as f64),
                _ => return None,
            };
            // NaN never satisfies any ordering; folding would be wrong.
            if x.is_nan() || y.is_nan() {
                return None;
            }
            let r = match op {
                BinOp::Lt => x < y,
                BinOp::LtEq => x <= y,
                BinOp::Gt => x > y,
                BinOp::GtEq => x >= y,
                _ => unreachable!(),
            };
            Some(Bool(r))
        }
        // `<=>` produces -1 / 0 / 1.
        BinOp::Cmp => {
            let ord = |o: std::cmp::Ordering| match o {
                std::cmp::Ordering::Less => -1i64,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            let v = match (a, b) {
                (Int(x), Int(y)) => ord(x.cmp(y)),
                (Float(x), Float(y)) => ord(x.partial_cmp(y)?),
                (Str(x), Str(y)) => ord(x.cmp(y)),
                _ => return None,
            };
            Some(Int(v))
        }
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
            let (Int(x), Int(y)) = (a, b) else {
                return None;
            };
            let v = match op {
                BinOp::BitAnd => x & y,
                BinOp::BitOr => x | y,
                BinOp::BitXor => x ^ y,
                _ => unreachable!(),
            };
            Some(Int(v))
        }
        BinOp::Shl | BinOp::Shr => {
            // Mirrors the emitted code, which masks the shift amount to 0..63.
            let (Int(x), Int(y)) = (a, b) else {
                return None;
            };
            let shift = (y & 63) as u32;
            let v = if op == BinOp::Shl {
                x.wrapping_shl(shift)
            } else {
                x.wrapping_shr(shift)
            };
            Some(Int(v))
        }
    }
}

fn eval_un(op: UnOp, operand_ty: &Ty, result_ty: &Ty, a: &Literal) -> Option<Literal> {
    match (op, a) {
        (UnOp::Neg, Literal::Int(v)) if operand_ty == &Ty::Int && result_ty == &Ty::Int => {
            v.checked_neg().map(Literal::Int)
        }
        (UnOp::Neg, Literal::Float(v)) => Some(Literal::Float(-v)),
        (UnOp::Not, Literal::Bool(b)) => Some(Literal::Bool(!b)),
        (UnOp::BitNot, Literal::Int(v)) => Some(Literal::Int(!v)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pxr_ir::Function;

    fn module_with(body: Vec<Inst>, ret: Ty) -> Module {
        let mut f = Function::new("f", "f", vec![], ret);
        for i in body {
            f.push(i);
        }
        f.sync_counters();
        let mut m = Module::new();
        m.function(f);
        m
    }

    #[test]
    fn folds_integer_addition() {
        let mut m = module_with(
            vec![
                Inst::Const {
                    dst: 0,
                    lit: Literal::Int(2),
                },
                Inst::Const {
                    dst: 1,
                    lit: Literal::Int(3),
                },
                Inst::Bin {
                    dst: 2,
                    op: BinOp::Add,
                    lhs_ty: Ty::Int,
                    rhs_ty: Ty::Int,
                    result_ty: Ty::Int,
                    a: 0,
                    b: 1,
                },
                Inst::Ret {
                    ty: Ty::Int,
                    src: Some(2),
                },
            ],
            Ty::Int,
        );
        let s = optimize(&mut m);
        assert!(s.constants_folded >= 1);
        // The add should now be a constant 5.
        let has5 = m.functions[0].body.iter().any(|i| {
            matches!(
                i,
                Inst::Const {
                    lit: Literal::Int(5),
                    ..
                }
            )
        });
        assert!(
            has5,
            "expected folded constant 5 in {:?}",
            m.functions[0].body
        );
    }

    #[test]
    fn dead_code_eliminated() {
        let mut m = module_with(
            vec![
                Inst::Const {
                    dst: 0,
                    lit: Literal::Int(1),
                },
                Inst::Const {
                    dst: 1,
                    lit: Literal::Int(2),
                },
                Inst::Ret {
                    ty: Ty::Void,
                    src: None,
                },
            ],
            Ty::Void,
        );
        let s = optimize(&mut m);
        assert!(s.instructions_removed >= 2);
        assert!(m.functions[0]
            .body
            .iter()
            .all(|i| !matches!(i, Inst::Const { .. })));
    }

    #[test]
    fn constant_branch_folds() {
        let mut m = module_with(
            vec![
                Inst::Const {
                    dst: 0,
                    lit: Literal::Bool(true),
                },
                Inst::Branch {
                    cond: 0,
                    then_label: 1,
                    else_label: 2,
                },
                Inst::Label(1),
                Inst::Ret {
                    ty: Ty::Void,
                    src: None,
                },
                Inst::Label(2),
                Inst::Ret {
                    ty: Ty::Void,
                    src: None,
                },
            ],
            Ty::Void,
        );
        let s = optimize(&mut m);
        assert!(s.branches_folded >= 1);
        // The conditional branch is gone (either folded to a jump or, because
        // the target immediately follows, removed entirely).
        assert!(m.functions[0]
            .body
            .iter()
            .all(|i| !matches!(i, Inst::Branch { .. })));
    }
}
