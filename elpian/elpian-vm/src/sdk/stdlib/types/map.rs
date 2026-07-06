//! `Map` methods — the members callable on a map value, grouped by the type
//! they relate to. Dispatched here from `stdlib::invoke` for any `Map.<m>` call.
//!
//! `keys`, `values`, and `containsKey` delegate to the single canonical builtin
//! implementation (`keys`/`values`/`has`) via the parent `stdlib::invoke`; the
//! remaining members keep their own logic because they have no builtin
//! equivalent with matching semantics (`remove` returns the removed value,
//! `putIfAbsent` inserts-and-returns, `isEmpty`/`isNotEmpty`).

use crate::sdk::data::*;
use crate::sdk::stdlib::*;

/// Invoke `Map.<method>` on `args[0]` (the receiver) plus trailing arguments.
pub(crate) fn invoke(name: &str, method: &str, args: &[Val]) -> Result<Val, String> {
    match method {
        // ---- delegated to the single canonical builtin implementation ------
        "keys" => super::super::invoke("keys", args),
        "values" => super::super::invoke("values", args),
        "containsKey" => super::super::invoke("has", args),

        // ---- members unique to the Map surface -----------------------------
        "remove" => {
            at_least(name, args, 2)?;
            let o = expect_object(name, &args[0])?;
            let removed = o.borrow_mut().data.data.remove(&args[1].as_string());
            Ok(removed.unwrap_or_else(|| Val::new(0, Payload::Null)))
        }
        "putIfAbsent" => {
            at_least(name, args, 3)?;
            let o = expect_object(name, &args[0])?;
            let key = args[1].as_string();
            let mut b = o.borrow_mut();
            if !b.data.data.contains_key(&key) {
                b.data.data.insert(key.clone(), args[2].clone());
            }
            let out = b.data.data.get(&key).cloned().unwrap_or_else(|| Val::new(0, Payload::Null));
            Ok(out)
        }
        "isEmpty" => {
            let o = expect_object(name, &args[0])?;
            let empty = o.borrow().data.data.is_empty();
            Ok(vbool(empty))
        }
        "isNotEmpty" => {
            let o = expect_object(name, &args[0])?;
            let empty = o.borrow().data.data.is_empty();
            Ok(vbool(!empty))
        }

        _ => Err(format!("unknown builtin '{name}'")),
    }
}
