//! The same description, the same request, six request types.
//!
//! Each framework builds the request in its own way and hands over its
//! own type; every one of them must reach the same verdict. That is the
//! whole claim this crate makes about `ToRequestView`, so it is tested
//! per framework rather than argued for in a doc comment.
//!
//! Run with `--all-features` to exercise every adapter; each test is
//! behind the feature that brings its framework in.

#![cfg(any(
    feature = "http",
    feature = "actix-web",
    feature = "poem",
    feature = "salvo",
    feature = "rocket",
    feature = "reqwest",
))]

use roas_http_validator::{Options, ToRequestView, Validator};

/// The request every adapter builds: a templated path, a query
/// parameter that must be coerced to an integer, an exploded array, a
/// required header and a cookie.
const PATH_AND_QUERY: &str = "/pets?limit=10&tags=cute&tags=small";

fn validator() -> Validator {
    let spec = serde_yaml_ng::from_str(include_str!("data/petstore.openapi.yaml"))
        .expect("the description must parse");
    // The description advertises `/v1`; these requests arrive without it.
    Validator::with_options(spec, Options::new())
}

/// Every adapter must read the same request out of its own type.
fn assert_reads_the_request(request: &impl ToRequestView) {
    let view = request.request_view();

    assert_eq!(view.method.to_ascii_uppercase(), "GET");
    assert_eq!(view.path, "/pets");
    assert_eq!(view.query.as_deref(), Some("limit=10&tags=cute&tags=small"));
    assert_eq!(view.header("x-request-id"), Some("abc-123"));
    assert_eq!(view.header("X-Request-Id"), Some("abc-123"));
    assert_eq!(
        view.cookies(),
        [("session".to_owned(), "opensesame".to_owned())]
    );

    let report = validator()
        .validate(&view)
        .expect("the description describes GET /pets");
    assert!(report.is_valid(), "{report}");
    assert_eq!(report.operation_id.as_deref(), Some("listPets"));
}

/// And must let the same request be judged invalid for the same reason:
/// `limit` is above its maximum, and the required header is absent.
fn assert_reports_the_same_errors(request: &impl ToRequestView) {
    let report = validator()
        .validate(&request.request_view())
        .expect("the description describes GET /pets");
    let errors: Vec<String> = report.errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        errors,
        [
            "header parameter \"X-Request-Id\": is required and was not sent",
            "query parameter \"limit\": 1000 is above maximum 100",
        ],
    );
}

/// The body is never part of a framework's conversion — the caller
/// supplies it — so every adapter must accept one the same way.
fn assert_takes_a_body(request: &impl ToRequestView) {
    let body = br#"{"tag":"cute"}"#;
    let view = request.request_view().with_body(body.as_slice());
    let report = validator()
        .validate(&view)
        .expect("the description describes POST /pets");
    let errors: Vec<String> = report.errors.iter().map(ToString::to_string).collect();
    assert_eq!(errors, ["body at /name: is required and was not sent"]);
}

// ── http: axum, warp, tonic, hyper, reqwest ──────────────────────────

#[cfg(feature = "http")]
mod http_crate {
    use super::*;

    fn get() -> http::Request<()> {
        http::Request::builder()
            .method("GET")
            .uri(PATH_AND_QUERY)
            .header("x-request-id", "abc-123")
            .header("cookie", "session=opensesame")
            .body(())
            .expect("the request must build")
    }

    #[test]
    fn an_http_request_is_read_as_it_arrived() {
        assert_reads_the_request(&get());
    }

    #[test]
    fn the_parts_of_an_http_request_are_read_the_same_way() {
        // What a middleware holds once it has split the body off.
        let (parts, ()) = get().into_parts();
        assert_reads_the_request(&parts);
    }

    #[test]
    fn an_http_request_reports_what_is_wrong_with_it() {
        let request = http::Request::builder()
            .method("GET")
            .uri("/pets?limit=1000")
            .body(())
            .expect("the request must build");
        assert_reports_the_same_errors(&request);
    }

    #[test]
    fn an_http_request_takes_a_body_from_the_caller() {
        let request = http::Request::builder()
            .method("POST")
            .uri("/pets")
            .header("x-request-id", "abc-123")
            .header("content-type", "application/json")
            .body(())
            .expect("the request must build");
        assert_takes_a_body(&request);
    }

    #[test]
    fn a_header_that_is_not_utf8_is_carried_through_rather_than_dropped() {
        let request = http::Request::builder()
            .method("GET")
            .uri("/pets")
            .header(
                "x-request-id",
                http::HeaderValue::from_bytes(&[0xff, 0xfe]).expect("bytes are a valid value"),
            )
            .body(())
            .expect("the request must build");
        // Present, so the `required` check passes; lossy, so the schema
        // judges what actually arrived.
        assert!(request.request_view().header("x-request-id").is_some());
        let report = validator()
            .validate(&request.request_view())
            .expect("the description describes GET /pets");
        assert!(report.is_valid(), "{report}");
    }
}

// ── axum ─────────────────────────────────────────────────────────────

#[cfg(feature = "http")]
mod axum_framework {
    use super::*;

    #[test]
    fn an_axum_request_is_an_http_request_and_needs_no_adapter_of_its_own() {
        // `axum::extract::Request` is a type alias for
        // `http::Request<axum::body::Body>`, so the `http` adapter
        // already covers it — this is that claim, compiled.
        let request: axum::extract::Request = http::Request::builder()
            .method("GET")
            .uri(PATH_AND_QUERY)
            .header("x-request-id", "abc-123")
            .header("cookie", "session=opensesame")
            .body(axum::body::Body::empty())
            .expect("the request must build");
        assert_reads_the_request(&request);
    }
}

// ── actix-web ────────────────────────────────────────────────────────

#[cfg(feature = "actix-web")]
mod actix_framework {
    use super::*;
    use actix_web::test::TestRequest;

    #[test]
    fn an_actix_request_is_read_as_it_arrived() {
        let request = TestRequest::get()
            .uri(PATH_AND_QUERY)
            .insert_header(("x-request-id", "abc-123"))
            .insert_header(("cookie", "session=opensesame"))
            .to_http_request();
        assert_reads_the_request(&request);
    }

    #[test]
    fn an_actix_request_reports_what_is_wrong_with_it() {
        let request = TestRequest::get().uri("/pets?limit=1000").to_http_request();
        assert_reports_the_same_errors(&request);
    }

    #[test]
    fn an_actix_request_takes_a_body_from_the_caller() {
        let request = TestRequest::post()
            .uri("/pets")
            .insert_header(("x-request-id", "abc-123"))
            .insert_header(("content-type", "application/json"))
            .to_http_request();
        assert_takes_a_body(&request);
    }

    #[test]
    fn an_actix_request_without_a_query_has_none() {
        let request = TestRequest::get().uri("/pets/mine").to_http_request();
        assert_eq!(request.request_view().query, None);
    }
}

// ── poem ─────────────────────────────────────────────────────────────

#[cfg(feature = "poem")]
mod poem_framework {
    use super::*;

    #[test]
    fn a_poem_request_is_read_as_it_arrived() {
        let request = poem::Request::builder()
            .method(poem::http::Method::GET)
            .uri(PATH_AND_QUERY.parse().expect("the uri must parse"))
            .header("x-request-id", "abc-123")
            .header("cookie", "session=opensesame")
            .finish();
        assert_reads_the_request(&request);
    }

    #[test]
    fn a_poem_request_reports_what_is_wrong_with_it() {
        let request = poem::Request::builder()
            .method(poem::http::Method::GET)
            .uri("/pets?limit=1000".parse().expect("the uri must parse"))
            .finish();
        assert_reports_the_same_errors(&request);
    }

    #[test]
    fn a_poem_request_takes_a_body_from_the_caller() {
        let request = poem::Request::builder()
            .method(poem::http::Method::POST)
            .uri("/pets".parse().expect("the uri must parse"))
            .header("x-request-id", "abc-123")
            .header("content-type", "application/json")
            .finish();
        assert_takes_a_body(&request);
    }
}

// ── salvo ────────────────────────────────────────────────────────────

#[cfg(feature = "salvo")]
mod salvo_framework {
    use super::*;
    use salvo_core::http::header::{HeaderName, HeaderValue};

    fn request(method: &str, uri: &str, headers: &[(&str, &str)]) -> salvo_core::http::Request {
        let mut request = salvo_core::http::Request::default();
        *request.method_mut() = method.parse().expect("the method must parse");
        *request.uri_mut() = uri.parse().expect("the uri must parse");
        for (name, value) in headers {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).expect("the header name must parse"),
                HeaderValue::from_str(value).expect("the header value must parse"),
            );
        }
        request
    }

    #[test]
    fn a_salvo_request_is_read_as_it_arrived() {
        let request = request(
            "GET",
            PATH_AND_QUERY,
            &[
                ("x-request-id", "abc-123"),
                ("cookie", "session=opensesame"),
            ],
        );
        assert_reads_the_request(&request);
    }

    #[test]
    fn a_salvo_request_reports_what_is_wrong_with_it() {
        assert_reports_the_same_errors(&request("GET", "/pets?limit=1000", &[]));
    }

    #[test]
    fn a_salvo_request_takes_a_body_from_the_caller() {
        assert_takes_a_body(&request(
            "POST",
            "/pets",
            &[
                ("x-request-id", "abc-123"),
                ("content-type", "application/json"),
            ],
        ));
    }
}

// ── rocket ───────────────────────────────────────────────────────────

#[cfg(feature = "rocket")]
mod rocket_framework {
    use super::*;
    use rocket::local::blocking::Client;

    /// Rocket's `Request` cannot be built on its own — it belongs to an
    /// instance — so a local client is what stands one up.
    fn client() -> Client {
        Client::untracked(rocket::build()).expect("the local client must build")
    }

    #[test]
    fn a_rocket_request_is_read_as_it_arrived() {
        let client = client();
        let local = client
            .get(PATH_AND_QUERY)
            .header(rocket::http::Header::new("x-request-id", "abc-123"))
            .header(rocket::http::Header::new("cookie", "session=opensesame"));
        assert_reads_the_request(local.inner());
    }

    #[test]
    fn a_rocket_request_reports_what_is_wrong_with_it() {
        let client = client();
        let local = client.get("/pets?limit=1000");
        assert_reports_the_same_errors(local.inner());
    }

    #[test]
    fn a_rocket_request_takes_a_body_from_the_caller() {
        let client = client();
        let local = client
            .post("/pets")
            .header(rocket::http::Header::new("x-request-id", "abc-123"))
            .header(rocket::http::Header::new(
                "content-type",
                "application/json",
            ));
        assert_takes_a_body(local.inner());
    }
}

// ── reqwest: the client's side ───────────────────────────────────────

#[cfg(feature = "reqwest")]
mod reqwest_client {
    use super::*;

    fn get(url: &str) -> reqwest::Request {
        reqwest::Client::new()
            .get(url)
            .header("x-request-id", "abc-123")
            .header("cookie", "session=opensesame")
            .build()
            .expect("the request must build")
    }

    #[test]
    fn a_reqwest_request_is_read_as_it_arrived() {
        assert_reads_the_request(&get(&format!("https://api.example.com{PATH_AND_QUERY}")));
    }

    #[test]
    fn a_blocking_reqwest_request_is_read_the_same_way() {
        let request = reqwest::blocking::Client::new()
            .get(format!("https://api.example.com{PATH_AND_QUERY}"))
            .header("x-request-id", "abc-123")
            .header("cookie", "session=opensesame")
            .build()
            .expect("the request must build");
        assert_reads_the_request(&request);
    }

    #[test]
    fn a_reqwest_request_reports_what_is_wrong_with_it() {
        let request = reqwest::Client::new()
            .get("https://api.example.com/pets?limit=1000")
            .build()
            .expect("the request must build");
        assert_reports_the_same_errors(&request);
    }

    #[test]
    fn a_reqwest_request_takes_a_body_from_the_caller() {
        // A request built without one behaves like every other adapter's.
        let request = reqwest::Client::new()
            .post("https://api.example.com/pets")
            .header("x-request-id", "abc-123")
            .header("content-type", "application/json")
            .build()
            .expect("the request must build");
        assert_eq!(request.request_view().body, None);
        assert_takes_a_body(&request);
    }

    #[test]
    fn a_reqwest_body_needs_no_buffering_because_it_is_already_bytes() {
        // The one adapter that supplies the body itself: nothing here
        // is a stream, so there is nothing for the caller to decide.
        let request = reqwest::Client::new()
            .post("https://api.example.com/pets")
            .header("x-request-id", "abc-123")
            .header("content-type", "application/json")
            .body(r#"{"tag":"cute"}"#)
            .build()
            .expect("the request must build");

        let report = validator()
            .validate(&request.request_view())
            .expect("the description describes POST /pets");
        let errors: Vec<String> = report.errors.iter().map(ToString::to_string).collect();
        assert_eq!(errors, ["body at /name: is required and was not sent"]);
    }
}
