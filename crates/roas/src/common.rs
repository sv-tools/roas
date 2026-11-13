//! Common Library

pub mod bool_or;
pub(crate) mod collapse;
pub mod extensions;
pub mod formats;
pub(crate) mod helpers;
pub(crate) mod merge;
pub mod reference;
#[cfg(any(feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
pub(crate) mod schema_keywords;
#[cfg(any(feature = "v2", feature = "v3_0", feature = "v3_1", feature = "v3_2"))]
pub(crate) mod schema_pointer;
