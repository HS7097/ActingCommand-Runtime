// SPDX-License-Identifier: AGPL-3.0-only

use super::{EventFamily, EventId, EventSeverity, EventSource, EventType, RunId};
use serde::{Deserialize, Serialize};

/// Overlapping slices of the same ledger facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerView {
    Events,
    Observation,
    Changes,
    Errors,
    Health,
    Lab,
}

/// Closed classification inputs shared with the storage owner's SQL projection.
/// Nonempty families and event types are a union. The other variants stand alone.
#[derive(Debug, Clone, Copy)]
pub enum LedgerViewDefinition {
    All,
    Types {
        families: &'static [EventFamily],
        events: &'static [EventType],
    },
    MinimumSeverity(EventSeverity),
    LabContext,
}

impl LedgerView {
    pub const ALL: [Self; 6] = [
        Self::Events,
        Self::Observation,
        Self::Changes,
        Self::Errors,
        Self::Health,
        Self::Lab,
    ];

    pub const fn definition(self) -> LedgerViewDefinition {
        use EventType::*;
        match self {
            Self::Events => LedgerViewDefinition::All,
            Self::Observation => LedgerViewDefinition::Types {
                families: &[
                    EventFamily::Recognition,
                    EventFamily::Input,
                    EventFamily::Capture,
                ],
                events: &[
                    TaskEvidenceIndexed,
                    TaskRecognitionStarted,
                    TaskRecognitionCompleted,
                    TaskEffectIntent,
                    TaskEffectCompleted,
                    TaskStepStarted,
                    TaskStepFinished,
                    TaskEntryPreflight,
                    FactPublished,
                    FactInvalidated,
                ],
            },
            Self::Changes => LedgerViewDefinition::Types {
                families: &[
                    EventFamily::Provider,
                    EventFamily::Runtime,
                    EventFamily::Command,
                    EventFamily::Scheduler,
                    EventFamily::Catalog,
                    EventFamily::Lease,
                    EventFamily::Application,
                    EventFamily::State,
                    EventFamily::Release,
                ],
                events: &[
                    PolicyDispatchIntent,
                    PolicyDispatchAdmitted,
                    PolicyDispatchRejected,
                    PolicyDispatchCompleted,
                    PolicyExecutionRecorded,
                    TaskRequested,
                    TaskStarted,
                    TaskCompleted,
                    TaskFailed,
                    TaskCancelled,
                    TaskTerminalIntent,
                    TaskTerminalCommitFailed,
                    TaskTerminalRejected,
                    ResourcePromoteIntent,
                    ResourcePromoted,
                    ResourcePromoteFailed,
                    AgentWakeRequested,
                    AgentSessionStarted,
                    AgentSessionResumed,
                    AgentSessionCompleted,
                    AgentSessionEscalated,
                ],
            },
            Self::Errors => LedgerViewDefinition::MinimumSeverity(EventSeverity::Warning),
            Self::Health => LedgerViewDefinition::Types {
                families: &[EventFamily::Performance, EventFamily::Monitor],
                events: &[],
            },
            Self::Lab => LedgerViewDefinition::LabContext,
        }
    }

    /// `lab_related` must come from request/correlation/run links at the same snapshot.
    pub fn contains(
        self,
        event_type: EventType,
        severity: EventSeverity,
        source: EventSource,
        lab_related: bool,
    ) -> bool {
        match self.definition() {
            LedgerViewDefinition::All => true,
            LedgerViewDefinition::Types { families, events } => {
                families.contains(&event_type.family()) || events.contains(&event_type)
            }
            LedgerViewDefinition::MinimumSeverity(minimum) => severity >= minimum,
            LedgerViewDefinition::LabContext => {
                source == EventSource::Lab || event_type == EventType::LabRequest || lab_related
            }
        }
    }

    pub fn memberships(
        event_type: EventType,
        severity: EventSeverity,
        source: EventSource,
        lab_related: bool,
    ) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|view| view.contains(event_type, severity, source, lab_related))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerReadSource {
    Runtime,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerMaterialReadState {
    NotRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerPageLimit {
    EventCount,
    ResponseBytes,
    SourceIncomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerReadScope {
    pub source: LedgerReadSource,
    pub material_read: LedgerMaterialReadState,
    pub scanned_through_position: u64,
    pub read_complete: bool,
    pub limits: Vec<LedgerPageLimit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerEventPosition {
    pub event_id: EventId,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryState {
    Recovered,
    Unresolved,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerRecoveryGap {
    MissingRelation,
    ConflictingOutcome,
    SourceIncomplete,
    ContextLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerFailureResolution {
    pub failure: LedgerEventPosition,
    pub success: Option<LedgerEventPosition>,
}

/// A read-time grouping, never an update to a persisted failure event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerRunRecovery {
    pub run_id: RunId,
    pub state: LedgerRecoveryState,
    pub evidence: Vec<LedgerFailureResolution>,
    pub gaps: Vec<LedgerRecoveryGap>,
}
