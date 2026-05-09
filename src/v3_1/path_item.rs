//! Path Items

use crate::common::helpers::{Context, PushError, ValidateWithContext};
use crate::common::reference::RefOr;
use crate::v3_1::operation::Operation;
use crate::v3_1::parameter::Parameter;
use crate::v3_1::server::Server;
use crate::v3_1::spec::Spec;
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

    /// A definition of the operations on this path.
    ///
    /// Any map items that can be converted to an `Operation` object will be stored here.
    /// This includes `get`, `put`, `post`, `delete`, `options`, `head`, `patch`, `trace`,
    /// and any other custom operations, like SEARCH and etc...
    pub operations: Option<BTreeMap<String, Operation>>,

    /// An alternative server array to service all operations in this path.
    pub servers: Option<Vec<Server>>,

    /// A list of parameters that are applicable for all the operations described under this path.
    /// These parameters can be overridden at the operation level, but cannot be removed there.
    /// The list MUST NOT include duplicated parameters.
    /// A unique parameter is defined by a combination of a name and location.
    /// The list can use the [Reference Object](crate::common::reference::Ref) to link to parameters
    /// that are defined at the [Swagger Object's](crate::v3_1::spec::Spec) parameters.
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
            "<custom method>",
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
                    } else if key.starts_with("x-") {
                        if extensions.contains_key(key.clone().as_str()) {
                            return Err(Error::custom(format!("duplicate field '{key}'")));
                        }
                        extensions.insert(key, map.next_value()?);
                    } else {
                        let key = key.to_lowercase();
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
        if let Some(r) = &self.reference
            && r.is_empty()
        {
            ctx.error(path.clone(), ".$ref: must not be empty");
        }

        if let Some(operations) = &self.operations {
            for (method, operation) in operations.iter() {
                operation.validate_with_context(ctx, format!("{path}.{method}"));
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

/// The Paths Object (and Webhooks shape, structurally identical):
/// holds the relative paths to the individual endpoints (or webhook
/// expressions) and supports `^x-` Specification Extensions per OAS 3.1.2.
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
    use serde_json::json;

    #[test]
    fn path_item_round_trip_with_3_1_fixed_fields() {
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
}
