#[cfg(feature = "v3_2")]
mod v3_2_tests {
    use std::fs;

    use roas::v3_2::spec::Spec;
    use roas::validation::{Options, Validate};

    #[test]
    fn files() {
        for entry in fs::read_dir("tests/v3_2_data").unwrap() {
            let path_buf = entry.unwrap().path();
            // Skip directories and non-JSON files (e.g. .DS_Store, README).
            if !path_buf.is_file() || path_buf.extension().and_then(|s| s.to_str()) != Some("json")
            {
                continue;
            }
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
