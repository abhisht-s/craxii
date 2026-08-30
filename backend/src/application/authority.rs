//! Typed V0 development-workstation authority decision seam.

use serde_json::{Value, json};

use crate::application::tool_registry::ToolDefinition;
use crate::domain::{
    AuthorityDecision, AuthorityDecisionSnapshot, AuthorityReasonCode, CraxiiId, PrivilegeMode,
    RuntimeInstanceId, Sha256Digest, ToolName, WorkId, WorkspaceId, WorkstationCapabilities,
    WorkstationGeneration, WorkstationId,
};
use crate::ports::workstation_preparation::RequiredWorkstationCapability;

/// Explicit structured constraints already known to the owning Work context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct V0AuthorityConstraints {
    pub tool_allowed: bool,
    pub administrative_allowed: bool,
}

impl Default for V0AuthorityConstraints {
    fn default() -> Self {
        Self {
            tool_allowed: true,
            administrative_allowed: true,
        }
    }
}

/// Complete typed decision input; it contains no raw argument payload or secret.
pub struct AuthorityEvaluationInput<'a> {
    pub craxii_id: CraxiiId,
    pub work_id: WorkId,
    pub runtime_instance_id: RuntimeInstanceId,
    pub expected_workstation_id: WorkstationId,
    pub expected_generation: WorkstationGeneration,
    pub expected_workspace_id: WorkspaceId,
    pub workspace_id: WorkspaceId,
    pub definition: Option<&'a ToolDefinition>,
    pub requested_tool_name: &'a ToolName,
    pub arguments_sha256: Sha256Digest,
    pub canonical_argument_bytes: usize,
    pub requested_privilege: PrivilegeMode,
    pub requested_timeout_ms: Option<u64>,
    pub requested_stdout_bytes: u64,
    pub requested_stderr_bytes: u64,
    pub work_cancelled: bool,
    pub malformed_arguments: bool,
    pub authority_widening_attempt: bool,
    pub constraints: V0AuthorityConstraints,
    pub capabilities: &'a WorkstationCapabilities,
}

/// Durable decision snapshot plus canonical bounded supporting evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorityEvaluation {
    snapshot: AuthorityDecisionSnapshot,
    evidence_json: String,
}

impl AuthorityEvaluation {
    pub const fn snapshot(&self) -> &AuthorityDecisionSnapshot {
        &self.snapshot
    }

    pub fn evidence_json(&self) -> &str {
        &self.evidence_json
    }

    pub const fn allowed(&self) -> bool {
        matches!(self.snapshot.decision(), AuthorityDecision::Allow)
    }
}

/// Replaceable policy seam. This is deliberately not the future Authority Service.
pub trait AuthorityEvaluator: Send + Sync {
    fn evaluate(&self, input: AuthorityEvaluationInput<'_>) -> AuthorityEvaluation;
}

/// Frozen V0 policy for the configured development workstation.
#[derive(Clone, Copy, Debug, Default)]
pub struct V0AuthorityEvaluator;

impl AuthorityEvaluator for V0AuthorityEvaluator {
    fn evaluate(&self, input: AuthorityEvaluationInput<'_>) -> AuthorityEvaluation {
        let reason = denial_reason(&input);
        let decision = if reason == "allowed" {
            AuthorityDecision::Allow
        } else {
            AuthorityDecision::Deny
        };
        let effective_privilege = if decision == AuthorityDecision::Allow {
            input.requested_privilege
        } else {
            PrivilegeMode::User
        };
        let reason_code = AuthorityReasonCode::try_new(reason).expect("static authority reason");
        let snapshot = AuthorityDecisionSnapshot::new(decision, effective_privilege, reason_code);
        let evidence = json!({
            "arguments_sha256": input.arguments_sha256.to_string(),
            "authority_facts": {
                "administrative_allowed": input.constraints.administrative_allowed,
                "authority_widening_attempt": input.authority_widening_attempt,
                "malformed_arguments": input.malformed_arguments,
                "requested_stderr_bytes": input.requested_stderr_bytes,
                "requested_stdout_bytes": input.requested_stdout_bytes,
                "requested_timeout_ms": input.requested_timeout_ms,
                "tool_allowed": input.constraints.tool_allowed,
                "work_cancelled": input.work_cancelled,
            },
            "canonical_argument_bytes": input.canonical_argument_bytes,
            "capabilities": {
                "cancel_execution": input.capabilities.flags().cancel_execution(),
                "filesystem_read": input.capabilities.flags().filesystem_read(),
                "foreground_execute": input.capabilities.flags().foreground_execute(),
                "inspect_execution": input.capabilities.flags().inspect_execution(),
                "max_execution_timeout_ms": input.capabilities.limits().max_execution_timeout_ms(),
                "max_stderr_bytes": input.capabilities.limits().max_stderr_bytes(),
                "max_stdout_bytes": input.capabilities.limits().max_stdout_bytes(),
                "privilege_administrative": input.capabilities.flags().privilege_administrative(),
                "privilege_user": input.capabilities.flags().privilege_user(),
                "workspace_present": input.capabilities.workspaces().iter().any(|workspace| workspace.workspace_id() == input.workspace_id),
            },
            "craxii_id": input.craxii_id.to_string(),
            "decision": match decision {
                AuthorityDecision::Allow => "allow",
                AuthorityDecision::Deny => "deny",
            },
            "effective_privilege": privilege(effective_privilege),
            "policy": "v0-development-workstation",
            "reason_code": reason,
            "required_capability": input.definition.map(|definition| match definition.required_capability() {
                RequiredWorkstationCapability::FilesystemRead => "filesystem_read",
                RequiredWorkstationCapability::ForegroundExecute => "foreground_execute",
            }),
            "requested_privilege": privilege(input.requested_privilege),
            "runtime_instance_id": input.runtime_instance_id.to_string(),
            "schema_version": input.definition.map(|definition| definition.schema_version().get()),
            "tool_name": input.requested_tool_name.as_str(),
            "tool_version": input.definition.map(|definition| definition.implementation_version().as_str()),
            "version": 1,
            "work_id": input.work_id.to_string(),
            "workspace_id": input.workspace_id.to_string(),
            "workstation_generation": input.expected_generation.get(),
            "workstation_id": input.expected_workstation_id.to_string(),
        });
        AuthorityEvaluation {
            snapshot,
            evidence_json: serde_json::to_string(&canonicalize(evidence))
                .expect("authority evidence serializes"),
        }
    }
}

fn denial_reason(input: &AuthorityEvaluationInput<'_>) -> &'static str {
    let Some(definition) = input.definition else {
        return "unregistered_tool";
    };
    if input.malformed_arguments {
        return "malformed_arguments";
    }
    if input.work_cancelled {
        return "cancelled_work";
    }
    if input.authority_widening_attempt {
        return "authority_widening";
    }
    if !input.constraints.tool_allowed
        || (input.requested_privilege == PrivilegeMode::Administrative
            && !input.constraints.administrative_allowed)
    {
        return "explicit_constraint_denial";
    }
    if input.capabilities.workstation_id() != input.expected_workstation_id {
        return "wrong_workstation";
    }
    if input.capabilities.generation() != input.expected_generation {
        return "stale_generation";
    }
    if input.workspace_id != input.expected_workspace_id
        || !input
            .capabilities
            .workspaces()
            .iter()
            .any(|workspace| workspace.workspace_id() == input.workspace_id)
    {
        return "wrong_workspace";
    }
    let flags = input.capabilities.flags();
    let capability_available = match definition.required_capability() {
        RequiredWorkstationCapability::FilesystemRead => flags.filesystem_read(),
        RequiredWorkstationCapability::ForegroundExecute => flags.foreground_execute(),
    };
    if !capability_available || !flags.privilege_user() {
        return "unsupported_capability";
    }
    if input.requested_privilege == PrivilegeMode::Administrative
        && (!definition
            .privilege_modes()
            .contains(&PrivilegeMode::Administrative)
            || !flags.privilege_administrative())
    {
        return "administrative_unavailable";
    }
    let limits = input.capabilities.limits();
    if input.canonical_argument_bytes
        > crate::application::tool_registry::MAX_RAW_TOOL_ARGUMENT_BYTES
        || input.requested_timeout_ms.is_some_and(|timeout| {
            timeout == 0
                || definition
                    .hard_timeout_ms()
                    .is_some_and(|hard| timeout > hard)
                || timeout > limits.max_execution_timeout_ms()
        })
        || input.requested_stdout_bytes > limits.max_stdout_bytes()
        || input.requested_stderr_bytes > limits.max_stderr_bytes()
    {
        return "limit_exceeded";
    }
    "allowed"
}

const fn privilege(value: PrivilegeMode) -> &'static str {
    match value {
        PrivilegeMode::User => "user",
        PrivilegeMode::Administrative => "administrative",
    }
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect(),
            )
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        LogicalPathReference, WorkspaceCapabilityRef, WorkstationCapabilitiesInput,
        WorkstationCapabilityFlags, WorkstationCapabilityFlagsInput, WorkstationCapabilityLimits,
    };

    fn capabilities_with(
        admin: bool,
        filesystem_read: bool,
        foreground_execute: bool,
    ) -> WorkstationCapabilities {
        let workstation_id = WorkstationId::generate();
        let workspace_id = WorkspaceId::generate();
        WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
            workstation_id,
            generation: WorkstationGeneration::try_new(1).unwrap(),
            cpu_architecture: "aarch64".into(),
            os_release: "macos".into(),
            default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
            flags: WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
                filesystem_read,
                foreground_execute,
                cancel_execution: true,
                inspect_execution: true,
                privilege_user: true,
                privilege_administrative: admin,
                process_group_cleanup: true,
                cgroup_cleanup: false,
            }),
            limits: WorkstationCapabilityLimits::try_new(900_000, 8_388_608, 8_388_608).unwrap(),
            workspaces: vec![
                WorkspaceCapabilityRef::try_new(
                    workspace_id,
                    LogicalPathReference::absolute("/workspace").unwrap(),
                )
                .unwrap(),
            ],
        })
        .unwrap()
    }

    fn capabilities(admin: bool) -> WorkstationCapabilities {
        capabilities_with(admin, true, true)
    }

    fn input<'a>(
        capabilities: &'a WorkstationCapabilities,
        definition: &'a ToolDefinition,
        privilege: PrivilegeMode,
    ) -> AuthorityEvaluationInput<'a> {
        AuthorityEvaluationInput {
            craxii_id: CraxiiId::generate(),
            work_id: WorkId::generate(),
            runtime_instance_id: RuntimeInstanceId::generate(),
            expected_workstation_id: capabilities.workstation_id(),
            expected_generation: capabilities.generation(),
            expected_workspace_id: capabilities.workspaces()[0].workspace_id(),
            workspace_id: capabilities.workspaces()[0].workspace_id(),
            definition: Some(definition),
            requested_tool_name: definition.name(),
            arguments_sha256: Sha256Digest::hash_bytes(b"{}"),
            canonical_argument_bytes: 2,
            requested_privilege: privilege,
            requested_timeout_ms: definition.default_timeout_ms(),
            requested_stdout_bytes: 1,
            requested_stderr_bytes: 1,
            work_cancelled: false,
            malformed_arguments: false,
            authority_widening_attempt: false,
            constraints: V0AuthorityConstraints::default(),
            capabilities,
        }
    }

    fn evaluate(
        capabilities: &WorkstationCapabilities,
        tool_index: usize,
        privilege: PrivilegeMode,
    ) -> AuthorityEvaluation {
        let registry = registry();
        let definition = &registry.definitions()[tool_index];
        V0AuthorityEvaluator.evaluate(input(capabilities, definition, privilege))
    }

    fn registry() -> crate::application::tool_registry::ToolRegistry {
        crate::application::tool_registry::ToolRegistry::v0(
            crate::application::tool_registry::ToolSemanticPolicy {
                read_file_default_bytes: 1_048_576,
                read_file_max_bytes: 8_388_608,
                run_shell_command_max_bytes: 65_536,
                run_shell_default_timeout_ms: 120_000,
                run_shell_max_timeout_ms: 900_000,
            },
        )
        .unwrap()
    }

    #[test]
    fn user_tools_are_allowed_with_stable_policy_and_canonical_evidence() {
        let capabilities = capabilities(false);
        for tool in 0..2 {
            let decision = evaluate(&capabilities, tool, PrivilegeMode::User);
            assert!(decision.allowed());
            assert_eq!(decision.snapshot().reason_code().as_str(), "allowed");
            assert_eq!(
                decision.snapshot().policy_version().as_str(),
                "v0-development-workstation"
            );
            assert_eq!(
                serde_json::to_string(&canonicalize(
                    serde_json::from_str(decision.evidence_json()).unwrap()
                ))
                .unwrap(),
                decision.evidence_json()
            );
        }
    }

    #[test]
    fn administrative_execution_requires_tool_support_and_machine_capability() {
        let local_macos = capabilities(false);
        assert_eq!(
            evaluate(&local_macos, 1, PrivilegeMode::Administrative)
                .snapshot()
                .reason_code()
                .as_str(),
            "administrative_unavailable"
        );
        let admin = capabilities(true);
        assert!(evaluate(&admin, 1, PrivilegeMode::Administrative).allowed());
        assert_eq!(
            evaluate(&admin, 0, PrivilegeMode::Administrative)
                .snapshot()
                .reason_code()
                .as_str(),
            "administrative_unavailable"
        );
    }

    #[test]
    fn every_v0_fail_closed_reason_is_reachable_without_machine_action() {
        let capabilities = capabilities(false);
        let registry = registry();
        let definition = &registry.definitions()[1];

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.definition = None;
        assert_eq!(denial_reason(&value), "unregistered_tool");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.malformed_arguments = true;
        assert_eq!(denial_reason(&value), "malformed_arguments");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.work_cancelled = true;
        assert_eq!(denial_reason(&value), "cancelled_work");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.authority_widening_attempt = true;
        assert_eq!(denial_reason(&value), "authority_widening");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.constraints.tool_allowed = false;
        assert_eq!(denial_reason(&value), "explicit_constraint_denial");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.expected_workstation_id = WorkstationId::generate();
        assert_eq!(denial_reason(&value), "wrong_workstation");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.expected_generation = WorkstationGeneration::try_new(2).unwrap();
        assert_eq!(denial_reason(&value), "stale_generation");

        let mut value = input(&capabilities, definition, PrivilegeMode::User);
        value.workspace_id = WorkspaceId::generate();
        assert_eq!(denial_reason(&value), "wrong_workspace");

        let unavailable = capabilities_with(false, true, false);
        let value = input(&unavailable, definition, PrivilegeMode::User);
        assert_eq!(denial_reason(&value), "unsupported_capability");

        let mut value = input(&capabilities, definition, PrivilegeMode::Administrative);
        assert_eq!(denial_reason(&value), "administrative_unavailable");
        value.requested_privilege = PrivilegeMode::User;
        value.requested_timeout_ms = Some(900_001);
        assert_eq!(denial_reason(&value), "limit_exceeded");
    }

    #[test]
    fn semantic_reason_changes_canonical_evidence_without_raw_argument_content() {
        let capabilities = capabilities(false);
        let registry = registry();
        let definition = &registry.definitions()[1];
        let allowed =
            V0AuthorityEvaluator.evaluate(input(&capabilities, definition, PrivilegeMode::User));
        let mut denied_input = input(&capabilities, definition, PrivilegeMode::User);
        denied_input.constraints.tool_allowed = false;
        let denied = V0AuthorityEvaluator.evaluate(denied_input);
        assert_ne!(allowed.evidence_json(), denied.evidence_json());
        assert!(
            allowed
                .evidence_json()
                .contains("\"reason_code\":\"allowed\"")
        );
        assert!(
            denied
                .evidence_json()
                .contains("\"reason_code\":\"explicit_constraint_denial\"")
        );
        for evidence in [allowed.evidence_json(), denied.evidence_json()] {
            assert!(!evidence.contains("command"));
        }
    }
}
