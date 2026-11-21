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
//! It keeps every key the typed fields do not claim, not only the `x-` extensions, so a document
//! round-trips without loss. A key that is not an `x-` extension is one the object does not
//! define, which the specification does not allow; validation reports it (see
//! [`crate::validation::Options::IgnoreUnknownFields`]).
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
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serializer};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;

thread_local! {
    /// Set while [`with_extensions_only`] runs: serializing then writes only the `x-` keys.
    static EXTENSIONS_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Whether `key` is a Specification Extension (`x-` prefix).
pub(crate) fn is_extension(key: &str) -> bool {
    key.starts_with("x-")
}

/// The keys `extensions` holds that are not `x-` extensions: keys the object does not define.
pub(crate) fn unknown_fields(
    extensions: &Option<BTreeMap<String, serde_json::Value>>,
) -> impl Iterator<Item = &str> {
    extensions
        .iter()
        .flat_map(BTreeMap::keys)
        .map(String::as_str)
        .filter(|key| !is_extension(key))
}

/// Runs `serialize` with the objects that use this module writing only their `x-` extensions,
/// leaving out the keys they do not define.
///
/// The version conversions serialize the source document through it: a key the source version
/// does not define on an object has no meaning to carry to the next version, and the next
/// version may define a field of that name with another shape, which would fail the whole
/// conversion. Validating the source reports such keys.
#[cfg(any(
    all(feature = "v2", feature = "v3_0"),
    all(feature = "v3_0", feature = "v3_1"),
    all(feature = "v3_1", feature = "v3_2"),
))]
pub(crate) fn with_extensions_only<T>(serialize: impl FnOnce() -> T) -> T {
    /// Restores the previous mode, also when `serialize` panics.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            EXTENSIONS_ONLY.set(self.0);
        }
    }

    let _restore = Restore(EXTENSIONS_ONLY.replace(true));
    serialize()
}

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
            formatter.write_str("a map of extensions and unclaimed keys")
        }

        fn visit_map<V>(self, mut map: V) -> Result<BTreeMap<String, serde_json::Value>, V::Error>
        where
            V: MapAccess<'de>,
        {
            let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if ext.contains_key(key.as_str()) {
                    return Err(Error::custom(format_args!("duplicate field `{key}`")));
                }
                let value: serde_json::Value = map.next_value()?;
                ext.insert(key, value);
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
    let Some(ext) = ext else {
        return serializer.serialize_none();
    };
    let extensions_only = EXTENSIONS_ONLY.get();
    let written = |key: &str| !extensions_only || is_extension(key);
    let count = ext.keys().filter(|key| written(key)).count();
    let mut map = serializer.serialize_map(Some(count))?;
    for (k, v) in ext {
        if written(k) {
            map.serialize_entry(k, v)?;
        }
    }
    map.end()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

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
                "unclaimed":1,
                "x-added":2,
            }))
            .unwrap(),
            TestExtensions {
                foo: String::from("bar"),
                extensions: Some({
                    let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                    ext.insert("unclaimed".to_owned(), 1.into());
                    ext.insert("x-added".to_owned(), 2.into());
                    ext
                }),
            },
            "an x- extension and an unclaimed key",
        );
        assert_eq!(
            serde_json::from_str::<TestExtensions>(r#"{"foo":"bar","x-added":1,"x-added":2}"#)
                .unwrap_err()
                .to_string(),
            "duplicate field `x-added` at line 1 column 37",
            "an x- extension and an unclaimed key",
        );
    }

    #[test]
    fn extensions_visitor_expecting_via_type_mismatch() {
        // ExtensionsVisitor::expecting is called by serde when the input type
        // is not a map. We call `super::deserialize` directly with a serde_json
        // deserializer backed by a JSON string, which will try `deserialize_map`
        // and fail with an error that uses `expecting`.
        let mut de = serde_json::Deserializer::from_str(r#""not-a-map""#);
        let err = super::deserialize(&mut de).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("map") || msg.contains("expected") || msg.contains("extensions"),
            "expected a serde type-mismatch error, got: {msg}"
        );
    }

    #[test]
    fn extensions_serialize_none_branch() {
        // Exercises the `else` branch of `super::serialize` (extensions = None)
        // by calling `serialize` directly with a JSON serializer.
        let none: Option<BTreeMap<String, serde_json::Value>> = None;
        let serializer = serde_json::value::Serializer;
        let result = super::serialize(&none, serializer).unwrap();
        assert_eq!(result, serde_json::Value::Null);
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
                    ext.insert("unclaimed".to_owned(), 2.into());
                    ext
                }),
            })
            .unwrap(),
            serde_json::json!({
                "foo": "bar",
                "unclaimed": 2,
                "x-added": 1
            }),
            "an x- extension and an unclaimed key",
        );
    }

    fn with_an_unclaimed_key() -> TestExtensions {
        TestExtensions {
            foo: String::from("bar"),
            extensions: Some(BTreeMap::from([
                ("unclaimed".to_owned(), 1.into()),
                ("x-added".to_owned(), 2.into()),
            ])),
        }
    }

    #[test]
    fn unknown_fields_are_the_keys_without_the_x_prefix() {
        let value = with_an_unclaimed_key();
        let unknown: Vec<&str> = unknown_fields(&value.extensions).collect();
        assert_eq!(unknown, ["unclaimed"]);
        assert_eq!(unknown_fields(&None).count(), 0);
    }

    #[cfg(any(
        all(feature = "v2", feature = "v3_0"),
        all(feature = "v3_0", feature = "v3_1"),
        all(feature = "v3_1", feature = "v3_2"),
    ))]
    #[test]
    fn extensions_only_leaves_out_the_unclaimed_keys() {
        let value = with_an_unclaimed_key();
        let only = with_extensions_only(|| serde_json::to_value(&value).unwrap());
        assert_eq!(only, serde_json::json!({"foo": "bar", "x-added": 2}));
        let all = serde_json::to_value(&value).unwrap();
        assert_eq!(
            all,
            serde_json::json!({"foo": "bar", "unclaimed": 1, "x-added": 2}),
            "the mode ends with the closure",
        );
    }

    #[cfg(any(
        all(feature = "v2", feature = "v3_0"),
        all(feature = "v3_0", feature = "v3_1"),
        all(feature = "v3_1", feature = "v3_2"),
    ))]
    #[test]
    fn extensions_only_ends_when_the_closure_panics() {
        let outcome =
            std::panic::catch_unwind(|| with_extensions_only(|| panic!("in the closure")));
        assert!(outcome.is_err());
        let all = serde_json::to_value(with_an_unclaimed_key()).unwrap();
        assert_eq!(all["unclaimed"], 1);
    }
}
