#![cfg(all(feature = "test-failpoints", unix))]
use craxii_server::worker_auth::{
    AuthError,
    client::WorkerClient,
    key_store::KeyStore,
    state::{Disposition, IdentityState},
};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn root() -> PathBuf {
    let base = std::env::var_os("CRAXII_WORKER_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
    let p = base.join(format!(
        "worker-auth-test-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    p
}
fn uid() -> u32 {
    // SAFETY: process identity query.
    unsafe { nix::libc::geteuid() }
}
#[test]
fn protected_unique_key_recovers_and_missing_corrupt_or_unsafe_key_never_rotates() {
    let p = root();
    let first = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    let id = first.key_id();
    assert_eq!(format!("{first:?}"), "KeyStore([REDACTED])");
    assert!(KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).is_err());
    drop(first);
    let second = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, false).unwrap();
    assert_eq!(id, second.key_id());
    drop(second);
    fs::remove_file(p.join("worker.pk8")).unwrap();
    assert!(matches!(
        KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true),
        Err(AuthError::KeyUnavailable)
    ));
    fs::write(p.join("worker.pk8"), b"corrupt").unwrap();
    fs::set_permissions(p.join("worker.pk8"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).is_err());
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn symlinks_hardlinks_permissions_same_uid_and_path_redirect_are_denied() {
    let p = root();
    let k = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    drop(k);
    fs::hard_link(p.join("worker.pk8"), p.join("copy.pk8")).unwrap();
    assert!(KeyStore::isolated_fixture(&p, uid(), uid() + 10000, false).is_err());
    fs::remove_file(p.join("copy.pk8")).unwrap();
    fs::set_permissions(p.join("worker.pk8"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(KeyStore::isolated_fixture(&p, uid(), uid() + 10000, false).is_err());
    fs::set_permissions(p.join("worker.pk8"), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(KeyStore::isolated_fixture(&p, uid(), uid(), false).is_err());
    let redirect = p.with_extension("linked");
    symlink(&p, &redirect).unwrap();
    assert!(KeyStore::isolated_fixture(&redirect, uid(), uid() + 10000, false).is_err());
    fs::remove_file(redirect).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(KeyStore::isolated_fixture(&p, uid(), uid() + 10000, false).is_err());
    fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn protected_recovery_state_preserves_identity_epoch_and_terminal_disposition() {
    let p = root();
    let key = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    let mut s = IdentityState {
        version: 1,
        installation_id: "10000000-0000-4000-8000-000000000001".into(),
        customer_id: "20000000-0000-4000-8000-000000000001".into(),
        logical_agent_id: "30000000-0000-4000-8000-000000000001".into(),
        generation: "1".into(),
        key_id: key.key_id(),
        highest_epoch: "2".into(),
        binding_id: None,
        pending_signed: None,
        disposition: Disposition::Bound,
    };
    s.persist(&key).unwrap();
    assert_eq!(IdentityState::load(&key).unwrap().unwrap(), s);
    s.highest_epoch = "1".into();
    assert!(s.persist(&key).is_err());
    s.highest_epoch = "2".into();
    s.disposition = Disposition::Revoked;
    s.persist(&key).unwrap();
    s.disposition = Disposition::Bound;
    assert!(s.persist(&key).is_err());
    drop(key);
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn client_rejects_plaintext_userinfo_query_fragment_and_untrusted_fixture_origin() {
    for origin in [
        "http://localhost/",
        "https://user:secret@localhost/",
        "https://localhost/?token=x",
        "https://localhost/#fragment",
        "https://localhost/other",
    ] {
        let p = root();
        let key = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
        let s = IdentityState {
            version: 1,
            installation_id: "10000000-0000-4000-8000-000000000001".into(),
            customer_id: "20000000-0000-4000-8000-000000000001".into(),
            logical_agent_id: "30000000-0000-4000-8000-000000000001".into(),
            generation: "1".into(),
            key_id: key.key_id(),
            highest_epoch: "1".into(),
            binding_id: None,
            pending_signed: None,
            disposition: Disposition::Unbound,
        };
        assert!(
            WorkerClient::new(
                origin,
                craxii_server::distribution_contracts::environment::WorkerEnvironment::Production,
                key,
                s
            )
            .is_err()
        );
        fs::remove_dir_all(p).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn actual_distinct_model_uid_cannot_read_or_write_worker_key() {
    if uid() != 0 {
        panic!("isolated root Linux fixture required")
    }
    let p = root();
    let key = KeyStore::isolated_fixture(&p, 0, 10001, true).unwrap();
    let original = key.key_id();
    drop(key);
    let read = std::process::Command::new("setpriv")
        .args(["--reuid=10001", "--regid=10001", "--clear-groups", "cat"])
        .arg(p.join("worker.pk8"))
        .output()
        .unwrap();
    assert!(!read.status.success());
    assert!(read.stdout.is_empty());
    let write = std::process::Command::new("setpriv")
        .args(["--reuid=10001", "--regid=10001", "--clear-groups", "touch"])
        .arg(p.join("worker.pk8"))
        .output()
        .unwrap();
    assert!(!write.status.success());
    let recovered = KeyStore::isolated_fixture(&p, 0, 10001, false).unwrap();
    assert_eq!(original, recovered.key_id());
    drop(recovered);
    fs::remove_dir_all(p).unwrap();
}

#[test]
fn first_boot_handoff_is_read_strictly_parsed_strictly_and_removed() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use craxii_server::worker_auth::handoff::EnrollmentHandoff;
    let p = root();
    let key = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    assert!(key.enrollment_handoff().is_err(), "absent");
    key.remove_enrollment_handoff().unwrap();
    let document = br#"{"instanceId":"i-0123456789abcdef0"}"#;
    let signature = [7u8; 128];
    let good = serde_json::json!({
        "contract_version": 1,
        "enrollment_id": "7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93",
        "enrollment_credential": "A".repeat(43),
        "expected_instance_ref": "1f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a94",
        "instance_identity_document_b64": STANDARD.encode(document),
        "instance_identity_signature_b64": STANDARD.encode(signature),
    });
    let file = p.join("enrollment.json");
    fs::write(&file, serde_json::to_vec(&good).unwrap()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let parsed = EnrollmentHandoff::parse(&key.enrollment_handoff().unwrap()).unwrap();
    assert_eq!(parsed.document, document);
    assert_eq!(parsed.signature, signature);
    assert_eq!(
        parsed.expected_instance_ref,
        "1f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a94"
    );
    assert_eq!(format!("{parsed:?}"), "EnrollmentHandoff([REDACTED])");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(key.enrollment_handoff().is_err(), "not 0600");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    for (field, value) in [
        ("contract_version", serde_json::json!(2)),
        ("enrollment_credential", serde_json::json!("short")),
        ("enrollment_id", serde_json::json!("not-a-uuid")),
        (
            "instance_identity_document_b64",
            serde_json::json!("not base64!"),
        ),
        ("instance_identity_signature_b64", serde_json::json!("")),
        ("extra", serde_json::json!(true)),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(
            EnrollmentHandoff::parse(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{field}"
        );
    }
    key.remove_enrollment_handoff().unwrap();
    assert!(!file.exists());
}

#[test]
fn first_boot_identity_starts_unbound_from_the_guest_ids_and_persists() {
    let p = root();
    let key = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    let state = IdentityState::unbound(
        "7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93",
        "2f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a95",
        "3f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a96",
        "1",
        &key,
    );
    assert_eq!(state.disposition, Disposition::Unbound);
    state.persist(&key).unwrap();
    assert_eq!(IdentityState::load(&key).unwrap(), Some(state));
}

#[test]
fn an_interrupted_enrollment_resumes_only_for_the_same_enrollment() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use craxii_server::worker_auth::handoff::{EnrollmentHandoff, settle_pending};
    let p = root();
    let key = KeyStore::isolated_fixture(&p, uid(), uid() + 10000, true).unwrap();
    let handoff = EnrollmentHandoff::parse(
        &serde_json::to_vec(&serde_json::json!({
            "contract_version": 1,
            "enrollment_id": "7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93",
            "enrollment_credential": "A".repeat(43),
            "expected_instance_ref": "1f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a94",
            "instance_identity_document_b64": STANDARD.encode(b"{}"),
            "instance_identity_signature_b64": STANDARD.encode([1u8; 128]),
        }))
        .unwrap(),
    )
    .unwrap();
    let base = IdentityState::unbound(
        "7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93",
        "2f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a95",
        "3f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a96",
        "1",
        &key,
    );
    let mut none = base.clone();
    assert!(!settle_pending(&mut none, &handoff));
    assert_eq!(none, base);
    let mut same = base.clone();
    same.disposition = Disposition::Enrolling;
    same.pending_signed =
        Some(serde_json::json!({"enrollment_id": "7f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a93"}));
    let kept = same.clone();
    assert!(
        settle_pending(&mut same, &handoff),
        "same enrollment resumes"
    );
    assert_eq!(same, kept);
    let mut other = kept.clone();
    other.pending_signed =
        Some(serde_json::json!({"enrollment_id": "9f1c5a2e-9b7d-4e41-8c2a-6d0f4b8e1a97"}));
    assert!(
        !settle_pending(&mut other, &handoff),
        "another enrollment starts afresh"
    );
    assert_eq!(
        (other.pending_signed, other.disposition),
        (None, Disposition::Unbound)
    );
}
