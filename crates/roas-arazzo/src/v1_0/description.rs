//! Arazzo v1.0 root document (the *Arazzo Description*).

use crate::v1_0::components::Components;
use crate::v1_0::info::Info;
use crate::v1_0::source_description::SourceDescription;
use crate::v1_0::version::Version;
use crate::v1_0::workflow::Workflow;
use crate::validation::{Context, Error, Validate, ValidateWithContext, ValidationOptions};
use enumset::EnumSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Root Arazzo v1.0 document.
///
/// See [Arazzo Description](https://spec.openapis.org/arazzo/v1.0.1.html#arazzo-description).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Description {
    /// **Required** `1.0.x` per the schema's pattern `^1\.0\.\d+(-.+)?$`.
    pub arazzo: Version,

    /// **Required** Metadata about the Arazzo description.
    pub info: Info,

    /// **Required** Non-empty list of referenced source descriptions
    /// (OpenAPI or Arazzo documents).
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
    /// The members of `raw` that this description did not keep: neither
    /// fields of their object nor `x-` extensions. `raw` is the value this
    /// description was deserialized from; see [`crate::unknown`].
    ///
    /// ```
    /// use roas_arazzo::v1_0::Description;
    ///
    /// let raw = serde_json::json!({
    ///     "arazzo": "1.0.1",
    ///     "info": { "title": "T", "version": "1", "summery": "typo" },
    ///     "sourceDescriptions": [{ "name": "api", "url": "api.json" }],
    ///     "workflows": []
    /// });
    /// let description: Description = serde_json::from_value(raw.clone()).unwrap();
    /// let unknown = description.unknown_members(&raw).unwrap();
    /// assert_eq!(unknown.version, "1.0.1");
    /// assert_eq!(unknown.members[0].path, "#.info.summery");
    /// ```
    ///
    /// # Errors
    /// Serializing the description failed, which a description read from a
    /// JSON value does not do.
    pub fn unknown_members(
        &self,
        raw: &serde_json::Value,
    ) -> Result<crate::unknown::UnknownMembers, serde_json::Error> {
        Ok(crate::unknown::diff(raw, &serde_json::to_value(self)?))
    }

    fn validate_inner(&self, options: EnumSet<ValidationOptions>) -> Result<(), Error> {
        let mut ctx = Context::new(options);

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

    #[test]
    fn input_refs_resolve_through_the_components() {
        let mut value = minimal();
        let bare: Description = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(bare.resolve_input_ref("#/components/inputs/pet"), None);
        assert_eq!(
            bare.resolved_input_ref("#/components/inputs/pet"),
            "#/components/inputs/pet"
        );

        value["components"] = json!({
            "inputs": {
                "pet": { "type": "object" },
                "alias": { "$ref": "#/components/inputs/pet" }
            }
        });
        let description: Description = serde_json::from_value(value).unwrap();
        let pet = description.resolve_input_ref("#/components/inputs/alias");
        assert_eq!(pet, Some(&json!({ "type": "object" })));
        assert_eq!(
            description.resolved_input_ref("#/components/inputs/alias"),
            "#/components/inputs/pet"
        );
    }

    fn minimal() -> serde_json::Value {
        json!({
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1.0.0" },
            "sourceDescriptions": [ { "name": "src", "url": "openapi.yaml" } ],
            "workflows": [
                { "workflowId": "wf", "steps": [ { "stepId": "s", "operationId": "op",
                    "parameters": [ { "name": "p", "in": "query", "value": 1 } ] } ] }
            ]
        })
    }

    #[test]
    fn deserialize_minimal_round_trips() {
        let doc: Description = serde_json::from_value(minimal()).unwrap();
        assert_eq!(doc.arazzo, Version::V1_0_1());
        assert_eq!(doc.source_descriptions.len(), 1);
        assert_eq!(doc.workflows.len(), 1);
        assert!(doc.components.is_none());
        doc.validate(EnumSet::empty()).expect("valid");
    }

    #[test]
    fn serialize_preserves_field_names() {
        let doc: Description = serde_json::from_value(minimal()).unwrap();
        let v = serde_json::to_value(&doc).unwrap();
        assert_eq!(v["arazzo"], json!("1.0.1"));
        assert!(v["sourceDescriptions"].is_array());
        assert!(v["workflows"].is_array());
        assert!(v.get("components").is_none());
    }

    #[test]
    fn deserialize_rejects_wrong_version() {
        let mut bad = minimal();
        bad["arazzo"] = json!("2.0.0");
        assert!(serde_json::from_value::<Description>(bad).is_err());
    }

    #[test]
    fn empty_collections_fail_validation() {
        let doc = Description {
            arazzo: Version::V1_0_1(),
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
            "arazzo": "1.0.1",
            "info": { "title": "T", "version": "1" },
            "sourceDescriptions": [
                { "name": "dup", "url": "a" },
                { "name": "dup", "url": "b" }
            ],
            "workflows": [
                { "workflowId": "w", "steps": [ { "stepId": "s", "workflowId": "x" } ] },
                { "workflowId": "w", "steps": [ { "stepId": "s", "workflowId": "y" } ] }
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

    #[test]
    fn root_extensions_are_captured() {
        let mut value = minimal();
        value["x-internal"] = json!(true);
        let doc: Description = serde_json::from_value(value).unwrap();
        assert_eq!(
            doc.extensions.as_ref().unwrap().get("x-internal"),
            Some(&json!(true))
        );
    }
}
