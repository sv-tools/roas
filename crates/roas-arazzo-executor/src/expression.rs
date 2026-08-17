//! Arazzo runtime expressions.
//!
//! The model keeps these as plain strings — `roas-arazzo` parses the
//! document, not the little language inside it — so this is where
//! `$response.body#/id` becomes a value.
//!
//! Per [Runtime Expressions](https://spec.openapis.org/arazzo/v1.1.0.html#runtime-expressions).
//! A name that resolves to nothing is an error rather than a null: a
//! workflow that reads an output no step wrote is a workflow with a bug,
//! and saying so where it happens beats a null surfacing three steps
//! later.

use crate::http::{HttpRequest, HttpResponse};
use serde_json::Value;
use std::collections::BTreeMap;

/// A step's exchange: what was sent, and what came back.
///
/// The request is kept alongside the values that went into it, because
/// `$request.path.id` asks about the value the step supplied, not about
/// the URL it ended up inside.
#[derive(Clone, Debug, Default)]
pub(crate) struct Exchange {
    pub request: HttpRequest,
    pub path: BTreeMap<String, Value>,
    pub query: BTreeMap<String, Value>,
    pub body: Option<Value>,
    pub response: Option<HttpResponse>,
    pub response_body: Option<Value>,
}

/// What a workflow that has finished was given and produced.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkflowState {
    pub inputs: Value,
    pub outputs: BTreeMap<String, Value>,
}

/// What a step has produced so far.
#[derive(Clone, Debug, Default)]
pub(crate) struct StepState {
    pub exchange: Option<Exchange>,
    pub outputs: BTreeMap<String, Value>,
}

/// Everything a runtime expression can name at one point in a run.
pub(crate) struct Scope<'a> {
    /// The inputs the workflow was called with.
    pub inputs: &'a Value,
    /// The outputs the workflow has named so far.
    pub outputs: &'a BTreeMap<String, Value>,
    /// Every step of this workflow that has run.
    pub steps: &'a BTreeMap<String, StepState>,
    /// The workflows that have finished, by id.
    pub workflows: &'a BTreeMap<String, WorkflowState>,
    /// The description's `$self`, when it declares one.
    pub self_: Option<&'a str>,
    /// The step being evaluated, when there is one.
    pub here: Option<&'a Exchange>,
    /// `sourceDescriptions`, by name.
    pub sources: &'a Value,
    /// The description's `components`.
    pub components: &'a Value,
}

/// Why an expression could not be turned into a value.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExpressionError {
    /// The expression does not start with a name this crate knows.
    #[error("`{0}` is not a runtime expression")]
    Unknown(String),
    /// The expression is well formed but names something absent.
    #[error("`{expression}` names {what}")]
    Missing {
        /// The expression as written.
        expression: String,
        /// What it wanted that was not there.
        what: String,
    },
    /// The expression belongs to a part of Arazzo this crate does not
    /// execute.
    #[error("`{0}` belongs to an AsyncAPI step, which this crate does not execute")]
    Unsupported(String),
}

fn missing(expression: &str, what: impl Into<String>) -> ExpressionError {
    ExpressionError::Missing {
        expression: expression.to_owned(),
        what: what.into(),
    }
}

/// Whether `text` is an expression rather than a literal.
#[must_use]
pub(crate) fn is_expression(text: &str) -> bool {
    text.starts_with('$')
}

/// Evaluate one whole expression, e.g. `$response.body#/id`.
pub(crate) fn evaluate(expression: &str, scope: &Scope<'_>) -> Result<Value, ExpressionError> {
    // A `#` starts the JSON Pointer half; everything before it is the
    // dotted name half.
    let (name, pointer) = match expression.split_once('#') {
        Some((name, pointer)) => (name, Some(pointer)),
        None => (expression, None),
    };
    let mut parts = name.split('.');
    let root = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();

    let value = match root {
        "$inputs" => walk(scope.inputs, &rest, expression, "an input")?,
        "$outputs" => from_map(scope.outputs, &rest, expression, "an output")?,
        "$components" => walk(scope.components, &rest, expression, "a component")?,
        "$sourceDescriptions" => walk(scope.sources, &rest, expression, "a source description")?,
        "$self" => Value::String(
            scope
                .self_
                .ok_or_else(|| missing(expression, "`$self`, which the description does not set"))?
                .to_owned(),
        ),
        "$workflows" => {
            let (id, rest) = split_first(&rest, expression, "a workflow id")?;
            let workflow = scope.workflows.get(id).ok_or_else(|| {
                missing(expression, format!("workflow `{id}`, which has not run"))
            })?;
            // `inputs` and `outputs` are both fields of a workflow; the
            // bare shorthand names an output.
            match rest.split_first() {
                Some((field, rest)) if *field == "inputs" => {
                    walk(&workflow.inputs, rest, expression, "an input")?
                }
                Some((field, rest)) if *field == "outputs" => {
                    from_map(&workflow.outputs, rest, expression, "an output")?
                }
                _ => from_map(&workflow.outputs, rest, expression, "an output")?,
            }
        }
        "$steps" => {
            let (id, rest) = split_first(&rest, expression, "a step id")?;
            let step = scope
                .steps
                .get(id)
                .ok_or_else(|| missing(expression, format!("step `{id}`, which has not run")))?;
            if let Some(rest) = rest.strip_prefix(&["outputs"][..]) {
                from_map(&step.outputs, rest, expression, "an output")?
            } else {
                let exchange = step.exchange.as_ref().ok_or_else(|| {
                    missing(
                        expression,
                        format!("the exchange of step `{id}`, which has none"),
                    )
                })?;
                within(exchange, rest, expression)?
            }
        }
        "$message" => return Err(ExpressionError::Unsupported(expression.to_owned())),
        // The rest read the step being evaluated. A name that is none of
        // them is not an expression at all, which is worth saying before
        // asking whether the step has sent anything.
        "$url" | "$method" | "$statusCode" | "$request" | "$response" => {
            let exchange = scope.here.ok_or_else(|| {
                missing(
                    expression,
                    "the current step, which has not sent anything yet",
                )
            })?;
            let mut whole = vec![root];
            whole.extend_from_slice(&rest);
            within(exchange, &whole, expression)?
        }
        other => return Err(ExpressionError::Unknown(other.to_owned())),
    };

    match pointer {
        None => Ok(value),
        Some(pointer) => value
            .pointer(pointer)
            .cloned()
            .ok_or_else(|| missing(expression, format!("`{pointer}`, which the value has not"))),
    }
}

/// Read one exchange: `$url`, `$method`, `$statusCode`, `$request.*`,
/// `$response.*` — with or without the leading `$`, so the same code
/// serves `$steps.<id>.url`.
fn within(exchange: &Exchange, parts: &[&str], expression: &str) -> Result<Value, ExpressionError> {
    let (head, rest) = split_first(parts, expression, "a name")?;
    let response = || {
        exchange
            .response
            .as_ref()
            .ok_or_else(|| missing(expression, "a response, which this step has not received"))
    };
    match head.trim_start_matches('$') {
        "url" => Ok(Value::String(exchange.request.url.clone())),
        "method" => Ok(Value::String(exchange.request.method.clone())),
        "statusCode" => Ok(Value::from(response()?.status)),
        "request" => {
            let (what, rest) = split_first(rest, expression, "a request part")?;
            match what {
                "header" => header(&exchange.request.headers, rest, expression),
                "path" => from_map(&exchange.path, rest, expression, "a path parameter"),
                "query" => from_map(&exchange.query, rest, expression, "a query parameter"),
                "body" => exchange
                    .body
                    .clone()
                    .ok_or_else(|| missing(expression, "a request body, which this step has none")),
                other => Err(ExpressionError::Unknown(format!("$request.{other}"))),
            }
        }
        "response" => {
            let (what, rest) = split_first(rest, expression, "a response part")?;
            match what {
                "header" => header(&response()?.headers, rest, expression),
                "body" => exchange.response_body.clone().ok_or_else(|| {
                    missing(expression, "a JSON response body, which this step has none")
                }),
                other => Err(ExpressionError::Unknown(format!("$response.{other}"))),
            }
        }
        "message" => Err(ExpressionError::Unsupported(expression.to_owned())),
        other => Err(ExpressionError::Unknown(format!("${other}"))),
    }
}

/// A header value. Header names may hold dots, so the remainder is
/// rejoined rather than taken one part at a time.
fn header(
    headers: &[(String, String)],
    rest: &[&str],
    expression: &str,
) -> Result<Value, ExpressionError> {
    let name = rest.join(".");
    if name.is_empty() {
        return Err(missing(expression, "a header name"));
    }
    headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(&name))
        .map(|(_, value)| Value::String(value.clone()))
        .ok_or_else(|| missing(expression, format!("header `{name}`, which is not present")))
}

fn split_first<'p>(
    parts: &'p [&'p str],
    expression: &str,
    what: &str,
) -> Result<(&'p str, &'p [&'p str]), ExpressionError> {
    match parts.split_first() {
        Some((head, rest)) if !head.is_empty() => Ok((head, rest)),
        _ => Err(missing(expression, format!("{what}, which is missing"))),
    }
}

/// Walk a dotted name into a JSON value.
fn walk(
    value: &Value,
    parts: &[&str],
    expression: &str,
    what: &str,
) -> Result<Value, ExpressionError> {
    let mut current = value;
    for part in parts {
        current = current
            .get(part)
            .ok_or_else(|| missing(expression, format!("{what} named `{part}`")))?;
    }
    Ok(current.clone())
}

/// The same, for the maps the run keeps by name.
fn from_map(
    map: &BTreeMap<String, Value>,
    parts: &[&str],
    expression: &str,
    what: &str,
) -> Result<Value, ExpressionError> {
    let (name, rest) = split_first(parts, expression, what)?;
    let value = map
        .get(name)
        .ok_or_else(|| missing(expression, format!("{what} named `{name}`")))?;
    walk(value, rest, expression, what)
}

/// Replace every `{$…}` in `text` with what it evaluates to.
///
/// A string is what the caller asked for, so a string value is put in as
/// it stands and anything else as its JSON.
pub(crate) fn interpolate(text: &str, scope: &Scope<'_>) -> Result<String, ExpressionError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{$") {
        let Some(end) = rest[start..].find('}').map(|end| start + end) else {
            break;
        };
        out.push_str(&rest[..start]);
        let value = evaluate(&rest[start + 1..end], scope)?;
        match value {
            Value::String(text) => out.push_str(&text),
            other => out.push_str(&other.to_string()),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A scope holding everything the tests name, so each test says only
    /// what it is about.
    pub(crate) struct Fixture {
        pub inputs: Value,
        pub outputs: BTreeMap<String, Value>,
        pub steps: BTreeMap<String, StepState>,
        pub workflows: BTreeMap<String, WorkflowState>,
        pub self_: Option<String>,
        pub here: Option<Exchange>,
        pub sources: Value,
        pub components: Value,
    }

    impl Default for Fixture {
        fn default() -> Self {
            Self {
                inputs: json!({ "petId": "7", "auth": { "token": "abc" } }),
                outputs: BTreeMap::new(),
                steps: BTreeMap::new(),
                workflows: BTreeMap::new(),
                self_: None,
                here: None,
                sources: json!({ "petStore": { "url": "https://api.example.com/openapi.json" } }),
                components: json!({ "parameters": { "locale": { "name": "locale" } } }),
            }
        }
    }

    impl Fixture {
        pub(crate) fn scope(&self) -> Scope<'_> {
            Scope {
                inputs: &self.inputs,
                outputs: &self.outputs,
                steps: &self.steps,
                workflows: &self.workflows,
                self_: self.self_.as_deref(),
                here: self.here.as_ref(),
                sources: &self.sources,
                components: &self.components,
            }
        }
    }

    pub(crate) fn exchange() -> Exchange {
        Exchange {
            request: HttpRequest {
                method: "GET".to_owned(),
                url: "https://api.example.com/pets/7".to_owned(),
                headers: vec![("Authorization".to_owned(), "Bearer abc".to_owned())],
                body: None,
                timeout: None,
            },
            path: BTreeMap::from([("petId".to_owned(), json!("7"))]),
            query: BTreeMap::from([("limit".to_owned(), json!(10))]),
            body: Some(json!({ "name": "fluffy" })),
            response: Some(HttpResponse {
                status: 200,
                headers: vec![("X-Rate-Limit".to_owned(), "9".to_owned())],
                body: br#"{"id":7,"tags":["cat"]}"#.to_vec(),
            }),
            response_body: Some(json!({ "id": 7, "tags": ["cat"] })),
        }
    }

    fn eval(expression: &str, fixture: &Fixture) -> Result<Value, ExpressionError> {
        evaluate(expression, &fixture.scope())
    }

    #[test]
    fn the_document_and_its_inputs_are_readable() {
        let fixture = Fixture::default();
        assert_eq!(eval("$inputs.petId", &fixture), Ok(json!("7")));
        assert_eq!(eval("$inputs.auth.token", &fixture), Ok(json!("abc")));
        assert_eq!(
            eval("$sourceDescriptions.petStore.url", &fixture),
            Ok(json!("https://api.example.com/openapi.json"))
        );
        assert_eq!(
            eval("$components.parameters.locale.name", &fixture),
            Ok(json!("locale"))
        );
    }

    #[test]
    fn the_current_exchange_is_readable_every_way_the_spec_spells_it() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        assert_eq!(
            eval("$url", &fixture),
            Ok(json!("https://api.example.com/pets/7"))
        );
        assert_eq!(eval("$method", &fixture), Ok(json!("GET")));
        assert_eq!(eval("$statusCode", &fixture), Ok(json!(200)));
        assert_eq!(
            eval("$request.header.authorization", &fixture),
            Ok(json!("Bearer abc"))
        );
        assert_eq!(eval("$request.path.petId", &fixture), Ok(json!("7")));
        assert_eq!(eval("$request.query.limit", &fixture), Ok(json!(10)));
        assert_eq!(
            eval("$request.body", &fixture),
            Ok(json!({ "name": "fluffy" }))
        );
        assert_eq!(
            eval("$response.header.X-Rate-Limit", &fixture),
            Ok(json!("9"))
        );
        assert_eq!(
            eval("$response.body", &fixture),
            Ok(json!({ "id": 7, "tags": ["cat"] }))
        );
    }

    #[test]
    fn a_pointer_reaches_into_whatever_the_name_produced() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        assert_eq!(eval("$response.body#/id", &fixture), Ok(json!(7)));
        assert_eq!(eval("$response.body#/tags/0", &fixture), Ok(json!("cat")));
        assert_eq!(eval("$request.body#/name", &fixture), Ok(json!("fluffy")));
        assert!(matches!(
            eval("$response.body#/nope", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
    }

    #[test]
    fn an_earlier_step_is_readable_by_id() {
        let mut steps = BTreeMap::new();
        steps.insert(
            "findPet".to_owned(),
            StepState {
                exchange: Some(exchange()),
                outputs: BTreeMap::from([("pet".to_owned(), json!({ "id": 7 }))]),
            },
        );
        let fixture = Fixture {
            steps,
            ..Fixture::default()
        };
        assert_eq!(
            eval("$steps.findPet.outputs.pet", &fixture),
            Ok(json!({ "id": 7 }))
        );
        assert_eq!(
            eval("$steps.findPet.outputs.pet#/id", &fixture),
            Ok(json!(7))
        );
        assert_eq!(eval("$steps.findPet.statusCode", &fixture), Ok(json!(200)));
        assert_eq!(
            eval("$steps.findPet.response.body#/id", &fixture),
            Ok(json!(7))
        );
        assert!(matches!(
            eval("$steps.nope.outputs.pet", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
    }

    #[test]
    fn a_finished_workflow_is_readable_by_field_and_by_shorthand() {
        let workflows = BTreeMap::from([(
            "authenticate".to_owned(),
            WorkflowState {
                inputs: json!({ "user": "ada" }),
                outputs: BTreeMap::from([("token".to_owned(), json!("abc"))]),
            },
        )]);
        let fixture = Fixture {
            workflows,
            ..Fixture::default()
        };
        assert_eq!(
            eval("$workflows.authenticate.outputs.token", &fixture),
            Ok(json!("abc"))
        );
        // What it was called with is readable too, which is a field of
        // its own rather than another way of naming an output.
        assert_eq!(
            eval("$workflows.authenticate.inputs.user", &fixture),
            Ok(json!("ada"))
        );
        assert_eq!(
            eval("$workflows.authenticate.token", &fixture),
            Ok(json!("abc")),
            "the shorthand names an output"
        );
        assert!(matches!(
            eval("$workflows.authenticate.inputs.nope", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
    }

    #[test]
    fn the_description_can_name_itself() {
        let fixture = Fixture {
            self_: Some("https://example.com/workflows.arazzo.yaml".to_owned()),
            ..Fixture::default()
        };
        assert_eq!(
            eval("$self", &fixture),
            Ok(json!("https://example.com/workflows.arazzo.yaml"))
        );
        // A description that sets no `$self` says so rather than
        // producing an empty string.
        assert!(matches!(
            eval("$self", &Fixture::default()),
            Err(ExpressionError::Missing { .. })
        ));
    }

    #[test]
    fn what_is_not_there_says_so_rather_than_reading_as_null() {
        let fixture = Fixture::default();
        assert_eq!(
            eval("$inputs.nope", &fixture).unwrap_err().to_string(),
            "`$inputs.nope` names an input named `nope`"
        );
        // No exchange yet: the step has sent nothing.
        assert!(matches!(
            eval("$statusCode", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
        assert_eq!(
            eval("$nonsense.x", &fixture),
            Err(ExpressionError::Unknown("$nonsense".to_owned()))
        );
        assert!(matches!(
            eval("$message.payload", &fixture),
            Err(ExpressionError::Unsupported(_))
        ));
    }

    #[test]
    fn a_response_that_has_not_arrived_is_not_a_status_code() {
        let fixture = Fixture {
            here: Some(Exchange {
                response: None,
                response_body: None,
                ..exchange()
            }),
            ..Fixture::default()
        };
        assert!(matches!(
            eval("$statusCode", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
        assert!(matches!(
            eval("$response.body", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
        assert!(matches!(
            eval("$response.header.x", &fixture),
            Err(ExpressionError::Missing { .. })
        ));
    }

    #[test]
    fn naming_a_workflow_or_a_steps_exchange_that_is_not_there_says_which() {
        let mut steps = BTreeMap::new();
        // A step that called a workflow has outputs but no exchange.
        steps.insert(
            "call".to_owned(),
            StepState {
                exchange: None,
                outputs: BTreeMap::from([("token".to_owned(), json!("abc"))]),
            },
        );
        let fixture = Fixture {
            steps,
            ..Fixture::default()
        };
        assert_eq!(
            eval("$workflows.nope.outputs.token", &fixture)
                .unwrap_err()
                .to_string(),
            "`$workflows.nope.outputs.token` names workflow `nope`, which has not run"
        );
        assert_eq!(
            eval("$steps.call.outputs.token", &fixture),
            Ok(json!("abc"))
        );
        assert_eq!(
            eval("$steps.call.statusCode", &fixture)
                .unwrap_err()
                .to_string(),
            "`$steps.call.statusCode` names the exchange of step `call`, which has none"
        );
    }

    #[test]
    fn a_string_carries_expressions_inside_it() {
        let fixture = Fixture {
            here: Some(exchange()),
            ..Fixture::default()
        };
        let scope = fixture.scope();
        assert_eq!(
            interpolate("Bearer {$inputs.auth.token}", &scope),
            Ok("Bearer abc".to_owned())
        );
        assert_eq!(
            interpolate(
                "/pets/{$inputs.petId}/tags/{$response.body#/tags/0}",
                &scope
            ),
            Ok("/pets/7/tags/cat".to_owned())
        );
        // A number goes in as its JSON, and text with no expression is
        // left exactly as it was.
        assert_eq!(
            interpolate("n={$request.query.limit}", &scope),
            Ok("n=10".to_owned())
        );
        assert_eq!(
            interpolate("nothing here", &scope),
            Ok("nothing here".to_owned())
        );
        assert_eq!(
            interpolate("unclosed {$inputs.petId", &scope),
            Ok("unclosed {$inputs.petId".to_owned())
        );
    }

    #[test]
    fn a_literal_is_told_apart_from_an_expression() {
        assert!(is_expression("$inputs.x"));
        assert!(!is_expression("plain"));
    }
}
