//! XPath criteria and selectors, evaluated by `oxixml-xpath` as the XPath
//! version the criterion or selector names.
//!
//! The data an XPath expression applies to is XML text: a response body
//! that is not JSON arrives as a string, and so does any other value a
//! context names that holds a document. Anything else is not XML, and
//! the expression cannot be evaluated against it.
//!
//! A response body is untrusted, so the engine only ever runs on a thread
//! of [`ENGINE_STACK`] bytes, which it is told it has, under a deadline of
//! [`DEADLINE`]: deep recursion and long evaluations become errors. The
//! engine checks that deadline only between steps and calls, so a run
//! waits for it no longer than [`WAIT`] whatever happens on that thread. A
//! context is screened by [`too_complex`] before it is parsed, and an
//! expression by [`screened`] before it is compiled, for the costs the
//! engine does not bound itself. A panic inside the engine becomes an
//! unsupported-capability error instead of unwinding through a run; the
//! panic hook still runs.

use crate::select::SelectError;
use oxixml_infoset::{AtomicData, Item, Tree};
use oxixml_xpath::{
    CompiledXPath, EvaluationLimits, StaticContext, XPath, XPathError, XPathVersion, XpathItem,
};
use serde_json::Value;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const KIND: &str = "XPath";

/// The stack of the thread the engine runs on. The engine measures what
/// it uses against it and stops a recursion with an error at two thirds.
const ENGINE_STACK: usize = 16 * 1024 * 1024;

/// How long one evaluation may run, parsing included. The engine checks
/// it between steps and calls.
const DEADLINE: Duration = Duration::from_secs(2);

/// How long a run waits for the engine before giving up on it. Parsing
/// and a single built-in run to completion past [`DEADLINE`]; the limits
/// on a context and the functions refused keep them short, and this
/// bounds what they miss. The engine's thread is then left to finish on
/// its own, and what it built is dropped there.
const WAIT: Duration = Duration::from_secs(4);

/// Check that `expression` is a valid expression of `version`.
pub(crate) fn compile(expression: &str, version: XPathVersion) -> Result<(), SelectError> {
    screened(expression)?;
    let owned = expression.to_owned();
    on_engine(expression, move || compiled(&owned, version).map(drop))
}

/// Whether `condition` holds over `context`: its effective boolean
/// value. A null context fails, as the specification has it.
pub(crate) fn passes(
    condition: &str,
    version: XPathVersion,
    context: &Value,
) -> Result<bool, SelectError> {
    if context.is_null() {
        return Ok(false);
    }
    evaluate(condition, version, context, |query, tree, condition| {
        query
            .evaluate_to_boolean_in(&query.context_with(tree.root()))
            .map_err(|error| failed(condition, &error.to_string()))
    })
}

/// What `selector` picks out of `context`. `None` when it picked
/// nothing; one item is its value and several are the list of them.
pub(crate) fn select(
    selector: &str,
    version: XPathVersion,
    context: &Value,
) -> Result<Option<Value>, SelectError> {
    if context.is_null() {
        return Ok(None);
    }
    evaluate(selector, version, context, |query, tree, selector| {
        let value = query
            .evaluate_value_in(&query.context_with(tree.root()))
            .map_err(|error| failed(selector, &error.to_string()))?;
        let mut items = value
            .iter()
            .map(|item| json(selector, item))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match items.len() {
            0 => None,
            1 => items.pop(),
            _ => Some(Value::Array(items)),
        })
    })
}

fn evaluate<T: Send + 'static>(
    expression: &str,
    version: XPathVersion,
    context: &Value,
    then: fn(&CompiledXPath, &Tree, &str) -> Result<T, SelectError>,
) -> Result<T, SelectError> {
    let Value::String(xml) = context else {
        return Err(failed(
            expression,
            "the context is not XML text, but a JSON value",
        ));
    };
    screened(expression)?;
    if let Some(reason) = too_complex(xml) {
        return Err(failed(expression, &reason));
    }
    let (owned, xml) = (expression.to_owned(), xml.clone());
    on_engine(expression, move || {
        let query = compiled(&owned, version)?;
        let tree = Tree::parse_str(&xml)
            .map_err(|error| failed(&owned, &format!("the context is not XML: {error}")))?;
        then(&query, &tree, &owned)
    })
}

fn compiled(expression: &str, version: XPathVersion) -> Result<CompiledXPath, SelectError> {
    XPath::compile(expression, version, StaticContext::new())
        .map_err(|error| malformed(expression, &error))
}

/// Run `work` on a thread of [`ENGINE_STACK`] bytes under [`DEADLINE`],
/// so that what it may spend is known to the engine and does not depend
/// on the caller's thread, and wait for it no longer than [`WAIT`]. A
/// panic inside it is an unsupported capability: what the engine cannot
/// do is a limit of this executor, not a condition that is false.
/// Everything the engine built is dropped on that thread, so nothing it
/// left half-done is seen again.
fn on_engine<T: Send + 'static>(
    expression: &str,
    work: impl FnOnce() -> Result<T, SelectError> + Send + 'static,
) -> Result<T, SelectError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("roas-xpath".to_owned())
        .stack_size(ENGINE_STACK)
        .spawn(move || {
            let _stack = EvaluationLimits::declare(ENGINE_STACK);
            let _deadline = EvaluationLimits::declare_deadline(Instant::now() + DEADLINE);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or(Err(SelectError::Unsupported(UNIMPLEMENTED)));
            // The run may have stopped waiting.
            let _ = sender.send(result);
        })
        .map_err(|_| SelectError::Unsupported(NO_THREAD))?;
    match receiver.recv_timeout(WAIT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(failed(
            expression,
            &format!(
                "the evaluation did not finish within {} seconds",
                WAIT.as_secs()
            ),
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(SelectError::Unsupported(UNIMPLEMENTED)),
    }
}

/// The capability an engine panic is reported as.
const UNIMPLEMENTED: &str = "partly unimplemented XPath";

/// The capability reported when no thread can be started for the engine.
const NO_THREAD: &str = "XPath without a thread to evaluate it on";

fn malformed(expression: &str, error: &XPathError) -> SelectError {
    SelectError::Malformed {
        selector: expression.to_owned(),
        kind: KIND,
        message: error.to_string(),
    }
}

/// An item as JSON: a node is its string value, a boolean stays one, a
/// number becomes a number when JSON can write it, and any other atomic
/// value is its string value.
fn json(selector: &str, item: &XpathItem) -> Result<Value, SelectError> {
    let no_json = || failed(selector, "a function, map or array item has no JSON value");
    let value = match item {
        XpathItem::Value(Item::Atomic(value)) => value,
        XpathItem::Value(Item::Node(_)) => {
            return item
                .string_value()
                .map(Value::String)
                .map_err(|error| failed(selector, &error.to_string()));
        }
        _ => return Err(no_json()),
    };
    let text = || oxixml_xpath::atomic_string_value(value);
    Ok(match value.data() {
        AtomicData::Boolean(_) => value
            .as_bool()
            .map_or_else(|| Value::String(text()), Value::Bool),
        // The integers and decimals too wide for 64 bits have variants
        // that can only be told by their accessors.
        data if matches!(data, AtomicData::Integer(_) | AtomicData::Decimal(_))
            || data.as_big_integer().is_some()
            || data.as_big_decimal().is_some() =>
        {
            // An integer or decimal JSON cannot hold exactly stays text,
            // rather than being rounded through `f64`. The value is what
            // is compared, not its spelling: `0.00000095367431640625` is
            // held exactly, though JSON writes it `9.5367431640625e-7`.
            //
            // serde_json's own parse is exact under `arbitrary_precision`;
            // otherwise it is not always correctly rounded, so Rust's
            // parse is tried as well.
            let text = text();
            let exact = |number: &Value| same_decimal(&number.to_string(), &text);
            serde_json::from_str::<Value>(&text)
                .ok()
                .filter(|number| number.is_number() && exact(number))
                .or_else(|| text.parse::<f64>().ok().and_then(floating).filter(exact))
                .unwrap_or(Value::String(text))
        }
        // A double's text is rounded to fewer digits than it holds, so it
        // is converted from its value. `NaN` and `INF` have no JSON
        // number, so they stay text.
        AtomicData::Double(_) => value
            .as_f64()
            .and_then(floating)
            .unwrap_or_else(|| Value::String(text())),
        // A float is read from the shortest numeral that names its `f32`:
        // the widened value would turn `0.1` into `0.10000000149011612`.
        // Rust writes that numeral and parses it back correctly rounded.
        AtomicData::Float(_) => value
            .as_f64()
            .and_then(|wide| (wide as f32).to_string().parse::<f64>().ok())
            .and_then(floating)
            .unwrap_or_else(|| Value::String(text())),
        _ => Value::String(text()),
    })
}

/// Functions an expression may not call or name.
///
/// - `fn:parse-xml`, `fn:parse-xml-fragment` and `fn:json-to-xml` build a
///   document out of text the limits on a context never saw.
/// - `fn:parse-json` builds maps and arrays nested as deep as that text
///   says, and the engine clones and drops nested values recursively,
///   beyond the stack it measures; so would a fold that wraps its
///   accumulator, which is why `fn:fold-left`, `fn:fold-right` and the
///   `array:` folds are refused.
/// - The regular-expression functions run a single pattern to completion,
///   past the deadline, over text that may be a whole response; an Arazzo
///   `regex` criterion matches instead.
/// - `fn:distinct-values` compares each value with every one it kept, and
///   `fn:id`, `fn:idref` and `fn:element-with-id` each element with every
///   token asked for, in a single built-in that runs past the deadline;
///   over a response's values that is minutes.
/// - `fn:function-lookup` could reach any of them by a name built at run
///   time.
const REFUSED_FUNCTIONS: [&str; 15] = [
    "parse-xml",
    "parse-xml-fragment",
    "json-to-xml",
    "parse-json",
    "matches",
    "replace",
    "tokenize",
    "analyze-string",
    "fold-left",
    "fold-right",
    "function-lookup",
    "distinct-values",
    "id",
    "idref",
    "element-with-id",
];

/// What an expression calling or naming one of [`REFUSED_FUNCTIONS`] is
/// refused as.
const UNSAFE_FUNCTION: &str = "XPath fn:parse-xml, fn:parse-xml-fragment, fn:json-to-xml, fn:parse-json, fn:distinct-values, fn:id, fn:idref, fn:element-with-id, fn:function-lookup, folds and regular-expression functions";

/// Refuse an expression that calls or names a function in
/// [`REFUSED_FUNCTIONS`], by one pass over it. Names inside string
/// literals, braced URIs and comments count for nothing. A name counts
/// as a function's when `(` or `#` follows it, whatever its prefix, so an
/// element named `matches` is still a path. How deep an expression nests
/// the engine limits itself.
fn screened(expression: &str) -> Result<(), SelectError> {
    let bytes = expression.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            // A braced URI, `Q{...}`, is one token to the engine: nothing
            // inside it opens a literal or a comment.
            b'Q' if bytes.get(index + 1) == Some(&b'{')
                && !index
                    .checked_sub(1)
                    .is_some_and(|before| is_name_byte(bytes[before])) =>
            {
                index += 2;
                while index < bytes.len() && bytes[index] != b'}' {
                    index += 1;
                }
            }
            // A literal ends at its quote; a doubled quote is one more
            // character of it, and reads the same as two literals here.
            quote @ (b'\'' | b'"') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
            }
            b'(' if bytes.get(index + 1) == Some(&b':') => {
                let mut comments = 1;
                index += 2;
                while index < bytes.len() && comments > 0 {
                    if bytes[index..].starts_with(b"(:") {
                        comments += 1;
                        index += 1;
                    } else if bytes[index..].starts_with(b":)") {
                        comments -= 1;
                        index += 1;
                    }
                    index += 1;
                }
                continue;
            }
            // A name starts with a letter, `_` or a non-ASCII character;
            // `-`, `.`, digits and `:` only continue one.
            byte if byte.is_ascii_alphabetic() || byte == b'_' || !byte.is_ascii() => {
                let start = index;
                while index < bytes.len() && is_name_byte(bytes[index]) {
                    index += 1;
                }
                let name = &expression[start..index];
                let local = name.rsplit_once(':').map_or(name, |(_, local)| local);
                let mut next = index;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                if REFUSED_FUNCTIONS.contains(&local)
                    && matches!(bytes.get(next), Some(b'(' | b'#'))
                {
                    return Err(SelectError::Unsupported(UNSAFE_FUNCTION));
                }
                continue;
            }
            _ => {}
        }
        index += 1;
    }
    Ok(())
}

/// Whether `byte` can be part of an XPath name, so that a `Q` after it
/// does not start a braced URI. Any non-ASCII byte is taken to be one.
fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':') || !byte.is_ascii()
}

/// Limits on a context, checked before the engine parses it. A response
/// body is untrusted, and parsing it runs to completion: the engine
/// spends close to a kilobyte on each node, checks each attribute against
/// the ones before it on the same element, and gives every element a node
/// for every namespace binding in scope on it, so a few megabytes of
/// markup could take seconds and gigabytes. These keep a parsed context
/// to a few hundred megabytes at most. Evaluating paths over a large or
/// deep document is bounded by the deadline instead. Real documents stay
/// far below all of these.
const MAX_CONTEXT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 1024;
/// Elements, comments and processing instructions; text nodes lie
/// between them, so they are bounded too.
const MAX_NODES: usize = 100_000;
const MAX_ATTRIBUTES_PER_ELEMENT: usize = 256;
const MAX_ATTRIBUTES: usize = 100_000;
/// Namespace bindings held by elements, summed over every element: one
/// for each prefix (or the default namespace) bound in scope on it,
/// besides `xml`, which every element holds and [`MAX_NODES`] counts.
const MAX_NAMESPACE_NODES: usize = 100_000;

/// Why `xml` is past one of the limits above, or holds a document type
/// declaration, if it does, by one pass over its markup. The engine
/// expands the internal entities a declaration defines, so a short
/// document could stand for a far larger one; none is accepted. Comments,
/// CDATA sections and processing instructions open nothing, and quoted
/// attribute values are read as values, so markup inside one counts for
/// nothing. Markup this pass cannot read is left for the parser to refuse.
fn too_complex(xml: &str) -> Option<String> {
    if xml.len() > MAX_CONTEXT_BYTES {
        return Some(format!(
            "the context is larger than {} MiB",
            MAX_CONTEXT_BYTES / 1024 / 1024
        ));
    }
    let bytes = xml.as_bytes();
    let length = bytes.len();
    // The prefixes each open element declared.
    let mut open: Vec<Vec<&str>> = Vec::new();
    let mut bindings = Bindings::default();
    let mut namespace_nodes = 0usize;
    let mut nodes = 0usize;
    let mut all_attributes = 0usize;
    let skip = |from: usize, end: &str| {
        xml[from..]
            .find(end)
            .map_or(length, |found| from + found + end.len())
    };
    let is_delimiter = |byte: u8| {
        byte.is_ascii_whitespace() || matches!(byte, b'=' | b'>' | b'/' | b'"' | b'\'' | b'?')
    };
    let name_at = |mut index: usize| {
        let start = index;
        while index < length && !is_delimiter(bytes[index]) {
            index += 1;
        }
        (&xml[start..index], index)
    };
    let mut at = 0;
    while let Some(found) = xml[at..].find('<') {
        let start = at + found;
        let rest = &xml[start..];
        if !rest.starts_with("</") && !rest.starts_with("<![CDATA[") {
            nodes += 1;
            if nodes > MAX_NODES {
                return Some(format!(
                    "the context has more than {MAX_NODES} elements, comments and processing instructions"
                ));
            }
        }
        if rest.starts_with("<!--") {
            at = skip(start, "-->");
        } else if rest.starts_with("<![CDATA[") {
            at = skip(start, "]]>");
        } else if rest.starts_with("<?") {
            at = skip(start, "?>");
        } else if rest.starts_with("<!") {
            return Some("the context has a document type declaration".to_owned());
        } else if rest.starts_with("</") {
            for prefix in open.pop().unwrap_or_default() {
                bindings.unbind(prefix);
            }
            at = skip(start, ">");
        } else {
            // A start tag: its name, then attributes, up to `>` or `/>`.
            let (_, mut index) = name_at(start + 1);
            let mut attributes = 0usize;
            let mut declared = Vec::new();
            let mut closed = false;
            while index < length {
                match bytes[index] {
                    b'>' => break,
                    b'/' if bytes.get(index + 1) == Some(&b'>') => {
                        closed = true;
                        index += 1;
                        break;
                    }
                    byte if is_delimiter(byte) => {
                        index += 1;
                        continue;
                    }
                    _ => {}
                }
                let (name, after) = name_at(index);
                index = after;
                if attributes == MAX_ATTRIBUTES_PER_ELEMENT {
                    return Some(format!(
                        "the context has an element with more than {MAX_ATTRIBUTES_PER_ELEMENT} attributes"
                    ));
                }
                attributes += 1;
                all_attributes += 1;
                if all_attributes > MAX_ATTRIBUTES {
                    return Some(format!(
                        "the context has more than {MAX_ATTRIBUTES} attributes"
                    ));
                }
                // `=` and a quoted value, if they follow.
                while index < length && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                let mut value = "";
                if bytes.get(index) == Some(&b'=') {
                    index += 1;
                    while index < length && bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    if let Some(&quote @ (b'"' | b'\'')) = bytes.get(index) {
                        let value_start = index + 1;
                        match xml[value_start..].find(char::from(quote)) {
                            Some(found) => {
                                value = &xml[value_start..value_start + found];
                                index = value_start + found + 1;
                            }
                            None => {
                                value = &xml[value_start..];
                                index = length;
                            }
                        }
                    }
                }
                let prefix = match name.split_once(':') {
                    Some(("xmlns", prefix)) => prefix,
                    None if name == "xmlns" => "",
                    _ => continue,
                };
                bindings.bind(prefix, value);
                declared.push(prefix);
            }
            namespace_nodes += bindings.in_scope;
            if namespace_nodes > MAX_NAMESPACE_NODES {
                return Some(format!(
                    "the context binds more than {MAX_NAMESPACE_NODES} namespaces across its elements"
                ));
            }
            if closed {
                for prefix in declared {
                    bindings.unbind(prefix);
                }
            } else {
                open.push(declared);
                if open.len() > MAX_DEPTH {
                    return Some(format!(
                        "the context nests elements more than {MAX_DEPTH} deep"
                    ));
                }
            }
            at = (index + 1).min(length);
        }
    }
    None
}

/// Every prefix's namespace bindings, innermost last, and how many
/// prefixes are bound in scope: a prefix is while its innermost binding
/// is not empty (`xmlns=""` undeclares the default namespace).
#[derive(Default)]
struct Bindings<'x> {
    stacks: std::collections::HashMap<&'x str, Vec<&'x str>>,
    in_scope: usize,
}

impl<'x> Bindings<'x> {
    fn bound(&self, prefix: &str) -> bool {
        self.stacks
            .get(prefix)
            .and_then(|stack| stack.last())
            .is_some_and(|namespace| !namespace.is_empty())
    }

    fn bind(&mut self, prefix: &'x str, namespace: &'x str) {
        let before = self.bound(prefix);
        self.stacks.entry(prefix).or_default().push(namespace);
        self.recount(before, prefix);
    }

    fn unbind(&mut self, prefix: &str) {
        let before = self.bound(prefix);
        if let Some(stack) = self.stacks.get_mut(prefix) {
            stack.pop();
        }
        self.recount(before, prefix);
    }

    fn recount(&mut self, before: bool, prefix: &str) {
        self.in_scope = self.in_scope + usize::from(self.bound(prefix)) - usize::from(before);
    }
}

/// A finite floating-point value as a JSON number: a whole one that an
/// integer holds exactly is written as that integer — `number(/id)`
/// over `<id>7</id>` goes into a request as `7`, not `7.0`.
fn floating(value: f64) -> Option<Value> {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    // `-0` has no integer to be written as.
    if value.fract() == 0.0 && value.abs() < EXACT && !(value == 0.0 && value.is_sign_negative()) {
        return Some(Value::from(value as i64));
    }
    serde_json::Number::from_f64(value).map(Value::Number)
}

/// Whether two decimal numerals, in plain or scientific notation, name
/// the same number.
fn same_decimal(left: &str, right: &str) -> bool {
    normalized(left).is_some_and(|left| normalized(right) == Some(left))
}

/// A numeral as its sign, its significant digits and the power of ten
/// they are scaled by; zero has no digits and no sign.
fn normalized(numeral: &str) -> Option<(bool, String, i64)> {
    let (negative, unsigned) = match numeral.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, numeral.strip_prefix('+').unwrap_or(numeral)),
    };
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (unsigned, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty() && fraction.is_empty()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let digits = format!("{whole}{fraction}");
    let mut exponent = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    let digits = digits.trim_start_matches('0');
    let significant = digits.trim_end_matches('0');
    if significant.is_empty() {
        return Some((false, String::new(), 0));
    }
    exponent = exponent.checked_add(i64::try_from(digits.len() - significant.len()).ok()?)?;
    Some((negative, significant.to_owned(), exponent))
}

fn failed(expression: &str, message: &str) -> SelectError {
    SelectError::Evaluation {
        selector: expression.to_owned(),
        kind: KIND,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const V31: XPathVersion = XPathVersion::XPath31;

    const ORDER: &str =
        r#"<order id="7"><item>a</item><item>b</item><total>12.5</total><paid>true</paid></order>"#;

    fn order() -> Value {
        Value::String(ORDER.to_owned())
    }

    fn holds(condition: &str) -> Result<bool, SelectError> {
        passes(condition, V31, &order())
    }

    fn picked(selector: &str) -> Option<Value> {
        select(selector, V31, &order()).unwrap()
    }

    fn evaluation_error(result: Result<Option<Value>, SelectError>) -> String {
        match result {
            Err(SelectError::Evaluation { message, .. }) => message,
            other => panic!("expected an evaluation error, got {other:?}"),
        }
    }

    #[test]
    fn a_criterion_holds_by_its_effective_boolean_value() {
        assert_eq!(holds("/order/item"), Ok(true));
        assert_eq!(holds("/order/missing"), Ok(false));
        assert_eq!(holds("/order/total > 10"), Ok(true));
        assert_eq!(holds("count(//item) = 3"), Ok(false));
        assert_eq!(holds("0"), Ok(false));
        assert_eq!(holds("''"), Ok(false));
        assert_eq!(holds("'x'"), Ok(true));
    }

    #[test]
    fn a_null_context_fails_without_an_error() {
        assert_eq!(passes("/order", V31, &Value::Null), Ok(false));
        assert_eq!(select("/order", V31, &Value::Null), Ok(None));
    }

    #[test]
    fn a_sequence_with_no_effective_boolean_value_is_an_error() {
        assert!(matches!(
            holds("(1, 2)"),
            Err(SelectError::Evaluation { message, .. }) if message.contains("FORG0006")
        ));
    }

    #[test]
    fn each_version_is_evaluated_as_itself() {
        // `'3' + 4` is 7 in XPath 1.0 and a type error later.
        assert_eq!(
            passes("'3' + 4 = 7", XPathVersion::XPath10, &order()),
            Ok(true)
        );
        assert!(passes("'3' + 4 = 7", V31, &order()).is_err());
        let refused = |expression, version| {
            matches!(
                compile(expression, version),
                Err(SelectError::Malformed { message, .. }) if message.contains("XPST0003")
            )
        };
        assert!(refused("1 to 3", XPathVersion::XPath10));
        assert_eq!(compile("1 to 3", XPathVersion::XPath20), Ok(()));
        assert!(refused("let $x := 1 return $x", XPathVersion::XPath20));
        assert_eq!(
            compile("let $x := 1 return $x", XPathVersion::XPath30),
            Ok(())
        );
        assert!(refused("map { 1: 2 }", XPathVersion::XPath30));
        assert_eq!(compile("map { 1: 2 }", V31), Ok(()));
        assert_eq!(
            select("/order/item", XPathVersion::XPath10, &order()),
            Ok(Some(json!(["a", "b"])))
        );
    }

    #[test]
    fn a_context_that_is_not_xml_is_an_error() {
        assert!(evaluation_error(select("/a", V31, &json!({ "a": 1 }))).contains("not XML text"));
        assert!(evaluation_error(select("/a", V31, &json!("not xml"))).contains("not XML"));
        // A document type declaration is refused, so no entity is ever
        // expanded.
        for declared in [
            r#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]><x>&e;</x>"#,
            r#"<?xml version="1.0"?><!-- c --><!DOCTYPE x [<!ENTITY a "aaaa">]><x>&a;</x>"#,
        ] {
            assert!(
                evaluation_error(select("/x", V31, &json!(declared)))
                    .contains("document type declaration")
            );
        }
    }

    #[test]
    fn malformed_syntax_is_reported_with_where_it_went_wrong() {
        let error = compile("/order/[", V31).unwrap_err();
        assert!(matches!(
            &error,
            SelectError::Malformed { kind: "XPath", message, .. }
                if message.starts_with("[XPST0003]") && message.contains("at offset")
        ));
        assert_eq!(select("/order/[", V31, &order()), Err(error));
        assert_eq!(compile("/order/item", V31), Ok(()));
        // A prefix the expression never bound is a static error.
        assert!(matches!(
            compile("/p:order", V31),
            Err(SelectError::Malformed { .. })
        ));
    }

    #[test]
    fn a_panic_inside_the_engine_is_unsupported() {
        let result: Result<(), _> = on_engine("x", || panic!("an engine bug"));
        assert_eq!(result, Err(SelectError::Unsupported(UNIMPLEMENTED)));
    }

    #[test]
    fn a_run_waits_for_the_engine_only_so_long() {
        let started = Instant::now();
        let result: Result<(), _> = on_engine("x", || {
            std::thread::sleep(WAIT + Duration::from_secs(1));
            Ok(())
        });
        assert!(
            matches!(&result, Err(SelectError::Evaluation { message, .. }) if message.contains("did not finish")),
            "{result:?}"
        );
        assert!(started.elapsed() < WAIT + Duration::from_secs(1));
    }

    #[test]
    fn an_evaluation_error_is_reported() {
        assert!(evaluation_error(select("1 div 0", V31, &order())).contains("FOAR0001"));
    }

    #[test]
    fn deep_recursion_is_an_error_not_an_abort() {
        let recursion = "let $f := function($f, $n) { if ($n = 0) then 0 else $f($f, $n - 1) + 1 } \
                         return $f($f, 1000000)";
        assert!(evaluation_error(select(recursion, V31, &order())).contains("XPDY0130"));
        // Nor does recursion that nests arrays as deep as it can go.
        let nesting = "let $f := function($f, $n) { if ($n = 0) then [] else [$f($f, $n - 1)] } \
                       return array:size($f($f, 1000000))";
        assert!(evaluation_error(select(nesting, V31, &order())).contains("XPDY0130"));
    }

    #[test]
    fn a_long_evaluation_stops_at_the_deadline() {
        // Chains of elements as deep as a context may nest: `//a//a`
        // over them takes several times the deadline.
        let chain = format!(
            "{}{}",
            "<a>".repeat(MAX_DEPTH - 1),
            "</a>".repeat(MAX_DEPTH - 1)
        );
        let deep = format!("<r>{}</r>", chain.repeat(300_000 / chain.len() + 1));
        let started = Instant::now();
        let message = evaluation_error(select("count(//a//a)", V31, &json!(deep)));
        assert!(message.contains("XPDY0130"), "{message}");
        assert!(started.elapsed() < DEADLINE * 3);
    }

    #[test]
    fn a_selector_maps_each_item_to_json() {
        assert_eq!(picked("/order/@id"), Some(json!("7")));
        assert_eq!(picked("/order/item"), Some(json!(["a", "b"])));
        assert_eq!(picked("/order/missing"), None);
        assert_eq!(picked("count(//item)"), Some(json!(2)));
        assert_eq!(
            picked("xs:integer('-9223372036854775808')"),
            Some(json!(i64::MIN))
        );
        assert_eq!(
            picked("xs:integer('18446744073709551615')"),
            Some(json!(u64::MAX))
        );
        let large = picked("xs:integer('123456789012345678901234567890')").unwrap();
        // Numbers, compared by value: under `arbitrary_precision` JSON
        // numbers compare by their spelling.
        let number = |selector: &str| picked(selector).and_then(|value| value.as_f64());
        assert_eq!(
            number("xs:double('1.2345678901234567')"),
            Some(1.2345678901234567)
        );
        assert_eq!(
            number("xs:decimal('0.00000095367431640625')"),
            Some(0.00000095367431640625)
        );
        for small in ["0.00001", "0.000001", "0.0000001"] {
            let selector = format!("xs:decimal('{small}')");
            assert_eq!(number(&selector), small.parse::<f64>().ok(), "{small}");
        }
        assert_eq!(number("xs:decimal('1000000000000000000000')"), Some(1e21));
        assert_eq!(picked("xs:double('INF')"), Some(json!("INF")));
        let negative_zero = picked("xs:double('-0')").and_then(|value| value.as_f64());
        assert!(negative_zero.is_some_and(|zero| zero == 0.0 && zero.is_sign_negative()));
        assert_eq!(picked("xs:float('-INF')"), Some(json!("-INF")));
        // A whole double or float is an integer, as XPath writes it.
        assert_eq!(picked("number(/order/@id)"), Some(json!(7)));
        assert_eq!(picked("sum((1e0, 2e0))"), Some(json!(3)));
        assert_eq!(picked("xs:float('7')"), Some(json!(7)));
        assert_eq!(number("xs:float('1e30')"), Some(1e30));
        assert_eq!(number("xs:float('500000.03')"), Some(500000.03));
        assert_eq!(
            number("xs:decimal('0.0000000000000000000000000001')"),
            Some(1e-28)
        );
        assert_eq!(picked("xs:decimal('25.0')"), Some(json!(25)));
        assert_eq!(number("xs:float('0.1')"), Some(0.1));
        assert_eq!(number("xs:double('1e300')"), Some(1e300));
        let decimal = picked("xs:decimal('12345678901234567890.5')").unwrap();
        assert!(
            decimal == json!("12345678901234567890.5")
                || format!("{decimal}") == "12345678901234567890.5",
            "{decimal}"
        );
        // Text by default; the exact number under `arbitrary_precision`.
        let digits = "123456789012345678901234567890";
        assert!(
            large == json!(digits) || format!("{large}") == digits,
            "{large}"
        );
        assert_eq!(picked("xs:decimal(/order/total) * 2"), Some(json!(25)));
        assert_eq!(picked("number(/order/total)"), Some(json!(12.5)));
        assert_eq!(picked("xs:float('1.5')"), Some(json!(1.5)));
        assert_eq!(picked("xs:double('NaN')"), Some(json!("NaN")));
        assert_eq!(picked("/order/paid = 'true'"), Some(json!(true)));
        assert_eq!(picked("xs:date('2026-11-09')"), Some(json!("2026-11-09")));
        assert_eq!(picked("(1, 'a', true())"), Some(json!([1, "a", true])));
        for function in ["fn:abs#1", "array { 1, 2 }", "map { 1: 2 }"] {
            assert!(
                evaluation_error(select(function, V31, &order())).contains("no JSON value"),
                "{function}"
            );
        }
    }

    #[test]
    fn a_context_too_complex_to_parse_cheaply_is_refused_first() {
        let nested = |depth: usize| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        let refused = |xml: &str| too_complex(xml).is_some();
        assert!(!refused(&nested(MAX_DEPTH)));
        assert!(refused(&nested(MAX_DEPTH + 1)));
        assert!(
            evaluation_error(select("/a", V31, &json!(nested(MAX_DEPTH + 1))))
                .contains("more than 1024 deep")
        );
        assert_eq!(passes("/a", V31, &json!(nested(MAX_DEPTH))), Ok(true));
        // Siblings, self-closing elements, and markup that opens nothing
        // do not add depth.
        let flat = format!(
            "<?xml version=\"1.0\"?><r>{}</r>",
            "<!-- <a> --><![CDATA[<a>]]><?pi <a>?><b/><c x='/>'></c><d y=\"<a>\"/>"
                .repeat(MAX_DEPTH * 2)
        );
        assert!(!refused(&flat));
        // Space around `=` is read as XML allows it.
        assert!(!refused("<a b ='x' c= \"y\" d = 'z'/>"));
        assert_eq!(
            passes("/a/@d = 'z'", V31, &json!("<a b ='x' d = 'z'/>")),
            Ok(true)
        );
        // Markup cut short is left for the parser to refuse.
        assert!(!refused("<a><!-- unterminated"));
        assert!(!refused("<a x='unterminated"));
        // Too many attributes on one element are refused too; spread
        // across elements they cost nothing more.
        let attributes = |count: usize| {
            (0..count)
                .map(|index| format!(" a{index}='x=y'"))
                .collect::<String>()
        };
        assert!(!refused(&format!(
            "<r{}/>",
            attributes(MAX_ATTRIBUTES_PER_ELEMENT)
        )));
        assert!(
            evaluation_error(select(
                "/r",
                V31,
                &json!(format!(
                    "<r{}/>",
                    attributes(MAX_ATTRIBUTES_PER_ELEMENT + 1)
                ))
            ))
            .contains("an element with more than 256 attributes")
        );
        assert!(!refused(&format!(
            "<r>{}</r>",
            format!("<e{}/>", attributes(8)).repeat(1024)
        )));
        // So are attributes over the whole document.
        let spread = |elements: usize| {
            format!(
                "<r>{}</r>",
                format!("<e{}/>", attributes(200)).repeat(elements)
            )
        };
        assert!(!refused(&spread(MAX_ATTRIBUTES / 200)));
        assert!(
            too_complex(&spread(MAX_ATTRIBUTES / 200 + 1))
                .is_some_and(|reason| reason.contains("more than 100000 attributes"))
        );
        // And nodes: elements, comments and processing instructions.
        let leaves = |markup: &str, count: usize| format!("<r>{}</r>", markup.repeat(count));
        assert!(!refused(&leaves("<l/>", MAX_NODES - 1)));
        for markup in ["<l/>", "<!---->", "<?p?>"] {
            assert!(
                too_complex(&leaves(markup, MAX_NODES))
                    .is_some_and(|reason| reason.contains("more than 100000 elements")),
                "{markup}"
            );
        }
        // CDATA sections and end tags are not nodes of their own.
        assert!(!refused(&format!(
            "<r>{}</r>",
            "<l><![CDATA[x]]></l>".repeat(MAX_NODES - 1)
        )));
        // Namespace bindings are counted on every element they are in
        // scope on: 200 prefixes on a root over 600 children.
        let wide = format!(
            "<r {}>{}</r>",
            (0..200)
                .map(|prefix| format!("xmlns:p{prefix}='u{prefix}'"))
                .collect::<Vec<_>>()
                .join(" "),
            (0..600)
                .map(|index| format!("<p{}:n/>", index % 200))
                .collect::<String>()
        );
        assert!(
            evaluation_error(select("count(/r/*)", V31, &json!(wide)))
                .contains("binds more than 100000 namespaces")
        );
        // Out of scope, a binding counts for nothing more; rebinding a
        // prefix, or undeclaring the default namespace, is not another.
        let siblings = format!(
            "<r>{}</r>",
            "<e xmlns:p='u' xmlns:q='v'/><g/>".repeat(MAX_NAMESPACE_NODES / 2 - 1)
        );
        assert!(!refused(&siblings));
        let rebound = format!(
            "{}{}{}",
            "<e xmlns:p='u'>".repeat(MAX_DEPTH),
            "<l/>".repeat(900),
            "</e>".repeat(MAX_DEPTH)
        );
        assert!(!refused(&rebound));
        let mut bindings = Bindings::default();
        bindings.bind("", "u");
        bindings.bind("p", "v");
        assert_eq!(bindings.in_scope, 2);
        bindings.bind("", "");
        bindings.bind("p", "w");
        assert_eq!(bindings.in_scope, 1);
        bindings.unbind("");
        bindings.unbind("p");
        assert_eq!(bindings.in_scope, 2);
        bindings.unbind("unknown");
        assert_eq!(bindings.in_scope, 2);
        let bindings = |count: usize| {
            format!(
                "<r {}>{}</r>",
                (0..100)
                    .map(|prefix| format!("xmlns:p{prefix}='u'"))
                    .collect::<Vec<_>>()
                    .join(" "),
                "<l/>".repeat(count)
            )
        };
        // The root and its leaves each hold all 100 bindings.
        assert!(!refused(&bindings(MAX_NAMESPACE_NODES / 100 - 1)));
        assert!(refused(&bindings(MAX_NAMESPACE_NODES / 100)));
        // A tag this pass cannot fully read is left to the parser.
        for odd in [
            "<a b>",
            "<a b=>",
            "<a = 'x'>",
            "<a/b>",
            "<a b='x' c=\"y",
            "<a xmlns='é",
            "<a b='é",
            "<",
            "<a",
            "</",
            "</a>",
        ] {
            let _ = too_complex(odd);
        }
    }

    #[test]
    fn an_expression_calling_a_refused_function_is_refused_before_it_is_compiled() {
        for expression in [
            "count(parse-xml(/a/text())//*)",
            "fn:parse-xml-fragment('<a/>')",
            "Q{http://www.w3.org/2005/xpath-functions}parse-xml('<a/>')",
            "function-lookup(xs:QName('fn:' || 'parse-' || 'xml'), 1)('<a/>')",
            "fn:parse-xml#1('<a/>')",
            "parse-json(/a)?x",
            "json-to-xml('[1]')",
            "matches('a', string(/r))",
            "fn:replace(/a, '(a)', 'b')",
            "tokenize (: comment :) (/a, ',')",
            "Q{http://www.w3.org/2005/xpath-functions}analyze-string('a', 'a')",
            "count(distinct-values(//l))",
            "id('a b')",
            "fn:idref('x')",
            "element-with-id('x')",
            "for-each(/a, matches#2(?, 'a'))",
            "1 -matches('a', 'a')",
            "count(fold-left(1 to 50000, [], function($a, $x) { [$a] }))",
            "array:fold-right([1, 2], (), function($x, $a) { $a })",
            "/a => fold-left(0, function($a, $x) { $a })",
        ] {
            assert_eq!(
                compile(expression, V31),
                Err(SelectError::Unsupported(UNSAFE_FUNCTION)),
                "{expression}"
            );
        }
        // Names that only look like them are paths and literals.
        for harmless in [
            "/order/matches",
            "/order/id",
            "//distinct-values/@id",
            "/replacement/tokenize-me",
            "/a[@replace = 'matches(']",
            "contains(/a, \"parse-xml(\")",
            "/x:analyze-string",
            "(: matches( (: nested :) :) 1",
            "Q{matches(}a",
            "map{'Q':1}?Q",
        ] {
            assert_eq!(screened(harmless), Ok(()), "{harmless}");
        }
        // A braced URI hides nothing that follows it.
        assert_eq!(
            screened("Q{'}a | matches('a', 'b')"),
            Err(SelectError::Unsupported(UNSAFE_FUNCTION))
        );
        // The engine limits how deep an expression nests by itself.
        let nested = |depth: usize| format!("{}1{}", "(".repeat(depth), ")".repeat(depth));
        assert_eq!(compile(&nested(32), V31), Ok(()));
        assert!(matches!(
            compile(&nested(10_000), V31),
            Err(SelectError::Malformed { .. })
        ));
        // Nothing of the process is visible through the engine either.
        assert_eq!(
            select("environment-variable('PATH')", V31, &order()),
            Ok(None)
        );
        assert_eq!(
            select("available-environment-variables()", V31, &order()),
            Ok(None)
        );
        for resource in [
            "doc('file:///etc/passwd')",
            "unparsed-text('file:///etc/passwd')",
            "json-doc('file:///etc/passwd')",
            "collection()",
        ] {
            evaluation_error(select(resource, V31, &order()));
        }
    }

    #[test]
    fn a_context_too_large_is_refused_before_it_is_parsed() {
        let large = format!("<r>{}</r>", "x".repeat(MAX_CONTEXT_BYTES));
        assert!(matches!(
            passes("/r", V31, &json!(large)),
            Err(SelectError::Evaluation { message, .. }) if message.contains("larger than 4 MiB")
        ));
    }

    #[test]
    fn numerals_are_compared_by_value() {
        assert!(same_decimal("9.5367431640625e-7", "0.00000095367431640625"));
        assert!(same_decimal("25", "25.0"));
        assert!(same_decimal("1.5E+2", "150"));
        assert!(same_decimal("0", "-0.000"));
        assert!(same_decimal("-12.5", "-12.50"));
        assert!(!same_decimal(
            "1.2345678901234568e29",
            "123456789012345678901234567890"
        ));
        assert!(!same_decimal("-1", "1"));
        assert!(!same_decimal("1", "abc"));
        assert!(!same_decimal("", ""));
        assert!(!same_decimal(".", "0"));
        assert!(!same_decimal("1e", "1"));
    }

    #[test]
    fn a_default_namespace_is_matched_by_wildcard_or_braced_uri() {
        let xml = json!(r#"<o xmlns="urn:x"><a>1</a></o>"#);
        assert_eq!(select("/o/a", V31, &xml), Ok(None));
        assert_eq!(select("/*:o/*:a", V31, &xml), Ok(Some(json!("1"))));
        assert_eq!(
            select("/Q{urn:x}o/Q{urn:x}a", V31, &xml),
            Ok(Some(json!("1")))
        );
    }
}
