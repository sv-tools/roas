//! The validator itself: a description, prepared once, judging many
//! requests.

use std::collections::BTreeMap;

use roas::v3_2::operation::Operation;
use roas::v3_2::parameter::Parameter;
use roas::v3_2::path_item::PathItem;
use roas::v3_2::spec::Spec;

use crate::body;
use crate::parameter;
use crate::paths;
use crate::report::{ErrorKind, Location, RoutingError, ValidationError, ValidationReport};
use crate::request::{RequestView, decode_path_segment};
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
    /// Every Path Item Object with its `$ref` followed and merged,
    /// resolved once here rather than on every request.
    path_items: BTreeMap<String, PathItem>,
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
        let path_items = paths::resolve(&spec);
        let router = Router::new(
            &path_items,
            spec.servers.as_deref(),
            options.base_path.as_deref(),
        );
        Self {
            spec,
            path_items,
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
        let matched = self
            .router
            .route(&request.path, &request.method)
            .ok_or_else(|| RoutingError::PathNotFound {
                path: request.path.clone().into_owned(),
            })?;
        let template = matched.template.to_owned();
        let path_parameters = matched.parameters;

        let path_item = self.path_item(&template);
        let Some((method, operation)) = path_item.and_then(|item| self.operation(item, request))
        else {
            return Err(RoutingError::MethodNotAllowed {
                template,
                // The token the request actually carried, not a
                // normalization of it: `get` was refused *because* it is
                // not `GET`, and saying "no GET here" beside an `Allow`
                // naming `GET` would be nonsense.
                method: request.method.clone().into_owned(),
                allowed: path_item.map(allowed_methods).unwrap_or_default(),
            });
        };

        let mut errors = Vec::new();
        // A `$ref` chain that could not be followed leaves part of this
        // Path Item Object unread — whatever it held went unapplied, so
        // it is reported rather than passed over.
        if let Some(reference) = path_item.and_then(|item| item.reference.as_ref()) {
            errors.push(ValidationError {
                location: Location::Description,
                name: String::new(),
                pointer: String::new(),
                kind: ErrorKind::UnresolvedReference(reference.clone()),
            });
        }
        let parameters = self.parameters(path_item, operation, &mut errors);
        // Decoded once for the whole operation rather than per parameter.
        let extracted = parameter::Extracted::new(request, &path_parameters);

        for parameter in &parameters {
            parameter::validate(parameter, request, &extracted, &self.spec, &mut errors);
        }

        if self.options.reject_undescribed_query_parameters {
            check_for_strays(&extracted, &parameters, &self.spec, &mut errors);
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
                    pointer: String::new(),
                    kind: ErrorKind::UnresolvedReference(error.to_string()),
                }),
            }
        }

        Ok(ValidationReport {
            template,
            method,
            operation_id: operation.operation_id.clone(),
            // Decoded here and only here: validation splits before it
            // decodes, but a report is for a reader.
            path_parameters: path_parameters
                .iter()
                .map(|(name, raw)| (name.clone(), decode_path_segment(raw)))
                .collect(),
            errors,
        })
    }

    /// The operation a request's method names, and the key the Path
    /// Item Object files it under.
    ///
    /// See [`crate::method`] for why `get` does not find `get`.
    fn operation<'i>(
        &self,
        path_item: &'i PathItem,
        request: &RequestView<'_>,
    ) -> Option<(String, &'i Operation)> {
        // Each map is searched with its own key and never the other's.
        if let Some(key) = crate::method::standard(&request.method)
            && let Some((key, operation)) = path_item
                .operations
                .as_ref()
                .and_then(|operations| operations.get_key_value(&key))
        {
            return Some((crate::method::from_standard_key(key), operation));
        }
        path_item
            .additional_operations
            .as_ref()?
            .get_key_value(request.method.as_ref())
            // Already a method token: `additionalOperations` is keyed by
            // the method itself.
            .map(|(key, operation)| (key.clone(), operation))
    }

    /// The Path Item Object for a template, already resolved.
    fn path_item(&self, template: &str) -> Option<&PathItem> {
        self.path_items.get(template)
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
                        pointer: String::new(),
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
    spec: &Spec,
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
    for (name, _) in &extracted.query {
        if !parameters
            .iter()
            .any(|parameter| parameter::accounts_for(parameter, name, spec))
        {
            errors.push(ValidationError {
                location: Location::Query,
                name: name.clone(),
                pointer: String::new(),
                kind: ErrorKind::Undescribed,
            });
        }
    }
}

/// Every method a Path Item Object describes, as method tokens — which
/// is what an `Allow` header wants, and what `operations`' lowercase
/// keys are not.
fn allowed_methods(path_item: &PathItem) -> Vec<String> {
    let standard = path_item
        .operations
        .iter()
        .flatten()
        .map(|(key, _)| crate::method::from_standard_key(key));
    let additional = path_item
        .additional_operations
        .iter()
        .flatten()
        .map(|(key, _)| key.clone());
    standard.chain(additional).collect()
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
