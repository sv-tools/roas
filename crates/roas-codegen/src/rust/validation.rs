//! Spelling [`Check`]s as Rust: the body of each generated type's
//! `Validate::validate_into`.

use crate::checks::{self, Check, Unique, ValueKind};
use crate::front::{FloatWidth, IntWidth};
use crate::ir::{Additional, Branch, TypeDef, TypeKind, TypeRef};
use crate::rust::view::TypeView;

thread_local! {
    /// Whether a regex was rendered since the last [`take_uses_regex`] —
    /// a `pattern` check, or patterns checked at decode: the generated code
    /// then needs `regex`.
    static USES_REGEX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The generated code compiles a regex, so it needs `regex`.
pub(crate) fn mark_uses_regex() {
    USES_REGEX.with(|flag| flag.set(true));
}

/// Whether a regex was rendered since the last call; resets it.
pub(crate) fn take_uses_regex() -> bool {
    USES_REGEX.with(|flag| flag.replace(false))
}

/// Where a check reads and reports: the value (a `&T`, parenthesised),
/// the pointer (a `&str`) and the report (a `&mut ValidationReport`).
#[derive(Clone, Copy)]
struct At<'a> {
    v: &'a str,
    p: &'a str,
    r: &'a str,
}

/// A float bound compared at the value's own width, so a value that
/// rounds to exactly the bound is not pushed past it by a widening.
/// `Err` decides the comparison statically, for a bound beyond the width.
fn float_bound(bound: f64, width: FloatWidth) -> Result<String, f64> {
    match width {
        FloatWidth::Double if bound.is_finite() => Ok(format!("{bound:?}")),
        FloatWidth::Double => Err(bound),
        FloatWidth::Float => {
            let narrowed = bound as f32;
            if narrowed.is_finite() {
                Ok(format!("{narrowed:?}_f32"))
            } else {
                Err(bound)
            }
        }
    }
}

/// An exact decimal as a `Decimal` constant.
fn decimal(d: &checks::Dec) -> String {
    format!(
        "Decimal {{ mantissa: {}, exponent: {} }}",
        d.mantissa, d.exponent
    )
}

/// Statements checking `list` at `at`. True when they read the value: some
/// checks only report that they could not be made.
fn render(list: &[Check], kind: ValueKind, at: At<'_>, out: &mut Vec<String>) -> bool {
    let At { v, p, r } = at;
    let int = format!("i128::from({v}.0)");
    // Some checks only report that they could not be made, and a bound
    // beyond every value emits nothing: the value is read only when a
    // check that reads it emitted something.
    let mut reads = false;
    for check in list {
        let before = out.len();
        match check {
            Check::MinLength(n) => out.push(format!(
                "{{ let n = {v}.chars().count(); if (n as u64) < {n} {{ {r}.violation({p}, format!(\"is {{n}} characters long, shorter than minLength {n}\")); }} }}"
            )),
            Check::MaxLength(n) => out.push(format!(
                "{{ let n = {v}.chars().count(); if (n as u64) > {n} {{ {r}.violation({p}, format!(\"is {{n}} characters long, longer than maxLength {n}\")); }} }}"
            )),
            Check::Pattern { regex, source } => {
                USES_REGEX.with(|flag| flag.set(true));
                // An argument needs no parentheses, and rustc says so.
                let arg = v.strip_prefix('(').and_then(|v| v.strip_suffix(')')).unwrap_or(v);
                // Compiled once, where it is used. A pattern past what the
                // engine compiles (its size limit) is not checked, not failed.
                out.push(format!(
                    "{{ static RE: std::sync::LazyLock<Option<regex::Regex>> = std::sync::LazyLock::new(|| regex::Regex::new({regex:?}).ok()); match RE.as_ref() {{ Some(re) => {{ if !re.is_match({arg}) {{ {r}.violation({p}, {:?}); }} }} None => {r}.unevaluated({p}, {:?}), }} }}",
                    format!("does not match pattern {source:?}"),
                    format!("pattern {source:?}: too large to compile here"),
                ))
            }
            Check::IntAtLeast {
                bound,
                keyword,
                literal,
            } => {
                let floor = match kind {
                    ValueKind::Integer(IntWidth::Int32) => i128::from(i32::MIN),
                    _ => i128::from(i64::MIN),
                };
                if *bound > floor {
                    out.push(format!(
                        "if {int} < {bound} {{ {r}.violation({p}, format!(\"{{}} is below {keyword} {literal}\", {v}.0)); }}"
                    ));
                }
            }
            Check::IntAtMost {
                bound,
                keyword,
                literal,
            } => {
                let ceiling = match kind {
                    ValueKind::Integer(IntWidth::Int32) => i128::from(i32::MAX),
                    _ => i128::from(i64::MAX),
                };
                if *bound < ceiling {
                    out.push(format!(
                        "if {int} > {bound} {{ {r}.violation({p}, format!(\"{{}} is above {keyword} {literal}\", {v}.0)); }}"
                    ));
                }
            }
            Check::IntMultipleOf { divisor, literal } => {
                if *divisor != 1 {
                    out.push(format!(
                        "if {int} % {divisor} != 0 {{ {r}.violation({p}, format!(\"{{}} is not a multiple of {literal}\", {v}.0)); }}"
                    ));
                }
            }
            Check::NumAtLeast {
                bound,
                exclusive,
                literal,
            }
            | Check::NumAtMost {
                bound,
                exclusive,
                literal,
            } => {
                let at_least = matches!(check, Check::NumAtLeast { .. });
                let (op, keyword, word) = checks::bound_words(at_least, *exclusive);
                let width = match kind {
                    ValueKind::Number(width) => width,
                    _ => FloatWidth::Double,
                };
                let value = match width {
                    FloatWidth::Double => format!("f64::from(*{v})"),
                    FloatWidth::Float => format!("*{v}"),
                };
                let violation = format!(
                    "{r}.violation({p}, format!(\"{{}} is {word} {keyword} {literal}\", {v}))"
                );
                // A `float` equal to the bound is ambiguous: literals on
                // either side of the bound decode to it. (A `double` is
                // decided as far as an f64 holds it.)
                let inexact = matches!(width, FloatWidth::Float);
                match float_bound(*bound, width) {
                    // Either float32 around the bound may be where a
                    // literal at the bound lands.
                    Ok(_) if inexact => {
                        let (below, above) = checks::f32_neighbours(*bound);
                        let at = if below == above {
                            format!("{value} == {below:?}_f32")
                        } else {
                            format!("{value} == {below:?}_f32 || {value} == {above:?}_f32")
                        };
                        out.push(format!(
                            "if {at} {{ {r}.unevaluated({p}, \"{keyword} {literal}: the value rounds to the same float as the bound\"); }} else if !(f64::from({value}) {op} {bound:?}) {{ {violation}; }}"
                        ))
                    }
                    Ok(bound) => out.push(format!(
                        "if !({value} {op} {bound}) {{ {violation}; }}"
                    )),
                    // Past every float32 but not every f64: compared wide,
                    // so a value decoded as an infinity is still caught.
                    // Past every float32 but not every f64: an infinity
                    // came from some literal past the float range, on
                    // either side of the bound.
                    // The largest float32 is where a literal at or past the
                    // bound lands: ambiguous too.
                    Err(bound) if bound.is_finite() => {
                        let edge = if bound > 0.0 { "f32::MAX" } else { "f32::MIN" };
                        out.push(format!(
                            "if {v}.is_infinite() || *{v} == {edge} {{ {r}.unevaluated({p}, \"{keyword} {literal}: the value is at the edge of the float range\"); }} else if !(f64::from(*{v}) {op} {bound:?}) {{ {violation}; }}"
                        ))
                    }
                    // Beyond every value: always or never met.
                    Err(bound) if (bound > 0.0) == at_least => out.push(format!("{violation};")),
                    Err(_) => {}
                }
            }
            Check::NumberIn {
                values,
                keyword,
                literal,
            } => {
                let what = if *keyword == "const" {
                    format!("the const {literal}")
                } else {
                    format!("one of the enum values {literal}")
                };
                match kind {
                    ValueKind::Integer(_) => {
                        // Exactly, as integers; a fraction is no integer's value.
                        // One written as a float past 2^53, read as one,
                        // stands for every integer that rounds to it: equal
                        // to it once rounded is undecided.
                        let mut held: Vec<i128> = values
                            .iter()
                            .filter(|n| checks::integer_held(n))
                            .filter_map(checks::exact_integer)
                            .collect();
                        held.sort_unstable();
                        held.dedup();
                        let mut unheld: Vec<f64> = values
                            .iter()
                            .filter(|n| !checks::integer_held(n))
                            .filter_map(serde_json::Number::as_f64)
                            .filter(|f| f.is_finite() && f.fract() == 0.0)
                            .collect();
                        unheld.sort_by(f64::total_cmp);
                        unheld.dedup();
                        let violation = format!(
                            "{r}.violation({p}, format!(\"{{}} is not {what}\", {v}.0));"
                        );
                        let missed = if unheld.is_empty() {
                            violation
                        } else {
                            let list = unheld
                                .iter()
                                .map(|f| format!("{f:?}"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!(
                                "if [{list}].contains(&({v}.0 as f64)) {{ {r}.unevaluated({p}, {:?}); }} else {{ {violation} }}",
                                format!("{keyword} {literal}: a listed value was read as a float, which does not tell the integers near it apart")
                            )
                        };
                        out.push(if held.is_empty() {
                            missed
                        } else {
                            let list =
                                held.iter().map(i128::to_string).collect::<Vec<_>>().join(", ");
                            format!("if ![{list}].contains(&{int}) {{ {missed} }}")
                        });
                    }
                    ValueKind::Decimal => {
                        // Exactly.
                        let mut set: Vec<checks::Dec> =
                            values.iter().filter_map(checks::dec).collect();
                        set.dedup();
                        // A listed value no decimal holds is no value's.
                        let violation = format!(
                            "{r}.violation({p}, format!(\"{{}} is not {what}\", {v}));"
                        );
                        out.push(if set.is_empty() {
                            violation
                        } else {
                            let list = set.iter().map(decimal).collect::<Vec<_>>().join(", ");
                            format!("if ![{list}].contains({v}) {{ {violation} }}")
                        });
                    }
                    ValueKind::Number(FloatWidth::Float) => {
                        // A `float` equal to a value is ambiguous: literals
                        // around it decode to it, as at a bound.
                        let mut set: Vec<f32> = values
                            .iter()
                            .filter_map(serde_json::Number::as_f64)
                            .filter(|f| f.is_finite())
                            .flat_map(checks::f32_candidates)
                            .filter(|f| f.is_finite())
                            .collect();
                        set.sort_by(f32::total_cmp);
                        set.dedup();
                        let violation = format!(
                            "{r}.violation({p}, format!(\"{{}} is not {what}\", {v}))"
                        );
                        out.push(if set.is_empty() {
                            format!("{violation};")
                        } else {
                            let at = set
                                .iter()
                                .map(|f| format!("*{v} == {f:?}_f32"))
                                .collect::<Vec<_>>()
                                .join(" || ");
                            format!(
                                "if {at} {{ {r}.unevaluated({p}, {:?}); }} else {{ {violation}; }}",
                                format!("{keyword} {literal}: the value rounds to the same float as a listed value")
                            )
                        });
                    }
                    _ => {
                        // Decided as far as an `f64` holds the value; a listed
                        // value no float holds as written leaves a value equal
                        // to its float undecided.
                        let floats = |exact: bool| {
                            let mut set: Vec<f64> = values
                                .iter()
                                .filter(|n| checks::f64_holds(n) == exact)
                                .filter_map(serde_json::Number::as_f64)
                                .filter(|f| f.is_finite())
                                .collect();
                            set.sort_by(f64::total_cmp);
                            set.dedup();
                            set
                        };
                        let any = |set: &[f64]| {
                            set.iter()
                                .map(|f| format!("f64::from(*{v}) == {f:?}"))
                                .collect::<Vec<_>>()
                                .join(" || ")
                        };
                        let (exact, inexact) = (floats(true), floats(false));
                        let violation = format!(
                            "{r}.violation({p}, format!(\"{{}} is not {what}\", {v}));"
                        );
                        // A value equal to a listed value held exactly is
                        // decided first; only then one no float holds.
                        let missed = if inexact.is_empty() {
                            violation
                        } else {
                            format!(
                                "if {} {{ {r}.unevaluated({p}, {:?}); }} else {{ {violation} }}",
                                any(&inexact),
                                format!("{keyword} {literal}: a listed value is not exactly a float, and the value is its float")
                            )
                        };
                        out.push(if exact.is_empty() {
                            missed
                        } else {
                            format!("if !({}) {{ {missed} }}", any(&exact))
                        });
                    }
                }
            }
            Check::DecAtLeast {
                bound,
                exclusive,
                literal,
            }
            | Check::DecAtMost {
                bound,
                exclusive,
                literal,
            } => {
                let at_least = matches!(check, Check::DecAtLeast { .. });
                let (op, keyword, word) = checks::bound_words(at_least, *exclusive);
                out.push(format!(
                    "if !(*{v} {op} {}) {{ {r}.violation({p}, format!(\"{{}} is {word} {keyword} {literal}\", {v})); }}",
                    decimal(bound)
                ));
            }
            Check::DecOutside {
                at_least,
                exclusive,
                literal,
            } => {
                let (_, keyword, word) = checks::bound_words(*at_least, *exclusive);
                out.push(format!(
                    "{r}.violation({p}, format!(\"{{}} is {word} {keyword} {literal}\", {v}));"
                ));
            }
            Check::DecMultipleOf { divisor, literal } => out.push(format!(
                "if !{v}.is_multiple_of(&{}) {{ {r}.violation({p}, format!(\"{{}} is not a multiple of {literal}\", {v})); }}",
                decimal(divisor)
            )),
            Check::NumMultipleOf { integral, literal } => {
                let unevaluated = format!(
                    "{r}.unevaluated({p}, \"multipleOf {literal}: a decoded float has lost its literal, so only a fraction against a whole multiple is decided\")"
                );
                if *integral {
                    // A fraction never came from a whole-number literal, so
                    // it is no multiple of a whole number; anything else
                    // may have been rounded on the way in.
                    out.push(format!(
                        "if f64::from(*{v}).is_finite() && f64::from(*{v}).fract() != 0.0 {{ {r}.violation({p}, format!(\"{{}} is not a multiple of {literal}\", {v})); }} else {{ {unevaluated}; }}"
                    ));
                } else {
                    out.push(format!("{unevaluated};"));
                }
            }
            Check::MinItems(n) => out.push(format!(
                "if ({v}.len() as u64) < {n} {{ {r}.violation({p}, format!(\"has {{}} items, fewer than minItems {n}\", {v}.len())); }}"
            )),
            Check::MaxItems(n) => out.push(format!(
                "if ({v}.len() as u64) > {n} {{ {r}.violation({p}, format!(\"has {{}} items, more than maxItems {n}\", {v}.len())); }}"
            )),
            Check::UniqueItems(Unique::Exact) => out.push(format!(
                "{{ let mut seen = std::collections::BTreeSet::new(); for (i, item) in {v}.iter().enumerate() {{ if !seen.insert(item) {{ {r}.violation(&child_pointer({p}, &i.to_string()), \"repeats an earlier item; uniqueItems is true\"); }} }} }}"
            )),
            Check::UniqueItems(Unique::Float) => out.push(format!(
                "{{ let mut sorted: Vec<f64> = {v}.iter().map(|x| f64::from(*x)).collect(); sorted.sort_by(f64::total_cmp); if sorted.windows(2).any(|w| w[0] == w[1]) {{ {r}.unevaluated({p}, \"uniqueItems: equal floats may come from different numbers\"); }} }}"
            )),
            Check::UniqueItems(Unique::Unknown) => out.push(format!(
                "if {v}.len() > 1 {{ {r}.unevaluated({p}, \"uniqueItems: items of this type are not compared yet\"); }}"
            )),
            Check::MinProperties(n) => out.push(format!(
                "if ({v}.len() as u64) < {n} {{ {r}.violation({p}, format!(\"has {{}} members, fewer than minProperties {n}\", {v}.len())); }}"
            )),
            Check::MaxProperties(n) => out.push(format!(
                "if ({v}.len() as u64) > {n} {{ {r}.violation({p}, format!(\"has {{}} members, more than maxProperties {n}\", {v}.len())); }}"
            )),
            Check::Unevaluated { keyword, reason } => out.push(format!(
                "{r}.unevaluated({p}, {:?});",
                format!("{keyword}: {reason}")
            )),
            Check::Unsupported(keywords) => out.push(format!(
                "{r}.unevaluated({p}, {:?});",
                format!("{}: not handled by the generator", keywords.join(", "))
            )),
            Check::Each {
                item,
                presence,
                value: list,
            } => {
                let at = At {
                    v: "item",
                    p: "&p",
                    r,
                };
                let mut body = Vec::new();
                let mut reads = render(presence, ValueKind::Other, at, &mut body);
                reads |= value(item, list, at, &mut body);
                if !body.is_empty() {
                    let binding = if reads { "item" } else { "_" };
                    let body = body.join(" ");
                    out.push(if kind == ValueKind::Map {
                        format!(
                            "for (k, {binding}) in {v}.iter() {{ let p = child_pointer({p}, k); {body} }}"
                        )
                    } else {
                        format!(
                            "for (i, {binding}) in {v}.iter().enumerate() {{ let p = child_pointer({p}, &i.to_string()); {body} }}"
                        )
                    });
                }
            }
            Check::Keys(list) => {
                let mut body = Vec::new();
                render(
                    list,
                    ValueKind::String,
                    At {
                        v: "k",
                        p: "&p",
                        r,
                    },
                    &mut body,
                );
                if !body.is_empty() {
                    out.push(format!(
                        "for k in {v}.keys() {{ let p = child_pointer({p}, k); {} }}",
                        body.join(" ")
                    ));
                }
            }
            Check::ItemsUnevaluated { keyword, reason } => out.push(format!(
                "if !{v}.is_empty() {{ {r}.unevaluated({p}, {:?}); }}",
                format!("{keyword} on the items: {reason}")
            )),
        }
        reads |= out.len() > before
            && !matches!(
                check,
                Check::Unevaluated { .. }
                    | Check::Unsupported(_)
                    | Check::NumMultipleOf {
                        integral: false,
                        ..
                    }
            );
    }
    reads
}

/// Checks on a value position: nullability seen through, then the
/// constraints.
fn value(ty: &TypeRef, list: &[Check], at: At<'_>, out: &mut Vec<String>) -> bool {
    if list.is_empty() {
        return false;
    }
    match ty {
        TypeRef::Nullable(inner) => {
            let mut body = Vec::new();
            if value(inner, list, At { v: "v", ..at }, &mut body) {
                // A scrutinee needs no parentheses, and rustc says so.
                let scrutinee =
                    at.v.strip_prefix('(')
                        .and_then(|v| v.strip_suffix(')'))
                        .unwrap_or(at.v);
                out.push(format!(
                    "if let Some(v) = {scrutinee} {{ {} }}",
                    body.join(" ")
                ));
            } else if body.is_empty() {
                // Every check was decided away.
                return false;
            } else {
                out.push(format!("if {}.is_some() {{ {} }}", at.v, body.join(" ")));
            }
            true
        }
        TypeRef::Boxed(inner) => value(inner, list, at, out),
        _ => render(list, checks::kind_of(ty), at, out),
    }
}

/// The checks and recursion on one branch's decoded value `v` (owned),
/// into the report `r` (passed on as `arg`). Empty when the branch is
/// only decoded.
fn branch(
    ir: &crate::ir::Ir,
    branch: &Branch,
    generated: &dyn Fn(&TypeRef) -> bool,
    (r, arg): (&str, &str),
) -> (Vec<String>, bool) {
    // What applies to null as much as to a value is checked outside the
    // null guard.
    let ext = checks::extern_checks(ir, &branch.ty, false);
    let mut body = Vec::new();
    render(
        &[
            checks::presence_checks(&branch.ty, &branch.constraints),
            ext.presence,
        ]
        .concat(),
        ValueKind::Other,
        At {
            v: "(&v)",
            p: "pointer",
            r,
        },
        &mut body,
    );
    let mut list = checks::value_checks(&branch.ty, &branch.constraints);
    list.extend(ext.value);
    let mut reads = value(
        &branch.ty,
        &list,
        At {
            v: "(&v)",
            p: "pointer",
            r,
        },
        &mut body,
    );
    if generated(&branch.ty) {
        body.push(format!("v.validate_into(pointer, {arg});"));
        reads = true;
    }
    (body, reads)
}

/// A holder whose schema allows `null` (`type: [..., "null"]`) carries it
/// at its use sites, but decoded directly it holds a raw `null`, which is
/// valid.
const NULL_IS_VALID: &str = "if self.raw.get().trim() == \"null\" { return; }";

/// Whether a field can be read under a `serde(alias)`. An alias this cannot
/// read counts: the attribute is on the field either way.
fn aliased(field: &super::view::FieldView) -> bool {
    !matches!(super::attrs::serde_aliases(&field.attrs, ""), Ok(a) if a.is_empty())
}

/// When decoding `ty` may fail for the decoder rather than the value, the
/// Rust condition on the holder's raw value under which a failure proves
/// nothing: always for a substitution, which may take any value; for a
/// struct Rust reads by other names than the schema, only when the value
/// is of the JSON kind the type reads, since no other kind could match.
fn decoder_rules(
    ir: &crate::ir::Ir,
    ty: &TypeRef,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
    raw: &str,
) -> Option<String> {
    if checks::reaches_extern(ir, ty) {
        return Some("true".to_owned());
    }
    if !checks::reaches_decoder_rules(ir, ty, reshaped) {
        return None;
    }
    fn opens(ir: &crate::ir::Ir, ty: &TypeRef, depth: usize) -> Option<char> {
        match ty {
            TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => opens(ir, inner, depth),
            TypeRef::Array(_) => Some('['),
            TypeRef::Map(_) => Some('{'),
            TypeRef::Named(id) if depth < 64 => match ir.types.get(id).map(|d| &d.kind) {
                Some(TypeKind::Struct(_) | TypeKind::UnionTagged(_)) => Some('{'),
                Some(TypeKind::Newtype(t) | TypeKind::Alias(t)) => opens(ir, t, depth + 1),
                _ => None,
            },
            _ => None,
        }
    }
    Some(match opens(ir, ty, 0) {
        Some(c) => format!("{raw}.trim_start().starts_with('{c}')"),
        None => "true".to_owned(),
    })
}

/// Whether a holder's raw value has a number its branches' types may fail
/// to decode for their range: such a failure proves no mismatch.
fn rust_wide<'a>(ir: &crate::ir::Ir, tys: impl Iterator<Item = &'a TypeRef>) -> String {
    rust_wide_of(ir, tys, "self.raw.get()")
}

/// [`rust_wide`] over the raw JSON text `raw`.
fn rust_wide_of<'a>(
    ir: &crate::ir::Ir,
    tys: impl Iterator<Item = &'a TypeRef>,
    raw: &str,
) -> String {
    let tys: Vec<&TypeRef> = tys.collect();
    let (limit, float32) = checks::number_range(ir, &tys);
    format!(
        "wide_number({raw}, {}, {float32})",
        limit.unwrap_or("f64::INFINITY")
    )
}

/// The checks on one member a struct holds raw beside `patternProperties`,
/// its name in `name` and its text in `raw`, at `pointer`: against the
/// schema of every pattern it matches, or of `additionalProperties` when
/// it matches none.
fn pattern_members(
    ir: &crate::ir::Ir,
    s: &crate::ir::StructDef,
    view: &TypeView,
    generated: &dyn Fn(&TypeRef) -> bool,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> Vec<String> {
    // A pattern that cannot be translated may match any member, so
    // whether `additionalProperties` applies is never known.
    let others_any = s.others_accept_anything(ir);
    let others_checked = !others_any && s.patterns.iter().all(|p| p.regex.is_ok());
    let mut out = Vec::new();
    if others_checked {
        out.push("let mut matched = false;".to_owned());
    }
    let matched = if others_checked {
        "matched = true; "
    } else {
        ""
    };
    for (pattern, ty) in s.patterns.iter().zip(&view.pattern_tys) {
        let label = format!("patternProperties `{}`", pattern.source);
        // A pattern that lets anything be only says whether a member is
        // one of the others: nothing when that is not asked.
        let accepts = pattern.accepts_anything(ir);
        // Untranslated, whether it matches is still not known: below.
        if accepts && !others_checked && pattern.regex.is_ok() {
            continue;
        }
        let part = if accepts {
            String::new()
        } else {
            pattern_part(ir, pattern, ty.as_deref(), generated, reshaped)
        };
        out.push(match &pattern.regex {
            Ok(regex) => {
                mark_uses_regex();
                format!(
                    "{{ static RE: std::sync::LazyLock<Option<regex::Regex>> = std::sync::LazyLock::new(|| regex::Regex::new({regex:?}).ok()); match RE.as_ref() {{ Some(re) => {{ if re.is_match(name) {{ {matched}{part} }} }} None => {{ {matched}report.unevaluated(pointer, {:?}); }} }} }}",
                    format!("{label}: the pattern is too large to compile, so the member is not checked against it")
                )
            }
            // Matching or not, nothing is decided against a pattern that
            // lets anything be, when the other members may be anything too.
            Err(_) if accepts && others_any => continue,
            // Whether the member matches is not known: nor whether
            // `additionalProperties` applies to it.
            Err(reason) => format!(
                "{matched}report.unevaluated(pointer, {:?});",
                format!("{label}: {reason}, so whether the member matches it is not known")
            ),
        });
    }
    match (&s.others, &view.others_ty) {
        _ if !others_checked => {}
        (crate::ir::Others::Schema(b), Some(ty)) => {
            let part = part(ir, b, ty, "additionalProperties", generated, reshaped);
            out.push(format!("if !matched {{ {part} }}"));
        }
        (crate::ir::Others::Denied, _) => out.push(
            "if !matched { report.violation(pointer, \"is not allowed: it matches no property and no pattern\"); }".to_owned(),
        ),
        _ => {}
    }
    out
}

/// What `pattern`'s schema says of a member matching it, its text in
/// `raw`, at `pointer`.
fn pattern_part(
    ir: &crate::ir::Ir,
    pattern: &crate::ir::PatternDef,
    ty: Option<&str>,
    generated: &dyn Fn(&TypeRef) -> bool,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> String {
    let label = format!("patternProperties `{}`", pattern.source);
    match (&pattern.schema, ty) {
        (crate::ir::PatternSchema::Branch(b), Some(ty)) => {
            part(ir, b, ty, &label, generated, reshaped)
        }
        (crate::ir::PatternSchema::Unchecked(reason), _) => format!(
            "report.unevaluated(pointer, {:?});",
            format!("{label}: {reason}")
        ),
        _ => format!(
            "report.violation(pointer, {:?});",
            format!("is not allowed by {label}")
        ),
    }
}

/// The checks the patterns a declared property's name matches put on its
/// value, known at generation: the value is written out and read again
/// as each pattern's schema, as an undeclared member is.
fn declared_patterns(
    ir: &crate::ir::Ir,
    s: &crate::ir::StructDef,
    view: &TypeView,
    generated: &dyn Fn(&TypeRef) -> bool,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    for (field, field_view) in s.fields.iter().zip(&view.fields) {
        // Reported once already: the name it was read by is not known.
        if aliased(field_view) {
            continue;
        }
        let mut parts = Vec::new();
        let mut reads = false;
        // Decoded into a substitution or a 32-bit float, the value written
        // out is not the one that was sent.
        // So is one read as a float, or as `serde_json::Value`, which
        // reads a number as a float too; and one read by other names.
        let lossy = checks::reaches_leaf(ir, &field.ty, &|ty| {
            matches!(ty, TypeRef::Extern(_) | TypeRef::Number(_) | TypeRef::Any)
                || matches!(ty, TypeRef::Named(id) if matches!(ir.types.get(id).map(|d| &d.kind), Some(TypeKind::Extern)))
        }) || checks::reaches_decoder_rules(ir, &field.ty, reshaped);
        for (pattern, ty) in s.patterns.iter().zip(&view.pattern_tys) {
            match pattern.matches_name(&field.raw_name) {
                None => {}
                // Matching or not, a pattern that lets anything be decides
                // nothing.
                Some(_) if pattern.accepts_anything(ir) => {}
                Some(Ok(())) if lossy && pattern.branch().is_some() => parts.push(format!(
                    "report.unevaluated(pointer, {:?});",
                    format!(
                        "patternProperties `{}`: `{}` is held as a type that does not keep the value as it was sent, so it is not checked against it",
                        pattern.source, field.raw_name
                    )
                )),
                Some(Ok(())) => {
                    reads |= pattern.branch().is_some();
                    parts.push(pattern_part(
                        ir,
                        pattern,
                        ty.as_deref(),
                        generated,
                        reshaped,
                    ));
                }
                Some(Err(reason)) => parts.push(format!(
                    "report.unevaluated(pointer, {:?});",
                    format!(
                        "patternProperties `{}`: {reason}, so whether `{}` matches it is not known",
                        pattern.source, field.raw_name
                    )
                )),
            }
        }
        if parts.is_empty() {
            continue;
        }
        let name = &field_view.name;
        let checks = parts.join(" ");
        let inner = if reads {
            format!(
                "if let Ok(text) = serde_json::to_string(member) {{ let raw = text.as_str(); {checks} }}"
            )
        } else {
            format!("let _ = member; {checks}")
        };
        let present = if field.required {
            format!("{{ let member = &self.{name}; {inner} }}")
        } else {
            format!("if let Some(member) = self.{name}.as_ref() {{ {inner} }}")
        };
        let gates = field_view.cfg.join(" ");
        if !gates.is_empty()
            && !out
                .iter()
                .any(|l: &String| l.starts_with("let _ = (pointer"))
        {
            out.insert(0, "let _ = (pointer, &report);".to_owned());
        }
        out.push(
            format!(
                "{gates} {{ let p = child_pointer(pointer, {:?}); let pointer: &str = &p; {present} }}",
                field.raw_name
            )
            .trim_start()
            .to_owned(),
        );
    }
    out
}

/// One schema a raw member is read as and checked against, as an `allOf`
/// part is: a value that does not decode misses it, unless the decoder
/// rather than the value is why.
fn part(
    ir: &crate::ir::Ir,
    b: &Branch,
    ty: &str,
    label: &str,
    generated: &dyn Fn(&TypeRef) -> bool,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> String {
    let renamed = checks::reaches_reshaped(ir, &b.ty, reshaped);
    let (mut body, reads) = if renamed {
        (Vec::new(), false)
    } else {
        branch(ir, b, generated, ("report", "report"))
    };
    if renamed {
        body.push(format!(
            "report.unevaluated(pointer, {:?});",
            format!("{label}: its Rust type reads members by other names than the schema")
        ));
    } else if checks::part_unsure(b) {
        body.push(format!(
            "report.unevaluated(pointer, {:?});",
            format!("{label}: it says more than is checked")
        ));
    }
    let wide = rust_wide_of(ir, std::iter::once(&b.ty), "raw");
    let decoder = match decoder_rules(ir, &b.ty, reshaped, "raw") {
        Some(cond) => format!(
            " else if {cond} {{ report.unevaluated(pointer, {:?}); }}",
            format!("{label}: its Rust type may not decode a value the schema allows")
        ),
        None => String::new(),
    };
    let miss = format!(
        "if {wide} {{ report.unevaluated(pointer, {:?}); }}{decoder} else {{ report.violation(pointer, {:?}); }}",
        format!("{label}: a number in the value is past what its type holds"),
        format!("does not satisfy {label}")
    );
    let binding = if reads { "v" } else { "_" };
    format!(
        "match serde_json::from_str::<{ty}>(raw) {{ Ok({binding}) => {{ {} }} Err(_) => {{ {miss} }} }}",
        body.join(" ")
    )
}

/// The body of `validate_null` for one type: empty when a `null` of it
/// has nothing to check.
pub(crate) fn null_body(def: &TypeDef) -> Vec<String> {
    // A `None` of a non-nullable type is an absent member, not a null: a
    // present null of a null-only type is decoded into it.
    if !def.nullable {
        return Vec::new();
    }
    if checks::null_raw(def) {
        return vec![
            "Self { raw: serde_json::value::RawValue::from_string(\"null\".to_owned()).expect(\"null is JSON\") }.validate_into(pointer, report);".to_owned(),
        ];
    }
    let mut out = Vec::new();
    render(
        &checks::null_checks(def),
        ValueKind::Other,
        At {
            v: "_",
            p: "pointer",
            r: "report",
        },
        &mut out,
    );
    out
}

/// The body of `validate_into` for one type, or `None` when it gets no
/// implementation (a type alias shares its target's).
pub(crate) fn body(
    ir: &crate::ir::Ir,
    def: &TypeDef,
    view: &TypeView,
    generated: &dyn Fn(&TypeRef) -> bool,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
    opted_out: bool,
) -> Option<Vec<String>> {
    // A branch that comes back to this holder with the same raw value would
    // never finish: it is decoded, not validated.
    let progressing = |ty: &TypeRef| generated(ty) && !checks::loops_back(ir, &def.id, ty);
    if view.kind == "alias" {
        return None;
    }
    let mut out = Vec::new();
    if opted_out {
        return Some(out);
    }
    let report = At {
        v: "self",
        p: "pointer",
        r: "report",
    };
    // A holder's own `const` constrains the whole value, null included,
    // whatever its branches say.
    if matches!(
        def.kind,
        TypeKind::UnionTagged(_) | TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_)
    ) {
        render(
            &checks::unenforced(&def.constraints),
            ValueKind::Other,
            report,
            &mut out,
        );
    }
    match &def.kind {
        TypeKind::Struct(s) => {
            let mut present = Vec::new();
            let mut gated: Vec<String> = Vec::new();
            // What the schema says of a member no property names, which is
            // what a member read under an alias really is.
            let others_constrained = (def.constraints.property_names
                && def.constraints.names.as_deref() != Some(&crate::front::Constraints::default()))
                || !s.patterns.is_empty()
                || match &s.additional {
                    Additional::Denied => true,
                    Additional::CatchAll(TypeRef::Any) => def.constraints.items.is_some(),
                    Additional::CatchAll(_) => true,
                };
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let mut list = checks::value_checks(&field.ty, &field.constraints);
                let ext = checks::extern_checks(ir, &field.ty, false);
                list.extend(ext.value);
                let recurse = generated(&field.ty);
                let gates = field_view.cfg.join(" ");
                present.push(if field.required {
                    "1".to_owned()
                } else {
                    format!("usize::from(self.{name}.is_some())")
                });
                gated.push(gates.clone());
                // What applies to a present null as much as to a value.
                let mut always = Vec::new();
                render(
                    &[
                        checks::presence_checks(&field.ty, &field.constraints),
                        ext.presence,
                    ]
                    .concat(),
                    ValueKind::Other,
                    At {
                        v: "v",
                        p: "&p",
                        r: "report",
                    },
                    &mut always,
                );
                let mut inner = Vec::new();
                let reads = value(
                    &field.ty,
                    &list,
                    At {
                        v: "v",
                        p: "&p",
                        r: "report",
                    },
                    &mut inner,
                );
                let says_something = !inner.is_empty() || recurse || !always.is_empty();
                // Decoded, a field does not say which name it was read by,
                // and the schema judges a member under an alias as one no
                // property names: neither verdict can be given.
                if aliased(field_view) && (says_something || others_constrained || field.required) {
                    let statement = format!(
                        "report.unevaluated(&child_pointer(pointer, {:?}), {:?});",
                        field.raw_name,
                        format!(
                            "{}: may have been read under a serde alias, so what the schema says of it is not checked",
                            field.raw_name
                        )
                    );
                    let statement = if field.required {
                        statement
                    } else {
                        format!("if self.{name}.is_some() {{ {statement} }}")
                    };
                    if !gates.is_empty() && !out.iter().any(|l| l.starts_with("let _ = (pointer")) {
                        out.insert(0, "let _ = (pointer, &report);".to_owned());
                    }
                    out.push(format!("{gates} {{ {statement} }}").trim_start().to_owned());
                    continue;
                }
                // Every check may have been decided away.
                if !says_something {
                    continue;
                }
                let mut block = vec![format!(
                    "let p = child_pointer(pointer, {:?});",
                    field.raw_name
                )];
                if !always.is_empty() {
                    block.push(if field.required {
                        always.join(" ")
                    } else {
                        format!("if self.{name}.is_some() {{ {} }}", always.join(" "))
                    });
                }
                if !inner.is_empty() {
                    let binding = if reads { "v" } else { "_" };
                    let pattern = match (field.required, field.nullable) {
                        (true, false) => None,
                        (false, true) => Some(format!("Some(Some({binding}))")),
                        _ => Some(format!("Some({binding})")),
                    };
                    match pattern {
                        None if reads => {
                            block.push(format!("let v = &self.{name}; {}", inner.join(" ")))
                        }
                        None => block.push(inner.join(" ")),
                        Some(pattern) => block.push(format!(
                            "if let {pattern} = &self.{name} {{ {} }}",
                            inner.join(" ")
                        )),
                    }
                }
                if recurse {
                    block.push(format!("self.{name}.validate_into(&p, report);"));
                }
                // A field compiled out is not read either; with every one
                // out, the parameters would go unused.
                if !gates.is_empty() && !out.iter().any(|l| l.starts_with("let _ = (pointer")) {
                    out.insert(0, "let _ = (pointer, &report);".to_owned());
                }
                out.push(
                    format!("{gates} {{ {} }}", block.join(" "))
                        .trim_start()
                        .to_owned(),
                );
            }
            if let Additional::CatchAll(ty) = &s.additional {
                let name = &view.fields.last().expect("the catch-all field").name;
                present.push(format!("self.{name}.len()"));
                gated.push(String::new());
                let each = checks::each(&TypeRef::Map(Box::new(ty.clone())), &def.constraints);
                // A property compiled out with `cfg` lands among the other
                // members, where `additionalProperties` does not apply to
                // it: with any gated, their values are not checked.
                let gates = view.fields.iter().any(|f| !f.cfg.is_empty());
                if !s.patterns.is_empty() {
                    // A declared property matching a pattern is checked
                    // against its schema too.
                    for line in declared_patterns(ir, s, view, generated, reshaped) {
                        if line.starts_with("let _ = (pointer") {
                            if !out.iter().any(|l| l.starts_with("let _ = (pointer")) {
                                out.insert(0, line);
                            }
                        } else {
                            out.push(line);
                        }
                    }
                    if gates {
                        out.push(format!(
                            "if !self.{name}.is_empty() {{ report.unevaluated(pointer, \"patternProperties: a property compiled out with cfg lands among the other members, so they are not checked\"); }}"
                        ));
                    } else {
                        let checks = pattern_members(ir, s, view, generated, reshaped);
                        // A member is read only against a pattern's schema: with
                        // every pattern untranslated, never.
                        let read = s.patterns.iter().any(|p| {
                            p.regex.is_ok() && p.branch().is_some() && !p.accepts_anything(ir)
                        }) || (matches!(s.others, crate::ir::Others::Schema(_))
                            && !s.others_accept_anything(ir)
                            && s.patterns.iter().all(|p| p.regex.is_ok()));
                        // Nothing a member could be is decided: no loop.
                        if checks.is_empty() {
                            // Nothing to check on any member.
                        } else if read {
                            out.push(format!(
                                "for (name, value) in &self.{name} {{ let p = child_pointer(pointer, name); let pointer: &str = &p; let raw = value.get(); {} }}",
                                checks.join(" ")
                            ));
                        } else {
                            out.push(format!(
                                "for name in self.{name}.keys() {{ let p = child_pointer(pointer, name); let pointer: &str = &p; {} }}",
                                checks.join(" ")
                            ));
                        }
                    }
                } else if gates && (generated(ty) || each.is_some()) {
                    out.push(format!(
                        "if !self.{name}.is_empty() {{ report.unevaluated(pointer, \"additionalProperties: a property compiled out with cfg lands among the other members, so their values are not checked\"); }}"
                    ));
                } else if generated(ty) {
                    out.push(format!("self.{name}.validate_into(pointer, report);"));
                }
                let each = each.filter(|_| !gates);
                let field = format!("self.{name}");
                render(
                    &checks::extern_checks(ir, &TypeRef::Map(Box::new(ty.clone())), false)
                        .value
                        .into_iter()
                        .chain(each)
                        .collect::<Vec<_>>(),
                    ValueKind::Map,
                    At {
                        v: &field,
                        p: "pointer",
                        r: "report",
                    },
                    &mut out,
                );
            }
            let list = checks::object_checks(&def.constraints);
            let counted = list
                .iter()
                .any(|c| matches!(c, Check::MinProperties(_) | Check::MaxProperties(_)));
            if counted && gated.iter().any(|g| !g.is_empty()) {
                // A gate cannot sit inside a sum: counted statement by
                // statement.
                out.push("#[allow(unused_mut)] let mut members: usize = 0;".to_owned());
                for (count, gates) in present.iter().zip(&gated) {
                    out.push(
                        format!("{gates} {{ members += {count}; }}")
                            .trim_start()
                            .to_owned(),
                    );
                }
            } else if counted {
                out.push(format!(
                    "let members: usize = {};",
                    if present.is_empty() {
                        "0".to_owned()
                    } else {
                        present.join(" + ")
                    }
                ));
            }
            for check in &list {
                match check {
                    Check::MinProperties(n) => out.push(format!(
                        "if (members as u64) < {n} {{ report.violation(pointer, format!(\"has {{members}} members, fewer than minProperties {n}\")); }}"
                    )),
                    Check::MaxProperties(n) => out.push(format!(
                        "if (members as u64) > {n} {{ report.violation(pointer, format!(\"has {{members}} members, more than maxProperties {n}\")); }}"
                    )),
                    // Declared names are known now: only whether the member
                    // is present is left to run time. The other members'
                    // names are read from the catch-all.
                    Check::Keys(names) => {
                        for (field, field_view) in s.fields.iter().zip(&view.fields) {
                            // Reported above: the name it was read by is
                            // not known.
                            if aliased(field_view) {
                                continue;
                            }
                            let pointer = format!("&child_pointer(pointer, {:?})", field.raw_name);
                            let statement = match checks::name_verdict(&field.raw_name, names) {
                                checks::Verdict::Pass => continue,
                                checks::Verdict::Fail(message) => {
                                    format!("report.violation({pointer}, {message:?});")
                                }
                                checks::Verdict::Unknown(message) => {
                                    format!("report.unevaluated({pointer}, {message:?});")
                                }
                            };
                            let statement = if field.required {
                                statement
                            } else {
                                format!("if self.{}.is_some() {{ {statement} }}", field_view.name)
                            };
                            let gates = field_view.cfg.join(" ");
                            // Compiled out, the statement reads nothing: the
                            // parameters must still count as used.
                            if !gates.is_empty()
                                && !out.iter().any(|l| l.starts_with("let _ = (pointer"))
                            {
                                out.insert(0, "let _ = (pointer, &report);".to_owned());
                            }
                            out.push(
                                format!("{gates} {{ {statement} }}").trim_start().to_owned(),
                            );
                        }
                        if matches!(s.additional, Additional::CatchAll(_)) {
                            let field = format!(
                                "self.{}",
                                view.fields.last().expect("the catch-all field").name
                            );
                            render(
                                std::slice::from_ref(check),
                                ValueKind::Map,
                                At {
                                    v: &field,
                                    ..report
                                },
                                &mut out,
                            );
                        }
                    }
                    other => {
                        render(
                            std::slice::from_ref(other),
                            ValueKind::Other,
                            report,
                            &mut out,
                        );
                    }
                }
            }
        }
        TypeKind::StringEnum(values) => {
            // Membership is the decoder's; the other string constraints
            // apply to the member's wire value. An enum of no members has
            // no value to check.
            let list = checks::checks(&TypeRef::String, &def.constraints);
            if !list.is_empty() && !values.is_empty() {
                let mut inner = Vec::new();
                let reads = render(
                    &list,
                    ValueKind::String,
                    At { v: "v", ..report },
                    &mut inner,
                );
                if reads {
                    let arms: Vec<String> = view
                        .variants
                        .iter()
                        .map(|variant| format!("Self::{} => {:?},", variant.name, variant.value))
                        .collect();
                    out.push(format!(
                        "let v: &str = match self {{ {} }};",
                        arms.join(" ")
                    ));
                }
                out.extend(inner);
            }
        }
        TypeKind::UnionTagged(u) => {
            let arms: Vec<String> = u
                .variants
                .iter()
                .zip(&view.variants)
                .map(|(variant, variant_view)| {
                    let gates = variant_view.cfg.join(" ");
                    let arm = |text: String| format!("{gates} {text}").trim_start().to_owned();
                    if generated(&variant.ty) {
                        return arm(format!(
                            "Self::{}(v) => v.validate_into(pointer, report),",
                            variant_view.name
                        ));
                    }
                    // A substituted variant is not checked against what
                    // it declares.
                    let ext = checks::extern_checks(ir, &variant.ty, false);
                    let mut body = Vec::new();
                    render(
                        &[ext.presence, ext.value].concat(),
                        ValueKind::Other,
                        At {
                            v: "_v",
                            p: "pointer",
                            r: "report",
                        },
                        &mut body,
                    );
                    if body.is_empty() {
                        arm(format!("Self::{}(_) => {{}}", variant_view.name))
                    } else {
                        arm(format!(
                            "Self::{}(_) => {{ {} }}",
                            variant_view.name,
                            body.join(" ")
                        ))
                    }
                })
                .collect();
            if !arms.iter().all(|arm| arm.ends_with("(_) => {}")) {
                out.push(format!("match self {{ {} }}", arms.join(" ")));
            }
        }
        TypeKind::UnionOpen(u) => {
            if def.nullable {
                let mut null = Vec::new();
                render(
                    &checks::shared_unsupported(ir, &u.branches),
                    ValueKind::Other,
                    report,
                    &mut null,
                );
                out.push(if null.is_empty() {
                    NULL_IS_VALID.to_owned()
                } else {
                    format!(
                        "if self.raw.get().trim() == \"null\" {{ {} return; }}",
                        null.join(" ")
                    )
                });
            }
            let mut lines = Vec::new();
            let mut checked = false;
            let mut counts_certain = false;
            let wide = rust_wide(ir, u.branches.iter().map(|b| &b.ty));
            for (b, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                // Read by other names than the schema, a decoded branch is
                // no proof either way, and neither is what its checks find:
                // an alias's member is checked as the field it is read into.
                let renamed = checks::reaches_reshaped(ir, &b.ty, reshaped);
                let (body, reads) = if renamed {
                    (Vec::new(), false)
                } else {
                    branch(ir, b, &progressing, ("branch", "&mut branch"))
                };
                // Decoding proves a match only when nothing the branch says
                // goes unchecked.
                let sure =
                    !checks::unsure(b) && !renamed && (!generated(&b.ty) || progressing(&b.ty));
                let counter = if sure { "certain" } else { "uncertain" };
                counts_certain |= sure;
                // A number past what the branch's type holds fails to decode
                // for the representation: no proof it matches no branch.
                checked = true;
                let beyond = match decoder_rules(ir, &b.ty, reshaped, "self.raw.get()") {
                    Some(cond) if cond == "true" => " else { uncertain += 1; }".to_owned(),
                    Some(cond) => format!(" else if {wide} || {cond} {{ uncertain += 1; }}"),
                    None => format!(" else if {wide} {{ uncertain += 1; }}"),
                };
                if body.is_empty() {
                    checked |= !sure;
                    lines.push(format!(
                        "if self.{accessor}().is_some() {{ {counter} += 1; }}{beyond}"
                    ));
                } else {
                    checked = true;
                    let binding = if reads { "v" } else { "_" };
                    lines.push(format!(
                        "if let Some({binding}) = self.{accessor}() {{ let mut branch = report.nested(pointer); {} report.resume(pointer, &branch); if branch.violations().next().is_none() {{ if branch.unchecked().next().is_none() {{ {counter} += 1; }} else {{ uncertain += 1; }} }} }}{beyond}",
                        body.join(" ")
                    ));
                }
            }
            out.push(if counts_certain {
                "let mut certain = 0usize;".to_owned()
            } else {
                "let certain = 0usize;".to_owned()
            });
            out.push(if checked {
                "let mut uncertain = 0usize;".to_owned()
            } else {
                "let uncertain = 0usize;".to_owned()
            });
            out.extend(lines);
            if u.exclusive {
                out.push("if certain + uncertain == 0 { report.violation(pointer, \"matches none of the oneOf branches\"); } else if certain > 1 { report.violation(pointer, format!(\"matches {certain} oneOf branches; exactly one must\")); } else if !(certain == 1 && uncertain == 0) { report.unevaluated(pointer, \"oneOf: whether exactly one branch matches depends on constraints that were not evaluated\"); }".to_owned());
            } else {
                out.push("if certain == 0 { if uncertain == 0 { report.violation(pointer, \"matches none of the anyOf branches\"); } else { report.unevaluated(pointer, \"anyOf: whether a branch matches depends on constraints that were not evaluated\"); } }".to_owned());
            }
        }
        TypeKind::ComposedOpen(branches) => {
            // An intersection is nullable when any side is, so null is left
            // to every part to accept.
            let wide = rust_wide(ir, branches.iter().map(|b| &b.ty));
            for (b, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = format!(
                    "does not satisfy the allOf part read by {}",
                    accessor.trim_start_matches("as_")
                );
                // Read by other names than the schema, neither a decoded part
                // nor what its checks find is proof: it is not walked.
                let renamed = checks::reaches_reshaped(ir, &b.ty, reshaped);
                let (mut body, reads) = if renamed {
                    (Vec::new(), false)
                } else {
                    branch(ir, b, &progressing, ("report", "report"))
                };
                if renamed {
                    body.push(format!(
                        "report.unevaluated(pointer, {:?});",
                        format!(
                            "allOf part read by {}: its Rust type reads members by other names than the schema",
                            accessor.trim_start_matches("as_")
                        )
                    ));
                } else if checks::part_unsure(b) || (generated(&b.ty) && !progressing(&b.ty)) {
                    body.push(format!(
                        "report.unevaluated(pointer, {:?});",
                        format!(
                            "allOf part read by {}: it says more than is checked",
                            accessor.trim_start_matches("as_")
                        )
                    ));
                }
                // A number past what the part's type holds fails to decode for
                // the representation: undecided, not a miss.
                let part_name = accessor.trim_start_matches("as_");
                let decoder = match decoder_rules(ir, &b.ty, reshaped, "self.raw.get()") {
                    Some(cond) => format!(
                        " else if {cond} {{ report.unevaluated(pointer, {:?}); }}",
                        format!(
                            "allOf part read by {part_name}: its Rust type may not decode a value the schema allows"
                        )
                    ),
                    None => String::new(),
                };
                let miss = format!(
                    "if {wide} {{ report.unevaluated(pointer, {:?}); }}{decoder} else {{ report.violation(pointer, {message:?}); }}",
                    format!(
                        "allOf part read by {part_name}: a number in the value is past what its type holds"
                    )
                );
                let part = if body.is_empty() {
                    format!("if self.{accessor}().is_none() {{ {miss} }}")
                } else {
                    let binding = if reads { "v" } else { "_" };
                    format!(
                        "match self.{accessor}() {{ Some({binding}) => {{ {} }} None => {{ {miss} }} }}",
                        body.join(" ")
                    )
                };
                // A typeless schema's object keywords say nothing about a
                // value that is not an object.
                out.push(if b.object_only {
                    format!("if self.raw.get().trim_start().starts_with('{{') {{ {part} }}")
                } else {
                    part
                });
            }
        }
        TypeKind::Newtype(inner) | TypeKind::Alias(inner) => {
            let ext = checks::extern_checks(ir, inner, false);
            render(
                &[
                    checks::presence_checks(inner, &def.constraints),
                    ext.presence,
                ]
                .concat(),
                ValueKind::Other,
                report,
                &mut out,
            );
            let mut list = checks::value_checks(inner, &def.constraints);
            list.extend(ext.value);
            let _ = value(
                inner,
                &list,
                At {
                    v: "(&self.0)",
                    ..report
                },
                &mut out,
            );
            if generated(inner) {
                out.push("self.0.validate_into(pointer, report);".to_owned());
            }
        }
        TypeKind::Extern => {}
    }
    // Every generated type counts its depth, and a raw holder its branches:
    // a raw holder decodes each projection afresh, so nothing else bounds
    // a recursive value, and nested unions would multiply the work.
    if !out.is_empty() {
        let branches = match &def.kind {
            TypeKind::UnionOpen(u) => u.branches.len(),
            TypeKind::ComposedOpen(b) => b.len(),
            _ => 0,
        };
        let mut wrapped = vec![
            if branches == 0 {
                "if !report.enter(pointer, 0) { return; }".to_owned()
            } else {
                // Each branch decodes the whole raw value again.
                format!(
                    "if !report.enter(pointer, {branches} * self.raw.get().len().max(640)) {{ return; }}"
                )
            },
            "let run = |report: &mut ValidationReport| {".to_owned(),
        ];
        wrapped.extend(out);
        wrapped.extend([
            "};".to_owned(),
            "run(report);".to_owned(),
            "report.leave();".to_owned(),
        ]);
        out = wrapped;
    }
    Some(out)
}
