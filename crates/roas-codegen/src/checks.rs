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
    /// `pattern`, as a regex both targets read as ECMAScript does.
    Pattern {
        regex: String,
        source: String,
    },
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
    /// A number's `enum` or `const`: the values it may be. `literal` is
    /// how they are written in a message.
    NumberIn {
        values: Vec<Number>,
        keyword: &'static str,
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
    /// Keywords the generator does not handle at all: whatever the value,
    /// it is reported as unevaluated.
    Unsupported(Vec<String>),
    /// Checks on each item of an array or member value of a map: those
    /// that apply to any value, `null` included, then the rest.
    Each {
        item: TypeRef,
        presence: Vec<Check>,
        value: Vec<Check>,
    },
    /// `propertyNames`: string checks on each member name of a map, or of
    /// a struct's other members (its declared names are decided when
    /// generating: see [`name_verdict`]).
    Keys(Vec<Check>),
    /// What the items or members of a non-empty array or map declare that
    /// nothing checks on them.
    ItemsUnevaluated {
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
    let mut out = unenforced(constraints);
    // Raw JSON may be any kind of value: what applies to one kind is not
    // decided for it yet.
    if matches!(strip(ty), TypeRef::Any) {
        out.extend(
            declared_keywords(constraints)
                .into_iter()
                .map(|keyword| Check::Unevaluated {
                    keyword,
                    reason: "a raw JSON value is not checked against it yet".into(),
                }),
        );
    }
    out
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
            if let Some(source) = &c.pattern {
                out.push(match crate::pattern::translate(source) {
                    Ok(regex) => Check::Pattern {
                        regex,
                        source: source.clone(),
                    },
                    Err(reason) => Check::Unevaluated {
                        keyword: "pattern",
                        reason: format!("{source:?} is not checked: {reason}"),
                    },
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
            out.extend(number_in(c));
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
            out.extend(number_in(c));
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
            out.extend(each(ty, c));
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
            out.extend(names(c));
            out.extend(pattern_properties(c));
            out.extend(each(ty, c));
        }
        // A substituted type is the user's: what applies to the value it
        // stands for is not checked on it.
        ValueKind::Other if matches!(strip(ty), TypeRef::Extern(_)) => {
            out.extend(
                declared_keywords(c)
                    .into_iter()
                    .map(|keyword| Check::Unevaluated {
                        keyword,
                        reason: "a substituted type is not checked against it".into(),
                    }),
            );
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
    out.extend(names(constraints));
    // A struct checks its pattern members itself.
    out
}

/// What an inline item or member-value schema constrains, checked on each
/// item or member value of the array or map `ty`. A generated item type
/// checks itself.
pub(crate) fn each(ty: &TypeRef, c: &Constraints) -> Option<Check> {
    let items = c.items.as_ref()?;
    let item = match strip(ty) {
        TypeRef::Array(item) | TypeRef::Map(item) => item.as_ref(),
        _ => return None,
    };
    let presence = presence_checks(item, &items.constraints);
    let value = value_checks(item, &items.constraints);
    (!presence.is_empty() || !value.is_empty()).then(|| Check::Each {
        item: item.clone(),
        presence,
        value,
    })
}

/// `propertyNames`: checked on each member name when it says only what a
/// string check covers, otherwise unevaluated.
fn names(c: &Constraints) -> Option<Check> {
    if !c.property_names {
        return None;
    }
    match &c.names {
        Some(names) => {
            let list = value_checks(&TypeRef::String, names);
            (!list.is_empty()).then_some(Check::Keys(list))
        }
        None => Some(Check::Unevaluated {
            keyword: "propertyNames",
            reason: "a member-name schema saying more than minLength, maxLength and pattern is not checked yet".into(),
        }),
    }
}

/// What the name checks say of a member name known when generating.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Pass,
    Fail(String),
    Unknown(String),
}

/// The `propertyNames` checks applied to a declared property's `name`.
pub(crate) fn name_verdict(name: &str, list: &[Check]) -> Verdict {
    let mut unknown = None;
    for check in list {
        let length = name.chars().count() as u64;
        match check {
            Check::MinLength(n) if length < *n => {
                return Verdict::Fail(format!(
                    "the name is shorter than propertyNames minLength {n}"
                ));
            }
            Check::MaxLength(n) if length > *n => {
                return Verdict::Fail(format!(
                    "the name is longer than propertyNames maxLength {n}"
                ));
            }
            Check::MinLength(_) | Check::MaxLength(_) => {}
            Check::Pattern { regex, source } => match regex::Regex::new(regex) {
                Ok(re) if re.is_match(name) => {}
                Ok(_) => {
                    return Verdict::Fail(format!(
                        "the name does not match propertyNames pattern {source:?}"
                    ));
                }
                Err(_) => {
                    unknown.get_or_insert_with(|| {
                        format!("propertyNames pattern {source:?}: too large to compile here")
                    });
                }
            },
            Check::Unevaluated { keyword, reason } => {
                unknown.get_or_insert_with(|| format!("propertyNames {keyword}: {reason}"));
            }
            _ => {
                unknown.get_or_insert_with(|| "propertyNames: not checked on this name".to_owned());
            }
        }
    }
    unknown.map_or(Verdict::Pass, Verdict::Unknown)
}

fn pattern_properties(c: &Constraints) -> Option<Check> {
    (!c.pattern_properties.is_empty()).then(|| Check::Unevaluated {
        keyword: "patternProperties",
        reason: "member patterns are not checked yet".into(),
    })
}

/// What a nullable type's `null` is checked against: the checks that
/// apply whatever the value. A use site holds that `null` outside the type
/// (`Option`, a nil pointer), so the type is asked for them on its behalf.
pub(crate) fn null_checks(def: &crate::ir::TypeDef) -> Vec<Check> {
    // A null-only type holds its null outside it too, as a nil.
    let null_only = matches!(def.kind, TypeKind::Newtype(TypeRef::Null));
    if !def.nullable && !null_only {
        return Vec::new();
    }
    match &def.kind {
        // A raw holder validates its own null instead: see `null_raw`.
        TypeKind::Alias(_)
        | TypeKind::Extern
        | TypeKind::UnionOpen(_)
        | TypeKind::ComposedOpen(_) => Vec::new(),
        TypeKind::Newtype(inner) => presence_checks(inner, &def.constraints),
        _ => unenforced(&def.constraints),
    }
}

/// Whether a `null` held outside the raw holder `def` is validated by
/// building the holder from that `null`: its branches have their say.
pub(crate) fn null_raw(def: &crate::ir::TypeDef) -> bool {
    def.nullable && matches!(def.kind, TypeKind::UnionOpen(_) | TypeKind::ComposedOpen(_))
}

/// What a substituted component declares, at a position holding it: the
/// user's type is not checked against it.
///
/// `wrappers` counts a newtype over a substitution too, for a backend that
/// spells it as an alias with no validator of its own. Inside an array or
/// map the items are not checked either, which a non-empty one reports.
pub(crate) fn extern_checks(ir: &crate::ir::Ir, ty: &TypeRef, wrappers: bool) -> Externs {
    if let TypeRef::Array(inner) | TypeRef::Map(inner) = strip(ty) {
        let inner = extern_checks(ir, inner, wrappers);
        let value = inner
            .presence
            .into_iter()
            .chain(inner.value)
            .map(|check| match check {
                Check::Unevaluated { keyword, reason }
                | Check::ItemsUnevaluated { keyword, reason } => {
                    Check::ItemsUnevaluated { keyword, reason }
                }
                Check::Unsupported(keywords) => Check::ItemsUnevaluated {
                    keyword: "unhandled keywords",
                    reason: format!("{} are not handled by the generator", keywords.join(", ")),
                },
                other => other,
            })
            .collect();
        return Externs {
            presence: Vec::new(),
            value,
        };
    }
    let declared = |c: &Constraints| -> Externs {
        Externs {
            presence: unenforced(c),
            value: declared_keywords(c)
                .into_iter()
                .map(|keyword| Check::Unevaluated {
                    keyword,
                    reason: "a substituted type is not checked against it".into(),
                })
                .collect(),
        }
    };
    let Some(mut id) = ty.direct_named() else {
        return Externs::default();
    };
    for _ in 0..=ir.types.len() {
        let Some(def) = ir.types.get(id) else {
            return Externs::default();
        };
        match &def.kind {
            TypeKind::Alias(inner) => match inner.direct_named() {
                Some(next) => id = next,
                None => return Externs::default(),
            },
            TypeKind::Newtype(TypeRef::Extern(_)) if wrappers => return declared(&def.constraints),
            TypeKind::Extern => return declared(&def.constraints),
            _ => return Externs::default(),
        }
    }
    Externs::default()
}

/// What a substituted type declares, split as everything else is: what
/// applies to any value present, `null` included, and what applies to a
/// non-null value.
#[derive(Default)]
pub(crate) struct Externs {
    pub presence: Vec<Check>,
    pub value: Vec<Check>,
}

/// Whether `def` has a null hook: a `validate_null` of its own.
pub(crate) fn has_null_hook(def: &crate::ir::TypeDef) -> bool {
    null_raw(def) || !null_checks(def).is_empty()
}

/// The unhandled keywords every branch of a union shares: a type list
/// gives them to each member, and they constrain its `null` too.
pub(crate) fn shared_unsupported(ir: &crate::ir::Ir, branches: &[crate::ir::Branch]) -> Vec<Check> {
    // A hoisted branch keeps its keywords on its own definition.
    let of = |b: &crate::ir::Branch| -> Vec<String> {
        match b.ty.direct_named().and_then(|id| ir.types.get(id)) {
            Some(def) => def.constraints.unsupported.clone(),
            None => b.constraints.unsupported.clone(),
        }
    };
    let Some((first, rest)) = branches.split_first() else {
        return Vec::new();
    };
    let rest: Vec<Vec<String>> = rest.iter().map(of).collect();
    let shared: Vec<String> = of(first)
        .into_iter()
        .filter(|k| rest.iter().all(|r| r.contains(k)))
        .collect();
    if shared.is_empty() {
        Vec::new()
    } else {
        vec![Check::Unsupported(shared)]
    }
}

/// The float32s nearest `bound` from below and above: the same one when
/// `bound` is a float32.
pub(crate) fn f32_neighbours(bound: f64) -> (f32, f32) {
    let rounded = bound as f32;
    // The next float32 above: from either zero, the smallest subnormal.
    fn next_up(f: f32) -> f32 {
        if f == 0.0 {
            return f32::from_bits(1);
        }
        let bits = f.to_bits();
        f32::from_bits(if f > 0.0 { bits + 1 } else { bits - 1 })
    }
    let step = |f: f32, up: bool| {
        let next = if up { next_up(f) } else { -next_up(-f) };
        if next.is_finite() { next } else { f }
    };
    match f64::from(rounded).partial_cmp(&bound) {
        Some(std::cmp::Ordering::Less) => (rounded, step(rounded, true)),
        Some(std::cmp::Ordering::Greater) => (step(rounded, false), rounded),
        _ => (rounded, rounded),
    }
}

#[cfg(test)]
mod neighbours {
    use super::f32_neighbours;

    #[test]
    fn steps_across_zero_both_ways() {
        // A tiny negative rounds to -0.0: the float below it is the
        // negative smallest subnormal.
        let (below, above) = f32_neighbours(-7.006492321624085e-46);
        assert_eq!(below, -f32::from_bits(1));
        assert_eq!(above, 0.0);
        let (below, above) = f32_neighbours(7.006492321624085e-46);
        assert_eq!(below, 0.0);
        assert_eq!(above, f32::from_bits(1));
        let (below, above) = f32_neighbours(0.1);
        assert!(f64::from(below) < 0.1 && f64::from(above) > 0.1);
    }
}

/// The ranges the types of a holder's branches can hold, found through
/// fields, items, values and references: the whole-number magnitude from
/// which the narrowest integer among them overflows (`None` with no
/// integer), and whether a 32-bit float is among them. A number past
/// either fails a branch's decoding for the representation, not because
/// the value is not one the branch's schema allows. A substituted type is
/// assumed to be as narrow as any.
pub(crate) fn number_range(ir: &crate::ir::Ir, tys: &[&TypeRef]) -> (Option<&'static str>, bool) {
    fn walk(
        ir: &crate::ir::Ir,
        ty: &TypeRef,
        seen: &mut std::collections::BTreeSet<crate::diagnostic::SchemaId>,
        int32: &mut bool,
        int64: &mut bool,
        float32: &mut bool,
    ) {
        match ty {
            TypeRef::Integer(IntWidth::Int32) => *int32 = true,
            TypeRef::Integer(_) => *int64 = true,
            TypeRef::Number(FloatWidth::Float) => *float32 = true,
            TypeRef::Extern(_) => {
                *int32 = true;
                *float32 = true;
            }
            TypeRef::Boxed(inner)
            | TypeRef::Nullable(inner)
            | TypeRef::Array(inner)
            | TypeRef::Map(inner) => walk(ir, inner, seen, int32, int64, float32),
            TypeRef::Named(id) => {
                if !seen.insert(id.clone()) {
                    return;
                }
                let Some(def) = ir.types.get(id) else { return };
                let mut inner: Vec<&TypeRef> = Vec::new();
                match &def.kind {
                    TypeKind::Struct(s) => {
                        inner.extend(s.fields.iter().map(|f| &f.ty));
                        if let crate::ir::Additional::CatchAll(ty) = &s.additional {
                            inner.push(ty);
                        }
                    }
                    TypeKind::Newtype(ty) | TypeKind::Alias(ty) => inner.push(ty),
                    TypeKind::UnionTagged(u) => inner.extend(u.variants.iter().map(|v| &v.ty)),
                    TypeKind::UnionOpen(crate::ir::OpenUnion { branches, .. })
                    | TypeKind::ComposedOpen(branches) => {
                        inner.extend(branches.iter().map(|b| &b.ty))
                    }
                    TypeKind::Extern => {
                        *int32 = true;
                        *float32 = true;
                    }
                    TypeKind::StringEnum(_) => {}
                }
                for ty in inner {
                    walk(ir, ty, seen, int32, int64, float32);
                }
            }
            _ => {}
        }
    }
    let (mut int32, mut int64, mut float32) = (false, false, false);
    let mut seen = std::collections::BTreeSet::new();
    for ty in tys {
        walk(ir, ty, &mut seen, &mut int32, &mut int64, &mut float32);
    }
    let limit = if int32 {
        Some("2147483648.0")
    } else if int64 {
        Some("9223372036854775808.0")
    } else {
        None
    };
    (limit, float32)
}

/// Whether `ty` reaches a substituted type, whose decoding is the user's:
/// a failure says nothing about the schema.
pub(crate) fn reaches_extern(ir: &crate::ir::Ir, ty: &TypeRef) -> bool {
    reaches_decoder_rules(ir, ty, &|_| false)
}

/// Whether decoding `ty` may fail for the decoder rather than the value:
/// it reaches a substitution, or a struct `reshaped` says reads its
/// members by other names than the schema does (a field compiled out,
/// whose member then lands among the others; an alias, read as the field).
pub(crate) fn reaches_decoder_rules(
    ir: &crate::ir::Ir,
    ty: &TypeRef,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> bool {
    reaches(ir, ty, reshaped, true)
}

/// Whether decoding `ty` reaches a struct `reshaped` says reads its members
/// by other names than the schema, whether or not it reaches a
/// substitution too.
pub(crate) fn reaches_reshaped(
    ir: &crate::ir::Ir,
    ty: &TypeRef,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
) -> bool {
    reaches(ir, ty, reshaped, false)
}

fn reaches(
    ir: &crate::ir::Ir,
    ty: &TypeRef,
    reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
    externs: bool,
) -> bool {
    fn walk(
        ir: &crate::ir::Ir,
        ty: &TypeRef,
        reshaped: &dyn Fn(&crate::diagnostic::SchemaId) -> bool,
        externs: bool,
        seen: &mut std::collections::BTreeSet<crate::diagnostic::SchemaId>,
    ) -> bool {
        match ty {
            TypeRef::Extern(_) => externs,
            TypeRef::Boxed(inner)
            | TypeRef::Nullable(inner)
            | TypeRef::Array(inner)
            | TypeRef::Map(inner) => walk(ir, inner, reshaped, externs, seen),
            TypeRef::Named(id) => {
                if !seen.insert(id.clone()) {
                    return false;
                }
                match ir.types.get(id).map(|d| &d.kind) {
                    Some(TypeKind::Extern) => externs,
                    Some(TypeKind::Struct(s)) => {
                        reshaped(id)
                            || s.fields
                                .iter()
                                .any(|f| walk(ir, &f.ty, reshaped, externs, seen))
                            || matches!(&s.additional, crate::ir::Additional::CatchAll(t) if walk(ir, t, reshaped, externs, seen))
                    }
                    Some(TypeKind::Newtype(t) | TypeKind::Alias(t)) => {
                        walk(ir, t, reshaped, externs, seen)
                    }
                    Some(TypeKind::UnionTagged(u)) => u
                        .variants
                        .iter()
                        .any(|v| walk(ir, &v.ty, reshaped, externs, seen)),
                    // A raw holder decodes anything: its branches are
                    // judged inside it.
                    _ => false,
                }
            }
            _ => false,
        }
    }
    walk(ir, ty, reshaped, externs, &mut Default::default())
}

/// Whether a value of `ty`, decoded, holds anything `leaf` picks out —
/// through containers, struct fields and catch-alls, newtypes, aliases and
/// tagged variants. An open holder keeps its raw text, so it is not
/// walked into.
pub(crate) fn reaches_leaf(
    ir: &crate::ir::Ir,
    ty: &TypeRef,
    leaf: &dyn Fn(&TypeRef) -> bool,
) -> bool {
    fn walk(
        ir: &crate::ir::Ir,
        ty: &TypeRef,
        leaf: &dyn Fn(&TypeRef) -> bool,
        seen: &mut std::collections::BTreeSet<crate::diagnostic::SchemaId>,
    ) -> bool {
        if leaf(ty) {
            return true;
        }
        match ty {
            TypeRef::Boxed(inner)
            | TypeRef::Nullable(inner)
            | TypeRef::Array(inner)
            | TypeRef::Map(inner) => walk(ir, inner, leaf, seen),
            TypeRef::Named(id) => {
                if !seen.insert(id.clone()) {
                    return false;
                }
                match ir.types.get(id).map(|d| &d.kind) {
                    // Beside `patternProperties`, the other members are held
                    // as their text in both languages.
                    Some(TypeKind::Struct(s)) => {
                        s.fields.iter().any(|f| walk(ir, &f.ty, leaf, seen))
                            || (s.patterns.is_empty()
                                && matches!(&s.additional, crate::ir::Additional::CatchAll(t) if walk(ir, t, leaf, seen)))
                    }
                    Some(TypeKind::Newtype(t) | TypeKind::Alias(t)) => walk(ir, t, leaf, seen),
                    Some(TypeKind::UnionTagged(u)) => {
                        u.variants.iter().any(|v| walk(ir, &v.ty, leaf, seen))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
    walk(ir, ty, leaf, &mut Default::default())
}

/// Whether decoding a branch proves nothing: it declares what neither its
/// type nor a generated check enforces.
pub(crate) fn unsure(branch: &crate::ir::Branch) -> bool {
    branch.unchecked || !unchecked_paths(&branch.ty, &branch.constraints).is_empty()
}

/// What `unchecked_keywords` finds at a position and, through inline items
/// and member values, inside it: each with the pointer suffix of the
/// schema that declares it.
pub(crate) fn unchecked_paths(ty: &TypeRef, c: &Constraints) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = unchecked_keywords(ty, c)
        .into_iter()
        .map(|keyword| (String::new(), keyword))
        .collect();
    if let (Some(items), TypeRef::Array(item) | TypeRef::Map(item)) = (&c.items, strip(ty)) {
        out.extend(
            unchecked_paths(item, &items.constraints)
                .into_iter()
                .map(|(path, keyword)| (format!("/{}{path}", items.keyword), keyword)),
        );
    }
    out
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
    let unsupported =
        (!c.unsupported.is_empty()).then(|| Check::Unsupported(c.unsupported.clone()));
    c.unenforced
        .iter()
        .map(|&keyword| Check::Unevaluated {
            keyword,
            reason: match keyword {
                "enum" => "this enum is not checked yet".into(),
                "required" => {
                    "a required member with no property of its own is not checked yet".into()
                }
                _ => format!("`{keyword}` is not checked yet"),
            },
        })
        .chain(unsupported)
        .collect()
}

/// Whether the constraints say anything a check above does not cover for
/// a value of this kind: they are then reported at generation time.
pub(crate) fn unchecked_keywords(ty: &TypeRef, c: &Constraints) -> Vec<&'static str> {
    // A named type checks the constraints it was hoisted with itself, and
    // raw JSON reports them at run time.
    if ty.direct_named().is_some() || matches!(strip(ty), TypeRef::Any | TypeRef::Extern(_)) {
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
        (!c.pattern_properties.is_empty(), "patternProperties", map),
        (c.enum_numbers.is_some(), "enum", number),
        (c.const_number.is_some(), "const", number),
    ] {
        if present && !applies {
            out.push(keyword);
        }
    }
    out
}

/// With validation on, a warning for every constraint no check covers, so
/// nothing a schema says is dropped without a word.
///
/// `opted_out` says which types check nothing: an untranslatable pattern
/// is reported as unevaluated only by a type that checks.
pub(crate) fn diagnostics(
    ir: &Ir,
    opted_out: &dyn Fn(&crate::ir::TypeDef) -> bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mut warn = |id: &crate::diagnostic::SchemaId, pointer: String, keyword: &str, message| {
        out.push(Diagnostic {
            severity: Severity::Warning,
            kind: DiagnosticKind::Unvalidated {
                keyword: keyword.to_owned(),
            },
            schema_id: id.clone(),
            pointer: source_of(&pointer),
            message,
        });
    };
    for id in &ir.order {
        if ir.suppressed.contains(id) {
            continue;
        }
        let def = &ir.types[id];
        // Each value position: where it is, its type and its constraints.
        let mut positions: Vec<(String, &TypeRef, &Constraints)> = Vec::new();
        match &def.kind {
            TypeKind::Struct(s) => {
                for field in &s.fields {
                    positions.push((field.pointer.clone(), &field.ty, &field.constraints));
                }
                // Each pattern's schema judges the members it matches,
                // and `additionalProperties` beside them the rest.
                for pattern in &s.patterns {
                    let mut pointer = def.id.pointer.clone();
                    push_token(&mut pointer, "patternProperties");
                    push_token(&mut pointer, &pattern.source);
                    // A pattern that lets anything be, beside members that may
                    // be anything too, decides nothing: nothing is reported.
                    let decides = !(pattern.accepts_anything(ir) && s.others_accept_anything(ir));
                    if !opted_out(def)
                        && decides
                        && let Err(reason) = &pattern.regex
                    {
                        warn(
                            &def.id,
                            pointer.clone(),
                            "patternProperties",
                            format!(
                                "pattern {:?} is reported as unevaluated for every member: {reason}",
                                pattern.source
                            ),
                        );
                    }
                    if let Some(b) = pattern.branch() {
                        let at = b.pointer.clone().unwrap_or(pointer);
                        positions.push((at, &b.ty, &b.constraints));
                    }
                }
                if let crate::ir::Others::Schema(b) = &s.others {
                    let at = b
                        .pointer
                        .clone()
                        .unwrap_or_else(|| format!("{}/additionalProperties", def.id.pointer));
                    positions.push((at, &b.ty, &b.constraints));
                }
                // The catch-all's values are the other members.
                if let (crate::ir::Additional::CatchAll(ty), Some(items)) =
                    (&s.additional, &def.constraints.items)
                {
                    positions.push((
                        format!("{}/{}", def.id.pointer, items.keyword),
                        ty,
                        &items.constraints,
                    ));
                }
            }
            TypeKind::Newtype(inner) => {
                positions.push((def.id.pointer.clone(), inner, &def.constraints));
            }
            // An enum of no members has no value to check.
            TypeKind::StringEnum(values) if !values.is_empty() => {
                positions.push((def.id.pointer.clone(), &TypeRef::String, &def.constraints));
            }
            TypeKind::UnionOpen(crate::ir::OpenUnion { branches, .. })
            | TypeKind::ComposedOpen(branches) => {
                for branch in branches {
                    let pointer = branch
                        .pointer
                        .clone()
                        .unwrap_or_else(|| def.id.pointer.clone());
                    positions.push((pointer, &branch.ty, &branch.constraints));
                }
            }
            _ => {}
        }
        // A struct's checked `propertyNames`: its pattern is a string
        // position. (Other positions' are found below.)
        if matches!(def.kind, TypeKind::Struct(_))
            && def.constraints.property_names
            && let Some(names) = &def.constraints.names
        {
            positions.push((
                format!("{}/propertyNames", def.id.pointer),
                &TypeRef::String,
                names,
            ));
        }
        // Inline items and member values, checked on each, and a map's
        // checked `propertyNames`.
        let mut i = 0;
        while i < positions.len() {
            let (pointer, ty, c) = positions[i].clone();
            if c.property_names
                && kind_of(ty) == ValueKind::Map
                && let Some(names) = &c.names
            {
                positions.push((format!("{pointer}/propertyNames"), &TypeRef::String, names));
            }
            if let (Some(items), TypeRef::Array(item) | TypeRef::Map(item)) = (&c.items, strip(ty))
            {
                positions.push((
                    format!("{pointer}/{}", items.keyword),
                    item,
                    &items.constraints,
                ));
            }
            i += 1;
        }
        for (pointer, ty, c) in positions {
            for keyword in unchecked_keywords(ty, c) {
                warn(
                    &def.id,
                    format!("{pointer}/{keyword}"),
                    keyword,
                    format!(
                        "`{keyword}` does not apply to a value of this type, so validation does not check it"
                    ),
                );
            }
            if !opted_out(def)
                && ty.direct_named().is_none()
                && kind_of(ty) == ValueKind::String
                && let Some(source) = &c.pattern
                && let Err(reason) = crate::pattern::translate(source)
            {
                warn(
                    &def.id,
                    format!("{pointer}/pattern"),
                    "pattern",
                    format!("pattern {source:?} is reported as unevaluated: {reason}"),
                );
            }
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
    let mut out = declared_keywords(c);
    out.extend(c.unenforced.iter().copied());
    out
}

/// The keyword constraints the schema declares, whatever value they apply to.
fn declared_keywords(c: &Constraints) -> Vec<&'static str> {
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
        (!c.pattern_properties.is_empty(), "patternProperties"),
        (c.enum_numbers.is_some(), "enum"),
        (c.const_number.is_some(), "const"),
    ]
    .into_iter()
    .filter_map(|(present, keyword)| present.then_some(keyword))
    .chain(c.items.as_ref().map(|items| items.keyword))
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
        // An exponent past `i64`, however many digits it has, saturates
        // by its sign: the bound is past every value either way, never
        // zero.
        Some((m, e)) => {
            let e = e.strip_prefix('+').unwrap_or(e);
            let (negative, magnitude) = match e.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, e),
            };
            if magnitude.is_empty() || !magnitude.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let exponent = match magnitude.parse::<i64>() {
                Ok(n) if negative => -n,
                Ok(n) => n,
                Err(_) if negative => i64::MIN,
                Err(_) => i64::MAX,
            };
            (m.to_owned(), exponent)
        }
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
/// A number's `enum` and `const`, as checks.
fn number_in(c: &Constraints) -> Vec<Check> {
    let mut out = Vec::new();
    if let Some(values) = &c.enum_numbers {
        out.push(Check::NumberIn {
            values: values.clone(),
            keyword: "enum",
            literal: format!(
                "[{}]",
                values
                    .iter()
                    .map(Number::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if let Some(n) = &c.const_number {
        out.push(Check::NumberIn {
            values: vec![n.clone()],
            keyword: "const",
            literal: n.to_string(),
        });
    }
    out
}

/// The integer `n` is, when it is one.
pub(crate) fn exact_integer(n: &Number) -> Option<i128> {
    let (floor, ceil) = floor_ceil(n);
    (floor == ceil).then_some(floor)
}

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
        let scale = u32::try_from(d.exponent.checked_neg()?).ok()?;
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
        // A scale past every exponent is not an exact divisor.
        assert_eq!(decimal(&n("1e-9223372036854775808")), None);
        // Past i128 too: saturated by sign, never zero.
        let huge = "1e170141183460469231731687303715884105728";
        assert_eq!(int_lower(&n(huge), false), SATURATE);
        assert_eq!(int_upper(&n(&format!("-{huge}")), false), -SATURATE);
        assert_eq!(
            int_lower(&n("1e-170141183460469231731687303715884105728"), false),
            1
        );
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
