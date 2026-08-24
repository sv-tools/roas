//! Reading one Parameter Object's value out of a request.
//!
//! This is where a request validator does most of its real work, and
//! where the naive version goes wrong. A parameter arrives as text —
//! `?limit=10` is the two characters `1` and `0`, not the number ten —
//! so before a Schema Object can judge it, the text has to be turned
//! back into the value the description says it is. `style` and `explode`
//! say how it was flattened on the way out; this undoes that.
//!
//! All seven styles of
//! [§4.8.11.2](https://spec.openapis.org/oas/v3.2.0#style-values) are
//! handled: `matrix`, `label` and `simple` for paths, `form`,
//! `spaceDelimited`, `pipeDelimited` and `deepObject` for queries.
//!
//! What the value is turned *into* is decided by the schema: an
//! `integer` parameter parses as a number, an `array` splits, an
//! `object` reassembles. A schema this crate cannot read that way — a
//! composition, say — leaves the value a string, so the schema still
//! judges it and the verdict is at worst too strict, never too lax.

use std::collections::BTreeMap;

use roas::common::bool_or::BoolOr;
use roas::common::formats::SchemaType;
use roas::common::reference::RefOr;
use roas::v3_2::media_type::{Encoding, MediaType};
use roas::v3_2::parameter::{InCookieStyle, InHeaderStyle, InPathStyle, InQueryStyle, Parameter};
use roas::v3_2::schema::{Schema, SingleSchema};
use roas::v3_2::spec::Spec;
use serde_json::Value;

use crate::body;
use crate::report::{ErrorKind, Location, ValidationError};
use crate::request::{RequestView, decode_form, decode_path_segment, split_query};
use crate::schema;

/// How a parameter's value was flattened into text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    Matrix,
    Label,
    Simple,
    Form,
    SpaceDelimited,
    PipeDelimited,
    DeepObject,
}

/// The fields of a Parameter Object this module cares about, with the
/// four `in` variants flattened into one shape.
struct Described<'p> {
    name: &'p str,
    location: Location,
    required: bool,
    style: Style,
    explode: bool,
    schema: Option<&'p RefOr<Schema>>,
    content: Option<&'p BTreeMap<String, RefOr<MediaType>>>,
}

/// What kind of value the schema says this parameter holds.
enum Shape<'s> {
    Primitive(Primitive),
    Array(Option<&'s RefOr<Schema>>),
    Object(Option<&'s BTreeMap<String, RefOr<Schema>>>),
    /// A schema this module does not read structurally — a composition,
    /// a `$ref` that does not resolve, or no schema at all.
    Opaque,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Primitive {
    String,
    Integer,
    Number,
    Boolean,
    Null,
}

/// What one request yields once rather than once per parameter.
///
/// Decoding the query string and the cookie header is not free, and an
/// operation with ten parameters would otherwise do it ten times — this
/// is middleware, on the request path of every call.
pub(crate) struct Extracted<'r> {
    /// Path parameters, as the matched template read them.
    pub(crate) path: &'r BTreeMap<String, String>,
    /// The query string, decoded, in order and with repeats.
    pub(crate) query: Vec<(String, String)>,
    /// The cookies from the `Cookie` header, in the order sent.
    pub(crate) cookies: Vec<(String, String)>,
}

impl<'r> Extracted<'r> {
    pub(crate) fn new(request: &RequestView<'_>, path: &'r BTreeMap<String, String>) -> Self {
        Self {
            path,
            query: request.query_pairs_raw(),
            cookies: request.cookies(),
        }
    }
}

/// Rebuild an `application/x-www-form-urlencoded` body.
///
/// A form body is a query string, and the Encoding Object says how each
/// field was flattened with exactly the `style` and `explode` a query
/// parameter uses — so it is read by the same code, field by field.
/// Without that, `tags=a&tags=b` collapses to the last value instead of
/// becoming a two-item array.
pub(crate) fn read_form_body(
    text: &str,
    properties: Option<&BTreeMap<String, RefOr<Schema>>>,
    encoding: Option<&BTreeMap<String, Encoding>>,
    spec: &Spec,
) -> Result<Value, String> {
    let pairs = split_query(text);
    let mut object = serde_json::Map::new();
    let mut fields = Vec::new();

    // Described fields first, so each is read through its own schema.
    if let Some(properties) = properties {
        for (name, schema) in properties {
            let field =
                Described::form_field(name, encoding.and_then(|e| e.get(name)), Some(schema));
            if let Some(value) = field.read_form_field(&pairs, spec)? {
                object.insert(name.clone(), value);
            }
            fields.push(field);
        }
    }

    // Then whatever else arrived, as the strings it arrived as, so
    // `additionalProperties` still has something to judge. A pair a
    // described field already consumed is not "whatever else": an
    // exploded object property `filter` swallows `role=admin`, and
    // adding a top-level `role` beside it would both misdescribe the
    // body and trip `additionalProperties: false`.
    for (name, value) in &pairs {
        if object.contains_key(name) || fields.iter().any(|field| field.accounts_for(name, spec)) {
            continue;
        }
        object.insert(name.clone(), Value::String(decode_form(value)));
    }
    Ok(Value::Object(object))
}

/// Whether one parameter accounts for a query-string name.
///
/// Usually that means the name *is* the parameter's — but not always,
/// and both exceptions matter to stray detection.
///
/// A `deepObject` parameter arrives as `filter[role]`, so it accounts
/// for anything under its bracket. And a `form` object that explodes
/// has no name in the query at all: its properties become top-level
/// pairs, so `filter` accounts for `role` and `age` and **not** for
/// `filter` — which is what makes `?filter=garbage` a stray rather
/// than something silently ignored.
pub(crate) fn accounts_for(parameter: &Parameter, name: &str, spec: &Spec) -> bool {
    Described::of(parameter).accounts_for(name, spec)
}

/// Judge one parameter, appending whatever is wrong to `errors`./// Judge one parameter, appending whatever is wrong to `errors`.
pub(crate) fn validate(
    parameter: &Parameter,
    request: &RequestView<'_>,
    extracted: &Extracted<'_>,
    spec: &Spec,
    errors: &mut Vec<ValidationError>,
) {
    let described = Described::of(parameter);
    let mut push = |pointer: String, kind: ErrorKind| {
        errors.push(ValidationError {
            location: described.location,
            name: described.name.to_owned(),
            pointer,
            kind,
        });
    };

    // A parameter serialized as a media type rather than by `style`.
    if let Some(content) = described.content {
        let Some(raw) = described.raw_text(request, extracted) else {
            if described.required {
                push(String::new(), ErrorKind::Missing);
            }
            return;
        };
        validate_as_content(&raw, content, spec, &mut push);
        return;
    }

    let shape = described
        .schema
        .map_or(Shape::Opaque, |schema| Shape::of(schema, spec));

    let value = match described.read(request, extracted, &shape, spec) {
        Ok(Some(value)) => value,
        Ok(None) => {
            if described.required {
                push(String::new(), ErrorKind::Missing);
            }
            return;
        }
        Err(why) => {
            push(String::new(), ErrorKind::Malformed(why));
            return;
        }
    };

    let Some(declared) = described.schema else {
        return;
    };
    report_failures(&value, declared, spec, &mut push);
}

/// A parameter whose value is a document of its own — `content` rather
/// than `style`. The body decoder does the reading, so a `querystring`
/// parameter carrying `application/x-www-form-urlencoded` and a body
/// carrying it are read by the same code.
fn validate_as_content(
    raw: &str,
    content: &BTreeMap<String, RefOr<MediaType>>,
    spec: &Spec,
    push: &mut impl FnMut(String, ErrorKind),
) {
    // The specification requires exactly one entry here.
    let Some((media_type, entry)) = content.iter().next() else {
        return;
    };
    let entry = match entry.get_item(spec) {
        Ok(entry) => entry,
        Err(error) => {
            push(
                String::new(),
                ErrorKind::UnresolvedReference(error.to_string()),
            );
            return;
        }
    };
    let Some(declared) = &entry.schema else {
        return;
    };
    let value = match body::decode(
        raw.as_bytes(),
        media_type,
        declared,
        entry.encoding.as_ref(),
        spec,
    ) {
        Ok(value) => value,
        Err(body::Decoded::Malformed(why)) => {
            push(String::new(), ErrorKind::Malformed(why));
            return;
        }
        Err(body::Decoded::Unsupported(what)) => {
            push(String::new(), ErrorKind::Unsupported(what));
            return;
        }
    };
    report_failures(&value, declared, spec, push);
}

/// Turn schema failures into validation errors.
fn report_failures(
    value: &Value,
    declared: &RefOr<Schema>,
    spec: &Spec,
    push: &mut impl FnMut(String, ErrorKind),
) {
    for failure in schema::check(value, declared, spec) {
        push(
            failure.pointer,
            match failure.kind {
                schema::FailureKind::Unresolved => ErrorKind::UnresolvedReference(failure.message),
                schema::FailureKind::Unchecked => ErrorKind::Unchecked(failure.message),
                schema::FailureKind::Violated => ErrorKind::Schema(failure.message),
            },
        );
    }
}

/// Whether a media type carries JSON, `+json` suffixes included.
pub(crate) fn is_json(media_type: &str) -> bool {
    let media_type = media_type.trim().to_ascii_lowercase();
    media_type == "application/json"
        || media_type.ends_with("+json")
        || media_type.starts_with("application/json;")
}

impl<'p> Described<'p> {
    /// One field of a form body, described by its Encoding Object.
    fn form_field(
        name: &'p str,
        encoding: Option<&Encoding>,
        schema: Option<&'p RefOr<Schema>>,
    ) -> Self {
        let style = match encoding.and_then(|encoding| encoding.style.as_ref()) {
            Some(InQueryStyle::SpaceDelimited) => Style::SpaceDelimited,
            Some(InQueryStyle::PipeDelimited) => Style::PipeDelimited,
            Some(InQueryStyle::DeepObject) => Style::DeepObject,
            Some(InQueryStyle::Form) | None => Style::Form,
        };
        Self {
            name,
            location: Location::Query,
            required: false,
            style,
            // As for a query parameter: `form` explodes by default.
            explode: encoding
                .and_then(|encoding| encoding.explode)
                .unwrap_or(style == Style::Form),
            schema,
            content: None,
        }
    }

    /// The property a `deepObject` pair names, as `filter[role]` names
    /// `role`. The one place this shape is spelled out, so accounting
    /// and decoding cannot disagree about what counts.
    fn deep_object_key<'n>(&self, name: &'n str) -> Option<&'n str> {
        name.strip_prefix(self.name)?
            .strip_prefix('[')?
            .strip_suffix(']')
    }

    /// Whether this parameter or form field accounts for a pair named
    /// `name` — see [`accounts_for`].
    fn accounts_for(&self, name: &str, spec: &Spec) -> bool {
        if self.location != Location::Query {
            return false;
        }
        if self.style == Style::DeepObject {
            // Exactly what the decoder accepts, and nothing more: a
            // `filter[role` that never closes, or a bare `filter`,
            // contributes no value, so calling it described would let it
            // slip past strict checking while doing nothing.
            return self.deep_object_key(name).is_some();
        }
        if self.explode
            && self.style == Style::Form
            && let Some(schema) = self.schema
            && let Shape::Object(Some(properties)) = Shape::of(schema, spec)
        {
            // The name itself is never serialized in this form.
            return properties.contains_key(name);
        }
        name == self.name
    }

    /// This field's value, read out of the form body's pairs.
    fn read_form_field(
        &self,
        pairs: &[(String, String)],
        spec: &Spec,
    ) -> Result<Option<Value>, String> {
        let shape = self
            .schema
            .map_or(Shape::Opaque, |schema| Shape::of(schema, spec));
        self.read_query(pairs, &shape, spec)
    }

    fn of(parameter: &'p Parameter) -> Self {
        match parameter {
            Parameter::Path(path) => Self {
                name: &path.name,
                location: Location::Path,
                // The specification requires `required: true` here, and
                // a path parameter that did not arrive means the path
                // did not match at all — so this is always true.
                required: true,
                style: match path.style {
                    Some(InPathStyle::Matrix) => Style::Matrix,
                    Some(InPathStyle::Label) => Style::Label,
                    Some(InPathStyle::Simple) | None => Style::Simple,
                },
                explode: path.explode.unwrap_or(false),
                schema: path.schema.as_ref(),
                content: path.content.as_ref(),
            },
            Parameter::Querystring(querystring) => Self {
                name: &querystring.name,
                location: Location::Querystring,
                required: querystring.required.unwrap_or(false),
                // `in: querystring` is defined only through `content`;
                // `style` and `explode` have no meaning for it.
                style: Style::Simple,
                explode: false,
                schema: None,
                content: Some(&querystring.content),
            },
            Parameter::Query(query) => Self {
                name: &query.name,
                location: Location::Query,
                required: query.required.unwrap_or(false),
                style: match query.style {
                    Some(InQueryStyle::SpaceDelimited) => Style::SpaceDelimited,
                    Some(InQueryStyle::PipeDelimited) => Style::PipeDelimited,
                    Some(InQueryStyle::DeepObject) => Style::DeepObject,
                    Some(InQueryStyle::Form) | None => Style::Form,
                },
                // `form` explodes by default; every other style does not.
                explode: query
                    .explode
                    .unwrap_or(matches!(query.style, Some(InQueryStyle::Form) | None)),
                schema: query.schema.as_ref(),
                content: query.content.as_ref(),
            },
            Parameter::Header(header) => Self {
                name: &header.name,
                location: Location::Header,
                required: header.required.unwrap_or(false),
                style: match header.style {
                    Some(InHeaderStyle::Simple) | None => Style::Simple,
                },
                explode: header.explode.unwrap_or(false),
                schema: header.schema.as_ref(),
                content: header.content.as_ref(),
            },
            Parameter::Cookie(cookie) => Self {
                name: &cookie.name,
                location: Location::Cookie,
                required: cookie.required.unwrap_or(false),
                style: match cookie.style {
                    Some(InCookieStyle::Form) | None => Style::Form,
                },
                explode: cookie.explode.unwrap_or(true),
                schema: cookie.schema.as_ref(),
                content: cookie.content.as_ref(),
            },
        }
    }

    /// The parameter's text exactly as it arrived, for `content`
    /// parameters, which are not flattened by `style` at all.
    fn raw_text(&self, request: &RequestView<'_>, extracted: &Extracted<'_>) -> Option<String> {
        match self.location {
            Location::Path => extracted
                .path
                .get(self.name)
                .map(|raw| self.decode_value(raw)),
            Location::Query => extracted
                .query
                .iter()
                .find(|(name, _)| name == self.name)
                .map(|(_, value)| self.decode_value(value)),
            // The whole query string, as sent. Absent and empty are the
            // same thing for a query string, so an empty one still
            // counts as supplied.
            Location::Querystring => Some(request.query.as_deref().unwrap_or_default().to_owned()),
            Location::Header => request.header(self.name).map(str::to_owned),
            Location::Cookie => extracted
                .cookies
                .iter()
                .find(|(name, _)| name == self.name)
                .map(|(_, value)| value.clone()),
            Location::Body | Location::Description => None,
        }
    }

    /// The parameter's value, rebuilt from the request. `Ok(None)` means
    /// it was not sent at all.
    fn read(
        &self,
        request: &RequestView<'_>,
        extracted: &Extracted<'_>,
        shape: &Shape<'_>,
        spec: &Spec,
    ) -> Result<Option<Value>, String> {
        match self.location {
            Location::Path => {
                let Some(raw) = extracted.path.get(self.name) else {
                    return Ok(None);
                };
                self.read_single(self.undecorate(raw), shape, spec)
                    .map(Some)
            }
            Location::Query => self.read_query(&extracted.query, shape, spec),
            Location::Header => {
                let values: Vec<&str> = request.header_values(self.name).collect();
                if values.is_empty() {
                    return Ok(None);
                }
                // A header repeated is the same as a header whose value
                // is the values joined by commas (RFC 9110 §5.3).
                self.read_single(&values.join(","), shape, spec).map(Some)
            }
            Location::Cookie => {
                let found = extracted.cookies.iter().find(|(name, _)| name == self.name);
                let Some((_, raw)) = found else {
                    return Ok(None);
                };
                self.read_single(raw, shape, spec).map(Some)
            }
            Location::Querystring | Location::Body | Location::Description => Ok(None),
        }
    }

    /// Undo the transport encoding of one value, once `style` has
    /// finished splitting it.
    ///
    /// Which encoding that is depends on where the value came from: a
    /// path segment is percent-encoded, a query value is form-encoded
    /// (`+` is a space), and header and cookie values are neither.
    fn decode_value(&self, raw: &str) -> String {
        match self.location {
            Location::Path => decode_path_segment(raw),
            Location::Query | Location::Querystring => decode_form(raw),
            Location::Header | Location::Cookie | Location::Body | Location::Description => {
                raw.to_owned()
            }
        }
    }

    /// Strip the prefix `label` and `matrix` add: `.blue` and
    /// `;color=blue` both carry the value `blue`.
    fn undecorate<'v>(&self, raw: &'v str) -> &'v str {
        match self.style {
            Style::Label => raw.strip_prefix('.').unwrap_or(raw),
            Style::Matrix => {
                let raw = raw.strip_prefix(';').unwrap_or(raw);
                // Non-exploded: `;id=3,4`. Exploded: `;id=3;id=4`, whose
                // first `name=` is stripped here and the rest below.
                raw.strip_prefix(&format!("{}=", self.name)).unwrap_or(raw)
            }
            _ => raw,
        }
    }

    /// Rebuild from one string, splitting it as the style says and
    /// decoding only afterwards.
    fn read_single(&self, raw: &str, shape: &Shape<'_>, spec: &Spec) -> Result<Value, String> {
        let separator = self.separator();
        match shape {
            Shape::Array(items) => {
                let values = self
                    .split_list(raw, separator)
                    .iter()
                    .map(|part| coerce(&self.decode_value(part), *items, spec))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Value::Array(values))
            }
            Shape::Object(properties) => {
                let pairs = if self.explode {
                    // `role=admin,firstName=Alex`
                    self.split_list(raw, separator)
                        .iter()
                        .filter_map(|part| {
                            part.split_once('=').map(|(name, value)| {
                                (self.decode_value(name), self.decode_value(value))
                            })
                        })
                        .collect()
                } else {
                    // `role,admin,firstName,Alex`
                    let flat: Vec<String> = self
                        .split_list(raw, separator)
                        .iter()
                        .map(|part| self.decode_value(part))
                        .collect();
                    flat.chunks(2)
                        .filter(|chunk| chunk.len() == 2)
                        .map(|chunk| (chunk[0].clone(), chunk[1].clone()))
                        .collect()
                };
                object_from(pairs, *properties, spec)
            }
            Shape::Primitive(primitive) => coerce_primitive(&self.decode_value(raw), *primitive),
            Shape::Opaque => Ok(Value::String(self.decode_value(raw))),
        }
    }

    /// The character that separates the members of a flattened list.
    fn separator(&self) -> char {
        match self.style {
            Style::SpaceDelimited => ' ',
            Style::PipeDelimited => '|',
            // `label` separates with the same `.` it opens with, whether
            // or not it explodes: `.3.4.5`.
            Style::Label => '.',
            // Exploded `matrix` repeats the whole `;name=` per member.
            Style::Matrix if self.explode => ';',
            _ => ',',
        }
    }

    /// Split a flattened list, dropping the `name=` that exploded
    /// `matrix` repeats before every member.
    ///
    /// Splitting happens on the text as it arrived, so a delimiter that
    /// was percent-encoded stays data: in a non-exploded `form` array,
    /// `a%2Cb` is one item containing a comma, not two items.
    ///
    /// `spaceDelimited` is the exception, and has to be. A literal
    /// space cannot appear in a query string at all
    /// ([RFC 3986 §3.4](https://www.rfc-editor.org/rfc/rfc3986#section-3.4)),
    /// so `%20` and `+` are the only spellings its delimiter has — which
    /// leaves the style with no way to express a space *inside* a
    /// member, as the specification itself notes of these styles.
    fn split_list(&self, raw: &str, separator: char) -> Vec<String> {
        if raw.is_empty() {
            return Vec::new();
        }
        let normalized;
        let raw = if separator == ' ' {
            normalized = raw.replace("%20", " ").replace('+', " ");
            normalized.as_str()
        } else {
            raw
        };
        raw.split(separator)
            .map(|part| {
                if self.style == Style::Matrix && self.explode {
                    part.strip_prefix(&format!("{}=", self.name))
                        .unwrap_or(part)
                        .to_owned()
                } else {
                    part.to_owned()
                }
            })
            .collect()
    }

    /// Rebuild from the query string, where a value may be spread over
    /// several pairs rather than flattened into one.
    fn read_query(
        &self,
        pairs: &[(String, String)],
        shape: &Shape<'_>,
        spec: &Spec,
    ) -> Result<Option<Value>, String> {
        // `deepObject`: `id[role]=admin&id[firstName]=Alex`
        if self.style == Style::DeepObject {
            let members: Vec<(String, String)> = pairs
                .iter()
                .filter_map(|(name, value)| {
                    let key = self.deep_object_key(name)?;
                    Some((key.to_owned(), self.decode_value(value)))
                })
                .collect();
            if members.is_empty() {
                return Ok(None);
            }
            let properties = match shape {
                Shape::Object(properties) => *properties,
                _ => None,
            };
            return object_from(members, properties, spec).map(Some);
        }

        // An exploded object spreads its properties over top-level
        // pairs, so what identifies it is the property names.
        if self.explode
            && let Shape::Object(Some(properties)) = shape
        {
            let members: Vec<(String, String)> = pairs
                .iter()
                .filter(|(name, _)| properties.contains_key(name))
                .map(|(name, value)| (name.clone(), self.decode_value(value)))
                .collect();
            if members.is_empty() {
                return Ok(None);
            }
            return object_from(members, Some(*properties), spec).map(Some);
        }

        let mine: Vec<&String> = pairs
            .iter()
            .filter(|(name, _)| name == self.name)
            .map(|(_, value)| value)
            .collect();
        if mine.is_empty() {
            return Ok(None);
        }

        // An exploded array is one pair per member.
        if self.explode
            && let Shape::Array(items) = shape
        {
            let values = mine
                .into_iter()
                .map(|value| coerce(&self.decode_value(value), *items, spec))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Some(Value::Array(values)));
        }

        self.read_single(mine[0], shape, spec).map(Some)
    }
}

/// Build an object from name/value pairs, coercing each member through
/// the property schema that names it.
fn object_from(
    pairs: Vec<(String, String)>,
    properties: Option<&BTreeMap<String, RefOr<Schema>>>,
    spec: &Spec,
) -> Result<Value, String> {
    let mut object = serde_json::Map::new();
    for (name, value) in pairs {
        let property = properties.and_then(|properties| properties.get(&name));
        object.insert(name, coerce(&value, property, spec)?);
    }
    Ok(Value::Object(object))
}

/// Coerce one string through whatever schema describes it.
pub(crate) fn coerce(
    raw: &str,
    schema: Option<&RefOr<Schema>>,
    spec: &Spec,
) -> Result<Value, String> {
    match schema.map(|schema| Shape::of(schema, spec)) {
        Some(Shape::Primitive(primitive)) => coerce_primitive(raw, primitive),
        _ => Ok(Value::String(raw.to_owned())),
    }
}

/// Turn text back into the primitive the schema says it is.
fn coerce_primitive(raw: &str, primitive: Primitive) -> Result<Value, String> {
    match primitive {
        Primitive::String => Ok(Value::String(raw.to_owned())),
        Primitive::Integer => raw
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{raw:?} is not an integer")),
        Primitive::Number => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("{raw:?} is not a number")),
        // The specification's own encoding of a boolean, and only it —
        // accepting `1` or `yes` would be inventing a dialect.
        Primitive::Boolean => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("{raw:?} is not `true` or `false`")),
        },
        Primitive::Null => match raw {
            "" | "null" => Ok(Value::Null),
            _ => Err(format!("{raw:?} is not null")),
        },
    }
}

impl<'s> Shape<'s> {
    /// What kind of value a schema describes, as far as rebuilding a
    /// flattened parameter needs to know.
    fn of(schema: &'s RefOr<Schema>, spec: &'s Spec) -> Self {
        let Ok(resolved) = schema.get_item(spec) else {
            return Shape::Opaque;
        };
        match resolved {
            Schema::Single(single) => match single.as_ref() {
                SingleSchema::String(_) => Shape::Primitive(Primitive::String),
                SingleSchema::Integer(_) => Shape::Primitive(Primitive::Integer),
                SingleSchema::Number(_) => Shape::Primitive(Primitive::Number),
                SingleSchema::Boolean(_) => Shape::Primitive(Primitive::Boolean),
                SingleSchema::Null(_) => Shape::Primitive(Primitive::Null),
                SingleSchema::Array(array) => Shape::Array(match &array.items {
                    Some(BoolOr::Item(items)) => Some(items),
                    _ => None,
                }),
                SingleSchema::Object(object) => Shape::Object(object.properties.as_ref()),
            },
            // `type: [integer, "null"]` is an integer that may be
            // absent; coerce as the first type that is not null so the
            // text still becomes a number.
            Schema::Multi(multi) => multi
                .schema_types
                .iter()
                .find_map(|schema_type| match schema_type {
                    SchemaType::String => Some(Shape::Primitive(Primitive::String)),
                    SchemaType::Integer => Some(Shape::Primitive(Primitive::Integer)),
                    SchemaType::Number => Some(Shape::Primitive(Primitive::Number)),
                    SchemaType::Boolean => Some(Shape::Primitive(Primitive::Boolean)),
                    SchemaType::Array => Some(Shape::Array(None)),
                    SchemaType::Object => Some(Shape::Object(None)),
                    SchemaType::Null | SchemaType::Custom(_) => None,
                })
                .unwrap_or(Shape::Opaque),
            _ => Shape::Opaque,
        }
    }
}
