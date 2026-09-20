//! Offline input validation, separate from model validation and HTTP execution.

use crate::Options;
use roas_arazzo::v1_1::{Description, Workflow};
use serde_json::Value;

/// Whether workflow entries enforce their declared input schemas.
/// Enabling a Cargo feature alone never changes this runtime policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputValidation {
    /// Preserve pass-through input semantics. Object-shaped arguments are still required.
    #[default]
    Disabled,
    /// Offline JSON Schema 2020-12; requires the `input-validation` feature.
    Draft202012,
}

/// One failed assertion, with JSON Pointer locations (empty means the root).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputViolation {
    /// Location in the workflow's input object.
    pub instance_path: String,
    /// Location of the failing keyword within its schema resource.
    pub schema_path: String,
    /// Absolute keyword URI when available, including an external resource's identity.
    pub schema_uri: Option<String>,
    /// Explanation with instance values masked to avoid echoing secrets.
    pub message: String,
}

impl std::fmt::Display for InputViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "input #{} against {}: {}",
            self.instance_path,
            self.schema_uri.as_deref().unwrap_or(&self.schema_path),
            self.message
        )
    }
}

/// Schema/configuration errors are distinct from instances that fail a valid schema.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InputError {
    /// A non-object batch was supplied through `Options::inputs`.
    #[error("workflow inputs must be a JSON object")]
    NotObject,
    /// Validation was requested in a build without the backend.
    #[error("input validation requires the `input-validation` Cargo feature")]
    Unavailable,
    /// Invalid document URI, conflicting resource, or unusable supplied schema catalog.
    #[error("input schema configuration: {0}")]
    Configuration(String),
    /// The workflow's schema could not be compiled, including unresolved references.
    #[error("workflow `{workflow}` has an invalid input schema at {schema_path}: {message}")]
    Schema {
        /// Workflow owning the schema.
        workflow: String,
        /// Backend schema-error location as a JSON Pointer.
        schema_path: String,
        /// Schema compilation or reference resolution error.
        message: String,
    },
    /// Actual bound inputs failed a compiled schema. This is a terminal engine error.
    #[error("workflow `{workflow}` has invalid inputs: {}", violations.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid {
        /// Workflow whose entry was rejected.
        workflow: String,
        /// All failed assertions, in deterministic location order.
        violations: Vec<InputViolation>,
    },
}

impl Options {
    /// Set the input-validation policy. No schema retrieval or coercion is performed.
    #[must_use]
    pub fn input_validation(mut self, mode: InputValidation) -> Self {
        self.input_validation = mode;
        self
    }

    /// The configured policy, independently of available Cargo features.
    #[must_use]
    pub fn input_validation_mode(&self) -> InputValidation {
        self.input_validation
    }

    /// Supply the Arazzo document's retrieval URI for input-schema references.
    /// Its `$self`, when present, resolves against this URI and becomes the base.
    /// A source registry supplies this metadata automatically.
    /// # Errors
    /// The URI is not absolute or has a nonempty fragment.
    pub fn input_schema_base(mut self, retrieval_uri: &str) -> Result<Self, InputError> {
        self.input_schema_base = Some(resource_uri(retrieval_uri)?);
        Ok(self)
    }

    /// Supply a complete schema document without fetching it or making it an API source.
    /// Register all resources before preparation; `$id` and `$anchor` are indexed by
    /// the validator. Identical registrations are idempotent.
    /// # Errors
    /// Invalid URI or different documents registered at the same URI.
    pub fn schema_document(mut self, uri: &str, document: Value) -> Result<Self, InputError> {
        let uri = resource_uri(uri)?;
        if let Some(previous) = self.schema_documents.get(&uri)
            && previous != &document
        {
            return Err(InputError::Configuration(format!(
                "different schemas supplied at `{uri}`"
            )));
        }
        self.schema_documents.insert(uri, document);
        Ok(self)
    }

    pub(crate) fn check_inputs(&self) -> Result<(), InputError> {
        if self.invalid_inputs {
            return Err(InputError::NotObject);
        }
        if self.input_validation == InputValidation::Draft202012
            && !cfg!(feature = "input-validation")
        {
            return Err(InputError::Unavailable);
        }
        Ok(())
    }
}

fn resource_uri(value: &str) -> Result<url::Url, InputError> {
    let mut uri =
        url::Url::parse(value).map_err(|error| InputError::Configuration(error.to_string()))?;
    if uri.fragment().is_some_and(|fragment| !fragment.is_empty()) {
        return Err(InputError::Configuration(format!(
            "schema document URI `{uri}` has a fragment"
        )));
    }
    uri.set_fragment(None);
    Ok(uri)
}

/// Each lazy run owns its validators; prepared runs borrow their plan's validators.
#[derive(Debug, Default)]
pub(crate) struct InputSchemas {
    enabled: bool,
    #[cfg(feature = "input-validation")]
    backend: Option<backend::Schemas>,
}

impl InputSchemas {
    pub(crate) fn compile(
        &mut self,
        description: &Description,
        options: &Options,
        workflow: &Workflow,
    ) -> Result<(), InputError> {
        options.check_inputs()?;
        self.enabled = options.input_validation == InputValidation::Draft202012;
        if options.input_validation == InputValidation::Disabled || workflow.inputs.is_none() {
            return Ok(());
        }
        #[cfg(feature = "input-validation")]
        {
            if self.backend.is_none() {
                self.backend = Some(backend::Schemas::new(description, options)?);
            }
            self.backend
                .as_mut()
                .expect("initialized above")
                .compile(description, workflow)
        }
        #[cfg(not(feature = "input-validation"))]
        {
            let _ = description;
            Err(InputError::Unavailable)
        }
    }

    pub(crate) fn validate(&self, workflow: &Workflow, inputs: &Value) -> Result<(), InputError> {
        if !self.enabled || workflow.inputs.is_none() {
            return Ok(());
        }
        #[cfg(feature = "input-validation")]
        if let Some(backend) = &self.backend {
            return backend.validate(workflow, inputs);
        }
        let _ = inputs;
        Err(InputError::Configuration(format!(
            "input schema for workflow `{}` was not compiled",
            workflow.workflow_id
        )))
    }
}

#[cfg(feature = "input-validation")]
mod backend {
    use super::{InputError, InputViolation, resource_uri};
    use crate::Options;
    use crate::input_catalog::{Catalog, Documents, children};
    use jsonschema::{Draft, Registry, Validator};
    use roas_arazzo::v1_1::{Description, Workflow};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use url::Url;

    const DRAFT: Draft = Draft::Draft202012;
    const LOCAL_BASE: &str = "https://roas-input.invalid/description";

    #[derive(Debug)]
    pub(super) struct Schemas {
        catalog: Catalog,
        base: Url,
        validators: BTreeMap<String, Validator>,
    }

    impl Schemas {
        pub(super) fn new(
            description: &Description,
            options: &Options,
        ) -> Result<Self, InputError> {
            let retrieval = options.input_schema_base.clone();
            #[cfg(feature = "source-graph")]
            let retrieval = retrieval.or_else(|| {
                options.registry.as_ref().and_then(|context| {
                    context
                        .registry
                        .document(context.owner)
                        .ok()
                        .map(|document| document.retrieval_uri().clone())
                })
            });
            let mut documents = BTreeMap::new();
            #[cfg(feature = "source-graph")]
            if let Some(context) = &options.registry {
                for (uri, value, base) in context.registry.schema_documents() {
                    documents.insert(uri.clone(), (base.clone(), value.clone()));
                }
            }
            for source in options.sources.values() {
                #[cfg(feature = "source-graph")]
                if matches!(source.data, crate::operation::SourceData::Registry(_)) {
                    continue;
                }
                if let Ok(uri) = resource_uri(&source.url) {
                    insert(&mut documents, uri, source.document().clone())?;
                }
            }
            for (uri, value) in &options.schema_documents {
                insert(&mut documents, uri.clone(), value.clone())?;
            }
            let base = match &description.self_ {
                Some(identity) => match &retrieval {
                    Some(retrieval) => retrieval.join(identity),
                    None => Url::parse(identity),
                }
                .map_err(|error| {
                    InputError::Configuration(format!(
                        "Arazzo $self `{identity}` needs an absolute base: {error}"
                    ))
                })?,
                None => retrieval
                    .clone()
                    .unwrap_or_else(|| Url::parse(LOCAL_BASE).expect("constant URI")),
            };
            let base = resource_uri(base.as_str())?;
            // Use the model actually passed to prepare/run, not a possibly stale
            // registry copy of that same description.
            let value = serde_json::to_value(description)
                .map_err(|error| InputError::Configuration(error.to_string()))?;
            documents.insert(base.clone(), (base.clone(), value.clone()));
            if let Some(retrieval) = retrieval {
                documents.insert(retrieval, (base.clone(), value));
            }
            Ok(Self {
                catalog: Catalog::new(documents),
                base,
                validators: BTreeMap::new(),
            })
        }

        fn registry(documents: Documents) -> Result<Registry<'static>, InputError> {
            let mut registry = Registry::new().draft(DRAFT);
            let mut identities = BTreeMap::new();
            let mut anchors = BTreeMap::new();
            let mut normalized = Vec::new();
            for (uri, (base, value)) in documents {
                let value = embedded_schemas(value, &base, &mut identities, &mut anchors)?;
                if let Some(previous) = identities.get(&uri)
                    && previous != &value
                {
                    return Err(InputError::Configuration(format!(
                        "a schema identity conflicts with the document supplied at `{uri}`"
                    )));
                }
                identities.insert(uri.clone(), value.clone());
                normalized.push((uri, value));
            }
            // Plain-name references may use a retrieval alias for a resource
            // whose `$id` is canonical. Preserve the same target/location.
            let canonical_anchors = anchors.clone();
            for (uri, value) in &normalized {
                let canonical = value
                    .get("$id")
                    .and_then(Value::as_str)
                    .unwrap_or(uri.as_str());
                for (anchor, target) in &canonical_anchors {
                    if let Some((resource, name)) = anchor.split_once('#')
                        && resource == canonical
                    {
                        let mut alias = uri.clone();
                        alias.set_fragment(Some(name));
                        anchors.insert(alias.to_string(), target.clone());
                    }
                }
            }
            for (uri, mut value) in normalized {
                rewrite_document_anchors(&mut value, &anchors);
                registry = registry
                    .add(uri.as_str(), value)
                    .map_err(|error| InputError::Configuration(error.to_string()))?;
            }
            // referencing::Registry's default retriever never performs IO.
            // Validator construction below additionally selects offline mode,
            // even if another dependency enables jsonschema's retrieval features.
            registry
                .prepare()
                .map_err(|error| InputError::Configuration(error.to_string()))
        }

        pub(super) fn compile(
            &mut self,
            description: &Description,
            workflow: &Workflow,
        ) -> Result<(), InputError> {
            if self.validators.contains_key(&workflow.workflow_id) {
                return Ok(());
            }
            let index = description
                .workflows
                .iter()
                .position(|candidate| std::ptr::eq(candidate, workflow))
                .expect("workflow belongs to the description");
            let mut uri = self.base.clone();
            uri.set_fragment(Some(&format!("/workflows/{index}/inputs")));
            let registry = Self::registry(self.catalog.reachable(&uri))?;
            let validator = jsonschema::options()
                .offline()
                .with_draft(DRAFT)
                .should_validate_formats(false)
                .with_registry(&registry)
                .build(&json!({"$ref": uri.as_str()}))
                .map_err(|error| InputError::Schema {
                    workflow: workflow.workflow_id.clone(),
                    schema_path: error.schema_path().to_string(),
                    message: error.to_string(),
                })?;
            self.validators
                .insert(workflow.workflow_id.clone(), validator);
            Ok(())
        }

        pub(super) fn validate(
            &self,
            workflow: &Workflow,
            inputs: &Value,
        ) -> Result<(), InputError> {
            #[cfg(test)]
            super::tests::VALIDATIONS.with(|count| count.set(count.get() + 1));
            // Prepared plans compile every potential entry. Never silently skip
            // validation if a future traversal misses a schema-bearing workflow.
            let validator = self.validators.get(&workflow.workflow_id).ok_or_else(|| {
                InputError::Configuration(format!(
                    "input schema for workflow `{}` was not compiled",
                    workflow.workflow_id
                ))
            })?;
            let mut violations: Vec<_> = validator
                .iter_errors(inputs)
                .map(|error| InputViolation {
                    instance_path: error.instance_path().to_string(),
                    schema_path: error.schema_path().to_string(),
                    schema_uri: error.absolute_keyword_location().map(ToString::to_string),
                    message: error.masked().to_string(),
                })
                .collect();
            violations.sort_by(|left, right| {
                (
                    &left.instance_path,
                    &left.schema_uri,
                    &left.schema_path,
                    &left.message,
                )
                    .cmp(&(
                        &right.instance_path,
                        &right.schema_uri,
                        &right.schema_path,
                        &right.message,
                    ))
            });
            if violations.is_empty() {
                Ok(())
            } else {
                Err(InputError::Invalid {
                    workflow: workflow.workflow_id.clone(),
                    violations,
                })
            }
        }
    }

    fn insert(
        documents: &mut BTreeMap<Url, (Url, Value)>,
        uri: Url,
        value: Value,
    ) -> Result<(), InputError> {
        if let Some((_, previous)) = documents.get(&uri) {
            return if previous == &value {
                Ok(())
            } else {
                Err(InputError::Configuration(format!(
                    "different documents supplied at `{uri}`"
                )))
            };
        }
        let base = if value.get("arazzo").is_some() || value.get("openapi").is_some() {
            value
                .get("$self")
                .and_then(Value::as_str)
                .map(|identity| uri.join(identity))
                .transpose()
                .map_err(|error| InputError::Configuration(error.to_string()))?
                .unwrap_or_else(|| uri.clone())
        } else {
            uri.clone()
        };
        documents.insert(uri, (base, value));
        Ok(())
    }

    /// JSON Schema crawlers do not know Arazzo/OpenAPI container keywords. A
    /// private indexing view exposes embedded schemas under `$defs`, retaining
    /// their original locations for pointer resolution. Only resource/anchor
    /// bearing roots need mirroring; ordinary targets are discovered through refs.
    /// Original source documents and runtime expression contexts remain untouched.
    fn embedded_schemas(
        mut value: Value,
        base: &Url,
        identities: &mut BTreeMap<Url, Value>,
        anchors: &mut BTreeMap<String, String>,
    ) -> Result<Value, InputError> {
        // A retrieval alias must still resolve schema references from the
        // document's canonical base, not from the alias's directory.
        if (value.get("arazzo").is_some() || value.get("openapi").is_some())
            && let Some(object) = value.as_object_mut()
        {
            object.insert("$id".into(), Value::String(base.to_string()));
        }
        let mut roots = Vec::new();
        if value.get("arazzo").is_some() {
            if let Some(workflows) = value.get_mut("workflows").and_then(Value::as_array_mut) {
                for (index, workflow) in workflows.iter_mut().enumerate() {
                    if let Some(schema) = workflow.get_mut("inputs") {
                        normalize(
                            schema,
                            base,
                            &format!("/workflows/{index}/inputs"),
                            identities,
                            anchors,
                        )?;
                        if has_identifier(schema) {
                            roots.push(schema.clone());
                        }
                    }
                }
            }
            if let Some(inputs) = value
                .pointer_mut("/components/inputs")
                .and_then(Value::as_object_mut)
            {
                for (name, schema) in inputs {
                    normalize(
                        schema,
                        base,
                        &format!("/components/inputs/{}", escape(name)),
                        identities,
                        anchors,
                    )?;
                    if has_identifier(schema) {
                        roots.push(schema.clone());
                    }
                }
            }
        } else if value.get("openapi").is_some() {
            if let Some(schemas) = value
                .pointer_mut("/components/schemas")
                .and_then(Value::as_object_mut)
            {
                for (name, schema) in schemas {
                    normalize(
                        schema,
                        base,
                        &format!("/components/schemas/{}", escape(name)),
                        identities,
                        anchors,
                    )?;
                    if has_identifier(schema) {
                        roots.push(schema.clone());
                    }
                }
            }
        } else {
            normalize(&mut value, base, "", identities, anchors)?;
        }
        if !roots.is_empty() {
            let object = value.as_object_mut().expect("document with named fields");
            // A non-asserting keyword avoids making a reference to the whole
            // document accidentally require all of its unrelated schemas.
            let definitions = object.entry("$defs").or_insert_with(|| json!({}));
            let definitions = definitions.as_object_mut().ok_or_else(|| {
                InputError::Configuration(format!("`{base}` has a non-object $defs"))
            })?;
            let mut key = "roas-input-index".to_owned();
            while definitions.contains_key(&key) {
                key.push('_');
            }
            definitions.insert(key, json!({"allOf":roots}));
        }
        Ok(value)
    }

    fn has_identifier(schema: &Value) -> bool {
        schema.get("$id").is_some()
            || schema.get("$anchor").is_some()
            || schema.get("$dynamicAnchor").is_some()
            || DRAFT.subresources_of(schema).any(has_identifier)
    }

    /// Freeze lexical URI scope before handing embedded schemas to a generic
    /// JSON Schema resolver: it cannot infer scope across Arazzo's unknown
    /// `workflows`/`components` keywords. Only schema-valued 2020-12 keywords
    /// are traversed; annotations such as `default`, `enum`, and examples are data.
    fn normalize(
        schema: &mut Value,
        parent: &Url,
        pointer: &str,
        identities: &mut BTreeMap<Url, Value>,
        anchors: &mut BTreeMap<String, String>,
    ) -> Result<(), InputError> {
        let Some(object) = schema.as_object_mut() else {
            return Ok(());
        };
        let pointer = if object.get("$id").and_then(Value::as_str).is_some() {
            ""
        } else {
            pointer
        };
        let base = if let Some(id) = object.get("$id").and_then(Value::as_str) {
            let base = parent.join(id).map_err(|error| {
                InputError::Configuration(format!("invalid schema $id `{id}`: {error}"))
            })?;
            let base = resource_uri(base.as_str())?;
            object.insert("$id".into(), Value::String(base.to_string()));
            base
        } else {
            parent.clone()
        };
        if let Some(dialect) = object.get("$schema").and_then(Value::as_str)
            && dialect.trim_end_matches('#') != "https://json-schema.org/draft/2020-12/schema"
        {
            return Err(InputError::Configuration(format!(
                "unsupported input-schema dialect `{dialect}` at `{base}#{pointer}/$schema`; expected JSON Schema 2020-12"
            )));
        }
        for name in ["$ref", "$dynamicRef"] {
            if let Some(reference) = object.get(name).and_then(Value::as_str) {
                let uri = base.join(reference).map_err(|error| {
                    InputError::Configuration(format!(
                        "invalid {name} `{reference}` at `{base}`: {error}"
                    ))
                })?;
                object.insert(name.into(), Value::String(uri.to_string()));
            }
        }
        for (path, child) in children(object) {
            normalize(
                child,
                &base,
                &format!("{pointer}{path}"),
                identities,
                anchors,
            )?;
        }
        let mut names = Vec::new();
        if object.contains_key("$id") {
            names.push(base.clone());
        }
        for name in ["$anchor", "$dynamicAnchor"] {
            if let Some(anchor) = object.get(name).and_then(Value::as_str) {
                let mut uri = base.clone();
                uri.set_fragment(Some(anchor));
                let mut target = base.clone();
                target.set_fragment(Some(&pointer.replace('%', "%25")));
                anchors.insert(uri.to_string(), target.to_string());
                names.push(uri);
            }
        }
        for uri in names {
            if let Some(previous) = identities.get(&uri)
                && previous != schema
            {
                return Err(InputError::Configuration(format!(
                    "different input schemas claim `{uri}`"
                )));
            }
            identities.insert(uri, schema.clone());
        }
        Ok(())
    }

    fn escape(segment: &str) -> String {
        segment.replace('~', "~0").replace('/', "~1")
    }

    fn rewrite_document_anchors(document: &mut Value, anchors: &BTreeMap<String, String>) {
        if document.get("arazzo").is_some() {
            if let Some(workflows) = document.get_mut("workflows").and_then(Value::as_array_mut) {
                for workflow in workflows {
                    if let Some(schema) = workflow.get_mut("inputs") {
                        rewrite_anchors(schema, anchors);
                    }
                }
            }
            if let Some(inputs) = document
                .pointer_mut("/components/inputs")
                .and_then(Value::as_object_mut)
            {
                for schema in inputs.values_mut() {
                    rewrite_anchors(schema, anchors);
                }
            }
        } else if document.get("openapi").is_some()
            && let Some(schemas) = document
                .pointer_mut("/components/schemas")
                .and_then(Value::as_object_mut)
        {
            for schema in schemas.values_mut() {
                rewrite_anchors(schema, anchors);
            }
        }
        // Also visit pure schema documents and the private `$defs` index.
        rewrite_anchors(document, anchors);
    }

    fn rewrite_anchors(schema: &mut Value, anchors: &BTreeMap<String, String>) {
        let Some(object) = schema.as_object_mut() else {
            return;
        };
        // Resolve only declared static anchors to their actual pointer targets.
        // This also gives backend diagnostics a real source location instead of
        // incorrectly treating every anchored target as its document root.
        // `$dynamicRef` must retain its anchor: changing it to a pointer would
        // disable dynamic-scope resolution.
        if let Some(reference) = object.get_mut("$ref")
            && let Some(target) = reference
                .as_str()
                .and_then(|reference| anchors.get(reference))
        {
            *reference = Value::String(target.clone());
        }
        for (_, child) in children(object) {
            rewrite_anchors(child, anchors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(feature = "input-validation")]
    thread_local! { pub(super) static VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

    #[cfg(feature = "input-validation")]
    #[test]
    fn root_inputs_are_checked_once_but_calls_revalidate_bound_arguments() {
        let description: Description = serde_json::from_value(json!({
            "arazzo":"1.1.0","info":{"title":"Entry counts","version":"1"},
            "sourceDescriptions":[{"name":"api","url":"https://example.com/api.json"}],"workflows":[
                {"workflowId":"root","inputs":{},"dependsOn":["dependency"],
                 "steps":[{"stepId":"call","workflowId":"dependency"}]},
                {"workflowId":"dependency","inputs":{},"steps":[{"stepId":"request","operationId":"check"}]}
            ]
        }))
        .unwrap();
        let options = Options::new().input_validation(InputValidation::Draft202012)
            .source("api", "https://example.com/api.json", json!({"openapi":"3.1.0",
                "servers":[{"url":"https://example.com"}],"paths":{"/check":{"get":{"operationId":"check"}}}}));
        for prepared in [false, true] {
            let plan = prepared.then(|| crate::prepare(&description, &options).unwrap());
            let before = VALIDATIONS.get();
            let mut run = match &plan {
                Some(plan) => plan.start(),
                None => crate::Run::start(&description, &options),
            }
            .unwrap();
            assert_eq!(
                VALIDATIONS.get() - before,
                2,
                "root and dependency preflight"
            );
            loop {
                match run.advance().unwrap() {
                    crate::Progress::Send(_) => run
                        .supply(crate::HttpResponse::json(200, &json!({})))
                        .unwrap(),
                    crate::Progress::Done(_) => break,
                    progress => panic!("unexpected progress: {progress:?}"),
                }
            }
            assert_eq!(
                VALIDATIONS.get() - before,
                3,
                "only the explicit call revalidates"
            );
        }
    }

    fn description() -> Description {
        serde_json::from_value(
            json!({"arazzo":"1.1.0","info":{"title":"Input cache","version":"1"},
            "sourceDescriptions":[],"workflows":[
                {"workflowId":"a","inputs":{},"steps":[]},
                {"workflowId":"b","inputs":{},"steps":[]}
            ]}),
        )
        .unwrap()
    }

    #[test]
    fn an_enabled_cache_never_silently_omits_validation() {
        let description = description();
        let schemas = InputSchemas {
            enabled: true,
            #[cfg(feature = "input-validation")]
            backend: None,
        };
        let error = schemas
            .validate(&description.workflows[0], &json!({}))
            .unwrap_err();
        assert!(error.to_string().contains("was not compiled"));
    }

    #[cfg(feature = "input-validation")]
    #[test]
    fn a_prepared_cache_requires_each_schema_bearing_workflow() {
        let description = description();
        let options = Options::new().input_validation(InputValidation::Draft202012);
        let mut schemas = InputSchemas::default();
        schemas
            .compile(&description, &options, &description.workflows[0])
            .unwrap();
        assert!(
            schemas
                .validate(&description.workflows[1], &json!({}))
                .unwrap_err()
                .to_string()
                .contains("was not compiled")
        );
    }

    #[test]
    fn diagnostics_without_absolute_locations_still_name_both_paths() {
        let violation = InputViolation {
            instance_path: "/name".into(),
            schema_path: "/properties/name/type".into(),
            schema_uri: None,
            message: "expected a string".into(),
        };
        assert_eq!(
            violation.to_string(),
            "input #/name against /properties/name/type: expected a string"
        );
    }
}
