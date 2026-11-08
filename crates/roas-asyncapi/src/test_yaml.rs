//! The YAML reader every test in this crate goes through, with the same
//! options the roas fetchers, `roas-codegen` and `roas-cli` read YAML with,
//! so no test drifts to `serde-saphyr`'s defaults: YAML 1.2 booleans, `017`
//! and `1_000` kept as strings, non-finite floats passed through, merge keys
//! expanded, duplicate keys rejected.

pub fn from_str<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, serde_saphyr::Error> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        leading_zero_numbers_as_strings: true,
        underscored_numbers_as_strings: true,
        non_finite_float_policy: serde_saphyr::NonFiniteFloatPolicy::PassThrough,
        merge_keys: serde_saphyr::MergeKeyPolicy::Merge,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
    };
    serde_saphyr::from_str_with_options(input, options)
}
