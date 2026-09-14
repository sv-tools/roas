//! Borrowed operation views: mounts and reference/server origins stay distinct.

use crate::Options;
use crate::operation::{Endpoint, OperationError, Source};
use crate::operation_document::{
    Document, Version, decode_pointer, equivalent, escape, source_name,
};
use roas_arazzo::v1_1::Step;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

#[derive(Clone)]
struct Field<'a> {
    value: &'a Value,
    document: Document<'a>,
    pointer: String,
}

struct Operation<'a> {
    method: String,
    path: String,
    pointer: String,
    field: Field<'a>,
    servers: Option<Field<'a>>,
}

struct Index<'a> {
    document: Document<'a>,
    operations: Vec<Operation<'a>>,
    ids: BTreeMap<&'a str, usize>,
}

pub(crate) struct Resolver<'a> {
    options: &'a Options,
    indexes: BTreeMap<String, Result<Index<'a>, OperationError>>,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(options: &'a Options) -> Self {
        Self {
            options,
            indexes: BTreeMap::new(),
        }
    }

    fn document(&self, source: &'a Source) -> Result<Document<'a>, OperationError> {
        #[cfg(feature = "source-graph")]
        if let crate::operation::SourceData::Registry(document) = &source.data {
            return Document::new(
                document.value(),
                Some(document.retrieval_uri().clone()),
                document.identity().to_string(),
                Version::Legacy,
            );
        }
        Document::new(
            source.document(),
            Url::parse(&source.url).ok(),
            source.url.clone(),
            Version::Legacy,
        )
    }

    fn lookup(
        &self,
        uri: &Url,
        inherited: Version,
    ) -> Result<Option<Document<'a>>, OperationError> {
        #[cfg(feature = "source-graph")]
        if let Some(context) = &self.options.registry
            && let Some((value, retrieval)) = context.registry.operation_document(uri)
        {
            return Document::new(value, Some(retrieval.clone()), uri.to_string(), inherited)
                .map(Some);
        }
        let mut result = None;
        for source in self.options.sources.values() {
            let Ok(mut document) = self.document(source) else {
                continue;
            };
            if document.version == Version::Legacy {
                document.version = inherited;
            }
            if document.base.as_ref() == Some(uri) || document.retrieval.as_ref() == Some(uri) {
                if result
                    .as_ref()
                    .is_some_and(|old: &Document<'_>| old.value != document.value)
                {
                    return Err(document.error(
                        "",
                        uri.as_str(),
                        "multiple supplied documents match this URI",
                    ));
                }
                result = Some(document);
            }
        }
        Ok(result)
    }

    fn path_item(
        &self,
        mut document: Document<'a>,
        mut pointer: String,
    ) -> Result<BTreeMap<&'a str, Field<'a>>, OperationError> {
        let mut active = BTreeSet::new();
        let mut fields = BTreeMap::new();
        loop {
            if !active.insert((document.key().to_owned(), pointer.clone())) {
                return Err(document.error(&pointer, "", "circular Path Item reference"));
            }
            let item = document
                .value
                .pointer(&pointer)
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    document.error(
                        &pointer,
                        "",
                        "Path Item target is missing or is not an object",
                    )
                })?;
            for (name, value) in item {
                if name == "$ref" {
                    continue;
                }
                if fields
                    .insert(
                        name.as_str(),
                        Field {
                            value,
                            document: document.clone(),
                            pointer: format!("{pointer}/{}", escape(name)),
                        },
                    )
                    .is_some()
                {
                    return Err(document.error(&pointer, "", format!("overlapping Path Item field `{name}` across $ref; merge behavior is undefined")));
                }
            }
            let Some(reference) = item.get("$ref") else {
                break;
            };
            let at = format!("{pointer}/$ref");
            let reference = reference
                .as_str()
                .ok_or_else(|| document.error(&at, "", "$ref must be a URI string"))?;
            let (target, target_pointer) = document.reference(reference, &at)?;
            if let Some(target) = target
                && document.base.as_ref() != Some(&target)
            {
                document = self.lookup(&target, document.version)?.ok_or_else(|| document.error(&at, reference, format!("document `{target}` was not supplied; load its operation references before preparation")))?;
            }
            pointer = target_pointer;
        }
        Ok(fields)
    }

    fn build(&self, name: &str) -> Result<Index<'a>, OperationError> {
        let source = self
            .options
            .sources
            .get(name)
            .ok_or_else(|| OperationError::MissingSource(name.into()))?;
        let document = self.document(source)?;
        let mut index = Index {
            document: document.clone(),
            operations: Vec::new(),
            ids: BTreeMap::new(),
        };
        let Some(paths) = document.value.get("paths") else {
            return Ok(index);
        };
        let paths = paths
            .as_object()
            .ok_or_else(|| document.error("/paths", "", "paths must be an object"))?;
        for (path, _) in paths {
            if path.starts_with("x-") {
                continue;
            }
            if !path.starts_with('/') {
                return Err(document.error("/paths", "", "path templates must begin with /"));
            }
            let pointer = format!("/paths/{}", escape(path));
            let fields = self.path_item(document.clone(), pointer.clone())?;
            let mut methods = METHODS
                .iter()
                .filter_map(|method| {
                    if *method == "trace" && document.version == Version::Swagger {
                        return None;
                    }
                    fields.get(method).map(|field| {
                        (
                            method.to_uppercase(),
                            format!("{pointer}/{method}"),
                            field.clone(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            if document.version == Version::V3_2 {
                if let Some(field) = fields.get("query") {
                    methods.push(("QUERY".into(), format!("{pointer}/query"), field.clone()));
                }
                if let Some(extra) = fields.get("additionalOperations") {
                    let extra_methods = extra.value.as_object().ok_or_else(|| {
                        extra.document.error(
                            &extra.pointer,
                            "",
                            "additionalOperations must be an object",
                        )
                    })?;
                    for (method, value) in extra_methods {
                        if method.is_empty()
                            || !method.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
                            })
                        {
                            return Err(extra.document.error(
                                &extra.pointer,
                                "",
                                "invalid HTTP method token",
                            ));
                        }
                        if method == "QUERY"
                            || METHODS.iter().any(|fixed| fixed.to_uppercase() == *method)
                        {
                            return Err(extra.document.error(
                                &extra.pointer,
                                "",
                                "HTTP method must use its fixed Path Item field",
                            ));
                        }
                        methods.push((
                            method.clone(),
                            format!("{pointer}/additionalOperations/{}", escape(method)),
                            Field {
                                value,
                                document: extra.document.clone(),
                                pointer: format!("{}/{}", extra.pointer, escape(method)),
                            },
                        ));
                    }
                }
            }
            for (method, mounted_pointer, field) in methods {
                if !field.value.is_object() {
                    return Err(field.document.error(
                        &field.pointer,
                        "",
                        "operation must be an object",
                    ));
                }
                if let Some(id) = field.value.get("operationId") {
                    let id = id.as_str().ok_or_else(|| {
                        field
                            .document
                            .error(&field.pointer, "", "operationId must be a string")
                    })?;
                    if let Some(previous) = index.ids.insert(id, index.operations.len()) {
                        return Err(OperationError::Duplicate {
                            operation: id.into(),
                            source_name: name.into(),
                            locations: format!(
                                "{}, {mounted_pointer}",
                                index.operations[previous].pointer
                            ),
                        });
                    }
                }
                index.operations.push(Operation {
                    method,
                    path: path.clone(),
                    pointer: mounted_pointer,
                    field,
                    servers: fields.get("servers").cloned(),
                });
            }
        }
        Ok(index)
    }

    fn index(&mut self, name: &str) -> Result<&Index<'a>, OperationError> {
        if !self.indexes.contains_key(name) {
            self.indexes.insert(name.into(), self.build(name));
        }
        self.indexes
            .get(name)
            .expect("index inserted")
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(crate) fn resolve(
        &mut self,
        step: &Step,
        missing: &[String],
    ) -> Result<Endpoint, OperationError> {
        if step.channel_path.is_some() || step.action.is_some() {
            return Err(OperationError::Async(step.step_id.clone()));
        }
        let (name, selected, named) = if let Some(id) = &step.operation_id {
            if let Some(rest) = id.strip_prefix("$sourceDescriptions.") {
                let (name, operation) =
                    rest.split_once('.')
                        .ok_or_else(|| OperationError::Unknown {
                            operation: id.clone(),
                        })?;
                let index = self.index(name)?;
                let position =
                    index
                        .ids
                        .get(operation)
                        .copied()
                        .ok_or_else(|| OperationError::Unknown {
                            operation: id.clone(),
                        })?;
                (name.to_owned(), position, id)
            } else {
                let mut hits = Vec::new();
                for name in self.options.sources.keys() {
                    let index = self.index(name)?;
                    if let Some(position) = index.ids.get(id.as_str()) {
                        hits.push((name.clone(), *position));
                    }
                }
                if hits.len() > 1 {
                    return Err(OperationError::Ambiguous {
                        operation: id.clone(),
                        sources: hits
                            .iter()
                            .map(|(name, _)| name.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
                if !missing.is_empty() {
                    return Err(OperationError::Unproven {
                        operation: id.clone(),
                        missing: missing.join(", "),
                    });
                }
                let (name, position) = hits.pop().ok_or_else(|| OperationError::Unknown {
                    operation: id.clone(),
                })?;
                (name, position, id)
            }
        } else if let Some(path) = &step.operation_path {
            let bad = |reason: String| OperationError::BadPath {
                path: path.clone(),
                reason,
            };
            let (document, fragment) = path.split_once('#').ok_or_else(|| {
                bad("it has no `#` and so names no operation inside the document".into())
            })?;
            let name = if let Some(name) = source_name(document) {
                name.to_owned()
            } else {
                let names = self.matching_sources(document);
                if names.is_empty() {
                    return Err(bad("no source description has that URL".into()));
                }
                if names.len() > 1 {
                    return Err(bad("more than one source description has that URL".into()));
                }
                names[0].clone()
            };
            let pointer = decode_pointer(fragment).map_err(bad)?;
            let index = self.index(&name)?;
            let positions = index
                .operations
                .iter()
                .enumerate()
                .filter(|(_, operation)| {
                    operation.pointer == pointer
                        || (operation.field.pointer == pointer
                            && operation.field.document.key() == index.document.key())
                })
                .map(|(position, _)| position)
                .collect::<Vec<_>>();
            if positions.len() > 1 {
                return Err(bad("the operation is mounted at more than one path; point at a specific /paths entry".into()));
            }
            let position = positions.first().copied().ok_or_else(|| {
                if index.document.value.pointer(&pointer).is_some() {
                    bad("it does not point at `/paths/<path>/<method>` (or a 3.2 additional operation)".into())
                } else { bad("the document has nothing at that pointer in its resolved paths".into()) }
            })?;
            (name, position, path)
        } else {
            return Err(OperationError::Nothing(step.step_id.clone()));
        };
        let options = self.options;
        let index = self.index(&name)?;
        endpoint(
            &index.document,
            &index.operations[selected],
            options.base_urls.get(&name),
            named,
        )
    }

    pub(crate) fn matching_sources(&self, reference: &str) -> Vec<String> {
        #[cfg(feature = "source-graph")]
        let base = self
            .options
            .registry
            .as_ref()
            .and_then(|context| context.registry.document(context.owner).ok())
            .map(|document| document.base_uri());
        #[cfg(not(feature = "source-graph"))]
        let base = None;
        let mut names = Vec::new();
        for (name, source) in &self.options.sources {
            if equivalent(&source.url, reference, base) {
                names.push(name.clone());
                continue;
            }
            let Ok(document) = self.document(source) else {
                continue;
            };
            if document
                .base
                .as_ref()
                .is_some_and(|uri| equivalent(uri.as_str(), reference, base))
                || document
                    .retrieval
                    .as_ref()
                    .is_some_and(|uri| equivalent(uri.as_str(), reference, base))
            {
                names.push(name.clone());
            }
        }
        names
    }

    pub(crate) fn declared_source_matches(
        &self,
        name: &str,
        declared: &str,
        reference: &str,
        base: Option<&Url>,
    ) -> bool {
        if equivalent(declared, reference, base) {
            return true;
        }
        #[cfg(feature = "source-graph")]
        if let Some(context) = &self.options.registry {
            let target = context
                .registry
                .overrides
                .get(&(context.owner, name.into()))
                .copied()
                .or_else(|| {
                    context
                        .registry
                        .resolve(context.owner, declared, true)
                        .ok()
                        .flatten()
                });
            if let Some(target) = target
                && let Ok(document) = context.registry.document(target)
            {
                return equivalent(document.identity().as_str(), reference, base)
                    || equivalent(document.retrieval_uri().as_str(), reference, base);
            }
        }
        #[cfg(not(feature = "source-graph"))]
        let _ = name;
        false
    }
}

fn endpoint(
    document: &Document<'_>,
    operation: &Operation<'_>,
    override_url: Option<&String>,
    named: &str,
) -> Result<Endpoint, OperationError> {
    let base = if let Some(url) = override_url {
        Url::parse(url).map_err(|error| {
            invalid_server(
                named,
                format!("base URL override `{url}` is not a URL: {error}"),
            )
        })?;
        absolute_server(url, document, named)?
    } else if document.version == Version::Swagger {
        swagger_server(document, operation, named)?
    } else {
        let candidates = [
            operation
                .field
                .value
                .get("servers")
                .map(|value| (value, &operation.field.document)),
            operation
                .servers
                .as_ref()
                .map(|field| (field.value, &field.document)),
            document.value.get("servers").map(|value| (value, document)),
        ];
        let mut selected = None;
        for (value, context) in candidates.into_iter().flatten() {
            let servers = value
                .as_array()
                .ok_or_else(|| invalid_server(named, "servers must be an array"))?;
            if let Some(server) = servers.first() {
                let url = server
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid_server(named, "server.url must be a string"))?;
                selected = Some(absolute_server(
                    &with_variables(url, server.get("variables"), named)?,
                    context,
                    named,
                )?);
                break;
            }
        }
        match selected {
            Some(server) => server,
            None if document.version != Version::Legacy => absolute_server("/", document, named)?,
            // Unversioned Options::source documents retain the historical profile.
            None if document.value.get("host").is_some() => {
                swagger_server(document, operation, named)?
            }
            None => return Err(OperationError::NoServer(named.into())),
        }
    };
    Ok(Endpoint {
        method: operation.method.clone(),
        path: operation.path.clone(),
        base: base.trim_end_matches('/').into(),
    })
}

fn invalid_server(operation: &str, reason: impl Into<String>) -> OperationError {
    OperationError::Server {
        operation: operation.into(),
        reason: reason.into(),
    }
}

fn absolute_server(
    written: &str,
    document: &Document<'_>,
    named: &str,
) -> Result<String, OperationError> {
    let url = match Url::parse(written) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => document
            .retrieval
            .as_ref()
            .ok_or_else(|| {
                invalid_server(
                    named,
                    "relative server URL has no retrieval base; pass an absolute base URL override",
                )
            })?
            .join(written)
            .map_err(|error| invalid_server(named, error.to_string()))?,
        Err(error) => return Err(invalid_server(named, error.to_string())),
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid_server(
            named,
            "server requires an HTTP(S) origin; pass an absolute base URL override",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_server(
            named,
            "server URLs with query strings or fragments are not supported",
        ));
    }
    Ok(url.to_string())
}

fn swagger_server(
    document: &Document<'_>,
    operation: &Operation<'_>,
    named: &str,
) -> Result<String, OperationError> {
    let retrieval = document
        .retrieval
        .as_ref()
        .filter(|uri| matches!(uri.scheme(), "http" | "https"));
    let scheme = operation
        .field
        .value
        .get("schemes")
        .or_else(|| document.value.get("schemes"))
        .and_then(Value::as_array)
        .and_then(|schemes| schemes.first())
        .and_then(Value::as_str)
        .or_else(|| retrieval.map(Url::scheme))
        .unwrap_or("https");
    let host = document
        .value
        .get("host")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            retrieval.map(|uri| uri[url::Position::BeforeHost..url::Position::AfterPort].to_owned())
        })
        .ok_or_else(|| OperationError::NoServer(named.into()))?;
    let base = document
        .value
        .get("basePath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    absolute_server(&format!("{scheme}://{host}{base}"), document, named)
}

fn with_variables(
    url: &str,
    variables: Option<&Value>,
    named: &str,
) -> Result<String, OperationError> {
    let mut result = String::new();
    let mut rest = url;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start + 1..];
        let end = rest
            .find('}')
            .ok_or_else(|| invalid_server(named, "unclosed server variable"))?;
        let name = &rest[..end];
        let value = variables
            .and_then(|variables| variables.get(name))
            .and_then(|variable| variable.get("default"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                invalid_server(
                    named,
                    format!("server variable `{name}` has no string default"),
                )
            })?;
        result.push_str(value);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    if result.contains(['{', '}']) {
        return Err(invalid_server(named, "unresolved server variable"));
    }
    Ok(result)
}
