//! Serde helpers for `x-*` extension fields, matching the Overlay
//! specification's
//! [§3.7 Specification Extensions](https://spec.openapis.org/overlay/v1.0.0.html#specification-extensions).
//!
//! Mirrors the analogous helper in the sibling `roas` crate; kept
//! local so this crate is dependency-free against `roas` itself.
//!
//! Only entries whose key starts with `x-` are deserialized / serialized.
//! Any non-`x-` key encountered during deserialization is silently
//! dropped (serde's `#[serde(flatten)]` ensures the typed fields claim
//! their keys first, so this only sees genuinely unknown ones).
//!
//! ### Usage
//!
//! ```rust
//! use std::collections::BTreeMap;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
//! pub struct WithExtensions {
//!     pub foo: String,
//!     #[serde(flatten)]
//!     #[serde(with = "roas_overlay::common::extensions")]
//!     #[serde(skip_serializing_if = "Option::is_none")]
//!     pub extensions: Option<BTreeMap<String, serde_json::Value>>,
//! }
//! ```

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub fn deserialize<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, serde_json::Value>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ExtensionsVisitor;
    impl<'de> Visitor<'de> for ExtensionsVisitor {
        type Value = BTreeMap<String, serde_json::Value>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("extensions: Option<BTreeMap<String, serde_json::Value>>")
        }

        fn visit_map<V>(self, mut map: V) -> Result<BTreeMap<String, serde_json::Value>, V::Error>
        where
            V: MapAccess<'de>,
        {
            let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if key.starts_with("x-") {
                    if ext.contains_key(key.as_str()) {
                        return Err(Error::custom(format_args!("duplicate field `{key}`")));
                    }
                    let value: serde_json::Value = map.next_value()?;
                    ext.insert(key, value);
                }
            }
            Ok(ext)
        }
    }

    let map = deserializer.deserialize_map(ExtensionsVisitor)?;
    Ok(if map.is_empty() { None } else { Some(map) })
}

pub fn serialize<S>(
    ext: &Option<BTreeMap<String, serde_json::Value>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if let Some(ext) = ext {
        let mut map = serializer.serialize_map(Some(ext.len()))?;
        for (k, v) in ext {
            if k.starts_with("x-") {
                map.serialize_entry(k, v)?;
            }
        }
        map.end()
    } else {
        serializer.serialize_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    struct TestExtensions {
        pub foo: String,
        #[serde(flatten)]
        #[serde(with = "super")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub extensions: Option<BTreeMap<String, serde_json::Value>>,
    }

    #[test]
    fn deserialize_filters_non_x_dash_keys() {
        let parsed: TestExtensions = serde_json::from_value(serde_json::json!({
            "foo": "bar",
            "skipped": 1,
            "x-added": 2,
        }))
        .unwrap();
        let mut expected_ext = BTreeMap::new();
        expected_ext.insert("x-added".to_owned(), serde_json::json!(2));
        assert_eq!(
            parsed,
            TestExtensions {
                foo: "bar".into(),
                extensions: Some(expected_ext),
            },
        );
    }

    #[test]
    fn deserialize_no_extensions_returns_none() {
        let parsed: TestExtensions =
            serde_json::from_value(serde_json::json!({ "foo": "bar" })).unwrap();
        assert_eq!(
            parsed,
            TestExtensions {
                foo: "bar".into(),
                extensions: None,
            },
        );
    }

    #[test]
    fn deserialize_rejects_duplicate_extension_keys() {
        let err =
            serde_json::from_str::<TestExtensions>(r#"{"foo":"bar","x-a":1,"x-a":2}"#).unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `x-a`"),
            "got: {err}"
        );
    }

    #[test]
    fn serialize_only_x_dash_keys_are_emitted() {
        let mut ext = BTreeMap::new();
        ext.insert("x-added".to_owned(), serde_json::json!(1));
        ext.insert("skipped".to_owned(), serde_json::json!(2));
        let v = serde_json::to_value(TestExtensions {
            foo: "bar".into(),
            extensions: Some(ext),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({ "foo": "bar", "x-added": 1 }));
    }

    #[test]
    fn serialize_none_emits_null() {
        let none: Option<BTreeMap<String, serde_json::Value>> = None;
        let out = super::serialize(&none, serde_json::value::Serializer).unwrap();
        assert_eq!(out, serde_json::Value::Null);
    }

    #[test]
    fn visitor_expecting_invoked_on_type_mismatch() {
        let mut de = serde_json::Deserializer::from_str(r#""not-a-map""#);
        let err = super::deserialize(&mut de).unwrap_err();
        assert!(err.to_string().contains("map") || err.to_string().contains("expected"));
    }
}
