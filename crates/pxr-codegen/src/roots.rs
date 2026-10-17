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
        Ty::String | Ty::Array(_) | Ty::Named(_) | Ty::Task(_) | Ty::TaskResult(_) | Ty::Mixed
    )
}

/// These helpers neither allocate managed memory nor run callbacks/collection.
/// Pointer arguments/results alone do not require a GC frame. Unknown calls
/// remain effectful, including mixed operations and native/PHP stringifiers.
fn runtime_leaf(callee: &str) -> bool {
    matches!(
        callee,
        "abort"
            | "fabs"
            | "pxr_abs_i64"
            | "pxr_shl"
            | "pxr_shr"
            | "pxr_div"
            | "pxr_mod"
            | "pxr_object_get_bits"
            | "pxr_object_set_bits"
            | "pxr_object_get_class_id"
            | "pxr_instanceof"
            | "pxr_array_count"
            | "pxr_array_val_bits_at"
            | "pxr_strlen"
            | "pxr_str_to_i64"
            | "pxr_str_to_f64"
            | "pxr_truthy_str"
            | "pxr_truthy_i64"
            | "pxr_truthy_f64"
            | "pxr_str_eq"
            | "pxr_str_cmp"
            | "pxr_str_cmp_loose"
            | "pxr_str_freeze"
            | "pxr_mixed_key_tag"
            | "pxr_mixed_key_int"
            | "pxr_mixed_key_str"
    )
}

fn allocates(inst: &Inst, leaf_functions: &BTreeSet<String>) -> bool {
    match inst {
        Inst::Call { callee, .. } => !runtime_leaf(callee) && !leaf_functions.contains(callee),
        Inst::Print { .. } // HTTP output can grow its response buffer.
        | Inst::Bin { op: pxr_ir::BinOp::Concat, .. }
        | Inst::Cast { to: Ty::String, .. } => true,
        _ => false,
    }
}

pub(crate) fn safepoint(inst: &Inst, leaf_functions: &BTreeSet<String>) -> bool {
    allocates(inst, leaf_functions) || matches!(inst, Inst::Ret { .. })
}

/// Propagate allocation/collection effects through the whole call graph,
/// including recursive components. Never infer effects from return types.
/// Allocation-free pointer getters and constructors need no roots or polls.
pub(crate) fn traced_functions(module: &pxr_ir::Module) -> BTreeSet<String> {
    let names: BTreeSet<_> = module.functions.iter().map(|f| f.name.clone()).collect();
    let mut traced: BTreeSet<_> = module
        .functions
        .iter()
        .filter(|f| f.body.iter().any(|i| allocates(i, &names)))
        .map(|f| f.name.clone())
        .collect();
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

/// Track whether a path may have allocated before a return. Allocating
/// operations already poll before execution, so nonallocating branch/loop edges
/// need no additional polls. Returns retain a final collection opportunity,
/// with the returned value published as a live root.
pub(crate) fn allocation_before(f: &Function, leaf_functions: &BTreeSet<String>) -> Vec<bool> {
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
            let pending =
                allocates(inst, leaf_functions) || (before[i] && !safepoint(inst, leaf_functions));
            if !pending {
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
        assert!(allocation_before(&f, &BTreeSet::new())[6]);
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
    fn pointer_getters_and_setters_need_no_frame_but_allocating_callees_do() {
        let node = Ty::Named("Node".into());
        let mut getter = Function::new("getter", "getter", vec![node.clone()], Ty::String);
        getter.body = vec![
            Inst::Const {
                dst: 1,
                lit: pxr_ir::Literal::Int(0),
            },
            Inst::Call {
                dst: Some(2),
                callee: "pxr_object_get_bits".into(),
                args: vec![0, 1],
                ret: Ty::Int,
            },
            Inst::BitCast {
                dst: 3,
                src: 2,
                from: Ty::Int,
                to: Ty::String,
            },
            Inst::Ret {
                ty: Ty::String,
                src: Some(3),
            },
        ];
        let mut setter = Function::new("setter", "setter", vec![node, Ty::Int], Ty::Void);
        setter.body = vec![Inst::Call {
            dst: None,
            callee: "pxr_object_set_bits".into(),
            args: vec![0, 1, 1, 1],
            ret: Ty::Void,
        }];
        let mut identity = Function::new("identity", "identity", vec![Ty::String], Ty::String);
        identity.body = vec![Inst::Ret {
            ty: Ty::String,
            src: Some(0),
        }];
        let mut caller = Function::new("caller", "caller", vec![Ty::String], Ty::Void);
        caller.body = vec![
            Inst::Call {
                dst: Some(1),
                callee: "identity".into(),
                args: vec![0],
                ret: Ty::String,
            },
            Inst::Call {
                dst: None,
                callee: "unknown_callback".into(),
                args: vec![],
                ret: Ty::Void,
            },
            Inst::Print {
                ty: Ty::String,
                src: 1,
            },
        ];
        assert_eq!(live_roots(&caller)[1], vec![Root::Value(1, false)]);
        let mut module = pxr_ir::Module::new();
        module.functions = vec![getter, setter, identity, caller];
        assert_eq!(traced_functions(&module), BTreeSet::from(["caller".into()]));
    }

    #[test]
    fn collection_effects_cross_recursive_calls_even_with_scalar_results() {
        let mut a = Function::new("a", "a", vec![], Ty::Int);
        a.body = vec![Inst::Call {
            dst: Some(0),
            callee: "b".into(),
            args: vec![],
            ret: Ty::Int,
        }];
        let mut b = Function::new("b", "b", vec![], Ty::Int);
        b.body = vec![
            Inst::Call {
                dst: Some(0),
                callee: "a".into(),
                args: vec![],
                ret: Ty::Int,
            },
            Inst::Call {
                dst: None,
                callee: "pxr_gc_collect".into(),
                args: vec![],
                ret: Ty::Void,
            },
        ];
        let mut module = pxr_ir::Module::new();
        module.functions = vec![a, b];
        assert_eq!(
            traced_functions(&module),
            BTreeSet::from(["a".into(), "b".into()])
        );
    }

    #[test]
    fn allocating_operations_poll_without_polling_nonallocating_loop_edges() {
        let leaves = BTreeSet::from(["getter".into()]);
        assert!(!safepoint(&Inst::Jump(0), &leaves));
        assert!(!safepoint(
            &Inst::Branch {
                cond: 0,
                then_label: 0,
                else_label: 1
            },
            &leaves
        ));
        assert!(!safepoint(
            &Inst::Call {
                dst: Some(1),
                callee: "getter".into(),
                args: vec![0],
                ret: Ty::String
            },
            &leaves
        ));
        assert!(safepoint(
            &Inst::Call {
                dst: Some(1),
                callee: "pxr_object_new".into(),
                args: vec![],
                ret: Ty::Mixed
            },
            &leaves
        ));
        assert!(safepoint(
            &Inst::Call {
                dst: Some(1),
                callee: "pxr_mixed_str".into(),
                args: vec![0],
                ret: Ty::String
            },
            &leaves
        ));
        assert!(safepoint(
            &Inst::Print {
                ty: Ty::Int,
                src: 0
            },
            &leaves
        ));
    }

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
        let before = allocation_before(&loop_before_allocation, &BTreeSet::new());
        assert!(!before[1]);
        assert!(before[4]);
        assert_eq!(
            traced_functions(&module),
            BTreeSet::from(["allocation".into(), "caller".into()])
        );
    }
}
