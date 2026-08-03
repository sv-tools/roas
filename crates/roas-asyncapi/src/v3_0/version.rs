//! Version newtype for AsyncAPI v3.0 (`asyncapi: "3.0.0"`).
//!
//! The schema pins this field to `const: "3.0.0"`
//! ([JSON Schema](https://asyncapi.com/schema-store/3.0.0.json)) — not a
//! pattern, so there is no `3.0.1` or `3.0.0-rc1` to accept. Each
//! AsyncAPI release publishes its own schema with its own constant, and
//! this crate models one per version module. Deserialization rejects
//! anything else up front, so a 2.6 or 3.1 document fails at parse time
//! rather than validation time.

use std::fmt::{self, Display, Formatter};

/// The only `asyncapi` value an AsyncAPI v3.0 document may carry.
pub const VERSION: &str = "3.0.0";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self(VERSION.to_owned())
    }
}

impl Version {
    /// The canonical — and only — `3.0.0` value.
    #[allow(non_snake_case)]
    pub fn V3_0_0() -> Self {
        Self(VERSION.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Version {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for Version {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

const VERSION_SCHEMA_DESCRIPTION: &str = "exactly `3.0.0` (AsyncAPI v3.0)";

impl<'de> serde::Deserialize<'de> for Version {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Version::try_from(String::deserialize(de)?).map_err(|InvalidVersion(s)| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Str(&s),
                &VERSION_SCHEMA_DESCRIPTION,
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidVersion(pub String);

impl Display for InvalidVersion {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "asyncapi version {:?} must be {VERSION_SCHEMA_DESCRIPTION}",
            self.0
        )
    }
}

impl std::error::Error for InvalidVersion {}

impl std::str::FromStr for Version {
    type Err = InvalidVersion;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == VERSION {
            Ok(Version(s.to_owned()))
        } else {
            Err(InvalidVersion(s.to_owned()))
        }
    }
}

impl TryFrom<&str> for Version {
    type Error = InvalidVersion;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl TryFrom<String> for Version {
    type Error = InvalidVersion;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s == VERSION {
            Ok(Version(s))
        } else {
            Err(InvalidVersion(s))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_3_0_0() {
        assert_eq!(Version::default().as_str(), VERSION);
        assert_eq!(VERSION, "3.0.0");
    }

    #[test]
    fn accepts_only_the_exact_constant() {
        assert!("3.0.0".parse::<Version>().is_ok());
    }

    #[test]
    fn rejects_everything_else() {
        // The schema uses `const`, so neither a later patch nor a
        // pre-release suffix is a v3.0 document.
        for bad in [
            "3.0.1",
            "3.0.7",
            "3.0.0-rc1",
            "3.1.0",
            "2.6.0",
            "4.0.0",
            "3.0",
            "3.0.x",
            "3.0.0-",
            " 3.0.0",
            "3.0.0 ",
            "",
        ] {
            assert!(bad.parse::<Version>().is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn serialize_round_trips() {
        let v = Version::V3_0_0();
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#""3.0.0""#);
        assert_eq!(serde_json::from_str::<Version>(&s).unwrap(), v);
    }

    #[test]
    fn deserialize_rejects_v2_6_and_v3_1() {
        assert!(serde_json::from_value::<Version>(serde_json::json!("2.6.0")).is_err());
        assert!(serde_json::from_value::<Version>(serde_json::json!("3.1.0")).is_err());
    }

    #[test]
    fn display_renders_inner_string() {
        assert_eq!(format!("{}", Version::V3_0_0()), "3.0.0");
    }

    #[test]
    fn try_from_str_and_string_match_from_str() {
        assert_eq!(Version::try_from("3.0.0").unwrap(), Version::V3_0_0());
        assert!(Version::try_from("3.1.0").is_err());

        let owned_ok = Version::try_from(String::from("3.0.0")).unwrap();
        assert_eq!(owned_ok.as_str(), "3.0.0");
        let owned_err = Version::try_from(String::from("nope")).unwrap_err();
        assert_eq!(owned_err.0, "nope");
    }

    #[test]
    fn invalid_version_error_echoes_input() {
        let err = "3.0.1".parse::<Version>().unwrap_err();
        assert!(err.to_string().contains("3.0.1"));
        assert!(err.to_string().contains("3.0.0"));
    }
}
