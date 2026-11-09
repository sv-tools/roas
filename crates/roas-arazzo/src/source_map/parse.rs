//! Build a [`SourceMap`] from JSON or YAML text with `serde-saphyr`, which
//! reports a byte span for every key and scalar.

use super::{Key, Kind, Node, SourceMap, SourceMapError};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_saphyr::Spanned;
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
        let (root, _) = node(&text, root);
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
fn node(text: &str, spanned: Spanned<Raw>) -> (Node, usize) {
    let alias = spanned.referenced != spanned.defined;
    let used = spanned.referenced.span();
    let used = used.byte_offset().map_or(0, |offset| offset as usize)
        + used.byte_len().map_or(0, |len| len as usize);
    let (mut start, mut end) = span(text, &spanned);
    if start == end
        && matches!(spanned.value, Raw::String(_) | Raw::Other)
        && let Some(header) = super::scalar::empty_block(text, start)
    {
        (start, end) = (header, header);
    }
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
                    let (value, value_end) = node(text, value);
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
                    let (item, item_end) = node(text, item);
                    end = end.max(item_end);
                    item
                })
                .collect(),
        ),
        Raw::String(value) => Kind::Scalar(Some(value)),
        Raw::Other => Kind::Scalar(None),
    };
    if !matches!(kind, Kind::Scalar(_)) {
        end = match text.as_bytes().get(start) {
            Some(b'[' | b'{') => closing(text, start, end),
            _ => block_end(text, start, end),
        };
    }
    let occurrence = if alias { used.min(text.len()) } else { end };
    (Node { start, end, kind }, occurrence)
}

/// A block collection has no delimiter, and what ends it may be nothing
/// this sees as a member — the `:` of an empty last value, a `<<` merge.
/// It runs on through the rest of the line its last item ends on, then
/// through every line indented at least as far as it starts (a sequence:
/// sibling lines must be `-` items), skipping blank and comment lines.
fn block_end(text: &str, start: usize, end: usize) -> usize {
    let column = start - text[..start].rfind('\n').map_or(0, |at| at + 1);
    let sequence = text[start..].starts_with('-');
    let content_end = |line: usize, body: &str| {
        let comment = body
            .char_indices()
            .find(|&(at, c)| c == '#' && (at == 0 || body[..at].ends_with([' ', '\t'])))
            .map_or(body.len(), |(at, _)| at);
        line + body[..comment].trim_end().len()
    };
    let line_of = |at: usize| {
        text[at..]
            .find('\n')
            .map_or(text.len(), |newline| at + newline)
    };
    let mut last = end;
    let rest = &text[end..line_of(end)];
    last = last.max(content_end(end, rest.trim_end_matches('\r')));
    let mut line = line_of(end) + 1;
    while line < text.len() {
        let stop = line_of(line);
        let body = text[line..stop].trim_end_matches('\r');
        let indent = body.len() - body.trim_start_matches(' ').len();
        let content = &body[indent..];
        if !(content.is_empty() || content.starts_with('#')) {
            let item = content == "-" || content.starts_with("- ");
            if indent < column
                || (indent == column && sequence && !item)
                || (column == 0 && (content.starts_with("---") || content.starts_with("...")))
            {
                break;
            }
            last = content_end(line, body);
        }
        line = stop + 1;
    }
    last.max(end)
}

/// A flow collection's span covers only its opening `[` / `{`, and its
/// last item ends before the closing one: step over separators (an empty
/// value's `:`, an explicit `?`), white space and comments to include
/// the delimiter.
fn closing(text: &str, start: usize, end: usize) -> usize {
    let close = match text.as_bytes().get(start) {
        Some(b'[') => b']',
        Some(b'{') => b'}',
        _ => unreachable!("only flow collections are closed"),
    };
    let bytes = text.as_bytes();
    let mut at = end.max(start + 1);
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' | b',' | b':' | b'?' => at += 1,
            b'#' => at += text[at..].find(['\n', '\r']).unwrap_or(text.len() - at),
            byte if byte == close => return at + 1,
            _ => break,
        }
    }
    end
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
        // A map ending in a merge ends after it.
        assert_eq!(end("base: &b\n  x: 1\nd:\n  x: 2\n  <<: *b\n", "#.d"), 34);
        // Comments and blank lines after the last item are not part of it;
        // a sibling key at the sequence's own column ends it.
        assert_eq!(end("k:\n- 1 # one\n\n# note\nz: 2\n", "#.k"), 6);
        assert_eq!(end("- a: 1\n  b: 2\n- c\n", "#[0]"), 13);
        assert_eq!(end("a:\n  - x\n...\n", "#"), 8);
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
        // Blank lines only: their breaks are not in the text, so no offset.
        let map = SourceMap::parse("k: |+\n\n\nz: 1\n").unwrap();
        assert_eq!(map.locate("#.k").unwrap().start.line, 1);
        assert_eq!(map.locate_in("#.k", 0), None);
    }

    #[test]
    fn a_byte_order_mark_shifts_no_column() {
        let map = SourceMap::parse("\u{feff}a: 1\nb: 2\n").unwrap();
        assert_eq!(at(&map, "#.a"), (1, 4));
        assert_eq!(at(&map, "#.b"), (2, 4));
    }

    #[test]
    fn unreadable_text_reports_where() {
        let error = SourceMap::parse("a: [1,\n").unwrap_err();
        assert!(error.message().contains("line"), "{error}");
        let error = SourceMap::parse("a: 1\na: 2\n").unwrap_err();
        assert!(error.message().contains('a'), "{error}");
    }
}
