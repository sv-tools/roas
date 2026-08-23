//! One [`ToRequestView`](crate::ToRequestView) impl per framework, each
//! behind its own feature so a build pays for the frameworks it uses.
//!
//! Every adapter converts the request *head* only. See
//! [`crate::request`] for why the body is the caller's to supply.

#[cfg(feature = "actix-web")]
mod actix;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "poem")]
mod poem;
#[cfg(feature = "reqwest")]
mod reqwest;
#[cfg(feature = "rocket")]
mod rocket;
#[cfg(feature = "salvo")]
mod salvo;
