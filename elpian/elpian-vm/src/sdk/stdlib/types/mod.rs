//! Built-in **type methods**, grouped object-orientedly by the type they relate
//! to. `stdlib::invoke` routes a qualified `Type.method` call to the matching
//! submodule; the [`crate::sdk::type_methods`] registry owns the *names* and
//! dispatch strategy, these modules own the *implementations*.

pub(crate) mod list;
pub(crate) mod map;
pub(crate) mod num;
pub(crate) mod string;
