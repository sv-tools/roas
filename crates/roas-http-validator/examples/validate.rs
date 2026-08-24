//! The shape of the whole crate in one file: load a description, judge a
//! request, turn the verdict into a response.
//!
//! ```text
//! cargo run -p roas-http-validator --example validate
//! ```

use roas_http_validator::{Options, RequestView, RoutingError, Validator};

const PETSTORE: &str = r#"
openapi: 3.2.0
info:
  title: Pets
  version: 1.0.0
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - name: limit
          in: query
          schema:
            type: integer
            minimum: 1
            maximum: 100
        - name: tags
          in: query
          style: form
          explode: true
          schema:
            type: array
            items:
              type: string
    post:
      operationId: createPet
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [name]
              properties:
                name:
                  type: string
                  minLength: 1
                age:
                  type: integer
                  minimum: 0
  /pets/{petId}:
    get:
      operationId: getPet
      parameters:
        - name: petId
          in: path
          required: true
          schema:
            type: integer
            minimum: 1
"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = serde_yaml_ng::from_str(PETSTORE)?;

    // Built once and kept: preparing a validator walks the description,
    // while judging a request is a match and a few schema checks.
    let validator = Validator::with_options(
        spec,
        // OpenAPI does not forbid query parameters nobody described, so
        // this is off by default — but it is what catches `?limti=10`.
        Options::new().reject_undescribed_query_parameters(),
    );

    let requests = [
        // Fine: the path template matches and every value fits.
        RequestView::new("GET", "/pets/7"),
        // The description advertises `/v1`, and a request may or may
        // not still carry it — both match.
        RequestView::new("GET", "/v1/pets").with_query("limit=10&tags=cute&tags=small"),
        // `limit` is text on the wire; it is read as the integer its
        // schema declares before `maximum` judges it.
        RequestView::new("GET", "/pets").with_query("limit=1000"),
        RequestView::new("GET", "/pets").with_query("limit=lots"),
        // A typo that would otherwise silently do nothing.
        RequestView::new("GET", "/pets").with_query("limti=10"),
        // Path parameters are judged too.
        RequestView::new("GET", "/pets/rex"),
        // Bodies: the caller supplies the bytes, having decided to
        // buffer them.
        RequestView::new("POST", "/pets")
            .with_header("content-type", "application/json")
            .with_body(br#"{"name":"Rex","age":4}"#.as_slice()),
        RequestView::new("POST", "/pets")
            .with_header("content-type", "application/json")
            .with_body(br#"{"age":-1}"#.as_slice()),
        // Routing failures are a different answer from validation ones.
        RequestView::new("DELETE", "/pets"),
        RequestView::new("GET", "/unicorns"),
    ];

    for request in &requests {
        let query = request.query.as_deref().unwrap_or("");
        let separator = if query.is_empty() { "" } else { "?" };
        println!("\n{} {}{separator}{query}", request.method, request.path);
        println!("  → {}", answer(&validator, request));
    }

    Ok(())
}

/// What a server would send back, and why.
fn answer(validator: &Validator, request: &RequestView<'_>) -> String {
    let report = match validator.validate(request) {
        // The description says nothing about this path. A gateway might
        // pass it through instead of refusing it.
        Err(RoutingError::PathNotFound { .. }) => return "404 (no such path)".to_owned(),
        // It says something about the path but not this method — and
        // `allowed` is already in the form an `Allow` header wants.
        Err(RoutingError::MethodNotAllowed { allowed, .. }) => {
            return format!("405, Allow: {}", allowed.join(", "));
        }
        // The description itself could not be read this far. That is
        // the server's problem, not the client's.
        Err(RoutingError::Unresolved { reference, .. }) => {
            return format!("500 (description references {reference}, which is missing)");
        }
        Ok(report) => report,
        // `RoutingError` is `#[non_exhaustive]`, so a new way for a
        // request to be unroutable cannot silently become a 200.
        Err(other) => return format!("500 ({other})"),
    };

    if report.is_valid() {
        return format!("200 ({})", report.operation_id.as_deref().unwrap_or("ok"));
    }

    // Two kinds of error, and they deserve different answers. A
    // violation is the client's fault; something the crate could not
    // check is not, and whether to refuse the request on it is a
    // decision worth making deliberately rather than by accident.
    let violations: Vec<String> = report.violations().map(ToString::to_string).collect();
    let unchecked: Vec<String> = report.unchecked().map(ToString::to_string).collect();

    let mut answer = if violations.is_empty() {
        "200, but not everything could be checked".to_owned()
    } else {
        format!("400 ({} problem(s))", violations.len())
    };
    for violation in &violations {
        answer.push_str(&format!("\n      {violation}"));
    }
    for note in &unchecked {
        answer.push_str(&format!("\n      (not checked) {note}"));
    }
    answer
}
