// SPDX-License-Identifier: AGPL-3.0-only

//! Durability class of every append (Workflow #191 I), fixed by the Ledger from the
//! event payload alone; see "Durability classes" in `contracts/ledger-store.md`.
//! Callers cannot choose it.
//!
//! A durable event commits under `synchronous=FULL`: its WAL commit is synced before
//! the append returns. An observational event commits under `NORMAL` for its one
//! transaction: it survives a process crash, and becomes durable across power loss at
//! the next FULL commit on the same database (Ledger or State) or SQLite checkpoint.
//!
//! Prefix equivalence: WAL recovery keeps the longest valid prefix ending at a commit,
//! and a FULL commit syncs every earlier commit with it. After a power loss the Ledger
//! and State therefore equal the prefix at some commit boundary no earlier than the last
//! durable commit, which is exactly what a power loss at that boundary leaves when every
//! commit is durable. Every external effect follows a durable commit, so the facts that
//! precede an effect are always inside the surviving prefix.
//!
//! The matches are exhaustive without a wildcard arm: a new event type or lifecycle
//! phase does not compile until it is classified here, and a new variant is durable
//! unless it is deliberately added to the observational list.

use actingcommand_contract::{
    EventPayload, EventType, RuntimeFactScope, RuntimeLifecyclePhase, RuntimePayload,
};
use actingcommand_runtime_database::CommitSync;

/// Commit synchronization the Ledger uses for one appended event.
pub(crate) fn commit_sync(payload: &EventPayload) -> CommitSync {
    match payload.event_type() {
        // Observational: read-only observation, diagnostics and derived hints. No
        // external effect, file write or durable decision depends on them alone.
        EventType::ProviderStartupObserved
        | EventType::MonitorProbeRequested
        | EventType::MonitorProbeStarted
        | EventType::MonitorProbeCompleted
        | EventType::MonitorProbeFailed
        | EventType::PerformancePressureStarted
        | EventType::PerformancePressureEnded
        | EventType::PerformanceStutterDetected
        | EventType::PerformanceSummary
        | EventType::PerformanceMonitorDegraded
        | EventType::PerformanceMonitorRecovered
        | EventType::TaskEvidenceIndexed
        | EventType::TaskGeometryObserved
        | EventType::TaskRecognitionStarted
        | EventType::TaskRecognitionCompleted
        | EventType::CaptureRequested
        | EventType::CaptureCompleted
        | EventType::CaptureFailed
        | EventType::CapturePressureChanged
        | EventType::CaptureDedupWindow
        | EventType::CapturePolicyChanged
        | EventType::RecognitionRequested
        | EventType::RecognitionCompleted
        | EventType::RecognitionFailed
        | EventType::ArtifactVerified
        | EventType::ArtifactPinRecorded => CommitSync::Normal,
        // Classified by payload. The event type is derived from the payload, so the
        // other payload arms are unreachable by construction; they stay durable.
        EventType::RuntimeLifecycleObserved => match payload {
            EventPayload::Runtime(RuntimePayload::DeviceSelfCheck(_)) => CommitSync::Normal,
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(lifecycle)) => {
                lifecycle_commit_sync(&lifecycle.phase())
            }
            // Unreachable: the event type is derived from the payload.
            _ => CommitSync::Full,
        },
        EventType::RuntimeFactRecorded => match payload {
            EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => {
                fact_scope_commit_sync(&recorded.record().scope)
            }
            // Unreachable: the event type is derived from the payload.
            _ => CommitSync::Full,
        },
        EventType::RuntimeFactInvalidated => match payload {
            EventPayload::Runtime(RuntimePayload::FactInvalidated(invalidated)) => {
                fact_scope_commit_sync(&invalidated.invalidation().scope)
            }
            // Unreachable: the event type is derived from the payload.
            _ => CommitSync::Full,
        },
        // Durable: Runtime lifecycle, configuration and recovery decisions.
        EventType::RuntimeStarted
        | EventType::RuntimeTakeover
        | EventType::RuntimeFailed
        | EventType::RuntimeInstanceBound
        | EventType::RuntimeFactSnapshot
        | EventType::MonitorRecoveryAdmitted
        | EventType::MonitorRecoveryDeferred
        | EventType::PerformanceBalanceChanged
        | EventType::LedgerRecovered
        // Durable: facts, approvals, commands, scheduling, policy, catalog and release.
        | EventType::FactPublished
        | EventType::FactInvalidated
        | EventType::ApprovalDecision
        | EventType::CommandReceived
        | EventType::CommandValidated
        | EventType::CommandRejected
        | EventType::SchedulerAdmitted
        | EventType::SchedulerQueued
        | EventType::SchedulerDenied
        | EventType::SchedulerPreempted
        | EventType::PolicyDispatchIntent
        | EventType::PolicyDispatchAdmitted
        | EventType::PolicyDispatchRejected
        | EventType::PolicyDispatchCompleted
        | EventType::PolicyExecutionRecorded
        | EventType::PolicyPlanningSignalObserved
        | EventType::CatalogTransitionIntent
        | EventType::CatalogActivated
        | EventType::CatalogRolledBack
        | EventType::CatalogTransitionFailed
        | EventType::StateMigrated
        | EventType::ReleaseStaged
        | EventType::ReleaseTransitionIntent
        | EventType::ReleaseActivated
        | EventType::ReleaseRolledBack
        | EventType::ReleaseTransitionFailed
        // Durable: leases, task execution structure and terminals.
        | EventType::LeaseRequested
        | EventType::LeaseGranted
        | EventType::LeaseTransferred
        | EventType::LeaseRenewed
        | EventType::LeaseReleased
        | EventType::LeaseExpired
        | EventType::LeaseTransitionIntent
        | EventType::LeaseTransitionFailed
        | EventType::TaskRequested
        | EventType::TaskStarted
        | EventType::TaskStepStarted
        | EventType::TaskEntryPreflight
        | EventType::TaskSelectionEvaluated
        | EventType::TaskEffectIntent
        | EventType::TaskEffectCompleted
        | EventType::TaskStepFinished
        | EventType::TaskCompleted
        | EventType::TaskFailed
        | EventType::TaskCancelled
        | EventType::TaskTerminalIntent
        | EventType::TaskTerminalCommitFailed
        | EventType::TaskTerminalRejected
        // Durable: device writes and application lifecycle.
        | EventType::ApplicationIntent
        | EventType::ApplicationCompleted
        | EventType::ApplicationFailed
        | EventType::InputIntent
        | EventType::InputCommitted
        | EventType::InputCompleted
        | EventType::InputFailed
        // Durable: evidence summary, material publication, retention and export.
        | EventType::CaptureSummaryCommitted
        | EventType::ArtifactPinReleased
        | EventType::ArtifactEvictionIntent
        | EventType::ArtifactEvictionOutcome
        | EventType::ArtifactCreated
        | EventType::ArtifactStoreFailed
        | EventType::ArtifactVerificationFailed
        | EventType::ArtifactExportCompleted
        | EventType::ArtifactExportFailed
        // Durable: resource authoring, client audit, agents and signatures.
        | EventType::ResourceAuthoringStarted
        | EventType::ResourceDraftBuilt
        | EventType::ResourceValidationCompleted
        | EventType::ResourcePromoteIntent
        | EventType::ResourcePromoted
        | EventType::ResourcePromoteFailed
        | EventType::UiAction
        | EventType::ClientAction
        | EventType::CliCommand
        | EventType::LabRequest
        | EventType::GovernanceIdentityDeclared
        | EventType::AgentWakeRequested
        | EventType::AgentSessionStarted
        | EventType::AgentSessionResumed
        | EventType::AgentResponseRecorded
        | EventType::AgentSessionCompleted
        | EventType::AgentSessionEscalated
        | EventType::SignatureRegistered
        | EventType::SignatureMatched
        | EventType::SignatureRetired => CommitSync::Full,
    }
}

/// After-the-fact backend and device observations are observational; every phase that
/// closes, releases, hands over or reports a decision is durable. `VendorStdioClose`
/// stays durable so that it reaches storage before the owner journal records the close.
fn lifecycle_commit_sync(phase: &RuntimeLifecyclePhase) -> CommitSync {
    match phase {
        RuntimeLifecyclePhase::BackendOpenObserved
        | RuntimeLifecyclePhase::AdbTargetRecovery
        | RuntimeLifecyclePhase::DeviceDiagnosticDetail => CommitSync::Normal,
        RuntimeLifecyclePhase::PriorEpochOwnerImported
        | RuntimeLifecyclePhase::PriorEpochScopeClosed
        | RuntimeLifecyclePhase::PriorEpochOwnerReleasedByExit { .. }
        | RuntimeLifecyclePhase::VendorStdioClose { .. }
        | RuntimeLifecyclePhase::DeviceDiagnosticSummary
        | RuntimeLifecyclePhase::PolicyForwardEntered
        | RuntimeLifecyclePhase::PolicyForwardReturned { .. }
        | RuntimeLifecyclePhase::StrategicReportEntered
        | RuntimeLifecyclePhase::StrategicReportReturned { .. }
        | RuntimeLifecyclePhase::ShutdownRequested
        | RuntimeLifecyclePhase::ShutdownRequest { .. }
        | RuntimeLifecyclePhase::ResourceQuiescence { .. }
        | RuntimeLifecyclePhase::StartupPackageScheduled { .. }
        | RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
        | RuntimeLifecyclePhase::RecoveryRungFinished { .. }
        | RuntimeLifecyclePhase::InstancePreparationFinished { .. }
        | RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
        | RuntimeLifecyclePhase::RecoveryInstanceStopped { .. }
        | RuntimeLifecyclePhase::RecoveryLadderFinished { .. }
        | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
        | RuntimeLifecyclePhase::FactSnapshotSetSkipped { .. } => CommitSync::Full,
    }
}

/// Instance-scoped runtime facts are observations; the Runtime scope holds only the
/// configuration inventory, which is durable.
fn fact_scope_commit_sync(scope: &RuntimeFactScope) -> CommitSync {
    match scope {
        RuntimeFactScope::Instance { .. } => CommitSync::Normal,
        RuntimeFactScope::Runtime => CommitSync::Full,
    }
}
