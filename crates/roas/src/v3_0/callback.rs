use crate::v3_0::path_item::PathItem;
use crate::v3_0::spec::Spec;
use crate::validation::{Context, ValidateWithContext};
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// A map of possible out-of band callbacks related to the parent operation.
/// Each value in the map is a Path Item Object that describes a set of requests
/// that may be initiated by the API provider and the expected responses.
/// The key value used to identify the path item object is an expression, evaluated at runtime,
/// that identifies a URL to use for the callback operation.
///
/// Specification example:
///
/// ```yaml
/// onData:
///   # when data is sent, it will be sent to the `callbackUrl` provided
///   # when making the subscription PLUS the suffix `/data`
///   '{$request.query.callbackUrl}/data':
///     post:
///       requestBody:
///         description: subscription payload
///         content:
///           application/json:
///             schema:
///               type: object
///               properties:
///                 timestamp:
///                   type: string
///                   format: date-time
///                 userData:
///                   type: string
///       responses:
///         '202':
///           description: |
///             Your server implementation should return this HTTP status code
///             if the data was received successfully
///         '204':
///           description: |
///             Your server should return this HTTP status code if no longer interested
///             in further updates
/// ```
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Callback {
    /// A Path Item Object used to define a callback request and expected responses.
    pub paths: BTreeMap<String, PathItem>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Serialize for Callback {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut len = self.paths.len();
        if let Some(ext) = &self.extensions {
            len += ext.len();
        }
        let mut map = serializer.serialize_map(Some(len))?;

        for (k, v) in &self.paths {
            map.serialize_entry(&k, &v)?;
        }

        if let Some(ext) = &self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(&k, &v)?;
                }
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for Callback {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] = &["<path name>", "x-<ext name>"];

        struct CallbackVisitor;

        impl<'de> Visitor<'de> for CallbackVisitor {
            type Value = Callback;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct Callback")
            }

            fn visit_map<V>(self, mut map: V) -> Result<Callback, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut res = Callback {
                    paths: BTreeMap::new(),
                    ..Default::default()
                };
                let mut extensions: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("x-") {
                        if extensions.contains_key(key.as_str()) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        if res.paths.contains_key(key.as_str()) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        res.paths.insert(key, map.next_value()?);
                    }
                }
                if !extensions.is_empty() {
                    res.extensions = Some(extensions);
                }
                Ok(res)
            }
        }

        deserializer.deserialize_struct("Callback", FIELDS, CallbackVisitor)
    }
}

impl ValidateWithContext<Spec> for Callback {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        for (name, path_item) in &self.paths {
            path_item.validate_with_context(ctx, format!("{path}[{name}]"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::Context;
    use crate::validation::Options;
    use serde_json::json;

    #[test]
    fn round_trip_paths_and_extensions() {
        let v = json!({
            "{$request.body#/callbackUrl}": {
                "post": {"responses": {"200": {"description": "ok"}}}
            },
            "x-internal": "yes"
        });
        let cb: Callback = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(cb.paths.len(), 1);
        assert!(cb.extensions.is_some());
        assert_eq!(serde_json::to_value(&cb).unwrap(), v);
    }

    #[test]
    fn duplicate_path_key_errors() {
        // Multiple identical keys in JSON are last-wins on parsing, so
        // construct via repeated keys explicitly through a string parse.
        let raw = r#"{"a": {}, "a": {}}"#;
        let res: Result<Callback, _> = serde_json::from_str(raw);
        assert!(
            res.is_err(),
            "expected duplicate-field error, got: {:?}",
            res.ok()
        );
    }

    #[test]
    fn duplicate_extension_key_errors() {
        let raw = r#"{"x-foo": 1, "x-foo": 2}"#;
        let res: Result<Callback, _> = serde_json::from_str(raw);
        assert!(res.is_err(), "expected duplicate extension error");
    }

    #[test]
    fn validate_walks_path_items() {
        // PathItem with operation that has empty responses → triggers the
        // "must declare at least one response" error in the new validator.
        let cb: Callback = serde_json::from_value(json!({
            "{$request.body#/cb}": {
                "post": {"responses": {}}
            }
        }))
        .unwrap();
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        cb.validate_with_context(&mut ctx, "cb".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("must declare at least one response")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn callback_nested_operation_security_is_validated() {
        // An operation inside a Callback path item must still have its
        // `security` requirements checked. `validate_path_item` is only
        // called for `Spec.paths`, so callback-nested operations rely on
        // `Operation::validate_with_context` to invoke
        // `validate_security_requirements` directly.
        let cb: Callback = serde_json::from_value(json!({
            "{$request.body#/url}": {
                "post": {
                    "responses": {"200": {"description": "ok"}},
                    "security": [{"missing-scheme": []}]
                }
            }
        }))
        .unwrap();
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, Options::new());
        cb.validate_with_context(&mut ctx, "cb".into());
        // With no Components on the Spec, the validator surfaces a
        // "no `components.securitySchemes`" error — proof the operation
        // inside the callback ran security validation.
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("post.security") && e.contains("missing-scheme")),
            "expected security validation inside callback: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn empty_callback_round_trips() {
        let cb: Callback = serde_json::from_value(json!({})).unwrap();
        assert!(cb.paths.is_empty());
        assert!(cb.extensions.is_none());
        assert_eq!(serde_json::to_value(&cb).unwrap(), json!({}));
    }

    /// Serialize a Callback with an x- extension — exercises line 76
    /// (`map.serialize_entry` inside the extensions for-loop).
    #[test]
    fn callback_with_extension_serializes_extension_key() {
        let mut ext = std::collections::BTreeMap::new();
        ext.insert("x-custom".to_owned(), serde_json::json!("val"));
        let cb = Callback {
            paths: std::collections::BTreeMap::new(),
            extensions: Some(ext),
        };
        let v = serde_json::to_value(&cb).unwrap();
        assert_eq!(v["x-custom"], "val");
    }

    /// Passing a non-map value to Callback deserializer triggers `expecting`
    /// (lines 96-98 in `CallbackVisitor::expecting`).
    #[test]
    fn callback_non_map_value_errors() {
        let res: Result<Callback, _> = serde_json::from_str(r#""not a map""#);
        assert!(res.is_err(), "expected wrong-type error");
    }

    /// Serialize a Callback whose `extensions` map contains a key that does
    /// NOT start with `x-`.  The serializer silently skips such keys, which
    /// exercises the false-branch of `if k.starts_with("x-")` (line 76).
    #[test]
    fn callback_extension_without_x_prefix_is_skipped_in_serialization() {
        let mut ext = std::collections::BTreeMap::new();
        ext.insert("no-prefix".to_owned(), serde_json::json!(42));
        let cb = Callback {
            paths: std::collections::BTreeMap::new(),
            extensions: Some(ext),
        };
        let v = serde_json::to_value(&cb).unwrap();
        // The non-x- key must not appear in the output.
        assert!(v.get("no-prefix").is_none(), "unexpected key in: {v}");
    }
}
