//! The validator itself: a description, prepared once, judging many
//! requests.

use std::collections::BTreeMap;

use roas::common::reference::ResolveReference;
use roas::v3_2::operation::Operation;
use roas::v3_2::parameter::Parameter;
use roas::v3_2::path_item::PathItem;
use roas::v3_2::spec::Spec;

use crate::body;
use crate::parameter;
use crate::report::{ErrorKind, Location, RoutingError, ValidationError, ValidationReport};
use crate::request::RequestView;
use crate::router::Router;

/// What to check, and where the description's paths start.
///
/// ```
/// use roas_http_validator::Options;
///
/// let options = Options::new().base_path("/api/v1").reject_undescribed_query_parameters();
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    base_path: Option<String>,
    skip_body: bool,
    reject_undescribed_query_parameters: bool,
}

impl Options {
    /// Everything checked, base path taken from the Server Objects.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The prefix a request path carries before the description's own
    /// paths begin, overriding whatever `servers` implies.
    #[must_use]
    pub fn base_path(mut self, base_path: impl Into<String>) -> Self {
        self.base_path = Some(base_path.into());
        self
    }

    /// Leave the body alone. Useful in a middleware that would rather
    /// not buffer one, and in a client-side check of a request that has
    /// not been serialized yet.
    #[must_use]
    pub fn skip_body(mut self) -> Self {
        self.skip_body = true;
        self
    }

    /// Report a query parameter the operation does not describe.
    ///
    /// Off by default: OpenAPI does not forbid undescribed query
    /// parameters, and plenty of real clients send tracking parameters
    /// that no description mentions. On, it catches the typo in
    /// `?limti=10` that would otherwise silently do nothing.
    #[must_use]
    pub fn reject_undescribed_query_parameters(mut self) -> Self {
        self.reject_undescribed_query_parameters = true;
        self
    }
}

/// One OpenAPI description, ready to judge requests against.
///
/// Building one walks the description's paths once; validating is then
/// a match and a handful of schema checks, so a server builds this at
/// startup and keeps it.
///
/// ```
/// use roas_http_validator::{RequestView, Validator};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let spec = serde_json::from_str(r#"{
///   "openapi": "3.2.0",
///   "info": { "title": "Pets", "version": "1.0.0" },
///   "paths": {
///     "/pets/{petId}": {
///       "get": {
///         "operationId": "getPet",
///         "parameters": [
///           { "name": "petId", "in": "path", "required": true,
///             "schema": { "type": "integer" } }
///         ]
///       }
///     }
///   }
/// }"#)?;
///
/// let validator = Validator::new(spec);
/// assert!(validator.validate(&RequestView::new("GET", "/pets/7"))?.is_valid());
/// assert!(!validator.validate(&RequestView::new("GET", "/pets/rex"))?.is_valid());
/// # Ok(()) }
/// ```
#[derive(Clone, Debug)]
pub struct Validator {
    spec: Spec,
    router: Router,
    options: Options,
}

impl Validator {
    /// Prepare a v3.2 description with the default [`Options`].
    #[must_use]
    pub fn new(spec: Spec) -> Self {
        Self::with_options(spec, Options::new())
    }

    /// Prepare a v3.2 description.
    #[must_use]
    pub fn with_options(spec: Spec, options: Options) -> Self {
        let router = match &spec.paths {
            Some(paths) => {
                Router::new(paths, spec.servers.as_deref(), options.base_path.as_deref())
            }
            None => Router::default(),
        };
        Self {
            spec,
            router,
            options,
        }
    }

    /// The description being validated against.
    #[must_use]
    pub fn spec(&self) -> &Spec {
        &self.spec
    }

    /// Judge one request.
    ///
    /// # Errors
    ///
    /// [`RoutingError`] when the description describes no such path or
    /// no such method on it — which is a different answer from "the
    /// request is invalid", and usually a different response code.
    pub fn validate(&self, request: &RequestView<'_>) -> Result<ValidationReport, RoutingError> {
        let matched =
            self.router
                .route(&request.path)
                .ok_or_else(|| RoutingError::PathNotFound {
                    path: request.path.clone().into_owned(),
                })?;
        let template = matched.template.to_owned();
        let path_parameters = matched.parameters;

        let path_item = self.path_item(&template);
        let method = request.method.to_ascii_lowercase();
        let operations = path_item.and_then(|item| item.operations.as_ref());
        let operation = operations.and_then(|operations| operations.get(&method));

        let Some(operation) = operation else {
            return Err(RoutingError::MethodNotAllowed {
                template,
                method: request.method.to_uppercase(),
                allowed: operations
                    .map(|operations| operations.keys().cloned().collect())
                    .unwrap_or_default(),
            });
        };

        let mut errors = Vec::new();
        let parameters = self.parameters(path_item, operation, &mut errors);
        // Decoded once for the whole operation rather than per parameter.
        let extracted = parameter::Extracted::new(request, &path_parameters);

        for parameter in &parameters {
            parameter::validate(parameter, request, &extracted, &self.spec, &mut errors);
        }

        if self.options.reject_undescribed_query_parameters {
            check_for_strays(&extracted, &parameters, &mut errors);
        }

        if !self.options.skip_body
            && let Some(request_body) = &operation.request_body
        {
            match request_body.get_item(&self.spec) {
                Ok(request_body) => {
                    body::validate(request_body, request, &self.spec, &mut errors);
                }
                Err(error) => errors.push(ValidationError {
                    location: Location::Body,
                    name: String::new(),
                    kind: ErrorKind::UnresolvedReference(error.to_string()),
                }),
            }
        }

        Ok(ValidationReport {
            template,
            method,
            operation_id: operation.operation_id.clone(),
            path_parameters: path_parameters.into_iter().collect(),
            errors,
        })
    }

    /// The Path Item Object for a template, following a `$ref` when the
    /// entry is one.
    fn path_item(&self, template: &str) -> Option<&PathItem> {
        let item = self.spec.paths.as_ref()?.paths.get(template)?;
        // A Path Item Object may itself be a reference. Adjacent fields
        // are implementation-defined; what is local wins here, and the
        // reference fills in only when there is nothing local. Resolving
        // through the spec rather than through a temporary `RefOr` keeps
        // the borrow tied to `self`.
        if item.operations.is_none()
            && let Some(reference) = &item.reference
            && reference.starts_with("#/")
            && let Some(resolved) =
                ResolveReference::<PathItem>::resolve_reference(&self.spec, reference)
        {
            return Some(resolved);
        }
        Some(item)
    }

    /// The parameters that apply to one operation: the Path Item
    /// Object's, overridden by the Operation Object's where both name
    /// the same `name` and `in`.
    fn parameters(
        &self,
        path_item: Option<&PathItem>,
        operation: &Operation,
        errors: &mut Vec<ValidationError>,
    ) -> Vec<Parameter> {
        let mut merged: BTreeMap<(String, Location), Parameter> = BTreeMap::new();
        let inherited = path_item.and_then(|item| item.parameters.as_deref());
        let declared = operation.parameters.as_deref();

        for source in [inherited, declared].into_iter().flatten() {
            for parameter in source {
                match parameter.get_item(&self.spec) {
                    Ok(parameter) => {
                        merged.insert(identity(parameter), parameter.clone());
                    }
                    // The parameter cannot be read, so it cannot be
                    // checked — which is the description's fault, not
                    // the request's, and says so.
                    Err(error) => errors.push(ValidationError {
                        location: Location::Description,
                        name: String::new(),
                        kind: ErrorKind::UnresolvedReference(error.to_string()),
                    }),
                }
            }
        }
        merged.into_values().collect()
    }
}

/// Report query parameters the operation says nothing about.
fn check_for_strays(
    extracted: &parameter::Extracted<'_>,
    parameters: &[Parameter],
    errors: &mut Vec<ValidationError>,
) {
    // `in: querystring` describes the query string whole, so there is no
    // such thing as a stray parameter alongside one.
    if parameters
        .iter()
        .any(|parameter| matches!(parameter, Parameter::Querystring(_)))
    {
        return;
    }
    let described: Vec<&str> = parameters
        .iter()
        .filter_map(|parameter| match parameter {
            Parameter::Query(query) => Some(query.name.as_str()),
            _ => None,
        })
        .collect();
    for (name, _) in &extracted.query {
        // A `deepObject` parameter arrives as `id[role]`, so a name that
        // opens a bracket belongs to whatever precedes it.
        let base = name.split_once('[').map_or(name.as_str(), |(base, _)| base);
        if !described.contains(&base) {
            errors.push(ValidationError {
                location: Location::Query,
                name: name.clone(),
                kind: ErrorKind::Undescribed,
            });
        }
    }
}

/// What makes a parameter unique: its name and its location.
fn identity(parameter: &Parameter) -> (String, Location) {
    match parameter {
        Parameter::Path(path) => (path.name.clone(), Location::Path),
        Parameter::Query(query) => (query.name.clone(), Location::Query),
        Parameter::Querystring(querystring) => (querystring.name.clone(), Location::Querystring),
        Parameter::Header(header) => (header.name.clone(), Location::Header),
        Parameter::Cookie(cookie) => (cookie.name.clone(), Location::Cookie),
    }
}
