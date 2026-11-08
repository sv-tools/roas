//! The one place the CLI reads and writes YAML, so every subcommand reads a
//! document the same way the fetchers and `roas-codegen` do.

use serde_json::Value;

/// Read `raw` as YAML.
///
/// - YAML 1.2 booleans only: `yes` / `on` / `no` / `off` stay strings.
/// - `017` and `1_000` stay strings rather than becoming numbers.
/// - `.inf` / `.nan` pass through (a `serde_json::Value` holds them as `null`).
/// - `<<: *anchor` merge keys are expanded.
/// - A duplicate mapping key is an error rather than a silent last-wins.
///
/// The default [`serde_saphyr::Budget`] stays on: it rejects alias bombs and
/// pathologically deep or large documents before they are materialised.
pub(crate) fn from_str(raw: &str) -> Result<Value, serde_saphyr::Error> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    };
    serde_saphyr::from_str_with_options(raw, options)
}

/// Write `value` as YAML. Strings that a YAML 1.1 reader would take for
/// something else (`"3.1.0"`, `"n"`, `"yes"`) are quoted, and the
/// document ends with a newline.
pub(crate) fn to_string(value: &Value) -> Result<String, serde_saphyr::SerializeError> {
    serde_saphyr::to_string(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(yaml: &str) -> Result<Value, String> {
        from_str(yaml).map_err(|e| e.to_string())
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

    #[test]
    fn yaml_alias_bomb_is_rejected_by_the_budget() {
        let mut yaml = String::from("l0: &l0 [lol, lol, lol, lol, lol, lol, lol, lol, lol, lol]\n");
        for i in 1..10 {
            let prev = format!("*l{}", i - 1);
            yaml.push_str(&format!("l{i}: &l{i} [{}]\n", vec![prev; 10].join(", ")));
        }
        let err = parse(&yaml).unwrap_err();
        assert!(err.contains("budget breached"), "{err}");
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
