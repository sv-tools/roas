//! Judging one JSON value against one Schema Object.
//!
//! `roas` validates that a Schema Object is *well formed*; this judges a
//! *value* against one, which is the other half and the half a request
//! validator needs. It walks `roas`'s typed schema tree directly rather
//! than serializing back to JSON and handing it to a general JSON Schema
//! library: the tree already knows which type it is, so the keywords
//! that can apply are the ones the variant carries, and a failure can
//! say what was expected in the description's own words.
//!
//! Every failure is collected with a JSON Pointer to the value it is
//! about, so a body with three bad fields reports three failures rather
//! than the first.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt::Display;

use roas::common::bool_or::BoolOr;
use roas::common::formats::SchemaType;
use roas::common::reference::RefOr;
use roas::v3_2::schema::{
    ArraySchema, IntegerSchema, NumberSchema, ObjectSchema, Schema, SingleSchema, StringSchema,
};
use roas::v3_2::spec::Spec;
use serde_json::Value;

/// One way in which a value did not satisfy its schema — or could not
/// be judged against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    /// RFC 6901 JSON Pointer to the offending value; empty for the root.
    pub(crate) pointer: String,
    /// What the schema wanted.
    pub(crate) message: String,
    /// Whether the value broke the schema, or the schema could not be
    /// applied to it.
    pub(crate) kind: FailureKind,
}

/// Why a value is being reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// The value broke the schema.
    Violated,
    /// A `$ref` named nothing, so there was no schema to apply.
    Unresolved,
    /// The schema could be read but not applied faithfully, so the
    /// value went **unchecked** — never "valid".
    Unchecked,
}

/// Judge `value` against `schema`, collecting every failure.
pub(crate) fn check(value: &Value, schema: &RefOr<Schema>, spec: &Spec) -> Vec<Failure> {
    let mut checker = Checker::new(spec);
    checker.schema(value, schema);
    checker.failures
}

/// How deep a schema may nest before the walk gives up.
///
/// A value's own depth is bounded — `serde_json` refuses to parse past
/// 128 levels — but a schema's is not: `A: { allOf: [ { $ref: A } ] }`
/// recurses without ever descending into the value. That description is
/// malformed, and `roas`'s validator says so, but a request validator
/// must not hang on one.
const MAX_DEPTH: u32 = 64;

struct Checker<'s> {
    spec: &'s Spec,
    pointer: String,
    failures: Vec<Failure>,
    depth: u32,
}

impl<'s> Checker<'s> {
    fn new(spec: &'s Spec) -> Self {
        Self {
            spec,
            pointer: String::new(),
            failures: Vec::new(),
            depth: 0,
        }
    }

    /// Record a failure at the current pointer.
    fn fail(&mut self, message: impl Into<String>) {
        self.record(FailureKind::Violated, message);
    }

    /// Record that the value could not be judged, rather than that it
    /// failed. "Not checked" must never read as "valid".
    fn unchecked(&mut self, message: impl Into<String>) {
        self.record(FailureKind::Unchecked, message);
    }

    fn record(&mut self, kind: FailureKind, message: impl Into<String>) {
        self.failures.push(Failure {
            pointer: self.pointer.clone(),
            message: message.into(),
            kind,
        });
    }

    /// Descend into `segment`, run `body`, and come back out. The
    /// pointer is one buffer for the whole walk rather than a string
    /// rebuilt per value.
    fn nested(&mut self, segment: &str, body: impl FnOnce(&mut Self)) {
        let restore = self.pointer.len();
        self.pointer.push('/');
        // RFC 6901: `~` and `/` are the only characters that escape.
        for character in segment.chars() {
            match character {
                '~' => self.pointer.push_str("~0"),
                '/' => self.pointer.push_str("~1"),
                _ => self.pointer.push(character),
            }
        }
        body(self);
        self.pointer.truncate(restore);
    }

    /// Whether `value` satisfies `schema`, without recording anything —
    /// what `anyOf`, `oneOf` and `not` need.
    fn passes(&self, value: &Value, schema: &RefOr<Schema>) -> bool {
        let mut probe = Checker::new(self.spec);
        // The probe continues this walk, so it inherits its depth — a
        // cycle that runs through `anyOf` must still terminate.
        probe.depth = self.depth;
        probe.schema(value, schema);
        probe.failures.is_empty()
    }

    fn schema(&mut self, value: &Value, schema: &RefOr<Schema>) {
        if self.depth >= MAX_DEPTH {
            self.unchecked(format!(
                "the schema nests more than {MAX_DEPTH} levels deep"
            ));
            return;
        }
        self.depth += 1;
        self.dispatch(value, schema);
        self.depth -= 1;
    }

    fn dispatch(&mut self, value: &Value, schema: &RefOr<Schema>) {
        match schema.get_item(self.spec) {
            Ok(resolved) => self.resolved(value, resolved),
            Err(error) => self.record(FailureKind::Unresolved, error.to_string()),
        }
    }

    fn resolved(&mut self, value: &Value, schema: &Schema) {
        match schema {
            // `true` and `{}` accept anything; `false` accepts nothing.
            Schema::Bool(true) | Schema::Empty(_) => {}
            Schema::Bool(false) => self.fail("no value is allowed here"),

            Schema::AllOf(all_of) => {
                for subschema in &all_of.all_of {
                    self.schema(value, subschema);
                }
            }
            Schema::AnyOf(any_of) => {
                if !any_of.any_of.iter().any(|s| self.passes(value, s)) {
                    self.fail(format!(
                        "does not match any of the {} schemas in `anyOf`",
                        any_of.any_of.len(),
                    ));
                }
            }
            Schema::OneOf(one_of) => {
                let matched = one_of
                    .one_of
                    .iter()
                    .filter(|s| self.passes(value, s))
                    .count();
                if matched != 1 {
                    self.fail(format!(
                        "matches {matched} of the {} schemas in `oneOf`; exactly one is required",
                        one_of.one_of.len(),
                    ));
                }
            }
            Schema::Not(not) => {
                if self.passes(value, &not.not) {
                    self.fail("matches the schema in `not`, which it must not");
                }
            }
            Schema::Multi(multi) => {
                let actual = type_name(value);
                let allowed = multi
                    .schema_types
                    .iter()
                    .any(|schema_type| accepts_type(schema_type, value));
                if !allowed {
                    let names: Vec<String> = multi
                        .schema_types
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect();
                    self.fail(format!(
                        "expected one of [{}], got {actual}",
                        names.join(", "),
                    ));
                }
            }
            Schema::Single(single) => self.single(value, single),
        }
    }

    fn single(&mut self, value: &Value, schema: &SingleSchema) {
        match schema {
            SingleSchema::String(string) => match value.as_str() {
                Some(text) => self.string(text, string),
                None => self.wrong_type("string", value),
            },
            SingleSchema::Integer(integer) => match Whole::of(value) {
                Some(number) => self.integer(number, integer),
                None => self.wrong_type("integer", value),
            },
            SingleSchema::Number(number) => match value.as_f64() {
                Some(found) => self.number(found, number),
                None => self.wrong_type("number", value),
            },
            SingleSchema::Boolean(_) => {
                if !value.is_boolean() {
                    self.wrong_type("boolean", value);
                }
            }
            SingleSchema::Null(_) => {
                if !value.is_null() {
                    self.wrong_type("null", value);
                }
            }
            SingleSchema::Array(array) => match value.as_array() {
                Some(items) => self.array(items, array),
                None => self.wrong_type("array", value),
            },
            SingleSchema::Object(object) => match value.as_object() {
                Some(members) => self.object(members, object),
                None => self.wrong_type("object", value),
            },
        }
    }

    fn wrong_type(&mut self, expected: &str, value: &Value) {
        let actual = type_name(value);
        self.fail(format!("expected {expected}, got {actual}"));
    }

    fn string(&mut self, value: &str, schema: &StringSchema) {
        if let Some(allowed) = &schema.enum_values
            && !allowed.iter().any(|candidate| candidate == value)
        {
            self.fail(format!("{value:?} is not one of: {}", allowed.join(", "),));
        }
        // JSON Schema counts characters, not bytes.
        let length = value.chars().count() as u64;
        if let Some(min) = schema.min_length
            && length < min
        {
            self.fail(format!(
                "is shorter than minLength {min} ({length} characters)"
            ));
        }
        if let Some(max) = schema.max_length
            && length > max
        {
            self.fail(format!(
                "is longer than maxLength {max} ({length} characters)"
            ));
        }
        if let Some(pattern) = &schema.pattern {
            match regex::Regex::new(pattern) {
                Ok(regex) => {
                    if !regex.is_match(value) {
                        self.fail(format!("does not match pattern {pattern:?}"));
                    }
                }
                // A pattern this crate cannot compile is the
                // description's problem, not the request's — but saying
                // nothing would let an unchecked value look checked.
                Err(error) => self.unchecked(format!(
                    "pattern {pattern:?} could not be compiled: {error}",
                )),
            }
        }
    }

    fn integer(&mut self, value: Whole, schema: &IntegerSchema) {
        if let Some(allowed) = &schema.enum_values
            && !allowed
                .iter()
                .any(|candidate| Whole::Exact(i128::from(*candidate)).cmp(value) == Ordering::Equal)
        {
            let names: Vec<String> = allowed
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            self.fail(format!("{value} is not one of: {}", names.join(", ")));
        }

        let bound = |number: &Option<serde_json::Number>| number.as_ref().map(Whole::of_number);
        for (limit, ordering, name, inclusive) in [
            (bound(&schema.minimum), Ordering::Less, "minimum", true),
            (bound(&schema.maximum), Ordering::Greater, "maximum", true),
            (
                bound(&schema.exclusive_minimum),
                Ordering::Less,
                "exclusiveMinimum",
                false,
            ),
            (
                bound(&schema.exclusive_maximum),
                Ordering::Greater,
                "exclusiveMaximum",
                false,
            ),
        ] {
            let Some(limit) = limit else { continue };
            // Compared as `i128` whenever both sides are whole and fit,
            // so `9007199254740993` against `maximum: 9007199254740992`
            // is decided exactly rather than by two equal `f64`s.
            let relation = value.cmp(limit);
            // A tie against a bound (or a value) that `serde_json` may
            // already have rounded is not a decision — it is the one
            // case where the lost digits would have settled it.
            if relation == Ordering::Equal && (limit.is_approximate() || value.is_approximate()) {
                self.unchecked(format!(
                    "{name} {limit} is beyond the range a 64-bit float represents exactly, so \
                     {value} could NOT be checked against it",
                ));
                continue;
            }
            let breached = relation == ordering || (!inclusive && relation == Ordering::Equal);
            if breached {
                let verb = match (ordering, inclusive) {
                    (Ordering::Less, true) => "is below",
                    (Ordering::Greater, true) => "is above",
                    (Ordering::Less, _) => "is not above",
                    (Ordering::Greater, _) => "is not below",
                    _ => unreachable!("only Less and Greater are used"),
                };
                self.fail(format!("{value} {verb} {name} {limit}"));
            }
        }

        if let Some(step) = schema.multiple_of
            && step != 0.0
        {
            let divides = match (value, whole_step(step)) {
                // Both whole: an exact remainder, no rounding involved.
                (Whole::Exact(value), Some(step)) => value % step == 0,
                _ => {
                    let quotient = value.as_f64() / step;
                    (quotient - quotient.round()).abs() <= 1e-9
                }
            };
            if !divides {
                self.fail(format!("{value} is not a multiple of {step}"));
            }
        }
    }

    fn number(&mut self, value: f64, schema: &NumberSchema) {
        if let Some(allowed) = &schema.enum_values
            && !allowed
                .iter()
                .any(|candidate| (candidate - value).abs() < f64::EPSILON)
        {
            let names: Vec<String> = allowed
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            self.fail(format!("{value} is not one of: {}", names.join(", ")));
        }
        self.bounds(
            value,
            schema.minimum,
            schema.maximum,
            schema.exclusive_minimum,
            schema.exclusive_maximum,
            schema.multiple_of,
        );
    }

    /// `type: number` bounds. `roas` models every one of these as an
    /// `f64` already, so there is no exactness to preserve here — only
    /// `integer` carries `serde_json::Number` bounds.
    fn bounds(
        &mut self,
        value: f64,
        minimum: Option<f64>,
        maximum: Option<f64>,
        exclusive_minimum: Option<f64>,
        exclusive_maximum: Option<f64>,
        multiple_of: Option<f64>,
    ) {
        if let Some(min) = minimum
            && value < min
        {
            self.fail(format!("{value} is below minimum {min}"));
        }
        if let Some(max) = maximum
            && value > max
        {
            self.fail(format!("{value} is above maximum {max}"));
        }
        if let Some(min) = exclusive_minimum
            && value <= min
        {
            self.fail(format!("{value} is not above exclusiveMinimum {min}"));
        }
        if let Some(max) = exclusive_maximum
            && value >= max
        {
            self.fail(format!("{value} is not below exclusiveMaximum {max}"));
        }
        if let Some(step) = multiple_of
            && step != 0.0
        {
            let quotient = value / step;
            if (quotient - quotient.round()).abs() > 1e-9 {
                self.fail(format!("{value} is not a multiple of {step}"));
            }
        }
    }

    fn array(&mut self, values: &[Value], schema: &ArraySchema) {
        let length = values.len() as u64;
        if let Some(min) = schema.min_items
            && length < min
        {
            self.fail(format!("has {length} items, fewer than minItems {min}"));
        }
        if let Some(max) = schema.max_items
            && length > max
        {
            self.fail(format!("has {length} items, more than maxItems {max}"));
        }
        if schema.unique_items == Some(true) {
            for (index, value) in values.iter().enumerate() {
                if values[..index].contains(value) {
                    self.nested(&index.to_string(), |checker| {
                        checker.fail("repeats an earlier item, but uniqueItems is set");
                    });
                }
            }
        }
        match &schema.items {
            None | Some(BoolOr::Bool(true)) => {}
            Some(BoolOr::Bool(false)) => {
                if !values.is_empty() {
                    self.fail("must be empty: `items` is `false`");
                }
            }
            Some(BoolOr::Item(items)) => {
                for (index, value) in values.iter().enumerate() {
                    self.nested(&index.to_string(), |checker| checker.schema(value, items));
                }
            }
        }
    }

    fn object(&mut self, members: &serde_json::Map<String, Value>, schema: &ObjectSchema) {
        let count = members.len() as u64;
        if let Some(min) = schema.min_properties
            && count < min
        {
            self.fail(format!(
                "has {count} properties, fewer than minProperties {min}"
            ));
        }
        if let Some(max) = schema.max_properties
            && count > max
        {
            self.fail(format!(
                "has {count} properties, more than maxProperties {max}"
            ));
        }

        if let Some(required) = &schema.required {
            for name in required {
                if !members.contains_key(name) {
                    self.nested(name, |checker| checker.fail("is required and was not sent"));
                }
            }
        }

        // Which properties some keyword accounted for; whatever is left
        // is what `additionalProperties` judges.
        let mut evaluated = BTreeSet::new();

        if let Some(properties) = &schema.properties {
            for (name, subschema) in properties {
                if let Some(value) = members.get(name) {
                    evaluated.insert(name.as_str());
                    self.nested(name, |checker| checker.schema(value, subschema));
                }
            }
        }

        if let Some(patterns) = &schema.pattern_properties {
            for (pattern, subschema) in patterns {
                let Ok(regex) = regex::Regex::new(pattern) else {
                    self.unchecked(format!(
                        "patternProperties key {pattern:?} could not be compiled",
                    ));
                    continue;
                };
                for (name, value) in members {
                    if regex.is_match(name) {
                        evaluated.insert(name.as_str());
                        self.nested(name, |checker| checker.schema(value, subschema));
                    }
                }
            }
        }

        if let Some(names) = &schema.property_names {
            for name in members.keys() {
                let as_value = Value::String(name.clone());
                self.nested(name, |checker| checker.schema(&as_value, names));
            }
        }

        // `additionalProperties` judges what no other keyword reached —
        // and, when it is a schema or `true`, evaluates it in turn.
        let mut all_evaluated = false;
        match &schema.additional_properties {
            None => {}
            Some(BoolOr::Bool(true)) => all_evaluated = true,
            Some(BoolOr::Bool(false)) => {
                for name in members.keys() {
                    if !evaluated.contains(name.as_str()) {
                        self.nested(name, |checker| {
                            checker.fail("is not described, and additionalProperties is `false`");
                        });
                    }
                }
            }
            Some(subschema) => {
                let BoolOr::Item(subschema) = subschema else {
                    unreachable!("the boolean forms are matched above")
                };
                for (name, value) in members {
                    if !evaluated.contains(name.as_str()) {
                        self.nested(name, |checker| checker.schema(value, subschema));
                    }
                }
                all_evaluated = true;
            }
        }

        // `unevaluatedProperties` then judges whatever is *still*
        // unevaluated. In general that needs annotations collected
        // across `allOf` and friends — but not here: `roas` models a
        // composition as its own schema variant, one that carries no
        // `properties` of its own, so an `ObjectSchema` is the only
        // place this keyword can sit and the only properties in play
        // are the ones just walked.
        if !all_evaluated && let Some(unevaluated) = &schema.unevaluated_properties {
            for (name, value) in members {
                if evaluated.contains(name.as_str()) {
                    continue;
                }
                match unevaluated {
                    BoolOr::Bool(true) => {}
                    BoolOr::Bool(false) => self.nested(name, |checker| {
                        checker.fail("is not described, and unevaluatedProperties is `false`");
                    }),
                    BoolOr::Item(subschema) => {
                        self.nested(name, |checker| checker.schema(value, subschema));
                    }
                }
            }
        }
    }
}

/// The JSON Schema type name of a value, for error messages.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Whether a `type` entry of a multi-typed schema accepts this value.
/// An unrecognized custom type accepts nothing — the alternative is
/// accepting everything, which would make an unreadable description
/// look like a passing request.
fn accepts_type(schema_type: &SchemaType, value: &Value) -> bool {
    match schema_type {
        SchemaType::String => value.is_string(),
        SchemaType::Number => value.is_number(),
        SchemaType::Integer => Whole::of(value).is_some(),
        SchemaType::Object => value.is_object(),
        SchemaType::Array => value.is_array(),
        SchemaType::Boolean => value.is_boolean(),
        SchemaType::Null => value.is_null(),
        SchemaType::Custom(_) => false,
    }
}

/// A JSON number that is a whole number, kept exact wherever it fits.
///
/// JSON Schema does not restrict numbers to IEEE-754, and OpenAPI's
/// `format: int64` reaches values an `f64` cannot tell apart —
/// `9007199254740993` and `9007199254740992` are the same float. So a
/// whole number is carried as an `i128` and only falls back to `f64`
/// when it genuinely is one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Whole {
    Exact(i128),
    Wide(f64),
}

/// The largest magnitude an `f64` represents every integer up to.
const EXACT_UP_TO: f64 = 9_007_199_254_740_992.0;

impl Whole {
    /// A value seen as an integer. JSON Schema counts `1.0` as an
    /// integer — a number with a zero fractional part — not only `1`.
    fn of(value: &Value) -> Option<Self> {
        if let Some(number) = value.as_i64() {
            return Some(Whole::Exact(i128::from(number)));
        }
        if let Some(number) = value.as_u64() {
            return Some(Whole::Exact(i128::from(number)));
        }
        let number = value.as_f64()?;
        (number.fract() == 0.0).then(|| Whole::of_f64(number))
    }

    /// A schema bound, whole or not.
    fn of_number(number: &serde_json::Number) -> Self {
        if let Some(number) = number.as_i64() {
            return Whole::Exact(i128::from(number));
        }
        if let Some(number) = number.as_u64() {
            return Whole::Exact(i128::from(number));
        }
        number.as_f64().map_or(Whole::Wide(f64::NAN), Whole::of_f64)
    }

    fn of_f64(number: f64) -> Self {
        // Past 2^53 an `f64` has already lost the value it came from,
        // so widening it to `i128` would recover nothing.
        if number.fract() == 0.0 && number.abs() <= EXACT_UP_TO {
            #[expect(clippy::cast_possible_truncation, reason = "bounded above")]
            Whole::Exact(number as i128)
        } else {
            Whole::Wide(number)
        }
    }

    #[expect(clippy::cast_precision_loss, reason = "only for the inexact path")]
    fn as_f64(self) -> f64 {
        match self {
            Whole::Exact(number) => number as f64,
            Whole::Wide(number) => number,
        }
    }

    /// Whether this number may not be the one the description wrote.
    ///
    /// Past 2^53 an `f64` cannot hold every integer, and `serde_json`
    /// parses a fractional literal into one long before this crate sees
    /// it — `9007199254740993.5` arrives as `9007199254740994.0`. Such
    /// a number can still be compared, but it cannot settle a tie.
    fn is_approximate(self) -> bool {
        matches!(self, Whole::Wide(number) if number.abs() > EXACT_UP_TO)
    }

    /// Exact whenever either side is.
    ///
    /// An exact integer is never rounded to compare it with a float:
    /// the float's `floor` and `ceil` are themselves exact, so the
    /// integer can be placed against them instead. What this cannot
    /// recover is precision `serde_json` lost while parsing the
    /// description — a bound written `9007199254740993.5` is already
    /// `9007199254740994.0` by the time it arrives here.
    fn cmp(self, other: Self) -> Ordering {
        match (self, other) {
            (Whole::Exact(mine), Whole::Exact(theirs)) => mine.cmp(&theirs),
            (Whole::Exact(mine), Whole::Wide(theirs)) => cmp_exact_to_float(mine, theirs),
            (Whole::Wide(mine), Whole::Exact(theirs)) => cmp_exact_to_float(theirs, mine).reverse(),
            (Whole::Wide(mine), Whole::Wide(theirs)) => {
                mine.partial_cmp(&theirs).unwrap_or(Ordering::Equal)
            }
        }
    }
}

impl Display for Whole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Whole::Exact(number) => write!(f, "{number}"),
            Whole::Wide(number) => write!(f, "{number}"),
        }
    }
}

/// Place an exact integer against a float without rounding the integer.
///
/// `floor` and `ceil` of a finite `f64` are exact `f64`s, and inside
/// `i128`'s range they convert exactly — so the comparison happens in
/// `i128` with the float's fractional part decided separately.
fn cmp_exact_to_float(exact: i128, float: f64) -> Ordering {
    if float.is_nan() {
        return Ordering::Equal;
    }
    // Beyond `i128` the float wins outright, in whichever direction.
    const I128_LIMIT: f64 = 1.701_411_834_604_692_3e38;
    if float > I128_LIMIT {
        return Ordering::Less;
    }
    if float < -I128_LIMIT {
        return Ordering::Greater;
    }

    let floor = float.floor();
    let ceil = float.ceil();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded by I128_LIMIT above"
    )]
    let (floor_exact, ceil_exact) = (floor as i128, ceil as i128);

    if exact < floor_exact {
        Ordering::Less
    } else if exact > ceil_exact {
        Ordering::Greater
    } else if exact == floor_exact && floor < float {
        // The float has a fractional part, so it sits above its floor.
        Ordering::Less
    } else if exact == ceil_exact && ceil > float {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// A `multipleOf` step that is itself a whole number, for exact
/// remainder arithmetic.
fn whole_step(step: f64) -> Option<i128> {
    match Whole::of_f64(step) {
        Whole::Exact(step) if step != 0 => Some(step),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    pub(crate) use super::tests::{failures, passes};
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec() -> Spec {
        serde_json::from_value(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "components": {
                "schemas": {
                    "Name": { "type": "string", "minLength": 2 },
                }
            }
        }))
        .expect("the spec must parse")
    }

    fn schema(value: serde_json::Value) -> RefOr<Schema> {
        serde_json::from_value(value).expect("the schema must parse")
    }

    /// Every failure as `pointer: message`, which is what the assertions
    /// below can read at a glance.
    pub(crate) fn failures(value: &Value, schema_json: serde_json::Value) -> Vec<String> {
        check(value, &schema(schema_json), &spec())
            .into_iter()
            .map(|failure| {
                if failure.pointer.is_empty() {
                    failure.message
                } else {
                    format!("{}: {}", failure.pointer, failure.message)
                }
            })
            .collect()
    }

    pub(crate) fn passes(value: &Value, schema_json: serde_json::Value) -> bool {
        failures(value, schema_json).is_empty()
    }

    #[test]
    fn a_boolean_schema_accepts_everything_or_nothing() {
        assert!(passes(&json!(1), json!(true)));
        assert!(passes(&json!(1), json!({})));
        assert_eq!(
            failures(&json!(1), json!(false)),
            ["no value is allowed here"]
        );
    }

    #[test]
    fn a_type_mismatch_names_both_types() {
        assert_eq!(
            failures(&json!("7"), json!({ "type": "integer" })),
            ["expected integer, got string"],
        );
    }

    #[test]
    fn a_string_is_measured_in_characters() {
        assert!(passes(
            &json!("héllo"),
            json!({ "type": "string", "maxLength": 5 })
        ));
        assert_eq!(
            failures(&json!("héllo"), json!({ "type": "string", "maxLength": 4 })),
            ["is longer than maxLength 4 (5 characters)"],
        );
        assert_eq!(
            failures(&json!("a"), json!({ "type": "string", "minLength": 2 })),
            ["is shorter than minLength 2 (1 characters)"],
        );
    }

    #[test]
    fn a_string_pattern_is_a_regex() {
        assert!(passes(
            &json!("ab12"),
            json!({ "type": "string", "pattern": "^[a-z]+[0-9]+$" })
        ));
        assert_eq!(
            failures(
                &json!("12ab"),
                json!({ "type": "string", "pattern": "^[a-z]+$" })
            ),
            [r#"does not match pattern "^[a-z]+$""#],
        );
    }

    #[test]
    fn a_pattern_that_will_not_compile_is_reported_not_ignored() {
        let found = failures(&json!("x"), json!({ "type": "string", "pattern": "(" }));
        assert_eq!(found.len(), 1);
        assert!(found[0].contains("could not be compiled"), "{found:?}");
    }

    #[test]
    fn a_string_enum_lists_what_was_allowed() {
        assert!(passes(
            &json!("sold"),
            json!({ "type": "string", "enum": ["sold", "free"] })
        ));
        assert_eq!(
            failures(
                &json!("gone"),
                json!({ "type": "string", "enum": ["sold", "free"] })
            ),
            [r#""gone" is not one of: sold, free"#],
        );
    }

    #[test]
    fn an_integer_accepts_a_whole_number_however_it_was_written() {
        assert!(passes(&json!(3), json!({ "type": "integer" })));
        assert!(passes(&json!(3.0), json!({ "type": "integer" })));
        assert_eq!(
            failures(&json!(3.5), json!({ "type": "integer" })),
            ["expected integer, got number"],
        );
    }

    #[test]
    fn numeric_bounds_are_checked_both_ways() {
        let schema = json!({ "type": "integer", "minimum": 1, "maximum": 10 });
        assert!(passes(&json!(1), schema.clone()));
        assert!(passes(&json!(10), schema.clone()));
        assert_eq!(
            failures(&json!(0), schema.clone()),
            ["0 is below minimum 1"]
        );
        assert_eq!(failures(&json!(11), schema), ["11 is above maximum 10"]);
    }

    #[test]
    fn exclusive_bounds_reject_the_bound_itself() {
        let schema = json!({ "type": "number", "exclusiveMinimum": 0.0, "exclusiveMaximum": 1.0 });
        assert!(passes(&json!(0.5), schema.clone()));
        assert_eq!(
            failures(&json!(0.0), schema.clone()),
            ["0 is not above exclusiveMinimum 0"],
        );
        assert_eq!(
            failures(&json!(1.0), schema),
            ["1 is not below exclusiveMaximum 1"]
        );
    }

    #[test]
    fn multiple_of_tolerates_floating_point() {
        assert!(passes(
            &json!(0.3),
            json!({ "type": "number", "multipleOf": 0.1 })
        ));
        assert_eq!(
            failures(&json!(0.35), json!({ "type": "number", "multipleOf": 0.1 })),
            ["0.35 is not a multiple of 0.1"],
        );
    }

    #[test]
    fn an_integer_enum_lists_what_was_allowed() {
        assert_eq!(
            failures(&json!(3), json!({ "type": "integer", "enum": [1, 2] })),
            ["3 is not one of: 1, 2"],
        );
    }

    #[test]
    fn a_number_enum_lists_what_was_allowed() {
        assert!(passes(
            &json!(1.5),
            json!({ "type": "number", "enum": [1.5, 2.5] })
        ));
        assert_eq!(
            failures(&json!(3.5), json!({ "type": "number", "enum": [1.5, 2.5] })),
            ["3.5 is not one of: 1.5, 2.5"],
        );
    }

    #[test]
    fn booleans_and_nulls_check_only_their_type() {
        assert!(passes(&json!(true), json!({ "type": "boolean" })));
        assert_eq!(
            failures(&json!("true"), json!({ "type": "boolean" })),
            ["expected boolean, got string"],
        );
        assert!(passes(&json!(null), json!({ "type": "null" })));
        assert_eq!(
            failures(&json!(0), json!({ "type": "null" })),
            ["expected null, got integer"]
        );
    }

    #[test]
    fn array_items_are_judged_one_by_one_and_pointed_at() {
        assert_eq!(
            failures(
                &json!(["a", 2, "c"]),
                json!({ "type": "array", "items": { "type": "string" } }),
            ),
            ["/1: expected string, got integer"],
        );
    }

    #[test]
    fn array_lengths_are_checked() {
        let schema = json!({ "type": "array", "items": true, "minItems": 1, "maxItems": 2 });
        assert!(passes(&json!([1]), schema.clone()));
        assert_eq!(
            failures(&json!([]), schema.clone()),
            ["has 0 items, fewer than minItems 1"],
        );
        assert_eq!(
            failures(&json!([1, 2, 3]), schema),
            ["has 3 items, more than maxItems 2"],
        );
    }

    #[test]
    fn unique_items_points_at_the_repeat() {
        assert_eq!(
            failures(
                &json!([1, 2, 1]),
                json!({ "type": "array", "items": true, "uniqueItems": true }),
            ),
            ["/2: repeats an earlier item, but uniqueItems is set"],
        );
    }

    #[test]
    fn items_false_demands_an_empty_array() {
        assert!(passes(
            &json!([]),
            json!({ "type": "array", "items": false })
        ));
        assert_eq!(
            failures(&json!([1]), json!({ "type": "array", "items": false })),
            ["must be empty: `items` is `false`"],
        );
    }

    #[test]
    fn a_missing_required_property_is_pointed_at_by_name() {
        assert_eq!(
            failures(
                &json!({ "id": 1 }),
                json!({
                    "type": "object",
                    "required": ["id", "name"],
                    "properties": { "id": { "type": "integer" }, "name": { "type": "string" } },
                }),
            ),
            ["/name: is required and was not sent"],
        );
    }

    #[test]
    fn every_bad_property_is_reported_not_just_the_first() {
        let found = failures(
            &json!({ "id": "one", "name": 2 }),
            json!({
                "type": "object",
                "properties": { "id": { "type": "integer" }, "name": { "type": "string" } },
            }),
        );
        assert_eq!(
            found,
            [
                "/id: expected integer, got string",
                "/name: expected string, got integer",
            ],
        );
    }

    #[test]
    fn additional_properties_false_names_the_undescribed_property() {
        assert_eq!(
            failures(
                &json!({ "id": 1, "extra": true }),
                json!({
                    "type": "object",
                    "properties": { "id": { "type": "integer" } },
                    "additionalProperties": false,
                }),
            ),
            ["/extra: is not described, and additionalProperties is `false`"],
        );
    }

    #[test]
    fn additional_properties_may_carry_a_schema_of_their_own() {
        let schema = json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "additionalProperties": { "type": "string" },
        });
        assert!(passes(&json!({ "id": 1, "tag": "x" }), schema.clone()));
        assert_eq!(
            failures(&json!({ "id": 1, "tag": 2 }), schema),
            ["/tag: expected string, got integer"],
        );
    }

    #[test]
    fn property_counts_are_checked() {
        let schema = json!({ "type": "object", "minProperties": 1, "maxProperties": 2 });
        assert!(passes(&json!({ "a": 1 }), schema.clone()));
        assert_eq!(
            failures(&json!({}), schema.clone()),
            ["has 0 properties, fewer than minProperties 1"],
        );
        assert_eq!(
            failures(&json!({ "a": 1, "b": 2, "c": 3 }), schema),
            ["has 3 properties, more than maxProperties 2"],
        );
    }

    #[test]
    fn pattern_properties_are_matched_by_name() {
        let schema = json!({
            "type": "object",
            "patternProperties": { "^x-": { "type": "string" } },
            "additionalProperties": false,
        });
        assert!(passes(&json!({ "x-tag": "a" }), schema.clone()));
        assert_eq!(
            failures(&json!({ "x-tag": 1 }), schema),
            ["/x-tag: expected string, got integer"],
        );
    }

    #[test]
    fn property_names_are_themselves_a_schema() {
        assert_eq!(
            failures(
                &json!({ "ok": 1, "TOO_LONG": 2 }),
                json!({ "type": "object", "propertyNames": { "type": "string", "maxLength": 4 } }),
            ),
            ["/TOO_LONG: is longer than maxLength 4 (8 characters)"],
        );
    }

    #[test]
    fn a_pointer_escapes_the_two_characters_rfc_6901_reserves() {
        assert_eq!(
            failures(
                &json!({ "a/b": 1, "c~d": 2 }),
                json!({
                    "type": "object",
                    "properties": { "a/b": { "type": "string" }, "c~d": { "type": "string" } },
                }),
            ),
            [
                "/a~1b: expected string, got integer",
                "/c~0d: expected string, got integer",
            ],
        );
    }

    #[test]
    fn all_of_requires_every_branch() {
        let schema = json!({
            "allOf": [
                { "type": "object", "required": ["id"] },
                { "type": "object", "required": ["name"] },
            ]
        });
        assert!(passes(&json!({ "id": 1, "name": "x" }), schema.clone()));
        assert_eq!(
            failures(&json!({ "id": 1 }), schema),
            ["/name: is required and was not sent"],
        );
    }

    #[test]
    fn any_of_requires_one_branch() {
        let schema = json!({ "anyOf": [{ "type": "string" }, { "type": "integer" }] });
        assert!(passes(&json!("x"), schema.clone()));
        assert!(passes(&json!(1), schema.clone()));
        assert_eq!(
            failures(&json!(true), schema),
            ["does not match any of the 2 schemas in `anyOf`"],
        );
    }

    #[test]
    fn one_of_requires_exactly_one_branch() {
        let schema = json!({
            "oneOf": [
                { "type": "integer", "minimum": 0 },
                { "type": "integer", "maximum": 10 },
            ]
        });
        assert_eq!(
            failures(&json!(5), schema.clone()),
            ["matches 2 of the 2 schemas in `oneOf`; exactly one is required"],
        );
        assert!(passes(&json!(-1), schema.clone()));
        assert_eq!(
            failures(&json!("x"), schema),
            ["matches 0 of the 2 schemas in `oneOf`; exactly one is required"],
        );
    }

    #[test]
    fn not_inverts_its_branch() {
        let schema = json!({ "not": { "type": "string" } });
        assert!(passes(&json!(1), schema.clone()));
        assert_eq!(
            failures(&json!("x"), schema),
            ["matches the schema in `not`, which it must not"],
        );
    }

    #[test]
    fn a_multi_typed_schema_accepts_any_of_its_types() {
        let schema = json!({ "type": ["string", "null"] });
        assert!(passes(&json!("x"), schema.clone()));
        assert!(passes(&json!(null), schema.clone()));
        assert_eq!(
            failures(&json!(1), schema),
            ["expected one of [string, null], got integer"],
        );
    }

    #[test]
    fn a_reference_is_followed_to_the_schema_it_names() {
        assert!(passes(
            &json!("ok"),
            json!({ "$ref": "#/components/schemas/Name" })
        ));
        assert_eq!(
            failures(&json!("x"), json!({ "$ref": "#/components/schemas/Name" })),
            ["is shorter than minLength 2 (1 characters)"],
        );
    }

    #[test]
    fn a_reference_that_names_nothing_is_reported_as_unresolved() {
        let found = check(
            &json!("x"),
            &schema(json!({ "$ref": "#/components/schemas/Gone" })),
            &spec(),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, FailureKind::Unresolved, "{found:?}");
        assert!(found[0].message.contains("not found"), "{found:?}");
    }

    #[test]
    fn a_schema_that_refers_to_itself_stops_rather_than_hanging() {
        let spec: Spec = serde_json::from_value(json!({
            "openapi": "3.2.0",
            "info": { "title": "t", "version": "1" },
            "components": { "schemas": {
                "Loop": { "allOf": [{ "$ref": "#/components/schemas/Loop" }] }
            } }
        }))
        .expect("the spec must parse");
        let found = check(
            &json!(1),
            &schema(json!({ "$ref": "#/components/schemas/Loop" })),
            &spec,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, FailureKind::Unchecked, "{found:?}");
    }

    #[test]
    fn a_nested_reference_is_pointed_at_where_it_failed() {
        assert_eq!(
            failures(
                &json!({ "name": "x" }),
                json!({
                    "type": "object",
                    "properties": { "name": { "$ref": "#/components/schemas/Name" } },
                }),
            ),
            ["/name: is shorter than minLength 2 (1 characters)"],
        );
    }
}

#[cfg(test)]
mod exactness_tests {
    use super::tests_support::{failures, passes};
    use serde_json::json;

    #[test]
    fn an_integer_bound_is_compared_exactly_not_through_a_float() {
        // Both sides round to the same `f64`; only exact arithmetic
        // tells them apart.
        let schema = json!({ "type": "integer", "maximum": 9_007_199_254_740_992_i64 });
        assert!(passes(&json!(9_007_199_254_740_992_i64), schema.clone()));
        assert_eq!(
            failures(&json!(9_007_199_254_740_993_i64), schema),
            ["9007199254740993 is above maximum 9007199254740992"],
        );
    }

    #[test]
    fn a_large_integer_enum_is_compared_exactly_too() {
        let schema = json!({ "type": "integer", "enum": [9_007_199_254_740_992_i64] });
        assert!(passes(&json!(9_007_199_254_740_992_i64), schema.clone()));
        assert_eq!(
            failures(&json!(9_007_199_254_740_993_i64), schema),
            ["9007199254740993 is not one of: 9007199254740992"],
        );
    }

    #[test]
    fn a_large_multiple_of_divides_exactly() {
        let schema = json!({ "type": "integer", "multipleOf": 1_000_000_007 });
        assert!(passes(&json!(3_000_000_021_i64), schema.clone()));
        assert_eq!(
            failures(&json!(3_000_000_022_i64), schema),
            ["3000000022 is not a multiple of 1000000007"],
        );
    }

    #[test]
    fn unevaluated_properties_judges_what_nothing_else_reached() {
        let schema = json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "unevaluatedProperties": false,
        });
        assert!(passes(&json!({ "id": 1 }), schema.clone()));
        assert_eq!(
            failures(&json!({ "id": 1, "extra": true }), schema),
            ["/extra: is not described, and unevaluatedProperties is `false`"],
        );
    }

    #[test]
    fn unevaluated_properties_may_carry_a_schema() {
        let schema = json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "unevaluatedProperties": { "type": "string" },
        });
        assert!(passes(&json!({ "id": 1, "tag": "x" }), schema.clone()));
        assert_eq!(
            failures(&json!({ "id": 1, "tag": 2 }), schema),
            ["/tag: expected string, got integer"],
        );
    }

    #[test]
    fn additional_properties_evaluates_them_so_unevaluated_sees_nothing() {
        // `additionalProperties: true` covers the rest, which leaves
        // `unevaluatedProperties: false` with nothing to reject.
        let schema = json!({
            "type": "object",
            "properties": { "id": { "type": "integer" } },
            "additionalProperties": true,
            "unevaluatedProperties": false,
        });
        assert!(passes(&json!({ "id": 1, "extra": true }), schema));
    }
}
