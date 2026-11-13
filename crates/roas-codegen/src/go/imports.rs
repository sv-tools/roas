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

/// The qualifiers in `text`, in order: for a substitution's spelling, the
/// first is the package of the type itself (`atomic` in
/// `atomic.Pointer[netip.Addr]`), the rest those of its arguments.
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

/// The name an `extra_imports` spec is used under: its alias, else what
/// its path ends in. `None` for a blank or dot import, which no qualifier
/// shows a use of.
pub(crate) fn spec_names(spec: &str) -> Option<Vec<String>> {
    let spec = spec.trim();
    let (alias, path) = match spec.find(['"', '`']) {
        Some(0) => (None, spec),
        Some(at) => (Some(spec[..at].trim()), &spec[at..]),
        None => (Some(spec), ""),
    };
    match alias {
        Some("_" | ".") => None,
        Some(alias) if !alias.is_empty() => Some(vec![alias.to_owned()]),
        _ => {
            // The path, up to its closing quote: a comment may follow.
            let quote = path.chars().next().unwrap_or('"');
            let path = path[quote.len_utf8()..]
                .split(quote)
                .next()
                .unwrap_or_default();
            let last = super::view::package_of(path);
            let mut names = vec![last.to_owned()];
            // `gopkg.in/yaml.v3` is package `yaml`.
            if let Some((stem, _)) = last.split_once('.') {
                names.push(stem.to_owned());
            }
            Some(names)
        }
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
    fn spec_names_read_the_alias_or_the_path() {
        assert_eq!(
            spec_names("\"github.com/google/uuid\""),
            Some(vec!["uuid".to_owned()])
        );
        assert_eq!(
            spec_names("\"gopkg.in/yaml.v3\""),
            Some(vec!["yaml.v3".to_owned(), "yaml".to_owned()])
        );
        assert_eq!(
            spec_names("y \"gopkg.in/yaml.v3\""),
            Some(vec!["y".to_owned()])
        );
        assert_eq!(spec_names("_ \"embed\""), None);
        assert_eq!(spec_names(". \"math\""), None);
        assert_eq!(
            spec_names("\"github.com/google/uuid\" // ids"),
            Some(vec!["uuid".to_owned()])
        );
    }
}
