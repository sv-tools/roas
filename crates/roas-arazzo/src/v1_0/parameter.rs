//! Arazzo v1.0 `Parameter` object.
//!
//! Per [Parameter Object](https://spec.openapis.org/arazzo/v1.0.1.html#parameter-object):
//! a single named value passed to an operation or workflow. `in` is
//! required when the enclosing step targets an `operationId` /
//! `operationPath` (enforced in [`crate::v1_0::step`]).

use crate::validation::{Context, ValidateWithContext, validate_required_string};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The named location a [`Parameter`] applies to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    #[default]
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Parameter {
    /// **Required** The name of the parameter.
    pub name: String,

    /// The named location of the parameter. Required when the step
    /// targets an operation.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub in_: Option<ParameterLocation>,

    /// **Required** The value to pass (any JSON type, typically a
    /// runtime expression string).
    pub value: serde_json::Value,

    /// `x-`-prefixed Specification Extensions.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext for Parameter {
    fn validate_with_context(&self, ctx: &mut Context, path: String) {
        validate_required_string(&self.name, ctx, format!("{path}.name"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn deserialize_round_trips() {
        let p: Parameter = serde_json::from_value(json!({
            "name": "petId",
            "in": "path",
            "value": "$inputs.petId",
        }))
        .unwrap();
        assert_eq!(p.name, "petId");
        assert_eq!(p.in_, Some(ParameterLocation::Path));
        assert_eq!(p.value, json!("$inputs.petId"));

        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["in"], json!("path"));
    }

    #[test]
    fn value_accepts_any_json_type() {
        let p: Parameter =
            serde_json::from_value(json!({ "name": "flags", "value": [1, 2, 3] })).unwrap();
        assert_eq!(p.value, json!([1, 2, 3]));
        assert!(p.in_.is_none());
    }

    #[test]
    fn validate_rejects_empty_name() {
        let mut c = Context::new(EnumSet::empty());
        Parameter::default().validate_with_context(&mut c, "#.p".into());
        assert!(c.errors.iter().any(|e| e == "#.p.name: must not be empty"));
    }

    #[test]
    fn all_locations_round_trip() {
        for (s, loc) in [
            ("path", ParameterLocation::Path),
            ("query", ParameterLocation::Query),
            ("header", ParameterLocation::Header),
            ("cookie", ParameterLocation::Cookie),
        ] {
            let p: Parameter =
                serde_json::from_value(json!({ "name": "n", "in": s, "value": 1 })).unwrap();
            assert_eq!(p.in_, Some(loc));
        }
    }
}
