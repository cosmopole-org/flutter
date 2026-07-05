//! Dart numeric tower on top of the Elpian value model.
//!
//! One of the "group 2" semantic gaps between Dart and a JavaScript value model
//! is that Dart has two *distinct* numeric types — `int` (64-bit, two's
//! complement, wrapping) and `double` (IEEE-754 binary64) — with rules that a
//! single JS `number` cannot express faithfully:
//!
//! * `1 is int` is true, `1.0 is int` is false;
//! * `/` always yields a `double` (`5 / 2 == 2.5`), while `~/` (truncating
//!   division) yields an `int` (`5 ~/ 2 == 2`);
//! * `int` arithmetic wraps on overflow (`0x7fff... + 1` wraps negative)
//!   rather than losing precision the way a float64 would.
//!
//! The Elpian executor already represents integers and floats with *separate*
//! value tags (`typ` 1/2/3 for i16/i32/i64, `typ` 4/5 for f32/f64), so the
//! representation split we need exists at the VM layer. This module supplies the
//! **semantic** layer on top: the Dart-correct operators and predicates. It is
//! pure and fully unit-tested, independent of any VM instance.

use serde_json::Value;

/// A Dart number: either an `int` (64-bit wrapping) or a `double` (binary64).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DartNum {
    Int(i64),
    Double(f64),
}

impl DartNum {
    /// `value is int` — true only for the integer variant, matching Dart where a
    /// `double` holding an integral value is still not an `int`.
    pub fn is_int(self) -> bool {
        matches!(self, DartNum::Int(_))
    }

    /// `value is double`.
    pub fn is_double(self) -> bool {
        matches!(self, DartNum::Double(_))
    }

    /// `.toDouble()` — widening is always exact for the range Dart guarantees.
    pub fn to_double(self) -> f64 {
        match self {
            DartNum::Int(i) => i as f64,
            DartNum::Double(d) => d,
        }
    }

    /// `+`, `-`, `*` follow Dart's numeric promotion: `int op int` stays `int`
    /// (with two's-complement wrapping), and any `double` operand promotes the
    /// whole expression to `double`.
    pub fn add(self, rhs: DartNum) -> DartNum {
        match (self, rhs) {
            (DartNum::Int(a), DartNum::Int(b)) => DartNum::Int(a.wrapping_add(b)),
            _ => DartNum::Double(self.to_double() + rhs.to_double()),
        }
    }

    pub fn sub(self, rhs: DartNum) -> DartNum {
        match (self, rhs) {
            (DartNum::Int(a), DartNum::Int(b)) => DartNum::Int(a.wrapping_sub(b)),
            _ => DartNum::Double(self.to_double() - rhs.to_double()),
        }
    }

    pub fn mul(self, rhs: DartNum) -> DartNum {
        match (self, rhs) {
            (DartNum::Int(a), DartNum::Int(b)) => DartNum::Int(a.wrapping_mul(b)),
            _ => DartNum::Double(self.to_double() * rhs.to_double()),
        }
    }

    /// `/` — Dart's slash operator *always* produces a `double`, even for two
    /// integer operands (`4 / 2 == 2.0`, still a `double`).
    pub fn div(self, rhs: DartNum) -> DartNum {
        DartNum::Double(self.to_double() / rhs.to_double())
    }

    /// `~/` — truncating division. Result is `int` when both operands are `int`,
    /// otherwise the `double` result truncated toward zero (still `int` in Dart).
    /// Division by an integer zero throws in Dart; we surface that as `None`.
    pub fn truncating_div(self, rhs: DartNum) -> Option<DartNum> {
        match (self, rhs) {
            (DartNum::Int(_), DartNum::Int(0)) => None,
            (DartNum::Int(a), DartNum::Int(b)) => Some(DartNum::Int(a.wrapping_div(b))),
            _ => {
                let q = (self.to_double() / rhs.to_double()).trunc();
                if q.is_finite() {
                    Some(DartNum::Int(q as i64))
                } else {
                    None
                }
            }
        }
    }

    /// Serialize back across the host seam. Integers stay JSON integers and
    /// doubles stay JSON reals so the guest observes the right runtime type.
    pub fn to_json(self) -> Value {
        match self {
            DartNum::Int(i) => Value::from(i),
            DartNum::Double(d) => Value::from(d),
        }
    }

    /// Reconstruct from a JSON value produced by the guest. A JSON integer maps
    /// to `int`; a JSON real (or any non-integral number) maps to `double`.
    pub fn from_json(v: &Value) -> Option<DartNum> {
        match v {
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(DartNum::Int(i))
                } else {
                    n.as_f64().map(DartNum::Double)
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_double_type_predicates_match_dart() {
        assert!(DartNum::Int(1).is_int());
        assert!(!DartNum::Int(1).is_double());
        // 1.0 is a double, NOT an int — the JS single-number model gets this wrong.
        assert!(!DartNum::Double(1.0).is_int());
        assert!(DartNum::Double(1.0).is_double());
    }

    #[test]
    fn slash_always_yields_double() {
        // 4 / 2 == 2.0 (a double), not the int 2.
        assert_eq!(DartNum::Int(4).div(DartNum::Int(2)), DartNum::Double(2.0));
    }

    #[test]
    fn truncating_div_yields_int_and_guards_zero() {
        assert_eq!(
            DartNum::Int(5).truncating_div(DartNum::Int(2)),
            Some(DartNum::Int(2))
        );
        assert_eq!(DartNum::Int(1).truncating_div(DartNum::Int(0)), None);
    }

    #[test]
    fn int_addition_wraps_like_dart() {
        assert_eq!(
            DartNum::Int(i64::MAX).add(DartNum::Int(1)),
            DartNum::Int(i64::MIN)
        );
    }

    #[test]
    fn double_operand_promotes() {
        assert_eq!(
            DartNum::Int(1).add(DartNum::Double(0.5)),
            DartNum::Double(1.5)
        );
    }

    #[test]
    fn json_roundtrip_preserves_int_vs_double() {
        assert_eq!(DartNum::from_json(&Value::from(3)), Some(DartNum::Int(3)));
        assert_eq!(
            DartNum::from_json(&Value::from(3.5)),
            Some(DartNum::Double(3.5))
        );
    }
}
