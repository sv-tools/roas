//! Decode a raw JSON or YAML scalar back to its value, recording for every
//! decoded byte the raw byte it came from.
//!
//! The table is only trusted when the decoding reproduces the value the
//! parser produced; anything this does not model (explicit block
//! indentation indicators, tags, unusual folding) then yields `None`
//! instead of a wrong location.

/// For each byte of `value`, the offset in `text` it was decoded from,
/// plus one entry for the end of the content (`table[value.len()]`).
pub(super) fn decode(text: &str, start: usize, end: usize, value: &str) -> Option<Vec<usize>> {
    let raw = text.get(start..end)?;
    let mut out = Decoded::default();
    // A block's content may itself open with a quote: the header decides.
    if let Some(style) = block_header(text, start) {
        block(raw, start, start - line_start(text, start), style, &mut out);
        return out.finish(value, true);
    }
    let block = match raw.as_bytes().first() {
        Some(b'"') => {
            double(raw.strip_suffix('"')?, start, &mut out)?;
            out.end = end - 1;
            false
        }
        Some(b'\'') => {
            single(raw.strip_suffix('\'')?, start, &mut out)?;
            out.end = end - 1;
            false
        }
        _ => {
            plain(raw, start, &mut out);
            false
        }
    };
    out.finish(value, block)
}

#[derive(Default)]
struct Decoded {
    text: String,
    raw: Vec<usize>,
    /// Length of `text` up to the last byte line folding may not trim.
    keep: usize,
    /// Where the content ends in the raw text.
    end: usize,
}

impl Decoded {
    /// A character that line folding keeps.
    fn push(&mut self, c: char, at: usize) {
        self.push_trimmable(c, at);
        self.keep = self.text.len();
    }

    /// Unescaped white space, dropped if a line break follows.
    fn push_trimmable(&mut self, c: char, at: usize) {
        self.text.push(c);
        self.raw.extend(std::iter::repeat_n(at, c.len_utf8()));
    }

    fn trim(&mut self) {
        self.text.truncate(self.keep);
        self.raw.truncate(self.keep);
    }

    fn finish(self, value: &str, block: bool) -> Option<Vec<usize>> {
        // Block chomping (`|-`, `|+`, clip) only changes trailing breaks.
        let same = self.text == value
            || (block && self.text.trim_end_matches('\n') == value.trim_end_matches('\n'));
        if !same {
            return None;
        }
        let at = |index: usize| self.raw.get(index).copied().unwrap_or(self.end);
        Some((0..=value.len()).map(at).collect())
    }
}

/// Fold the line break at `content[i]` and the empty lines after it:
/// one break becomes a space, `n` further empty lines become `n` breaks,
/// and white space around the breaks goes. Returns where the next line's
/// content starts.
fn fold(content: &str, base: usize, mut i: usize, out: &mut Decoded) -> usize {
    out.trim();
    let first = base + i;
    i = after_break(content, i);
    let mut breaks = Vec::new();
    loop {
        i = after_blanks(content, i);
        if is_break(content, i) {
            breaks.push(base + i);
            i = after_break(content, i);
        } else {
            break;
        }
    }
    if breaks.is_empty() {
        out.push(' ', first);
    } else {
        for at in breaks {
            out.push('\n', at);
        }
    }
    i
}

fn is_break(content: &str, i: usize) -> bool {
    matches!(content.as_bytes().get(i), Some(b'\n' | b'\r'))
}

fn after_break(content: &str, i: usize) -> usize {
    if content[i..].starts_with("\r\n") {
        i + 2
    } else {
        i + 1
    }
}

fn after_blanks(content: &str, mut i: usize) -> usize {
    while matches!(content.as_bytes().get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    i
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at]
        .rfind(['\n', '\r'])
        .map_or(0, |newline| newline + 1)
}

fn plain(raw: &str, base: usize, out: &mut Decoded) {
    let content = raw.trim_end_matches([' ', '\t', '\n', '\r']);
    out.end = base + content.len();
    let mut i = 0;
    while let Some(c) = content[i..].chars().next() {
        match c {
            '\n' | '\r' => i = fold(content, base, i, out),
            ' ' | '\t' => {
                out.push_trimmable(c, base + i);
                i += 1;
            }
            _ => {
                out.push(c, base + i);
                i += c.len_utf8();
            }
        }
    }
}

fn single(raw: &str, start: usize, out: &mut Decoded) -> Option<()> {
    let content = raw.strip_prefix('\'')?;
    let base = start + 1;
    let mut i = 0;
    while let Some(c) = content[i..].chars().next() {
        match c {
            '\'' => {
                // `''` is a quote; a lone one cannot occur inside.
                content[i + 1..].starts_with('\'').then_some(())?;
                out.push('\'', base + i);
                i += 2;
            }
            '\n' | '\r' => i = fold(content, base, i, out),
            ' ' | '\t' => {
                out.push_trimmable(c, base + i);
                i += 1;
            }
            _ => {
                out.push(c, base + i);
                i += c.len_utf8();
            }
        }
    }
    Some(())
}

fn double(raw: &str, start: usize, out: &mut Decoded) -> Option<()> {
    let content = raw.strip_prefix('"')?;
    let base = start + 1;
    let mut i = 0;
    while let Some(c) = content[i..].chars().next() {
        match c {
            '\\' => {
                let at = base + i;
                let escape = content[i + 1..].chars().next()?;
                i += 1;
                let decoded = match escape {
                    '\n' | '\r' => {
                        // An escaped line break joins the lines with
                        // nothing between them.
                        i = after_blanks(content, after_break(content, i));
                        out.keep = out.text.len();
                        continue;
                    }
                    'x' => hex(content, &mut i, 2)?,
                    'u' => {
                        let unit = hex_value(content, &mut i, 4)?;
                        if (0xD800..0xDC00).contains(&unit) {
                            // JSON spells non-BMP characters as a pair.
                            content[i..].starts_with("\\u").then_some(())?;
                            i += 1;
                            let low = hex_value(content, &mut i, 4)?;
                            if !(0xDC00..0xE000).contains(&low) {
                                return None;
                            }
                            char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00))?
                        } else {
                            char::from_u32(unit)?
                        }
                    }
                    'U' => hex(content, &mut i, 8)?,
                    simple => {
                        i += simple.len_utf8();
                        match simple {
                            '0' => '\0',
                            'a' => '\x07',
                            'b' => '\x08',
                            't' | '\t' => '\t',
                            'n' => '\n',
                            'v' => '\x0b',
                            'f' => '\x0c',
                            'r' => '\r',
                            'e' => '\x1b',
                            ' ' => ' ',
                            '"' => '"',
                            '/' => '/',
                            '\\' => '\\',
                            'N' => '\u{85}',
                            '_' => '\u{a0}',
                            'L' => '\u{2028}',
                            'P' => '\u{2029}',
                            _ => return None,
                        }
                    }
                };
                out.push(decoded, at);
            }
            '\n' | '\r' => i = fold(content, base, i, out),
            ' ' | '\t' => {
                out.push_trimmable(c, base + i);
                i += 1;
            }
            _ => {
                out.push(c, base + i);
                i += c.len_utf8();
            }
        }
    }
    Some(())
}

/// The `digits` hex digits after the escape letter at `content[*i]`.
fn hex_value(content: &str, i: &mut usize, digits: usize) -> Option<u32> {
    let from = *i + 1;
    let text = content.get(from..from + digits)?;
    if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    *i = from + digits;
    u32::from_str_radix(text, 16).ok()
}

fn hex(content: &str, i: &mut usize, digits: usize) -> Option<char> {
    char::from_u32(hex_value(content, i, digits)?)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Block {
    Literal,
    Folded,
}

/// The style of the block scalar whose content starts at `start`, read
/// off the header (`|`, `>-`, …) ending the last non-blank line before
/// it. `None` for anything else, including an explicit indentation
/// indicator, whose indentation this does not reconstruct.
fn block_header(text: &str, start: usize) -> Option<Block> {
    header(text, start)
        .filter(|&(_, _, explicit)| !explicit)
        .map(|(style, _, _)| style)
}

/// Where an empty block scalar is written: just after its header.
/// `serde-saphyr` gives `k: |-` with no content a zero-length span on
/// the token after it, which belongs to another field.
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
pub(super) fn empty_block(text: &str, at: usize) -> Option<usize> {
    header(text, at).map(|(_, end, _)| end)
}

/// Whether the scalar whose content starts at `start` is a block scalar.
#[cfg_attr(not(feature = "source-map"), allow(dead_code))]
pub(super) fn is_block(text: &str, start: usize) -> bool {
    header(text, start).is_some()
}

/// The block style announced before `start`, where its indicator ends,
/// and whether it sets an explicit indentation.
fn header(text: &str, start: usize) -> Option<(Block, usize, bool)> {
    // Block content starts a line after its indentation; look back over
    // the spaces only, never along a long line (minified JSON).
    let line = start - (text[..start].len() - text[..start].trim_end_matches(' ').len());
    if line > 0 && !matches!(text.as_bytes()[line - 1], b'\n' | b'\r') {
        return None;
    }
    // The last non-blank line before this one.
    let mut end = line;
    let (from, content) = loop {
        let previous = line_start(text, end.checked_sub(1)?);
        let content = text[previous..end].trim_end_matches(['\n', '\r']);
        if !content.trim().is_empty() {
            break (previous, content);
        }
        end = previous;
    };
    // A comment starts at a `#` after white space.
    let comment = content
        .char_indices()
        .find(|&(at, c)| c == '#' && (at == 0 || content[..at].ends_with([' ', '\t'])))
        .map_or(content.len(), |(at, _)| at);
    let head = content[..comment].trim_end();
    let from_indicator = head.rfind([' ', '\t']).map_or(0, |at| at + 1);
    let indicator = &head[from_indicator..];
    // Only where a value may start: after `:`, `-`, `?`, a tag or an
    // anchor, or alone on its line — not the end of a plain `a |`.
    let before = head[..from_indicator].trim_end();
    let last = before.rsplit([' ', '\t']).next().unwrap_or_default();
    if !(before.is_empty()
        || before.ends_with(':')
        || matches!(last, "-" | "?")
        || last.starts_with(['!', '&']))
    {
        return None;
    }
    let mut chars = indicator.chars();
    let style = match chars.next()? {
        '|' => Block::Literal,
        '>' => Block::Folded,
        _ => return None,
    };
    // Chomping and indentation indicators, in either order.
    chars
        .all(|c| c == '-' || c == '+' || ('1'..='9').contains(&c))
        .then_some((
            style,
            from + head.len(),
            indicator.bytes().any(|byte| byte.is_ascii_digit()),
        ))
}

fn block(raw: &str, start: usize, indent: usize, style: Block, out: &mut Decoded) {
    // (content, where it starts, where its line break is)
    let mut lines = Vec::new();
    let mut offset = 0;
    // `\n`, `\r\n` and a bare `\r` each end a line.
    let mut lines_of = Vec::new();
    let mut from = 0;
    while from < raw.len() {
        let stop = raw[from..]
            .find(['\n', '\r'])
            .map_or(raw.len(), |at| from + at);
        let next = if raw[stop..].starts_with("\r\n") {
            stop + 2
        } else {
            (stop + 1).min(raw.len())
        };
        lines_of.push(&raw[from..next]);
        from = next;
    }
    for (index, line) in lines_of.into_iter().enumerate() {
        let body = line.trim_end_matches(['\n', '\r']);
        let skip = if index == 0 {
            0
        } else {
            body.bytes()
                .take(indent)
                .take_while(|&byte| byte == b' ')
                .count()
        };
        let newline = (body.len() < line.len()).then(|| start + offset + body.len());
        lines.push((&body[skip..], start + offset + skip, newline));
        offset += line.len();
    }
    // The span runs on into the next line's indentation.
    if matches!(lines.last(), Some((body, _, None)) if body.trim().is_empty()) {
        lines.pop();
    }
    out.end = start + raw.trim_end_matches([' ', '\t', '\n', '\r']).len();
    let normal = |body: &str| !body.is_empty() && !body.starts_with([' ', '\t']);
    let mut index = 0;
    while index < lines.len() {
        let (body, at, newline) = lines[index];
        for (position, c) in body.char_indices() {
            out.push(c, at + position);
        }
        let next = (index + 1..lines.len()).find(|&next| !lines[next].0.is_empty());
        match (style, next) {
            (Block::Folded, Some(next)) if normal(body) && normal(lines[next].0) => {
                let empties = &lines[index + 1..next];
                if empties.is_empty() {
                    out.push(' ', newline.unwrap_or(out.end));
                } else {
                    for (_, _, newline) in empties {
                        out.push('\n', newline.unwrap_or(out.end));
                    }
                }
                index = next;
            }
            _ => {
                if let Some(newline) = newline {
                    out.push('\n', newline);
                }
                index += 1;
            }
        }
    }
    // The value ends after the last thing it decoded — a kept break
    // (`|+`), trailing spaces of the last line — not at trimmed content.
    if let Some(&last) = out.raw.last() {
        let rest = &raw[last - start..];
        let width = if rest.starts_with("\r\n") {
            2
        } else {
            rest.chars().next().map_or(0, char::len_utf8)
        };
        out.end = out.end.max(last + width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode `raw` as it sits at the end of `prefix`, against `value`.
    fn table(prefix: &str, raw: &str, value: &str) -> Option<Vec<usize>> {
        let text = format!("{prefix}{raw}");
        decode(&text, prefix.len(), text.len(), value)
    }

    /// The raw text each decoded character came from.
    fn sources(prefix: &str, raw: &str, value: &str) -> Vec<String> {
        let text = format!("{prefix}{raw}");
        let table = table(prefix, raw, value).expect("decodes");
        value
            .char_indices()
            .map(|(at, c)| text[table[at]..table[at + c.len_utf8()]].to_owned())
            .collect()
    }

    #[test]
    fn json_escapes_map_back_to_their_spelling() {
        let raw = r#""$st\u00e9p\n == \ud83d\ude00 \"x\"""#;
        let value = "$stép\n == 😀 \"x\"";
        let table = table("a: ", raw, value).unwrap();
        // `é` came from `\u00e9`, both its bytes.
        assert_eq!(table[3], 3 + 4);
        assert_eq!(table[4], 3 + 4);
        // `p` follows the six-character escape.
        assert_eq!(table[5], 3 + 10);
        let emoji = value.find('😀').unwrap();
        assert_eq!(table[emoji], 3 + raw.find("\\ud83d").unwrap());
        assert_eq!(table[value.len()], 3 + raw.len() - 1);
        assert_eq!(sources("a: ", raw, value)[5], "\\n");
    }

    #[test]
    fn yaml_double_quoted_escapes_and_folding_decode() {
        for (raw, value) in [
            (
                r#""\x41\U0001F600\t\ \/\0\a\b\v\f\r\e\N\_\L\P""#,
                "A😀\t /\0\x07\x08\x0b\x0c\r\x1b\u{85}\u{a0}\u{2028}\u{2029}",
            ),
            ("\"one  \n   two\"", "one two"),
            ("\"one\n\n  two\"", "one\ntwo"),
            ("\"one\\\n   two\"", "onetwo"),
            ("\"one\\ \n two\"", "one  two"),
            ("\"crlf\r\n two\"", "crlf two"),
            ("\"tab\\\tx\"", "tab\tx"),
            (r#""a\\b""#, "a\\b"),
        ] {
            assert!(table("k: ", raw, value).is_some(), "{raw:?}");
        }
        let table = table("k: ", "\"one\n  two\"", "one two").unwrap();
        // The folded space stands for the line break.
        assert_eq!(table[3], 3 + 4);
        assert_eq!(table[4], 3 + 7);
    }

    #[test]
    fn malformed_or_unmodelled_escapes_give_no_table() {
        for raw in [
            r#""\q""#,
            r#""\u12""#,
            r#""\uZZZZ""#,
            r#""\ud83d""#,
            r#""\ud83d\u0041""#,
            r#""\"#,
            "\"open",
            r#""\UFFFFFFFF""#,
        ] {
            assert_eq!(table("k: ", raw, "x"), None, "{raw:?}");
        }
        // Decodes, but not to the value the parser saw.
        assert_eq!(table("k: ", "\"abc\"", "abd"), None);
    }

    #[test]
    fn single_quoted_scalars_unescape_quotes_and_fold() {
        let table = table("k: ", "'it''s\n  ok'", "it's ok").unwrap();
        assert_eq!(table[2], 3 + 3);
        assert_eq!(table[3], 3 + 5);
        assert_eq!(table[5], 3 + 9);
        assert_eq!(super::decode("k: 'a'b'", 3, 8, "a"), None);
        assert_eq!(super::decode("k: 'ab", 3, 6, "ab"), None);
    }

    #[test]
    fn plain_scalars_fold_and_keep_unicode() {
        let table = table("k: ", "$a == ünï", "$a == ünï").unwrap();
        assert_eq!(table[6], 3 + 6);
        assert_eq!(table[8], 3 + 8);
        let folded =
            super::decode("k: one\n   two\n\n   three  \n", 3, 26, "one two\nthree").unwrap();
        assert_eq!(folded[4], 10);
        assert_eq!(folded[7], 14);
        assert_eq!(folded[8], 18);
        assert_eq!(*folded.last().unwrap(), 23);
    }

    #[test]
    fn literal_blocks_strip_indentation() {
        let text = "k: |\n    line1\n      $x == 2\nnext: 1\n";
        let start = text.find("line1").unwrap();
        let end = text.find("next").unwrap();
        let value = "line1\n  $x == 2\n";
        let table = decode(text, start, end, value).unwrap();
        assert_eq!(table[6], text.find("  $x").unwrap());
        assert_eq!(table[8], text.find("$x").unwrap());
        // Chomping only changes trailing breaks.
        for chomp in ["|-", "|+"] {
            let text = text.replace('|', chomp);
            let start = text.find("line1").unwrap();
            let end = text.find("next").unwrap();
            assert!(
                decode(&text, start, end, value.trim_end()).is_some(),
                "{chomp}"
            );
        }
        // Empty lines inside the block keep their breaks.
        let text = "k: |\n  a\n\n  b\n";
        let start = text.find('a').unwrap();
        assert!(decode(text, start, text.len(), "a\n\nb\n").is_some());
        // An explicit indentation indicator is not modelled.
        let text = "k: |2\n    x\n";
        let start = text.find('x').unwrap() - 2;
        assert_eq!(decode(text, start, text.len(), "  x\n"), None);
    }

    #[test]
    fn folded_blocks_join_normal_lines() {
        let text = "k: > # note\n  one\n  two\n\n  three\n    more\n  end\n";
        let start = text.find("one").unwrap();
        let value = "one two\nthree\n  more\nend\n";
        let table = decode(text, start, text.len(), value).unwrap();
        assert_eq!(table[3], text.find("one\n").unwrap() + 3);
        assert_eq!(table[8], text.find("three").unwrap());
        assert_eq!(table[16], text.find("more").unwrap());
    }

    #[test]
    fn a_header_needs_a_block_indicator_at_the_end_of_the_line() {
        let text = "k:\n  value\n";
        assert_eq!(block_header(text, text.find("value").unwrap()), None);
        let text = "- >-\n  x\n";
        assert_eq!(
            block_header(text, text.find('x').unwrap()),
            Some(Block::Folded)
        );
        let text = "k: |x\n  x\n";
        assert_eq!(block_header(text, text.rfind('x').unwrap()), None);
        // A tab before the comment, and a `#` that starts no comment.
        let text = "k: |\t# note\n  x\n";
        assert_eq!(
            block_header(text, text.rfind('x').unwrap()),
            Some(Block::Literal)
        );
        let text = "k:\n# |\n  x\n";
        assert_eq!(block_header(text, text.rfind('x').unwrap()), None);
        let text = "? a |\n: x\n";
        assert_eq!(block_header(text, text.rfind('x').unwrap()), None);
        for header in ["k: !!str |", "k: &a >", "|", "- - |", "? |"] {
            let text = format!("{header}\n  x\n");
            assert!(
                block_header(&text, text.rfind('x').unwrap()).is_some(),
                "{header}"
            );
        }
        let text = "k: a#|\n  x\n";
        assert_eq!(block_header(text, text.rfind('x').unwrap()), None);
        // An empty block is written at its header, not at what follows.
        let text = "k: |-\n\nz: 1\n";
        assert_eq!(empty_block(text, text.find('z').unwrap()), Some(5));
        assert_eq!(empty_block("z", 0), None);
        // Not at the start of its line.
        let text = "k: |\n  a b\n";
        assert_eq!(block_header(text, text.find('b').unwrap()), None);
        assert_eq!(block_header("x", 0), None);
    }
}
