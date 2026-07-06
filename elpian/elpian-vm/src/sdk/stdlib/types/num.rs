//! `Num` methods — the members callable on a num value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `Num.<m>` call.

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `Num.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        "toInt" => Ok(vi64(as_num(&args[0])?.trunc() as i64)),
        "toDouble" => Ok(vf64(as_num(&args[0])?)),
        "abs" => {
            if matches!(args[0].typ, 1 | 2 | 3) {
                Ok(vi64(as_int(&args[0])?.abs()))
            } else {
                Ok(vf64(as_num(&args[0])?.abs()))
            }
        }
        "floor" => Ok(vi64(as_num(&args[0])?.floor() as i64)),
        "ceil" => Ok(vi64(as_num(&args[0])?.ceil() as i64)),
        "round" => Ok(vi64(as_num(&args[0])?.round() as i64)),
        "isNaN" => Ok(vbool(as_num(&args[0])?.is_nan())),
        "isNegative" => Ok(vbool(as_num(&args[0])? < 0.0)),
        "toString" => {
            if matches!(args[0].typ, 1 | 2 | 3) {
                Ok(vstr(as_int(&args[0])?.to_string()))
            } else {
                let d = as_num(&args[0])?;
                Ok(vstr(if d.fract() == 0.0 { format!("{d:.1}") } else { format!("{d}") }))
            }
        }
        "toStringAsFixed" => {
            at_least(name, args, 2)?;
            let d = as_num(&args[0])?;
            let k = as_int(&args[1])? as usize;
            Ok(vstr(format!("{d:.*}", k)))
        }
        "clamp" => {
            at_least(name, args, 3)?;
            let (x, lo, hi) = (as_num(&args[0])?, as_num(&args[1])?, as_num(&args[2])?);
            Ok(num_result(x.max(lo).min(hi)))
        }

        // ---- more List methods --------------------------------------------
        _ => Err(format!("unknown builtin '{name}'")),
    }
}
