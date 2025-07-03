use std::collections::BTreeMap;
use std::fmt;

use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::common::helpers::{Context, ValidateWithContext};
use crate::v3_0::path_item::PathItem;
use crate::v3_0::spec::Spec;

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
