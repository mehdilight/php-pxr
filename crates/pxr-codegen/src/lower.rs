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
    }
    m.function(lower_func(&prog.main));
    m
}

fn lower_func(tf: &TFunc) -> Function {
    let params: Vec<Ty> = tf.params.iter().map(|(_, t)| t.clone()).collect();
    let mut lw = Lowerer::new(Function::new(
        tf.name.clone(),
        tf.display.clone(),
        params,
        tf.ret.clone(),
    ));

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
            TStmt::Block(body) => self.lower_stmts(body),
            TStmt::Echo(exprs) => {
                for e in exprs {
                    let v = self.lower_expr(e);
                    let ty = e.ty();
                    self.emit(Inst::Print { ty, src: v });
                }
            }
            TStmt::Print(e) => {
                let v = self.lower_expr(e);
                let ty = e.ty();
                self.emit(Inst::Print { ty, src: v });
            }
            TStmt::Expr(e) => {
                let _ = self.lower_expr(e);
            }
            TStmt::Assign { target, ty, value } => {
                let mut v = self.lower_expr(value);
                let vt = value.ty();
                if vt != *ty {
                    v = self.cast(v, &vt, ty);
                }
                self.lower_store(target, ty.clone(), v);
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
                    self.lower_coalesce_assign(target, ty, value);
                    return;
                }
                // Read the current value, apply the operation, store back.
                let (cur, cur_ty) = self.lower_load(target, ty.clone());
                let v = self.lower_expr(value);
                let vt = value.ty();
                let result = self.lower_bin_expr(*op, &cur_ty, cur, &vt, v);
                // `result` is well typed for the operator; store it back.
                self.lower_store(target, result.1, result.0);
            }
            TStmt::IncDec { target, ty, delta } => {
                let (cur, cur_ty) = self.lower_load(target, ty.clone());
                let dx = self.emit_const(Literal::Int(*delta));
                let res = self.lower_bin_expr(pxr_sema::TBinOp::Add, &cur_ty, cur, &Ty::Int, dx);
                self.lower_store(target, res.1, res.0);
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
                value_slot,
                value_ty,
                body,
            } => self.lower_foreach(base, base_ty, *value_slot, value_ty, body),
            TStmt::Switch {
                subject,
                subject_ty,
                cases,
            } => self.lower_switch(subject, subject_ty, cases),
            TStmt::Break(level) => self.lower_jump(false, *level),
            TStmt::Continue(level) => self.lower_jump(true, *level),
            TStmt::Return { value, ret } => {
                let v = value.as_ref().map(|e| self.lower_expr(e));
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

    fn lower_expr(&mut self, e: &TExpr) -> Value {
        match e {
            TExpr::Int(v) => self.emit_const(Literal::Int(*v)),
            TExpr::Float(v) => self.emit_const(Literal::Float(*v)),
            TExpr::Str(s) => self.emit_const(Literal::Str(s.clone())),
            TExpr::Bool(b) => self.emit_const(Literal::Bool(*b)),
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
                if lhs_ty.is_numeric() && rhs_ty.is_numeric() && lhs_ty != rhs_ty {
                    let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                    return self.bin(iop, a, &ty, b, &ty, &Ty::Bool);
                }
                let ty = lhs_ty.clone();
                self.bin(iop, a, &ty, b, &ty, &Ty::Bool)
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

    /// Lower `a ?? b` with proper laziness. In PXR only an array element read
    /// can be "unset"; every other expression is always set, so `??` degrades
    /// to its left operand.
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
            _ => {
                let v = self.lower_expr(lhs);
                self.cast(v, lhs_ty, &ty)
            }
        }
    }

    /// The common type of a binary/coalesce/ternary result, mirroring the
    /// analyzer's `unify_types` (used only when both operands are non-null).
    fn common_of(&self, a: &Ty, b: &Ty) -> Ty {
        if a == b {
            return a.clone();
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
    /// another name. PXR realises this with an eager deep copy (`pxr_array_copy`)
    /// at every place an array value is stored into a location, passed to a
    /// function, embedded in another array, or taken as a `foreach` value.
    fn maybe_copy_array(&mut self, v: Value, ty: &Ty) -> Value {
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

    fn lower_store(&mut self, target: &TTarget, ty: Ty, v: Value) {
        // Storing an array value copies it (PHP value semantics). Assignments
        // through an index target store the element's value, so the copy below
        // covers both plain variables and array elements.
        match target {
            TTarget::Var { slot, .. } => {
                let v = self.maybe_copy_array(v, &ty);
                self.emit(Inst::SlotSet {
                    slot: *slot,
                    src: v,
                    ty,
                })
            }
            TTarget::Index {
                base,
                elem_ty: _,
                key,
                key_ty,
            } => {
                // Use the statement's element type, not the target's: they can
                // differ on the assignment that refines an empty `array<null>`.
                let arr = self.lower_expr(base);
                let (ktag, kint, kstr) = self.lower_key(key, key_ty);
                let vtag = self.emit_const(Literal::Int(Self::tag_of(&ty)));
                let v = self.maybe_copy_array(v, &ty);
                let bits = self.encode_bits(v, &ty);
                self.array_set(arr, ktag, kint, kstr, vtag, bits);
            }
            TTarget::Push { base, elem_ty: _ } => {
                let arr = self.lower_expr(base);
                let vtag = self.emit_const(Literal::Int(Self::tag_of(&ty)));
                let v = self.maybe_copy_array(v, &ty);
                let bits = self.encode_bits(v, &ty);
                self.array_push(arr, vtag, bits);
            }
        }
    }

    /// Read the current value of an assignment target (used by `op=` and
    /// `++`/`--`).
    fn lower_load(&mut self, target: &TTarget, ty: Ty) -> (Value, Ty) {
        match target {
            TTarget::Var { slot, .. } => {
                let dst = self.fresh();
                self.emit(Inst::SlotGet {
                    dst,
                    slot: *slot,
                    ty: ty.clone(),
                });
                (dst, ty)
            }
            TTarget::Index {
                base,
                elem_ty: _,
                key,
                key_ty,
            } => {
                let arr = self.lower_expr(base);
                let (ktag, kint, kstr) = self.lower_key(key, key_ty);
                let bits = self.array_get(arr, ktag, kint, kstr);
                let v = self.decode_bits(bits, &ty);
                (v, ty)
            }
            TTarget::Push { .. } => (self.emit_const(Literal::Int(0)), ty),
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

    fn lower_coalesce_assign(&mut self, target: &TTarget, ty: &Ty, value: &TExpr) {
        match target {
            TTarget::Var { .. } => {
                // PXR variables are always set; `??=` is a no-op.
            }
            TTarget::Index {
                base,
                elem_ty: _,
                key,
                key_ty,
            } => {
                let arr = self.lower_expr(base);
                let (ktag, kint, kstr) = self.lower_key(key, key_ty);
                let has = self.array_has(arr, ktag, kint, kstr);
                let set_l = self.fresh_label();
                let skip_l = self.fresh_label();
                self.terminator(Inst::Branch {
                    cond: has,
                    then_label: skip_l,
                    else_label: set_l,
                });
                self.switch_to(set_l);
                let mut v = self.lower_expr(value);
                let vt = value.ty();
                if vt != *ty {
                    v = self.cast(v, &vt, ty);
                }
                let vtag = self.emit_const(Literal::Int(Self::tag_of(ty)));
                let bits = self.encode_bits(v, ty);
                self.array_set(arr, ktag, kint, kstr, vtag, bits);
                self.terminator(Inst::Jump(skip_l));
                self.switch_to(skip_l);
            }
            TTarget::Push { .. } => {}
        }
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
        let bits = self.array_val_at(arr, iv2);
        let v = self.decode_bits(bits, value_ty);
        // The loop variable receives a copy of each value (PHP `foreach` by
        // value), so mutating it cannot touch the iterated array.
        let v = self.maybe_copy_array(v, value_ty);
        self.emit(Inst::SlotSet {
            slot: value_slot,
            src: v,
            ty: value_ty.clone(),
        });
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
                let eq = if *subject_ty == Ty::String && c.test_ty == Ty::String {
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
            Callee::User {
                mangled, params, ..
            } => {
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let v = self.lower_expr(a);
                    // Widen `int` arguments to `float` parameters, and copy
                    // arrays (PHP passes arrays by value).
                    let want = params.get(i).cloned().unwrap_or_else(|| arg_tys[i].clone());
                    let v = self.cast(v, &arg_tys[i], &want);
                    vals.push(self.maybe_copy_array(v, &want));
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
        let dst = self.fresh();
        self.emit(Inst::Call {
            dst: Some(dst),
            callee: callee.into(),
            args,
            ret,
        });
        dst
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
            Strlen => self.rt_call("pxr_strlen", vec![a[0]], Ty::Int),
            Intval => match arg_tys[0] {
                Ty::Int => a[0],
                Ty::Float => self.cast(a[0], &Ty::Float, &Ty::Int),
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::Int),
                Ty::String => self.cast(a[0], &Ty::String, &Ty::Int),
                _ => a[0],
            },
            Floatval => match arg_tys[0] {
                Ty::Float => a[0],
                Ty::Int => self.cast(a[0], &Ty::Int, &Ty::Float),
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::Float),
                Ty::String => self.cast(a[0], &Ty::String, &Ty::Float),
                _ => a[0],
            },
            Strval => match arg_tys[0] {
                Ty::String => a[0],
                Ty::Int => self.cast(a[0], &Ty::Int, &Ty::String),
                Ty::Float => self.cast(a[0], &Ty::Float, &Ty::String),
                Ty::Bool => self.cast(a[0], &Ty::Bool, &Ty::String),
                _ => a[0],
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
                let mut acc = self.array_new();
                for i in 0..argc {
                    acc = self.rt_call("pxr_array_merge", vec![acc, a[i]], ret.clone());
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
                let (ktag, ki, ks) = match arg_tys[0] {
                    Ty::Int => (
                        self.emit_const(Literal::Int(1)),
                        a[0],
                        self.emit_const(Literal::Str(Vec::new())),
                    ),
                    Ty::String => (
                        self.emit_const(Literal::Int(4)),
                        self.emit_const(Literal::Int(0)),
                        a[0],
                    ),
                    _ => unreachable!("key type is enforced by sema"),
                };
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
