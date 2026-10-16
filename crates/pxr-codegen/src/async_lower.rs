//! Resumable IR lowering. Ordinary control flow stays ordinary control flow;
//! only await returns to the executor. Frame cells preserve values across polls.
use crate::roots::{live_roots, pointer, Root};
use pxr_ir::{BinOp, Function, Inst, Literal, Module, Value};
use pxr_types::Ty;
use std::collections::{HashMap, HashSet};

pub(crate) fn lower(module: &Module) -> Module {
    let mut out = Module::new();
    for function in &module.functions {
        if function.is_async {
            let (constructor, resume) = lower_function(function);
            out.function(constructor);
            out.function(resume);
        } else {
            out.function(function.clone());
        }
    }
    out
}
struct Builder {
    f: Function,
}
impl Builder {
    fn constant(&mut self, lit: Literal) -> Value {
        let dst = self.f.fresh_value();
        self.f.push(Inst::Const { dst, lit });
        dst
    }
    fn int(&mut self, value: usize) -> Value {
        self.constant(Literal::Int(value as i64))
    }
    fn call(&mut self, name: &str, args: Vec<Value>, ret: Ty) -> Value {
        let dst = self.f.fresh_value();
        self.f.push(Inst::Call {
            dst: (ret != Ty::Void).then_some(dst),
            callee: name.into(),
            args,
            ret,
        });
        dst
    }
    fn cast(&mut self, value: Value, from: &Ty, to: &Ty) -> Value {
        if from == to {
            return value;
        }
        let dst = self.f.fresh_value();
        self.f.push(Inst::BitCast {
            dst,
            src: value,
            from: from.clone(),
            to: to.clone(),
        });
        dst
    }
    fn get(&mut self, task: Value, cell: usize, ty: &Ty) -> Value {
        let index = self.int(cell);
        let bits = self.call("pxr_async_get", vec![task, index], Ty::Int);
        self.cast(bits, &Ty::Int, ty)
    }
    fn set(&mut self, task: Value, cell: usize, value: Value, ty: &Ty, root: bool) {
        let mut bits = self.cast(value, ty, &Ty::Int);
        // Preserve the packed-pointer marker for the ordinary stack-root pass.
        // Raw i64 payloads otherwise look like scalars after frame reloads.
        if root && *ty == Ty::Int {
            let ptr = self.cast(bits, &Ty::Int, &Ty::Named("__pxr_packed".into()));
            bits = self.cast(ptr, &Ty::Named("__pxr_packed".into()), &Ty::Int);
        }
        let index = self.int(cell);
        let traced = self.int(usize::from(root));
        self.call("pxr_async_set", vec![task, index, bits, traced], Ty::Void);
    }
    fn publish(&mut self, roots: &[Root], cells: usize, slots: usize) {
        let mut mask = vec![0; cells.div_ceil(8)];
        for root in roots {
            let index = match root {
                Root::Value(v, _) => 1 + *v as usize,
                Root::Slot(s) => slots + *s as usize,
            };
            mask[index / 8] |= 1 << (index % 8);
        }
        let mask = self.constant(Literal::Str(mask));
        self.call("pxr_async_publish", vec![0, mask], Ty::Void);
    }
    fn ret(&mut self) {
        self.f.push(Inst::Ret {
            ty: Ty::Void,
            src: None,
        });
    }
}
fn lower_function(source: &Function) -> (Function, Function) {
    let roots = live_roots(source);
    let slots = 1 + source.value_count() as usize;
    let cells = slots + source.slots.len();
    let mut types: HashMap<Value, Ty> = source
        .params
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, t)| (i as Value, t))
        .collect();
    for inst in &source.body {
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
            Inst::FunctionAddr { dst, .. } => Some((*dst, Ty::Named("__pxr_callback".into()))),
            _ => None,
        };
        if let Some((v, ty)) = defined {
            types.insert(v, ty);
        }
    }
    let packed: HashSet<_> = roots
        .iter()
        .flatten()
        .filter_map(|r| match r {
            Root::Value(v, true) => Some(*v),
            _ => None,
        })
        .collect();
    let resume_name = format!("{}$resume", source.name);
    let task_ty = Ty::Task(Box::new(source.ret.clone()));
    let control = Ty::Named("__pxr_task".into());
    let mut constructor = Builder {
        f: Function::new(
            &source.name,
            &source.display,
            source.params.clone(),
            task_ty.clone(),
        ),
    };
    let callback = constructor.f.fresh_value();
    constructor.f.push(Inst::FunctionAddr {
        dst: callback,
        name: resume_name.clone(),
    });
    let count = constructor.int(cells);
    let task = constructor.call("pxr_async_create", vec![callback, count], task_ty);
    for (i, ty) in source.params.iter().enumerate() {
        constructor.set(task, i + 1, i as Value, ty, pointer(ty));
    }
    constructor.f.push(Inst::Ret {
        ty: Ty::Task(Box::new(source.ret.clone())),
        src: Some(task),
    });

    let mut b = Builder {
        f: Function::new(
            resume_name,
            format!("{} resume", source.display),
            vec![control.clone(), control],
            Ty::Void,
        ),
    };
    let labels: Vec<_> = (0..source.body.len()).map(|_| b.f.fresh_label()).collect();
    let old_labels: HashMap<_, _> = source
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, inst)| {
            if let Inst::Label(l) = inst {
                Some((*l, labels[i]))
            } else {
                None
            }
        })
        .collect();
    let awaits: HashMap<_, _> = source
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, inst)| {
            matches!(inst, Inst::Call { callee, .. } if callee == "$pxr_await")
                .then(|| (i, b.f.fresh_label()))
        })
        .collect();
    let pc = b.get(0, 0, &Ty::Int);
    let mut entries = vec![(0, labels[0])];
    entries.extend(awaits.iter().map(|(i, label)| (i + 1, *label)));
    entries.sort_unstable();
    for (counter, label) in entries {
        let expected = b.int(counter);
        let matches = b.f.fresh_value();
        let next = b.f.fresh_label();
        b.f.push(Inst::Bin {
            dst: matches,
            op: BinOp::Eq,
            lhs_ty: Ty::Int,
            rhs_ty: Ty::Int,
            result_ty: Ty::Bool,
            a: pc,
            b: expected,
        });
        b.f.push(Inst::Branch {
            cond: matches,
            then_label: label,
            else_label: next,
        });
        b.f.push(Inst::Label(next));
    }
    b.call("abort", vec![], Ty::Void);
    b.ret();
    for (i, inst) in source.body.iter().enumerate() {
        b.f.push(Inst::Label(labels[i]));
        b.publish(&roots[i], cells, slots);
        let mut uses = HashMap::new();
        for value in inst.used() {
            uses.entry(value).or_insert_with(|| {
                let loaded = b.get(0, 1 + value as usize, &types[&value]);
                if packed.contains(&value) && types[&value] == Ty::Int {
                    let ptr = b.cast(loaded, &Ty::Int, &Ty::Named("__pxr_packed".into()));
                    b.cast(ptr, &Ty::Named("__pxr_packed".into()), &Ty::Int)
                } else {
                    loaded
                }
            });
        }
        if let Inst::Call {
            dst,
            callee,
            args,
            ret,
        } = inst
        {
            if callee == "$pxr_await" {
                let pc = b.int(i + 1);
                b.set(0, 0, pc, &Ty::Int, false);
                b.call("pxr_async_await", vec![0, uses[&args[0]]], Ty::Void);
                b.ret();
                b.f.push(Inst::Label(awaits[&i]));
                let child = b.get(0, 1 + args[0] as usize, &types[&args[0]]);
                let bits = b.call("pxr_async_take", vec![child], Ty::Int);
                if let Some(dst) = dst {
                    let value = b.cast(bits, &Ty::Int, ret);
                    b.set(
                        0,
                        1 + *dst as usize,
                        value,
                        ret,
                        pointer(ret) || packed.contains(dst),
                    );
                }
                if i + 1 < labels.len() {
                    b.f.push(Inst::Jump(labels[i + 1]));
                } else {
                    b.ret();
                }
                continue;
            }
            let after: Vec<_> = roots
                .get(i + 1)
                .into_iter()
                .flatten()
                .filter(|r| !matches!(r,Root::Value(v,_) if Some(*v)==*dst))
                .copied()
                .collect();
            b.publish(&after, cells, slots);
        }
        let target = inst.defined().map(|_| b.f.fresh_value());
        let rewritten = match inst {
            Inst::Label(_) | Inst::SlotAlloc { .. } => None,
            Inst::Const { lit, .. } => Some(Inst::Const {
                dst: target.unwrap(),
                lit: lit.clone(),
            }),
            Inst::FunctionAddr { name, .. } => Some(Inst::FunctionAddr {
                dst: target.unwrap(),
                name: name.clone(),
            }),
            Inst::Bin {
                op,
                lhs_ty,
                rhs_ty,
                result_ty,
                a,
                b: rhs,
                ..
            } => Some(Inst::Bin {
                dst: target.unwrap(),
                op: *op,
                lhs_ty: lhs_ty.clone(),
                rhs_ty: rhs_ty.clone(),
                result_ty: result_ty.clone(),
                a: uses[a],
                b: uses[rhs],
            }),
            Inst::Un {
                op,
                operand_ty,
                result_ty,
                a,
                ..
            } => Some(Inst::Un {
                dst: target.unwrap(),
                op: *op,
                operand_ty: operand_ty.clone(),
                result_ty: result_ty.clone(),
                a: uses[a],
            }),
            Inst::Cast {
                op, from, to, src, ..
            } => Some(Inst::Cast {
                dst: target.unwrap(),
                op: *op,
                from: from.clone(),
                to: to.clone(),
                src: uses[src],
            }),
            Inst::BitCast { from, to, src, .. } => Some(Inst::BitCast {
                dst: target.unwrap(),
                from: from.clone(),
                to: to.clone(),
                src: uses[src],
            }),
            Inst::Call {
                callee, args, ret, ..
            } => Some(Inst::Call {
                dst: target,
                callee: callee.clone(),
                args: args.iter().map(|v| uses[v]).collect(),
                ret: ret.clone(),
            }),
            Inst::SlotGet { slot, ty, .. } => {
                let value = b.get(0, slots + *slot as usize, ty);
                b.set(
                    0,
                    1 + inst.defined().unwrap() as usize,
                    value,
                    ty,
                    pointer(ty),
                );
                None
            }
            Inst::SlotSet { slot, src, ty } => {
                b.set(0, slots + *slot as usize, uses[src], ty, pointer(ty));
                None
            }
            Inst::Print { ty, src } => Some(Inst::Print {
                ty: ty.clone(),
                src: uses[src],
            }),
            Inst::Jump(label) => Some(Inst::Jump(old_labels[label])),
            Inst::Branch {
                cond,
                then_label,
                else_label,
            } => Some(Inst::Branch {
                cond: uses[cond],
                then_label: old_labels[then_label],
                else_label: old_labels[else_label],
            }),
            Inst::Ret { ty, src } => {
                let tag = b.int(tag(ty));
                let bits = src
                    .map(|v| b.cast(uses[&v], ty, &Ty::Int))
                    .unwrap_or_else(|| b.int(0));
                b.call("pxr_async_finish", vec![0, tag, bits], Ty::Void);
                b.ret();
                None
            }
        };
        if let Some(rewritten) = rewritten {
            b.f.push(rewritten);
            if let (Some(old), Some(new)) = (inst.defined(), target) {
                let ty = &types[&old];
                b.set(
                    0,
                    1 + old as usize,
                    new,
                    ty,
                    pointer(ty) || packed.contains(&old),
                );
            }
        }
        if !matches!(inst, Inst::Jump(_) | Inst::Branch { .. } | Inst::Ret { .. }) {
            if i + 1 < labels.len() {
                b.f.push(Inst::Jump(labels[i + 1]));
            } else {
                b.ret();
            }
        }
    }
    (constructor.f, b.f)
}
fn tag(ty: &Ty) -> usize {
    match ty {
        Ty::Void | Ty::Null => 0,
        Ty::Int => 1,
        Ty::Float => 2,
        Ty::Bool => 3,
        Ty::String => 4,
        Ty::Array(_) => 5,
        Ty::Named(_) => 6,
        Ty::Mixed => 7,
        Ty::Task(_) | Ty::Infer => unreachable!("invalid async result type"),
    }
}
