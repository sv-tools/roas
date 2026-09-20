//! Finding the operation a step names, in the descriptions it points at.
//!
//! Source values stay immutable. `operation_index` builds borrowed, version-aware
//! views of mounted operations and referenced fields; `operation_document` keeps
//! document identities separate from server retrieval bases.

use serde_json::Value;

/// A source description the run was given.
#[derive(Clone, Debug)]
pub(crate) struct Source {
    /// The URL the description was declared with.
    pub url: String,
    pub data: SourceData,
}

/// Legacy callers own their value; registry-backed sources share the same
/// immutable document with the registry and loader, including cloned Options.
#[derive(Clone, Debug)]
pub(crate) enum SourceData {
    Owned(Value),
    #[cfg(feature = "source-graph")]
    Registry(std::sync::Arc<crate::SourceDocument>),
}

impl Source {
    pub(crate) fn document(&self) -> &Value {
        match &self.data {
            SourceData::Owned(value) => value,
            #[cfg(feature = "source-graph")]
            SourceData::Registry(document) => document.value(),
        }
    }
}

/// Where a step's request is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    /// HTTP method, preserving case for OpenAPI 3.2 additional operations.
    pub method: String,
    /// The path template, `{parameters}` still in it.
    pub path: String,
    /// The server the path hangs off, without a trailing slash.
    pub base: String,
}

/// Why a step could not be turned into a request.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OperationError {
    /// Duplicate IDs cannot identify a unique mounted operation within a source.
    #[error("operation `{operation}` is duplicated in source `{source_name}`: {locations}")]
    Duplicate {
        /// Duplicated operation ID.
        operation: String,
        /// Source alias.
        source_name: String,
        /// Conflicting mounted operation pointers.
        locations: String,
    },
    /// A Path Item reference or target could not be resolved safely.
    #[error("{document}#{pointer}: Path Item reference `{reference}`: {reason}")]
    Reference {
        /// Document identity or supplied location.
        document: String,
        /// JSON Pointer to the referring field or invalid target.
        pointer: String,
        /// Reference as written, empty for an invalid object/cycle.
        reference: String,
        /// Resolution, shape, cycle or conflicting-field explanation.
        reason: String,
    },
    /// The supplied source cannot be interpreted by the OpenAPI execution profile.
    #[error("cannot index operation document `{document}`: {reason}")]
    Document {
        /// Supplied document location.
        document: String,
        /// Version or document-context error.
        reason: String,
    },
    /// A selected server cannot produce an absolute HTTP endpoint.
    #[error("invalid server for operation `{operation}`: {reason}")]
    Server {
        /// Operation ID or operationPath as written.
        operation: String,
        /// Invalid template, missing origin or unsupported URL explanation.
        reason: String,
    },
    /// No source description holds the named operation.
    #[error("operation `{operation}` is in none of the source descriptions")]
    Unknown {
        /// The `operationId` as written.
        operation: String,
    },
    /// More than one does, so the name does not say which.
    #[error("operation `{operation}` is in more than one source description: {sources}")]
    Ambiguous {
        /// The `operationId` as written.
        operation: String,
        /// The names that hold it.
        sources: String,
    },
    /// A bare `operationId` must be unique across the descriptions, and
    /// with one of them missing that cannot be shown.
    #[error(
        "operation `{operation}` is named without a source description, and {missing} \
         was not supplied — supply it, or name the source as \
         `$sourceDescriptions.<name>.{operation}`"
    )]
    Unproven {
        /// The `operationId` as written.
        operation: String,
        /// The description(s) that were not supplied.
        missing: String,
    },
    /// A source description was named but not supplied.
    #[error("source description `{0}` was not supplied — pass its document in the options")]
    MissingSource(String),
    /// An `operationPath` this crate cannot follow.
    #[error("`{path}` is not an operation path this crate can follow: {reason}")]
    BadPath {
        /// The `operationPath` as written.
        path: String,
        /// What was wrong with it.
        reason: String,
    },
    /// Nothing named a server, so there is nowhere to send the request.
    #[error("no server URL for operation `{0}` — pass a base URL for its source description")]
    NoServer(String),
    /// An AsyncAPI step, which this crate does not execute.
    #[error("step `{0}` is an AsyncAPI step, which this executor does not run")]
    Async(String),
    /// A step naming no operation at all.
    #[error("step `{0}` names neither an operation nor a workflow")]
    Nothing(String),
}

/// Test adapter for the original operation-resolution contract.
#[cfg(test)]
fn resolve(
    step: &roas_arazzo::v1_1::Step,
    sources: &std::collections::BTreeMap<String, Source>,
    base_urls: &std::collections::BTreeMap<String, String>,
    missing: &[String],
) -> Result<Endpoint, OperationError> {
    let mut options = crate::Options::default();
    options.sources = sources.clone();
    options.base_urls = base_urls.clone();
    crate::operation_index::Resolver::new(&options).resolve(step, missing)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use roas_arazzo::v1_1::Step;
    use serde_json::json;
    use std::collections::BTreeMap;

    pub(crate) fn petstore() -> Value {
        json!({
            "openapi": "3.0.3",
            "servers": [{ "url": "https://api.example.com/v1" }],
            "paths": {
                "/pets/{petId}": {
                    "get": { "operationId": "getPetById" },
                    "delete": { "operationId": "deletePet" }
                },
                "/orders": {
                    "post": {
                        "operationId": "placeOrder",
                        "servers": [{ "url": "https://orders.example.com" }]
                    }
                }
            }
        })
    }

    pub(crate) fn sources() -> BTreeMap<String, Source> {
        BTreeMap::from([(
            "petStore".to_owned(),
            Source {
                url: "https://api.example.com/openapi.json".to_owned(),
                data: SourceData::Owned(petstore()),
            },
        )])
    }

    fn step(id: &str, operation: Option<&str>, path: Option<&str>) -> Step {
        Step {
            step_id: id.to_owned(),
            operation_id: operation.map(ToOwned::to_owned),
            operation_path: path.map(ToOwned::to_owned),
            ..Step::default()
        }
    }

    fn find(step: &Step) -> Result<Endpoint, OperationError> {
        resolve(step, &sources(), &BTreeMap::new(), &[])
    }

    #[test]
    fn an_operation_id_finds_its_method_path_and_server() {
        assert_eq!(
            find(&step("s", Some("getPetById"), None)),
            Ok(Endpoint {
                method: "GET".to_owned(),
                path: "/pets/{petId}".to_owned(),
                base: "https://api.example.com/v1".to_owned(),
            })
        );
    }

    #[test]
    fn an_operations_own_server_wins_over_the_documents() {
        assert_eq!(
            find(&step("s", Some("placeOrder"), None)).map(|endpoint| endpoint.base),
            Ok("https://orders.example.com".to_owned())
        );
    }

    #[test]
    fn an_operation_id_may_name_the_source_it_is_in() {
        assert_eq!(
            find(&step(
                "s",
                Some("$sourceDescriptions.petStore.getPetById"),
                None
            ))
            .map(|endpoint| endpoint.path),
            Ok("/pets/{petId}".to_owned())
        );
        assert_eq!(
            find(&step(
                "s",
                Some("$sourceDescriptions.other.getPetById"),
                None
            )),
            Err(OperationError::MissingSource("other".to_owned()))
        );
    }

    #[test]
    fn an_id_in_two_descriptions_is_refused_rather_than_guessed() {
        let mut sources = sources();
        sources.insert(
            "mirror".to_owned(),
            Source {
                url: "https://mirror.example.com/openapi.json".to_owned(),
                data: SourceData::Owned(petstore()),
            },
        );
        let error = resolve(
            &step("s", Some("getPetById"), None),
            &sources,
            &BTreeMap::new(),
            &[],
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "operation `getPetById` is in more than one source description: mirror, petStore"
        );
    }

    #[test]
    fn an_unknown_operation_says_so() {
        assert_eq!(
            find(&step("s", Some("nope"), None)),
            Err(OperationError::Unknown {
                operation: "nope".to_owned()
            })
        );
    }

    #[test]
    fn an_operation_path_points_at_the_method_inside_the_document() {
        assert_eq!(
            find(&step(
                "s",
                None,
                Some("{$sourceDescriptions.petStore.url}#/paths/~1pets~1{petId}/get")
            )),
            Ok(Endpoint {
                method: "GET".to_owned(),
                path: "/pets/{petId}".to_owned(),
                base: "https://api.example.com/v1".to_owned(),
            })
        );
        // A literal URL, matched against the declared source.
        assert_eq!(
            find(&step(
                "s",
                None,
                Some("https://api.example.com/openapi.json#/paths/~1orders/post")
            ))
            .map(|endpoint| endpoint.method),
            Ok("POST".to_owned())
        );
    }

    #[test]
    fn an_operation_path_that_leads_nowhere_says_why() {
        for (path, reason) in [
            (
                "{$sourceDescriptions.petStore.url}/paths/~1pets/get",
                "it has no `#`",
            ),
            (
                "{$sourceDescriptions.petStore.url}#/paths/~1nope/get",
                "the document has nothing at that pointer",
            ),
            (
                // It resolves, but a path item is not an operation.
                "{$sourceDescriptions.petStore.url}#/paths/~1pets~1{petId}",
                "it does not point at `/paths/<path>/<method>`",
            ),
            (
                "https://elsewhere.example.com/openapi.json#/paths/~1pets/get",
                "no source description has that URL",
            ),
        ] {
            let error = find(&step("s", None, Some(path))).unwrap_err();
            assert!(
                error.to_string().contains(reason),
                "`{path}`: expected {reason:?}, got {error}"
            );
        }
    }

    #[test]
    fn a_base_url_from_the_caller_wins_over_the_document() {
        let base_urls =
            BTreeMap::from([("petStore".to_owned(), "http://127.0.0.1:8080/".to_owned())]);
        assert_eq!(
            resolve(
                &step("s", Some("getPetById"), None),
                &sources(),
                &base_urls,
                &[]
            )
            .map(|endpoint| endpoint.base),
            Ok("http://127.0.0.1:8080".to_owned())
        );
    }

    #[test]
    fn a_server_variable_is_filled_in_from_its_default() {
        let document = json!({
            "servers": [{
                "url": "https://{region}.example.com",
                "variables": { "region": { "default": "eu" } }
            }],
            "paths": { "/pets": { "get": { "operationId": "listPets" } } }
        });
        let sources = BTreeMap::from([(
            "petStore".to_owned(),
            Source {
                url: "https://api.example.com/openapi.json".to_owned(),
                data: SourceData::Owned(document),
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new(),
                &[],
            )
            .map(|endpoint| endpoint.base),
            Ok("https://eu.example.com".to_owned())
        );
    }

    #[test]
    fn a_swagger_document_says_its_server_in_three_fields() {
        let document = json!({
            "swagger": "2.0",
            "schemes": ["http"],
            "host": "api.example.com",
            "basePath": "/v2",
            "paths": { "/pets": { "get": { "operationId": "listPets" } } }
        });
        let sources = BTreeMap::from([(
            "petStore".to_owned(),
            Source {
                url: "https://api.example.com/swagger.json".to_owned(),
                data: SourceData::Owned(document),
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new(),
                &[],
            )
            .map(|endpoint| endpoint.base),
            Ok("http://api.example.com/v2".to_owned())
        );
    }

    #[test]
    fn a_document_naming_no_server_asks_for_a_base_url() {
        let sources = BTreeMap::from([(
            "petStore".to_owned(),
            Source {
                url: "u".to_owned(),
                data: SourceData::Owned(
                    json!({ "paths": { "/pets": { "get": { "operationId": "listPets" } } } }),
                ),
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new(),
                &[],
            ),
            Err(OperationError::NoServer("listPets".to_owned()))
        );
    }

    #[test]
    fn the_steps_this_crate_does_not_run_say_which_they_are() {
        let async_step = Step {
            step_id: "s".to_owned(),
            channel_path: Some("{$sourceDescriptions.events.url}#/channels/pets".to_owned()),
            ..Step::default()
        };
        assert_eq!(
            find(&async_step),
            Err(OperationError::Async("s".to_owned()))
        );
        assert_eq!(
            find(&step("s", None, None)),
            Err(OperationError::Nothing("s".to_owned()))
        );
    }
}
