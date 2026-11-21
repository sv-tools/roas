//! Tag Object

use crate::common::helpers::{validate_required_string, validate_unknown_fields};
use crate::v3_2::external_documentation::ExternalDocumentation;
use crate::v3_2::spec::Spec;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Adds metadata to a single tag that is used by the Operation Object.
/// It is not mandatory to have a Tag Object per tag defined in the Operation Object instances.
///
/// Specification Example:
///
/// ```yaml
/// name: pet
/// description: Pets operations
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// **Required** The name of the tag.
    pub name: String,

    /// A short summary of the tag (added in OAS 3.2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A short description for the tag.
    /// [CommonMark](https://spec.commonmark.org) syntax MAY be used for rich text representation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Additional external documentation for this tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocumentation>,

    /// Name of the parent tag, allowing hierarchical organization
    /// (added in OAS 3.2). Resolves against another `Tag.name` in
    /// `Spec.tags`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,

    /// Tag kind hint for UIs (e.g. `"nav"`, `"audience"`, `"badge"`),
    /// added in OAS 3.2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    ///
    /// Any other key the object does not define is kept here too, so the document round-trips;
    /// validation reports it unless [`crate::validation::Options::IgnoreUnknownFields`] is set.
    #[serde(flatten)]
    #[serde(with = "crate::common::extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl ValidateWithContext<Spec> for Tag {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        validate_unknown_fields(&self.extensions, ctx, &path);
        validate_required_string(&self.name, ctx, format!("{path}.name"));
        if let Some(doc) = &self.external_docs {
            doc.validate_with_context(ctx, format!("{path}.externalDocs"));
        }
        // OAS 3.2: a tag may declare a `parent`. The referenced tag MUST
        // exist in `Spec.tags`, and the parent chain MUST NOT form a cycle.
        if let Some(parent) = &self.parent {
            if parent.is_empty() {
                ctx.error(format!("{path}.parent"), "must not be empty");
            } else if parent == &self.name {
                ctx.error(
                    format!("{path}.parent"),
                    "tag must not name itself as its parent",
                );
            } else if !ctx
                .spec
                .tags
                .as_ref()
                .is_some_and(|tags| tags.iter().any(|t| t.name == *parent))
            {
                ctx.error(
                    format!("{path}.parent"),
                    format_args!("`{parent}` is not declared in `#/tags`"),
                );
            } else if let Some(tags) = &ctx.spec.tags {
                // Walk ancestor links to detect multi-hop cycles (e.g. A -> B -> A).
                let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
                seen.insert(self.name.as_str());
                let mut current = parent.as_str();
                while let Some(ancestor) = tags.iter().find(|t| t.name == current) {
                    if !seen.insert(current) {
                        if current == self.name {
                            ctx.error(
                                format!("{path}.parent"),
                                format_args!("tag parent cycle detected involving `{}`", self.name),
                            );
                        }
                        break;
                    }
                    let Some(next) = ancestor.parent.as_deref() else {
                        break;
                    };
                    current = next;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize() {
        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
                "description": "Pets operations",
                "externalDocs": {
                    "description": "Find more info here",
                    "url": "https://example.com/about"
                },
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                external_docs: Some(ExternalDocumentation {
                    description: Some(String::from("Find more info here")),
                    url: String::from("https://example.com/about"),
                    ..Default::default()
                }),
                ..Default::default()
            },
            "deserialize name, description and externalDocs"
        );

        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
                "description": "Pets operations",
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                ..Default::default()
            },
            "deserialize name and description"
        );

        assert_eq!(
            serde_json::from_value::<Tag>(serde_json::json!({
                "name": "pet",
            }))
            .unwrap(),
            Tag {
                name: String::from("pet"),
                ..Default::default()
            },
            "deserialize name only"
        );
    }

    #[test]
    fn serialize() {
        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                external_docs: Some(ExternalDocumentation {
                    description: Some(String::from("Find more info here")),
                    url: String::from("https://example.com/about"),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
                "description":"Pets operations",
                "externalDocs": {
                    "description": "Find more info here",
                    "url": "https://example.com/about"
                },
            }),
            "serialize name, description and externalDocs",
        );

        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                description: Some(String::from("Pets operations")),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
                "description":"Pets operations",
            }),
            "serialize name and description",
        );

        assert_eq!(
            serde_json::to_value(Tag {
                name: String::from("pet"),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({
                "name": "pet",
            }),
            "serialize name only",
        );
    }

    #[test]
    fn validate() {
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Default::default());
        Tag {
            name: String::from("pet"),
            description: Some(String::from("Pets operations")),
            external_docs: Some(ExternalDocumentation {
                description: Some(String::from("Find more info here")),
                url: String::from("https://example.com/about"),
                ..Default::default()
            }),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            name: String::from("pet"),
            description: Some(String::from("Pets operations")),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            name: String::from("pet"),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        Tag {
            ..Default::default()
        }
        .validate_with_context(&mut ctx, String::from("tag"));
        assert_eq!(
            ctx.errors,
            vec!["tag.name: must not be empty"],
            "name error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn summary_kind_parent_round_trip() {
        let v = serde_json::json!({
            "name": "international",
            "summary": "International",
            "description": "Cross-border flights",
            "kind": "nav",
            "parent": "flights"
        });
        let tag: Tag = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(tag.summary.as_deref(), Some("International"));
        assert_eq!(tag.kind.as_deref(), Some("nav"));
        assert_eq!(tag.parent.as_deref(), Some("flights"));
        assert_eq!(serde_json::to_value(&tag).unwrap(), v);
    }

    #[test]
    fn parent_must_exist_in_spec_tags() {
        let spec = Spec {
            tags: Some(vec![Tag {
                name: "flights".into(),
                ..Default::default()
            }]),
            ..Default::default()
        };
        // OK case: parent resolves.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Tag {
            name: "international".into(),
            parent: Some("flights".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "t".into());
        assert!(ctx.errors.is_empty(), "no errors: {:?}", ctx.errors);

        // Dangling parent.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Tag {
            name: "x".into(),
            parent: Some("missing".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "t".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("parent") && e.contains("missing")),
            "errors: {:?}",
            ctx.errors
        );

        // Self-parent.
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Tag {
            name: "loop".into(),
            parent: Some("loop".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "t".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must not name itself")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn x_display_name_round_trip_via_generic_extensions() {
        // 3.2 supersedes the Redoc-specific `x-displayName` with `summary`.
        // The key still survives round-trip through the generic
        // `extensions` map.
        let tag: Tag = serde_json::from_value(serde_json::json!({
            "name": "pet",
            "x-displayName": "Pets"
        }))
        .unwrap();
        assert_eq!(
            tag.extensions.as_ref().and_then(|m| m.get("x-displayName")),
            Some(&serde_json::Value::String("Pets".to_owned()))
        );
        assert_eq!(
            serde_json::to_value(tag).unwrap(),
            serde_json::json!({
                "name": "pet",
                "x-displayName": "Pets"
            })
        );
    }

    #[test]
    fn parent_empty_string_errors() {
        use crate::v3_2::spec::Spec;
        use crate::validation::Context;
        // line 68: empty parent string must emit an error
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        Tag {
            name: "pets".into(),
            parent: Some("".into()),
            ..Default::default()
        }
        .validate_with_context(&mut ctx, "t".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("must not be empty")),
            "expected empty-parent error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn multi_hop_parent_cycle_errors() {
        let spec = Spec {
            tags: Some(vec![
                Tag {
                    name: "a".into(),
                    parent: Some("b".into()),
                    ..Default::default()
                },
                Tag {
                    name: "b".into(),
                    parent: Some("a".into()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        spec.tags.as_ref().unwrap()[0].validate_with_context(&mut ctx, "tags[0]".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("cycle")),
            "expected cycle error, got: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn used_child_tag_marks_parent_tag_as_used() {
        use crate::validation::{Options, Validate, ValidationErrorsExt};
        let spec: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.0",
            "info": { "title": "T", "version": "1.0" },
            "tags": [
                { "name": "nav", "kind": "nav" },
                { "name": "pets", "parent": "nav" }
            ],
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["pets"],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }))
        .unwrap();
        assert!(
            spec.validate(Options::new(), None).is_ok(),
            "parent tag of used child tag must not be flagged unused: {:?}",
            spec.validate(Options::new(), None)
        );

        // Shared parent (two used children) and dangling parent on a used tag.
        let spec2: Spec = serde_json::from_value(serde_json::json!({
            "openapi": "3.2.0",
            "info": { "title": "T", "version": "1.0" },
            "tags": [
                { "name": "nav", "kind": "nav" },
                { "name": "cats", "parent": "nav" },
                { "name": "dogs", "parent": "nav" },
                { "name": "orphan", "parent": "ghost" }
            ],
            "paths": {
                "/pets": {
                    "get": {
                        "tags": ["cats", "dogs", "orphan"],
                        "responses": { "200": { "description": "ok" } }
                    }
                }
            }
        }))
        .unwrap();
        let err2 = spec2.validate(Options::new(), None).unwrap_err();
        assert!(!err2.mentions("unused"), "got: {:?}", err2.errors);
        assert!(err2.mentions("ghost"), "got: {:?}", err2.errors);
    }

    #[test]
    fn lasso_parent_chain_flags_cycle_members_not_entry() {
        let spec = Spec {
            tags: Some(vec![
                Tag {
                    name: "a".into(),
                    parent: Some("b".into()),
                    ..Default::default()
                },
                Tag {
                    name: "b".into(),
                    parent: Some("c".into()),
                    ..Default::default()
                },
                Tag {
                    name: "c".into(),
                    parent: Some("b".into()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let mut ctx_a = Context::new(&spec, crate::validation::Options::new());
        spec.tags.as_ref().unwrap()[0].validate_with_context(&mut ctx_a, "tags[0]".into());
        assert!(
            ctx_a.errors.is_empty(),
            "entry tag outside cycle should not emit false cycle error: {:?}",
            ctx_a.errors
        );
        let mut ctx_b = Context::new(&spec, crate::validation::Options::new());
        spec.tags.as_ref().unwrap()[1].validate_with_context(&mut ctx_b, "tags[1]".into());
        assert!(ctx_b.errors.iter().any(|e| e.contains("cycle")));
    }
}
