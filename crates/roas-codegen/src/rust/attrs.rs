//! User-supplied attributes are compilable Rust, so they are parsed as
//! such — and the serde keys the backend owns are refused by name.

use crate::generate::GenerateError;
use syn::parse::{Parse, Parser};

/// Field-level serde keys that carry the wire contract.
const FIELD_OWNED: &[&str] = &[
    "skip_serializing_if",
    "default",
    "deserialize_with",
    "serialize_with",
    "with",
    "rename",
    "flatten",
    "skip",
    "skip_deserializing",
    "skip_serializing",
];

/// Container-level serde keys that carry the wire contract.
const CONTAINER_OWNED: &[&str] = &[
    "tag",
    "content",
    "untagged",
    "transparent",
    "deny_unknown_fields",
    "rename_all",
    "rename",
    "default",
    "from",
    "into",
    "try_from",
    "remote",
    "bound",
];

fn parse(attr: &str, at: &str) -> Result<syn::Attribute, GenerateError> {
    let attrs = syn::Attribute::parse_outer
        .parse_str(attr)
        .map_err(|error| GenerateError::InvalidAttribute {
            at: at.to_owned(),
            attr: attr.to_owned(),
            reason: error.to_string(),
        })?;
    match attrs.len() {
        1 => Ok(attrs.into_iter().next().expect("one")),
        n => Err(GenerateError::InvalidAttribute {
            at: at.to_owned(),
            attr: attr.to_owned(),
            reason: format!("expected one attribute, found {n}"),
        }),
    }
}

fn serde_keys(attr: &syn::Attribute) -> Vec<String> {
    let mut keys = Vec::new();
    if attr.path().is_ident("serde") {
        let _ = attr.parse_nested_meta(|meta| {
            if let Some(ident) = meta.path.get_ident() {
                keys.push(ident.to_string());
            }
            // Consume any value so the parser can move on.
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Token![=] = meta.input.parse()?;
                let _: syn::Expr = meta.input.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in meta.input);
                let _: syn::punctuated::Punctuated<syn::Expr, syn::Token![,]> =
                    content.parse_terminated(syn::Expr::parse, syn::Token![,])?;
            }
            Ok(())
        });
    }
    keys
}

fn check(attrs: &[String], at: &str, owned: &[&str], level: &str) -> Result<(), GenerateError> {
    for attr in attrs {
        let parsed = parse(attr, at)?;
        for key in serde_keys(&parsed) {
            if owned.contains(&key.as_str()) {
                return Err(GenerateError::ReservedAttribute {
                    at: at.to_owned(),
                    key: key.clone(),
                    reason: format!(
                        "`serde({key})` on a {level} is how the generator encodes the wire contract; setting it would silently change what the type accepts"
                    ),
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn check_field_attrs(attrs: &[String], at: &str) -> Result<(), GenerateError> {
    check(attrs, at, FIELD_OWNED, "field")
}

pub(crate) fn check_container_attrs(attrs: &[String], at: &str) -> Result<(), GenerateError> {
    check(attrs, at, CONTAINER_OWNED, "type")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_serde_keys_are_refused_by_name() {
        let error = check_field_attrs(
            &["#[serde(skip_serializing_if = \"Option::is_none\")]".into()],
            "Pet.tag",
        )
        .unwrap_err();
        assert!(
            matches!(&error, GenerateError::ReservedAttribute { key, .. } if key == "skip_serializing_if"),
            "{error}"
        );
        let error =
            check_container_attrs(&["#[serde(deny_unknown_fields)]".into()], "Pet").unwrap_err();
        assert!(
            matches!(&error, GenerateError::ReservedAttribute { key, .. } if key == "deny_unknown_fields")
        );
        assert!(
            check_field_attrs(
                &[
                    "#[validate(length(min = 1))]".into(),
                    "#[serde(alias = \"t\")]".into()
                ],
                "Pet.tag"
            )
            .is_ok()
        );
        assert!(check_container_attrs(&["#[derive(Eq, Hash)]".into()], "Pet").is_ok());
    }

    #[test]
    fn malformed_attributes_are_reported() {
        let error = check_field_attrs(&["#[validate(".into()], "Pet.tag").unwrap_err();
        assert!(matches!(error, GenerateError::InvalidAttribute { .. }));
        let error = check_field_attrs(&["#[a] #[b]".into()], "Pet.tag").unwrap_err();
        assert!(matches!(error, GenerateError::InvalidAttribute { .. }));
    }
}
