//! Arazzo v1.1 root document (the *Arazzo Description*).
//!
//! New in v1.1: the optional `$self` field.

use crate::v1_1::components::Components;
use crate::v1_1::info::Info;
use crate::v1_1::source_description::SourceDescription;
use crate::v1_1::version::Version;
use crate::v1_1::workflow::Workflow;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Root Arazzo v1.1 document.
///
/// See [Arazzo Description](https://spec.openapis.org/arazzo/v1.1.0.html#arazzo-description).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Description {
    /// **Required** `1.1.x` per the schema's pattern `^1\.1\.\d+(-.+)?$`.
    pub arazzo: Version,

    /// A self-identifying URI reference for this document. Added in
    /// v1.1; MUST NOT contain a fragment.
    #[serde(rename = "$self", skip_serializing_if = "Option::is_none")]
    pub self_: Option<String>,

    /// **Required** Metadata about the Arazzo description.
    pub info: Info,

    /// **Required** Non-empty list of referenced source descriptions.
    #[serde(rename = "sourceDescriptions")]
    pub source_descriptions: Vec<SourceDescription>,

    /// **Required** Non-empty list of workflows.
    pub workflows: Vec<Workflow>,

    /// Reusable components referenced throughout the description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,

    /// `x-`-prefixed Specification Extensions on the root.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Description {
    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx = Context::new(options);

        if let Some(self_) = &self.self_
            && self_.contains('#')
        {
            ctx.error_field("$self", "must not contain a fragment (`#`)");
        }

        ctx.in_field("info", |ctx| self.info.validate_with_context(ctx));

        if self.source_descriptions.is_empty() {
            ctx.error_field("sourceDescriptions", "must contain at least one entry");
        }
        let mut seen_names = BTreeSet::new();
        for (i, source) in self.source_descriptions.iter().enumerate() {
            ctx.in_index("sourceDescriptions", i, |ctx| {
                source.validate_with_context(ctx);
                if !source.name.is_empty() && !seen_names.insert(source.name.as_str()) {
                    ctx.error_field("name", format!("duplicate source name `{}`", source.name));
                }
            });
        }

        if self.workflows.is_empty() {
            ctx.error_field("workflows", "must contain at least one entry");
        }
        let mut seen_workflow_ids = BTreeSet::new();
        for (i, workflow) in self.workflows.iter().enumerate() {
            ctx.in_index("workflows", i, |ctx| {
                workflow.validate_with_context(ctx);
                if !workflow.workflow_id.is_empty()
                    && !seen_workflow_ids.insert(workflow.workflow_id.as_str())
                {
                    ctx.error_field(
                        "workflowId",
                        format!("duplicate workflowId `{}`", workflow.workflow_id),
                    );
                }
            });
        }

        if let Some(components) = &self.components {
            ctx.in_field("components", |ctx| components.validate_with_context(ctx));
        }

        ctx.into_result()
    }
}

impl Validate for Description {
    fn validate(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        self.validate_inner(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal() -> serde_json::Value {
        json!({
            "arazzo": "1.1.0",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "openapi.yaml" } ],
            "workflows": [
                { "workflowId": "wf", "steps": [ { "stepId": "s", "operationId": "op",
                    "parameters": [ { "name": "p", "in": "query", "value": 1 } ] } ] }
            ]
        })
    }

    #[test]
    fn deserialize_minimal_round_trips_and_validates() {
        let doc: Description = serde_json::from_value(minimal()).unwrap();
        assert_eq!(doc.arazzo, Version::V1_1_0());
        doc.validate(EnumSet::empty()).expect("valid");
    }

    #[test]
    fn self_with_fragment_is_rejected() {
        let mut value = minimal();
        value["$self"] = json!("urn:example#frag");
        let doc: Description = serde_json::from_value(value).unwrap();
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.$self: must not contain a fragment (`#`)")
        );
    }

    #[test]
    fn self_round_trips_under_dollar_key() {
        let mut value = minimal();
        value["$self"] = json!("urn:example:arazzo");
        let doc: Description = serde_json::from_value(value).unwrap();
        assert_eq!(doc.self_.as_deref(), Some("urn:example:arazzo"));
        assert_eq!(
            serde_json::to_value(&doc).unwrap()["$self"],
            json!("urn:example:arazzo")
        );
    }

    #[test]
    fn deserialize_rejects_v1_0_version() {
        let mut bad = minimal();
        bad["arazzo"] = json!("1.0.0");
        assert!(serde_json::from_value::<Description>(bad).is_err());
    }

    #[test]
    fn empty_collections_fail_validation() {
        let doc = Description {
            arazzo: Version::V1_1_0(),
            info: Info {
                title: "T".into(),
                version: "1".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.sourceDescriptions: must contain at least one entry")
        );
        assert!(
            err.errors
                .iter()
                .any(|e| e == "#.workflows: must contain at least one entry")
        );
    }

    #[test]
    fn duplicate_source_names_and_workflow_ids_fail() {
        let doc: Description = serde_json::from_value(json!({
            "arazzo": "1.1.0",
            "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [
                { "name": "dup", "url": "a" },
                { "name": "dup", "url": "b" }
            ],
            "workflows": [
                { "workflowId": "w", "steps": [ { "stepId": "s", "workflowId": "x" } ] },
                { "workflowId": "w", "steps": [ { "stepId": "s2", "workflowId": "y" } ] }
            ]
        }))
        .unwrap();
        let err = doc.validate(EnumSet::empty()).unwrap_err();
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("duplicate source name `dup`"))
        );
        assert!(
            err.errors
                .iter()
                .any(|e| e.contains("duplicate workflowId `w`"))
        );
    }
}
