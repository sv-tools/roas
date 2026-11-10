//! Spelling [`Check`]s as Rust: the body of each generated type's
//! `Validate::validate_into`.

use crate::checks::{self, Check, Unique, ValueKind};
use crate::front::IntWidth;
use crate::ir::{Additional, TypeDef, TypeKind, TypeRef};
use crate::rust::view::TypeView;

/// Statements checking `checks` on `v`, a `&T` of the value's type, with
/// findings at the pointer expression `p` (a `&str`). True when they read
/// the value: some checks only report that they could not be made.
fn render(list: &[Check], kind: ValueKind, v: &str, p: &str, out: &mut Vec<String>) -> bool {
    let int = |v: &str| format!("i128::from({v}.0)");
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
    for check in list {
        match check {
            Check::MinLength(n) => out.push(format!(
                "{{ let n = {v}.chars().count(); if n < {n} {{ report.violation({p}, format!(\"is {{n}} characters long, shorter than minLength {n}\")); }} }}"
            )),
            Check::MaxLength(n) => out.push(format!(
                "{{ let n = {v}.chars().count(); if n > {n} {{ report.violation({p}, format!(\"is {{n}} characters long, longer than maxLength {n}\")); }} }}"
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
                        "if {} < {bound} {{ report.violation({p}, format!(\"{{}} is below {keyword} {literal}\", {v}.0)); }}",
                        int(v)
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
                        "if {} > {bound} {{ report.violation({p}, format!(\"{{}} is above {keyword} {literal}\", {v}.0)); }}",
                        int(v)
                    ));
                }
            }
            Check::IntMultipleOf {
                factor,
                scale,
                literal,
            } => out.push(format!(
                "match {}.checked_mul(10i128.pow({scale})) {{ Some(n) if n % {factor} != 0 => report.violation({p}, format!(\"{{}} is not a multiple of {literal}\", {v}.0)), Some(_) => {{}} None => report.unevaluated({p}, \"multipleOf {literal}: the value is too large to check exactly\"), }}",
                int(v)
            )),
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
                    "if !(f64::from(*{v}) {op} {bound:?}) {{ report.violation({p}, format!(\"{{}} is below {keyword} {literal}\", {v})); }}"
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
                    "if !(f64::from(*{v}) {op} {bound:?}) {{ report.violation({p}, format!(\"{{}} is above {keyword} {literal}\", {v})); }}"
                ));
            }
            Check::NumMultipleOf { integral, literal } => {
                let unevaluated = format!(
                    "report.unevaluated({p}, \"multipleOf {literal}: a decoded float has lost its literal, so only a fraction against a whole multiple is decided\")"
                );
                if *integral {
                    // A fraction never came from a whole-number literal, so
                    // it is no multiple of a whole number; anything else
                    // may have been rounded on the way in.
                    out.push(format!(
                        "if f64::from(*{v}).fract() != 0.0 {{ report.violation({p}, format!(\"{{}} is not a multiple of {literal}\", {v})); }} else {{ {unevaluated}; }}"
                    ));
                } else {
                    out.push(format!("{unevaluated};"));
                }
            }
            Check::MinItems(n) => out.push(format!(
                "if {v}.len() < {n} {{ report.violation({p}, format!(\"has {{}} items, fewer than minItems {n}\", {v}.len())); }}"
            )),
            Check::MaxItems(n) => out.push(format!(
                "if {v}.len() > {n} {{ report.violation({p}, format!(\"has {{}} items, more than maxItems {n}\", {v}.len())); }}"
            )),
            Check::UniqueItems(Unique::Exact) => out.push(format!(
                "{{ let mut seen = std::collections::BTreeSet::new(); for (i, item) in {v}.iter().enumerate() {{ if !seen.insert(item) {{ report.violation(&child_pointer({p}, &i.to_string()), \"repeats an earlier item; uniqueItems is true\"); }} }} }}"
            )),
            Check::UniqueItems(Unique::Float) => out.push(format!(
                "if {v}.iter().enumerate().any(|(i, a)| {v}[..i].contains(a)) {{ report.unevaluated({p}, \"uniqueItems: equal floats may come from different numbers\"); }}"
            )),
            Check::UniqueItems(Unique::Unknown) => out.push(format!(
                "if {v}.len() > 1 {{ report.unevaluated({p}, \"uniqueItems: items of this type are not compared yet\"); }}"
            )),
            Check::MinProperties(n) => out.push(format!(
                "if {v}.len() < {n} {{ report.violation({p}, format!(\"has {{}} members, fewer than minProperties {n}\", {v}.len())); }}"
            )),
            Check::MaxProperties(n) => out.push(format!(
                "if {v}.len() > {n} {{ report.violation({p}, format!(\"has {{}} members, more than maxProperties {n}\", {v}.len())); }}"
            )),
            Check::Unevaluated { keyword, reason } => out.push(format!(
                "report.unevaluated({p}, {:?});",
                format!("{keyword}: {reason}")
            )),
        }
    }
    reads
}

/// Checks on a value position: nullability seen through, then the
/// constraints.
fn value(ty: &TypeRef, list: &[Check], v: &str, p: &str, out: &mut Vec<String>) -> bool {
    if list.is_empty() {
        return false;
    }
    let kind = checks::kind_of(ty);
    match ty {
        TypeRef::Nullable(inner) => {
            let mut body = Vec::new();
            if value(inner, list, "v", p, &mut body) {
                out.push(format!("if let Some(v) = {v} {{ {} }}", body.join(" ")));
            } else {
                out.push(format!("if {v}.is_some() {{ {} }}", body.join(" ")));
            }
            true
        }
        TypeRef::Boxed(inner) => value(inner, list, v, p, out),
        _ => render(list, kind, v, p, out),
    }
}

/// The body of `validate_into` for one type, or `None` when it gets no
/// implementation (a type alias shares its target's).
pub(crate) fn body(
    def: &TypeDef,
    view: &TypeView,
    generated: &dyn Fn(&TypeRef) -> bool,
    opted_out: bool,
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
            let mut present = Vec::new();
            for (field, field_view) in s.fields.iter().zip(&view.fields) {
                let name = &field_view.name;
                let list = checks::checks(&field.ty, &field.constraints);
                let recurse = generated(&field.ty);
                present.push(match (field.required, field.nullable) {
                    (true, _) => "1".to_owned(),
                    _ => format!("usize::from(self.{name}.is_some())"),
                });
                if list.is_empty() && !recurse {
                    continue;
                }
                let mut block = vec![format!(
                    "let p = child_pointer(pointer, {:?});",
                    field.raw_name
                )];
                if !list.is_empty() {
                    let mut inner = Vec::new();
                    let reads = value(&field.ty, &list, "v", "&p", &mut inner);
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
            if !list.is_empty() {
                out.push(format!(
                    "let members: usize = {};",
                    if present.is_empty() {
                        "0".to_owned()
                    } else {
                        present.join(" + ")
                    }
                ));
                for check in &list {
                    match check {
                        Check::MinProperties(n) => out.push(format!(
                            "if members < {n} {{ report.violation(pointer, format!(\"has {{members}} members, fewer than minProperties {n}\")); }}"
                        )),
                        Check::MaxProperties(n) => out.push(format!(
                            "if members > {n} {{ report.violation(pointer, format!(\"has {{members}} members, more than maxProperties {n}\")); }}"
                        )),
                        other => {
                            render(
                                std::slice::from_ref(other),
                                ValueKind::Other,
                                "self",
                                "pointer",
                                &mut out,
                            );
                        }
                    }
                }
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
            out.push(format!("match self {{ {} }}", arms.join(" ")));
        }
        TypeKind::UnionOpen(u) => {
            let validated = u.branches.iter().any(|b| generated(&b.ty));
            out.push("let mut certain = 0usize;".to_owned());
            out.push(if validated {
                "let mut uncertain = 0usize;".to_owned()
            } else {
                "let uncertain = 0usize;".to_owned()
            });
            for (branch, branch_view) in u.branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                if generated(&branch.ty) {
                    out.push(format!(
                        "if let Some(v) = self.{accessor}() {{ let mut branch = ValidationReport::default(); v.validate_into(pointer, &mut branch); if branch.violations().next().is_none() {{ if branch.unchecked().next().is_none() {{ certain += 1; }} else {{ uncertain += 1; }} }} }}"
                    ));
                } else {
                    out.push(format!(
                        "if self.{accessor}().is_some() {{ certain += 1; }}"
                    ));
                }
            }
            if u.exclusive {
                out.push("if certain + uncertain == 0 { report.violation(pointer, \"matches none of the oneOf branches\"); } else if certain > 1 { report.violation(pointer, format!(\"matches {certain} oneOf branches; exactly one must\")); } else if !(certain == 1 && uncertain == 0) { report.unevaluated(pointer, \"oneOf: whether exactly one branch matches depends on constraints that were not evaluated\"); }".to_owned());
            } else {
                out.push("if certain == 0 { if uncertain == 0 { report.violation(pointer, \"matches none of the anyOf branches\"); } else { report.unevaluated(pointer, \"anyOf: whether a branch matches depends on constraints that were not evaluated\"); } }".to_owned());
            }
        }
        TypeKind::ComposedOpen(branches) => {
            for (branch, branch_view) in branches.iter().zip(&view.branches) {
                let accessor = &branch_view.accessor;
                let message = format!(
                    "does not satisfy the allOf part read by {}",
                    accessor.trim_start_matches("as_")
                );
                if generated(&branch.ty) {
                    out.push(format!(
                        "match self.{accessor}() {{ Some(v) => v.validate_into(pointer, report), None => report.violation(pointer, {message:?}), }}"
                    ));
                } else {
                    out.push(format!(
                        "if self.{accessor}().is_none() {{ report.violation(pointer, {message:?}); }}"
                    ));
                }
            }
        }
        TypeKind::Newtype(inner) | TypeKind::Alias(inner) => {
            let list = checks::checks(inner, &def.constraints);
            let _ = value(inner, &list, "(&self.0)", "pointer", &mut out);
            if generated(inner) {
                out.push("self.0.validate_into(pointer, report);".to_owned());
            }
        }
        TypeKind::StringEnum(_) | TypeKind::Extern => {}
    }
    Some(out)
}
