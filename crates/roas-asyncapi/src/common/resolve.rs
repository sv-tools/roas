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
use crate::validation::Context;
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
/// Depth does not enter into it: what names the kind is the token
/// *before* the last one, wherever that falls. `#/channels/c/messages/m`
/// is a message however deep it sits, and using it where a channel
/// belongs is the same mistake as `#/components/messages/m` there.
/// A token that is not a kind name at all — `publish` in v2.6's
/// `#/channels/user/publish/message` — says nothing either way, and the
/// pointer is judged on what it finds instead.
fn names_something_else(path: &[String], expected: &str) -> bool {
    match path {
        // Containers: the whole document, `#/components`, a map.
        [] => true,
        [components] if components == "components" => true,
        [single] => SINGLETONS.contains(&single.as_str()) || KINDS.contains(&single.as_str()),
        [components, kind] if components == "components" => KINDS.contains(&kind.as_str()),
        // The token before the last one names the kind, at any depth.
        [.., named, _] => named != expected && KINDS.contains(&named.as_str()),
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
) -> (Vec<String>, Resolution<'a, T>)
where
    D: Serialize,
    T: DeserializeOwned,
    F: Fn(&[String]) -> Option<&'a RefOr<T>>,
{
    let mut current = start;
    let mut at = start_path;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    loop {
        let reference = match current {
            RefOr::Item(item) => return (at, Resolution::Found(item)),
            RefOr::Reference(reference) => reference,
        };
        if reference.is_external() {
            return (at, Resolution::Opaque);
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
                at = path;
            }
            None => return (path, classify_unresolved(document, local, expected_kind)),
        }
    }
}

/// Check every document-local `$ref` in the document, wherever it sits.
///
/// The typed checks above judge the references a version *wires* —
/// which channel an operation names, which messages belong to it — and
/// they are the better error when they apply. But a reference is a
/// document bug whether or not anything is wired through it, and a
/// dangling pointer inside a channel's `messages`, a server's
/// `variables`, or an operation's `traits` would otherwise be found
/// only if some other check happened to walk that way.
///
/// This walks the serialized document instead of the model, so it needs
/// no per-position code and covers positions the model holds as opaque
/// JSON. It reports only what it can see structurally — a pointer that
/// names nothing, or a chain that loops — and stays quiet where a typed
/// check has already spoken, so the specific error wins.
///
/// `x-` extensions are skipped: what a `$ref` means inside one is the
/// extension's business.
pub(crate) fn check_every_reference<D>(ctx: &mut Context, document: &D)
where
    D: Serialize,
{
    let snapshot = serde_json::to_value(document).unwrap_or_default();
    visit(ctx, &snapshot, &snapshot);
}

fn visit(ctx: &mut Context, root: &serde_json::Value, node: &serde_json::Value) {
    match node {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(reference)) = map.get("$ref") {
                check_one(ctx, root, reference);
            }
            for (key, value) in map {
                if key.starts_with("x-") || key == "$ref" {
                    continue;
                }
                ctx.in_field(key, |ctx| visit(ctx, root, value));
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                ctx.in_element(index, |ctx| visit(ctx, root, item));
            }
        }
        _ => {}
    }
}

/// Follow one reference through the document as plain JSON.
fn check_one(ctx: &mut Context, root: &serde_json::Value, reference: &str) {
    // External references land in a document this crate cannot see, and
    // an empty one is reported for being empty.
    let Some(local) = reference.strip_prefix('#') else {
        return;
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pointer = local.to_owned();
    let problem = loop {
        if !seen.insert(pointer.clone()) {
            break "is part of a reference cycle";
        }
        match walk_as_written(root, &pointer) {
            // Nothing to say: either it resolved, or what would have
            // resolved it is in another document.
            Walk::Deferred => return,
            Walk::Missing => break "names nothing in this document",
            Walk::Unrecognized => break "is not a usable JSON Pointer",
            Walk::Landed(target) => {
                // Keep following while the target is itself a
                // reference, which is how a cycle shows up.
                match target.get("$ref").and_then(serde_json::Value::as_str) {
                    Some(next) => match next.strip_prefix('#') {
                        Some(next) => pointer = next.to_owned(),
                        None => return,
                    },
                    None => return,
                }
            }
        }
    };
    if !ctx.has_error_at_field("$ref") {
        ctx.error_field("$ref", format!("`{reference}` {problem}"));
    }
}

/// Where a structural walk ended.
enum Walk<'v> {
    Landed(&'v serde_json::Value),
    /// The walk failed, but passed a `$ref` on the way — so what it was
    /// looking for may well exist, just not in this document as
    /// written. Not something to report.
    Deferred,
    Missing,
    Unrecognized,
}

/// Walk a pointer over the document exactly as written.
///
/// A JSON Pointer does not resolve as it walks: `#/channels/c/publish`
/// names what is written beside a `$ref` in `c`, not what that `$ref`
/// points at. But when the walk *fails* after passing one, the missing
/// step is very likely inside what the reference names — a split-file
/// document walks `#/channels/user/messages/m` through a `channels.user`
/// that is only a `$ref` — so a failure downstream of a reference is
/// left unjudged.
fn walk_as_written<'v>(root: &'v serde_json::Value, pointer: &str) -> Walk<'v> {
    let Some(tokens) = pointer::tokens(pointer) else {
        return Walk::Unrecognized;
    };
    let mut current = root;
    let mut deferred = false;
    for token in &tokens {
        deferred |= current.get("$ref").is_some();
        let next = match current {
            serde_json::Value::Object(map) => map.get(token),
            serde_json::Value::Array(items) => {
                pointer::array_index(token).and_then(|index| items.get(index))
            }
            _ => None,
        };
        match next {
            Some(value) => current = value,
            None if deferred => return Walk::Deferred,
            None => return Walk::Missing,
        }
    }
    Walk::Landed(current)
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
    fn a_chain_reports_where_it_ended_not_where_it_began() {
        let doc = document();
        let start = pointer::tokens("/entries/hop").expect("a pointer");
        let lookup = |path: &[String]| match path {
            [entries, key] if entries == "entries" => doc.entries.get(key),
            _ => None,
        };
        let (terminal, resolution) =
            follow_tracked(&doc, start, &doc.entries["hop"], "entries", lookup);
        assert_eq!(terminal, vec!["entries", "real"], "hop -> alias -> real");
        assert!(resolution.found().is_some());

        // A chain that leaves the map ends at the pointer that took it
        // there, not at the entry that pointed.
        let start = pointer::tokens("/entries/unmodeled").expect("a pointer");
        let (terminal, resolution) =
            follow_tracked(&doc, start, &doc.entries["unmodeled"], "entries", lookup);
        assert_eq!(terminal, vec!["x-extra"]);
        assert!(matches!(resolution, Resolution::Opaque));

        // An entry that is already an object ends where it started.
        let start = pointer::tokens("/entries/real").expect("a pointer");
        let (terminal, _) = follow_tracked(&doc, start, &doc.entries["real"], "entries", |_| None);
        assert_eq!(terminal, vec!["entries", "real"]);
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
