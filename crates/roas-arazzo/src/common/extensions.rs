//! Serde helpers for `x-*` extension fields, matching the Arazzo
//! specification's
//! [Specification Extensions](https://spec.openapis.org/arazzo/v1.0.1.html#specification-extensions).
//!
//! Mirrors the analogous helper in the sibling `roas` / `roas-overlay`
//! crates; kept local so this crate is dependency-free against them.
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
//!     #[serde(with = "roas_arazzo::common::extensions")]
//!     #[serde(skip_serializing_if = "Option::is_none")]
//!     pub extensions: Option<BTreeMap<String, serde_json::Value>>,
//! }
//! ```

use serde::de::{Error, IgnoredAny, MapAccess, Visitor};
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
                } else {
                    let _ = map.next_value::<IgnoredAny>()?;
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
        let len = ext.keys().filter(|k| k.starts_with("x-")).count();
        let mut map = serializer.serialize_map(Some(len))?;
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

    #[test]
    fn deserialize_direct_streaming_skips_non_x_dash_keys() {
        let mut de = serde_json::Deserializer::from_str(
            r#"{"ignored":1,"x-a":2,"nested":{"k":3},"x-b":"ok"}"#,
        );
        let parsed = super::deserialize(&mut de).unwrap().unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed["x-a"], serde_json::json!(2));
        assert_eq!(parsed["x-b"], serde_json::json!("ok"));
    }

    #[test]
    fn serialize_reports_filtered_x_dash_length_hint() {
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
        ext.insert("dropped".to_owned(), serde_json::json!(2));
        let mut capture = CaptureMapLen {
            hint: None,
            entries: 0,
        };
        super::serialize(&Some(ext), &mut capture).unwrap();
        assert_eq!(capture.hint, Some(1));
        assert_eq!(capture.entries, 1);
        assert!(super::serialize(&None, &mut capture).is_err());
    }
}
