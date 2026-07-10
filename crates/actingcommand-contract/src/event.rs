// SPDX-License-Identifier: AGPL-3.0-only

//! Typed global event contracts shared by Runtime producers and ledger adapters.
//!
//! Producers can supply semantic event data, but cannot choose field sensitivity or
//! redaction policy. The contract owns both the raw draft schema and its sanitized form.
//!
//! Caller-selected policy types are intentionally unavailable:
//!
//! ```compile_fail
//! use actingcommand_contract::{ClassifiedField, StructuredPayloadDraft};
//! ```
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
//! Runtime strings cannot be promoted into static codes or typed IDs:
//!
//! ```compile_fail
//! use actingcommand_contract::{EventId, StaticCode};
//!
//! let runtime = String::from("runtime.value");
//! let _ = StaticCode::new(&runtime);
//! let _ = EventId::new(runtime);
//! ```

mod artifact;
mod envelope;
mod ids;
mod payload;

pub use artifact::*;
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
    Command,
    Scheduler,
    Lease,
    Task,
    Input,
    Client,
    Ledger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
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
    #[serde(rename = "lease.requested")]
    LeaseRequested,
    #[serde(rename = "lease.granted")]
    LeaseGranted,
    #[serde(rename = "lease.transferred")]
    LeaseTransferred,
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
    #[serde(rename = "input.intent")]
    InputIntent,
    #[serde(rename = "input.committed")]
    InputCommitted,
    #[serde(rename = "input.completed")]
    InputCompleted,
    #[serde(rename = "input.failed")]
    InputFailed,
    #[serde(rename = "ui.action")]
    UiAction,
    #[serde(rename = "cli.command")]
    CliCommand,
    #[serde(rename = "lab.request")]
    LabRequest,
    #[serde(rename = "ledger.recovered")]
    LedgerRecovered,
}

impl EventType {
    pub fn family(self) -> EventFamily {
        match self {
            Self::CommandReceived | Self::CommandValidated | Self::CommandRejected => {
                EventFamily::Command
            }
            Self::SchedulerAdmitted
            | Self::SchedulerQueued
            | Self::SchedulerDenied
            | Self::SchedulerPreempted => EventFamily::Scheduler,
            Self::LeaseRequested
            | Self::LeaseGranted
            | Self::LeaseTransferred
            | Self::LeaseReleased
            | Self::LeaseExpired
            | Self::LeaseTransitionIntent
            | Self::LeaseTransitionFailed => EventFamily::Lease,
            Self::TaskRequested
            | Self::TaskStarted
            | Self::TaskStepStarted
            | Self::TaskStepFinished
            | Self::TaskCompleted
            | Self::TaskFailed
            | Self::TaskCancelled
            | Self::TaskTerminalIntent
            | Self::TaskTerminalCommitFailed => EventFamily::Task,
            Self::InputIntent | Self::InputCommitted | Self::InputCompleted | Self::InputFailed => {
                EventFamily::Input
            }
            Self::UiAction | Self::CliCommand | Self::LabRequest => EventFamily::Client,
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
