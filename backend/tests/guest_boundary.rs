use craxii_server::distribution_contracts::canonical::sha256;
use craxii_server::guest_boundary::{
    QuiescenceObservation, QuiescenceState,
    config::{FIXTURES_SHA256, GuestConfig},
    storage::{parse_mounts, verify_mounts},
};
use serde_json::Value;
#[test]
fn guest_contract_corpus_and_canonical_parity() {
    let bytes = include_bytes!("../../ops/v0.0.1/guest-bootstrap-fixtures.json");
    assert_eq!(sha256(bytes), FIXTURES_SHA256);
    let corpus: Value = serde_json::from_slice(bytes).unwrap();
    for case in corpus["cases"].as_array().unwrap() {
        let decoded = GuestConfig::decode(case["json"].as_str().unwrap().as_bytes());
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}",
            case["id"]
        );
        if let Ok(c) = decoded {
            assert_eq!(sha256(&c.canonical().unwrap()), case["canonical_sha256"]);
        }
    }
}
#[test]
fn bounded_parser_holds_invalid_utf8_duplicate_depth_and_size() {
    for bytes in [
        vec![0xff],
        vec![b' '; 16385],
        b"true".to_vec(),
        b"\xef\xbb\xbf{}".to_vec(),
        format!("{}0{}", "[".repeat(17), "]".repeat(17)).into_bytes(),
    ] {
        assert!(GuestConfig::decode(&bytes).is_err());
    }
}
fn mounts() -> Vec<craxii_server::guest_boundary::storage::MountObservation> {
    parse_mounts(b"1 0 8:0 / / rw - ext4 /dev/root rw\n2 1 8:16 / /srv/craxii-data rw,nosuid,nodev - ext4 /dev/data rw\n3 1 8:16 /state /var/lib/craxii rw,nosuid,nodev - ext4 /dev/data rw\n4 1 8:16 /workspaces /srv/craxii/workspaces rw,nosuid,nodev - ext4 /dev/data rw\n5 1 8:16 /home/craxii /home/craxii rw,nosuid,nodev - ext4 /dev/data rw\n").unwrap()
}
#[test]
fn mount_identity_binds_flags_root_and_overmounts() {
    let good = mounts();
    assert!(verify_mounts(&good, "8:16").is_ok());
    assert!(verify_mounts(&good, "8:0").is_err());
    for index in 1..5 {
        let mut m = good.clone();
        m.remove(index);
        assert!(verify_mounts(&m, "8:16").is_err());
        let mut m = good.clone();
        m[index].device = "8:32".into();
        assert!(verify_mounts(&m, "8:16").is_err());
        let mut m = good.clone();
        m[index].root = "/foreign".into();
        assert!(verify_mounts(&m, "8:16").is_err());
        let mut m = good.clone();
        m[index].flags = vec!["rw".into()];
        assert!(verify_mounts(&m, "8:16").is_err());
        let mut m = good.clone();
        m.push(m[index].clone());
        assert!(verify_mounts(&m, "8:16").is_err());
    }
    let mut m = good.clone();
    let mut nested = m[2].clone();
    nested.target = "/var/lib/craxii/db".into();
    m.push(nested);
    assert!(verify_mounts(&m, "8:16").is_err());
    assert!(parse_mounts(&vec![b' '; 1048577]).is_err());
}
#[test]
fn quiescence_never_hides_pending_exports_cleanup_or_incomplete_evidence() {
    let good = QuiescenceObservation {
        active_tasks: 0,
        descendants: Some(0),
        pending_cleanup: 0,
        pending_receipts: 0,
        pending_exports: 0,
        uncertain_exports: 0,
        journal_flushed: true,
        storage_healthy: true,
        visibility_complete: true,
    };
    assert_eq!(good.state(), QuiescenceState::LocallyQuiescent);
    for busy in [
        QuiescenceObservation {
            active_tasks: 1,
            ..good
        },
        QuiescenceObservation {
            descendants: Some(1),
            ..good
        },
        QuiescenceObservation {
            pending_cleanup: 1,
            ..good
        },
        QuiescenceObservation {
            pending_receipts: 1,
            ..good
        },
        QuiescenceObservation {
            pending_exports: 1,
            ..good
        },
        QuiescenceObservation {
            uncertain_exports: 1,
            ..good
        },
    ] {
        assert_eq!(busy.state(), QuiescenceState::Busy);
    }
    for unknown in [
        QuiescenceObservation {
            descendants: None,
            ..good
        },
        QuiescenceObservation {
            journal_flushed: false,
            ..good
        },
        QuiescenceObservation {
            storage_healthy: false,
            ..good
        },
        QuiescenceObservation {
            visibility_complete: false,
            ..good
        },
    ] {
        assert_eq!(unknown.state(), QuiescenceState::Unknown);
    }
}
#[tokio::test]
async fn installed_open_denies_before_canonical_writes() {
    use craxii_server::distribution::startup::{DistributionConfig, DistributionRuntime};
    let root = std::env::temp_dir().join(format!("craxii-guest-denial-{}", uuid::Uuid::now_v7()));
    let config = DistributionConfig {
        contract_version: 1,
        environment: "development".into(),
        origin: "https://invalid.example".into(),
        state_root: root.join("state").display().to_string(),
        artifact_root: root.join("artifacts").display().to_string(),
        workspace_root: root.join("workspace").display().to_string(),
        profile_public_key: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        power_operation_id: "00000001-1111-4111-8111-111111111111".into(),
        release_id: "synthetic".into(),
        release_sha256: "0".repeat(64),
    };
    assert!(DistributionRuntime::open(config).await.is_err());
    assert!(!root.exists());
}
#[test]
fn protected_descriptor_walk_denies_links_and_wrong_modes() {
    use craxii_server::guest_boundary::storage::protected_open;
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("guest-owned-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = root.join("config");
    std::fs::write(&file, b"synthetic").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let uid = unsafe { nix::libc::geteuid() };
    assert!(protected_open(&file, uid, 0o600, false).is_ok());
    assert!(protected_open(&file, uid, 0o644, false).is_err());
    let link = root.join("symlink");
    symlink(&file, &link).unwrap();
    assert!(protected_open(&link, uid, 0o600, false).is_err());
    let hard = root.join("hardlink");
    std::fs::hard_link(&file, &hard).unwrap();
    assert!(protected_open(&file, uid, 0o600, false).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}
