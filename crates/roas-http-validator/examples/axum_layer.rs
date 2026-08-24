//! Validating every request an axum service receives, as middleware.
//!
//! ```text
//! cargo run -p roas-http-validator --features http --example axum_layer
//! ```
//!
//! `axum::extract::Request` *is* `http::Request`, so the `http` adapter
//! already covers it — and covers warp, tonic and plain hyper with it.
//!
//! The interesting part is the body. A framework body is a stream, and
//! validating one means buffering it; how much, and whether at all, is
//! this middleware's decision rather than the crate's. That is why the
//! adapters convert the head and leave `with_body` to the caller: here
//! the buffering is explicit, capped, and undone afterwards so the
//! handler still receives a body it can read.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use roas_http_validator::{RoutingError, ToRequestView, Validator};
use tower::ServiceExt;

/// The most this middleware is willing to hold in memory to check a
/// body. A request above it is refused rather than buffered.
const MAX_BODY: usize = 64 * 1024;

const PETSTORE: &str = r#"
openapi: 3.2.0
info: { title: Pets, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - name: limit
          in: query
          schema: { type: integer, minimum: 1, maximum: 100 }
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
                name: { type: string, minLength: 1 }
"#;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let validator = Arc::new(Validator::new(serde_yaml_ng::from_str(PETSTORE)?));

    let app = Router::new()
        .route("/pets", get(list_pets).post(create_pet))
        .layer(from_fn_with_state(validator, validate));

    // Driven directly rather than served on a port, so the example runs
    // and finishes. A real service would `axum::serve` this router.
    for (method, uri, body) in [
        ("GET", "/pets?limit=10", None),
        ("GET", "/pets?limit=1000", None),
        ("POST", "/pets", Some(r#"{"name":"Rex"}"#)),
        ("POST", "/pets", Some(r#"{"age":4}"#)),
        ("DELETE", "/pets", None),
    ] {
        let mut request = Request::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(body.map_or_else(Body::empty, Body::from))?;

        let response = app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY).await?;

        println!("\n{method} {uri}");
        println!("  → {status}");
        for line in String::from_utf8_lossy(&bytes).lines() {
            println!("    {line}");
        }
    }

    Ok(())
}

/// Judge the request, then hand it on unchanged.
async fn validate(
    State(validator): State<Arc<Validator>>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();

    // Buffering happens here, deliberately and with a limit.
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            "body too large to validate\n",
        )
            .into_response();
    };

    // `parts` is `http::request::Parts`, which the `http` adapter reads
    // without copying anything out of it.
    let view = parts.request_view().with_body(bytes.as_ref());

    let outcome = match validator.validate(&view) {
        Err(RoutingError::PathNotFound { .. }) => {
            Some((StatusCode::NOT_FOUND, "no such path\n".to_owned()))
        }
        Err(RoutingError::MethodNotAllowed { allowed, .. }) => Some((
            StatusCode::METHOD_NOT_ALLOWED,
            format!("allowed: {}\n", allowed.join(", ")),
        )),
        Err(RoutingError::Unresolved { reference, .. }) => Some((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("the description references {reference}, which is missing\n"),
        )),
        // `RoutingError` is `#[non_exhaustive]`; anything new is a
        // refusal rather than a request that quietly gets through.
        Err(other) => Some((StatusCode::INTERNAL_SERVER_ERROR, format!("{other}\n"))),
        Ok(report) => {
            // Only definite violations refuse the request. What could
            // not be checked is logged and let through — a choice this
            // middleware makes, not one the crate makes for it.
            for note in report.unchecked() {
                eprintln!("note: {note}");
            }
            let violations: Vec<String> = report.violations().map(ToString::to_string).collect();
            (!violations.is_empty()).then(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("{}\n", violations.join("\n")),
                )
            })
        }
    };
    if let Some((status, message)) = outcome {
        return (status, message).into_response();
    }

    // Put back what was taken apart, so the handler sees a whole
    // request with a readable body.
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

async fn list_pets() -> &'static str {
    "[]\n"
}

async fn create_pet() -> (StatusCode, &'static str) {
    (StatusCode::CREATED, "created\n")
}
