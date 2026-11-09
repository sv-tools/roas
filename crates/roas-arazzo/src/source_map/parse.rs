//! Build a [`SourceMap`] from JSON or YAML text with `serde-saphyr`, which
//! reports a byte span for every key and scalar.

use super::{Key, Kind, Node, SourceMap, SourceMapError};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_saphyr::Spanned;
use std::collections::HashMap;
use std::fmt;

impl SourceMap {
    /// Read `text` (JSON or YAML; JSON is read as the YAML it also is) and
    /// record where each member and value sits.
    ///
    /// The map is a sidecar: it locates fields of the document that was
    /// parsed separately, and does not itself validate or authorize
    /// anything. Text the typed parse accepts but this rejects (or the
    /// reverse) simply has no map — for instance a member repeated
    /// anywhere, which `serde_json` takes last-wins and this refuses, or
    /// nesting deeper than 32 levels.
    ///
    /// ```
    /// use roas_arazzo::source_map::SourceMap;
    ///
    /// let map = SourceMap::parse("workflows:\n  - workflowId: \"caf\\u00e9\"\n").unwrap();
    /// let value = map.locate("#.workflows[0].workflowId").unwrap();
    /// assert_eq!((value.start.line, value.start.column), (2, 17));
    /// // `é` is the fourth character of the value, spelled `é`.
    /// assert_eq!(map.locate_in("#.workflows[0].workflowId", 3).unwrap().column, 21);
    /// ```
    pub fn parse(text: impl Into<String>) -> Result<Self, SourceMapError> {
        let text = text.into();
        let root: Spanned<Raw> =
            serde_saphyr::from_str_with_options(&text, options()).map_err(|error| {
                SourceMapError {
                    message: error.to_string(),
                }
            })?;
        let (root, _) = node(&text, root, false, &mut HashMap::new());
        Ok(Self::new(text, root))
    }
}

/// Nesting past this many levels is refused rather than mapped.
const MAX_DEPTH: usize = 32;

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        // Same options in every roas crate; keep the copies in step
        // (`grep -rn 'strict_booleans: true' crates`). The budget is raised
        // from the defaults (250k nodes, 1M events, 50k aliases, 50k anchors,
        // 10k merge keys), which reject ordinary large or generated specs. Depth stays at 64: deeper nesting overflows a 2 MiB
        // thread stack in debug builds. The alias-to-anchor ratio check is off:
        // it rejects any document reusing one anchor 100+ times, a common
        // pattern in specs. Alias replay is raised to 10M events so a large
        // schema can be aliased thousands of times; the 1M cap on events
        // recorded under anchors still stops an alias bomb, and also caps
        // how large one anchored node can be. `!!binary` is
        // read as its text, as before, not decoded.
        budget: serde_saphyr::budget! {
            // Not the 64 of the other copies: this visitor nests
            // `Spanned` values, and a debug build overflows a 2 MiB
            // thread stack between 40 and 50 levels. Deeper text gets no
            // map (`SourceMapError`), never an abort.
            max_depth: MAX_DEPTH,
            max_nodes: 10_000_000,
            max_events: 25_000_000,
            max_aliases: 1_000_000,
            max_anchors: 1_000_000,
            max_merge_keys: 1_000_000,
            enforce_alias_anchor_ratio: false,
        },
        alias_limits: serde_saphyr::alias_limits! {
            max_total_replayed_events: 10_000_000,
        },
        strict_booleans: true,
        ignore_binary_tag_for_string: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    }
}

/// What `serde-saphyr` hands over, before offsets are made absolute.
enum Raw {
    Map(Vec<(Spanned<Raw>, Spanned<Raw>)>),
    Seq(Vec<Spanned<Raw>>),
    String(String),
    Other,
}

fn span(text: &str, spanned: &Spanned<Raw>) -> (usize, usize) {
    // Where the value is written: for an alias, its anchor.
    let span = spanned.defined.span();
    let start = span.byte_offset().map_or(0, |offset| offset as usize);
    let end = start + span.byte_len().map_or(0, |len| len as usize);
    (start.min(text.len()), end.min(text.len()))
}

/// The node, and where its occurrence ends: for an alias, the end of the
/// `*name` that uses it rather than of the anchored value, so the
/// collection holding the alias is measured where the alias is written.
/// Inside a replayed alias (`replayed`) every node is measured at its
/// definition: the `*name` belongs to the alias alone. `ends` holds the
/// end measured for each collection defined so far, by its start.
fn node(
    text: &str,
    spanned: Spanned<Raw>,
    replayed: bool,
    ends: &mut HashMap<usize, usize>,
) -> (Node, usize) {
    let alias = spanned.referenced != spanned.defined;
    let used = spanned.referenced.span();
    let used = used.byte_offset().map_or(0, |offset| offset as usize)
        + used.byte_len().map_or(0, |len| len as usize);
    let (mut start, mut end) = span(text, &spanned);
    let mut text_end = end;
    match spanned.value {
        Raw::String(_) | Raw::Other if start == end => {
            if let Some(header) = super::scalar::empty_block(text, start) {
                (start, end, text_end) = (header, header, header);
            }
        }
        // A block scalar's span runs on into the next line's indentation;
        // its range stops at its content, its decoding sees all of it.
        Raw::String(_) if super::scalar::is_block(text, start) => {
            end = start + text[start..end].trim_end().len();
        }
        _ => {}
    }
    let inner = replayed || alias;
    let mut first = None;
    let kind = match spanned.value {
        Raw::Map(members) => Kind::Map(
            members
                .into_iter()
                .map(|(key, value)| {
                    let (key_start, key_end) = span(text, &key);
                    let name = match key.value {
                        Raw::String(name) => name,
                        // `1: x` reads as the member `"1"`.
                        _ => text[key_start..key_end].to_owned(),
                    };
                    let (mut value, mut value_end) = node(text, value, inner, ends);
                    // `? key` with no value: its empty value is given the
                    // next token's place, on a later line. It belongs
                    // right after the key (or its `:`).
                    if value.start == value.end
                        && matches!(value.kind, Kind::Scalar(None))
                        // An alias to an empty value sits back at its anchor.
                        && text
                            .get(key_end..value.start)
                            .is_some_and(|gap| gap.contains(['\n', '\r']))
                    {
                        let at = colon(text, key_end);
                        (value.start, value.end, value.text_end, value_end) = (at, at, at, at);
                    }
                    end = end.max(key_end).max(value_end);
                    (
                        Key {
                            name,
                            start: key_start,
                            end: key_end,
                        },
                        value,
                    )
                })
                .collect(),
        ),
        Raw::Seq(items) => Kind::Seq(
            items
                .into_iter()
                .map(|item| {
                    let (item, item_end) = node(text, item, inner, ends);
                    first.get_or_insert(item.start);
                    end = end.max(item_end);
                    item
                })
                .collect(),
        ),
        Raw::String(value) => Kind::Scalar(Some(value)),
        Raw::Other => Kind::Scalar(None),
    };
    if !matches!(kind, Kind::Scalar(_)) {
        // An indentless sequence (`a:\n- x`) is reported from its first
        // item's first token (`x`, `&k`, `!t`, `|`, `*q`, `[`), not from
        // its own `-`: a sequence not starting at a `-` with one just
        // before it is that. A flow sequence (`- [1]` written inside an
        // item) starts at `[` too, but before its first item. Only the
        // spaces before it are looked at — never the whole line, which in
        // minified JSON is the whole document.
        let spaces = text[..start].len() - text[..start].trim_end_matches(' ').len();
        let dash = start - spaces;
        let opens = text.as_bytes().get(start).copied();
        let indentless = matches!(kind, Kind::Seq(_))
            && spaces > 0
            && text[..dash].ends_with('-')
            && match opens {
                // At a `-` or `[` that opens its first item, not itself.
                Some(b'-' | b'[') => first == Some(start),
                _ => true,
            };
        if indentless {
            start = dash - 1;
        } else if let Some(at) = empty_first_item(text, &kind, first, start) {
            start = at;
        }
        // A replayed alias is its definition: reuse the range measured
        // there, which saw where its own nested aliases are written — and
        // do not measure it again per alias.
        let known = (replayed || alias)
            .then(|| ends.get(&start).copied())
            .flatten();
        end = match (known, text.as_bytes().get(start)) {
            (Some(known), _) => known,
            // Only a collection of the delimiter's kind: `[a]: 1` in a flow
            // sequence is a one-pair map that starts at its key's `[`.
            (None, Some(b'[')) if !indentless && matches!(kind, Kind::Seq(_)) => {
                closing(text, start, end)
            }
            (None, Some(b'{')) if matches!(kind, Kind::Map(_)) => closing(text, start, end),
            _ => colon(text, end),
        };
        if !(replayed || alias) {
            ends.insert(start, end);
        }
        text_end = end;
    }
    let occurrence = if alias && !replayed {
        used.min(text.len())
    } else {
        end
    };
    (
        Node {
            start,
            end,
            text_end,
            kind,
        },
        occurrence,
    )
}

/// An indentless sequence whose first item is empty (`a:\n- # note`,
/// `a:\n-\n- x`) is reported at the end of that item's line, past its
/// `-` and any comment: back up to the `-`, if only indentation is
/// before it. Only that one line is looked at.
fn empty_first_item(text: &str, kind: &Kind, first: Option<usize>, start: usize) -> Option<usize> {
    let Kind::Seq(items) = kind else { return None };
    let item = items.first()?;
    if first != Some(start) || item.start != item.end || !matches!(item.kind, Kind::Scalar(None)) {
        return None;
    }
    let line = text[..start].rfind(['\n', '\r']).map_or(0, |at| at + 1);
    let prefix = &text[line..start];
    let comment = prefix
        .char_indices()
        .find(|&(at, c)| c == '#' && (at == 0 || prefix[..at].ends_with([' ', '\t'])))
        .map_or(prefix.len(), |(at, _)| at);
    let before = prefix[..comment]
        .trim_end_matches([' ', '\t'])
        .strip_suffix('-')?;
    before
        .bytes()
        .all(|byte| byte == b' ')
        .then_some(line + before.len())
}

/// A block collection has no delimiter: it ends at its last key or value,
/// and past the `:` of an empty last value. A `<<` merge ending a map is
/// not seen as a member, so it is not in the range.
fn colon(text: &str, end: usize) -> usize {
    let rest = &text[end..];
    let after = rest.trim_start_matches([' ', '\t']);
    if after.starts_with(':') {
        end + (rest.len() - after.len()) + 1
    } else {
        end
    }
}

/// A flow collection's span covers only its opening `[` / `{`, and its
/// members end before the closing one — or, merged in by `<<` (measured
/// where their alias or map is), well before it.
fn closing(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    // Usually only separators, white space and comments are left between
    // the last member and the delimiter: step over them.
    let close = if bytes[start] == b'[' { b']' } else { b'}' };
    let mut at = end.max(start + 1);
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' | b'?' => at += 1,
            b'#' => at += text[at..].find(['\n', '\r']).unwrap_or(text.len() - at),
            byte if byte == close => return at + 1,
            _ => break,
        }
    }
    // Something else is in the way (a `<<` merge, whose members are
    // measured where they are defined): scan the whole collection.
    scan(text, start, end)
}

/// The end of the flow collection opening at `start`: scan from it to
/// its match, stepping over quoted scalars and comments; a plain scalar
/// holds no flow indicator, so every other bracket counts. Only merges
/// get here, and nested ones rescan their own text: at most
/// [`MAX_DEPTH`] times.
fn scan(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut at = start;
    // A quote opens a scalar only where a token starts: after a flow
    // indicator, the `: ` of a value, or the `:` of a JSON-like key
    // (`"a":"b"`), past any anchor or tag. White space keeps the state,
    // so `b 'c` is one plain scalar; a comment needs white space before it.
    let mut token = true;
    let mut space = false;
    let mut quoted = false;
    while let Some(&byte) = bytes.get(at) {
        let was_quoted = std::mem::take(&mut quoted);
        let was_space = std::mem::take(&mut space);
        match byte {
            b'[' | b'{' => {
                depth += 1;
                token = true;
            }
            b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return (at + 1).max(end);
                }
                token = false;
                quoted = true;
            }
            b',' => token = true,
            // `"a" : "b"` is still a JSON-like key's `:`.
            b' ' | b'\t' | b'\n' | b'\r' => (space, quoted) = (true, was_quoted),
            // An anchor or tag before a node keeps its place a token start.
            // A verbatim tag `!<…>` runs to its `>`.
            b'!' if token && bytes.get(at + 1) == Some(&b'<') => {
                at += text[at..]
                    .find('>')
                    .map_or(text.len() - at, |close| close + 1);
                continue;
            }
            b'&' | b'!' if token => {
                at += text[at..]
                    .find([' ', '\t', '\n', '\r', ',', '[', ']', '{', '}'])
                    .unwrap_or(text.len() - at);
                continue;
            }
            // `?` is an indicator only where a token starts: `a ? b` is
            // one plain scalar.
            b'?' => {}
            b':' => {
                let next = bytes.get(at + 1).copied();
                token = was_quoted
                    || matches!(
                        next,
                        None | Some(
                            b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}'
                        )
                    );
            }
            b'#' if was_space || token => {
                at += text[at..].find(['\n', '\r']).unwrap_or(text.len() - at);
                continue;
            }
            b'"' | b'\'' if token => {
                at = quoted_end(text, at);
                token = false;
                quoted = true;
                continue;
            }
            _ => token = false,
        }
        at += 1;
    }
    end
}

/// Just past the quoted scalar opening at `at` (or the end of the text).
fn quoted_end(text: &str, at: usize) -> usize {
    let bytes = text.as_bytes();
    let quote = bytes[at];
    let mut next = at + 1;
    while let Some(&byte) = bytes.get(next) {
        match byte {
            b'\\' if quote == b'"' => next += 2,
            b'\'' if quote == b'\'' && bytes.get(next + 1) == Some(&b'\'') => next += 2,
            byte if byte == quote => return next + 1,
            _ => next += 1,
        }
    }
    text.len()
}

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RawVisitor)
    }
}

struct RawVisitor;

impl<'de> Visitor<'de> for RawVisitor {
    type Value = Raw;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON or YAML value")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Raw, E> {
        Ok(Raw::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Raw, E> {
        Ok(Raw::String(value))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_i128<E: de::Error>(self, _: i128) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_u128<E: de::Error>(self, _: u128) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Raw, E> {
        Ok(Raw::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Raw, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Raw::Seq(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Raw, A::Error> {
        let mut members = Vec::new();
        while let Some(key) = map.next_key()? {
            members.push((key, map.next_value()?));
        }
        Ok(Raw::Map(members))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(map: &SourceMap, path: &str) -> (usize, usize) {
        let range = map.locate(path).unwrap_or_else(|| panic!("{path}"));
        (range.start.line, range.start.column)
    }

    fn inner(map: &SourceMap, path: &str, offset: usize) -> Option<(usize, usize)> {
        map.locate_in(path, offset)
            .map(|position| (position.line, position.column))
    }

    #[test]
    fn json_members_and_escaped_strings_are_located() {
        let text = "{\n  \"workflows\": [\n    {\"workflowId\": \"w\",\n     \"steps\": [{\"stepId\": \"s\", \"successCriteria\": [{\"condition\": \"$st\\u00e9p\\n == 😀 && $x =\"}]}]}\n  ]\n}\n";
        let map = SourceMap::parse(text).unwrap();
        let path = "#.workflows[0].steps[0].successCriteria[0].condition";
        assert_eq!(at(&map, "#.workflows[0].workflowId"), (3, 20));
        assert_eq!(at(&map, path), (4, 66));
        assert_eq!(
            map.locate_key("#.workflows[0].steps")
                .map(|range| range.start.column),
            Some(6)
        );
        // `p` after `é`; `😀` after the `\n` escape; the trailing `=`.
        let value = "$stép\n == 😀 && $x =";
        assert_eq!(inner(&map, path, value.find('p').unwrap()), Some((4, 76)));
        assert_eq!(inner(&map, path, value.find('😀').unwrap()), Some((4, 83)));
        assert_eq!(inner(&map, path, value.len() - 1), Some((4, 91)));
        assert_eq!(inner(&map, path, value.len()), Some((4, 92)));
        assert_eq!(inner(&map, path, value.len() + 1), None);
    }

    #[test]
    fn yaml_scalars_of_every_style_are_located() {
        let text = "\
workflows:
- workflowId: w
  steps:
  - stepId: s
    successCriteria:
    - condition: $statusCode == ü0
    - condition: \"$st\\u00e9p == \\\"x\\\"\"
    - condition: 'it''s == 1'
    - condition: |
        $a == 1 &&
          $b
    - condition: >-
        $a ==
        2
";
        let map = SourceMap::parse(text).unwrap();
        let criterion =
            |index: usize| format!("#.workflows[0].steps[0].successCriteria[{index}].condition");
        assert_eq!(at(&map, &criterion(0)), (6, 18));
        assert_eq!(
            inner(&map, &criterion(0), "$statusCode == ü".len()),
            Some((6, 34))
        );
        assert_eq!(inner(&map, &criterion(1), "$sté".len()), Some((7, 28)));
        assert_eq!(inner(&map, &criterion(1), "$stép == ".len()), Some((7, 33)));
        assert_eq!(inner(&map, &criterion(2), 3), Some((8, 23)));
        assert_eq!(at(&map, &criterion(3)), (10, 9));
        assert_eq!(
            inner(&map, &criterion(3), "$a == 1 &&\n  ".len()),
            Some((11, 11))
        );
        assert_eq!(inner(&map, &criterion(4), "$a == ".len()), Some((14, 9)));
        // Numbers and booleans have a place but no inner offsets.
        let map = SourceMap::parse("a: 1\nb: true\n1: x\n").unwrap();
        assert_eq!(at(&map, "#.b"), (2, 4));
        assert_eq!(inner(&map, "#.a", 0), None);
        assert_eq!(at(&map, "#.1"), (3, 4));
    }

    #[test]
    fn collections_span_their_contents_and_aliases_point_at_their_anchor() {
        let text = "a: &shared\n  b: [1, \"two\"]\nc: *shared\nd:\n  <<: *shared\n  e: 3\n";
        let map = SourceMap::parse(text).unwrap();
        let a = map.locate("#.a").unwrap();
        assert_eq!(a.start.line, 2);
        assert!(a.end.offset >= text.find("\"two\"").unwrap() + 5);
        assert_eq!(at(&map, "#.a.b[1]"), (2, 10));
        assert_eq!(at(&map, "#.c.b[1]"), (2, 10));
        assert_eq!(at(&map, "#.d.b[1]"), (2, 10));
        assert_eq!(at(&map, "#.d.e"), (6, 6));
    }

    #[test]
    fn scalars_of_every_type_have_a_place() {
        let text = "a: -1\nb: 1.5\nc: null\nd: ~\ne: 340282366920938463463374607431768211455\nf: -170141183460469231731687303715884105728\ng: false\nh: .inf\ni: !!binary aGk=\nj:\n";
        let map = SourceMap::parse(text).unwrap();
        for (line, member) in ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(at(&map, &format!("#.{member}")).0, line + 1, "{member}");
        }
        assert!(map.locate("#.j").is_some());
        assert_eq!(inner(&map, "#.i", 0), Some((9, 13)));
        assert_eq!(inner(&map, "#.b", 0), None);
    }

    /// What `serde-saphyr` hands over for numbers past 64 bits, owned
    /// strings and bytes, which no JSON or YAML text above produces.
    #[test]
    fn the_visitor_takes_every_value_a_deserializer_offers() {
        use serde::de::IntoDeserializer;
        use serde::de::value::{BytesDeserializer, Error};
        let other = |raw: Result<Raw, Error>| matches!(raw, Ok(Raw::Other));
        assert!(other(Raw::deserialize(
            IntoDeserializer::<Error>::into_deserializer(1_i128)
        )));
        assert!(other(Raw::deserialize(
            IntoDeserializer::<Error>::into_deserializer(1_u128)
        )));
        assert!(matches!(
            Raw::deserialize(IntoDeserializer::<Error>::into_deserializer(String::from("x"))),
            Ok(Raw::String(text)) if text == "x"
        ));
        let error = Raw::deserialize(BytesDeserializer::<Error>::new(b"x"))
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("a JSON or YAML value"),
            "{error}"
        );
    }

    #[test]
    fn flow_collections_end_at_their_closing_delimiter() {
        let end = |text: &str, path: &str| {
            SourceMap::parse(text)
                .unwrap()
                .locate(path)
                .unwrap()
                .end
                .offset
        };
        assert_eq!(end("[1, 2]", "#"), 6);
        assert_eq!(end("[]", "#"), 2);
        assert_eq!(end("{}", "#"), 2);
        assert_eq!(end("{\"a\": {\"b\": [1, \"x\" ]} , }", "#.a"), 22);
        assert_eq!(end("{\"a\": {\"b\": [1, \"x\" ]} , }", "#"), 26);
        assert_eq!(end("a: [1, # one\n  2\n  ]\n", "#.a"), 20);
        // A trailing alias is measured where it is written, not at its anchor.
        assert_eq!(end("a: &one abc\nb: [*one]\n", "#.b"), 21);
        assert_eq!(end("a: &one abc\nb: {k: *one}\n", "#.b"), 24);
        assert_eq!(end("a: &one abc\nb:\n  - *one\nc: 1\n", "#.b"), 23);
        // An empty last value ends after its `:`.
        assert_eq!(end("k: {a: }\n", "#.k"), 8);
        assert_eq!(end("k: {a: 1, b: }\n", "#.k"), 14);
        assert_eq!(end("k: [a: ]\n", "#.k"), 8);
        assert_eq!(end("k: {a: , }\n", "#.k"), 10);
        assert_eq!(end("k:\n  a: 1\n  b:\nc: 1\n", "#.k"), 14);
        // A `<<` merge is not a member this sees: a map ending in one
        // ends at its last own member (documented).
        assert_eq!(end("base: &b\n  x: 1\nd:\n  x: 2\n  <<: *b\n", "#.d"), 25);
        // Comments and blank lines after the last item are not part of it;
        // a sibling key at the sequence's own column ends it.
        assert_eq!(end("k:\n- 1 # one\n\n# note\nz: 2\n", "#.k"), 6);
        assert_eq!(end("- a: 1\n  b: 2\n- c\n", "#[0]"), 13);
        assert_eq!(end("a:\n  - x\n...\n", "#"), 8);
        // A block scalar ending the last member does not take the sibling.
        let steps =
            "steps:\n  - stepId: a\n    description: |\n      text\n  - stepId: b\n    x: 1\n";
        assert_eq!(end(steps, "#.steps[0]"), steps.find("text").unwrap() + 4);
        assert_eq!(end(steps, "#.steps"), steps.len() - 1);
        assert_eq!(end("a:\n  b:\n    c: |\n      x\n  d: 1\n", "#.a.b"), 24);
        assert_eq!(end("k:\n  a: 1\n  b: |+\n    x\n\n\nz: 1\n", "#.k"), 23);
        assert_eq!(end("a:\r  - 1\r  - 2\rb: 1\r", "#.a"), 14);
        assert_eq!(end("a:\n  b: |\n    text\nc: 1\n", "#.a"), 18);
        assert_eq!(end("a:\r  b: value\rc: 2\r", "#.a"), 13);
        // Block collections have no delimiter: they end at their last item.
        assert_eq!(end("a:\n  - 1\n  - 2\nb: 3\n", "#.a"), 14);
    }

    #[test]
    fn nesting_past_the_limit_is_refused_not_overflowed() {
        let nested = |depth: usize| format!("{}1{}", "{\"a\": ".repeat(depth), "}".repeat(depth));
        // On a small stack, in a debug build: refused, never an abort.
        let outcome = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                (
                    SourceMap::parse(nested(MAX_DEPTH - 1)).is_ok(),
                    SourceMap::parse(nested(100)).is_err(),
                )
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(outcome, (true, true));
    }

    #[test]
    fn an_empty_block_scalar_is_where_its_header_is() {
        for text in ["k: |\nz: 1\n", "k: |-  # none\n\nz: 1\n", "- >-\n- 2\n"] {
            let map = SourceMap::parse(text).unwrap();
            let path = if text.starts_with('-') { "#[0]" } else { "#.k" };
            let range = map.locate(path).unwrap();
            assert_eq!(range.start.line, 1, "{text:?}");
            assert_eq!(range.start, range.end, "{text:?}");
            assert_eq!(
                map.locate_in(path, 0).map(|at| at.line),
                Some(1),
                "{text:?}"
            );
        }
        // An explicit indentation indicator is a header too.
        let map = SourceMap::parse("a: |2-\nnext: 1\n").unwrap();
        assert_eq!(map.locate("#.a").unwrap().start.offset, 6);
        // Kept trailing breaks: the end of the value is after them.
        let map = SourceMap::parse("a: |+\n  one\n\n\nb: 1\n").unwrap();
        assert_eq!(map.locate_in("#.a", 6).unwrap().offset, 14);
        assert_eq!(map.locate_in("#.a", 3).unwrap().offset, 11);
        let map = SourceMap::parse("a: |+\r\n  one\r\n\r\nb: 1\r\n").unwrap();
        assert_eq!(map.locate_in("#.a", 5).unwrap().offset, 16);
        // Trailing spaces at the end of the text are content, mapped forward.
        let map = SourceMap::parse("k: |\n    -   ").unwrap();
        let ends: Vec<_> = (0..=5)
            .map(|at| map.locate_in("#.k", at).map(|p| p.offset))
            .collect();
        assert_eq!(
            ends,
            [Some(9), Some(10), Some(11), Some(12), Some(13), Some(13)]
        );
        // A block whose content opens with a quote is still a block.
        let map = SourceMap::parse("k: >-\n  \"ok\" == \"ok\" &&\n  (\n").unwrap();
        assert_eq!(map.locate_in("#.k", 0).unwrap().offset, 8);
        assert_eq!(map.locate_in("#.k", 16).unwrap().offset, 26);
        // Bare `\r` line ends.
        let map = SourceMap::parse("k: |\rz: 1\r").unwrap();
        assert_eq!(map.locate("#.k").unwrap().start.offset, 4);
        let map = SourceMap::parse("k: |\r  text\rz: 1\r").unwrap();
        let k = map.locate("#.k").unwrap();
        assert_eq!((k.start.offset, k.end.offset), (7, 11));
        assert_eq!(map.locate_in("#.k", 4).unwrap().offset, 11);
        // Blank lines only: their breaks are not in the text, so no offset.
        let map = SourceMap::parse("k: |+\n\n\nz: 1\n").unwrap();
        assert_eq!(map.locate("#.k").unwrap().start.line, 1);
        assert_eq!(map.locate_in("#.k", 0), None);
    }

    #[test]
    fn nested_sequences_and_aliased_collections_keep_their_own_ranges() {
        let range = |text: &str, path: &str| {
            let range = SourceMap::parse(text).unwrap().locate(path).unwrap();
            (range.start.offset, range.end.offset)
        };
        assert_eq!(range("- - a\n  - b\n- c\n", "#[0]"), (2, 11));
        assert_eq!(
            range("x:\n  a:\n  - - 1\n    - 2\n  - 3\n  b: 1\n", "#.x.a[0]"),
            (12, 23)
        );
        assert_eq!(
            range("seq:\n- a\n-   - b\n    - c\n- d\n", "#.seq[1]"),
            (13, 24)
        );
        // An aliased collection is its anchor's text, not anchor-to-alias.
        assert_eq!(range("a: &x [1]\nb: *x\n", "#.b"), (6, 9));
        assert_eq!(range("a: &x\n  k: 1\nb: *x\nc: 2\n", "#.b"), (8, 12));
        assert_eq!(
            range("a: &x {k: [1, 2]}\nzz: 1\nb: *x\n", "#.b.k"),
            (10, 16)
        );
        // An indentless sequence of flow items starts at its first `-`.
        assert_eq!(range("a:\n- [1]\n- [2]\nb: 1\n", "#.a"), (3, 14));
        assert_eq!(range("a:\n- [1]\n- [2]\nb: 1\n", "#.a[0]"), (5, 8));
        // An alias of a collection holding an alias: the definition's range.
        assert_eq!(range("a: &x [1]\nb: &y [*x]\nc: *y\n", "#.c"), (16, 20));
        assert_eq!(range("a: &x [1]\nb: &y\n  - *x\nc: *y\n", "#.c"), (18, 22));
        // A mapping inside a flow sequence ends at its own value.
        assert_eq!(range("a: [x: y, z: w]\n", "#.a[0]"), (4, 8));
        assert_eq!(range("a: [x: y, z: w]\n", "#.a"), (3, 15));
        assert_eq!(range("a: &x [one]\nb: *x\n", "#.b"), (6, 11));
        // A block scalar's range stops at its content.
        assert_eq!(
            range("a:\n  - x\n  - |\n    t\n\n  - z\n", "#.a[1]"),
            (19, 20)
        );
    }

    #[test]
    fn an_indentless_sequence_starts_at_its_dash_whatever_its_first_item() {
        let start = |text: &str, path: &str| {
            SourceMap::parse(text)
                .unwrap()
                .locate(path)
                .unwrap()
                .start
                .offset
        };
        for text in [
            "a:\n- &k x\n- y\nb: 1\n",
            "a:\n- !t x\n- y\n",
            "a:\n- |\n  x\n- y\nb: 1\n",
            "a:\n- >-\n- y\n",
            "a:\n- {k: 1}\n",
            "a:\n- - 1\n",
        ] {
            assert_eq!(start(text, "#.a"), 3, "{text:?}");
        }
        assert_eq!(start("x: &q 1\na:\n- *q\n- y\n", "#.a"), 11);
        assert_eq!(start("a: &x\n  - 1\nb:\n- *x\n", "#.b"), 15);
        assert_eq!(
            start("steps:\n- &s\n  stepId: a\n- stepId: b\n", "#.steps"),
            7
        );
        // A block map in an item, and a flow sequence in one, are not moved.
        assert_eq!(start("- a: 1\n", "#[0]"), 2);
        assert_eq!(start("- [1, 2]\n", "#[0]"), 2);
    }

    #[test]
    fn an_explicit_key_without_a_value_is_at_the_key() {
        let map = SourceMap::parse("x:\n  a: 1\n  ? b\nc: 2\n").unwrap();
        let b = map.locate("#.x.b").unwrap();
        assert_eq!((b.start.offset, b.end.offset), (15, 15));
        assert_eq!(map.locate("#.x").unwrap().end.offset, 15);
    }

    #[test]
    fn an_alias_to_an_empty_value_stays_at_its_anchor() {
        let map = SourceMap::parse("a: &x\nb: *x\n").unwrap();
        assert_eq!(map.locate("#.b").unwrap().start.offset, 5);
        // Explicit keys, and an empty block scalar as the key: none panics.
        for text in ["a: &x\n? b\n: *x\n", "? |-\n  ", "\t? |\r\n"] {
            let _ = SourceMap::parse(text);
        }
    }

    /// A block of blank lines must not rescan the rest of them per line.
    #[test]
    fn a_long_run_of_blank_block_lines_decodes_in_linear_time() {
        // `x`, its break (folded away under `>`), the blank lines, `y`.
        for (style, y) in [("|", 50_002), (">", 50_001)] {
            let text = format!("a: {style}\n  x\n{}  y\n", "\n".repeat(50_000));
            let map = SourceMap::parse(text.clone()).unwrap();
            let started = std::time::Instant::now();
            let y = map.locate_in("#.a", y).unwrap();
            assert_eq!(y.offset, text.rfind('y').unwrap(), "{style}");
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "{style}: {:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn an_indentless_sequence_starts_at_its_first_dash() {
        let map = SourceMap::parse("a:\n- x\n- y\n").unwrap();
        let range = map.locate("#.a").unwrap();
        assert_eq!((range.start.offset, range.end.offset), (3, 10));
        let map = SourceMap::parse("a:\n  - x\n").unwrap();
        assert_eq!(map.locate("#.a").unwrap().start.offset, 5);
        assert_eq!(map.locate("#.a[0]").unwrap().start.offset, 7);
    }

    #[test]
    fn an_indentless_sequence_with_an_empty_first_item_starts_at_its_dash() {
        for (text, range) in [
            ("a:\n- # placeholder\n- x\n", (3, 22)),
            ("a:\n- # c\n", (3, 8)),
            ("a:\n-\n- x\n", (3, 8)),
            ("a:\n-\n", (3, 4)),
            ("a:\n- \n- x\n", (3, 9)),
        ] {
            let map = SourceMap::parse(text).unwrap();
            let a = map.locate("#.a").unwrap();
            assert_eq!((a.start.offset, a.end.offset), range, "{text:?}");
        }
        // An indented sequence is reported at its dash already.
        let map = SourceMap::parse("a:\n  - # c\n  - x\n").unwrap();
        assert_eq!(map.locate("#.a").unwrap().start.offset, 5);
        let map = SourceMap::parse("a:\n- - # c\n").unwrap();
        assert_eq!(map.locate("#.a").unwrap().start.offset, 3);
    }

    #[test]
    fn a_flow_map_ends_past_a_trailing_merge_that_adds_nothing() {
        for (text, close) in [
            ("base: &b {foo: x}\nobj: {foo: y, <<: *b}\n", 39),
            ("base: &b {foo: x}\nobj: {foo: y, << : [ *b, *b ] }\n", 49),
            ("base: &b {foo: x}\nobj: [{foo: y, <<: *b}]\n", 41),
        ] {
            let map = SourceMap::parse(text).unwrap();
            let obj = map.locate("#.obj").unwrap();
            assert_eq!((obj.start.offset, obj.end.offset), (23, close), "{text:?}");
        }
        for (text, close) in [
            // A merged member is measured at its `*b`, inside `[…]`.
            ("base: &b {x: 1}\nobj: {<<: [*b]}\n", 31),
            ("base: &b {x: 1}\nobj: {<<: [*b, *b]}\n", 35),
            ("base: &b {x: 1}\nobj: {<<: *b}\n", 29),
        ] {
            let map = SourceMap::parse(text).unwrap();
            let obj = map.locate("#.obj").unwrap();
            assert_eq!((obj.start.offset, obj.end.offset), (21, close), "{text:?}");
            assert_eq!(map.locate("#").unwrap().end.offset, close, "{text:?}");
        }
    }

    #[test]
    fn a_flow_collection_ends_at_its_own_delimiter_whatever_is_inside() {
        for (text, path, range) in [
            // Merges of maps, kept or overridden, and of a list of aliases.
            (
                "base: &b {x: 1}\nobj: {foo: 2, <<: [*b, *b]}\n",
                "#.obj",
                (21, 43),
            ),
            ("obj: {foo: y, <<: {bar: 1}}\nz: 1\n", "#.obj", (5, 27)),
            ("obj: {foo: y, <<: {foo: 1}}\n", "#.obj", (5, 27)),
            ("obj: [{foo: y, <<: {foo: 1}}]\n", "#.obj", (5, 29)),
            ("obj: [{foo: y, <<: {foo: 1}}]\n", "#", (0, 29)),
            (
                "obj:\n  - {foo: y, <<: {bar: 1}}\n  - q\n",
                "#.obj[0]",
                (9, 31),
            ),
            // Brackets inside quotes, plain scalars and comments.
            ("obj: {a: \"<<: [x\", b: 1}\n", "#.obj", (5, 24)),
            ("obj: {a: '}'']', b: \"\\\"]\"}\n", "#.obj", (5, 26)),
            ("obj: {a: b 'c, d: e'}\n", "#.obj", (5, 21)),
            ("obj: {\"a\":'}'}\n", "#.obj", (5, 14)),
            ("obj: [a, # ] }\n  b]\n", "#.obj", (5, 19)),
            ("obj: {\"a\" :\"}\"}\n", "#.obj", (5, 15)),
            ("obj: {\"a\"\n  :\n  \"}\"}\n", "#.obj", (5, 20)),
            ("a: [&s \"]\", x]\n", "#.a", (3, 14)),
            ("a: [!!str ']', &t !!str \"[\", *t]\n", "#.a", (3, 32)),
            ("a: [x&y, '!]']\n", "#.a", (3, 14)),
            // Behind a trailing merge the whole collection is scanned.
            ("obj: {a: \"<<: [x\", b: 1, <<: {m: 1}}\n", "#.obj", (5, 36)),
            (
                "obj: {a: '}'']', b: \"\\\"]\", <<: {m: '}'}}\n",
                "#.obj",
                (5, 40),
            ),
            ("obj: {a: b 'c, d: e', <<: {m: 1}}\n", "#.obj", (5, 33)),
            ("obj: {\"a\" :'}', <<: {\"m\" : \"]\"}}\n", "#.obj", (5, 32)),
            ("obj: {a: [x, # ] }\n  y], <<: {m: 1}}\n", "#.obj", (5, 36)),
            (
                "obj: {a: &s \"]\", <<: {m: !!str \"}\"}}\n",
                "#.obj",
                (5, 36),
            ),
            ("obj: {a: x&y, b: '!}', <<: [{m: 1}]}\n", "#.obj", (5, 36)),
            ("obj: {a: x?y, b: x:y, <<: {m: 1}} # }\n", "#.obj", (5, 33)),
            // JSON-like keys spaced from `:`, properties, `?` mid-scalar.
            ("{\"a\": {\"b\" :\",\"}, \"c\": \"d\"}", "#.a", (6, 16)),
            ("{\"a\": {\"b\" :\"}\"}, \"c\": \"d\"}", "#.a", (6, 16)),
            ("a: [[&x ' #'],\n  [b]]", "#.a[0]", (4, 13)),
            ("k: [&x 'a]b', c]", "#.k", (3, 16)),
            ("k: [!!str ' #', b]", "#.k", (3, 18)),
            ("k: [[a ? 'x], 'y']", "#.k[0]", (4, 12)),
            ("k: [!<tag:a]> x, y]", "#.k", (3, 19)),
            ("k: {a: [!<tag:a]> x, y], <<: {m: 1}}", "#.k", (3, 36)),
            ("k: {a: [a ? 'x], <<: {m: ']'}}", "#.k", (3, 30)),
            // A one-pair map whose key is a flow sequence.
            ("k: [[a]: 1]\n", "#.k[0]", (4, 10)),
            ("k: [[a]: [b]]\n", "#.k[0]", (4, 12)),
            ("k: [[a]: [b]]\n", "#.k", (3, 13)),
        ] {
            let map = SourceMap::parse(text).unwrap();
            let found = map.locate(path).unwrap();
            assert_eq!(
                (found.start.offset, found.end.offset),
                range,
                "{text:?} {path}: {:?}",
                &text[found.start.offset..found.end.offset]
            );
        }
    }

    /// An alias reuses its anchor's range instead of measuring it again.
    #[test]
    fn many_aliases_of_a_large_collection_map_in_linear_time() {
        let text = format!(
            "a: &x [1{}]\nb: [{}]\n",
            " ".repeat(100_000),
            vec!["*x"; 10_000].join(",")
        );
        let started = std::time::Instant::now();
        let map = SourceMap::parse(text.clone()).unwrap();
        let alias = map.locate("#.b[9999]").unwrap();
        assert_eq!((alias.start.offset, alias.end.offset), (6, 100_009));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_byte_order_mark_shifts_no_column() {
        let map = SourceMap::parse("\u{feff}a: 1\nb: 2\n").unwrap();
        assert_eq!(at(&map, "#.a"), (1, 4));
        assert_eq!(at(&map, "#.b"), (2, 4));
    }

    /// Minified JSON is one long line: nothing may rescan it per value.
    #[test]
    fn a_minified_document_maps_in_linear_time() {
        let item = r#"{"a":[1,"x"],"b":{"c":"y"}}"#;
        let text = format!("[{}]", vec![item; 20_000].join(","));
        let started = std::time::Instant::now();
        let map = SourceMap::parse(text).unwrap();
        assert_eq!(map.locate("#[19999].b.c").unwrap().start.line, 1);
        // Quadratic took tens of seconds here; linear takes well under one.
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn unreadable_text_reports_where() {
        let error = SourceMap::parse("a: [1,\n").unwrap_err();
        assert!(error.message().contains("line"), "{error}");
        let error = SourceMap::parse("a: 1\na: 2\n").unwrap_err();
        assert!(error.message().contains('a'), "{error}");
    }
}
