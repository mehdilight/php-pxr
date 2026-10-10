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
use pxr_sema::{Builtin, Callee, TExpr, TFunc, TProgram, TStmt};
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
}

impl Lowerer {
    fn new(f: Function) -> Self {
        Lowerer {
            f,
            prelude: Vec::new(),
            terminated: false,
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
            Ty::String => Literal::Str(String::new()),
            _ => Literal::Int(0),
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
            TStmt::Assign { slot, ty, value } => {
                let mut v = self.lower_expr(value);
                let vt = value.ty();
                if vt != *ty {
                    v = self.cast(v, &vt, ty);
                }
                self.emit(Inst::SlotSet {
                    slot: *slot,
                    src: v,
                    ty: ty.clone(),
                });
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
                let a = match op {
                    pxr_sema::TUnOp::Not => self.to_bool(a, operand_ty),
                    pxr_sema::TUnOp::Neg => a,
                };
                let dst = self.fresh();
                let uop = match op {
                    pxr_sema::TUnOp::Neg => UnOp::Neg,
                    pxr_sema::TUnOp::Not => UnOp::Not,
                };
                let op_ty = match op {
                    pxr_sema::TUnOp::Not => Ty::Bool,
                    pxr_sema::TUnOp::Neg => operand_ty.clone(),
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
                let (a, b, ty) = self.unify_numeric(a, lhs_ty, b, rhs_ty);
                let iop = match op {
                    B::Add => IBinOp::Add,
                    B::Sub => IBinOp::Sub,
                    B::Mul => IBinOp::Mul,
                    B::Mod => IBinOp::Mod,
                    _ => unreachable!(),
                };
                self.bin(iop, a, &ty, b, &ty, &ty)
            }
            B::Div => {
                let a = self.lower_expr(lhs);
                let a = self.cast(a, lhs_ty, &Ty::Float);
                let b = self.lower_expr(rhs);
                let b = self.cast(b, rhs_ty, &Ty::Float);
                self.bin(IBinOp::Div, a, &Ty::Float, b, &Ty::Float, &Ty::Float)
            }
            B::Eq | B::NotEq | B::Identical | B::NotIdentical => {
                let iop = match op {
                    B::Eq | B::Identical => IBinOp::Eq,
                    _ => IBinOp::NotEq,
                };
                let a = self.lower_expr(lhs);
                let b = self.lower_expr(rhs);
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
                let (a, b, ty) = if lhs_ty.is_numeric() && rhs_ty.is_numeric() {
                    self.unify_numeric(a, lhs_ty, b, rhs_ty)
                } else {
                    (a, b, Ty::String)
                };
                let _ = result_ty;
                self.bin(iop, a, &ty, b, &ty, &Ty::Bool)
            }
        }
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

    fn lower_call(
        &mut self,
        callee: &Callee,
        ret: &Ty,
        args: &[TExpr],
        arg_tys: &[Ty],
    ) -> Value {
        match callee {
            Callee::User { mangled, params, .. } => {
                let mut vals = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let v = self.lower_expr(a);
                    // Widen `int` arguments to `float` parameters.
                    let want = params.get(i).cloned().unwrap_or_else(|| arg_tys[i].clone());
                    vals.push(self.cast(v, &arg_tys[i], &want));
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
            Callee::Builtin { kind, arg_ty } => {
                let a = self.lower_expr(&args[0]);
                self.lower_builtin(*kind, arg_ty, a)
            }
        }
    }

    fn lower_builtin(&mut self, kind: Builtin, arg_ty: &Ty, a: Value) -> Value {
        match kind {
            Builtin::Strlen => {
                let dst = self.fresh();
                self.emit(Inst::Call {
                    dst: Some(dst),
                    callee: "pxr_strlen".into(),
                    args: vec![a],
                    ret: Ty::Int,
                });
                dst
            }
            Builtin::Intval => match arg_ty {
                Ty::Int => a,
                Ty::Float => self.cast(a, &Ty::Float, &Ty::Int),
                Ty::Bool => self.cast(a, &Ty::Bool, &Ty::Int),
                Ty::String => self.cast(a, &Ty::String, &Ty::Int),
                _ => a,
            },
            Builtin::Floatval => match arg_ty {
                Ty::Float => a,
                Ty::Int => self.cast(a, &Ty::Int, &Ty::Float),
                Ty::Bool => self.cast(a, &Ty::Bool, &Ty::Float),
                Ty::String => self.cast(a, &Ty::String, &Ty::Float),
                _ => a,
            },
            Builtin::Strval => match arg_ty {
                Ty::String => a,
                Ty::Int => self.cast(a, &Ty::Int, &Ty::String),
                Ty::Float => self.cast(a, &Ty::Float, &Ty::String),
                Ty::Bool => self.cast(a, &Ty::Bool, &Ty::String),
                _ => a,
            },
            Builtin::Abs => match arg_ty {
                Ty::Int => {
                    let dst = self.fresh();
                    self.emit(Inst::Call {
                        dst: Some(dst),
                        callee: "pxr_abs_i64".into(),
                        args: vec![a],
                        ret: Ty::Int,
                    });
                    dst
                }
                _ => {
                    let dst = self.fresh();
                    self.emit(Inst::Call {
                        dst: Some(dst),
                        callee: "fabs".into(),
                        args: vec![a],
                        ret: Ty::Float,
                    });
                    dst
                }
            },
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
