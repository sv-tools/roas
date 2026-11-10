//! Spelling [`Check`]s as Go: the body of each generated type's
//! `validateInto`. Go has no blanket implementations, so recursion into
//! slices, maps and pointers is written out from the type's shape.

use crate::checks::{self, Check, Unique, ValueKind};
use crate::front::IntWidth;
use crate::go::view::TypeView;
use crate::ir::{Additional, TypeDef, TypeKind, TypeRef};

/// What the rendered checks need imported.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Needs {
    pub utf8: bool,
    pub math: bool,
    /// Map keys walked in order: `maps` and `slices`.
    pub sorted: bool,
}

pub(crate) struct Context<'a> {
    /// The type holds a generated type with its own `validateInto`.
    pub validates: &'a dyn Fn(&TypeRef) -> bool,
    /// A named type spelled as a pointer alias, which may be nil.
    pub nil_able: &'a dyn Fn(&TypeRef) -> bool,
    pub needs: Needs,
}

/// Go's `%q`-free literal for a message: the text is ours, ASCII and
/// quote-free except where a literal from the description is quoted.
fn lit(text: &str) -> String {
    crate::go::naming::quote(text)
}

/// Statements checking `list` on the value expression `e`, findings at the
/// pointer expression `p` into the report `r`. True when they read `e`.
fn render(
    list: &[Check],
    (kind, item): (ValueKind, Option<&TypeRef>),
    (e, p, r): (&str, &str, &str),
    out: &mut Vec<String>,
    needs: &mut Needs,
) -> bool {
    let reads = list.iter().any(|check| {
        !matches!(
            check,
            Check::Unevaluated { .. }
                | Check::NumMultipleOf {
                    integral: false,
                    ..
                }
        )
    });
    let (floor, ceiling) = match kind {
        ValueKind::Integer(IntWidth::Int32) => (i128::from(i32::MIN), i128::from(i32::MAX)),
        _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
    };
    for check in list {
        match check {
            Check::MinLength(n) => {
                needs.utf8 = true;
                out.push(format!(
                    "if n := utf8.RuneCountInString(string({e})); n < {n} {{ {r}.violation({p}, fmt.Sprintf({}, n)) }}",
                    lit(&format!("is %d characters long, shorter than minLength {n}"))
                ));
            }
            Check::MaxLength(n) => {
                needs.utf8 = true;
                out.push(format!(
                    "if n := utf8.RuneCountInString(string({e})); n > {n} {{ {r}.violation({p}, fmt.Sprintf({}, n)) }}",
                    lit(&format!("is %d characters long, longer than maxLength {n}"))
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
                if *divisor > i128::from(i64::MAX) {
                    // Larger than any value: only zero is a multiple.
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
            } => {
                let (op, keyword) = if *exclusive {
                    (">", "exclusiveMinimum")
                } else {
                    (">=", "minimum")
                };
                out.push(format!(
                    "if !(float64({e}) {op} {bound:?}) {{ {r}.violation({p}, fmt.Sprintf({}, float64({e}))) }}",
                    lit(&format!("%v is below {keyword} {literal}"))
                ));
            }
            Check::NumAtMost {
                bound,
                exclusive,
                literal,
            } => {
                let (op, keyword) = if *exclusive {
                    ("<", "exclusiveMaximum")
                } else {
                    ("<=", "maximum")
                };
                out.push(format!(
                    "if !(float64({e}) {op} {bound:?}) {{ {r}.violation({p}, fmt.Sprintf({}, float64({e}))) }}",
                    lit(&format!("%v is above {keyword} {literal}"))
                ));
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
            Check::MinItems(n) => out.push(format!(
                "if len({e}) < {n} {{ {r}.violation({p}, fmt.Sprintf({}, len({e}))) }}",
                lit(&format!("has %d items, fewer than minItems {n}"))
            )),
            Check::MaxItems(n) => out.push(format!(
                "if len({e}) > {n} {{ {r}.violation({p}, fmt.Sprintf({}, len({e}))) }}",
                lit(&format!("has %d items, more than maxItems {n}"))
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
            Check::MinProperties(n) => out.push(format!(
                "if len({e}) < {n} {{ {r}.violation({p}, fmt.Sprintf({}, len({e}))) }}",
                lit(&format!("has %d members, fewer than minProperties {n}"))
            )),
            Check::MaxProperties(n) => out.push(format!(
                "if len({e}) > {n} {{ {r}.violation({p}, fmt.Sprintf({}, len({e}))) }}",
                lit(&format!("has %d members, more than maxProperties {n}"))
            )),
            Check::Unevaluated { keyword, reason } => out.push(format!(
                "{r}.unevaluated({p}, {})",
                lit(&format!("{keyword}: {reason}"))
            )),
        }
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
fn recurse(
    ty: &TypeRef,
    e: &str,
    p: &str,
    r: &str,
    depth: usize,
    out: &mut Vec<String>,
    cx: &mut Context<'_>,
) {
    if !(cx.validates)(ty) {
        return;
    }
    match ty {
        TypeRef::Named(_) if (cx.nil_able)(ty) => {
            out.push(format!("if {e} != nil {{ {e}.validateInto({p}, {r}) }}"))
        }
        TypeRef::Named(_) => out.push(format!("{e}.validateInto({p}, {r})")),
        TypeRef::Boxed(inner) | TypeRef::Nullable(inner) => {
            let mut body = Vec::new();
            recurse(inner, &format!("(*{e})"), p, r, depth, &mut body, cx);
            out.push(format!("if {e} != nil {{ {} }}", body.join("; ")));
        }
        TypeRef::Array(inner) => {
            let (i, item) = (format!("i{depth}"), format!("item{depth}"));
            let mut body = Vec::new();
            recurse(
                inner,
                &item,
                &format!("childPointer({p}, fmt.Sprint({i}))"),
                r,
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
            let mut body = Vec::new();
            recurse(
                inner,
                &format!("{e}[{key}]"),
                &format!("childPointer({p}, {key})"),
                r,
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

/// Checks then recursion on a value of type `ty` at `e`.
fn value(
    ty: &TypeRef,
    list: &[Check],
    e: &str,
    p: &str,
    out: &mut Vec<String>,
    cx: &mut Context<'_>,
) -> bool {
    let kind = checks::kind_of(ty);
    let reads = render(list, (kind, item_of(ty)), (e, p, "r"), out, &mut cx.needs);
    let before = out.len();
    recurse(ty, e, p, "r", 0, out, cx);
    reads || out.len() > before
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
    match &def.kind {
        TypeKind::Struct(s) => {
            let mut members = Vec::new();
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let list = checks::checks(&field.ty, &field.constraints);
                // The field holds the value directly, behind a pointer, or in
                // a `Nullable`.
                let (guard, access) = if field_view.ty.starts_with("Nullable[") {
                    (
                        Some(format!("v.{name}.Value != nil")),
                        format!("(*v.{name}.Value)"),
                    )
                } else if field_view.ty.starts_with('*') {
                    (Some(format!("v.{name} != nil")), format!("(*v.{name})"))
                } else {
                    (None, format!("v.{name}"))
                };
                members.push(
                    match (field.required, field_view.ty.starts_with("Nullable[")) {
                        (true, _) => "members++".to_owned(),
                        (false, true) => format!("if v.{name}.Set {{ members++ }}"),
                        (false, false) => format!("if v.{name} != nil {{ members++ }}"),
                    },
                );
                let inner_ty = match &field.ty {
                    TypeRef::Boxed(inner) => inner.as_ref(),
                    other => other,
                };
                let mut body = Vec::new();
                let reads = value(inner_ty, &list, "x", "p", &mut body, cx);
                if body.is_empty() {
                    continue;
                }
                let mut block = vec![format!(
                    "p := childPointer(pointer, {})",
                    field_view.literal
                )];
                // Bound only when read; Go refuses an unused variable.
                let bind = if reads {
                    format!("x := {access}; ")
                } else {
                    String::new()
                };
                match guard {
                    Some(guard) => {
                        block.push(format!("if {guard} {{ {bind}{} }}", body.join("; ")))
                    }
                    None => block.push(format!("{bind}{}", body.join("; "))),
                }
                out.push(format!("{{ {} }}", block.join("; ")));
            }
            if let Additional::CatchAll(ty) = &s.additional {
                let name = &view.extra_name;
                members.push(format!("members += len(v.{name})"));
                recurse(
                    &TypeRef::Map(Box::new(ty.clone())),
                    &format!("v.{name}"),
                    "pointer",
                    "r",
                    0,
                    &mut out,
                    cx,
                );
            }
            let list = checks::object_checks(&def.constraints);
            if !list.is_empty() {
                out.push("members := 0".to_owned());
                out.extend(members);
                for check in &list {
                    match check {
                        Check::MinProperties(n) => out.push(format!(
                            "if members < {n} {{ r.violation(pointer, fmt.Sprintf({}, members)) }}",
                            lit(&format!("has %d members, fewer than minProperties {n}"))
                        )),
                        Check::MaxProperties(n) => out.push(format!(
                            "if members > {n} {{ r.violation(pointer, fmt.Sprintf({}, members)) }}",
                            lit(&format!("has %d members, more than maxProperties {n}"))
                        )),
                        other => {
                            render(
                                std::slice::from_ref(other),
                                (ValueKind::Other, None),
                                ("v", "pointer", "r"),
                                &mut out,
                                &mut cx.needs,
                            );
                        }
                    }
                }
            }
        }
        TypeKind::UnionTagged(u) => {
            for (variant, variant_view) in u.variants.iter().zip(&view.variants) {
                let mut body = Vec::new();
                recurse(&variant.ty, "x", "pointer", "r", 0, &mut body, cx);
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
            out.push("certain, uncertain := 0, 0".to_owned());
            for (branch, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let mut body = Vec::new();
                recurse(&branch.ty, "x", "pointer", "branch", 0, &mut body, cx);
                if body.is_empty() {
                    out.push(format!("if _, ok := v.{accessor}(); ok {{ certain++ }}"));
                } else {
                    out.push(format!(
                        "if x, ok := v.{accessor}(); ok {{ branch := &ValidationReport{{}}; {}; if len(branch.Violations()) == 0 {{ if len(branch.Unchecked()) == 0 {{ certain++ }} else {{ uncertain++ }} }} }}",
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
            for (branch, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = lit(&format!(
                    "does not satisfy the allOf part read by {accessor}"
                ));
                let mut body = Vec::new();
                recurse(&branch.ty, "x", "pointer", "r", 0, &mut body, cx);
                if body.is_empty() {
                    out.push(format!(
                        "if _, ok := v.{accessor}(); !ok {{ r.violation(pointer, {message}) }}"
                    ));
                } else {
                    out.push(format!(
                        "if x, ok := v.{accessor}(); ok {{ {} }} else {{ r.violation(pointer, {message}) }}",
                        body.join("; ")
                    ));
                }
            }
        }
        TypeKind::Newtype(inner) | TypeKind::Alias(inner) => {
            // A defined type does not inherit its underlying type's
            // methods: recursion goes through a conversion.
            let list = checks::checks(inner, &def.constraints);
            let converted = format!("{}(v)", view.inner);
            let kind = checks::kind_of(inner);
            render(
                &list,
                (kind, item_of(inner)),
                ("v", "pointer", "r"),
                &mut out,
                &mut cx.needs,
            );
            let target = if matches!(inner, TypeRef::Named(_)) {
                converted.as_str()
            } else {
                "v"
            };
            recurse(inner, target, "pointer", "r", 0, &mut out, cx);
        }
        TypeKind::StringEnum(_) | TypeKind::Extern => {}
    }
    Some(out)
}
