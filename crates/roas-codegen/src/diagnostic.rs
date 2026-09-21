//! What the generator reports, and how a report names its subject.

use std::fmt;
use url::Url;

/// The identity of a schema, or of any other location in a description:
/// the document's URI plus a JSON Pointer ([RFC 6901]) into it.
///
/// Two schemas from two documents never collide, and the same schema
/// gets the same id on every run, because both halves come from the
/// input rather than from anything the generator decides.
///
/// [RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SchemaId {
    /// The document the schema lives in.
    pub uri: Url,
    /// Where in that document, as a JSON Pointer such as
    /// `/components/schemas/Pet`. Empty for the document root.
    pub pointer: String,
}

impl SchemaId {
    pub fn new(uri: Url, pointer: impl Into<String>) -> Self {
        SchemaId {
            uri,
            pointer: pointer.into(),
        }
    }
}

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.uri, self.pointer)
    }
}

/// How much a diagnostic matters.
///
/// An `Error` prevents generation of the affected type and of anything
/// that depends on it; the rest of the document is still generated. A
/// `Warning` changes nothing by itself, and a strict run promotes every
/// warning to an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Warning => "warning",
            Severity::Error => "error",
        })
    }
}

/// What kind of thing a diagnostic reports. The message says it in
/// words; the kind lets a caller act on it without parsing them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// Upconverting the source version to 3.2 dropped something the
    /// generator would have used. `keyword` names it.
    NormalizationLoss { keyword: String },
    /// A `$ref` to another resource. Nothing outside the document is
    /// resolved yet, because a fetched document has its own version
    /// and dialect and would be read under the root's.
    ExternalReference { reference: String },
    /// A `$ref` whose fragment is a plain-name anchor such as `#Pet`
    /// rather than a JSON Pointer.
    AnchorReference { reference: String },
    /// An `$id` that re-bases the references beneath it. Their meaning
    /// depends on the re-basing, which is not tracked yet.
    IdRebasing { id: String },
    /// A `$ref` to a local pointer that is not a component schema.
    UnsupportedReferenceTarget { reference: String },
    /// A keyword the model keeps but the generator does not handle.
    /// Nothing is silently skipped: every one is named.
    UnsupportedKeyword { keyword: String },
    /// A schema no type can represent: `false`, or `not`.
    Ungeneratable { keyword: String },
    /// A `$schema` or `jsonSchemaDialect` other than the OAS dialect
    /// and Draft 2020-12.
    UnsupportedDialect { dialect: String },
    /// The mapped numeric type narrows what the schema permits.
    LossyNumber,
    /// A composition that could not be proven safe and so lowers to
    /// an open holder rather than a struct or an enum.
    OpenComposition { keyword: String },
    /// A typeless schema with object keywords: it permits every
    /// instance type, so it lowers to an open holder.
    TypelessObject,
    /// An `enum` of numbers; the type stays the scalar.
    NumericEnum,
    /// A code-bearing `x-` extension left unapplied because
    /// `allow_codegen_extensions` is off.
    IgnoredExtension { keyword: String },
}

/// One thing the generator has to say about the description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
    /// The schema, or the nearest schema-like object, the diagnostic is
    /// about.
    pub schema_id: SchemaId,
    /// The exact spot, as a JSON Pointer into the same document as
    /// [`schema_id`](Self::schema_id): usually one keyword beneath it.
    pub pointer: String,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}#{}: {}",
            self.severity, self.schema_id.uri, self.pointer, self.message
        )
    }
}

/// Escape one reference token per RFC 6901 and append it to `pointer`.
pub(crate) fn push_token(pointer: &mut String, token: &str) {
    pointer.push('/');
    for c in token.chars() {
        match c {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            c => pointer.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_id_displays_as_uri_and_pointer() {
        let id = SchemaId::new(
            Url::parse("file:///a.json").unwrap(),
            "/components/schemas/Pet",
        );
        assert_eq!(id.to_string(), "file:///a.json#/components/schemas/Pet");
    }

    #[test]
    fn pointer_tokens_are_escaped() {
        let mut p = String::new();
        push_token(&mut p, "a/b");
        push_token(&mut p, "c~d");
        assert_eq!(p, "/a~1b/c~0d");
    }

    #[test]
    fn diagnostic_displays_severity_location_and_message() {
        let d = Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::NormalizationLoss {
                keyword: "discriminator".into(),
            },
            schema_id: SchemaId::new(Url::parse("file:///a.json").unwrap(), "/definitions/Pet"),
            pointer: "/definitions/Pet/discriminator".into(),
            message: "dropped".into(),
        };
        assert_eq!(
            d.to_string(),
            "warning: file:///a.json#/definitions/Pet/discriminator: dropped"
        );
        assert!(Severity::Warning < Severity::Error);
    }
}
