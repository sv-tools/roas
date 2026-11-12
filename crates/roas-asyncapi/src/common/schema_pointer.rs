//! Following a schema `$ref` whose pointer reaches *into* a schema.
//!
//! `#/components/schemas/User` names a component; JSON Schema also lets
//! a reference name any subschema below it, such as
//! `#/components/schemas/User/properties/address` or `…/allOf/0`. Every
//! version models draft-07 the same way, so one macro gives each
//! version's `Schema` the walk, and the chain is followed by the same
//! [`follow`](crate::common::resolve::follow) as every other kind.
//!
//! A pointer walks the document *as written*: stepping through a schema
//! that is itself only a `$ref` names nothing, as it would in the JSON.

/// An array index token: digits only, without a leading zero.
pub(crate) fn index(token: &str) -> Option<usize> {
    if token.is_empty()
        || !token.bytes().all(|b| b.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    token.parse().ok()
}

/// Give a version's `Schema` the subschema walk, and its `SubSchema` the
/// conversion to a [`Step`](crate::common::resolve::Step).
macro_rules! schema_pointer {
    () => {
        impl Schema {
            /// The subschema the leading `tokens` name, and how many
            /// tokens that took.
            fn subschema_at<'a>(&'a self, tokens: &[String]) -> Option<(&'a SubSchema, usize)> {
                use $crate::common::schema_pointer::index;
                let one = |child: Option<&'a SubSchema>| child.map(|c| (c, 1));
                let keyed =
                    |map: &'a std::collections::BTreeMap<String, SubSchema>,
                     key: Option<&String>| { map.get(key?).map(|c| (c, 2)) };
                let indexed = |list: Option<&'a Vec<SubSchema>>, i: Option<&String>| {
                    list?.get(index(i?)?).map(|c| (c, 2))
                };
                let (keyword, next) = (tokens.first()?.as_str(), tokens.get(1));
                match keyword {
                    "definitions" => keyed(&self.definitions, next),
                    "properties" => keyed(&self.properties, next),
                    "patternProperties" => keyed(&self.pattern_properties, next),
                    "dependencies" => match self.dependencies.get(next?)? {
                        Dependency::Schema(schema) => Some((schema, 2)),
                        Dependency::Required(_) => None,
                    },
                    "additionalProperties" => one(self.additional_properties.as_ref()),
                    "propertyNames" => one(self.property_names.as_deref()),
                    "items" => match self.items.as_ref()? {
                        Items::Single(schema) => one(Some(schema)),
                        Items::Tuple(schemas) => indexed(Some(schemas), next),
                    },
                    "additionalItems" => one(self.additional_items.as_deref()),
                    "contains" => one(self.contains.as_deref()),
                    "allOf" => indexed(self.all_of.as_ref(), next),
                    "anyOf" => indexed(self.any_of.as_ref(), next),
                    "oneOf" => indexed(self.one_of.as_ref(), next),
                    "not" => one(self.not.as_deref()),
                    "if" => one(self.if_schema.as_deref()),
                    "then" => one(self.then_schema.as_deref()),
                    "else" => one(self.else_schema.as_deref()),
                    _ => None,
                }
            }

            /// Walk `tokens` down from this schema, as written.
            pub(crate) fn step_at<'a>(
                &'a self,
                tokens: &[String],
            ) -> Option<$crate::common::resolve::Step<'a, Schema>> {
                use $crate::common::resolve::Step;
                let mut step = Step::Item(self);
                let mut tokens = tokens;
                while !tokens.is_empty() {
                    let Step::Item(schema) = step else {
                        return None;
                    };
                    let (child, used) = schema.subschema_at(tokens)?;
                    step = Step::from(child);
                    tokens = &tokens[used..];
                }
                Some(step)
            }
        }

        /// A boolean schema is a schema, but not a `Schema` object.
        impl<'a> From<&'a SubSchema> for $crate::common::resolve::Step<'a, Schema> {
            fn from(schema: &'a SubSchema) -> Self {
                match schema {
                    SubSchema::Bool(_) => Self::Opaque,
                    SubSchema::Schema(schema) => Self::from(schema.as_ref()),
                }
            }
        }
    };
}
pub(crate) use schema_pointer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_accepts_canonical_decimals_only() {
        assert_eq!(index("0"), Some(0));
        assert_eq!(index("12"), Some(12));
        assert_eq!(index(""), None);
        assert_eq!(index("01"), None);
        assert_eq!(index("-1"), None);
    }
}
