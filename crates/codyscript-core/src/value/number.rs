//! [`Number`]: a number as a data format gave it.

use std::fmt;

/// A number, kept in the form it was given: an integer stays exact, a float
/// stays a float.
///
/// Any `f64` can be held, including `NaN` and the infinities, because some
/// data formats (YAML, CBOR) can express them; `compile` refuses them in
/// programs. Equality is numeric: `1 == 1.0`, and `NaN != NaN`.
#[derive(Clone, Copy)]
pub struct Number(Repr);

#[derive(Clone, Copy)]
enum Repr {
    /// Zero or more.
    PosInt(u64),
    /// Less than zero.
    NegInt(i64),
    Float(f64),
}

impl Number {
    /// A float. Any `f64` is accepted, finite or not.
    pub const fn from_f64(n: f64) -> Self {
        Number(Repr::Float(n))
    }

    /// The number as an `f64`, rounding big integers to the nearest float.
    pub fn as_f64(&self) -> f64 {
        match self.0 {
            Repr::PosInt(n) => n as f64,
            Repr::NegInt(n) => n as f64,
            Repr::Float(n) => n,
        }
    }

    /// The number, if it's an integer that fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        match self.0 {
            Repr::PosInt(n) => i64::try_from(n).ok(),
            Repr::NegInt(n) => Some(n),
            Repr::Float(_) => None,
        }
    }

    /// The number, if it's an integer that fits a `u64`.
    pub fn as_u64(&self) -> Option<u64> {
        match self.0 {
            Repr::PosInt(n) => Some(n),
            Repr::NegInt(_) | Repr::Float(_) => None,
        }
    }

    /// Whether it's an integer that fits an `i64`.
    pub fn is_i64(&self) -> bool {
        self.as_i64().is_some()
    }

    /// Whether it's an integer that fits a `u64`.
    pub fn is_u64(&self) -> bool {
        self.as_u64().is_some()
    }

    /// Whether it's a float (even a whole one, like `1.0`).
    pub fn is_f64(&self) -> bool {
        matches!(self.0, Repr::Float(_))
    }

    /// Whether it's neither infinite nor `NaN`. Integers always are.
    pub fn is_finite(&self) -> bool {
        self.as_f64().is_finite()
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self.0, other.0) {
            (Repr::PosInt(a), Repr::PosInt(b)) => a == b,
            (Repr::NegInt(a), Repr::NegInt(b)) => a == b,
            (Repr::PosInt(_), Repr::NegInt(_)) | (Repr::NegInt(_), Repr::PosInt(_)) => false,
            _ => self.as_f64() == other.as_f64(),
        }
    }
}

macro_rules! from_unsigned {
    ($($t:ty)*) => {$(
        impl From<$t> for Number {
            fn from(n: $t) -> Self {
                Number(Repr::PosInt(n as u64))
            }
        }
    )*};
}

macro_rules! from_signed {
    ($($t:ty)*) => {$(
        impl From<$t> for Number {
            fn from(n: $t) -> Self {
                if n < 0 {
                    Number(Repr::NegInt(n as i64))
                } else {
                    Number(Repr::PosInt(n as u64))
                }
            }
        }
    )*};
}

from_unsigned!(u8 u16 u32 u64 usize);
from_signed!(i8 i16 i32 i64 isize);

impl From<f64> for Number {
    fn from(n: f64) -> Self {
        Number::from_f64(n)
    }
}

impl From<f32> for Number {
    fn from(n: f32) -> Self {
        Number::from_f64(f64::from(n))
    }
}

impl fmt::Display for Number {
    /// Integers as written; floats always show they're floats (`1.0`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Repr::PosInt(n) => fmt::Display::fmt(&n, f),
            Repr::NegInt(n) => fmt::Display::fmt(&n, f),
            Repr::Float(n) => fmt::Debug::fmt(&n, f),
        }
    }
}

impl fmt::Debug for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Number({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_stay_exact_and_floats_stay_floats() {
        assert_eq!(Number::from(u64::MAX).as_u64(), Some(u64::MAX));
        assert_eq!(Number::from(-5).as_i64(), Some(-5));
        assert_eq!(Number::from(-5).as_u64(), None);
        assert!(Number::from(1.0).is_f64());
        assert_eq!(Number::from(1.0).as_i64(), None);
        assert_eq!(Number::from(1.0).to_string(), "1.0");
        assert_eq!(Number::from(7u8).to_string(), "7");
    }

    #[test]
    fn equality_is_numeric() {
        assert_eq!(Number::from(1), Number::from(1.0));
        assert_eq!(Number::from(-2), Number::from(-2.0));
        assert_ne!(Number::from(1), Number::from(-1));
        assert_ne!(Number::from_f64(f64::NAN), Number::from_f64(f64::NAN));
        assert!(!Number::from_f64(f64::INFINITY).is_finite());
    }
}
