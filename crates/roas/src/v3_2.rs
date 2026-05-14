//! Implementation of OpenAPI v3.2.X Specification
//!
//! Full specification can be found [here](https://spec.openapis.org/oas/v3.2.0.html).
pub mod callback;
pub mod components;
pub mod discriminator;
pub mod example;
pub mod external_documentation;
#[cfg(feature = "v3_1")]
pub mod from_v3_1;
pub mod header;
pub mod info;
pub mod link;
pub mod media_type;
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
