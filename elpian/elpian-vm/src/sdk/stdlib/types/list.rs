//! `List` methods — the members callable on a list value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `List.<m>` call.
//!
//! This layer is an **adapter**, not a second implementation: every operation
//! that also exists as a core builtin (`contains`, `indexOf`, `join`, `first`,
//! `last`, …) delegates to that single canonical implementation via the parent
//! `stdlib::invoke`, so a list operation behaves identically whether it is
//! reached as a bare builtin call (the functional surface one front-end lowers
//! to) or as a `list.method(...)` member call (the surface another lowers to).
//! Only members with no builtin equivalent carry their own logic here.

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `List.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        // ---- delegated to the single canonical builtin implementation ------
        "contains" => super::super::invoke("contains", args),
        "indexOf" => super::super::invoke("indexOf", args),
        "first" => super::super::invoke("first", args),
        "last" => super::super::invoke("last", args),
        "join" => {
            // Dart's separator is optional (defaults to ""); the canonical `join`
            // builtin takes it positionally, so supply the default when omitted.
            if args.len() < 2 {
                let recv = args.first().cloned().unwrap_or_else(|| varr(vec![]));
                super::super::invoke("join", &[recv, vstr(String::new())])
            } else {
                super::super::invoke("join", args)
            }
        }

        // ---- members unique to the List surface ----------------------------
        "add" => {
            at_least(name, args, 2)?;
            args[0].as_array().borrow_mut().data.push(args[1].clone());
            Ok(Val::new(0, Payload::Null))
        }
        "removeLast" => {
            let popped = args[0].as_array().borrow_mut().data.pop();
            Ok(popped.unwrap_or_else(|| Val::new(0, Payload::Null)))
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
            // Dart's `reversed` yields a *new* sequence without mutating the
            // receiver, so it cannot delegate to the in-place `reverse` builtin.
            let mut v = args[0].as_array().borrow().data.clone();
            v.reverse();
            Ok(varr(v))
        }

        _ => Err(format!("unknown builtin '{name}'")),
    }
}
