//! Version-agnostic helpers shared across overlay versions.

#[cfg(feature = "v1_1")]
pub(crate) mod action;
pub mod apply;
pub mod extensions;
