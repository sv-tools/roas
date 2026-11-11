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

/// The attributes among `attrs` that gate compilation — `cfg`, or a
/// `cfg_attr` that applies only gates — which every impl generated beside
/// the item must repeat. A `cfg_attr` mixing a gate with other attributes
/// cannot be repeated on an impl, so it is refused.
pub(crate) fn cfg_gates(attrs: &[String], at: &str) -> Result<Vec<String>, GenerateError> {
    fn gates(meta: &syn::Meta) -> Result<Option<bool>, syn::Error> {
        if meta.path().is_ident("cfg") {
            return Ok(Some(true));
        }
        if !meta.path().is_ident("cfg_attr") {
            return Ok(Some(false));
        }
        let list = meta.require_list()?;
        let args = list.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        )?;
        let mut kinds = Vec::new();
        for nested in args.iter().skip(1) {
            match gates(nested)? {
                Some(kind) => kinds.push(kind),
                None => return Ok(None),
            }
        }
        Ok(if kinds.iter().all(|k| *k) {
            Some(true)
        } else if kinds.iter().all(|k| !*k) {
            Some(false)
        } else {
            None
        })
    }
    let mut out = Vec::new();
    for attr in attrs {
        let parsed = parse(attr, at)?;
        match gates(&parsed.meta) {
            Ok(Some(true)) => out.push(attr.clone()),
            Ok(Some(false)) => {}
            Ok(None) => {
                return Err(GenerateError::InvalidAttribute {
                    at: at.to_owned(),
                    attr: attr.clone(),
                    reason: "a `cfg_attr` mixing `cfg` with other attributes cannot be repeated on the generated impls; give the `cfg` an attribute of its own".to_owned(),
                });
            }
            Err(error) => {
                return Err(GenerateError::InvalidAttribute {
                    at: at.to_owned(),
                    attr: attr.clone(),
                    reason: error.to_string(),
                });
            }
        }
    }
    Ok(out)
}

/// The serde keys `attr` sets, including those under a `cfg_attr`, which
/// set them just the same when the predicate holds.
fn serde_keys(attr: &syn::Attribute) -> Vec<String> {
    let mut keys = Vec::new();
    meta_serde_keys(&attr.meta, &mut keys);
    keys
}

fn meta_serde_keys(meta: &syn::Meta, keys: &mut Vec<String>) {
    if meta.path().is_ident("cfg_attr") {
        if let Ok(list) = meta.require_list()
            && let Ok(args) = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
        {
            for nested in args.iter().skip(1) {
                meta_serde_keys(nested, keys);
            }
        }
        return;
    }
    if meta.path().is_ident("serde")
        && let Ok(list) = meta.require_list()
    {
        let _ = list.parse_nested_meta(|meta| {
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
}

/// The names `#[serde(alias = "…")]` among `attrs` lets a field also be
/// read by. A struct with a catch-all routes members by name before its
/// derived code sees them, so an alias it cannot read is refused rather
/// than routed into the catch-all: one written as anything but a string
/// literal, or one under a `cfg_attr`.
pub(crate) fn serde_aliases(attrs: &[String], at: &str) -> Result<Vec<String>, GenerateError> {
    fn walk(meta: &syn::Meta, gated: bool, out: &mut Vec<String>) -> Result<(), String> {
        if meta.path().is_ident("cfg_attr") {
            let list = meta.require_list().map_err(|e| e.to_string())?;
            let args = list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .map_err(|e| e.to_string())?;
            for nested in args.iter().skip(1) {
                walk(nested, true, out)?;
            }
            return Ok(());
        }
        if !meta.path().is_ident("serde") {
            return Ok(());
        }
        let list = meta.require_list().map_err(|e| e.to_string())?;
        let mut found = Vec::new();
        list.parse_nested_meta(|nested| {
            let alias = nested.path.is_ident("alias");
            if nested.input.peek(syn::Token![=]) {
                let _: syn::Token![=] = nested.input.parse()?;
                let value: syn::Expr = nested.input.parse()?;
                if alias {
                    match value {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(s),
                            ..
                        }) => found.push(Some(s.value())),
                        _ => found.push(None),
                    }
                }
            } else if nested.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in nested.input);
                let _: syn::punctuated::Punctuated<syn::Expr, syn::Token![,]> =
                    content.parse_terminated(syn::Expr::parse, syn::Token![,])?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        for alias in found {
            match alias {
                Some(_) if gated => {
                    return Err("a `serde(alias)` under `cfg_attr` on a struct with a catch-all cannot be routed to its field; give the alias an attribute of its own".to_owned());
                }
                Some(name) => out.push(name),
                None => {
                    return Err("a `serde(alias)` on a struct with a catch-all must be a string literal, so members can be routed to the field".to_owned());
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for attr in attrs {
        let parsed = parse(attr, at)?;
        walk(&parsed.meta, false, &mut out).map_err(|reason| GenerateError::InvalidAttribute {
            at: at.to_owned(),
            attr: attr.clone(),
            reason,
        })?;
    }
    Ok(out)
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
        // Under a `cfg_attr`, an owned key is set all the same.
        let error = check_field_attrs(
            &["#[cfg_attr(all(), serde(rename = \"zz\"))]".into()],
            "Pet.tag",
        )
        .unwrap_err();
        assert!(matches!(&error, GenerateError::ReservedAttribute { key, .. } if key == "rename"));
        let error = check_container_attrs(
            &["#[cfg_attr(test, derive(Eq), serde(deny_unknown_fields))]".into()],
            "Pet",
        )
        .unwrap_err();
        assert!(
            matches!(&error, GenerateError::ReservedAttribute { key, .. } if key == "deny_unknown_fields")
        );
    }

    #[test]
    fn aliases_are_read_from_string_literals_only() {
        let aliases = serde_aliases(
            &[
                "#[serde(alias = \"t\", alias = \"u\")]".into(),
                "#[validate(alias = \"no\")]".into(),
                "#[cfg_attr(test, derive(Eq))]".into(),
            ],
            "Pet.tag",
        )
        .unwrap();
        assert_eq!(aliases, ["t", "u"]);
        for refused in [
            "#[serde(alias = concat!(\"a\"))]",
            "#[cfg_attr(test, serde(alias = \"t\"))]",
        ] {
            let error = serde_aliases(&[refused.into()], "Pet.tag").unwrap_err();
            assert!(
                matches!(error, GenerateError::InvalidAttribute { .. }),
                "{refused}"
            );
        }
    }

    #[test]
    fn malformed_attributes_are_reported() {
        let error = check_field_attrs(&["#[validate(".into()], "Pet.tag").unwrap_err();
        assert!(matches!(error, GenerateError::InvalidAttribute { .. }));
        let error = check_field_attrs(&["#[a] #[b]".into()], "Pet.tag").unwrap_err();
        assert!(matches!(error, GenerateError::InvalidAttribute { .. }));
    }
}
