//! The Arazzo documents one run can enter: the entry description, plus every
//! external Arazzo document a source registry links from it.
//!
//! Each document keeps its own source aliases, base-URL overrides, components,
//! `$self` and workflow namespace. Handles are table indices, so documents that
//! link back to each other are plain data, not recursive ownership.

use crate::{ExecutionError, Options};
use roas_arazzo::v1_1::{Description, SourceType, Workflow};
use std::collections::BTreeMap;

/// A document in a run's table. [`Doc::ENTRY`] is the description the run
/// was started with; every other handle names an external Arazzo document.
/// How to supply an external Arazzo document, in this build.
#[cfg(feature = "source-graph")]
const SUPPLY_ARAZZO: &str =
    "load the Arazzo document into a source registry and pass it with `Options::source_registry`";
#[cfg(not(feature = "source-graph"))]
const SUPPLY_ARAZZO: &str = "external workflows need the `source-graph` feature";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Doc(pub(crate) usize);

impl Doc {
    pub(crate) const ENTRY: Self = Self(0);
}

/// One document's scope.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Document<'d> {
    pub(crate) description: &'d Description,
    /// Sources, base URLs and registry owner of this document only. Global
    /// settings (limits, headers, input policy) always come from the root.
    pub(crate) options: &'d Options,
    /// Canonical identity of an external document, for reports and
    /// diagnostics. The entry description keeps its unqualified history.
    pub(crate) identity: Option<&'d str>,
    links: Option<&'d BTreeMap<String, usize>>,
}

/// The documents a run can enter, indexed by [`Doc`].
#[derive(Clone, Debug)]
pub(crate) struct Documents<'d> {
    list: Vec<Document<'d>>,
}

/// Prefix of a workflow reference into another Arazzo document.
const EXTERNAL: &str = "$sourceDescriptions.";

impl<'d> Documents<'d> {
    pub(crate) fn new(description: &'d Description, options: &'d Options) -> Self {
        #[cfg_attr(not(feature = "source-graph"), allow(unused_mut))]
        let mut list = vec![Document {
            description,
            options,
            identity: None,
            links: None,
        }];
        #[cfg(feature = "source-graph")]
        if let Some(table) = &options.workflow_documents {
            list[0].links = Some(&table.entry);
            for external in &table.documents {
                list.push(Document {
                    description: external
                        .document
                        .arazzo()
                        .expect("workflow documents are Arazzo"),
                    options: &external.options,
                    identity: Some(external.document.identity().as_str()),
                    links: Some(&external.links),
                });
            }
        }
        Self { list }
    }

    pub(crate) fn get(&self, doc: Doc) -> &Document<'d> {
        &self.list[doc.0]
    }

    pub(crate) fn len(&self) -> usize {
        self.list.len()
    }

    /// The source alias of an external reference, when `reference` is one.
    pub(crate) fn alias(reference: &str) -> Option<&str> {
        reference
            .strip_prefix(EXTERNAL)
            .and_then(|rest| rest.split_once('.'))
            .map(|(alias, _)| alias)
    }

    /// The document an alias of `doc` links to, if one was supplied.
    pub(crate) fn linked(&self, doc: Doc, alias: &str) -> Option<Doc> {
        self.get(doc)
            .links
            .and_then(|links| links.get(alias))
            .map(|index| Doc(*index))
    }

    /// Resolve a workflow reference written in `doc`: a local `workflowId`,
    /// or `$sourceDescriptions.<name>.<workflowId>` naming an Arazzo source.
    pub(crate) fn resolve(
        &self,
        doc: Doc,
        reference: &str,
    ) -> Result<(Doc, usize), ExecutionError> {
        let Some(rest) = reference.strip_prefix(EXTERNAL) else {
            if reference.starts_with('$') {
                return Err(ExecutionError::Unsupported(format!(
                    "workflow reference `{reference}` is a runtime expression this executor cannot resolve statically"
                )));
            }
            return self
                .position(doc, reference)
                .map(|index| (doc, index))
                .ok_or_else(|| {
                    match self.get(doc).identity {
                        // Keep the id bare; the run names the external
                        // document around it, as preparation's diagnostic does.
                        Some(_) => ExecutionError::ExternalWorkflow {
                            reference: reference.to_owned(),
                            reason: format!("this document has no workflow `{reference}`"),
                        },
                        None => ExecutionError::UnknownWorkflow(reference.to_owned()),
                    }
                });
        };
        let external = |reason: String| ExecutionError::ExternalWorkflow {
            reference: reference.to_owned(),
            reason,
        };
        let (alias, workflow_id) = rest
            .split_once('.')
            .filter(|(alias, id)| !alias.is_empty() && !id.is_empty())
            .ok_or_else(|| external("expected `$sourceDescriptions.<name>.<workflowId>`".into()))?;
        let source = self
            .get(doc)
            .description
            .source_descriptions
            .iter()
            .find(|source| source.name == alias)
            .ok_or_else(|| external(format!("this document declares no source `{alias}`")))?;
        if let Some(kind) = source.type_
            && kind != SourceType::Arazzo
        {
            return Err(external(format!(
                "source `{alias}` is declared as {kind:?}, not an Arazzo document"
            )));
        }
        let target = self.linked(doc, alias).ok_or_else(|| {
            external(
                match self
                    .get(doc)
                    .options
                    .sources
                    .get(alias)
                    .map(|source| &source.data)
                {
                    Some(crate::operation::SourceData::Owned(_)) => format!(
                        "source `{alias}` was given with `Options::source`, which supplies API \
                         documents only; {SUPPLY_ARAZZO}"
                    ),
                    #[cfg(feature = "source-graph")]
                    Some(crate::operation::SourceData::Registry(_)) => {
                        format!("source `{alias}` is not an Arazzo document")
                    }
                    #[cfg(feature = "source-graph")]
                    None => format!("source `{alias}` was not supplied as an Arazzo document"),
                    #[cfg(not(feature = "source-graph"))]
                    None => format!(
                        "source `{alias}` was not supplied as an Arazzo document; {SUPPLY_ARAZZO}"
                    ),
                },
            )
        })?;
        let index = self.position(target, workflow_id).ok_or_else(|| {
            external(format!(
                "document {} has no workflow `{workflow_id}`",
                self.get(target)
                    .identity
                    .unwrap_or("(the entry description)")
            ))
        })?;
        Ok((target, index))
    }

    pub(crate) fn workflow(&self, doc: Doc, index: usize) -> &'d Workflow {
        &self.get(doc).description.workflows[index]
    }

    fn position(&self, doc: Doc, workflow_id: &str) -> Option<usize> {
        self.get(doc)
            .description
            .workflows
            .iter()
            .position(|workflow| workflow.workflow_id == workflow_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn description(sources: serde_json::Value) -> Description {
        serde_json::from_value(json!({
            "arazzo": "1.1.0", "info": { "title": "t", "version": "1" },
            "sourceDescriptions": sources,
            "workflows": [{ "workflowId": "w", "steps": [{ "stepId": "s", "operationId": "x" }] }]
        }))
        .unwrap()
    }

    #[test]
    fn unsupplied_and_mistyped_references_explain_themselves() {
        let description = description(json!([
            { "name": "api", "url": "https://example.test/api.json", "type": "openapi" },
            { "name": "shared", "url": "https://example.test/shared.arazzo.json" }
        ]));
        let options = Options::new();
        let documents = Documents::new(&description, &options);
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.resolve(Doc::ENTRY, "w").unwrap(), (Doc::ENTRY, 0));
        for (reference, expected) in [
            ("missing", "no workflow `missing`"),
            ("$inputs.w", "runtime expression"),
            (
                "$sourceDescriptions.shared",
                "expected `$sourceDescriptions.",
            ),
            ("$sourceDescriptions..w", "expected `$sourceDescriptions."),
            ("$sourceDescriptions.typo.w", "declares no source `typo`"),
            ("$sourceDescriptions.api.w", "not an Arazzo document"),
            ("$sourceDescriptions.shared.w", "was not supplied"),
        ] {
            let error = documents.resolve(Doc::ENTRY, reference).unwrap_err();
            assert!(error.to_string().contains(expected), "{reference}: {error}");
        }
        assert_eq!(
            Documents::alias("$sourceDescriptions.shared.w"),
            Some("shared")
        );
        assert_eq!(Documents::alias("w"), None);
        // The advice is one this build can follow.
        let owned = Options::new().source(
            "shared",
            "https://example.test/shared.arazzo.json",
            json!({}),
        );
        let error = Documents::new(&description, &owned)
            .resolve(Doc::ENTRY, "$sourceDescriptions.shared.w")
            .unwrap_err()
            .to_string();
        assert!(error.ends_with(SUPPLY_ARAZZO), "{error}");
        #[cfg(not(feature = "source-graph"))]
        assert!(error.contains("need the `source-graph` feature"), "{error}");
    }
}
