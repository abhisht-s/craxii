//! Offline trusted Draft-7 validation; unknown and remote references never resolve.
use craxii_server::distribution_contracts::primitives::format_valid;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, sync::OnceLock};
static CHECKS: OnceLock<BTreeMap<String, jsonschema::Validator>> = OnceLock::new();
pub fn checks() -> &'static BTreeMap<String, jsonschema::Validator> {
    CHECKS.get_or_init(|| {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/distribution/1.0.0/schemas");
        let mut schemas = Vec::new();
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            let s: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert!(jsonschema::draft7::meta::is_valid(&s));
            schemas.push(s);
        }
        let mut registry = jsonschema::Registry::new();
        for schema in &schemas {
            registry = registry
                .add(schema["$id"].as_str().unwrap(), schema.clone())
                .unwrap();
        }
        let registry = registry.prepare().unwrap();
        let mut checks = BTreeMap::new();
        for schema in &schemas {
            let uri = schema["$id"].as_str().unwrap();
            let mut targets = vec![(uri.to_string(), schema.clone())];
            for name in schema["definitions"].as_object().unwrap().keys() {
                let id = format!("{uri}#/definitions/{name}");
                targets.push((id.clone(), json!({"$ref":id})));
            }
            for (id, target) in targets {
                let mut options = jsonschema::draft7::options()
                    .offline()
                    .with_registry(&registry)
                    .should_validate_formats(true)
                    .should_ignore_unknown_formats(false);
                for kind in [
                    "U4",
                    "U7",
                    "Uuid",
                    "D0",
                    "Dp",
                    "U64",
                    "ChatId",
                    "Timestamp",
                    "Digest",
                    "Binary32",
                    "Binary64",
                ] {
                    options = options
                        .with_format(format!("craxii_{kind}"), move |v| format_valid(kind, v));
                }
                for max in [
                    19usize, 20, 27, 32, 36, 43, 64, 86, 128, 256, 512, 65536, 21846, 10923,
                    2097152,
                ] {
                    options =
                        options.with_format(format!("craxii_utf8_{max}"), move |v| v.len() <= max);
                }
                checks.insert(id, options.build(&target).unwrap());
            }
        }
        assert!(
            jsonschema::draft7::options()
                .offline()
                .build(&json!({"$ref":"https://example.invalid/schema"}))
                .is_err()
        );
        assert!(
            jsonschema::draft7::options()
                .offline()
                .build(&json!({"$ref":"file:///not-allowed"}))
                .is_err()
        );
        checks
    })
}
pub fn valid(family: &str, name: &str, v: &Value) -> bool {
    checks()[&format!("urn:craxii:distribution:1.0.0:{family}#/definitions/{name}")].is_valid(v)
}
pub fn fixture(v: &Value) {
    assert!(
        checks()["urn:craxii:distribution:1.0.0:fixture"].is_valid(v),
        "fixture schema mismatch"
    );
}
