//! Version newtype for Arazzo v1.1 (`arazzo: "1.1.x"`).
//!
//! Constrained to the schema pattern `^1\.1\.\d+(-.+)?$`
//! ([JSON Schema](https://spec.openapis.org/arazzo/1.1/schema/2026-04-15)).
//! Deserialization rejects non-1.1 values up front.

use std::fmt::{self, Display, Formatter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("1.1.0".to_owned())
    }
}

impl Version {
    /// Canonical `1.1.0` value.
    #[allow(non_snake_case)]
    pub fn V1_1_0() -> Self {
        Self("1.1.0".to_owned())
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

const VERSION_SCHEMA_DESCRIPTION: &str = "`1.1.<patch>` with optional `-suffix` (Arazzo v1.1)";

/// Schema pattern `^1\.1\.\d+(-.+)?$` — hand-rolled to avoid a regex
/// engine for one check.
fn matches_arazzo_1_1_version(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("1.1.") else {
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
            "arazzo version {:?} must be {VERSION_SCHEMA_DESCRIPTION}",
            self.0
        )
    }
}

impl std::error::Error for InvalidVersion {}

impl std::str::FromStr for Version {
    type Err = InvalidVersion;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if matches_arazzo_1_1_version(s) {
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
        if matches_arazzo_1_1_version(&s) {
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
    fn default_is_1_1_0() {
        assert_eq!(Version::default().as_str(), "1.1.0");
    }

    #[test]
    fn accepts_matching_versions() {
        assert!("1.1.0".parse::<Version>().is_ok());
        assert!("1.1.7".parse::<Version>().is_ok());
        assert!("1.1.0-rc1".parse::<Version>().is_ok());
    }

    #[test]
    fn rejects_non_matching_versions() {
        for bad in ["1.0.0", "1.2.0", "2.1.0", "1.1", "1.1.x", "1.1.0-", ""] {
            assert!(bad.parse::<Version>().is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn serialize_round_trips() {
        let v = Version::V1_1_0();
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#""1.1.0""#);
        assert_eq!(serde_json::from_str::<Version>(&s).unwrap(), v);
    }

    #[test]
    fn deserialize_rejects_v1_0() {
        assert!(serde_json::from_value::<Version>(serde_json::json!("1.0.0")).is_err());
    }
}
