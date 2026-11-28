//! Serde helpers for `x-*` extension fields, matching the Overlay
//! specification's
//! [§3.7 Specification Extensions](https://spec.openapis.org/overlay/v1.0.0.html#specification-extensions).
//!
//! Mirrors the analogous helper in the sibling `roas` crate; kept
//! local so this crate is dependency-free against `roas` itself.
//!
//! Keeps every key the typed fields do not claim, not only the `x-`
//! extensions, so a document round-trips without loss. A key that is
//! not an `x-` extension is one the object does not define, which the
//! specification does not allow; validation reports it (see
//! [`crate::validation::ValidationOptions::IgnoreUnknownFields`]).
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

/// Whether `key` is a Specification Extension (`x-` prefix).
pub(crate) fn is_extension(key: &str) -> bool {
    key.starts_with("x-")
}

/// The keys `extensions` holds that are not `x-` extensions: keys the object
/// does not define.
pub(crate) fn unknown_fields(
    extensions: &Option<BTreeMap<String, serde_json::Value>>,
) -> impl Iterator<Item = &str> {
    extensions
        .iter()
        .flat_map(BTreeMap::keys)
        .map(String::as_str)
        .filter(|key| !is_extension(key))
}

/// Retain only `x-` Specification Extensions in `extensions`, setting it to
/// `None` if no `x-` keys remain.
#[cfg(any(
    all(feature = "v1_0", feature = "v1_1"),
    all(feature = "v1_1", feature = "v1_2"),
))]
pub(crate) fn retain_extensions_only(extensions: &mut Option<BTreeMap<String, serde_json::Value>>) {
    if let Some(map) = extensions {
        map.retain(|k, _| is_extension(k));
        if map.is_empty() {
            *extensions = None;
        }
    }
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
    let mut map = serializer.serialize_map(Some(ext.len()))?;
    for (k, v) in ext {
        map.serialize_entry(k, v)?;
    }
    map.end()
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
    fn deserialize_keeps_x_dash_and_unclaimed_keys() {
        let parsed: TestExtensions = serde_json::from_value(serde_json::json!({
            "foo": "bar",
            "unclaimed": 1,
            "x-added": 2,
        }))
        .unwrap();
        let expected_ext = BTreeMap::from([
            ("unclaimed".to_owned(), serde_json::json!(1)),
            ("x-added".to_owned(), serde_json::json!(2)),
        ]);
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
    fn deserialize_rejects_duplicate_keys() {
        let err =
            serde_json::from_str::<TestExtensions>(r#"{"foo":"bar","x-a":1,"x-a":2}"#).unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `x-a`"),
            "got: {err}"
        );

        let err =
            serde_json::from_str::<TestExtensions>(r#"{"foo":"bar","unclaimed":1,"unclaimed":2}"#)
                .unwrap_err();
        assert!(
            err.to_string().contains("duplicate field `unclaimed`"),
            "got: {err}"
        );
    }

    #[test]
    fn serialize_emits_x_dash_and_unclaimed_keys() {
        let ext = BTreeMap::from([
            ("x-added".to_owned(), serde_json::json!(1)),
            ("unclaimed".to_owned(), serde_json::json!(2)),
        ]);
        let v = serde_json::to_value(TestExtensions {
            foo: "bar".into(),
            extensions: Some(ext),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "foo": "bar", "unclaimed": 2, "x-added": 1 }),
        );
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

    #[test]
    fn deserialize_direct_streaming_keeps_all_keys() {
        let mut de = serde_json::Deserializer::from_str(
            r#"{"unclaimed":1,"x-a":2,"nested":{"k":3},"x-b":"ok"}"#,
        );
        let parsed = super::deserialize(&mut de).unwrap().unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed["unclaimed"], serde_json::json!(1));
        assert_eq!(parsed["x-a"], serde_json::json!(2));
        assert_eq!(parsed["nested"], serde_json::json!({"k": 3}));
        assert_eq!(parsed["x-b"], serde_json::json!("ok"));
    }

    #[test]
    fn unknown_fields_are_the_keys_without_the_x_prefix() {
        let ext = Some(BTreeMap::from([
            ("unclaimed".to_owned(), serde_json::json!(1)),
            ("x-added".to_owned(), serde_json::json!(2)),
        ]));
        let unknown: Vec<&str> = unknown_fields(&ext).collect();
        assert_eq!(unknown, ["unclaimed"]);
        assert_eq!(unknown_fields(&None).count(), 0);
    }

    #[cfg(any(
        all(feature = "v1_0", feature = "v1_1"),
        all(feature = "v1_1", feature = "v1_2"),
    ))]
    #[test]
    fn retain_extensions_only_drops_unclaimed_keys_and_clears_empty_map() {
        let mut ext = Some(BTreeMap::from([
            ("unclaimed".to_owned(), serde_json::json!(1)),
            ("x-added".to_owned(), serde_json::json!(2)),
        ]));
        retain_extensions_only(&mut ext);
        assert_eq!(
            ext,
            Some(BTreeMap::from([(
                "x-added".to_owned(),
                serde_json::json!(2)
            )])),
        );

        let mut only_unknown = Some(BTreeMap::from([(
            "unclaimed".to_owned(),
            serde_json::json!(1),
        )]));
        retain_extensions_only(&mut only_unknown);
        assert_eq!(only_unknown, None);

        let mut none: Option<BTreeMap<String, serde_json::Value>> = None;
        retain_extensions_only(&mut none);
        assert_eq!(none, None);
    }

    #[test]
    fn serialize_reports_exact_length_hint() {
        macro_rules! unexpected_ser {
            ($(fn $name:ident $(<$t:ident>)? ($($arg:ty),*) -> $ret:ty;)*) => {
                $(fn $name $(<$t: ?Sized + Serialize>)? (self, $(_: $arg),*) -> $ret {
                    Err(serde::ser::Error::custom("unexpected"))
                })*
            };
        }
        struct CaptureMapLen {
            hint: Option<usize>,
            entries: usize,
        }
        impl serde::ser::SerializeMap for &mut CaptureMapLen {
            type Ok = ();
            type Error = serde_json::Error;
            fn serialize_key<T: ?Sized + Serialize>(&mut self, _: &T) -> Result<(), Self::Error> {
                Ok(())
            }
            fn serialize_value<T: ?Sized + Serialize>(&mut self, _: &T) -> Result<(), Self::Error> {
                self.entries += 1;
                Ok(())
            }
            fn end(self) -> Result<Self::Ok, Self::Error> {
                Ok(())
            }
        }
        impl Serializer for &mut CaptureMapLen {
            type Ok = ();
            type Error = serde_json::Error;
            type SerializeSeq = serde::ser::Impossible<(), serde_json::Error>;
            type SerializeTuple = serde::ser::Impossible<(), serde_json::Error>;
            type SerializeTupleStruct = serde::ser::Impossible<(), serde_json::Error>;
            type SerializeTupleVariant = serde::ser::Impossible<(), serde_json::Error>;
            type SerializeMap = Self;
            type SerializeStruct = serde::ser::Impossible<(), serde_json::Error>;
            type SerializeStructVariant = serde::ser::Impossible<(), serde_json::Error>;
            fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
                self.hint = len;
                Ok(self)
            }
            unexpected_ser! {
                fn serialize_bool(bool) -> Result<(), Self::Error>;
                fn serialize_i8(i8) -> Result<(), Self::Error>;
                fn serialize_i16(i16) -> Result<(), Self::Error>;
                fn serialize_i32(i32) -> Result<(), Self::Error>;
                fn serialize_i64(i64) -> Result<(), Self::Error>;
                fn serialize_u8(u8) -> Result<(), Self::Error>;
                fn serialize_u16(u16) -> Result<(), Self::Error>;
                fn serialize_u32(u32) -> Result<(), Self::Error>;
                fn serialize_u64(u64) -> Result<(), Self::Error>;
                fn serialize_f32(f32) -> Result<(), Self::Error>;
                fn serialize_f64(f64) -> Result<(), Self::Error>;
                fn serialize_char(char) -> Result<(), Self::Error>;
                fn serialize_str(&str) -> Result<(), Self::Error>;
                fn serialize_bytes(&[u8]) -> Result<(), Self::Error>;
                fn serialize_none() -> Result<(), Self::Error>;
                fn serialize_some<T>(&T) -> Result<(), Self::Error>;
                fn serialize_unit() -> Result<(), Self::Error>;
                fn serialize_unit_struct(&'static str) -> Result<(), Self::Error>;
                fn serialize_unit_variant(&'static str, u32, &'static str) -> Result<(), Self::Error>;
                fn serialize_newtype_struct<T>(&'static str, &T) -> Result<(), Self::Error>;
                fn serialize_newtype_variant<T>(&'static str, u32, &'static str, &T) -> Result<(), Self::Error>;
                fn serialize_seq(Option<usize>) -> Result<Self::SerializeSeq, Self::Error>;
                fn serialize_tuple(usize) -> Result<Self::SerializeTuple, Self::Error>;
                fn serialize_tuple_struct(&'static str, usize) -> Result<Self::SerializeTupleStruct, Self::Error>;
                fn serialize_tuple_variant(&'static str, u32, &'static str, usize) -> Result<Self::SerializeTupleVariant, Self::Error>;
                fn serialize_struct(&'static str, usize) -> Result<Self::SerializeStruct, Self::Error>;
                fn serialize_struct_variant(&'static str, u32, &'static str, usize) -> Result<Self::SerializeStructVariant, Self::Error>;
            }
        }

        let mut ext = BTreeMap::new();
        ext.insert("x-kept".to_owned(), serde_json::json!(1));
        ext.insert("unclaimed".to_owned(), serde_json::json!(2));
        let mut capture = CaptureMapLen {
            hint: None,
            entries: 0,
        };
        super::serialize(&Some(ext), &mut capture).unwrap();
        assert_eq!(capture.hint, Some(2));
        assert_eq!(capture.entries, 2);
        assert!(super::serialize(&None, &mut capture).is_err());
    }
}
