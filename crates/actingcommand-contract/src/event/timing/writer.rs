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
}

impl TaskTimingWriterObservation {
    pub(super) fn is_valid(&self) -> bool {
        self.previous_processing.is_valid() && self.previous_after_reply.is_valid()
    }
}
