//! Rust identifiers: casing, reserved words, and collision-free
//! allocation. Names arrive from the IR as suggested parts; this is
//! where they become final.

use std::collections::BTreeSet;

/// Whether `c` is legal inside a Rust identifier (XID_Continue, ASCII fast path).
fn is_ident_continue(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric()
    } else {
        c.is_alphanumeric() && syn::parse_str::<syn::Ident>(&format!("_{c}")).is_ok()
    }
}

/// Whether `ident` starts with a character legal at the start of a Rust identifier.
fn starts_valid_ident(ident: &str) -> bool {
    ident.chars().next().is_some_and(|c| {
        if c.is_ascii() {
            c.is_ascii_alphabetic()
        } else {
            syn::parse_str::<syn::Ident>(ident).is_ok()
        }
    })
}

/// Split a raw name into lowercase words: on non-identifier characters, on
/// lower-to-upper boundaries, and at the end of an acronym
/// (`HTTPServer` → `http`, `server`).
pub(crate) fn words(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !is_ident_continue(c) {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let boundary = match prev {
            Some(p) if is_ident_continue(p) => {
                (c.is_uppercase() && p.is_lowercase())
                    || (c.is_uppercase()
                        && p.is_uppercase()
                        && next.is_some_and(char::is_lowercase))
                    || (c.is_numeric() != p.is_numeric())
            }
            _ => false,
        };
        if boundary && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `PascalCase` from name parts. Never empty, never starts with a non-XID_Start char.
pub(crate) fn pascal(parts: &[String]) -> String {
    let mut out: String = parts
        .iter()
        .flat_map(|p| words(p))
        .map(|w| capitalize(&w))
        .collect();
    if out.is_empty() {
        out = "Unnamed".to_owned();
    }
    if !starts_valid_ident(&out) {
        out.insert(0, 'N');
    }
    out
}

/// `snake_case` from name parts. Never empty, never starts with a non-XID_Start char.
pub(crate) fn snake(parts: &[String]) -> String {
    let mut out = parts
        .iter()
        .flat_map(|p| words(p))
        .collect::<Vec<_>>()
        .join("_");
    if out.is_empty() {
        out = "unnamed".to_owned();
    }
    if !starts_valid_ident(&out) {
        out.insert(0, 'n');
    }
    out
}

pub(crate) fn screaming_snake(parts: &[String]) -> String {
    snake(parts).to_uppercase()
}

pub(crate) fn camel(parts: &[String]) -> String {
    let pascal = pascal(parts);
    let mut chars = pascal.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => pascal,
    }
}

pub(crate) fn kebab(parts: &[String]) -> String {
    snake(parts).replace('_', "-")
}

/// Rust's strict and reserved keywords, 2024 edition.
const KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "gen", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "union", "raw", "safe",
];

/// Keywords that cannot be raw identifiers.
const NOT_RAW: &[&str] = &["self", "Self", "super", "crate"];

/// Whether `ident` is a Rust keyword, strict or reserved.
pub(crate) fn is_keyword(ident: &str) -> bool {
    KEYWORDS.contains(&ident)
}

/// Make an identifier legal: raw-prefix a keyword, suffix the few that
/// cannot be raw.
pub(crate) fn legal(ident: String) -> String {
    if NOT_RAW.contains(&ident.as_str()) {
        format!("{ident}_")
    } else if KEYWORDS.contains(&ident.as_str()) {
        format!("r#{ident}")
    } else {
        ident
    }
}

/// Hands out names that do not collide, deterministically: the first
/// claimant of a base keeps it, later ones get `2`, `3`, …
#[derive(Debug, Default)]
pub(crate) struct Allocator {
    taken: BTreeSet<String>,
}

impl Allocator {
    pub(crate) fn reserve(&mut self, name: &str) {
        self.taken.insert(name.to_owned());
    }

    pub(crate) fn allocate(&mut self, base: String) -> String {
        if self.taken.insert(base.clone()) {
            return base;
        }
        for i in 2u32.. {
            let candidate = format!("{base}{i}");
            if self.taken.insert(candidate.clone()) {
                return candidate;
            }
        }
        unreachable!("ran out of suffixes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        vec![s.to_owned()]
    }

    #[test]
    fn casing() {
        assert_eq!(pascal(&p("pet-status")), "PetStatus");
        assert_eq!(pascal(&p("pet_status")), "PetStatus");
        assert_eq!(pascal(&p("PetStatus")), "PetStatus");
        assert_eq!(pascal(&p("HTTPServer")), "HttpServer");
        assert_eq!(pascal(&p("x-request-id")), "XRequestId");
        assert_eq!(
            pascal(&["Pet".to_owned(), "status".to_owned()]),
            "PetStatus"
        );
        assert_eq!(pascal(&p("123abc")), "N123Abc");
        assert_eq!(pascal(&p("")), "Unnamed");
        assert_eq!(snake(&p("createdAt")), "created_at");
        assert_eq!(snake(&p("X-Request-Id")), "x_request_id");
        assert_eq!(snake(&p("ID")), "id");
        assert_eq!(snake(&p("user2fa")), "user_2_fa");
        assert_eq!(camel(&p("pet-status")), "petStatus");
        assert_eq!(kebab(&p("PetStatus")), "pet-status");
        assert_eq!(screaming_snake(&p("PetStatus")), "PET_STATUS");
    }

    #[test]
    fn keywords_are_made_legal() {
        assert_eq!(legal("type".into()), "r#type");
        assert_eq!(legal("match".into()), "r#match");
        assert_eq!(legal("union".into()), "r#union");
        assert_eq!(legal("self".into()), "self_");
        assert_eq!(legal("name".into()), "name");
    }

    #[test]
    fn allocation_is_deterministic() {
        let mut a = Allocator::default();
        a.reserve("Int64");
        assert_eq!(a.allocate("Pet".into()), "Pet");
        assert_eq!(a.allocate("Pet".into()), "Pet2");
        assert_eq!(a.allocate("Pet".into()), "Pet3");
        assert_eq!(a.allocate("Int64".into()), "Int642");
    }

    #[test]
    fn non_ascii_numerals_and_non_xid_chars_produce_valid_rust_idents() {
        for raw in ["x²", "½", "١", "foo١bar", "café", "éclair", "αβ", "m²/s"] {
            let p_name = legal(pascal(&p(raw)));
            let s_name = legal(snake(&p(raw)));
            assert!(
                syn::parse_str::<syn::Ident>(&p_name).is_ok(),
                "invalid pascal ident {p_name:?} for {raw:?}"
            );
            assert!(
                syn::parse_str::<syn::Ident>(&s_name).is_ok(),
                "invalid snake ident {s_name:?} for {raw:?}"
            );
        }
        assert_eq!(pascal(&p("x²")), "X");
        assert_eq!(pascal(&p("½")), "Unnamed");
        assert_eq!(pascal(&p("١")), "N١");
        assert_eq!(snake(&p("١")), "n١");
    }
}
