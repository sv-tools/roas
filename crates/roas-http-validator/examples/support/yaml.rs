//! The YAML reader every example and test in this crate goes through, with the same
//! options the roas fetchers, `roas-codegen` and `roas-cli` read YAML with,
//! so none drifts to `serde-saphyr`'s defaults: YAML 1.2 booleans, `017`
//! and `1_000` kept as strings, non-finite floats passed through, merge keys
//! expanded, duplicate keys rejected.

pub fn from_str<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, serde_saphyr::Error> {
    let options = serde_saphyr::options! {
        // Same options in every roas crate; keep the copies in step
        // (`grep -rn 'strict_booleans: true' crates`). The budget is raised
        // from the defaults (250k nodes, 1M events), which reject ordinary
        // large specs. Depth stays at 64: deeper nesting overflows a 2 MiB
        // thread stack in debug builds. The alias-to-anchor ratio check is off:
        // it rejects any document reusing one anchor 100+ times, a common
        // pattern in specs. The scalar-size and alias-replay limits stay at
        // their defaults and still stop an alias bomb.
        budget: serde_saphyr::budget! {
            max_nodes: 10_000_000,
            max_events: 25_000_000,
            enforce_alias_anchor_ratio: false,
        },
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    };
    serde_saphyr::from_str_with_options(input, options)
}
