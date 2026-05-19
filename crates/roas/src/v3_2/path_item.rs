//! Path Items

use crate::common::reference::RefOr;
use crate::v3_2::operation::Operation;
use crate::v3_2::parameter::Parameter;
use crate::v3_2::server::Server;
use crate::v3_2::spec::Spec;
use crate::validation::Options;
use crate::validation::{Context, PushError, ValidateWithContext};
use serde::de::{Error, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// Describes the operations available on a single path.
/// A Path Item may be empty, due to [ACL constraints](https://spec.openapis.org/oas/v3.0.3#securityFiltering).
/// The path itself is still exposed to the documentation viewer
/// but they will not know which operations and parameters are available.
///
/// Specification example:
///
/// ```yaml
/// get:
///   description: Returns pets based on ID
///   summary: Find pets by ID
///   operationId: getPetsById
///   responses:
///     '200':
///       description: pet response
///       content:
///         '*/*' :
///           schema:
///             type: array
///             items:
///               $ref: '#/components/schemas/Pet'
///     default:
///       description: error payload
///       content:
///         'text/html':
///           schema:
///             $ref: '#/components/schemas/ErrorModel'
/// parameters:
/// - name: id
///   in: path
///   description: ID of pet to use
///   required: true
///   schema:
///     type: array
///     items:
///       type: string  
///   style: simple
/// ```
#[derive(Clone, Debug, PartialEq, Default)]
pub struct PathItem {
    /// Allows for a referenced definition of this path item. Per OAS 3.1
    /// this points to another path item; in 3.1 the entry MAY also carry
    /// `summary` and `description`. Adjacent operation fields' behavior
    /// when a `$ref` is present is implementation-defined.
    pub reference: Option<String>,

    /// An optional, string summary intended to apply to all operations in
    /// this path. Added in OAS 3.1.
    pub summary: Option<String>,

    /// An optional, CommonMark description intended to apply to all
    /// operations in this path.
    pub description: Option<String>,

    /// Operations on this path, keyed by lowercase HTTP method name.
    /// OAS 3.2.0 defines exactly these nine standard methods: `get`, `put`,
    /// `post`, `delete`, `options`, `head`, `patch`, `trace`, `query`.
    /// Use `additional_operations` for non-standard methods.
    pub operations: Option<BTreeMap<String, Operation>>,

    /// Additional operations on this path keyed by HTTP method name in the
    /// exact capitalization sent in the request (per OAS 3.2.0). Standard
    /// methods (those handled by `operations`) MUST NOT appear here.
    pub additional_operations: Option<BTreeMap<String, Operation>>,

    /// An alternative server array to service all operations in this path.
    pub servers: Option<Vec<Server>>,

    /// A list of parameters that are applicable for all the operations described under this path.
    /// These parameters can be overridden at the operation level, but cannot be removed there.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the [Reference Object](crate::common::reference::Ref) to link to parameters
    /// defined under
    /// [`Components.parameters`](crate::v3_2::components::Components::parameters).
    pub parameters: Option<Vec<RefOr<Parameter>>>,

    /// Allows extensions to the Swagger Schema.
    /// The field name MUST begin with `x-`, for example, `x-internal-id`.
    /// The value can be null, a primitive, an array or an object.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Serialize for PathItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        if let Some(r) = &self.reference {
            map.serialize_entry("$ref", r)?;
        }
        if let Some(s) = &self.summary {
            map.serialize_entry("summary", s)?;
        }
        if let Some(d) = &self.description {
            map.serialize_entry("description", d)?;
        }

        if let Some(o) = &self.operations {
            for (k, v) in o {
                map.serialize_entry(&k, &v)?;
            }
        }

        if let Some(extra) = &self.additional_operations {
            map.serialize_entry("additionalOperations", extra)?;
        }

        if let Some(parameters) = &self.parameters {
            map.serialize_entry("parameters", parameters)?;
        }

        if let Some(servers) = &self.servers {
            map.serialize_entry("servers", servers)?;
        }

        if let Some(ref ext) = self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(&k, &v)?;
                }
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for PathItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const FIELDS: &[&str] = &[
            "$ref",
            "summary",
            "description",
            "parameters",
            "servers",
            "get",
            "head",
            "post",
            "put",
            "patch",
            "delete",
            "options",
            "trace",
            "query",
            "additionalOperations",
            "x-<ext name>",
        ];

        struct PathItemVisitor;

        impl<'de> Visitor<'de> for PathItemVisitor {
            type Value = PathItem;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct PathItem")
            }

            fn visit_map<V>(self, mut map: V) -> Result<PathItem, V::Error>
            where
                V: MapAccess<'de>,
            {
                let mut res = PathItem::default();
                let mut operations: BTreeMap<String, Operation> = BTreeMap::new();
                let mut extensions: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "$ref" {
                        if res.reference.is_some() {
                            return Err(Error::duplicate_field("$ref"));
                        }
                        res.reference = Some(map.next_value()?);
                    } else if key == "summary" {
                        if res.summary.is_some() {
                            return Err(Error::duplicate_field("summary"));
                        }
                        res.summary = Some(map.next_value()?);
                    } else if key == "description" {
                        if res.description.is_some() {
                            return Err(Error::duplicate_field("description"));
                        }
                        res.description = Some(map.next_value()?);
                    } else if key == "parameters" {
                        if res.parameters.is_some() {
                            return Err(Error::duplicate_field("parameters"));
                        }
                        res.parameters = Some(map.next_value()?);
                    } else if key == "servers" {
                        if res.servers.is_some() {
                            return Err(Error::duplicate_field("servers"));
                        }
                        res.servers = Some(map.next_value()?);
                    } else if key == "additionalOperations" {
                        if res.additional_operations.is_some() {
                            return Err(Error::duplicate_field("additionalOperations"));
                        }
                        res.additional_operations = Some(map.next_value()?);
                    } else if key.starts_with("x-") {
                        if extensions.contains_key(key.clone().as_str()) {
                            return Err(Error::custom(format!("duplicate field '{key}'")));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        // OAS 3.2.0 fixes the Operation field set to these
                        // nine lowercase method names. Field names are
                        // case-sensitive: `GET` is not a fixed field. Use
                        // `additionalOperations` for non-standard methods.
                        const HTTP_METHODS: &[&str] = &[
                            "get", "put", "post", "delete", "options", "head", "patch", "trace",
                            "query",
                        ];
                        if !HTTP_METHODS.contains(&key.as_str()) {
                            return Err(Error::unknown_field(key.as_str(), FIELDS));
                        }
                        if operations.contains_key(key.as_str()) {
                            return Err(Error::custom(format!("duplicate field '{key}'")));
                        }
                        operations.insert(key, map.next_value()?);
                    }
                }
                if !operations.is_empty() {
                    res.operations = Some(operations);
                }
                if !extensions.is_empty() {
                    res.extensions = Some(extensions);
                }
                Ok(res)
            }
        }

        deserializer.deserialize_struct("PathItem", FIELDS, PathItemVisitor)
    }
}

impl ValidateWithContext<Spec> for PathItem {
    fn validate_with_context(&self, ctx: &mut Context<Spec>, path: String) {
        if let Some(r) = &self.reference {
            if r.is_empty() {
                ctx.error(path.clone(), ".$ref: must not be empty");
            } else if r.starts_with("#/") {
                if internal_path_item_ref_target_exists(ctx.spec, r) {
                    // Mark the target as visited so unused-component
                    // detection (e.g. `components.pathItems[X]` reached
                    // only via a `$ref`) doesn't falsely flag it. Also
                    // mark the component container itself when the ref
                    // is into `components.pathItems` or
                    // `components.callbacks`, which the unused-check
                    // keys off of.
                    ctx.visit(r.clone());
                    if let Some(component) = component_container_visit(r) {
                        ctx.visit(component);
                    }
                } else {
                    ctx.error(
                        path.clone(),
                        format_args!(".$ref: target `{r}` is not declared in this document"),
                    );
                }
            } else if !ctx.is_option(Options::IgnoreExternalReferences) {
                ctx.error(
                    path.clone(),
                    format_args!(".$ref: external reference `{r}` is not supported"),
                );
            }
        }

        if let Some(operations) = &self.operations {
            for (method, operation) in operations.iter() {
                operation.validate_with_context(ctx, format!("{path}.{method}"));
            }
        }

        if let Some(extra) = &self.additional_operations {
            // OAS 3.2.0: keys must use exact request capitalization, must
            // not be empty, and must NOT name a standard HTTP method (those
            // belong in the corresponding fixed field).
            const STANDARD_METHODS: &[&str] = &[
                "GET", "PUT", "POST", "DELETE", "OPTIONS", "HEAD", "PATCH", "TRACE", "QUERY",
            ];
            for (method, operation) in extra.iter() {
                let method_path = format!("{path}.additionalOperations[{method}]");
                if method.is_empty() {
                    ctx.error(method_path.clone(), "method name must not be empty");
                } else if STANDARD_METHODS.contains(&method.to_ascii_uppercase().as_str()) {
                    ctx.error(
                        method_path.clone(),
                        format_args!(
                            "`{method}` is a standard HTTP method; declare it as a fixed field instead"
                        ),
                    );
                }
                operation.validate_with_context(ctx, method_path);
            }
        }

        if let Some(servers) = &self.servers {
            for (i, server) in servers.iter().enumerate() {
                server.validate_with_context(ctx, format!("{path}.servers[{i}]"));
            }
        }

        if let Some(parameters) = &self.parameters {
            for (i, parameter) in parameters.iter().enumerate() {
                parameter.validate_with_context(ctx, format!("{path}.parameters[{i}]"));
            }
        }
    }
}

/// Decode one JSON Pointer reference token (RFC 6901): `~1` → `/`,
/// `~0` → `~`. Order matters so `~01` round-trips to `~1`.
fn unescape_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// If `reference` resolves to a `PathItem` housed under a Components
/// container (`#/components/pathItems/<name>` or
/// `#/components/callbacks/<name>/<expr>`), return the component-level
/// reference (`#/components/pathItems/<name>` or
/// `#/components/callbacks/<name>`) that the unused-check keys off.
fn component_container_visit(reference: &str) -> Option<String> {
    if reference.starts_with("#/components/pathItems/") {
        Some(reference.to_owned())
    } else if let Some(after) = reference.strip_prefix("#/components/callbacks/") {
        let cb_token = after.split_once('/').map(|(c, _)| c).unwrap_or(after);
        Some(format!("#/components/callbacks/{cb_token}"))
    } else {
        None
    }
}

/// True if `reference` (an internal `#/...` pointer) names a `PathItem`
/// declared anywhere in the document. PathItem `$ref`s may target any of
/// the four containers that hold PathItem objects: `#/paths`,
/// `#/webhooks`, `#/components/pathItems`, or
/// `#/components/callbacks/<name>/<expression>` (each Callback's `paths`
/// map values are PathItem objects too).
fn internal_path_item_ref_target_exists(spec: &Spec, reference: &str) -> bool {
    let one_token = |after: &str| -> Option<String> {
        if after.contains('/') {
            None
        } else {
            Some(unescape_pointer_token(after))
        }
    };
    if let Some(after) = reference.strip_prefix("#/paths/") {
        one_token(after).is_some_and(|k| {
            spec.paths
                .as_ref()
                .is_some_and(|p| p.paths.contains_key(&k))
        })
    } else if let Some(after) = reference.strip_prefix("#/webhooks/") {
        one_token(after).is_some_and(|k| {
            spec.webhooks
                .as_ref()
                .is_some_and(|w| w.paths.contains_key(&k))
        })
    } else if let Some(after) = reference.strip_prefix("#/components/pathItems/") {
        one_token(after).is_some_and(|k| {
            spec.components
                .as_ref()
                .and_then(|c| c.path_items.as_ref())
                .is_some_and(|m| m.contains_key(&k))
        })
    } else if let Some(after) = reference.strip_prefix("#/components/callbacks/") {
        let mut split = after.splitn(2, '/');
        let (Some(cb_token), Some(expr_token)) = (split.next(), split.next()) else {
            return false;
        };
        if expr_token.contains('/') {
            return false;
        }
        let cb_name = unescape_pointer_token(cb_token);
        let expr = unescape_pointer_token(expr_token);
        spec.components
            .as_ref()
            .and_then(|c| c.callbacks.as_ref())
            .and_then(|m| m.get(&cb_name))
            .and_then(|cb_ref| cb_ref.get_item(spec).ok())
            .is_some_and(|cb| cb.paths.contains_key(&expr))
    } else {
        false
    }
}

/// The Paths Object (and Webhooks shape, structurally identical):
/// holds the relative paths to the individual endpoints (or webhook
/// expressions) and supports `^x-` Specification Extensions per OAS 3.2.0.
///
/// Per OAS 3.1, the patterned values are `Path Item Object`s — and a Path
/// Item Object's `$ref` is one of its own fixed fields. We therefore use
/// **bare `PathItem`** (not `RefOr<PathItem>`) here; the reference case is
/// modelled as a `PathItem` whose `reference` field is set, which preserves
/// the spec-allowed adjacent fields (`summary`, `description`) instead of
/// dropping them via Reference Object semantics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paths {
    /// Map from path / webhook key to its `PathItem`.
    pub paths: BTreeMap<String, PathItem>,

    /// `^x-` Specification Extensions on the Paths / Webhooks Object itself.
    pub extensions: Option<BTreeMap<String, serde_json::Value>>,
}

impl Paths {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, PathItem> {
        self.paths.iter()
    }
}

impl<S, K> From<S> for Paths
where
    S: IntoIterator<Item = (K, PathItem)>,
    K: Into<String>,
{
    fn from(iter: S) -> Self {
        Paths {
            paths: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            extensions: None,
        }
    }
}

impl Serialize for Paths {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ext_x_count = self
            .extensions
            .as_ref()
            .map(|e| e.keys().filter(|k| k.starts_with("x-")).count())
            .unwrap_or(0);
        let total = self.paths.len() + ext_x_count;
        let mut map = serializer.serialize_map(Some(total))?;
        for (k, v) in &self.paths {
            map.serialize_entry(k, v)?;
        }
        if let Some(ext) = &self.extensions {
            for (k, v) in ext {
                if k.starts_with("x-") {
                    map.serialize_entry(k, v)?;
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Paths {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PathsVisitor;
        impl<'de> Visitor<'de> for PathsVisitor {
            type Value = Paths;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a Paths or Webhooks object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Paths, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut paths: BTreeMap<String, PathItem> = BTreeMap::new();
                let mut ext: BTreeMap<String, serde_json::Value> = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("x-") {
                        if ext.contains_key(&key) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        ext.insert(key, map.next_value()?);
                    } else {
                        if paths.contains_key(&key) {
                            return Err(Error::custom(format_args!("duplicate field `{key}`")));
                        }
                        paths.insert(key, map.next_value()?);
                    }
                }
                Ok(Paths {
                    paths,
                    extensions: if ext.is_empty() { None } else { Some(ext) },
                })
            }
        }
        deserializer.deserialize_map(PathsVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::ValidationErrorsExt;
    use serde_json::json;

    #[test]
    fn path_item_round_trip_with_3_2_fixed_fields() {
        let v = json!({
            "$ref": "#/components/pathItems/Common",
            "summary": "Pets path",
            "description": "All pet operations",
            "get": {"responses": {"200": {"description": "ok"}}},
            "parameters": [
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}
            ],
            "servers": [{"url": "https://api.example.com"}],
            "x-internal": "yes"
        });
        let pi: PathItem = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(
            pi.reference.as_deref(),
            Some("#/components/pathItems/Common")
        );
        assert_eq!(pi.summary.as_deref(), Some("Pets path"));
        assert_eq!(pi.description.as_deref(), Some("All pet operations"));
        let back = serde_json::to_value(&pi).unwrap();
        let re: PathItem = serde_json::from_value(back).unwrap();
        assert_eq!(re, pi);
    }

    #[test]
    fn empty_ref_reported() {
        let pi = PathItem {
            reference: Some("".into()),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains(".$ref: must not be empty")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn dangling_internal_ref_reported() {
        let pi = PathItem {
            reference: Some("#/components/pathItems/Missing".into()),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in this document")),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn external_ref_reported_unless_ignored() {
        let pi = PathItem {
            reference: Some("https://example.com/spec#/paths/~1pets".into()),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.mentions("external reference"),
            "errors: {:?}",
            ctx.errors
        );
        let mut ctx = Context::new(&spec, Options::IgnoreExternalReferences.only());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            !ctx.errors.mentions("external reference"),
            "errors: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn ref_to_components_callbacks_marks_callback_container_visited() {
        // Codex: a `$ref` to `#/components/callbacks/CB/e` reaches a
        // PathItem inside the callback. The unused-callbacks check keys
        // off `#/components/callbacks/CB`, so that container — not just
        // the deep path — must be marked visited.
        use crate::v3_2::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("e".to_owned(), PathItem::default());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = crate::v3_2::components::Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let pi = PathItem {
            reference: Some("#/components/callbacks/CB/e".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::empty());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.is_visited("#/components/callbacks/CB"),
            "callback container must be marked visited"
        );
    }

    #[test]
    fn ref_target_marked_visited_for_unused_detection() {
        // A `paths` entry that is purely a `$ref` to
        // `components.pathItems[Foo]` must mark the target as used so the
        // unused-detection pass doesn't flag it.
        let mut cp = BTreeMap::new();
        cp.insert("Foo".to_owned(), PathItem::default());
        let comp = crate::v3_2::components::Components {
            path_items: Some(cp),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let pi = PathItem {
            reference: Some("#/components/pathItems/Foo".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::empty());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.is_visited("#/components/pathItems/Foo"),
            "$ref target must be marked visited"
        );
    }

    #[test]
    fn internal_ref_resolved_against_components_callbacks() {
        // Callback values are PathItem objects, so a `$ref` to
        // `#/components/callbacks/<n>/<expr>` is a valid PathItem ref.
        use crate::v3_2::callback::Callback;
        let mut cb_paths = BTreeMap::new();
        cb_paths.insert("e".to_owned(), PathItem::default());
        let cb = Callback {
            paths: cb_paths,
            ..Default::default()
        };
        let comp = crate::v3_2::components::Components {
            callbacks: Some(BTreeMap::from([("CB".to_owned(), RefOr::new_item(cb))])),
            ..Default::default()
        };
        let spec = Spec {
            components: Some(comp),
            ..Default::default()
        };
        let pi = PathItem {
            reference: Some("#/components/callbacks/CB/e".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            !ctx.errors.mentions("not declared"),
            "callback path-item target should resolve: {:?}",
            ctx.errors
        );

        // Dangling target reports.
        let pi = PathItem {
            reference: Some("#/components/callbacks/CB/missing".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("not declared in this document")),
            "dangling callback path-item should error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn internal_ref_resolved_against_each_container() {
        let mut paths = Paths::default();
        paths.paths.insert("/pets".to_owned(), PathItem::default());
        let mut webhooks = Paths::default();
        webhooks
            .paths
            .insert("petCreated".to_owned(), PathItem::default());
        let mut cp = BTreeMap::new();
        cp.insert("Reusable".to_owned(), PathItem::default());
        let comp = crate::v3_2::components::Components {
            path_items: Some(cp),
            ..Default::default()
        };
        let spec = Spec {
            paths: Some(paths),
            webhooks: Some(webhooks),
            components: Some(comp),
            ..Default::default()
        };
        for r in [
            "#/paths/~1pets",
            "#/webhooks/petCreated",
            "#/components/pathItems/Reusable",
        ] {
            let pi = PathItem {
                reference: Some(r.into()),
                ..Default::default()
            };
            let mut ctx = Context::new(&spec, crate::validation::Options::new());
            pi.validate_with_context(&mut ctx, "p".into());
            assert!(
                !ctx.errors.mentions("not declared"),
                "{r} should resolve: {:?}",
                ctx.errors
            );
        }
    }

    #[test]
    fn paths_struct_round_trip_extensions() {
        let v = json!({
            "/pets": {"get": {"responses": {"200": {"description": "ok"}}}},
            "x-key": "value"
        });
        let p: Paths = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(p.len(), 1);
        assert!(p.extensions.is_some());
        let back = serde_json::to_value(&p).unwrap();
        let re: Paths = serde_json::from_value(back).unwrap();
        assert_eq!(re, p);
    }

    #[test]
    fn paths_dup_path_errors() {
        let raw = r#"{"/a": {}, "/a": {}}"#;
        let res: Result<Paths, _> = serde_json::from_str(raw);
        assert!(res.is_err());
    }

    #[test]
    fn paths_iter_and_from() {
        let p: Paths = [("/a", PathItem::default())].into();
        assert_eq!(p.len(), 1);
        assert_eq!(p.iter().count(), 1);
    }

    #[test]
    fn unknown_method_key_rejected() {
        let raw = r#"{"gett": {"responses": {"200": {"description": "ok"}}}}"#;
        let err = serde_json::from_str::<PathItem>(raw).expect_err("expected unknown-field error");
        let msg = err.to_string();
        assert!(
            msg.contains("gett") && msg.contains("unknown field"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn uppercase_method_rejected() {
        let raw = r#"{"GET": {"responses": {"200": {"description": "ok"}}}}"#;
        let err = serde_json::from_str::<PathItem>(raw)
            .expect_err("expected unknown-field error for uppercase method");
        assert!(
            err.to_string().contains("unknown field") && err.to_string().contains("GET"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn path_item_serialize_with_operations_and_extensions() {
        // Exercises the serialize branches for operations, additional_operations, and extensions.
        let mut ops = BTreeMap::new();
        ops.insert(
            "get".to_owned(),
            crate::v3_2::operation::Operation {
                responses: Some(crate::v3_2::response::Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(crate::v3_2::response::Response {
                            description: Some("ok".into()),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let mut additional_ops = BTreeMap::new();
        additional_ops.insert(
            "SEARCH".to_owned(),
            crate::v3_2::operation::Operation {
                responses: Some(crate::v3_2::response::Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(crate::v3_2::response::Response {
                            description: Some("ok".into()),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let mut ext = BTreeMap::new();
        ext.insert("x-custom".to_owned(), serde_json::json!("val"));
        let pi = PathItem {
            operations: Some(ops),
            additional_operations: Some(additional_ops),
            extensions: Some(ext),
            ..Default::default()
        };
        let v = serde_json::to_value(&pi).unwrap();
        assert!(v["get"].is_object(), "get operation should be serialized");
        assert!(v["additionalOperations"].is_object());
        assert_eq!(v["x-custom"], "val");
    }

    #[test]
    fn path_item_deserialize_duplicate_fields_rejected() {
        // Duplicate $ref, summary, description, parameters, servers, additionalOperations
        for raw in [
            r##"{"$ref": "#/a", "$ref": "#/b"}"##,
            r#"{"summary": "a", "summary": "b"}"#,
            r#"{"description": "a", "description": "b"}"#,
            r#"{"parameters": [], "parameters": []}"#,
            r#"{"servers": [], "servers": []}"#,
        ] {
            let result = serde_json::from_str::<PathItem>(raw);
            assert!(result.is_err(), "duplicate field should error for: {raw}");
        }
    }

    #[test]
    fn path_item_deserialize_duplicate_x_field_rejected() {
        let raw = r#"{"x-foo": 1, "x-foo": 2}"#;
        let result = serde_json::from_str::<PathItem>(raw);
        assert!(result.is_err(), "duplicate x- field should error");
    }

    #[test]
    fn path_item_deserialize_duplicate_method_rejected() {
        let raw = r#"{"get": {"responses": {"200": {"description": "ok"}}}, "get": {"responses": {"200": {"description": "ok"}}}}"#;
        let result = serde_json::from_str::<PathItem>(raw);
        assert!(result.is_err(), "duplicate method should error");
    }

    #[test]
    fn additional_operations_validates_standard_method_error() {
        // additionalOperations must not use standard HTTP method names.
        let mut extra = BTreeMap::new();
        extra.insert(
            "get".to_owned(),
            crate::v3_2::operation::Operation {
                responses: Some(crate::v3_2::response::Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(crate::v3_2::response::Response {
                            description: Some("ok".into()),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let pi = PathItem {
            additional_operations: Some(extra),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("standard HTTP method")),
            "standard method in additionalOperations must error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn additional_operations_validates_empty_method_error() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "".to_owned(),
            crate::v3_2::operation::Operation {
                responses: Some(crate::v3_2::response::Responses {
                    responses: Some(BTreeMap::from([(
                        "200".to_owned(),
                        RefOr::new_item(crate::v3_2::response::Response {
                            description: Some("ok".into()),
                            ..Default::default()
                        }),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        let pi = PathItem {
            additional_operations: Some(extra),
            ..Default::default()
        };
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors
                .iter()
                .any(|e| e.contains("method name must not be empty")),
            "empty method name must error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_item_parameters_and_servers_walk() {
        // Exercises the servers and parameters validate branches.
        let spec = Spec::default();
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        let pi = PathItem {
            servers: Some(vec![crate::v3_2::server::Server {
                url: "".into(), // empty url — will error
                ..Default::default()
            }]),
            parameters: Some(vec![RefOr::new_item(
                crate::v3_2::parameter::Parameter::Query(Box::new(
                    crate::v3_2::parameter::InQuery {
                        name: "q".into(),
                        description: None,
                        required: None,
                        deprecated: None,
                        allow_empty_value: None,
                        style: None,
                        explode: None,
                        allow_reserved: None,
                        schema: Some(RefOr::new_item(crate::v3_2::schema::Schema::Single(
                            Box::new(crate::v3_2::schema::SingleSchema::String(
                                crate::v3_2::schema::StringSchema::default(),
                            )),
                        ))),
                        example: None,
                        examples: None,
                        content: None,
                        extensions: None,
                    },
                )),
            )]),
            ..Default::default()
        };
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.mentions("servers[0]"),
            "expected server url error: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn paths_struct_from_iter_of_str_tuples() {
        // Exercises the From<S> impl where K: Into<String>.
        let p: Paths = vec![
            ("/a".to_owned(), PathItem::default()),
            ("/b".to_owned(), PathItem::default()),
        ]
        .into();
        assert_eq!(p.len(), 2);
        assert!(!p.is_empty());
    }

    #[test]
    fn paths_struct_deserialize_duplicate_path_rejected() {
        let raw = r#"{"/a": {}, "/a": {}}"#;
        let result = serde_json::from_str::<Paths>(raw);
        assert!(result.is_err(), "duplicate path should error");
    }

    #[test]
    fn paths_struct_deserialize_duplicate_extension_rejected() {
        let raw = r#"{"x-foo": 1, "x-foo": 2}"#;
        let result = serde_json::from_str::<Paths>(raw);
        assert!(result.is_err(), "duplicate x- extension should error");
    }

    #[test]
    fn path_item_ref_to_webhooks_resolves() {
        use crate::v3_2::path_item::Paths;
        let mut webhooks = Paths::default();
        webhooks
            .paths
            .insert("petCreated".to_owned(), PathItem::default());
        let spec = Spec {
            webhooks: Some(webhooks),
            ..Default::default()
        };
        let pi = PathItem {
            reference: Some("#/webhooks/petCreated".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            !ctx.errors.mentions("not declared"),
            "webhooks ref should resolve: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn path_item_serialize_with_ref_covers_ref_field() {
        // line 107: serialize_entry for $ref
        // lines 136-138: extension key branch — covers both x- (true) and non-x- (false)
        let mut ext = BTreeMap::new();
        ext.insert("x-custom".to_owned(), serde_json::json!("v"));
        // A non-x- key exercises the "condition false" path (line 138 else).
        ext.insert("non-x-key".to_owned(), serde_json::json!("ignored"));
        let pi = PathItem {
            reference: Some("#/components/pathItems/Common".into()),
            extensions: Some(ext),
            ..Default::default()
        };
        let v = serde_json::to_value(&pi).unwrap();
        assert_eq!(v["$ref"], "#/components/pathItems/Common");
        assert_eq!(v["x-custom"], "v");
        assert!(v.get("non-x-key").is_none());
    }

    #[test]
    fn path_item_deserialize_visitor_expecting_on_non_map() {
        // lines 175-177: PathItemVisitor::expecting is invoked when serde
        // encounters a non-map value.
        let err = serde_json::from_value::<PathItem>(serde_json::json!("not a map"))
            .expect_err("expected error");
        assert!(
            err.to_string().contains("PathItem"),
            "expecting message should mention PathItem: {err}"
        );
    }

    #[test]
    fn path_item_deserialize_duplicate_additional_operations_rejected() {
        // line 214: duplicate additionalOperations field
        let raw = r#"{"additionalOperations": {}, "additionalOperations": {}}"#;
        let result = serde_json::from_str::<PathItem>(raw);
        assert!(
            result.is_err(),
            "duplicate additionalOperations should error"
        );
    }

    #[test]
    fn internal_path_item_ref_target_exists_malformed_paths_token() {
        // line 360: #/paths/<token> where token contains `/` (malformed) returns false
        let spec = Spec::default();
        let pi = PathItem {
            reference: Some("#/paths/a/b".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        // The ref target check returns false → reports "not declared"
        assert!(
            ctx.errors.iter().any(|e| e.contains("not declared")),
            "malformed paths token should report not declared: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn internal_path_item_ref_target_exists_callbacks_no_slash_returns_false() {
        // line 387: #/components/callbacks/<token> with no `/` in after returns false
        // (callback target needs two tokens: <cb_name>/<expr>)
        let spec = Spec::default();
        let pi = PathItem {
            reference: Some("#/components/callbacks/OnlyOnePart".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not declared")),
            "callback ref with no expression should report not declared: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn internal_path_item_ref_target_exists_callbacks_expr_with_slash_returns_false() {
        // line 390: #/components/callbacks/<cb>/<expr> where expr_token contains `/`
        let spec = Spec::default();
        let pi = PathItem {
            reference: Some("#/components/callbacks/CB/expr/extra".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not declared")),
            "callback expr with extra slash should report not declared: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn internal_path_item_ref_unknown_prefix_returns_false() {
        // line 401: else { false } branch for unknown prefix
        let spec = Spec::default();
        let pi = PathItem {
            reference: Some("#/unknown/foo".into()),
            ..Default::default()
        };
        let mut ctx = Context::new(&spec, crate::validation::Options::new());
        pi.validate_with_context(&mut ctx, "p".into());
        assert!(
            ctx.errors.iter().any(|e| e.contains("not declared")),
            "unknown prefix should report not declared: {:?}",
            ctx.errors
        );
    }

    #[test]
    fn paths_serialize_x_extension_covered() {
        // line 469: serialize_entry for x- extension in Paths
        // line 470: closing `}` — both the true branch (x- key) and the
        // false branch (non-x- key that is silently skipped) must be exercised.
        let mut ext = BTreeMap::new();
        ext.insert("x-info".to_owned(), serde_json::json!("yes"));
        // Add a non-x- key to exercise the "condition false" branch (line 470).
        ext.insert("not-x".to_owned(), serde_json::json!("ignored"));
        let paths = Paths {
            paths: BTreeMap::new(),
            extensions: Some(ext),
        };
        let v = serde_json::to_value(&paths).unwrap();
        assert_eq!(v["x-info"], "yes");
        // non-x- key is filtered out by the serializer
        assert!(v.get("not-x").is_none());
    }

    #[test]
    fn paths_deserialize_visitor_expecting_on_non_map() {
        // lines 486-488: PathsVisitor::expecting is invoked for non-map input
        let err = serde_json::from_value::<Paths>(serde_json::json!("not a map"))
            .expect_err("expected error");
        assert!(
            err.to_string().contains("Paths") || err.to_string().contains("Webhooks"),
            "expecting message should mention Paths or Webhooks: {err}"
        );
    }
}
