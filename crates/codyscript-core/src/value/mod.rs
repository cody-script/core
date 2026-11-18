//! The data model: [`Value`], [`Number`] and [`Map`].
//!
//! Programs, variables and results are all `Value`s. They implement serde's
//! `Serialize` and `Deserialize`, so any serde data format (JSON through
//! `codyscript-json`, or YAML, CBOR, MessagePack…) can read and write them;
//! this crate itself knows no format.

mod macros;
mod map;
mod number;
mod serde_impls;

use std::borrow::Cow;
use std::fmt;
use std::ops::{Index, IndexMut};
use std::sync::Arc;

pub use map::{IntoIter, Iter, Keys, Map, Values};
pub use number::Number;

/// Any value: everything a program is made of, reads, computes or returns.
///
/// Build one with [`value!`](crate::value!), with `From` (`Value::from(1)`,
/// `"text".into()`), or by deserializing from a data format.
#[derive(Clone, Default, PartialEq)]
pub enum Value {
    /// `null`: nothing.
    #[default]
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number.
    Number(Number),
    /// Text.
    String(String),
    /// A list.
    Array(Vec<Value>),
    /// An object, keys in order.
    Object(Map),
}

static NULL: Value = Value::Null;

impl Value {
    /// Whether it's `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Whether it's `true` or `false`.
    pub fn is_boolean(&self) -> bool {
        matches!(self, Value::Bool(_))
    }

    /// Whether it's a number.
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }

    /// Whether it's text.
    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    /// Whether it's a list.
    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    /// Whether it's an object.
    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    /// The boolean, if it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The number, if it is one.
    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(n) => Some(n),
            _ => None,
        }
    }

    /// The number as an `f64`, if it is one.
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().map(Number::as_f64)
    }

    /// The number, if it's an integer that fits an `i64`.
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }

    /// The number, if it's an integer that fits a `u64`.
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }

    /// The text, if it is text.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The list, if it is one.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The list, to change, if it is one.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The object, if it is one.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The object, to change, if it is one.
    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The object, taken, if it is one.
    pub fn into_object(self) -> Option<Map> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// An object's value for `key` (`None` if it's missing or this isn't an
    /// object).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|map| map.get(key))
    }

    /// Takes the value, leaving `null`.
    pub fn take(&mut self) -> Value {
        std::mem::take(self)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("Null"),
            Value::Bool(b) => write!(f, "Bool({b})"),
            Value::Number(n) => fmt::Debug::fmt(n, f),
            Value::String(s) => write!(f, "String({s:?})"),
            Value::Array(items) => {
                f.write_str("Array ")?;
                f.debug_list().entries(items).finish()
            }
            Value::Object(map) => {
                f.write_str("Object ")?;
                fmt::Debug::fmt(map, f)
            }
        }
    }
}

impl Index<&str> for Value {
    type Output = Value;

    /// An object's value for `key`; `null` if it's missing or this isn't an
    /// object.
    fn index(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&NULL)
    }
}

impl Index<usize> for Value {
    type Output = Value;

    /// A list's item `i`; `null` if it's missing or this isn't a list.
    fn index(&self, i: usize) -> &Value {
        self.as_array()
            .and_then(|items| items.get(i))
            .unwrap_or(&NULL)
    }
}

impl IndexMut<&str> for Value {
    /// An object's value for `key`, inserted as `null` if missing; `null`
    /// becomes an empty object first. Panics on anything else.
    fn index_mut(&mut self, key: &str) -> &mut Value {
        if self.is_null() {
            *self = Value::Object(Map::new());
        }
        match self {
            Value::Object(map) => map.get_or_insert_null(key),
            other => panic!("cannot index {other:?} with key {key:?}"),
        }
    }
}

impl IndexMut<usize> for Value {
    /// A list's item `i`. Panics if this isn't a list or `i` is out of range.
    fn index_mut(&mut self, i: usize) -> &mut Value {
        match self {
            Value::Array(items) => &mut items[i],
            other => panic!("cannot index {other:?} with {i}"),
        }
    }
}

// ── Conversions ─────────────────────────────────────────────────────────────

macro_rules! from_number {
    ($($t:ty)*) => {$(
        impl From<$t> for Value {
            fn from(n: $t) -> Self {
                Value::Number(Number::from(n))
            }
        }
    )*};
}

from_number!(u8 u16 u32 u64 usize i8 i16 i32 i64 isize f32 f64);

impl From<Number> for Value {
    fn from(n: Number) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::String(s.clone())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<Cow<'_, str>> for Value {
    fn from(s: Cow<'_, str>) -> Self {
        Value::String(s.into_owned())
    }
}

impl From<char> for Value {
    fn from(c: char) -> Self {
        Value::String(c.to_string())
    }
}

impl From<Map> for Value {
    fn from(map: Map) -> Self {
        Value::Object(map)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Self {
        Value::Array(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Clone + Into<Value>> From<&[T]> for Value {
    fn from(items: &[T]) -> Self {
        Value::Array(items.iter().cloned().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    /// `None` is `null`.
    fn from(option: Option<T>) -> Self {
        option.map_or(Value::Null, Into::into)
    }
}

impl From<()> for Value {
    fn from((): ()) -> Self {
        Value::Null
    }
}

impl<T: Into<Value>> FromIterator<T> for Value {
    /// A list.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Value::Array(iter.into_iter().map(Into::into).collect())
    }
}

impl<K: Into<Arc<str>>, V: Into<Value>> FromIterator<(K, V)> for Value {
    /// An object.
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Value::Object(iter.into_iter().collect())
    }
}

// ── Comparison with plain Rust values ───────────────────────────────────────

impl PartialEq<str> for Value {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for Value {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for Value {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<bool> for Value {
    fn eq(&self, other: &bool) -> bool {
        self.as_bool() == Some(*other)
    }
}

macro_rules! eq_number {
    ($($t:ty)*) => {$(
        impl PartialEq<$t> for Value {
            fn eq(&self, other: &$t) -> bool {
                self.as_number() == Some(&Number::from(*other))
            }
        }
    )*};
}

eq_number!(i32 i64 u32 u64 usize f64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_reads_null_for_anything_missing() {
        let v = crate::value!({ "a": [1, { "b": true }] });
        assert_eq!(v["a"][1]["b"], true);
        assert!(v["nope"].is_null());
        assert!(v["a"][9].is_null());
        assert!(v["a"]["not an object"].is_null());
    }

    #[test]
    fn index_mut_inserts_and_turns_null_into_an_object() {
        let mut v = Value::Null;
        v["a"] = Value::from(1);
        v["b"]["c"] = "x".into();
        assert_eq!(v, crate::value!({ "a": 1, "b": { "c": "x" } }));
        v["a"] = Value::Array(vec![Value::Null]);
        v["a"][0] = Value::from(2);
        assert_eq!(v["a"], crate::value!([2]));
    }

    #[test]
    fn conversions_and_comparisons() {
        assert_eq!(Value::from(Some(3)), 3);
        assert_eq!(Value::from(None::<i32>), Value::Null);
        assert_eq!(Value::from(vec!["a", "b"]), crate::value!(["a", "b"]));
        assert_eq!(Value::from(2.5), 2.5);
        assert_eq!(Value::from(1), 1.0);
        assert_eq!(Value::from("x"), "x");
        let object: Value = [("k", 1)].into_iter().collect();
        assert_eq!(object, crate::value!({ "k": 1 }));
        assert_eq!(object.clone().into_object().map(|m| m.len()), Some(1));
        let list: Value = (1..=2).collect();
        assert_eq!(list, crate::value!([1, 2]));
    }

    #[test]
    fn debug_is_readable() {
        let v = crate::value!({ "a": [null, true, 1, 1.5, "s"] });
        assert_eq!(
            format!("{v:?}"),
            r#"Object {"a": Array [Null, Bool(true), Number(1), Number(1.5), String("s")]}"#
        );
    }
}
