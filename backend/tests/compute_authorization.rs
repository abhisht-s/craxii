use craxii_server::{
    compute::{
        authorization::{Binding, ClockSample, RunningAuthority},
        watchdog::{Action, Policy, Receipt},
    },
    distribution_contracts::{canonical::canonical_bytes, primitives::timestamp_micros},
};
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../ops/v0.0.1/compute-watchdog-fixtures.json"
    ))
    .unwrap()
}
fn policy(v: &Value) -> Policy {
    Policy {
        contract_version: 1,
        binding: serde_json::from_value::<Binding>(v["binding"].clone()).unwrap(),
        drain_ms: 2000,
        stop_ms: 2000,
        skew_ms: 1000,
    }
}
fn clock(seconds: u64) -> ClockSample {
    let v = fixture();
    ClockSample {
        boot_id: v["grant"]["body"]["grant"]["boot_id"]
            .as_str()
            .unwrap()
            .into(),
        ticks_us: seconds * 1_000_000,
        utc_us: timestamp_micros("2026-10-15T00:00:00.000000Z").unwrap()
            + i128::from(seconds) * 1_000_000,
        trusted: true,
    }
}
fn bootstrap(v: &Value, p: &Policy) -> Receipt {
    Receipt::consume_bootstrap(
        &canonical_bytes(&v["bootstrap"]).unwrap(),
        p,
        &clock(0),
        None,
    )
    .unwrap()
}
#[test]
fn typescript_ed25519_grant_and_bootstrap_domains() {
    let v = fixture();
    let p = policy(&v);
    let mut r = bootstrap(&v, &p);
    assert_eq!(r.action(&p, &clock(0)).unwrap(), Action::BootstrapOnly);
    assert!(r.worker_authority(&p, &clock(0)).is_err());
    assert!(
        r.accept(&canonical_bytes(&v["grant"]).unwrap(), &p, &clock(1))
            .unwrap()
    );
    assert_eq!(r.action(&p, &clock(2)).unwrap(), Action::Running);
    let a = r.worker_authority(&p, &clock(2)).unwrap();
    a.check_work(&clock(2), "00000009-1414-4414-8414-141414141414", 500, 5000)
        .unwrap();
    assert!(
        a.check_work(&clock(2), "00000010-1414-4414-8414-141414141414", 500, 5000)
            .is_err()
    );
    assert!(
        a.check_work(
            &clock(2),
            "00000009-1414-4414-8414-141414141414",
            120000,
            5000
        )
        .is_err()
    );
}
#[test]
fn signature_identity_generation_epoch_boot_and_untrusted_clock_refuse() {
    let v = fixture();
    let p = policy(&v);
    let bytes = canonical_bytes(&v["grant"]).unwrap();
    for field in [
        "customer_id",
        "logical_agent_id",
        "installation_id",
        "generation",
        "authorization_epoch",
        "power_operation_id",
        "period_id",
    ] {
        let mut b = v["binding"].clone();
        b[field] = if field == "generation" || field == "authorization_epoch" {
            Value::String("2".into())
        } else {
            Value::String("00000099-1414-4414-8414-141414141414".into())
        };
        assert!(
            RunningAuthority::verify(&bytes, &serde_json::from_value(b).unwrap(), &clock(1))
                .is_err()
        );
    }
    let mut c = clock(1);
    c.boot_id = "00000099-1414-4414-8414-141414141414".into();
    assert!(RunningAuthority::verify(&bytes, &p.binding, &c).is_err());
    c = clock(1);
    c.trusted = false;
    assert!(RunningAuthority::verify(&bytes, &p.binding, &c).is_err());
    let mut bad = v["grant"].clone();
    bad["body"]["grant"]["expires_at"] = "2026-10-15T01:00:00.000000Z".into();
    assert!(
        RunningAuthority::verify(&canonical_bytes(&bad).unwrap(), &p.binding, &clock(1)).is_err()
    );
}
#[test]
fn restart_and_replayed_response_never_refresh_deadline() {
    let v = fixture();
    let p = policy(&v);
    let mut r = bootstrap(&v, &p);
    let bytes = canonical_bytes(&v["grant"]).unwrap();
    r.accept(&bytes, &p, &clock(1)).unwrap();
    let cutoff = r.deadline.ticks_cutoff_us;
    assert!(!r.accept(&bytes, &p, &clock(30)).unwrap());
    assert_eq!(r.deadline.ticks_cutoff_us, cutoff);
    let restored = Receipt::parse(&serde_json::to_vec(&r).unwrap(), &p, &clock(40)).unwrap();
    assert_eq!(restored.deadline.ticks_cutoff_us, cutoff);
    assert_eq!(restored.action(&p, &clock(118)).unwrap(), Action::Poweroff);
    assert_eq!(restored.action(&p, &clock(122)).unwrap(), Action::Poweroff);
}
#[test]
fn funded_successor_extends_once_and_drain_is_irreversible() {
    let v = fixture();
    let p = policy(&v);
    let mut r = bootstrap(&v, &p);
    r.accept(&canonical_bytes(&v["grant"]).unwrap(), &p, &clock(1))
        .unwrap();
    let old = r.deadline.ticks_cutoff_us;
    assert!(
        r.accept(&canonical_bytes(&v["renewal"]).unwrap(), &p, &clock(2))
            .unwrap()
    );
    assert!(r.deadline.ticks_cutoff_us > old);
    assert!(
        !r.accept(&canonical_bytes(&v["renewal"]).unwrap(), &p, &clock(3))
            .unwrap()
    );
    assert!(
        r.accept(&canonical_bytes(&v["grant"]).unwrap(), &p, &clock(4))
            .is_err()
    );
    r.deadline.draining = true;
    assert!(
        r.accept(&canonical_bytes(&v["renewal"]).unwrap(), &p, &clock(5))
            .is_err()
    );
    assert!(r.worker_authority(&p, &clock(5)).is_err());
}
#[test]
fn one_use_permit_wrong_boot_corruption_and_clock_changes_fail_closed() {
    let v = fixture();
    let p = policy(&v);
    let r = bootstrap(&v, &p);
    assert!(
        Receipt::consume_bootstrap(
            &canonical_bytes(&v["bootstrap"]).unwrap(),
            &p,
            &clock(0),
            Some(&r)
        )
        .is_err()
    );
    let mut c = clock(5);
    c.boot_id = "00000099-1414-4414-8414-141414141414".into();
    assert!(Receipt::parse(&serde_json::to_vec(&r).unwrap(), &p, &c).is_err());
    assert!(Receipt::parse(b"{}", &p, &clock(5)).is_err());
    let mut c = clock(20);
    c.utc_us = clock(2).utc_us;
    assert_eq!(r.action(&p, &c).unwrap(), Action::Poweroff);
    c = clock(20);
    c.utc_us = clock(500).utc_us;
    assert_eq!(r.action(&p, &c).unwrap(), Action::Poweroff);
}
#[test]
fn strict_bounded_parsing_rejects_duplicate_unknown_and_oversize() {
    let v = fixture();
    let p = policy(&v);
    let bytes = canonical_bytes(&v["grant"]).unwrap();
    let text = String::from_utf8(bytes).unwrap().replacen(
        "\"grant_version\":1",
        "\"grant_version\":1,\"grant_version\":1",
        1,
    );
    assert!(RunningAuthority::verify(text.as_bytes(), &p.binding, &clock(1)).is_err());
    assert!(RunningAuthority::verify(&vec![b' '; 16385], &p.binding, &clock(1)).is_err());
    let mut invalid = v["bootstrap"].clone();
    invalid["permit"]["extra"] = "ignored".into();
    assert!(
        Receipt::consume_bootstrap(&canonical_bytes(&invalid).unwrap(), &p, &clock(0), None)
            .is_err()
    );
}
#[test]
fn fresh_boot_requires_new_signed_power_authority_and_retains_consumed_history() {
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let v = fixture();
    let old_policy = policy(&v);
    let old = bootstrap(&v, &old_policy);
    let old = Receipt::previous(&serde_json::to_vec(&old).unwrap()).unwrap();
    let key = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let mut p = old_policy.clone();
    p.binding.public_key = URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
    p.binding.power_operation_id = "00000098-1414-4414-8414-141414141414".into();
    let mut permit = v["bootstrap"]["permit"].clone();
    permit["permit_id"] = "00000097-1414-4414-8414-141414141414".into();
    permit["power_operation_id"] = p.binding.power_operation_id.clone().into();
    permit["issued_at"] = "2026-10-15T00:00:03.000000Z".into();
    permit["expires_at"] = "2026-10-15T00:02:03.000000Z".into();
    let mut preimage = b"CRAXII-COMPUTE-BOOTSTRAP\x001\x00".to_vec();
    preimage.extend(canonical_bytes(&permit).unwrap());
    let envelope = serde_json::json!({"permit":permit,"key_id":craxii_server::distribution_contracts::canonical::sha256(key.public_key().as_ref()),"signature":URL_SAFE_NO_PAD.encode(key.sign(&preimage).as_ref())});
    let mut c = clock(3);
    c.boot_id = "00000096-1414-4414-8414-141414141414".into();
    c.ticks_us = 0;
    let receipt =
        Receipt::consume_bootstrap(&canonical_bytes(&envelope).unwrap(), &p, &c, Some(&old))
            .unwrap();
    assert_eq!(receipt.consumed_permits.len(), 2);
    assert!(receipt.grant_id.is_none());
    assert!(
        Receipt::consume_bootstrap(&canonical_bytes(&envelope).unwrap(), &p, &c, Some(&receipt))
            .is_err()
    );
}
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the explicitly owned offline guest and protected root receipt"]
fn installed_root_receipt() {
    use craxii_server::compute::watchdog::protected_read;
    use std::path::Path;
    assert_eq!(
        std::fs::read_to_string("/etc/craxii-v001-fixture")
            .unwrap()
            .trim(),
        "craxii-v001-guest-boundary-disposable"
    );
    assert_eq!(unsafe { nix::libc::geteuid() }, 0);
    let bytes = protected_read(Path::new("/var/lib/craxii-compute/receipt.json"), 0)
        .expect("root receipt file protection");
    let r = Receipt::previous(&bytes).expect("restart receipt cryptographic restoration");
    let c = ClockSample {
        boot_id: r.deadline.boot_id.clone(),
        ticks_us: r.deadline.sampled_ticks_us,
        utc_us: r.deadline.sampled_utc_us.parse().unwrap(),
        trusted: true,
    };
    let p =
        Policy::parse(&protected_read(Path::new("/etc/craxii/compute-watchdog.json"), 0).unwrap())
            .unwrap();
    Receipt::parse(&bytes, &p, &c).expect("same policy receipt restoration");
}

#[test]
fn signed_inner_grant_does_not_authorize_a_foreign_outer_owner() {
    let v = fixture();
    for field in [
        "customer_id",
        "logical_agent_id",
        "installation_id",
        "generation",
    ] {
        let mut message = v["grant"].clone();
        message["authority"][field] = if field == "generation" {
            "2".into()
        } else {
            "00000099-1414-4414-8414-141414141414".into()
        };
        assert!(
            RunningAuthority::verify(
                &canonical_bytes(&message).unwrap(),
                &policy(&v).binding,
                &clock(1)
            )
            .is_err()
        );
    }
}

// Real signed grants exercise the receipt shared by installed worker and root watchdog.
struct RenewalFixture {
    key: aws_lc_rs::signature::Ed25519KeyPair,
    p: Policy,
    v: Value,
}
impl RenewalFixture {
    fn new() -> Self {
        use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let key = Ed25519KeyPair::from_pkcs8(
            Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new())
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let v = fixture();
        let mut p = policy(&v);
        p.drain_ms = 45000;
        p.stop_ms = 60000;
        p.skew_ms = 5000;
        p.binding.public_key = URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
        Self { key, p, v }
    }
    fn time(seconds: u64) -> String {
        format!(
            "2026-10-15T{:02}:{:02}:{:02}.000000Z",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    }
    fn envelope(&self, value: Value, bootstrap: bool) -> Vec<u8> {
        use aws_lc_rs::signature::KeyPair;
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use craxii_server::distribution_contracts::{
            canonical::sha256,
            signing::{SignaturePurpose, signature_input},
        };
        let preimage = if bootstrap {
            let mut bytes = b"CRAXII-COMPUTE-BOOTSTRAP\x001\x00".to_vec();
            bytes.extend(canonical_bytes(&value).unwrap());
            bytes
        } else {
            signature_input(SignaturePurpose::ComputeAuthorization, &value).unwrap()
        };
        let signature = URL_SAFE_NO_PAD.encode(self.key.sign(&preimage).as_ref());
        let key_id = sha256(self.key.public_key().as_ref());
        if bootstrap {
            canonical_bytes(
                &serde_json::json!({"permit":value,"key_id":key_id,"signature":signature}),
            )
            .unwrap()
        } else {
            let mut message = self.v["grant"].clone();
            message["body"] =
                serde_json::json!({"grant":value,"key_id":key_id,"signature":signature});
            canonical_bytes(&message).unwrap()
        }
    }
    fn start(&self) -> Receipt {
        Receipt::consume_bootstrap(
            &self.envelope(self.v["bootstrap"]["permit"].clone(), true),
            &self.p,
            &clock(0),
            None,
        )
        .unwrap()
    }
    fn grant(&self, n: u64, issued: u64, expires: u64) -> Vec<u8> {
        let mut g = self.v["grant"]["body"]["grant"].clone();
        g["grant_id"] = format!("{:08}-1414-4414-8414-141414141414", 100 + n).into();
        g["compute_reservation_id"] = format!("{:08}-1414-4414-8414-141414141414", 200 + n).into();
        g["issued_at"] = Self::time(issued).into();
        g["expires_at"] = Self::time(expires).into();
        g["max_elapsed_ms"] = ((expires - issued) * 1000).to_string().into();
        self.envelope(g, false)
    }
}
fn shifted(ticks: u64, offset_us: i128) -> ClockSample {
    let mut c = clock(ticks);
    c.utc_us += offset_us;
    c
}
fn restart(r: &Receipt, p: &Policy, c: &ClockSample) -> Receipt {
    Receipt::parse(&serde_json::to_vec(r).unwrap(), p, c).unwrap()
}
#[test]
fn original_700_to_712_challenge_is_contained_for_worker_and_watchdog() {
    let f = RenewalFixture::new();
    for rollback in [false, true] {
        let mut r = f.start();
        for (n, issued, expires, ticks, utc) in [
            (0, 1, 310, 1, 1),
            (1, 100, 440, 104, 100),
            (2, 230, 570, 238, 230),
            (3, 360, 700, 372, 360),
        ] {
            let c = if rollback {
                shifted(ticks, -(i128::from(ticks - utc) * 1_000_000))
            } else {
                clock(utc)
            };
            r = restart(&r, &f.p, &c); // watchdog process restart before every response
            if rollback && n >= 2 {
                assert!(r.accept(&f.grant(n, issued, expires), &f.p, &c).is_err());
                assert_eq!(r.action(&f.p, &c).unwrap(), Action::Poweroff);
                assert!(r.worker_authority(&f.p, &c).is_err());
            } else {
                assert!(r.accept(&f.grant(n, issued, expires), &f.p, &c).unwrap());
                assert_eq!(r.deadline.ticks_cutoff_us, expires * 1_000_000);
            }
        }
        let c = shifted(595, if rollback { -12_000_000 } else { 0 });
        assert!(r.worker_authority(&f.p, &c).is_err());
        assert_eq!(
            r.action(&f.p, &shifted(635, if rollback { -12_000_000 } else { 0 }))
                .unwrap(),
            Action::Poweroff
        );
    }
}
#[test]
fn arbitrary_bounded_clock_sequences_never_mint_time() {
    let f = RenewalFixture::new();
    let mut sequences = vec![
        vec![-4_000_000, -8_000_000, -12_000_000],
        vec![4_000_000, 8_000_000, 12_000_000],
        vec![4_000_000, 0, 4_000_000, 0],
        vec![-30_000_000],
        vec![30_000_000],
        (1..=100).map(|n| -n * 100_000).collect(),
    ];
    // Deterministic many small/alternating corrections, with restarts and delayed duplicates.
    let mut seed = 14_u64;
    for _ in 0..32 {
        let mut offset = 0;
        sequences.push(
            (0..64)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    offset += ((seed >> 32) as i128 % 200_001) - 100_000;
                    offset
                })
                .collect(),
        );
    }
    for offsets in sequences {
        let mut r = f.start();
        r.accept(&f.grant(0, 1, 310), &f.p, &clock(1)).unwrap();
        let mut high = 0_i128;
        let mut last_end = 310;
        for (index, offset) in offsets.into_iter().enumerate() {
            let ticks = 100 + index as u64 * 2;
            let c = shifted(ticks, offset);
            r = restart(&r, &f.p, &c);
            let bytes = f.grant(index as u64 + 1, ticks - 40, 440 + index as u64 * 130);
            if r.action(&f.p, &c).unwrap() == Action::Poweroff {
                assert!(r.accept(&bytes, &f.p, &c).is_err());
                assert!(r.worker_authority(&f.p, &c).is_err());
                break;
            }
            assert!(r.accept(&bytes, &f.p, &c).unwrap());
            high = high.max(offset);
            last_end = 440 + index as u64 * 130;
            assert!(
                i128::from(r.deadline.ticks_cutoff_us) <= i128::from(last_end) * 1_000_000 - high
            );
            let deadline = r.deadline.ticks_cutoff_us;
            assert!(!r.accept(&bytes, &f.p, &c).unwrap());
            assert_eq!(r.deadline.ticks_cutoff_us, deadline);
            let a = r.worker_authority(&f.p, &c).unwrap(); // worker restart revalidates signature + root receipt
            a.check_work(&c, "00000009-1414-4414-8414-141414141414", 1000, 110000)
                .unwrap();
        }
        assert!(r.deadline.ticks_cutoff_us <= last_end * 1_000_000);
    }
}
#[test]
fn forward_observation_and_crash_recovery_keep_the_tighter_horizon() {
    let f = RenewalFixture::new();
    let mut r = f.start();
    r.accept(&f.grant(0, 1, 310), &f.p, &clock(1)).unwrap();
    assert!(r.deadline.observe(&shifted(10, 4_000_000)).unwrap());
    assert_eq!(r.deadline.ticks_cutoff_us, 306_000_000);
    r = restart(&r, &f.p, &shifted(11, 0));
    assert!(
        !r.accept(&f.grant(0, 1, 310), &f.p, &shifted(12, 0))
            .unwrap()
    );
    assert_eq!(r.deadline.ticks_cutoff_us, 306_000_000);
    assert!(r.accept(&f.grant(1, 100, 440), &f.p, &clock(100)).unwrap());
    assert_eq!(r.deadline.ticks_cutoff_us, 436_000_000);
    let saved = serde_json::to_vec(&r).unwrap();
    // Crash before publishing a successor leaves the old durable bound; retry commits once.
    let mut recovered = Receipt::parse(&saved, &f.p, &clock(230)).unwrap();
    let response = f.grant(2, 230, 570);
    recovered.accept(&response, &f.p, &clock(230)).unwrap();
    recovered = restart(&recovered, &f.p, &clock(231));
    assert!(!recovered.accept(&response, &f.p, &clock(232)).unwrap());
    assert_eq!(recovered.deadline.ticks_cutoff_us, 566_000_000);
    assert!(
        recovered
            .accept(&f.grant(1, 100, 440), &f.p, &clock(233))
            .is_err()
    );
    assert!(recovered.worker_authority(&f.p, &clock(456)).is_err());
    assert_eq!(
        recovered.action(&f.p, &clock(501)).unwrap(),
        Action::Poweroff
    );
}
#[test]
fn correction_before_after_renewal_and_at_expiry_closes_all_gates() {
    let f = RenewalFixture::new();
    for correction_before in [true, false] {
        let mut r = f.start();
        r.accept(&f.grant(0, 1, 310), &f.p, &clock(1)).unwrap();
        let c = shifted(100, if correction_before { -4_000_000 } else { 0 });
        r.accept(&f.grant(1, 90, 440), &f.p, &c).unwrap();
        let c = shifted(105, -4_000_000);
        r = restart(&r, &f.p, &c);
        assert!(!r.accept(&f.grant(1, 90, 440), &f.p, &c).unwrap());
        assert_eq!(r.deadline.ticks_cutoff_us, 440_000_000);
        let a = r.worker_authority(&f.p, &c).unwrap();
        // A queued permit is rechecked when execution actually starts.
        assert!(
            a.check_work(
                &shifted(330, -4_000_000),
                "00000009-1414-4414-8414-141414141414",
                1,
                110000
            )
            .is_err()
        );
        assert_eq!(
            r.action(&f.p, &shifted(375, -4_000_000)).unwrap(),
            Action::Poweroff
        );
        assert!(
            r.accept(&f.grant(2, 370, 570), &f.p, &shifted(440, -4_000_000))
                .is_err()
        );
        assert!(r.worker_authority(&f.p, &shifted(440, -4_000_000)).is_err());
    }
}
#[test]
fn legacy_or_corrupt_projection_cannot_resume_but_permit_history_survives() {
    let f = RenewalFixture::new();
    let r = f.start();
    let mut legacy = serde_json::to_value(&r).unwrap();
    legacy["contract_version"] = 1.into();
    legacy["deadline"]["ticks_cutoff_us"] = r.deadline.ticks_cutoff_us.into();
    legacy["deadline"]["sampled_ticks_us"] = r.deadline.sampled_ticks_us.into();
    legacy["deadline"]
        .as_object_mut()
        .unwrap()
        .remove("projection_offset_us");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    assert!(Receipt::parse(&bytes, &f.p, &clock(1)).is_err());
    let history = Receipt::previous(&bytes).unwrap();
    assert_eq!(history.consumed_permits, r.consumed_permits);
    assert_eq!(history.action(&f.p, &clock(1)).unwrap(), Action::Poweroff);
    assert!(history.worker_authority(&f.p, &clock(1)).is_err());
    assert!(
        Receipt::consume_bootstrap(
            &f.envelope(f.v["bootstrap"]["permit"].clone(), true),
            &f.p,
            &clock(1),
            Some(&history)
        )
        .is_err()
    );
    for offset in [
        Value::Null,
        "invalid".into(),
        (clock(0).utc_us - 1).to_string().into(),
    ] {
        let mut invalid = serde_json::to_value(&r).unwrap();
        invalid["deadline"]["projection_offset_us"] = offset;
        assert!(Receipt::parse(&serde_json::to_vec(&invalid).unwrap(), &f.p, &clock(1)).is_err());
    }
}

#[test]
fn later_signed_response_can_reuse_a_funded_interval_without_adding_time() {
    let f = RenewalFixture::new();
    let mut r = f.start();
    let first = f.grant(0, 1, 310);
    r.accept(&first, &f.p, &clock(1)).unwrap();
    let mut grant = f.v["grant"]["body"]["grant"].clone();
    grant["grant_id"] = "00000300-1414-4414-8414-141414141414".into();
    grant["compute_reservation_id"] = "00000200-1414-4414-8414-141414141414".into();
    grant["issued_at"] = RenewalFixture::time(90).into();
    grant["expires_at"] = RenewalFixture::time(310).into();
    grant["max_elapsed_ms"] = "220000".into();
    let response = f.envelope(grant, false);
    assert!(
        r.accept(&response, &f.p, &shifted(104, -4_000_000))
            .unwrap()
    );
    assert_eq!(r.deadline.ticks_cutoff_us, 310_000_000);
    r = restart(&r, &f.p, &shifted(105, -4_000_000));
    assert!(
        !r.accept(&response, &f.p, &shifted(106, -4_000_000))
            .unwrap()
    );
    assert!(r.accept(&first, &f.p, &shifted(107, -4_000_000)).is_err());
    assert_eq!(r.deadline.ticks_cutoff_us, 310_000_000);
}

#[test]
fn signed_elapsed_bound_and_long_boot_ticks_survive_renewal_and_restart() {
    let f = RenewalFixture::new();
    let mut r = f.start();
    let bytes = f.grant(0, 1, 5000);
    let mut grant: Value = serde_json::from_slice(&bytes).unwrap();
    grant["body"]["grant"]["max_elapsed_ms"] = "3999000".into();
    let bytes = f.envelope(grant["body"]["grant"].clone(), false);
    r.accept(&bytes, &f.p, &clock(1)).unwrap();
    assert_eq!(r.deadline.ticks_cutoff_us, 4_000_000_000);
    r = restart(&r, &f.p, &clock(3000));
    assert!(
        r.accept(&f.grant(1, 3000, 6000), &f.p, &clock(3000))
            .unwrap()
    );
    r = restart(&r, &f.p, &clock(3001));
    assert_eq!(r.deadline.ticks_cutoff_us, 6_000_000_000);
    r.worker_authority(&f.p, &clock(3001))
        .unwrap()
        .check_work(
            &clock(3001),
            "00000009-1414-4414-8414-141414141414",
            1000,
            110000,
        )
        .unwrap();
    let mut invalid = serde_json::to_value(&r).unwrap();
    invalid["deadline"]["sampled_ticks_us"] = 1.into();
    assert!(Receipt::parse(&serde_json::to_vec(&invalid).unwrap(), &f.p, &clock(3001)).is_err());
}
