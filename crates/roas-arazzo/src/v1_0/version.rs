//! Version newtype for Arazzo v1.0 (`arazzo: "1.0.x"`).
//!
//! The field is a string at the wire level but constrained to the
//! schema pattern `^1\.0\.\d+(-.+)?$`
//! ([JSON Schema](https://spec.openapis.org/arazzo/1.0/schema/2025-10-15)).
//! Deserialization rejects non-1.0 values up front so callers don't have
//! to revalidate. A trailing pre-release suffix (`1.0.0-rc1`) is allowed.

use std::fmt::{self, Display, Formatter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(String);

impl Default for Version {
    fn default() -> Self {
        Self("1.0.1".to_owned())
    }
}

impl Version {
    /// Canonical `1.0.1` value (the latest published v1.0 patch).
    #[allow(non_snake_case)]
    pub fn V1_0_1() -> Self {
        Self("1.0.1".to_owned())
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

const VERSION_SCHEMA_DESCRIPTION: &str = "`1.0.<patch>` with optional `-suffix` (Arazzo v1.0)";

/// Schema pattern is `^1\.0\.\d+(-.+)?$` — hand-rolled to avoid pulling
/// in a regex engine for one check.
fn matches_arazzo_1_0_version(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("1.0.") else {
        return false;
    };
    let (digits, suffix) = match rest.split_once('-') {
        Some((digits, suffix)) => (digits, Some(suffix)),
        None => (rest, None),
    };
    // At least one patch digit, all-ASCII-digits, and any pre-release
    // suffix (the `-.+` group) must be non-empty when present.
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
        if matches_arazzo_1_0_version(s) {
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
        if matches_arazzo_1_0_version(&s) {
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
    fn default_is_1_0_1() {
        assert_eq!(Version::default().as_str(), "1.0.1");
    }

    #[test]
    fn accepts_matching_patch_versions() {
        assert!("1.0.0".parse::<Version>().is_ok());
        assert!("1.0.1".parse::<Version>().is_ok());
        assert!("1.0.42".parse::<Version>().is_ok());
    }

    #[test]
    fn accepts_prerelease_suffix() {
        assert!("1.0.0-rc1".parse::<Version>().is_ok());
        assert!("1.0.3-beta.2".parse::<Version>().is_ok());
    }

    #[test]
    fn rejects_non_matching_versions() {
        for bad in ["1.1.0", "2.0.0", "1.0", "1.0.x", "v1.0.0", "1.0.0-", ""] {
            let err = bad.parse::<Version>().unwrap_err();
            assert_eq!(err.0, bad);
            let msg = err.to_string();
            assert!(msg.contains(bad), "msg should echo input: {msg}");
        }
    }

    #[test]
    fn deserialize_rejects_wrong_minor() {
        let err = serde_json::from_value::<Version>(serde_json::json!("1.1.0")).unwrap_err();
        assert!(
            err.to_string().contains("1.0"),
            "expected schema description in error: {err}"
        );
    }

    #[test]
    fn serialize_round_trips_through_string() {
        let v = Version::V1_0_1();
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#""1.0.1""#);
        let back: Version = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn display_renders_inner_string() {
        let v: Version = "1.0.3".parse().unwrap();
        assert_eq!(format!("{v}"), "1.0.3");
    }

    #[test]
    fn try_from_str_and_string_match_from_str() {
        assert_eq!(Version::try_from("1.0.1").unwrap(), Version::V1_0_1());
        assert!(Version::try_from("2.0.0").is_err());

        let owned_ok = Version::try_from(String::from("1.0.7")).unwrap();
        assert_eq!(owned_ok.as_str(), "1.0.7");
        let owned_err = Version::try_from(String::from("nope")).unwrap_err();
        assert_eq!(owned_err.0, "nope");
    }
}
