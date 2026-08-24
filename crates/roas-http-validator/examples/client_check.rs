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
                # `roas` models `multipleOf` as an `f64`, so the step
                # stands for any decimal that rounds to it — enough to
                # disprove divisibility, never enough to prove it.
                weight: { type: number, multipleOf: 0.5 }
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
        (
            "a call carrying a value nothing here can decide",
            r#"{"name":"Rex","weight":2.5}"#,
            true,
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
            Verdict::Matches => {
                println!("  ✓ matches the description — safe to send");
                // client.execute(request)?;
            }
            // Not the same as matching, and a contract test that
            // treated it as such would be reporting a check it never
            // made. What to do about it is the caller's call: fail the
            // test, warn, or send anyway.
            Verdict::Undetermined(notes) => {
                println!("  ? nothing found wrong, but not everything could be checked");
                for note in notes {
                    println!("    {note}");
                }
            }
            Verdict::Violates(problems) => {
                for problem in problems {
                    println!("  ✗ {problem}");
                }
            }
        }
    }

    Ok(())
}

/// What a contract check can honestly conclude.
enum Verdict {
    /// Every check ran, and the call passed all of them.
    Matches,
    /// Nothing was found wrong — but some check could not be made, so
    /// "nothing found wrong" is not "nothing wrong".
    Undetermined(Vec<String>),
    /// The call does not match the description.
    Violates(Vec<String>),
}

/// Judge a request this client is about to make.
fn check(validator: &Validator, request: &reqwest::blocking::Request) -> Verdict {
    // No `with_body` here: the adapter already has the bytes.
    let report = match validator.validate(&request.request_view()) {
        Ok(report) => report,
        // The description does not describe this call at all, which for
        // a client is as much a mismatch as a bad field.
        Err(error) => return Verdict::Violates(vec![error.to_string()]),
    };

    let problems: Vec<String> = report.violations().map(ToString::to_string).collect();
    if !problems.is_empty() {
        return Verdict::Violates(problems);
    }
    let notes: Vec<String> = report.unchecked().map(ToString::to_string).collect();
    if notes.is_empty() {
        Verdict::Matches
    } else {
        Verdict::Undetermined(notes)
    }
}
