//! Reified types, subtyping, `const` canonicalization, and `noSuchMethod`.
//!
//! Where Dart most deeply diverges from a JS value model is that Dart types are
//! **reified and sound**: `x is List<int>` really inspects the element type at
//! runtime, `as` throws on a mismatch, generics carry their arguments, `const`
//! objects are canonicalized so structurally-equal constants are `identical`,
//! and a missing member dispatches to `noSuchMethod` rather than silently
//! returning `undefined`. This module is the runtime substrate for all of that.
//!
//! It is standalone and fully unit-tested. Wiring it to the front-end (which
//! must emit type metadata at allocation sites and `is`/`as` positions) is a
//! later step; this provides the semantics those emissions will call into.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Reified type representation
// ---------------------------------------------------------------------------

/// The "shape" of a Dart type, independent of its nullability.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// The top type `dynamic` (and, for our purposes, `void`).
    Dynamic,
    Object,
    Null,
    /// The bottom type `Never`.
    Never,
    Num,
    Int,
    Double,
    Bool,
    Str,
    /// `List<E>`.
    List(Box<DartType>),
    /// A user-declared class `Name<args...>`.
    Interface(String, Vec<DartType>),
    /// A function type `(params...) -> ret`.
    Function {
        ret: Box<DartType>,
        params: Vec<DartType>,
    },
    /// A generic type parameter reference (e.g. `T`), resolved by substitution.
    TypeParam(String),
}

/// A reified Dart type: a shape plus a nullability flag (`T` vs `T?`).
#[derive(Debug, Clone, PartialEq)]
pub struct DartType {
    pub kind: TypeKind,
    pub nullable: bool,
}

impl DartType {
    pub fn new(kind: TypeKind, nullable: bool) -> Self {
        DartType { kind, nullable }
    }
    pub fn int() -> Self {
        DartType::new(TypeKind::Int, false)
    }
    pub fn double() -> Self {
        DartType::new(TypeKind::Double, false)
    }
    pub fn num() -> Self {
        DartType::new(TypeKind::Num, false)
    }
    pub fn boolean() -> Self {
        DartType::new(TypeKind::Bool, false)
    }
    pub fn string() -> Self {
        DartType::new(TypeKind::Str, false)
    }
    pub fn object() -> Self {
        DartType::new(TypeKind::Object, false)
    }
    pub fn list(elem: DartType) -> Self {
        DartType::new(TypeKind::List(Box::new(elem)), false)
    }
    pub fn interface(name: &str, args: Vec<DartType>) -> Self {
        DartType::new(TypeKind::Interface(name.to_string(), args), false)
    }
    pub fn function(ret: DartType, params: Vec<DartType>) -> Self {
        DartType::new(TypeKind::Function { ret: Box::new(ret), params }, false)
    }
    pub fn type_param(name: &str) -> Self {
        DartType::new(TypeKind::TypeParam(name.to_string()), false)
    }

    /// Substitute type parameters using `env` (e.g. instantiating `List<T>` with
    /// `T=int`). Nullability is preserved across substitution.
    pub fn substitute(&self, env: &std::collections::HashMap<String, DartType>) -> DartType {
        let kind = match &self.kind {
            TypeKind::TypeParam(name) => {
                if let Some(t) = env.get(name) {
                    // A nullable `T?` stays nullable after substitution.
                    let mut r = t.clone();
                    r.nullable = r.nullable || self.nullable;
                    return r;
                }
                TypeKind::TypeParam(name.clone())
            }
            TypeKind::List(e) => TypeKind::List(Box::new(e.substitute(env))),
            TypeKind::Interface(n, args) => {
                TypeKind::Interface(n.clone(), args.iter().map(|a| a.substitute(env)).collect())
            }
            TypeKind::Function { ret, params } => TypeKind::Function {
                ret: Box::new(ret.substitute(env)),
                params: params.iter().map(|p| p.substitute(env)).collect(),
            },
            other => other.clone(),
        };
        DartType::new(kind, self.nullable)
    }
    /// The nullable version of this type (`T` -> `T?`).
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    fn is_top(&self) -> bool {
        matches!(self.kind, TypeKind::Dynamic) || (matches!(self.kind, TypeKind::Object) && self.nullable)
    }
}

// ---------------------------------------------------------------------------
// Class hierarchy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ClassInfo {
    superclass: Option<String>,
    interfaces: Vec<String>,
    methods: Vec<String>,
}

/// The registered class hierarchy: used for interface subtyping and method
/// resolution / `noSuchMethod`.
#[derive(Debug, Default)]
pub struct ClassTable {
    classes: HashMap<String, ClassInfo>,
}

/// Outcome of resolving a member on a class.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// The class (or a supertype) declares the member.
    Found,
    /// No such member; Dart would invoke `noSuchMethod` with this invocation.
    NoSuchMethod(Invocation),
}

/// A reified description of a failed call, as passed to `noSuchMethod`.
#[derive(Debug, Clone, PartialEq)]
pub struct Invocation {
    pub member_name: String,
    pub positional_arg_count: usize,
}

impl ClassTable {
    pub fn new() -> Self {
        ClassTable::default()
    }

    pub fn register(
        &mut self,
        name: &str,
        superclass: Option<&str>,
        interfaces: &[&str],
        methods: &[&str],
    ) {
        self.classes.insert(
            name.to_string(),
            ClassInfo {
                superclass: superclass.map(|s| s.to_string()),
                interfaces: interfaces.iter().map(|s| s.to_string()).collect(),
                methods: methods.iter().map(|s| s.to_string()).collect(),
            },
        );
    }

    /// Is `sub` the same class as, or a subclass/implementer of, `sup`?
    pub fn is_subclass(&self, sub: &str, sup: &str) -> bool {
        if sub == sup {
            return true;
        }
        let mut stack = vec![sub.to_string()];
        while let Some(cur) = stack.pop() {
            if cur == sup {
                return true;
            }
            if let Some(info) = self.classes.get(&cur) {
                if let Some(s) = &info.superclass {
                    stack.push(s.clone());
                }
                for i in &info.interfaces {
                    stack.push(i.clone());
                }
            }
        }
        false
    }

    /// Resolve a member call, walking supertypes; returns `NoSuchMethod` when
    /// absent, exactly modelling Dart's dispatch fallback.
    pub fn resolve(&self, class: &str, member: &str, positional_arg_count: usize) -> Resolution {
        let mut stack = vec![class.to_string()];
        while let Some(cur) = stack.pop() {
            if let Some(info) = self.classes.get(&cur) {
                if info.methods.iter().any(|m| m == member) {
                    return Resolution::Found;
                }
                if let Some(s) = &info.superclass {
                    stack.push(s.clone());
                }
                for i in &info.interfaces {
                    stack.push(i.clone());
                }
            }
        }
        Resolution::NoSuchMethod(Invocation {
            member_name: member.to_string(),
            positional_arg_count,
        })
    }

    // ---- subtyping -------------------------------------------------------

    /// Dart's subtype relation `sub <: sup`, covering null-safety, the numeric
    /// tower, covariant `List`, and interface inheritance.
    pub fn is_subtype(&self, sub: &DartType, sup: &DartType) -> bool {
        // Top type on the right absorbs everything.
        if sup.is_top() {
            return true;
        }
        // Bottom type on the left is a subtype of everything.
        if matches!(sub.kind, TypeKind::Never) {
            return true;
        }
        // Nullability: a nullable source needs a nullable (or top) target.
        if sub.nullable && !sup.nullable && !sup.is_top() {
            return false;
        }
        // Null literal type.
        if matches!(sub.kind, TypeKind::Null) {
            return sup.nullable || matches!(sup.kind, TypeKind::Null);
        }
        // Object (non-nullable) is a supertype of every non-nullable type.
        if matches!(sup.kind, TypeKind::Object) && !sup.nullable {
            return !sub.nullable;
        }

        self.kind_subtype(&sub.kind, &sup.kind)
    }

    fn kind_subtype(&self, sub: &TypeKind, sup: &TypeKind) -> bool {
        use TypeKind::*;
        match (sub, sup) {
            (a, b) if a == b => true,
            // Numeric tower: int <: num, double <: num.
            (Int, Num) | (Double, Num) => true,
            // Covariant List.
            (List(a), List(b)) => self.is_subtype(a, b),
            // Function subtyping: covariant return, contravariant parameters.
            (Function { ret: r1, params: p1 }, Function { ret: r2, params: p2 }) => {
                p1.len() == p2.len()
                    && self.is_subtype(r1, r2)
                    && p1.iter().zip(p2).all(|(a, b)| self.is_subtype(b, a))
            }
            // User interfaces: walk the hierarchy; args covariant when same arity.
            (Interface(n1, a1), Interface(n2, a2)) => {
                if !self.is_subclass(n1, n2) {
                    return false;
                }
                // If both name the same class, require covariant arg subtyping.
                if n1 == n2 && a1.len() == a2.len() {
                    return a1.iter().zip(a2).all(|(x, y)| self.is_subtype(x, y));
                }
                true
            }
            _ => false,
        }
    }

    /// Evaluate `value_type is T` (an `is` test) — sound reified check.
    pub fn value_is(&self, value_type: &DartType, test: &DartType) -> bool {
        self.is_subtype(value_type, test)
    }

    /// Evaluate `value as T` — returns Ok on success or a Dart `TypeError`
    /// message on failure, matching Dart's `as` semantics.
    pub fn value_as(&self, value_type: &DartType, target: &DartType) -> Result<(), String> {
        if self.is_subtype(value_type, target) {
            Ok(())
        } else {
            Err(format!(
                "TypeError: {:?} is not a subtype of {:?}",
                value_type.kind, target.kind
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// const canonicalization & identity
// ---------------------------------------------------------------------------

/// A compile-time-constant value, for canonicalization.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i64),
    /// Stored as bits so NaN/`-0.0` canonicalize by bit-pattern like Dart const.
    Double(u64),
    Bool(bool),
    Str(String),
    Null,
    /// A `const` instance: class name + canonicalized field values in order.
    Instance(String, Vec<(String, ConstValue)>),
}

impl ConstValue {
    pub fn double(d: f64) -> Self {
        ConstValue::Double(d.to_bits())
    }

    fn canonical_key(&self) -> String {
        match self {
            ConstValue::Int(i) => format!("i:{i}"),
            ConstValue::Double(b) => format!("d:{b}"),
            ConstValue::Bool(b) => format!("b:{b}"),
            ConstValue::Str(s) => format!("s:{}:{s}", s.len()),
            ConstValue::Null => "n".into(),
            ConstValue::Instance(name, fields) => {
                let mut k = format!("c:{name}(");
                for (fname, fval) in fields {
                    k.push_str(fname);
                    k.push('=');
                    k.push_str(&fval.canonical_key());
                    k.push(';');
                }
                k.push(')');
                k
            }
        }
    }

    /// `hashCode` consistent with `==`/`identical` for canonicalized constants.
    pub fn hash_code(&self) -> i64 {
        // FNV-1a over the canonical key — stable and equal for equal values.
        let mut h: u64 = 0xcbf29ce484222325;
        for byte in self.canonical_key().as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        (h & 0x7fff_ffff_ffff_ffff) as i64
    }
}

/// Canonical id assigned to a constant. Equal ids mean `identical`.
pub type CanonId = u32;

/// The const canonicalization table: structurally-equal constants receive the
/// same [`CanonId`], so `identical(a, b)` holds exactly as in Dart.
#[derive(Debug, Default)]
pub struct ConstTable {
    by_key: HashMap<String, CanonId>,
    next: CanonId,
}

impl ConstTable {
    pub fn new() -> Self {
        ConstTable::default()
    }

    /// Intern a constant, returning its canonical id.
    pub fn canonicalize(&mut self, v: &ConstValue) -> CanonId {
        let key = v.canonical_key();
        if let Some(id) = self.by_key.get(&key) {
            return *id;
        }
        let id = self.next;
        self.next += 1;
        self.by_key.insert(key, id);
        id
    }

    /// `identical(a, b)` for two constants.
    pub fn identical(&mut self, a: &ConstValue, b: &ConstValue) -> bool {
        self.canonicalize(a) == self.canonicalize(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_tower_and_nullability() {
        let t = ClassTable::new();
        assert!(t.is_subtype(&DartType::int(), &DartType::num()));
        assert!(t.is_subtype(&DartType::double(), &DartType::num()));
        assert!(!t.is_subtype(&DartType::num(), &DartType::int()));
        // int <: int?  but  int? </: int
        assert!(t.is_subtype(&DartType::int(), &DartType::int().nullable()));
        assert!(!t.is_subtype(&DartType::int().nullable(), &DartType::int()));
        // Object is not a supertype of a nullable type; Object? is.
        assert!(t.is_subtype(&DartType::int(), &DartType::object()));
        assert!(!t.is_subtype(&DartType::int().nullable(), &DartType::object()));
        assert!(t.is_subtype(&DartType::int().nullable(), &DartType::object().nullable()));
    }

    #[test]
    fn list_is_covariant() {
        let t = ClassTable::new();
        assert!(t.is_subtype(
            &DartType::list(DartType::int()),
            &DartType::list(DartType::num())
        ));
        assert!(!t.is_subtype(
            &DartType::list(DartType::num()),
            &DartType::list(DartType::int())
        ));
    }

    #[test]
    fn interface_inheritance_subtyping() {
        let mut t = ClassTable::new();
        t.register("Animal", None, &[], &["eat"]);
        t.register("Dog", Some("Animal"), &[], &["bark"]);
        assert!(t.is_subtype(
            &DartType::interface("Dog", vec![]),
            &DartType::interface("Animal", vec![])
        ));
        assert!(!t.is_subtype(
            &DartType::interface("Animal", vec![]),
            &DartType::interface("Dog", vec![])
        ));
    }

    #[test]
    fn is_and_as_checks() {
        let t = ClassTable::new();
        assert!(t.value_is(&DartType::int(), &DartType::num()));
        assert!(!t.value_is(&DartType::num(), &DartType::int()));
        assert!(t.value_as(&DartType::int(), &DartType::num()).is_ok());
        assert!(t.value_as(&DartType::num(), &DartType::int()).is_err());
    }

    #[test]
    fn nosuchmethod_resolution() {
        let mut t = ClassTable::new();
        t.register("Animal", None, &[], &["eat"]);
        t.register("Dog", Some("Animal"), &[], &["bark"]);
        assert_eq!(t.resolve("Dog", "bark", 0), Resolution::Found);
        assert_eq!(t.resolve("Dog", "eat", 0), Resolution::Found); // inherited
        match t.resolve("Dog", "fly", 2) {
            Resolution::NoSuchMethod(inv) => {
                assert_eq!(inv.member_name, "fly");
                assert_eq!(inv.positional_arg_count, 2);
            }
            _ => panic!("expected noSuchMethod"),
        }
    }

    #[test]
    fn function_subtyping_is_variance_correct() {
        let t = ClassTable::new();
        // (num) -> int  <:  (int) -> num   (contravariant params, covariant ret)
        let a = DartType::function(DartType::int(), vec![DartType::num()]);
        let b = DartType::function(DartType::num(), vec![DartType::int()]);
        assert!(t.is_subtype(&a, &b));
        assert!(!t.is_subtype(&b, &a));
    }

    #[test]
    fn generic_substitution_instantiates_type_params() {
        let mut env = std::collections::HashMap::new();
        env.insert("T".to_string(), DartType::int());
        // List<T> with T=int  ==  List<int>
        let list_t = DartType::list(DartType::type_param("T"));
        assert_eq!(list_t.substitute(&env), DartType::list(DartType::int()));
        // Nullable T? stays nullable.
        let nullable_t = DartType::type_param("T").nullable();
        assert_eq!(nullable_t.substitute(&env), DartType::int().nullable());
    }

    #[test]
    fn const_canonicalization_makes_equal_constants_identical() {
        let mut c = ConstTable::new();
        let a = ConstValue::Instance(
            "Point".into(),
            vec![("x".into(), ConstValue::Int(1)), ("y".into(), ConstValue::Int(2))],
        );
        let b = ConstValue::Instance(
            "Point".into(),
            vec![("x".into(), ConstValue::Int(1)), ("y".into(), ConstValue::Int(2))],
        );
        let d = ConstValue::Instance(
            "Point".into(),
            vec![("x".into(), ConstValue::Int(1)), ("y".into(), ConstValue::Int(3))],
        );
        assert!(c.identical(&a, &b), "equal const instances are identical");
        assert!(!c.identical(&a, &d), "different const instances are not identical");
        assert_eq!(a.hash_code(), b.hash_code());
    }
}
