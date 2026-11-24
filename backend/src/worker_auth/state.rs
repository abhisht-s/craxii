use super::{AuthError, AuthResult, key_store::KeyStore};
use crate::distribution_contracts::environment::WorkerEnvironment;
use crate::distribution_contracts::primitives::{decimal, uuid};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityState {
    pub version: u32,
    pub installation_id: String,
    pub customer_id: String,
    pub logical_agent_id: String,
    pub generation: String,
    pub key_id: String,
    pub highest_epoch: String,
    pub binding_id: Option<String>,
    #[serde(default)]
    pub pending_signed: Option<serde_json::Value>,
    pub disposition: Disposition,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Unbound,
    Enrolling,
    Bound,
    Revoked,
    Stale,
    Support,
}
impl IdentityState {
    pub fn validate(&self, key: &KeyStore) -> AuthResult<()> {
        if self.version != 1 || self.key_id != key.key_id() {
            return Err(AuthError::InvalidContext);
        }
        for id in [
            &self.installation_id,
            &self.customer_id,
            &self.logical_agent_id,
        ] {
            uuid(id, Some(4)).map_err(|_| AuthError::InvalidContext)?;
        }
        decimal(&self.generation, "Dp").map_err(|_| AuthError::InvalidContext)?;
        decimal(&self.highest_epoch, "Dp").map_err(|_| AuthError::InvalidContext)?;
        if let Some(id) = &self.binding_id {
            uuid(id, Some(4)).map_err(|_| AuthError::InvalidContext)?;
        }
        if let Some(signed) = &self.pending_signed {
            crate::distribution_contracts::wire::validate_definition(
                "authentication",
                "EnrollmentSigned",
                signed,
            )
            .map_err(|_| AuthError::InvalidContext)?;
            for (field, value) in [
                ("installation_id", self.installation_id.as_str()),
                ("customer_id", self.customer_id.as_str()),
                ("logical_agent_id", self.logical_agent_id.as_str()),
                ("generation", self.generation.as_str()),
                ("key_id", self.key_id.as_str()),
                ("public_key", key.public_key().as_str()),
            ] {
                if signed[field] != value {
                    return Err(AuthError::InvalidContext);
                }
            }
            let environment = signed["environment"]
                .as_str()
                .and_then(WorkerEnvironment::parse)
                .ok_or(AuthError::InvalidContext)?;
            if signed["audience"] != environment.audience() {
                return Err(AuthError::InvalidContext);
            }
        }
        Ok(())
    }
    /// A pending enrollment belongs to one environment; a client configured for another refuses it.
    pub fn require_environment(&self, environment: WorkerEnvironment) -> AuthResult<()> {
        match &self.pending_signed {
            Some(signed) if signed["environment"] != environment.as_str() => {
                Err(AuthError::InvalidContext)
            }
            _ => Ok(()),
        }
    }
    pub fn load(key: &KeyStore) -> AuthResult<Option<Self>> {
        key.read_state()?
            .map(|b| {
                serde_json::from_slice::<Self>(&b)
                    .map_err(|_| AuthError::InvalidContext)
                    .and_then(|s| {
                        s.validate(key)?;
                        Ok(s)
                    })
            })
            .transpose()
    }
    pub fn persist(&self, key: &KeyStore) -> AuthResult<()> {
        self.validate(key)?;
        if let Some(previous) = Self::load(key)?
            && (previous.installation_id != self.installation_id
                || previous.generation != self.generation
                || previous.key_id != self.key_id
                || previous.customer_id != self.customer_id
                || previous.logical_agent_id != self.logical_agent_id
                || self
                    .highest_epoch
                    .parse::<i64>()
                    .map_err(|_| AuthError::InvalidContext)?
                    < previous
                        .highest_epoch
                        .parse::<i64>()
                        .map_err(|_| AuthError::InvalidContext)?
                || matches!(
                    previous.disposition,
                    Disposition::Revoked | Disposition::Stale
                ) && previous.disposition != self.disposition)
        {
            return Err(AuthError::InvalidContext);
        }
        key.save_state(&serde_json::to_vec(self).map_err(|_| AuthError::InvalidContext)?)
    }
}

#[cfg(test)]
mod environment_tests {
    use super::{Disposition, IdentityState, WorkerEnvironment};

    fn state(pending: Option<serde_json::Value>) -> IdentityState {
        IdentityState {
            version: 1,
            installation_id: "10000000-0000-4000-8000-000000000001".into(),
            customer_id: "20000000-0000-4000-8000-000000000001".into(),
            logical_agent_id: "30000000-0000-4000-8000-000000000001".into(),
            generation: "1".into(),
            key_id: "k".into(),
            highest_epoch: "1".into(),
            binding_id: None,
            pending_signed: pending,
            disposition: Disposition::Enrolling,
        }
    }

    #[test]
    fn a_pending_enrollment_is_bound_to_its_environment() {
        let development = state(Some(serde_json::json!({"environment": "development"})));
        assert!(
            development
                .require_environment(WorkerEnvironment::Development)
                .is_ok()
        );
        assert!(
            development
                .require_environment(WorkerEnvironment::Production)
                .is_err()
        );
        let unbound = state(None);
        assert!(
            unbound
                .require_environment(WorkerEnvironment::Production)
                .is_ok()
        );
    }
}
