//! `String` methods — the members callable on a string value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `String.<m>` call.

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `String.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        "toUpperCase" => Ok(vstr(args[0].as_string().to_uppercase())),
        "toLowerCase" => Ok(vstr(args[0].as_string().to_lowercase())),
        "trim" => Ok(vstr(args[0].as_string().trim().to_string())),
        "contains" => {
            at_least(name, args, 2)?;
            Ok(vbool(args[0].as_string().contains(&args[1].as_string())))
        }
        "startsWith" => {
            at_least(name, args, 2)?;
            Ok(vbool(args[0].as_string().starts_with(&args[1].as_string())))
        }
        "endsWith" => {
            at_least(name, args, 2)?;
            Ok(vbool(args[0].as_string().ends_with(&args[1].as_string())))
        }
        "replaceAll" => {
            at_least(name, args, 3)?;
            Ok(vstr(args[0].as_string().replace(&args[1].as_string(), &args[2].as_string())))
        }
        "indexOf" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let needle = args[1].as_string();
            let idx = s.find(&needle).map(|b| s[..b].chars().count() as i64).unwrap_or(-1);
            Ok(vi64(idx))
        }
        "substring" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let chars: Vec<char> = s.chars().collect();
            let start = (as_int(&args[1])? as usize).min(chars.len());
            let end = match args.get(2) {
                Some(v) => (as_int(v)? as usize).min(chars.len()),
                None => chars.len(),
            }
            .max(start);
            Ok(vstr(chars[start..end].iter().collect()))
        }
        "split" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let sep = args[1].as_string();
            Ok(varr(s.split(&sep).map(|p| vstr(p.to_string())).collect()))
        }
        "codeUnitAt" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let i = as_int(&args[1])? as usize;
            Ok(vi64(s.encode_utf16().nth(i).map(|c| c as i64).unwrap_or(0)))
        }
        "padRight" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let width = as_int(&args[1])? as usize;
            let pad = args.get(2).map(|v| v.as_string()).unwrap_or_else(|| " ".into());
            let padc = pad.chars().next().unwrap_or(' ');
            let deficit = width.saturating_sub(s.chars().count());
            Ok(vstr(format!("{}{}", s, padc.to_string().repeat(deficit))))
        }
        "padLeft" => {
            at_least(name, args, 2)?;
            let s = args[0].as_string();
            let width = as_int(&args[1])? as usize;
            let pad = args.get(2).map(|v| v.as_string()).unwrap_or_else(|| " ".into());
            let padc = pad.chars().next().unwrap_or(' ');
            let deficit = width.saturating_sub(s.chars().count());
            Ok(vstr(format!("{}{}", padc.to_string().repeat(deficit), s)))
        }
        "replaceFirst" => {
            at_least(name, args, 3)?;
            Ok(vstr(args[0].as_string().replacen(&args[1].as_string(), &args[2].as_string(), 1)))
        }
        "trimLeft" => Ok(vstr(args[0].as_string().trim_start().to_string())),
        "trimRight" => Ok(vstr(args[0].as_string().trim_end().to_string())),

        // ---- num methods (receiver is a number) ----------------------------
        _ => Err(format!("unknown builtin '{name}'")),
    }
}
