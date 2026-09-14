//! URI and object context shared by operation indexing and reference loading.

use crate::operation::OperationError;
use serde_json::Value;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Version {
    Legacy,
    Swagger,
    V3_0,
    V3_1,
    V3_2,
}

#[derive(Clone)]
pub(crate) struct Document<'a> {
    pub(crate) value: &'a Value,
    pub(crate) retrieval: Option<Url>,
    pub(crate) base: Option<Url>,
    pub(crate) label: String,
    pub(crate) version: Version,
}

impl<'a> Document<'a> {
    pub(crate) fn new(
        value: &'a Value,
        retrieval: Option<Url>,
        label: String,
        inherited: Version,
    ) -> Result<Self, OperationError> {
        let bad = |reason: &str| OperationError::Document {
            document: label.clone(),
            reason: reason.into(),
        };
        let version = if value.get("asyncapi").is_some() || value.get("arazzo").is_some() {
            return Err(bad("this is not an OpenAPI operation source"));
        } else if let Some(version) = value.get("swagger") {
            if version != "2.0" || value.get("openapi").is_some() {
                return Err(bad("unsupported or conflicting OpenAPI version"));
            }
            Version::Swagger
        } else if let Some(version) = value.get("openapi") {
            let parts = version
                .as_str()
                .unwrap_or_default()
                .split('.')
                .collect::<Vec<_>>();
            if parts.len() != 3
                || parts[0] != "3"
                || parts[2].is_empty()
                || !parts[2].bytes().all(|b| b.is_ascii_digit())
            {
                return Err(bad("expected an OpenAPI 3.0.x, 3.1.x or 3.2.x version"));
            }
            match parts[1] {
                "0" => Version::V3_0,
                "1" => Version::V3_1,
                "2" => Version::V3_2,
                _ => return Err(bad("unsupported OpenAPI version")),
            }
        } else {
            inherited
        };
        let mut base = retrieval.clone();
        // $self was introduced for OpenAPI documents in 3.2. It is not a
        // server base, and an arbitrary fragment container does not acquire it.
        if version == Version::V3_2
            && value.get("openapi").is_some()
            && let Some(identity) = value.get("$self")
        {
            let identity = identity
                .as_str()
                .ok_or_else(|| bad("$self must be a URI string"))?;
            let resolved = match &retrieval {
                Some(uri) => uri.join(identity),
                None => Url::parse(identity),
            }
            .map_err(|_| bad("$self requires an absolute URI or a usable retrieval base"))?;
            if resolved.fragment().is_some() {
                return Err(bad("$self must not contain a fragment"));
            }
            base = Some(resolved);
        }
        Ok(Self {
            value,
            retrieval,
            base,
            label,
            version,
        })
    }

    pub(crate) fn key(&self) -> &str {
        self.base.as_ref().map_or(&self.label, Url::as_str)
    }

    pub(crate) fn error(
        &self,
        pointer: &str,
        reference: &str,
        reason: impl Into<String>,
    ) -> OperationError {
        OperationError::Reference {
            document: self.key().into(),
            pointer: pointer.into(),
            reference: reference.into(),
            reason: reason.into(),
        }
    }

    pub(crate) fn reference(
        &self,
        reference: &str,
        at: &str,
    ) -> Result<(Option<Url>, String), OperationError> {
        let (resource, fragment) = reference.split_once('#').unwrap_or((reference, ""));
        let pointer =
            decode_pointer(fragment).map_err(|reason| self.error(at, reference, reason))?;
        if resource.is_empty() {
            return Ok((self.base.clone(), pointer));
        }
        let mut target = match &self.base {
            Some(base) => base.join(resource),
            None => Url::parse(resource),
        }
        .map_err(|error| self.error(at, reference, format!("no usable reference base: {error}")))?;
        target.set_fragment(None);
        Ok((Some(target), pointer))
    }
}

/// Decode an RFC 6901 URI-fragment representation. A percent sign must start
/// a complete byte escape; decoded bytes must be UTF-8; ~ only escapes 0 or 1.
pub(crate) fn decode_pointer(fragment: &str) -> Result<String, String> {
    let bytes = fragment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = |b: u8| (b as char).to_digit(16).map(|n| n as u8);
            let pair = bytes.get(i + 1..i + 3).ok_or("incomplete percent escape")?;
            decoded.push(
                hex(pair[0])
                    .zip(hex(pair[1]))
                    .map(|(a, b)| a * 16 + b)
                    .ok_or("invalid percent escape")?,
            );
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let pointer = String::from_utf8(decoded).map_err(|_| "fragment is not UTF-8")?;
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err("fragment must be a JSON Pointer, not an anchor".into());
    }
    for token in pointer.split('/').skip(1) {
        let mut chars = token.chars();
        while let Some(c) = chars.next() {
            if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
                return Err("invalid JSON Pointer escape (expected ~0 or ~1)".into());
            }
        }
    }
    Ok(pointer)
}

pub(crate) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn source_name(document: &str) -> Option<&str> {
    document
        .trim()
        .strip_prefix("{$sourceDescriptions.")?
        .strip_suffix(".url}")
}

pub(crate) fn equivalent(left: &str, right: &str, base: Option<&Url>) -> bool {
    if left == right {
        return true;
    }
    let resolve = |value| match base {
        Some(base) => base.join(value),
        None => Url::parse(value),
    };
    match (resolve(left), resolve(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
