#[cfg(feature = "v3_1")]
mod v3_1_tests {
    use std::fs;

    use roas::v3_1::spec::Spec;
    use roas::validation::{Options, Validate};

    #[test]
    fn files() {
        for path in fs::read_dir("tests/v3_1_data").unwrap() {
            let path_buf = path.unwrap().path();
            println!("validating: {path_buf:?}");
            let json_spec = fs::read_to_string(&path_buf).unwrap();
            let spec = serde_json::from_str::<Spec>(&json_spec).unwrap();
            match spec.validate(Options::IgnoreMissingTags.only()) {
                Ok(_) => {}
                Err(err) => {
                    panic!("validation failed: {err}");
                }
            }
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&json_spec).unwrap(),
                serde_json::to_value(spec).unwrap(),
            );
        }
    }
}
