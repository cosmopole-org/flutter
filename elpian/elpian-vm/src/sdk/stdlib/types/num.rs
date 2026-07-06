//! `Num` methods — the members callable on a num value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `Num.<m>` call.
//!
//! The operations that share the core builtins' exact semantics (`floor`,
//! `ceil`, `round`, `isNaN`, `clamp`) delegate to that single implementation via
//! the parent `stdlib::invoke`. The rest keep bespoke logic because they carry
//! the Dart-specific `int`/`double` distinction the collapsing math builtins
//! deliberately do not (e.g. `(-3.0).abs()` must stay a `double`).

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `Num.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        // ---- delegated to the single canonical builtin implementation ------
        "floor" => super::super::invoke("floor", args),
        "ceil" => super::super::invoke("ceil", args),
        "round" => super::super::invoke("round", args),
        "isNaN" => super::super::invoke("isNaN", args),
        "clamp" => super::super::invoke("clamp", args),

        // ---- members carrying the Dart int/double distinction --------------
        "toInt" => Ok(vi64(as_num(&args[0])?.trunc() as i64)),
        "toDouble" => Ok(vf64(as_num(&args[0])?)),
        "abs" => {
            if matches!(args[0].typ, 1 | 2 | 3) {
                Ok(vi64(as_int(&args[0])?.abs()))
            } else {
                Ok(vf64(as_num(&args[0])?.abs()))
            }
        }
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

        _ => Err(format!("unknown builtin '{name}'")),
    }
}
