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
        assert!(WorkerClient::new(origin, key, s).is_err());
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
