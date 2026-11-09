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
//! Language Server Protocol) convert at that boundary, from the line's
//! text and the byte offset, and nowhere else.
//!
//! An offset *inside* a string value — a malformed expression three
//! characters into a condition — is a byte offset into the decoded
//! string. [`SourceMap::locate_in`] maps it back through quotes, escapes
//! (`\u00e9`, surrogate pairs, `\n`), YAML line folding and block-scalar
//! indentation. When the decoded text cannot be reproduced from the raw
//! scalar exactly, it answers `None` rather than guess.

#[cfg(feature = "source-map")]
mod parse;
mod scalar;

use std::fmt;

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
#[derive(Debug, Clone)]
pub struct SourceMap {
    text: String,
    /// Byte offset each line starts at; `lines[0] == 0`.
    lines: Vec<usize>,
    root: Node,
}

#[derive(Debug, Clone)]
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
struct Node {
    start: usize,
    end: usize,
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
    start: usize,
    end: usize,
}

#[derive(Debug, PartialEq)]
enum Token<'p> {
    Key(&'p str),
    Index(usize),
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

    /// The member name `path` ends in; `None` for the document itself and
    /// for array items.
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
        let table = scalar::decode(&self.text, node.start, node.end, value)?;
        table.get(offset).map(|&raw| self.position(raw))
    }

    /// The line and column of a byte offset, clamped to the text and to
    /// the character boundary at or before it.
    pub fn position(&self, offset: usize) -> SourcePosition {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.lines.partition_point(|&start| start <= offset);
        let start = self.lines[line - 1];
        SourcePosition {
            offset,
            line,
            column: self.text[start..offset].chars().count() + 1,
        }
    }

    fn range(&self, start: usize, end: usize) -> SourceRange {
        SourceRange {
            start: self.position(start),
            end: self.position(end),
        }
    }

    #[cfg_attr(not(feature = "source-map"), allow(dead_code))]
    fn new(text: String, root: Node) -> Self {
        let lines = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Self { text, lines, root }
    }

    fn resolve(&self, path: &str) -> Option<(&Node, Option<&Key>)> {
        resolve(&self.root, &tokens(path)?)
    }
}

/// `#.a.b[0][1].c` → `a`, `b`, `0`, `1`, `c`. A leading `#` is optional;
/// `None` when a bracket is not a closed run of digits.
fn tokens(path: &str) -> Option<Vec<Token<'_>>> {
    let path = path.strip_prefix('#').unwrap_or(path);
    let path = path.strip_prefix('.').unwrap_or(path);
    let mut tokens = Vec::new();
    if path.is_empty() {
        return Some(tokens);
    }
    for segment in path.split('.') {
        let (name, mut rest) = segment.split_at(segment.find('[').unwrap_or(segment.len()));
        if !name.is_empty() {
            tokens.push(Token::Key(name));
        }
        while let Some(tail) = rest.strip_prefix('[') {
            let (index, after) = tail.split_once(']')?;
            if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            tokens.push(Token::Index(index.parse().ok()?));
            rest = after;
        }
        if !rest.is_empty() {
            return None;
        }
    }
    Some(tokens)
}

/// Walk `tokens` down from `node`. Member names may themselves contain
/// dots, so a run of name tokens is tried as one name, two joined by
/// `.`, and so on, backtracking until the rest of the path resolves.
fn resolve<'n>(node: &'n Node, tokens: &[Token<'_>]) -> Option<(&'n Node, Option<&'n Key>)> {
    let Some(first) = tokens.first() else {
        return Some((node, None));
    };
    match (first, &node.kind) {
        (Token::Index(index), Kind::Seq(items)) => resolve(items.get(*index)?, &tokens[1..]),
        (Token::Key(_), Kind::Map(members)) => {
            let names = tokens
                .iter()
                .take_while(|token| matches!(token, Token::Key(_)))
                .count();
            let mut name = String::new();
            for taken in 1..=names {
                let Token::Key(part) = &tokens[taken - 1] else {
                    unreachable!("counted name tokens only");
                };
                if taken > 1 {
                    name.push('.');
                }
                name.push_str(part);
                let Some((key, child)) = members.iter().find(|(key, _)| key.name == name) else {
                    continue;
                };
                let rest = &tokens[taken..];
                if rest.is_empty() {
                    return Some((child, Some(key)));
                }
                if let Some(found) = resolve(child, rest) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(start: usize, end: usize, value: &str) -> Node {
        Node {
            start,
            end,
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
            kind: Kind::Map(vec![(
                key("b", 12),
                Node {
                    start: 17,
                    end: 20,
                    kind: Kind::Seq(vec![scalar(19, 20, "y")]),
                },
            )]),
        };
        let root = Node {
            start: 0,
            end: 20,
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
        assert_eq!(
            tokens("#.workflows[0].steps[12][3].type.version"),
            Some(vec![
                Token::Key("workflows"),
                Token::Index(0),
                Token::Key("steps"),
                Token::Index(12),
                Token::Index(3),
                Token::Key("type"),
                Token::Key("version"),
            ])
        );
        assert_eq!(tokens("a.b"), Some(vec![Token::Key("a"), Token::Key("b")]));
        for broken in ["#.a[", "#.a[x]", "#.a[]", "#.a[0]b", "#.a[1"] {
            assert_eq!(tokens(broken), None, "{broken}");
        }
    }

    #[test]
    fn dotted_member_names_resolve_by_backtracking() {
        let map = dotted();
        // `a.b` is both a member of its own and a path through `a`; the
        // shorter split wins only when the rest of the path resolves.
        assert_eq!(map.locate("#.a.b").unwrap().start.offset, 17);
        assert_eq!(map.locate("#.a.b[0]").unwrap().start.offset, 19);
        assert_eq!(map.locate_key("#.a.b").unwrap().start.offset, 12);
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
            kind: Kind::Map(vec![(key("a.b", 0), scalar(5, 6, "x"))]),
        };
        let map = SourceMap::new(text.into(), root);
        let range = map.locate("#.a.b").unwrap();
        assert_eq!((range.start.offset, range.end.offset), (5, 6));
        assert_eq!(map.locate_key("#.a.b").unwrap().end.offset, 3);
        assert_eq!(map.locate_in("#.a.b", 0).unwrap().offset, 5);
    }

    #[test]
    fn positions_count_lines_and_characters() {
        let map = SourceMap::new(
            "é: 1\n  ü\n".into(),
            Node {
                start: 0,
                end: 0,
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
                kind: Kind::Map(vec![
                    (key("a", 0), scalar(3, 7, "é")),
                    (
                        key("b", 8),
                        Node {
                            start: 11,
                            end: 12,
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
