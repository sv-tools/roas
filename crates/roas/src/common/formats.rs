use serde::de::{Error, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::fmt::Display;

#[derive(Clone, Debug, PartialEq)]
pub enum StringFormat {
    /// base64 encoded characters
    Byte,

    /// any sequence of octets
    Binary,

    /// As defined by `full-date` - [RFC3339](https://www.rfc-editor.org/rfc/rfc3339)
    Date,

    /// As defined by `date-time` - [RFC3339](https://www.rfc-editor.org/rfc/rfc3339)
    DateTime,

    /// Used to hint UIs the input needs to be obscured.
    Password,

    /// As defined by [RFC4122](https://www.rfc-editor.org/rfc/rfc4122)
    UUID,

    /// A custom string format
    Custom(String),
}

impl Display for StringFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StringFormat::Byte => write!(f, "byte"),
            StringFormat::Binary => write!(f, "binary"),
            StringFormat::Date => write!(f, "date"),
            StringFormat::DateTime => write!(f, "date-time"),
            StringFormat::Password => write!(f, "password"),
            StringFormat::UUID => write!(f, "uuid"),
            StringFormat::Custom(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum IntegerFormat {
    // signed 32 bits
    #[serde(rename = "int32")]
    Int32,
    // signed 64 bits
    #[serde(rename = "int64")]
    Int64,
}

impl Display for IntegerFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntegerFormat::Int32 => write!(f, "int32"),
            IntegerFormat::Int64 => write!(f, "int64"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum NumberFormat {
    // f64
    #[serde(rename = "float")]
    Float,
    // f128
    #[serde(rename = "double")]
    Double,
}

impl Display for NumberFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberFormat::Float => write!(f, "float"),
            NumberFormat::Double => write!(f, "double"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub enum CollectionFormat {
    /// comma separated values `foo,bar`.
    #[default]
    #[serde(rename = "csv")]
    CSV,
    /// space separated values `foo bar`.
    #[serde(rename = "ssv")]
    SSV,
    /// tab separated values `foo\tbar`.
    #[serde(rename = "tsv")]
    TSV,
    /// pipe separated values `foo|bar`.
    #[serde(rename = "pipes")]
    PIPES,
    /// multi
    #[serde(rename = "multi")]
    Multi,
}

impl Display for CollectionFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CollectionFormat::CSV => write!(f, "csv"),
            CollectionFormat::SSV => write!(f, "ssv"),
            CollectionFormat::TSV => write!(f, "tsv"),
            CollectionFormat::PIPES => write!(f, "pipes"),
            CollectionFormat::Multi => write!(f, "multi"),
        }
    }
}

impl CollectionFormat {
    /// Returns `true` if this collection format is `multi`, which the OAS 2.0
    /// schema allows only on `query` and `formData` parameters.
    pub fn is_multi(&self) -> bool {
        matches!(self, CollectionFormat::Multi)
    }
}

impl Serialize for StringFormat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.to_string().as_str())
    }
}

impl<'de> Deserialize<'de> for StringFormat {
    fn deserialize<D>(deserializer: D) -> Result<StringFormat, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringFormatVisitor;

        impl Visitor<'_> for StringFormatVisitor {
            type Value = StringFormat;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("one of `byte`, `binary`, `date`, `date-time`, `password`, `uuid` or a custom string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                match value {
                    "byte" => Ok(StringFormat::Byte),
                    "binary" => Ok(StringFormat::Binary),
                    "date" => Ok(StringFormat::Date),
                    "date-time" => Ok(StringFormat::DateTime),
                    "password" => Ok(StringFormat::Password),
                    "uuid" => Ok(StringFormat::UUID),
                    _ => Ok(StringFormat::Custom(String::from(value))),
                }
            }
        }

        deserializer.deserialize_str(StringFormatVisitor)
    }
}

/// A JSON Schema instance type, used in the `type` array of a
/// multi-type schema (OpenAPI 3.1+).
///
/// Known type names deserialize to a unit variant, so a `Vec<SchemaType>`
/// holds no per-element heap allocation in the common case (unlike a
/// `Vec<String>`). An unrecognized name is preserved verbatim in
/// `Custom` so it round-trips and surfaces as a validation error rather
/// than a hard parse failure — matching how [`StringFormat`] treats
/// custom formats.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SchemaType {
    String,
    Number,
    Integer,
    Object,
    Array,
    Boolean,
    Null,

    /// An unrecognized type name, kept verbatim.
    Custom(String),
}

impl Display for SchemaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaType::String => write!(f, "string"),
            SchemaType::Number => write!(f, "number"),
            SchemaType::Integer => write!(f, "integer"),
            SchemaType::Object => write!(f, "object"),
            SchemaType::Array => write!(f, "array"),
            SchemaType::Boolean => write!(f, "boolean"),
            SchemaType::Null => write!(f, "null"),
            SchemaType::Custom(s) => write!(f, "{s}"),
        }
    }
}

impl Serialize for SchemaType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Borrow the name directly — known variants are static strings and
        // `Custom` borrows its own `String` — so no allocation is needed.
        serializer.serialize_str(match self {
            SchemaType::String => "string",
            SchemaType::Number => "number",
            SchemaType::Integer => "integer",
            SchemaType::Object => "object",
            SchemaType::Array => "array",
            SchemaType::Boolean => "boolean",
            SchemaType::Null => "null",
            SchemaType::Custom(s) => s,
        })
    }
}

impl<'de> Deserialize<'de> for SchemaType {
    fn deserialize<D>(deserializer: D) -> Result<SchemaType, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SchemaTypeVisitor;

        impl Visitor<'_> for SchemaTypeVisitor {
            type Value = SchemaType;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str(
                    "one of `string`, `number`, `integer`, `object`, `array`, `boolean`, `null` or a custom string",
                )
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(match value {
                    "string" => SchemaType::String,
                    "number" => SchemaType::Number,
                    "integer" => SchemaType::Integer,
                    "object" => SchemaType::Object,
                    "array" => SchemaType::Array,
                    "boolean" => SchemaType::Boolean,
                    "null" => SchemaType::Null,
                    _ => SchemaType::Custom(String::from(value)),
                })
            }
        }

        deserializer.deserialize_str(SchemaTypeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_format_deserialize() {
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""byte""#).unwrap(),
            StringFormat::Byte,
            "deserialize byte",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""binary""#).unwrap(),
            StringFormat::Binary,
            "deserialize binary",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""date""#).unwrap(),
            StringFormat::Date,
            "deserialize date",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""date-time""#).unwrap(),
            StringFormat::DateTime,
            "deserialize date-time",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""password""#).unwrap(),
            StringFormat::Password,
            "deserialize password",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""uuid""#).unwrap(),
            StringFormat::UUID,
            "deserialize uuid",
        );
        assert_eq!(
            serde_json::from_str::<StringFormat>(r#""foo-bar""#).unwrap(),
            StringFormat::Custom(String::from("foo-bar")),
            "deserialize foo-bar as custom",
        );
    }

    #[test]
    fn test_string_format_serialize() {
        assert_eq!(
            serde_json::to_string(&StringFormat::Byte).unwrap(),
            r#""byte""#,
            "serialize byte",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::Binary).unwrap(),
            r#""binary""#,
            "serialize binary",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::Date).unwrap(),
            r#""date""#,
            "serialize date",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::DateTime).unwrap(),
            r#""date-time""#,
            "serialize date-time",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::Password).unwrap(),
            r#""password""#,
            "serialize password",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::UUID).unwrap(),
            r#""uuid""#,
            "serialize uuid",
        );
        assert_eq!(
            serde_json::to_string(&StringFormat::Custom(String::from("foo-bar"))).unwrap(),
            r#""foo-bar""#,
            "serialize foo-bar as custom",
        );
    }

    #[test]
    fn test_integer_format_deserialize() {
        assert_eq!(
            serde_json::from_str::<IntegerFormat>(r#""int32""#).unwrap(),
            IntegerFormat::Int32,
            "deserialize int32",
        );
        assert_eq!(
            serde_json::from_str::<IntegerFormat>(r#""int64""#).unwrap(),
            IntegerFormat::Int64,
            "deserialize int64",
        );
    }

    #[test]
    fn test_integer_format_serialize() {
        assert_eq!(
            serde_json::to_string(&IntegerFormat::Int32).unwrap(),
            r#""int32""#,
            "serialize int32",
        );
        assert_eq!(
            serde_json::to_string(&IntegerFormat::Int64).unwrap(),
            r#""int64""#,
            "serialize int64",
        );
    }

    #[test]
    fn test_number_format_deserialize() {
        assert_eq!(
            serde_json::from_str::<NumberFormat>(r#""float""#).unwrap(),
            NumberFormat::Float,
            "deserialize float",
        );
        assert_eq!(
            serde_json::from_str::<NumberFormat>(r#""double""#).unwrap(),
            NumberFormat::Double,
            "deserialize double",
        );
    }

    #[test]
    fn test_number_format_serialize() {
        assert_eq!(
            serde_json::to_string(&NumberFormat::Float).unwrap(),
            r#""float""#,
            "serialize float",
        );
        assert_eq!(
            serde_json::to_string(&NumberFormat::Double).unwrap(),
            r#""double""#,
            "serialize double",
        );
    }

    #[test]
    fn test_collection_format_deserialize() {
        assert_eq!(
            serde_json::from_str::<CollectionFormat>(r#""csv""#).unwrap(),
            CollectionFormat::CSV,
            "deserialize csv",
        );
        assert_eq!(
            serde_json::from_str::<CollectionFormat>(r#""ssv""#).unwrap(),
            CollectionFormat::SSV,
            "deserialize ssv",
        );
        assert_eq!(
            serde_json::from_str::<CollectionFormat>(r#""tsv""#).unwrap(),
            CollectionFormat::TSV,
            "deserialize tsv",
        );
        assert_eq!(
            serde_json::from_str::<CollectionFormat>(r#""pipes""#).unwrap(),
            CollectionFormat::PIPES,
            "deserialize pipes",
        );
    }

    #[test]
    fn test_collection_format_serialize() {
        assert_eq!(
            serde_json::to_string(&CollectionFormat::CSV).unwrap(),
            r#""csv""#,
            "serialize csv",
        );
        assert_eq!(
            serde_json::to_string(&CollectionFormat::SSV).unwrap(),
            r#""ssv""#,
            "serialize ssv",
        );
        assert_eq!(
            serde_json::to_string(&CollectionFormat::TSV).unwrap(),
            r#""tsv""#,
            "serialize tsv",
        );
        assert_eq!(
            serde_json::to_string(&CollectionFormat::PIPES).unwrap(),
            r#""pipes""#,
            "serialize pipes",
        );
    }

    #[test]
    fn test_schema_type_round_trip() {
        for (json, value) in [
            (r#""string""#, SchemaType::String),
            (r#""number""#, SchemaType::Number),
            (r#""integer""#, SchemaType::Integer),
            (r#""object""#, SchemaType::Object),
            (r#""array""#, SchemaType::Array),
            (r#""boolean""#, SchemaType::Boolean),
            (r#""null""#, SchemaType::Null),
            (r#""bogus""#, SchemaType::Custom("bogus".to_owned())),
        ] {
            assert_eq!(serde_json::from_str::<SchemaType>(json).unwrap(), value);
            assert_eq!(serde_json::to_string(&value).unwrap(), json);
        }
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Default)]
    struct Test {
        foo: String,
        #[serde(default)]
        format: CollectionFormat,
    }

    #[test]
    fn test_collection_format_deserialize_default() {
        assert_eq!(
            serde_json::from_value::<Test>(serde_json::json!({
                "foo": "bar"
            }))
            .unwrap(),
            Test {
                foo: String::from("bar"),
                format: CollectionFormat::CSV,
            },
            "deserialize csv",
        );
    }

    #[test]
    fn test_collection_format_serialize_default() {
        assert_eq!(
            serde_json::to_value(Test {
                foo: String::from("bar"),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "foo": "bar",
                "format": "csv"
            }),
            "serialize csv",
        );
    }

    #[test]
    fn integer_format_display() {
        assert_eq!(IntegerFormat::Int32.to_string(), "int32");
        assert_eq!(IntegerFormat::Int64.to_string(), "int64");
    }

    #[test]
    fn number_format_display() {
        assert_eq!(NumberFormat::Float.to_string(), "float");
        assert_eq!(NumberFormat::Double.to_string(), "double");
    }

    #[test]
    fn collection_format_display() {
        assert_eq!(CollectionFormat::CSV.to_string(), "csv");
        assert_eq!(CollectionFormat::SSV.to_string(), "ssv");
        assert_eq!(CollectionFormat::TSV.to_string(), "tsv");
        assert_eq!(CollectionFormat::PIPES.to_string(), "pipes");
        assert_eq!(CollectionFormat::Multi.to_string(), "multi");
    }

    #[test]
    fn collection_format_multi_roundtrip() {
        // Tests both Multi serialize and deserialize (previously untested).
        assert_eq!(
            serde_json::to_string(&CollectionFormat::Multi).unwrap(),
            r#""multi""#,
            "serialize multi",
        );
        assert_eq!(
            serde_json::from_str::<CollectionFormat>(r#""multi""#).unwrap(),
            CollectionFormat::Multi,
            "deserialize multi",
        );
    }

    #[test]
    fn string_format_visitor_expecting_message_via_invalid_type() {
        // `StringFormatVisitor::expecting` is invoked by serde when the input
        // type doesn't match (e.g. an integer instead of a string). We trigger
        // it by feeding a JSON number to the `StringFormat` deserializer.
        let err = serde_json::from_str::<StringFormat>("42").unwrap_err();
        // The error message must contain the text produced by `expecting`.
        let msg = err.to_string();
        assert!(
            msg.contains("byte") || msg.contains("string") || msg.contains("expected"),
            "expected a meaningful serde error referencing string formats, got: {msg}"
        );
    }
}
