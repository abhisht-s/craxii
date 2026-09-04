#![cfg(all(feature = "test-failpoints", unix))]

#[path = "support/stage18_harness.rs"]
mod stage18_harness;
#[path = "support/stage24_harness.rs"]
mod stage24_harness;

use serde_json::Value;
use stage24_harness::{
    run_canonical_scenario, run_runtime_child_from_environment, validate_frozen_contract,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_local_deterministic_product_composition_repeats() {
    let first = run_canonical_scenario("first").await;
    let second = run_canonical_scenario("second").await;
    assert_eq!(first, second, "normalized Stage 24 semantics diverged");

    let contract: Value =
        serde_json::from_str(include_str!("fixtures/stage24-v1/evidence-contract.json"))
            .expect("valid Stage 24 evidence contract");
    validate_frozen_contract(&contract, &first).expect("Stage 24 frozen evidence contract");
    let mut relationship_mutation = contract.clone();
    relationship_mutation["required_relationships"][0] =
        Value::String("wrong_relationship".to_owned());
    assert_eq!(
        relationship_mutation["required_relationships"]
            .as_array()
            .unwrap()
            .len(),
        contract["required_relationships"].as_array().unwrap().len()
    );
    assert!(validate_frozen_contract(&relationship_mutation, &first).is_err());
    assert!(!include_str!("fixtures/stage24-v1/evidence-contract.json").contains("real_bash_lc"));
    assert!(
        contract["normalization"]["removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "machine_specific_values")
    );
    assert_eq!(
        contract["portable_host"]["result"],
        first.portable_host_result
    );
    assert_eq!(
        contract["ubuntu_target"]["result"],
        first.ubuntu_target_result
    );
}

#[test]
fn stage24_runtime_child() {
    run_runtime_child_from_environment();
}
