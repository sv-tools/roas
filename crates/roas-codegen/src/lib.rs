//! Generate Rust and Go types from OpenAPI descriptions.
//!
//! The pipeline is: detect the version, parse the typed model, normalize
//! everything to OpenAPI 3.2 while recording what that loses, validate,
//! lower every schema to a representation where the hard decisions are
//! already made, and render. [`SourceDocument::parse`] and [`validate`]
//! turn bytes into a [`ValidatedSourceDocument`]; [`generate`] turns that
//! and a [`Config`] into a [`Generation`]: files in memory, every
//! diagnostic, and the dependency list the generated code needs, in Rust
//! or in Go.
//!
//! ```no_run
//! use roas_codegen::{
//!     ConfigFile, Input, SourceDocument, Target, generate, generation_options, validate,
//! };
//! use url::Url;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("openapi.json")?;
//! let document = SourceDocument::parse(Input::Json(bytes), Url::parse("file:///openapi.json")?)?;
//! let document = validate(document, generation_options())?;
//! let mut file = ConfigFile::default();
//! file.target = Some(Target::Rust);
//! let config = file.build()?;
//! let generation = generate(&document, &config)?;
//! for file in &generation.files {
//!     println!("{}: {} bytes", file.path.display(), file.contents.len());
//! }
//! for diagnostic in &generation.diagnostics {
//!     eprintln!("{diagnostic}");
//! }
//! # Ok(()) }
//! ```

mod checks;
mod config;
mod diagnostic;
mod front;
mod generate;
mod go;
mod ir;
mod lower;
mod pattern;
mod report;
mod rust;
mod source;
mod templates;
mod validate;
mod write;
mod yaml_value;

pub use config::{
    Config, ConfigError, ConfigFile, FieldConfig, GoConfig, GoConfigFile, Layout, RustConfig,
    RustConfigFile, Substitution, Target, TypeConfig, TypeSpelling, rust_module_name,
};
pub use diagnostic::{Diagnostic, DiagnosticKind, SchemaId, Severity};
pub use generate::{
    Dependencies, GenerateError, GeneratedFile, Generation, TemplateContext, generate, render,
    template_context,
};
pub use go::GoDependency;
pub use go::naming::go_file_name;
pub use rust::RustDependency;
pub use source::{Fidelity, Input, SourceDocument, SourceError, SourceVersion};
pub use templates::{TEMPLATE_API, TEMPLATE_MANIFEST};
pub use validate::{SourceValidationError, ValidatedSourceDocument, generation_options, validate};
pub use write::{CheckReport, WriteError, WriteOptions, WriteReport, check, manifest_name, write};
