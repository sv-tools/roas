//! OpenAPI Specification

pub mod common;
pub mod validation;

#[cfg(feature = "v2")]
pub mod v2;

#[cfg(feature = "v3_0")]
pub mod v3_0;
