//! Immutable Stage 3.2 principal, conversation, message, work, and machine entities.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::{
    ClientMessageId, ConversationId, ConversationWorkOrdinal, CorrelationId, CraxiiId, DeviceId,
    DomainValidationError, DomainValidationKind, JournalEventId, LogicalPathReference,
    MessageContent, MessageId, Sha256Digest, UtcTimestamp, WorkId, WorkspaceId, WorkstationId,
};

macro_rules! exact_positive_integer {
    ($name:ident, $error_kind:expr, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(i64);

        impl $name {
            /// Constructs a value in `1..=i64::MAX`.
            pub const fn try_new(value: i64) -> Result<Self, DomainValidationError> {
                if value > 0 {
                    Ok(Self(value))
                } else {
                    Err(DomainValidationError::new($error_kind))
                }
            }

            /// Returns the signed-64-bit-safe numeric value.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }

            /// Returns the checked next value.
            pub const fn checked_increment(self) -> Result<Self, DomainValidationError> {
                match self.0.checked_add(1) {
                    Some(value) => Ok(Self(value)),
                    None => Err(DomainValidationError::new(
                        DomainValidationKind::ArithmeticOverflow,
                    )),
                }
            }
        }

        impl TryFrom<i64> for $name {
            type Error = DomainValidationError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                Self::try_new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_i64(self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct PositiveVisitor;

                impl<'de> de::Visitor<'de> for PositiveVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a positive signed-64-bit integer")
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        $name::try_new(value).map_err(E::custom)
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        i64::try_from(value)
                            .map_err(|_| E::custom("integer exceeds i64::MAX"))
                            .and_then(|value| $name::try_new(value).map_err(E::custom))
                    }
                }

                deserializer.deserialize_any(PositiveVisitor)
            }
        }
    };
}

exact_positive_integer!(
    WorkstationGeneration,
    DomainValidationKind::InvalidWorkstationGeneration,
    "The replacement/restore/reprovision generation scoped with a WorkstationId."
);
exact_positive_integer!(
    ProjectionVersion,
    DomainValidationKind::InvalidPositiveInteger,
    "The guarded positive version of a current-state projection."
);
exact_positive_integer!(
    SchemaVersion,
    DomainValidationKind::InvalidPositiveInteger,
    "The positive durable schema version."
);
exact_positive_integer!(
    WorkInputOrdinal,
    DomainValidationKind::InvalidWorkInput,
    "The positive relationship order within one work item."
);

/// The only V0 Craxii lifecycle literal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CraxiiLifecycle {
    /// The principal is active.
    Active,
}

/// The only V0 conversation kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    /// The one visible V0 conversation.
    Primary,
}

/// The only V0 conversation lifecycle literal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationLifecycle {
    /// The conversation is active.
    Active,
}

/// Exact committed-message roles.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    /// Content accepted from an authenticated client device.
    User,
    /// Terminal user-facing content produced by one work item.
    Assistant,
    /// Craxii-owned system content with no client or work producer.
    System,
}

/// The only V0 structural work kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// One conversational responsibility.
    Conversational,
}

/// Frozen relation-shaped work input relationships.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkInputRelationship {
    /// The accepted event that creates V0 conversational work.
    Trigger,
    /// Reserved future steering input.
    Steering,
    /// Reserved future supplemental input.
    Supplemental,
    /// Reserved future scheduled trigger.
    ScheduledTrigger,
    /// Reserved future external trigger.
    ExternalTrigger,
    /// Reserved future recovery instruction.
    RecoveryInstruction,
}

/// Closed Stage 3.2 provenance actors for work-input relationships.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkInputActor {
    /// An authenticated user relationship.
    User,
    /// A Craxii-owned relationship.
    Craxii,
    /// A system relationship.
    System,
    /// A recovery relationship.
    Recovery,
}

/// The only V0 workstation kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkstationKind {
    /// The Ubuntu/Linux workstation hosted with the backend.
    Local,
}

/// The only V0 workspace lifecycle literal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLifecycle {
    /// The workspace is active.
    Active,
}

/// A bounded opaque hosting-provider identifier.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostingProvider(String);

impl HostingProvider {
    /// Validates 1..=64 lowercase ASCII reference grammar.
    pub fn try_new(value: impl Into<String>) -> Result<Self, DomainValidationError> {
        let value = value.into();
        if !valid_lower_reference(&value, 64, false) {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidBoundedIdentifier,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the exact opaque identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HostingProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HostingProvider")
            .field(&self.0)
            .finish()
    }
}

fn valid_lower_reference(value: &str, max: usize, first_alpha_only: bool) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > max || !value.is_ascii() {
        return false;
    }
    let first = bytes[0];
    let first_valid = if first_alpha_only {
        first.is_ascii_lowercase()
    } else {
        first.is_ascii_lowercase() || first.is_ascii_digit()
    };
    first_valid
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn validate_human_label(value: &str, max: usize) -> Result<(), DomainValidationError> {
    if value.is_empty()
        || value.len() > max
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidBoundedIdentifier,
        ));
    }
    Ok(())
}

fn validate_metadata(value: &str, max: usize) -> Result<(), DomainValidationError> {
    validate_human_label(value, max)
}

fn validate_visible_ascii(value: &str, max: usize) -> Result<(), DomainValidationError> {
    if value.is_empty()
        || value.len() > max
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !byte.is_ascii_whitespace())
    {
        return Err(DomainValidationError::new(
            DomainValidationKind::InvalidBoundedIdentifier,
        ));
    }
    Ok(())
}

/// Construction data for an immutable principal snapshot.
pub struct CraxiiPrincipalInput {
    pub craxii_id: CraxiiId,
    pub display_name: String,
    pub owner_label: String,
    pub primary_conversation_id: ConversationId,
    pub default_workspace_id: WorkspaceId,
    pub created_at: UtcTimestamp,
    pub architecture_revision: String,
    pub schema_revision: SchemaVersion,
}

/// The immutable active V0 Craxii principal snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct CraxiiPrincipal {
    craxii_id: CraxiiId,
    display_name: String,
    owner_label: String,
    primary_conversation_id: ConversationId,
    default_workspace_id: WorkspaceId,
    created_at: UtcTimestamp,
    architecture_revision: String,
    schema_revision: SchemaVersion,
    lifecycle: CraxiiLifecycle,
}

impl CraxiiPrincipal {
    /// Validates and constructs the one active V0 principal snapshot.
    pub fn try_new(input: CraxiiPrincipalInput) -> Result<Self, DomainValidationError> {
        validate_human_label(&input.display_name, 128)?;
        validate_human_label(&input.owner_label, 128)?;
        validate_visible_ascii(&input.architecture_revision, 128)?;
        Ok(Self {
            craxii_id: input.craxii_id,
            display_name: input.display_name,
            owner_label: input.owner_label,
            primary_conversation_id: input.primary_conversation_id,
            default_workspace_id: input.default_workspace_id,
            created_at: input.created_at,
            architecture_revision: input.architecture_revision,
            schema_revision: input.schema_revision,
            lifecycle: CraxiiLifecycle::Active,
        })
    }

    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
    pub fn owner_label(&self) -> &str {
        &self.owner_label
    }
    pub const fn primary_conversation_id(&self) -> ConversationId {
        self.primary_conversation_id
    }
    pub const fn default_workspace_id(&self) -> WorkspaceId {
        self.default_workspace_id
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
    pub fn architecture_revision(&self) -> &str {
        &self.architecture_revision
    }
    pub const fn schema_revision(&self) -> SchemaVersion {
        self.schema_revision
    }
    pub const fn lifecycle(&self) -> CraxiiLifecycle {
        self.lifecycle
    }
}

impl fmt::Debug for CraxiiPrincipal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CraxiiPrincipal")
            .field("craxii_id", &self.craxii_id)
            .field("primary_conversation_id", &self.primary_conversation_id)
            .field("default_workspace_id", &self.default_workspace_id)
            .field("created_at", &self.created_at)
            .field("schema_revision", &self.schema_revision)
            .field("lifecycle", &self.lifecycle)
            .finish_non_exhaustive()
    }
}

/// The immutable primary/active V0 conversation projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    conversation_id: ConversationId,
    craxii_id: CraxiiId,
    kind: ConversationKind,
    lifecycle: ConversationLifecycle,
    created_at: UtcTimestamp,
    next_work_ordinal: ConversationWorkOrdinal,
    projection_version: ProjectionVersion,
}

impl Conversation {
    /// Constructs the only V0 kind/lifecycle combination.
    #[must_use]
    pub const fn new(
        conversation_id: ConversationId,
        craxii_id: CraxiiId,
        created_at: UtcTimestamp,
        next_work_ordinal: ConversationWorkOrdinal,
        projection_version: ProjectionVersion,
    ) -> Self {
        Self {
            conversation_id,
            craxii_id,
            kind: ConversationKind::Primary,
            lifecycle: ConversationLifecycle::Active,
            created_at,
            next_work_ordinal,
            projection_version,
        }
    }

    pub const fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn kind(&self) -> ConversationKind {
        self.kind
    }
    pub const fn lifecycle(&self) -> ConversationLifecycle {
        self.lifecycle
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
    pub const fn next_work_ordinal(&self) -> ConversationWorkOrdinal {
        self.next_work_ordinal
    }
    pub const fn projection_version(&self) -> ProjectionVersion {
        self.projection_version
    }
}

/// Construction data for one immutable committed message.
pub struct MessageInput {
    pub message_id: MessageId,
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub role: MessageRole,
    pub content: MessageContent,
    pub produced_by_work_id: Option<WorkId>,
    pub device_id: Option<DeviceId>,
    pub client_message_id: Option<ClientMessageId>,
    pub committed_at: UtcTimestamp,
}

/// One immutable committed V0 message.
#[derive(Clone, Eq, PartialEq)]
pub struct Message {
    message_id: MessageId,
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    role: MessageRole,
    content: MessageContent,
    produced_by_work_id: Option<WorkId>,
    device_id: Option<DeviceId>,
    client_message_id: Option<ClientMessageId>,
    content_sha256: Sha256Digest,
    committed_at: UtcTimestamp,
}

impl Message {
    /// Validates the exact role/client/work provenance matrix.
    pub fn try_new(input: MessageInput) -> Result<Self, DomainValidationError> {
        let valid_provenance = match input.role {
            MessageRole::User => {
                input.produced_by_work_id.is_none()
                    && input.device_id.is_some()
                    && input.client_message_id.is_some()
            }
            MessageRole::Assistant => {
                input.produced_by_work_id.is_some()
                    && input.device_id.is_none()
                    && input.client_message_id.is_none()
            }
            MessageRole::System => {
                input.produced_by_work_id.is_none()
                    && input.device_id.is_none()
                    && input.client_message_id.is_none()
            }
        };
        if !valid_provenance {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidMessageProvenance,
            ));
        }

        let content_sha256 = input.content.content_sha256();
        Ok(Self {
            message_id: input.message_id,
            craxii_id: input.craxii_id,
            conversation_id: input.conversation_id,
            role: input.role,
            content: input.content,
            produced_by_work_id: input.produced_by_work_id,
            device_id: input.device_id,
            client_message_id: input.client_message_id,
            content_sha256,
            committed_at: input.committed_at,
        })
    }

    pub const fn message_id(&self) -> MessageId {
        self.message_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }
    pub const fn role(&self) -> MessageRole {
        self.role
    }
    pub const fn content(&self) -> &MessageContent {
        &self.content
    }
    pub const fn produced_by_work_id(&self) -> Option<WorkId> {
        self.produced_by_work_id
    }
    pub const fn device_id(&self) -> Option<DeviceId> {
        self.device_id
    }
    pub const fn client_message_id(&self) -> Option<ClientMessageId> {
        self.client_message_id
    }
    pub const fn content_sha256(&self) -> Sha256Digest {
        self.content_sha256
    }
    pub const fn committed_at(&self) -> UtcTimestamp {
        self.committed_at
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Message")
            .field("message_id", &self.message_id)
            .field("craxii_id", &self.craxii_id)
            .field("conversation_id", &self.conversation_id)
            .field("role", &self.role)
            .field("content_sha256", &self.content_sha256)
            .field("committed_at", &self.committed_at)
            .finish_non_exhaustive()
    }
}

/// Construction data for a structural Stage 3.2 work item.
pub struct WorkItemInputData {
    pub work_id: WorkId,
    pub craxii_id: CraxiiId,
    pub conversation_id: ConversationId,
    pub conversation_work_ordinal: ConversationWorkOrdinal,
    pub workspace_id: WorkspaceId,
    pub correlation_id: CorrelationId,
    pub created_at: UtcTimestamp,
    pub queued_at: UtcTimestamp,
}

/// Immutable work creation/topology fields; lifecycle belongs to Stage 4.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkItem {
    work_id: WorkId,
    craxii_id: CraxiiId,
    conversation_id: ConversationId,
    conversation_work_ordinal: ConversationWorkOrdinal,
    kind: WorkKind,
    priority: i64,
    workspace_id: WorkspaceId,
    correlation_id: CorrelationId,
    created_at: UtcTimestamp,
    queued_at: UtcTimestamp,
}

impl WorkItem {
    /// Constructs fixed-kind, fixed-priority V0 conversational work.
    #[must_use]
    pub const fn new(input: WorkItemInputData) -> Self {
        Self {
            work_id: input.work_id,
            craxii_id: input.craxii_id,
            conversation_id: input.conversation_id,
            conversation_work_ordinal: input.conversation_work_ordinal,
            kind: WorkKind::Conversational,
            priority: 0,
            workspace_id: input.workspace_id,
            correlation_id: input.correlation_id,
            created_at: input.created_at,
            queued_at: input.queued_at,
        }
    }

    pub const fn work_id(&self) -> WorkId {
        self.work_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn conversation_id(&self) -> ConversationId {
        self.conversation_id
    }
    pub const fn conversation_work_ordinal(&self) -> ConversationWorkOrdinal {
        self.conversation_work_ordinal
    }
    pub const fn kind(&self) -> WorkKind {
        self.kind
    }
    pub const fn priority(&self) -> i64 {
        self.priority
    }
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
    pub const fn queued_at(&self) -> UtcTimestamp {
        self.queued_at
    }
}

/// One immutable relation from work to an input journal event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkItemInput {
    work_id: WorkId,
    input_event_id: JournalEventId,
    relationship: WorkInputRelationship,
    ordinal_within_work: WorkInputOrdinal,
    attached_at: UtcTimestamp,
    actor: WorkInputActor,
}

impl WorkItemInput {
    /// Constructs a structurally valid relationship, including reserved variants.
    #[must_use]
    pub const fn new(
        work_id: WorkId,
        input_event_id: JournalEventId,
        relationship: WorkInputRelationship,
        ordinal_within_work: WorkInputOrdinal,
        attached_at: UtcTimestamp,
        actor: WorkInputActor,
    ) -> Self {
        Self {
            work_id,
            input_event_id,
            relationship,
            ordinal_within_work,
            attached_at,
            actor,
        }
    }

    pub const fn work_id(&self) -> WorkId {
        self.work_id
    }
    pub const fn input_event_id(&self) -> JournalEventId {
        self.input_event_id
    }
    pub const fn relationship(&self) -> WorkInputRelationship {
        self.relationship
    }
    pub const fn ordinal_within_work(&self) -> WorkInputOrdinal {
        self.ordinal_within_work
    }
    pub const fn attached_at(&self) -> UtcTimestamp {
        self.attached_at
    }
    pub const fn actor(&self) -> WorkInputActor {
        self.actor
    }
}

/// Construction data for immutable workstation identity.
pub struct WorkstationIdentityInput {
    pub workstation_id: WorkstationId,
    pub craxii_id: CraxiiId,
    pub generation: WorkstationGeneration,
    pub hosting_provider: HostingProvider,
    pub provider_instance_id: Option<String>,
    pub image_id: Option<String>,
    pub provisioning_revision: Option<String>,
    pub cpu_architecture: String,
    pub os_release: String,
    pub created_at: UtcTimestamp,
}

/// Immutable logical workstation identity plus hosting evidence.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkstationIdentity {
    workstation_id: WorkstationId,
    craxii_id: CraxiiId,
    kind: WorkstationKind,
    generation: WorkstationGeneration,
    hosting_provider: HostingProvider,
    provider_instance_id: Option<String>,
    image_id: Option<String>,
    provisioning_revision: Option<String>,
    cpu_architecture: String,
    os_release: String,
    created_at: UtcTimestamp,
}

impl WorkstationIdentity {
    /// Validates bounded hosting evidence without treating it as identity.
    pub fn try_new(input: WorkstationIdentityInput) -> Result<Self, DomainValidationError> {
        for value in [
            input.provider_instance_id.as_deref(),
            input.image_id.as_deref(),
            input.provisioning_revision.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            validate_metadata(value, 256)?;
        }
        validate_visible_ascii(&input.cpu_architecture, 64)?;
        validate_metadata(&input.os_release, 128)?;

        Ok(Self {
            workstation_id: input.workstation_id,
            craxii_id: input.craxii_id,
            kind: WorkstationKind::Local,
            generation: input.generation,
            hosting_provider: input.hosting_provider,
            provider_instance_id: input.provider_instance_id,
            image_id: input.image_id,
            provisioning_revision: input.provisioning_revision,
            cpu_architecture: input.cpu_architecture,
            os_release: input.os_release,
            created_at: input.created_at,
        })
    }

    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn kind(&self) -> WorkstationKind {
        self.kind
    }
    pub const fn generation(&self) -> WorkstationGeneration {
        self.generation
    }
    pub const fn hosting_provider(&self) -> &HostingProvider {
        &self.hosting_provider
    }
    pub fn provider_instance_id(&self) -> Option<&str> {
        self.provider_instance_id.as_deref()
    }
    pub fn image_id(&self) -> Option<&str> {
        self.image_id.as_deref()
    }
    pub fn provisioning_revision(&self) -> Option<&str> {
        self.provisioning_revision.as_deref()
    }
    pub fn cpu_architecture(&self) -> &str {
        &self.cpu_architecture
    }
    pub fn os_release(&self) -> &str {
        &self.os_release
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

impl fmt::Debug for WorkstationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkstationIdentity")
            .field("workstation_id", &self.workstation_id)
            .field("craxii_id", &self.craxii_id)
            .field("kind", &self.kind)
            .field("generation", &self.generation)
            .field("hosting_provider", &self.hosting_provider)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

/// One workspace advertised in a capability snapshot.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceCapabilityRef {
    workspace_id: WorkspaceId,
    logical_root: LogicalPathReference,
}

impl WorkspaceCapabilityRef {
    /// Requires an absolute logical root.
    pub fn try_new(
        workspace_id: WorkspaceId,
        logical_root: LogicalPathReference,
    ) -> Result<Self, DomainValidationError> {
        if !logical_root.is_absolute() {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidCapabilitySnapshot,
            ));
        }
        Ok(Self {
            workspace_id,
            logical_root,
        })
    }

    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub const fn logical_root(&self) -> &LogicalPathReference {
        &self.logical_root
    }
}

impl fmt::Debug for WorkspaceCapabilityRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceCapabilityRef")
            .field("workspace_id", &self.workspace_id)
            .field("logical_root", &"[REDACTED]")
            .finish()
    }
}

/// The exact V1 capability-snapshot version.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkstationCapabilitiesVersion;

impl WorkstationCapabilitiesVersion {
    pub const V1: Self = Self;
    #[must_use]
    pub const fn get(self) -> i64 {
        1
    }
}

impl Serialize for WorkstationCapabilitiesVersion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(self.get())
    }
}

impl<'de> Deserialize<'de> for WorkstationCapabilitiesVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = i64::deserialize(deserializer)?;
        if value == Self::V1.get() {
            Ok(Self::V1)
        } else {
            Err(de::Error::custom(
                "workstation capabilities version must be 1",
            ))
        }
    }
}

/// Construction data for exact machine-ability flags.
pub struct WorkstationCapabilityFlagsInput {
    pub filesystem_read: bool,
    pub foreground_execute: bool,
    pub cancel_execution: bool,
    pub inspect_execution: bool,
    pub privilege_user: bool,
    pub privilege_administrative: bool,
    pub process_group_cleanup: bool,
    pub cgroup_cleanup: bool,
}

/// Exact machine abilities; these booleans do not grant authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkstationCapabilityFlags {
    filesystem_read: bool,
    foreground_execute: bool,
    cancel_execution: bool,
    inspect_execution: bool,
    privilege_user: bool,
    privilege_administrative: bool,
    process_group_cleanup: bool,
    cgroup_cleanup: bool,
}

impl WorkstationCapabilityFlags {
    /// Constructs immutable capability facts; no authority decision is made.
    #[must_use]
    pub const fn new(input: WorkstationCapabilityFlagsInput) -> Self {
        Self {
            filesystem_read: input.filesystem_read,
            foreground_execute: input.foreground_execute,
            cancel_execution: input.cancel_execution,
            inspect_execution: input.inspect_execution,
            privilege_user: input.privilege_user,
            privilege_administrative: input.privilege_administrative,
            process_group_cleanup: input.process_group_cleanup,
            cgroup_cleanup: input.cgroup_cleanup,
        }
    }

    pub const fn filesystem_read(self) -> bool {
        self.filesystem_read
    }
    pub const fn foreground_execute(self) -> bool {
        self.foreground_execute
    }
    pub const fn cancel_execution(self) -> bool {
        self.cancel_execution
    }
    pub const fn inspect_execution(self) -> bool {
        self.inspect_execution
    }
    pub const fn privilege_user(self) -> bool {
        self.privilege_user
    }
    pub const fn privilege_administrative(self) -> bool {
        self.privilege_administrative
    }
    pub const fn process_group_cleanup(self) -> bool {
        self.process_group_cleanup
    }
    pub const fn cgroup_cleanup(self) -> bool {
        self.cgroup_cleanup
    }
}

/// Signed-64-bit-safe nonnegative capability bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkstationCapabilityLimits {
    max_execution_timeout_ms: i64,
    max_stdout_bytes: i64,
    max_stderr_bytes: i64,
}

impl WorkstationCapabilityLimits {
    /// Validates all nonnegative capability bounds without truncation.
    pub fn try_new(
        max_execution_timeout_ms: u64,
        max_stdout_bytes: u64,
        max_stderr_bytes: u64,
    ) -> Result<Self, DomainValidationError> {
        let convert = |value| {
            i64::try_from(value).map_err(|_| {
                DomainValidationError::new(DomainValidationKind::InvalidCapabilitySnapshot)
            })
        };
        Ok(Self {
            max_execution_timeout_ms: convert(max_execution_timeout_ms)?,
            max_stdout_bytes: convert(max_stdout_bytes)?,
            max_stderr_bytes: convert(max_stderr_bytes)?,
        })
    }

    pub const fn max_execution_timeout_ms(self) -> u64 {
        self.max_execution_timeout_ms as u64
    }
    pub const fn max_stdout_bytes(self) -> u64 {
        self.max_stdout_bytes as u64
    }
    pub const fn max_stderr_bytes(self) -> u64 {
        self.max_stderr_bytes as u64
    }
}

/// Construction data for the immutable V1 capability snapshot.
pub struct WorkstationCapabilitiesInput {
    pub workstation_id: WorkstationId,
    pub generation: WorkstationGeneration,
    pub cpu_architecture: String,
    pub os_release: String,
    pub default_shell: LogicalPathReference,
    pub flags: WorkstationCapabilityFlags,
    pub limits: WorkstationCapabilityLimits,
    pub workspaces: Vec<WorkspaceCapabilityRef>,
}

/// Immutable V1 machine-capability evidence, not an authority grant.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkstationCapabilities {
    version: WorkstationCapabilitiesVersion,
    workstation_id: WorkstationId,
    generation: WorkstationGeneration,
    kind: WorkstationKind,
    cpu_architecture: String,
    os_release: String,
    default_shell: LogicalPathReference,
    flags: WorkstationCapabilityFlags,
    limits: WorkstationCapabilityLimits,
    workspaces: Vec<WorkspaceCapabilityRef>,
}

impl WorkstationCapabilities {
    /// Constructs V1 evidence and rejects duplicate workspace IDs.
    pub fn try_new(input: WorkstationCapabilitiesInput) -> Result<Self, DomainValidationError> {
        validate_visible_ascii(&input.cpu_architecture, 64)?;
        validate_metadata(&input.os_release, 128)?;
        if !input.default_shell.is_absolute() {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidCapabilitySnapshot,
            ));
        }
        for (index, workspace) in input.workspaces.iter().enumerate() {
            if input.workspaces[..index]
                .iter()
                .any(|prior| prior.workspace_id == workspace.workspace_id)
            {
                return Err(DomainValidationError::new(
                    DomainValidationKind::InvalidCapabilitySnapshot,
                ));
            }
        }
        Ok(Self {
            version: WorkstationCapabilitiesVersion::V1,
            workstation_id: input.workstation_id,
            generation: input.generation,
            kind: WorkstationKind::Local,
            cpu_architecture: input.cpu_architecture,
            os_release: input.os_release,
            default_shell: input.default_shell,
            flags: input.flags,
            limits: input.limits,
            workspaces: input.workspaces,
        })
    }

    pub const fn version(&self) -> WorkstationCapabilitiesVersion {
        self.version
    }
    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub const fn generation(&self) -> WorkstationGeneration {
        self.generation
    }
    pub const fn kind(&self) -> WorkstationKind {
        self.kind
    }
    pub fn cpu_architecture(&self) -> &str {
        &self.cpu_architecture
    }
    pub fn os_release(&self) -> &str {
        &self.os_release
    }
    pub const fn default_shell(&self) -> &LogicalPathReference {
        &self.default_shell
    }
    pub const fn flags(&self) -> WorkstationCapabilityFlags {
        self.flags
    }
    pub const fn limits(&self) -> WorkstationCapabilityLimits {
        self.limits
    }
    pub fn workspaces(&self) -> &[WorkspaceCapabilityRef] {
        &self.workspaces
    }
}

impl fmt::Debug for WorkstationCapabilities {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkstationCapabilities")
            .field("version", &self.version)
            .field("workstation_id", &self.workstation_id)
            .field("generation", &self.generation)
            .field("kind", &self.kind)
            .field("flags", &self.flags)
            .field("limits", &self.limits)
            .field("workspace_count", &self.workspaces.len())
            .finish_non_exhaustive()
    }
}

/// Construction data for immutable workspace identity.
pub struct WorkspaceIdentityInput {
    pub workspace_id: WorkspaceId,
    pub craxii_id: CraxiiId,
    pub workstation_id: WorkstationId,
    pub logical_name: String,
    pub logical_root: LogicalPathReference,
    pub created_at: UtcTimestamp,
}

/// Immutable logical workspace identity, independent of workstation generation.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkspaceIdentity {
    workspace_id: WorkspaceId,
    craxii_id: CraxiiId,
    workstation_id: WorkstationId,
    logical_name: String,
    logical_root: LogicalPathReference,
    lifecycle: WorkspaceLifecycle,
    created_at: UtcTimestamp,
}

impl WorkspaceIdentity {
    /// Validates stable logical name/root and fixes lifecycle to active.
    pub fn try_new(input: WorkspaceIdentityInput) -> Result<Self, DomainValidationError> {
        validate_human_label(&input.logical_name, 128)?;
        if !input.logical_root.is_absolute() {
            return Err(DomainValidationError::new(
                DomainValidationKind::InvalidEvidenceReference,
            ));
        }
        Ok(Self {
            workspace_id: input.workspace_id,
            craxii_id: input.craxii_id,
            workstation_id: input.workstation_id,
            logical_name: input.logical_name,
            logical_root: input.logical_root,
            lifecycle: WorkspaceLifecycle::Active,
            created_at: input.created_at,
        })
    }

    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }
    pub const fn craxii_id(&self) -> CraxiiId {
        self.craxii_id
    }
    pub const fn workstation_id(&self) -> WorkstationId {
        self.workstation_id
    }
    pub fn logical_name(&self) -> &str {
        &self.logical_name
    }
    pub const fn logical_root(&self) -> &LogicalPathReference {
        &self.logical_root
    }
    pub const fn lifecycle(&self) -> WorkspaceLifecycle {
        self.lifecycle
    }
    pub const fn created_at(&self) -> UtcTimestamp {
        self.created_at
    }
}

impl fmt::Debug for WorkspaceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceIdentity")
            .field("workspace_id", &self.workspace_id)
            .field("craxii_id", &self.craxii_id)
            .field("workstation_id", &self.workstation_id)
            .field("logical_root", &"[REDACTED]")
            .field("lifecycle", &self.lifecycle)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V7: &str = "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d";

    fn id<T: std::str::FromStr>(value: &str) -> T
    where
        T::Err: fmt::Debug,
    {
        value.parse().unwrap()
    }

    fn now() -> UtcTimestamp {
        "2026-08-27T12:34:56.000001Z".parse().unwrap()
    }

    fn content() -> MessageContent {
        MessageContent::try_new(vec![ContentBlock::text("hello").unwrap()]).unwrap()
    }

    use crate::domain::ContentBlock;

    #[test]
    fn exact_v0_literals_roundtrip_and_no_extra_roles_exist() {
        let literals = [
            (
                serde_json::to_string(&ConversationKind::Primary).unwrap(),
                "\"primary\"",
            ),
            (
                serde_json::to_string(&ConversationLifecycle::Active).unwrap(),
                "\"active\"",
            ),
            (
                serde_json::to_string(&MessageRole::User).unwrap(),
                "\"user\"",
            ),
            (
                serde_json::to_string(&MessageRole::Assistant).unwrap(),
                "\"assistant\"",
            ),
            (
                serde_json::to_string(&MessageRole::System).unwrap(),
                "\"system\"",
            ),
            (
                serde_json::to_string(&WorkKind::Conversational).unwrap(),
                "\"conversational\"",
            ),
            (
                serde_json::to_string(&WorkstationKind::Local).unwrap(),
                "\"local\"",
            ),
        ];
        for (actual, expected) in literals {
            assert_eq!(actual, expected);
        }
        assert!(serde_json::from_str::<MessageRole>("\"developer\"").is_err());
        assert!(serde_json::from_str::<MessageRole>("\"tool\"").is_err());
    }

    #[test]
    fn message_role_provenance_matrix_is_exact_and_content_hash_is_content_only() {
        let base_hash = content().content_sha256();
        let user = Message::try_new(MessageInput {
            message_id: id(V7),
            craxii_id: id(V7),
            conversation_id: id(V7),
            role: MessageRole::User,
            content: content(),
            produced_by_work_id: None,
            device_id: Some(id(V7)),
            client_message_id: Some(id(V7)),
            committed_at: now(),
        })
        .unwrap();
        let assistant = Message::try_new(MessageInput {
            message_id: MessageId::generate(),
            craxii_id: id(V7),
            conversation_id: id(V7),
            role: MessageRole::Assistant,
            content: content(),
            produced_by_work_id: Some(id(V7)),
            device_id: None,
            client_message_id: None,
            committed_at: "2027-01-01T00:00:00.000000Z".parse().unwrap(),
        })
        .unwrap();
        let system = Message::try_new(MessageInput {
            message_id: MessageId::generate(),
            craxii_id: CraxiiId::generate(),
            conversation_id: ConversationId::generate(),
            role: MessageRole::System,
            content: content(),
            produced_by_work_id: None,
            device_id: None,
            client_message_id: None,
            committed_at: now(),
        })
        .unwrap();
        assert_eq!(user.content_sha256(), base_hash);
        assert_eq!(assistant.content_sha256(), base_hash);
        assert_eq!(system.content_sha256(), base_hash);

        let invalid = [
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::User,
                content: content(),
                produced_by_work_id: None,
                device_id: Some(id(V7)),
                client_message_id: None,
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::User,
                content: content(),
                produced_by_work_id: None,
                device_id: None,
                client_message_id: Some(id(V7)),
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::User,
                content: content(),
                produced_by_work_id: Some(id(V7)),
                device_id: Some(id(V7)),
                client_message_id: Some(id(V7)),
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::Assistant,
                content: content(),
                produced_by_work_id: None,
                device_id: None,
                client_message_id: None,
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::Assistant,
                content: content(),
                produced_by_work_id: Some(id(V7)),
                device_id: Some(id(V7)),
                client_message_id: Some(id(V7)),
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::System,
                content: content(),
                produced_by_work_id: Some(id(V7)),
                device_id: None,
                client_message_id: None,
                committed_at: now(),
            },
            MessageInput {
                message_id: id(V7),
                craxii_id: id(V7),
                conversation_id: id(V7),
                role: MessageRole::System,
                content: content(),
                produced_by_work_id: None,
                device_id: Some(id(V7)),
                client_message_id: Some(id(V7)),
                committed_at: now(),
            },
        ];
        for input in invalid {
            assert_eq!(
                Message::try_new(input).unwrap_err().kind(),
                DomainValidationKind::InvalidMessageProvenance
            );
        }
    }

    #[test]
    fn all_work_relationship_and_actor_literals_are_exact() {
        let relationships = [
            (WorkInputRelationship::Trigger, "trigger"),
            (WorkInputRelationship::Steering, "steering"),
            (WorkInputRelationship::Supplemental, "supplemental"),
            (WorkInputRelationship::ScheduledTrigger, "scheduled_trigger"),
            (WorkInputRelationship::ExternalTrigger, "external_trigger"),
            (
                WorkInputRelationship::RecoveryInstruction,
                "recovery_instruction",
            ),
        ];
        for (value, literal) in relationships {
            let json = format!("\"{literal}\"");
            assert_eq!(serde_json::to_string(&value).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<WorkInputRelationship>(&json).unwrap(),
                value
            );
        }
        for (value, literal) in [
            (WorkInputActor::User, "user"),
            (WorkInputActor::Craxii, "craxii"),
            (WorkInputActor::System, "system"),
            (WorkInputActor::Recovery, "recovery"),
        ] {
            assert_eq!(
                serde_json::to_string(&value).unwrap(),
                format!("\"{literal}\"")
            );
        }
    }

    #[test]
    fn workstation_generation_is_positive_numeric_ordered_and_checked() {
        assert_eq!(
            WorkstationGeneration::try_new(0).unwrap_err().kind(),
            DomainValidationKind::InvalidWorkstationGeneration
        );
        let generation = WorkstationGeneration::try_new(1).unwrap();
        assert_eq!(generation.checked_increment().unwrap().get(), 2);
        assert_eq!(serde_json::to_string(&generation).unwrap(), "1");
        assert_eq!(
            serde_json::from_str::<WorkstationGeneration>("1").unwrap(),
            generation
        );
        assert_eq!(
            WorkstationGeneration::try_new(i64::MAX)
                .unwrap()
                .checked_increment()
                .unwrap_err()
                .kind(),
            DomainValidationKind::ArithmeticOverflow
        );
        let same_after_restart = generation;
        assert_eq!(same_after_restart, generation);
    }

    #[test]
    fn capabilities_are_v1_nonnegative_and_reject_duplicate_workspaces() {
        let workspace_id = id(V7);
        let root = LogicalPathReference::absolute("/srv/craxii/workspaces/main").unwrap();
        let workspace = WorkspaceCapabilityRef::try_new(workspace_id, root.clone()).unwrap();
        let limits = WorkstationCapabilityLimits::try_new(0, 0, i64::MAX as u64).unwrap();
        assert!(WorkstationCapabilityLimits::try_new(i64::MAX as u64 + 1, 0, 0).is_err());
        let flags = WorkstationCapabilityFlags::new(WorkstationCapabilityFlagsInput {
            filesystem_read: true,
            foreground_execute: true,
            cancel_execution: true,
            inspect_execution: true,
            privilege_user: true,
            privilege_administrative: false,
            process_group_cleanup: true,
            cgroup_cleanup: true,
        });
        let make = |workspaces| {
            WorkstationCapabilities::try_new(WorkstationCapabilitiesInput {
                workstation_id: id(V7),
                generation: WorkstationGeneration::try_new(1).unwrap(),
                cpu_architecture: "aarch64".into(),
                os_release: "Ubuntu 24.04".into(),
                default_shell: LogicalPathReference::absolute("/bin/bash").unwrap(),
                flags,
                limits,
                workspaces,
            })
        };
        let capabilities = make(vec![workspace.clone()]).unwrap();
        assert_eq!(capabilities.version().get(), 1);
        assert_eq!(serde_json::to_string(&capabilities.version()).unwrap(), "1");
        assert!(serde_json::from_str::<WorkstationCapabilitiesVersion>("2").is_err());
        assert_eq!(
            capabilities.workspaces()[0].logical_root().canonical(),
            root.canonical()
        );
        assert!(capabilities.flags().filesystem_read());
        assert!(make(vec![workspace.clone(), workspace]).is_err());
    }

    #[test]
    fn workspace_identity_survives_workstation_generation_changes() {
        let workstation_id = id(V7);
        let workspace = WorkspaceIdentity::try_new(WorkspaceIdentityInput {
            workspace_id: id(V7),
            craxii_id: id(V7),
            workstation_id,
            logical_name: "primary".into(),
            logical_root: LogicalPathReference::absolute("/workspace/primary").unwrap(),
            created_at: now(),
        })
        .unwrap();
        let first_generation = WorkstationGeneration::try_new(1).unwrap();
        let replacement_generation = first_generation.checked_increment().unwrap();
        assert_ne!(first_generation, replacement_generation);
        assert_eq!(workspace.workstation_id(), workstation_id);
        assert_eq!(workspace.logical_root().canonical(), "/workspace/primary");
    }

    #[test]
    fn hosting_evidence_does_not_change_logical_identity() {
        let workstation_id = id(V7);
        let base = |provider_instance_id: &str| {
            WorkstationIdentity::try_new(WorkstationIdentityInput {
                workstation_id,
                craxii_id: id(V7),
                generation: WorkstationGeneration::try_new(1).unwrap(),
                hosting_provider: HostingProvider::try_new("aws").unwrap(),
                provider_instance_id: Some(provider_instance_id.into()),
                image_id: Some("ami-123".into()),
                provisioning_revision: Some("rev-1".into()),
                cpu_architecture: "aarch64".into(),
                os_release: "Ubuntu 24.04".into(),
                created_at: now(),
            })
            .unwrap()
        };
        let first = base("i-first");
        let second = base("i-replacement-evidence");
        assert_eq!(first.workstation_id(), second.workstation_id());
        assert_eq!(first.generation(), second.generation());
        assert_ne!(first.provider_instance_id(), second.provider_instance_id());
    }

    #[test]
    fn principal_and_workspace_text_contracts_preserve_internal_spacing() {
        let principal = CraxiiPrincipal::try_new(CraxiiPrincipalInput {
            craxii_id: id(V7),
            display_name: "Craxii  Dev".into(),
            owner_label: "Owner  One".into(),
            primary_conversation_id: id(V7),
            default_workspace_id: id(V7),
            created_at: now(),
            architecture_revision: "V0.0.01-r3".into(),
            schema_revision: SchemaVersion::try_new(1).unwrap(),
        })
        .unwrap();
        assert_eq!(principal.display_name(), "Craxii  Dev");
        assert_eq!(principal.owner_label(), "Owner  One");
        for invalid in ["", " leading", "trailing ", "bad\0value", "bad\nvalue"] {
            let result = CraxiiPrincipal::try_new(CraxiiPrincipalInput {
                craxii_id: id(V7),
                display_name: invalid.into(),
                owner_label: "owner".into(),
                primary_conversation_id: id(V7),
                default_workspace_id: id(V7),
                created_at: now(),
                architecture_revision: "r3".into(),
                schema_revision: SchemaVersion::try_new(1).unwrap(),
            });
            assert!(result.is_err());
        }
    }
}
