//! Implementation of OpenAPI v3.0.X Specification
//!
//! Full specification can be found [here](https://spec.openapis.org/oas/v3.0.4).
//!
//! # Intentional permissive deviations
//!
//! * `PathItem` accepts arbitrary HTTP method names (e.g. `search`, `trace`)
//!   in addition to the closed `get/put/post/delete/options/head/patch/trace`
//!   set defined in the spec.
//! * `Schema` accepts a `null` type via `NullSchema`. OAS 3.0 has no `null`
//!   type (the recommended idiom is `nullable: true` on a typed schema; `null`
//!   was added as a real type in 3.1). Kept on purpose so v3.0 specs that
//!   borrowed the 2020-12 idiom round-trip cleanly.
pub mod callback;
pub mod collapse;
pub mod components;
pub mod discriminator;
pub mod example;
pub mod external_documentation;
#[cfg(feature = "v2")]
pub mod from_v2;
pub mod header;
pub mod info;
pub mod link;
pub mod media_type;
pub mod merge;
pub mod operation;
pub mod parameter;
pub mod path_item;
pub mod request_body;
pub mod response;
pub mod schema;
pub mod security_scheme;
pub mod server;
pub mod spec;
pub mod tag;
pub(crate) mod validation;
pub mod xml;
