//! Version-agnostic helpers shared by every AsyncAPI version module.

pub mod bindings;
pub mod extensions;
pub(crate) mod pointer;
pub mod reference;
pub(crate) mod resolve;
pub mod runtime_expression;
#[cfg(any(feature = "v2_6", feature = "v3_0", feature = "v3_1"))]
pub(crate) mod schema_pointer;

pub use resolve::Resolution;
