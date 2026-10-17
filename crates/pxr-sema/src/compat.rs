//! Checked entry from opt-in compatibility storage into concrete native code.
use super::*;
impl Analyzer {
    pub(super) fn compat_boundary(&self, value: TExpr, expected: &Ty, span: Span) -> TExpr {
        fn borrowed(value: &TExpr) -> bool {
            match value {
                TExpr::Pass { borrowed: true, .. } => true,
                TExpr::Coerce { expr, .. } => borrowed(expr),
                _ => false,
            }
        }
        if borrowed(&value)
            || !self.compat
            || !value.ty().is_boxed()
            || expected.is_boxed()
            || !compat_value_type(expected)
        {
            return value;
        }
        let Some(descriptor) = expected.runtime_descriptor(&|name| {
            self.classes
                .get(&name.to_ascii_lowercase())
                .map(|c| c.class_id)
        }) else {
            return value;
        };
        TExpr::Coerce {
            expr: Box::new(TExpr::Call {
                callee: Callee::User {
                    mangled: "pxr_mixed_require".into(),
                    name: "checked native boundary".into(),
                    params: vec![Ty::Mixed, Ty::String],
                },
                ret: Ty::Mixed,
                arg_tys: vec![value.ty(), Ty::String],
                args: vec![value, TExpr::Str(descriptor.into_bytes())],
                span,
            }),
            ty: expected.clone(),
        }
    }
}

pub(super) fn builtin_parameter(kind: Builtin, index: usize, count: usize) -> Option<Ty> {
    use Builtin::*;
    Some(match kind {
        Strlen | Strtoupper | Strtolower | Trim | Ltrim | Rtrim | StrReplace | StrContains
        | StrStartsWith | StrEndsWith | Strrev | Ucfirst | Lcfirst | Ucwords | Addslashes
        | Stripslashes | Bin2hex | Base64Encode | Urlencode | Rawurlencode | Urldecode
        | Rawurldecode => Ty::String,
        Substr | StrRepeat | StrSplit | SubstrCount => {
            if index == 0 || (kind == SubstrCount && index == 1) {
                Ty::String
            } else {
                Ty::Int
            }
        }
        Explode => {
            if index < 2 {
                Ty::String
            } else {
                Ty::Int
            }
        }
        StrPad => {
            if index == 0 || index == 2 {
                Ty::String
            } else {
                Ty::Int
            }
        }
        Count | ArrayValues | ArrayMerge => Ty::array(Ty::Mixed),
        ArrayReverse => {
            if index == 0 {
                Ty::array(Ty::Mixed)
            } else {
                Ty::Bool
            }
        }
        ArraySlice => match index {
            0 => Ty::array(Ty::Mixed),
            3 => Ty::Bool,
            _ => Ty::Int,
        },
        ArrayPad => match index {
            0 => Ty::array(Ty::Mixed),
            1 => Ty::Int,
            _ => return None,
        },
        ArrayFill if index < 2 => Ty::Int,
        ArrayKeyExists if index == 1 => Ty::array(Ty::Mixed),
        Implode if count == 2 && index == 0 => Ty::String,
        Sprintf | Printf if index == 0 => Ty::String,
        _ => return None,
    })
}
