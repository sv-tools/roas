//! Finding the channel an AsyncAPI step names.
//!
//! A step names an AsyncAPI operation by `operationId`, or a channel by
//! `channelPath` (`$sourceDescriptions.<name>#/channels/<key>`, or
//! `{$sourceDescriptions.<name>.url}#/channels/<key>`). AsyncAPI 3.x keeps
//! operations under `operations`, each pointing at a channel; 2.6 keeps
//! them inside the channel, under `publish` and `subscribe`, with the
//! channel's key as its address. Only references within the same
//! document are followed.
//!
//! The step's own `action` decides whether a message is sent or listened
//! for: an AsyncAPI operation's `action` (3.x) or `publish`/`subscribe`
//! (2.6) describes the application the document is about, and whether a
//! workflow plays that application or talks to it is not something
//! Arazzo says.

use crate::message::Channel;
use crate::operation::{OperationError, Source};
use crate::operation_document::{decode_pointer, source_name};
use roas_arazzo::v1_1::Step;
use serde_json::Value;
use std::collections::BTreeMap;

/// A step's channel, with what sending and receiving on it needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Resolved {
    /// The channel, its address still a template.
    pub channel: Channel,
    /// Where each message the channel may carry keeps its correlation id:
    /// AsyncAPI runtime expressions such as `$message.header#/id`.
    pub correlation: Vec<String>,
    /// The media type a payload is written in.
    pub content_type: String,
}

/// How many `$ref`s are followed before giving up on a chain.
const MAX_HOPS: usize = 32;

pub(crate) fn resolve(
    step: &Step,
    sources: &BTreeMap<String, Source>,
    missing: &[String],
) -> Result<Resolved, OperationError> {
    if let Some(path) = &step.channel_path {
        let bad = |reason: &str| OperationError::BadPath {
            path: path.clone(),
            reason: reason.to_owned(),
        };
        let (document, fragment) = path
            .split_once('#')
            .ok_or_else(|| bad("it has no `#` and so names no channel inside the document"))?;
        let name = source_name(document)
            .or_else(|| document.trim().strip_prefix("$sourceDescriptions."))
            .ok_or_else(|| bad("it does not start with `$sourceDescriptions.<name>`"))?;
        let source = sources
            .get(name)
            .ok_or_else(|| OperationError::MissingSource(name.to_owned()))?;
        let document = asyncapi(name, source.document())?;
        let pointer = decode_pointer(fragment).map_err(|reason| bad(&reason))?;
        let channel = follow(document, document.pointer(&pointer), &pointer)
            .map_err(|reason| bad(&reason))?;
        let key = pointer.rsplit('/').next().unwrap_or_default();
        let key = key.replace("~1", "/").replace("~0", "~");
        let messages = if is_v2(document) {
            ["publish", "subscribe"]
                .iter()
                .filter_map(|which| channel.get(*which))
                .filter_map(|operation| operation.get("message"))
                .collect()
        } else {
            channel_messages(channel)
        };
        return finish(name, None, document, channel, &key, &messages);
    }
    let Some(id) = &step.operation_id else {
        return Err(OperationError::Nothing(step.step_id.clone()));
    };
    let (name, operation) = match id
        .strip_prefix("$sourceDescriptions.")
        .and_then(|rest| rest.split_once('.'))
    {
        Some((name, operation)) => (name.to_owned(), operation),
        None => {
            let hits: Vec<&String> = sources
                .iter()
                .filter(|(_, source)| source.document().get("asyncapi").is_some())
                .filter(|(_, source)| find(source.document(), id).is_some())
                .map(|(name, _)| name)
                .collect();
            if hits.len() > 1 {
                return Err(OperationError::Ambiguous {
                    operation: id.clone(),
                    sources: hits
                        .iter()
                        .map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                });
            }
            if !missing.is_empty() {
                return Err(OperationError::Unproven {
                    operation: id.clone(),
                    missing: missing.join(", "),
                });
            }
            let name = hits.first().ok_or_else(|| OperationError::Unknown {
                operation: id.clone(),
            })?;
            ((*name).clone(), id.as_str())
        }
    };
    let source = sources
        .get(&name)
        .ok_or_else(|| OperationError::MissingSource(name.clone()))?;
    let document = asyncapi(&name, source.document())?;
    let found = find(document, operation).ok_or_else(|| OperationError::Unknown {
        operation: id.clone(),
    })?;
    let bad = |reason: String| OperationError::BadPath {
        path: id.clone(),
        reason,
    };
    match found {
        Found::V2 {
            key,
            channel,
            messages,
        } => finish(&name, Some(operation), document, channel, &key, &messages),
        Found::V3 { operation: object } => {
            let reference = object
                .get("channel")
                .and_then(|channel| channel.get("$ref"))
                .and_then(Value::as_str)
                .ok_or_else(|| bad("the operation names no channel".into()))?;
            let pointer = local(reference).map_err(bad)?;
            let channel = follow(document, document.pointer(&pointer), &pointer).map_err(bad)?;
            let key = pointer.rsplit('/').next().unwrap_or_default();
            let key = key.replace("~1", "/").replace("~0", "~");
            let messages = match object.get("messages").and_then(Value::as_array) {
                Some(messages) if !messages.is_empty() => messages.iter().collect(),
                _ => channel_messages(channel),
            };
            finish(&name, Some(operation), document, channel, &key, &messages)
        }
    }
}

fn is_v2(document: &Value) -> bool {
    document
        .get("asyncapi")
        .and_then(Value::as_str)
        .is_some_and(|version| version.starts_with("2."))
}

fn asyncapi<'v>(name: &str, document: &'v Value) -> Result<&'v Value, OperationError> {
    match document.get("asyncapi").and_then(Value::as_str) {
        Some(version) if version.starts_with("2.") || version.starts_with("3.") => Ok(document),
        Some(version) => Err(OperationError::Document {
            document: name.to_owned(),
            reason: format!("AsyncAPI {version} is not supported; 2.x and 3.x are"),
        }),
        None => Err(OperationError::Document {
            document: name.to_owned(),
            reason: "an AsyncAPI step needs an AsyncAPI source description".into(),
        }),
    }
}

enum Found<'v> {
    V2 {
        key: String,
        channel: &'v Value,
        messages: Vec<&'v Value>,
    },
    V3 {
        operation: &'v Value,
    },
}

/// The operation `id` of an AsyncAPI document.
fn find<'v>(document: &'v Value, id: &str) -> Option<Found<'v>> {
    if is_v2(document) {
        for (key, channel) in document.get("channels")?.as_object()? {
            let Ok(channel) = follow(document, Some(channel), key) else {
                continue;
            };
            for which in ["publish", "subscribe"] {
                if let Some(operation) = channel.get(which)
                    && operation.get("operationId").and_then(Value::as_str) == Some(id)
                {
                    return Some(Found::V2 {
                        key: key.clone(),
                        channel,
                        messages: operation.get("message").into_iter().collect(),
                    });
                }
            }
        }
        None
    } else {
        let operation = document.get("operations")?.get(id)?;
        Some(Found::V3 {
            operation: follow(document, Some(operation), id).ok()?,
        })
    }
}

fn channel_messages(channel: &Value) -> Vec<&Value> {
    channel
        .get("messages")
        .and_then(Value::as_object)
        .map(|messages| messages.values().collect())
        .unwrap_or_default()
}

/// A local reference's JSON Pointer.
fn local(reference: &str) -> Result<String, String> {
    let fragment = reference
        .strip_prefix('#')
        .ok_or_else(|| format!("`{reference}` is not a reference within the document"))?;
    decode_pointer(fragment)
}

/// The object at `value`, following `$ref`s within the document.
fn follow<'v>(
    document: &'v Value,
    mut value: Option<&'v Value>,
    pointer: &str,
) -> Result<&'v Value, String> {
    for _ in 0..MAX_HOPS {
        let found = value.ok_or_else(|| format!("the document has nothing at `{pointer}`"))?;
        match found.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let next = local(reference)?;
                value = document.pointer(&next);
            }
            None => return Ok(found),
        }
    }
    Err(format!(
        "`{pointer}` is a chain of more than {MAX_HOPS} references"
    ))
}

fn finish(
    name: &str,
    operation: Option<&str>,
    document: &Value,
    channel: &Value,
    key: &str,
    messages: &[&Value],
) -> Result<Resolved, OperationError> {
    let address = if is_v2(document) {
        key.to_owned()
    } else {
        // "If the field is null or absent, it MUST be interpreted as
        // unknown": there is nowhere to send to or listen on.
        channel
            .get("address")
            .and_then(Value::as_str)
            .ok_or_else(|| OperationError::BadPath {
                path: key.to_owned(),
                reason: "the channel's address is unknown (null or absent)".into(),
            })?
            .to_owned()
    };
    let all = document
        .get("servers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let servers = match channel.get("servers").and_then(Value::as_array) {
        Some(named) if !named.is_empty() => named
            .iter()
            .filter_map(|server| {
                // 3.x references a server; 2.6 names one.
                let name = match server.get("$ref").and_then(Value::as_str) {
                    Some(reference) => reference
                        .rsplit('/')
                        .next()?
                        .replace("~1", "/")
                        .replace("~0", "~"),
                    None => server.as_str()?.to_owned(),
                };
                let object = follow(document, all.get(&name), &name).ok()?.clone();
                Some((name, object))
            })
            .collect(),
        _ => all
            .iter()
            .filter_map(|(name, server)| {
                Some((
                    name.clone(),
                    follow(document, Some(server), name).ok()?.clone(),
                ))
            })
            .collect(),
    };
    let bindings = channel
        .get("bindings")
        .map(|bindings| follow(document, Some(bindings), "bindings").cloned())
        .transpose()
        .map_err(|reason| OperationError::BadPath {
            path: key.to_owned(),
            reason,
        })?;
    let mut correlation = Vec::new();
    let mut content_type = None;
    for message in messages
        .iter()
        .flat_map(|message| alternatives(document, message))
    {
        // A field the message does not set may come from one of its
        // traits; the message's own wins.
        let field = |name: &str| {
            message.get(name).or_else(|| {
                message
                    .get("traits")
                    .and_then(Value::as_array)?
                    .iter()
                    .filter_map(|item| follow(document, Some(item), "traits").ok())
                    .find_map(|item| item.get(name))
            })
        };
        if content_type.is_none() {
            content_type = field("contentType")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if let Some(id) = field("correlationId")
            && let Ok(id) = follow(document, Some(id), "correlationId")
            && let Some(location) = id.get("location").and_then(Value::as_str)
            && !correlation.iter().any(|known| known == location)
        {
            correlation.push(location.to_owned());
        }
    }
    let content_type = content_type
        .or_else(|| {
            document
                .get("defaultContentType")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "application/json".to_owned());
    Ok(Resolved {
        channel: Channel {
            source: name.to_owned(),
            operation: operation.map(str::to_owned),
            address,
            servers,
            bindings,
        },
        correlation,
        content_type,
    })
}

/// A message object, or each one of a 2.6 `oneOf`, references followed.
fn alternatives<'v>(document: &'v Value, message: &'v Value) -> Vec<&'v Value> {
    let Ok(message) = follow(document, Some(message), "message") else {
        return Vec::new();
    };
    match message.get("oneOf").and_then(Value::as_array) {
        Some(each) => each
            .iter()
            .filter_map(|message| follow(document, Some(message), "message").ok())
            .collect(),
        None => vec![message],
    }
}

/// Fill a channel address's `{parameters}`.
pub(crate) fn address(template: &str, values: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..]
            .find('}')
            .ok_or_else(|| format!("`{template}` has an unclosed `{{`"))?;
        let name = &rest[open + 1..open + close];
        let value = values
            .get(name)
            .ok_or_else(|| format!("no `channel` parameter gives `{{{name}}}` of `{template}`"))?;
        out.push_str(value);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
