//! Synthetic pure conformance projections only; not production authority or persistence.
use craxii_server::distribution_contracts::{
    canonical::worst_case_cost,
    primitives::{decimal, timestamp_micros},
};
use serde_json::{Value, json};
fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().expect("typed fixture string")
}
fn n(v: &Value, k: &str) -> i128 {
    decimal(s(v, k), "D0").expect("bounded fixture decimal")
}
fn terminal(state: &Value) -> bool {
    matches!(
        s(state, "work_state"),
        "completed" | "failed" | "cancelled" | "interrupted"
    )
}
fn fresh(c: &Value) -> bool {
    let start = timestamp_micros(s(c, "issued_at")).unwrap();
    let end = timestamp_micros(s(c, "expires_at")).unwrap();
    let now = timestamp_micros(s(c, "now")).unwrap();
    start < end
        && start <= now
        && now < end
        && end - start <= decimal(s(c, "max_ttl_ms"), "Dp").unwrap() * 1000
}
fn outcome(state: &Value, disposition: &str, commits: &[&str], kind: &str) -> Value {
    json!({"outcome":kind,"disposition":disposition,"next":state,"required_commits":commits,"forbidden_effects":["duplicate_execution","duplicate_outbox","unfunded_charge","blind_resend","authority_from_fixture"]})
}
fn rejection(state: &Value, reason: &str) -> Value {
    let (code, disposition) = match reason {
        "schema_invalid" => ("invalid_contract", "reject"),
        "unauthenticated" | "session_expired" => ("ownership_denied", "reauthenticate"),
        "customer_mismatch"
        | "installation_mismatch"
        | "stale_generation"
        | "unauthorized_scope"
        | "revoked_authority" => ("ownership_denied", "reject"),
        "stale_lease" | "stale_resource_fence" | "stale_resource_revision" => {
            ("lease_lost", "reconcile")
        }
        "digest_conflict" | "result_conflict" => ("operation_identity_conflict", "reject"),
        "restored_interval_missing" | "reconciliation_required" | "provider_effect_unknown" => {
            ("outcome_unknown", "reconcile")
        }
        "entitlement_expired"
        | "compute_authorization_expired"
        | "authorization_epoch_stale"
        | "model_budget_unavailable"
        | "policy_unapproved"
        | "proof_expired"
        | "consent_required" => ("dispatch_disabled", "hold"),
        "terminal_cancellation" => ("dispatch_disabled", "terminal"),
        "manual_resolution" => ("support_needed", "support"),
        _ => panic!("unclassified fixture rejection"),
    };
    let mut result = outcome(state, disposition, &[], "rejected");
    result["code"] = json!(code);
    result["reason"] = json!(reason);
    result
}
pub fn transition(machine: &str, initial: &Value, event: &Value, c: &Value) -> Value {
    let mut state = initial.clone();
    let kind = s(event, "kind");
    macro_rules! reject {
        ($r:expr) => {
            return rejection(initial, $r)
        };
    }
    macro_rules! accept{($($c:expr),* $(,)?)=>{return outcome(&state,"terminal",&[$($c),*],"allowed")}}
    macro_rules! noop {
        () => {
            return outcome(&state, "terminal", &[], "no_op")
        };
    }
    macro_rules! set {
        ($k:expr,$v:expr) => {
            state[$k] = json!($v)
        };
    }
    if c["authenticated"] != true {
        reject!("unauthenticated");
    }
    if c["customer_id"] != c["expected_customer_id"]
        || c["logical_agent_id"] != c["expected_agent_id"]
    {
        reject!("customer_mismatch");
    }
    if c["job_fence"] != c["expected_job_fence"] {
        reject!("stale_lease");
    }
    if c["resource_fence"] != c["expected_resource_fence"] {
        reject!("stale_resource_fence");
    }
    if c["resource_revision"] != c["expected_resource_revision"] {
        reject!("stale_resource_revision");
    }
    if matches!(
        machine,
        "admission"
            | "receipt"
            | "execution"
            | "cancellation"
            | "export"
            | "acceptance"
            | "enrollment"
            | "power"
            | "compute"
            | "spend"
            | "model_stream"
            | "profile"
    ) {
        if c["principal"] != "worker" {
            reject!("unauthorized_scope");
        }
        if c["installation_id"] != state["installation_id"] {
            reject!("installation_mismatch");
        }
        if c["generation"] != state["generation"] {
            reject!("stale_generation");
        }
    }
    match (machine, kind) {
        ("admission" | "receipt", "restore_gap") => {
            set!("restored_missing", true);
            set!("status", "reconciliation_required");
            return outcome(&state, "reconcile", &["recovery_hold"], "allowed");
        }
        ("admission" | "receipt", "deliver" | "ack") => {
            if event["id"] != state["work_id"] || event["digest"] != state["material_digest"] {
                reject!("digest_conflict");
            }
            if state["restored_missing"] == true {
                reject!("restored_interval_missing");
            }
            if state["status"] == "cancelled_before_admission" {
                noop!();
            }
            if state["receipt_id"] != "" {
                if kind == "ack" && state["status"] != "centrally_recorded" {
                    set!("status", "centrally_recorded");
                    accept!("central_receipt");
                }
                noop!();
            }
            if !fresh(c) {
                reject!("session_expired");
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            if c["request_proof"] != true {
                reject!("unauthorized_scope");
            }
            set!("receipt_id", "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c01");
            set!("local_work_id", "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c02");
            set!("work_state", "queued");
            set!("status", "receipt_pending");
            accept!("local_receipt_and_work");
        }
        ("execution" | "cancellation", "cancel") => {
            if terminal(&state) {
                noop!();
            }
            match s(&state, "work_state") {
                "absent" => {
                    set!("status", "cancelled_before_admission");
                    accept!("cancel_tombstone");
                }
                "queued" => {
                    set!("work_state", "cancelled");
                    accept!("terminal_cancel");
                }
                "cancel_requested" => {
                    noop!();
                }
                _ => {
                    set!("work_state", "cancel_requested");
                    accept!("cancel_intent");
                }
            }
        }
        ("execution" | "cancellation", "complete") => {
            if terminal(&state) {
                noop!();
            }
            if state["work_state"] == "cancel_requested" {
                reject!("terminal_cancellation");
            }
            if !matches!(s(&state, "work_state"), "running" | "waiting_on_model") {
                reject!("schema_invalid");
            }
            set!("work_state", "completed");
            accept!("terminal_result_and_export");
        }
        ("execution" | "cancellation", "start") => {
            if terminal(&state) || state["work_state"] == "cancel_requested" {
                reject!("terminal_cancellation");
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            if !fresh(c) {
                reject!("compute_authorization_expired");
            }
            if state["work_state"] != "queued" {
                reject!("reconciliation_required");
            }
            set!("work_state", "running");
            set!(
                "execution_count",
                state["execution_count"].as_u64().unwrap() + 1
            );
            accept!("execution_claim");
        }
        ("execution" | "cancellation", "crash") => {
            if !matches!(
                s(&state, "work_state"),
                "running" | "waiting_on_model" | "waiting_on_tool" | "cancel_requested"
            ) {
                noop!();
            }
            set!("work_state", "interrupted");
            return outcome(&state, "reconcile", &["recovery_interruption"], "allowed");
        }
        ("execution" | "cancellation", "cleanup_unknown") => {
            set!("work_state", "interrupted");
            return outcome(&state, "reconcile", &["cleanup_uncertain"], "allowed");
        }
        ("export", "prepare") => {
            if !terminal(&state) {
                reject!("schema_invalid");
            }
            if state["export_id"] != "" {
                if event["id"] != state["export_id"] || event["digest"] != state["export_digest"] {
                    reject!("result_conflict");
                }
                noop!();
            }
            state["export_id"] = event["id"].clone();
            state["export_digest"] = event["digest"].clone();
            set!("status", "export_pending");
            accept!("terminal_result_and_export");
        }
        ("export", "crash") => {
            return outcome(&state, "query_same_identity", &[], "no_op");
        }
        ("export", "transfer") => {
            if state["export_id"] == "" || event["digest"] != state["export_digest"] {
                reject!("result_conflict");
            }
            set!("status", "transferred");
            accept!("transfer_ack");
        }
        ("acceptance", "accept") => {
            if event["id"] != state["export_id"] || event["digest"] != state["export_digest"] {
                reject!("result_conflict");
            }
            if state["restored_missing"] == true {
                reject!("restored_interval_missing");
            }
            if state["status"] == "accepted" {
                noop!();
            }
            if c["lineage_authorized"] != true {
                reject!("unauthorized_scope");
            }
            set!("status", "accepted");
            set!("acceptance_id", "11111111-1111-4111-8111-111111111107");
            set!("outbox_source_id", "11111111-1111-4111-8111-111111111108");
            set!("outbox_count", state["outbox_count"].as_u64().unwrap() + 1);
            accept!("central_import_and_outbox");
        }
        ("telegram", "revoke_route") => {
            set!("route_revoked", true);
            accept!("route_revocation");
        }
        ("telegram", "dispatch") => {
            if state["route_revoked"] == true {
                reject!("unauthorized_scope");
            }
            if matches!(s(&state, "status"), "outcome_unknown" | "permanent_failure") {
                reject!("provider_effect_unknown");
            }
            if !matches!(s(&state, "status"), "queued" | "retry_wait") {
                reject!("reconciliation_required");
            }
            set!(
                "dispatch_count",
                state["dispatch_count"].as_u64().unwrap() + 1
            );
            set!("status", "dispatching");
            accept!("send_attempt_before_io");
        }
        ("telegram", "timeout" | "crash") => {
            if state["status"] != "dispatching" {
                reject!("schema_invalid");
            }
            set!("status", "outcome_unknown");
            return outcome(&state, "reconcile", &["uncertain_send"], "allowed");
        }
        ("telegram", "provider_ack") => {
            if c["provider_verified"] != true
                || !matches!(s(&state, "status"), "dispatching" | "outcome_unknown")
            {
                reject!("unauthorized_scope");
            }
            set!("status", "accepted");
            accept!("provider_acceptance");
        }
        ("telegram", "retry") => {
            if state["status"] == "outcome_unknown" {
                reject!("provider_effect_unknown");
            }
            if c["no_effect"] != true {
                reject!("reconciliation_required");
            }
            set!("status", "retry_wait");
            return outcome(
                &state,
                "retry_same_identity",
                &["proven_nonacceptance"],
                "allowed",
            );
        }
        ("telegram", "next_part") => {
            if state["status"] != "accepted" {
                reject!("provider_effect_unknown");
            }
            set!("status", "queued");
            accept!("ordered_next_part");
        }
        ("provisioning", "create") => {
            if state["status"] == "outcome_unknown" {
                reject!("provider_effect_unknown");
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            set!("status", "provisioning");
            set!(
                "dispatch_count",
                state["dispatch_count"].as_u64().unwrap() + 1
            );
            accept!("allocation_intent");
        }
        ("provisioning", "timeout") => {
            set!("status", "outcome_unknown");
            return outcome(&state, "reconcile", &["allocation_uncertain"], "allowed");
        }
        ("provisioning", "cloud_running") => {
            set!("status", "enrolled");
            accept!("power_observation");
        }
        ("provisioning", "ready") => {
            if c["storage_recovered"] != true
                || c["binds_verified"] != true
                || c["model_qualified"] != true
            {
                reject!("reconciliation_required");
            }
            set!("status", "ready");
            accept!("current_boot_readiness");
        }
        ("enrollment", "replace") => {
            if n(event, "generation") <= n(&state, "generation") {
                reject!("stale_generation");
            }
            let Some(epoch) = i64::try_from(n(&state, "epoch"))
                .ok()
                .and_then(|epoch| epoch.checked_add(1))
            else {
                reject!("manual_resolution");
            };
            state["generation"] = event["generation"].clone();
            state["installation_id"] = event["installation_id"].clone();
            set!("epoch", epoch.to_string());
            set!("status", "expected");
            set!("nonce_consumed", false);
            accept!("generation_and_revocation");
        }
        ("enrollment", "proof") => {
            if state["nonce_consumed"] == true {
                reject!("revoked_authority");
            }
            if !fresh(c) {
                reject!("session_expired");
            }
            if c["request_proof"] != true || c["provider_verified"] != true {
                reject!("unauthorized_scope");
            }
            set!("nonce_consumed", true);
            set!("status", "key_bound");
            accept!("nonce_and_key_binding");
        }
        ("enrollment", "session") => {
            if state["status"] != "key_bound" || c["request_proof"] != true {
                reject!("unauthorized_scope");
            }
            set!("status", "session");
            accept!("bounded_session");
        }
        ("power", "heartbeat") => {
            if c["boot_id"] != state["boot_id"] {
                reject!("revoked_authority");
            }
            if c["storage_recovered"] != true
                || c["binds_verified"] != true
                || c["model_qualified"] != true
            {
                reject!("reconciliation_required");
            }
            set!("status", "ready");
            accept!("current_boot_readiness");
        }
        ("power", "drain") => {
            if c["claims_closed"] != true
                || c["active_count"] != 0
                || c["pending_count"] != 0
                || c["descendants"] != 0
                || c["uncommitted"] == true
            {
                reject!("reconciliation_required");
            }
            set!("status", "drained");
            accept!("drain_barrier");
        }
        ("power", "ingress") => {
            state["ingress_revision"] = event["ingress_revision"].clone();
            set!(
                "status",
                if state["status"] == "stop_dispatched" {
                    "wake_after_stop"
                } else {
                    "ready"
                }
            );
            accept!("ingress_and_wake");
        }
        ("power", "stop") => {
            if state["status"] != "drained"
                || event["ingress_revision"] != state["ingress_revision"]
            {
                reject!("stale_resource_revision");
            }
            set!("status", "stop_dispatched");
            set!(
                "dispatch_count",
                state["dispatch_count"].as_u64().unwrap() + 1
            );
            accept!("stop_intent");
        }
        ("power", "unreachable") => {
            reject!("reconciliation_required");
        }
        ("compute", "authorize") => {
            if c["boot_id"] != state["boot_id"] {
                reject!("installation_mismatch");
            }
            if c["epoch"] != state["epoch"] || state["status"] == "revoked" {
                reject!("authorization_epoch_stale");
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            if !fresh(c) {
                reject!("compute_authorization_expired");
            }
            set!("status", "active");
            accept!("funded_grant");
        }
        ("compute", "expire") => {
            set!("status", "expired");
            return outcome(&state, "hold", &["charge_gate_closed"], "allowed");
        }
        ("compute", "revoke") => {
            let Some(epoch) = event["epoch"]
                .as_str()
                .and_then(|epoch| decimal(epoch, "Dp").ok())
            else {
                reject!("schema_invalid");
            };
            let current = n(&state, "epoch");
            if epoch < current {
                reject!("authorization_epoch_stale");
            }
            if epoch == current && state["status"] == "revoked" {
                return outcome(&state, "hold", &[], "no_op");
            }
            set!("status", "revoked");
            set!("epoch", epoch.to_string());
            return outcome(&state, "hold", &["epoch_revocation"], "allowed");
        }
        ("compute", "charge") => {
            if state["status"] != "active"
                || timestamp_micros(s(c, "now")).unwrap()
                    >= timestamp_micros(s(c, "expires_at")).unwrap()
            {
                reject!("compute_authorization_expired");
            }
            if c["epoch"] != state["epoch"] {
                reject!("authorization_epoch_stale");
            }
            accept!("bounded_charge");
        }
        ("compute", "cleanup") => {
            if c["cleanup"] != true {
                reject!("unauthorized_scope");
            }
            accept!("cleanup_only");
        }
        ("spend", "reserve") => {
            let amount = if let Some(categories) = event["categories"].as_array() {
                worst_case_cost(categories).unwrap()
            } else {
                s(event, "amount").to_string()
            };
            if state["status"] != "absent" {
                if event["id"] != state["export_id"]
                    || amount != s(&state, "amount")
                    || event["digest"] != state["export_digest"]
                {
                    reject!("digest_conflict");
                }
                noop!();
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            if n(&state, "consumed") + n(&state, "reserved") + decimal(&amount, "D0").unwrap()
                > n(&state, "allowance")
            {
                reject!("model_budget_unavailable");
            }
            state["reservation_period_id"] = state["period_id"].clone();
            set!("reserved", &amount);
            set!("amount", &amount);
            state["export_id"] = event["id"].clone();
            state["export_digest"] = event["digest"].clone();
            set!("status", "reserved");
            accept!("spend_reservation");
        }
        ("spend", "dispatch") => {
            if state["status"] != "reserved" {
                reject!("provider_effect_unknown");
            }
            set!("status", "dispatch_intended");
            set!(
                "dispatch_count",
                state["dispatch_count"].as_u64().unwrap() + 1
            );
            accept!("model_attempt_before_io");
        }
        ("spend", "timeout" | "crash") => {
            if state["status"] != "dispatch_intended" {
                reject!("schema_invalid");
            }
            set!("status", "uncertain");
            return outcome(&state, "reconcile", &["retained_liability"], "allowed");
        }
        ("spend", "release") => {
            if state["status"] == "released" {
                noop!();
            }
            if c["no_effect"] != true || state["status"] == "uncertain" {
                reject!("provider_effect_unknown");
            }
            set!("reserved", "0");
            set!("status", "released");
            accept!("verified_release");
        }
        ("spend", "settle") => {
            if state["status"] == "settled" {
                if event["actual"] != state["settled_amount"] {
                    reject!("digest_conflict");
                }
                noop!();
            }
            if c["provider_verified"] != true {
                reject!("unauthorized_scope");
            }
            if n(event, "actual") > n(&state, "reserved") {
                reject!("manual_resolution");
            }
            let consumed = n(&state, "consumed") + n(event, "actual");
            if consumed > i64::MAX as i128 {
                reject!("schema_invalid");
            }
            set!("consumed", consumed.to_string());
            state["settled_amount"] = event["actual"].clone();
            set!("reserved", "0");
            set!("status", "settled");
            set!(
                "settlement_count",
                state["settlement_count"].as_u64().unwrap() + 1
            );
            accept!("trusted_settlement");
        }
        ("spend", "renew_period") => {
            state["period_id"] = event["period_id"].clone();
            accept!("new_period_old_liability_retained");
        }
        ("trial", "activate") => {
            if c["environment"] == "production" || c["policy_approved"] != true {
                reject!("policy_unapproved");
            }
            if c["proof_current"] != true {
                reject!("proof_expired");
            }
            if c["consent_current"] != true {
                reject!("consent_required");
            }
            if state["status"] == "active" {
                noop!();
            }
            set!("status", "active");
            set!("allowance", "5000000");
            state["period_id"] = event["period_id"].clone();
            state["ends_at"] = event["ends_at"].clone();
            accept!("synthetic_trial_once");
        }
        ("trial", "link") => {
            noop!();
        }
        ("period", "payment") => {
            if c["provider_verified"] != true {
                reject!("unauthorized_scope");
            }
            if event["period_id"] == state["period_id"]
                || (state["period_id"] != ""
                    && timestamp_micros(s(event, "ends_at")).unwrap()
                        <= timestamp_micros(s(&state, "ends_at")).unwrap())
            {
                noop!();
            }
            state["period_id"] = event["period_id"].clone();
            state["ends_at"] = event["ends_at"].clone();
            set!("allowance", "20000000");
            set!("status", "paid");
            accept!("verified_period_once");
        }
        ("period", "renewal_failed") => {
            noop!();
        }
        ("period", "cancel_renewal") => {
            set!("status", "cancel_future");
            accept!("future_collection_cancel");
        }
        ("period", "charge") => {
            if timestamp_micros(s(c, "now")).unwrap()
                >= timestamp_micros(s(&state, "ends_at")).unwrap()
            {
                reject!("entitlement_expired");
            }
            accept!("paid_through_authorization");
        }
        ("backup", "snapshot_complete") => {
            if c["provider_verified"] != true {
                reject!("unauthorized_scope");
            }
            set!("status", "snapshot_complete");
            accept!("snapshot_evidence");
        }
        ("backup", "restore_verified") => {
            if state["status"] != "snapshot_complete" || c["storage_recovered"] != true {
                reject!("reconciliation_required");
            }
            set!("restore_verified", true);
            set!("status", "restore_verified");
            accept!("restore_evidence");
        }
        ("backup", "delete") => {
            if c["principal"] != "operator"
                || c["operator_scope"] != true
                || state["restore_verified"] != true
                || c["replay_evidence"] != true
            {
                reject!("unauthorized_scope");
            }
            set!("status", "retired");
            accept!("scoped_cleanup");
        }
        ("backup", "restore_gap") => {
            set!("restored_missing", true);
            set!("status", "reconciliation_required");
            return outcome(&state, "reconcile", &["recovery_hold"], "allowed");
        }
        ("model_stream", _) => {
            if matches!(
                s(&state, "status"),
                "completed" | "outcome_unknown" | "provider_error"
            ) {
                reject!("provider_effect_unknown");
            }
            if n(event, "sequence") != n(&state, "event_sequence") + 1 {
                reject!("digest_conflict");
            }
            state["event_sequence"] = event["sequence"].clone();
            if kind == "stream_start" {
                if state["status"] != "absent" {
                    reject!("schema_invalid");
                }
                set!("status", "streaming");
                accept!("stream_start");
            }
            if state["status"] != "streaming" {
                reject!("schema_invalid");
            }
            match kind {
                "tool_start" => {
                    if state["call_id"] != "" {
                        reject!("digest_conflict");
                    }
                    state["call_id"] = event["call_id"].clone();
                    accept!("tool_item");
                }
                "delta" => {
                    if event["call_id"] != state["call_id"] {
                        reject!("digest_conflict");
                    }
                    let args = s(&state, "tool_arguments").to_owned() + s(event, "delta");
                    let bytes =
                        state["stream_bytes"].as_u64().unwrap() + s(event, "delta").len() as u64;
                    if args.len() > 65536 || bytes > 2097152 {
                        reject!("schema_invalid");
                    }
                    set!("tool_arguments", args);
                    set!("stream_bytes", bytes);
                    accept!("ordered_frame");
                }
                "tool_complete" => {
                    if event["call_id"] != state["call_id"]
                        || event["delta"] != state["tool_arguments"]
                    {
                        reject!("digest_conflict");
                    }
                    accept!("complete_tool_arguments");
                }
                "stream_complete" => {
                    set!("status", "completed");
                    accept!("terminal_stream");
                }
                "timeout" => {
                    set!("status", "outcome_unknown");
                    return outcome(&state, "reconcile", &["retained_liability"], "allowed");
                }
                _ => {}
            }
        }
        ("profile", "authorize") => {
            if c["environment"] == "production" {
                reject!("policy_unapproved");
            }
            if c["entitlement"] != true {
                reject!("entitlement_expired");
            }
            if c["policy_approved"] != true
                || c["model_qualified"] != true
                || c["request_proof"] != true
            {
                reject!("unauthorized_scope");
            }
            if !fresh(c) {
                reject!("compute_authorization_expired");
            }
            if event["id"] != state["work_id"] || event["digest"] != state["material_digest"] {
                reject!("digest_conflict");
            }
            set!("status", "acceptance_only");
            accept!("scoped_profile");
        }
        _ => {}
    }
    rejection(initial, "schema_invalid")
}
