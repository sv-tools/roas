//! Where a parsed document's fields sit in the text it was read from.
//!
//! The typed models are built from a [`serde_json::Value`] and keep no
//! text, so locations live beside them in a [`SourceMap`]: built from the
//! original JSON or YAML text with [`SourceMap::parse`] (feature
//! `source-map`), and queried with the same model-style paths that
//! [`ValidationError::path`](crate::validation::ValidationError) and the
//! executor's diagnostics carry, such as
//! `#.workflows[0].steps[1].successCriteria[0].condition`.
//!
//! # Offsets
//!
//! Every offset is a zero-based UTF-8 byte offset into the original text.
//! Lines and columns are one-based; a column counts Unicode scalar values
//! (`char`s) from the start of its line. Editors that speak UTF-16 (the
//! Language Server Protocol) convert at that boundary, nowhere else:
//! [`SourceMap::utf16_column`] counts a position's column in UTF-16 code
//! units.
//!
//! An offset *inside* a string value — a malformed expression three
//! characters into a condition — is a byte offset into the decoded
//! string. [`SourceMap::locate_in`] maps it back through quotes, escapes
//! (`\u00e9`, surrogate pairs, `\n`), YAML line folding and block-scalar
//! indentation. When the decoded text cannot be reproduced from the raw
//! scalar exactly, it answers `None` rather than guess.
//!
//! # Paths
//!
//! Member names may contain dots and `[n]`, so a path is matched against
//! the document's actual members. A path more than one way of reading
//! resolves (`a.b` beside `a` → `b`) is ambiguous and locates nothing.
//!
//! # Ranges
//!
//! A value's range is its own text: an aliased value is its anchor's
//! text, a flow collection runs to its closing delimiter, and a block
//! collection to its last key or value. A `<<` merge key is not seen as
//! a member, so a block map ending in one ends before it. An anchor
//! defined inside a merged member that the map's own key overrides is
//! never seen as written, so an alias of it is measured on its own.

#[cfg(feature = "source-map")]
pub(crate) mod parse;
mod scalar;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// A point in the original text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct SourcePosition {
    /// Zero-based UTF-8 byte offset.
    pub offset: usize,
    /// One-based line.
    pub line: usize,
    /// One-based column, in Unicode scalar values.
    pub column: usize,
}

impl fmt::Display for SourcePosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.column)
    }
}

/// The byte offset each line of `text` starts at, the first `0`: `\n`,
/// `\r\n` and a bare `\r` each end a line, as in YAML.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    std::iter::once(0)
        .chain(bytes.iter().enumerate().filter_map(|(at, &byte)| {
            (byte == b'\n' || (byte == b'\r' && bytes.get(at + 1) != Some(&b'\n')))
                .then_some(at + 1)
        }))
        .collect()
}

/// Where the lines of a text start, and checkpoints along long lines, so
/// a column is never counted from the start of a minified document's one
/// line.
#[derive(Clone, Debug)]
pub(crate) struct LineIndex {
    /// Byte offset each line starts at; `lines[0] == 0`.
    lines: Vec<usize>,
    /// `(byte offset, characters, UTF-16 code units since its line
    /// started)` every [`CHECKPOINT`] bytes or so along long lines.
    checkpoints: Vec<(usize, usize, usize)>,
}

impl LineIndex {
    pub(crate) fn new(text: &str) -> Self {
        let lines = line_starts(text);
        let mut checkpoints = Vec::new();
        let (mut line, mut chars, mut units, mut next) = (0, 0, 0, CHECKPOINT);
        for (at, c) in text.char_indices() {
            if lines.get(line + 1) == Some(&at) {
                line += 1;
                (chars, units, next) = (0, 0, at + CHECKPOINT);
            }
            if at >= next {
                checkpoints.push((at, chars, units));
                next = at + CHECKPOINT;
            }
            // The byte-order mark takes no column.
            if !(at == 0 && c == '\u{feff}') {
                chars += 1;
                units += c.len_utf16();
            }
        }
        Self { lines, checkpoints }
    }

    /// How many lines there are.
    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    /// The line `offset` is on, and from where on it to count: the start
    /// of the line (past a byte-order mark) or the nearest checkpoint
    /// before `offset`, with the characters and UTF-16 code units before
    /// that point.
    fn count_from(&self, text: &str, offset: usize) -> (usize, usize, usize, usize) {
        let line = self.lines.partition_point(|&start| start <= offset);
        let mut start = self.lines[line - 1];
        // A byte-order mark is not a column editors show.
        if start == 0 && text.starts_with('\u{feff}') {
            start = offset.min('\u{feff}'.len_utf8());
        }
        let nearest = self.checkpoints.partition_point(|&(at, ..)| at <= offset);
        match nearest.checked_sub(1).map(|index| self.checkpoints[index]) {
            Some((at, chars, units)) if at > start => (line, at, chars, units),
            _ => (line, start, 0, 0),
        }
    }

    /// The line and column of a byte offset, clamped to the text and to
    /// the character boundary at or before it.
    pub(crate) fn position(&self, text: &str, offset: usize) -> SourcePosition {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let (line, start, chars, _) = self.count_from(text, offset);
        SourcePosition {
            offset,
            line,
            column: chars + text[start..offset].chars().count() + 1,
        }
    }

    /// The one-based column of `position` in UTF-16 code units.
    pub(crate) fn utf16_column(&self, text: &str, position: SourcePosition) -> usize {
        let mut offset = position.offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let (_, start, _, units) = self.count_from(text, offset);
        units + text[start..offset].encode_utf16().count() + 1
    }
}

/// A span of the original text, `start` inclusive and `end` exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct SourceRange {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

/// The text could not be read as JSON or YAML; the message names where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMapError {
    message: String,
}

impl SourceMapError {
    /// What went wrong, including the parser's location when it has one.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SourceMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SourceMapError {}

/// The locations of a document's members and values.
///
/// Built from text with [`SourceMap::parse`] (feature `source-map`).
pub struct SourceMap {
    text: String,
    index: LineIndex,
    /// Boxed, so its address — a cache key — stays put when the map moves.
    root: Box<Node>,
    /// What lookups worked out, kept so many findings in one large
    /// string or object do not redo it each time.
    cache: Cache,
}

/// Lazily built lookups, keyed by the address of the node they are
/// about: the tree never changes once built.
#[derive(Default)]
struct Cache {
    /// A string's decoded-to-raw offset table (`None`: not decodable).
    decoded: Mutex<HashMap<usize, Option<Table>>>,
    /// A large map's members by name, as indices into it.
    members: Mutex<HashMap<usize, Index>>,
}

/// Raw offsets of a string's decoded bytes.
type Table = Arc<[usize]>;
/// A map's members by name, as indices into it.
type Index = Arc<HashMap<String, Vec<usize>>>;

/// Maps with fewer members are scanned rather than indexed.
const INDEXED: usize = 16;

fn address(node: &Node) -> usize {
    std::ptr::from_ref(node) as usize
}

impl Cache {
    fn decoded(&self, node: &Node, build: impl FnOnce() -> Option<Vec<usize>>) -> Option<Table> {
        let lock = || {
            self.decoded
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        };
        if let Some(table) = lock().get(&address(node)) {
            return table.clone();
        }
        // Decode without the lock: other strings need not wait for it.
        let table = build().map(Arc::from);
        lock().entry(address(node)).or_insert(table).clone()
    }

    fn members(&self, node: &Node, members: &[(Key, Node)]) -> Index {
        let mut cache = self
            .members
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .entry(address(node))
            .or_insert_with(|| {
                let mut index: HashMap<String, Vec<usize>> = HashMap::new();
                for (at, (key, _)) in members.iter().enumerate() {
                    index.entry(key.name.clone()).or_default().push(at);
                }
                Arc::new(index)
            })
            .clone()
    }
}

impl Clone for SourceMap {
    /// A clone has its own tree, so it starts with an empty cache.
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            index: self.index.clone(),
            root: self.root.clone(),
            cache: Cache::default(),
        }
    }
}

/// How far apart column checkpoints are, in bytes.
const CHECKPOINT: usize = 1024;

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
struct Node {
    start: usize,
    end: usize,
    /// Where the raw scalar ends for decoding; past `end` for a block
    /// scalar, whose kept breaks and trailing spaces follow its content.
    text_end: usize,
    kind: Kind,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
enum Kind {
    Map(Vec<(Key, Node)>),
    Seq(Vec<Node>),
    /// The decoded value of a string scalar; `None` for numbers, booleans
    /// and null.
    Scalar(Option<String>),
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
struct Key {
    name: String,
    /// Where the member is written in its mapping: the key, or, for an
    /// alias key, a member merged in or one of a replayed alias, the
    /// `*name` that brings it there, not the anchor.
    start: usize,
    end: usize,
}

impl fmt::Debug for SourceMap {
    /// The size of what is mapped, not the text itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceMap")
            .field("bytes", &self.text.len())
            .field("lines", &self.index.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq)]
enum Token<'p> {
    Key(&'p str),
    /// The digits as written: `[00]` is not `[0]` to a member name.
    Index(&'p str),
}

impl SourceMap {
    /// The text the map was built from.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The whole value at `path` (`#` is the document itself).
    pub fn locate(&self, path: &str) -> Option<SourceRange> {
        let (node, _) = self.resolve(path)?;
        Some(self.range(node.start, node.end))
    }

    /// The member name `path` ends in, where it is written in its object:
    /// the name, or, for a name an alias brings in — an alias key
    /// (`*k : v`), a member merged with `<<: *a`, a member of a replayed
    /// `*o` — that `*name`, where the object can be edited, rather than
    /// the anchor. `None` for the document itself and for array items.
    pub fn locate_key(&self, path: &str) -> Option<SourceRange> {
        let (_, key) = self.resolve(path)?;
        let key = key?;
        Some(self.range(key.start, key.end))
    }

    /// `offset` bytes into the decoded string at `path`, in the original
    /// text. `None` when `path` is not a string, `offset` is past its end
    /// or not on a character boundary, or the raw scalar cannot be decoded
    /// back to exactly the value that was parsed.
    pub fn locate_in(&self, path: &str, offset: usize) -> Option<SourcePosition> {
        let (node, _) = self.resolve(path)?;
        let Kind::Scalar(Some(value)) = &node.kind else {
            return None;
        };
        if !value.is_char_boundary(offset) {
            return None;
        }
        let table = self.cache.decoded(node, || {
            scalar::decode(&self.text, node.start, node.text_end, value)
        })?;
        table.get(offset).map(|&raw| self.position(raw))
    }

    /// The one-based column of `position` counted in UTF-16 code units,
    /// as an LSP client counts them (zero-based there: subtract one).
    /// `position` is one of this map's.
    #[must_use]
    pub fn utf16_column(&self, position: SourcePosition) -> usize {
        self.index.utf16_column(&self.text, position)
    }

    /// The line and column of a byte offset, clamped to the text and to
    /// the character boundary at or before it.
    pub fn position(&self, offset: usize) -> SourcePosition {
        self.index.position(&self.text, offset)
    }

    fn range(&self, start: usize, end: usize) -> SourceRange {
        SourceRange {
            start: self.position(start),
            end: self.position(end),
        }
    }

    #[cfg_attr(not(feature = "source-map"), allow(dead_code))]
    fn new(text: String, root: Node) -> Self {
        Self {
            index: LineIndex::new(&text),
            text,
            root: Box::new(root),
            cache: Cache::default(),
        }
    }

    fn resolve(&self, path: &str) -> Option<(&Node, Option<&Key>)> {
        resolve(&self.root, &tokens(path)?, &self.cache)
    }

    /// Where the first repeat of key `name` in a mapping starts, keys
    /// compared as JSON reads them (`1` and `'1'` repeat), in the order
    /// JSON reading meets it: a value before the key after it. Of the two,
    /// the one written later in the mapping, where an alias key or a merge
    /// counts at its `*name`, not at the anchor.
    #[cfg(feature = "editor")]
    pub(crate) fn repeated_key(&self, name: &str) -> Option<usize> {
        fn find(node: &Node, name: &str) -> Option<usize> {
            match &node.kind {
                Kind::Map(members) => {
                    let mut seen = None;
                    for (key, value) in members {
                        if let Some(found) = find(value, name) {
                            return Some(found);
                        }
                        if key.name == name {
                            // A merge's members come after the map's own,
                            // whatever the text order: take the later.
                            match seen {
                                Some(first) => return Some(key.start.max(first)),
                                None => seen = Some(key.start),
                            }
                        }
                    }
                    None
                }
                Kind::Seq(items) => items.iter().find_map(|item| find(item, name)),
                Kind::Scalar(_) => None,
            }
        }
        // A map is no deeper than `MAX_DEPTH`, so this recursion is bounded.
        find(&self.root, name)
    }

    /// How many places `path` names: `0` when it is not written, `1` when
    /// [`SourceMap::locate`] finds it, more when it is ambiguous; `None`
    /// when it is no path a map can follow.
    #[cfg(feature = "editor")]
    pub(crate) fn places(&self, path: &str) -> Option<usize> {
        let tokens = tokens(path)?;
        let mut found = Vec::new();
        walk(&self.root, &tokens, None, &self.cache, &mut found);
        Some(found.len())
    }
}

/// `#.a.b[0][1].c` → `a`, `b`, `0`, `1`, `c`. A leading `#` is optional;
/// `None` when a bracket is not a closed run of digits.
fn tokens(original: &str) -> Option<Vec<Token<'_>>> {
    let mut path = original.strip_prefix('#').unwrap_or(original);
    let mut tokens = Vec::new();
    match path.strip_prefix('.') {
        // `#.` is the member named `""`, not the document.
        Some("") => return Some(vec![Token::Key("")]),
        Some(rest) => path = rest,
        None if path.is_empty() => return Some(tokens),
        None => {}
    }
    let dotted = original
        .strip_prefix('#')
        .unwrap_or(original)
        .starts_with('.');
    for (position, segment) in path.split('.').enumerate() {
        let (name, mut rest) = segment.split_at(segment.find('[').unwrap_or(segment.len()));
        // An empty name is kept: `foo..x` is member `foo.` then `x`, and
        // `a.[0]` is member `a.` then index `0` — never a sibling. Only a
        // path that opens on `[n]` (`#[0]`, a root array) has none.
        if !name.is_empty() || rest.is_empty() || position > 0 || dotted {
            tokens.push(Token::Key(name));
        }
        while let Some(tail) = rest.strip_prefix('[') {
            let (index, after) = tail.split_once(']')?;
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            tokens.push(Token::Index(index));
            rest = after;
        }
        if !rest.is_empty() {
            return None;
        }
    }
    Some(tokens)
}

/// Walk `tokens` down from `node`. A member name may itself contain dots
/// or `[n]` (`a.b`, `items[0]`), so every split of the path into names is
/// tried; a path more than one split resolves is ambiguous and locates
/// nothing, rather than whichever split came first.
fn resolve<'n>(
    node: &'n Node,
    tokens: &[Token<'_>],
    cache: &Cache,
) -> Option<(&'n Node, Option<&'n Key>)> {
    let mut found: Vec<(&'n Node, Option<&'n Key>)> = Vec::new();
    walk(node, tokens, None, cache, &mut found);
    match found.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

fn walk<'n>(
    node: &'n Node,
    tokens: &[Token<'_>],
    key: Option<&'n Key>,
    cache: &Cache,
    found: &mut Vec<(&'n Node, Option<&'n Key>)>,
) {
    match (tokens.first(), &node.kind) {
        (None, _) if !found.iter().any(|(seen, _)| std::ptr::eq(*seen, node)) => {
            found.push((node, key));
        }
        (Some(Token::Index(index)), Kind::Seq(items)) => {
            if let Some(item) = index
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get(index))
            {
                walk(item, &tokens[1..], None, cache, found);
            }
        }
        (Some(Token::Key(first)), Kind::Map(members)) => {
            let index = (members.len() > INDEXED).then(|| cache.members(node, members));
            let mut name = String::from(*first);
            for taken in 1..=tokens.len() {
                if taken > 1 {
                    match &tokens[taken - 1] {
                        Token::Key(part) => {
                            name.push('.');
                            name.push_str(part);
                        }
                        Token::Index(index) => {
                            name.push('[');
                            name.push_str(index);
                            name.push(']');
                        }
                    }
                }
                let named: Vec<&(Key, Node)> = match &index {
                    Some(index) => index
                        .get(&name)
                        .into_iter()
                        .flatten()
                        .map(|&at| &members[at])
                        .collect(),
                    None => members
                        .iter()
                        .filter(|(member, _)| member.name == name)
                        .collect(),
                };
                for (member, child) in named {
                    walk(child, &tokens[taken..], Some(member), cache, found);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(start: usize, end: usize, value: &str) -> Node {
        Node {
            start,
            end,
            text_end: end,
            kind: Kind::Scalar(Some(value.into())),
        }
    }

    fn key(name: &str, start: usize) -> Key {
        Key {
            name: name.into(),
            start,
            end: start + name.len(),
        }
    }

    /// `a.b: x` / `a: {b: [y]}` by hand, so resolution is tested in every
    /// feature configuration.
    fn dotted() -> SourceMap {
        let text = "a.b: x\na:\n  b:\n  - y\n";
        let inner = Node {
            start: 12,
            end: 20,
            text_end: 20,
            kind: Kind::Map(vec![(
                key("b", 12),
                Node {
                    start: 17,
                    end: 20,
                    text_end: 20,
                    kind: Kind::Seq(vec![scalar(19, 20, "y")]),
                },
            )]),
        };
        let root = Node {
            start: 0,
            end: 20,
            text_end: 20,
            kind: Kind::Map(vec![
                (key("a.b", 0), scalar(5, 6, "x")),
                (key("a", 7), inner),
            ]),
        };
        SourceMap::new(text.into(), root)
    }

    #[test]
    fn paths_tokenize_into_names_and_indices() {
        assert_eq!(tokens("#"), Some(vec![]));
        assert_eq!(tokens(""), Some(vec![]));
        assert_eq!(tokens("#."), Some(vec![Token::Key("")]));
        assert_eq!(tokens("#[0]"), Some(vec![Token::Index("0")]));
        assert_eq!(
            tokens("#.[0]"),
            Some(vec![Token::Key(""), Token::Index("0")])
        );
        assert_eq!(
            tokens("#.a.[0]"),
            Some(vec![Token::Key("a"), Token::Key(""), Token::Index("0")])
        );
        assert_eq!(
            tokens("#.workflows[0].steps[12][3].type.version"),
            Some(vec![
                Token::Key("workflows"),
                Token::Index("0"),
                Token::Key("steps"),
                Token::Index("12"),
                Token::Index("3"),
                Token::Key("type"),
                Token::Key("version"),
            ])
        );
        assert_eq!(tokens("a.b"), Some(vec![Token::Key("a"), Token::Key("b")]));
        assert_eq!(
            tokens("#.outputs.foo..x"),
            Some(vec![
                Token::Key("outputs"),
                Token::Key("foo"),
                Token::Key(""),
                Token::Key("x")
            ])
        );
        for broken in ["#.a[", "#.a[x]", "#.a[]", "#.a[0]b", "#.a[1"] {
            assert_eq!(tokens(broken), None, "{broken}");
        }
    }

    #[test]
    fn dotted_member_names_resolve_by_backtracking() {
        let map = dotted();
        // `a.b` is both a member of its own and a path through `a`:
        // ambiguous, so nothing is located rather than a guess.
        assert_eq!(map.locate("#.a.b"), None);
        assert_eq!(map.locate_key("#.a.b"), None);
        // One more step only resolves through `a`.
        assert_eq!(map.locate("#.a.b[0]").unwrap().start.offset, 19);
        assert_eq!(map.locate("#.a.b[1]"), None);
        assert_eq!(map.locate("#.a.c"), None);
        assert_eq!(map.locate("#.a[0]"), None);
        assert_eq!(map.locate("#.a.b[0].z"), None);
        assert_eq!(map.locate("#.a[x]"), None);
        assert_eq!(map.locate_key("#"), None);
        assert_eq!(map.locate_key("#.a.b[0]"), None);
        assert_eq!(map.locate("#").unwrap().end.offset, 20);
    }

    #[test]
    fn a_member_name_with_dots_is_found_when_nothing_shorter_resolves() {
        let text = "a.b: x\n";
        let root = Node {
            start: 0,
            end: 6,
            text_end: 6,
            kind: Kind::Map(vec![(key("a.b", 0), scalar(5, 6, "x"))]),
        };
        let map = SourceMap::new(text.into(), root);
        let range = map.locate("#.a.b").unwrap();
        assert_eq!((range.start.offset, range.end.offset), (5, 6));
        assert_eq!(map.locate_key("#.a.b").unwrap().end.offset, 3);
        assert_eq!(map.locate_in("#.a.b", 0).unwrap().offset, 5);
    }

    #[test]
    fn an_empty_name_segment_is_part_of_the_member_name() {
        // Members `foo` and `foo.`, each with an `x`.
        let text = "foo: {x: 1}\nfoo.: {x: 2}\n";
        let member = |name: &str, start: usize, value: usize| {
            (
                key(name, start),
                Node {
                    start: value - 4,
                    end: value + 2,
                    text_end: value + 2,
                    kind: Kind::Map(vec![(key("x", value - 3), scalar(value, value + 1, "1"))]),
                },
            )
        };
        let root = Node {
            start: 0,
            end: text.len(),
            text_end: text.len(),
            kind: Kind::Map(vec![member("foo", 0, 9), member("foo.", 12, 22)]),
        };
        let map = SourceMap::new(text.into(), root);
        assert_eq!(map.locate("#.foo..x").unwrap().start.offset, 22);
        assert_eq!(map.locate("#.foo.x").unwrap().start.offset, 9);
    }

    #[test]
    fn a_member_name_with_an_index_is_told_apart_from_an_array() {
        let text = "items: [x]\nitems[0]: y\n";
        let both = Node {
            start: 0,
            end: text.len(),
            text_end: text.len(),
            kind: Kind::Map(vec![
                (
                    key("items", 0),
                    Node {
                        start: 7,
                        end: 10,
                        text_end: 10,
                        kind: Kind::Seq(vec![scalar(8, 9, "x")]),
                    },
                ),
                (key("items[0]", 11), scalar(21, 22, "y")),
            ]),
        };
        let map = SourceMap::new(text.into(), both.clone());
        assert_eq!(map.locate("#.items[0]"), None);
        let Kind::Map(members) = both.kind else {
            unreachable!()
        };
        let literal = Node {
            kind: Kind::Map(members.into_iter().skip(1).collect()),
            ..both
        };
        let map = SourceMap::new(text.into(), literal);
        assert_eq!(map.locate("#.items[0]").unwrap().start.offset, 21);
        assert_eq!(map.locate_key("#.items[0]").unwrap().end.offset, 19);
        // Leading zeros are part of a member name, not an index.
        assert_eq!(map.locate("#.items[00]"), None);
        assert_eq!(format!("{map:?}"), "SourceMap { bytes: 23, lines: 3, .. }");
    }

    #[test]
    fn a_byte_order_mark_takes_no_column() {
        let map = SourceMap::new(
            "\u{feff}a: 1\nb".into(),
            Node {
                start: 0,
                end: 0,
                text_end: 0,
                kind: Kind::Scalar(None),
            },
        );
        assert_eq!(map.position(0).column, 1);
        assert_eq!(map.position(3).column, 1);
        assert_eq!(map.position(6).column, 4);
        assert_eq!(map.position(8).column, 1);
    }

    #[test]
    fn a_bare_carriage_return_ends_a_line() {
        let map = SourceMap::new(
            "a\rb\r\nc\nd".into(),
            Node {
                start: 0,
                end: 0,
                text_end: 0,
                kind: Kind::Scalar(None),
            },
        );
        let at = |offset| (map.position(offset).line, map.position(offset).column);
        assert_eq!(
            [at(0), at(2), at(3), at(5), at(7)],
            [(1, 1), (2, 1), (2, 2), (3, 1), (4, 1)]
        );
    }

    /// Columns on long lines come from checkpoints: they agree with
    /// counting from the line start, across lines, multi-byte characters
    /// and a byte-order mark.
    #[test]
    #[cfg(feature = "source-map")]
    fn checkpointed_columns_agree_with_counting() {
        let line = "é".repeat(700) + &"😀".repeat(200) + &"x".repeat(1500);
        let map =
            SourceMap::parse(format!("\u{feff}a: {line}\r\nb: {line}\n\rc: {line}\n")).unwrap();
        assert!(!map.index.checkpoints.is_empty());
        let raw = map.text().to_owned();
        let starts: Vec<usize> = map.index.lines.clone();
        for (offset, _) in raw.char_indices().step_by(7) {
            let position = map.position(offset);
            let line_start = match position.line {
                1 => offset.min(3),
                line => starts[line - 1],
            };
            let expected = raw[line_start..offset].chars().count() + 1;
            assert_eq!(position.column, expected, "at {offset}");
            let units = raw[line_start..offset].encode_utf16().count() + 1;
            assert_eq!(map.utf16_column(position), units, "at {offset}");
        }
    }

    /// Many findings in one long string decode it once; many findings in
    /// one wide object look its members up by name.
    #[test]
    #[cfg(feature = "source-map")]
    fn repeated_lookups_reuse_what_they_worked_out() {
        let value = "{$statusCode.extra} ".repeat(5_000);
        let members = (0..10_000)
            .map(|index| {
                format!(
                    "\"m{index}\": \"{value_of}\"",
                    value_of = if index == 0 { value.as_str() } else { "x" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let map = SourceMap::parse(format!("{{{members}}}")).unwrap();
        let started = std::time::Instant::now();
        for offset in (0..value.len()).step_by(20) {
            assert!(map.locate_in("#.m0", offset).is_some());
        }
        for index in 0..10_000 {
            assert!(map.locate(&format!("#.m{index}")).is_some());
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        // A clone starts afresh and finds the same.
        let clone = map.clone();
        assert_eq!(clone.locate_in("#.m0", 20), map.locate_in("#.m0", 20));
        assert_eq!(clone.locate("#.m9999"), map.locate("#.m9999"));
        assert_eq!(map.locate("#.missing"), None);
    }

    /// Locating every member of a minified document is linear, not a
    /// count from the start of its one line each time.
    #[test]
    #[cfg(feature = "source-map")]
    fn many_positions_on_one_long_line_are_cheap() {
        let text = format!("[{}]", vec![r#"{"a":"é"}"#; 50_000].join(","));
        let map = SourceMap::parse(text).unwrap();
        let started = std::time::Instant::now();
        for index in (0..50_000).step_by(5) {
            assert!(map.locate(&format!("#[{index}].a")).is_some());
        }
        assert_eq!(map.locate("#[49999].a").unwrap().start.column, 499_997);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn positions_count_lines_and_characters() {
        let map = SourceMap::new(
            "é: 1\n  ü\n".into(),
            Node {
                start: 0,
                end: 0,
                text_end: 0,
                kind: Kind::Scalar(None),
            },
        );
        let at = |offset| {
            let position = map.position(offset);
            (position.offset, position.line, position.column)
        };
        assert_eq!(at(0), (0, 1, 1));
        // Inside `é`: clamped back to its first byte.
        assert_eq!(at(1), (0, 1, 1));
        assert_eq!(at(2), (2, 1, 2));
        assert_eq!(at(6), (6, 2, 1));
        assert_eq!(at(8), (8, 2, 3));
        assert_eq!(at(100), (11, 3, 1));
        assert_eq!(map.position(8).to_string(), "line 2, column 3");
        assert_eq!(map.text(), "é: 1\n  ü\n");
    }

    #[test]
    fn inner_offsets_only_land_on_strings_and_character_boundaries() {
        let map = SourceMap::new(
            "a: \"é\"\nb: 1\n".into(),
            Node {
                start: 0,
                end: 12,
                text_end: 12,
                kind: Kind::Map(vec![
                    (key("a", 0), scalar(3, 7, "é")),
                    (
                        key("b", 8),
                        Node {
                            start: 11,
                            end: 12,
                            text_end: 12,
                            kind: Kind::Scalar(None),
                        },
                    ),
                ]),
            },
        );
        assert_eq!(map.locate_in("#.a", 0).unwrap().offset, 4);
        assert_eq!(map.locate_in("#.a", 1), None);
        assert_eq!(map.locate_in("#.a", 2).unwrap().offset, 6);
        assert_eq!(map.locate_in("#.a", 3), None);
        assert_eq!(map.locate_in("#.b", 0), None);
        assert_eq!(map.locate_in("#.c", 0), None);
    }

    #[test]
    fn the_error_type_shows_its_message() {
        let error = SourceMapError {
            message: "bad".into(),
        };
        assert_eq!(error.message(), "bad");
        assert_eq!(error.to_string(), "bad");
    }
}
