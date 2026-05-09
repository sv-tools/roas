//! Implementation of v2.0 Specification
//!
//! Full specification can be found [here](https://spec.openapis.org/oas/v2.0).
//!
//! # Intentional permissive deviations from the spec
//!
//! The following are *additions* not present in the OAS 2.0 / JSON Schema
//! draft-04 spec, kept on purpose:
//!
//! * `Schema` accepts a `null` type. Draft-04 has no `null` type (that
//!   arrived in draft-06). This keeps the v2 model interoperable with
//!   tools that emit JSON Schema 2020-12 idioms.
//! * `PathItem` accepts arbitrary HTTP method names (e.g. `search`,
//!   `trace`) in addition to the closed `get/put/post/delete/options/head/patch`
//!   set defined in the spec.
//!
//! Both deviations are documented at the relevant types and are not
//! flagged by `validate()`.

pub mod external_documentation;
pub mod header;
pub mod info;
pub mod items;
pub mod operation;
pub mod parameter;
pub mod path_item;
pub mod reference;
pub mod response;
pub mod schema;
pub mod security_scheme;
pub mod spec;
pub mod tag;
pub(crate) mod validation;
pub mod xml;
