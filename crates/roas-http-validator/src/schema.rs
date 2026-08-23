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
                let actual = type_name(value);
                let accepting: Vec<&SchemaType> = multi
                    .schema_types
                    .iter()
                    .filter(|schema_type| accepts_type(schema_type, value))
                    .collect();
                // Accepted only because it looks like an integer, and
                // whether it is one cannot be established — same
                // ambiguity as a single `type: integer`.
                if !accepting.is_empty()
                    && accepting
                        .iter()
                        .all(|schema_type| matches!(schema_type, SchemaType::Integer))
                    && Num::of_value(value).is_some_and(|number| !number.is_whole())
                {
                    self.unchecked(
                        "arrived as a floating-point number, so whether it is an integer could \
                         NOT be established",
                    );
                    return;
                }
                let allowed = !accepting.is_empty();
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
            SingleSchema::Integer(integer) => match Num::of_value(value) {
                // Parsed as an integer, so it provably had no fraction.
                Some(number) if number.is_whole() => self.integer(number, integer),
                // It definitely has one.
                Some(number) if !number.may_be_whole() => self.wrong_type("integer", value),
                // It arrived as a float with nothing left of a fraction
                // — which is not the same as never having had one.
                Some(number) => self.unchecked(format!(
                    "{number} arrived as a floating-point number, so whether it was written \
                     without a fraction could NOT be established",
                )),
                None => self.wrong_type("integer", value),
            },
            SingleSchema::Number(number) => match Num::of_value(value) {
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

    fn integer(&mut self, value: Num, schema: &IntegerSchema) {
        if let Some(allowed) = &schema.enum_values {
            let candidates = allowed
                .iter()
                .map(|candidate| Num::Exact(i128::from(*candidate)));
            self.enumerated(value, candidates, allowed.len());
        }
        let bound = |number: &Option<serde_json::Number>| number.as_ref().map(Num::of_number);
        self.bounds(
            value,
            bound(&schema.minimum),
            bound(&schema.maximum),
            bound(&schema.exclusive_minimum),
            bound(&schema.exclusive_maximum),
            schema.multiple_of,
        );
    }

    fn number(&mut self, value: Num, schema: &NumberSchema) {
        if let Some(allowed) = &schema.enum_values {
            let candidates = allowed.iter().copied().map(Num::Real);
            self.enumerated(value, candidates, allowed.len());
        }
        self.bounds(
            value,
            schema.minimum.map(Num::Real),
            schema.maximum.map(Num::Real),
            schema.exclusive_minimum.map(Num::Real),
            schema.exclusive_maximum.map(Num::Real),
            schema.multiple_of,
        );
    }

    /// `enum` over numbers.
    ///
    /// Two numbers that differ as `f64` differed as written, so "no
    /// member matched" is always safe to say. A member that *does*
    /// match but sits past the exact range is the uncertain case: two
    /// different literals could have landed on it.
    fn enumerated(&mut self, value: Num, candidates: impl Iterator<Item = Num>, total: usize) {
        let mut names = Vec::with_capacity(total);
        let mut matched = None;
        for candidate in candidates {
            names.push(candidate.to_string());
            if matched.is_none() && candidate.cmp(value) == Ordering::Equal {
                matched = Some(candidate);
            }
        }
        match matched {
            Some(candidate) if candidate.is_approximate() || value.is_approximate() => {
                self.unchecked(format!(
                    "{value} and the `enum` member {candidate} are beyond the range a 64-bit \
                     float represents exactly, so whether they are the same number could NOT be \
                     established",
                ));
            }
            Some(_) => {}
            None => self.fail(format!("{value} is not one of: {}", names.join(", "))),
        }
    }

    /// `minimum` / `maximum` / their exclusive twins / `multipleOf`,
    /// for both `integer` and `number`.
    fn bounds(
        &mut self,
        value: Num,
        minimum: Option<Num>,
        maximum: Option<Num>,
        exclusive_minimum: Option<Num>,
        exclusive_maximum: Option<Num>,
        multiple_of: Option<f64>,
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
            // Compared exactly whenever both sides are whole and fit, so
            // `9007199254740993` against `maximum: 9007199254740992` is
            // decided rather than rounded into a tie.
            let relation = value.cmp(limit);
            // A tie against a bound (or a value) that floating point may
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

        if let Some(step) = multiple_of
            && step != 0.0
        {
            // A whole value and a whole step: an integer remainder, no
            // float anywhere in it, at any magnitude.
            if let (Num::Exact(value), Some(step)) = (value, whole_step(step)) {
                if value % step != 0 {
                    self.fail(format!("{value} is not a multiple of {step}"));
                }
                return;
            }

            // Everything else goes through a division, and a division
            // is only evidence while its remainder survives it.
            //
            // `9007199254740993 / 4503599627370496` is `2.0000...` with
            // a remainder of 1 — but the numerator does not fit an
            // `f64`, so it arrives as `9007199254740992` and the
            // quotient comes out exactly `2`. `9007199254740992 / 1.5`
            // loses its remainder the other way, in a quotient too
            // large to hold one. Neither can be called divisible.
            let quotient = value.as_f64() / step;
            if !value.survives_f64() || quotient.abs() >= EXACT_WHOLE_LIMIT {
                self.unchecked(format!(
                    "whether {value} is a multiple of {step} could NOT be established: the \
                     division loses its remainder to floating point",
                ));
                return;
            }

            // Relative, not absolute: the error to allow for is the
            // representation error of the quotient itself. A fixed
            // `1e-9` would wave through `1.0000000005` against a step
            // of `1`, which is not a rounding artefact but a different
            // number.
            let tolerance = f64::EPSILON * quotient.abs().max(1.0) * 4.0;
            if (quotient - quotient.round()).abs() > tolerance {
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

/// What a subschema made of a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    Failed,
    /// The subschema could not be applied, so there is no verdict —
    /// which is not the same as the value failing it.
    Unchecked,
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
        SchemaType::Integer => Num::of_value(value).is_some_and(Num::may_be_whole),
        SchemaType::Object => value.is_object(),
        SchemaType::Array => value.is_array(),
        SchemaType::Boolean => value.is_boolean(),
        SchemaType::Null => value.is_null(),
        SchemaType::Custom(_) => false,
    }
}

/// A JSON number, kept exact wherever it fits.
///
/// JSON Schema does not restrict numbers to IEEE-754, and OpenAPI's
/// `format: int64` reaches values an `f64` cannot tell apart —
/// `9007199254740993` and `9007199254740992` are the same float. A
/// whole number is therefore carried as an `i128`, and only what
/// genuinely is a real falls back to `f64`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Exact(i128),
    Real(f64),
}

/// The magnitude at which an `f64`'s steps reach 1, so whole digits
/// start being lost rather than only fractional ones.
const EXACT_WHOLE_LIMIT: f64 = 4_503_599_627_370_496.0;

impl Num {
    /// Any JSON number.
    ///
    /// `Exact` is reserved for numbers `serde_json` parsed **as
    /// integers** — that is the only evidence available that the number
    /// was written without a fraction. Everything else is a `Real`,
    /// however whole it happens to look.
    fn of_value(value: &Value) -> Option<Self> {
        if let Some(number) = value.as_i64() {
            return Some(Num::Exact(i128::from(number)));
        }
        if let Some(number) = value.as_u64() {
            return Some(Num::Exact(i128::from(number)));
        }
        value.as_f64().map(Num::Real)
    }

    /// A schema bound.
    ///
    /// Same rule, and for the same reason: a bound `roas` holds as an
    /// `f64` is one whose lexeme is already gone.
    fn of_number(number: &serde_json::Number) -> Self {
        if let Some(number) = number.as_i64() {
            return Num::Exact(i128::from(number));
        }
        if let Some(number) = number.as_u64() {
            return Num::Exact(i128::from(number));
        }
        number.as_f64().map_or(Num::Real(f64::NAN), Num::Real)
    }

    /// Whether the number is *provably* a whole number.
    ///
    /// Only an integer that survived parsing as an integer is. No
    /// magnitude threshold can stand in for this: `2251799813685248.25`
    /// is stored as `2251799813685248.0` at 2^51, and a small enough
    /// fraction rounds away at *every* magnitude — `1.0000000000000001`
    /// is `1.0`. The lexeme is the only proof, and it is gone by the
    /// time this crate is handed a `Value`.
    fn is_whole(self) -> bool {
        matches!(self, Num::Exact(_))
    }

    /// Whether the number *could* be whole: it is, or it arrived as a
    /// float with nothing left of a fraction.
    fn may_be_whole(self) -> bool {
        match self {
            Num::Exact(_) => true,
            Num::Real(number) => number.fract() == 0.0,
        }
    }

    /// Whether this number may not be the one that was written, in a
    /// way that can flip a comparison.
    ///
    /// A real at or above [`EXACT_WHOLE_LIMIT`] has lost whole digits,
    /// not just fractional ones, so it cannot settle a tie. Below that,
    /// the ordinary floating-point caveats apply and are left alone:
    /// every JSON Schema implementation compares `0.1` with the `f64`
    /// nearest `0.1`, and flagging that would make the crate useless
    /// rather than careful.
    fn is_approximate(self) -> bool {
        matches!(self, Num::Real(number) if number.abs() >= EXACT_WHOLE_LIMIT)
    }

    #[expect(clippy::cast_precision_loss, reason = "only for the inexact path")]
    fn as_f64(self) -> f64 {
        match self {
            Num::Exact(number) => number as f64,
            Num::Real(number) => number,
        }
    }

    /// Whether `as_f64` would give this number back unchanged.
    fn survives_f64(self) -> bool {
        match self {
            Num::Exact(number) => {
                let as_float = self.as_f64();
                as_float.abs() < 1.701_411_834_604_692_3e38 && {
                    #[expect(clippy::cast_possible_truncation, reason = "bounded above")]
                    let back = as_float as i128;
                    back == number
                }
            }
            Num::Real(_) => true,
        }
    }

    /// Exact whenever either side is.
    ///
    /// An exact integer is never rounded to compare it with a float:
    /// the float's `floor` and `ceil` are themselves exact, so the
    /// integer can be placed against them instead.
    fn cmp(self, other: Self) -> Ordering {
        match (self, other) {
            (Num::Exact(mine), Num::Exact(theirs)) => mine.cmp(&theirs),
            (Num::Exact(mine), Num::Real(theirs)) => cmp_exact_to_float(mine, theirs),
            (Num::Real(mine), Num::Exact(theirs)) => cmp_exact_to_float(theirs, mine).reverse(),
            (Num::Real(mine), Num::Real(theirs)) => {
                mine.partial_cmp(&theirs).unwrap_or(Ordering::Equal)
            }
        }
    }
}

impl Display for Num {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Num::Exact(number) => write!(f, "{number}"),
            Num::Real(number) => write!(f, "{number}"),
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

/// A `multipleOf` step that is a whole number, for exact remainder
/// arithmetic.
///
/// `roas` holds `multipleOf` as an `f64`, so the step's lexeme is gone
/// either way; a whole one is taken at face value as the integer it
/// looks like, which is what every implementation does with it.
fn whole_step(step: f64) -> Option<i128> {
    if step.fract() != 0.0 || step == 0.0 || step.abs() >= 1.701_411_834_604_692_3e38 {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, reason = "bounded above")]
    Some(step as i128)
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
    fn an_integer_must_be_provably_whole_not_merely_whole_looking() {
        // Parsed as an integer: proof enough.
        assert!(passes(&json!(3), json!({ "type": "integer" })));
        // Definitely has a fraction: definitely the wrong type.
        assert_eq!(
            failures(&json!(3.5), json!({ "type": "integer" })),
            ["expected integer, got number"],
        );
        // `3.0` looks whole, but so does `3.0000000000000001` once an
        // `f64` has had it — and nothing here can tell them apart.
        let found = failures(&json!(3.0), json!({ "type": "integer" }));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("could NOT be established"), "{found:?}");
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
