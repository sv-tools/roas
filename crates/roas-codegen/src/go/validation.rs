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
    /// A raw holder is built from `null`: `jsontext`.
    pub jsontext: bool,
}

pub(crate) struct Context<'a> {
    /// The type holds a generated type with its own `validateInto`.
    pub validates: &'a dyn Fn(&TypeRef) -> bool,
    /// A named type spelled as a pointer alias, which may be nil.
    pub nil_able: &'a dyn Fn(&TypeRef) -> bool,
    /// The Go name of the raw holder (open union or composition) `ty`
    /// names, if it names one.
    pub raw_holder: &'a dyn Fn(&TypeRef) -> Option<String>,
    /// A branch that comes back to the holder being validated with the
    /// same raw value: decoded, never validated, or it would not finish.
    pub loops: &'a dyn Fn(&TypeRef) -> bool,
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
                    // The bound as float32 rounds it, exactly as Rust does;
                    // left to Go's constant conversion a midpoint could
                    // round the other way.
                    out.push(format!(
                        "if !({value} {op} {:?}) {{ {violation} }}",
                        *bound as f32
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
            Check::UniqueItems(Unique::Float) => out.push(format!(
                "{{ repeated := false; for i := range {e} {{ for j := 0; j < i; j++ {{ if {e}[i] == {e}[j] {{ repeated = true }} }} }}; if repeated {{ {r}.unevaluated({p}, \"uniqueItems: equal floats may come from different numbers\") }} }}"
            )),
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
        }
        reads |= out.len() > before
            && !matches!(
                check,
                Check::Unevaluated { .. }
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
            out.push(format!("if {e} != nil {{ {} }}", body.join("; ")));
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

/// Checks then recursion on a value of type `ty` at `at`. True when the
/// statements read the value.
fn value(
    ty: &TypeRef,
    list: &[Check],
    at: At<'_>,
    out: &mut Vec<String>,
    cx: &mut Context<'_>,
) -> bool {
    let reads = match ty {
        // The checks apply to the value behind the pointer, when there is
        // one.
        TypeRef::Nullable(inner) | TypeRef::Boxed(inner) if !list.is_empty() => {
            let deref = format!("(*{})", at.e);
            let mut body = Vec::new();
            render(
                list,
                (checks::kind_of(inner), item_of(inner)),
                At { e: &deref, ..at },
                &mut body,
                &mut cx.needs,
            );
            if !body.is_empty() {
                out.push(format!("if {} != nil {{ {} }}", at.e, body.join("; ")));
            }
            !body.is_empty()
        }
        _ => render(
            list,
            (checks::kind_of(ty), item_of(ty)),
            at,
            out,
            &mut cx.needs,
        ),
    };
    let before = out.len();
    recurse(ty, at, 0, out, cx);
    reads || out.len() > before
}

/// The checks and recursion on one branch's decoded value `x`, into the
/// report `r`, and whether they read `x`.
fn branch(branch: &Branch, r: &str, cx: &mut Context<'_>) -> (Vec<String>, bool) {
    let list = checks::checks(&branch.ty, &branch.constraints);
    let mut body = Vec::new();
    let at = At {
        e: "x",
        p: "pointer",
        r,
    };
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
    match &def.kind {
        TypeKind::Struct(s) => {
            let mut members = Vec::new();
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let list = checks::checks(&field.ty, &field.constraints);
                let in_nullable = field_view.ty.starts_with("Nullable[");
                members.push(match (field.required, in_nullable) {
                    (true, _) => "members++".to_owned(),
                    (false, true) => format!("if v.{name}.Set {{ members++ }}"),
                    (false, false) => format!("if v.{name} != nil {{ members++ }}"),
                });
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
                if body.is_empty() {
                    continue;
                }
                let mut block = vec![format!(
                    "p := childPointer(pointer, {})",
                    field_view.literal
                )];
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
                        "if v.{name}.Set {{ x := {holder}{{raw: jsontext.Value(\"null\")}}; if v.{name}.Value != nil {{ x = *v.{name}.Value }}; {statements} }}"
                    ));
                } else if in_nullable {
                    block.push(format!(
                        "if v.{name}.Value != nil {{ {}{statements} }}",
                        bind(&format!("(*v.{name}.Value)"))
                    ));
                } else if field_view.ty.starts_with('*') {
                    block.push(format!(
                        "if v.{name} != nil {{ {}{statements} }}",
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
                // A raw null is a value its schema allows.
                out.push("if v.raw.Kind() == 'n' { return }".to_owned());
            }
            out.push("certain, uncertain := 0, 0".to_owned());
            for (b, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let (body, reads) = branch(b, "branch", cx);
                // Decoding proves a match only when nothing the branch says
                // goes unchecked.
                let counter = if b.unchecked || (cx.loops)(&b.ty) {
                    "uncertain"
                } else {
                    "certain"
                };
                if body.is_empty() {
                    out.push(format!("if _, ok := v.{accessor}(); ok {{ {counter}++ }}"));
                } else {
                    let binding = if reads { "x" } else { "_" };
                    out.push(format!(
                        "if {binding}, ok := v.{accessor}(); ok {{ branch := &ValidationReport{{}}; {}; if len(branch.Violations()) == 0 {{ if len(branch.Unchecked()) == 0 {{ {counter}++ }} else {{ uncertain++ }} }} }}",
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
            if def.nullable {
                out.push("if v.raw.Kind() == 'n' { return }".to_owned());
            }
            for (b, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = lit(&format!(
                    "does not satisfy the allOf part read by {accessor}"
                ));
                let (mut body, reads) = branch(b, "r", cx);
                if b.unchecked || (cx.loops)(&b.ty) {
                    body.push(format!(
                        "r.unevaluated(pointer, {})",
                        lit(&format!(
                            "allOf part read by {accessor}: it says more than is checked"
                        ))
                    ));
                }
                let part = if body.is_empty() {
                    format!("if _, ok := v.{accessor}(); !ok {{ r.violation(pointer, {message}) }}")
                } else {
                    let binding = if reads { "x" } else { "_" };
                    format!(
                        "if {binding}, ok := v.{accessor}(); ok {{ {} }} else {{ r.violation(pointer, {message}) }}",
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
            let list = checks::checks(inner, &def.constraints);
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
    Some(out)
}
