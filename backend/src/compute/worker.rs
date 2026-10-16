//! Installed worker consumer of root-owned authority. No duration or grant is minted here.
use super::{
    ComputeResult,
    authorization::RunningAuthority,
    denied,
    watchdog::{self, Policy, Receipt},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
const POLICY: &str = "/etc/craxii/compute-watchdog.json";
const RECEIPT: &str = "/var/lib/craxii-compute/receipt.json";
const INBOX: &str = "/var/lib/craxii-worker/compute-inbox/grant.json";
const INTENT: &str = "/var/lib/craxii-worker/compute-inbox/renewal.json";
#[derive(Clone)]
pub struct InstalledCompute {
    policy: Policy,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Renewal {
    contract_version: u8,
    request_id: String,
    body: Value,
    issued_at: String,
    attempts: u8,
}
impl InstalledCompute {
    #[cfg(target_os = "linux")]
    pub fn load(
        customer: &str,
        agent: &str,
        installation: &str,
        generation: &str,
        power: &str,
    ) -> ComputeResult<Self> {
        let policy = Policy::parse(&watchdog::protected_read(Path::new(POLICY), 0)?)?;
        let b = &policy.binding;
        if b.customer_id != customer
            || b.logical_agent_id != agent
            || b.installation_id != installation
            || b.generation != generation
            || b.power_operation_id != power
        {
            return Err(denied());
        }
        Ok(Self { policy })
    }
    #[cfg(target_os = "linux")]
    pub fn current(&self) -> ComputeResult<RunningAuthority> {
        let c = watchdog::clock_sample()?;
        Receipt::parse(
            &watchdog::protected_read(Path::new(RECEIPT), 0)?,
            &self.policy,
            &c,
        )?
        .worker_authority(&self.policy, &c)
    }
    #[cfg(target_os = "linux")]
    pub fn check_work(&self, work: &str, grant: &str, duration_ms: u64) -> ComputeResult<()> {
        let a = self.current()?;
        if a.grant_id()? != grant {
            return Err(denied());
        }
        a.check_work(
            &watchdog::clock_sample()?,
            work,
            duration_ms,
            self.policy.margins_us()? / 1000,
        )
    }
    #[cfg(target_os = "linux")]
    pub fn running(&self) -> ComputeResult<()> {
        self.current()?
            .deadline
            .remaining_us(&watchdog::clock_sample()?)?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn cleanup_cutoff(&self) -> ComputeResult<std::time::Instant> {
        let clock = watchdog::clock_sample()?;
        let receipt = Receipt::parse(
            &watchdog::protected_read(Path::new(RECEIPT), 0)?,
            &self.policy,
            &clock,
        )?;
        let remaining = receipt.deadline.remaining_us(&clock)?;
        let stop = (self.policy.stop_ms + self.policy.skew_ms)
            .checked_mul(1000)
            .ok_or_else(denied)?;
        Ok(std::time::Instant::now()
            + std::time::Duration::from_micros(remaining.saturating_sub(stop)))
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn should_drain(&self) -> bool {
        let state = (|| {
            let c = watchdog::clock_sample()?;
            Receipt::parse(
                &watchdog::protected_read(Path::new(RECEIPT), 0)?,
                &self.policy,
                &c,
            )?
            .action(&self.policy, &c)
        })();
        !matches!(
            state,
            Ok(watchdog::Action::Running | watchdog::Action::BootstrapOnly)
        )
    }
    /// Persist exact signed bytes for root acceptance. A staged response has no local authority.
    #[cfg(target_os = "linux")]
    fn stage(&self, response: &Value) -> ComputeResult<()> {
        let bytes = crate::distribution_contracts::canonical::canonical_bytes(response)
            .map_err(|_| denied())?;
        RunningAuthority::verify(&bytes, &self.policy.binding, &watchdog::clock_sample()?)?;
        watchdog::atomic_write_owned(Path::new(INBOX), &bytes, unsafe { nix::libc::geteuid() })
    }
    /// One bounded network exchange per invocation; failed attempts retain the immutable intent.
    /// A lost acknowledgement is recovered with the same request ID after session/process restart.
    #[cfg(target_os = "linux")]
    pub async fn refresh(
        &self,
        client: &mut crate::worker_auth::client::WorkerClient,
        boot: &str,
    ) -> Result<(), crate::worker_auth::AuthError> {
        use crate::worker_auth::AuthError;
        let c = watchdog::clock_sample().map_err(|_| AuthError::InvalidContext)?;
        let receipt = Receipt::parse(
            &watchdog::protected_read(Path::new(RECEIPT), 0)
                .map_err(|_| AuthError::InvalidContext)?,
            &self.policy,
            &c,
        )
        .map_err(|_| AuthError::InvalidContext)?;
        if !matches!(
            receipt
                .action(&self.policy, &c)
                .map_err(|_| AuthError::InvalidContext)?,
            watchdog::Action::Running | watchdog::Action::BootstrapOnly
        ) {
            return Err(AuthError::Unavailable);
        }
        // Recover an already committed current grant first. This cannot create a new reservation.
        if receipt.grant_id.is_none() {
            let response = client
                .compute_authorization(boot, &self.policy.binding.power_operation_id)
                .await?;
            return self.stage(&response).map_err(|_| AuthError::InvalidContext);
        }
        let a = receipt
            .worker_authority(&self.policy, &c)
            .map_err(|_| AuthError::InvalidContext)?;
        let path = Path::new(INTENT);
        let uid = unsafe { nix::libc::geteuid() };
        let saved = if path.try_exists().map_err(|_| AuthError::InvalidContext)? {
            let v = crate::distribution_contracts::raw_json::parse_bounded(
                &watchdog::protected_read(path, uid).map_err(|_| AuthError::InvalidContext)?,
                crate::distribution_contracts::raw_json::CONTROL_LIMITS,
            )
            .map_err(|_| AuthError::InvalidContext)?;
            Some(serde_json::from_value::<Renewal>(v).map_err(|_| AuthError::InvalidContext)?)
        } else {
            None
        };
        let mut intent = if let Some(i) = saved {
            if i.contract_version != 1 || i.attempts > 3 {
                return Err(AuthError::InvalidContext);
            }
            if i.body["grant_id"] != a.grant()["grant_id"] {
                // Only a root-accepted successor retires the prior funding request.
                if receipt
                    .accepted_issued_us
                    .parse::<i128>()
                    .map_err(|_| AuthError::InvalidContext)?
                    <= crate::distribution_contracts::primitives::timestamp_micros(&i.issued_at)
                        .map_err(|_| AuthError::InvalidContext)?
                {
                    return Err(AuthError::InvalidContext);
                }
                std::fs::remove_file(path).map_err(|_| AuthError::InvalidContext)?;
                std::fs::File::open(path.parent().ok_or(AuthError::InvalidContext)?)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| AuthError::InvalidContext)?;
                return Ok(());
            }
            i
        } else {
            if a.deadline
                .remaining_us(&c)
                .map_err(|_| AuthError::InvalidContext)?
                > self
                    .policy
                    .margins_us()
                    .map_err(|_| AuthError::InvalidContext)?
                    .saturating_mul(2)
                    .saturating_add(30_000_000)
            {
                return Ok(());
            }
            Renewal {
                contract_version: 1,
                request_id: crate::distribution::request_id()
                    .map_err(|_| AuthError::InvalidContext)?,
                body: json!({"grant_id":a.grant()["grant_id"],"period_id":a.grant()["period_id"],"power_operation_id":a.grant()["power_operation_id"],"authorization_epoch":a.grant()["authorization_epoch"],"boot_id":boot}),
                issued_at: a.grant()["issued_at"]
                    .as_str()
                    .ok_or(AuthError::InvalidContext)?
                    .to_owned(),
                attempts: 0,
            }
        };
        crate::distribution_contracts::primitives::uuid(&intent.request_id, Some(4))
            .map_err(|_| AuthError::InvalidContext)?;
        if intent.attempts >= 3 {
            // Exhausting bounded send attempts must not strand authority that already
            // committed centrally. This read-only recovery never allocates funding and
            // never resets the persisted request/attempts or the root drain latch.
            let response = client
                .compute_authorization(boot, &self.policy.binding.power_operation_id)
                .await?;
            return self.stage(&response).map_err(|_| AuthError::InvalidContext);
        }
        intent.attempts += 1;
        watchdog::atomic_write_owned(
            path,
            &serde_json::to_vec(&intent).map_err(|_| AuthError::InvalidContext)?,
            uid,
        )
        .map_err(|_| AuthError::InvalidContext)?;
        let response = client
            .renew_compute(&intent.request_id, &intent.body)
            .await?;
        self.stage(&response).map_err(|_| AuthError::InvalidContext)
    }
}

/// Canonical SQLite mapping, sealed after signed delivery/profile and root authority checks.
#[derive(Clone)]
pub struct WorkPermit {
    pub(crate) compute: std::sync::Arc<InstalledCompute>,
    pub(crate) central_work: String,
    pub(crate) local_work: crate::domain::WorkId,
    pub(crate) grant_id: String,
    pub(crate) max_duration_ms: u64,
    pub(crate) profile_cutoff: std::time::Instant,
    pub(crate) cleanup_cutoff: std::time::Instant,
}
impl WorkPermit {
    pub fn check(&self, work: crate::domain::WorkId, duration_ms: u64) -> ComputeResult<()> {
        if work != self.local_work
            || duration_ms > self.max_duration_ms
            || self
                .profile_cutoff
                .saturating_duration_since(std::time::Instant::now())
                <= std::time::Duration::from_millis(duration_ms)
        {
            return Err(denied());
        }
        self.compute
            .check_work(&self.central_work, &self.grant_id, duration_ms)
    }
    pub(crate) fn cleanup_cutoff(&self) -> tokio::time::Instant {
        tokio::time::Instant::from_std(self.cleanup_cutoff)
    }
    pub(crate) fn prepare(&self) -> ComputeResult<()> {
        self.check(self.local_work, 1)
    }
}
