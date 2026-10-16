//! Precise, backwards CFG liveness for stack roots at native safepoints.
use pxr_ir::{Function, Inst, Slot, Value};
use pxr_types::Ty;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Place {
    Value(Value),
    Slot(Slot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Root {
    Value(Value, bool), // true: pointer packed into i64 for the runtime ABI
    Slot(Slot),
}

pub(crate) fn pointer(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::String | Ty::Array(_) | Ty::Named(_) | Ty::Task(_) | Ty::Mixed
    )
}

pub(crate) fn safepoint(inst: &Inst) -> bool {
    matches!(
        inst,
        Inst::Call { .. }
            | Inst::Print { .. }
            | Inst::Jump(_)
            | Inst::Branch { .. }
            | Inst::Ret { .. }
            | Inst::Bin {
                op: pxr_ir::BinOp::Concat,
                ..
            }
            | Inst::Cast { to: Ty::String, .. }
    )
}

/// Functions with no pointer values or allocation-capable callees need neither
/// root frames nor safepoints. Propagate effects through recursion before LLVM
/// emission; never infer purity from a scalar return signature alone.
pub(crate) fn traced_functions(module: &pxr_ir::Module) -> BTreeSet<String> {
    let names: BTreeSet<_> = module.functions.iter().map(|f| f.name.as_str()).collect();
    let scalar_runtime = [
        "abort",
        "fabs",
        "pxr_abs_i64",
        "pxr_shl",
        "pxr_shr",
        "pxr_div",
        "pxr_mod",
        "pxr_print_i64",
        "pxr_print_f64",
        "pxr_print_bool",
    ];
    let mut traced = BTreeSet::new();
    for f in &module.functions {
        let values = pointer(&f.ret)
            || f.params.iter().chain(&f.slots).any(pointer)
            || f.body.iter().any(|i| match i {
                Inst::Const {
                    lit: pxr_ir::Literal::Str(_),
                    ..
                } => true,
                Inst::Call { ret, callee, .. } => {
                    pointer(ret)
                        || (!names.contains(callee.as_str())
                            && !scalar_runtime.contains(&callee.as_str()))
                }
                Inst::Bin {
                    lhs_ty,
                    rhs_ty,
                    result_ty,
                    ..
                } => [lhs_ty, rhs_ty, result_ty].into_iter().any(pointer),
                Inst::Un {
                    operand_ty,
                    result_ty,
                    ..
                } => pointer(operand_ty) || pointer(result_ty),
                Inst::Cast { from, to, .. } | Inst::BitCast { from, to, .. } => {
                    pointer(from) || pointer(to)
                }
                Inst::Print { ty, .. }
                | Inst::SlotGet { ty, .. }
                | Inst::SlotSet { ty, .. }
                | Inst::SlotAlloc { ty, .. } => pointer(ty),
                _ => false,
            });
        if values {
            traced.insert(f.name.clone());
        }
    }
    loop {
        let mut changed = false;
        for f in &module.functions {
            if !traced.contains(&f.name)
                && f.body
                    .iter()
                    .any(|i| matches!(i, Inst::Call { callee, .. } if traced.contains(callee)))
            {
                traced.insert(f.name.clone());
                changed = true;
            }
        }
        if !changed {
            return traced;
        }
    }
}

/// Before the first possible allocation in a region, scalar control flow
/// cannot accumulate garbage. Keep those loops free of runtime polling calls;
/// calls/allocating casts still publish roots before any collection can occur.
pub(crate) fn allocation_before(f: &Function) -> Vec<bool> {
    let n = f.body.len();
    let labels: HashMap<_, _> = f
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, inst)| {
            if let Inst::Label(l) = inst {
                Some((*l, i))
            } else {
                None
            }
        })
        .collect();
    let mut before = vec![false; n];
    loop {
        let mut changed = false;
        for (i, inst) in f.body.iter().enumerate() {
            let allocates = matches!(
                inst,
                Inst::Call { .. }
                    | Inst::Bin {
                        op: pxr_ir::BinOp::Concat,
                        ..
                    }
                    | Inst::Cast { to: Ty::String, .. }
                    | Inst::Print { ty: Ty::Mixed, .. }
            );
            if !before[i] && !allocates {
                continue;
            }
            let next = match inst {
                Inst::Ret { .. } => vec![],
                Inst::Jump(l) => vec![labels[l]],
                Inst::Branch {
                    then_label,
                    else_label,
                    ..
                } => vec![labels[then_label], labels[else_label]],
                _ if i + 1 < n => vec![i + 1],
                _ => vec![],
            };
            for j in next {
                if !before[j] {
                    before[j] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return before;
        }
    }
}

pub(crate) fn live_roots(f: &Function) -> Vec<Vec<Root>> {
    let mut types: HashMap<Value, Ty> = f
        .params
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, t)| (i as Value, t))
        .collect();
    let mut packed = BTreeSet::new();
    let mut labels = HashMap::new();
    for (i, inst) in f.body.iter().enumerate() {
        if let Inst::Label(label) = inst {
            labels.insert(*label, i);
        }
        let defined = match inst {
            Inst::Const { dst, lit } => Some((*dst, lit.ty())),
            Inst::Bin { dst, result_ty, .. } | Inst::Un { dst, result_ty, .. } => {
                Some((*dst, result_ty.clone()))
            }
            Inst::Cast { dst, to, .. } | Inst::BitCast { dst, to, .. } => Some((*dst, to.clone())),
            Inst::SlotGet { dst, ty, .. } => Some((*dst, ty.clone())),
            Inst::Call {
                dst: Some(dst),
                ret,
                ..
            } => Some((*dst, ret.clone())),
            _ => None,
        };
        if let Some((v, t)) = defined {
            types.insert(v, t);
        }
        if let Inst::BitCast { dst, src, from, to } = inst {
            if pointer(from) && *to == Ty::Int {
                packed.insert(*dst);
            }
            // A getter can return packed pointers before their typed decode.
            if *from == Ty::Int && pointer(to) {
                packed.insert(*src);
            }
        }
    }
    let n = f.body.len();
    let mut live = vec![BTreeSet::<Place>::new(); n];
    loop {
        let mut changed = false;
        for i in (0..n).rev() {
            let inst = &f.body[i];
            let successors: Vec<usize> = match inst {
                Inst::Ret { .. } => vec![],
                Inst::Jump(l) => vec![labels[l]],
                Inst::Branch {
                    then_label,
                    else_label,
                    ..
                } => vec![labels[then_label], labels[else_label]],
                _ if i + 1 < n => vec![i + 1],
                _ => vec![],
            };
            let mut before = BTreeSet::new();
            for next in successors {
                before.extend(live[next].iter().copied());
            }
            if let Some(dst) = inst.defined() {
                before.remove(&Place::Value(dst));
            }
            if let Inst::SlotSet { slot, .. } | Inst::SlotAlloc { slot, .. } = inst {
                before.remove(&Place::Slot(*slot));
            }
            for used in inst.used() {
                before.insert(Place::Value(used));
            }
            if let Inst::SlotGet { slot, .. } = inst {
                before.insert(Place::Slot(*slot));
            }
            if before != live[i] {
                live[i] = before;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    live.into_iter()
        .map(|places| {
            places
                .into_iter()
                .filter_map(|p| match p {
                    Place::Value(v) if types.get(&v).is_some_and(pointer) => {
                        Some(Root::Value(v, false))
                    }
                    Place::Value(v) if packed.contains(&v) => Some(Root::Value(v, true)),
                    Place::Slot(s) if pointer(&f.slots[s as usize]) => Some(Root::Slot(s)),
                    _ => None,
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn noise() -> Inst {
        Inst::Call {
            dst: None,
            callee: "noise".into(),
            args: vec![],
            ret: Ty::Void,
        }
    }
    #[test]
    fn slots_retain_only_live_values() {
        let mut f = Function::new("f", "f", vec![Ty::String], Ty::Void);
        f.alloc_slot(Ty::String);
        f.body = vec![
            Inst::SlotAlloc {
                slot: 0,
                ty: Ty::String,
            },
            Inst::SlotSet {
                slot: 0,
                src: 0,
                ty: Ty::String,
            },
            noise(),
            Inst::SlotGet {
                dst: 1,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Print {
                ty: Ty::String,
                src: 1,
            },
            noise(),
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        let roots = live_roots(&f);
        assert_eq!(roots[2], vec![Root::Slot(0)]);
        assert_eq!(roots[4], vec![Root::Value(1, false)]);
        assert!(roots[5].is_empty());
    }
    #[test]
    fn backedges_keep_loop_carried_roots() {
        let mut f = Function::new("f", "f", vec![Ty::String, Ty::Bool], Ty::Void);
        f.alloc_slot(Ty::String);
        f.body = vec![
            Inst::SlotAlloc {
                slot: 0,
                ty: Ty::String,
            },
            Inst::SlotSet {
                slot: 0,
                src: 0,
                ty: Ty::String,
            },
            Inst::Label(0),
            noise(),
            Inst::SlotGet {
                dst: 2,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Print {
                ty: Ty::String,
                src: 2,
            },
            Inst::Branch {
                cond: 1,
                then_label: 0,
                else_label: 1,
            },
            Inst::Label(1),
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        let roots = live_roots(&f);
        assert_eq!(roots[3], vec![Root::Slot(0)]);
        assert_eq!(roots[6], vec![Root::Slot(0)]);
        assert!(allocation_before(&f)[6]);
        assert!(roots[8].is_empty());
    }
    #[test]
    fn packed_pointer_payloads_are_roots_without_retaining_scalars() {
        let mut f = Function::new("f", "f", vec![Ty::String, Ty::Int], Ty::Void);
        f.body = vec![
            Inst::BitCast {
                dst: 2,
                src: 0,
                from: Ty::String,
                to: Ty::Int,
            },
            Inst::Call {
                dst: None,
                callee: "store".into(),
                args: vec![2, 1],
                ret: Ty::Void,
            },
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        assert_eq!(live_roots(&f)[1], vec![Root::Value(2, true)]);
    }
}

#[cfg(test)]
mod effects_tests {
    use super::*;
    #[test]
    fn allocation_effects_propagate_without_penalizing_scalar_recursion() {
        let mut module = pxr_ir::Module::new();
        let mut scalar = Function::new("scalar", "scalar", vec![Ty::Int], Ty::Int);
        scalar.body = vec![
            Inst::Call {
                dst: Some(1),
                callee: "scalar".into(),
                args: vec![0],
                ret: Ty::Int,
            },
            Inst::Ret {
                ty: Ty::Int,
                src: Some(1),
            },
        ];
        let mut allocation = Function::new("allocation", "allocation", vec![], Ty::Void);
        allocation.body = vec![
            Inst::Call {
                dst: Some(0),
                callee: "pxr_array_new".into(),
                args: vec![],
                ret: Ty::array(Ty::Int),
            },
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        let mut caller = Function::new("caller", "caller", vec![], Ty::Void);
        caller.body = vec![
            Inst::Call {
                dst: None,
                callee: "allocation".into(),
                args: vec![],
                ret: Ty::Void,
            },
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        module.functions = vec![scalar, allocation, caller];
        let mut loop_before_allocation = Function::new("loop", "loop", vec![Ty::Bool], Ty::Void);
        loop_before_allocation.body = vec![
            Inst::Label(0),
            Inst::Branch {
                cond: 0,
                then_label: 0,
                else_label: 1,
            },
            Inst::Label(1),
            Inst::Call {
                dst: None,
                callee: "allocation".into(),
                args: vec![],
                ret: Ty::Void,
            },
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            },
        ];
        let before = allocation_before(&loop_before_allocation);
        assert!(!before[1]);
        assert!(before[4]);
        assert_eq!(
            traced_functions(&module),
            BTreeSet::from(["allocation".into(), "caller".into()])
        );
    }
}
