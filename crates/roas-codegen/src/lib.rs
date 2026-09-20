//! Generate Rust and Go types from OpenAPI descriptions.
//!
//! The pipeline is: detect the version, parse the typed model, normalize
//! everything to OpenAPI 3.2 while recording what that loses, validate,
//! and only then generate. This release covers everything up to and
//! including "only then": [`SourceDocument::parse`] and [`validate`] turn
//! bytes into a [`ValidatedSourceDocument`], and
//! [`ValidatedSourceDocument::diagnostics`] reports what the normalization
//! changed and what the first release will not generate. Emission comes
//! next.
//!
//! ```no_run
//! use roas_codegen::{Input, SourceDocument, validate};
//! use url::Url;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("openapi.json")?;
//! let document = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///openapi.json")?)?;
//! let validated = validate(document, Default::default())?;
//! for diagnostic in validated.diagnostics() {
//!     println!("{diagnostic}");
//! }
//! # Ok(()) }
//! ```

mod config;
mod diagnostic;
mod report;
mod source;
mod validate;

pub use config::{
    Config, ConfigError, ConfigFile, FieldConfig, GoConfig, GoConfigFile, Layout, RustConfig,
    RustConfigFile, Substitution, Target, TypeConfig, TypeSpelling,
};
pub use diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
pub use source::{Fidelity, Input, SourceDocument, SourceError, SourceVersion};
pub use validate::{SourceValidationError, ValidatedSourceDocument, validate};
