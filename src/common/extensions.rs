//! While the Swagger Specification tries to accommodate most use cases,
//! additional data can be added to extend the specification at certain points.
//!
//! The extensions properties are always prefixed by "x-" and can have any valid JSON format value.
//!
//! The extensions may or may not be supported by the available tooling,
//! but those may be extended as well to add requested support (if tools are internal or open-sourced).
//!
//! See the list of some [Vendor Extensions](https://github.com/swagger-api/swagger-codegen/wiki/Vendor-Extensions) for further details.
//!
//! The module provides a `serde` helper to deserialize and serialize extensions.
//! Only the entries with key starts with `x-` will be deserialized and/or serialized.
//!
//! Example:
//!
//! ```rust
//! use std::collections::BTreeMap;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
//! pub struct TestExtensions {
//!     pub foo: String,
//!     #[serde(flatten)]
//!     #[serde(with = "roas::common::extensions")]
//!     #[serde(skip_serializing_if = "Option::is_none")]
//!     pub extensions: Option<BTreeMap<String, serde_json::Value>>,
//! }
//! ```
use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serialize, Serializer};

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
                        return Err(Error::custom(format_args!("duplicate field `{}`", key)));
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
        for (k, v) in ext.clone() {
            if k.starts_with("x-") {
                map.serialize_entry(&k, &v)?;
            }
        }
        map.end()
    } else {
        None::<BTreeMap<String, serde_json::Value>>.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    pub struct TestExtensions {
        pub foo: String,
        #[serde(flatten)]
        #[serde(with = "super")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub extensions: Option<BTreeMap<String, serde_json::Value>>,
    }

    #[test]
    fn test_extensions_deserialize() {
        assert_eq!(
            serde_json::from_value::<TestExtensions>(serde_json::json!({
                "foo": "bar"
            }))
            .unwrap(),
            TestExtensions {
                foo: String::from("bar"),
                ..Default::default()
            },
            "no extensions",
        );
        assert_eq!(
            serde_json::from_value::<TestExtensions>(serde_json::json!({
                "foo": "bar",
                "skipped":1,
                "x-added":2,
            }))
            .unwrap(),
            TestExtensions {
                foo: String::from("bar"),
                extensions: Some({
                    let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                    ext.insert("x-added".to_owned(), 2.into());
                    ext
                }),
            },
            "one ext with x- prefix another without",
        );
        assert_eq!(
            serde_json::from_str::<TestExtensions>(r#"{"foo":"bar","x-added":1,"x-added":2}"#)
                .unwrap_err()
                .to_string(),
            "duplicate field `x-added` at line 1 column 37",
            "one ext with x- prefix another without",
        );
    }

    #[test]
    fn test_extensions_serialize() {
        assert_eq!(
            serde_json::to_value(TestExtensions {
                foo: String::from("bar"),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "foo": "bar"
            }),
            "no extensions",
        );
        assert_eq!(
            serde_json::to_value(TestExtensions {
                foo: String::from("bar"),
                extensions: Some({
                    let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                    ext.insert("x-added".to_owned(), 1.into());
                    ext.insert("skipped".to_owned(), 2.into());
                    ext
                }),
            })
            .unwrap(),
            serde_json::json!({
                "foo": "bar",
                "x-added": 1
            }),
            "one ext with x- prefix another without",
        );
    }
}
