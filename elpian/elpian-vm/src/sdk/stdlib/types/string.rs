//! `String` methods — the members callable on a string value, grouped by the
//! type they relate to. Dispatched here from `stdlib::invoke` for any
//! `String.<m>` call.
//!
//! Like the `List` adapter, every operation that has a core builtin equivalent
//! delegates to that single canonical implementation via the parent
//! `stdlib::invoke` rather than re-implementing it, so a string operation
//! behaves identically whether reached as a bare builtin call or as a
//! `str.method(...)` member call. Only members with no builtin equivalent keep
//! their own logic here.

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `String.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        // ---- delegated to the single canonical builtin implementation ------
        "toUpperCase" => super::super::invoke("upper", args),
        "toLowerCase" => super::super::invoke("lower", args),
        "trim" => super::super::invoke("trim", args),
        "contains" => super::super::invoke("contains", args),
        "startsWith" => super::super::invoke("startsWith", args),
        "endsWith" => super::super::invoke("endsWith", args),
        "replaceAll" => super::super::invoke("replace", args),
        "indexOf" => super::super::invoke("indexOf", args),
        "substring" => super::super::invoke("substring", args),
        "split" => super::super::invoke("split", args),
        "padRight" => super::super::invoke("padEnd", args),
        "padLeft" => super::super::invoke("padStart", args),

        // ---- members unique to the String surface --------------------------
        "codeUnitAt" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let i = as_int(&args[1])? as usize;
            Ok(vi64(s.encode_utf16().nth(i).map(|c| c as i64).unwrap_or(0)))
        }
        "replaceFirst" => {
            at_least(name, args, 3)?;
            Ok(vstr(args[0].as_string().replacen(&args[1].as_string(), &args[2].as_string(), 1)))
        }
        "trimLeft" => Ok(vstr(args[0].as_string().trim_start().to_string())),
        "trimRight" => Ok(vstr(args[0].as_string().trim_end().to_string())),

        _ => Err(format!("unknown builtin '{name}'")),
    }
}
