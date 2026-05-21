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

    // ---- Merge coverage ----

    use crate::merge::{ConflictKind, MergeContext, MergeOptions, MergeWithContext, Resolution};

    impl MergeWithContext<()> for Foo {
        fn merge_with_context(
            &mut self,
            other: Self,
            ctx: &mut MergeContext<()>,
            path: &mut String,
        ) {
            if self.foo != other.foo
                && ctx.should_take_incoming(path, ConflictKind::ScalarOverridden)
            {
                self.foo = other.foo;
            }
        }
    }

    fn ctx_with(opts: enumset::EnumSet<MergeOptions>) -> MergeContext<'static, ()> {
        MergeContext::new(&(), opts)
    }

    #[test]
    fn bool_or_item_item_recurses_into_inner() {
        let mut base: BoolOr<Foo> = BoolOr::Item(Foo { foo: "a".into() });
        let mut ctx = ctx_with(MergeOptions::new());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Item(Foo { foo: "b".into() }), &mut ctx, &mut path);
        match base {
            BoolOr::Item(f) => assert_eq!(f.foo, "b"),
            _ => panic!("expected Item"),
        }
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::ScalarOverridden);
    }

    #[test]
    fn bool_or_bool_bool_same_value_no_conflict() {
        let mut base: BoolOr<Foo> = BoolOr::Bool(true);
        let mut ctx = ctx_with(MergeOptions::new());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Bool(true), &mut ctx, &mut path);
        assert!(matches!(base, BoolOr::Bool(true)));
        assert!(ctx.conflicts.is_empty());
    }

    #[test]
    fn bool_or_bool_bool_different_takes_incoming_by_default() {
        let mut base: BoolOr<Foo> = BoolOr::Bool(true);
        let mut ctx = ctx_with(MergeOptions::new());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Bool(false), &mut ctx, &mut path);
        assert!(matches!(base, BoolOr::Bool(false)));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::ScalarOverridden);
    }

    #[test]
    fn bool_or_bool_bool_different_base_wins() {
        let mut base: BoolOr<Foo> = BoolOr::Bool(true);
        let mut ctx = ctx_with(MergeOptions::BaseWins.only());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Bool(false), &mut ctx, &mut path);
        assert!(matches!(base, BoolOr::Bool(true)));
        assert_eq!(ctx.conflicts[0].resolution, Resolution::Base);
    }

    #[test]
    fn bool_or_bool_vs_item_replaces_with_ref_vs_value_kind() {
        let mut base: BoolOr<Foo> = BoolOr::Bool(true);
        let mut ctx = ctx_with(MergeOptions::new());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Item(Foo { foo: "x".into() }), &mut ctx, &mut path);
        assert!(matches!(base, BoolOr::Item(_)));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::RefVsValue);
    }

    #[test]
    fn bool_or_item_vs_bool_replaces_with_ref_vs_value_kind() {
        let mut base: BoolOr<Foo> = BoolOr::Item(Foo { foo: "x".into() });
        let mut ctx = ctx_with(MergeOptions::new());
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Bool(false), &mut ctx, &mut path);
        assert!(matches!(base, BoolOr::Bool(false)));
        assert_eq!(ctx.conflicts.len(), 1);
        assert_eq!(ctx.conflicts[0].kind, ConflictKind::RefVsValue);
    }

    #[test]
    fn bool_or_errored_entry_short_circuits() {
        let mut base: BoolOr<Foo> = BoolOr::Bool(true);
        let mut ctx = ctx_with(MergeOptions::new());
        ctx.errored = true;
        let mut path = "#".to_owned();
        base.merge_with_context(BoolOr::Bool(false), &mut ctx, &mut path);
        // Errored entry → no-op; base unchanged, no new conflict.
        assert!(matches!(base, BoolOr::Bool(true)));
        assert!(ctx.conflicts.is_empty());
    }
}
