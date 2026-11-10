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
    /// A `pattern` is matched: `regexp`, `sync` and `matchPattern`.
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
    let tys: Vec<&TypeRef> = tys.collect();
    let (limit, float32) = checks::number_range(ir, &tys);
    format!(
        "wideNumber(v.raw, {}, {float32})",
        limit.unwrap_or("math.Inf(1)")
    )
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
