//! OpenAPI Specification

// Everything internal here is reached through a versioned
// document: the validation impls, the walkers, the shared
// helpers. Build with no version feature at all and none of it
// is called — which is that configuration saying so, not code
// nobody uses. Every real configuration keeps the lint.
#![cfg_attr(
    not(any(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2")),
    allow(dead_code)
)]

pub mod common;
pub mod loader;
pub mod merge;
pub mod validation;

#[cfg(feature = "v2")]
pub mod v2;

#[cfg(feature = "v3_0")]
pub mod v3_0;

#[cfg(feature = "v3_1")]
pub mod v3_1;

#[cfg(feature = "v3_2")]
pub mod v3_2;
