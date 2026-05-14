//! Discriminator Object

use crate::common::helpers::validate_required_string;
use crate::common::reference::RefOr;
use crate::v3_0::schema::Schema;
use crate::v3_0::spec::Spec;
use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// When request bodies or response payloads may be one of a number of different schemas,
/// a discriminator object can be used to aid in serialization, deserialization, and validation.
/// The discriminator is a specific object in a schema which is used to inform the consumer of t
/// he specification of an alternative schema based on the value associated with it.
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

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Discriminator {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.property_name, ctx, format!("{path}.propertyName"));

        if let Some(mapping) = &self.mapping {
            for (k, v) in mapping {
                let schema_ref = RefOr::<Schema>::new_ref(format!("#/components/schemas/{v}"));
                schema_ref.validate_with_context(ctx, format!("{path}.mapping[{k}]"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v3_0::schema::{ObjectSchema, SingleSchema};
    use crate::validation::Context;
    use crate::validation::Options;

    #[test]
    fn round_trip_with_mapping() {
        let json = serde_json::json!({
            "propertyName": "type",
            "mapping": {
                "cat": "Cat",
                "dog": "Dog",
            },
        });
        let d: Discriminator = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(d.property_name, "type");
        assert_eq!(d.mapping.as_ref().unwrap().len(), 2);
        assert_eq!(serde_json::to_value(&d).unwrap(), json);
    }

    #[test]
    fn round_trip_property_only() {
        let json = serde_json::json!({"propertyName": "kind"});
        let d: Discriminator = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(d.mapping, None);
        assert_eq!(serde_json::to_value(&d).unwrap(), json);
    }

    #[test]
    fn round_trip_with_extensions() {
        let json = serde_json::json!({
            "propertyName": "type",
            "x-internal": "v",
        });
        let d: Discriminator = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(
            d.extensions.as_ref().and_then(|m| m.get("x-internal")),
            Some(&serde_json::json!("v")),
        );
        assert_eq!(serde_json::to_value(&d).unwrap(), json);
    }

    #[test]
    fn validate_empty_property_name() {
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
    fn validate_mapping_resolves() {
        let mut spec = Spec::default();
        spec.define_schema("Cat", SingleSchema::from(ObjectSchema::default()))
            .unwrap();
        let d = Discriminator {
            property_name: "type".into(),
            mapping: Some(BTreeMap::from([
                ("cat".to_owned(), "Cat".to_owned()),
                ("missing".to_owned(), "Missing".to_owned()),
            ])),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, Options::new());
        d.validate_with_context(&mut ctx, "d".to_owned());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("Missing") && e.contains("not found")),
            "expected missing schema reported: {:?}",
            ctx.errors
        );
        assert!(
            !ctx.errors
                .iter()
                .any(|e| e.contains("schemas/Cat") && e.contains("not found")),
            "Cat must resolve: {:?}",
            ctx.errors
        );
    }
}
