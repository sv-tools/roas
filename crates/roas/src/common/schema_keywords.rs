//! A schema's catch-all for the keywords its typed fields do not claim.
//!
//! JSON Schema allows keywords a model does not know, so a schema keeps every unclaimed key, not
//! only `x-` extensions as [`crate::common::extensions`] does for the other objects: the document
//! round-trips without loss, and a conversion carries the keywords forward.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serialize, Serializer};

/// Reads every key the typed fields left, refusing a duplicate.
pub(crate) fn deserialize<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, serde_json::Value>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct KeywordsVisitor;
    impl<'de> Visitor<'de> for KeywordsVisitor {
        type Value = BTreeMap<String, serde_json::Value>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("extensions: Option<BTreeMap<String, serde_json::Value>>")
        }

        fn visit_map<V>(self, mut map: V) -> Result<BTreeMap<String, serde_json::Value>, V::Error>
        where
            V: MapAccess<'de>,
        {
            let mut keywords: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if keywords.contains_key(key.as_str()) {
                    return Err(Error::custom(format_args!("duplicate field `{key}`")));
                }
                let value: serde_json::Value = map.next_value()?;
                keywords.insert(key, value);
            }
            Ok(keywords)
        }
    }

    let keywords = deserializer.deserialize_map(KeywordsVisitor)?;
    Ok(if keywords.is_empty() {
        None
    } else {
        Some(keywords)
    })
}

/// Writes every kept key back.
pub(crate) fn serialize<S>(
    keywords: &Option<BTreeMap<String, serde_json::Value>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(keywords) = keywords {
        let mut map = serializer.serialize_map(Some(keywords.len()))?;
        for (key, value) in keywords {
            map.serialize_entry(key, value)?;
        }
        map.end()
    } else {
        None::<BTreeMap<String, serde_json::Value>>.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{deserialize, serialize};

    #[test]
    fn every_unclaimed_key_is_kept_and_written_back() {
        let value = serde_json::json!({ "const": 1, "prefixItems": [], "x-a": true });

        let keywords = deserialize(value.clone()).unwrap();
        let written = serialize(&keywords, serde_json::value::Serializer).unwrap();

        assert_eq!(keywords.as_ref().map(BTreeMap::len), Some(3));
        assert_eq!(written, value);
    }

    #[test]
    fn no_keys_are_none_and_none_writes_nothing() {
        let keywords = deserialize(serde_json::json!({})).unwrap();
        let written = serialize(&None, serde_json::value::Serializer).unwrap();

        assert_eq!(keywords, None);
        assert_eq!(written, serde_json::Value::Null);
    }

    #[test]
    fn a_value_that_is_not_a_map_is_an_error() {
        let mut de = serde_json::Deserializer::from_str(r#""not-a-map""#);

        let message = deserialize(&mut de).unwrap_err().to_string();

        assert!(message.contains("expected extensions"), "got: {message}");
    }

    #[test]
    fn a_repeated_key_is_an_error() {
        let mut de = serde_json::Deserializer::from_slice(br#"{"const": 1, "const": 2}"#);

        let message = deserialize(&mut de).unwrap_err().to_string();

        assert!(
            message.contains("duplicate field `const`"),
            "got: {message}"
        );
    }
}
