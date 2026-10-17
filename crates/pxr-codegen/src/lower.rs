//! Typed AST -> IR lowering.
//!
//! The lowering pass is where the "PHP surface" is desugared into an explicit,
//! fully typed representation:
//!
//! * `int`/`float` mixing inserts `IntToFloat` casts,
//! * `.` inserts `IntToStr`/`FloatToStr`/`BoolToStr` casts around operands,
//! * `&&` / `||` become real short-circuit control flow (a value is threaded
//!   through a compiler-private slot, since the IR is deliberately phi-free),
//! * builtins become explicit runtime calls or casts,
//! * `if` / `while` become basic blocks.

use pxr_ir::{BinOp as IBinOp, CastOp, Function, Inst, Label, Literal, Module, Slot, UnOp, Value};
use pxr_sema::{Builtin, Callee, TExpr, TFunc, TProgram, TStmt, TTarget};
use pxr_types::Ty;

pub fn lower_program(prog: &TProgram) -> Module {
    let mut m = Module::new();
    for f in &prog.funcs {
        m.function(lower_func(f));
        if prog.parallel_handlers.contains(&f.name) {
            let ty = &f.params[0].1;
            let mut bridge = Lowerer::new(Function::new(
                format!("pxr$parallel${}", f.name),
                "parallel bridge",
                vec![Ty::Int, Ty::Int],
                Ty::Task(Box::new(f.ret.clone())),
            ));
            let input = bridge.decode_bits(1, ty);
            let task = bridge.rt_call(&f.name, vec![input], Ty::Task(Box::new(f.ret.clone())));
            bridge.terminator(Inst::Ret {
                ty: Ty::Task(Box::new(f.ret.clone())),
                src: Some(task),
            });
            m.function(bridge.into_function());
        }
    }
    if !prog.parallel_handlers.is_empty() {
        let mut bootstrap = prog.main.clone();
        bootstrap.name = "pxr_parallel_bootstrap".into();
        bootstrap.body = prog.worker_bootstrap.clone();
        m.function(lower_main(&bootstrap, &prog.class_registrations));
    }
    m.function(lower_stringifier(&prog.stringifiers));
    m.function(lower_main(&prog.main, &prog.class_registrations));
    m
}

fn lower_stringifier(stringifiers: &[(u32, String)]) -> Function {
    let object_ty = Ty::Named("__pxr_object".into());
    let mut lower = Lowerer::new(Function::new(
        "pxr_native_object_to_string",
        "native __toString dispatch",
        vec![object_ty],
        Ty::String,
    ));
    let class = lower.rt_call("pxr_object_get_class_id", vec![0], Ty::Int);
    for (id, method) in stringifiers {
        let expected = lower.emit_const(Literal::Int(*id as i64));
        let matches = lower.bin(IBinOp::Eq, class, &Ty::Int, expected, &Ty::Int, &Ty::Bool);
        let body = lower.fresh_label();
        let next = lower.fresh_label();
        lower.terminator(Inst::Branch {
            cond: matches,
            then_label: body,
            else_label: next,
        });
        lower.switch_to(body);
        let result = lower.rt_call(method, vec![0], Ty::String);
        lower.terminator(Inst::Ret {
            ty: Ty::String,
            src: Some(result),
        });
        lower.switch_to(next);
    }
    let null = lower.emit_const(Literal::Null);
    lower.terminator(Inst::Ret {
        ty: Ty::String,
        src: Some(null),
    });
    lower.into_function()
}

fn lower_main(tf: &TFunc, class_registrations: &[(u32, u32)]) -> Function {
    let params: Vec<Ty> = tf.params.iter().map(|(_, t)| t.clone()).collect();
    let mut lw = Lowerer::new(Function::new(
        tf.name.clone(),
        tf.display.clone(),
        params,
        tf.ret.clone(),
    ));

    for ty in tf.slots.iter() {
        let slot = lw.f.alloc_slot(ty.clone());
        lw.emit_prelude(Inst::SlotAlloc {
            slot,
            ty: ty.clone(),
        });
    }
    for (i, (_, ty)) in tf.params.iter().enumerate() {
        lw.emit_prelude(Inst::SlotSet {
            slot: i as Slot,
            src: i as Value,
            ty: ty.clone(),
        });
    }

    for (cid, super_id) in class_registrations {
        let cid_val = lw.emit_const(Literal::Int(*cid as i64));
        let super_val = lw.emit_const(Literal::Int(*super_id as i64));
        lw.rt_call("pxr_class_register", vec![cid_val, super_val], Ty::Void);
    }

    lw.lower_stmts(&tf.body);
    lw.finish();
    lw.into_function()
}

fn lower_func(tf: &TFunc) -> Function {
    let params: Vec<Ty> = tf.params.iter().map(|(_, t)| t.clone()).collect();
    let mut lw = Lowerer::new(Function::new(
        tf.name.clone(),
        tf.display.clone(),
        params,
        tf.ret.clone(),
    ));

    lw.f.is_async = tf.is_async;

    // Slots live in the entry block so LLVM can promote them to registers.
    // Registering them through `alloc_slot` keeps the slot table in sync so
    // that compiler-private temporaries (short-circuit results) get fresh ids.
    for ty in tf.slots.iter() {
        let slot = lw.f.alloc_slot(ty.clone());
        lw.emit_prelude(Inst::SlotAlloc {
            slot,
            ty: ty.clone(),
        });
    }
    // Copy incoming parameters into their slots. `pxr-sema` guarantees that the
    // first `params.len()` slots correspond to the parameters, in order.
    for (i, (_, ty)) in tf.params.iter().enumerate() {
        lw.emit_prelude(Inst::SlotSet {
            slot: i as Slot,
            src: i as Value,
            ty: ty.clone(),
        });
    }

    lw.lower_stmts(&tf.body);
    lw.finish();
    lw.into_function()
}

enum ArrayBase {
    Slot { slot: u32, storage: Ty, view: Ty },
    Value(Value),
}
enum PreparedTarget {
    Slot {
        slot: u32,
        storage: Ty,
    },
    Array {
        base: ArrayBase,
        key: Option<(Value, Value, Value)>,
    },
    Property {
        object: Value,
        index: Value,
    },
    Nested {
        parent: Box<PreparedTarget>,
        parent_ty: Ty,
        array_ty: Ty,
        key: Option<(Value, Value, Value)>,
    },
}

struct Lowerer {
    f: Function,
    prelude: Vec<Inst>,
    terminated: bool,
    /// Targets for `break` / `continue`, innermost first. A `switch` pushes an
    /// entry whose `cont` equals its `brk` (PHP treats `continue` inside a
    /// `switch` as breaking the `switch`).
    loops: Vec<LoopCtx>,
}

/// `break` / `continue` destinations for one enclosing loop or `switch`.
struct LoopCtx {
    brk: Label,
    cont: Label,
}

impl Lowerer {
    fn new(f: Function) -> Self {
        Lowerer {
            f,
            prelude: Vec::new(),
            terminated: false,
            loops: Vec::new(),
        }
    }

    fn emit(&mut self, inst: Inst) {
        self.f.body.push(inst);
    }

    fn emit_prelude(&mut self, inst: Inst) {
        self.prelude.push(inst);
    }

    fn fresh(&mut self) -> Value {
        self.f.fresh_value()
    }

    fn fresh_label(&mut self) -> Label {
        self.f.fresh_label()
    }

    /// Emit a terminator and mark the current block closed.
    fn terminator(&mut self, inst: Inst) {
        self.emit(inst);
        self.terminated = true;
    }

    /// Close the current block with a jump to `l` (if needed) and start it.
    fn switch_to(&mut self, l: Label) {
        if !self.terminated {
            self.terminator(Inst::Jump(l));
        }
        self.emit(Inst::Label(l));
        self.terminated = false;
    }

    fn alloc_temp_slot(&mut self, ty: Ty) -> Slot {
        let slot = self.f.alloc_slot(ty.clone());
        self.emit_prelude(Inst::SlotAlloc { slot, ty });
        slot
    }

    fn finish(&mut self) {
        if self.terminated {
            return;
        }
        let ret = self.f.ret.clone();
        let inst = if ret == Ty::Void {
            Inst::Ret {
                ty: Ty::Void,
                src: None,
            }
        } else {
            // Defensive: semantic analysis rejects fall-through, but if it ever
            // happens we emit a well-typed zero rather than invalid IR.
            let zero = self.zero_value(&ret);
            Inst::Ret {
                ty: ret,
                src: Some(zero),
            }
        };
        self.terminator(inst);
    }

    fn zero_value(&mut self, ty: &Ty) -> Value {
        let dst = self.fresh();
        let lit = match ty {
            Ty::Int => Literal::Int(0),
            Ty::Float => Literal::Float(0.0),
            Ty::Bool => Literal::Bool(false),
            Ty::String => Literal::Str(Vec::new()),
            Ty::Mixed => Literal::Null,
            _ => Literal::Str(Vec::new()),
        };
        self.emit_prelude(Inst::Const { dst, lit });
        dst
    }

    fn into_function(mut self) -> Function {
        let mut body = std::mem::take(&mut self.prelude);
        body.append(&mut self.f.body);
        self.f.body = body;
        self.f
    }

    // ---- Statements -------------------------------------------------------

    fn lower_stmts(&mut self, stmts: &[TStmt]) {
        for s in stmts {
            if self.terminated {
                break;
            }
            self.lower_stmt(s);
        }
    }

    fn lower_stmt(&mut self, stmt: &TStmt) {
        match stmt {
            TStmt::PhpInclude { path, mode } => {
                let path = self.emit_const(Literal::Str(path.as_bytes().to_vec()));
                let mode = self.emit_const(Literal::Int(*mode as i64));
                self.rt_call("pxr_php_include", vec![path, mode], Ty::Void);
            }
            TStmt::Block(body) => self.lower_stmts(body),
            TStmt::Echo(exprs) => {
                for e in exprs {
                    let v = self.lower_expr(e);
                    let ty = e.ty();
                    let (ty, v) = if matches!(ty, Ty::Named(_)) {
                        (Ty::Mixed, self.cast(v, &ty, &Ty::Mixed))
                    } else {
                        (ty, v)
                    };
                    self.emit(Inst::Print { ty, src: v });
                }
            }
            TStmt::Print(e) => {
                let v = self.lower_expr(e);
                let ty = e.ty();
                let (ty, v) = if matches!(ty, Ty::Named(_)) {
                    (Ty::Mixed, self.cast(v, &ty, &Ty::Mixed))
                } else {
                    (ty, v)
                };
                self.emit(Inst::Print { ty, src: v });
            }
            TStmt::Expr(e) => {
                let value = self.lower_expr(e);
                if matches!(e.ty(), Ty::Task(_)) {
                    self.rt_call("pxr_async_drop", vec![value], Ty::Void);
                }
            }
            TStmt::Assign { .. } | TStmt::AssignOp { .. } | TStmt::IncDec { .. } => {
                self.lower_assignment(stmt, false);
            }
            TStmt::If {
                cond,
                cond_ty,
                then_branch,
                elifs,
                else_branch,
            } => self.lower_if(cond, cond_ty, then_branch, elifs, else_branch),
            TStmt::While {
                cond,
                cond_ty,
                body,
            } => self.lower_while(cond, cond_ty, body),
            TStmt::For {
                init,
                cond,
                update,
                body,
            } => self.lower_for(init, cond, update, body),
            TStmt::DoWhile {
                body,
                cond,
                cond_ty,
            } => self.lower_do_while(body, cond, cond_ty),
            TStmt::Foreach {
                base,
                base_ty,
                key_slot,
                value_slot,
                value_ty,
                body,
            } => self.lower_foreach(base, base_ty, *key_slot, *value_slot, value_ty, body),
            TStmt::Switch {
                subject,
                subject_ty,
                cases,
            } => self.lower_switch(subject, subject_ty, cases),
            TStmt::Break(level) => self.lower_jump(false, *level),
            TStmt::Continue(level) => self.lower_jump(true, *level),
            TStmt::Return { value, ret } => {
                let v = value.as_ref().map(|e| {
                    let value = self.lower_expr(e);
                    let value = self.cast(value, &e.ty(), ret);
                    if is_pass(e) {
                        value
                    } else {
                        self.maybe_copy_array(value, ret)
                    }
                });
                self.terminator(Inst::Ret {
                    ty: ret.clone(),
                    src: v,
                });
            }
        }
    }

    fn lower_if(
        &mut self,
        cond: &TExpr,
        cond_ty: &Ty,
        then_branch: &[TStmt],
        elifs: &[(TExpr, Ty, Vec<TStmt>)],
        else_branch: &Option<Vec<TStmt>>,
    ) {
        let end = self.fresh_label();
        let c = self.lower_cond(cond, cond_ty);
        let then_l = self.fresh_label();
        let else_l = self.fresh_label();
        self.terminator(Inst::Branch {
            cond: c,
            then_label: then_l,
            else_label: else_l,
        });

        self.switch_to(then_l);
        self.lower_stmts(then_branch);
        if !self.terminated {
            self.terminator(Inst::Jump(end));
        }

        self.switch_to(else_l);
        for (ec, ety, eb) in elifs {
            let c = self.lower_cond(ec, ety);
            let t2 = self.fresh_label();
            let e2 = self.fresh_label();
            self.terminator(Inst::Branch {
                cond: c,
                then_label: t2,
                else_label: e2,
            });
            self.switch_to(t2);
            self.lower_stmts(eb);
            if !self.terminated {
                self.terminator(Inst::Jump(end));
            }
            self.switch_to(e2);
        }
        if let Some(eb) = else_branch {
            self.lower_stmts(eb);
        }
        if !self.terminated {
            self.terminator(Inst::Jump(end));
        }
        self.switch_to(end);
    }

    fn lower_while(&mut self, cond: &TExpr, cond_ty: &Ty, body: &[TStmt]) {
        let head = self.fresh_label();
        let body_l = self.fresh_label();
        let end = self.fresh_label();
        self.loops.push(LoopCtx {
            brk: end,
            cont: head,
        });

        self.switch_to(head);
        let c = self.lower_cond(cond, cond_ty);
        self.terminator(Inst::Branch {
            cond: c,
            then_label: body_l,
            else_label: end,
        });

        self.switch_to(body_l);
        self.lower_stmts(body);
        if !self.terminated {
            self.terminator(Inst::Jump(head));
        }

        self.switch_to(end);
        self.loops.pop();
    }

    fn lower_cond(&mut self, cond: &TExpr, cond_ty: &Ty) -> Value {
        let v = self.lower_expr(cond);
        self.to_bool(v, cond_ty)
    }

    // ---- Expressions ------------------------------------------------------

    fn lower_assignment(&mut self, statement: &TStmt, postfix: bool) -> Value {
        match statement {
            TStmt::Assign { target, ty, value } => {
                let place = self.prepare_target(target);
                let mut v = self.lower_expr(value);
                let vt = value.ty();
                if vt != *ty {
                    v = self.cast(v, &vt, ty);
                }
                if is_pass(value) {
                    if let TTarget::Var { slot, .. } = target {
                        self.emit(Inst::SlotSet {
                            slot: *slot,
                            src: v,
                            ty: ty.clone(),
                        });
                    } else {
                        self.write_target(&place, ty, v);
                    }
                } else {
                    self.write_target(&place, ty, v);
                }
                v
            }
            TStmt::AssignOp {
                target,
                ty,
                op,
                value,
            } => {
                // `??=` must not evaluate its right-hand side when the target
                // is already set (and for a plain variable it is always set).
                if *op == pxr_sema::TBinOp::NullCoalesce {
                    return self.lower_coalesce_assign(target, &target.storage_ty(), value);
                }
                // Read the current value, apply the operation, store back.
                let place = self.prepare_target(target);
                let v = self.lower_expr(value);
                let vt = value.ty();
                let cur_ty = ty.clone();
                let cur = self.read_target(&place, &cur_ty);
                let result = self.lower_bin_expr(*op, &cur_ty, cur, &vt, v);
                // Coerce the operator result to the destination representation.
                let storage = target.storage_ty();
                let value = self.cast(result.0, &result.1, &storage);
                self.write_target(&place, &storage, value);
                value
            }
            TStmt::IncDec { target, ty, delta } => {
                let place = self.prepare_target(target);
                let cur_ty = ty.clone();
                let cur = self.read_target(&place, &cur_ty);
                let dx = self.emit_const(Literal::Int(*delta));
                let res = self.lower_bin_expr(pxr_sema::TBinOp::Add, &cur_ty, cur, &Ty::Int, dx);
                let storage = target.storage_ty();
                let value = self.cast(res.0, &res.1, &storage);
                self.write_target(&place, &storage, value);
                if postfix {
                    cur
                } else {
                    res.0
                }
            }
            _ => unreachable!("effect expression must contain an assignment"),
        }
    }

    fn lower_expr(&mut self, e: &TExpr) -> Value {
        match e {
            TExpr::Effect {
                statement, postfix, ..
            } => self.lower_assignment(statement, *postfix),
            TExpr::Deferred => unreachable!("deferred inference cannot reach lowering"),
            TExpr::Pass { expr, .. } => self.lower_expr(expr),
            TExpr::Coerce { expr, ty } => {
                let v = self.lower_expr(expr);
                self.cast(v, &expr.ty(), ty)
            }
            TExpr::Int(v) => self.emit_const(Literal::Int(*v)),
            TExpr::Float(v) => self.emit_const(Literal::Float(*v)),
            TExpr::Str(s) => self.emit_const(Literal::Str(s.clone())),
            TExpr::Bool(b) => self.emit_const(Literal::Bool(*b)),
            TExpr::Null => {
                let zero = self.emit_const(Literal::Int(0));
                self.rt_call("pxr_mixed_box", vec![zero, zero], Ty::Mixed)
            }
            TExpr::Var { slot, ty, .. } => {
                let dst = self.fresh();
                self.emit(Inst::SlotGet {
                    dst,
                    slot: *slot,
                    ty: ty.clone(),
                });
                dst
            }
            TExpr::Un {
                op,
                operand_ty,
                ty,
                expr,
            } => {
                let a = self.lower_expr(expr);
                if *operand_ty == Ty::Mixed {
                    if *op == pxr_sema::TUnOp::Neg {
                        return self.rt_call("pxr_mixed_negate", vec![a], Ty::Mixed);
                    }
                    if *op == pxr_sema::TUnOp::BitNot {
                        return self.rt_call("pxr_mixed_bit_not", vec![a], Ty::Mixed);
                    }
                }
                let (a, op_ty) = match op {
                    pxr_sema::TUnOp::Not => (self.to_bool(a, operand_ty), Ty::Bool),
                    pxr_sema::TUnOp::Neg => (a, operand_ty.clone()),
                    pxr_sema::TUnOp::BitNot => (a, operand_ty.clone()),
                };
                let dst = self.fresh();
                let uop = match op {
                    pxr_sema::TUnOp::Neg => UnOp::Neg,
                    pxr_sema::TUnOp::Not => UnOp::Not,
                    pxr_sema::TUnOp::BitNot => UnOp::BitNot,
                };
                self.emit(Inst::Un {
                    dst,
                    op: uop,
                    operand_ty: op_ty,
                    result_ty: ty.clone(),
                    a,
                });
                dst
            }
            TExpr::Bin {
                op,
                lhs_ty,
                rhs_ty,
                ty,
                lhs,
                rhs,
            } => self.lower_bin(*op, lhs_ty, rhs_ty, ty, lhs, rhs),
            TExpr::Ternary {
                cond,
                cond_ty,
                then,
                else_branch,
                ty,
            } => self.lower_ternary(cond, cond_ty, then.as_deref(), else_branch, ty),
            TExpr::Match {
                subject,
                subject_ty,
                arms,
                ty,
            } => self.lower_match(subject, subject_ty, arms, ty),
            TExpr::ArrayLit { elems, ety } => self.lower_array_lit(elems, ety),
            TExpr::Index {
                base,
                index,
                index_ty,
                ety,
                ..
            } => {
                let arr = self.lower_expr(base);
                let (ktag, kint, kstr) = self.lower_key(index, index_ty);
                let bits = self.array_get(arr, ktag, kint, kstr);
                self.decode_bits(bits, ety)
            }
            TExpr::Call {
                callee,
                ret,
                args,
                arg_tys,
                ..
            } => self.lower_call(callee, ret, args, arg_tys),
            TExpr::Property {
                base, prop_idx, ty, ..
            } => {
                let obj = self.lower_expr(base);
                let p_idx = self.emit_const(Literal::Int(*prop_idx as i64));
                let bits = self.rt_call("pxr_object_get_bits", vec![obj, p_idx], Ty::Int);
                self.decode_bits(bits, ty)
            }
            TExpr::New {
                class_id,
                class_name,
                num_props,
                prop_defaults,
                ctor,
                ty,
            } => {
                let cid_val = self.emit_const(Literal::Int(*class_id as i64));
                let name_val = self.emit_const(Literal::Str(class_name.as_bytes().to_vec()));
                let num_props_val = self.emit_const(Literal::Int(*num_props as i64));
                let obj = self.rt_call(
                    "pxr_object_new",
                    vec![cid_val, name_val, num_props_val],
                    ty.clone(),
                );
                for (slot_idx, def_expr) in prop_defaults {
                    let s_idx = self.emit_const(Literal::Int(*slot_idx as i64));
                    let def_val = self.lower_expr(def_expr);
                    let def_ty = def_expr.ty();
                    let vtag = self.emit_const(Literal::Int(Self::tag_of(&def_ty)));
                    let bits = self.encode_bits(def_val, &def_ty);
                    self.rt_call(
                        "pxr_object_set_bits",
                        vec![obj, s_idx, vtag, bits],
                        Ty::Void,
                    );
                }
                if let Some((ctor_mangled, ctor_args, _)) = ctor {
                    let mut args = vec![obj];
                    for arg in ctor_args {
                        let value = self.lower_expr(arg);
                        args.push(if is_pass(arg) {
                            value
                        } else {
                            self.maybe_copy_array(value, &arg.ty())
                        });
                    }
                    self.emit(Inst::Call {
                        dst: None,
                        callee: ctor_mangled.clone(),
                        args,
                        ret: Ty::Void,
                    });
                }
                obj
            }
            TExpr::Instanceof {
                expr,
                target_class_id,
                target_class_name: _,
            } => {
                let obj = self.lower_expr(expr);
                let target_cid = self.emit_const(Literal::Int(*target_class_id as i64));
                let function = if expr.ty() == Ty::Mixed {
                    "pxr_mixed_instanceof"
                } else {
                    "pxr_instanceof"
                };
                let res = self.rt_call(function, vec![obj, target_cid], Ty::Int);
                self.to_bool(res, &Ty::Int)
            }
        }
    }

    fn emit_const(&mut self, lit: Literal) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Const { dst, lit });
        dst
    }

    fn cast(&mut self, v: Value, from: &Ty, to: &Ty) -> Value {
        if from == to {
            return v;
        }
        if *to == Ty::Mixed && *from != Ty::Void {
            let tag = self.emit_const(Literal::Int(Self::tag_of(from)));
            let bits = self.encode_bits(v, from);
            return self.rt_call("pxr_mixed_box", vec![tag, bits], Ty::Mixed);
        }
        if let (Ty::Array(element), Ty::Array(target)) = (from, to) {
            if **target == Ty::Mixed && **element != Ty::Mixed {
                return self.rt_call("pxr_array_to_mixed", vec![v], to.clone());
            }
        }
        if matches!(from, Ty::Array(_) | Ty::Named(_)) && *to == Ty::Bool {
            let boxed = self.cast(v, from, &Ty::Mixed);
            return self.cast(boxed, &Ty::Mixed, to);
        }
        if *from == Ty::Mixed {
            let function = match to {
                Ty::Int => "pxr_mixed_i64",
                Ty::Float => "pxr_mixed_f64",
                Ty::String => "pxr_mixed_str",
                Ty::Bool => "pxr_mixed_bool",
                Ty::Array(_) => "pxr_mixed_array",
                Ty::Named(_) => "pxr_mixed_object",
                _ => panic!("unsupported mixed conversion to {to}"),
            };
            let result_ty = if *to == Ty::Bool { Ty::Int } else { to.clone() };
            let result = self.rt_call(function, vec![v], result_ty);
            return if *to == Ty::Bool {
                self.cast(result, &Ty::Int, &Ty::Bool)
            } else {
                result
            };
        }
        let (op, ok) = cast_op(from, to);
        if !ok {
            // Semantic analysis should have rejected this; returning the value
            // unchanged keeps the compiler robust without emitting invalid IR.
            return v;
        }
        let dst = self.fresh();
        self.emit(Inst::Cast {
            dst,
            op,
            from: from.clone(),
            to: to.clone(),
            src: v,
        });
        dst
    }

    fn to_bool(&mut self, v: Value, ty: &Ty) -> Value {
        if *ty == Ty::Bool {
            return v;
        }
        self.cast(v, ty, &Ty::Bool)
    }

    fn to_string(&mut self, v: Value, ty: &Ty) -> Value {
        if *ty == Ty::String {
            return v;
        }
        self.cast(v, ty, &Ty::String)
    }

    /// Bring two numeric operands to a common type.
    fn unify_numeric(&mut self, a: Value, at: &Ty, b: Value, bt: &Ty) -> (Value, Value, Ty) {
        match (at, bt) {
            (Ty::Int, Ty::Float) => (self.cast(a, at, &Ty::Float), b, Ty::Float),
            (Ty::Float, Ty::Int) => (a, self.cast(b, bt, &Ty::Float), Ty::Float),
            (Ty::Float, Ty::Float) => (a, b, Ty::Float),
            _ => (a, b, Ty::Int),
        }
    }

    fn mixed_op(op: pxr_sema::TBinOp) -> Option<i64> {
        use pxr_sema::TBinOp::*;
        Some(match op {
            Add => 0,
            Sub => 1,
            Mul => 2,
            Div => 3,
            Mod => 4,
            BitAnd => 5,
            BitOr => 6,
            BitXor => 7,
            Shl => 8,
            Shr => 9,
            Eq => 10,
            NotEq => 11,
            Lt => 12,
            LtEq => 13,
            Gt => 14,
            GtEq => 15,
            Spaceship => 16,
            _ => return None,
        })
    }

    fn lower_mixed_binary(
        &mut self,
        op: pxr_sema::TBinOp,
        left: Value,
        left_ty: &Ty,
        right: Value,
        right_ty: &Ty,
    ) -> Value {
        let left = self.cast(left, left_ty, &Ty::Mixed);
        let right = self.cast(right, right_ty, &Ty::Mixed);
        let op = self.emit_const(Literal::Int(Self::mixed_op(op).unwrap()));
        self.rt_call("pxr_mixed_binary", vec![op, left, right], Ty::Mixed)
    }

    fn lower_bin(
        &mut self,
        op: pxr_sema::TBinOp,
        lhs_ty: &Ty,
        rhs_ty: &Ty,
        result_ty: &Ty,
        lhs: &TExpr,
        rhs: &TExpr,
    ) -> Value {
        use pxr_sema::TBinOp as B;
        if (*lhs_ty == Ty::Mixed || *rhs_ty == Ty::Mixed) && Self::mixed_op(op).is_some() {
            let left = self.lower_expr(lhs);
            let right = self.lower_expr(rhs);
            let result = self.lower_mixed_binary(op, left, lhs_ty, right, rhs_ty);
            return self.cast(result, &Ty::Mixed, result_ty);
        }

        match op {
            B::And | B::Or => self.lower_short_circuit(op, lhs_ty, lhs, rhs_ty, rhs),
            B::Concat => {
                let a = self.lower_expr(lhs);
                let a = self.to_string(a, lhs_ty);
                let b = self.lower_expr(rhs);
                let b = self.to_string(b, rhs_ty);
                self.bin(IBinOp::Concat, a, &Ty::String, b, &Ty::String, &Ty::String)
            }
            B::Add | B::Sub | B::Mul | B::Mod => {
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                if op == B::Mod {
                    // PHP's `%`: integer remainder; a zero divisor is a fatal
                    // DivisionByZeroError (not UB / SIGFPE).
                    let dst = self.fresh();
                    self.emit(Inst::Call {
                        dst: Some(dst),
                        callee: "pxr_mod".into(),
                        args: vec![a, b],
                        ret: Ty::Int,
                    });
                    return dst;
                }
                let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                let iop = match op {
                    B::Add => IBinOp::Add,
                    B::Sub => IBinOp::Sub,
                    B::Mul => IBinOp::Mul,
                    _ => unreachable!(),
                };
                self.bin(iop, a, &ty, b, &ty, &ty)
            }
            B::Div => {
                let a = self.lower_expr(lhs);
                let a = self.cast(a, lhs_ty, &Ty::Float);
                let b = self.lower_expr(rhs);
                let b = self.cast(b, rhs_ty, &Ty::Float);
                // PHP's `/` always returns float; a zero divisor is a fatal
                // DivisionByZeroError rather than IEEE infinity.
                let dst = self.fresh();
                self.emit(Inst::Call {
                    dst: Some(dst),
                    callee: "pxr_div".into(),
                    args: vec![a, b],
                    ret: Ty::Float,
                });
                dst
            }
            B::Eq | B::NotEq | B::Identical | B::NotIdentical => {
                let strict = matches!(op, B::Identical | B::NotIdentical);
                let want_eq = matches!(op, B::Eq | B::Identical);
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                // Loose comparison of two strings follows PHP's numeric-string
                // rule; strict comparison is always byte equality.
                if !strict && *lhs_ty == Ty::String && *rhs_ty == Ty::String {
                    let cmp = self.str_cmp_loose(a, b);
                    let zero = self.emit_const(Literal::Int(0));
                    let iop = if want_eq { IBinOp::Eq } else { IBinOp::NotEq };
                    return self.bin(iop, cmp, &Ty::Int, zero, &Ty::Int, &Ty::Bool);
                }
                let iop = if want_eq { IBinOp::Eq } else { IBinOp::NotEq };
                if strict && (lhs_ty.is_array() || matches!(lhs_ty, Ty::Named(_))) {
                    let a = self.cast(a, lhs_ty, &Ty::Mixed);
                    let b = self.cast(b, rhs_ty, &Ty::Mixed);
                    return self.bin(iop, a, &Ty::Mixed, b, &Ty::Mixed, &Ty::Bool);
                }
                if lhs_ty.is_numeric() && rhs_ty.is_numeric() && lhs_ty != rhs_ty {
                    let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                    return self.bin(iop, a, &ty, b, &ty, &Ty::Bool);
                }
                self.bin(iop, a, lhs_ty, b, rhs_ty, &Ty::Bool)
            }
            B::Lt | B::LtEq | B::Gt | B::GtEq => {
                let iop = match op {
                    B::Lt => IBinOp::Lt,
                    B::LtEq => IBinOp::LtEq,
                    B::Gt => IBinOp::Gt,
                    B::GtEq => IBinOp::GtEq,
                    _ => unreachable!(),
                };
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                // Ordering two strings uses PHP's numeric-aware comparison.
                if *lhs_ty == Ty::String && *rhs_ty == Ty::String {
                    let cmp = self.str_cmp_loose(a, b);
                    let zero = self.emit_const(Literal::Int(0));
                    return self.bin(iop, cmp, &Ty::Int, zero, &Ty::Int, &Ty::Bool);
                }
                let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                let _ = result_ty;
                self.bin(iop, a, &ty, b, &ty, &Ty::Bool)
            }
            B::Spaceship => {
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                if *lhs_ty == Ty::String && *rhs_ty == Ty::String {
                    return self.str_cmp_loose(a, b);
                }
                let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                self.bin(IBinOp::Cmp, a, &ty, b, &ty, &Ty::Int)
            }
            B::BitAnd | B::BitOr | B::BitXor => {
                let iop = match op {
                    B::BitAnd => IBinOp::BitAnd,
                    B::BitOr => IBinOp::BitOr,
                    B::BitXor => IBinOp::BitXor,
                    _ => unreachable!(),
                };
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                self.bin(iop, a, &Ty::Int, b, &Ty::Int, &Ty::Int)
            }
            B::Shl | B::Shr => {
                // PHP semantics live in the runtime: a negative shift count is a
                // fatal ArithmeticError and a count >= 64 yields 0 (or -1 for
                // `>>` of a negative value) instead of masking to 6 bits.
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
                let callee = if op == B::Shl { "pxr_shl" } else { "pxr_shr" };
                let dst = self.fresh();
                self.emit(Inst::Call {
                    dst: Some(dst),
                    callee: callee.into(),
                    args: vec![a, b],
                    ret: Ty::Int,
                });
                dst
            }
            B::NullCoalesce => self.lower_null_coalesce(lhs, lhs_ty, rhs, rhs_ty),
            B::Pow => unreachable!("`**` is rejected during semantic analysis"),
        }
    }

    /// Lower `a ?? b` lazily, testing both key presence and boxed nulls.
    fn lower_null_coalesce(&mut self, lhs: &TExpr, lhs_ty: &Ty, rhs: &TExpr, rhs_ty: &Ty) -> Value {
        // Keep a reference around so we can borrow it in `match` arms later.
        let ty = self.common_of(lhs_ty, rhs_ty);
        match lhs {
            TExpr::Index {
                base,
                index,
                index_ty,
                ety,
                ..
            } => {
                let tmp = self.alloc_temp_slot(ty.clone());
                let arr = self.lower_expr(base);
                let aty = base.ty();
                let arr_slot = self.alloc_temp_slot(aty.clone());
                self.emit(Inst::SlotSet {
                    slot: arr_slot,
                    src: arr,
                    ty: aty.clone(),
                });
                let (ktag, kint, kstr) = self.lower_key(index, index_ty);
                let arr2 = {
                    let dst = self.fresh();
                    self.emit(Inst::SlotGet {
                        dst,
                        slot: arr_slot,
                        ty: aty.clone(),
                    });
                    dst
                };
                let has = self.array_has(arr2, ktag, kint, kstr);
                let take_l = self.fresh_label();
                let rhs_l = self.fresh_label();
                let end = self.fresh_label();
                self.terminator(Inst::Branch {
                    cond: has,
                    then_label: take_l,
                    else_label: rhs_l,
                });

                self.switch_to(take_l);
                let a3 = {
                    let dst = self.fresh();
                    self.emit(Inst::SlotGet {
                        dst,
                        slot: arr_slot,
                        ty: aty,
                    });
                    dst
                };
                let bits = self.array_get(a3, ktag, kint, kstr);
                let v1 = self.decode_bits(bits, ety);
                if *ety == Ty::Mixed {
                    let is_null = self.mixed_is_null(v1);
                    let present_l = self.fresh_label();
                    self.terminator(Inst::Branch {
                        cond: is_null,
                        then_label: rhs_l,
                        else_label: present_l,
                    });
                    self.switch_to(present_l);
                }
                let v1 = self.cast(v1, ety, &ty);
                self.emit(Inst::SlotSet {
                    slot: tmp,
                    src: v1,
                    ty: ty.clone(),
                });
                self.terminator(Inst::Jump(end));

                self.switch_to(rhs_l);
                let v2 = self.lower_expr(rhs);
                let v2 = self.cast(v2, rhs_ty, &ty);
                self.emit(Inst::SlotSet {
                    slot: tmp,
                    src: v2,
                    ty: ty.clone(),
                });
                self.terminator(Inst::Jump(end));

                self.switch_to(end);
                let dst = self.fresh();
                self.emit(Inst::SlotGet { dst, slot: tmp, ty });
                dst
            }
            _ if *lhs_ty == Ty::Mixed => {
                let v = self.lower_expr(lhs);
                let is_null = self.mixed_is_null(v);
                let tmp = self.alloc_temp_slot(ty.clone());
                let rhs_l = self.fresh_label();
                let take_l = self.fresh_label();
                let end = self.fresh_label();
                self.terminator(Inst::Branch {
                    cond: is_null,
                    then_label: rhs_l,
                    else_label: take_l,
                });
                self.switch_to(take_l);
                self.emit(Inst::SlotSet {
                    slot: tmp,
                    src: v,
                    ty: ty.clone(),
                });
                self.terminator(Inst::Jump(end));
                self.switch_to(rhs_l);
                let fallback = self.lower_expr(rhs);
                let fallback = self.cast(fallback, rhs_ty, &ty);
                self.emit(Inst::SlotSet {
                    slot: tmp,
                    src: fallback,
                    ty: ty.clone(),
                });
                self.terminator(Inst::Jump(end));
                self.switch_to(end);
                let dst = self.fresh();
                self.emit(Inst::SlotGet { dst, slot: tmp, ty });
                dst
            }
            _ => {
                let v = self.lower_expr(lhs);
                self.cast(v, lhs_ty, &ty)
            }
        }
    }

    fn mixed_is_null(&mut self, value: Value) -> Value {
        let result = self.rt_call("pxr_mixed_is_nullish", vec![value], Ty::Int);
        self.to_bool(result, &Ty::Int)
    }

    /// The common type of a binary/coalesce/ternary result, mirroring the
    /// analyzer's `unify_types` (used only when both operands are non-null).
    fn common_of(&self, a: &Ty, b: &Ty) -> Ty {
        if a == b {
            return a.clone();
        }
        if (*a == Ty::Mixed && *b != Ty::Void) || (*b == Ty::Mixed && *a != Ty::Void) {
            return Ty::Mixed;
        }
        if a.is_numeric() && b.is_numeric() {
            return if *a == Ty::Float || *b == Ty::Float {
                Ty::Float
            } else {
                Ty::Int
            };
        }
        // Fall back to the left type; semantic analysis rejected real
        // mismatches long before we get here.
        a.clone()
    }

    fn lower_ternary(
        &mut self,
        cond: &TExpr,
        cond_ty: &Ty,
        then: Option<&TExpr>,
        else_branch: &TExpr,
        ty: &Ty,
    ) -> Value {
        let raw = self.lower_expr(cond);
        let c = self.to_bool(raw, cond_ty);
        let tmp = self.alloc_temp_slot(ty.clone());
        let then_l = self.fresh_label();
        let else_l = self.fresh_label();
        let end = self.fresh_label();
        self.terminator(Inst::Branch {
            cond: c,
            then_label: then_l,
            else_label: else_l,
        });

        self.switch_to(then_l);
        let tv = match then {
            Some(t) => self.lower_expr(t),
            None => raw, // short form `?:` reuses the condition's value
        };
        let then_ty: Ty = match then {
            Some(t) => t.ty(),
            None => cond_ty.clone(),
        };
        let tv = self.cast(tv, &then_ty, ty);
        self.emit(Inst::SlotSet {
            slot: tmp,
            src: tv,
            ty: ty.clone(),
        });
        self.terminator(Inst::Jump(end));

        self.switch_to(else_l);
        let ev = self.lower_expr(else_branch);
        let ev = self.cast(ev, &else_branch.ty(), ty);
        self.emit(Inst::SlotSet {
            slot: tmp,
            src: ev,
            ty: ty.clone(),
        });
        self.terminator(Inst::Jump(end));

        self.switch_to(end);
        let dst = self.fresh();
        self.emit(Inst::SlotGet {
            dst,
            slot: tmp,
            ty: ty.clone(),
        });
        dst
    }

    fn lower_match(
        &mut self,
        subject: &TExpr,
        subject_ty: &Ty,
        arms: &[pxr_sema::TArm],
        ty: &Ty,
    ) -> Value {
        let sval = self.lower_expr(subject);
        let tmp = self.alloc_temp_slot(ty.clone());
        let end = self.fresh_label();
        let body_labels: Vec<Label> = arms.iter().map(|_| self.fresh_label()).collect();

        // PHP evaluates `match` arms in order, but a `default` arm matches only when
        // no conditional arm matched — regardless of its position in the source.
        // Dispatch: test each non-default arm's conditions in order; the first
        // match jumps into that arm's body.
        for (i, arm) in arms.iter().enumerate() {
            if arm.is_default {
                continue;
            }
            let arm_l = body_labels[i];
            let n = arm.conds.len();
            let next_arm = self.fresh_label();
            for (j, cond) in arm.conds.iter().enumerate() {
                let cv = self.lower_expr(cond);
                let eq = self.bin(IBinOp::Eq, sval, subject_ty, cv, &cond.ty(), &Ty::Bool);
                if j + 1 < n {
                    let cont = self.fresh_label();
                    self.terminator(Inst::Branch {
                        cond: eq,
                        then_label: arm_l,
                        else_label: cont,
                    });
                    self.switch_to(cont);
                } else {
                    self.terminator(Inst::Branch {
                        cond: eq,
                        then_label: arm_l,
                        else_label: next_arm,
                    });
                }
            }
            self.switch_to(next_arm);
        }

        // Everything that fell through every non-default arm lands here; jump
        // into the default arm (PXR requires one), or straight to the end.
        let default_body = arms
            .iter()
            .position(|a| a.is_default)
            .map(|i| body_labels[i])
            .unwrap_or(end);

        if let Some(idx) = arms.iter().position(|a| a.is_default) {
            let arm = &arms[idx];
            self.terminator(Inst::Jump(default_body));
            self.switch_to(body_labels[idx]);
            let r = self.lower_expr(&arm.result);
            let r = self.cast(r, &arm.result.ty(), ty);
            self.emit(Inst::SlotSet {
                slot: tmp,
                src: r,
                ty: ty.clone(),
            });
            self.terminator(Inst::Jump(end));
        } else {
            self.terminator(Inst::Jump(default_body));
        }

        for (i, arm) in arms.iter().enumerate() {
            if arm.is_default {
                continue;
            }
            self.switch_to(body_labels[i]);
            let r = self.lower_expr(&arm.result);
            let r = self.cast(r, &arm.result.ty(), ty);
            self.emit(Inst::SlotSet {
                slot: tmp,
                src: r,
                ty: ty.clone(),
            });
            self.terminator(Inst::Jump(end));
        }

        self.switch_to(end);
        let dst = self.fresh();
        self.emit(Inst::SlotGet {
            dst,
            slot: tmp,
            ty: ty.clone(),
        });
        dst
    }

    fn bin(&mut self, op: IBinOp, a: Value, at: &Ty, b: Value, bt: &Ty, rt: &Ty) -> Value {
        if (*at == Ty::Mixed || *bt == Ty::Mixed) && matches!(op, IBinOp::Eq | IBinOp::NotEq) {
            let a = self.cast(a, at, &Ty::Mixed);
            let b = self.cast(b, bt, &Ty::Mixed);
            let result = self.rt_call("pxr_mixed_identical", vec![a, b], Ty::Int);
            let zero = self.emit_const(Literal::Int(0));
            return self.bin(
                if op == IBinOp::Eq {
                    IBinOp::NotEq
                } else {
                    IBinOp::Eq
                },
                result,
                &Ty::Int,
                zero,
                &Ty::Int,
                &Ty::Bool,
            );
        }
        let dst = self.fresh();
        self.emit(Inst::Bin {
            dst,
            op,
            lhs_ty: at.clone(),
            rhs_ty: bt.clone(),
            result_ty: rt.clone(),
            a,
            b,
        });
        dst
    }

    /// PHP loose string comparison, which is numeric-aware: two numeric strings
    /// compare by value (`"10" < "9"`, `"1" == "01"`), others byte-wise.
    /// Returns the normalised three-way result (-1/0/1) as an `int`.
    fn str_cmp_loose(&mut self, a: Value, b: Value) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_str_cmp_loose".into(),
            args: vec![a, b],
            ret: Ty::Int,
        });
        dst
    }

    fn lower_short_circuit(
        &mut self,
        op: pxr_sema::TBinOp,
        lhs_ty: &Ty,
        lhs: &TExpr,
        rhs_ty: &Ty,
        rhs: &TExpr,
    ) -> Value {
        let is_and = op == pxr_sema::TBinOp::And;
        let tmp = self.alloc_temp_slot(Ty::Bool);

        let a = self.lower_expr(lhs);
        let a = self.to_bool(a, lhs_ty);

        let rhs_l = self.fresh_label();
        let shortcut_l = self.fresh_label();
        let end = self.fresh_label();

        if is_and {
            // `a && b`: if a is false, short-circuit to false.
            self.terminator(Inst::Branch {
                cond: a,
                then_label: rhs_l,
                else_label: shortcut_l,
            });
        } else {
            // `a || b`: if a is true, short-circuit to true.
            self.terminator(Inst::Branch {
                cond: a,
                then_label: shortcut_l,
                else_label: rhs_l,
            });
        }

        self.switch_to(rhs_l);
        let b = self.lower_expr(rhs);
        let b = self.to_bool(b, rhs_ty);
        self.emit(Inst::SlotSet {
            slot: tmp,
            src: b,
            ty: Ty::Bool,
        });
        self.terminator(Inst::Jump(end));

        self.switch_to(shortcut_l);
        let c = self.emit_const(Literal::Bool(!is_and));
        self.emit(Inst::SlotSet {
            slot: tmp,
            src: c,
            ty: Ty::Bool,
        });
        self.terminator(Inst::Jump(end));

        self.switch_to(end);
        let dst = self.fresh();
        self.emit(Inst::SlotGet {
            dst,
            slot: tmp,
            ty: Ty::Bool,
        });
        dst
    }

    // ---- Arrays --------------------------------------------------------

    /// Runtime tag for a value type, used when storing elements.
    fn tag_of(ty: &Ty) -> i64 {
        match ty {
            Ty::Int => 1,
            Ty::Float => 2,
            Ty::Bool => 3,
            Ty::String => 4,
            Ty::Array(_) => 5,
            Ty::Named(_) | Ty::TaskResult(_) => 6,
            Ty::Mixed => 7,
            _ => 0,
        }
    }

    fn array_new(&mut self) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_new".into(),
            args: vec![],
            ret: Ty::array(Ty::Null),
        });
        dst
    }

    fn array_get(&mut self, arr: Value, ktag: Value, kint: Value, kstr: Value) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_get_bits".into(),
            args: vec![arr, ktag, kint, kstr],
            ret: Ty::Int,
        });
        dst
    }

    fn array_has(&mut self, arr: Value, ktag: Value, kint: Value, kstr: Value) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_has".into(),
            args: vec![arr, ktag, kint, kstr],
            ret: Ty::Bool,
        });
        dst
    }

    fn array_set(
        &mut self,
        arr: Value,
        ktag: Value,
        kint: Value,
        kstr: Value,
        vtag: Value,
        vbits: Value,
    ) {
        self.emit(Inst::Call {
            dst: None,
            callee: "pxr_array_set".into(),
            args: vec![arr, ktag, kint, kstr, vtag, vbits],
            ret: Ty::Void,
        });
    }

    fn array_push(&mut self, arr: Value, vtag: Value, vbits: Value) {
        self.emit(Inst::Call {
            dst: None,
            callee: "pxr_array_push".into(),
            args: vec![arr, vtag, vbits],
            ret: Ty::Void,
        });
    }

    fn array_count(&mut self, arr: Value) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_count".into(),
            args: vec![arr],
            ret: Ty::Int,
        });
        dst
    }

    /// PHP arrays are value types: assigning, passing, returning and iterating
    /// all work on a copy, so a later mutation can never be observed through
    /// another name. PXR realises this with a copy-on-write header (`pxr_array_copy`)
    /// at every place an array value is stored into a location, passed to a
    /// function, embedded in another array, or taken as a `foreach` value.
    fn maybe_copy_array(&mut self, v: Value, ty: &Ty) -> Value {
        if *ty == Ty::Mixed {
            return self.rt_call("pxr_mixed_copy", vec![v], Ty::Mixed);
        }
        if !ty.is_array() {
            return v;
        }
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_copy".into(),
            args: vec![v],
            ret: ty.clone(),
        });
        dst
    }

    /// Pack a value of the given type into an `i64` payload for storage.
    fn encode_bits(&mut self, v: Value, ty: &Ty) -> Value {
        if *ty == Ty::Int {
            return v;
        }
        let dst = self.fresh();
        self.emit(Inst::BitCast {
            dst,
            from: ty.clone(),
            to: Ty::Int,
            src: v,
        });
        dst
    }

    /// Unpack an `i64` payload read from an array back to a value of `ty`.
    fn decode_bits(&mut self, v: Value, ty: &Ty) -> Value {
        if *ty == Ty::Int {
            return v;
        }
        let dst = self.fresh();
        self.emit(Inst::BitCast {
            dst,
            from: Ty::Int,
            to: ty.clone(),
            src: v,
        });
        dst
    }

    /// Produce the `(tag, int, str)` triple for an array key.
    fn lower_key(&mut self, key: &TExpr, kty: &Ty) -> (Value, Value, Value) {
        let kv = self.lower_expr(key);
        self.lower_key_value(kv, kty)
    }

    fn lower_key_value(&mut self, kv: Value, kty: &Ty) -> (Value, Value, Value) {
        match kty {
            Ty::Int => {
                let ktag = self.emit_const(Literal::Int(1));
                let kstr = self.emit_const(Literal::Str(Vec::new()));
                (ktag, kv, kstr)
            }
            Ty::String => {
                let ktag = self.emit_const(Literal::Int(4));
                let kint = self.emit_const(Literal::Int(0));
                (ktag, kint, kv)
            }
            Ty::Mixed => {
                let tag = self.rt_call("pxr_mixed_key_tag", vec![kv], Ty::Int);
                let integer = self.rt_call("pxr_mixed_key_int", vec![kv], Ty::Int);
                let string = self.rt_call("pxr_mixed_key_str", vec![kv], Ty::String);
                (tag, integer, string)
            }
            _ => unreachable!("array key types are enforced by sema"),
        }
    }

    fn lower_array_lit(&mut self, elems: &[(Option<TExpr>, TExpr)], ety: &Ty) -> Value {
        let arr = self.array_new();
        let vtag = self.emit_const(Literal::Int(Self::tag_of(ety)));
        for (key, value) in elems {
            let mut vv = self.lower_expr(value);
            let vt = value.ty();
            if vt != *ety {
                vv = self.cast(vv, &vt, ety);
            }
            // Nested arrays are copied so a later mutation of the literal
            // element cannot be observed through the source variable.
            let vv = self.maybe_copy_array(vv, ety);
            let bits = self.encode_bits(vv, ety);
            match key {
                Some(k) => {
                    let (ktag, kint, kstr) = self.lower_key(k, &k.ty());
                    self.array_set(arr, ktag, kint, kstr, vtag, bits);
                }
                None => self.array_push(arr, vtag, bits),
            }
        }
        arr
    }

    fn prepare_target(&mut self, target: &TTarget) -> PreparedTarget {
        match target {
            TTarget::Var { slot, ty, .. } => PreparedTarget::Slot {
                slot: *slot,
                storage: ty.clone(),
            },
            TTarget::MixedVar { slot, .. } => PreparedTarget::Slot {
                slot: *slot,
                storage: Ty::Mixed,
            },
            TTarget::Index {
                base, key, key_ty, ..
            } => {
                let base = self.prepare_array_base(base);
                let key = Some(self.lower_key(key, key_ty));
                PreparedTarget::Array { base, key }
            }
            TTarget::Push { base, .. } => {
                let base = self.prepare_array_base(base);
                PreparedTarget::Array { base, key: None }
            }
            TTarget::Property { base, prop_idx, .. } => {
                let object = self.lower_expr(base);
                let index = self.emit_const(Literal::Int(*prop_idx as i64));
                PreparedTarget::Property { object, index }
            }
            TTarget::Nested {
                parent,
                array_ty,
                key,
                ..
            } => {
                let parent_ty = parent.storage_ty();
                let parent = Box::new(self.prepare_target(parent));
                let key = key.as_ref().map(|(key, ty)| self.lower_key(key, ty));
                PreparedTarget::Nested {
                    parent,
                    parent_ty,
                    array_ty: array_ty.clone(),
                    key,
                }
            }
        }
    }

    fn prepare_array_base(&mut self, base: &TExpr) -> ArrayBase {
        match base {
            TExpr::Var { slot, ty, .. } => ArrayBase::Slot {
                slot: *slot,
                storage: ty.clone(),
                view: ty.clone(),
            },
            TExpr::Coerce { expr, ty } if matches!(&**expr, TExpr::Var { .. }) => {
                let TExpr::Var {
                    slot, ty: storage, ..
                } = &**expr
                else {
                    unreachable!()
                };
                ArrayBase::Slot {
                    slot: *slot,
                    storage: storage.clone(),
                    view: ty.clone(),
                }
            }
            _ => ArrayBase::Value(self.lower_expr(base)),
        }
    }

    fn array_base_value(&mut self, base: &ArrayBase) -> Value {
        match base {
            ArrayBase::Value(value) => *value,
            ArrayBase::Slot {
                slot,
                storage,
                view,
            } => {
                let value = self.load_slot(*slot, storage.clone());
                self.cast(value, storage, view)
            }
        }
    }

    fn array_location(&mut self, place: &PreparedTarget) -> (Value, Option<(Value, Value, Value)>) {
        match place {
            PreparedTarget::Array { base, key } => (self.array_base_value(base), *key),
            PreparedTarget::Nested {
                parent,
                parent_ty,
                array_ty,
                key,
            } => (self.read_container(parent, parent_ty, array_ty), *key),
            _ => unreachable!("array location"),
        }
    }

    // Reading a missing intermediate creates an unattached empty array. It is
    // published only by write-back after the RHS successfully completes.
    fn read_container(&mut self, place: &PreparedTarget, storage: &Ty, view: &Ty) -> Value {
        match place {
            PreparedTarget::Array { .. } | PreparedTarget::Nested { .. } => {
                let (array, key) = self.array_location(place);
                if let Some((tag, integer, string)) = key {
                    let boxed = self.emit_const(Literal::Int(i64::from(*storage == Ty::Mixed)));
                    self.rt_call(
                        "pxr_array_child_or_new",
                        vec![array, tag, integer, string, boxed],
                        view.clone(),
                    )
                } else {
                    self.array_new()
                }
            }
            _ => {
                let value = self.read_target(place, storage);
                if *storage == Ty::Mixed {
                    self.rt_call("pxr_mixed_array_or_new", vec![value], view.clone())
                } else {
                    self.cast(value, storage, view)
                }
            }
        }
    }

    fn read_target(&mut self, place: &PreparedTarget, ty: &Ty) -> Value {
        match place {
            PreparedTarget::Slot { slot, storage } => {
                let value = self.load_slot(*slot, storage.clone());
                self.cast(value, storage, ty)
            }
            PreparedTarget::Property { object, index } => {
                let bits = self.rt_call("pxr_object_get_bits", vec![*object, *index], Ty::Int);
                self.decode_bits(bits, ty)
            }
            PreparedTarget::Array { .. } | PreparedTarget::Nested { .. } => {
                let (array, key) = self.array_location(place);
                let Some((tag, integer, string)) = key else {
                    return self.emit_const(Literal::Int(0));
                };
                let bits = self.array_get(array, tag, integer, string);
                self.decode_bits(bits, ty)
            }
        }
    }

    fn write_target(&mut self, place: &PreparedTarget, ty: &Ty, value: Value) {
        match place {
            PreparedTarget::Slot { slot, storage } => {
                let value = self.cast(value, ty, storage);
                let value = self.maybe_copy_array(value, storage);
                self.emit(Inst::SlotSet {
                    slot: *slot,
                    src: value,
                    ty: storage.clone(),
                });
            }
            PreparedTarget::Property { object, index } => {
                let value = self.maybe_copy_array(value, ty);
                let tag = self.emit_const(Literal::Int(Self::tag_of(ty)));
                let bits = self.encode_bits(value, ty);
                self.rt_call(
                    "pxr_object_set_bits",
                    vec![*object, *index, tag, bits],
                    Ty::Void,
                );
            }
            PreparedTarget::Array { base, key } => {
                let array = self.array_base_value(base);
                self.write_array_element(array, *key, ty, value);
            }
            PreparedTarget::Nested {
                parent,
                parent_ty,
                array_ty,
                key,
            } => {
                let array = self.read_container(parent, parent_ty, array_ty);
                let array = self.maybe_copy_array(array, array_ty);
                self.write_array_element(array, *key, ty, value);
                let value = self.cast(array, array_ty, parent_ty);
                self.write_target(parent, parent_ty, value);
            }
        }
    }

    fn write_array_element(
        &mut self,
        array: Value,
        key: Option<(Value, Value, Value)>,
        ty: &Ty,
        value: Value,
    ) {
        let value = self.maybe_copy_array(value, ty);
        let tag = self.emit_const(Literal::Int(Self::tag_of(ty)));
        let bits = self.encode_bits(value, ty);
        if let Some((key_tag, integer, string)) = key {
            self.array_set(array, key_tag, integer, string, tag, bits);
        } else {
            self.array_push(array, tag, bits);
        }
    }

    /// Apply a binary operator to two already-lowered values, returning the
    /// result value together with its LLVM type. Used by `op=` and `++`/`--`.
    fn lower_bin_expr(
        &mut self,
        op: pxr_sema::TBinOp,
        lt: &Ty,
        lv: Value,
        rt: &Ty,
        rv: Value,
    ) -> (Value, Ty) {
        use pxr_sema::TBinOp as B;
        if (*lt == Ty::Mixed || *rt == Ty::Mixed) && Self::mixed_op(op).is_some() {
            return (self.lower_mixed_binary(op, lv, lt, rv, rt), Ty::Mixed);
        }

        match op {
            B::Concat => {
                let a = self.to_string(lv, lt);
                let b = self.to_string(rv, rt);
                let d = self.bin(IBinOp::Concat, a, &Ty::String, b, &Ty::String, &Ty::String);
                (d, Ty::String)
            }
            B::Add | B::Sub | B::Mul | B::Mod => {
                if op == B::Mod {
                    let dst = self.fresh();
                    self.emit(Inst::Call {
                        dst: Some(dst),
                        callee: "pxr_mod".into(),
                        args: vec![lv, rv],
                        ret: Ty::Int,
                    });
                    return (dst, Ty::Int);
                }
                let (a, b, ty) = self.unify_numeric(lv, lt, rv, rt);
                let iop = match op {
                    B::Add => IBinOp::Add,
                    B::Sub => IBinOp::Sub,
                    B::Mul => IBinOp::Mul,
                    _ => unreachable!(),
                };
                let d = self.bin(iop, a, &ty, b, &ty, &ty);
                (d, ty)
            }
            B::Div => {
                let a = self.cast(lv, lt, &Ty::Float);
                let b = self.cast(rv, rt, &Ty::Float);
                let dst = self.fresh();
                self.emit(Inst::Call {
                    dst: Some(dst),
                    callee: "pxr_div".into(),
                    args: vec![a, b],
                    ret: Ty::Float,
                });
                (dst, Ty::Float)
            }
            B::Eq | B::NotEq | B::Identical | B::NotIdentical => {
                let iop = if matches!(op, B::Eq | B::Identical) {
                    IBinOp::Eq
                } else {
                    IBinOp::NotEq
                };
                let (a, b, ty) = if lt.is_numeric() && rt.is_numeric() && lt != rt {
                    self.unify_numeric(lv, lt, rv, rt)
                } else {
                    (lv, rv, lt.clone())
                };
                let d = self.bin(iop, a, &ty, b, &ty, &Ty::Bool);
                (d, Ty::Bool)
            }
            B::Lt | B::LtEq | B::Gt | B::GtEq => {
                let iop = match op {
                    B::Lt => IBinOp::Lt,
                    B::LtEq => IBinOp::LtEq,
                    B::Gt => IBinOp::Gt,
                    _ => IBinOp::GtEq,
                };
                let (a, b, ty) = if lt.is_numeric() && rt.is_numeric() {
                    self.unify_numeric(lv, lt, rv, rt)
                } else {
                    (lv, rv, Ty::String)
                };
                let d = self.bin(iop, a, &ty, b, &ty, &Ty::Bool);
                (d, Ty::Bool)
            }
            B::Spaceship => {
                let (a, b, ty) = if lt.is_numeric() && rt.is_numeric() {
                    self.unify_numeric(lv, lt, rv, rt)
                } else {
                    (lv, rv, Ty::String)
                };
                let d = self.bin(IBinOp::Cmp, a, &ty, b, &ty, &Ty::Int);
                (d, Ty::Int)
            }
            B::BitAnd | B::BitOr | B::BitXor | B::Shl | B::Shr => {
                let iop = match op {
                    B::BitAnd => IBinOp::BitAnd,
                    B::BitOr => IBinOp::BitOr,
                    B::BitXor => IBinOp::BitXor,
                    B::Shl => IBinOp::Shl,
                    _ => IBinOp::Shr,
                };
                let d = self.bin(iop, lv, &Ty::Int, rv, &Ty::Int, &Ty::Int);
                (d, Ty::Int)
            }
            _ => (lv, lt.clone()),
        }
    }

    fn lower_coalesce_assign(&mut self, target: &TTarget, ty: &Ty, value: &TExpr) -> Value {
        let place = self.prepare_target(target);
        let present = match &place {
            PreparedTarget::Array { .. } | PreparedTarget::Nested { .. } => {
                let (array, key) = self.array_location(&place);
                if let Some((tag, integer, string)) = key {
                    let present = self.rt_call(
                        "pxr_array_isset",
                        vec![array, tag, integer, string],
                        Ty::Int,
                    );
                    self.to_bool(present, &Ty::Int)
                } else {
                    self.emit_const(Literal::Bool(false))
                }
            }
            _ if *ty == Ty::Mixed => {
                let current = self.read_target(&place, ty);
                let null = self.mixed_is_null(current);
                let no = self.emit_const(Literal::Bool(false));
                self.bin(IBinOp::Eq, null, &Ty::Bool, no, &Ty::Bool, &Ty::Bool)
            }
            _ => return self.read_target(&place, ty),
        };
        let result = self.alloc_temp_slot(ty.clone());
        let done = self.fresh_label();
        let set = self.fresh_label();
        let skip = self.fresh_label();
        self.terminator(Inst::Branch {
            cond: present,
            then_label: skip,
            else_label: set,
        });
        self.switch_to(set);
        let v = self.lower_expr(value);
        let v = self.cast(v, &value.ty(), ty);
        self.write_target(&place, ty, v);
        self.emit(Inst::SlotSet {
            slot: result,
            src: v,
            ty: ty.clone(),
        });
        self.terminator(Inst::Jump(done));
        self.switch_to(skip);
        let current = self.read_target(&place, ty);
        self.emit(Inst::SlotSet {
            slot: result,
            src: current,
            ty: ty.clone(),
        });
        self.terminator(Inst::Jump(done));
        self.switch_to(done);
        let dst = self.fresh();
        self.emit(Inst::SlotGet {
            dst,
            slot: result,
            ty: ty.clone(),
        });
        dst
    }

    // ---- Loops and switches --------------------------------------------

    fn lower_jump(&mut self, is_continue: bool, level: u32) {
        // `level` is validated by sema (1 = innermost); guard anyway so an
        // unexpected value degrades to a no-op instead of a panic.
        let Some(idx) = self.loops.len().checked_sub(level as usize) else {
            return;
        };
        let target = if is_continue {
            self.loops[idx].cont
        } else {
            self.loops[idx].brk
        };
        self.terminator(Inst::Jump(target));
    }

    fn lower_for(
        &mut self,
        init: &[TStmt],
        cond: &Option<(TExpr, Ty)>,
        update: &[TStmt],
        body: &[TStmt],
    ) {
        self.lower_stmts(init);
        let head = self.fresh_label();
        let body_l = self.fresh_label();
        let step = self.fresh_label();
        let end = self.fresh_label();
        self.loops.push(LoopCtx {
            brk: end,
            cont: step,
        });

        if let Some((c, cty)) = cond {
            self.switch_to(head);
            let b = self.lower_cond(c, cty);
            self.terminator(Inst::Branch {
                cond: b,
                then_label: body_l,
                else_label: end,
            });
        } else {
            self.switch_to(head);
            self.terminator(Inst::Jump(body_l));
        }

        self.switch_to(body_l);
        self.lower_stmts(body);
        if !self.terminated {
            self.terminator(Inst::Jump(step));
        }

        self.switch_to(step);
        self.lower_stmts(update);
        if !self.terminated {
            self.terminator(Inst::Jump(head));
        }

        self.switch_to(end);
        self.loops.pop();
    }

    fn lower_do_while(&mut self, body: &[TStmt], cond: &TExpr, cond_ty: &Ty) {
        let body_l = self.fresh_label();
        let cond_l = self.fresh_label();
        let end = self.fresh_label();
        self.loops.push(LoopCtx {
            brk: end,
            cont: cond_l,
        });

        self.terminator(Inst::Jump(body_l));
        self.switch_to(body_l);
        self.lower_stmts(body);
        if !self.terminated {
            self.terminator(Inst::Jump(cond_l));
        }

        self.switch_to(cond_l);
        let c = self.lower_cond(cond, cond_ty);
        self.terminator(Inst::Branch {
            cond: c,
            then_label: body_l,
            else_label: end,
        });

        self.switch_to(end);
        self.loops.pop();
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_foreach(
        &mut self,
        base: &TExpr,
        base_ty: &Ty,
        key_slot: Option<u32>,
        value_slot: u32,
        value_ty: &Ty,
        body: &[TStmt],
    ) {
        let _ = base_ty;
        // The array pointer is computed once in the entry block, so it
        // dominates every loop block below. `foreach` iterates a snapshot
        // (PHP value semantics), so the base is copied here.
        let arr = self.lower_expr(base);
        let arr = self.maybe_copy_array(arr, base_ty);
        let count = self.array_count(arr);
        let idx = self.alloc_temp_slot(Ty::Int);
        let zero = self.emit_const(Literal::Int(0));
        self.emit(Inst::SlotSet {
            slot: idx,
            src: zero,
            ty: Ty::Int,
        });

        let head = self.fresh_label();
        let body_l = self.fresh_label();
        let step = self.fresh_label();
        let end = self.fresh_label();
        self.loops.push(LoopCtx {
            brk: end,
            cont: step,
        });

        self.switch_to(head);
        let iv = self.load_slot(idx, Ty::Int);
        let cmp = self.bin(IBinOp::Lt, iv, &Ty::Int, count, &Ty::Int, &Ty::Bool);
        self.terminator(Inst::Branch {
            cond: cmp,
            then_label: body_l,
            else_label: end,
        });

        self.switch_to(body_l);
        let iv2 = self.load_slot(idx, Ty::Int);
        let key_value = key_slot.map(|slot| {
            (
                slot,
                self.rt_call("pxr_array_key_at", vec![arr, iv2], Ty::Mixed),
            )
        });
        let bits = self.array_val_at(arr, iv2);
        let element_ty = base_ty.array_elem().expect("foreach subject is an array");
        let v = self.decode_bits(bits, element_ty);
        let v = self.cast(v, element_ty, value_ty);
        // The loop variable receives a copy of each value (PHP `foreach` by
        // value), so mutating it cannot touch the iterated array.
        let v = self.maybe_copy_array(v, value_ty);
        self.emit(Inst::SlotSet {
            slot: value_slot,
            src: v,
            ty: value_ty.clone(),
        });
        // PHP binds the value first, then the key when destinations alias.
        if let Some((slot, key)) = key_value {
            self.emit(Inst::SlotSet {
                slot,
                src: key,
                ty: Ty::Mixed,
            });
        }
        self.lower_stmts(body);
        if !self.terminated {
            self.terminator(Inst::Jump(step));
        }

        self.switch_to(step);
        let iv3 = self.load_slot(idx, Ty::Int);
        let one = self.emit_const(Literal::Int(1));
        let ni = self.bin(IBinOp::Add, iv3, &Ty::Int, one, &Ty::Int, &Ty::Int);
        self.emit(Inst::SlotSet {
            slot: idx,
            src: ni,
            ty: Ty::Int,
        });
        self.terminator(Inst::Jump(head));

        self.switch_to(end);
        self.loops.pop();
    }

    fn load_slot(&mut self, slot: u32, ty: Ty) -> Value {
        let dst = self.fresh();
        self.emit(Inst::SlotGet { dst, slot, ty });
        dst
    }

    fn array_val_at(&mut self, arr: Value, idx: Value) -> Value {
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: "pxr_array_val_bits_at".into(),
            args: vec![arr, idx],
            ret: Ty::Int,
        });
        dst
    }

    fn lower_switch(&mut self, subject: &TExpr, subject_ty: &Ty, cases: &[pxr_sema::TCase]) {
        let sval = self.lower_expr(subject);
        let end = self.fresh_label();
        let body_labels: Vec<Label> = cases.iter().map(|_| self.fresh_label()).collect();
        self.loops.push(LoopCtx {
            brk: end,
            cont: end,
        });

        // Dispatch: compare the subject against each non-default case.
        let mut default_label = end;
        for (i, c) in cases.iter().enumerate() {
            if let Some(t) = &c.test {
                let tv = self.lower_expr(t);
                // PHP's `switch` compares loosely; numeric strings compare by
                // value (`"1" == "01"`).
                let eq = if *subject_ty == Ty::Mixed || c.test_ty == Ty::Mixed {
                    let result = self.lower_mixed_binary(
                        pxr_sema::TBinOp::Eq,
                        sval,
                        subject_ty,
                        tv,
                        &c.test_ty,
                    );
                    self.cast(result, &Ty::Mixed, &Ty::Bool)
                } else if *subject_ty == Ty::String && c.test_ty == Ty::String {
                    let cmp = self.str_cmp_loose(sval, tv);
                    let zero = self.emit_const(Literal::Int(0));
                    self.bin(IBinOp::Eq, cmp, &Ty::Int, zero, &Ty::Int, &Ty::Bool)
                } else if subject_ty.is_numeric() && c.test_ty.is_numeric() {
                    let (a, b, ty) = self.unify_numeric(sval, subject_ty, tv, &c.test_ty);
                    self.bin(IBinOp::Eq, a, &ty, b, &ty, &Ty::Bool)
                } else {
                    let ty = subject_ty.clone();
                    self.bin(IBinOp::Eq, sval, &ty, tv, &ty, &Ty::Bool)
                };
                let cont_l = self.fresh_label();
                self.terminator(Inst::Branch {
                    cond: eq,
                    then_label: body_labels[i],
                    else_label: cont_l,
                });
                self.switch_to(cont_l);
            } else {
                default_label = body_labels[i];
            }
        }
        self.terminator(Inst::Jump(default_label));

        // Bodies are emitted in source order without a trailing jump, so that
        // `case A: case B:` falls through naturally; `break` jumps to `end`.
        for (i, c) in cases.iter().enumerate() {
            self.switch_to(body_labels[i]);
            self.lower_stmts(&c.body);
        }

        self.switch_to(end);
        self.loops.pop();
    }

    fn lower_call(&mut self, callee: &Callee, ret: &Ty, args: &[TExpr], arg_tys: &[Ty]) -> Value {
        match callee {
            Callee::Parallel { handler } => {
                let value = self.lower_expr(&args[0]);
                let value = self.cast(value, &args[0].ty(), &arg_tys[0]);
                let bits = self.encode_bits(value, &arg_tys[0]);
                let tag = self.emit_const(Literal::Int(Self::tag_of(&arg_tys[0])));
                let callback = self.fresh();
                self.emit(Inst::FunctionAddr {
                    dst: callback,
                    name: format!("pxr$parallel${handler}"),
                });
                let bootstrap = self.fresh();
                self.emit(Inst::FunctionAddr {
                    dst: bootstrap,
                    name: "pxr_parallel_bootstrap".into(),
                });
                self.rt_call(
                    "pxr_async_parallel",
                    vec![callback, tag, bits, bootstrap],
                    ret.clone(),
                )
            }
            Callee::AsyncServer { handler, http } => {
                let mut values: Vec<_> = args.iter().map(|a| self.lower_expr(a)).collect();
                let callback = self.fresh();
                self.emit(Inst::FunctionAddr {
                    dst: callback,
                    name: handler.clone(),
                });
                values.insert(2, callback);
                self.rt_call(
                    if *http {
                        "pxr_async_http_serve"
                    } else {
                        "pxr_async_tcp_serve"
                    },
                    values,
                    ret.clone(),
                )
            }
            Callee::Php { name, fallback } => {
                let name = self.emit_const(Literal::Str(name.as_bytes().to_vec()));
                let fallback = self.emit_const(Literal::Str(
                    fallback.as_deref().unwrap_or("").as_bytes().to_vec(),
                ));
                let arguments = self.rt_call("pxr_array_new", vec![], Ty::array(Ty::Mixed));
                for argument in args {
                    let value = self.lower_expr(argument);
                    let boxed = self.cast(value, &argument.ty(), &Ty::Mixed);
                    let tag = self.emit_const(Literal::Int(7));
                    let bits = self.encode_bits(boxed, &Ty::Mixed);
                    self.array_push(arguments, tag, bits);
                }
                self.rt_call("pxr_php_call", vec![name, fallback, arguments], Ty::Mixed)
            }
            Callee::User {
                mangled, params, ..
            } => {
                if mangled == "$pxr_value_drop" {
                    let value = self.lower_expr(&args[0]);
                    let tag = self.emit_const(Literal::Int(Self::tag_of(&arg_tys[0])));
                    let bits = self.encode_bits(value, &arg_tys[0]);
                    self.rt_call("pxr_value_drop", vec![tag, bits], Ty::Void);
                    return self.rt_call("pxr_gc_collect", vec![], Ty::Void);
                }
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let v = self.lower_expr(a);
                    // Widen `int` arguments to `float` parameters, and copy
                    // arrays (PHP passes arrays by value).
                    let want = params.get(i).cloned().unwrap_or_else(|| arg_tys[i].clone());
                    let v = self.cast(v, &arg_tys[i], &want);
                    vals.push(if is_pass(a) {
                        v
                    } else {
                        self.maybe_copy_array(v, &want)
                    });
                }
                if *ret == Ty::Void {
                    self.emit(Inst::Call {
                        dst: None,
                        callee: mangled.clone(),
                        args: vals,
                        ret: Ty::Void,
                    });
                    return self.emit_const(Literal::Int(0));
                }
                let dst = self.fresh();
                self.emit(Inst::Call {
                    dst: Some(dst),
                    callee: mangled.clone(),
                    args: vals,
                    ret: ret.clone(),
                });
                dst
            }
            Callee::Builtin { kind, arg_tys } => {
                let vals: Vec<Value> = args.iter().map(|a| self.lower_expr(a)).collect();
                self.lower_builtin(*kind, arg_tys, &vals, args.len(), ret)
            }
        }
    }

    /// Emit a one-shot runtime call and return its result value.
    fn rt_call(&mut self, callee: &str, args: Vec<Value>, ret: Ty) -> Value {
        let dst = if ret == Ty::Void {
            None
        } else {
            Some(self.fresh())
        };
        self.emit(Inst::Call {
            dst,
            callee: callee.into(),
            args,
            ret,
        });
        dst.unwrap_or(0)
    }

    fn lower_builtin(
        &mut self,
        kind: Builtin,
        arg_tys: &[Ty],
        a: &[Value],
        argc: usize,
        ret: &Ty,
    ) -> Value {
        use Builtin::*;
        match kind {
            Boolval => self.to_bool(a[0], &arg_tys[0]),
            IsInt | IsFloat | IsBool | IsString | IsArray | IsObject | IsNull | IsScalar => {
                let tag = match kind {
                    IsInt => 1,
                    IsFloat => 2,
                    IsBool => 3,
                    IsString => 4,
                    IsArray => 5,
                    IsObject => 6,
                    IsNull => 0,
                    IsScalar => -1,
                    _ => unreachable!(),
                };
                if arg_tys[0] == Ty::Mixed {
                    let tag = self.emit_const(Literal::Int(tag));
                    let result = self.rt_call("pxr_mixed_is", vec![a[0], tag], Ty::Int);
                    self.to_bool(result, &Ty::Int)
                } else {
                    self.emit_const(Literal::Bool(if tag == -1 {
                        arg_tys[0].is_scalar()
                    } else {
                        Self::tag_of(&arg_tys[0]) == tag
                    }))
                }
            }
            Strlen => self.rt_call("pxr_strlen", vec![a[0]], Ty::Int),
            Intval => match arg_tys[0] {
                Ty::Mixed => self.cast(a[0], &Ty::Mixed, &Ty::Int),
                Ty::Int => a[0],
                Ty::Float => {
                    let boxed = self.cast(a[0], &Ty::Float, &Ty::Mixed);
                    self.cast(boxed, &Ty::Mixed, &Ty::Int)
                }
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::Int),
                Ty::String => self.cast(a[0], &Ty::String, &Ty::Int),
                _ => {
                    let boxed = self.cast(a[0], &arg_tys[0], &Ty::Mixed);
                    self.cast(boxed, &Ty::Mixed, &Ty::Int)
                }
            },
            Floatval => match arg_tys[0] {
                Ty::Mixed => self.cast(a[0], &Ty::Mixed, &Ty::Float),
                Ty::Float => a[0],
                Ty::Int => self.cast(a[0], &Ty::Int, &Ty::Float),
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::Float),
                Ty::String => self.cast(a[0], &Ty::String, &Ty::Float),
                _ => {
                    let boxed = self.cast(a[0], &arg_tys[0], &Ty::Mixed);
                    self.cast(boxed, &Ty::Mixed, &Ty::Float)
                }
            },
            Strval => match arg_tys[0] {
                Ty::Mixed => self.cast(a[0], &Ty::Mixed, &Ty::String),
                Ty::String => a[0],
                Ty::Int => self.cast(a[0], &Ty::Int, &Ty::String),
                Ty::Float => self.cast(a[0], &Ty::Float, &Ty::String),
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::String),
                _ => {
                    let boxed = self.cast(a[0], &arg_tys[0], &Ty::Mixed);
                    self.cast(boxed, &Ty::Mixed, &Ty::String)
                }
            },
            Abs => match arg_tys[0] {
                Ty::Int => self.rt_call("pxr_abs_i64", vec![a[0]], Ty::Int),
                _ => self.rt_call("fabs", vec![a[0]], Ty::Float),
            },
            Count => self.rt_call("pxr_array_count", vec![a[0]], Ty::Int),
            Strtoupper => self.rt_call("pxr_str_toupper", vec![a[0]], Ty::String),
            Strtolower => self.rt_call("pxr_str_tolower", vec![a[0]], Ty::String),
            Trim | Ltrim | Rtrim => {
                let mode = match kind {
                    Trim => 0,
                    Ltrim => 1,
                    _ => 2,
                };
                let mask = a
                    .get(1)
                    .copied()
                    .unwrap_or_else(|| self.emit_const(Literal::Null));
                let mode_v = self.emit_const(Literal::Int(mode));
                self.rt_call("pxr_str_trim", vec![a[0], mask, mode_v], Ty::String)
            }
            Substr => {
                let has_len = self.emit_const(Literal::Int(if argc >= 3 { 1 } else { 0 }));
                let l = if argc >= 3 {
                    a[2]
                } else {
                    self.emit_const(Literal::Int(0))
                };
                self.rt_call("pxr_substr", vec![a[0], a[1], l, has_len], Ty::String)
            }
            StrRepeat => self.rt_call("pxr_str_repeat", vec![a[0], a[1]], Ty::String),
            StrReplace => self.rt_call("pxr_str_replace", vec![a[0], a[1], a[2]], Ty::String),
            Implode => {
                let glue = if argc == 1 {
                    self.emit_const(Literal::Str(Vec::new()))
                } else {
                    a[0]
                };
                let arr = if argc == 1 { a[0] } else { a[1] };
                self.rt_call("pxr_implode", vec![glue, arr], Ty::String)
            }
            Explode => {
                let limit = if argc == 3 {
                    a[2]
                } else {
                    self.emit_const(Literal::Int(i64::MAX))
                };
                self.rt_call(
                    "pxr_explode",
                    vec![a[0], a[1], limit],
                    Ty::array(Ty::String),
                )
            }
            StrContains | StrStartsWith | StrEndsWith => {
                let f = match kind {
                    StrContains => "pxr_str_contains",
                    StrStartsWith => "pxr_str_starts_with",
                    _ => "pxr_str_ends_with",
                };
                let v = self.rt_call(f, vec![a[0], a[1]], Ty::Int);
                self.to_bool(v, &Ty::Int)
            }
            Strrev => self.rt_call("pxr_strrev", vec![a[0]], Ty::String),
            Ucfirst => self.rt_call("pxr_ucfirst", vec![a[0]], Ty::String),
            Lcfirst => self.rt_call("pxr_lcfirst", vec![a[0]], Ty::String),
            Ucwords => self.rt_call("pxr_ucwords", vec![a[0]], Ty::String),
            Addslashes => self.rt_call("pxr_addslashes", vec![a[0]], Ty::String),
            Stripslashes => self.rt_call("pxr_stripslashes", vec![a[0]], Ty::String),
            Bin2hex => self.rt_call("pxr_bin2hex", vec![a[0]], Ty::String),
            Base64Encode => self.rt_call("pxr_base64_encode", vec![a[0]], Ty::String),
            Urlencode => self.rt_call("pxr_urlencode", vec![a[0]], Ty::String),
            Rawurlencode => self.rt_call("pxr_rawurlencode", vec![a[0]], Ty::String),
            Urldecode => self.rt_call("pxr_urldecode", vec![a[0]], Ty::String),
            Rawurldecode => self.rt_call("pxr_rawurldecode", vec![a[0]], Ty::String),
            Sprintf => {
                let arr = self.array_new();
                for i in 1..argc {
                    let vtag = self.emit_const(Literal::Int(Self::tag_of(&arg_tys[i])));
                    let vbits = self.encode_bits(a[i], &arg_tys[i]);
                    self.array_push(arr, vtag, vbits);
                }
                self.rt_call("pxr_sprintf", vec![a[0], arr], Ty::String)
            }
            Printf => {
                let arr = self.array_new();
                for i in 1..argc {
                    let vtag = self.emit_const(Literal::Int(Self::tag_of(&arg_tys[i])));
                    let vbits = self.encode_bits(a[i], &arg_tys[i]);
                    self.array_push(arr, vtag, vbits);
                }
                self.rt_call("pxr_printf", vec![a[0], arr], Ty::Int)
            }
            StrSplit => {
                let len = if argc == 2 {
                    a[1]
                } else {
                    self.emit_const(Literal::Int(1))
                };
                self.rt_call("pxr_str_split", vec![a[0], len], Ty::array(Ty::String))
            }
            ArrayValues => {
                let ret = arg_tys[0].clone();
                self.rt_call("pxr_array_values", vec![a[0]], ret)
            }
            ArrayReverse => {
                // The runtime flag is an i64, so widen a `bool` argument.
                let flag = if argc == 2 {
                    self.cast(a[1], &Ty::Bool, &Ty::Int)
                } else {
                    self.emit_const(Literal::Int(0))
                };
                let ret = arg_tys[0].clone();
                self.rt_call("pxr_array_reverse", vec![a[0], flag], ret)
            }
            ArrayMerge => {
                // Fold a variadic call into a chain of binary merges, starting
                // from an empty array so that even one argument is reindexed.
                if argc == 2 {
                    let left = self.cast(a[0], &arg_tys[0], ret);
                    let right = self.cast(a[1], &arg_tys[1], ret);
                    return self.rt_call("pxr_array_merge_pair", vec![left, right], ret.clone());
                }
                let mut acc = self.array_new();
                for i in 0..argc {
                    let array = self.cast(a[i], &arg_tys[i], ret);
                    acc = self.rt_call("pxr_array_merge", vec![acc, array], ret.clone());
                }
                acc
            }
            ArraySlice => {
                let offset = a[1];
                let (length, has_length) = if argc >= 3 {
                    (a[2], 1i64)
                } else {
                    (self.emit_const(Literal::Int(0)), 0)
                };
                let has_length = self.emit_const(Literal::Int(has_length));
                let preserve = if argc >= 4 {
                    self.cast(a[3], &Ty::Bool, &Ty::Int)
                } else {
                    self.emit_const(Literal::Int(0))
                };
                self.rt_call(
                    "pxr_array_slice",
                    vec![a[0], offset, length, has_length, preserve],
                    ret.clone(),
                )
            }
            ArrayPad => {
                let elem = arg_tys[0].array_elem().cloned().unwrap_or(Ty::Null);
                // The value may widen `int` -> `float` to match the elements.
                let v = self.cast(a[2], &arg_tys[2], &elem);
                let vtag = self.emit_const(Literal::Int(Self::tag_of(&elem)));
                let vbits = self.encode_bits(v, &elem);
                self.rt_call("pxr_array_pad", vec![a[0], a[1], vtag, vbits], ret.clone())
            }
            ArrayFill => {
                let elem = arg_tys[2].clone();
                let vtag = self.emit_const(Literal::Int(Self::tag_of(&elem)));
                let vbits = self.encode_bits(a[2], &elem);
                self.rt_call("pxr_array_fill", vec![a[0], a[1], vtag, vbits], ret.clone())
            }
            ArrayKeyExists => {
                let (ktag, ki, ks) = self.lower_key_value(a[0], &arg_tys[0]);
                let v = self.rt_call("pxr_array_has", vec![a[1], ktag, ki, ks], Ty::Int);
                self.to_bool(v, &Ty::Int)
            }
            StrPad => {
                let length = a[1];
                let pad = if argc >= 3 {
                    a[2]
                } else {
                    self.emit_const(Literal::Str(vec![b' ']))
                };
                let ty = if argc >= 4 {
                    a[3]
                } else {
                    self.emit_const(Literal::Int(1))
                };
                self.rt_call("pxr_str_pad", vec![a[0], length, pad, ty], Ty::String)
            }
            SubstrCount => {
                let offset = if argc >= 3 {
                    a[2]
                } else {
                    self.emit_const(Literal::Int(0))
                };
                let (length, has_length) = if argc >= 4 {
                    (a[3], 1i64)
                } else {
                    (self.emit_const(Literal::Int(0)), 0)
                };
                let has_length = self.emit_const(Literal::Int(has_length));
                self.rt_call(
                    "pxr_substr_count",
                    vec![a[0], a[1], offset, length, has_length],
                    Ty::Int,
                )
            }
        }
    }
}

fn cast_op(from: &Ty, to: &Ty) -> (CastOp, bool) {
    use CastOp::*;
    let op = match (from, to) {
        (Ty::Int, Ty::Float) => IntToFloat,
        (Ty::Float, Ty::Int) => FloatToInt,
        (Ty::Bool, Ty::Int) => BoolToInt,
        (Ty::Bool, Ty::Float) => BoolToFloat,
        (Ty::Int, Ty::Bool) => IntToBool,
        (Ty::Float, Ty::Bool) => FloatToBool,
        (Ty::String, Ty::Bool) => StrToBool,
        (Ty::Int, Ty::String) => IntToStr,
        (Ty::Float, Ty::String) => FloatToStr,
        (Ty::Bool, Ty::String) => BoolToStr,
        (Ty::String, Ty::Int) => StrToInt,
        (Ty::String, Ty::Float) => StrToFloat,
        _ => return (IntToFloat, false),
    };
    (op, true)
}

// Constructors and some contextual calls wrap validated arguments in a coercion
// even when their representation is unchanged. Preserve the ownership marker.
fn is_pass(expr: &TExpr) -> bool {
    match expr {
        TExpr::Pass { .. } => true,
        TExpr::Coerce { expr, .. } => is_pass(expr),
        _ => false,
    }
}
