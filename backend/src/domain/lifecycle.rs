//! Pure lifecycle state machines and terminal/recovery decisions.
//!
//! This module owns no persistence, provider, process, scheduler, or clock behavior. Its
//! decisions describe the state and semantic effects that a later owning transaction must
//! commit atomically.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    AgentStepNo, Certainty, ExecutionId, ModelAttemptReference, ModelInvocationId, NormalizedError,
    ProjectionVersion, RuntimeInstanceId, ToolAttemptReference, ToolExecutionId, ToolOrdinal,
    WorkId,
};

macro_rules! stable_lifecycle_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $literal:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// The exact frozen values in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Returns the exact frozen durable literal.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $literal),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }


        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct FrozenLifecycleVisitor;

                impl<'de> serde::de::Visitor<'de> for FrozenLifecycleVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a frozen lifecycle literal")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        match value {
                            $($literal => Ok($name::$variant)),+,
                            _ => Err(E::unknown_variant(value, &[$($literal),+])),
                        }
                    }
                }

                deserializer.deserialize_str(FrozenLifecycleVisitor)
            }
        }
    };
}

stable_lifecycle_enum! {
    /// Durable work-item lifecycle states.
    pub enum WorkState {
        Queued => "queued",
        Running => "running",
        WaitingOnModel => "waiting_on_model",
        WaitingOnTool => "waiting_on_tool",
        CancelRequested => "cancel_requested",
        Completed => "completed",
        Failed => "failed",
        Cancelled => "cancelled",
        Interrupted => "interrupted",
    }
}

impl WorkState {
    /// Whether the state is terminal and absorbing.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Whether persistence must enforce live runtime ownership.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running | Self::WaitingOnModel | Self::WaitingOnTool | Self::CancelRequested
        )
    }
}

stable_lifecycle_enum! {
    /// Durable model-invocation lifecycle states.
    pub enum ModelInvocationState {
        Requesting => "requesting",
        Streaming => "streaming",
        Completed => "completed",
        Failed => "failed",
        CancelledLocally => "cancelled_locally",
        ProviderOutcomeUnknown => "provider_outcome_unknown",
    }
}

impl ModelInvocationState {
    /// Whether the state is terminal and absorbing.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::CancelledLocally | Self::ProviderOutcomeUnknown
        )
    }
}

stable_lifecycle_enum! {
    /// Durable tool-execution lifecycle states.
    pub enum ToolExecutionState {
        Requested => "requested",
        Dispatching => "dispatching",
        Completed => "completed",
        InterruptedBeforeDispatch => "interrupted_before_dispatch",
        OutcomeUnknown => "outcome_unknown",
    }
}

impl ToolExecutionState {
    /// Whether the state is terminal and absorbing.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::InterruptedBeforeDispatch | Self::OutcomeUnknown
        )
    }
}

stable_lifecycle_enum! {
    /// Successful work completion reasons.
    pub enum WorkCompletionReason {
        Answered => "answered",
        Refused => "refused",
    }
}

stable_lifecycle_enum! {
    /// Durable cancellation origins.
    pub enum WorkCancellationReason {
        UserRequest => "user_request",
        GracefulShutdown => "graceful_shutdown",
    }
}

stable_lifecycle_enum! {
    /// Durable work interruption reasons.
    pub enum WorkInterruptionReason {
        RuntimeOwnershipLost => "runtime_ownership_lost",
        ProviderOutcomeUnknown => "provider_outcome_unknown",
        ToolInterruptedBeforeDispatch => "tool_interrupted_before_dispatch",
        ToolOutcomeUnknown => "tool_outcome_unknown",
        CleanupUnconfirmed => "cleanup_unconfirmed",
    }
}

stable_lifecycle_enum! {
    /// Closed lifecycle limits. Timing variants are semantic facts, not timers.
    pub enum LifecycleLimit {
        Context => "context",
        ModelAttempts => "model_attempts",
        AgentLoopSteps => "agent_loop_steps",
        ToolCalls => "tool_calls",
        ModelOutputItems => "model_output_items",
        ToolArgumentBytes => "tool_argument_bytes",
        ModelInvocationTime => "model_invocation_time",
        TotalWorkTime => "total_work_time",
    }
}

stable_lifecycle_enum! {
    /// Closed lifecycle classification of a terminal tool result.
    pub enum ToolResultClass {
        Success => "success",
        ValidationRejection => "validation_rejection",
        UnknownTool => "unknown_tool",
        AuthorityDenial => "authority_denial",
        FileError => "file_error",
        ProcessExit => "process_exit",
        SignalTermination => "signal_termination",
        Timeout => "timeout",
        Cancellation => "cancellation",
        SpawnFailure => "spawn_failure",
        CleanupFailure => "cleanup_failure",
    }
}

impl ToolResultClass {
    const fn allowed_before_dispatch(self) -> bool {
        matches!(
            self,
            Self::ValidationRejection
                | Self::UnknownTool
                | Self::AuthorityDenial
                | Self::FileError
                | Self::Cancellation
        )
    }

    const fn requires_confirmed_cleanup(self) -> bool {
        matches!(
            self,
            Self::ProcessExit
                | Self::SignalTermination
                | Self::Timeout
                | Self::Cancellation
                | Self::CleanupFailure
        )
    }
}

stable_lifecycle_enum! {
    /// Whether external cleanup was required and durably confirmed.
    pub enum CleanupStatus {
        NotRequired => "not_required",
        Confirmed => "confirmed",
        Unconfirmed => "unconfirmed",
    }
}

stable_lifecycle_enum! {
    /// Exact semantic work-event names emitted by transition decisions.
    pub enum WorkEventKind {
        WorkStarted => "work_started",
        WorkWaitingOnModel => "work_waiting_on_model",
        WorkWaitingOnTool => "work_waiting_on_tool",
        WorkResumed => "work_resumed",
        WorkCancelRequested => "work_cancel_requested",
        WorkCompleted => "work_completed",
        WorkFailed => "work_failed",
        WorkCancelled => "work_cancelled",
        WorkInterrupted => "work_interrupted",
    }
}

stable_lifecycle_enum! {
    /// Safe local transition-conflict classifications.
    pub enum LifecycleConflictKind {
        StaleState => "stale_state",
        StaleVersion => "stale_version",
        StaleOwner => "stale_owner",
        WrongCurrentAttempt => "wrong_current_attempt",
        IllegalTransition => "illegal_transition",
        DuplicateTerminalDecision => "duplicate_terminal_decision",
        DuplicateAttemptIdentity => "duplicate_attempt_identity",
        DuplicateAttemptNumber => "duplicate_attempt_number",
    }
}

stable_lifecycle_enum! {
    /// Safe trusted-state invariant classifications.
    pub enum LifecycleInvariantKind {
        InvalidStateShape => "invalid_state_shape",
        MissingRequiredEvidence => "missing_required_evidence",
        VersionOverflow => "version_overflow",
        UnclassifiableRecovery => "unclassifiable_recovery",
        ContradictoryProjection => "contradictory_projection",
        ImpossibleTerminalShape => "impossible_terminal_shape",
    }
}

/// A closed, payload-free lifecycle failure.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum LifecycleTransitionError {
    /// Optimistic state/ownership/identity conflict.
    Conflict(LifecycleConflictKind),
    /// Trusted projection or evidence invariant failure.
    Invariant(LifecycleInvariantKind),
}

impl LifecycleTransitionError {
    const fn conflict(kind: LifecycleConflictKind) -> Self {
        Self::Conflict(kind)
    }

    const fn invariant(kind: LifecycleInvariantKind) -> Self {
        Self::Invariant(kind)
    }

    /// Returns the conflict kind, if this is a conflict.
    #[must_use]
    pub const fn conflict_kind(self) -> Option<LifecycleConflictKind> {
        match self {
            Self::Conflict(kind) => Some(kind),
            Self::Invariant(_) => None,
        }
    }

    /// Returns the invariant kind, if this is an invariant failure.
    #[must_use]
    pub const fn invariant_kind(self) -> Option<LifecycleInvariantKind> {
        match self {
            Self::Conflict(_) => None,
            Self::Invariant(kind) => Some(kind),
        }
    }

    /// Explicitly projects lifecycle failures to the existing safe normalized vocabulary.
    #[must_use]
    pub const fn to_normalized_error(self) -> NormalizedError {
        match self {
            Self::Conflict(_) => NormalizedError::state_conflict(),
            Self::Invariant(_) => NormalizedError::internal_invariant(),
        }
    }
}

impl fmt::Display for LifecycleTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(_) => formatter.write_str("lifecycle transition conflict"),
            Self::Invariant(_) => formatter.write_str("lifecycle invariant violation"),
        }
    }
}

impl fmt::Debug for LifecycleTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(kind) => formatter
                .debug_tuple("LifecycleConflict")
                .field(kind)
                .finish(),
            Self::Invariant(kind) => formatter
                .debug_tuple("LifecycleInvariant")
                .field(kind)
                .finish(),
        }
    }
}

impl std::error::Error for LifecycleTransitionError {}

/// The one current external attempt owned by a work projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurrentWorkAttempt {
    /// No external attempt is current.
    None,
    /// The current model invocation.
    Model(ModelInvocationId),
    /// The current tool execution.
    Tool(ToolExecutionId),
}

/// Definite terminal work failure evidence.
#[derive(Debug, Eq, PartialEq)]
pub enum WorkFailureReason {
    /// A safe normalized error whose certainty is definite.
    Definite(NormalizedError),
    /// Bounded model policy exhausted all eligible provider attempts.
    ProviderExhausted,
    /// Complete model output was definitely invalid for progression/finalization.
    InvalidModelOutput(ModelOutputFailure),
    /// A closed lifecycle limit was exhausted.
    Limit(LifecycleLimit),
}

/// The reason shape permitted by a terminal work state.
#[derive(Debug, Eq, PartialEq)]
pub enum WorkTerminalReason {
    /// Successful answer or refusal.
    Completion(WorkCompletionReason),
    /// Definite terminal failure.
    Failure(WorkFailureReason),
    /// Confirmed cancellation.
    Cancellation(WorkCancellationReason),
    /// Loss of runtime or external-outcome certainty.
    Interruption(WorkInterruptionReason),
}

/// Construction data for a validated immutable work lifecycle projection.
pub struct WorkLifecycleSnapshotInput {
    pub work_id: WorkId,
    pub state: WorkState,
    pub projection_version: ProjectionVersion,
    pub runtime_owner: Option<RuntimeInstanceId>,
    pub current_attempt: CurrentWorkAttempt,
    pub cancellation_reason: Option<WorkCancellationReason>,
    pub terminal_reason: Option<WorkTerminalReason>,
}

/// Immutable current work lifecycle projection. `WorkItem` remains structural data.
#[derive(Debug, Eq, PartialEq)]
pub struct WorkLifecycleSnapshot {
    work_id: WorkId,
    state: WorkState,
    projection_version: ProjectionVersion,
    runtime_owner: Option<RuntimeInstanceId>,
    current_attempt: CurrentWorkAttempt,
    cancellation_reason: Option<WorkCancellationReason>,
    terminal_reason: Option<WorkTerminalReason>,
}

impl WorkLifecycleSnapshot {
    /// Creates the exact initial queued projection at version one.
    #[must_use]
    pub fn initial(work_id: WorkId) -> Self {
        Self {
            work_id,
            state: WorkState::Queued,
            projection_version: ProjectionVersion::try_new(1)
                .expect("one is a valid projection version"),
            runtime_owner: None,
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: None,
        }
    }

    /// Rehydrates only a projection whose ownership/current-attempt/reason shape is legal.
    pub fn try_new(input: WorkLifecycleSnapshotInput) -> Result<Self, LifecycleTransitionError> {
        validate_work_shape(
            input.state,
            input.runtime_owner,
            input.current_attempt,
            input.cancellation_reason,
            input.terminal_reason.as_ref(),
        )?;
        Ok(Self {
            work_id: input.work_id,
            state: input.state,
            projection_version: input.projection_version,
            runtime_owner: input.runtime_owner,
            current_attempt: input.current_attempt,
            cancellation_reason: input.cancellation_reason,
            terminal_reason: input.terminal_reason,
        })
    }

    pub const fn work_id(&self) -> WorkId {
        self.work_id
    }

    pub const fn state(&self) -> WorkState {
        self.state
    }

    pub const fn projection_version(&self) -> ProjectionVersion {
        self.projection_version
    }

    pub const fn runtime_owner(&self) -> Option<RuntimeInstanceId> {
        self.runtime_owner
    }

    pub const fn current_attempt(&self) -> CurrentWorkAttempt {
        self.current_attempt
    }

    pub const fn cancellation_reason(&self) -> Option<WorkCancellationReason> {
        self.cancellation_reason
    }

    pub const fn terminal_reason(&self) -> Option<&WorkTerminalReason> {
        self.terminal_reason.as_ref()
    }
}

fn validate_work_shape(
    state: WorkState,
    owner: Option<RuntimeInstanceId>,
    attempt: CurrentWorkAttempt,
    cancellation_reason: Option<WorkCancellationReason>,
    terminal_reason: Option<&WorkTerminalReason>,
) -> Result<(), LifecycleTransitionError> {
    let shape_ok = match state {
        WorkState::Queued => {
            owner.is_none()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && terminal_reason.is_none()
        }
        WorkState::Running => {
            owner.is_some()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && terminal_reason.is_none()
        }
        WorkState::WaitingOnModel => {
            owner.is_some()
                && matches!(attempt, CurrentWorkAttempt::Model(_))
                && cancellation_reason.is_none()
                && terminal_reason.is_none()
        }
        WorkState::WaitingOnTool => {
            owner.is_some()
                && matches!(attempt, CurrentWorkAttempt::Tool(_))
                && cancellation_reason.is_none()
                && terminal_reason.is_none()
        }
        WorkState::CancelRequested => {
            owner.is_some() && cancellation_reason.is_some() && terminal_reason.is_none()
        }
        WorkState::Completed => {
            owner.is_none()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && matches!(terminal_reason, Some(WorkTerminalReason::Completion(_)))
        }
        WorkState::Failed => {
            owner.is_none()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && matches!(terminal_reason, Some(WorkTerminalReason::Failure(_)))
        }
        WorkState::Cancelled => {
            owner.is_none()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && matches!(terminal_reason, Some(WorkTerminalReason::Cancellation(_)))
        }
        WorkState::Interrupted => {
            owner.is_none()
                && attempt == CurrentWorkAttempt::None
                && cancellation_reason.is_none()
                && matches!(terminal_reason, Some(WorkTerminalReason::Interruption(_)))
        }
    };

    if shape_ok {
        Ok(())
    } else if state.is_terminal() {
        Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::ImpossibleTerminalShape,
        ))
    } else {
        Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::InvalidStateShape,
        ))
    }
}

/// Exact optimistic guard checked before a Work decision is produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkTransitionGuard {
    expected_state: WorkState,
    expected_version: ProjectionVersion,
    expected_owner: Option<RuntimeInstanceId>,
    expected_current_attempt: CurrentWorkAttempt,
}

impl WorkTransitionGuard {
    #[must_use]
    pub const fn new(
        expected_state: WorkState,
        expected_version: ProjectionVersion,
        expected_owner: Option<RuntimeInstanceId>,
        expected_current_attempt: CurrentWorkAttempt,
    ) -> Self {
        Self {
            expected_state,
            expected_version,
            expected_owner,
            expected_current_attempt,
        }
    }

    #[must_use]
    pub const fn for_snapshot(snapshot: &WorkLifecycleSnapshot) -> Self {
        Self::new(
            snapshot.state,
            snapshot.projection_version,
            snapshot.runtime_owner,
            snapshot.current_attempt,
        )
    }
}

/// Evidence needed before a final answer can be committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkCompletionEvidence {
    assistant_message_in_owning_transaction: bool,
    terminal_model_invocation_completed: bool,
    cancellation_has_not_won: bool,
}

impl WorkCompletionEvidence {
    #[must_use]
    pub const fn new(
        assistant_message_in_owning_transaction: bool,
        terminal_model_invocation_completed: bool,
        cancellation_has_not_won: bool,
    ) -> Self {
        Self {
            assistant_message_in_owning_transaction,
            terminal_model_invocation_completed,
            cancellation_has_not_won,
        }
    }

    /// The only evidence shape that authorizes completion.
    pub const SATISFIED: Self = Self::new(true, true, true);

    const fn is_satisfied(self) -> bool {
        self.assistant_message_in_owning_transaction
            && self.terminal_model_invocation_completed
            && self.cancellation_has_not_won
    }
}

/// A requested Work lifecycle change. It has no adapter or persistence behavior.
#[derive(Debug)]
pub enum WorkTransitionRequest {
    Start {
        runtime_owner: RuntimeInstanceId,
    },
    WaitForModel {
        model_invocation_id: ModelInvocationId,
    },
    ResumeFromModel {
        model_invocation_id: ModelInvocationId,
    },
    WaitForTool {
        tool_execution_id: ToolExecutionId,
    },
    ResumeFromTool {
        tool_execution_id: ToolExecutionId,
    },
    RequestCancellation {
        reason: WorkCancellationReason,
    },
    Complete {
        reason: WorkCompletionReason,
        evidence: WorkCompletionEvidence,
    },
    Fail {
        reason: WorkFailureReason,
        cleanup_status: CleanupStatus,
    },
    Cancel {
        reason: WorkCancellationReason,
        cleanup_status: CleanupStatus,
    },
    Interrupt {
        reason: WorkInterruptionReason,
    },
}

impl WorkTransitionRequest {
    const fn target_state(&self) -> WorkState {
        match self {
            Self::Start { .. } | Self::ResumeFromModel { .. } | Self::ResumeFromTool { .. } => {
                WorkState::Running
            }
            Self::WaitForModel { .. } => WorkState::WaitingOnModel,
            Self::WaitForTool { .. } => WorkState::WaitingOnTool,
            Self::RequestCancellation { .. } => WorkState::CancelRequested,
            Self::Complete { .. } => WorkState::Completed,
            Self::Fail { .. } => WorkState::Failed,
            Self::Cancel { .. } => WorkState::Cancelled,
            Self::Interrupt { .. } => WorkState::Interrupted,
        }
    }

    const fn is_terminal_decision(&self) -> bool {
        self.target_state().is_terminal()
    }
}

/// Semantic effect a later owning transaction must satisfy for a Work transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkRequiredEffect {
    ClaimRuntimeOwnership,
    CommitModelIntent,
    CommitToolRequestOrDispatchIntent,
    CommitObservedModelTerminal,
    CommitObservedToolTerminal,
    CommitCancellationRequest,
    CommitFinalAnswer(FinalAnswerRequiredEffects),
    CommitFailureEvidence,
    CommitConfirmedCancellation,
    CommitInterruptionEvidence,
}

/// Exact atomic effects required for successful final-answer persistence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalAnswerRequiredEffects;

/// Individual effects in the final-answer atomic transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalAnswerEffect {
    InsertImmutableAssistantMessage,
    AppendAssistantMessageCommittedEvent,
    SetWorkCompleted,
    AppendWorkCompletedEvent,
    ClearOwnerAndCurrentAttempt,
}

impl FinalAnswerRequiredEffects {
    /// The complete exact effect set; no member may be committed separately.
    pub const ALL: [FinalAnswerEffect; 5] = [
        FinalAnswerEffect::InsertImmutableAssistantMessage,
        FinalAnswerEffect::AppendAssistantMessageCommittedEvent,
        FinalAnswerEffect::SetWorkCompleted,
        FinalAnswerEffect::AppendWorkCompletedEvent,
        FinalAnswerEffect::ClearOwnerAndCurrentAttempt,
    ];
}

/// Immutable result of a legal Work transition.
#[derive(Debug, Eq, PartialEq)]
pub struct WorkTransitionDecision {
    next: WorkLifecycleSnapshot,
    event_kind: WorkEventKind,
    required_effect: WorkRequiredEffect,
}

impl WorkTransitionDecision {
    pub const fn next(&self) -> &WorkLifecycleSnapshot {
        &self.next
    }

    pub const fn event_kind(&self) -> WorkEventKind {
        self.event_kind
    }

    pub const fn required_effect(&self) -> WorkRequiredEffect {
        self.required_effect
    }

    #[must_use]
    pub fn into_next(self) -> WorkLifecycleSnapshot {
        self.next
    }
}

/// Produces a pure guarded Work transition decision.
pub fn decide_work_transition(
    current: &WorkLifecycleSnapshot,
    guard: WorkTransitionGuard,
    request: WorkTransitionRequest,
) -> Result<WorkTransitionDecision, LifecycleTransitionError> {
    validate_work_shape(
        current.state,
        current.runtime_owner,
        current.current_attempt,
        current.cancellation_reason,
        current.terminal_reason.as_ref(),
    )?;
    check_guard(current, guard)?;

    if current.state.is_terminal() {
        let kind = if request.is_terminal_decision() {
            LifecycleConflictKind::DuplicateTerminalDecision
        } else {
            LifecycleConflictKind::IllegalTransition
        };
        return Err(LifecycleTransitionError::conflict(kind));
    }

    let target = request.target_state();
    if current.state == target || !is_legal_work_pair(current.state, target) {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    let next_version = current
        .projection_version
        .checked_increment()
        .map_err(|_| {
            LifecycleTransitionError::invariant(LifecycleInvariantKind::VersionOverflow)
        })?;

    let (owner, attempt, cancellation_reason, terminal_reason, event_kind, required_effect) =
        transition_fields(current, request)?;
    validate_work_shape(
        target,
        owner,
        attempt,
        cancellation_reason,
        terminal_reason.as_ref(),
    )?;

    Ok(WorkTransitionDecision {
        next: WorkLifecycleSnapshot {
            work_id: current.work_id,
            state: target,
            projection_version: next_version,
            runtime_owner: owner,
            current_attempt: attempt,
            cancellation_reason,
            terminal_reason,
        },
        event_kind,
        required_effect,
    })
}

fn check_guard(
    current: &WorkLifecycleSnapshot,
    guard: WorkTransitionGuard,
) -> Result<(), LifecycleTransitionError> {
    if guard.expected_state != current.state {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::StaleState,
        ));
    }
    if guard.expected_version != current.projection_version {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::StaleVersion,
        ));
    }
    if guard.expected_owner != current.runtime_owner {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::StaleOwner,
        ));
    }
    if guard.expected_current_attempt != current.current_attempt {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::WrongCurrentAttempt,
        ));
    }
    Ok(())
}

/// The exact reviewed Work transition matrix.
#[must_use]
pub const fn is_legal_work_pair(from: WorkState, to: WorkState) -> bool {
    matches!(
        (from, to),
        (WorkState::Queued, WorkState::Running)
            | (WorkState::Queued, WorkState::Cancelled)
            | (WorkState::Running, WorkState::WaitingOnModel)
            | (WorkState::WaitingOnModel, WorkState::Running)
            | (WorkState::WaitingOnModel, WorkState::Failed)
            | (WorkState::Running, WorkState::WaitingOnTool)
            | (WorkState::WaitingOnTool, WorkState::Running)
            | (WorkState::Running, WorkState::CancelRequested)
            | (WorkState::WaitingOnModel, WorkState::CancelRequested)
            | (WorkState::WaitingOnTool, WorkState::CancelRequested)
            | (WorkState::Running, WorkState::Completed)
            | (WorkState::Running, WorkState::Failed)
            | (WorkState::Running, WorkState::Interrupted)
            | (WorkState::WaitingOnModel, WorkState::Interrupted)
            | (WorkState::WaitingOnTool, WorkState::Interrupted)
            | (WorkState::CancelRequested, WorkState::Cancelled)
            | (WorkState::CancelRequested, WorkState::Interrupted)
    )
}

type WorkTransitionFields = (
    Option<RuntimeInstanceId>,
    CurrentWorkAttempt,
    Option<WorkCancellationReason>,
    Option<WorkTerminalReason>,
    WorkEventKind,
    WorkRequiredEffect,
);

fn transition_fields(
    current: &WorkLifecycleSnapshot,
    request: WorkTransitionRequest,
) -> Result<WorkTransitionFields, LifecycleTransitionError> {
    match request {
        WorkTransitionRequest::Start { runtime_owner } => Ok((
            Some(runtime_owner),
            CurrentWorkAttempt::None,
            None,
            None,
            WorkEventKind::WorkStarted,
            WorkRequiredEffect::ClaimRuntimeOwnership,
        )),
        WorkTransitionRequest::WaitForModel {
            model_invocation_id,
        } => Ok((
            current.runtime_owner,
            CurrentWorkAttempt::Model(model_invocation_id),
            None,
            None,
            WorkEventKind::WorkWaitingOnModel,
            WorkRequiredEffect::CommitModelIntent,
        )),
        WorkTransitionRequest::ResumeFromModel {
            model_invocation_id,
        } => {
            if current.current_attempt != CurrentWorkAttempt::Model(model_invocation_id) {
                return Err(LifecycleTransitionError::conflict(
                    LifecycleConflictKind::WrongCurrentAttempt,
                ));
            }
            Ok((
                current.runtime_owner,
                CurrentWorkAttempt::None,
                None,
                None,
                WorkEventKind::WorkResumed,
                WorkRequiredEffect::CommitObservedModelTerminal,
            ))
        }
        WorkTransitionRequest::WaitForTool { tool_execution_id } => Ok((
            current.runtime_owner,
            CurrentWorkAttempt::Tool(tool_execution_id),
            None,
            None,
            WorkEventKind::WorkWaitingOnTool,
            WorkRequiredEffect::CommitToolRequestOrDispatchIntent,
        )),
        WorkTransitionRequest::ResumeFromTool { tool_execution_id } => {
            if current.current_attempt != CurrentWorkAttempt::Tool(tool_execution_id) {
                return Err(LifecycleTransitionError::conflict(
                    LifecycleConflictKind::WrongCurrentAttempt,
                ));
            }
            Ok((
                current.runtime_owner,
                CurrentWorkAttempt::None,
                None,
                None,
                WorkEventKind::WorkResumed,
                WorkRequiredEffect::CommitObservedToolTerminal,
            ))
        }
        WorkTransitionRequest::RequestCancellation { reason } => Ok((
            current.runtime_owner,
            current.current_attempt,
            Some(reason),
            None,
            WorkEventKind::WorkCancelRequested,
            WorkRequiredEffect::CommitCancellationRequest,
        )),
        WorkTransitionRequest::Complete { reason, evidence } => {
            if !evidence.is_satisfied() {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            Ok((
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Completion(reason)),
                WorkEventKind::WorkCompleted,
                WorkRequiredEffect::CommitFinalAnswer(FinalAnswerRequiredEffects),
            ))
        }
        WorkTransitionRequest::Fail {
            reason,
            cleanup_status,
        } => {
            if cleanup_status == CleanupStatus::Unconfirmed
                || matches!(
                    &reason,
                    WorkFailureReason::Definite(error)
                        if error.certainty() != Certainty::Definite
                )
            {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            Ok((
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Failure(reason)),
                WorkEventKind::WorkFailed,
                WorkRequiredEffect::CommitFailureEvidence,
            ))
        }
        WorkTransitionRequest::Cancel {
            reason,
            cleanup_status,
        } => {
            if cleanup_status == CleanupStatus::Unconfirmed
                || (current.state == WorkState::Queued
                    && cleanup_status != CleanupStatus::NotRequired)
                || (current.state == WorkState::CancelRequested
                    && current.cancellation_reason != Some(reason))
            {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            Ok((
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Cancellation(reason)),
                WorkEventKind::WorkCancelled,
                WorkRequiredEffect::CommitConfirmedCancellation,
            ))
        }
        WorkTransitionRequest::Interrupt { reason } => Ok((
            None,
            CurrentWorkAttempt::None,
            None,
            Some(WorkTerminalReason::Interruption(reason)),
            WorkEventKind::WorkInterrupted,
            WorkRequiredEffect::CommitInterruptionEvidence,
        )),
    }
}

/// Immutable model lifecycle containing only identity/linkage and state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelInvocationLifecycle {
    reference: ModelAttemptReference,
    state: ModelInvocationState,
}

impl ModelInvocationLifecycle {
    /// Creates the durable initial requesting state.
    pub fn start(reference: ModelAttemptReference) -> Result<Self, LifecycleTransitionError> {
        validate_model_reference_shape(&reference)?;
        Ok(Self {
            reference,
            state: ModelInvocationState::Requesting,
        })
    }

    /// Rehydrates an immutable model lifecycle after validating retry shape.
    pub fn try_new(
        reference: ModelAttemptReference,
        state: ModelInvocationState,
    ) -> Result<Self, LifecycleTransitionError> {
        validate_model_reference_shape(&reference)?;
        Ok(Self { reference, state })
    }

    pub const fn reference(&self) -> &ModelAttemptReference {
        &self.reference
    }

    pub const fn state(&self) -> ModelInvocationState {
        self.state
    }
}

fn validate_model_reference_shape(
    reference: &ModelAttemptReference,
) -> Result<(), LifecycleTransitionError> {
    let is_first = reference.attempt_no().get() == 1;
    if is_first == reference.retry_of().is_none() {
        Ok(())
    } else {
        Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::InvalidStateShape,
        ))
    }
}

/// Requested model lifecycle change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelTransitionRequest {
    ObserveFirstProviderDelta,
    Complete {
        normalized_response_durably_observed: bool,
    },
    Fail {
        definite_terminal_failure_observed: bool,
    },
    CancelLocally {
        local_wait_cancellation_confirmed: bool,
    },
    MarkProviderOutcomeUnknown,
}

impl ModelTransitionRequest {
    const fn target_state(self) -> ModelInvocationState {
        match self {
            Self::ObserveFirstProviderDelta => ModelInvocationState::Streaming,
            Self::Complete { .. } => ModelInvocationState::Completed,
            Self::Fail { .. } => ModelInvocationState::Failed,
            Self::CancelLocally { .. } => ModelInvocationState::CancelledLocally,
            Self::MarkProviderOutcomeUnknown => ModelInvocationState::ProviderOutcomeUnknown,
        }
    }
}

/// Semantic model lifecycle effect; provider wire data is deliberately absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelLifecycleEffect {
    FirstValidProviderDeltaObserved,
    CompleteNormalizedResponseDurablyObserved,
    DefiniteTerminalFailureObserved,
    LocalWaitCancellationConfirmed,
    ProviderOutcomeClassifiedUnknown,
}

/// Immutable result of a model lifecycle transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelTransitionDecision {
    next: ModelInvocationLifecycle,
    effect: ModelLifecycleEffect,
}

impl ModelTransitionDecision {
    pub const fn next(&self) -> &ModelInvocationLifecycle {
        &self.next
    }

    pub const fn effect(&self) -> ModelLifecycleEffect {
        self.effect
    }

    #[must_use]
    pub fn into_next(self) -> ModelInvocationLifecycle {
        self.next
    }
}

/// First-delta decisions distinguish the one durable state change from later ephemeral deltas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FirstProviderDeltaDecision {
    Transition(Box<ModelTransitionDecision>),
    AlreadyStreamingNoOp,
}

/// Applies the first-provider-delta rule without creating streaming self-transitions.
pub fn decide_first_provider_delta(
    current: &ModelInvocationLifecycle,
) -> Result<FirstProviderDeltaDecision, LifecycleTransitionError> {
    match current.state {
        ModelInvocationState::Requesting => {
            decide_model_transition(current, ModelTransitionRequest::ObserveFirstProviderDelta)
                .map(Box::new)
                .map(FirstProviderDeltaDecision::Transition)
        }
        ModelInvocationState::Streaming => Ok(FirstProviderDeltaDecision::AlreadyStreamingNoOp),
        _ => Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        )),
    }
}

/// Produces a pure model lifecycle decision.
pub fn decide_model_transition(
    current: &ModelInvocationLifecycle,
    request: ModelTransitionRequest,
) -> Result<ModelTransitionDecision, LifecycleTransitionError> {
    if current.state.is_terminal() {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }
    let target = request.target_state();
    if current.state == target || !is_legal_model_pair(current.state, target) {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    let effect = match request {
        ModelTransitionRequest::ObserveFirstProviderDelta => {
            ModelLifecycleEffect::FirstValidProviderDeltaObserved
        }
        ModelTransitionRequest::Complete {
            normalized_response_durably_observed,
        } => {
            if !normalized_response_durably_observed {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            ModelLifecycleEffect::CompleteNormalizedResponseDurablyObserved
        }
        ModelTransitionRequest::Fail {
            definite_terminal_failure_observed,
        } => {
            if !definite_terminal_failure_observed {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            ModelLifecycleEffect::DefiniteTerminalFailureObserved
        }
        ModelTransitionRequest::CancelLocally {
            local_wait_cancellation_confirmed,
        } => {
            if !local_wait_cancellation_confirmed {
                return Err(LifecycleTransitionError::invariant(
                    LifecycleInvariantKind::MissingRequiredEvidence,
                ));
            }
            ModelLifecycleEffect::LocalWaitCancellationConfirmed
        }
        ModelTransitionRequest::MarkProviderOutcomeUnknown => {
            ModelLifecycleEffect::ProviderOutcomeClassifiedUnknown
        }
    };

    Ok(ModelTransitionDecision {
        next: ModelInvocationLifecycle {
            reference: current.reference.clone(),
            state: target,
        },
        effect,
    })
}

/// The exact reviewed model transition matrix.
#[must_use]
pub const fn is_legal_model_pair(from: ModelInvocationState, to: ModelInvocationState) -> bool {
    matches!(
        (from, to),
        (
            ModelInvocationState::Requesting,
            ModelInvocationState::Streaming
        ) | (
            ModelInvocationState::Requesting,
            ModelInvocationState::Completed
        ) | (
            ModelInvocationState::Streaming,
            ModelInvocationState::Completed
        ) | (
            ModelInvocationState::Requesting,
            ModelInvocationState::Failed
        ) | (
            ModelInvocationState::Streaming,
            ModelInvocationState::Failed
        ) | (
            ModelInvocationState::Requesting,
            ModelInvocationState::CancelledLocally
        ) | (
            ModelInvocationState::Streaming,
            ModelInvocationState::CancelledLocally
        ) | (
            ModelInvocationState::Requesting,
            ModelInvocationState::ProviderOutcomeUnknown
        ) | (
            ModelInvocationState::Streaming,
            ModelInvocationState::ProviderOutcomeUnknown
        )
    )
}

/// Validates and creates the requesting lifecycle for an immediate model retry.
pub fn decide_next_model_attempt(
    predecessor: &ModelInvocationLifecycle,
    candidate: ModelAttemptReference,
    existing_attempts: &[&ModelInvocationLifecycle],
) -> Result<ModelInvocationLifecycle, LifecycleTransitionError> {
    if !predecessor.state.is_terminal() {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    if candidate.model_invocation_id() == predecessor.reference.model_invocation_id()
        || existing_attempts.iter().any(|attempt| {
            attempt.reference.model_invocation_id() == candidate.model_invocation_id()
        })
    {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::DuplicateAttemptIdentity,
        ));
    }

    if existing_attempts.iter().any(|attempt| {
        attempt.reference.logical_invocation_id() == candidate.logical_invocation_id()
            && attempt.reference.work_id() == candidate.work_id()
            && attempt.reference.attempt_no() == candidate.attempt_no()
    }) || candidate.attempt_no() == predecessor.reference.attempt_no()
    {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::DuplicateAttemptNumber,
        ));
    }

    let previous = &predecessor.reference;
    let expected_attempt = previous.attempt_no().checked_increment().map_err(|_| {
        LifecycleTransitionError::invariant(LifecycleInvariantKind::VersionOverflow)
    })?;
    let exact_linkage = candidate.logical_invocation_id() == previous.logical_invocation_id()
        && candidate.work_id() == previous.work_id()
        && candidate.context_manifest_id() == previous.context_manifest_id()
        && candidate.agent_step_no() == previous.agent_step_no()
        && candidate.attempt_no() == expected_attempt
        && candidate.retry_of() == Some(previous.model_invocation_id());
    if !exact_linkage {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    ModelInvocationLifecycle::start(candidate)
}

/// Narrow tool lifecycle identity used before authority evidence exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolLifecycleReference {
    tool_execution_id: ToolExecutionId,
    execution_id: ExecutionId,
    work_id: WorkId,
    runtime_instance_id: RuntimeInstanceId,
    source_model_invocation_id: ModelInvocationId,
    agent_step_no: AgentStepNo,
    tool_ordinal: ToolOrdinal,
}

impl ToolLifecycleReference {
    #[must_use]
    pub const fn new(
        tool_execution_id: ToolExecutionId,
        execution_id: ExecutionId,
        work_id: WorkId,
        runtime_instance_id: RuntimeInstanceId,
        source_model_invocation_id: ModelInvocationId,
        agent_step_no: AgentStepNo,
        tool_ordinal: ToolOrdinal,
    ) -> Self {
        Self {
            tool_execution_id,
            execution_id,
            work_id,
            runtime_instance_id,
            source_model_invocation_id,
            agent_step_no,
            tool_ordinal,
        }
    }

    /// Narrows a post-authority Stage 3 attempt reference without losing lifecycle identity.
    #[must_use]
    pub const fn from_attempt_reference(reference: &ToolAttemptReference) -> Self {
        Self::new(
            reference.tool_execution_id(),
            reference.execution_id(),
            reference.work_id(),
            reference.runtime_instance_id(),
            reference.source_model_invocation_id(),
            reference.agent_step_no(),
            reference.tool_ordinal(),
        )
    }

    pub const fn tool_execution_id(self) -> ToolExecutionId {
        self.tool_execution_id
    }

    pub const fn execution_id(self) -> ExecutionId {
        self.execution_id
    }

    pub const fn work_id(self) -> WorkId {
        self.work_id
    }

    pub const fn runtime_instance_id(self) -> RuntimeInstanceId {
        self.runtime_instance_id
    }

    pub const fn source_model_invocation_id(self) -> ModelInvocationId {
        self.source_model_invocation_id
    }

    pub const fn agent_step_no(self) -> AgentStepNo {
        self.agent_step_no
    }

    pub const fn tool_ordinal(self) -> ToolOrdinal {
        self.tool_ordinal
    }
}

/// Immutable tool-execution lifecycle without authority or provider/process payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolExecutionLifecycle {
    reference: ToolLifecycleReference,
    state: ToolExecutionState,
}

impl ToolExecutionLifecycle {
    /// Creates the pre-dispatch requested state.
    #[must_use]
    pub const fn requested(reference: ToolLifecycleReference) -> Self {
        Self {
            reference,
            state: ToolExecutionState::Requested,
        }
    }

    /// Rehydrates an immutable lifecycle state.
    #[must_use]
    pub const fn new(reference: ToolLifecycleReference, state: ToolExecutionState) -> Self {
        Self { reference, state }
    }

    pub const fn reference(self) -> ToolLifecycleReference {
        self.reference
    }

    pub const fn state(self) -> ToolExecutionState {
        self.state
    }
}

/// Requested tool lifecycle change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolTransitionRequest {
    BeginDispatch,
    Complete {
        result: ToolResultClass,
        cleanup_status: CleanupStatus,
    },
    InterruptBeforeDispatch,
    MarkOutcomeUnknown,
}

/// Exact semantic evidence returned by a tool transition decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolLifecycleEffect {
    DispatchIntentMustCommitBeforeAction,
    TerminalResult {
        result: ToolResultClass,
        cleanup_status: CleanupStatus,
    },
    ExternalSideEffectDefinitelyAbsent,
    DispatchOutcomeOrCleanupUnknown,
}

/// Immutable result of a tool lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolTransitionDecision {
    next: ToolExecutionLifecycle,
    effect: ToolLifecycleEffect,
}

impl ToolTransitionDecision {
    pub const fn next(self) -> ToolExecutionLifecycle {
        self.next
    }

    pub const fn effect(self) -> ToolLifecycleEffect {
        self.effect
    }
}

/// Produces a pure tool lifecycle decision, closing unconfirmed post-dispatch cleanup to unknown.
pub fn decide_tool_transition(
    current: ToolExecutionLifecycle,
    request: ToolTransitionRequest,
) -> Result<ToolTransitionDecision, LifecycleTransitionError> {
    if current.state.is_terminal() {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    let (target, effect) = match request {
        ToolTransitionRequest::BeginDispatch => (
            ToolExecutionState::Dispatching,
            ToolLifecycleEffect::DispatchIntentMustCommitBeforeAction,
        ),
        ToolTransitionRequest::InterruptBeforeDispatch => (
            ToolExecutionState::InterruptedBeforeDispatch,
            ToolLifecycleEffect::ExternalSideEffectDefinitelyAbsent,
        ),
        ToolTransitionRequest::MarkOutcomeUnknown => (
            ToolExecutionState::OutcomeUnknown,
            ToolLifecycleEffect::DispatchOutcomeOrCleanupUnknown,
        ),
        ToolTransitionRequest::Complete {
            result,
            cleanup_status,
        } => {
            if current.state == ToolExecutionState::Requested {
                if !result.allowed_before_dispatch() || cleanup_status != CleanupStatus::NotRequired
                {
                    return Err(LifecycleTransitionError::invariant(
                        LifecycleInvariantKind::MissingRequiredEvidence,
                    ));
                }
                (
                    ToolExecutionState::Completed,
                    ToolLifecycleEffect::TerminalResult {
                        result,
                        cleanup_status,
                    },
                )
            } else if cleanup_status == CleanupStatus::Unconfirmed {
                (
                    ToolExecutionState::OutcomeUnknown,
                    ToolLifecycleEffect::DispatchOutcomeOrCleanupUnknown,
                )
            } else {
                if result.requires_confirmed_cleanup() && cleanup_status != CleanupStatus::Confirmed
                {
                    return Err(LifecycleTransitionError::invariant(
                        LifecycleInvariantKind::MissingRequiredEvidence,
                    ));
                }
                (
                    ToolExecutionState::Completed,
                    ToolLifecycleEffect::TerminalResult {
                        result,
                        cleanup_status,
                    },
                )
            }
        }
    };

    if current.state == target || !is_legal_tool_pair(current.state, target) {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ));
    }

    Ok(ToolTransitionDecision {
        next: ToolExecutionLifecycle {
            reference: current.reference,
            state: target,
        },
        effect,
    })
}

/// The exact reviewed tool transition matrix.
#[must_use]
pub const fn is_legal_tool_pair(from: ToolExecutionState, to: ToolExecutionState) -> bool {
    matches!(
        (from, to),
        (
            ToolExecutionState::Requested,
            ToolExecutionState::Dispatching
        ) | (ToolExecutionState::Requested, ToolExecutionState::Completed)
            | (
                ToolExecutionState::Requested,
                ToolExecutionState::InterruptedBeforeDispatch
            )
            | (
                ToolExecutionState::Dispatching,
                ToolExecutionState::Completed
            )
            | (
                ToolExecutionState::Dispatching,
                ToolExecutionState::OutcomeUnknown
            )
    )
}

stable_lifecycle_enum! {
    /// Required cancellation observation points in the agent lifecycle.
    pub enum CancellationCheckpoint {
        BeforeModel => "before_model",
        AfterProvider => "after_provider",
        BeforeToolRequested => "before_tool_requested",
        BeforeToolDispatch => "before_tool_dispatch",
        BeforeWorkstationAction => "before_workstation_action",
        WhileExternalWait => "while_external_wait",
        BeforeNextIteration => "before_next_iteration",
        BeforeFinalCommit => "before_final_commit",
    }
}

/// Pure cancellation outcome. No-op variants carry no event or new projection.
#[derive(Debug, Eq, PartialEq)]
pub enum CancellationDecision {
    DirectCancelled {
        checkpoint: CancellationCheckpoint,
        transition: WorkTransitionDecision,
    },
    CancellationRequested {
        checkpoint: CancellationCheckpoint,
        transition: WorkTransitionDecision,
    },
    AlreadyRequestedNoOp {
        checkpoint: CancellationCheckpoint,
    },
    AlreadyTerminalNoOp {
        checkpoint: CancellationCheckpoint,
        state: WorkState,
    },
}

impl CancellationDecision {
    /// No-op cancellation decisions must not append an event or increment a version.
    #[must_use]
    pub const fn transition(&self) -> Option<&WorkTransitionDecision> {
        match self {
            Self::DirectCancelled { transition, .. }
            | Self::CancellationRequested { transition, .. } => Some(transition),
            Self::AlreadyRequestedNoOp { .. } | Self::AlreadyTerminalNoOp { .. } => None,
        }
    }
}

/// Decides cancellation at any required checkpoint.
pub fn decide_cancellation(
    current: &WorkLifecycleSnapshot,
    checkpoint: CancellationCheckpoint,
    reason: WorkCancellationReason,
) -> Result<CancellationDecision, LifecycleTransitionError> {
    match current.state {
        WorkState::Queued => decide_work_transition(
            current,
            WorkTransitionGuard::for_snapshot(current),
            WorkTransitionRequest::Cancel {
                reason,
                cleanup_status: CleanupStatus::NotRequired,
            },
        )
        .map(|transition| CancellationDecision::DirectCancelled {
            checkpoint,
            transition,
        }),
        WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool => {
            decide_work_transition(
                current,
                WorkTransitionGuard::for_snapshot(current),
                WorkTransitionRequest::RequestCancellation { reason },
            )
            .map(|transition| CancellationDecision::CancellationRequested {
                checkpoint,
                transition,
            })
        }
        WorkState::CancelRequested => Ok(CancellationDecision::AlreadyRequestedNoOp { checkpoint }),
        state if state.is_terminal() => {
            Ok(CancellationDecision::AlreadyTerminalNoOp { checkpoint, state })
        }
        _ => Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::ContradictoryProjection,
        )),
    }
}

/// Progression actions explicitly blocked after cancellation has won.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkProgressionAction {
    StartModel,
    DispatchTool,
    ContinueAgentLoop,
    CommitFinalAnswer,
}

/// Checks only Work-level progression authority; it performs no action.
pub fn ensure_work_progression_allowed(
    current: &WorkLifecycleSnapshot,
    action: WorkProgressionAction,
) -> Result<(), LifecycleTransitionError> {
    let allowed = match action {
        WorkProgressionAction::StartModel
        | WorkProgressionAction::ContinueAgentLoop
        | WorkProgressionAction::CommitFinalAnswer => current.state == WorkState::Running,
        WorkProgressionAction::DispatchTool => current.state == WorkState::WaitingOnTool,
    };
    if allowed {
        Ok(())
    } else {
        Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::IllegalTransition,
        ))
    }
}

/// Whether a child-cancellation decision permits confirmed Work cancellation or interruption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationChildOutcome {
    WorkMayCancel,
    WorkMustInterrupt(WorkInterruptionReason),
}

/// Model cancellation evidence at the local provider-wait boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCancellationEvidence {
    ConfirmedLocalWaitCancellation,
    ProviderContinuityLost,
}

/// Paired child/Work semantic outcome for model cancellation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCancellationDecision {
    model: ModelTransitionDecision,
    work: CancellationChildOutcome,
}

impl ModelCancellationDecision {
    pub const fn model(&self) -> &ModelTransitionDecision {
        &self.model
    }

    pub const fn work(&self) -> CancellationChildOutcome {
        self.work
    }
}

/// Classifies confirmed local model wait cancellation versus provider continuity loss.
pub fn decide_model_cancellation(
    current: &ModelInvocationLifecycle,
    evidence: ModelCancellationEvidence,
) -> Result<ModelCancellationDecision, LifecycleTransitionError> {
    let (request, work) = match evidence {
        ModelCancellationEvidence::ConfirmedLocalWaitCancellation => (
            ModelTransitionRequest::CancelLocally {
                local_wait_cancellation_confirmed: true,
            },
            CancellationChildOutcome::WorkMayCancel,
        ),
        ModelCancellationEvidence::ProviderContinuityLost => (
            ModelTransitionRequest::MarkProviderOutcomeUnknown,
            CancellationChildOutcome::WorkMustInterrupt(
                WorkInterruptionReason::ProviderOutcomeUnknown,
            ),
        ),
    };
    decide_model_transition(current, request).map(|model| ModelCancellationDecision { model, work })
}

/// Whether a requested pre-dispatch tool cancellation is live or old-runtime recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCancellationContext {
    LiveRuntime,
    RecoveredOldRuntime,
}

/// Paired child/Work semantic outcome for tool cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolCancellationDecision {
    tool: ToolTransitionDecision,
    work: CancellationChildOutcome,
}

impl ToolCancellationDecision {
    pub const fn tool(self) -> ToolTransitionDecision {
        self.tool
    }

    pub const fn work(self) -> CancellationChildOutcome {
        self.work
    }
}

/// Classifies tool cancellation on both sides of durable dispatch intent.
pub fn decide_tool_cancellation(
    current: ToolExecutionLifecycle,
    context: ToolCancellationContext,
    cleanup_status: CleanupStatus,
) -> Result<ToolCancellationDecision, LifecycleTransitionError> {
    let (request, work) = match (current.state, context, cleanup_status) {
        (
            ToolExecutionState::Requested,
            ToolCancellationContext::RecoveredOldRuntime,
            CleanupStatus::NotRequired,
        ) => (
            ToolTransitionRequest::InterruptBeforeDispatch,
            CancellationChildOutcome::WorkMustInterrupt(
                WorkInterruptionReason::ToolInterruptedBeforeDispatch,
            ),
        ),
        (
            ToolExecutionState::Requested,
            ToolCancellationContext::LiveRuntime,
            CleanupStatus::NotRequired,
        ) => (
            ToolTransitionRequest::Complete {
                result: ToolResultClass::Cancellation,
                cleanup_status,
            },
            CancellationChildOutcome::WorkMayCancel,
        ),
        (ToolExecutionState::Dispatching, _, CleanupStatus::Confirmed) => (
            ToolTransitionRequest::Complete {
                result: ToolResultClass::Cancellation,
                cleanup_status,
            },
            CancellationChildOutcome::WorkMayCancel,
        ),
        (ToolExecutionState::Dispatching, _, CleanupStatus::Unconfirmed) => (
            ToolTransitionRequest::MarkOutcomeUnknown,
            CancellationChildOutcome::WorkMustInterrupt(WorkInterruptionReason::CleanupUnconfirmed),
        ),
        _ => {
            return Err(LifecycleTransitionError::invariant(
                LifecycleInvariantKind::MissingRequiredEvidence,
            ));
        }
    };
    decide_tool_transition(current, request).map(|tool| ToolCancellationDecision { tool, work })
}

stable_lifecycle_enum! {
    /// Mutually racing process-control observations.
    pub enum ExecutionControlEvent {
        ProcessExit => "process_exit",
        Timeout => "timeout",
        Cancellation => "cancellation",
    }
}

/// Immutable semantic first-observation latch. It performs no OS or wall-clock observation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionControlLatch {
    winner: Option<ExecutionControlEvent>,
}

impl ExecutionControlLatch {
    #[must_use]
    pub const fn new() -> Self {
        Self { winner: None }
    }

    pub const fn winner(self) -> Option<ExecutionControlEvent> {
        self.winner
    }

    /// Returns a new latch; only the first observed event changes it.
    #[must_use]
    pub const fn observe(self, event: ExecutionControlEvent) -> ExecutionControlLatchDecision {
        match self.winner {
            Some(winner) => ExecutionControlLatchDecision {
                next: self,
                winner,
                newly_latched: false,
            },
            None => ExecutionControlLatchDecision {
                next: Self {
                    winner: Some(event),
                },
                winner: event,
                newly_latched: true,
            },
        }
    }
}

/// Result of observing a semantic process-control event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionControlLatchDecision {
    next: ExecutionControlLatch,
    winner: ExecutionControlEvent,
    newly_latched: bool,
}

impl ExecutionControlLatchDecision {
    pub const fn next(self) -> ExecutionControlLatch {
        self.next
    }

    pub const fn winner(self) -> ExecutionControlEvent {
        self.winner
    }

    pub const fn newly_latched(self) -> bool {
        self.newly_latched
    }
}

/// Applies the first control latch to post-dispatch terminal classification.
pub fn decide_latched_tool_terminal(
    current: ToolExecutionLifecycle,
    latch: ExecutionControlLatch,
    cleanup_status: CleanupStatus,
) -> Result<ToolTransitionDecision, LifecycleTransitionError> {
    let result = match latch.winner {
        Some(ExecutionControlEvent::ProcessExit) => ToolResultClass::ProcessExit,
        Some(ExecutionControlEvent::Timeout) => ToolResultClass::Timeout,
        Some(ExecutionControlEvent::Cancellation) => ToolResultClass::Cancellation,
        None => {
            return Err(LifecycleTransitionError::invariant(
                LifecycleInvariantKind::MissingRequiredEvidence,
            ));
        }
    };
    decide_tool_transition(
        current,
        ToolTransitionRequest::Complete {
            result,
            cleanup_status,
        },
    )
}

/// Limit exhaustion closes to failure only when external cleanup is definite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitOutcome {
    Fail(LifecycleLimit),
    Interrupt(WorkInterruptionReason),
}

#[must_use]
pub const fn decide_limit_outcome(
    limit: LifecycleLimit,
    cleanup_status: CleanupStatus,
) -> LimitOutcome {
    match cleanup_status {
        CleanupStatus::Unconfirmed => {
            LimitOutcome::Interrupt(WorkInterruptionReason::CleanupUnconfirmed)
        }
        CleanupStatus::NotRequired | CleanupStatus::Confirmed => LimitOutcome::Fail(limit),
    }
}

/// Pure graceful-shutdown action. Queued work is deliberately retained.
#[derive(Debug, Eq, PartialEq)]
pub enum GracefulShutdownDecision {
    RetainQueued,
    RequestCancellation(CancellationDecision),
    FinalizeCancellation(WorkTransitionDecision),
    Interrupt(WorkTransitionDecision),
    AlreadyTerminal(WorkState),
}

/// Applies V0 graceful-shutdown semantics without signals, tasks, or timers.
pub fn decide_graceful_shutdown(
    current: &WorkLifecycleSnapshot,
    cleanup_status: CleanupStatus,
) -> Result<GracefulShutdownDecision, LifecycleTransitionError> {
    match current.state {
        WorkState::Queued => Ok(GracefulShutdownDecision::RetainQueued),
        WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool => {
            decide_cancellation(
                current,
                CancellationCheckpoint::WhileExternalWait,
                WorkCancellationReason::GracefulShutdown,
            )
            .map(GracefulShutdownDecision::RequestCancellation)
        }
        WorkState::CancelRequested if cleanup_status != CleanupStatus::Unconfirmed => {
            let reason = current.cancellation_reason.ok_or_else(|| {
                LifecycleTransitionError::invariant(LifecycleInvariantKind::InvalidStateShape)
            })?;
            decide_work_transition(
                current,
                WorkTransitionGuard::for_snapshot(current),
                WorkTransitionRequest::Cancel {
                    reason,
                    cleanup_status,
                },
            )
            .map(GracefulShutdownDecision::FinalizeCancellation)
        }
        WorkState::CancelRequested => decide_work_transition(
            current,
            WorkTransitionGuard::for_snapshot(current),
            WorkTransitionRequest::Interrupt {
                reason: WorkInterruptionReason::CleanupUnconfirmed,
            },
        )
        .map(GracefulShutdownDecision::Interrupt),
        state if state.is_terminal() => Ok(GracefulShutdownDecision::AlreadyTerminal(state)),
        _ => Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::ContradictoryProjection,
        )),
    }
}

/// Durable child-attempt evidence relevant to startup recovery classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryAttempt {
    None,
    Model {
        model_invocation_id: ModelInvocationId,
        state: ModelInvocationState,
        committed_response_consistent: bool,
    },
    Tool {
        tool_execution_id: ToolExecutionId,
        state: ToolExecutionState,
        committed_result_consistent: bool,
    },
}

/// Complete read-only input to pure startup recovery classification.
pub struct RecoveryInput<'a> {
    pub work: &'a WorkLifecycleSnapshot,
    pub current_runtime_id: RuntimeInstanceId,
    pub attempt: RecoveryAttempt,
    pub cleanup_status: CleanupStatus,
}

stable_lifecycle_enum! {
    /// Exact deterministic startup recovery classifications.
    pub enum RecoveryClassification {
        RetainQueued => "retain_queued",
        AlreadyTerminal => "already_terminal",
        InterruptActiveWork => "interrupt_active_work",
        MarkModelProviderOutcomeUnknownAndInterrupt => "mark_model_provider_outcome_unknown_and_interrupt",
        MarkToolInterruptedBeforeDispatchAndInterrupt => "mark_tool_interrupted_before_dispatch_and_interrupt",
        MarkToolOutcomeUnknownAndInterrupt => "mark_tool_outcome_unknown_and_interrupt",
        ReconcileCommittedToolResultWithoutExecution => "reconcile_committed_tool_result_without_execution",
        FinalizeCancellation => "finalize_cancellation",
    }
}

stable_lifecycle_enum! {
    /// Semantic requirement for later model-visible recovery uncertainty.
    pub enum SyntheticStatusRequirement {
        None => "none",
        RuntimeOwnershipLost => "runtime_ownership_lost",
        ProviderOutcomeUnknown => "provider_outcome_unknown",
        ToolInterruptedBeforeDispatch => "tool_interrupted_before_dispatch",
        ToolOutcomeUnknown => "tool_outcome_unknown",
        CleanupUnconfirmed => "cleanup_unconfirmed",
    }
}

/// Pure recovery output. It cannot authorize an adapter call, dispatch, or retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryDecision {
    classification: RecoveryClassification,
    synthetic_status: SyntheticStatusRequirement,
}

impl RecoveryDecision {
    const fn new(
        classification: RecoveryClassification,
        synthetic_status: SyntheticStatusRequirement,
    ) -> Self {
        Self {
            classification,
            synthetic_status,
        }
    }

    pub const fn classification(self) -> RecoveryClassification {
        self.classification
    }

    pub const fn synthetic_status(self) -> SyntheticStatusRequirement {
        self.synthetic_status
    }

    /// Recovery classifications never request external repetition.
    pub const fn emits_retry(self) -> bool {
        false
    }

    /// Recovery classifications never request tool/provider dispatch.
    pub const fn emits_dispatch(self) -> bool {
        false
    }
}

/// Classifies old-runtime state without calling or recommending any external adapter.
pub fn classify_recovery(
    input: RecoveryInput<'_>,
) -> Result<RecoveryDecision, LifecycleTransitionError> {
    validate_work_shape(
        input.work.state,
        input.work.runtime_owner,
        input.work.current_attempt,
        input.work.cancellation_reason,
        input.work.terminal_reason.as_ref(),
    )?;

    if input.work.runtime_owner == Some(input.current_runtime_id) {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::StaleOwner,
        ));
    }

    match input.work.state {
        WorkState::Queued => {
            if input.attempt != RecoveryAttempt::None {
                return contradictory_recovery();
            }
            Ok(RecoveryDecision::new(
                RecoveryClassification::RetainQueued,
                SyntheticStatusRequirement::None,
            ))
        }
        state if state.is_terminal() => {
            if input.attempt != RecoveryAttempt::None {
                return contradictory_recovery();
            }
            Ok(RecoveryDecision::new(
                RecoveryClassification::AlreadyTerminal,
                SyntheticStatusRequirement::None,
            ))
        }
        WorkState::Running => {
            match input.attempt {
                RecoveryAttempt::None
                | RecoveryAttempt::Model {
                    state: ModelInvocationState::Completed,
                    committed_response_consistent: true,
                    ..
                } => {}
                _ => return contradictory_recovery(),
            }
            Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::RuntimeOwnershipLost,
            ))
        }
        WorkState::WaitingOnModel => classify_model_recovery(input),
        WorkState::WaitingOnTool => classify_tool_recovery(input),
        WorkState::CancelRequested => classify_cancel_recovery(input),
        _ => Err(LifecycleTransitionError::invariant(
            LifecycleInvariantKind::UnclassifiableRecovery,
        )),
    }
}

fn classify_model_recovery(
    input: RecoveryInput<'_>,
) -> Result<RecoveryDecision, LifecycleTransitionError> {
    let CurrentWorkAttempt::Model(current_id) = input.work.current_attempt else {
        return contradictory_recovery();
    };
    let RecoveryAttempt::Model {
        model_invocation_id,
        state,
        committed_response_consistent,
    } = input.attempt
    else {
        return contradictory_recovery();
    };
    if current_id != model_invocation_id {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::WrongCurrentAttempt,
        ));
    }
    if state != ModelInvocationState::Completed && committed_response_consistent {
        return contradictory_recovery();
    }
    match state {
        ModelInvocationState::Requesting | ModelInvocationState::Streaming => {
            Ok(RecoveryDecision::new(
                RecoveryClassification::MarkModelProviderOutcomeUnknownAndInterrupt,
                SyntheticStatusRequirement::ProviderOutcomeUnknown,
            ))
        }
        ModelInvocationState::Completed if committed_response_consistent => {
            // V0 deliberately does not synthesize an assistant Message or resume the old loop.
            Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::RuntimeOwnershipLost,
            ))
        }
        ModelInvocationState::Completed => contradictory_recovery(),
        ModelInvocationState::ProviderOutcomeUnknown => Ok(RecoveryDecision::new(
            RecoveryClassification::InterruptActiveWork,
            SyntheticStatusRequirement::ProviderOutcomeUnknown,
        )),
        ModelInvocationState::Failed | ModelInvocationState::CancelledLocally => {
            Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::RuntimeOwnershipLost,
            ))
        }
    }
}

fn classify_tool_recovery(
    input: RecoveryInput<'_>,
) -> Result<RecoveryDecision, LifecycleTransitionError> {
    let CurrentWorkAttempt::Tool(current_id) = input.work.current_attempt else {
        return contradictory_recovery();
    };
    let RecoveryAttempt::Tool {
        tool_execution_id,
        state,
        committed_result_consistent,
    } = input.attempt
    else {
        return contradictory_recovery();
    };
    if current_id != tool_execution_id {
        return Err(LifecycleTransitionError::conflict(
            LifecycleConflictKind::WrongCurrentAttempt,
        ));
    }
    if state != ToolExecutionState::Completed && committed_result_consistent {
        return contradictory_recovery();
    }
    match state {
        ToolExecutionState::Requested => Ok(RecoveryDecision::new(
            RecoveryClassification::MarkToolInterruptedBeforeDispatchAndInterrupt,
            SyntheticStatusRequirement::ToolInterruptedBeforeDispatch,
        )),
        ToolExecutionState::Dispatching => Ok(RecoveryDecision::new(
            RecoveryClassification::MarkToolOutcomeUnknownAndInterrupt,
            SyntheticStatusRequirement::ToolOutcomeUnknown,
        )),
        ToolExecutionState::Completed if committed_result_consistent => Ok(RecoveryDecision::new(
            RecoveryClassification::ReconcileCommittedToolResultWithoutExecution,
            SyntheticStatusRequirement::None,
        )),
        ToolExecutionState::Completed => contradictory_recovery(),
        ToolExecutionState::InterruptedBeforeDispatch => Ok(RecoveryDecision::new(
            RecoveryClassification::InterruptActiveWork,
            SyntheticStatusRequirement::ToolInterruptedBeforeDispatch,
        )),
        ToolExecutionState::OutcomeUnknown => Ok(RecoveryDecision::new(
            RecoveryClassification::InterruptActiveWork,
            SyntheticStatusRequirement::ToolOutcomeUnknown,
        )),
    }
}

fn classify_cancel_recovery(
    input: RecoveryInput<'_>,
) -> Result<RecoveryDecision, LifecycleTransitionError> {
    match (input.work.current_attempt, input.attempt) {
        (CurrentWorkAttempt::None, RecoveryAttempt::None) => {
            if input.cleanup_status == CleanupStatus::Unconfirmed {
                Ok(RecoveryDecision::new(
                    RecoveryClassification::InterruptActiveWork,
                    SyntheticStatusRequirement::CleanupUnconfirmed,
                ))
            } else {
                Ok(RecoveryDecision::new(
                    RecoveryClassification::FinalizeCancellation,
                    SyntheticStatusRequirement::None,
                ))
            }
        }
        (
            CurrentWorkAttempt::Model(current_id),
            RecoveryAttempt::Model {
                model_invocation_id,
                state,
                committed_response_consistent,
            },
        ) if current_id == model_invocation_id => match state {
            ModelInvocationState::Requesting | ModelInvocationState::Streaming
                if !committed_response_consistent =>
            {
                Ok(RecoveryDecision::new(
                    RecoveryClassification::MarkModelProviderOutcomeUnknownAndInterrupt,
                    SyntheticStatusRequirement::ProviderOutcomeUnknown,
                ))
            }
            ModelInvocationState::Completed if !committed_response_consistent => {
                contradictory_recovery()
            }
            _ if state != ModelInvocationState::Completed && committed_response_consistent => {
                contradictory_recovery()
            }
            ModelInvocationState::ProviderOutcomeUnknown => Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::ProviderOutcomeUnknown,
            )),
            _ if input.cleanup_status == CleanupStatus::Unconfirmed => Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::CleanupUnconfirmed,
            )),
            _ => Ok(RecoveryDecision::new(
                RecoveryClassification::FinalizeCancellation,
                SyntheticStatusRequirement::None,
            )),
        },
        (
            CurrentWorkAttempt::Tool(current_id),
            RecoveryAttempt::Tool {
                tool_execution_id,
                state,
                committed_result_consistent,
            },
        ) if current_id == tool_execution_id => match state {
            _ if state != ToolExecutionState::Completed && committed_result_consistent => {
                contradictory_recovery()
            }
            ToolExecutionState::Requested => Ok(RecoveryDecision::new(
                RecoveryClassification::MarkToolInterruptedBeforeDispatchAndInterrupt,
                SyntheticStatusRequirement::ToolInterruptedBeforeDispatch,
            )),
            ToolExecutionState::Dispatching => Ok(RecoveryDecision::new(
                RecoveryClassification::MarkToolOutcomeUnknownAndInterrupt,
                SyntheticStatusRequirement::ToolOutcomeUnknown,
            )),
            ToolExecutionState::Completed if !committed_result_consistent => {
                contradictory_recovery()
            }
            ToolExecutionState::OutcomeUnknown => Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::ToolOutcomeUnknown,
            )),
            ToolExecutionState::InterruptedBeforeDispatch => Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::ToolInterruptedBeforeDispatch,
            )),
            ToolExecutionState::Completed if input.cleanup_status != CleanupStatus::Unconfirmed => {
                Ok(RecoveryDecision::new(
                    RecoveryClassification::FinalizeCancellation,
                    SyntheticStatusRequirement::None,
                ))
            }
            ToolExecutionState::Completed => Ok(RecoveryDecision::new(
                RecoveryClassification::InterruptActiveWork,
                SyntheticStatusRequirement::CleanupUnconfirmed,
            )),
        },
        (CurrentWorkAttempt::Model(_), RecoveryAttempt::Model { .. })
        | (CurrentWorkAttempt::Tool(_), RecoveryAttempt::Tool { .. }) => Err(
            LifecycleTransitionError::conflict(LifecycleConflictKind::WrongCurrentAttempt),
        ),
        _ => contradictory_recovery(),
    }
}

fn contradictory_recovery<T>() -> Result<T, LifecycleTransitionError> {
    Err(LifecycleTransitionError::invariant(
        LifecycleInvariantKind::ContradictoryProjection,
    ))
}

stable_lifecycle_enum! {
    /// Provider-response terminal observation independent of provider wire DTOs.
    pub enum ModelResponseStatus {
        Complete => "complete",
        Incomplete => "incomplete",
        Failed => "failed",
    }
}

/// Content-shape facts extracted from ordered normalized model output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelOutputFacts {
    pub has_text: bool,
    pub has_structured_output: bool,
    pub has_refusal: bool,
    pub has_tool_calls: bool,
    pub has_unknown_correctness_bearing_item: bool,
}

/// Minimal facts required for a model terminal/progression decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelTerminalFacts {
    pub response_status: ModelResponseStatus,
    pub output: ModelOutputFacts,
    pub cancellation_won: bool,
    pub exhausted_limit: Option<LifecycleLimit>,
}

stable_lifecycle_enum! {
    /// Definite normalized-model-output failures.
    pub enum ModelOutputFailure {
        Incomplete => "incomplete",
        ProviderFailed => "provider_failed",
        Empty => "empty",
        UnknownCorrectnessBearingItem => "unknown_correctness_bearing_item",
        ContradictoryRefusal => "contradictory_refusal",
    }
}

/// Pure agent-loop terminal/progression decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalDecision {
    Answered,
    Refused,
    ContinueWithTools,
    Failure(ModelOutputFailure),
    LimitReached(LifecycleLimit),
    CancelWins,
}

/// Classifies complete/incomplete/failed and mixed ordered-output facts.
#[must_use]
pub const fn decide_model_terminal(facts: ModelTerminalFacts) -> TerminalDecision {
    if facts.cancellation_won {
        return TerminalDecision::CancelWins;
    }
    if let Some(limit) = facts.exhausted_limit {
        return TerminalDecision::LimitReached(limit);
    }
    match facts.response_status {
        ModelResponseStatus::Incomplete => {
            return TerminalDecision::Failure(ModelOutputFailure::Incomplete);
        }
        ModelResponseStatus::Failed => {
            return TerminalDecision::Failure(ModelOutputFailure::ProviderFailed);
        }
        ModelResponseStatus::Complete => {}
    }
    let output = facts.output;
    if output.has_unknown_correctness_bearing_item {
        return TerminalDecision::Failure(ModelOutputFailure::UnknownCorrectnessBearingItem);
    }
    if output.has_refusal
        && (output.has_text || output.has_structured_output || output.has_tool_calls)
    {
        return TerminalDecision::Failure(ModelOutputFailure::ContradictoryRefusal);
    }
    if output.has_refusal {
        return TerminalDecision::Refused;
    }
    if output.has_tool_calls {
        return TerminalDecision::ContinueWithTools;
    }
    if output.has_text || output.has_structured_output {
        return TerminalDecision::Answered;
    }
    TerminalDecision::Failure(ModelOutputFailure::Empty)
}

/// Converts only final answer/refusal decisions to guarded Work completion.
pub fn decide_final_answer(
    current: &WorkLifecycleSnapshot,
    terminal: TerminalDecision,
    evidence: WorkCompletionEvidence,
) -> Result<Option<WorkTransitionDecision>, LifecycleTransitionError> {
    let reason = match terminal {
        TerminalDecision::Answered => WorkCompletionReason::Answered,
        TerminalDecision::Refused => WorkCompletionReason::Refused,
        TerminalDecision::CancelWins => {
            return Err(LifecycleTransitionError::conflict(
                LifecycleConflictKind::IllegalTransition,
            ));
        }
        TerminalDecision::ContinueWithTools
        | TerminalDecision::Failure(_)
        | TerminalDecision::LimitReached(_) => return Ok(None),
    };
    decide_work_transition(
        current,
        WorkTransitionGuard::for_snapshot(current),
        WorkTransitionRequest::Complete { reason, evidence },
    )
    .map(Some)
}

stable_lifecycle_enum! {
    /// Reserved Stage 2 architecture failpoint names, represented without activating hooks.
    pub enum LifecycleFailpoint {
        AfterMessageTransactionCommit => "after_message_transaction_commit",
        AfterWorkClaimCommit => "after_work_claim_commit",
        AfterContextManifestCommit => "after_context_manifest_commit",
        AfterModelIntentCommit => "after_model_intent_commit",
        AfterFirstProviderDelta => "after_first_provider_delta",
        AfterModelResponseCommit => "after_model_response_commit",
        AfterToolRequestedCommit => "after_tool_requested_commit",
        AfterToolDispatchIntentCommit => "after_tool_dispatch_intent_commit",
        AfterToolProcessSpawn => "after_tool_process_spawn",
        AfterToolProcessExitBeforeOutcomeCommit => "after_tool_process_exit_before_outcome_commit",
        AfterArtifactRenameBeforeDbCommit => "after_artifact_rename_before_db_commit",
        AfterAssistantMessageCommit => "after_assistant_message_commit",
        AfterCancelRequestedCommit => "after_cancel_requested_commit",
        DuringGracefulShutdown => "during_graceful_shutdown",
    }
}

stable_lifecycle_enum! {
    /// Static recovery meaning expected after a crash at a reserved failpoint.
    pub enum FailpointRecoveryExpectation {
        RetainQueued => "retain_queued",
        InterruptActiveWork => "interrupt_active_work",
        MarkModelProviderOutcomeUnknownAndInterrupt => "mark_model_provider_outcome_unknown_and_interrupt",
        MarkToolInterruptedBeforeDispatchAndInterrupt => "mark_tool_interrupted_before_dispatch_and_interrupt",
        MarkToolOutcomeUnknownAndInterrupt => "mark_tool_outcome_unknown_and_interrupt",
        AtomicModelAttemptBoundary => "interrupt_or_model_outcome_unknown_by_resolved_atomic_hook",
        AtomicFinalAnswerBoundary => "interrupt_or_already_terminal_by_resolved_atomic_hook",
    }
}

stable_lifecycle_enum! {
    /// Whether a compatibility failpoint name needs an explicitly resolved atomic hook.
    pub enum FailpointPhysicalInterpretation {
        Exact => "exact",
        ContextManifestAtomicAlias => "context_manifest_atomic_alias",
        ModelIntentAtomicAlias => "model_intent_atomic_alias",
        AssistantMessageAtomicAlias => "assistant_message_atomic_alias",
    }
}

/// One static semantic-map row for a reserved failpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailpointSemanticExpectation {
    failpoint: LifecycleFailpoint,
    recovery: FailpointRecoveryExpectation,
    physical_interpretation: FailpointPhysicalInterpretation,
}

impl FailpointSemanticExpectation {
    pub const fn failpoint(self) -> LifecycleFailpoint {
        self.failpoint
    }

    pub const fn recovery(self) -> FailpointRecoveryExpectation {
        self.recovery
    }

    pub const fn physical_interpretation(self) -> FailpointPhysicalInterpretation {
        self.physical_interpretation
    }
}

/// Maps every reserved failpoint to Stage 4 lifecycle/recovery semantics only.
#[must_use]
pub const fn failpoint_semantic_expectation(
    failpoint: LifecycleFailpoint,
) -> FailpointSemanticExpectation {
    use FailpointPhysicalInterpretation as Physical;
    use FailpointRecoveryExpectation as Recovery;
    let (recovery, physical_interpretation) = match failpoint {
        LifecycleFailpoint::AfterMessageTransactionCommit => {
            (Recovery::RetainQueued, Physical::Exact)
        }
        LifecycleFailpoint::AfterWorkClaimCommit => {
            (Recovery::InterruptActiveWork, Physical::Exact)
        }
        LifecycleFailpoint::AfterContextManifestCommit => (
            Recovery::AtomicModelAttemptBoundary,
            Physical::ContextManifestAtomicAlias,
        ),
        LifecycleFailpoint::AfterModelIntentCommit => (
            Recovery::AtomicModelAttemptBoundary,
            Physical::ModelIntentAtomicAlias,
        ),
        LifecycleFailpoint::AfterFirstProviderDelta => (
            Recovery::MarkModelProviderOutcomeUnknownAndInterrupt,
            Physical::Exact,
        ),
        LifecycleFailpoint::AfterModelResponseCommit => {
            (Recovery::InterruptActiveWork, Physical::Exact)
        }
        LifecycleFailpoint::AfterToolRequestedCommit => (
            Recovery::MarkToolInterruptedBeforeDispatchAndInterrupt,
            Physical::Exact,
        ),
        LifecycleFailpoint::AfterToolDispatchIntentCommit
        | LifecycleFailpoint::AfterToolProcessSpawn
        | LifecycleFailpoint::AfterToolProcessExitBeforeOutcomeCommit
        | LifecycleFailpoint::AfterArtifactRenameBeforeDbCommit => (
            Recovery::MarkToolOutcomeUnknownAndInterrupt,
            Physical::Exact,
        ),
        LifecycleFailpoint::AfterAssistantMessageCommit => (
            Recovery::AtomicFinalAnswerBoundary,
            Physical::AssistantMessageAtomicAlias,
        ),
        LifecycleFailpoint::AfterCancelRequestedCommit
        | LifecycleFailpoint::DuringGracefulShutdown => {
            (Recovery::InterruptActiveWork, Physical::Exact)
        }
    };
    FailpointSemanticExpectation {
        failpoint,
        recovery,
        physical_interpretation,
    }
}

#[cfg(test)]
mod tests {
    use serde::{Serialize, de::DeserializeOwned};

    use super::*;
    use crate::domain::{
        AttemptNo, ContextManifestId, ErrorCategory, LogicalInvocationId,
        ModelAttemptReferenceInput, ModelCapabilitySnapshot, ModelCapabilitySnapshotInput,
        ModelTargetId, ProviderId, ProviderModelId, ProviderModelReference, Retryability,
        TargetConfigurationVersion, TokenCount,
    };

    fn assert_vocabulary<T>(values: &[T])
    where
        T: Copy + DeserializeOwned + Eq + Serialize + fmt::Debug + fmt::Display,
    {
        for value in values {
            let json = serde_json::to_string(value).unwrap();
            assert_eq!(json, format!("\"{}\"", value));
            assert_eq!(serde_json::from_str::<T>(&json).unwrap(), *value);
        }
        assert!(serde_json::from_str::<T>("\"not_a_frozen_value\"").is_err());
    }

    fn valid_work(state: WorkState) -> WorkLifecycleSnapshot {
        let owner = RuntimeInstanceId::generate();
        let (runtime_owner, current_attempt, cancellation_reason, terminal_reason) = match state {
            WorkState::Queued => (None, CurrentWorkAttempt::None, None, None),
            WorkState::Running => (Some(owner), CurrentWorkAttempt::None, None, None),
            WorkState::WaitingOnModel => (
                Some(owner),
                CurrentWorkAttempt::Model(ModelInvocationId::generate()),
                None,
                None,
            ),
            WorkState::WaitingOnTool => (
                Some(owner),
                CurrentWorkAttempt::Tool(ToolExecutionId::generate()),
                None,
                None,
            ),
            WorkState::CancelRequested => (
                Some(owner),
                CurrentWorkAttempt::None,
                Some(WorkCancellationReason::UserRequest),
                None,
            ),
            WorkState::Completed => (
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Completion(
                    WorkCompletionReason::Answered,
                )),
            ),
            WorkState::Failed => (
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Failure(
                    WorkFailureReason::ProviderExhausted,
                )),
            ),
            WorkState::Cancelled => (
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Cancellation(
                    WorkCancellationReason::UserRequest,
                )),
            ),
            WorkState::Interrupted => (
                None,
                CurrentWorkAttempt::None,
                None,
                Some(WorkTerminalReason::Interruption(
                    WorkInterruptionReason::RuntimeOwnershipLost,
                )),
            ),
        };
        WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: WorkId::generate(),
            state,
            projection_version: ProjectionVersion::try_new(7).unwrap(),
            runtime_owner,
            current_attempt,
            cancellation_reason,
            terminal_reason,
        })
        .unwrap()
    }

    fn work_request_for_target(
        current: &WorkLifecycleSnapshot,
        target: WorkState,
    ) -> Option<WorkTransitionRequest> {
        match target {
            WorkState::Queued => None,
            WorkState::Running => match current.current_attempt() {
                CurrentWorkAttempt::Model(model_invocation_id) => {
                    Some(WorkTransitionRequest::ResumeFromModel {
                        model_invocation_id,
                    })
                }
                CurrentWorkAttempt::Tool(tool_execution_id) => {
                    Some(WorkTransitionRequest::ResumeFromTool { tool_execution_id })
                }
                CurrentWorkAttempt::None => Some(WorkTransitionRequest::Start {
                    runtime_owner: RuntimeInstanceId::generate(),
                }),
            },
            WorkState::WaitingOnModel => Some(WorkTransitionRequest::WaitForModel {
                model_invocation_id: ModelInvocationId::generate(),
            }),
            WorkState::WaitingOnTool => Some(WorkTransitionRequest::WaitForTool {
                tool_execution_id: ToolExecutionId::generate(),
            }),
            WorkState::CancelRequested => Some(WorkTransitionRequest::RequestCancellation {
                reason: WorkCancellationReason::UserRequest,
            }),
            WorkState::Completed => Some(WorkTransitionRequest::Complete {
                reason: WorkCompletionReason::Answered,
                evidence: WorkCompletionEvidence::SATISFIED,
            }),
            WorkState::Failed => Some(WorkTransitionRequest::Fail {
                reason: WorkFailureReason::ProviderExhausted,
                cleanup_status: CleanupStatus::NotRequired,
            }),
            WorkState::Cancelled => Some(WorkTransitionRequest::Cancel {
                reason: current
                    .cancellation_reason()
                    .unwrap_or(WorkCancellationReason::UserRequest),
                cleanup_status: if current.state() == WorkState::Queued {
                    CleanupStatus::NotRequired
                } else {
                    CleanupStatus::Confirmed
                },
            }),
            WorkState::Interrupted => Some(WorkTransitionRequest::Interrupt {
                reason: WorkInterruptionReason::RuntimeOwnershipLost,
            }),
        }
    }

    fn expected_legal_work_pairs() -> Vec<(WorkState, WorkState)> {
        vec![
            (WorkState::Queued, WorkState::Running),
            (WorkState::Queued, WorkState::Cancelled),
            (WorkState::Running, WorkState::WaitingOnModel),
            (WorkState::WaitingOnModel, WorkState::Running),
            (WorkState::WaitingOnModel, WorkState::Failed),
            (WorkState::Running, WorkState::WaitingOnTool),
            (WorkState::WaitingOnTool, WorkState::Running),
            (WorkState::Running, WorkState::CancelRequested),
            (WorkState::WaitingOnModel, WorkState::CancelRequested),
            (WorkState::WaitingOnTool, WorkState::CancelRequested),
            (WorkState::Running, WorkState::Completed),
            (WorkState::Running, WorkState::Failed),
            (WorkState::Running, WorkState::Interrupted),
            (WorkState::WaitingOnModel, WorkState::Interrupted),
            (WorkState::WaitingOnTool, WorkState::Interrupted),
            (WorkState::CancelRequested, WorkState::Cancelled),
            (WorkState::CancelRequested, WorkState::Interrupted),
        ]
    }

    fn expected_work_event(from: WorkState, to: WorkState) -> WorkEventKind {
        match (from, to) {
            (WorkState::Queued, WorkState::Running) => WorkEventKind::WorkStarted,
            (_, WorkState::WaitingOnModel) => WorkEventKind::WorkWaitingOnModel,
            (_, WorkState::WaitingOnTool) => WorkEventKind::WorkWaitingOnTool,
            (WorkState::WaitingOnModel | WorkState::WaitingOnTool, WorkState::Running) => {
                WorkEventKind::WorkResumed
            }
            (_, WorkState::CancelRequested) => WorkEventKind::WorkCancelRequested,
            (_, WorkState::Completed) => WorkEventKind::WorkCompleted,
            (_, WorkState::Failed) => WorkEventKind::WorkFailed,
            (_, WorkState::Cancelled) => WorkEventKind::WorkCancelled,
            (_, WorkState::Interrupted) => WorkEventKind::WorkInterrupted,
            _ => unreachable!(),
        }
    }

    #[test]
    fn frozen_lifecycle_vocabularies_roundtrip_exactly_and_closed() {
        assert_vocabulary(WorkState::ALL);
        assert_vocabulary(ModelInvocationState::ALL);
        assert_vocabulary(ToolExecutionState::ALL);
        assert_vocabulary(WorkCompletionReason::ALL);
        assert_vocabulary(WorkCancellationReason::ALL);
        assert_vocabulary(WorkInterruptionReason::ALL);
        assert_vocabulary(LifecycleLimit::ALL);
        assert_vocabulary(ToolResultClass::ALL);
        assert_vocabulary(CleanupStatus::ALL);
        assert_vocabulary(WorkEventKind::ALL);
        assert_vocabulary(LifecycleConflictKind::ALL);
        assert_vocabulary(LifecycleInvariantKind::ALL);
        assert_vocabulary(CancellationCheckpoint::ALL);
        assert_vocabulary(ExecutionControlEvent::ALL);
        assert_vocabulary(RecoveryClassification::ALL);
        assert_vocabulary(SyntheticStatusRequirement::ALL);
        assert_vocabulary(ModelResponseStatus::ALL);
        assert_vocabulary(ModelOutputFailure::ALL);
        assert_vocabulary(LifecycleFailpoint::ALL);
        assert_vocabulary(FailpointRecoveryExpectation::ALL);
        assert_vocabulary(FailpointPhysicalInterpretation::ALL);
    }

    #[test]
    fn work_pair_matrix_is_exact_and_has_no_self_transition() {
        let expected = expected_legal_work_pairs();
        for from in WorkState::ALL {
            for to in WorkState::ALL {
                assert_eq!(
                    is_legal_work_pair(*from, *to),
                    expected.contains(&(*from, *to)),
                    "unexpected Work pair {from} -> {to}"
                );
                if from == to {
                    assert!(!is_legal_work_pair(*from, *to));
                }
            }
        }
    }

    #[test]
    fn every_legal_work_transition_changes_state_and_increments_once() {
        for (from, to) in expected_legal_work_pairs() {
            let current = valid_work(from);
            let before = current.projection_version().get();
            let request = work_request_for_target(&current, to).unwrap();
            let decision = decide_work_transition(
                &current,
                WorkTransitionGuard::for_snapshot(&current),
                request,
            )
            .unwrap();
            assert_eq!(decision.next().state(), to, "{from} -> {to}");
            assert_eq!(decision.next().projection_version().get(), before + 1);
            assert_eq!(decision.event_kind(), expected_work_event(from, to));
            if to.is_terminal() {
                assert_eq!(decision.next().runtime_owner(), None);
                assert_eq!(decision.next().current_attempt(), CurrentWorkAttempt::None);
            }
        }
    }

    #[test]
    fn every_unlisted_expressible_work_pair_rejects() {
        for from in WorkState::ALL {
            for to in WorkState::ALL {
                if is_legal_work_pair(*from, *to) {
                    continue;
                }
                let current = valid_work(*from);
                if let Some(request) = work_request_for_target(&current, *to) {
                    let error = decide_work_transition(
                        &current,
                        WorkTransitionGuard::for_snapshot(&current),
                        request,
                    )
                    .unwrap_err();
                    assert!(matches!(error, LifecycleTransitionError::Conflict(_)));
                }
            }
        }
    }

    #[test]
    fn all_terminal_work_states_are_absorbing_and_duplicate_terminal_is_precise() {
        for state in [
            WorkState::Completed,
            WorkState::Failed,
            WorkState::Cancelled,
            WorkState::Interrupted,
        ] {
            let current = valid_work(state);
            for target in WorkState::ALL {
                if let Some(request) = work_request_for_target(&current, *target) {
                    let error = decide_work_transition(
                        &current,
                        WorkTransitionGuard::for_snapshot(&current),
                        request,
                    )
                    .unwrap_err();
                    let expected = if target.is_terminal() {
                        LifecycleConflictKind::DuplicateTerminalDecision
                    } else {
                        LifecycleConflictKind::IllegalTransition
                    };
                    assert_eq!(error.conflict_kind(), Some(expected));
                }
            }
        }
    }

    #[test]
    fn work_shape_validation_covers_queued_active_waiting_cancel_and_terminal() {
        let initial = WorkLifecycleSnapshot::initial(WorkId::generate());
        assert_eq!(initial.state(), WorkState::Queued);
        assert_eq!(initial.projection_version().get(), 1);
        assert_eq!(initial.runtime_owner(), None);
        assert_eq!(initial.current_attempt(), CurrentWorkAttempt::None);
        assert_eq!(
            WorkState::ALL
                .iter()
                .copied()
                .filter(|state| state.is_active())
                .collect::<Vec<_>>(),
            vec![
                WorkState::Running,
                WorkState::WaitingOnModel,
                WorkState::WaitingOnTool,
                WorkState::CancelRequested,
            ]
        );
        for state in WorkState::ALL {
            assert_eq!(valid_work(*state).state(), *state);
        }
        for current_attempt in [
            CurrentWorkAttempt::Model(ModelInvocationId::generate()),
            CurrentWorkAttempt::Tool(ToolExecutionId::generate()),
        ] {
            assert!(
                WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
                    work_id: WorkId::generate(),
                    state: WorkState::CancelRequested,
                    projection_version: ProjectionVersion::try_new(1).unwrap(),
                    runtime_owner: Some(RuntimeInstanceId::generate()),
                    current_attempt,
                    cancellation_reason: Some(WorkCancellationReason::UserRequest),
                    terminal_reason: None,
                })
                .is_ok()
            );
        }

        let invalid_nonterminal = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: WorkId::generate(),
            state: WorkState::WaitingOnModel,
            projection_version: ProjectionVersion::try_new(1).unwrap(),
            runtime_owner: None,
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: None,
        })
        .unwrap_err();
        assert_eq!(
            invalid_nonterminal.invariant_kind(),
            Some(LifecycleInvariantKind::InvalidStateShape)
        );

        let impossible_terminal = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: WorkId::generate(),
            state: WorkState::Completed,
            projection_version: ProjectionVersion::try_new(1).unwrap(),
            runtime_owner: Some(RuntimeInstanceId::generate()),
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: Some(WorkTerminalReason::Completion(
                WorkCompletionReason::Answered,
            )),
        })
        .unwrap_err();
        assert_eq!(
            impossible_terminal.invariant_kind(),
            Some(LifecycleInvariantKind::ImpossibleTerminalShape)
        );
        let wrong_terminal_reason = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: WorkId::generate(),
            state: WorkState::Completed,
            projection_version: ProjectionVersion::try_new(1).unwrap(),
            runtime_owner: None,
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: Some(WorkTerminalReason::Cancellation(
                WorkCancellationReason::UserRequest,
            )),
        })
        .unwrap_err();
        assert_eq!(
            wrong_terminal_reason.invariant_kind(),
            Some(LifecycleInvariantKind::ImpossibleTerminalShape)
        );
    }

    #[test]
    fn work_guard_reports_each_stale_dimension() {
        let current = valid_work(WorkState::Running);
        let request = || WorkTransitionRequest::Interrupt {
            reason: WorkInterruptionReason::RuntimeOwnershipLost,
        };
        let cases = [
            (
                WorkTransitionGuard::new(
                    WorkState::Queued,
                    current.projection_version(),
                    current.runtime_owner(),
                    current.current_attempt(),
                ),
                LifecycleConflictKind::StaleState,
            ),
            (
                WorkTransitionGuard::new(
                    current.state(),
                    ProjectionVersion::try_new(current.projection_version().get() - 1).unwrap(),
                    current.runtime_owner(),
                    current.current_attempt(),
                ),
                LifecycleConflictKind::StaleVersion,
            ),
            (
                WorkTransitionGuard::new(
                    current.state(),
                    current.projection_version(),
                    Some(RuntimeInstanceId::generate()),
                    current.current_attempt(),
                ),
                LifecycleConflictKind::StaleOwner,
            ),
            (
                WorkTransitionGuard::new(
                    current.state(),
                    current.projection_version(),
                    current.runtime_owner(),
                    CurrentWorkAttempt::Model(ModelInvocationId::generate()),
                ),
                LifecycleConflictKind::WrongCurrentAttempt,
            ),
        ];
        for (guard, expected) in cases {
            assert_eq!(
                decide_work_transition(&current, guard, request())
                    .unwrap_err()
                    .conflict_kind(),
                Some(expected)
            );
        }
    }

    #[test]
    fn work_version_overflow_is_an_internal_invariant() {
        let owner = RuntimeInstanceId::generate();
        let current = WorkLifecycleSnapshot::try_new(WorkLifecycleSnapshotInput {
            work_id: WorkId::generate(),
            state: WorkState::Running,
            projection_version: ProjectionVersion::try_new(i64::MAX).unwrap(),
            runtime_owner: Some(owner),
            current_attempt: CurrentWorkAttempt::None,
            cancellation_reason: None,
            terminal_reason: None,
        })
        .unwrap();
        assert_eq!(
            decide_work_transition(
                &current,
                WorkTransitionGuard::for_snapshot(&current),
                WorkTransitionRequest::Interrupt {
                    reason: WorkInterruptionReason::RuntimeOwnershipLost,
                },
            )
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::VersionOverflow)
        );
    }

    #[test]
    fn completion_requires_all_evidence_and_returns_exact_atomic_effects() {
        let current = valid_work(WorkState::Running);
        for evidence in [
            WorkCompletionEvidence::new(false, true, true),
            WorkCompletionEvidence::new(true, false, true),
            WorkCompletionEvidence::new(true, true, false),
        ] {
            assert_eq!(
                decide_work_transition(
                    &current,
                    WorkTransitionGuard::for_snapshot(&current),
                    WorkTransitionRequest::Complete {
                        reason: WorkCompletionReason::Answered,
                        evidence,
                    },
                )
                .unwrap_err()
                .invariant_kind(),
                Some(LifecycleInvariantKind::MissingRequiredEvidence)
            );
        }
        let decision = decide_final_answer(
            &current,
            TerminalDecision::Answered,
            WorkCompletionEvidence::SATISFIED,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            decision.required_effect(),
            WorkRequiredEffect::CommitFinalAnswer(FinalAnswerRequiredEffects)
        );
        assert_eq!(
            FinalAnswerRequiredEffects::ALL,
            [
                FinalAnswerEffect::InsertImmutableAssistantMessage,
                FinalAnswerEffect::AppendAssistantMessageCommittedEvent,
                FinalAnswerEffect::SetWorkCompleted,
                FinalAnswerEffect::AppendWorkCompletedEvent,
                FinalAnswerEffect::ClearOwnerAndCurrentAttempt,
            ]
        );
        assert_eq!(decision.event_kind(), WorkEventKind::WorkCompleted);
        let refused = decide_final_answer(
            &current,
            TerminalDecision::Refused,
            WorkCompletionEvidence::SATISFIED,
        )
        .unwrap()
        .unwrap()
        .into_next();
        assert!(matches!(
            refused.terminal_reason(),
            Some(WorkTerminalReason::Completion(
                WorkCompletionReason::Refused
            ))
        ));
    }

    #[test]
    fn definite_failure_and_limit_require_definite_cleanup() {
        let current = valid_work(WorkState::Running);
        assert_eq!(
            decide_work_transition(
                &current,
                WorkTransitionGuard::for_snapshot(&current),
                WorkTransitionRequest::Fail {
                    reason: WorkFailureReason::Limit(LifecycleLimit::TotalWorkTime),
                    cleanup_status: CleanupStatus::Unconfirmed,
                },
            )
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::MissingRequiredEvidence)
        );
        assert_eq!(
            decide_limit_outcome(
                LifecycleLimit::ModelInvocationTime,
                CleanupStatus::Unconfirmed
            ),
            LimitOutcome::Interrupt(WorkInterruptionReason::CleanupUnconfirmed)
        );
        let unknown_error = NormalizedError::provider(Certainty::OutcomeUnknown, None);
        assert_eq!(
            decide_work_transition(
                &current,
                WorkTransitionGuard::for_snapshot(&current),
                WorkTransitionRequest::Fail {
                    reason: WorkFailureReason::Definite(unknown_error),
                    cleanup_status: CleanupStatus::NotRequired,
                },
            )
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::MissingRequiredEvidence)
        );
    }

    #[test]
    fn cancellation_state_table_is_idempotent_and_has_no_noop_version_or_event() {
        for state in WorkState::ALL {
            let current = valid_work(*state);
            let decision = decide_cancellation(
                &current,
                CancellationCheckpoint::BeforeFinalCommit,
                WorkCancellationReason::UserRequest,
            )
            .unwrap();
            match state {
                WorkState::Queued => {
                    let transition = decision.transition().unwrap();
                    assert_eq!(transition.next().state(), WorkState::Cancelled);
                    assert_eq!(
                        transition.next().projection_version().get(),
                        current.projection_version().get() + 1
                    );
                }
                WorkState::Running | WorkState::WaitingOnModel | WorkState::WaitingOnTool => {
                    let transition = decision.transition().unwrap();
                    assert_eq!(transition.next().state(), WorkState::CancelRequested);
                    assert_eq!(transition.event_kind(), WorkEventKind::WorkCancelRequested);
                }
                WorkState::CancelRequested
                | WorkState::Completed
                | WorkState::Failed
                | WorkState::Cancelled
                | WorkState::Interrupted => assert!(decision.transition().is_none()),
            }
        }
    }

    #[test]
    fn cancel_requested_blocks_late_completion_and_completion_blocks_late_cancel() {
        let running = valid_work(WorkState::Running);
        let requested = match decide_cancellation(
            &running,
            CancellationCheckpoint::BeforeFinalCommit,
            WorkCancellationReason::UserRequest,
        )
        .unwrap()
        {
            CancellationDecision::CancellationRequested { transition, .. } => {
                transition.into_next()
            }
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            decide_final_answer(
                &requested,
                TerminalDecision::Answered,
                WorkCompletionEvidence::SATISFIED,
            )
            .unwrap_err()
            .conflict_kind(),
            Some(LifecycleConflictKind::IllegalTransition)
        );

        let completed = decide_final_answer(
            &running,
            TerminalDecision::Answered,
            WorkCompletionEvidence::SATISFIED,
        )
        .unwrap()
        .unwrap()
        .into_next();
        assert!(matches!(
            decide_cancellation(
                &completed,
                CancellationCheckpoint::BeforeFinalCommit,
                WorkCancellationReason::UserRequest,
            )
            .unwrap(),
            CancellationDecision::AlreadyTerminalNoOp {
                state: WorkState::Completed,
                ..
            }
        ));
        for action in [
            WorkProgressionAction::StartModel,
            WorkProgressionAction::DispatchTool,
            WorkProgressionAction::ContinueAgentLoop,
            WorkProgressionAction::CommitFinalAnswer,
        ] {
            assert_eq!(
                ensure_work_progression_allowed(&requested, action)
                    .unwrap_err()
                    .conflict_kind(),
                Some(LifecycleConflictKind::IllegalTransition)
            );
        }
    }

    #[test]
    fn graceful_shutdown_retains_queued_and_closes_cleanup_uncertainty_to_interruption() {
        let queued = valid_work(WorkState::Queued);
        assert_eq!(
            decide_graceful_shutdown(&queued, CleanupStatus::NotRequired).unwrap(),
            GracefulShutdownDecision::RetainQueued
        );
        let running = valid_work(WorkState::Running);
        let requested = match decide_graceful_shutdown(&running, CleanupStatus::Confirmed).unwrap()
        {
            GracefulShutdownDecision::RequestCancellation(decision) => match decision {
                CancellationDecision::CancellationRequested { transition, .. } => {
                    transition.into_next()
                }
                other => panic!("unexpected {other:?}"),
            },
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(
            requested.cancellation_reason(),
            Some(WorkCancellationReason::GracefulShutdown)
        );
        assert!(matches!(
            decide_graceful_shutdown(&requested, CleanupStatus::Unconfirmed).unwrap(),
            GracefulShutdownDecision::Interrupt(_)
        ));
    }

    #[derive(Clone)]
    struct ModelReferenceSeed {
        logical_invocation_id: LogicalInvocationId,
        work_id: WorkId,
        runtime_instance_id: RuntimeInstanceId,
        context_manifest_id: ContextManifestId,
        agent_step_no: AgentStepNo,
        provider_model: ProviderModelReference,
    }

    fn model_seed() -> ModelReferenceSeed {
        let capabilities = ModelCapabilitySnapshot::new(ModelCapabilitySnapshotInput {
            text_input: true,
            text_output: true,
            custom_tool_calling: true,
            streaming: true,
            ordered_output_items: true,
            structured_output: true,
            reasoning_continuation: false,
            context_window_tokens: TokenCount::try_new(8_192).unwrap(),
            max_output_tokens: TokenCount::try_new(1_024).unwrap(),
        });
        ModelReferenceSeed {
            logical_invocation_id: LogicalInvocationId::generate(),
            work_id: WorkId::generate(),
            runtime_instance_id: RuntimeInstanceId::generate(),
            context_manifest_id: ContextManifestId::generate(),
            agent_step_no: AgentStepNo::try_new(1).unwrap(),
            provider_model: ProviderModelReference::new(
                ModelTargetId::try_new("default").unwrap(),
                ProviderId::try_new("openai").unwrap(),
                ProviderModelId::try_new("gpt-test").unwrap(),
                TargetConfigurationVersion::try_new(1).unwrap(),
                capabilities,
            ),
        }
    }

    fn model_reference(
        seed: &ModelReferenceSeed,
        invocation_id: ModelInvocationId,
        attempt_no: i64,
        retry_of: Option<ModelInvocationId>,
    ) -> ModelAttemptReference {
        ModelAttemptReference::new(ModelAttemptReferenceInput {
            logical_invocation_id: seed.logical_invocation_id,
            model_invocation_id: invocation_id,
            work_id: seed.work_id,
            runtime_instance_id: seed.runtime_instance_id,
            context_manifest_id: seed.context_manifest_id,
            agent_step_no: seed.agent_step_no,
            attempt_no: AttemptNo::try_new(attempt_no).unwrap(),
            provider_model: seed.provider_model.clone(),
            retry_of,
        })
    }

    fn model_lifecycle(state: ModelInvocationState) -> ModelInvocationLifecycle {
        ModelInvocationLifecycle::try_new(
            model_reference(&model_seed(), ModelInvocationId::generate(), 1, None),
            state,
        )
        .unwrap()
    }

    fn model_request_for_target(target: ModelInvocationState) -> ModelTransitionRequest {
        match target {
            ModelInvocationState::Requesting => unreachable!(),
            ModelInvocationState::Streaming => ModelTransitionRequest::ObserveFirstProviderDelta,
            ModelInvocationState::Completed => ModelTransitionRequest::Complete {
                normalized_response_durably_observed: true,
            },
            ModelInvocationState::Failed => ModelTransitionRequest::Fail {
                definite_terminal_failure_observed: true,
            },
            ModelInvocationState::CancelledLocally => ModelTransitionRequest::CancelLocally {
                local_wait_cancellation_confirmed: true,
            },
            ModelInvocationState::ProviderOutcomeUnknown => {
                ModelTransitionRequest::MarkProviderOutcomeUnknown
            }
        }
    }

    fn expected_model_pairs() -> Vec<(ModelInvocationState, ModelInvocationState)> {
        vec![
            (
                ModelInvocationState::Requesting,
                ModelInvocationState::Streaming,
            ),
            (
                ModelInvocationState::Requesting,
                ModelInvocationState::Completed,
            ),
            (
                ModelInvocationState::Streaming,
                ModelInvocationState::Completed,
            ),
            (
                ModelInvocationState::Requesting,
                ModelInvocationState::Failed,
            ),
            (
                ModelInvocationState::Streaming,
                ModelInvocationState::Failed,
            ),
            (
                ModelInvocationState::Requesting,
                ModelInvocationState::CancelledLocally,
            ),
            (
                ModelInvocationState::Streaming,
                ModelInvocationState::CancelledLocally,
            ),
            (
                ModelInvocationState::Requesting,
                ModelInvocationState::ProviderOutcomeUnknown,
            ),
            (
                ModelInvocationState::Streaming,
                ModelInvocationState::ProviderOutcomeUnknown,
            ),
        ]
    }

    #[test]
    fn model_pair_matrix_is_exact_and_terminal_states_are_absorbing() {
        let expected = expected_model_pairs();
        for from in ModelInvocationState::ALL {
            for to in ModelInvocationState::ALL {
                assert_eq!(
                    is_legal_model_pair(*from, *to),
                    expected.contains(&(*from, *to)),
                    "unexpected model pair {from} -> {to}"
                );
                if *to != ModelInvocationState::Requesting {
                    let current = model_lifecycle(*from);
                    let actual = decide_model_transition(&current, model_request_for_target(*to));
                    assert_eq!(actual.is_ok(), expected.contains(&(*from, *to)));
                }
            }
        }
    }

    #[test]
    fn first_provider_delta_transitions_once_and_later_deltas_are_noops() {
        let requesting = model_lifecycle(ModelInvocationState::Requesting);
        let streaming = match decide_first_provider_delta(&requesting).unwrap() {
            FirstProviderDeltaDecision::Transition(decision) => decision.into_next(),
            FirstProviderDeltaDecision::AlreadyStreamingNoOp => panic!("first delta was lost"),
        };
        assert_eq!(streaming.state(), ModelInvocationState::Streaming);
        assert_eq!(
            decide_first_provider_delta(&streaming).unwrap(),
            FirstProviderDeltaDecision::AlreadyStreamingNoOp
        );
    }

    #[test]
    fn model_terminal_evidence_is_required_and_failed_attempt_cannot_resurrect() {
        let requesting = model_lifecycle(ModelInvocationState::Requesting);
        for request in [
            ModelTransitionRequest::Complete {
                normalized_response_durably_observed: false,
            },
            ModelTransitionRequest::Fail {
                definite_terminal_failure_observed: false,
            },
            ModelTransitionRequest::CancelLocally {
                local_wait_cancellation_confirmed: false,
            },
        ] {
            assert_eq!(
                decide_model_transition(&requesting, request)
                    .unwrap_err()
                    .invariant_kind(),
                Some(LifecycleInvariantKind::MissingRequiredEvidence)
            );
        }
        let failed = decide_model_transition(
            &requesting,
            ModelTransitionRequest::Fail {
                definite_terminal_failure_observed: true,
            },
        )
        .unwrap()
        .into_next();
        assert_eq!(
            decide_first_provider_delta(&failed)
                .unwrap_err()
                .conflict_kind(),
            Some(LifecycleConflictKind::IllegalTransition)
        );
    }

    #[test]
    fn model_retry_linkage_is_exact_and_predecessor_remains_terminal() {
        let seed = model_seed();
        let first_id = ModelInvocationId::generate();
        let first =
            ModelInvocationLifecycle::start(model_reference(&seed, first_id, 1, None)).unwrap();
        assert_eq!(
            decide_next_model_attempt(
                &first,
                model_reference(&seed, ModelInvocationId::generate(), 2, Some(first_id)),
                &[&first]
            )
            .unwrap_err()
            .conflict_kind(),
            Some(LifecycleConflictKind::IllegalTransition)
        );
        let terminal = decide_model_transition(
            &first,
            ModelTransitionRequest::Fail {
                definite_terminal_failure_observed: true,
            },
        )
        .unwrap()
        .into_next();
        let second_id = ModelInvocationId::generate();
        let second = decide_next_model_attempt(
            &terminal,
            model_reference(&seed, second_id, 2, Some(first_id)),
            &[&terminal],
        )
        .unwrap();
        assert_eq!(second.state(), ModelInvocationState::Requesting);
        assert_eq!(second.reference().retry_of(), Some(first_id));
        assert_eq!(terminal.state(), ModelInvocationState::Failed);

        let duplicate_identity = model_reference(&seed, first_id, 2, Some(first_id));
        assert_eq!(
            decide_next_model_attempt(&terminal, duplicate_identity, &[&terminal])
                .unwrap_err()
                .conflict_kind(),
            Some(LifecycleConflictKind::DuplicateAttemptIdentity)
        );
        let duplicate_number =
            model_reference(&seed, ModelInvocationId::generate(), 1, Some(first_id));
        assert_eq!(
            decide_next_model_attempt(&terminal, duplicate_number, &[&terminal])
                .unwrap_err()
                .conflict_kind(),
            Some(LifecycleConflictKind::DuplicateAttemptNumber)
        );
        let skipped = model_reference(&seed, ModelInvocationId::generate(), 3, Some(first_id));
        assert_eq!(
            decide_next_model_attempt(&terminal, skipped, &[&terminal])
                .unwrap_err()
                .conflict_kind(),
            Some(LifecycleConflictKind::IllegalTransition)
        );

        let mut wrong_logical = seed.clone();
        wrong_logical.logical_invocation_id = LogicalInvocationId::generate();
        let mut wrong_work = seed.clone();
        wrong_work.work_id = WorkId::generate();
        let mut wrong_context = seed.clone();
        wrong_context.context_manifest_id = ContextManifestId::generate();
        let mut wrong_step = seed.clone();
        wrong_step.agent_step_no = AgentStepNo::try_new(2).unwrap();
        let mismatched = [
            model_reference(
                &wrong_logical,
                ModelInvocationId::generate(),
                2,
                Some(first_id),
            ),
            model_reference(
                &wrong_work,
                ModelInvocationId::generate(),
                2,
                Some(first_id),
            ),
            model_reference(
                &wrong_context,
                ModelInvocationId::generate(),
                2,
                Some(first_id),
            ),
            model_reference(
                &wrong_step,
                ModelInvocationId::generate(),
                2,
                Some(first_id),
            ),
            model_reference(
                &seed,
                ModelInvocationId::generate(),
                2,
                Some(ModelInvocationId::generate()),
            ),
        ];
        for candidate in mismatched {
            assert_eq!(
                decide_next_model_attempt(&terminal, candidate, &[&terminal])
                    .unwrap_err()
                    .conflict_kind(),
                Some(LifecycleConflictKind::IllegalTransition)
            );
        }
    }

    fn tool_reference() -> ToolLifecycleReference {
        ToolLifecycleReference::new(
            ToolExecutionId::generate(),
            ExecutionId::generate(),
            WorkId::generate(),
            RuntimeInstanceId::generate(),
            ModelInvocationId::generate(),
            AgentStepNo::try_new(1).unwrap(),
            ToolOrdinal::try_new(1).unwrap(),
        )
    }

    fn expected_tool_pairs() -> Vec<(ToolExecutionState, ToolExecutionState)> {
        vec![
            (
                ToolExecutionState::Requested,
                ToolExecutionState::Dispatching,
            ),
            (ToolExecutionState::Requested, ToolExecutionState::Completed),
            (
                ToolExecutionState::Requested,
                ToolExecutionState::InterruptedBeforeDispatch,
            ),
            (
                ToolExecutionState::Dispatching,
                ToolExecutionState::Completed,
            ),
            (
                ToolExecutionState::Dispatching,
                ToolExecutionState::OutcomeUnknown,
            ),
        ]
    }

    #[test]
    fn tool_pair_matrix_is_exact_and_all_terminal_states_are_absorbing() {
        let expected = expected_tool_pairs();
        for from in ToolExecutionState::ALL {
            for to in ToolExecutionState::ALL {
                assert_eq!(
                    is_legal_tool_pair(*from, *to),
                    expected.contains(&(*from, *to)),
                    "unexpected tool pair {from} -> {to}"
                );
            }
        }
        for terminal in [
            ToolExecutionState::Completed,
            ToolExecutionState::InterruptedBeforeDispatch,
            ToolExecutionState::OutcomeUnknown,
        ] {
            let current = ToolExecutionLifecycle::new(tool_reference(), terminal);
            for request in [
                ToolTransitionRequest::BeginDispatch,
                ToolTransitionRequest::Complete {
                    result: ToolResultClass::Success,
                    cleanup_status: CleanupStatus::NotRequired,
                },
                ToolTransitionRequest::InterruptBeforeDispatch,
                ToolTransitionRequest::MarkOutcomeUnknown,
            ] {
                assert_eq!(
                    decide_tool_transition(current, request)
                        .unwrap_err()
                        .conflict_kind(),
                    Some(LifecycleConflictKind::IllegalTransition)
                );
            }
        }
    }

    #[test]
    fn tool_requested_and_dispatching_preserve_the_side_effect_boundary() {
        let requested = ToolExecutionLifecycle::requested(tool_reference());
        let rejected = decide_tool_transition(
            requested,
            ToolTransitionRequest::Complete {
                result: ToolResultClass::ValidationRejection,
                cleanup_status: CleanupStatus::NotRequired,
            },
        )
        .unwrap();
        assert_eq!(rejected.next().state(), ToolExecutionState::Completed);
        assert!(matches!(
            rejected.effect(),
            ToolLifecycleEffect::TerminalResult {
                result: ToolResultClass::ValidationRejection,
                ..
            }
        ));

        let dispatching =
            decide_tool_transition(requested, ToolTransitionRequest::BeginDispatch).unwrap();
        assert_eq!(
            dispatching.effect(),
            ToolLifecycleEffect::DispatchIntentMustCommitBeforeAction
        );
        let unknown = decide_tool_transition(
            dispatching.next(),
            ToolTransitionRequest::Complete {
                result: ToolResultClass::Timeout,
                cleanup_status: CleanupStatus::Unconfirmed,
            },
        )
        .unwrap();
        assert_eq!(unknown.next().state(), ToolExecutionState::OutcomeUnknown);
        assert_eq!(
            unknown.effect(),
            ToolLifecycleEffect::DispatchOutcomeOrCleanupUnknown
        );
        assert!(ToolExecutionState::OutcomeUnknown.is_terminal());
    }

    #[test]
    fn every_tool_result_class_has_a_definite_completed_path() {
        for result in ToolResultClass::ALL {
            let requested = ToolExecutionLifecycle::requested(tool_reference());
            let (current, cleanup_status) = if result.allowed_before_dispatch() {
                (requested, CleanupStatus::NotRequired)
            } else {
                (
                    decide_tool_transition(requested, ToolTransitionRequest::BeginDispatch)
                        .unwrap()
                        .next(),
                    if result.requires_confirmed_cleanup() {
                        CleanupStatus::Confirmed
                    } else {
                        CleanupStatus::NotRequired
                    },
                )
            };
            let decision = decide_tool_transition(
                current,
                ToolTransitionRequest::Complete {
                    result: *result,
                    cleanup_status,
                },
            )
            .unwrap();
            assert_eq!(decision.next().state(), ToolExecutionState::Completed);
        }
    }

    #[test]
    fn predispatch_interruption_is_definite_and_outcome_unknown_has_no_retry_path() {
        let requested = ToolExecutionLifecycle::requested(tool_reference());
        let interrupted =
            decide_tool_transition(requested, ToolTransitionRequest::InterruptBeforeDispatch)
                .unwrap();
        assert_eq!(
            interrupted.effect(),
            ToolLifecycleEffect::ExternalSideEffectDefinitelyAbsent
        );
        assert_eq!(
            decide_tool_transition(interrupted.next(), ToolTransitionRequest::BeginDispatch)
                .unwrap_err()
                .conflict_kind(),
            Some(LifecycleConflictKind::IllegalTransition)
        );

        let dispatching = decide_tool_transition(
            ToolExecutionLifecycle::requested(tool_reference()),
            ToolTransitionRequest::BeginDispatch,
        )
        .unwrap()
        .next();
        let unknown =
            decide_tool_transition(dispatching, ToolTransitionRequest::MarkOutcomeUnknown)
                .unwrap()
                .next();
        for target in ToolExecutionState::ALL {
            assert!(!is_legal_tool_pair(unknown.state(), *target));
        }
    }

    #[test]
    fn tool_and_model_cancellation_child_rules_close_uncertainty_conservatively() {
        let model = model_lifecycle(ModelInvocationState::Requesting);
        let local = decide_model_cancellation(
            &model,
            ModelCancellationEvidence::ConfirmedLocalWaitCancellation,
        )
        .unwrap();
        assert_eq!(
            local.model().next().state(),
            ModelInvocationState::CancelledLocally
        );
        assert_eq!(local.work(), CancellationChildOutcome::WorkMayCancel);
        let unknown =
            decide_model_cancellation(&model, ModelCancellationEvidence::ProviderContinuityLost)
                .unwrap();
        assert_eq!(
            unknown.model().next().state(),
            ModelInvocationState::ProviderOutcomeUnknown
        );
        assert_eq!(
            unknown.work(),
            CancellationChildOutcome::WorkMustInterrupt(
                WorkInterruptionReason::ProviderOutcomeUnknown
            )
        );

        let requested = ToolExecutionLifecycle::requested(tool_reference());
        let recovered = decide_tool_cancellation(
            requested,
            ToolCancellationContext::RecoveredOldRuntime,
            CleanupStatus::NotRequired,
        )
        .unwrap();
        assert_eq!(
            recovered.tool().next().state(),
            ToolExecutionState::InterruptedBeforeDispatch
        );
        let dispatching = decide_tool_transition(requested, ToolTransitionRequest::BeginDispatch)
            .unwrap()
            .next();
        let uncertain = decide_tool_cancellation(
            dispatching,
            ToolCancellationContext::LiveRuntime,
            CleanupStatus::Unconfirmed,
        )
        .unwrap();
        assert_eq!(
            uncertain.tool().next().state(),
            ToolExecutionState::OutcomeUnknown
        );
        assert_eq!(
            uncertain.work(),
            CancellationChildOutcome::WorkMustInterrupt(WorkInterruptionReason::CleanupUnconfirmed)
        );
    }

    #[test]
    fn exit_timeout_cancellation_latch_is_first_observation_wins() {
        for first in ExecutionControlEvent::ALL {
            let first_decision = ExecutionControlLatch::new().observe(*first);
            assert!(first_decision.newly_latched());
            assert_eq!(first_decision.winner(), *first);
            for later in ExecutionControlEvent::ALL {
                let later_decision = first_decision.next().observe(*later);
                assert!(!later_decision.newly_latched());
                assert_eq!(later_decision.winner(), *first);
            }
        }
        let dispatching = decide_tool_transition(
            ToolExecutionLifecycle::requested(tool_reference()),
            ToolTransitionRequest::BeginDispatch,
        )
        .unwrap()
        .next();
        let latch = ExecutionControlLatch::new()
            .observe(ExecutionControlEvent::Timeout)
            .next();
        assert_eq!(
            decide_latched_tool_terminal(dispatching, latch, CleanupStatus::Unconfirmed)
                .unwrap()
                .next()
                .state(),
            ToolExecutionState::OutcomeUnknown
        );
    }

    fn recovery_input<'a>(
        work: &'a WorkLifecycleSnapshot,
        attempt: RecoveryAttempt,
        cleanup_status: CleanupStatus,
    ) -> RecoveryInput<'a> {
        RecoveryInput {
            work,
            current_runtime_id: RuntimeInstanceId::generate(),
            attempt,
            cleanup_status,
        }
    }

    #[test]
    fn recovery_retains_queued_accepts_terminal_and_interrupts_old_running() {
        let queued = valid_work(WorkState::Queued);
        let decision = classify_recovery(recovery_input(
            &queued,
            RecoveryAttempt::None,
            CleanupStatus::NotRequired,
        ))
        .unwrap();
        assert_eq!(
            decision.classification(),
            RecoveryClassification::RetainQueued
        );
        assert!(!decision.emits_retry());
        assert!(!decision.emits_dispatch());

        for terminal in [
            WorkState::Completed,
            WorkState::Failed,
            WorkState::Cancelled,
            WorkState::Interrupted,
        ] {
            assert_eq!(
                classify_recovery(recovery_input(
                    &valid_work(terminal),
                    RecoveryAttempt::None,
                    CleanupStatus::NotRequired,
                ))
                .unwrap()
                .classification(),
                RecoveryClassification::AlreadyTerminal
            );
        }

        let running = valid_work(WorkState::Running);
        let interrupted = classify_recovery(recovery_input(
            &running,
            RecoveryAttempt::None,
            CleanupStatus::NotRequired,
        ))
        .unwrap();
        assert_eq!(
            interrupted.classification(),
            RecoveryClassification::InterruptActiveWork
        );
        assert_eq!(
            interrupted.synthetic_status(),
            SyntheticStatusRequirement::RuntimeOwnershipLost
        );

        let committed_model_response = classify_recovery(recovery_input(
            &running,
            RecoveryAttempt::Model {
                model_invocation_id: ModelInvocationId::generate(),
                state: ModelInvocationState::Completed,
                committed_response_consistent: true,
            },
            CleanupStatus::NotRequired,
        ))
        .unwrap();
        assert_eq!(
            committed_model_response.classification(),
            RecoveryClassification::InterruptActiveWork
        );
        assert!(!committed_model_response.emits_dispatch());
    }

    #[test]
    fn model_recovery_classifies_inflight_unknown_and_committed_response_interruption() {
        let waiting = valid_work(WorkState::WaitingOnModel);
        let CurrentWorkAttempt::Model(model_invocation_id) = waiting.current_attempt() else {
            unreachable!()
        };
        for state in [
            ModelInvocationState::Requesting,
            ModelInvocationState::Streaming,
        ] {
            let decision = classify_recovery(recovery_input(
                &waiting,
                RecoveryAttempt::Model {
                    model_invocation_id,
                    state,
                    committed_response_consistent: false,
                },
                CleanupStatus::Unconfirmed,
            ))
            .unwrap();
            assert_eq!(
                decision.classification(),
                RecoveryClassification::MarkModelProviderOutcomeUnknownAndInterrupt
            );
            assert_eq!(
                decision.synthetic_status(),
                SyntheticStatusRequirement::ProviderOutcomeUnknown
            );
        }
        let committed = classify_recovery(recovery_input(
            &waiting,
            RecoveryAttempt::Model {
                model_invocation_id,
                state: ModelInvocationState::Completed,
                committed_response_consistent: true,
            },
            CleanupStatus::NotRequired,
        ))
        .unwrap();
        assert_eq!(
            committed.classification(),
            RecoveryClassification::InterruptActiveWork
        );
        assert!(!committed.emits_dispatch());
    }

    #[test]
    fn tool_recovery_covers_before_dispatch_after_dispatch_and_committed_reconciliation() {
        let waiting = valid_work(WorkState::WaitingOnTool);
        let CurrentWorkAttempt::Tool(tool_execution_id) = waiting.current_attempt() else {
            unreachable!()
        };
        let cases = [
            (
                ToolExecutionState::Requested,
                false,
                RecoveryClassification::MarkToolInterruptedBeforeDispatchAndInterrupt,
            ),
            (
                ToolExecutionState::Dispatching,
                false,
                RecoveryClassification::MarkToolOutcomeUnknownAndInterrupt,
            ),
            (
                ToolExecutionState::Completed,
                true,
                RecoveryClassification::ReconcileCommittedToolResultWithoutExecution,
            ),
        ];
        for (state, committed_result_consistent, expected) in cases {
            let decision = classify_recovery(recovery_input(
                &waiting,
                RecoveryAttempt::Tool {
                    tool_execution_id,
                    state,
                    committed_result_consistent,
                },
                CleanupStatus::Unconfirmed,
            ))
            .unwrap();
            assert_eq!(decision.classification(), expected);
            assert!(!decision.emits_retry());
            assert!(!decision.emits_dispatch());
        }
    }

    #[test]
    fn recovery_rejects_same_runtime_mismatch_and_contradictory_durable_shapes() {
        let running = valid_work(WorkState::Running);
        let same_runtime = RecoveryInput {
            work: &running,
            current_runtime_id: running.runtime_owner().unwrap(),
            attempt: RecoveryAttempt::None,
            cleanup_status: CleanupStatus::NotRequired,
        };
        assert_eq!(
            classify_recovery(same_runtime).unwrap_err().conflict_kind(),
            Some(LifecycleConflictKind::StaleOwner)
        );

        let waiting = valid_work(WorkState::WaitingOnTool);
        let CurrentWorkAttempt::Tool(tool_execution_id) = waiting.current_attempt() else {
            unreachable!()
        };
        assert_eq!(
            classify_recovery(recovery_input(
                &waiting,
                RecoveryAttempt::None,
                CleanupStatus::NotRequired,
            ))
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::ContradictoryProjection)
        );
        assert_eq!(
            classify_recovery(recovery_input(
                &waiting,
                RecoveryAttempt::Tool {
                    tool_execution_id,
                    state: ToolExecutionState::Dispatching,
                    committed_result_consistent: true,
                },
                CleanupStatus::Confirmed,
            ))
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::ContradictoryProjection)
        );
        assert_eq!(
            classify_recovery(recovery_input(
                &waiting,
                RecoveryAttempt::Tool {
                    tool_execution_id,
                    state: ToolExecutionState::Completed,
                    committed_result_consistent: false,
                },
                CleanupStatus::Confirmed,
            ))
            .unwrap_err()
            .invariant_kind(),
            Some(LifecycleInvariantKind::ContradictoryProjection)
        );
    }

    #[test]
    fn cancel_requested_recovery_finalizes_only_with_proven_cleanup() {
        let cancelled_pending = valid_work(WorkState::CancelRequested);
        assert_eq!(
            classify_recovery(recovery_input(
                &cancelled_pending,
                RecoveryAttempt::None,
                CleanupStatus::Confirmed,
            ))
            .unwrap()
            .classification(),
            RecoveryClassification::FinalizeCancellation
        );
        let uncertain = classify_recovery(recovery_input(
            &cancelled_pending,
            RecoveryAttempt::None,
            CleanupStatus::Unconfirmed,
        ))
        .unwrap();
        assert_eq!(
            uncertain.classification(),
            RecoveryClassification::InterruptActiveWork
        );
        assert_eq!(
            uncertain.synthetic_status(),
            SyntheticStatusRequirement::CleanupUnconfirmed
        );
    }

    fn terminal_facts(
        response_status: ModelResponseStatus,
        output: ModelOutputFacts,
    ) -> ModelTerminalFacts {
        ModelTerminalFacts {
            response_status,
            output,
            cancellation_won: false,
            exhausted_limit: None,
        }
    }

    #[test]
    fn model_terminal_output_fact_matrix_is_closed_and_ordered_by_safety() {
        let text = ModelOutputFacts {
            has_text: true,
            ..ModelOutputFacts::default()
        };
        let structured = ModelOutputFacts {
            has_structured_output: true,
            ..ModelOutputFacts::default()
        };
        let refusal = ModelOutputFacts {
            has_refusal: true,
            ..ModelOutputFacts::default()
        };
        let tools = ModelOutputFacts {
            has_tool_calls: true,
            ..ModelOutputFacts::default()
        };
        let text_and_tools = ModelOutputFacts {
            has_text: true,
            has_tool_calls: true,
            ..ModelOutputFacts::default()
        };
        let unknown = ModelOutputFacts {
            has_unknown_correctness_bearing_item: true,
            ..ModelOutputFacts::default()
        };
        let refusal_and_text = ModelOutputFacts {
            has_text: true,
            has_refusal: true,
            ..ModelOutputFacts::default()
        };
        let structured_and_tools = ModelOutputFacts {
            has_structured_output: true,
            has_tool_calls: true,
            ..ModelOutputFacts::default()
        };
        let cases = [
            (text, TerminalDecision::Answered),
            (structured, TerminalDecision::Answered),
            (refusal, TerminalDecision::Refused),
            (tools, TerminalDecision::ContinueWithTools),
            (text_and_tools, TerminalDecision::ContinueWithTools),
            (structured_and_tools, TerminalDecision::ContinueWithTools),
            (
                ModelOutputFacts::default(),
                TerminalDecision::Failure(ModelOutputFailure::Empty),
            ),
            (
                unknown,
                TerminalDecision::Failure(ModelOutputFailure::UnknownCorrectnessBearingItem),
            ),
            (
                refusal_and_text,
                TerminalDecision::Failure(ModelOutputFailure::ContradictoryRefusal),
            ),
        ];
        for (output, expected) in cases {
            assert_eq!(
                decide_model_terminal(terminal_facts(ModelResponseStatus::Complete, output)),
                expected
            );
        }
        assert_eq!(
            decide_model_terminal(terminal_facts(ModelResponseStatus::Incomplete, text)),
            TerminalDecision::Failure(ModelOutputFailure::Incomplete)
        );
        assert_eq!(
            decide_model_terminal(terminal_facts(ModelResponseStatus::Failed, text)),
            TerminalDecision::Failure(ModelOutputFailure::ProviderFailed)
        );
    }

    #[test]
    fn cancellation_and_limits_precede_model_output_finalization() {
        let mut facts = terminal_facts(
            ModelResponseStatus::Complete,
            ModelOutputFacts {
                has_text: true,
                ..ModelOutputFacts::default()
            },
        );
        facts.exhausted_limit = Some(LifecycleLimit::AgentLoopSteps);
        assert_eq!(
            decide_model_terminal(facts),
            TerminalDecision::LimitReached(LifecycleLimit::AgentLoopSteps)
        );
        facts.cancellation_won = true;
        assert_eq!(decide_model_terminal(facts), TerminalDecision::CancelWins);
    }

    #[test]
    fn lifecycle_errors_project_explicitly_and_expose_no_payload_surface() {
        let conflict = LifecycleTransitionError::conflict(LifecycleConflictKind::StaleVersion);
        assert_eq!(
            conflict.to_normalized_error().category(),
            ErrorCategory::StateConflict
        );
        assert_eq!(
            conflict.to_normalized_error().retryability(),
            Retryability::Bounded
        );
        assert_eq!(
            conflict.to_normalized_error().code().as_str(),
            "state_conflict"
        );
        assert_eq!(conflict.to_string(), "lifecycle transition conflict");
        assert_eq!(format!("{conflict:?}"), "LifecycleConflict(StaleVersion)");

        let invariant =
            LifecycleTransitionError::invariant(LifecycleInvariantKind::ContradictoryProjection);
        assert_eq!(
            invariant.to_normalized_error().category(),
            ErrorCategory::InternalInvariantError
        );
        assert_eq!(
            invariant.to_normalized_error().code().as_str(),
            "internal_invariant_error"
        );
        assert_eq!(invariant.to_string(), "lifecycle invariant violation");
        for forbidden in ["provider", "command", "path", "content", "stdout"] {
            assert!(!format!("{invariant:?}").contains(forbidden));
        }
    }

    #[test]
    fn failpoint_semantic_map_is_complete_exact_and_preserves_atomic_aliases() {
        let expected = [
            FailpointRecoveryExpectation::RetainQueued,
            FailpointRecoveryExpectation::InterruptActiveWork,
            FailpointRecoveryExpectation::AtomicModelAttemptBoundary,
            FailpointRecoveryExpectation::AtomicModelAttemptBoundary,
            FailpointRecoveryExpectation::MarkModelProviderOutcomeUnknownAndInterrupt,
            FailpointRecoveryExpectation::InterruptActiveWork,
            FailpointRecoveryExpectation::MarkToolInterruptedBeforeDispatchAndInterrupt,
            FailpointRecoveryExpectation::MarkToolOutcomeUnknownAndInterrupt,
            FailpointRecoveryExpectation::MarkToolOutcomeUnknownAndInterrupt,
            FailpointRecoveryExpectation::MarkToolOutcomeUnknownAndInterrupt,
            FailpointRecoveryExpectation::MarkToolOutcomeUnknownAndInterrupt,
            FailpointRecoveryExpectation::AtomicFinalAnswerBoundary,
            FailpointRecoveryExpectation::InterruptActiveWork,
            FailpointRecoveryExpectation::InterruptActiveWork,
        ];
        assert_eq!(LifecycleFailpoint::ALL.len(), expected.len());
        for (failpoint, recovery) in LifecycleFailpoint::ALL.iter().zip(expected) {
            let row = failpoint_semantic_expectation(*failpoint);
            assert_eq!(row.failpoint(), *failpoint);
            assert_eq!(row.recovery(), recovery);
        }
        assert_eq!(
            failpoint_semantic_expectation(LifecycleFailpoint::AfterContextManifestCommit)
                .physical_interpretation(),
            FailpointPhysicalInterpretation::ContextManifestAtomicAlias
        );
        assert_eq!(
            failpoint_semantic_expectation(LifecycleFailpoint::AfterModelIntentCommit)
                .physical_interpretation(),
            FailpointPhysicalInterpretation::ModelIntentAtomicAlias
        );
        assert_eq!(
            failpoint_semantic_expectation(LifecycleFailpoint::AfterAssistantMessageCommit)
                .physical_interpretation(),
            FailpointPhysicalInterpretation::AssistantMessageAtomicAlias
        );
    }
}
