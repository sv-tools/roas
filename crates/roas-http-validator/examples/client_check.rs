//! Checking a request *before* sending it.
//!
//! ```text
//! cargo run -p roas-http-validator --features reqwest --example client_check
//! ```
//!
//! The other half of what a description is for. A server asks "is this
//! request one I described?"; a client asks "is the call I am about to
//! make one the API described?" — which is the question a contract test
//! wants answered, and it wants it answered without a server.
//!
//! `reqwest` is the one adapter that supplies the body itself: a
//! non-streaming reqwest body is already bytes in memory, so there is
//! nothing to buffer and nothing for the caller to decide.

use roas_http_validator::{ToRequestView, Validator};

const PETSTORE: &str = r#"
openapi: 3.2.0
info: { title: Pets, version: 1.0.0 }
servers:
  - url: https://api.example.com/v1
paths:
  /pets:
    post:
      operationId: createPet
      parameters:
        - name: X-Request-Id
          in: header
          required: true
          schema: { type: string }
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [name]
              properties:
                name: { type: string, minLength: 1 }
                age: { type: integer, minimum: 0 }
"#;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let validator = Validator::new(serde_yaml_ng::from_str(PETSTORE)?);
    let client = reqwest::blocking::Client::new();

    let calls = [
        (
            "a call that matches the description",
            r#"{"name":"Rex","age":4}"#,
            true,
        ),
        ("a body missing a required field", r#"{"age":4}"#, true),
        (
            "a call that forgot the required header",
            r#"{"name":"Rex"}"#,
            false,
        ),
    ];

    for (what, body, with_header) in calls {
        let mut request = client
            .post("https://api.example.com/v1/pets")
            .header("content-type", "application/json");
        if with_header {
            request = request.header("x-request-id", "abc-123");
        }
        let request = request.body(body).build()?;

        println!("\n{what}");
        match check(&validator, &request) {
            Ok(()) => {
                println!("  ✓ matches the description — safe to send");
                // client.execute(request)?;
            }
            Err(problems) => {
                for problem in problems {
                    println!("  ✗ {problem}");
                }
            }
        }
    }

    Ok(())
}

/// Whatever is wrong with a request this client is about to make.
///
/// # Errors
///
/// The problems found, so a test can fail on them.
fn check(validator: &Validator, request: &reqwest::blocking::Request) -> Result<(), Vec<String>> {
    // No `with_body` here: the adapter already has the bytes.
    let report = validator
        .validate(&request.request_view())
        .map_err(|error| vec![error.to_string()])?;

    let problems: Vec<String> = report.violations().map(ToString::to_string).collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}
