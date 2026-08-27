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

use roas::common::bool_or::BoolOr;
use roas::common::formats::SchemaType;
use roas::common::reference::RefOr;
use roas::v3_2::schema::{
    ArraySchema, IntegerSchema, NumberSchema, ObjectSchema, Schema, SingleSchema, StringSchema,
};
use roas::v3_2::spec::Spec;
use serde_json::Value;

use crate::decimal::Decimal;

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

    /// What `schema` makes of `value`, without recording anything —
    /// what `anyOf`, `oneOf` and `not` need.
    ///
    /// Three states, not two. A subschema that could not be applied is
    /// not a subschema the value failed: reading it as one would let
    /// `{ "not": { "pattern": "(" } }` accept anything at all, on the
    /// strength of a check that never ran.
    fn judge(&self, value: &Value, schema: &RefOr<Schema>) -> Verdict {
        let mut probe = Checker::new(self.spec);
        // The probe continues this walk, so it inherits its depth — a
        // cycle that runs through `anyOf` must still terminate.
        probe.depth = self.depth;
        probe.schema(value, schema);
        if probe.failures.is_empty() {
            return Verdict::Passed;
        }
        // The constraints of one schema are a conjunction, and in
        // three-valued logic `false AND unknown` is false: one
        // constraint the value definitely broke settles the schema,
        // however many others could not be applied. `minLength: 2`
        // rejects `"x"` whether or not the `pattern` beside it compiles.
        if probe
            .failures
            .iter()
            .any(|failure| failure.kind == FailureKind::Violated)
        {
            return Verdict::Failed;
        }
        // Nothing definite either way: everything recorded was a check
        // that could not be made.
        Verdict::Unchecked
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
                let total = any_of.any_of.len();
                let verdicts: Vec<Verdict> = any_of
                    .any_of
                    .iter()
                    .map(|subschema| self.judge(value, subschema))
                    .collect();
                if verdicts.contains(&Verdict::Passed) {
                    // One branch is enough, and this one really matched.
                } else if verdicts.contains(&Verdict::Unchecked) {
                    self.unchecked(format!(
                        "no branch of `anyOf` matched, but not all {total} could be applied",
                    ));
                } else {
                    self.fail(format!(
                        "does not match any of the {total} schemas in `anyOf`"
                    ));
                }
            }
            Schema::OneOf(one_of) => {
                let total = one_of.one_of.len();
                let verdicts: Vec<Verdict> = one_of
                    .one_of
                    .iter()
                    .map(|subschema| self.judge(value, subschema))
                    .collect();
                let matched = verdicts.iter().filter(|v| **v == Verdict::Passed).count();
                if matched > 1 {
                    // More than one match is a failure whatever the
                    // remaining branches would have said.
                    self.fail(format!(
                        "matches {matched} of the {total} schemas in `oneOf`; exactly one is required",
                    ));
                } else if verdicts.contains(&Verdict::Unchecked) {
                    // An unapplied branch might have matched too, and
                    // `oneOf` turns on exactly how many did.
                    self.unchecked(format!(
                        "`oneOf` matched {matched} of {total} schemas, but not all of them could be applied",
                    ));
                } else if matched != 1 {
                    self.fail(format!(
                        "matches {matched} of the {total} schemas in `oneOf`; exactly one is required",
                    ));
                }
            }
            Schema::Not(not) => match self.judge(value, &not.not) {
                Verdict::Passed => self.fail("matches the schema in `not`, which it must not"),
                Verdict::Failed => {}
                // The value is accepted only when the inner schema
                // really rejected it.
                Verdict::Unchecked => {
                    self.unchecked("the schema in `not` could not be applied");
                }
            },
            Schema::Multi(multi) => {
                let verdicts: Vec<Accepts> = multi
                    .schema_types
                    .iter()
                    .map(|schema_type| accepts_type(schema_type, value))
                    .collect();
                if verdicts.contains(&Accepts::Yes) {
                    // One of the listed types really does accept it.
                } else if verdicts.contains(&Accepts::Undecidable) {
                    // Nothing else matched, and the type that might have
                    // is the one that cannot be told — the same answer a
                    // single `type: integer` gives.
                    self.unchecked(
                        "has more digits than this crate can hold, so it could NOT be checked",
                    );
                } else {
                    let names: Vec<String> = multi
                        .schema_types
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect();
                    self.fail(format!(
                        "expected one of [{}], got {}",
                        names.join(", "),
                        type_name(value),
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
            SingleSchema::Integer(integer) => match read(value) {
                // Exact, so `1.0` is an integer and
                // `1.0000000000000001` is not — a distinction the
                // literal carries and a double does not.
                Reading::Exact(number) if number.is_integer() => self.integer(number, integer),
                Reading::Exact(_) => self.wrong_type("integer", value),
                Reading::TooLarge => self.unchecked(
                    "has more digits than this crate can hold, so it could NOT be checked",
                ),
                Reading::NotANumber => self.wrong_type("integer", value),
            },
            SingleSchema::Number(number_schema) => match read(value) {
                Reading::Exact(number) => self.number(number, number_schema),
                Reading::TooLarge => self.unchecked(
                    "has more digits than this crate can hold, so it could NOT be checked",
                ),
                Reading::NotANumber => self.wrong_type("number", value),
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

    fn integer(&mut self, value: Decimal, schema: &IntegerSchema) {
        if let Some(allowed) = &schema.enum_values {
            let candidates = allowed
                .iter()
                .map(|candidate| Decimal::parse(&candidate.to_string()));
            self.enumerated(value, candidates);
        }
        self.bounds(
            value,
            schema.minimum.as_ref().map(bound_of),
            schema.maximum.as_ref().map(bound_of),
            schema.exclusive_minimum.as_ref().map(bound_of),
            schema.exclusive_maximum.as_ref().map(bound_of),
            schema.multiple_of.as_ref().map(bound_of),
        );
    }

    fn number(&mut self, value: Decimal, schema: &NumberSchema) {
        if let Some(allowed) = &schema.enum_values {
            let candidates = allowed.iter().copied().map(bound_of_f64);
            self.enumerated(value, candidates);
        }
        self.bounds(
            value,
            schema.minimum.map(bound_of_f64),
            schema.maximum.map(bound_of_f64),
            schema.exclusive_minimum.map(bound_of_f64),
            schema.exclusive_maximum.map(bound_of_f64),
            // `multipleOf` is a `serde_json::Number` even here, so this
            // one keeps its literal.
            schema.multiple_of.as_ref().map(bound_of),
        );
    }

    /// `enum` over numbers, compared by value rather than by spelling:
    /// `1`, `1.0` and `10e-1` are the same member.
    fn enumerated(&mut self, value: Decimal, candidates: impl Iterator<Item = Option<Decimal>>) {
        let mut names = Vec::new();
        let mut unreadable = false;
        for candidate in candidates {
            let Some(candidate) = candidate else {
                unreadable = true;
                continue;
            };
            names.push(candidate.to_string());
            if candidate.compare(value) == Some(Ordering::Equal) {
                return;
            }
        }
        if unreadable {
            // A member nobody can read might have been the match.
            self.unchecked(format!(
                "whether {value} is one of the `enum` members could NOT be established: one of \
                 them has more digits than this crate can hold",
            ));
            return;
        }
        self.fail(format!("{value} is not one of: {}", names.join(", ")));
    }

    /// `minimum` / `maximum` / their exclusive twins / `multipleOf`.
    fn bounds(
        &mut self,
        value: Decimal,
        minimum: Option<Option<Decimal>>,
        maximum: Option<Option<Decimal>>,
        exclusive_minimum: Option<Option<Decimal>>,
        exclusive_maximum: Option<Option<Decimal>>,
        multiple_of: Option<Option<Decimal>>,
    ) {
        for (limit, ordering, name, inclusive) in [
            (minimum, Ordering::Less, "minimum", true),
            (maximum, Ordering::Greater, "maximum", true),
            (exclusive_minimum, Ordering::Less, "exclusiveMinimum", false),
            (
                exclusive_maximum,
                Ordering::Greater,
                "exclusiveMaximum",
                false,
            ),
        ] {
            let Some(limit) = limit else { continue };
            let Some(limit) = limit else {
                self.unchecked(format!(
                    "{name} has more digits than this crate can hold, so {value} could NOT be \
                     checked against it",
                ));
                continue;
            };
            let Some(relation) = value.compare(limit) else {
                self.unchecked(format!(
                    "whether {value} satisfies {name} {limit} could NOT be established: the \
                     comparison does not fit",
                ));
                continue;
            };
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

        if let Some(step) = multiple_of {
            self.divisibility(value, step);
        }
    }

    /// `multipleOf`, which is now the same kind of question as the rest.
    ///
    /// It used to be the one keyword a rounding error flipped outright,
    /// because divisibility is true on a set of measure zero and a
    /// division destroys the remainder that decides it. With both
    /// numbers exact it is an integer remainder and nothing else.
    fn divisibility(&mut self, value: Decimal, step: Option<Decimal>) {
        let Some(step) = step else {
            self.unchecked(
                "`multipleOf` has more digits than this crate can hold, so divisibility could \
                 NOT be checked",
            );
            return;
        };
        match value.is_multiple_of(step) {
            Some(true) => {}
            Some(false) => self.fail(format!("{value} is not a multiple of {step}")),
            // A zero step is the description's error, which `roas`'s own
            // validator reports; scaling that will not fit is this
            // crate's limit and is reported as one.
            None if step.is_zero() => {}
            None => self.unchecked(format!(
                "whether {value} is a multiple of {step} could NOT be established: the \
                 arithmetic does not fit",
            )),
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
                if values[..index]
                    .iter()
                    .any(|earlier| same_value(earlier, value))
                {
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

/// What a subschema made of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    /// The subschema could not be applied, so there is no verdict —
    /// which is not the same as the value failing it.
    Unchecked,
}

/// Whether two values are the same *value*, which is not the same
/// question as whether they are the same JSON.
///
/// With `arbitrary_precision` a number carries its literal, so
/// `Value`'s own equality compares spellings: `1.0` and `1.00` are
/// different values to it and the same number to JSON Schema. That
/// matters wherever equality decides something — `uniqueItems` is the
/// one place, and it would otherwise call `[1.0, 1.00]` unique.
fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(_), Value::Number(_)) => match (read(left), read(right)) {
            (Reading::Exact(left), Reading::Exact(right)) => {
                left.compare(right) == Some(Ordering::Equal)
            }
            // Two literals nothing can read are the same only if they
            // were written the same.
            _ => left == right,
        },
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_value(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, left)| right.get(key).is_some_and(|right| same_value(left, right)))
        }
        _ => left == right,
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
fn accepts_type(schema_type: &SchemaType, value: &Value) -> Accepts {
    let yes_no = |accepted: bool| {
        if accepted { Accepts::Yes } else { Accepts::No }
    };
    match schema_type {
        SchemaType::String => yes_no(value.is_string()),
        SchemaType::Number => yes_no(value.is_number()),
        // The one type whose answer can be unknown: a literal too large
        // to read is certainly a number and may or may not be whole.
        SchemaType::Integer => match read(value) {
            Reading::Exact(number) => yes_no(number.is_integer()),
            Reading::TooLarge => Accepts::Undecidable,
            Reading::NotANumber => Accepts::No,
        },
        SchemaType::Object => yes_no(value.is_object()),
        SchemaType::Array => yes_no(value.is_array()),
        SchemaType::Boolean => yes_no(value.is_boolean()),
        SchemaType::Null => yes_no(value.is_null()),
        SchemaType::Custom(_) => Accepts::No,
    }
}

/// What one entry of a multi-typed schema makes of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Accepts {
    Yes,
    No,
    /// The type cannot be told, so neither accepting nor rejecting on
    /// its account would be honest.
    Undecidable,
}

/// A JSON value read as an exact number.
///
/// Three answers rather than two, because a literal past `i128`'s range
/// is neither a number this crate can reason about nor a value of the
/// wrong type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    Exact(Decimal),
    /// A number, but written with more digits than can be held.
    TooLarge,
    NotANumber,
}

/// Read a JSON value as the number it was written as.
///
/// `serde_json` carries the literal here — see [`crate::decimal`] — so
/// this is the number the document actually contained, not what an
/// `f64` made of it.
fn read(value: &Value) -> Reading {
    let Some(number) = value.as_number() else {
        return Reading::NotANumber;
    };
    match Decimal::parse(number.as_str()) {
        Some(decimal) => Reading::Exact(decimal),
        None => Reading::TooLarge,
    }
}

/// A schema bound `roas` keeps as a `serde_json::Number`, which means
/// it keeps its literal too.
fn bound_of(number: &serde_json::Number) -> Option<Decimal> {
    Decimal::parse(number.as_str())
}

/// A schema bound `roas` models as an `f64`.
///
/// `NumberSchema`'s bounds and both `enum` lists are still `f64` in the
/// model, so their literals are gone before this crate is handed them —
/// `maximum: 9007199254740993` arrives as `9007199254740992`. The
/// shortest representation of the `f64` is the best available reading
/// of what was meant, and it is exactly what every other implementation
/// compares against. Only the *instance* side is exact for these.
fn bound_of_f64(number: f64) -> Option<Decimal> {
    Decimal::parse(&number.to_string())
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
    fn zero_is_a_multiple_of_anything_at_all() {
        assert!(passes(
            &json!(0),
            json!({ "type": "integer", "multipleOf": 7 })
        ));
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
