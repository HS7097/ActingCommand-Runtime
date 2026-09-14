// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

/// A direct process-Instant endpoint relative to this append's send-call start.
/// Direction is retained even when a sub-microsecond distance rounds to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "position", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskTimingWriterEndpoint {
    Unobserved,
    AtSendStart,
    BeforeSendStart { distance_us: ObservedMicroseconds },
    AfterSendStart { distance_us: ObservedMicroseconds },
    Unavailable { reason: TimingObservationIssue },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingWriterCommand {
    RetentionCandidates,
    AdmitArtifactEviction,
    FinishArtifactEviction,
    AppendTransaction,
    Append,
    ReconcileScheduledPolicySettlement,
    Query,
    QueryPage,
    ProjectViewPage,
    ProjectSchedulingOutcomes,
    LatestSequence,
    Subscribe,
    ReplayPage,
    Project,
    ProjectPage,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingWriterReceiveOrder {
    Unobserved,
    Incomplete,
    BeforeSendReturned,
    AtSendReturn,
    AfterSendReturned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTimingPreviousWorkRelation {
    Unobserved,
    Incomplete,
    CompletedBySendStart,
    OverlapsSend,
    StartedAtOrAfterSendReturn,
}

/// A same-command ProjectViewPage scale value; absence is not a measured zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskTimingProjectViewCount {
    Unobserved,
    Measured { value: u64 },
    Unavailable { reason: TimingObservationIssue },
}

/// Writer-owned work, without the observing task's identity, budget or aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingWriterSpan {
    pub status: TaskTimingObservationState,
    pub started: TaskTimingWriterEndpoint,
    pub finished: TaskTimingWriterEndpoint,
    pub elapsed_us: Option<ObservedMicroseconds>,
    pub result: TaskTimingResult,
}

impl TaskTimingWriterSpan {
    fn is_valid(&self) -> bool {
        match self.status {
            TaskTimingObservationState::Unobserved => {
                self.started == TaskTimingWriterEndpoint::Unobserved
                    && self.finished == TaskTimingWriterEndpoint::Unobserved
                    && self.elapsed_us.is_none()
                    && self.result == TaskTimingResult::Unobserved
            }
            TaskTimingObservationState::Observed => {
                matches!(self.elapsed_us, Some(ObservedMicroseconds::Measured { .. }))
                    && self.result != TaskTimingResult::Unobserved
            }
            TaskTimingObservationState::Incomplete { .. } => true,
        }
    }
}

/// The original optional read limits, not a new service or task deadline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingProjectViewReadBudget {
    pub max_bytes: u64,
    pub max_events: TaskTimingProjectViewCount,
    pub deadline: TaskTimingWriterEndpoint,
}

/// Direct stages and existing sizes of the preceding SQLite ProjectViewPage call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingProjectViewObservation {
    pub admission: TaskTimingWriterSpan,
    pub connection: TaskTimingWriterSpan,
    pub with_connection: TaskTimingWriterSpan,
    pub begin_transaction: TaskTimingWriterSpan,
    pub read_snapshot: TaskTimingWriterSpan,
    pub verify_snapshot: TaskTimingWriterSpan,
    pub prepare_events: TaskTimingWriterSpan,
    pub select_sequences: TaskTimingWriterSpan,
    pub project_page: TaskTimingWriterSpan,
    pub commit: TaskTimingWriterSpan,
    pub rollback: TaskTimingWriterSpan,
    pub read_budget: Option<TaskTimingProjectViewReadBudget>,
    pub requested_limit: TaskTimingProjectViewCount,
    pub selection_limit: TaskTimingProjectViewCount,
    pub max_page_events: TaskTimingProjectViewCount,
    pub max_response_bytes: TaskTimingProjectViewCount,
    pub max_recovery_context_events: TaskTimingProjectViewCount,
    pub raw_bytes: TaskTimingProjectViewCount,
    pub raw_event_rows: TaskTimingProjectViewCount,
    pub raw_link_rows: TaskTimingProjectViewCount,
    pub raw_artifact_rows: TaskTimingProjectViewCount,
    pub verified_records: TaskTimingProjectViewCount,
    pub prepared_events: TaskTimingProjectViewCount,
    pub selected_sequences: TaskTimingProjectViewCount,
    pub returned_events: TaskTimingProjectViewCount,
    pub returned_recovery_groups: TaskTimingProjectViewCount,
}

impl TaskTimingProjectViewObservation {
    fn is_valid(&self) -> bool {
        [
            &self.admission,
            &self.connection,
            &self.with_connection,
            &self.begin_transaction,
            &self.read_snapshot,
            &self.verify_snapshot,
            &self.prepare_events,
            &self.select_sequences,
            &self.project_page,
            &self.commit,
            &self.rollback,
        ]
        .into_iter()
        .all(TaskTimingWriterSpan::is_valid)
    }
}

/// One same-append snapshot; the previous command is not an operation of this task.
/// Nested/overlapping intervals must not be added to obtain queue or task cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTimingWriterObservation {
    pub send_returned: TaskTimingWriterEndpoint,
    pub writer_received: TaskTimingWriterEndpoint,
    pub receive_order: TaskTimingWriterReceiveOrder,
    pub previous_work_relation: TaskTimingPreviousWorkRelation,
    pub previous_command: Option<TaskTimingWriterCommand>,
    pub previous_processing: TaskTimingWriterSpan,
    pub previous_after_reply: TaskTimingWriterSpan,
    pub previous_reply_result: TaskTimingResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_project_view: Option<Box<TaskTimingProjectViewObservation>>,
}

impl TaskTimingWriterObservation {
    pub(super) fn is_valid(&self) -> bool {
        self.previous_processing.is_valid()
            && self.previous_after_reply.is_valid()
            && self.previous_project_view.as_ref().is_none_or(|view| {
                self.previous_command == Some(TaskTimingWriterCommand::ProjectViewPage)
                    && view.is_valid()
            })
    }
}
