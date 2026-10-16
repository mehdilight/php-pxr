//! Reuse string accumulator capacity until an observable snapshot is taken.
//!
//! Storing a loaded value elsewhere, returning it, boxing it, borrowing it, or
//! passing it to an unknown call first freezes the backing allocation. A load
//! that survives an overwrite also freezes it, preserving old SSA values.
//! Ordinary concatenation must keep producing fresh, immutable strings. The only
//! calls allowed below either return scalars or allocate an independent result.
use pxr_ir::{BinOp, Function, Inst, Module, Slot, Value};
use pxr_types::Ty;
use std::collections::{HashMap, HashSet};

pub(crate) fn optimize(module: &mut Module) {
    for function in &mut module.functions {
        optimize_function(function);
    }
}

fn reads_without_aliasing(inst: &Inst) -> bool {
    match inst {
        Inst::Print { ty: Ty::String, .. } | Inst::Bin { .. } => true,
        Inst::Cast { to, .. } => matches!(to, Ty::Int | Ty::Float | Ty::Bool),
        Inst::Call { callee, .. } => matches!(
            callee.as_str(),
            "pxr_strlen"
                | "pxr_str_to_i64"
                | "pxr_str_to_f64"
                | "pxr_truthy_str"
                | "pxr_str_eq"
                | "pxr_str_cmp"
                | "pxr_str_cmp_loose"
                | "pxr_print_str"
                | "pxr_substr"
        ),
        _ => false,
    }
}

fn optimize_function(f: &mut Function) {
    let mut uses: HashMap<Value, Vec<usize>> = HashMap::new();
    let mut loads = HashMap::new();
    let mut writes: HashMap<Slot, Vec<usize>> = HashMap::new();
    let mut blocks = Vec::with_capacity(f.body.len());
    let mut block = 0;
    for (i, inst) in f.body.iter().enumerate() {
        if matches!(inst, Inst::Label(_)) {
            block += 1;
        }
        blocks.push(block);
        for value in inst.used() {
            uses.entry(value).or_default().push(i);
        }
        match inst {
            Inst::SlotGet {
                dst,
                slot,
                ty: Ty::String,
            } => {
                loads.insert(*dst, (*slot, i));
            }
            Inst::SlotSet { slot, .. } => writes.entry(*slot).or_default().push(i),
            _ => {}
        }
    }
    let mut candidates = HashMap::new();
    for (i, pair) in f.body.windows(2).enumerate() {
        if let [Inst::Bin {
            dst,
            op: BinOp::Concat,
            a,
            ..
        }, Inst::SlotSet {
            slot,
            src,
            ty: Ty::String,
        }] = pair
        {
            if src == dst
                && loads.get(a).is_some_and(|(source, _)| source == slot)
                && uses
                    .get(dst)
                    .is_some_and(|sites| sites.as_slice() == [i + 1])
            {
                candidates.insert(i, *slot);
            }
        }
    }
    let accumulators: HashSet<_> = candidates.values().copied().collect();
    let mut freezes = HashMap::new();
    for (value, (slot, definition)) in &loads {
        if !accumulators.contains(slot) {
            continue;
        }
        let Some(sites) = uses.get(value) else {
            continue;
        };
        let last = *sites.iter().max().unwrap();
        // Do not let an old SSA load observe a later in-place mutation. Reject
        // cross-block loads conservatively by freezing their backing storage.
        let local_lifetime = sites.iter().all(|i| {
            *i > *definition
                && blocks[*i] == blocks[*definition]
                && reads_without_aliasing(&f.body[*i])
        }) && !writes
            .get(slot)
            .is_some_and(|sites| sites.iter().any(|i| *i > *definition && *i < last));
        if !local_lifetime {
            freezes.insert(*definition, *value);
        }
    }
    for i in candidates.keys() {
        let i = *i;
        let Inst::Bin { dst, a, b, .. } = f.body[i] else {
            unreachable!()
        };
        f.body[i] = Inst::Call {
            dst: Some(dst),
            callee: "pxr_str_append_local".into(),
            args: vec![a, b],
            ret: Ty::String,
        };
    }
    if !freezes.is_empty() {
        let mut body = Vec::with_capacity(f.body.len() + freezes.len());
        for (i, inst) in f.body.drain(..).enumerate() {
            body.push(inst);
            if let Some(value) = freezes.get(&i) {
                body.push(Inst::Call {
                    dst: None,
                    callee: "pxr_str_freeze".into(),
                    args: vec![*value],
                    ret: Ty::Void,
                });
            }
        }
        f.body = body;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulator() -> Function {
        let mut f = Function::new("test", "test", vec![], Ty::Void);
        f.slots = vec![Ty::String, Ty::String];
        f.body = vec![
            Inst::SlotGet {
                dst: 0,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Const {
                dst: 1,
                lit: pxr_ir::Literal::Str(b"x".to_vec()),
            },
            Inst::Bin {
                dst: 2,
                op: BinOp::Concat,
                lhs_ty: Ty::String,
                rhs_ty: Ty::String,
                result_ty: Ty::String,
                a: 0,
                b: 1,
            },
            Inst::SlotSet {
                slot: 0,
                src: 2,
                ty: Ty::String,
            },
        ];
        f
    }
    fn freezes(f: &Function) -> Vec<Value> {
        f.body
            .iter()
            .filter_map(|i| match i {
                Inst::Call { callee, args, .. } if callee == "pxr_str_freeze" => Some(args[0]),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn snapshots_and_unknown_calls_freeze_but_scalar_reads_do_not() {
        let mut f = accumulator();
        f.body.extend([
            Inst::SlotGet {
                dst: 3,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Call {
                dst: Some(4),
                callee: "pxr_strlen".into(),
                args: vec![3],
                ret: Ty::Int,
            },
            Inst::SlotGet {
                dst: 5,
                slot: 0,
                ty: Ty::String,
            },
            Inst::SlotSet {
                slot: 1,
                src: 5,
                ty: Ty::String,
            },
            Inst::SlotGet {
                dst: 6,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Call {
                dst: None,
                callee: "user".into(),
                args: vec![6],
                ret: Ty::Void,
            },
        ]);
        optimize_function(&mut f);
        assert_eq!(freezes(&f), vec![5, 6]);
        assert!(f
            .body
            .iter()
            .any(|i| matches!(i, Inst::Call { callee, .. } if callee == "pxr_str_append_local")));
    }
    #[test]
    fn old_loads_and_cross_block_values_freeze_before_mutation() {
        let mut f = accumulator();
        f.body.extend([
            Inst::Print {
                ty: Ty::String,
                src: 0,
            },
            Inst::SlotGet {
                dst: 3,
                slot: 0,
                ty: Ty::String,
            },
            Inst::Jump(0),
            Inst::Label(0),
            Inst::Print {
                ty: Ty::String,
                src: 3,
            },
        ]);
        optimize_function(&mut f);
        assert_eq!(freezes(&f), vec![0, 3]);
        assert!(matches!(&f.body[1], Inst::Call { callee, .. } if callee == "pxr_str_freeze"));
    }
    #[test]
    fn concat_result_with_an_extra_alias_is_never_mutated() {
        let mut f = accumulator();
        f.body.push(Inst::SlotSet {
            slot: 1,
            src: 2,
            ty: Ty::String,
        });
        optimize_function(&mut f);
        assert!(!f
            .body
            .iter()
            .any(|i| matches!(i, Inst::Call { callee, .. } if callee == "pxr_str_append_local")));
    }
}
