//! The reference-resolution API as a dependent crate sees it.
//!
//! These pin visibility and paths rather than outcomes: every import
//! below has to stay public, and the trait has to work as a trait
//! object. What each reference resolves *to* is the unit tests' job.

use roas_asyncapi::common::Resolution;
use roas_asyncapi::common::reference::{RefOr, Reference, ResolveReference};

/// A document of one's own: the trait is implementable from outside.
struct Fallback(u8);

impl ResolveReference<u8> for Fallback {
    fn resolve_reference<'a>(&'a self, entry: &'a RefOr<u8>) -> Resolution<'a, u8> {
        match entry {
            RefOr::Item(item) => Resolution::Found(item),
            RefOr::Reference(_) => Resolution::Found(&self.0),
        }
    }
}

#[test]
fn the_common_paths_are_public() {
    let doc = Fallback(7);
    let entry: RefOr<u8> = RefOr::Reference(Reference {
        reference: "#/anything".to_owned(),
    });
    let via_trait_object: &dyn ResolveReference<u8> = &doc;
    // The default `resolved_reference` reports the entry's own reference.
    assert_eq!(entry.resolved_ref(&doc), Some("#/anything"));
    assert_eq!(entry.resolved_ref(via_trait_object), Some("#/anything"));
    assert_eq!(RefOr::Item(1u8).resolved_ref(&doc), None);
    for resolution in [
        entry.get_item(&doc),
        entry.get_item(via_trait_object),
        via_trait_object.resolve_reference(&entry),
    ] {
        match resolution {
            Resolution::Found(item) => assert_eq!(*item, 7),
            other => panic!("{:?}", other.problem()),
        }
    }
}

/// The same checks against a v3 module, which differ only in name.
#[cfg(any(feature = "v3_0", feature = "v3_1"))]
macro_rules! v3_paths_are_public {
    ($version:ident, $number:literal) => {
        mod $version {
            use roas_asyncapi::$version::{
                Channel, Document, Operation, RefOr, Resolution, ResolveReference,
            };

            #[test]
            fn the_version_paths_are_public() {
                let doc: Document = serde_json::from_value(serde_json::json!({
                    "asyncapi": $number,
                    "info": { "title": "T", "version": "1" },
                    "channels": { "real": { "address": "a" } },
                    "operations": {
                        "send": { "action": "send", "channel": { "$ref": "#/channels/real" } },
                        "alias": { "$ref": "#/operations/send" }
                    }
                }))
                .expect("parses");

                let entry: &RefOr<Operation> = &doc.operations["alias"];
                let via_trait_object: &dyn ResolveReference<Operation> = &doc;
                let operation = match entry.get_item(via_trait_object) {
                    Resolution::Found(operation) => operation,
                    other => panic!("{:?}", other.problem()),
                };
                assert!(entry.get_item(&doc).found().is_some());

                let channel: Resolution<'_, Channel> = doc.resolve_channel(&operation.channel);
                match channel {
                    Resolution::Found(channel) => assert!(channel.address.is_some()),
                    other => panic!("{:?}", other.problem()),
                }
            }
        }
    };
}

#[cfg(feature = "v3_0")]
v3_paths_are_public!(v3_0, "3.0.0");

#[cfg(feature = "v3_1")]
v3_paths_are_public!(v3_1, "3.1.0");

#[cfg(feature = "v2_6")]
mod v2_6 {
    use roas_asyncapi::v2_6::{ChannelItem, Document, RefOr, Resolution, ResolveReference, Server};

    #[test]
    fn the_version_paths_are_public() {
        let doc: Document = serde_json::from_value(serde_json::json!({
            "asyncapi": "2.6.0",
            "info": { "title": "T", "version": "1" },
            "servers": {
                "real": { "url": "kafka://broker", "protocol": "kafka" },
                "alias": { "$ref": "#/servers/real" }
            },
            "channels": {
                "real": { "description": "declared" },
                "alias": { "$ref": "#/channels/real" }
            }
        }))
        .expect("parses");

        let entry: &RefOr<Server> = &doc.servers["alias"];
        let via_trait_object: &dyn ResolveReference<Server> = &doc;
        match entry.get_item(via_trait_object) {
            Resolution::Found(server) => assert_eq!(server.protocol, "kafka"),
            other => panic!("{:?}", other.problem()),
        }
        assert!(entry.get_item(&doc).found().is_some());

        let channel: Resolution<'_, ChannelItem> = doc.resolve_channel(&doc.channels["alias"]);
        match channel {
            Resolution::Found(channel) => assert!(channel.description.is_some()),
            other => panic!("{:?}", other.problem()),
        }
    }
}
