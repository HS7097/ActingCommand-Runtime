// SPDX-License-Identifier: AGPL-3.0-only

//! Typed global event contracts shared by Runtime producers and ledger adapters.
//!
//! Producers can supply semantic event data, but cannot choose field sensitivity or
//! redaction policy. The contract owns both the raw draft schema and its sanitized form.
//!
//! Raw audit fields cannot be assigned directly:
//!
//! ```compile_fail
//! use actingcommand_contract::AuditInput;
//!
//! let _ = AuditInput { account: Some("raw".to_string()) };
//! ```
//!
//! A sanitized draft cannot be mutated or deserialized:
//!
//! ```compile_fail
//! use actingcommand_contract::SanitizedEventDraft;
//!
//! fn forge(mut draft: SanitizedEventDraft) {
//!     draft.timestamp_unix_ms = 1;
//! }
//! ```
//!
//! ```compile_fail
//! use actingcommand_contract::SanitizedEventDraft;
//!
//! let _: SanitizedEventDraft = serde_json::from_str("{}").unwrap();
//! ```
//!
//! Transport IDs cannot be promoted into producer-issued identifiers:
//!
//! ```compile_fail
//! use actingcommand_contract::{
//!     AuditInput, ClientPayloadDraft, EventAction, EventActor, EventDraft, EventId,
//!     EventLinksDraft, EventOrigin, EventSeverity, EventSource, OriginModule,
//! };
//!
//! let transport: EventId = serde_json::from_str(
//!     "\"evt_11111111111111111111111111111111\"",
//! ).unwrap();
//! let _ = EventDraft::new(
//!     transport,
//!     1,
//!     EventSeverity::Info,
//!     EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::User),
//!     EventLinksDraft::default(),
//!     ClientPayloadDraft::cli_command(EventAction::RuntimeStatus, AuditInput::new()).into(),
//! );
//! ```
//!
//! Every producer link slot likewise requires an issuer-owned capability:
//!
//! ```compile_fail
//! use actingcommand_contract::{EventLinksDraft, RequestId};
//!
//! let transport: RequestId = serde_json::from_str(
//!     "\"request_11111111111111111111111111111111\"",
//! ).unwrap();
//! let _ = EventLinksDraft::default().with_request_id(transport);
//! ```
//!
//! Runtime strings, including leaked `'static` strings, cannot construct schema codes:
//!
//! ```compile_fail
//! use actingcommand_contract::EventAction;
//!
//! let leaked: &'static str = Box::leak(String::from("token-secret").into_boxed_str());
//! let _ = EventAction::new(leaked);
//! ```
//!
//! Transport IDs intentionally have no `Display` implementation:
//!
//! ```compile_fail
//! use actingcommand_contract::EventId;
//!
//! let transport: EventId = serde_json::from_str(
//!     "\"evt_11111111111111111111111111111111\"",
//! ).unwrap();
//! let _ = format!("{transport}");
//! ```
//!
//! A deserialized artifact reference is not a store-issued attachment capability:
//!
//! ```compile_fail
//! use actingcommand_contract::{ArtifactReference, EventDraft};
//!
//! fn attach(draft: EventDraft, transport: ArtifactReference) {
//!     let _ = draft.with_artifacts(vec![transport]);
//! }
//! ```
//!
//! Artifact transport metadata cannot be promoted into a trusted reference by deserialization:
//!
//! ```compile_fail
//! use actingcommand_contract::ArtifactReference;
//!
//! let _: ArtifactReference = serde_json::from_str("{}").unwrap();
//! ```
//!
//! Artifact-store authority cannot be reconstructed from transport data:
//!
//! ```compile_fail
//! use actingcommand_contract::ArtifactStoreIssuer;
//!
//! let _: ArtifactStoreIssuer = serde_json::from_str("{}").unwrap();
//! ```

mod artifact;
mod codes;
mod envelope;
mod ids;
mod payload;

pub use artifact::*;
pub use codes::*;
pub use envelope::*;
pub use ids::*;
pub use payload::*;

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

pub const GLOBAL_EVENT_SCHEMA_VERSION: &str = "actingcommand.event.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    Debug,
    Info,
    Warning,
    Error,
    Fatal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Internal,
    Sensitive,
    Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventFamily {
    Runtime,
    Monitor,
    Performance,
    Fact,
    Approval,
    Command,
    Scheduler,
    Policy,
    Catalog,
    Lease,
    Task,
    Application,
    Input,
    Capture,
    Recognition,
    Artifact,
    ResourceAuthoring,
    Client,
    State,
    Release,
    Ledger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "runtime.started")]
    RuntimeStarted,
    #[serde(rename = "runtime.takeover")]
    RuntimeTakeover,
    #[serde(rename = "monitor.probe_requested")]
    MonitorProbeRequested,
    #[serde(rename = "monitor.probe_started")]
    MonitorProbeStarted,
    #[serde(rename = "monitor.probe_completed")]
    MonitorProbeCompleted,
    #[serde(rename = "monitor.probe_failed")]
    MonitorProbeFailed,
    #[serde(rename = "monitor.recovery_admitted")]
    MonitorRecoveryAdmitted,
    #[serde(rename = "monitor.recovery_deferred")]
    MonitorRecoveryDeferred,
    #[serde(rename = "perf.pressure_started")]
    PerformancePressureStarted,
    #[serde(rename = "perf.pressure_ended")]
    PerformancePressureEnded,
    #[serde(rename = "perf.stutter_detected")]
    PerformanceStutterDetected,
    #[serde(rename = "perf.summary")]
    PerformanceSummary,
    #[serde(rename = "perf.monitor_degraded")]
    PerformanceMonitorDegraded,
    #[serde(rename = "perf.monitor_recovered")]
    PerformanceMonitorRecovered,
    #[serde(rename = "perf.balance_changed")]
    PerformanceBalanceChanged,
    #[serde(rename = "fact.published")]
    FactPublished,
    #[serde(rename = "fact.invalidated")]
    FactInvalidated,
    #[serde(rename = "approval.decision")]
    ApprovalDecision,
    #[serde(rename = "command.received")]
    CommandReceived,
    #[serde(rename = "command.validated")]
    CommandValidated,
    #[serde(rename = "command.rejected")]
    CommandRejected,
    #[serde(rename = "scheduler.admitted")]
    SchedulerAdmitted,
    #[serde(rename = "scheduler.queued")]
    SchedulerQueued,
    #[serde(rename = "scheduler.denied")]
    SchedulerDenied,
    #[serde(rename = "scheduler.preempted")]
    SchedulerPreempted,
    #[serde(rename = "policy.dispatch_intent")]
    PolicyDispatchIntent,
    #[serde(rename = "policy.dispatch_admitted")]
    PolicyDispatchAdmitted,
    #[serde(rename = "policy.dispatch_rejected")]
    PolicyDispatchRejected,
    #[serde(rename = "policy.dispatch_completed")]
    PolicyDispatchCompleted,
    #[serde(rename = "policy.execution_recorded")]
    PolicyExecutionRecorded,
    #[serde(rename = "policy.planning_signal_observed")]
    PolicyPlanningSignalObserved,
    #[serde(rename = "catalog.transition_intent")]
    CatalogTransitionIntent,
    #[serde(rename = "catalog.activated")]
    CatalogActivated,
    #[serde(rename = "catalog.rolled_back")]
    CatalogRolledBack,
    #[serde(rename = "catalog.transition_failed")]
    CatalogTransitionFailed,
    #[serde(rename = "lease.requested")]
    LeaseRequested,
    #[serde(rename = "lease.granted")]
    LeaseGranted,
    #[serde(rename = "lease.transferred")]
    LeaseTransferred,
    #[serde(rename = "lease.renewed")]
    LeaseRenewed,
    #[serde(rename = "lease.released")]
    LeaseReleased,
    #[serde(rename = "lease.expired")]
    LeaseExpired,
    #[serde(rename = "lease.transition_intent")]
    LeaseTransitionIntent,
    #[serde(rename = "lease.transition_failed")]
    LeaseTransitionFailed,
    #[serde(rename = "task.requested")]
    TaskRequested,
    #[serde(rename = "task.started")]
    TaskStarted,
    #[serde(rename = "task.step_started")]
    TaskStepStarted,
    #[serde(rename = "task.evidence_indexed")]
    TaskEvidenceIndexed,
    #[serde(rename = "task.recognition_started")]
    TaskRecognitionStarted,
    #[serde(rename = "task.recognition_completed")]
    TaskRecognitionCompleted,
    #[serde(rename = "task.effect_intent")]
    TaskEffectIntent,
    #[serde(rename = "task.effect_completed")]
    TaskEffectCompleted,
    #[serde(rename = "task.step_finished")]
    TaskStepFinished,
    #[serde(rename = "task.completed")]
    TaskCompleted,
    #[serde(rename = "task.failed")]
    TaskFailed,
    #[serde(rename = "task.cancelled")]
    TaskCancelled,
    #[serde(rename = "task.terminal_intent")]
    TaskTerminalIntent,
    #[serde(rename = "task.terminal_commit_failed")]
    TaskTerminalCommitFailed,
    #[serde(rename = "task.terminal_rejected")]
    TaskTerminalRejected,
    #[serde(rename = "application.intent")]
    ApplicationIntent,
    #[serde(rename = "application.completed")]
    ApplicationCompleted,
    #[serde(rename = "application.failed")]
    ApplicationFailed,
    #[serde(rename = "input.intent")]
    InputIntent,
    #[serde(rename = "input.committed")]
    InputCommitted,
    #[serde(rename = "input.completed")]
    InputCompleted,
    #[serde(rename = "input.failed")]
    InputFailed,
    #[serde(rename = "capture.requested")]
    CaptureRequested,
    #[serde(rename = "capture.completed")]
    CaptureCompleted,
    #[serde(rename = "capture.failed")]
    CaptureFailed,
    #[serde(rename = "capture.pressure_changed")]
    CapturePressureChanged,
    #[serde(rename = "capture.dedup_window")]
    CaptureDedupWindow,
    #[serde(rename = "capture.policy_changed")]
    CapturePolicyChanged,
    #[serde(rename = "recognition.requested")]
    RecognitionRequested,
    #[serde(rename = "recognition.completed")]
    RecognitionCompleted,
    #[serde(rename = "recognition.failed")]
    RecognitionFailed,
    #[serde(rename = "artifact.created")]
    ArtifactCreated,
    #[serde(rename = "artifact.verified")]
    ArtifactVerified,
    #[serde(rename = "artifact.store_failed")]
    ArtifactStoreFailed,
    #[serde(rename = "artifact.verification_failed")]
    ArtifactVerificationFailed,
    #[serde(rename = "artifact.export_completed")]
    ArtifactExportCompleted,
    #[serde(rename = "artifact.export_failed")]
    ArtifactExportFailed,
    #[serde(rename = "resource.authoring_started")]
    ResourceAuthoringStarted,
    #[serde(rename = "resource.draft_built")]
    ResourceDraftBuilt,
    #[serde(rename = "resource.validation_completed")]
    ResourceValidationCompleted,
    #[serde(rename = "resource.promote_intent")]
    ResourcePromoteIntent,
    #[serde(rename = "resource.promoted")]
    ResourcePromoted,
    #[serde(rename = "resource.promote_failed")]
    ResourcePromoteFailed,
    #[serde(rename = "ui.action")]
    UiAction,
    #[serde(rename = "client.action")]
    ClientAction,
    #[serde(rename = "cli.command")]
    CliCommand,
    #[serde(rename = "lab.request")]
    LabRequest,
    #[serde(rename = "state.migrated")]
    StateMigrated,
    #[serde(rename = "release.staged")]
    ReleaseStaged,
    #[serde(rename = "release.transition_intent")]
    ReleaseTransitionIntent,
    #[serde(rename = "release.activated")]
    ReleaseActivated,
    #[serde(rename = "release.rolled_back")]
    ReleaseRolledBack,
    #[serde(rename = "release.transition_failed")]
    ReleaseTransitionFailed,
    #[serde(rename = "ledger.recovered")]
    LedgerRecovered,
}

impl EventType {
    pub fn family(self) -> EventFamily {
        match self {
            Self::RuntimeStarted | Self::RuntimeTakeover => EventFamily::Runtime,
            Self::MonitorProbeRequested
            | Self::MonitorProbeStarted
            | Self::MonitorProbeCompleted
            | Self::MonitorProbeFailed
            | Self::MonitorRecoveryAdmitted
            | Self::MonitorRecoveryDeferred => EventFamily::Monitor,
            Self::PerformancePressureStarted
            | Self::PerformancePressureEnded
            | Self::PerformanceStutterDetected
            | Self::PerformanceSummary
            | Self::PerformanceMonitorDegraded
            | Self::PerformanceMonitorRecovered
            | Self::PerformanceBalanceChanged => EventFamily::Performance,
            Self::FactPublished | Self::FactInvalidated => EventFamily::Fact,
            Self::ApprovalDecision => EventFamily::Approval,
            Self::CommandReceived | Self::CommandValidated | Self::CommandRejected => {
                EventFamily::Command
            }
            Self::SchedulerAdmitted
            | Self::SchedulerQueued
            | Self::SchedulerDenied
            | Self::SchedulerPreempted => EventFamily::Scheduler,
            Self::PolicyDispatchIntent
            | Self::PolicyDispatchAdmitted
            | Self::PolicyDispatchRejected
            | Self::PolicyDispatchCompleted
            | Self::PolicyExecutionRecorded
            | Self::PolicyPlanningSignalObserved => EventFamily::Policy,
            Self::CatalogTransitionIntent
            | Self::CatalogActivated
            | Self::CatalogRolledBack
            | Self::CatalogTransitionFailed => EventFamily::Catalog,
            Self::LeaseRequested
            | Self::LeaseGranted
            | Self::LeaseTransferred
            | Self::LeaseRenewed
            | Self::LeaseReleased
            | Self::LeaseExpired
            | Self::LeaseTransitionIntent
            | Self::LeaseTransitionFailed => EventFamily::Lease,
            Self::TaskRequested
            | Self::TaskStarted
            | Self::TaskStepStarted
            | Self::TaskEvidenceIndexed
            | Self::TaskRecognitionStarted
            | Self::TaskRecognitionCompleted
            | Self::TaskEffectIntent
            | Self::TaskEffectCompleted
            | Self::TaskStepFinished
            | Self::TaskCompleted
            | Self::TaskFailed
            | Self::TaskCancelled
            | Self::TaskTerminalIntent
            | Self::TaskTerminalCommitFailed
            | Self::TaskTerminalRejected => EventFamily::Task,
            Self::ApplicationIntent | Self::ApplicationCompleted | Self::ApplicationFailed => {
                EventFamily::Application
            }
            Self::InputIntent | Self::InputCommitted | Self::InputCompleted | Self::InputFailed => {
                EventFamily::Input
            }
            Self::CaptureRequested
            | Self::CaptureCompleted
            | Self::CaptureFailed
            | Self::CapturePressureChanged
            | Self::CaptureDedupWindow
            | Self::CapturePolicyChanged => EventFamily::Capture,
            Self::RecognitionRequested | Self::RecognitionCompleted | Self::RecognitionFailed => {
                EventFamily::Recognition
            }
            Self::ArtifactCreated
            | Self::ArtifactVerified
            | Self::ArtifactStoreFailed
            | Self::ArtifactVerificationFailed
            | Self::ArtifactExportCompleted
            | Self::ArtifactExportFailed => EventFamily::Artifact,
            Self::ResourceAuthoringStarted
            | Self::ResourceDraftBuilt
            | Self::ResourceValidationCompleted
            | Self::ResourcePromoteIntent
            | Self::ResourcePromoted
            | Self::ResourcePromoteFailed => EventFamily::ResourceAuthoring,
            Self::UiAction | Self::ClientAction | Self::CliCommand | Self::LabRequest => {
                EventFamily::Client
            }
            Self::StateMigrated => EventFamily::State,
            Self::ReleaseStaged
            | Self::ReleaseTransitionIntent
            | Self::ReleaseActivated
            | Self::ReleaseRolledBack
            | Self::ReleaseTransitionFailed => EventFamily::Release,
            Self::LedgerRecovered => EventFamily::Ledger,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Runtime,
    Scheduler,
    Device,
    Cli,
    Ui,
    Lab,
    System,
    Adapter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventActor {
    User,
    Runtime,
    Scheduler,
    Cli,
    Ui,
    Lab,
    Agent,
    System,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventQuery {
    pub from_sequence: Option<u64>,
    pub to_sequence: Option<u64>,
    pub event_type: Option<EventType>,
    pub minimum_severity: Option<EventSeverity>,
    pub source: Option<EventSource>,
    pub instance_id: Option<InstanceId>,
    pub request_id: Option<RequestId>,
    pub correlation_id: Option<CorrelationId>,
    pub causation_id: Option<CausationId>,
    pub task_id: Option<TaskId>,
    pub run_id: Option<RunId>,
    pub lease_id: Option<LeaseId>,
    pub frame_id: Option<FrameId>,
    pub action_id: Option<ActionId>,
    pub recognition_id: Option<RecognitionId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionCursor {
    pub after_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionProfile {
    Cli,
    Ui,
    Lab,
    Concise,
    Normal,
    Verbose,
    Forensic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectedEvent {
    pub schema_version: String,
    pub sequence: u64,
    pub event_id: EventId,
    pub timestamp_unix_ms: u64,
    pub event_type: EventType,
    pub severity: EventSeverity,
    pub sensitivity: Sensitivity,
    pub origin: EventOrigin,
    pub links: EventLinks,
    pub payload_schema: String,
    pub payload: ProjectionPayload,
    pub artifacts: Vec<ProjectedArtifactReference>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizationError {
    code: &'static str,
    field: &'static str,
}

impl SanitizationError {
    pub(crate) const fn new(code: &'static str, field: &'static str) -> Self {
        Self { code, field }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn fingerprinter_failure() -> Self {
        Self::new("fingerprinter_failed", "fingerprinter")
    }
}

impl fmt::Display for SanitizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "event sanitization failed with {} at {}",
            self.code, self.field
        )
    }
}

impl Error for SanitizationError {}

#[cfg(test)]
#[path = "event/v2_tests.rs"]
mod tests;
