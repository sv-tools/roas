//! Protocol bindings, kept untyped.
//!
//! A Bindings Object maps a protocol name (`kafka`, `amqp`, `mqtt`, …)
//! to a protocol-specific object. AsyncAPI 3.0's schema types ~17
//! protocols across server / channel / operation / message, and each
//! protocol's binding carries its *own* `bindingVersion` that evolves
//! independently of the document version — kafka alone ships 0.3.0,
//! 0.4.0, and 0.5.0 shapes in the same schema.
//!
//! Modeling that cross-product in Rust would be the single largest part
//! of this crate and would go stale with every binding release, for
//! little validation value. So bindings are held as raw JSON keyed by
//! protocol: they round-trip losslessly, and typed accessors can be
//! layered on later behind a feature without a breaking change.

use crate::validation::{Context, ValidateWithContext};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A map of protocol name → protocol-specific binding object.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
#[serde(transparent)]
pub struct Bindings(pub BTreeMap<String, serde_json::Value>);

impl Bindings {
    /// The binding object for `protocol`, if present.
    #[must_use]
    pub fn get(&self, protocol: &str) -> Option<&serde_json::Value> {
        self.0.get(protocol)
    }

    /// The `bindingVersion` declared by `protocol`'s binding, if any.
    ///
    /// A binding that omits it means "latest" per the specification.
    #[must_use]
    pub fn binding_version(&self, protocol: &str) -> Option<&str> {
        self.get(protocol)?.get("bindingVersion")?.as_str()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl ValidateWithContext for Bindings {
    fn validate_with_context(&self, ctx: &mut Context) {
        for (protocol, value) in &self.0 {
            // The binding payload itself is protocol-defined and not
            // checked here, but it must be an object for any binding to
            // make sense — a bare string or array is a modeling error.
            if !value.is_object() {
                ctx.error_field(protocol, "binding must be an object");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumset::EnumSet;
    use serde_json::json;

    #[test]
    fn round_trips_transparently() {
        let value = json!({
            "kafka": { "topic": "my-topic", "bindingVersion": "0.5.0" },
            "ws": { "method": "GET" }
        });
        let bindings: Bindings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&bindings).unwrap(), value);
    }

    #[test]
    fn accessors_read_protocol_and_binding_version() {
        let bindings: Bindings = serde_json::from_value(json!({
            "kafka": { "topic": "t", "bindingVersion": "0.5.0" },
            "mqtt": { "qos": 1 }
        }))
        .unwrap();

        assert_eq!(bindings.binding_version("kafka"), Some("0.5.0"));
        // Absent `bindingVersion` means "latest", not an error.
        assert_eq!(bindings.binding_version("mqtt"), None);
        assert_eq!(bindings.binding_version("amqp"), None);
        assert!(bindings.get("kafka").is_some());
        assert!(bindings.get("amqp").is_none());
        assert!(!bindings.is_empty());
        assert!(Bindings::default().is_empty());
    }

    #[test]
    fn validate_rejects_non_object_binding() {
        let bindings: Bindings =
            serde_json::from_value(json!({ "kafka": { "topic": "t" }, "ws": "nope" })).unwrap();
        let mut ctx = Context::with_path(EnumSet::empty(), "#.channels.user.bindings");
        bindings.validate_with_context(&mut ctx);
        assert_eq!(ctx.errors.len(), 1);
        assert!(ctx.errors[0] == "#.channels.user.bindings.ws: binding must be an object");
    }
}
