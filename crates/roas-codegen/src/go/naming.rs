//! Go identifiers and literals: exported casing, collision-free
//! allocation (shared with the Rust backend), and the quoting Go source
//! and struct tags need.

use crate::rust::naming::words;

/// An exported Go identifier from name parts: ASCII only, so it is a
/// legal identifier however the description spells it, and upper-case
/// led, so it is exported. Never empty.
pub(crate) fn exported(parts: &[String]) -> String {
    let mut out = String::new();
    for word in parts.iter().flat_map(|p| words(p)) {
        let word: String = word.chars().filter(char::is_ascii_alphanumeric).collect();
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    if out.is_empty() {
        out = "Unnamed".to_owned();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, 'N');
    }
    out
}

/// Go's keywords; an identifier the configuration supplies must not be
/// one.
const KEYWORDS: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

pub(crate) fn is_keyword(ident: &str) -> bool {
    KEYWORDS.contains(&ident)
}

/// An exported ASCII Go identifier, as a name from the configuration
/// must be.
pub(crate) fn is_exported_ident(ident: &str) -> bool {
    ident.starts_with(|c: char| c.is_ascii_uppercase())
        && ident
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A package name: lowercase ASCII, a letter first, not a keyword, and
/// one another package can import (`main` and `init` cannot be).
pub(crate) fn is_package_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !is_keyword(name)
        && name != "main"
        && name != "init"
        // `go build` skips a file whose package is `documentation`.
        && name != "documentation"
}

/// A module path as `go.mod` takes one, by `go mod init`'s rules for an
/// import path: `/`-separated elements of ASCII letters, digits, `-`, `.`,
/// `_` and `~`, none empty, `.` or `..`, none starting or ending with a
/// dot, and none a name Windows reserves.
pub(crate) fn is_module_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|element| {
            !element.is_empty()
                && !element.starts_with('.')
                && !element.ends_with('.')
                && element
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                && crate::write::windows_safe(element.split('.').next().unwrap_or(element))
        })
}

/// Operating systems and architectures `go build` knows, from
/// `internal/syslist`: a file named `x_<os>.go` or `x_<arch>.go` builds
/// only there.
const KNOWN_OS: &[&str] = &[
    "aix",
    "android",
    "darwin",
    "dragonfly",
    "freebsd",
    "hurd",
    "illumos",
    "ios",
    "js",
    "linux",
    "nacl",
    "netbsd",
    "openbsd",
    "plan9",
    "solaris",
    "wasip1",
    "windows",
    "zos",
];
const KNOWN_ARCH: &[&str] = &[
    "386",
    "amd64",
    "amd64p32",
    "arm",
    "armbe",
    "arm64",
    "arm64be",
    "loong64",
    "mips",
    "mipsle",
    "mips64",
    "mips64le",
    "mips64p32",
    "mips64p32le",
    "ppc",
    "ppc64",
    "ppc64le",
    "riscv",
    "riscv64",
    "s390",
    "s390x",
    "sparc",
    "sparc64",
    "wasm",
];

/// Whether `go build` would read something into a file named
/// `<stem>.go` beyond its contents: a `_test` file is built only by
/// `go test`, and an `_<os>` or `_<arch>` suffix restricts the platforms.
fn constrained(stem: &str) -> bool {
    let last = stem.rsplit('_').next().unwrap_or(stem);
    stem.contains('_') && (last == "test" || KNOWN_OS.contains(&last) || KNOWN_ARCH.contains(&last))
}

/// A file stem `go build` reads as plain source: lowercase ASCII, a
/// letter first, no build-constraint suffix, nothing Windows resolves
/// elsewhere.
pub(crate) fn is_file_stem(stem: &str) -> bool {
    stem.starts_with(|c: char| c.is_ascii_lowercase())
        && stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && !constrained(stem)
        && crate::write::windows_safe(stem)
}

/// A file stem derived from free text such as a label: lowercase, other
/// characters to `_`, never empty, never digit-led, and never one `go
/// build` would constrain.
pub fn go_file_name(text: &str) -> String {
    let mut stem: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while stem.contains("__") {
        stem = stem.replace("__", "_");
    }
    let stem = stem.trim_matches('_').to_owned();
    let stem = match stem.chars().next() {
        None => "api".to_owned(),
        Some(c) if c.is_ascii_digit() => format!("m_{stem}"),
        Some(_) => stem,
    };
    if is_file_stem(&stem) {
        stem
    } else {
        format!("{stem}_types")
    }
}

/// A Go interpreted string literal for `text`, which may hold anything.
pub(crate) fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() || is_invisible(c) => {
                if (c as u32) < 0x10000 {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                } else {
                    out.push_str(&format!("\\U{:08x}", c as u32));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Characters that reorder or hide text: escaped in literals and
/// comments, so the source reads as it compiles.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// One comment line's text, with anything that would end the line or
/// hide what it says escaped.
pub(crate) fn comment_text(text: &str) -> String {
    // `gofmt` reads a `// +build ...` line anywhere as a build constraint
    // and moves it to the top, where it can exclude the whole file; the
    // escaped `+` reads the same and constrains nothing.
    let trimmed = text.trim_start();
    let constraint = trimmed
        .strip_prefix("+build")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
    let text = if constraint {
        format!(
            "{}\\u{{2b}}{}",
            &text[..text.len() - trimmed.len()],
            &trimmed[1..]
        )
    } else {
        text.to_owned()
    };
    text.chars()
        .flat_map(|c| {
            if (c.is_control() && c != '\t') || is_invisible(c) {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

/// Whether `encoding/json/v2` can carry `name` in a struct tag: the
/// stdlib reserves `,`, `\`, `'`, `"` and the backtick in a tag name,
/// takes an empty name as "use the field name", and `-` alone as "skip
/// this field" (`-,` names it).
pub(crate) fn tag_name_ok(name: &str) -> bool {
    !name.is_empty() && !name.contains([',', '\\', '\'', '"', '`'])
}

/// A struct-tag key as `reflect.StructTag` parses one: non-empty, no
/// space, quote, colon or control character.
pub(crate) fn is_tag_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_graphic() && c != '"' && c != ':' && c != '`')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        vec![s.to_owned()]
    }

    #[test]
    fn exported_names() {
        assert_eq!(exported(&p("pet-status")), "PetStatus");
        assert_eq!(exported(&p("HTTPServer")), "HttpServer");
        assert_eq!(exported(&p("123abc")), "N123Abc");
        assert_eq!(exported(&p("имя")), "Unnamed");
        assert_eq!(exported(&p("café")), "Caf");
        assert_eq!(exported(&p("type")), "Type");
    }

    #[test]
    fn file_stems_avoid_build_constraints() {
        assert_eq!(go_file_name("pet-store"), "pet_store");
        assert_eq!(go_file_name("api_linux"), "api_linux_types");
        assert_eq!(go_file_name("x-test"), "x_test_types");
        assert_eq!(go_file_name("386"), "m_386_types");
        assert_eq!(go_file_name("linux"), "linux");
        assert_eq!(go_file_name("--"), "api");
        assert_eq!(go_file_name("con"), "con_types");
        assert!(!is_file_stem("api_amd64"));
        assert!(is_file_stem("types"));
    }

    #[test]
    fn module_paths() {
        assert!(is_module_path("example.com/pets"));
        assert!(is_module_path("pets"));
        assert!(is_module_path("github.com/a-b/c_d/v2"));
        assert!(!is_module_path(""));
        assert!(!is_module_path("/pets"));
        assert!(!is_module_path("pets/"));
        assert!(!is_module_path("a//b"));
        assert!(!is_module_path("a/../b"));
        assert!(!is_module_path(".hidden/x"));
        assert!(!is_module_path("a b"));
        assert!(!is_module_path("example.com/con"));
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(quote("\u{202E}"), "\"\\u202e\"");
        assert_eq!(quote("\u{1}"), "\"\\u0001\"");
        assert_eq!(comment_text("a\u{0}b"), "a\\u{0}b");
        assert_eq!(comment_text("+build ignore"), "\\u{2b}build ignore");
        assert_eq!(comment_text("  +build"), "  \\u{2b}build");
        assert_eq!(comment_text("+builder"), "+builder");
    }

    #[test]
    fn tag_names() {
        assert!(tag_name_ok("name"));
        assert!(tag_name_ok("-"));
        assert!(tag_name_ok("x y"));
        assert!(!tag_name_ok(""));
        assert!(!tag_name_ok("a,b"));
        assert!(!tag_name_ok("a\"b"));
        assert!(is_tag_key("db"));
        assert!(!is_tag_key("a b"));
        assert!(!is_tag_key("a:b"));
    }
}
