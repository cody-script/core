//! `Serialize` and `Deserialize` for the data model: the one place the core
//! meets serde, and so every serde data format.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error as _, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, Serializer};

use super::{Map, Number, Value};

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Number(n) => n.serialize(serializer),
            Value::String(s) => serializer.serialize_str(s),
            Value::Array(items) => serializer.collect_seq(items),
            Value::Object(map) => map.serialize(serializer),
        }
    }
}

impl Serialize for Number {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if let Some(n) = self.as_u64() {
            serializer.serialize_u64(n)
        } else if let Some(n) = self.as_i64() {
            serializer.serialize_i64(n)
        } else {
            serializer.serialize_f64(self.as_f64())
        }
    }
}

impl Serialize for Map {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (key, value) in self {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// Builds a [`Value`] from whatever a data format holds.
struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("null, a boolean, a number, a string, a list or an object with string keys")
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Value, D::Error> {
        Value::deserialize(deserializer)
    }

    fn visit_bool<E>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> Result<Value, E> {
        Ok(Value::from(n))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Value, E> {
        Ok(Value::from(n))
    }

    fn visit_i128<E>(self, n: i128) -> Result<Value, E> {
        Ok(match (i64::try_from(n), u64::try_from(n)) {
            (Ok(n), _) => Value::from(n),
            (_, Ok(n)) => Value::from(n),
            _ => Value::from(n as f64),
        })
    }

    fn visit_u128<E>(self, n: u128) -> Result<Value, E> {
        Ok(u64::try_from(n).map_or(Value::from(n as f64), Value::from))
    }

    fn visit_f64<E>(self, n: f64) -> Result<Value, E> {
        Ok(Value::from(n))
    }

    fn visit_str<E>(self, s: &str) -> Result<Value, E> {
        Ok(Value::from(s))
    }

    fn visit_string<E>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, access: A) -> Result<Value, A::Error> {
        MapVisitor.visit_map(access).map(Value::Object)
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// Builds a [`Map`], entries in the order the format gives them. A key may
/// appear only once: a repeated one is an error, in any format, rather than
/// a silent choice between its values.
struct MapVisitor;

impl<'de> Visitor<'de> for MapVisitor {
    type Value = Map;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object with string keys")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Map, A::Error> {
        let mut map = Map::with_capacity(access.size_hint().unwrap_or(0).min(4096));
        while let Some((key, value)) = access.next_entry::<String, Value>()? {
            if let Err((key, _)) = map.insert_new(key, value) {
                return Err(A::Error::custom(format_args!("repeated key \"{key}\"")));
            }
        }
        Ok(map)
    }
}

impl<'de> Deserialize<'de> for Map {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MapVisitor)
    }
}

/// Builds a [`Number`] from any numeric type a format holds.
struct NumberVisitor;

impl Visitor<'_> for NumberVisitor {
    type Value = Number;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a number")
    }

    fn visit_i64<E>(self, n: i64) -> Result<Number, E> {
        Ok(Number::from(n))
    }

    fn visit_u64<E>(self, n: u64) -> Result<Number, E> {
        Ok(Number::from(n))
    }

    fn visit_f64<E>(self, n: f64) -> Result<Number, E> {
        Ok(Number::from(n))
    }
}

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NumberVisitor)
    }
}

#[cfg(test)]
mod tests {
    use serde::de::IntoDeserializer;
    use serde::de::value::{I128Deserializer, MapDeserializer, U128Deserializer};
    use serde_test::{Token, assert_de_tokens, assert_tokens};

    use super::*;
    use crate::value;

    #[test]
    fn values_round_trip_through_serde_tokens() {
        let v = value!({ "b": [null, true, -1, 2, 1.5], "a": "s" });
        assert_tokens(
            &v,
            &[
                Token::Map { len: Some(2) },
                Token::Str("b"),
                Token::Seq { len: Some(5) },
                Token::Unit,
                Token::Bool(true),
                Token::I64(-1),
                Token::U64(2),
                Token::F64(1.5),
                Token::SeqEnd,
                Token::Str("a"),
                Token::Str("s"),
                Token::MapEnd,
            ],
        );
    }

    #[test]
    fn objects_keep_the_order_the_format_gives() {
        let tokens = [
            Token::Map { len: None },
            Token::Str("z"),
            Token::U8(1),
            Token::Str("a"),
            Token::I8(2),
            Token::MapEnd,
        ];
        assert_de_tokens(&value!({ "z": 1, "a": 2 }), &tokens);
        // Equality ignores order, so check it directly.
        let entries = vec![("z", 1u8), ("a", 2u8)].into_iter();
        let deserializer = MapDeserializer::<_, serde::de::value::Error>::new(entries);
        let v = Value::deserialize(deserializer).unwrap();
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["z", "a"]);
    }

    #[test]
    fn options_units_and_wide_integers_read_naturally() {
        assert_de_tokens(&Value::Null, &[Token::None]);
        assert_de_tokens(&value!(3), &[Token::Some, Token::U32(3)]);
        let wide = |n: i128| {
            let deserializer: I128Deserializer<serde::de::value::Error> = n.into_deserializer();
            Value::deserialize(deserializer).unwrap()
        };
        assert_eq!(wide(-3).as_i64(), Some(-3));
        assert_eq!(wide(i128::from(u64::MAX)).as_u64(), Some(u64::MAX));
        assert_eq!(wide(-(1 << 70)).as_f64(), Some(-(2f64.powi(70))));
        let deserializer: U128Deserializer<serde::de::value::Error> =
            (1u128 << 70).into_deserializer();
        assert_eq!(
            Value::deserialize(deserializer).unwrap().as_f64(),
            Some(2f64.powi(70))
        );
        assert_de_tokens(&value!("s"), &[Token::String("s")]);
        assert_de_tokens(&Number::from(5), &[Token::U16(5)]);
    }
}
