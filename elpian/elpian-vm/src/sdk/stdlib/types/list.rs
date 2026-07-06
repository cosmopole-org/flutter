//! `List` methods — the members callable on a list value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `List.<m>` call.

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `List.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        "add" => {
            at_least(name, args, 2)?;
            args[0].as_array().borrow_mut().data.push(args[1].clone());
            Ok(Val::new(0, Payload::Null))
        }
        "removeLast" => {
            let popped = args[0].as_array().borrow_mut().data.pop();
            Ok(popped.unwrap_or_else(|| Val::new(0, Payload::Null)))
        }
        "first" => {
            let a = args[0].as_array();
            let out = a.borrow().data.first().cloned();
            Ok(out.unwrap_or_else(|| Val::new(0, Payload::Null)))
        }
        "last" => {
            let a = args[0].as_array();
            let out = a.borrow().data.last().cloned();
            Ok(out.unwrap_or_else(|| Val::new(0, Payload::Null)))
        }
        "contains" => {
            at_least(name, args, 2)?;
            let target = args[1].stringify();
            let a = args[0].as_array();
            let found = a.borrow().data.iter().any(|e| e.stringify() == target);
            Ok(vbool(found))
        }
        "indexOf" => {
            at_least(name, args, 2)?;
            let target = args[1].stringify();
            let a = args[0].as_array();
            let idx = a
                .borrow()
                .data
                .iter()
                .position(|e| e.stringify() == target)
                .map(|i| i as i64)
                .unwrap_or(-1);
            Ok(vi64(idx))
        }
        "sublist" => {
            at_least(name, args, 2)?;
            let a = args[0].as_array();
            let b = a.borrow();
            let start = (as_int(&args[1])? as usize).min(b.data.len());
            let end = match args.get(2) {
                Some(v) => (as_int(v)? as usize).min(b.data.len()),
                None => b.data.len(),
            }
            .max(start);
            Ok(varr(b.data[start..end].to_vec()))
        }
        "join" => {
            let a = args[0].as_array();
            let sep = args.get(1).map(|v| v.as_string()).unwrap_or_default();
            let joined = a
                .borrow()
                .data
                .iter()
                .map(|e| if e.typ == 7 { e.as_string() } else { e.stringify() })
                .collect::<Vec<_>>()
                .join(&sep);
            Ok(vstr(joined))
        }
        "addAll" => {
            at_least(name, args, 2)?;
            let other = args[1].as_array().borrow().data.clone();
            args[0].as_array().borrow_mut().data.extend(other);
            Ok(Val::new(0, Payload::Null))
        }
        "removeAt" => {
            at_least(name, args, 2)?;
            let i = as_int(&args[1])? as usize;
            let a = args[0].as_array();
            let mut b = a.borrow_mut();
            if i < b.data.len() {
                Ok(b.data.remove(i))
            } else {
                Err("RangeError: removeAt out of range".into())
            }
        }
        "insert" => {
            at_least(name, args, 3)?;
            let i = as_int(&args[1])? as usize;
            let a = args[0].as_array();
            let mut b = a.borrow_mut();
            let idx = i.min(b.data.len());
            b.data.insert(idx, args[2].clone());
            Ok(Val::new(0, Payload::Null))
        }
        "clear" => {
            args[0].as_array().borrow_mut().data.clear();
            Ok(Val::new(0, Payload::Null))
        }
        "reversed" => {
            let mut v = args[0].as_array().borrow().data.clone();
            v.reverse();
            Ok(varr(v))
        }

        // ---- Map methods (receiver is a plain object) ----------------------
        _ => Err(format!("unknown builtin '{name}'")),
    }
}
