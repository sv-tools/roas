//! A schema's `pattern`, read as the ECMAScript regular expression JSON
//! Schema names and printed as one that Rust's `regex` and Go's `regexp`
//! read the same way.
//!
//! The two engines disagree on far more than they fail to compile: `\d`
//! is Unicode in Rust and ASCII in Go, `.` excludes different line
//! terminators, and so on. So nothing is passed through as written. Every
//! character set — a shorthand, a class, `.`, a single character — is
//! computed as the code points ECMAScript gives it and printed as explicit
//! ranges, and what cannot be made to mean the same everywhere (lookaround,
//! backreferences, `\b`, Unicode properties, inline flags, counts Go will
//! not compile) is refused with a reason, never approximated.

use oxc_allocator::Allocator;
use oxc_regular_expression::ast::{
    Alternative, BoundaryAssertionKind, CharacterClass, CharacterClassContents,
    CharacterClassContentsKind, CharacterClassEscapeKind, Disjunction, Term,
};
use oxc_regular_expression::{LiteralParser, Options};

/// The largest repetition count Go compiles.
const MAX_COUNT: u64 = 1000;
const MAX_CHAR: u32 = 0x10FFFF;
const SURROGATES: (u32, u32) = (0xD800, 0xDFFF);
const LEGACY: &str =
    "it is valid only without the Unicode flag, where part of it can match half of a character";

/// `pattern` as a regex both targets read alike, or why it cannot be one.
pub(crate) fn translate(pattern: &str) -> Result<String, String> {
    let allocator = Allocator::default();
    // Unicode mode, as JSON Schema recommends: an astral character is one
    // character, as it is to Rust and Go.
    let (ast, legacy) =
        match LiteralParser::new(&allocator, pattern, Some("u"), Options::default()).parse() {
            Ok(ast) => (ast, false),
            // Many schemas are written for the legacy grammar (`\-` outside a
            // class, say). It means the same as long as nothing in it can
            // match half of an astral character, which `Printer` checks.
            Err(unicode) => {
                match LiteralParser::new(&allocator, pattern, None, Options::default()).parse() {
                    Ok(ast) => (ast, true),
                    Err(_) => {
                        return Err(format!(
                            "not an ECMAScript regular expression: {}",
                            unicode
                                .message
                                .trim_start_matches("Invalid regular expression: ")
                        ));
                    }
                }
            }
        };
    let mut printer = Printer {
        legacy,
        out: String::new(),
    };
    printer.disjunction(&ast.body)?;
    // The printer's output is meant to compile; this is the proof.
    regex_syntax::Parser::new()
        .parse(&printer.out)
        .map_err(|e| format!("its translation does not compile: {e}"))?;
    Ok(printer.out)
}

struct Printer {
    /// Parsed without the Unicode flag: a code unit is a character, so a
    /// set touching the surrogates could match half an astral character.
    legacy: bool,
    out: String,
}

/// A set of code points: sorted, disjoint, non-adjacent inclusive ranges.
type Set = Vec<(u32, u32)>;

impl Printer {
    fn disjunction(&mut self, d: &Disjunction<'_>) -> Result<(), String> {
        for (i, alternative) in d.body.iter().enumerate() {
            if i > 0 {
                self.out.push('|');
            }
            self.alternative(alternative)?;
        }
        Ok(())
    }

    fn alternative(&mut self, a: &Alternative<'_>) -> Result<(), String> {
        a.body.iter().try_for_each(|term| self.term(term))
    }

    fn group(&mut self, d: &Disjunction<'_>) -> Result<(), String> {
        self.out.push_str("(?:");
        self.disjunction(d)?;
        self.out.push(')');
        Ok(())
    }

    fn term(&mut self, term: &Term<'_>) -> Result<(), String> {
        match term {
            Term::BoundaryAssertion(b) => match b.kind {
                BoundaryAssertionKind::Start => self.out.push('^'),
                BoundaryAssertionKind::End => self.out.push('$'),
                BoundaryAssertionKind::Boundary | BoundaryAssertionKind::NegativeBoundary => {
                    return Err("word boundaries (`\\b`, `\\B`) are not supported".into());
                }
            },
            Term::LookAroundAssertion(_) => {
                return Err("lookaround is not supported".into());
            }
            Term::Quantifier(q) => {
                if q.min > MAX_COUNT || q.max.is_some_and(|max| max > MAX_COUNT) {
                    return Err(format!(
                        "a repetition count above {MAX_COUNT} is not supported"
                    ));
                }
                self.term(&q.body)?;
                // Laziness changes which match is found, never whether one
                // is: a check only asks the latter.
                match (q.min, q.max) {
                    (0, None) => self.out.push('*'),
                    (1, None) => self.out.push('+'),
                    (0, Some(1)) => self.out.push('?'),
                    (min, None) => self.out.push_str(&format!("{{{min},}}")),
                    (min, Some(max)) if min == max => self.out.push_str(&format!("{{{min}}}")),
                    (min, Some(max)) => self.out.push_str(&format!("{{{min},{max}}}")),
                }
            }
            Term::Character(c) => {
                self.explicit(c.value, c.value)?;
                self.set(vec![(c.value, c.value)])?;
            }
            Term::Dot(_) => self.set(complement(&normalize(vec![
                (0x0A, 0x0A),
                (0x0D, 0x0D),
                (0x2028, 0x2029),
            ])))?,
            Term::CharacterClassEscape(e) => self.set(escape(e.kind))?,
            Term::UnicodePropertyEscape(_) => {
                return Err("Unicode property escapes are not supported".into());
            }
            Term::CharacterClass(class) => {
                let set = self.class(class)?;
                self.set(set)?;
            }
            Term::CapturingGroup(g) => self.group(&g.body)?,
            Term::IgnoreGroup(g) => {
                if g.modifiers.is_some() {
                    return Err("inline flags are not supported".into());
                }
                self.group(&g.body)?;
            }
            Term::IndexedReference(_) | Term::NamedReference(_) => {
                return Err("backreferences are not supported".into());
            }
        }
        Ok(())
    }

    /// A range the pattern spells out: a surrogate there matches only a
    /// lone surrogate, which a decoded string never holds as one.
    fn explicit(&self, lo: u32, hi: u32) -> Result<(), String> {
        if lo <= SURROGATES.1 && hi >= SURROGATES.0 {
            return Err(if self.legacy {
                LEGACY.into()
            } else {
                "surrogate code points are not supported".into()
            });
        }
        Ok(())
    }

    fn class(&self, class: &CharacterClass<'_>) -> Result<Set, String> {
        if class.kind != CharacterClassContentsKind::Union {
            return Err("class set operations are not supported".into());
        }
        let mut set = Vec::new();
        for item in &class.body {
            match item {
                CharacterClassContents::CharacterClassRange(r) => {
                    self.explicit(r.min.value, r.max.value)?;
                    set.push((r.min.value, r.max.value));
                }
                CharacterClassContents::CharacterClassEscape(e) => set.extend(escape(e.kind)),
                CharacterClassContents::Character(c) => {
                    self.explicit(c.value, c.value)?;
                    set.push((c.value, c.value));
                }
                CharacterClassContents::UnicodePropertyEscape(_) => {
                    return Err("Unicode property escapes are not supported".into());
                }
                CharacterClassContents::NestedCharacterClass(_)
                | CharacterClassContents::ClassStringDisjunction(_) => {
                    return Err("class set operations are not supported".into());
                }
            }
        }
        let set = normalize(set);
        Ok(if class.negative {
            complement(&set)
        } else {
            set
        })
    }

    /// One character from `set`, printed as explicit ranges.
    fn set(&mut self, set: Set) -> Result<(), String> {
        if self.legacy && overlaps(&set, SURROGATES) {
            return Err(LEGACY.into());
        }
        let set = without_surrogates(&set);
        match set.as_slice() {
            // Matches nothing: an empty class, or one of surrogates only.
            [] => self.out.push_str("[^\\x{0}-\\x{10FFFF}]"),
            [(lo, hi)] if lo == hi => push_char(&mut self.out, *lo, false),
            ranges => {
                self.out.push('[');
                for &(lo, hi) in ranges {
                    push_char(&mut self.out, lo, true);
                    if hi > lo {
                        if hi > lo + 1 {
                            self.out.push('-');
                        }
                        push_char(&mut self.out, hi, true);
                    }
                }
                self.out.push(']');
            }
        }
        Ok(())
    }
}

/// What a shorthand class means in ECMAScript, which is ASCII for `\d`
/// and `\w` but the full whitespace and line-terminator set for `\s`.
fn escape(kind: CharacterClassEscapeKind) -> Set {
    let digits = vec![(0x30, 0x39)];
    let word = normalize(vec![(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)]);
    // WhiteSpace and LineTerminator: tab, LF, VT, FF, CR, space, NBSP,
    // the `Zs` category, LS, PS and the BOM.
    let space = normalize(vec![
        (0x09, 0x0D),
        (0x20, 0x20),
        (0xA0, 0xA0),
        (0x1680, 0x1680),
        (0x2000, 0x200A),
        (0x2028, 0x2029),
        (0x202F, 0x202F),
        (0x205F, 0x205F),
        (0x3000, 0x3000),
        (0xFEFF, 0xFEFF),
    ]);
    match kind {
        CharacterClassEscapeKind::D => digits,
        CharacterClassEscapeKind::NegativeD => complement(&digits),
        CharacterClassEscapeKind::W => word,
        CharacterClassEscapeKind::NegativeW => complement(&word),
        CharacterClassEscapeKind::S => space,
        CharacterClassEscapeKind::NegativeS => complement(&space),
    }
}

fn normalize(mut set: Set) -> Set {
    set.sort_unstable();
    let mut out: Set = Vec::with_capacity(set.len());
    for (lo, hi) in set {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

fn complement(set: &Set) -> Set {
    let mut out = Vec::new();
    let mut next = 0u32;
    for &(lo, hi) in set {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi.saturating_add(1);
    }
    if next <= MAX_CHAR {
        out.push((next, MAX_CHAR));
    }
    out
}

fn overlaps(set: &Set, (lo, hi): (u32, u32)) -> bool {
    set.iter().any(|&(a, b)| a <= hi && b >= lo)
}

fn without_surrogates(set: &Set) -> Set {
    let (s_lo, s_hi) = SURROGATES;
    let mut out = Vec::new();
    for &(lo, hi) in set {
        if lo < s_lo {
            out.push((lo, hi.min(s_lo - 1)));
        }
        if hi > s_hi {
            out.push((lo.max(s_hi + 1), hi));
        }
    }
    out
}

/// One character, spelled the same for both engines: ASCII letters, digits
/// and other plain symbols as themselves, a metacharacter escaped with a
/// backslash, and anything else (control, space, non-ASCII) as `\x{…}`,
/// the one hex escape both accept. `\u` would not compile in Go.
fn push_char(out: &mut String, c: u32, in_class: bool) {
    let meta: &[u8] = if in_class {
        // `&`, `~` and `-` start Rust's class operators.
        b"\\[]^-&~"
    } else {
        b"\\.+*?()[]{}|^$"
    };
    match u8::try_from(c) {
        Ok(b) if meta.contains(&b) => {
            out.push('\\');
            out.push(char::from(b));
        }
        // `#` starts a comment under Rust's `x` flag, never set here, but
        // spelling it as an escape costs nothing.
        Ok(b) if b.is_ascii_graphic() && b != b'#' => out.push(char::from(b)),
        _ => out.push_str(&format!("\\x{{{c:X}}}")),
    }
}

#[cfg(test)]
mod tests {
    use super::translate;

    fn ok(pattern: &str) -> String {
        translate(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"))
    }

    #[test]
    fn shorthands_are_spelled_out() {
        assert_eq!(ok(r"^\d+$"), "^[0-9]+$");
        assert_eq!(ok(r"\w"), "[0-9A-Z_a-z]");
        assert_eq!(ok(r"[\d-]"), r"[\-0-9]");
        assert_eq!(
            ok(r"\s"),
            r"[\x{9}-\x{D}\x{20}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]"
        );
        assert_eq!(ok(r"\D"), r"[\x{0}-/:-\x{D7FF}\x{E000}-\x{10FFFF}]");
        assert_eq!(
            ok("."),
            r"[\x{0}-\x{9}\x{B}\x{C}\x{E}-\x{2027}\x{202A}-\x{D7FF}\x{E000}-\x{10FFFF}]"
        );
    }

    #[test]
    fn ecmascript_classes_are_plain() {
        // Rust would read `&&` and `--` as class operators.
        assert_eq!(ok("[a&&b]"), r"[\&ab]");
        assert_eq!(ok("[+--]"), r"[+-\-]");
        assert_eq!(ok("[]"), r"[^\x{0}-\x{10FFFF}]");
        assert_eq!(ok("[^]"), r"[\x{0}-\x{D7FF}\x{E000}-\x{10FFFF}]");
    }

    #[test]
    fn structure_is_kept() {
        assert_eq!(ok("^(a|bc)*?x{2,5}$"), "^(?:a|bc)*x{2,5}$");
        assert_eq!(ok("(?<year>[0-9]{4})-(?:x)"), "(?:[0-9]{4})-(?:x)");
        assert_eq!(ok(""), "(?:)");
        assert_eq!(ok("a{1000}"), "a{1000}");
        assert!(ok(r"^https?://\S+$").starts_with("^https?://["));
        assert_eq!(ok("😀+"), r"\x{1F600}+");
        assert_eq!(ok(r"\u{1F600}"), r"\x{1F600}");
        assert_eq!(ok(r"a\.b\$"), r"a\.b\$");
        assert_eq!(ok("é"), r"\x{E9}");
    }

    #[test]
    fn legacy_grammar_is_read_when_it_means_the_same() {
        // `\-` outside a class is legacy-only syntax.
        assert_eq!(ok(r"^\d{3}\-\d{4}$"), "^[0-9]{3}-[0-9]{4}$");
        // A legacy `.` could match half of an astral character.
        assert!(translate(r"\-.").is_err());
        assert!(translate(r"\-[^a]").is_err());
        assert!(translate(r"\-\S").is_err());
        assert!(translate("\\-😀").is_err());
    }

    #[test]
    fn what_differs_is_refused() {
        for pattern in [
            r"\bword\b",
            "a(?=b)",
            "(?<!a)b",
            r"(a)\1",
            r"(?<n>a)\k<n>",
            r"\p{L}",
            "[\\p{L}]",
            "(?i:a)",
            "a{1001}",
            "a{0,1001}",
            r"\uD800",
            "(",
        ] {
            assert!(translate(pattern).is_err(), "{pattern}");
        }
    }
}
