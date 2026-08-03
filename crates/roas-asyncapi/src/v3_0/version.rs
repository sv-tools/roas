//! Version newtype for AsyncAPI v3.0 (`asyncapi: "3.0.x"`).
//!
//! Constrained to `3.0.<patch>` with an optional `-suffix`
//! ([JSON Schema](https://asyncapi.com/schema-store/3.0.0.json)).
//! Deserialization rejects non-3.0 values up front, so a 2.6 or 3.1
//! document fails at parse time rather than validation time.

use std::fmt::{self, Display, Formatter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("3.0.0".to_owned())
    }
}

impl Version {
    /// Canonical `3.0.0` value.
    #[allow(non_snake_case)]
    pub fn V3_0_0() -> Self {
        Self("3.0.0".to_owned())
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

const VERSION_SCHEMA_DESCRIPTION: &str = "`3.0.<patch>` with optional `-suffix` (AsyncAPI v3.0)";

/// Pattern `^3\.0\.\d+(-.+)?$` — hand-rolled to avoid a regex engine for
/// one check.
fn matches_asyncapi_3_0_version(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("3.0.") else {
        return false;
    };
    let (digits, suffix) = match rest.split_once('-') {
        Some((digits, suffix)) => (digits, Some(suffix)),
        None => (rest, None),
    };
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && suffix.is_none_or(|s| !s.is_empty())
}

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
        if matches_asyncapi_3_0_version(s) {
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
        if matches_asyncapi_3_0_version(&s) {
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
        assert_eq!(Version::default().as_str(), "3.0.0");
    }

    #[test]
    fn accepts_matching_versions() {
        assert!("3.0.0".parse::<Version>().is_ok());
        assert!("3.0.7".parse::<Version>().is_ok());
        assert!("3.0.0-rc1".parse::<Version>().is_ok());
    }

    #[test]
    fn rejects_non_matching_versions() {
        for bad in ["2.6.0", "3.1.0", "4.0.0", "3.0", "3.0.x", "3.0.0-", ""] {
            assert!(bad.parse::<Version>().is_err(), "should reject {bad}");
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
        let v: Version = "3.0.3".parse().unwrap();
        assert_eq!(format!("{v}"), "3.0.3");
    }

    #[test]
    fn try_from_str_and_string_match_from_str() {
        assert_eq!(Version::try_from("3.0.0").unwrap(), Version::V3_0_0());
        assert!(Version::try_from("2.6.0").is_err());

        let owned_ok = Version::try_from(String::from("3.0.9")).unwrap();
        assert_eq!(owned_ok.as_str(), "3.0.9");
        let owned_err = Version::try_from(String::from("nope")).unwrap_err();
        assert_eq!(owned_err.0, "nope");
    }

    #[test]
    fn invalid_version_error_echoes_input() {
        let err = "2.6.0".parse::<Version>().unwrap_err();
        assert!(err.to_string().contains("2.6.0"));
    }
}
