//! Spelling [`Check`]s as Go: the body of each generated type's
//! `validateInto`. Go has no blanket implementations, so recursion into
//! slices, maps and pointers is written out from the type's shape.

use crate::checks::{self, Check, Unique, ValueKind};
use crate::front::{FloatWidth, IntWidth};
use crate::go::view::TypeView;
use crate::ir::{Additional, Branch, TypeDef, TypeKind, TypeRef};

/// What the rendered checks need imported.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Needs {
    pub utf8: bool,
    pub math: bool,
    /// Map keys walked in order: `maps` and `slices`.
    pub sorted: bool,
    /// Floats sorted: `slices`.
    pub slices: bool,
    /// A raw holder is built from `null`: `jsontext`.
    pub jsontext: bool,
    /// A pattern is matched — a `pattern` check, or patterns checked at
    /// decode: `regexp`, `sync` and `matchPattern`.
    pub pattern: bool,
}

pub(crate) struct Context<'a> {
    /// The type holds a generated type with its own `validateInto`.
    pub validates: &'a dyn Fn(&TypeRef) -> bool,
    /// A named type spelled as a pointer alias, which may be nil.
    pub nil_able: &'a dyn Fn(&TypeRef) -> bool,
    /// A `null` of the raw holder (open union, composition or JSON-value
    /// wrapper) `ty` names, as a Go expression, if it names one.
    pub raw_holder: &'a dyn Fn(&TypeRef) -> Option<String>,
    /// A branch that comes back to the holder being validated with the
    /// same raw value: decoded, never validated, or it would not finish.
    pub loops: &'a dyn Fn(&TypeRef) -> bool,
    /// The Go name of a named type with a `validateNull`, through aliases.
    pub null_name: &'a dyn Fn(&TypeRef) -> Option<String>,
    /// What a substituted component declares, at a position holding it.
    pub extern_checks: &'a dyn Fn(&TypeRef) -> checks::Externs,
    pub ir: &'a crate::ir::Ir,
    pub needs: Needs,
}

/// An exact decimal as a `Decimal` literal.
fn decimal(d: &checks::Dec) -> String {
    let digits = d.mantissa.unsigned_abs().to_string();
    if d.mantissa == 0 {
        "Decimal{}".to_owned()
    } else {
        format!(
            "Decimal{{negative: {}, digits: \"{digits}\", exponent: {}}}",
            d.mantissa < 0,
            d.exponent
        )
    }
}

fn lit(text: &str) -> String {
    crate::go::naming::quote(text)
}

/// Where a check reads and reports: the value, the pointer and the
/// report, as Go expressions.
#[derive(Clone, Copy)]
struct At<'a> {
    e: &'a str,
    p: &'a str,
    r: &'a str,
}

/// Statements checking `list` at `at`. True when they read the value.
fn render(
    list: &[Check],
    (kind, item): (ValueKind, Option<&TypeRef>),
    at: At<'_>,
    out: &mut Vec<String>,
    needs: &mut Needs,
) -> bool {
    let At { e, p, r } = at;
    // Some checks only report that they could not be made, and a bound
    // beyond every value emits nothing: the value is read only when a
    // check that reads it emitted something.
    let mut reads = false;
    let (floor, ceiling) = match kind {
        ValueKind::Integer(IntWidth::Int32) => (i128::from(i32::MIN), i128::from(i32::MAX)),
        _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
    };
    // A count compared as `uint64`, which holds any bound a schema can
    // state; `int` would not.
    let count = |what: &str, op: &str, n: u64, message: String| {
        format!(
            "if uint64({what}) {op} {n} {{ {r}.violation({p}, fmt.Sprintf({}, {what})) }}",
            lit(&message)
        )
    };
    for check in list {
        let before = out.len();
        match check {
            Check::MinLength(n) => {
                needs.utf8 = true;
                out.push(format!(
                    "{{ n := utf8.RuneCountInString(string({e})); {} }}",
                    count(
                        "n",
                        "<",
                        *n,
                        format!("is %d characters long, shorter than minLength {n}")
                    )
                ));
            }
            Check::Pattern { regex, source } => {
                needs.pattern = true;
                out.push(format!(
                    "if matched, compiled := matchPattern({}, string({e})); !compiled {{ {r}.unevaluated({p}, {}) }} else if !matched {{ {r}.violation({p}, {}) }}",
                    lit(regex),
                    lit(&format!("pattern {source:?}: too large to compile here")),
                    lit(&format!("does not match pattern {source:?}")),
                ));
            }
            Check::MaxLength(n) => {
                needs.utf8 = true;
                out.push(format!(
                    "{{ n := utf8.RuneCountInString(string({e})); {} }}",
                    count(
                        "n",
                        ">",
                        *n,
                        format!("is %d characters long, longer than maxLength {n}")
                    )
                ));
            }
            Check::IntAtLeast {
                bound,
                keyword,
                literal,
            } => {
                let message = lit(&format!("%d is below {keyword} {literal}"));
                if *bound > ceiling {
                    out.push(format!(
                        "{r}.violation({p}, fmt.Sprintf({message}, int64({e})))"
                    ));
                } else if *bound > floor {
                    out.push(format!(
                        "if int64({e}) < {bound} {{ {r}.violation({p}, fmt.Sprintf({message}, int64({e}))) }}"
                    ));
                }
            }
            Check::IntAtMost {
                bound,
                keyword,
                literal,
            } => {
                let message = lit(&format!("%d is above {keyword} {literal}"));
                if *bound < floor {
                    out.push(format!(
                        "{r}.violation({p}, fmt.Sprintf({message}, int64({e})))"
                    ));
                } else if *bound < ceiling {
                    out.push(format!(
                        "if int64({e}) > {bound} {{ {r}.violation({p}, fmt.Sprintf({message}, int64({e}))) }}"
                    ));
                }
            }
            Check::IntMultipleOf { divisor, literal } => {
                let message = lit(&format!("%d is not a multiple of {literal}"));
                let min = i128::from(i64::MIN);
                if *divisor == -min {
                    // 2^63: zero and math.MinInt64 are its only multiples
                    // an int64 can hold.
                    out.push(format!(
                        "if x := int64({e}); x != 0 && x != -9223372036854775808 {{ {r}.violation({p}, fmt.Sprintf({message}, x)) }}"
                    ));
                } else if *divisor > i128::from(i64::MAX) {
                    // Larger than any int64: only zero is a multiple.
                    out.push(format!(
                        "if int64({e}) != 0 {{ {r}.violation({p}, fmt.Sprintf({message}, int64({e}))) }}"
                    ));
                } else if *divisor != 1 {
                    out.push(format!(
                        "if int64({e})%{divisor} != 0 {{ {r}.violation({p}, fmt.Sprintf({message}, int64({e}))) }}"
                    ));
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
                        // Exactly, as integers; a fraction is no integer's
                        // value, nor is one past every int64.
                        // One written as a float past 2^53, read as one,
                        // stands for every integer that rounds to it: equal
                        // to it once rounded is undecided.
                        let mut held: Vec<i64> = values
                            .iter()
                            .filter(|n| checks::integer_held(n))
                            .filter_map(checks::exact_integer)
                            .filter_map(|n| i64::try_from(n).ok())
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
                            "{r}.violation({p}, fmt.Sprintf({}, int64({e})))",
                            lit(&format!("%d is not {what}"))
                        );
                        let missed = if unheld.is_empty() {
                            violation
                        } else {
                            let at = unheld
                                .iter()
                                .map(|f| format!("float64(int64({e})) == {f:?}"))
                                .collect::<Vec<_>>()
                                .join(" || ");
                            format!(
                                "if {at} {{ {r}.unevaluated({p}, {}) }} else {{ {violation} }}",
                                lit(&format!("{keyword} {literal}: a listed value was read as a float, which does not tell the integers near it apart"))
                            )
                        };
                        out.push(if held.is_empty() {
                            missed
                        } else {
                            let list = held.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
                            format!("switch int64({e}) {{ case {list}: default: {missed} }}")
                        });
                    }
                    ValueKind::Decimal => {
                        // Exactly; a value no decimal holds was an error
                        // when lowering.
                        let mut set: Vec<checks::Dec> =
                            values.iter().filter_map(checks::dec).collect();
                        set.dedup();
                        let list = set.iter().map(decimal).collect::<Vec<_>>().join(", ");
                        out.push(format!(
                            "switch {e} {{ case {list}: default: {r}.violation({p}, fmt.Sprintf({}, {e})) }}",
                            lit(&format!("%v is not {what}"))
                        ));
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
                            "{r}.violation({p}, fmt.Sprintf({}, float32({e})))",
                            lit(&format!("%v is not {what}"))
                        );
                        out.push(if set.is_empty() {
                            violation
                        } else {
                            let at = set
                                .iter()
                                .map(|f| format!("float32({e}) == {f:?}"))
                                .collect::<Vec<_>>()
                                .join(" || ");
                            format!(
                                "if {at} {{ {r}.unevaluated({p}, {}) }} else {{ {violation} }}",
                                lit(&format!("{keyword} {literal}: the value rounds to the same float as a listed value"))
                            )
                        });
                    }
                    _ => {
                        // Decided as far as a float64 holds the value; a
                        // listed value no float holds as written leaves a
                        // value equal to its float undecided.
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
                                .map(|f| format!("float64({e}) == {f:?}"))
                                .collect::<Vec<_>>()
                                .join(" || ")
                        };
                        let (exact, inexact) = (floats(true), floats(false));
                        let violation = format!(
                            "{r}.violation({p}, fmt.Sprintf({}, float64({e})))",
                            lit(&format!("%v is not {what}"))
                        );
                        // A value equal to a listed value held exactly is
                        // decided first; only then one no float holds.
                        let missed = if inexact.is_empty() {
                            violation
                        } else {
                            format!(
                                "if {} {{ {r}.unevaluated({p}, {}) }} else {{ {violation} }}",
                                any(&inexact),
                                lit(&format!("{keyword} {literal}: a listed value is not exactly a float, and the value is its float"))
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
                let (op, keyword, word) = match (at_least, exclusive) {
                    (true, false) => (">=", "minimum", "below"),
                    (true, true) => (">", "exclusiveMinimum", "below"),
                    (false, false) => ("<=", "maximum", "above"),
                    (false, true) => ("<", "exclusiveMaximum", "above"),
                };
                let float = matches!(kind, ValueKind::Number(FloatWidth::Float));
                // Compared at the value's own width: a float32 bound is the
                // constant rounded to float32, as the value was.
                let value = if float {
                    format!("float32({e})")
                } else {
                    format!("float64({e})")
                };
                let violation = format!(
                    "{r}.violation({p}, fmt.Sprintf({}, {value}))",
                    lit(&format!("%v is {word} {keyword} {literal}"))
                );
                if !bound.is_finite() {
                    // Beyond every value: always or never met.
                    if (*bound > 0.0) == at_least {
                        out.push(violation);
                    }
                } else if float && (*bound as f32).is_finite() {
                    // A float32 at the bound is ambiguous: literals on
                    // either side of the bound decode to it. The bound
                    // was read through an f64, so the decoder may round
                    // its literal to either float32 around that f64.
                    let (below, above) = checks::f32_neighbours(*bound);
                    let at = if below == above {
                        format!("{value} == {below:?}")
                    } else {
                        format!("({value} == {below:?} || {value} == {above:?})")
                    };
                    let rounded = format!("{bound:?}");
                    out.push(format!(
                        "if {at} {{ {r}.unevaluated({p}, {}) }} else if !(float64({value}) {op} {rounded}) {{ {violation} }}",
                        lit(&format!("{keyword} {literal}: the value rounds to the same float as the bound"))
                    ));
                } else if float {
                    // Past every float32: a literal at or past the bound
                    // decodes to the largest one, which is ambiguous.
                    needs.math = true;
                    let edge = if *bound > 0.0 { "math.MaxFloat32" } else { "-math.MaxFloat32" };
                    let wide = format!("float64({e})");
                    out.push(format!(
                        "if {wide} == {edge} {{ {r}.unevaluated({p}, {}) }} else if !({wide} {op} {bound:?}) {{ {violation} }}",
                        lit(&format!("{keyword} {literal}: the value is at the edge of the float range"))
                    ));
                } else {
                    let wide = format!("float64({e})");
                    out.push(format!(
                        "if !({wide} {op} {bound:?}) {{ {violation} }}"
                    ));
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
                let (op, keyword, word) = match (at_least, exclusive) {
                    (true, false) => (">=", "minimum", "below"),
                    (true, true) => (">", "exclusiveMinimum", "below"),
                    (false, false) => ("<=", "maximum", "above"),
                    (false, true) => ("<", "exclusiveMaximum", "above"),
                };
                out.push(format!(
                    "if !({e}.Cmp({}) {op} 0) {{ {r}.violation({p}, fmt.Sprintf({}, {e})) }}",
                    decimal(bound),
                    lit(&format!("%v is {word} {keyword} {literal}"))
                ));
            }
            Check::DecMultipleOf { divisor, literal } => out.push(format!(
                "if !{e}.IsMultipleOf({}) {{ {r}.violation({p}, fmt.Sprintf({}, {e})) }}",
                decimal(divisor),
                lit(&format!("%v is not a multiple of {literal}"))
            )),
            Check::NumMultipleOf { integral, literal } => {
                let unevaluated = format!(
                    "{r}.unevaluated({p}, {})",
                    lit(&format!("multipleOf {literal}: a decoded float has lost its literal, so only a fraction against a whole multiple is decided"))
                );
                if *integral {
                    needs.math = true;
                    out.push(format!(
                        "if f := float64({e}); f != math.Trunc(f) {{ {r}.violation({p}, fmt.Sprintf({}, f)) }} else {{ {unevaluated} }}",
                        lit(&format!("%v is not a multiple of {literal}"))
                    ));
                } else {
                    out.push(unevaluated);
                }
            }
            Check::MinItems(n) => out.push(count(
                &format!("len({e})"),
                "<",
                *n,
                format!("has %d items, fewer than minItems {n}"),
            )),
            Check::MaxItems(n) => out.push(count(
                &format!("len({e})"),
                ">",
                *n,
                format!("has %d items, more than maxItems {n}"),
            )),
            Check::UniqueItems(Unique::Exact) => {
                let key = match item {
                    Some(TypeRef::String) => "string",
                    Some(TypeRef::Boolean) => "bool",
                    Some(TypeRef::Integer(IntWidth::Int32)) => "Int32",
                    Some(TypeRef::Decimal) => "Decimal",
                    _ => "Int64",
                };
                out.push(format!(
                    "{{ seen := map[{key}]bool{{}}; for i, item := range {e} {{ if seen[item] {{ {r}.violation(childPointer({p}, fmt.Sprint(i)), \"repeats an earlier item; uniqueItems is true\") }}; seen[item] = true }} }}"
                ));
            }
            Check::UniqueItems(Unique::Float) => {
                // Sorted, equal floats sit side by side: linearithmic, not
                // quadratic.
                needs.slices = true;
                out.push(format!(
                    "{{ sorted := make([]float64, 0, len({e})); for _, x := range {e} {{ sorted = append(sorted, float64(x)) }}; slices.Sort(sorted); for i := 1; i < len(sorted); i++ {{ if sorted[i] == sorted[i-1] {{ {r}.unevaluated({p}, \"uniqueItems: equal floats may come from different numbers\"); break }} }} }}"
                ))
            }
            Check::UniqueItems(Unique::Unknown) => out.push(format!(
                "if len({e}) > 1 {{ {r}.unevaluated({p}, \"uniqueItems: items of this type are not compared yet\") }}"
            )),
            Check::MinProperties(n) => out.push(count(
                &format!("len({e})"),
                "<",
                *n,
                format!("has %d members, fewer than minProperties {n}"),
            )),
            Check::MaxProperties(n) => out.push(count(
                &format!("len({e})"),
                ">",
                *n,
                format!("has %d members, more than maxProperties {n}"),
            )),
            Check::Unevaluated { keyword, reason } => out.push(format!(
                "{r}.unevaluated({p}, {})",
                lit(&format!("{keyword}: {reason}"))
            )),
            Check::Unsupported(keywords) => out.push(format!(
                "{r}.unevaluated({p}, {})",
                lit(&format!("{}: not handled by the generator", keywords.join(", ")))
            )),
            Check::Each {
                item,
                presence,
                value: list,
            } => {
                let at = At {
                    e: "item",
                    p: "p",
                    r,
                };
                let mut body = Vec::new();
                let mut reads = render(presence, (ValueKind::Other, None), at, &mut body, needs);
                reads |= checked(item, list, at, &mut body, needs);
                if !body.is_empty() {
                    let body = body.join("; ");
                    out.push(if kind == ValueKind::Map {
                        needs.sorted = true;
                        let item = if reads {
                            format!("item := {e}[k]; ")
                        } else {
                            String::new()
                        };
                        format!(
                            "for _, k := range slices.Sorted(maps.Keys({e})) {{ {item}p := childPointer({p}, k); {body} }}"
                        )
                    } else {
                        let binding = if reads { "i, item" } else { "i" };
                        format!(
                            "for {binding} := range {e} {{ p := childPointer({p}, fmt.Sprint(i)); {body} }}"
                        )
                    });
                }
            }
            Check::Keys(list) => {
                let mut body = Vec::new();
                render(
                    list,
                    (ValueKind::String, None),
                    At { e: "k", p: "p", r },
                    &mut body,
                    needs,
                );
                if !body.is_empty() {
                    needs.sorted = true;
                    out.push(format!(
                        "for _, k := range slices.Sorted(maps.Keys({e})) {{ p := childPointer({p}, k); {} }}",
                        body.join("; ")
                    ));
                }
            }
            Check::ItemsUnevaluated { keyword, reason } => out.push(format!(
                "if len({e}) > 0 {{ {r}.unevaluated({p}, {}) }}",
                lit(&format!("{keyword} on the items: {reason}"))
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

/// The element type of an array, for `uniqueItems`.
fn item_of(ty: &TypeRef) -> Option<&TypeRef> {
    match ty {
        TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => item_of(inner),
        TypeRef::Array(item) => Some(item),
        _ => None,
    }
}

/// Recursion into the generated types `ty` holds, from the value `e`.
fn recurse(ty: &TypeRef, at: At<'_>, depth: usize, out: &mut Vec<String>, cx: &mut Context<'_>) {
    if !(cx.validates)(ty) {
        return;
    }
    let At { e, p, r } = at;
    match ty {
        TypeRef::Named(_) if (cx.nil_able)(ty) => {
            out.push(format!("if {e} != nil {{ {e}.validateInto({p}, {r}) }}"))
        }
        TypeRef::Named(_) => out.push(format!("{e}.validateInto({p}, {r})")),
        TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => {
            let mut body = Vec::new();
            let deref = format!("(*{e})");
            recurse(inner, At { e: &deref, ..at }, depth, &mut body, cx);
            match matches!(ty, TypeRef::Nullable(_))
                .then(|| (cx.null_name)(inner))
                .flatten()
            {
                Some(name) => out.push(format!(
                    "if {e} != nil {{ {} }} else {{ {} }}",
                    body.join("; "),
                    null_call(&name, p, r)
                )),
                None => out.push(format!("if {e} != nil {{ {} }}", body.join("; "))),
            }
        }
        TypeRef::Array(inner) => {
            let (i, item) = (format!("i{depth}"), format!("item{depth}"));
            let pointer = format!("childPointer({p}, fmt.Sprint({i}))");
            let mut body = Vec::new();
            recurse(
                inner,
                At {
                    e: &item,
                    p: &pointer,
                    r,
                },
                depth + 1,
                &mut body,
                cx,
            );
            out.push(format!(
                "for {i}, {item} := range {e} {{ {} }}",
                body.join("; ")
            ));
        }
        TypeRef::Map(inner) => {
            cx.needs.sorted = true;
            let key = format!("k{depth}");
            let (value, pointer) = (format!("{e}[{key}]"), format!("childPointer({p}, {key})"));
            let mut body = Vec::new();
            recurse(
                inner,
                At {
                    e: &value,
                    p: &pointer,
                    r,
                },
                depth + 1,
                &mut body,
                cx,
            );
            out.push(format!(
                "for _, {key} := range slices.Sorted(maps.Keys({e})) {{ {} }}",
                body.join("; ")
            ));
        }
        _ => {}
    }
}

/// Whether a holder's raw value has a number its branches' types may fail
/// to decode for their range: such a failure proves no mismatch.
fn go_wide<'a>(ir: &crate::ir::Ir, tys: impl Iterator<Item = &'a TypeRef>) -> String {
    go_wide_of(ir, tys, "v.raw")
}

/// [`go_wide`] over the raw JSON `raw`.
fn go_wide_of<'a>(ir: &crate::ir::Ir, tys: impl Iterator<Item = &'a TypeRef>, raw: &str) -> String {
    let tys: Vec<&TypeRef> = tys.collect();
    let (limit, float32) = checks::number_range(ir, &tys);
    format!(
        "wideNumber({raw}, {}, {float32})",
        limit.unwrap_or("math.Inf(1)")
    )
}

/// The checks on one member a struct holds raw beside `patternProperties`,
/// its name in `name` and its JSON in `value`, at `pointer`: against the
/// schema of every pattern it matches, or of `additionalProperties` when
/// it matches none.
fn pattern_members(s: &crate::ir::StructDef, view: &TypeView, cx: &mut Context<'_>) -> Vec<String> {
    // A pattern that cannot be translated may match any member, so
    // whether `additionalProperties` applies is never known.
    let others_any = s.others_accept_anything(cx.ir);
    let others_checked = !others_any && s.patterns.iter().all(|p| p.regex.is_ok());
    let mut out = Vec::new();
    if others_checked {
        out.push("matched := false".to_owned());
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
        let accepts = pattern.accepts_anything(cx.ir);
        // Untranslated, whether it matches is still not known: below.
        if accepts && !others_checked && pattern.regex.is_ok() {
            continue;
        }
        let part = if accepts {
            String::new()
        } else {
            pattern_part(pattern, ty.as_deref(), cx)
        };
        out.push(match &pattern.regex {
            Ok(regex) => {
                cx.needs.pattern = true;
                format!(
                    "if m, ok := matchPattern({}, name); !ok {{ {matched}r.unevaluated(pointer, {}) }} else if m {{ {matched}{part} }}",
                    lit(regex),
                    lit(&format!("{label}: the pattern is too large to compile, so the member is not checked against it"))
                )
            }
            // Matching or not, nothing is decided against a pattern that
            // lets anything be, when the other members may be anything too.
            Err(_) if accepts && others_any => continue,
            // Whether the member matches is not known: nor whether
            // `additionalProperties` applies to it.
            Err(reason) => format!(
                "{matched}r.unevaluated(pointer, {})",
                lit(&format!("{label}: {reason}, so whether the member matches it is not known"))
            ),
        });
    }
    match (&s.others, &view.others_ty) {
        _ if !others_checked => {}
        (crate::ir::Others::Schema(b), Some(ty)) => {
            let part = part(b, ty, "additionalProperties", cx);
            out.push(format!("if !matched {{ {part} }}"));
        }
        (crate::ir::Others::Denied, _) => out.push(format!(
            "if !matched {{ r.violation(pointer, {}) }}",
            lit("is not allowed: it matches no property and no pattern")
        )),
        _ => {}
    }
    out
}

/// What `pattern`'s schema says of a member matching it, its JSON in
/// `value`, at `pointer`.
fn pattern_part(pattern: &crate::ir::PatternDef, ty: Option<&str>, cx: &mut Context<'_>) -> String {
    let label = format!("patternProperties `{}`", pattern.source);
    match (&pattern.schema, ty) {
        (crate::ir::PatternSchema::Branch(b), Some(ty)) => part(b, ty, &label, cx),
        (crate::ir::PatternSchema::Unchecked(reason), _) => format!(
            "r.unevaluated(pointer, {})",
            lit(&format!("{label}: {reason}"))
        ),
        _ => format!(
            "r.violation(pointer, {})",
            lit(&format!("is not allowed by {label}"))
        ),
    }
}

/// The checks the patterns a declared property's name matches put on its
/// value, known at generation: the value is written out and read again
/// as each pattern's schema, as an undeclared member is.
fn declared_patterns(
    s: &crate::ir::StructDef,
    view: &TypeView,
    cx: &mut Context<'_>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (field, field_view) in s.fields.iter().zip(&view.fields) {
        let mut parts = Vec::new();
        let mut reads = false;
        // Decoded into a substitution or a 32-bit float, the value written
        // out is not the one that was sent.
        // So is one read as a float. (`jsontext.Value` keeps its text.)
        let ir = cx.ir;
        let lossy = checks::reaches_leaf(ir, &field.ty, &|ty| {
            matches!(ty, TypeRef::Extern(_) | TypeRef::Number(_))
                || matches!(ty, TypeRef::Named(id) if matches!(ir.types.get(id).map(|d| &d.kind), Some(TypeKind::Extern)))
        });
        for (pattern, ty) in s.patterns.iter().zip(&view.pattern_tys) {
            match pattern.matches_name(&field.raw_name) {
                None => {}
                // Matching or not, a pattern that lets anything be decides
                // nothing.
                Some(_) if pattern.accepts_anything(cx.ir) => {}
                Some(Ok(())) if lossy && pattern.branch().is_some() => parts.push(format!(
                    "r.unevaluated(pointer, {})",
                    lit(&format!(
                        "patternProperties `{}`: `{}` is held as a type that does not keep the value as it was sent, so it is not checked against it",
                        pattern.source, field.raw_name
                    ))
                )),
                Some(Ok(())) => {
                    reads |= pattern.branch().is_some();
                    parts.push(pattern_part(pattern, ty.as_deref(), cx));
                }
                Some(Err(reason)) => parts.push(format!(
                    "r.unevaluated(pointer, {})",
                    lit(&format!(
                        "patternProperties `{}`: {reason}, so whether `{}` matches it is not known",
                        pattern.source, field.raw_name
                    ))
                )),
            }
        }
        if parts.is_empty() {
            continue;
        }
        let name = &field_view.name;
        let checks = parts.join("; ");
        let inner = if reads {
            format!(
                "if raw, err := json.Marshal(&v.{name}); err == nil {{ value := jsontext.Value(raw); {checks} }}"
            )
        } else {
            checks
        };
        let in_nullable = field_view.ty.starts_with("Nullable[");
        let body = match (field.required, in_nullable) {
            (true, _) => inner,
            (false, true) => format!("if v.{name}.Set {{ {inner} }}"),
            (false, false) => format!("if v.{name} != nil {{ {inner} }}"),
        };
        out.push(format!(
            "{{ pointer := childPointer(pointer, {}); {body} }}",
            field_view.literal
        ));
    }
    out
}

/// One schema a raw member is read as and checked against, as an `allOf`
/// part is: a value that does not decode misses it, unless the decoder
/// rather than the value is why.
fn part(b: &Branch, ty: &str, label: &str, cx: &mut Context<'_>) -> String {
    let (mut body, reads) = branch(b, "r", cx);
    if checks::part_unsure(b) || (cx.loops)(&b.ty) {
        body.push(format!(
            "r.unevaluated(pointer, {})",
            lit(&format!("{label}: it says more than is checked"))
        ));
    }
    let fragile = if checks::reaches_extern(cx.ir, &b.ty) {
        "true".to_owned()
    } else {
        go_wide_of(cx.ir, std::iter::once(&b.ty), "value")
    };
    let miss = format!(
        "if {fragile} {{ r.unevaluated(pointer, {}) }} else {{ r.violation(pointer, {}) }}",
        lit(&format!(
            "{label}: a number in the value is past what its type holds"
        )),
        lit(&format!("does not satisfy {label}"))
    );
    if reads {
        format!(
            "{{ var x {ty}; if err := json.Unmarshal(value, &x, decodeOptions); err == nil {{ {} }} else {{ {miss} }} }}",
            body.join("; ")
        )
    } else if body.is_empty() {
        format!(
            "if err := json.Unmarshal(value, new({ty}), decodeOptions); err != nil {{ {miss} }}"
        )
    } else {
        format!(
            "if err := json.Unmarshal(value, new({ty}), decodeOptions); err == nil {{ {} }} else {{ {miss} }}",
            body.join("; ")
        )
    }
}

/// A `null` of the type `name` checked at `p` into `r`.
fn null_call(name: &str, p: &str, r: &str) -> String {
    format!("var zero {name}; zero.validateNull({p}, {r})")
}

/// The body of `validateNull` for one type: empty when a `null` of it
/// has nothing to check.
pub(crate) fn null_body(def: &TypeDef, name: &str, cx: &mut Context<'_>) -> Vec<String> {
    if checks::null_raw(def) {
        cx.needs.jsontext = true;
        return vec![format!(
            "{name}{{raw: jsontext.Value(\"null\")}}.validateInto(pointer, r)"
        )];
    }
    let mut out = Vec::new();
    render(
        &checks::null_checks(def),
        (ValueKind::Other, None),
        At {
            e: "v",
            p: "pointer",
            r: "r",
        },
        &mut out,
        &mut cx.needs,
    );
    out
}

/// The checks on a value of type `ty` at `at`, behind its pointer when it
/// has one. True when they read the value.
fn checked(
    ty: &TypeRef,
    list: &[Check],
    at: At<'_>,
    out: &mut Vec<String>,
    needs: &mut Needs,
) -> bool {
    match ty {
        TypeRef::Nullable(inner) | TypeRef::Boxed(inner) if !list.is_empty() => {
            let deref = format!("(*{})", at.e);
            let mut body = Vec::new();
            render(
                list,
                (checks::kind_of(inner), item_of(inner)),
                At { e: &deref, ..at },
                &mut body,
                needs,
            );
            if !body.is_empty() {
                out.push(format!("if {} != nil {{ {} }}", at.e, body.join("; ")));
            }
            !body.is_empty()
        }
        _ => render(list, (checks::kind_of(ty), item_of(ty)), at, out, needs),
    }
}

/// Checks then recursion on a value of type `ty` at `at`. True when the
/// statements read the value.
fn value(
    ty: &TypeRef,
    list: &[Check],
    at: At<'_>,
    out: &mut Vec<String>,
    cx: &mut Context<'_>,
) -> bool {
    let reads = checked(ty, list, at, out, &mut cx.needs);
    let before = out.len();
    recurse(ty, at, 0, out, cx);
    reads || out.len() > before
}

/// The checks and recursion on one branch's decoded value `x`, into the
/// report `r`, and whether they read `x`.
fn branch(branch: &Branch, r: &str, cx: &mut Context<'_>) -> (Vec<String>, bool) {
    let mut body = Vec::new();
    let at = At {
        e: "x",
        p: "pointer",
        r,
    };
    // What applies to null as much as to a value is checked outside the
    // null guard.
    render(
        &[
            checks::presence_checks(&branch.ty, &branch.constraints),
            (cx.extern_checks)(&branch.ty).presence,
        ]
        .concat(),
        (ValueKind::Other, None),
        at,
        &mut body,
        &mut cx.needs,
    );
    let mut list = checks::value_checks(&branch.ty, &branch.constraints);
    list.extend((cx.extern_checks)(&branch.ty).value);
    let reads = if (cx.loops)(&branch.ty) {
        // Checks only: recursing would come back here.
        render(
            &list,
            (checks::kind_of(&branch.ty), item_of(&branch.ty)),
            at,
            &mut body,
            &mut cx.needs,
        )
    } else {
        value(&branch.ty, &list, at, &mut body, cx)
    };
    (body, reads)
}

/// The body of `validateInto` for one type, or `None` when it has no
/// methods of its own (an alias).
pub(crate) fn body(
    def: &TypeDef,
    view: &TypeView,
    opted_out: bool,
    cx: &mut Context<'_>,
) -> Option<Vec<String>> {
    if view.kind == "alias" {
        return None;
    }
    let mut out = Vec::new();
    if opted_out {
        return Some(out);
    }
    let own = At {
        e: "v",
        p: "pointer",
        r: "r",
    };
    // A holder's own `const` constrains the whole value, null included,
    // whatever its branches say.
    if matches!(
        def.kind,
        TypeKind::UnionTagged(_) | TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_)
    ) {
        render(
            &checks::unenforced(&def.constraints),
            (ValueKind::Other, None),
            own,
            &mut out,
            &mut cx.needs,
        );
    }
    match &def.kind {
        TypeKind::Struct(s) => {
            let mut members = Vec::new();
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let mut list = checks::value_checks(&field.ty, &field.constraints);
                list.extend((cx.extern_checks)(&field.ty).value);
                let in_nullable = field_view.ty.starts_with("Nullable[");
                let presence = match (field.required, in_nullable) {
                    (true, _) => None,
                    (false, true) => Some(format!("v.{name}.Set")),
                    (false, false) => Some(format!("v.{name} != nil")),
                };
                members.push(match &presence {
                    None => "members++".to_owned(),
                    Some(test) => format!("if {test} {{ members++ }}"),
                });
                // What applies to a present null as much as to a value.
                let mut always = Vec::new();
                render(
                    &[
                        checks::presence_checks(&field.ty, &field.constraints),
                        (cx.extern_checks)(&field.ty).presence,
                    ]
                    .concat(),
                    (ValueKind::Other, None),
                    At {
                        e: "x",
                        p: "p",
                        r: "r",
                    },
                    &mut always,
                    &mut cx.needs,
                );
                let inner_ty = match &field.ty {
                    TypeRef::Boxed(inner) => inner.as_ref(),
                    other => other,
                };
                let mut body = Vec::new();
                let reads = value(
                    inner_ty,
                    &list,
                    At {
                        e: "x",
                        p: "p",
                        r: "r",
                    },
                    &mut body,
                    cx,
                );
                if body.is_empty() && always.is_empty() {
                    continue;
                }
                let mut block = vec![format!(
                    "p := childPointer(pointer, {})",
                    field_view.literal
                )];
                if !always.is_empty() {
                    block.push(match &presence {
                        None => always.join("; "),
                        Some(test) => format!("if {test} {{ {} }}", always.join("; ")),
                    });
                }
                if body.is_empty() {
                    out.push(format!("{{ {} }}", block.join("; ")));
                    continue;
                }
                // Bound only when read; Go refuses an unused variable.
                let bind = |access: &str| {
                    if reads {
                        format!("x := {access}; ")
                    } else {
                        String::new()
                    }
                };
                let holder = (cx.raw_holder)(inner_ty);
                let statements = body.join("; ");
                if let Some(holder) = holder.filter(|_| in_nullable && !field.nullable) {
                    // A raw holder is in a `Nullable` because `null` is one
                    // of the values it holds, not because the schema allows
                    // null outright: a present null is validated too.
                    cx.needs.jsontext = true;
                    block.push(format!(
                        "if v.{name}.Set {{ x := {holder}; if v.{name}.Value != nil {{ x = *v.{name}.Value }}; {statements} }}"
                    ));
                } else if in_nullable {
                    let null = (cx.null_name)(inner_ty)
                        .map(|t| format!(" else if v.{name}.Set {{ {} }}", null_call(&t, "p", "r")))
                        .unwrap_or_default();
                    block.push(format!(
                        "if v.{name}.Value != nil {{ {}{statements} }}{null}",
                        bind(&format!("(*v.{name}.Value)"))
                    ));
                } else if field_view.ty.starts_with('*') {
                    let null = (cx.null_name)(inner_ty)
                        .filter(|_| field.nullable)
                        .map(|t| format!(" else {{ {} }}", null_call(&t, "p", "r")))
                        .unwrap_or_default();
                    block.push(format!(
                        "if v.{name} != nil {{ {}{statements} }}{null}",
                        bind(&format!("(*v.{name})"))
                    ));
                } else {
                    block.push(format!("{}{statements}", bind(&format!("v.{name}"))));
                }
                out.push(format!("{{ {} }}", block.join("; ")));
            }
            if let Additional::CatchAll(ty) = &s.additional
                && !s.patterns.is_empty()
            {
                let name = &view.extra_name;
                let checks = pattern_members(s, view, cx);
                // A member is read only against a pattern's schema: with
                // every pattern untranslated, never.
                let read =
                    s.patterns.iter().any(|p| {
                        p.regex.is_ok() && p.branch().is_some() && !p.accepts_anything(cx.ir)
                    }) || (matches!(s.others, crate::ir::Others::Schema(_))
                        && !s.others_accept_anything(cx.ir)
                        && s.patterns.iter().all(|p| p.regex.is_ok()));
                let value = if read {
                    format!("value := v.{name}[name]; ")
                } else {
                    String::new()
                };
                // Nothing a member could be is decided: no loop.
                if !checks.is_empty() {
                    cx.needs.sorted = true;
                    out.push(format!(
                        "for _, name := range slices.Sorted(maps.Keys(v.{name})) {{ {value}pointer := childPointer(pointer, name); {} }}",
                        checks.join("; ")
                    ));
                }
                out.extend(declared_patterns(s, view, cx));
                let _ = ty;
            }
            if let Additional::CatchAll(ty) = &s.additional {
                let name = &view.extra_name;
                members.push(format!("members += len(v.{name})"));
                let map = format!("v.{name}");
                recurse(
                    &TypeRef::Map(Box::new(ty.clone())),
                    At { e: &map, ..own },
                    0,
                    &mut out,
                    cx,
                );
                render(
                    &(cx.extern_checks)(&TypeRef::Map(Box::new(ty.clone())))
                        .value
                        .into_iter()
                        .chain(checks::each(
                            &TypeRef::Map(Box::new(ty.clone())),
                            &def.constraints,
                        ))
                        .collect::<Vec<_>>(),
                    (ValueKind::Map, None),
                    At { e: &map, ..own },
                    &mut out,
                    &mut cx.needs,
                );
            }
            let list = checks::object_checks(&def.constraints);
            let counted = list
                .iter()
                .any(|c| matches!(c, Check::MinProperties(_) | Check::MaxProperties(_)));
            if counted {
                out.push("members := 0".to_owned());
                out.extend(members);
            }
            for check in &list {
                match check {
                    Check::MinProperties(n) => out.push(format!(
                        "if uint64(members) < {n} {{ r.violation(pointer, fmt.Sprintf({}, members)) }}",
                        lit(&format!("has %d members, fewer than minProperties {n}"))
                    )),
                    Check::MaxProperties(n) => out.push(format!(
                        "if uint64(members) > {n} {{ r.violation(pointer, fmt.Sprintf({}, members)) }}",
                        lit(&format!("has %d members, more than maxProperties {n}"))
                    )),
                    // Declared names are known now: only whether the member
                    // is present is left to run time. The other members'
                    // names are read from the catch-all.
                    Check::Keys(names) => {
                        for (field, field_view) in s.fields.iter().zip(&view.fields) {
                            let pointer = format!("childPointer(pointer, {})", lit(&field.raw_name));
                            let statement = match checks::name_verdict(&field.raw_name, names) {
                                checks::Verdict::Pass => continue,
                                checks::Verdict::Fail(message) => {
                                    format!("r.violation({pointer}, {})", lit(&message))
                                }
                                checks::Verdict::Unknown(message) => {
                                    format!("r.unevaluated({pointer}, {})", lit(&message))
                                }
                            };
                            let name = &field_view.name;
                            out.push(if field.required {
                                statement
                            } else if field_view.ty.starts_with("Nullable[") {
                                format!("if v.{name}.Set {{ {statement} }}")
                            } else {
                                format!("if v.{name} != nil {{ {statement} }}")
                            });
                        }
                        if matches!(s.additional, Additional::CatchAll(_)) {
                            let map = format!("v.{}", view.extra_name);
                            render(
                                std::slice::from_ref(check),
                                (ValueKind::Map, None),
                                At { e: &map, ..own },
                                &mut out,
                                &mut cx.needs,
                            );
                        }
                    }
                    other => {
                        render(
                            std::slice::from_ref(other),
                            (ValueKind::Other, None),
                            own,
                            &mut out,
                            &mut cx.needs,
                        );
                    }
                }
            }
        }
        TypeKind::StringEnum(_) => {
            // Membership is the decoder's; the other string constraints
            // apply to the member's wire value.
            let list = checks::checks(&TypeRef::String, &def.constraints);
            render(
                &list,
                (ValueKind::String, None),
                own,
                &mut out,
                &mut cx.needs,
            );
        }
        TypeKind::UnionTagged(u) => {
            for (variant, variant_view) in u.variants.iter().zip(&view.variants) {
                let mut body = Vec::new();
                recurse(&variant.ty, At { e: "x", ..own }, 0, &mut body, cx);
                // A substituted variant is not checked against what it
                // declares.
                let ext = (cx.extern_checks)(&variant.ty);
                let mut unchecked = Vec::new();
                render(
                    &[ext.presence, ext.value].concat(),
                    (ValueKind::Other, None),
                    At { e: "x", ..own },
                    &mut unchecked,
                    &mut cx.needs,
                );
                if !unchecked.is_empty() {
                    out.push(format!(
                        "if _, ok := v.{}(); ok {{ {} }}",
                        variant_view.accessor,
                        unchecked.join("; ")
                    ));
                }
                if !body.is_empty() {
                    out.push(format!(
                        "if x, ok := v.{}(); ok {{ {} }}",
                        variant_view.accessor,
                        body.join("; ")
                    ));
                }
            }
        }
        TypeKind::UnionOpen(u) => {
            if def.nullable {
                // A raw null is a value its schema allows, under what every
                // member says.
                let mut null = Vec::new();
                render(
                    &checks::shared_unsupported(cx.ir, &u.branches),
                    (ValueKind::Other, None),
                    own,
                    &mut null,
                    &mut cx.needs,
                );
                null.push("return".to_owned());
                out.push(format!("if v.raw.Kind() == 'n' {{ {} }}", null.join("; ")));
            }
            out.push("certain, uncertain := 0, 0".to_owned());
            let wide = go_wide(cx.ir, u.branches.iter().map(|b| &b.ty));
            for (b, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let (body, reads) = branch(b, "branch", cx);
                // Decoding proves a match only when nothing the branch says
                // goes unchecked.
                let counter = if checks::unsure(b) || (cx.loops)(&b.ty) {
                    "uncertain"
                } else {
                    "certain"
                };
                // A number past what the branch's type holds fails to decode
                // for the representation: no proof it matches no branch.
                // A substitution's decoding is the user's: its failure is
                // no proof either.
                let beyond = if checks::reaches_extern(cx.ir, &b.ty) {
                    " else { uncertain++ }".to_owned()
                } else {
                    format!(" else if {wide} {{ uncertain++ }}")
                };
                if body.is_empty() {
                    out.push(format!(
                        "if _, ok := v.{accessor}(); ok {{ {counter}++ }}{beyond}"
                    ));
                } else {
                    let binding = if reads { "x" } else { "_" };
                    out.push(format!(
                        "if {binding}, ok := v.{accessor}(); ok {{ branch := r.nested(pointer); {}; r.resume(pointer, branch); if len(branch.Violations()) == 0 {{ if len(branch.Unchecked()) == 0 {{ {counter}++ }} else {{ uncertain++ }} }} }}{beyond}",
                        body.join("; ")
                    ));
                }
            }
            if u.exclusive {
                out.push("if certain+uncertain == 0 { r.violation(pointer, \"matches none of the oneOf branches\") } else if certain > 1 { r.violation(pointer, fmt.Sprintf(\"matches %d oneOf branches; exactly one must\", certain)) } else if !(certain == 1 && uncertain == 0) { r.unevaluated(pointer, \"oneOf: whether exactly one branch matches depends on constraints that were not evaluated\") }".to_owned());
            } else {
                out.push("if certain == 0 { if uncertain == 0 { r.violation(pointer, \"matches none of the anyOf branches\") } else { r.unevaluated(pointer, \"anyOf: whether a branch matches depends on constraints that were not evaluated\") } }".to_owned());
            }
        }
        TypeKind::ComposedOpen(branches) => {
            // An intersection is nullable when any side is, so null is left
            // to every part to accept.
            let wide = go_wide(cx.ir, branches.iter().map(|b| &b.ty));
            for (b, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = lit(&format!(
                    "does not satisfy the allOf part read by {accessor}"
                ));
                let (mut body, reads) = branch(b, "r", cx);
                if checks::part_unsure(b) || (cx.loops)(&b.ty) {
                    body.push(format!(
                        "r.unevaluated(pointer, {})",
                        lit(&format!(
                            "allOf part read by {accessor}: it says more than is checked"
                        ))
                    ));
                }
                // A number past what the part's type holds fails to decode for
                // the representation: undecided, not a miss.
                let fragile = if checks::reaches_extern(cx.ir, &b.ty) {
                    "true"
                } else {
                    wide.as_str()
                };
                let miss = format!(
                    "if {fragile} {{ r.unevaluated(pointer, {}) }} else {{ r.violation(pointer, {message}) }}",
                    lit(&format!(
                        "allOf part read by {accessor}: a number in the value is past what its type holds"
                    ))
                );
                let part = if body.is_empty() {
                    format!("if _, ok := v.{accessor}(); !ok {{ {miss} }}")
                } else {
                    let binding = if reads { "x" } else { "_" };
                    format!(
                        "if {binding}, ok := v.{accessor}(); ok {{ {} }} else {{ {miss} }}",
                        body.join("; ")
                    )
                };
                // A typeless schema's object keywords say nothing about a
                // value that is not an object.
                out.push(if b.object_only {
                    format!("if v.raw.Kind() == '{{' {{ {part} }}")
                } else {
                    part
                });
            }
        }
        TypeKind::Newtype(inner) | TypeKind::Alias(inner) => {
            // A defined type does not inherit its underlying type's
            // methods: recursion goes through a conversion.
            render(
                &[
                    checks::presence_checks(inner, &def.constraints),
                    (cx.extern_checks)(inner).presence,
                ]
                .concat(),
                (ValueKind::Other, None),
                own,
                &mut out,
                &mut cx.needs,
            );
            let mut list = checks::value_checks(inner, &def.constraints);
            list.extend((cx.extern_checks)(inner).value);
            render(
                &list,
                (checks::kind_of(inner), item_of(inner)),
                own,
                &mut out,
                &mut cx.needs,
            );
            let converted = format!("{}(v)", view.inner);
            let target = if matches!(inner, TypeRef::Named(_)) {
                converted.as_str()
            } else {
                "v"
            };
            recurse(inner, At { e: target, ..own }, 0, &mut out, cx);
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
                "if !r.enter(pointer, 0) { return }".to_owned()
            } else {
                // Each branch decodes the whole raw value again.
                format!("if !r.enter(pointer, {branches}*max(len(v.raw), 640)) {{ return }}")
            },
            "defer func() { r.depth-- }()".to_owned(),
        ];
        wrapped.extend(out);
        out = wrapped;
    }
    Some(out)
}
