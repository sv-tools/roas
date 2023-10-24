use std::fmt::Display;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub enum StringFormat {
    /// base64 encoded characters
    #[serde(rename = "byte")]
    Byte,

    /// any sequence of octets
    #[serde(rename = "binary")]
    Binary,

    /// As defined by `full-date` - [RFC3339](https://www.rfc-editor.org/rfc/rfc3339)
    #[serde(rename = "date")]
    Date,

    /// As defined by `date-time` - [RFC3339](https://www.rfc-editor.org/rfc/rfc3339)
    #[serde(rename = "date-time")]
    DateTime,

    /// Used to hint UIs the input needs to be obscured.
    #[serde(rename = "password")]
    Password,

    /// As defined by [RFC4122](https://www.rfc-editor.org/rfc/rfc4122)
    #[serde(rename = "uuid")]
    UUID,
}

impl Display for StringFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StringFormat::Byte => write!(f, "byte"),
            StringFormat::Binary => write!(f, "binary"),
            StringFormat::Date => write!(f, "date"),
            StringFormat::DateTime => write!(f, "date-time"),
            StringFormat::Password => write!(f, "password"),
            StringFormat::UUID => write!(f, "uuid"),
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CollectionFormat::CSV => write!(f, "csv"),
            CollectionFormat::SSV => write!(f, "ssv"),
            CollectionFormat::TSV => write!(f, "tsv"),
            CollectionFormat::PIPES => write!(f, "pipes"),
            CollectionFormat::Multi => write!(f, "multi"),
        }
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
}
