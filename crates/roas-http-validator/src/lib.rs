//! Validates HTTP requests against an OpenAPI description.
//!
//! [`roas`](https://crates.io/crates/roas) parses a description and
//! checks that the *description* is well formed. This checks that a
//! *request* is what the description says it should be: the path is one
//! the description names, the method is one that path offers, every
//! required parameter arrived, each one is the type its Schema Object
//! declares, and the body is what the Request Body Object describes.
//!
//! ```
//! use roas_http_validator::{RequestView, Validator};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let spec = serde_json::from_str(r#"{
//! #   "openapi": "3.2.0",
//! #   "info": { "title": "Pets", "version": "1.0.0" },
//! #   "paths": { "/pets": { "get": { "operationId": "listPets", "parameters": [
//! #     { "name": "limit", "in": "query", "schema": { "type": "integer", "maximum": 100 } }
//! #   ] } } }
//! # }"#)?;
//! let validator = Validator::new(spec);
//!
//! let request = RequestView::new("GET", "/pets").with_query("limit=1000");
//! let report = validator.validate(&request)?;
//!
//! assert!(!report.is_valid());
//! assert_eq!(
//!     report.errors[0].to_string(),
//!     "query parameter \"limit\": 1000 is above maximum 100",
//! );
//! # Ok(()) }
//! ```
//!
//! ## Examples
//!
//! The repository carries three runnable ones: `validate` for the shape
//! of the whole crate, `axum_layer` for the same thing as middleware
//! (and for what buffering a body actually looks like), and
//! `client_check` for asking whether a call you are about to *make*
//! matches the description.
//!
//! ## Which request type
//!
//! None of them, and all of them. Rust has no single HTTP request type
//! to validate: `http::Request` comes closest, but it is generic over a
//! body that is usually a stream, and it is version-split — actix-web 4
//! is on `http` 0.2 while hyper 1, axum 0.8 and reqwest are on 1.x, so
//! their `HeaderMap`s are different types. Taking either one would shut
//! out half the ecosystem.
//!
//! So this crate takes [`RequestView`], the small set of things an
//! OpenAPI description actually talks about, and each framework gets a
//! [`ToRequestView`] impl behind its own feature:
//!
//! | Feature | Covers |
//! | --- | --- |
//! | `http` | `http::Request`, `http::request::Parts` — and so axum, warp, tonic, hyper |
//! | `actix-web` | `actix_web::HttpRequest` |
//! | `poem` | `poem::Request` |
//! | `salvo` | `salvo_core::http::Request` |
//! | `rocket` | `rocket::Request` |
//! | `reqwest` | `reqwest::Request` and its blocking twin — the client's side, for checking an outgoing call |
//!
//! The body is not part of that conversion. A framework body is a
//! stream, and validating one means buffering it — how much, and
//! whether at all, is the caller's decision, so the adapters convert
//! the head and [`RequestView::with_body`] takes the bytes. The one
//! exception is `reqwest`, where a non-streaming body is already bytes
//! in memory and there is nothing to buffer.
//!
//! ## Versions
//!
//! The interpreter is v3.2. Enable `v3_1`, `v3_0` or `v2` to accept a
//! description written to an older version: it is upconverted through
//! `roas`'s own migrations first, so there is one interpreter rather
//! than four.
//!
//! ## Media types it does not read itself
//!
//! JSON, `application/x-www-form-urlencoded` and `text/*` are built in.
//! Anything else — `multipart/form-data`, XML — is reported rather than
//! guessed at, and [`Options::decoder`] is the way in: the bytes become
//! a value and the Schema Object judges it like any other.
//!
//! Those two are a hook rather than more built-ins on purpose.
//! Multipart would mean owning a boundary parser and buffering file
//! uploads, which is exactly where this crate leaves buffering to the
//! caller. XML has no specified mapping onto a schema instance at all —
//! OpenAPI's XML Object is serialization metadata for code generators —
//! so any translation is a choice, and taking the caller's beats
//! inventing one.
//!
//! ## What it does not check yet
//!
//! Response validation and security requirements.
//!
//! Everything a check could not judge is reported rather than passed
//! over, so a request never looks valid because nothing looked at it:
//! [`ErrorKind::Unsupported`] for what is not implemented,
//! [`ErrorKind::Unchecked`] for a description this crate can read but
//! cannot apply faithfully. [`ValidationReport::unchecked`] separates
//! both from what the request definitely got wrong.

mod body;
mod decimal;
mod decoder;
mod method;
mod parameter;
mod paths;
mod report;
mod request;
mod router;
mod schema;
mod validator;

mod adapters;

pub use decoder::Decoder;
pub use report::{ErrorKind, Location, RoutingError, ValidationError, ValidationReport};
pub use request::{RequestView, ToRequestView};
pub use validator::{Options, Validator};

impl Validator {
    /// Prepare a v3.1 description, upconverting it to v3.2 first.
    #[cfg(feature = "v3_1")]
    #[must_use]
    pub fn from_v3_1(spec: roas::v3_1::spec::Spec, options: Options) -> Self {
        Self::with_options(spec.into(), options)
    }

    /// Prepare a v3.0 description, upconverting it to v3.2 first.
    #[cfg(feature = "v3_0")]
    #[must_use]
    pub fn from_v3_0(spec: roas::v3_0::spec::Spec, options: Options) -> Self {
        let v3_1: roas::v3_1::spec::Spec = spec.into();
        Self::from_v3_1(v3_1, options)
    }

    /// Prepare a v2.0 (Swagger) description, upconverting it to v3.2
    /// first.
    #[cfg(feature = "v2")]
    #[must_use]
    pub fn from_v2(spec: roas::v2::spec::Spec, options: Options) -> Self {
        let v3_0: roas::v3_0::spec::Spec = spec.into();
        Self::from_v3_0(v3_0, options)
    }
}
