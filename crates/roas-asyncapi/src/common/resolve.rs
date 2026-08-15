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

    /// Why this pointer names the wrong sort of thing, if it does.
    ///
    /// Existence is [`check_names_something`]'s business — every
    /// reference gets that — so a check that also knows what kind
    /// belongs here reports only what that general one cannot see.
    ///
    /// (Only v3 layers a kind check over the general one; v2.6 reports
    /// each of its own resolutions in full.)
    #[must_use]
    #[cfg(any(feature = "v3_0", feature = "v3_1"))]
    pub(crate) fn kind_problem(&self) -> Option<&'static str> {
        match self {
            Resolution::WrongKind => self.problem(),
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

/// What kind of object a modelled type is, as its kind is spelled in a
/// pointer.
///
/// Lets a reference be judged wherever the model holds one — a `$ref`
/// in `info.tags` names a tag, whatever else it may resolve to — rather
/// than only where some check was written by hand.
///
/// `None` where the type is not one kind: bindings are declared under
/// four different names depending on what they bind.
pub(crate) trait Kind {
    const KIND: Option<&'static str>;
}

/// Implement [`Kind`] for a version's modelled types.
macro_rules! kinds {
    ($( $ty:ty => $kind:expr ),+ $(,)?) => {
        $(
            impl $crate::common::resolve::Kind for $ty {
                const KIND: Option<&'static str> = $kind;
            }
        )+
    };
}
pub(crate) use kinds;

/// Report a reference that names something other than the kind its
/// position calls for.
///
/// Existence is [`check_names_something`]'s business; this adds only
/// what knowing the kind reveals, and defers to any check that has
/// already spoken about the same reference.
pub(crate) fn check_names_kind<T>(ctx: &mut Context, reference: &str, expected: &str)
where
    T: DeserializeOwned,
{
    let (Some(local), Some(document)) = (reference.strip_prefix('#'), ctx.document()) else {
        return;
    };
    // A pointer that is not one, or that names nothing, is reported as
    // such rather than as the wrong kind.
    let Some(path) = pointer::tokens(local) else {
        return;
    };
    if !wrong_kind::<T>(document, &path, expected) {
        return;
    }
    if !ctx.has_error_at_field("$ref") {
        ctx.error_field(
            "$ref",
            format!("`{reference}` does not point at an object of the expected kind"),
        );
    }
}

/// Whether a pointer, judged by where it lands, names something other
/// than `expected`.
fn wrong_kind<T>(root: &serde_json::Value, path: &[String], expected: &str) -> bool
where
    T: DeserializeOwned,
{
    if names_something_else(path, expected) {
        return true;
    }
    // Nothing there, or nothing this document can say about what is
    // there: either way, not a question of kind.
    let Some(target) = pointer::walk(root, path) else {
        return false;
    };
    let Some((terminal, target)) = follow_json(root, path, target) else {
        return false;
    };
    // Where a chain ends is as much a position as where it starts: an
    // alias reached through an unmodelled location has only its far end
    // to be judged by.
    if names_something_else(&terminal, expected) {
        return true;
    }
    // A schema may be `true` or `false`, which no struct deserializes.
    if target.is_boolean() && expected == "schemas" {
        return false;
    }
    serde_json::from_value::<T>(target.clone()).is_err()
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
/// What a token names, given where in the document it sits.
#[derive(Clone, Copy)]
enum Role<'a> {
    /// A map or list whose entries are of this kind — `None` where the
    /// kind depends on context, as `traits` does.
    Collection(Option<&'a str>),
    /// One object of this kind.
    Object(Option<&'a str>),
    /// A key or index in the collection named just before it.
    Entry(Option<&'a str>),
    /// Something this crate does not model as holding objects, so
    /// nothing below it can be judged by position.
    Opaque,
}

/// The members that hold modelled objects, outside `components` where
/// every member is a map of its own kind.
///
/// Anything absent is [`Role::Opaque`]: v2.6's `publish`, a binding's
/// contents, a schema's `default`. Inference stops there rather than
/// guessing, which is the point of listing these at all.
const MEMBERS: &[(&str, Role<'static>)] = &[
    // Schemas, wherever a schema hangs off another.
    ("properties", Role::Collection(Some("schemas"))),
    ("patternProperties", Role::Collection(Some("schemas"))),
    ("definitions", Role::Collection(Some("schemas"))),
    ("allOf", Role::Collection(Some("schemas"))),
    ("anyOf", Role::Collection(Some("schemas"))),
    ("oneOf", Role::Collection(Some("schemas"))),
    ("items", Role::Collection(Some("schemas"))),
    ("additionalProperties", Role::Object(Some("schemas"))),
    ("propertyNames", Role::Object(Some("schemas"))),
    ("contains", Role::Object(Some("schemas"))),
    ("not", Role::Object(Some("schemas"))),
    ("if", Role::Object(Some("schemas"))),
    ("then", Role::Object(Some("schemas"))),
    ("else", Role::Object(Some("schemas"))),
    ("payload", Role::Object(Some("schemas"))),
    ("headers", Role::Object(Some("schemas"))),
    // The rest of the model.
    ("variables", Role::Collection(Some("serverVariables"))),
    ("messages", Role::Collection(Some("messages"))),
    ("parameters", Role::Collection(Some("parameters"))),
    ("security", Role::Collection(Some("securitySchemes"))),
    ("tags", Role::Collection(Some("tags"))),
    ("servers", Role::Collection(Some("servers"))),
    ("channel", Role::Object(Some("channels"))),
    ("correlationId", Role::Object(Some("correlationIds"))),
    ("externalDocs", Role::Object(Some("externalDocs"))),
    ("reply", Role::Object(Some("replies"))),
    ("address", Role::Object(Some("replyAddresses"))),
    // A trait is an operation's or a message's depending on where it
    // hangs, and bindings are declared under four names: both are
    // modelled, neither is one kind.
    ("traits", Role::Collection(None)),
    ("bindings", Role::Object(None)),
];

/// The role of a token, given the role of the one before it.
fn role_after<'a>(previous: Role<'a>, token: &'a str) -> Role<'a> {
    match previous {
        // A key is whatever its collection holds.
        Role::Collection(kind) => Role::Entry(kind),
        Role::Opaque | Role::Entry(None) => Role::Opaque,
        // A member of an object. Under `components` every member is a
        // map of the kind it is named for; elsewhere the model says.
        Role::Object(Some("components")) if KINDS.contains(&token) => Role::Collection(Some(token)),
        Role::Object(_) | Role::Entry(Some(_)) => {
            // An extension's contents are the extension's business.
            if token.starts_with("x-") {
                return Role::Opaque;
            }
            MEMBERS
                .iter()
                .find(|(name, _)| *name == token)
                .map_or(Role::Opaque, |(_, role)| *role)
        }
    }
}

/// Only the document's own structure declares a kind.
///
/// Each token gets a role from the one before it — a map, an object, a
/// key in a map, or something unmodelled — and the last one says what
/// the pointer names. A pointer that starts outside the structure
/// (`#/x-store/messages/c`) or passes through an extension has no role
/// to speak of, and is judged on what it finds instead.
fn names_something_else(path: &[String], expected: &str) -> bool {
    let Some(role) = role_of(path) else {
        return false;
    };
    match role {
        // A map is not one of the objects in it, and the document as a
        // whole is not an object at all.
        Role::Collection(_) => true,
        Role::Entry(Some(kind)) | Role::Object(Some(kind)) => kind != expected,
        Role::Entry(None) | Role::Object(None) | Role::Opaque => false,
    }
}

/// The role of a pointer's last token, or `None` where the pointer
/// starts outside anything this crate models.
fn role_of(path: &[String]) -> Option<Role<'_>> {
    let Some(first) = path.first() else {
        // The document itself: a container of everything.
        return Some(Role::Collection(None));
    };
    let mut role = match first.as_str() {
        "components" => Role::Object(Some("components")),
        // The root maps are named for what they hold.
        kind if KINDS.contains(&kind) => Role::Collection(Some(kind)),
        // A singleton is its own kind, which no reference names — a
        // pointer at `#/info` is the Info object and nothing else.
        single if SINGLETONS.contains(&single) => Role::Object(Some(single)),
        _ => return None,
    };
    for token in &path[1..] {
        role = role_after(role, token);
    }
    Some(role)
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
    // Serializing a document model cannot fail; falling back to `null`
    // keeps that from needing a branch, and leaves every pointer naming
    // nothing if it somehow does.
    // Position first: the document says what lives under
    // `#/components/schemas/…`, whether or not that particular key is
    // there, so naming one where a channel belongs is a kind error
    // rather than a missing one.
    if names_something_else(&path, expected_kind) {
        return Resolution::WrongKind;
    }
    let snapshot = serde_json::to_value(document).unwrap_or_default();
    if pointer::walk(&snapshot, &path).is_none() {
        return Resolution::Missing;
    }
    // The same judgement every other reference gets: what is there,
    // where the pointer says it should be, and where any chain from it
    // ends up.
    if wrong_kind::<T>(&snapshot, &path, expected_kind) {
        return Resolution::WrongKind;
    }
    Resolution::Opaque
}

/// Follow a chain of Reference Objects through plain JSON, reporting
/// where the chain ends as well as what is there.
///
/// `None` where there is nothing more this document can say: the chain
/// leaves for another document, loops, or leads nowhere — each of which
/// some other check reports, and none of which says anything about the
/// kind.
fn follow_json<'v>(
    root: &'v serde_json::Value,
    at: &[String],
    start: &'v serde_json::Value,
) -> Option<(Vec<String>, &'v serde_json::Value)> {
    let mut current = start;
    let mut terminal = at.to_vec();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while let Some(reference) = current.get("$ref").and_then(serde_json::Value::as_str) {
        let local = reference.strip_prefix('#')?;
        if !seen.insert(local.to_owned()) {
            return None;
        }
        let path = pointer::tokens(local)?;
        current = pointer::walk(root, &path)?;
        terminal = path;
    }
    Some((terminal, current))
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
    /// The resource is normalized so that spellings which resolve
    /// alike compare alike: `./channels.yaml` and `channels.yaml` name
    /// the same file, and every reference in a document resolves
    /// against the same base, so removing dot-segments is enough to
    /// tell them apart from genuinely different resources.
    ///
    /// `None` when the fragment is not a usable JSON Pointer.
    pub(crate) fn parse(reference: &str) -> Option<Self> {
        let (resource, fragment) = match reference.split_once('#') {
            Some((resource, fragment)) => (resource, fragment),
            None => (reference, ""),
        };
        Some(Self {
            resource: remove_dot_segments(resource),
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

/// Normalize a reference's percent-encoding, per
/// [RFC 3986 §6.2.2](https://www.rfc-editor.org/rfc/rfc3986#section-6.2.2):
/// decode octets that stand for unreserved characters, and case-normalize
/// the escapes that remain.
///
/// A reserved character stays encoded, `%2F` above all: decoding it
/// would turn one segment into two.
fn normalize_percent_encoding(reference: &str) -> String {
    let bytes = reference.as_bytes();
    let mut out = String::with_capacity(reference.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = (bytes[i] == b'%')
            .then(|| reference.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok().map(|byte| (hex, byte)));
        match escape {
            Some((_, byte))
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') =>
            {
                out.push(char::from(byte));
                i += 3;
            }
            Some((hex, _)) => {
                out.push('%');
                out.push_str(&hex.to_ascii_uppercase());
                i += 3;
            }
            None => {
                out.push(char::from(bytes[i]));
                i += 1;
            }
        }
    }
    out
}

/// Remove dot-segments from a URI reference's path, in the spirit of
/// [RFC 3986 §5.2.4](https://www.rfc-editor.org/rfc/rfc3986#section-5.2.4).
///
/// Only `.` and `..` segments are touched, and only in the path: a
/// scheme, authority, or query is left as written, being already in
/// comparable form — a slash inside a query is not a path separator.
/// An empty segment is a segment, so `a//b.yaml` and `a/b.yaml` are
/// different paths, though `..` removes one like any other.
///
/// A `..` that cannot be resolved is kept rather than dropped, so two
/// references that climb equally far still compare alike. §5.2.4
/// discards those, but it is defined for a path already merged onto a
/// base, and these references have none.
fn remove_dot_segments(reference: &str) -> String {
    // Unreserved characters are decoded first: `%2E` *is* a `.`, and so
    // makes a dot-segment.
    let normalized = normalize_percent_encoding(reference);
    let reference = normalized.as_str();
    // A query is not a path, however many slashes it contains.
    let (before_query, query) = match reference.find('?') {
        Some(cut) => reference.split_at(cut),
        None => (reference, ""),
    };
    let path_start = match before_query.find("//") {
        Some(slashes) if slashes == 0 || before_query[..slashes].ends_with(':') => before_query
            [slashes + 2..]
            .find('/')
            .map_or(before_query.len(), |offset| slashes + 2 + offset),
        _ => before_query.find(':').map_or(0, |colon| colon + 1),
    };
    let (prefix, path) = before_query.split_at(path_start);
    if path.is_empty() {
        return normalized;
    }

    let absolute = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    let mut segments = path.strip_prefix('/').unwrap_or(path).split('/').peekable();
    while let Some(segment) = segments.next() {
        let last = segments.peek().is_none();
        match segment {
            "." | ".." => {
                if segment == ".." {
                    match out.last() {
                        // An empty segment is a segment, and `..`
                        // removes it like any other.
                        Some(&previous) if previous != ".." => {
                            out.pop();
                        }
                        // Climbing past the start: kept where there is
                        // no base to climb through.
                        _ if !absolute => out.push(".."),
                        _ => {}
                    }
                }
                // `a/.` and `a/..` name directories, so they end in a
                // separator even though the segment itself is gone.
                if last {
                    out.push("");
                }
            }
            segment => out.push(segment),
        }
    }

    let mut normalized = String::new();
    if absolute {
        normalized.push('/');
    }
    normalized.push_str(&out.join("/"));
    // A relative path that normalizes away is the current *directory*,
    // which is not the current document.
    if normalized.is_empty() {
        normalized.push_str("./");
    }
    format!("{prefix}{normalized}{query}")
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

/// Check that a reference names something, wherever it sits.
///
/// This is the one check every reference in the document gets, because
/// it runs from [`Reference`](crate::common::reference::Reference)'s own
/// validation — the model already walks each of them, at the right
/// path, and nothing but a modelled reference is ever looked at. It
/// answers only what it can see structurally: a pointer that names
/// nothing, or a chain that loops.
///
/// It stays quiet where a check that knows what the reference is *for*
/// has already spoken, so the specific message survives.
pub(crate) fn check_names_something(ctx: &mut Context, reference: &str) {
    // External references land in a document this crate cannot see,
    // and an empty one is reported for being empty.
    let (Some(local), Some(document)) = (reference.strip_prefix('#'), ctx.document()) else {
        return;
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pointer = local.to_owned();
    let problem = loop {
        if !seen.insert(pointer.clone()) {
            break "is part of a reference cycle";
        }
        match walk_as_written(document, &pointer) {
            Walk::Missing => break "names nothing in this document",
            Walk::Unrecognized => break "is not a usable JSON Pointer",
            // Keep following while the target is itself a reference,
            // which is how a cycle shows up.
            Walk::Landed(target) => match target.get("$ref").and_then(serde_json::Value::as_str) {
                Some(next) => match next.strip_prefix('#') {
                    Some(next) => pointer = next.to_owned(),
                    None => return,
                },
                None => return,
            },
        }
    };
    if !ctx.has_error_at_field("$ref") {
        ctx.error_field("$ref", format!("`{reference}` {problem}"));
    }
}

/// Where a structural walk ended.
enum Walk<'v> {
    Landed(&'v serde_json::Value),
    Missing,
    Unrecognized,
}

/// Walk a pointer over the document exactly as written.
///
/// A JSON Pointer does not dereference as it walks
/// ([RFC 6901 §4](https://www.rfc-editor.org/rfc/rfc6901#section-4)):
/// `#/channels/c/publish` names what is written beside a `$ref` in `c`,
/// not what that `$ref` points at, and `#/components/tags/alias/name`
/// names nothing at all when `alias` is only a Reference Object. Only a
/// pointer that lands *on* such an object gets to follow it, which the
/// caller does.
fn walk_as_written<'v>(root: &'v serde_json::Value, pointer: &str) -> Walk<'v> {
    let Some(tokens) = pointer::tokens(pointer) else {
        return Walk::Unrecognized;
    };
    let mut current = root;
    for token in &tokens {
        let next = match current {
            serde_json::Value::Object(map) => map.get(token),
            serde_json::Value::Array(items) => {
                pointer::array_index(token).and_then(|index| items.get(index))
            }
            _ => None,
        };
        match next {
            Some(value) => current = value,
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
        #[serde(rename = "x-outward")]
        outward: serde_json::Value,
        #[serde(rename = "x-loop")]
        looping: serde_json::Value,
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
            outward: serde_json::json!({ "$ref": "./other.yaml#/entries/real" }),
            looping: serde_json::json!({ "$ref": "#/x-loop" }),
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

        // A target that is itself a Reference Object is followed, not
        // judged: what it leads to is what matters, and where that is
        // another document or a loop there is nothing to judge at all.
        for pointer in ["/x-outward", "/x-loop"] {
            assert_eq!(
                classify_unresolved::<_, Demo>(&doc, pointer, "entries").problem(),
                None,
                "{pointer}",
            );
        }
    }

    #[test]
    fn a_resource_keeps_everything_but_its_dot_segments() {
        for (written, resource) in [
            ("dir/./spec.yaml?v=1", "dir/spec.yaml?v=1"),
            ("./dir/", "dir/"),
            ("dir/sub/../", "dir/"),
            ("/a/b/../c", "/a/c"),
            ("/../a", "/a"),
            ("", ""),
            // An empty segment is a segment, not a typo — though
            // `..` removes one like any other.
            ("a//b.yaml", "a//b.yaml"),
            ("a//../b.yaml", "a/b.yaml"),
            // A query is not a path, however many slashes it holds.
            ("http://host?x=/a/../b", "http://host?x=/a/../b"),
            ("http://host/a/../b?q=/x/../y", "http://host/b?q=/x/../y"),
            // The current directory is not the current document.
            ("././", "./"),
            ("./", "./"),
            (".", "./"),
        ] {
            assert_eq!(
                Terminus::parse(written).expect("a resource").resource,
                resource,
                "{written}",
            );
        }
    }

    #[test]
    fn a_terminus_is_a_resource_and_a_pointer() {
        let local = Terminus::parse("#/channels/user").expect("a pointer");
        assert_eq!(local.resource, "");
        assert_eq!(local.to_string(), "#/channels/user");

        // A whole document, with no fragment at all.
        let whole = Terminus::parse("./other.yaml").expect("a resource");
        assert!(whole.at.is_empty());
        assert_eq!(whole.to_string(), "other.yaml#");

        // Spellings that resolve alike compare alike.
        for spelling in [
            "./channels.yaml",
            "channels.yaml",
            "a/../channels.yaml",
            "././channels.yaml",
        ] {
            assert_eq!(
                Terminus::parse(spelling).expect("a resource").resource,
                "channels.yaml",
                "{spelling}",
            );
        }
        // …and ones that do not, do not.
        for spelling in [
            "../channels.yaml",
            "/channels.yaml",
            "other.yaml",
            "channels.yaml/",
            "a//channels.yaml",
        ] {
            assert_ne!(
                Terminus::parse(spelling).expect("a resource").resource,
                "channels.yaml",
                "{spelling}",
            );
        }
        assert_eq!(
            Terminus::parse("https://example.com/a/../b/spec.yaml#/c")
                .expect("a resource")
                .resource,
            "https://example.com/b/spec.yaml",
        );

        // A message of a channel over there is named over there.
        let channel = Terminus::parse("./channels.yaml#/user").expect("a pointer");
        let message = Terminus::parse("channels.yaml#/user/messages/signup").expect("a pointer");
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
