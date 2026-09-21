//! Generate Rust and Go types from OpenAPI descriptions.
//!
//! The pipeline is: detect the version, parse the typed model, normalize
//! everything to OpenAPI 3.2 while recording what that loses, validate,
//! lower every schema to a representation where the hard decisions are
//! already made, and render. [`SourceDocument::parse`] and [`validate`]
//! turn bytes into a [`ValidatedSourceDocument`]; [`generate`] turns that
//! and a [`Config`] into a [`Generation`]: files in memory, every
//! diagnostic, and the dependency list the generated code needs. The
//! Rust backend is here; Go is next.
//!
//! ```no_run
//! use roas_codegen::{ConfigFile, Input, SourceDocument, Target, generate, validate};
//! use url::Url;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("openapi.json")?;
//! let document = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///openapi.json")?)?;
//! let document = validate(document, Default::default())?;
//! let config = ConfigFile { target: Some(Target::Rust), ..Default::default() }.build()?;
//! let generation = generate(&document, &config)?;
//! for file in &generation.files {
//!     println!("{}: {} bytes", file.path.display(), file.contents.len());
//! }
//! for diagnostic in &generation.diagnostics {
//!     eprintln!("{diagnostic}");
//! }
//! # Ok(()) }
//! ```

mod config;
mod diagnostic;
mod front;
mod generate;
mod ir;
mod lower;
mod report;
mod rust;
mod source;
mod validate;

pub use config::{
    Config, ConfigError, ConfigFile, FieldConfig, GoConfig, GoConfigFile, Layout, RustConfig,
    RustConfigFile, Substitution, Target, TypeConfig, TypeSpelling,
};
pub use diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
pub use generate::{Dependencies, GenerateError, GeneratedFile, Generation, generate};
pub use rust::RustDependency;
pub use source::{Fidelity, Input, SourceDocument, SourceError, SourceVersion};
pub use validate::{SourceValidationError, ValidatedSourceDocument, validate};
