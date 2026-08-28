use crate::bootstrap::health::Health;
use crate::bootstrap::metadata::ProcessMetadata;
use crate::domain::{
    Conversation, CraxiiPrincipal, DomainValidationError, DomainValidationKind, WorkInputActor,
    WorkInputRelationship, WorkItem, WorkItemInput, WorkspaceIdentity,
};
use crate::ports::state_store::BootstrapSnapshot;

pub mod authentication;
pub mod command_service;
pub mod device_provisioning;
pub mod projector;

/// Validates the complete in-memory V0 principal/conversation/default-workspace topology.
///
/// Persistence/bootstrap uniqueness is deliberately deferred to its owning stages.
pub fn validate_v0_topology(
    principal: &CraxiiPrincipal,
    conversations: &[Conversation],
    workspaces: &[WorkspaceIdentity],
) -> Result<(), DomainValidationError> {
    let [conversation] = conversations else {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidPrimaryConversation,
        ));
    };
    if conversation.conversation_id() != principal.primary_conversation_id()
        || conversation.craxii_id() != principal.craxii_id()
    {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidPrimaryConversation,
        ));
    }

    let mut matching = workspaces
        .iter()
        .filter(|workspace| workspace.workspace_id() == principal.default_workspace_id());
    let Some(default_workspace) = matching.next() else {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidPrimaryConversation,
        ));
    };
    if matching.next().is_some() || default_workspace.craxii_id() != principal.craxii_id() {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidPrimaryConversation,
        ));
    }

    Ok(())
}

/// Validates the exact current V0 conversational work-input shape.
///
/// The referenced journal event's semantic type cannot be inferred from its ID
/// here and is intentionally deferred to the later transaction service.
pub fn validate_v0_conversational_work_inputs(
    work: &WorkItem,
    inputs: &[WorkItemInput],
) -> Result<(), DomainValidationError> {
    let [input] = inputs else {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidWorkInput,
        ));
    };
    if input.work_id() != work.work_id()
        || input.relationship() != WorkInputRelationship::Trigger
        || input.ordinal_within_work().get() != 1
        || input.actor() != WorkInputActor::User
    {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidWorkInput,
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct ApplicationShell {
    process_metadata: ProcessMetadata,
    health: Health,
    bootstrap_snapshot: BootstrapSnapshot,
}

impl ApplicationShell {
    pub(crate) fn new(
        process_metadata: ProcessMetadata,
        health: Health,
        bootstrap_snapshot: BootstrapSnapshot,
    ) -> Self {
        Self {
            process_metadata,
            health,
            bootstrap_snapshot,
        }
    }

    pub fn process_metadata(&self) -> &ProcessMetadata {
        &self.process_metadata
    }

    pub fn health(&self) -> &Health {
        &self.health
    }

    pub fn bootstrap_snapshot(&self) -> &BootstrapSnapshot {
        &self.bootstrap_snapshot
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt, str::FromStr};

    use super::*;
    use crate::domain::{
        ConversationId, ConversationWorkOrdinal, CorrelationId, CraxiiId, CraxiiPrincipalInput,
        JournalEventId, LogicalPathReference, ProjectionVersion, SchemaVersion, UtcTimestamp,
        WorkId, WorkInputActor, WorkInputOrdinal, WorkItemInputData, WorkspaceId,
        WorkspaceIdentityInput, WorkstationId,
    };

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    fn id<T: FromStr>(value: &str) -> T
    where
        T::Err: fmt::Debug,
    {
        value.parse().unwrap()
    }

    fn now() -> UtcTimestamp {
        "2026-08-27T12:34:56.000001Z".parse().unwrap()
    }

    fn topology() -> (CraxiiPrincipal, Conversation, WorkspaceIdentity) {
        let craxii_id = id(V7);
        let conversation_id = ConversationId::generate();
        let workspace_id = WorkspaceId::generate();
        let principal = CraxiiPrincipal::try_new(CraxiiPrincipalInput {
            craxii_id,
            display_name: "Craxii".into(),
            owner_label: "Owner".into(),
            primary_conversation_id: conversation_id,
            default_workspace_id: workspace_id,
            created_at: now(),
            architecture_revision: "V0.0.01-r3".into(),
            schema_revision: SchemaVersion::try_new(1).unwrap(),
        })
        .unwrap();
        let conversation = Conversation::new(
            conversation_id,
            craxii_id,
            now(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
            ProjectionVersion::try_new(1).unwrap(),
        );
        let workspace = WorkspaceIdentity::try_new(WorkspaceIdentityInput {
            workspace_id,
            craxii_id,
            workstation_id: WorkstationId::generate(),
            logical_name: "primary".into(),
            logical_root: LogicalPathReference::absolute("/srv/craxii/workspaces/primary").unwrap(),
            created_at: now(),
        })
        .unwrap();
        (principal, conversation, workspace)
    }

    fn work() -> WorkItem {
        WorkItem::new(WorkItemInputData {
            work_id: id(V7),
            craxii_id: id(V7),
            conversation_id: id(V7),
            conversation_work_ordinal: ConversationWorkOrdinal::try_new(1).unwrap(),
            workspace_id: id(V7),
            correlation_id: id::<CorrelationId>(V7),
            created_at: now(),
            queued_at: now(),
        })
    }

    fn work_input(
        work_id: WorkId,
        relationship: WorkInputRelationship,
        ordinal: i64,
    ) -> WorkItemInput {
        WorkItemInput::new(
            work_id,
            id::<JournalEventId>(V7),
            relationship,
            WorkInputOrdinal::try_new(ordinal).unwrap(),
            now(),
            WorkInputActor::User,
        )
    }

    fn work_input_with_actor(work_id: WorkId, actor: WorkInputActor) -> WorkItemInput {
        WorkItemInput::new(
            work_id,
            id::<JournalEventId>(V7),
            WorkInputRelationship::Trigger,
            WorkInputOrdinal::try_new(1).unwrap(),
            now(),
            actor,
        )
    }

    #[test]
    fn topology_accepts_exact_matching_principal_conversation_and_default_workspace() {
        let (principal, conversation, workspace) = topology();
        assert!(validate_v0_topology(&principal, &[conversation], &[workspace]).is_ok());
    }

    #[test]
    fn topology_rejects_missing_multiple_replacement_and_ownership_mismatches() {
        let (principal, conversation, workspace) = topology();
        assert!(validate_v0_topology(&principal, &[], std::slice::from_ref(&workspace)).is_err());
        assert!(
            validate_v0_topology(
                &principal,
                &[conversation.clone(), conversation.clone()],
                std::slice::from_ref(&workspace)
            )
            .is_err()
        );

        let replacement = Conversation::new(
            ConversationId::generate(),
            principal.craxii_id(),
            now(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
            ProjectionVersion::try_new(1).unwrap(),
        );
        assert!(
            validate_v0_topology(&principal, &[replacement], std::slice::from_ref(&workspace))
                .is_err()
        );

        let wrong_owner_conversation = Conversation::new(
            principal.primary_conversation_id(),
            CraxiiId::generate(),
            now(),
            ConversationWorkOrdinal::try_new(1).unwrap(),
            ProjectionVersion::try_new(1).unwrap(),
        );
        assert!(
            validate_v0_topology(
                &principal,
                &[wrong_owner_conversation],
                std::slice::from_ref(&workspace)
            )
            .is_err()
        );
        assert!(
            validate_v0_topology(&principal, std::slice::from_ref(&conversation), &[]).is_err()
        );

        let duplicate_workspace = workspace.clone();
        assert!(
            validate_v0_topology(
                &principal,
                &[conversation],
                &[workspace, duplicate_workspace]
            )
            .is_err()
        );
    }

    #[test]
    fn v0_work_input_accepts_exactly_one_matching_trigger_at_ordinal_one() {
        let work = work();
        let input = work_input(work.work_id(), WorkInputRelationship::Trigger, 1);
        assert!(validate_v0_conversational_work_inputs(&work, &[input]).is_ok());
    }

    #[test]
    fn v0_work_input_rejects_zero_multiple_wrong_work_nonordinal_and_reserved_relationships() {
        let work = work();
        assert!(validate_v0_conversational_work_inputs(&work, &[]).is_err());
        let trigger = work_input(work.work_id(), WorkInputRelationship::Trigger, 1);
        assert!(
            validate_v0_conversational_work_inputs(&work, &[trigger.clone(), trigger]).is_err()
        );
        assert!(
            validate_v0_conversational_work_inputs(
                &work,
                &[work_input(
                    WorkId::generate(),
                    WorkInputRelationship::Trigger,
                    1
                )]
            )
            .is_err()
        );
        assert!(
            validate_v0_conversational_work_inputs(
                &work,
                &[work_input(
                    work.work_id(),
                    WorkInputRelationship::Trigger,
                    2
                )]
            )
            .is_err()
        );

        for relationship in [
            WorkInputRelationship::Steering,
            WorkInputRelationship::Supplemental,
            WorkInputRelationship::ScheduledTrigger,
            WorkInputRelationship::ExternalTrigger,
            WorkInputRelationship::RecoveryInstruction,
        ] {
            assert!(
                validate_v0_conversational_work_inputs(
                    &work,
                    &[work_input(work.work_id(), relationship, 1)]
                )
                .is_err()
            );
        }
        for actor in [
            WorkInputActor::Craxii,
            WorkInputActor::System,
            WorkInputActor::Recovery,
        ] {
            assert!(
                validate_v0_conversational_work_inputs(
                    &work,
                    &[work_input_with_actor(work.work_id(), actor)]
                )
                .is_err()
            );
        }
    }
}
