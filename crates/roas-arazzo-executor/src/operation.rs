//! Finding the operation a step names, in the descriptions it points at.
//!
//! Source descriptions are read as plain JSON rather than through a
//! typed OpenAPI model: only a handful of fields matter here — the
//! method, the path template and a server — and reading them from JSON
//! serves OpenAPI 3.x and Swagger 2.0 with the same code, which is what
//! an Arazzo description is allowed to point at.

use roas_arazzo::v1_1::Step;
use serde_json::Value;
use std::collections::BTreeMap;

/// The HTTP methods a path item may hold, in the order a search sees
/// them.
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// A source description the run was given.
#[derive(Clone, Debug)]
pub(crate) struct Source {
    /// The URL the description was declared with.
    pub url: String,
    /// The parsed document.
    pub document: Value,
}

/// Where a step's request is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Endpoint {
    /// Upper-case HTTP method.
    pub method: String,
    /// The path template, `{parameters}` still in it.
    pub path: String,
    /// The server the path hangs off, without a trailing slash.
    pub base: String,
}

/// Why a step could not be turned into a request.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
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

/// Resolve the endpoint a step's request goes to.
pub(crate) fn resolve(
    step: &Step,
    sources: &BTreeMap<String, Source>,
    base_urls: &BTreeMap<String, String>,
) -> Result<Endpoint, OperationError> {
    if step.channel_path.is_some() || step.action.is_some() {
        return Err(OperationError::Async(step.step_id.clone()));
    }
    if let Some(operation) = &step.operation_id {
        let (source, found) = by_id(operation, sources)?;
        return endpoint(source, &found, base_urls, operation);
    }
    if let Some(path) = &step.operation_path {
        let (source, found) = by_path(path, sources)?;
        return endpoint(source, &found, base_urls, path);
    }
    Err(OperationError::Nothing(step.step_id.clone()))
}

/// Where an operation sits inside a document.
struct Found {
    /// The source description's name.
    name: String,
    /// The path template.
    path: String,
    /// The lower-case method key.
    method: String,
}

/// Find an operation by `operationId`, either bare or written as
/// `$sourceDescriptions.<name>.<operationId>`.
fn by_id<'s>(
    operation: &str,
    sources: &'s BTreeMap<String, Source>,
) -> Result<(&'s Source, Found), OperationError> {
    if let Some(rest) = operation.strip_prefix("$sourceDescriptions.") {
        let (name, id) = rest
            .split_once('.')
            .ok_or_else(|| OperationError::Unknown {
                operation: operation.to_owned(),
            })?;
        let source = sources
            .get(name)
            .ok_or_else(|| OperationError::MissingSource(name.to_owned()))?;
        let found = search(&source.document, id).ok_or_else(|| OperationError::Unknown {
            operation: operation.to_owned(),
        })?;
        return Ok((
            source,
            Found {
                name: name.to_owned(),
                ..found
            },
        ));
    }

    // A bare id must be unique across the descriptions — the spec says
    // so, and if it is not, guessing would send the request somewhere
    // the author did not name.
    let mut hits = sources.iter().filter_map(|(name, source)| {
        search(&source.document, operation).map(|found| {
            (
                source,
                Found {
                    name: name.clone(),
                    ..found
                },
            )
        })
    });
    let first = hits.next().ok_or_else(|| OperationError::Unknown {
        operation: operation.to_owned(),
    })?;
    let rest: Vec<String> = hits.map(|(_, found)| found.name).collect();
    if rest.is_empty() {
        Ok(first)
    } else {
        let mut names = vec![first.1.name.clone()];
        names.extend(rest);
        Err(OperationError::Ambiguous {
            operation: operation.to_owned(),
            sources: names.join(", "),
        })
    }
}

/// Find the operation an `operationPath` points at: a source URL, then a
/// JSON Pointer into that document.
fn by_path<'s>(
    path: &str,
    sources: &'s BTreeMap<String, Source>,
) -> Result<(&'s Source, Found), OperationError> {
    let bad = |reason: &str| OperationError::BadPath {
        path: path.to_owned(),
        reason: reason.to_owned(),
    };
    let (document, pointer) = path
        .split_once('#')
        .ok_or_else(|| bad("it has no `#` and so names no operation inside the document"))?;

    // The document half is usually `{$sourceDescriptions.<name>.url}`;
    // a literal URL matching a declared one works too.
    let name = match document
        .trim()
        .strip_prefix("{$sourceDescriptions.")
        .and_then(|rest| rest.strip_suffix(".url}"))
    {
        Some(name) => name.to_owned(),
        None => sources
            .iter()
            .find(|(_, source)| source.url == document)
            .map(|(name, _)| name.clone())
            .ok_or_else(|| bad("no source description has that URL"))?,
    };
    let source = sources
        .get(&name)
        .ok_or_else(|| OperationError::MissingSource(name.clone()))?;

    if source.document.pointer(pointer).is_none() {
        return Err(bad("the document has nothing at that pointer"));
    }
    // `/paths/~1pets~1{petId}/get` — the pointer itself says which path
    // and which method.
    let tokens: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect();
    let [paths, template, method] = tokens.as_slice() else {
        return Err(bad("it does not point at `/paths/<path>/<method>`"));
    };
    if paths != "paths" || !METHODS.contains(&method.as_str()) {
        return Err(bad("it does not point at `/paths/<path>/<method>`"));
    }
    Ok((
        source,
        Found {
            name,
            path: template.clone(),
            method: method.clone(),
        },
    ))
}

/// Search a document's `paths` for an operation id.
fn search(document: &Value, operation: &str) -> Option<Found> {
    let paths = document.get("paths")?.as_object()?;
    for (path, item) in paths {
        for method in METHODS {
            let Some(candidate) = item.get(method) else {
                continue;
            };
            if candidate.get("operationId").and_then(Value::as_str) == Some(operation) {
                return Some(Found {
                    name: String::new(),
                    path: path.clone(),
                    method: method.to_owned(),
                });
            }
        }
    }
    None
}

/// The endpoint for an operation that has been found.
fn endpoint(
    source: &Source,
    found: &Found,
    base_urls: &BTreeMap<String, String>,
    named: &str,
) -> Result<Endpoint, OperationError> {
    let base = base_urls
        .get(&found.name)
        .cloned()
        .or_else(|| server(&source.document, &found.path, &found.method))
        .ok_or_else(|| OperationError::NoServer(named.to_owned()))?;
    Ok(Endpoint {
        method: found.method.to_uppercase(),
        path: found.path.clone(),
        base: base.trim_end_matches('/').to_owned(),
    })
}

/// The server an operation hangs off: the operation's own, else the path
/// item's, else the document's — or, for Swagger 2.0, the scheme, host
/// and base path it was written with.
fn server(document: &Value, path: &str, method: &str) -> Option<String> {
    let item = document.get("paths").and_then(|paths| paths.get(path));
    let operation = item.and_then(|item| item.get(method));
    let first = |value: Option<&Value>| -> Option<String> {
        let servers = value?.get("servers")?.as_array()?;
        let server = servers.first()?;
        let url = server.get("url")?.as_str()?;
        Some(with_variables(url, server.get("variables")))
    };
    if let Some(url) = first(operation)
        .or_else(|| first(item))
        .or_else(|| first(Some(document)))
    {
        return Some(url);
    }

    // Swagger 2.0 spells the same thing in three fields.
    let host = document.get("host").and_then(Value::as_str)?;
    let scheme = document
        .get("schemes")
        .and_then(Value::as_array)
        .and_then(|schemes| schemes.first())
        .and_then(Value::as_str)
        .unwrap_or("https");
    let base = document
        .get("basePath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(format!("{scheme}://{host}{base}"))
}

/// A server URL with its variables replaced by their defaults.
fn with_variables(url: &str, variables: Option<&Value>) -> String {
    let Some(variables) = variables.and_then(Value::as_object) else {
        return url.to_owned();
    };
    let mut url = url.to_owned();
    for (name, variable) in variables {
        if let Some(default) = variable.get("default").and_then(Value::as_str) {
            url = url.replace(&format!("{{{name}}}"), default);
        }
    }
    url
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

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
                document: petstore(),
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
        resolve(step, &sources(), &BTreeMap::new())
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
                document: petstore(),
            },
        );
        let error = resolve(
            &step("s", Some("getPetById"), None),
            &sources,
            &BTreeMap::new(),
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
            resolve(&step("s", Some("getPetById"), None), &sources(), &base_urls)
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
                document,
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new()
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
                document,
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new()
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
                document: json!({ "paths": { "/pets": { "get": { "operationId": "listPets" } } } }),
            },
        )]);
        assert_eq!(
            resolve(
                &step("s", Some("listPets"), None),
                &sources,
                &BTreeMap::new()
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
