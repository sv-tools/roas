//! What generated validation checks, decided once for both backends.
//!
//! A schema's constraints become a list of [`Check`]s on the value they
//! constrain; each backend only spells them. Only what the type cannot
//! express is checked: presence, nullability, enum membership and unknown
//! members are the decoders' already. A check that cannot be decided
//! exactly on the mapped type is never turned into a guessed verdict: it
//! becomes [`Check::Unevaluated`], reported as such at run time.

use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity, push_token, source_of};
use crate::front::{Constraints, FloatWidth, IntWidth};
use crate::ir::{Ir, TypeKind, TypeRef};
use serde_json::Number;

/// The value kind a check applies to, after nullability and indirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    String,
    Integer(IntWidth),
    Number(FloatWidth),
    Array,
    Map,
    /// Anything else: a generated type, a substitution, raw JSON.
    Other,
}

/// How `uniqueItems` can be decided for an array's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unique {
    /// Strings, booleans, integers: equal exactly when their values are.
    Exact,
    /// Floats: unequal values came from different numbers, but equal ones
    /// may not have — a decoded float has lost its literal.
    Float,
    /// Anything else needs a JSON equality the generated code lacks.
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Check {
    MinLength(u64),
    MaxLength(u64),
    /// The smallest integer allowed, from `minimum` or `exclusiveMinimum`.
    IntAtLeast {
        bound: i128,
        keyword: &'static str,
        literal: String,
    },
    /// The largest integer allowed.
    IntAtMost {
        bound: i128,
        keyword: &'static str,
        literal: String,
    },
    /// `multipleOf` on an integer, exactly: with the multiple written as
    /// `F / 10^s`, `v * 10^s % F == 0` holds exactly when
    /// `v % (F / gcd(F, 10^s)) == 0`, so no product can overflow.
    IntMultipleOf {
        divisor: i128,
        literal: String,
    },
    /// `value >= bound` (or `>` when exclusive), compared as floats.
    NumAtLeast {
        bound: f64,
        exclusive: bool,
        literal: String,
    },
    NumAtMost {
        bound: f64,
        exclusive: bool,
        literal: String,
    },
    /// `multipleOf` on a float: decidable only when it fails for certain.
    NumMultipleOf {
        integral: bool,
        literal: String,
    },
    MinItems(u64),
    MaxItems(u64),
    UniqueItems(Unique),
    MinProperties(u64),
    MaxProperties(u64),
    /// A keyword this generator does not check; every value it applies to
    /// is reported as unevaluated rather than passed.
    Unevaluated {
        keyword: &'static str,
        reason: String,
    },
}

/// The kind of value `ty` holds once nullability and boxing are seen
/// through.
pub(crate) fn kind_of(ty: &TypeRef) -> ValueKind {
    match ty {
        TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => kind_of(inner),
        TypeRef::String => ValueKind::String,
        TypeRef::Integer(width) => ValueKind::Integer(*width),
        TypeRef::Number(width) => ValueKind::Number(*width),
        TypeRef::Array(_) => ValueKind::Array,
        TypeRef::Map(_) => ValueKind::Map,
        _ => ValueKind::Other,
    }
}

/// Whether `ty` holds a generated type that validates itself: a named
/// type that was emitted, seen through aliases, and not a substitution.
pub(crate) fn validates(
    ir: &Ir,
    emitted: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
    ty: &TypeRef,
) -> bool {
    fn named(
        ir: &Ir,
        emitted: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
        id: &crate::diagnostic::SchemaId,
        depth: usize,
    ) -> bool {
        if depth > ir.types.len() || !emitted(id) {
            return false;
        }
        match &ir.types[id].kind {
            TypeKind::Extern => false,
            // An alias has its target's implementation, or none.
            TypeKind::Alias(inner) => inner
                .direct_named()
                .is_some_and(|target| named(ir, emitted, target, depth + 1)),
            _ => true,
        }
    }
    let mut ids = Vec::new();
    ty.named(&mut ids);
    ids.iter().any(|id| named(ir, emitted, id, 0))
}

/// The checks `constraints` put on a value of type `ty`.
pub(crate) fn checks(ty: &TypeRef, constraints: &Constraints) -> Vec<Check> {
    let mut out = presence_checks(ty, constraints);
    out.extend(value_checks(ty, constraints));
    out
}

/// The checks that apply to whatever value is present, `null` included.
pub(crate) fn presence_checks(ty: &TypeRef, constraints: &Constraints) -> Vec<Check> {
    // A named type checks what it was hoisted with itself.
    if ty.direct_named().is_some() {
        return Vec::new();
    }
    unenforced(constraints)
}

/// The checks that apply to a non-null value of type `ty`.
pub(crate) fn value_checks(ty: &TypeRef, constraints: &Constraints) -> Vec<Check> {
    let c = constraints;
    let kind = kind_of(ty);
    let mut out = Vec::new();
    match kind {
        ValueKind::String => {
            out.extend(c.min_length.filter(|n| *n > 0).map(Check::MinLength));
            out.extend(c.max_length.filter(|n| *n < u64::MAX).map(Check::MaxLength));
            if c.pattern.is_some() {
                out.push(Check::Unevaluated {
                    keyword: "pattern",
                    reason: "patterns are not checked yet".into(),
                });
            }
        }
        ValueKind::Integer(_) => {
            if let Some(n) = &c.minimum {
                out.push(Check::IntAtLeast {
                    bound: int_lower(n, false),
                    keyword: "minimum",
                    literal: n.to_string(),
                });
            }
            if let Some(n) = &c.exclusive_minimum {
                out.push(Check::IntAtLeast {
                    bound: int_lower(n, true),
                    keyword: "exclusiveMinimum",
                    literal: n.to_string(),
                });
            }
            if let Some(n) = &c.maximum {
                out.push(Check::IntAtMost {
                    bound: int_upper(n, false),
                    keyword: "maximum",
                    literal: n.to_string(),
                });
            }
            if let Some(n) = &c.exclusive_maximum {
                out.push(Check::IntAtMost {
                    bound: int_upper(n, true),
                    keyword: "exclusiveMaximum",
                    literal: n.to_string(),
                });
            }
            if let Some(n) = &c.multiple_of {
                out.push(match decimal(n) {
                    Some((factor, scale)) if factor != 0 => Check::IntMultipleOf {
                        divisor: factor / gcd(factor, 10i128.pow(scale)),
                        literal: n.to_string(),
                    },
                    _ => Check::Unevaluated {
                        keyword: "multipleOf",
                        reason: format!("{n} has more digits than an exact check can hold"),
                    },
                });
            }
        }
        ValueKind::Number(_) => {
            for (n, exclusive, at_least) in [
                (&c.minimum, false, true),
                (&c.exclusive_minimum, true, true),
                (&c.maximum, false, false),
                (&c.exclusive_maximum, true, false),
            ] {
                let Some(n) = n else { continue };
                // A bound past every `f64` is kept as an infinity, which the
                // backends decide statically rather than drop.
                let bound = match n.as_f64() {
                    Some(bound) => bound,
                    None if n.to_string().starts_with('-') => f64::NEG_INFINITY,
                    None => f64::INFINITY,
                };
                let literal = n.to_string();
                out.push(if at_least {
                    Check::NumAtLeast {
                        bound,
                        exclusive,
                        literal,
                    }
                } else {
                    Check::NumAtMost {
                        bound,
                        exclusive,
                        literal,
                    }
                });
            }
            if let Some(n) = &c.multiple_of {
                out.push(Check::NumMultipleOf {
                    integral: n.is_i64() || n.is_u64(),
                    literal: n.to_string(),
                });
            }
        }
        ValueKind::Array => {
            out.extend(c.min_items.filter(|n| *n > 0).map(Check::MinItems));
            out.extend(c.max_items.filter(|n| *n < u64::MAX).map(Check::MaxItems));
            if c.unique_items {
                let item = match strip(ty) {
                    TypeRef::Array(item) => item.as_ref(),
                    _ => unreachable!("an array kind is an array"),
                };
                out.push(Check::UniqueItems(match item {
                    TypeRef::String | TypeRef::Boolean | TypeRef::Integer(_) => Unique::Exact,
                    TypeRef::Number(_) => Unique::Float,
                    _ => Unique::Unknown,
                }));
            }
        }
        ValueKind::Map => {
            out.extend(
                c.min_properties
                    .filter(|n| *n > 0)
                    .map(Check::MinProperties),
            );
            out.extend(
                c.max_properties
                    .filter(|n| *n < u64::MAX)
                    .map(Check::MaxProperties),
            );
            if c.property_names {
                out.push(Check::Unevaluated {
                    keyword: "propertyNames",
                    reason: "member-name schemas are not checked yet".into(),
                });
            }
        }
        ValueKind::Other => {}
    }
    out
}

/// The constraints on an object's own members, for a generated struct.
pub(crate) fn object_checks(constraints: &Constraints) -> Vec<Check> {
    let mut out = unenforced(constraints);
    out.extend(
        constraints
            .min_properties
            .filter(|n| *n > 0)
            .map(Check::MinProperties),
    );
    out.extend(
        constraints
            .max_properties
            .filter(|n| *n < u64::MAX)
            .map(Check::MaxProperties),
    );
    if constraints.property_names {
        out.push(Check::Unevaluated {
            keyword: "propertyNames",
            reason: "member-name schemas are not checked yet".into(),
        });
    }
    out
}

/// Whether decoding a branch proves nothing: it declares what neither its
/// type nor a generated check enforces.
pub(crate) fn unsure(branch: &crate::ir::Branch) -> bool {
    branch.unchecked || !unchecked_keywords(&branch.ty, &branch.constraints).is_empty()
}

/// Whether an allOf part needs a finding of its own for what goes
/// unchecked: its own checks already report one where they can.
pub(crate) fn part_unsure(branch: &crate::ir::Branch) -> bool {
    // Only what is reported for every value, null included, stands in
    // for it.
    unsure(branch) && presence_checks(&branch.ty, &branch.constraints).is_empty()
}

/// What the constraints declare that nothing checks yet: a decoded value
/// is never taken to satisfy it.
pub(crate) fn unenforced(c: &Constraints) -> Vec<Check> {
    c.unenforced
        .iter()
        .map(|&keyword| Check::Unevaluated {
            keyword,
            reason: match keyword {
                "enum" => "numeric enums are not checked yet".into(),
                _ => format!("`{keyword}` is not checked yet"),
            },
        })
        .collect()
}

/// Whether the constraints say anything a check above does not cover for
/// a value of this kind: they are then reported at generation time.
pub(crate) fn unchecked_keywords(ty: &TypeRef, c: &Constraints) -> Vec<&'static str> {
    // A named type checks the constraints it was hoisted with itself.
    if ty.direct_named().is_some() {
        return Vec::new();
    }
    let kind = kind_of(ty);
    let mut out = Vec::new();
    let string = matches!(kind, ValueKind::String);
    let number = matches!(kind, ValueKind::Integer(_) | ValueKind::Number(_));
    let array = matches!(kind, ValueKind::Array);
    let map = matches!(kind, ValueKind::Map);
    for (present, keyword, applies) in [
        (c.min_length.is_some(), "minLength", string),
        (c.max_length.is_some(), "maxLength", string),
        (c.pattern.is_some(), "pattern", string),
        (c.minimum.is_some(), "minimum", number),
        (c.maximum.is_some(), "maximum", number),
        (c.exclusive_minimum.is_some(), "exclusiveMinimum", number),
        (c.exclusive_maximum.is_some(), "exclusiveMaximum", number),
        (c.multiple_of.is_some(), "multipleOf", number),
        (c.min_items.is_some(), "minItems", array),
        (c.max_items.is_some(), "maxItems", array),
        (c.unique_items, "uniqueItems", array),
        (c.min_properties.is_some(), "minProperties", map),
        (c.max_properties.is_some(), "maxProperties", map),
        (c.property_names, "propertyNames", map),
    ] {
        if present && !applies {
            out.push(keyword);
        }
    }
    out
}

/// With validation on, a warning for every constraint no check covers, so
/// nothing a schema says is dropped without a word.
pub(crate) fn diagnostics(ir: &Ir) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut warn = |id: &crate::diagnostic::SchemaId, pointer: String, keyword: &str| {
        out.push(Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::Unvalidated {
                keyword: keyword.to_owned(),
            },
            schema_id: id.clone(),
            pointer: source_of(&pointer),
            message: format!(
                "`{keyword}` does not apply to a value of this type, so validation does not check it"
            ),
        });
    };
    for id in &ir.order {
        if ir.suppressed.contains(id) {
            continue;
        }
        let def = &ir.types[id];
        match &def.kind {
            TypeKind::Struct(s) => {
                for field in &s.fields {
                    let mut pointer = def.id.pointer.clone();
                    push_token(&mut pointer, "properties");
                    push_token(&mut pointer, &field.raw_name);
                    for keyword in unchecked_keywords(&field.ty, &field.constraints) {
                        warn(&def.id, format!("{pointer}/{keyword}"), keyword);
                    }
                }
            }
            TypeKind::Newtype(inner) => {
                for keyword in unchecked_keywords(inner, &def.constraints) {
                    warn(&def.id, format!("{}/{keyword}", def.id.pointer), keyword);
                }
            }
            TypeKind::UnionOpen(crate::ir::OpenUnion { branches, .. })
            | TypeKind::ComposedOpen(branches) => {
                for branch in branches {
                    for keyword in unchecked_keywords(&branch.ty, &branch.constraints) {
                        warn(&def.id, format!("{}/{keyword}", def.id.pointer), keyword);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether validating `ty`, a branch of the raw holder `holder`, would
/// come back to `holder` with the same raw value: a composition that
/// refers to itself without consuming anything never finishes.
pub(crate) fn loops_back(ir: &Ir, holder: &crate::diagnostic::SchemaId, ty: &TypeRef) -> bool {
    fn raw_holder<'a>(ir: &'a Ir, ty: &'a TypeRef) -> Option<&'a crate::diagnostic::SchemaId> {
        let mut id = ty.direct_named()?;
        for _ in 0..=ir.types.len() {
            match &ir.types.get(id)?.kind {
                TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_) => return Some(id),
                TypeKind::Alias(inner) | TypeKind::Newtype(inner) => id = inner.direct_named()?,
                _ => return None,
            }
        }
        None
    }
    let mut stack: Vec<&crate::diagnostic::SchemaId> = raw_holder(ir, ty).into_iter().collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = stack.pop() {
        if id == holder {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        let branches = match &ir.types[id].kind {
            TypeKind::UnionOpen(u) => &u.branches,
            TypeKind::ComposedOpen(b) => b,
            _ => continue,
        };
        stack.extend(branches.iter().filter_map(|b| raw_holder(ir, &b.ty)));
    }
    false
}

/// Every constraint keyword `c` declares.
pub(crate) fn present_keywords(c: &Constraints) -> Vec<&'static str> {
    [
        (c.min_length.is_some(), "minLength"),
        (c.max_length.is_some(), "maxLength"),
        (c.pattern.is_some(), "pattern"),
        (c.minimum.is_some(), "minimum"),
        (c.maximum.is_some(), "maximum"),
        (c.exclusive_minimum.is_some(), "exclusiveMinimum"),
        (c.exclusive_maximum.is_some(), "exclusiveMaximum"),
        (c.multiple_of.is_some(), "multipleOf"),
        (c.min_items.is_some(), "minItems"),
        (c.max_items.is_some(), "maxItems"),
        (c.unique_items, "uniqueItems"),
        (c.min_properties.is_some(), "minProperties"),
        (c.max_properties.is_some(), "maxProperties"),
        (c.property_names, "propertyNames"),
    ]
    .into_iter()
    .filter_map(|(present, keyword)| present.then_some(keyword))
    .chain(c.unenforced.iter().copied())
    .collect()
}

fn strip(ty: &TypeRef) -> &TypeRef {
    match ty {
        TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => strip(inner),
        other => other,
    }
}

/// Far enough past any `i64` that a bound beyond it decides every value.
const SATURATE: i128 = 1 << 70;

/// A number as written: sign, significant digits, and the power of ten
/// they are scaled by. Read from the number's text, which keeps every
/// digit when the description was read exactly.
struct Decimal {
    negative: bool,
    digits: String,
    exponent: i64,
}

fn parse_decimal(n: &Number) -> Option<Decimal> {
    let text = n.to_string();
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.to_owned()),
        None => (false, text),
    };
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        // An exponent past `i64` saturates: the bound is past every value
        // either way, never zero.
        Some((m, e)) => (
            m.to_owned(),
            e.trim_start_matches('+')
                .parse::<i128>()
                .ok()?
                .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64,
        ),
        None => (text, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((&mantissa, ""));
    let exponent = exponent.saturating_sub(i64::try_from(fraction.len()).ok()?);
    let digits = format!("{whole}{fraction}")
        .trim_start_matches('0')
        .to_owned();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Trailing zeros move into the exponent, so `2.50` is `25e-1`.
    let trimmed = digits.trim_end_matches('0');
    let exponent = exponent.saturating_add((digits.len() - trimmed.len()) as i64);
    Some(Decimal {
        negative,
        digits: trimmed.to_owned(),
        exponent,
    })
}

/// The floor and ceiling of `n`, saturated to `±SATURATE`.
fn floor_ceil(n: &Number) -> (i128, i128) {
    let Some(d) = parse_decimal(n) else {
        return (0, 0);
    };
    if d.digits.is_empty() {
        return (0, 0);
    }
    let len = d.digits.len() as i64;
    let (whole, fractional) = if d.exponent >= 0 {
        if len.saturating_add(d.exponent) > 30 {
            let edge = if d.negative { -SATURATE } else { SATURATE };
            return (edge, edge);
        }
        let zeros = "0".repeat(d.exponent as usize);
        (format!("{}{zeros}", d.digits), false)
    } else {
        let point = len.saturating_add(d.exponent);
        if point > 30 {
            let edge = if d.negative { -SATURATE } else { SATURATE };
            return (edge, edge);
        }
        let whole = if point > 0 {
            d.digits[..point as usize].to_owned()
        } else {
            String::new()
        };
        // Trailing zeros were trimmed, so any digit after the point is a
        // fraction.
        (whole, true)
    };
    let magnitude: i128 = if whole.is_empty() {
        0
    } else {
        whole.parse().unwrap_or(SATURATE)
    };
    let step = i128::from(fractional);
    if d.negative {
        (-magnitude - step, -magnitude)
    } else {
        (magnitude, magnitude + step)
    }
}

/// The smallest integer satisfying `>= n` (or `> n`).
fn int_lower(n: &Number, exclusive: bool) -> i128 {
    let (floor, ceil) = floor_ceil(n);
    if exclusive { floor + 1 } else { ceil }
}

/// The largest integer satisfying `<= n` (or `< n`).
fn int_upper(n: &Number, exclusive: bool) -> i128 {
    let (floor, ceil) = floor_ceil(n);
    if exclusive { ceil - 1 } else { floor }
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A number's magnitude as `digits / 10^scale`, exactly as written. `None`
/// when it does not fit the `i128` arithmetic the check is done in.
pub(crate) fn decimal(n: &Number) -> Option<(i128, u32)> {
    let d = parse_decimal(n)?;
    if d.digits.is_empty() {
        return Some((0, 0));
    }
    let digits: i128 = d.digits.parse().ok()?;
    if d.exponent >= 0 {
        let factor = 10i128.checked_pow(u32::try_from(d.exponent).ok()?)?;
        Some((digits.checked_mul(factor)?, 0))
    } else {
        let scale = u32::try_from(-d.exponent).ok()?;
        (scale <= 38).then_some((digits, scale))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(text: &str) -> Number {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn integer_bounds_round_inwards() {
        assert_eq!(int_lower(&n("5"), false), 5);
        assert_eq!(int_lower(&n("5"), true), 6);
        assert_eq!(int_lower(&n("1.5"), false), 2);
        assert_eq!(int_lower(&n("1.5"), true), 2);
        assert_eq!(int_lower(&n("2.0"), true), 3);
        assert_eq!(int_upper(&n("1.5"), false), 1);
        assert_eq!(int_upper(&n("1.5"), true), 1);
        assert_eq!(int_upper(&n("-3"), true), -4);
        assert_eq!(int_lower(&n("1e300"), false), SATURATE);
        assert_eq!(int_upper(&n("-1e300"), false), -SATURATE);
    }

    #[test]
    fn multiples_are_exact_decimals() {
        assert_eq!(decimal(&n("3")), Some((3, 0)));
        assert_eq!(decimal(&n("0.5")), Some((5, 1)));
        assert_eq!(decimal(&n("0.01")), Some((1, 2)));
        assert_eq!(decimal(&n("2.50")), Some((25, 1)));
        assert_eq!(decimal(&n("1e-7")), Some((1, 7)));
        assert_eq!(decimal(&n("1e300")), None);
        // v * 10^s % F == 0  <=>  v % (F / gcd(F, 10^s)) == 0
        let divisor = |text: &str| match &checks(
            &TypeRef::Integer(IntWidth::Int64),
            &Constraints {
                multiple_of: Some(n(text)),
                ..Default::default()
            },
        )[0]
        {
            Check::IntMultipleOf { divisor, .. } => *divisor,
            other => panic!("{other:?}"),
        };
        assert_eq!(divisor("6"), 6);
        assert_eq!(divisor("0.5"), 1);
        assert_eq!(divisor("2.5"), 5);
        assert_eq!(divisor("0.03"), 3);
    }

    // Only an exact build keeps such literals as written.
    #[cfg(feature = "exact-numbers")]
    #[test]
    fn exact_literals_are_read_as_written() {
        assert_eq!(int_lower(&n("1e9223372036854775807"), false), SATURATE);
        assert_eq!(int_lower(&n("1e9223372036854775808"), false), SATURATE);
        assert_eq!(int_upper(&n("-1e9223372036854775807"), false), -SATURATE);
        assert_eq!(int_upper(&n("9007199254740993.5"), false), 9007199254740993);
        let divisor = match &checks(
            &TypeRef::Integer(IntWidth::Int64),
            &Constraints {
                multiple_of: Some(n("0.5000000000000000000001")),
                ..Default::default()
            },
        )[0]
        {
            Check::IntMultipleOf { divisor, .. } => *divisor,
            other => panic!("{other:?}"),
        };
        assert_eq!(divisor, 5000000000000000000001);
        let bound = match &checks(
            &TypeRef::Number(FloatWidth::Double),
            &Constraints {
                minimum: Some(n("1e400")),
                ..Default::default()
            },
        )[0]
        {
            Check::NumAtLeast { bound, .. } => *bound,
            other => panic!("{other:?}"),
        };
        assert_eq!(bound, f64::INFINITY);
    }
}
