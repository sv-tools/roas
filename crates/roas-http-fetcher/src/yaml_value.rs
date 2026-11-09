//! A `serde_json::Value` that refuses a mapping whose keys collapse to the
//! same JSON string. The same file is copied into every roas crate that reads
//! YAML (`grep -rln 'struct JsonValue' crates`); keep the copies in step.
//!
//! YAML tells `200` and `"200"` apart; JSON does not. Read straight into a
//! `Value`, the later of two such keys silently replaces the earlier, and with
//! `<<` merge keys the inherited entry arrives last, so it would overwrite the
//! explicit one. Building the `Value` here reports that collision instead,
//! the same way a literal duplicate key is reported. Keys are read as strings,
//! as `serde_json::Value` reads them, so each keeps its spelling (`1.10`,
//! `0x10`); a null or collection key is still an error.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

/// Deserialize YAML into this, then take the [`Value`] with
/// [`JsonValue::into_inner`].
pub(crate) struct JsonValue(Value);

impl JsonValue {
    pub(crate) fn into_inner(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(JsonValueVisitor)
            .map(JsonValue)
    }
}

struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(v.into())
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(v.into())
    }

    /// Non-finite values become `null`, as `serde_json::Value` does.
    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Number::from_f64(v).map_or(Value::Null, Value::Number))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_none<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(JsonValueVisitor)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(JsonValue(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut entries = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let JsonValue(value) = map.next_value()?;
            if entries.contains_key(&key) {
                return Err(de::Error::custom(format!(
                    "mapping key `{key}` appears twice once read as JSON \
                     (for example `200` and \"200\", possibly through a `<<` merge)"
                )));
            }
            entries.insert(key, value);
        }
        // With `serde_json/arbitrary_precision` a number is written out as
        // this one-key map; `serde_json::Value` reads it back as the number,
        // and so does this, by handing the map to `serde_json` itself.
        if entries.len() == 1 && entries.contains_key("$serde_json::private::Number") {
            return serde_json::from_value(Value::Object(entries)).map_err(de::Error::custom);
        }
        Ok(Value::Object(entries))
    }
}
