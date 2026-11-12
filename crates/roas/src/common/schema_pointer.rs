//! Following a schema `$ref` whose pointer reaches *into* a schema.
//!
//! `#/components/schemas/Pet` names a component; JSON Schema also lets a
//! reference name any subschema below it, such as
//! `#/components/schemas/Pet/properties/owner` or
//! `#/components/schemas/Pet/allOf/0`. Each version's schema model says
//! which of its keywords hold subschemas through [`SchemaPointer`]; the
//! walk and the chain-following are the same everywhere and live here.
//!
//! A pointer walks the document *as written*: stepping through a schema
//! that is itself only a `$ref` names nothing, as it would in the JSON.

use crate::common::reference::{RefOr, ReferenceObject};
use crate::loader::decode_fragment;
use std::collections::BTreeSet;

/// A schema type whose subschemas a JSON Pointer can step into.
pub(crate) trait SchemaPointer: Sized {
    /// The reference payload a `RefOr<Self, _>` slot holds.
    type Ref: ReferenceObject;

    /// The subschema the leading `tokens` name, and how many tokens that
    /// took (`items` is one, `properties/<name>` two). `None` when the
    /// keyword is not one this model holds a subschema under.
    fn child<'a>(&'a self, tokens: &[String]) -> Option<(&'a RefOr<Self, Self::Ref>, usize)>;
}

/// One place a pointer can land: an inline schema, or a reference to
/// follow.
pub(crate) enum Hop<'a, S, R> {
    Item(&'a S),
    Ref(&'a R),
}

impl<'a, S, R> From<&'a RefOr<S, R>> for Hop<'a, S, R> {
    fn from(entry: &'a RefOr<S, R>) -> Self {
        match entry {
            RefOr::Item(item) => Hop::Item(item),
            RefOr::Ref(reference) => Hop::Ref(reference),
        }
    }
}

/// Step from `hop` through `tokens`, as written.
fn walk<'a, S>(mut hop: Hop<'a, S, S::Ref>, mut tokens: &[String]) -> Option<Hop<'a, S, S::Ref>>
where
    S: SchemaPointer,
{
    while !tokens.is_empty() {
        let Hop::Item(schema) = hop else {
            return None;
        };
        let (child, used) = schema.child(tokens)?;
        hop = Hop::from(child);
        tokens = &tokens[used..];
    }
    Some(hop)
}

/// Follow a schema reference from `reference` to the schema at the end
/// of its chain, reporting the last reference reached as well.
///
/// `prefix` is where the schema components live (`#/components/schemas/`
/// or `#/definitions/`), and `root` looks a component up by name. The
/// rest of the pointer is percent- and `~`-decoded and walked through
/// the component with [`SchemaPointer::child`].
///
/// The schema is `None` when the chain dangles, loops, or leaves the
/// components (another document, or another part of this one); the
/// reference is then the last one reached.
pub(crate) fn follow_schema<'a, 'r, S, F>(
    reference: &'r str,
    prefix: &str,
    root: F,
) -> (Option<&'a S>, &'r str)
where
    'a: 'r,
    S: SchemaPointer,
    F: Fn(&str) -> Option<Hop<'a, S, S::Ref>>,
{
    let mut current = reference;
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    loop {
        let Some(tokens) = component_tokens(current, prefix) else {
            return (None, current);
        };
        let Some((name, rest)) = tokens.split_first() else {
            return (None, current);
        };
        match root(name).and_then(|hop| walk(hop, rest)) {
            Some(Hop::Item(schema)) => return (Some(schema), current),
            Some(Hop::Ref(next)) => {
                let next = next.reference();
                if !visited.insert(next) {
                    return (None, current);
                }
                current = next;
            }
            None => return (None, current),
        }
    }
}

/// The decoded pointer tokens after `prefix`: the component name first.
fn component_tokens(reference: &str, prefix: &str) -> Option<Vec<String>> {
    let rest = reference.strip_prefix(prefix)?;
    let decoded = decode_fragment(&format!("/{rest}")).ok()?;
    Some(
        decoded[1..]
            .split('/')
            .map(|token| token.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// An array index token: digits only, without a leading zero.
pub(crate) fn index(token: &str) -> Option<usize> {
    if token.is_empty()
        || !token.bytes().all(|b| b.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    token.parse().ok()
}

/// `name/<key>` in a map of subschemas.
pub(crate) fn keyed<'a, S, R>(
    tokens: &[String],
    keyword: &str,
    map: Option<&'a std::collections::BTreeMap<String, RefOr<S, R>>>,
) -> Option<(&'a RefOr<S, R>, usize)> {
    match tokens {
        [k, key, ..] if k == keyword => Some((map?.get(key)?, 2)),
        _ => None,
    }
}

/// `name/<index>` in a list of subschemas.
pub(crate) fn indexed<'a, S, R>(
    tokens: &[String],
    keyword: &str,
    list: Option<&'a [RefOr<S, R>]>,
) -> Option<(&'a RefOr<S, R>, usize)> {
    match tokens {
        [k, i, ..] if k == keyword => Some((list?.get(index(i)?)?, 2)),
        _ => None,
    }
}

/// `name` naming a single subschema.
pub(crate) fn single<'a, S, R>(
    tokens: &[String],
    keyword: &str,
    child: Option<&'a RefOr<S, R>>,
) -> Option<(&'a RefOr<S, R>, usize)> {
    match tokens {
        [k, ..] if k == keyword => Some((child?, 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_accepts_canonical_decimals_only() {
        assert_eq!(index("0"), Some(0));
        assert_eq!(index("12"), Some(12));
        assert_eq!(index(""), None);
        assert_eq!(index("01"), None);
        assert_eq!(index("-1"), None);
        assert_eq!(index("1a"), None);
    }

    #[test]
    fn component_tokens_decode_escapes() {
        assert_eq!(
            component_tokens(
                "#/components/schemas/Pet/properties/a~1b~0c%20d",
                "#/components/schemas/"
            ),
            Some(vec![
                "Pet".to_owned(),
                "properties".to_owned(),
                "a/b~c d".to_owned()
            ])
        );
        assert_eq!(
            component_tokens("#/definitions/Pet", "#/components/schemas/"),
            None
        );
        assert_eq!(
            component_tokens("#/components/schemas/Pet%zz", "#/components/schemas/"),
            None
        );
    }
}
