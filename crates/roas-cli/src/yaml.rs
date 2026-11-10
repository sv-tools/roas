//! The one place the CLI reads and writes YAML, so every subcommand reads a
//! document the same way the fetchers and `roas-codegen` do.

use serde::ser::Error as _;
use serde::{Serialize, Serializer};
use serde_json::{Number, Value};

/// Read `raw` as YAML.
///
/// - YAML 1.2 booleans only: `yes` / `on` / `no` / `off` stay strings.
/// - Integer spellings `017` and `1_000` stay strings rather than becoming numbers.
/// - `!!binary` scalars are read as their base64 text, not decoded.
/// - `.inf` / `.nan` pass through (a `serde_json::Value` holds them as `null`).
/// - `<<: *anchor` merge keys are expanded.
/// - A duplicate mapping key is an error rather than a silent last-wins.
///
/// A [`serde_saphyr::Budget`] stays on, raised so large specs fit: it rejects
/// alias bombs, nesting past 64 levels, documents past 10M nodes or
/// 64 MiB of scalar text, and an anchored node past 1M events.
pub(crate) fn from_str(raw: &str) -> Result<Value, serde_saphyr::Error> {
    let options = serde_saphyr::options! {
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
    };
    serde_saphyr::from_str_with_options::<crate::yaml_value::JsonValue>(raw, options)
        .map(crate::yaml_value::JsonValue::into_inner)
}

/// Write `value` as YAML. Strings that a YAML 1.1 reader would take for
/// something else (`"3.1.0"`, `"n"`, `"yes"`) are quoted, and the
/// document ends with a newline.
///
/// Each number is written as a plain YAML number, also when
/// `serde_json/arbitrary_precision` is on (the `exact-numbers` feature),
/// under which a `Value` number would otherwise serialize as a
/// `$serde_json::private::Number` mapping. An integer is written exactly;
/// any other number goes through an `f64`, as it does without the feature,
/// and one no `f64` can hold (`1e400`) is an error.
pub(crate) fn to_string(value: &Value) -> Result<String, serde_saphyr::SerializeError> {
    serde_saphyr::to_string(&PlainNumbers(value))
}

/// Write a typed `value` as YAML, with the quoting of [`to_string`]. Only
/// for types that hold no `serde_json::Value`, whose numbers would be
/// written as mappings under `arbitrary_precision`; a document goes
/// through [`to_string`].
pub(crate) fn typed_to_string<T: Serialize>(
    value: &T,
) -> Result<String, serde_saphyr::SerializeError> {
    serde_saphyr::to_string(value)
}

/// A `Value` whose numbers serialize as numbers, never as the private
/// mapping `arbitrary_precision` uses to carry a number's literal.
struct PlainNumbers<'a>(&'a Value);

impl Serialize for PlainNumbers<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Number(number) => serialize_number(number, serializer),
            Value::Array(items) => serializer.collect_seq(items.iter().map(PlainNumbers)),
            Value::Object(members) => serializer.collect_map(
                members
                    .iter()
                    .map(|(name, member)| (name, PlainNumbers(member))),
            ),
            other => other.serialize(serializer),
        }
    }
}

fn serialize_number<S: Serializer>(number: &Number, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(n) = number.as_u64() {
        return serializer.serialize_u64(n);
    }
    if let Some(n) = number.as_i64() {
        return serializer.serialize_i64(n);
    }
    // Only a literal kept by `arbitrary_precision` can be an integer
    // outside `i64`/`u64`.
    let literal = number.to_string();
    if let Ok(n) = literal.parse::<u128>() {
        return serializer.serialize_u128(n);
    }
    if let Ok(n) = literal.parse::<i128>() {
        return serializer.serialize_i128(n);
    }
    match number.as_f64() {
        Some(n) => serializer.serialize_f64(n),
        None => Err(S::Error::custom(format!(
            "the number {literal} is out of range for YAML output"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(yaml: &str) -> Result<Value, String> {
        from_str(yaml).map_err(|e| e.to_string())
    }

    fn write(json: &str) -> String {
        to_string(&serde_json::from_str(json).unwrap()).unwrap()
    }

    /// Without `arbitrary_precision` a `Value` number serializes natively;
    /// with it, as a `$serde_json::private::Number` mapping, which the
    /// writer must not let through.
    #[test]
    fn yaml_numbers_are_written_as_numbers() {
        let json = r#"[7, -7, 1.5, 1e2, [0, {"x": -0.25}]]"#;
        let yaml = write(json);
        assert_eq!(yaml, "- 7\n- -7\n- 1.5\n- 100.0\n- - 0\n  - x: -0.25\n");
        // What is written reads back as the same numbers.
        assert_eq!(
            from_str(&yaml).unwrap(),
            json!([7, -7, 1.5, 100.0, [0, { "x": -0.25 }]])
        );
    }

    /// An integer past `u64` keeps every digit only when the literal is
    /// kept; otherwise it was rounded to an `f64` when read.
    #[test]
    fn yaml_wide_integers_are_written_exactly_when_kept() {
        let yaml = write(
            r#"{"big": 123456789012345678901234567890, "neg": -123456789012345678901234567890}"#,
        );
        assert!(!yaml.contains("serde_json"), "{yaml}");
        if cfg!(feature = "exact-numbers") {
            assert_eq!(
                yaml,
                "big: 123456789012345678901234567890\nneg: -123456789012345678901234567890\n"
            );
        }
    }

    #[cfg(feature = "exact-numbers")]
    #[test]
    fn yaml_number_no_f64_holds_is_an_error() {
        let value: Value = serde_json::from_str(r#"{"n": 1e400}"#).unwrap();
        let err = to_string(&value).unwrap_err().to_string();
        assert!(err.contains("is out of range for YAML output"), "{err}");
    }

    /// Each scalar whose meaning depends on the YAML options, next to what
    /// it must read as.
    #[test]
    fn yaml_scalars_read_as_yaml_1_2() {
        for (scalar, expected) in [
            ("yes", json!("yes")),
            ("on", json!("on")),
            ("no", json!("no")),
            ("off", json!("off")),
            ("true", json!(true)),
            ("False", json!(false)),
            ("017", json!("017")),
            ("1_000", json!("1_000")),
            ("17", json!(17)),
            ("0x11", json!(17)),
            ("1.5", json!(1.5)),
            (".inf", json!(null)),
            ("3.1.0", json!("3.1.0")),
            ("!!binary SGVsbG8=", json!("SGVsbG8=")),
            ("!!binary /wAB", json!("/wAB")),
            ("1e400", json!(null)),
            ("!include other.yaml", json!("other.yaml")),
        ] {
            assert_eq!(
                parse(&format!("v: {scalar}\n")).unwrap(),
                json!({ "v": expected }),
                "{scalar}",
            );
        }
    }

    #[test]
    fn yaml_merge_keys_expand_through_an_anchor() {
        let yaml = "base: &base { type: string, minLength: 1 }\n\
                    name:\n  <<: *base\n  minLength: 3\n";
        assert_eq!(
            parse(yaml).unwrap(),
            json!({
                "base": { "type": "string", "minLength": 1 },
                "name": { "type": "string", "minLength": 3 },
            }),
        );
    }

    #[test]
    fn yaml_duplicate_key_is_an_error() {
        let err = parse("a: 1\na: 2\n").unwrap_err();
        assert!(err.contains("duplicate mapping key"), "{err}");
    }

    /// `200` and `"200"` are different YAML keys but the same JSON one.
    /// Through a `<<` merge the inherited entry would otherwise overwrite the
    /// explicit one, in either direction.
    #[test]
    fn yaml_keys_colliding_as_json_are_an_error() {
        for yaml in [
            "base: &b { 200: { description: inherited } }\n\
             responses:\n  <<: *b\n  \"200\": { description: explicit }\n",
            "base: &b { \"200\": { description: inherited } }\n\
             responses:\n  <<: *b\n  200: { description: explicit }\n",
            "responses:\n  200: a\n  \"200\": b\n",
        ] {
            let err = parse(yaml).unwrap_err();
            assert!(err.contains("appears twice once read as JSON"), "{err}");
        }
    }

    #[test]
    fn yaml_merge_keeps_the_explicit_entry_when_keys_match() {
        let yaml = "base: &b { 200: { description: inherited }, 404: { description: nf } }\n\
                    responses:\n  <<: *b\n  200: { description: explicit }\n";
        assert_eq!(
            parse(yaml).unwrap()["responses"],
            json!({ "200": { "description": "explicit" }, "404": { "description": "nf" } }),
        );
    }

    /// Keys are read as strings, so a number-looking key keeps its spelling
    /// and two spellings of one number are two keys.
    #[test]
    fn yaml_keys_keep_their_spelling() {
        assert_eq!(
            parse("r: { 1.10: a, 1.1: b, 0x10: c, 1e3: d, True: e, 017: f }\n").unwrap(),
            json!({ "r": { "1.10": "a", "1.1": "b", "0x10": "c", "1e3": "d", "True": "e", "017": "f" } }),
        );
    }

    #[test]
    fn yaml_null_and_collection_keys_are_an_error() {
        for yaml in [
            "r: { null: a }\n",
            "r: { ~: a }\n",
            "r:\n  ? [1, 2]\n  : a\n",
        ] {
            parse(yaml).unwrap_err();
        }
    }

    /// The number map YAML output carries under `arbitrary_precision` reads
    /// back the way `serde_json` reads it, with the feature on or off.
    #[test]
    fn yaml_arbitrary_precision_number_map_matches_serde_json() {
        let json = r#"{"n":{"$serde_json::private::Number":"12.50"}}"#;
        let expected: Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            parse("n: { \"$serde_json::private::Number\": \"12.50\" }\n").unwrap(),
            expected
        );
    }

    #[test]
    fn yaml_alias_bomb_is_rejected_by_the_budget() {
        // Billion laughs: each level aliases the previous one ten times, so
        // full expansion is 10^7 scalars. The anchor-replay limits stop it.
        let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..7 {
            let prev = format!("*l{}", i - 1);
            yaml.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 10].join(", ")));
        }
        let err = parse(&yaml).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// One shared anchor reused many times, as specs do for common
    /// responses, is not a bomb.
    #[test]
    fn yaml_anchor_reused_many_times_is_accepted() {
        // 40 properties aliased 6,000 times replays over a million events.
        let props: String = (0..40)
            .map(|i| format!("    p{i}: {{ type: string }}\n"))
            .collect();
        let mut yaml = format!("schema: &s\n  type: object\n  properties:\n{props}responses:\n");
        for i in 0..6_000 {
            yaml.push_str(&format!("  r{i}: *s\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(
            value["responses"]["r5999"]["properties"]["p39"]["type"],
            "string"
        );
    }

    /// Past the library's default caps of 50k aliases and 50k anchors,
    /// which generated specs (one anchor per shared object) can reach.
    #[test]
    fn yaml_many_aliases_and_anchors_fit_the_budget() {
        let mut yaml = String::from("shared: &s x\n");
        for i in 0..60_000 {
            yaml.push_str(&format!("a{i}: *s\nb{i}: &b{i} y\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(value["a59999"], "x");
        assert_eq!(value["b59999"], "y");
    }

    /// Past the library's default cap of 10k merge keys, which only applies
    /// because merge keys are expanded.
    #[test]
    fn yaml_many_merge_keys_fit_the_budget() {
        let mut yaml = String::from("base: &b { type: string }\n");
        for i in 0..10_001 {
            yaml.push_str(&format!("s{i}: {{ <<: *b, minLength: 1 }}\n"));
        }
        let value = parse(&yaml).unwrap();
        assert_eq!(value["s10000"]["type"], "string");
    }

    #[test]
    fn yaml_nesting_is_capped_at_64_levels() {
        let nested = |depth: usize| format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        parse(&nested(64)).unwrap();
        let err = parse(&nested(65)).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
    }

    /// Past the library's default budget of 250k nodes, which rejects
    /// ordinary large specs.
    #[test]
    fn yaml_large_document_fits_the_budget() {
        let yaml: String = (0..60_000).map(|i| format!("k{i}: [a, b, c]\n")).collect();
        let value = parse(&yaml).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 60_000);
    }

    #[test]
    fn yaml_output_quotes_what_yaml_1_1_would_misread_and_round_trips() {
        let value = json!({
            "openapi": "3.1.0",
            "a": "n",
            "b": "yes",
            "c": "017",
            "d": "1_000",
            "e": "",
            "f": "null",
            "g": 12,
            "h": 1.5,
            "i": true,
            "j": "multi\nline\n",
        });
        let out = to_string(&value).unwrap();
        for quoted in [
            "openapi: \"3.1.0\"",
            "a: \"n\"",
            "b: \"yes\"",
            "c: \"017\"",
            "d: \"1_000\"",
        ] {
            assert!(out.contains(quoted), "{quoted} in:\n{out}");
        }
        assert!(out.ends_with('\n'), "{out}");
        assert_eq!(from_str(&out).unwrap(), value);
    }
}
