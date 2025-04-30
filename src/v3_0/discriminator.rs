//! Discriminator Object

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::helpers::{Context, ValidateWithContext, validate_required_string};
use crate::common::reference::RefOr;
use crate::v3_0::schema::Schema;
use crate::v3_0::spec::Spec;

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
}

impl ValidateWithContext<Spec> for Discriminator {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_required_string(&self.property_name, ctx, format!("{}.propertyName", path));

        if let Some(mapping) = &self.mapping {
            for (k, v) in mapping {
                let schema_ref = RefOr::<Schema>::new_ref(format!("#/components/schemas/{}", v));
                schema_ref.validate_with_context(ctx, format!("{}.mapping[{}]", path, k));
            }
        }
    }
}
