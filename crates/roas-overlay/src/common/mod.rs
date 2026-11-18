//! Version-agnostic helpers shared across overlay versions.

#[cfg(any(feature = "v1_1", feature = "v1_2"))]
pub(crate) mod action;
pub mod apply;
pub mod extensions;
