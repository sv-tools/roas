//! Following document-local `$ref`s, and saying what happened.
//!
//! Every AsyncAPI version lets a `$ref` stand in for an object, and a
//! validator has to know more than "did that work": an external
//! reference is fine and simply unresolvable here, a dangling one is a
//! document bug, a cycle is a different bug, and a pointer at something
//! this crate does not model is neither. [`Resolution`] carries that
//! distinction so each version module reports it the same way.
//!
//! Pointer syntax lives in [`pointer`](crate::common::pointer); this
//! module is about what the pointers *mean*.

use crate::common::pointer;
use crate::common::reference::RefOr;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;

/// What following a document-local pointer turned out to be.
#[derive(Debug)]
pub(crate) enum Resolution<'a, T> {
    /// Resolved to a concrete object in this document.
    Found(&'a T),
    /// The chain ends at a reference into another document, or at a
    /// location this crate does not model. Either way it is not a
    /// document bug, and not something this crate can inspect.
    Opaque,
    /// A local pointer that names nothing.
    Missing,
    /// The chain revisits a pointer it already followed.
    Cycle,
    /// Not a usable pointer: malformed escapes, or a shape that is not
    /// a JSON Pointer at all.
    Unrecognized,
    /// A pointer at a location the document itself declares to be some
    /// *other* kind of object — a server where a channel belongs, say.
    ///
    /// Distinct from [`Opaque`](Resolution::Opaque): there the target
    /// is something this crate does not model and cannot judge; here
    /// the document has already said what it is.
    WrongKind,
}

impl<'a, T> Resolution<'a, T> {
    /// The object, if the pointer resolved inside this document.
    ///
    /// Takes `self` so the borrow is the document's, not this
    /// `Resolution`'s — callers routinely resolve into a temporary.
    #[must_use]
    pub(crate) fn found(self) -> Option<&'a T> {
        match self {
            Resolution::Found(item) => Some(item),
            _ => None,
        }
    }

    /// Why this pointer is a document bug, if it is one.
    ///
    /// `None` means it resolved, left the document, or landed somewhere
    /// this crate does not model — none of which the document is at
    /// fault for.
    #[must_use]
    pub(crate) fn problem(&self) -> Option<&'static str> {
        match self {
            Resolution::Found(_) | Resolution::Opaque => None,
            Resolution::Missing => Some("names nothing in this document"),
            Resolution::Cycle => Some("is part of a reference cycle"),
            Resolution::Unrecognized => Some("is not a usable JSON Pointer"),
            Resolution::WrongKind => Some("does not point at an object of the expected kind"),
        }
    }
}

/// The object kinds an AsyncAPI document declares, as they are spelled
/// in a pointer — the union across versions, since a name that is not a
/// kind in one of them simply never appears in its pointers.
const KINDS: &[&str] = &[
    "servers",
    "channels",
    "operations",
    "messages",
    "schemas",
    "securitySchemes",
    "serverVariables",
    "parameters",
    "correlationIds",
    "replies",
    "replyAddresses",
    "externalDocs",
    "tags",
    "operationTraits",
    "messageTraits",
    "serverBindings",
    "channelBindings",
    "operationBindings",
    "messageBindings",
];

/// The root fields that hold exactly one object of a fixed kind, none
/// of which is ever a referenceable component. A pointer at one of
/// these names that object and nothing else.
const SINGLETONS: &[&str] = &["info", "asyncapi", "id", "defaultContentType"];

/// Whether a pointer names something the document has already declared
/// to be other than the kind expected here.
///
/// Three shapes qualify, and none of them can be settled by looking at
/// the JSON: what is wrong with them is *where they are*.
///
/// A **container** — the document itself, `#/components`, or a whole
/// map such as `#/channels` — holds objects rather than being one.
/// These are the cases structural deserialization gets wrong most
/// readily, since a map of channels is quite happy to read as a channel
/// whose every field is absent.
///
/// A **singleton** such as `#/info` is that object however plausibly
/// its JSON reads as something else.
///
/// An **entry of another map** — `#/servers/prod` where a channel
/// belongs.
///
/// Only the document's own structure declares a kind.
///
/// A pointer that starts anywhere else — `#/x-store/messages/c`, into
/// an extension that happens to spell a map `messages` — is arbitrary
/// JSON, and nothing about its shape says what it holds. Within the
/// document, what names the kind is the token *before* the last one,
/// at whatever depth: `#/channels/c/messages/m` is a message, and
/// using it where a channel belongs is the same mistake as
/// `#/components/messages/m` there. A token that is no kind at all —
/// `publish` in v2.6's `#/channels/user/publish/message` — says nothing
/// either way, and the pointer is judged on what it finds instead.
fn names_something_else(path: &[String], expected: &str) -> bool {
    // The whole document is a container.
    let [first, rest @ ..] = path else {
        return true;
    };
    // Rooted in the document's own structure, or not our business.
    if first != "components" && !KINDS.contains(&first.as_str()) {
        return rest.is_empty() && SINGLETONS.contains(&first.as_str());
    }
    match rest {
        // `#/components` itself, or a whole map.
        [] => true,
        // A whole map under `components`.
        [kind] if first == "components" => KINDS.contains(&kind.as_str()),
        _ => {
            let named = &path[path.len() - 2];
            named != expected && KINDS.contains(&named.as_str())
        }
    }
}

/// Decide what an unresolved local pointer *is*, by walking the
/// document as plain JSON.
///
/// Outcomes, in order of how much the document is at fault. A pointer
/// at something the document declares to be a *different* kind — or at
/// a container of objects rather than an object — is [`WrongKind`]. So is one whose target cannot be read as a `T` at
/// all — a scalar `x-` extension used as a message, say. One that lands
/// on JSON that *does* read as a `T` is [`Opaque`]: `$ref` may name
/// anything, so a message-shaped `x-` extension is a legal target this
/// crate simply does not model as its own object. One that lands
/// nowhere is [`Missing`].
///
/// The `T` check is deliberately a shape check, not an equality one:
/// these models drop unknown non-`x-` keys and normalize numbers, so
/// demanding an exact round-trip would reject legitimate targets.
///
/// [`WrongKind`]: Resolution::WrongKind
/// [`Opaque`]: Resolution::Opaque
/// [`Missing`]: Resolution::Missing
pub(crate) fn classify_unresolved<'a, D, T>(
    document: &D,
    local_pointer: &str,
    expected_kind: &str,
) -> Resolution<'a, T>
where
    D: Serialize,
    T: DeserializeOwned,
{
    let Some(path) = pointer::tokens(local_pointer) else {
        return Resolution::Unrecognized;
    };
    if names_something_else(&path, expected_kind) {
        return Resolution::WrongKind;
    }
    // Serializing a document model cannot fail; falling back to `null`
    // keeps that from needing a branch, and leaves every pointer naming
    // nothing if it somehow does.
    let snapshot = serde_json::to_value(document).unwrap_or_default();
    match pointer::walk(&snapshot, &path) {
        Some(target) => match serde_json::from_value::<T>(target.clone()) {
            Ok(_) => Resolution::Opaque,
            Err(_) => Resolution::WrongKind,
        },
        None => Resolution::Missing,
    }
}

/// Follow a chain of `RefOr` entries to the object at its end.
///
/// `lookup` maps a pointer's decoded tokens to the next entry, which is
/// what differs between kinds — messages live in one map, servers in
/// another, and some versions allow both a root and a components map.
/// Everything else — external termini, cycles, malformed pointers, and
/// the fallback for unmodeled locations — is the same everywhere and
/// lives here.
pub(crate) fn follow<'a, D, T, F>(
    document: &D,
    start: &'a RefOr<T>,
    expected_kind: &str,
    lookup: F,
) -> Resolution<'a, T>
where
    D: Serialize,
    T: DeserializeOwned,
    F: Fn(&[String]) -> Option<&'a RefOr<T>>,
{
    follow_tracked(document, Vec::new(), start, expected_kind, lookup).1
}

/// Where a chain ended, as an identity two references can be compared
/// by.
///
/// A chain that leaves the document still has one: `./channels.yaml#/user`
/// names a channel as definitely as `#/channels/user` does, and a
/// message of *that* channel is `./channels.yaml#/user/messages/m`. A
/// caller comparing a message against its channel needs the resource as
/// well as the pointer, or every split document looks mismatched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Terminus {
    /// The document the chain ended in — empty for this one.
    pub(crate) resource: String,
    /// The pointer within that document, as decoded tokens.
    pub(crate) at: Vec<String>,
}

impl Terminus {
    /// Split a reference as written into the resource it names and the
    /// pointer within it.
    ///
    /// `None` when the fragment is not a usable JSON Pointer.
    pub(crate) fn parse(reference: &str) -> Option<Self> {
        let (resource, fragment) = match reference.split_once('#') {
            Some((resource, fragment)) => (resource, fragment),
            None => (reference, ""),
        };
        Some(Self {
            resource: resource.to_owned(),
            at: pointer::tokens(fragment)?,
        })
    }

    /// The key `other` names, when it names an entry of this object's
    /// `field` map — this terminus with `/<field>/<key>` on the end, in
    /// the same document, and nothing else.
    ///
    /// Only v3 compares references this way; v2.6 keys its channels by
    /// address and has no equivalent relationship.
    #[cfg(any(feature = "v3_0", feature = "v3_1", test))]
    pub(crate) fn child_key<'o>(&self, field: &str, other: &'o Self) -> Option<&'o String> {
        if self.resource != other.resource {
            return None;
        }
        let (prefix, tail) = other.at.split_at(other.at.len().checked_sub(2)?);
        match tail {
            [map, key] if prefix == self.at && map == field => Some(key),
            _ => None,
        }
    }
}

impl std::fmt::Display for Terminus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#", self.resource)?;
        for token in &self.at {
            write!(f, "/{}", token.replace('~', "~0").replace('/', "~1"))?;
        }
        Ok(())
    }
}

/// [`follow`], reporting *where* the chain ended as well as what it
/// found.
///
/// The terminal path is the pointer that produced the last entry, which
/// is not the one the caller started from: `#/channels/alias` where
/// `alias` is a `$ref` to `#/channels/real` ends at `real`. A caller
/// that keys anything off the target — "is this message one of *that*
/// channel's?" — has to key it off the terminus, or every alias looks
/// like a mismatch.
///
/// `start_path` is where `start` itself was found, and is the answer
/// when the chain does not move.
pub(crate) fn follow_tracked<'a, D, T, F>(
    document: &D,
    start_path: Vec<String>,
    start: &'a RefOr<T>,
    expected_kind: &str,
    lookup: F,
) -> (Terminus, Resolution<'a, T>)
where
    D: Serialize,
    T: DeserializeOwned,
    F: Fn(&[String]) -> Option<&'a RefOr<T>>,
{
    let mut current = start;
    let mut at = Terminus {
        resource: String::new(),
        at: start_path,
    };
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    loop {
        let reference = match current {
            RefOr::Item(item) => return (at, Resolution::Found(item)),
            RefOr::Reference(reference) => reference,
        };
        if reference.is_external() {
            // The chain ends in another document, and *that* is the
            // identity of what it found.
            let terminus = Terminus::parse(&reference.reference);
            return match terminus {
                Some(terminus) => (terminus, Resolution::Opaque),
                None => (at, Resolution::Unrecognized),
            };
        }
        let Some(local) = reference.local_pointer() else {
            return (at, Resolution::Unrecognized);
        };
        if !seen.insert(local) {
            return (at, Resolution::Cycle);
        }
        let Some(path) = pointer::tokens(local) else {
            return (at, Resolution::Unrecognized);
        };
        match lookup(&path) {
            Some(next) => {
                current = next;
                at = Terminus {
                    resource: String::new(),
                    at: path,
                };
            }
            None => {
                let terminus = Terminus {
                    resource: String::new(),
                    at: path,
                };
                return (
                    terminus,
                    classify_unresolved(document, local, expected_kind),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::reference::Reference;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Demo {
        name: String,
    }

    #[derive(Serialize)]
    struct Doc {
        entries: BTreeMap<String, RefOr<Demo>>,
        #[serde(rename = "x-extra")]
        extra: serde_json::Value,
        #[serde(rename = "x-scalar")]
        scalar: serde_json::Value,
    }

    fn reference(target: &str) -> RefOr<Demo> {
        RefOr::Reference(Reference {
            reference: target.to_owned(),
        })
    }

    fn document() -> Doc {
        let mut entries = BTreeMap::new();
        entries.insert(
            "real".to_owned(),
            RefOr::Item(Demo {
                name: "real".to_owned(),
            }),
        );
        entries.insert("alias".to_owned(), reference("#/entries/real"));
        entries.insert("hop".to_owned(), reference("#/entries/alias"));
        entries.insert("loop-a".to_owned(), reference("#/entries/loop-b"));
        entries.insert("loop-b".to_owned(), reference("#/entries/loop-a"));
        entries.insert(
            "outside".to_owned(),
            reference("./other.yaml#/entries/real"),
        );
        entries.insert("ghost".to_owned(), reference("#/entries/nope"));
        entries.insert("unmodeled".to_owned(), reference("#/x-extra"));
        entries.insert("malformed".to_owned(), reference("#/entries/bad~2escape"));
        // Empty is neither external nor a pointer.
        entries.insert("empty".to_owned(), reference(""));
        Doc {
            entries,
            extra: serde_json::json!({ "name": "shared" }),
            scalar: serde_json::json!("not an object"),
        }
    }

    fn resolve<'a>(doc: &'a Doc, entry: &'a RefOr<Demo>) -> Resolution<'a, Demo> {
        follow(doc, entry, "entries", |path| match path {
            [entries, key] if entries == "entries" => doc.entries.get(key),
            _ => None,
        })
    }

    #[test]
    fn follows_a_chain_to_its_object() {
        let doc = document();
        let resolved = resolve(&doc, &doc.entries["hop"])
            .found()
            .expect("resolves");
        assert_eq!(resolved.name, "real");

        // An inline entry resolves to itself.
        assert!(resolve(&doc, &doc.entries["real"]).found().is_some());
    }

    #[test]
    fn each_outcome_is_told_apart() {
        let doc = document();
        for (key, problem) in [
            ("ghost", Some("names nothing in this document")),
            ("loop-a", Some("is part of a reference cycle")),
            ("malformed", Some("is not a usable JSON Pointer")),
            ("empty", Some("is not a usable JSON Pointer")),
            // Neither of these is the document's fault.
            ("outside", None),
            ("unmodeled", None),
        ] {
            assert_eq!(
                resolve(&doc, &doc.entries[key]).problem(),
                problem,
                "entry `{key}`",
            );
        }
    }

    #[test]
    fn an_unmodeled_but_real_location_is_opaque_not_found() {
        let doc = document();
        // The pointer lands on real JSON, so it is legal — but this
        // lookup does not model it, so there is nothing to hand back.
        let resolution = resolve(&doc, &doc.entries["unmodeled"]);
        assert!(matches!(resolution, Resolution::Opaque));
        assert!(resolution.found().is_none());
    }

    #[test]
    fn a_target_that_cannot_be_the_expected_kind_is_wrong_not_opaque() {
        let doc = document();
        // Shaped like a `Demo`, so naming it is legal even though this
        // crate does not model the location as one.
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/x-extra", "entries"),
            Resolution::Opaque
        ));
        // Not shaped like one at all.
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/x-scalar", "entries"),
            Resolution::WrongKind
        ));
        // A root singleton is what the document says it is, however
        // its JSON happens to read.
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/info", "entries"),
            Resolution::WrongKind
        ));
    }

    #[test]
    fn a_terminus_is_a_resource_and_a_pointer() {
        let local = Terminus::parse("#/channels/user").expect("a pointer");
        assert_eq!(local.resource, "");
        assert_eq!(local.to_string(), "#/channels/user");

        // A whole document, with no fragment at all.
        let whole = Terminus::parse("./other.yaml").expect("a resource");
        assert_eq!(whole.resource, "./other.yaml");
        assert!(whole.at.is_empty());
        assert_eq!(whole.to_string(), "./other.yaml#");

        // A message of a channel over there is named over there.
        let channel = Terminus::parse("./channels.yaml#/user").expect("a pointer");
        let message = Terminus::parse("./channels.yaml#/user/messages/signup").expect("a pointer");
        assert_eq!(
            channel.child_key("messages", &message).map(String::as_str),
            Some("signup"),
        );
        // The same pointer in *this* document is a different object.
        let here = Terminus::parse("#/user/messages/signup").expect("a pointer");
        assert!(channel.child_key("messages", &here).is_none());

        // A fragment that is not a pointer is not a terminus.
        assert!(Terminus::parse("./other.yaml#bad").is_none());
    }

    #[test]
    fn a_chain_reports_where_it_ended_not_where_it_began() {
        let doc = document();
        let start = pointer::tokens("/entries/hop").expect("a pointer");
        let lookup = |path: &[String]| match path {
            [entries, key] if entries == "entries" => doc.entries.get(key),
            _ => None,
        };
        let (terminal, resolution) =
            follow_tracked(&doc, start, &doc.entries["hop"], "entries", lookup);
        assert_eq!(
            terminal.to_string(),
            "#/entries/real",
            "hop -> alias -> real"
        );
        assert!(resolution.found().is_some());

        // A chain that leaves the map ends at the pointer that took it
        // there, not at the entry that pointed.
        let start = pointer::tokens("/entries/unmodeled").expect("a pointer");
        let (terminal, resolution) =
            follow_tracked(&doc, start, &doc.entries["unmodeled"], "entries", lookup);
        assert_eq!(terminal.to_string(), "#/x-extra");
        assert!(matches!(resolution, Resolution::Opaque));

        // An entry that is already an object ends where it started.
        let start = pointer::tokens("/entries/real").expect("a pointer");
        let (terminal, _) = follow_tracked(&doc, start, &doc.entries["real"], "entries", |_| None);
        assert_eq!(terminal.to_string(), "#/entries/real");
    }

    #[test]
    fn classify_separates_dangling_from_unmodeled_and_malformed() {
        let doc = document();
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/x-extra", "entries"),
            Resolution::Opaque
        ));
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/nothing/here", "entries"),
            Resolution::Missing
        ));
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/bad~2escape", "entries"),
            Resolution::Unrecognized
        ));
        // A pointer into a different declared kind is the document's
        // own contradiction.
        assert!(matches!(
            classify_unresolved::<_, Demo>(&doc, "/servers/prod", "channels"),
            Resolution::WrongKind
        ));
    }
}
