// SPDX-License-Identifier: AGPL-3.0-only

//! The wire shapes of the Runtime's messages to registered channels (Workflow #432 AGT-1b;
//! `contracts/agent-wake.md`; `runtime-client` revision 5): the status member `agent_wake`, the
//! answer of `AgentWakeTest`, and the two vocabularies a delivery failure's record carries.

use serde::{Deserialize, Serialize};

crate::outcome_vocabulary! {
    /// The state of one wake's delivery (vocabulary `agent_delivery_state`). No state claims a
    /// session acted: delivery is not completion.
    pub enum AgentDeliveryState: "agent_delivery_state" {
        Prepared => "prepared",
        Deferred => "deferred",
        Sending => "sending",
        Delivered => "delivered",
        Failed => "failed",
        Unknown => "unknown",
        Expired => "expired",
        BodyFailed => "body_failed",
    }
}

impl AgentDeliveryState {
    /// The state a row spells `token`.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == token)
    }

    /// A send may still come of the wake, or one is under way.
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Prepared | Self::Deferred | Self::Sending)
    }

    /// A person or an agent should look: the states that record a failure.
    pub const fn is_loud(self) -> bool {
        matches!(
            self,
            Self::Failed | Self::Unknown | Self::Expired | Self::BodyFailed
        )
    }
}

crate::outcome_vocabulary! {
    /// What went wrong with a delivery (vocabulary `agent_delivery_problem`): the transports'
    /// neutral problems and the worker's own.
    pub enum AgentDeliveryProblem: "agent_delivery_problem" {
        ProtocolChanged => "protocol_changed",
        Rejected => "rejected",
        OwnerMismatch => "owner_mismatch",
        WriteTimedOut => "write_timed_out",
        WriteFailed => "write_failed",
        ReceiptMismatch => "receipt_mismatch",
        HelperFailed => "helper_failed",
        ChannelUnknown => "channel_unknown",
        /// The message cannot hold its header, kind word and path within `summary.max_chars`.
        MessageTooLong => "message_too_long",
        /// A restart found the wake in `sending`: a write may have happened.
        InterruptedByStop => "interrupted_by_stop",
        /// The intent could not be committed, so nothing was written.
        IntentRefused => "intent_refused",
        /// The bounded queue was full and candidates were dropped.
        QueueFull => "wake_queue_full",
        /// A row names a kind this build has no word for.
        KindUnknown => "kind_unknown",
        /// The body file could not be written or does not fit its bound.
        BodyUnwritable => "body_unwritable",
        /// The delivery detail file could not be written; the delivery goes on.
        DetailUnwritable => "detail_unwritable",
        /// The retention sweep could not read or remove a file.
        SweepFailed => "sweep_failed",
        /// The quiet windows could not be judged.
        QuietUnreadable => "quiet_unreadable",
        /// The worker's iteration panicked; it backs off and goes on.
        WorkerPanicked => "worker_panicked",
        /// The worker thread could not start; the feature is off until the next start.
        WorkerSpawnFailed => "worker_spawn_failed",
        /// The host's local day could not be read, so the day caps could not be judged; the
        /// waiting candidates stay queued (Workflow #432 AGT-2).
        LocalTimeUnknown => "local_time_unknown",
    }
}

/// What the worker thread does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWakeWorkerState {
    /// Running its iterations.
    Running,
    /// Waiting after a failed iteration (`delivery.panic_backoff_ms`, doubling).
    BackingOff,
    /// Not running: its thread could not start, or the section could not be read at start.
    Unavailable,
}

/// Why no message is sent now, though the feature is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWakeQuiet {
    /// A window of `quiet.windows` covers now.
    Window,
    /// `quiet.hold_until_unix_ms` is in the future.
    Hold,
}

/// `RuntimeControlPlaneStatus.agent_wake`: the delivery as the worker knows it, from memory. It
/// is absent with no `agent_wake` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeStatus {
    pub enabled: bool,
    pub worker: AgentWakeWorkerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<AgentWakeQuiet>,
    /// The rows in `prepared`, `deferred` or `sending`.
    pub open: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wake_at_unix_ms: Option<u64>,
    /// The rows that ended `failed`, `unknown`, `expired` or `body_failed` today. A day is the
    /// host's local day.
    pub failed_today: u32,
    /// The candidates dropped today: the feature off, the queue or a burst full, a rule's
    /// cool-down or a day cap (Workflow #432 AGT-2).
    pub suppressed_today: u32,
    /// Something here needs a look: a row that failed today, a day cap used up today, or an
    /// unreadable channel registry.
    pub attention: bool,
}

/// `AgentWakeTest`'s answer: the test wake is queued for the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentWakeTestQueued {
    /// The row's `wake_id` in `host.agent_wake`.
    pub wake_id: String,
    pub channel: String,
    /// Why it will not be sent now, when a quiet time holds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<AgentWakeQuiet>,
}
