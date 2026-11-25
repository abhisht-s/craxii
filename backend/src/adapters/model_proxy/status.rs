//! Closed transport status. Financial settlement never implies recoverable output.
use super::{unknown, wire};
use crate::ports::model_provider::*;
use crate::worker_auth::client::WorkerClient;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AttemptStatus {
    contract_version: u8,
    request_id: String,
    attempt_id: String,
    invocation_digest: String,
    state: String,
    dispatch_state: String,
    output_state: String,
    provider_acceptance: String,
    reservation_id: String,
    operation_id: String,
    output_recoverability: String,
    settlement_id: Option<String>,
    evidence_id: Option<String>,
    retry_allowed: bool,
    reconciliation_required: bool,
}
impl AttemptStatus {
    pub fn parse(v: &Value, q: &Value) -> Result<Self, ProviderError> {
        let s: Self = serde_json::from_value(v.clone()).map_err(|_| unknown())?;
        if s.contract_version != 1
            || s.request_id != wire::text(q, "request_id")?
            || s.attempt_id != wire::text(&q["body"]["invocation"], "attempt_id")?
            || s.invocation_digest != wire::text(&q["body"], "invocation_digest")?
            || !matches!(
                s.state.as_str(),
                "reserved"
                    | "dispatch_intended"
                    | "uncertain"
                    | "settled"
                    | "released"
                    | "support_needed"
            )
            || !matches!(
                s.dispatch_state.as_str(),
                "not_dispatched" | "possible_effect"
            )
            || !matches!(s.output_state.as_str(), "complete" | "unavailable")
            || !matches!(
                s.provider_acceptance.as_str(),
                "accepted"
                    | "not_dispatched"
                    | "rejected_no_charge"
                    | "rejected_usage_unknown"
                    | "unknown"
            )
            || !matches!(
                s.output_recoverability.as_str(),
                "unavailable" | "available"
            )
            || s.output_recoverability == "available" && s.output_state != "complete"
            || (s.state == "settled") != s.settlement_id.is_some()
            || s.retry_allowed && (s.state != "released" || s.evidence_id.is_none())
            || s.state == "reserved" && s.dispatch_state != "not_dispatched"
        {
            return Err(unknown());
        }
        for id in [&s.reservation_id, &s.operation_id]
            .into_iter()
            .chain(s.settlement_id.iter())
            .chain(s.evidence_id.iter())
        {
            crate::distribution_contracts::primitives::uuid(id, Some(4)).map_err(|_| unknown())?;
        }
        Ok(s)
    }
    fn disposition(&self) -> ProviderError {
        if self.reconciliation_required {
            unknown()
        } else if self.retry_evidence().is_some() {
            ProviderError::new(
                ProviderErrorKind::TemporarilyUnavailable,
                ProviderOutcomeCertainty::DefiniteProviderFailure,
            )
        } else if self.state == "released" {
            ProviderError::new(
                ProviderErrorKind::InvalidRequest,
                if self.dispatch_state == "not_dispatched" {
                    ProviderOutcomeCertainty::DefinitelyNotSent
                } else {
                    ProviderOutcomeCertainty::DefiniteProviderFailure
                },
            )
        } else {
            unknown()
        }
    }
    pub fn retry_evidence(&self) -> Option<String> {
        (!self.reconciliation_required
            && self.state == "released"
            && self.retry_allowed
            && self.provider_acceptance == "rejected_no_charge"
            && self.output_state == "unavailable")
            .then(|| self.evidence_id.clone())
            .flatten()
    }
    fn processing(&self) -> bool {
        !self.reconciliation_required
            && matches!(self.state.as_str(), "reserved" | "dispatch_intended")
    }
}
// Only reads past facts. A transient status conflict is never a no-charge proof or
// a reason to manufacture a new physical attempt. The caller also pins its original
// monotonic model/compute deadline around this bounded operation.
pub(super) async fn recover(
    client: Arc<tokio::sync::Mutex<WorkerClient>>,
    q: &Value,
) -> Result<reqwest::Response, ProviderError> {
    for _ in 0..30 {
        let state = client.lock().await.model_status(q).await;
        if let Ok(v) = state {
            let s = AttemptStatus::parse(&v, q)?;
            if s.output_recoverability == "available" && !s.reconciliation_required {
                if let Ok(response) = client.lock().await.model_result(q).await {
                    return Ok(response);
                }
            } else if !s.processing() {
                return Err(s.disposition());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(unknown())
}
pub(super) async fn failure(
    client: Arc<tokio::sync::Mutex<WorkerClient>>,
    q: &Value,
) -> ProviderError {
    let query = async {
        let v = client.lock().await.model_status(q).await.ok()?;
        let s = AttemptStatus::parse(&v, q).ok()?;
        Some(s.disposition())
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), query)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_bound_explicit_no_charge_evidence_can_authorize_a_successor() {
        let id = "11111111-1111-4111-8111-111111111111";
        let attempt = "019e0000-0000-7000-8000-000000000001";
        let q = json!({"request_id":id,"body":{"invocation":{"attempt_id":attempt},"invocation_digest":"a".repeat(64)}});
        let base = json!({"contract_version":1,"request_id":id,"attempt_id":attempt,"invocation_digest":"a".repeat(64),"state":"released","dispatch_state":"possible_effect","provider_acceptance":"rejected_no_charge","output_state":"unavailable","reservation_id":id,"operation_id":id,"output_recoverability":"unavailable","settlement_id":null,"evidence_id":id,"retry_allowed":true,"reconciliation_required":false});
        assert_eq!(
            AttemptStatus::parse(&base, &q).unwrap().retry_evidence(),
            Some(id.into())
        );
        let mut disputed = base.clone();
        disputed["reconciliation_required"] = json!(true);
        assert!(
            AttemptStatus::parse(&disputed, &q)
                .unwrap()
                .retry_evidence()
                .is_none()
        );
        assert_eq!(
            AttemptStatus::parse(&disputed, &q)
                .unwrap()
                .disposition()
                .certainty(),
            ProviderOutcomeCertainty::ProviderOutcomeUnknown
        );
        let mut rejected = base.clone();
        rejected["retry_allowed"] = json!(false);
        let status = AttemptStatus::parse(&rejected, &q).unwrap();
        assert_eq!(
            status.disposition().kind(),
            ProviderErrorKind::InvalidRequest
        );
        assert_eq!(
            status.disposition().certainty(),
            ProviderOutcomeCertainty::DefiniteProviderFailure
        );
        rejected["dispatch_state"] = json!("not_dispatched");
        rejected["provider_acceptance"] = json!("not_dispatched");
        assert_eq!(
            AttemptStatus::parse(&rejected, &q)
                .unwrap()
                .disposition()
                .certainty(),
            ProviderOutcomeCertainty::DefinitelyNotSent
        );
        for state in [
            "reserved",
            "dispatch_intended",
            "uncertain",
            "settled",
            "support_needed",
        ] {
            let mut v = base.clone();
            v["state"] = json!(state);
            v["retry_allowed"] = json!(false);
            if state == "settled" {
                v["settlement_id"] = json!(id);
            }
            if state == "reserved" {
                v["dispatch_state"] = json!("not_dispatched");
            }
            assert!(
                AttemptStatus::parse(&v, &q)
                    .unwrap()
                    .retry_evidence()
                    .is_none()
            );
        }
        for (key, value) in [
            ("request_id", json!("22222222-2222-4222-8222-222222222222")),
            ("invocation_digest", json!("b".repeat(64))),
            ("extra", json!(true)),
            ("state", json!("invented")),
        ] {
            let mut v = base.clone();
            v[key] = value;
            assert!(AttemptStatus::parse(&v, &q).is_err());
        }
    }
}
