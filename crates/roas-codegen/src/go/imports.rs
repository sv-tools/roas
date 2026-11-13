//! Which imports a Go file uses, for writing a file per type: Go refuses
//! an import a file does not use.

use std::collections::BTreeSet;

/// The identifiers `text` uses as package qualifiers: each one followed by
/// a `.` and not itself after one, outside comments and literals. Over a
/// rendered file, every package the file names; over a substitution's
/// spelling, the packages that spelling names.
pub(crate) fn qualifiers(text: &str) -> BTreeSet<String> {
    qualifier_list(text).into_iter().collect()
}

/// The qualifiers in `text`, in order.
pub(crate) fn qualifier_list(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // The last token was a `.`: the identifier after it is a selector.
    let mut after_dot = false;
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
            }
            quote @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote && bytes[i] != b'\n' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                after_dot = false;
            }
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    i += 1;
                }
                i += 1;
                after_dot = false;
            }
            b'.' => {
                // `...` is an ellipsis, not a selector.
                if bytes[i..].starts_with(b"...") {
                    i += 3;
                    after_dot = false;
                } else {
                    i += 1;
                    after_dot = true;
                }
            }
            b if b.is_ascii_digit() => {
                // A number, with its fraction, exponent and suffixes.
                while i < bytes.len() && (ident(bytes[i]) || bytes[i] == b'.') {
                    i += 1;
                }
                after_dot = false;
            }
            b if ident(b) => {
                let start = i;
                while i < bytes.len() && ident(bytes[i]) {
                    i += 1;
                }
                let mut next = i;
                while next < bytes.len() && matches!(bytes[next], b' ' | b'\t') {
                    next += 1;
                }
                if !after_dot
                    && bytes.get(next) == Some(&b'.')
                    && bytes.get(next + 1) != Some(&b'.')
                {
                    out.push(text[start..i].to_owned());
                }
                after_dot = false;
            }
            b if b.is_ascii_whitespace() => i += 1,
            _ => {
                i += 1;
                after_dot = false;
            }
        }
    }
    out
}

/// What an `extra_imports` spec is known to be used under, as the spec
/// says it: nothing is guessed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SpecName {
    /// A blank or dot import, which no qualifier shows a use of.
    Unnamed,
    /// Its alias, or its path's last element where that is a plain
    /// identifier and no major version.
    Named(String),
    /// The package name could differ from anything the path says
    /// (`gopkg.in/yaml.v3`, `k8s.io/api/core/v1`, `.../go-thing`).
    Uncertain,
}

/// The import path a spec names, up to its closing quote: a comment may
/// follow.
pub(crate) fn spec_path(spec: &str) -> &str {
    let spec = spec.trim();
    let Some(at) = spec.find(['"', '`']) else {
        return "";
    };
    let quote = &spec[at..at + 1];
    spec[at + 1..].split(quote).next().unwrap_or_default()
}

pub(crate) fn spec_name(spec: &str) -> SpecName {
    let spec = spec.trim();
    let alias = match spec.find(['"', '`']) {
        Some(at) => spec[..at].trim(),
        None => spec,
    };
    match alias {
        "_" | "." => SpecName::Unnamed,
        "" => {
            let last = spec_path(spec).rsplit('/').next().unwrap_or_default();
            let identifier = last
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && last.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && !super::naming::is_keyword(last);
            let version = last.len() > 1
                && last.starts_with('v')
                && last[1..].bytes().all(|b| b.is_ascii_digit());
            if identifier && !version {
                SpecName::Named(last.to_owned())
            } else {
                SpecName::Uncertain
            }
        }
        alias => SpecName::Named(alias.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualifiers_skip_selectors_comments_and_literals() {
        let text = "package x\nimport (\n\t\"fmt\"\n)\n// strings.Builder\n/* bytes.Buffer */\nvar a = fmt.Errorf(\"errors.New %d\", x.json.y, 1.5, `math.Pi`, '.', f(xs...))\nvar b = time .\n\tNow()\n";
        let found: Vec<_> = qualifiers(text).into_iter().collect();
        assert_eq!(found, ["fmt", "time", "x"]);
    }

    #[test]
    fn spec_name_reads_the_alias_or_a_plain_last_element() {
        let named = |n: &str| SpecName::Named(n.to_owned());
        assert_eq!(spec_name("\"github.com/google/uuid\""), named("uuid"));
        assert_eq!(
            spec_name("\"github.com/google/uuid\" // ids"),
            named("uuid")
        );
        assert_eq!(spec_name("y \"gopkg.in/yaml.v3\""), named("y"));
        assert_eq!(spec_name("_ \"embed\""), SpecName::Unnamed);
        assert_eq!(spec_name(". \"math\""), SpecName::Unnamed);
        for uncertain in [
            "\"gopkg.in/yaml.v3\"",
            "\"k8s.io/api/core/v1\"",
            "\"example.com/go-thing\"",
            "\"github.com/json-iterator/go\"",
        ] {
            assert_eq!(spec_name(uncertain), SpecName::Uncertain, "{uncertain}");
        }
        assert_eq!(spec_path("x `a/b` // c"), "a/b");
    }
}
