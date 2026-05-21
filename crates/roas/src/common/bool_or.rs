use crate::common::reference::{RefOr, ResolveReference};
use crate::validation::{Context, ValidateWithContext};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum BoolOr<T> {
    Bool(bool),
    Item(T),
}

impl<D> BoolOr<RefOr<D>> {
    pub(crate) fn validate_with_context<T>(&self, ctx: &mut Context<T>, path: String)
    where
        T: ResolveReference<D>,
        D: ValidateWithContext<T> + 'static + Clone + DeserializeOwned,
    {
        match self {
            BoolOr::Bool(_) => {}
            BoolOr::Item(d) => {
                d.validate_with_context(ctx, path);
            }
        }
    }
}

impl<D, T> crate::merge::MergeWithContext<T> for BoolOr<D>
where
    D: crate::merge::MergeWithContext<T>,
{
    fn merge_with_context(
        &mut self,
        other: Self,
        ctx: &mut crate::merge::MergeContext<T>,
        path: &mut String,
    ) {
        if ctx.errored {
            return;
        }
        use crate::merge::ConflictKind;
        match (self, other) {
            (BoolOr::Item(base), BoolOr::Item(incoming)) => {
                base.merge_with_context(incoming, ctx, path);
            }
            (slot @ BoolOr::Bool(_), BoolOr::Bool(incoming_bool)) => {
                let BoolOr::Bool(base_bool) = slot else {
                    unreachable!()
                };
                if *base_bool != incoming_bool
                    && ctx.should_take_incoming(path, ConflictKind::ScalarOverridden)
                {
                    *base_bool = incoming_bool;
                }
            }
            (slot, incoming) => {
                if ctx.should_take_incoming(path, ConflictKind::RefVsValue) {
                    *slot = incoming;
                }
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
