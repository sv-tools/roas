//! Version newtype for Overlay v1.1 (`overlay: "1.1.x"`).
//!
//! Mirrors `v1_0::Version`: the field is a string at the wire level
//! but constrained to the schema pattern `^1\.1\.\d+$`
//! ([JSON Schema](https://spec.openapis.org/overlay/1.1/schema/2026-04-01)).
//! Deserialization rejects non-1.1 values up front so callers don't
//! have to revalidate.

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

const VERSION_SCHEMA_DESCRIPTION: &str = "`1.1.<patch>` semver (Overlay v1.1)";

/// Schema pattern is `^1\.1\.\d+$` — hand-rolled to avoid pulling in
/// `lazy-regex` just for this one check.
fn matches_overlay_1_1_version(s: &str) -> bool {
    s.strip_prefix("1.1.")
        .is_some_and(|patch| !patch.is_empty() && patch.bytes().all(|b| b.is_ascii_digit()))
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
            "overlay version {:?} must be {VERSION_SCHEMA_DESCRIPTION}",
            self.0
        )
    }
}

impl std::error::Error for InvalidVersion {}

impl std::str::FromStr for Version {
    type Err = InvalidVersion;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if matches_overlay_1_1_version(s) {
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
        if matches_overlay_1_1_version(&s) {
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
    fn accepts_matching_patch_versions() {
        assert!("1.1.0".parse::<Version>().is_ok());
        assert!("1.1.42".parse::<Version>().is_ok());
    }

    #[test]
    fn rejects_non_matching_versions() {
        for bad in ["1.0.0", "1.2.0", "2.1.0", "1.1", "1.1.x", "v1.1.0", ""] {
            let err = bad.parse::<Version>().unwrap_err();
            assert_eq!(err.0, bad);
            assert!(err.to_string().contains(bad));
        }
    }

    #[test]
    fn deserialize_rejects_wrong_minor() {
        let err = serde_json::from_value::<Version>(serde_json::json!("1.0.0")).unwrap_err();
        assert!(err.to_string().contains("1.1"));
    }

    #[test]
    fn serialize_round_trips_through_string() {
        let v = Version::V1_1_0();
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, r#""1.1.0""#);
        let back: Version = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn display_renders_inner_string() {
        let v: Version = "1.1.3".parse().unwrap();
        assert_eq!(format!("{v}"), "1.1.3");
    }

    #[test]
    fn try_from_str_and_string_match_from_str() {
        assert_eq!(Version::try_from("1.1.0").unwrap(), Version::V1_1_0());
        assert!(Version::try_from("2.0.0").is_err());

        let owned_ok = Version::try_from(String::from("1.1.7")).unwrap();
        assert_eq!(owned_ok.as_str(), "1.1.7");
        let owned_err = Version::try_from(String::from("nope")).unwrap_err();
        assert_eq!(owned_err.0, "nope");
    }
}
