//! Discriminator Object

use crate::common::helpers::validate_required_string;
use crate::common::reference::RefOr;
use crate::v3_2::schema::Schema;
use crate::v3_2::spec::Spec;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// When request bodies or response payloads may be one of a number of different schemas,
/// a discriminator object can be used to aid in serialization, deserialization, and validation.
/// The discriminator is a specific object in a schema which is used to inform the consumer
/// of the specification of an alternative schema based on the value associated with it.
///
/// When using the discriminator, inline schemas will not be considered.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Discriminator {
    /// **Required** The name of the property in the payload that will hold the discriminator value.
    #[serde(rename = "propertyName")]
    pub property_name: String,

    /// An object to hold mappings between payload values and schema names or references.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping: Option<BTreeMap<String, String>>,

    /// Default mapping target (added in OAS 3.2). Used when the discriminator
    /// property is missing or its value doesn't appear in `mapping`.
    /// Resolved as either a component schema name
    /// (`#/components/schemas/<value>`) or a full URI reference, with the
    /// same shape rules as `mapping` values.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "defaultMapping")]
    pub default_mapping: Option<String>,
}

impl ValidateWithContext<Spec> for Discriminator {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.property_name, ctx, format!("{path}.propertyName"));

        let resolve = |v: &str| -> String {
            // Mapping values are EITHER component schema *names* (resolved
            // against `#/components/schemas/<name>`) OR URI references per
            // RFC 3986. Component names match `^[a-zA-Z0-9._-]+$` so any
            // value containing `/`, `#`, or `:` is treated as a URI ref.
            let is_uri_ref = v.contains('/') || v.starts_with('#') || v.contains(':');
            if is_uri_ref {
                v.to_owned()
            } else {
                format!("#/components/schemas/{v}")
            }
        };
        if let Some(mapping) = &self.mapping {
            for (k, v) in mapping {
                RefOr::<Schema>::new_ref(resolve(v))
                    .validate_with_context(ctx, format!("{path}.mapping[{k}]"));
            }
        }
        if let Some(v) = &self.default_mapping {
            RefOr::<Schema>::new_ref(resolve(v))
                .validate_with_context(ctx, format!("{path}.defaultMapping"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_2::schema::{ObjectSchema, SingleSchema};
    use crate::validation::Context;
    use crate::validation::Options;

    #[test]
    fn round_trip_with_mapping() {
        let json = serde_json::json!({
            "propertyName": "type",
            "mapping": {"cat": "Cat", "dog": "Dog"}
        });
        let d: Discriminator = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(d.property_name, "type");
        assert_eq!(d.mapping.as_ref().unwrap().len(), 2);
        assert_eq!(serde_json::to_value(&d).unwrap(), json);
    }

    #[test]
    fn validate_empty_property_name_errors() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        Discriminator::default().validate_with_context(&mut ctx, "d".to_owned());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("propertyName") && e.contains("must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn mapping_resolves_against_components() {
        let mut spec = Spec::default();
        spec.define_schema(
            "Cat",
            Schema::Single(Box::new(SingleSchema::Object(ObjectSchema::default()))),
        )
        .unwrap();
        let d = Discriminator {
            property_name: "type".into(),
            mapping: Some(BTreeMap::from([
                ("cat".to_owned(), "Cat".to_owned()),
                ("missing".to_owned(), "Missing".to_owned()),
            ])),
            default_mapping: None,
        };
        let mut ctx = Context::new(&spec, Options::new());
        d.validate_with_context(&mut ctx, "d".to_owned());
        assert!(
            ctx.errors.iter().any(|e| e.contains("Missing")),
            "expected missing schema error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn default_mapping_round_trip_and_resolution() {
        // OAS 3.2: defaultMapping resolves like mapping values.
        let v = serde_json::json!({
            "propertyName": "type",
            "mapping": {"cat": "Cat"},
            "defaultMapping": "Animal"
        });
        let d: Discriminator = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(d.default_mapping.as_deref(), Some("Animal"));
        assert_eq!(serde_json::to_value(&d).unwrap(), v);

        // Validation: dangling defaultMapping target reports.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        d.validate_with_context(&mut ctx, "d".to_owned());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("defaultMapping") && e.contains("Animal")),
            "expected dangling defaultMapping target: {:?}",
            ctx.errors
        );
    }
}
