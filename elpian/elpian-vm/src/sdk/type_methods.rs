//! The authoritative catalog of built-in **type methods** — the members exposed
//! by core values (`List`, `String`, `num`, `Map`). It is the single owner of
//! every type-method *name* and of *how* each dispatches, so the executor holds
//! no hardcoded method knowledge: it asks [`resolve`] and acts on the returned
//! [`Member`]. The *implementations* live in [`crate::sdk::stdlib`]; this module
//! maps a `(type, name)` pair to a qualified stdlib key plus a dispatch strategy.
//!
//! Organised object-orientedly — one submodule per core type, each declaring its
//! own members — so adding a method is a one-line change in exactly one place,
//! and the executor never duplicates a method name. This is the decoupling seam
//! between *what* a type can do (here) and *how the interpreter delivers it*.

/// A core built-in type, identified from a VM value's type tag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoreType {
    List,
    String,
    Num,
    Map,
}

impl CoreType {
    /// The core type of a value tag, if it is a built-in type with members.
    /// Tags: `9` = List, `7` = String, `1..=5` = numeric (int/double variants),
    /// `8` = plain object / Map.
    pub fn of_tag(tag: i64) -> Option<CoreType> {
        match tag {
            9 => Some(CoreType::List),
            7 => Some(CoreType::String),
            1..=5 => Some(CoreType::Num),
            8 => Some(CoreType::Map),
            _ => None,
        }
    }

    /// The stdlib namespace prefix for this type's members (`List.add`, …).
    pub fn prefix(self) -> &'static str {
        match self {
            CoreType::List => "List",
            CoreType::String => "String",
            CoreType::Num => "Num",
            CoreType::Map => "Map",
        }
    }
}

/// How a resolved member is delivered to the executor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dispatch {
    /// A getter (read, no call): evaluate now via
    /// `stdlib::invoke(qualified, &[receiver])`.
    Getter,
    /// A method: hand back a bound native (VM type-tag 253) that, when called,
    /// runs `stdlib::invoke(qualified, &[receiver, ..args])`.
    Method,
    /// A higher-order method realised as the guest prelude fn `__<Type>_<name>`,
    /// bound to the receiver so its closure argument runs as guest bytecode.
    Prelude,
}

/// A resolved member: how it dispatches, its fully-qualified stdlib key
/// (`"List.add"`), and its bare name (for prelude binding: `__List_<name>`).
pub struct Member {
    pub dispatch: Dispatch,
    pub qualified: String,
    pub name: String,
}

/// Resolve `name` as a member of `ty`, or `None` if the type has no such member.
pub fn resolve(ty: CoreType, name: &str) -> Option<Member> {
    let dispatch = match ty {
        CoreType::List => list::dispatch(name),
        CoreType::String => string::dispatch(name),
        CoreType::Num => num::dispatch(name),
        CoreType::Map => map::dispatch(name),
    }?;
    Some(Member {
        dispatch,
        qualified: format!("{}.{}", ty.prefix(), name),
        name: name.to_string(),
    })
}

/// Whether `ty` has a member named `name` — the existence check the executor
/// asks before deciding how to read `receiver.name`.
pub fn has(ty: CoreType, name: &str) -> bool {
    resolve(ty, name).is_some()
}

// --- per-type member catalogs (object-oriented grouping) --------------------

mod list {
    use super::Dispatch;
    /// Members of `List`. Getters read eagerly; the higher-order closure methods
    /// run as guest prelude functions; the rest are bound native methods.
    pub fn dispatch(name: &str) -> Option<Dispatch> {
        Some(match name {
            "first" | "last" | "reversed" => Dispatch::Getter,
            "map" | "where" | "forEach" | "fold" | "any" | "every" | "reduce" => Dispatch::Prelude,
            "add" | "contains" | "indexOf" | "removeLast" | "sublist" | "join" | "addAll"
            | "removeAt" | "insert" | "clear" => Dispatch::Method,
            _ => return None,
        })
    }
}

mod string {
    use super::Dispatch;
    /// Members of `String` — all bound native methods over the receiver string.
    pub fn dispatch(name: &str) -> Option<Dispatch> {
        Some(match name {
            "substring" | "contains" | "indexOf" | "toUpperCase" | "toLowerCase" | "trim"
            | "split" | "startsWith" | "endsWith" | "replaceAll" | "codeUnitAt" | "padRight"
            | "padLeft" | "replaceFirst" | "trimLeft" | "trimRight" => Dispatch::Method,
            _ => return None,
        })
    }
}

mod num {
    use super::Dispatch;
    /// Members of `num`/`int`/`double`.
    pub fn dispatch(name: &str) -> Option<Dispatch> {
        Some(match name {
            "isNaN" | "isNegative" => Dispatch::Getter,
            "toInt" | "toDouble" | "abs" | "floor" | "ceil" | "round" | "toString"
            | "toStringAsFixed" | "clamp" => Dispatch::Method,
            _ => return None,
        })
    }
}

mod map {
    use super::Dispatch;
    /// Members of a plain `Map` (objects without a `__class` tag).
    pub fn dispatch(name: &str) -> Option<Dispatch> {
        Some(match name {
            "keys" | "values" | "isEmpty" | "isNotEmpty" => Dispatch::Getter,
            "containsKey" | "remove" | "putIfAbsent" => Dispatch::Method,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_by_type_and_kind() {
        assert_eq!(resolve(CoreType::List, "add").unwrap().dispatch, Dispatch::Method);
        assert_eq!(resolve(CoreType::List, "first").unwrap().dispatch, Dispatch::Getter);
        assert_eq!(resolve(CoreType::List, "map").unwrap().dispatch, Dispatch::Prelude);
        assert_eq!(resolve(CoreType::String, "toUpperCase").unwrap().dispatch, Dispatch::Method);
        assert_eq!(resolve(CoreType::Num, "isNaN").unwrap().dispatch, Dispatch::Getter);
        assert_eq!(resolve(CoreType::Map, "keys").unwrap().dispatch, Dispatch::Getter);
        assert_eq!(resolve(CoreType::List, "add").unwrap().qualified, "List.add");
    }

    #[test]
    fn unknown_members_are_none() {
        assert!(resolve(CoreType::List, "nope").is_none());
        assert!(!has(CoreType::String, "add")); // add is a List method, not String
        assert!(CoreType::of_tag(99).is_none());
        assert_eq!(CoreType::of_tag(9), Some(CoreType::List));
    }
}
