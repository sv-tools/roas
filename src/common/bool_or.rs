use crate::common::helpers::{Context, ValidateWithContext};
use crate::common::reference::{RefOr, ResolveReference};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum BoolOr<T> {
    Bool(bool),
    Item(T),
}

impl<D> BoolOr<RefOr<Box<D>>> {
    pub fn validate_with_context_boxed<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T>,
    {
        match self {
            BoolOr::Bool(_) => {}
            BoolOr::Item(d) => {
                d.validate_with_context_boxed(ctx, path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Default)]
    struct Foo {
        pub foo: String,
    }

    #[test]
    fn test_bool_or_foo_serialize() {
        assert_eq!(
            serde_json::to_value(BoolOr::Item(Foo {
                foo: String::from("bar"),
            }))
            .unwrap(),
            serde_json::json!({
                "foo": "bar"
            }),
            "serialize item",
        );

        assert_eq!(
            serde_json::to_value(BoolOr::Bool::<Foo>(true)).unwrap(),
            serde_json::json!(true),
            "serialize true",
        );

        assert_eq!(
            serde_json::to_value(BoolOr::Bool::<Foo>(false)).unwrap(),
            serde_json::json!(false),
            "serialize false",
        );
    }

    #[test]
    fn test_ref_or_foo_deserialize() {
        assert_eq!(
            serde_json::from_value::<BoolOr<Foo>>(serde_json::json!({
                "foo":"bar",
            }))
            .unwrap(),
            BoolOr::Item(Foo {
                foo: String::from("bar"),
            }),
            "deserialize item",
        );

        assert_eq!(
            serde_json::from_value::<BoolOr<Foo>>(serde_json::json!(true)).unwrap(),
            BoolOr::Bool(true),
            "deserialize true",
        );

        assert_eq!(
            serde_json::from_value::<BoolOr<Foo>>(serde_json::json!(false)).unwrap(),
            BoolOr::Bool(false),
            "deserialize true",
        );
    }
}
