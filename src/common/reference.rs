//! Reference Object

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::common::helpers::{Context, ValidateWithContext};
use crate::validation::Options;

/// ResolveReference is a trait for resolving references.
pub trait ResolveReference<D> {
    fn resolve_reference(&self, reference: &str) -> Option<&D>;
}

/// ResolveError is an error type for resolving references.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// NotFound is returned when the reference is not found.
    #[error("reference `{0}` not found")]
    NotFound(String),

    /// External is returned when the resolving of an external reference failed.
    #[error("resolving of an external reference `{0}` is not supported")]
    ExternalUnsupported(String),
}

/// RefOr is a simple object to allow storing a reference to another component or a component itself.
///
/// Example:
///
/// ```rust
/// use serde::{Deserialize, Serialize};
/// use roas::common::reference::RefOr;
///
/// #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
/// struct Foo {
///     pub value: String,
/// }
///
/// #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
/// struct Bar {
///     pub foo: Option<RefOr<Foo>>,
/// }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum RefOr<T> {
    /// A reference to another component.
    Ref(Ref),

    /// The component itself.
    Item(T),
}

/// Ref is a simple object to allow referencing other components in the OpenAPI document,
/// internally and externally.
/// The $ref string value contains a URI [RFC3986](https://www.rfc-editor.org/rfc/rfc3986),
/// which identifies the location of the value being referenced.
/// See the rules for resolving Relative References.
///
/// Specification example:
///
/// ```yaml
/// $ref: '#/components/schemas/Pet'
/// ```
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
pub struct Ref {
    /// **Required** The reference identifier.
    /// This MUST be in the form of a URI.
    #[serde(rename = "$ref")]
    pub reference: String,

    /// A short summary which by default SHOULD override that of the referenced component.
    /// If the referenced object-type does not allow a summary field, then this field has no effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// A description which by default SHOULD override that of the referenced component.
    /// CommonMark syntax MAY be used for rich text representation.
    /// If the referenced object-type does not allow a description field, then this field has no effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl<D> RefOr<D> {
    pub fn validate_with_context<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        match self {
            RefOr::Ref(r) => {
                r.validate_with_context(ctx, path);
            }
            RefOr::Item(d) => {
                d.validate_with_context(ctx, path);
            }
        }
    }

    /// Create a new RefOr with a reference.
    pub fn new_ref(reference: String) -> Self {
        RefOr::Ref(Ref::new(reference))
    }

    /// Create a new RefOr with an item.
    pub fn new_item(item: D) -> Self {
        RefOr::Item(item)
    }

    /// Get the item from the RefOr by returning the Item or resolving a reference.
    pub fn get_item<'a, T>(&'a self, spec: &'a T) -> Result<&D, ResolveError>
    where
        T: ResolveReference<D>,
    {
        match self {
            RefOr::Item(d) => Ok(d),
            RefOr::Ref(r) => r.resolve(spec),
        }
    }
}

impl<D> RefOr<Box<D>> {
    pub fn validate_with_context_boxed<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        match self {
            RefOr::Ref(r) => {
                r.validate_with_context(ctx, path);
            }
            RefOr::Item(d) => {
                d.validate_with_context(ctx, path);
            }
        }
    }
}

impl Ref {
    pub fn validate_with_context<T, D>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        if self.reference.is_empty() {
            ctx.errors.push(format!("{}.$ref: must not be empty", path));
            return;
        }
        if ctx.visited.insert(self.reference.clone()) {
            match self.resolve(ctx.spec) {
                Ok(d) => {
                    d.validate_with_context(ctx, self.reference.clone());
                }
                Err(e) => match e {
                    ResolveError::NotFound(r) => {
                        ctx.errors.push(format!("{}.$ref: `{}` not found", path, r));
                    }
                    ResolveError::ExternalUnsupported(r) => {
                        if !ctx.options.contains(Options::IgnoreExternalReferences) {
                            ctx.errors.push(format!("{}.$ref: {}", path, r));
                        }
                    }
                },
            }
        }
    }

    /// Resolve the reference.
    pub fn resolve<'a, T, D>(&'a self, spec: &'a T) -> Result<&D, ResolveError>
    where
        T: ResolveReference<D>,
    {
        if self.reference.starts_with("#/") {
            match spec.resolve_reference(&self.reference) {
                Some(d) => Ok(d),
                None => Err(ResolveError::NotFound(self.reference.clone())),
            }
        } else {
            // TODO: resolve external reference
            Err(ResolveError::ExternalUnsupported(self.reference.clone()))
        }
    }

    pub fn new(reference: String) -> Self {
        Ref {
            reference,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    struct Foo {
        pub foo: String,
    }

    #[test]
    fn test_ref_or_foo_serialize() {
        assert_eq!(
            serde_json::to_value(RefOr::new_item(Foo {
                foo: String::from("bar"),
            }))
            .unwrap(),
            serde_json::json!({
                "foo": "bar"
            }),
            "serialize item",
        );
        assert_eq!(
            serde_json::to_value(RefOr::Ref::<Foo>(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            }))
            .unwrap(),
            serde_json::json!({
                "$ref": "#/components/schemas/Foo"
            }),
            "serialize ref",
        );
    }

    #[test]
    fn test_ref_or_foo_deserialize() {
        assert_eq!(
            serde_json::from_value::<RefOr<Foo>>(serde_json::json!({
                "foo":"bar",
            }))
            .unwrap(),
            RefOr::new_item(Foo {
                foo: String::from("bar"),
            }),
            "deserialize item",
        );

        assert_eq!(
            serde_json::from_value::<RefOr<Foo>>(serde_json::json!({
                "$ref":"#/components/schemas/Foo",
            }))
            .unwrap(),
            RefOr::Ref(Ref {
                reference: String::from("#/components/schemas/Foo"),
                ..Default::default()
            }),
            "deserialize ref",
        );
    }
}
