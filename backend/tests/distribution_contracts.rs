//! Both independent codecs evaluate the same immutable public vectors.
#[path = "support/distribution_schema.rs"]
mod distribution_schema;
#[path = "support/distribution_state_models.rs"]
mod distribution_state_models;
use aws_lc_rs::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use craxii_server::{
    distribution_contracts::{
        canonical::{DigestDomain, canonical_bytes, digest_material, sha256, worst_case_cost},
        compatibility::{classify_schema_change, negotiate},
        primitives::{binary, decimal, format_valid},
        raw_json::{CONTROL_LIMITS, LARGE_LIMITS, parse_bounded},
        signing::{SignaturePurpose, signature_input},
        wire::{check_semantics, decode_message, encode_message, validate_definition},
    },
    domain::{
        ClientMessageId, CommandRequestHash, ContentBlock, ConversationId, MessageContent, WorkId,
        WorkState,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
const GROUPS: &[&str] = &[
    "canonical",
    "primitives",
    "signatures",
    "negotiation",
    "envelopes",
    "raw-invalid",
    "limits",
    "admission-cancel",
    "result-delivery",
    "authority-lifecycle",
    "budgets-model",
    "neutral-policy-lineage",
    "legacy-regression",
];
fn load(group: &str) -> Value {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/distribution/1.0.0/fixtures");
    let v = serde_json::from_slice(&fs::read(root.join(format!("{group}.json"))).unwrap()).unwrap();
    distribution_schema::fixture(&v);
    v
}
fn string<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn materialize(case: &Value, envelopes: &BTreeMap<String, Value>) -> (String, String, Value) {
    let base = case
        .get("base_case_id")
        .map(|id| &envelopes[id.as_str().unwrap()])
        .unwrap_or(case);
    let mut value = base["value"].clone();
    if let Some(m) = case.get("mutation") {
        let path = m["path"].as_array().unwrap();
        let mut parent = &mut value;
        for key in &path[..path.len() - 1] {
            parent = if let Some(k) = key.as_str() {
                &mut parent[k]
            } else {
                &mut parent[key.as_u64().unwrap() as usize]
            };
        }
        let key = path.last().unwrap();
        if m["kind"] == "remove" {
            parent
                .as_object_mut()
                .unwrap()
                .remove(key.as_str().unwrap());
        } else {
            if let Some(k) = key.as_str() {
                parent[k] = m["value"].clone();
            } else {
                parent[key.as_u64().unwrap() as usize] = m["value"].clone();
            }
        }
    }
    (
        string(base, "family").into(),
        string(base, "definition").into(),
        value,
    )
}
fn evaluate(group: &str, c: &Value, envelopes: &BTreeMap<String, Value>) -> Value {
    let mut report = json!({"case_id":c["case_id"]});
    match group {
        "canonical" => {
            let bytes = canonical_bytes(&c["value"]).unwrap();
            assert_eq!(hex(&bytes), c["canonical_utf8_hex"]);
            report["canonical_sha256"] = json!(sha256(&bytes));
            assert_eq!(report["canonical_sha256"], c["canonical_sha256"]);
            report["digest"] = json!(
                digest_material(
                    DigestDomain::parse(string(c, "domain")).unwrap(),
                    &c["value"]
                )
                .unwrap()
            );
            assert_eq!(report["digest"], c["digest"]);
        }
        "primitives" => {
            let kind = string(c, "primitive");
            let value = string(c, "value");
            let valid = distribution_schema::valid("primitives", kind, &c["value"])
                && format_valid(kind, value);
            assert_eq!(
                valid,
                c["valid"].as_bool().unwrap(),
                "{}",
                string(c, "case_id")
            );
            if valid && matches!(kind, "D0" | "Dp" | "U64" | "ChatId") {
                assert_eq!(decimal(value, kind).unwrap().to_string(), value);
            }
            report["valid"] = json!(valid);
        }
        "envelopes" => {
            let (family, name, value) = materialize(c, envelopes);
            let schema_ok = distribution_schema::valid(&family, &name, &value);
            let mut actual = validate_definition(&family, &name, &value);
            if actual.is_ok() && value.is_object() {
                actual = check_semantics(&value);
            }
            if actual.is_ok()
                && (value["protocol"] == "craxii.worker" || value["type"] == "protocol_rejection")
            {
                let bytes = canonical_bytes(&value).unwrap();
                actual = decode_message(&bytes).and_then(|message| {
                    let encoded = encode_message(&message)?;
                    assert_eq!(encoded, bytes);
                    Ok(())
                });
            }
            let valid = actual.is_ok();
            assert_eq!(
                valid,
                c["valid"].as_bool().unwrap(),
                "{}",
                string(c, "case_id")
            );
            if c.get("code").is_some() {
                assert_eq!(actual.unwrap_err().0, string(c, "code"));
            }
            if valid {
                assert!(schema_ok);
            }
            report["valid"] = json!(valid);
        }
        "raw-invalid" => {
            let parsed = parse_bounded(&unhex(string(c, "raw_utf8_hex")), CONTROL_LIMITS);
            let valid = parsed.is_ok();
            assert_eq!(valid, c["valid"].as_bool().unwrap());
            if c.get("canonical_utf8_hex").is_some() {
                assert_eq!(
                    hex(&canonical_bytes(&parsed.unwrap()).unwrap()),
                    string(c, "canonical_utf8_hex")
                );
            }
            report["valid"] = json!(valid);
        }
        "negotiation" if c["kind"] == "schema_evolution" => {
            let actual = classify_schema_change(&c["before"], &c["after"]);
            assert_eq!(actual, string(c, "expected"));
            report["state_sha256"] = json!(sha256(&canonical_bytes(&json!(actual)).unwrap()));
        }
        "negotiation" => {
            let actual = negotiate(&c["peer"], &c["policy"]);
            assert_eq!(actual, c["expected"]);
            report["valid"] = actual["accepted"].clone();
            report["state_sha256"] = json!(sha256(&canonical_bytes(&actual).unwrap()));
        }
        "signatures" => {
            let purpose = string(c, "purpose");
            let input = if purpose == "standard" {
                Vec::new()
            } else {
                signature_input(
                    SignaturePurpose::parse(purpose).unwrap(),
                    &c["signed_object"],
                )
                .unwrap()
            };
            assert_eq!(hex(&input), string(c, "signature_input_hex"));
            assert_eq!(sha256(&input), string(c, "signature_input_sha256"));
            let key =
                Ed25519KeyPair::from_seed_unchecked(&unhex(string(c, "public_test_seed_hex")))
                    .unwrap();
            assert_eq!(hex(key.public_key().as_ref()), string(c, "public_key_hex"));
            assert_eq!(
                binary(string(c, "public_key_base64url"), Some(32)).unwrap(),
                key.public_key().as_ref()
            );
            let signed = key.sign(&input);
            assert_eq!(hex(signed.as_ref()), string(c, "signature_hex"));
            assert_eq!(
                binary(string(c, "signature_base64url"), Some(64)).unwrap(),
                signed.as_ref()
            );
            let public = UnparsedPublicKey::new(&ED25519, key.public_key().as_ref());
            assert!(public.verify(&input, signed.as_ref()).is_ok());
            let mut altered = input.clone();
            altered.push(b'x');
            assert!(public.verify(&altered, signed.as_ref()).is_err());
            if c.get("signed_object").is_some() {
                for field in [
                    "audience",
                    "purpose",
                    "environment",
                    "installation_id",
                    "generation",
                    "nonce",
                    "expires_at",
                    "request_digest",
                    "signature_version",
                    "key_id",
                ] {
                    let mut changed = c["signed_object"].clone();
                    changed[field] = if changed.get(field).is_none() {
                        json!("synthetic-mutation")
                    } else if changed[field].is_number() {
                        json!(2)
                    } else {
                        json!(changed[field].as_str().unwrap().to_string() + "x")
                    };
                    let mutated =
                        signature_input(SignaturePurpose::parse(purpose).unwrap(), &changed)
                            .unwrap();
                    assert!(public.verify(&mutated, signed.as_ref()).is_err());
                }
            }
            report["signature_valid"] = json!(true);
            report["canonical_sha256"] = json!(sha256(&input));
        }
        "limits" => {
            let size = c["size"].as_u64().unwrap_or(0) as usize;
            let mut result = None;
            let valid = match string(c, "kind") {
                "content_text" => {
                    let content =
                        json!({"version":1,"blocks":[{"type":"text","text":"x".repeat(size)}]});
                    validate_definition("primitives", "Content", &content)
                        .and_then(|()| check_semantics(&content))
                        .is_ok()
                }
                "content_escaped" => {
                    let content =
                        json!({"version":1,"blocks":[{"type":"text","text":"\u{0}".repeat(size)}]});
                    let valid = validate_definition("primitives", "Content", &content)
                        .and_then(|()| check_semantics(&content))
                        .is_ok();
                    if valid {
                        assert_eq!(canonical_bytes(&content).unwrap().len(), 6 * size + 50);
                    }
                    valid
                }
                "wire_raw" => {
                    let (_, _, value) =
                        materialize(&envelopes[string(c, "base_case_id")], envelopes);
                    let mut bytes = canonical_bytes(&value).unwrap();
                    bytes.resize(size, b' ');
                    decode_message(&bytes).is_ok()
                }
                "nodes_large" => parse_bounded(
                    format!("[{}]", vec!["0"; size - 1].join(",")).as_bytes(),
                    LARGE_LIMITS,
                )
                .is_ok(),
                "raw_bytes" => parse_bounded(
                    format!("{{}}{}", " ".repeat(size - 2)).as_bytes(),
                    CONTROL_LIMITS,
                )
                .is_ok(),
                "depth" => parse_bounded(
                    format!("{}0{}", "[".repeat(size), "]".repeat(size)).as_bytes(),
                    CONTROL_LIMITS,
                )
                .is_ok(),
                "nodes" => parse_bounded(
                    format!("[{}]", vec!["0"; size - 1].join(",")).as_bytes(),
                    CONTROL_LIMITS,
                )
                .is_ok(),
                "cost" => {
                    let r = worst_case_cost(c["categories"].as_array().unwrap());
                    if let Ok(v) = &r {
                        result = Some(v.clone());
                    }
                    r.is_ok()
                }
                _ => panic!("unknown limit fixture"),
            };
            assert_eq!(
                valid,
                c["valid"].as_bool().unwrap(),
                "{}",
                string(c, "case_id")
            );
            if c.get("expected").is_some() {
                assert_eq!(result.as_deref(), c["expected"].as_str());
            }
            report["valid"] = json!(valid);
        }
        "legacy-regression" => {
            match string(c, "kind") {
                "internal_digest" => assert_eq!(
                    sha256(&canonical_bytes(&c["value"]).unwrap()),
                    string(c, "expected")
                ),
                "gate_uuid" => {
                    assert_eq!(
                        string(c, "expected"),
                        "b3848cef-3a8f-5de9-8eab-6fc8a36b60a8"
                    );
                }
                "local_id" => assert_eq!(
                    string(c, "value").parse::<WorkId>().is_ok(),
                    c["valid"].as_bool().unwrap()
                ),
                "work_state" => {
                    let state: WorkState = serde_json::from_value(c["value"].clone()).unwrap();
                    assert_eq!(state.is_terminal(), c["valid"].as_bool().unwrap());
                }
                "content_digest" => {
                    let content = MessageContent::try_new(vec![
                        ContentBlock::text(string(c, "value")).unwrap(),
                    ])
                    .unwrap();
                    assert_eq!(content.content_sha256().to_string(), string(c, "expected"));
                }
                "command_digest" => {
                    let content =
                        MessageContent::try_new(vec![ContentBlock::text("synthetic").unwrap()])
                            .unwrap();
                    let hash = CommandRequestHash::for_message(
                        1,
                        string(c, "value").parse::<ConversationId>().unwrap(),
                        string(c, "value").parse::<ClientMessageId>().unwrap(),
                        &content,
                    );
                    assert_eq!(hash.digest().to_string(), string(c, "expected"));
                }
                _ => panic!("unknown legacy fixture"),
            }
            report["valid"] = json!(true);
        }
        _ => {
            let mut state = c["initial"].clone();
            let mut outputs = Vec::new();
            for step in c["steps"].as_array().unwrap() {
                let before = state.clone();
                let actual = distribution_state_models::transition(
                    string(c, "machine"),
                    &state,
                    &step["event"],
                    &step["context"],
                );
                assert_eq!(state, before, "input mutated: {}", string(c, "case_id"));
                assert!(distribution_schema::valid(
                    "fixture",
                    "Projection",
                    &actual["next"]
                ));
                assert_eq!(actual, step["expected"], "{}", string(c, "case_id"));
                state = actual["next"].clone();
                outputs.push(actual);
            }
            report["state_sha256"] = json!(sha256(&canonical_bytes(&json!(outputs)).unwrap()));
        }
    }
    report
}
fn run_group(group: &str) -> Vec<Value> {
    let envelopes = load("envelopes")["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (string(c, "case_id").to_string(), c.clone()))
        .collect::<BTreeMap<_, _>>();
    load(group)["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| evaluate(group, c, &envelopes))
        .collect()
}
#[test]
fn canonical_and_primitives() {
    for g in [
        "canonical",
        "primitives",
        "raw-invalid",
        "limits",
        "legacy-regression",
    ] {
        run_group(g);
    }
}
#[test]
fn strict_schemas_and_dtos() {
    run_group("envelopes");
}
#[test]
fn actual_ed25519_goldens() {
    run_group("signatures");
}
#[test]
fn version_negotiation() {
    run_group("negotiation");
    assert!(
        !craxii_server::distribution_contracts::compatibility::validate_selection(
            &json!({"major":1,"minor":1}),
            &json!({"major":1,"minor":0})
        )
    );
}
#[test]
fn authorization_epoch_boundaries() {
    let mut count = 0;
    for group in ["authority-lifecycle", "budgets-model"] {
        for case in load(group)["cases"].as_array().unwrap() {
            if string(case, "case_id").contains(".epoch_boundary.") {
                evaluate(group, case, &BTreeMap::new());
                count += 1;
            }
        }
    }
    assert_eq!(count, 6);
}
#[test]
fn state_machine_conformance() {
    for g in [
        "admission-cancel",
        "result-delivery",
        "authority-lifecycle",
        "budgets-model",
        "neutral-policy-lineage",
    ] {
        run_group(g);
    }
}
#[test]
fn integrity_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/distribution/1.0.0");
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    for f in manifest["files"].as_array().unwrap() {
        let bytes = fs::read(root.join(string(f, "path"))).unwrap();
        assert_eq!(bytes.len() as u64, f["bytes"].as_u64().unwrap());
        assert_eq!(sha256(&bytes), string(f, "sha256"));
    }
    let input = json!({"bundle_version":manifest["bundle_version"],"files":manifest["files"]});
    assert_eq!(
        sha256(&canonical_bytes(&input).unwrap()),
        string(&manifest, "bundle_digest")
    );
}
#[test]
fn adversarial_programmatic_and_content_bounds() {
    for v in [
        json!(-1),
        json!(1.1),
        json!(-0.0),
        json!({"é":1}),
        json!({"__proto__":1}),
    ] {
        assert!(canonical_bytes(&v).is_err());
    }
    let excessive = json!({"content":{"version":1,"blocks":[{"type":"text","text":"x".repeat(32769)},{"type":"text","text":"x".repeat(32768)}]}});
    assert!(check_semantics(&excessive).is_err());
    assert!(parse_bounded(b"{} trailing", LARGE_LIMITS).is_err());
}
#[test]
fn cross_language_report() {
    if std::env::var_os("CRAXII_CONTRACT_REPORT").is_none() {
        assert_eq!(GROUPS.len(), 13);
        assert_eq!(
            GROUPS
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            13
        );
        return;
    }
    let mut summaries = GROUPS.iter().flat_map(|g| run_group(g)).collect::<Vec<_>>();
    summaries.sort_by(|a, b| string(a, "case_id").cmp(string(b, "case_id")));
    println!(
        "CONTRACT-REPORT:{}",
        serde_json::to_string(&summaries).unwrap()
    );
}
