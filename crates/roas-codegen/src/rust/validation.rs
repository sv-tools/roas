//! Spelling [`Check`]s as Rust: the body of each generated type's
//! `Validate::validate_into`.

use crate::checks::{self, Check, Unique, ValueKind};
use crate::front::{FloatWidth, IntWidth};
use crate::ir::{Additional, Branch, TypeDef, TypeKind, TypeRef};
use crate::rust::view::TypeView;

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
                let (op, keyword, word) = match (at_least, exclusive) {
                    (true, false) => (">=", "minimum", "below"),
                    (true, true) => (">", "exclusiveMinimum", "below"),
                    (false, false) => ("<=", "maximum", "above"),
                    (false, true) => ("<", "exclusiveMaximum", "above"),
                };
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
                    Ok(rounded) if inexact => out.push(format!(
                        "if {value} == {rounded} {{ {r}.unevaluated({p}, \"{keyword} {literal}: the value rounds to the same float as the bound\"); }} else if !({value} {op} {rounded}) {{ {violation}; }}"
                    )),
                    Ok(bound) => out.push(format!(
                        "if !({value} {op} {bound}) {{ {violation}; }}"
                    )),
                    // Past every float32 but not every f64: compared wide,
                    // so a value decoded as an infinity is still caught.
                    // Past every float32 but not every f64: an infinity
                    // came from some literal past the float range, on
                    // either side of the bound.
                    Err(bound) if bound.is_finite() => out.push(format!(
                        "if {v}.is_infinite() {{ {r}.unevaluated({p}, \"{keyword} {literal}: the value is past the float range\"); }} else if !(f64::from(*{v}) {op} {bound:?}) {{ {violation}; }}"
                    )),
                    // Beyond every value: always or never met.
                    Err(bound) if (bound > 0.0) == at_least => out.push(format!("{violation};")),
                    Err(_) => {}
                }
            }
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
                "if {v}.iter().enumerate().any(|(i, a)| {v}[..i].contains(a)) {{ {r}.unevaluated({p}, \"uniqueItems: equal floats may come from different numbers\"); }}"
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
    branch: &Branch,
    generated: &dyn Fn(&TypeRef) -> bool,
    (r, arg): (&str, &str),
) -> (Vec<String>, bool) {
    // What applies to null as much as to a value is checked outside the
    // null guard.
    let mut body = Vec::new();
    render(
        &checks::presence_checks(&branch.ty, &branch.constraints),
        ValueKind::Other,
        At {
            v: "(&v)",
            p: "pointer",
            r,
        },
        &mut body,
    );
    let list = checks::value_checks(&branch.ty, &branch.constraints);
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

/// The body of `validate_into` for one type, or `None` when it gets no
/// implementation (a type alias shares its target's).
pub(crate) fn body(
    ir: &crate::ir::Ir,
    def: &TypeDef,
    view: &TypeView,
    generated: &dyn Fn(&TypeRef) -> bool,
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
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let list = checks::value_checks(&field.ty, &field.constraints);
                let recurse = generated(&field.ty);
                present.push(if field.required {
                    "1".to_owned()
                } else {
                    format!("usize::from(self.{name}.is_some())")
                });
                // What applies to a present null as much as to a value.
                let mut always = Vec::new();
                render(
                    &checks::presence_checks(&field.ty, &field.constraints),
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
                // Every check may have been decided away.
                if inner.is_empty() && !recurse && always.is_empty() {
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
                out.push(format!("{{ {} }}", block.join(" ")));
            }
            if let Additional::CatchAll(ty) = &s.additional {
                let name = &view.fields.last().expect("the catch-all field").name;
                present.push(format!("self.{name}.len()"));
                if generated(ty) {
                    out.push(format!("self.{name}.validate_into(pointer, report);"));
                }
            }
            let list = checks::object_checks(&def.constraints);
            let counted = list
                .iter()
                .any(|c| matches!(c, Check::MinProperties(_) | Check::MaxProperties(_)));
            if counted {
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
        TypeKind::StringEnum(_) => {
            // Membership is the decoder's; the other string constraints
            // apply to the member's wire value.
            let list = checks::checks(&TypeRef::String, &def.constraints);
            if !list.is_empty() {
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
                    if generated(&variant.ty) {
                        format!(
                            "Self::{}(v) => v.validate_into(pointer, report),",
                            variant_view.name
                        )
                    } else {
                        format!("Self::{}(_) => {{}}", variant_view.name)
                    }
                })
                .collect();
            if !arms.iter().all(|arm| arm.ends_with("(_) => {}")) {
                out.push(format!("match self {{ {} }}", arms.join(" ")));
            }
        }
        TypeKind::UnionOpen(u) => {
            if def.nullable {
                out.push(NULL_IS_VALID.to_owned());
            }
            let mut lines = Vec::new();
            let mut checked = false;
            let mut counts_certain = false;
            for (b, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let (body, reads) = branch(b, &progressing, ("branch", "&mut branch"));
                // Decoding proves a match only when nothing the branch says
                // goes unchecked.
                let sure = !checks::unsure(b) && (!generated(&b.ty) || progressing(&b.ty));
                let counter = if sure { "certain" } else { "uncertain" };
                counts_certain |= sure;
                if body.is_empty() {
                    checked |= !sure;
                    lines.push(format!(
                        "if self.{accessor}().is_some() {{ {counter} += 1; }}"
                    ));
                } else {
                    checked = true;
                    let binding = if reads { "v" } else { "_" };
                    lines.push(format!(
                        "if let Some({binding}) = self.{accessor}() {{ let mut branch = ValidationReport::default(); {} if branch.violations().next().is_none() {{ if branch.unchecked().next().is_none() {{ {counter} += 1; }} else {{ uncertain += 1; }} }} }}",
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
            for (b, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = format!(
                    "does not satisfy the allOf part read by {}",
                    accessor.trim_start_matches("as_")
                );
                let (mut body, reads) = branch(b, &progressing, ("report", "report"));
                if checks::part_unsure(b) || (generated(&b.ty) && !progressing(&b.ty)) {
                    body.push(format!(
                        "report.unevaluated(pointer, {:?});",
                        format!(
                            "allOf part read by {}: it says more than is checked",
                            accessor.trim_start_matches("as_")
                        )
                    ));
                }
                let part = if body.is_empty() {
                    format!(
                        "if self.{accessor}().is_none() {{ report.violation(pointer, {message:?}); }}"
                    )
                } else {
                    let binding = if reads { "v" } else { "_" };
                    format!(
                        "match self.{accessor}() {{ Some({binding}) => {{ {} }} None => report.violation(pointer, {message:?}), }}",
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
            render(
                &checks::presence_checks(inner, &def.constraints),
                ValueKind::Other,
                report,
                &mut out,
            );
            let list = checks::value_checks(inner, &def.constraints);
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
    Some(out)
}
