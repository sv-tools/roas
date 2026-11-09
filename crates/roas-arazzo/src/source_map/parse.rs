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
    /// reverse) simply has no map.
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
        let root = node(&text, root);
        Ok(Self::new(text, root))
    }
}

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

fn node(text: &str, spanned: Spanned<Raw>) -> Node {
    let (start, mut end) = span(text, &spanned);
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
                    let value = node(text, value);
                    end = end.max(value.end);
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
                    let item = node(text, item);
                    end = end.max(item.end);
                    item
                })
                .collect(),
        ),
        Raw::String(value) => Kind::Scalar(Some(value)),
        Raw::Other => Kind::Scalar(None),
    };
    Node { start, end, kind }
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
    fn unreadable_text_reports_where() {
        let error = SourceMap::parse("a: [1,\n").unwrap_err();
        assert!(error.message().contains("line"), "{error}");
        let error = SourceMap::parse("a: 1\na: 2\n").unwrap_err();
        assert!(error.message().contains('a'), "{error}");
    }
}
