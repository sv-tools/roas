use crate::v3_2::path_item::PathItem;
use crate::v3_2::spec::Spec;
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
    /// A `Path Item` describing the callback request and its expected
    /// responses. The reference form is modelled as a `PathItem` whose
    /// `reference` field is set — bare `PathItem` is used here so adjacent
    /// fields (`summary`, `description`) are preserved instead of dropped
    /// by Reference Object semantics.
    pub paths: BTreeMap<String, PathItem>,

    /// This object MAY be extended with Specification Extensions.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Callback {
    /// Per-expression-key merge: for every `(expr, PathItem)` in `other`,
    /// if `self` already has that expression key the two `PathItem`s are
    /// merged in place via [`PathItem::merge`]; otherwise the incoming
    /// entry is inserted. Specification extensions on the Callback Object
    /// itself are merged per-key.
    pub fn merge(&mut self, other: Callback) {
        for (key, item) in other.paths {
            match self.paths.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut e) => e.get_mut().merge(item),
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(item);
                }
            }
        }
        crate::common::merge::merge_optional_map(&mut self.extensions, other.extensions);
    }
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
        // Round-trip via reparse to be order-tolerant.
        let back = serde_json::to_value(&cb).unwrap();
        let re: Callback = serde_json::from_value(back).unwrap();
        assert_eq!(re, cb);
    }

    #[test]
    fn callback_path_value_can_be_ref() {
        // OAS 3.1 allows the callback path-item slot to be a Reference,
        // but the Reference form is captured as `PathItem.reference` set
        // (with sibling fields preserved) — there is no separate Ref form.
        let v = json!({
            "{$request.body#/callbackUrl}": {"$ref": "#/components/pathItems/Hook"}
        });
        let cb: Callback = serde_json::from_value(v).unwrap();
        let entry = cb.paths.get("{$request.body#/callbackUrl}").expect("entry");
        assert_eq!(
            entry.reference.as_deref(),
            Some("#/components/pathItems/Hook"),
        );
    }

    #[test]
    fn callback_validate_walks_path_items() {
        // PathItem with operation that has empty responses → triggers
        // "must declare at least one response".
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
}
