// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow 114-A (register `contracts/v012-register.md` §1.236, since 0.12.35; ledger revision
//! 5; design `rd-1031/design-114-reasons-v5.md` §2 with its freeze addendum): the record
//! `policy.not_run_changed`, the change-only history of why a candidate did not run, reader
//! first. This module holds every shape of the record, the ledger's own copies of the
//! `Status` and `ScheduleExplanation` types it embeds, its closed vocabularies and the read-side
//! self-check. No writer exists before 114-B.
//!
//! What the reader checks ([`PolicyNotRunChangedPayload::validate`], every read of every row):
//! the shape (unknown members refused, required members, the numeric ranges), the closed
//! vocabularies (a word outside one does not decode), the self-consistency that depends on no
//! code, and the spelling of every code. It never checks that a code is registered, that a row
//! agrees with the golden table, or that `effective` is the first gate in the one order of the
//! control gates: those are 114-B's write-side checks, so a build that reads a record written
//! by a later build with a new code still reads it, and an unknown code is carried as it is.

use super::{PayloadDetail, validate_catalog_hash, validate_policy_token};
use crate::codes::SanitizationCode;
use crate::outcome::{CodeStr, Outcome};
use crate::{
    DiagnosticCode, EffectDisposition, EventAction, InstanceId, OwnerEpoch, RequestId,
    SanitizationError, SanitizedAudit, SchedulingPauseReason,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The most compact JSON bytes of one record's payload (design §2.5, the precedent
/// `TASK_SELECTION_RECORD_MAX_BYTES`).
pub const NOT_RUN_RECORD_MAX_BYTES: usize = 64 * 1024;
/// The most parts of one baseline (design §2.5: 4 096 tasks of the catalog bound, 256 a part).
pub const NOT_RUN_BASELINE_MAX_PARTS: u32 = 16;
/// The most `rows`, and the most `selected`, of one baseline part.
pub const NOT_RUN_BASELINE_PART_MAX_ITEMS: usize = 256;
/// The most `entries` of one change set: the upper bound of `scheduling.not_run.coalesce_rows`.
pub const NOT_RUN_CHANGE_MAX_ENTRIES: usize = 64;
/// The index prefix of a key's latest record: `not_run/<alias>/<task>` (ledger observer §11.1).
pub const NOT_RUN_INDEX_PREFIX: &str = "not_run/";
/// The index prefix of an alias's latest record of any kind: `not_run_stream/<alias>`.
pub const NOT_RUN_STREAM_INDEX_PREFIX: &str = "not_run_stream/";

/// A closed ledger vocabulary: a plain serde enum whose every word is spelled once, with its
/// frozen order (`ALL`). A word outside it does not decode. Not an outcome vocabulary: the
/// catalog does not list it, and the ledger revision, not the catalog, versions it.
macro_rules! not_run_vocabulary {
    (
        $(#[$meta:meta])*
        $name:ident : $vocabulary:literal {
            $( $(#[$variant_meta:meta])* $variant:ident => $word:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$variant_meta])* #[serde(rename = $word)] $variant ),+
        }

        impl $name {
            /// The vocabulary's name in the design and the contract.
            pub const VOCABULARY: &'static str = $vocabulary;
            /// Every word, in its frozen order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The word's spelling.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $word),+
                }
            }
        }
    };
}

not_run_vocabulary! {
    /// The kind of a record (design §2.5).
    NotRunKind: "not_run_kind" {
        /// This alias's changes of one evaluated round.
        Change => "change",
        /// This alias's complete state, possibly in parts.
        Baseline => "baseline",
        /// The budget marker: this alias writes nothing more in this window.
        Suppressed => "suppressed",
        /// This alias no longer appears.
        Retired => "retired",
    }
}

not_run_vocabulary! {
    /// Why a baseline was written (design §4).
    NotRunBaselineReason: "not_run_baseline_reason" {
        Start => "start",
        Catalog => "catalog",
        Binding => "binding",
        Coalesced => "coalesced",
        Periodic => "periodic",
        Count => "count",
        Budget => "budget",
    }
}

not_run_vocabulary! {
    /// Why a key left the explanation table (design §2.5 `Gone`).
    NotRunGoneReason: "not_run_gone_reason" {
        Selected => "selected",
        TaskAbsent => "task_absent",
    }
}

not_run_vocabulary! {
    /// Where a selected decision's dispatch stands: the five words of `ScheduleExplanation`'s
    /// `ScheduleDispatchState`, as the ledger's own copy.
    NotRunDispatch: "not_run_dispatch" {
        Pending => "pending",
        Intent => "intent",
        Admitted => "admitted",
        Rejected => "rejected",
        Completed => "completed",
    }
}

not_run_vocabulary! {
    /// Which layer gave a row's `why` (design §2.3), as the writer's golden table says.
    NotRunLayer: "not_run_layer" {
        Candidate => "candidate",
        Round => "round",
        Instance => "instance",
        Gate => "gate",
    }
}

not_run_vocabulary! {
    /// The side of the value gate a candidate-layer code is on (design §2.3).
    NotRunValueGate: "not_run_value_gate" {
        Before => "before",
        At => "at",
        After => "after",
    }
}

not_run_vocabulary! {
    /// A row's merged class (design §2.3, §3.2): the waiting-for-a-turn class of ruling 118 A,
    /// or the control class of R-7a §2.6.
    NotRunClass: "not_run_class" {
        Turn => "turn",
        Control => "control",
    }
}

not_run_vocabulary! {
    /// A row's state: the words of `ScheduleCandidateState`, as the ledger's own copy.
    NotRunCandidateState: "not_run_candidate_state" {
        Blocked => "blocked",
        Deferred => "deferred",
    }
}

not_run_vocabulary! {
    /// A row's eligibility: the words of `ScheduleCandidateEligibility`, as the ledger's own
    /// copy.
    NotRunCandidateEligibility: "not_run_candidate_eligibility" {
        True => "true",
        False => "false",
        Unknown => "unknown",
    }
}

not_run_vocabulary! {
    /// An instance pause's stage: the words of `InstancePauseStage`, as the ledger's own copy.
    NotRunInstancePauseStage: "not_run_instance_pause_stage" {
        Draining => "draining",
        Paused => "paused",
        Released => "released",
    }
}

not_run_vocabulary! {
    /// How the previous owner epoch ended (design §6.2).
    NotRunPreviousClose: "not_run_previous_close" {
        Clean => "clean",
        Fatal => "fatal",
        Unconfirmed => "unconfirmed",
        None => "none",
    }
}

not_run_vocabulary! {
    /// Why a merged interval's counts are unknown (design §2.5 `closes`).
    NotRunSuppressedUnknown: "not_run_suppressed_unknown" {
        CrossedEpoch => "crossed_epoch",
    }
}

not_run_vocabulary! {
    /// Why an alias's stream ended (design §2.5 `retired`). `recorder_faulted`: a write-side
    /// check failed and the alias writes nothing more in this owner epoch (its scheduling goes
    /// on); the other two mean the alias is gone.
    NotRunRetiredReason: "not_run_retired_reason" {
        Unconfigured => "unconfigured",
        Unregistered => "unregistered",
        RecorderFaulted => "recorder_faulted",
    }
}

not_run_vocabulary! {
    /// Which budget limit a marker met (design §2.5 `suppressed`).
    NotRunTrigger: "not_run_trigger" {
        Records => "records",
        Bytes => "bytes",
    }
}

not_run_vocabulary! {
    /// The write-side check that failed (design §2.1 "写侧另查什么", §8; the freeze addendum,
    /// minor 10: `retired{recorder_faulted}` carries it as `check`). The same words as the value
    /// key `not_run_check` of 114-B's code.
    NotRunCheck: "not_run_check" {
        CodeRegistered => "code_registered",
        GoldenTable => "golden_table",
        ControlOrder => "control_order",
        Construction => "construction",
    }
}

// The ledger's own copies of the R-7a vocabularies (the freeze addendum §2, R-7a alignment 5):
// plain enums, word for word R-7a's `ALL` (the guard test asserts it), versioned by the ledger
// revision. A word R-7a adds is a new `ledger` revision for these copies too (R-7a §2.6 O-6).

not_run_vocabulary! {
    /// A control gate (R-7a `instance_gate`), in the one order of R-7a §2.6.
    NotRunInstanceGate: "instance_gate" {
        PlayerHold => "player_hold",
        InstancePause => "instance_pause",
        GlobalPause => "global_pause",
        ResourceFence => "resource_fence",
    }
}

not_run_vocabulary! {
    /// A player hold's phase (R-7a `player_hold_phase`). An operation's `phase` and
    /// `failed_phase` take the four other than `held` (a ledger tightening).
    NotRunHoldPhase: "player_hold_phase" {
        Requesting => "requesting",
        Draining => "draining",
        HandingBack => "handing_back",
        Held => "held",
        Releasing => "releasing",
    }
}

not_run_vocabulary! {
    /// Who asked for a player hold operation (R-7a `player_hold_source_actor`).
    NotRunHoldSourceActor: "player_hold_source_actor" {
        User => "user",
        Cli => "cli",
        Lab => "lab",
    }
}

not_run_vocabulary! {
    /// A player hold operation's kind (R-7a `player_hold_operation_kind`).
    NotRunHoldOperationKind: "player_hold_operation_kind" {
        Acquire => "acquire",
        Release => "release",
    }
}

not_run_vocabulary! {
    /// A player hold operation's state (R-7a `player_hold_operation_state`).
    NotRunHoldOperationState: "player_hold_operation_state" {
        Running => "running",
        Completed => "completed",
        Failed => "failed",
    }
}

not_run_vocabulary! {
    /// What a release did next (R-7a `player_hold_next`).
    NotRunHoldNext: "player_hold_next" {
        Reconnected => "reconnected",
        InstancePaused => "instance_paused",
        SchedulingPaused => "scheduling_paused",
    }
}

not_run_vocabulary! {
    /// The pause that holds the startup package a release queued: the two pauses of
    /// `player_hold_next` (R-7a `startup_package_withheld`).
    NotRunStartupPackageWithheld: "player_hold_next" {
        InstancePaused => "instance_paused",
        SchedulingPaused => "scheduling_paused",
    }
}

not_run_vocabulary! {
    /// What a release did with the startup package (R-7a `player_hold_startup_package`).
    NotRunHoldStartupPackage: "player_hold_startup_package" {
        Scheduled => "scheduled",
        Cancelled => "cancelled",
        None => "none",
    }
}

not_run_vocabulary! {
    /// Whether a restored hold had finished its hand-back (R-7a `player_hold_restored_from`).
    NotRunHoldRestoredFrom: "player_hold_restored_from" {
        Acquiring => "acquiring",
        Held => "held",
    }
}

not_run_vocabulary! {
    /// The channel of the connection a hold operation came on (the `channel` vocabulary,
    /// `RuntimeChannel`).
    NotRunChannel: "channel" {
        Cli => "cli",
        Mcp => "mcp",
        Ui => "ui",
        Lab => "lab",
    }
}

not_run_vocabulary! {
    /// The counter that handed a Lab request over (`ChannelVia`).
    NotRunChannelVia: "channel_via" {
        Mcp => "mcp",
        Ui => "ui",
    }
}

impl NotRunHoldPhase {
    /// One of the acquire's three phases, which carry the two deadlines (R-7a §2.2).
    pub const fn is_acquiring(self) -> bool {
        matches!(self, Self::Requesting | Self::Draining | Self::HandingBack)
    }
}

impl NotRunHoldNext {
    /// The pause `startup_package_withheld` names when this is a pause.
    const fn withheld(self) -> Option<NotRunStartupPackageWithheld> {
        match self {
            Self::Reconnected => None,
            Self::InstancePaused => Some(NotRunStartupPackageWithheld::InstancePaused),
            Self::SchedulingPaused => Some(NotRunStartupPackageWithheld::SchedulingPaused),
        }
    }
}

/// `stream`: the record's place in its stream, one alias in one owner epoch (design §2.2);
/// `seq` starts at 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunStream {
    pub owner_epoch: OwnerEpoch,
    pub seq: u64,
}

/// A record of a stream by its ledger sequence and its stream place: `prev`, and a merged
/// interval's `closes.marker`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunRecordRef {
    pub sequence: u64,
    pub owner_epoch: OwnerEpoch,
    pub seq: u64,
}

/// The first part of a baseline by its ledger sequence and its `seq`: `base`, `prev_base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunBase {
    pub sequence: u64,
    pub seq: u64,
}

/// `budget`: this alias's window after this record (design §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunBudget {
    pub window_start_unix_ms: u64,
    pub used: u64,
    pub bytes: u64,
}

/// The previous epoch's tail gap (design §6.2 `previous_gap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunGap {
    pub start_unix_ms: u64,
    pub end_unix_ms: u64,
    pub sequence: u64,
}

/// The previous epoch's last recorded round (design §6.2 `previous_last_cycle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunLastCycle {
    pub eval_seq: u64,
    pub evaluated_at_unix_ms: u64,
}

/// `prior`: how this owner epoch began, on this alias's first record of the epoch (design
/// §6.2). The record's `Prior` keeps `first_evaluated_at_unix_ms` required; the answer's
/// `HeadPrior` of 114-D is another shape (the freeze addendum §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunPrior {
    pub started_at_unix_ms: u64,
    pub first_evaluated_at_unix_ms: u64,
    #[serde(deserialize_with = "required")]
    pub previous_owner_epoch: Option<OwnerEpoch>,
    pub previous_close: NotRunPreviousClose,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_closed_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_gap: Option<NotRunGap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_last_cycle: Option<NotRunLastCycle>,
    #[serde(deserialize_with = "required")]
    pub stream_epoch: Option<OwnerEpoch>,
    #[serde(deserialize_with = "required")]
    pub stream_link: Option<InstanceId>,
    pub suppression_open: bool,
}

/// A merged interval's counts (design §2.5 `closes.suppressed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunSuppressedCounts {
    pub records: u64,
    pub key_changes: u64,
    pub cycles: u64,
}

/// `closes`: on the first record after a merged interval, of any kind (design §2.5, §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunCloses {
    pub marker: NotRunRecordRef,
    pub from_unix_ms: u64,
    pub window_end_unix_ms: u64,
    pub to_unix_ms: u64,
    pub trigger: NotRunTrigger,
    pub limit_records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_bytes: Option<u64>,
    #[serde(deserialize_with = "required")]
    pub suppressed: Option<NotRunSuppressedCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_because: Option<NotRunSuppressedUnknown>,
}

/// The common head of every record (design §2.5), beside `action`, `kind` and `audit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunHead {
    pub instance_alias: String,
    pub stream: NotRunStream,
    /// This alias's previous record, `null` when it has none.
    #[serde(deserialize_with = "required")]
    pub prev: Option<NotRunRecordRef>,
    /// The first part of the latest valid baseline this record folds onto; `null` on a
    /// baseline's first part.
    #[serde(deserialize_with = "required")]
    pub base: Option<NotRunBase>,
    pub evaluated_at_unix_ms: u64,
    pub eval_seq: u64,
    pub catalog_hash: String,
    pub budget: NotRunBudget,
    /// Only on this alias's first record of the owner epoch (design §6.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior: Option<Box<NotRunPrior>>,
    /// Only on the first record after a merged interval (design §5.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closes: Option<Box<NotRunCloses>>,
}

/// A candidate row (design §2.3): `why`, `state`, `eligibility`, `since_unix_ms` and
/// `next_wake_unix_ms` as the same round's `ScheduleExplanation` row has them; `layer`,
/// `value_gate` and `class` as the writer's golden table gave them, read as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunRow {
    pub task_id: String,
    pub why: Outcome,
    pub layer: NotRunLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_gate: Option<NotRunValueGate>,
    pub state: NotRunCandidateState,
    pub eligibility: NotRunCandidateEligibility,
    pub since_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<NotRunClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_since_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_unix_ms: Option<u64>,
}

/// The not-running row of the alias (design §2.5 `NotRunSummary`): `ScheduleExplanation`'s
/// four members, with the golden table's `layer` and class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub why: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_wake_unix_ms: Option<u64>,
    pub since_unix_ms: u64,
    pub layer: NotRunLayer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<NotRunClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_since_unix_ms: Option<u64>,
}

/// The global pause, as the ledger's own copy (design §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunGlobalPause {
    pub revision: u64,
    pub pause_reason: SchedulingPauseReason,
    pub since_unix_ms: u64,
}

/// The instance's own pause, as the ledger's own copy (design §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunInstancePause {
    pub revision: u64,
    pub pause_reason: SchedulingPauseReason,
    pub since_unix_ms: u64,
    pub stage: NotRunInstancePauseStage,
}

/// Who began or ended a hold operation (R-7a `PlayerHoldSource`): `channel` is required (a
/// record without it does not decode), `via` only on the Lab channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunHoldSource {
    pub actor: NotRunHoldSourceActor,
    pub channel: NotRunChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<NotRunChannelVia>,
}

/// The instance's player hold, as the ledger's own copy of R-7a's `PlayerHoldState` (design
/// §2.4, its eleven members).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunPlayerHold {
    pub hold_id: RequestId,
    pub revision: u64,
    pub phase: NotRunHoldPhase,
    pub since_unix_ms: u64,
    pub source: NotRunHoldSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drain_deadline_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_deadline_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_started_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reached_held_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from_owner_epoch: Option<OwnerEpoch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_from_phase: Option<NotRunHoldRestoredFrom>,
}

/// The instance's resource fence, as the ledger's own copy (design §2.4): the members that
/// count for the change key or never change once set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunResourceFence {
    pub cause: CodeStr,
    pub since_unix_ms: u64,
    pub awaiting_person: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_wait_since_unix_ms: Option<u64>,
}

/// The instance's control block (design §2.4): the four gates in effect at the round's start
/// and `effective`, the first of them in the one order of R-7a §2.6 as the writer computed it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunControl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_hold: Option<NotRunPlayerHold>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_pause: Option<NotRunGlobalPause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_pause: Option<NotRunInstancePause>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_fence: Option<NotRunResourceFence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<NotRunInstanceGate>,
}

impl NotRunControl {
    /// Whether the block holds the member of `gate`.
    pub const fn has(&self, gate: NotRunInstanceGate) -> bool {
        match gate {
            NotRunInstanceGate::PlayerHold => self.player_hold.is_some(),
            NotRunInstanceGate::InstancePause => self.instance_pause.is_some(),
            NotRunInstanceGate::GlobalPause => self.global_pause.is_some(),
            NotRunInstanceGate::ResourceFence => self.resource_fence.is_some(),
        }
    }
}

/// The revisions of the pauses in effect when a release ended a hold (R-7a
/// `PlayerHoldPausesAtEnd`): at least one, never 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunPausesAtEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_revision: Option<u64>,
}

impl NotRunPausesAtEnd {
    fn holds(&self) -> bool {
        (self.instance_revision.is_some() || self.global_revision.is_some())
            && self.instance_revision != Some(0)
            && self.global_revision != Some(0)
    }
}

/// The operation that ended a hold, as the ledger's own copy of R-7a's `PlayerHoldOperation`
/// (design §2.4): its `result`'s `next`, `startup_package`, `startup_package_withheld` and
/// `pauses_at_end` flattened, of its `selfcheck` only the failure code; no `hold_id` (the end's
/// own), no `result.revision` and no `result.drained`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunHoldOperation {
    pub request_id: RequestId,
    pub kind: NotRunHoldOperationKind,
    pub state: NotRunHoldOperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<NotRunHoldPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_phase: Option<NotRunHoldPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeStr>,
    pub source: NotRunHoldSource,
    pub since_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<NotRunHoldNext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_package: Option<NotRunHoldStartupPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_package_withheld: Option<NotRunStartupPackageWithheld>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selfcheck_failure_code: Option<CodeStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pauses_at_end: Option<NotRunPausesAtEnd>,
}

/// The accepted operation that replaced the ended hold's record before the writer read it
/// (design §2.4, minor 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunHoldSuperseded {
    pub request_id: RequestId,
    pub kind: NotRunHoldOperationKind,
}

/// How a hold ended (design §2.4 `PlayerHoldEnd`): the ended hold, and its operation, or the
/// operation that replaced it, or neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunPlayerHoldEnd {
    pub hold_id: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<NotRunHoldOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded: Option<NotRunHoldSuperseded>,
}

/// A key's state before a change (design §2.5 `From`), when it was in the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunFromRow {
    pub code: CodeStr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<NotRunClass>,
}

/// A key's state before a change, when it had left the table (design §2.5 `From`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "gone", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotRunFromGone {
    Selected {
        dispatch: NotRunDispatch,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dispatch_code: Option<CodeStr>,
        attempts: u64,
    },
    TaskAbsent {},
}

impl NotRunFromGone {
    /// Why the key had left the table.
    pub const fn gone(&self) -> NotRunGoneReason {
        match self {
            Self::Selected { .. } => NotRunGoneReason::Selected,
            Self::TaskAbsent {} => NotRunGoneReason::TaskAbsent,
        }
    }
}

/// `from`: the key's actual state in the previous round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotRunFrom {
    Row(NotRunFromRow),
    Gone(NotRunFromGone),
}

/// A key that left the table (design §2.5 `Gone`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "gone", rename_all = "snake_case", deny_unknown_fields)]
pub enum NotRunGone {
    Selected {
        decision_id: String,
        dispatch: NotRunDispatch,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dispatch_code: Option<CodeStr>,
        attempts: u64,
    },
    TaskAbsent {},
}

impl NotRunGone {
    /// Why the key left the table.
    pub const fn gone(&self) -> NotRunGoneReason {
        match self {
            Self::Selected { .. } => NotRunGoneReason::Selected,
            Self::TaskAbsent {} => NotRunGoneReason::TaskAbsent,
        }
    }
}

/// `to`: the key's state after the change, a row or gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotRunTo {
    Row(Box<NotRunRow>),
    Gone(NotRunGone),
}

/// One changed key of a change set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunEntry {
    pub task_id: String,
    /// `null` for a key that is new.
    #[serde(deserialize_with = "required")]
    pub from: Option<NotRunFrom>,
    pub to: NotRunTo,
}

/// A change of the control block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunControlChange {
    pub from: NotRunControl,
    pub to: NotRunControl,
}

/// A change of the not-running row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunSummaryChange {
    #[serde(deserialize_with = "required")]
    pub from: Option<NotRunSummary>,
    pub to: NotRunSummary,
}

/// `change`: the alias's changes of one evaluated round (design §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunChange {
    pub entries: Vec<NotRunEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<NotRunControlChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<NotRunSummaryChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_hold_end: Option<NotRunPlayerHoldEnd>,
}

/// A key selected at the baseline's round, out of the table (design §2.5 `selected`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunSelected {
    pub task_id: String,
    pub decision_id: String,
    pub dispatch: NotRunDispatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_code: Option<CodeStr>,
    pub attempts: u64,
}

/// `baseline`: the alias's complete state, one part of at most 16 (design §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunBaseline {
    pub baseline_reason: NotRunBaselineReason,
    pub rows: Vec<NotRunRow>,
    pub selected: Vec<NotRunSelected>,
    /// The keys of the last written state that are neither in `rows` nor in `selected`, as many
    /// as fit; the index rows' only reader of it.
    pub gone: Vec<String>,
    pub control: NotRunControl,
    pub summary: NotRunSummary,
    pub part: u32,
    pub parts: u32,
    #[serde(deserialize_with = "required")]
    pub prev_base: Option<NotRunBase>,
}

/// A marker's window (design §2.5 `suppressed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunWindow {
    pub start_unix_ms: u64,
    pub end_unix_ms: u64,
}

/// `suppressed`: the budget marker (design §2.5, its shape frozen with pending decision 136).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunSuppressed {
    pub window: NotRunWindow,
    pub trigger: NotRunTrigger,
    pub limit_records: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit_bytes: Option<u64>,
    pub pending_key_changes: u64,
}

/// `retired`: the alias no longer appears (design §2.5). `recorder_faulted` carries no `gone`
/// and names the failed write-side check (`check`, the freeze addendum, minor 10); the other
/// two reasons carry no `check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotRunRetired {
    pub reason: NotRunRetiredReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gone: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<NotRunCheck>,
}

/// The members of a record's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum NotRunBody {
    Change(Box<NotRunChange>),
    Baseline(Box<NotRunBaseline>),
    Suppressed(NotRunSuppressed),
    Retired(NotRunRetired),
}

impl NotRunBody {
    pub const fn kind(&self) -> NotRunKind {
        match self {
            Self::Change(_) => NotRunKind::Change,
            Self::Baseline(_) => NotRunKind::Baseline,
            Self::Suppressed(_) => NotRunKind::Suppressed,
            Self::Retired(_) => NotRunKind::Retired,
        }
    }
}

/// `policy.not_run_changed` (register §1.236): one record of one alias's stream. Serialized
/// flat, as the design writes it: `action`, `kind`, the head, the kind's members, `audit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyNotRunChangedPayload {
    action: EventAction,
    head: NotRunHead,
    body: NotRunBody,
    audit: SanitizedAudit,
}

/// The members of the common head, beside `action`, `kind` and `audit`.
const HEAD_MEMBERS: [&str; 10] = [
    "instance_alias",
    "stream",
    "prev",
    "base",
    "evaluated_at_unix_ms",
    "eval_seq",
    "catalog_hash",
    "budget",
    "prior",
    "closes",
];

#[derive(Serialize)]
struct NotRunWire<'a> {
    action: EventAction,
    kind: NotRunKind,
    #[serde(flatten)]
    head: &'a NotRunHead,
    #[serde(flatten)]
    body: &'a NotRunBody,
    audit: &'a SanitizedAudit,
}

impl Serialize for PolicyNotRunChangedPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NotRunWire {
            action: self.action,
            kind: self.body.kind(),
            head: &self.head,
            body: &self.body,
            audit: &self.audit,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PolicyNotRunChangedPayload {
    /// The head and the kind's members decode on their own, each refusing a member it does not
    /// know, so a member of another kind is refused too.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut members = Map::<String, Value>::deserialize(deserializer)?;
        let action = take::<_, D::Error>(&mut members, "action")?;
        let kind = take::<NotRunKind, D::Error>(&mut members, "kind")?;
        let audit = take::<_, D::Error>(&mut members, "audit")?;
        let mut head = Map::new();
        for name in HEAD_MEMBERS {
            if let Some(value) = members.shift_remove(name) {
                head.insert(name.to_owned(), value);
            }
        }
        let head = decode::<NotRunHead, D::Error>(Value::Object(head))?;
        let rest = Value::Object(members);
        let body = match kind {
            NotRunKind::Change => NotRunBody::Change(Box::new(decode::<_, D::Error>(rest)?)),
            NotRunKind::Baseline => NotRunBody::Baseline(Box::new(decode::<_, D::Error>(rest)?)),
            NotRunKind::Suppressed => NotRunBody::Suppressed(decode::<_, D::Error>(rest)?),
            NotRunKind::Retired => NotRunBody::Retired(decode::<_, D::Error>(rest)?),
        };
        Ok(Self {
            action,
            head,
            body,
            audit,
        })
    }
}

/// A member that must be present and may be `null`.
fn required<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn take<T: DeserializeOwned, E: serde::de::Error>(
    members: &mut Map<String, Value>,
    name: &'static str,
) -> Result<T, E> {
    let value = members
        .shift_remove(name)
        .ok_or_else(|| E::missing_field(name))?;
    serde_json::from_value(value).map_err(E::custom)
}

fn decode<T: DeserializeOwned, E: serde::de::Error>(value: Value) -> Result<T, E> {
    serde_json::from_value(value).map_err(E::custom)
}

impl PolicyNotRunChangedPayload {
    pub const fn kind(&self) -> NotRunKind {
        self.body.kind()
    }

    pub const fn head(&self) -> &NotRunHead {
        &self.head
    }

    pub const fn body(&self) -> &NotRunBody {
        &self.body
    }

    /// The tasks whose `not_run/<alias>/<task>` index rows this record names (design §2.6): a
    /// change set's changed keys; a baseline part's `rows`, `selected` and `gone`; a retirement's
    /// `gone`; a marker none. Every record also names `not_run_stream/<alias>`.
    pub fn index_task_ids(&self) -> Vec<&str> {
        match &self.body {
            NotRunBody::Change(change) => change
                .entries
                .iter()
                .map(|entry| entry.task_id.as_str())
                .collect(),
            NotRunBody::Baseline(baseline) => baseline
                .rows
                .iter()
                .map(|row| row.task_id.as_str())
                .chain(baseline.selected.iter().map(|key| key.task_id.as_str()))
                .chain(baseline.gone.iter().map(String::as_str))
                .collect(),
            NotRunBody::Suppressed(_) => Vec::new(),
            NotRunBody::Retired(retired) => {
                retired.gone.iter().flatten().map(String::as_str).collect()
            }
        }
    }

    /// The read-side self-check (design §2.1 "读侧查什么"): the shape and its ranges, the closed
    /// vocabularies (decoded), the self-consistency that depends on no code, and the spelling of
    /// every code (`validate_policy_token`). Never whether a code is registered, the golden
    /// table or the order of the control gates.
    pub(super) fn validate(&self) -> Result<(), SanitizationError> {
        if self.action != EventAction::PolicyNotRun {
            return Err(invalid("action"));
        }
        let head = &self.head;
        head.validate()?;
        if head.closes.is_some() && head.prev.is_none() {
            return Err(invalid("closes"));
        }
        if let Some(prior) = &head.prior
            && !prior.suppression_open
            && prior.first_evaluated_at_unix_ms != head.evaluated_at_unix_ms
        {
            return Err(invalid("prior.first_evaluated_at_unix_ms"));
        }
        match &self.body {
            NotRunBody::Change(change) => change.validate()?,
            NotRunBody::Baseline(baseline) => baseline.validate(head.base.is_none())?,
            NotRunBody::Suppressed(_) => {}
            NotRunBody::Retired(retired) => retired.validate()?,
        }
        let bytes = serde_json::to_vec(self).map_err(|_| invalid("not_run_changed"))?;
        if bytes.len() > NOT_RUN_RECORD_MAX_BYTES {
            return Err(invalid("not_run_changed"));
        }
        Ok(())
    }
}

impl PayloadDetail for PolicyNotRunChangedPayload {
    fn action(&self) -> EventAction {
        self.action
    }

    fn diagnostic_code(&self) -> Option<DiagnosticCode> {
        None
    }

    fn effect_disposition(&self) -> Option<EffectDisposition> {
        None
    }

    fn audit(&self) -> &SanitizedAudit {
        &self.audit
    }
}

impl NotRunHead {
    fn validate(&self) -> Result<(), SanitizationError> {
        crate::validate_instance_alias(&self.instance_alias)
            .map_err(|_| invalid("instance_alias"))?;
        if self.stream.seq == 0 {
            return Err(invalid("stream.seq"));
        }
        if self.prev.is_some_and(|prev| prev.seq == 0) {
            return Err(invalid("prev.seq"));
        }
        if self.base.is_some_and(|base| base.seq == 0) {
            return Err(invalid("base.seq"));
        }
        if self
            .closes
            .as_ref()
            .is_some_and(|closes| closes.marker.seq == 0)
        {
            return Err(invalid("closes.marker.seq"));
        }
        validate_catalog_hash(&self.catalog_hash, "catalog_hash")
    }
}

impl NotRunRow {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_policy_token(&self.task_id, "task_id")?;
        validate_policy_token(self.why.code().as_str(), "why")?;
        if self.value_gate.is_some() && self.layer != NotRunLayer::Candidate {
            return Err(invalid("value_gate"));
        }
        if self.class.is_some() != self.class_since_unix_ms.is_some() {
            return Err(invalid("class_since_unix_ms"));
        }
        Ok(())
    }
}

impl NotRunSummary {
    fn validate(&self) -> Result<(), SanitizationError> {
        if let Some(task_id) = &self.task_id {
            validate_policy_token(task_id, "summary.task_id")?;
        }
        validate_policy_token(self.why.code().as_str(), "summary.why")?;
        if self.class.is_some() != self.class_since_unix_ms.is_some() {
            return Err(invalid("summary.class_since_unix_ms"));
        }
        Ok(())
    }
}

impl NotRunControl {
    fn validate(&self) -> Result<(), SanitizationError> {
        let any = self.player_hold.is_some()
            || self.instance_pause.is_some()
            || self.global_pause.is_some()
            || self.resource_fence.is_some();
        // `effective` is present exactly when a member is, and names a member that is present;
        // whether it is the first in the one order is the writer's check.
        let effective = match self.effective {
            Some(gate) => self.has(gate),
            None => !any,
        };
        if !effective {
            return Err(invalid("control.effective"));
        }
        if let Some(hold) = &self.player_hold {
            hold.validate()?;
        }
        if let Some(fence) = &self.resource_fence {
            validate_policy_token(fence.cause.as_str(), "control.resource_fence.cause")?;
        }
        Ok(())
    }
}

impl NotRunHoldSource {
    /// `via` only on the Lab channel (R-7a `PlayerHoldSource::validate`).
    fn validate(&self) -> Result<(), SanitizationError> {
        if self.via.is_some() && self.channel != NotRunChannel::Lab {
            return Err(invalid("source.via"));
        }
        Ok(())
    }
}

impl NotRunPlayerHold {
    /// Every condition of R-7a's `PlayerHoldState::validate` (the freeze addendum §2, R-7a
    /// alignment 4).
    fn validate(&self) -> Result<(), SanitizationError> {
        self.source.validate()?;
        let acquiring = self.phase.is_acquiring();
        let deadlines = [self.drain_deadline_unix_ms, self.grace_deadline_unix_ms];
        let restored_acquiring =
            self.restored_from_phase == Some(NotRunHoldRestoredFrom::Acquiring);
        let deadlines_hold = if acquiring {
            deadlines.iter().all(Option::is_some)
        } else {
            deadlines.iter().all(Option::is_none)
        };
        let reached_held = if acquiring || restored_acquiring {
            self.reached_held_unix_ms.is_none()
        } else {
            self.reached_held_unix_ms.is_some()
        };
        let holds = self.revision != 0
            && self.since_unix_ms != 0
            && deadlines_hold
            && (self.close_started_unix_ms.is_none() || self.phase == NotRunHoldPhase::HandingBack)
            && reached_held
            && self.restored_from_owner_epoch.is_some() == self.restored_from_phase.is_some();
        if !holds {
            return Err(invalid("control.player_hold"));
        }
        Ok(())
    }
}

impl NotRunHoldOperation {
    /// Every condition of R-7a's `PlayerHoldOperation::validate` on the flattened copy (the
    /// freeze addendum §2, R-7a alignment 4), with `hold_id` the ended hold's; and the ledger's
    /// tightening: `phase` and `failed_phase` are never `held`.
    fn validate(&self, hold_id: RequestId) -> Result<(), SanitizationError> {
        use NotRunHoldOperationKind as Kind;
        use NotRunHoldOperationState as State;
        self.source.validate()?;
        if let Some(code) = &self.code {
            validate_policy_token(code.as_str(), "player_hold_end.operation.code")?;
        }
        if let Some(code) = &self.selfcheck_failure_code {
            validate_policy_token(
                code.as_str(),
                "player_hold_end.operation.selfcheck_failure_code",
            )?;
        }
        let not_held = self.phase != Some(NotRunHoldPhase::Held)
            && self.failed_phase != Some(NotRunHoldPhase::Held);
        let by_state = match self.state {
            State::Running => {
                self.ended_unix_ms.is_none()
                    && self.code.is_none()
                    && self.failed_phase.is_none()
                    && self.phase.is_some_and(|phase| match self.kind {
                        Kind::Acquire => phase.is_acquiring(),
                        Kind::Release => phase == NotRunHoldPhase::Releasing,
                    })
            }
            State::Failed => {
                self.ended_unix_ms.is_some() && self.code.is_some() && self.failed_phase.is_some()
            }
            State::Completed => {
                self.ended_unix_ms.is_some() && self.code.is_none() && self.failed_phase.is_none()
            }
        };
        let release_members = self.next.is_some()
            || self.startup_package.is_some()
            || self.startup_package_withheld.is_some()
            || self.pauses_at_end.is_some()
            || self.selfcheck_failure_code.is_some();
        let by_kind = match (self.kind, self.state) {
            (Kind::Release, State::Completed) => match (self.next, self.startup_package) {
                (Some(next), Some(startup_package)) => {
                    self.release_shape_holds(next, startup_package)
                }
                _ => false,
            },
            // An acquire, and a release that did not complete, carry no release result.
            _ => !release_members,
        };
        let identity = self.kind == Kind::Release || self.request_id == hold_id;
        if !(not_held && by_state && by_kind && identity && self.since_unix_ms != 0) {
            return Err(invalid("player_hold_end.operation"));
        }
        Ok(())
    }

    /// R-7a §3.2's "the result's shape self-check" on the copy: `reconnected` carries neither
    /// `pauses_at_end` nor `startup_package_withheld`; `instance_paused` carries
    /// `pauses_at_end.instance_revision`; `scheduling_paused` carries
    /// `pauses_at_end.global_revision` and no `instance_revision`; a pause carries no self-check
    /// failure; `startup_package_withheld` only beside `startup_package: scheduled`, equal to
    /// `next`. R-7a's "`reconnected` carries a complete `selfcheck`" is not checked: the copy
    /// keeps only its failure code.
    fn release_shape_holds(
        &self,
        next: NotRunHoldNext,
        startup_package: NotRunHoldStartupPackage,
    ) -> bool {
        let pauses = self.pauses_at_end.as_ref();
        let selfcheck_failure = self.selfcheck_failure_code.is_some();
        let by_next = match next {
            NotRunHoldNext::Reconnected => {
                pauses.is_none() && self.startup_package_withheld.is_none()
            }
            NotRunHoldNext::InstancePaused => {
                !selfcheck_failure && pauses.is_some_and(|p| p.instance_revision.is_some())
            }
            NotRunHoldNext::SchedulingPaused => {
                !selfcheck_failure
                    && pauses.is_some_and(|p| {
                        p.global_revision.is_some() && p.instance_revision.is_none()
                    })
            }
        };
        let withheld = self.startup_package_withheld.is_none_or(|withheld| {
            startup_package == NotRunHoldStartupPackage::Scheduled
                && next.withheld() == Some(withheld)
        });
        by_next && withheld && pauses.is_none_or(NotRunPausesAtEnd::holds)
    }
}

impl NotRunPlayerHoldEnd {
    fn validate(&self) -> Result<(), SanitizationError> {
        if self.operation.is_some() && self.superseded.is_some() {
            return Err(invalid("player_hold_end"));
        }
        if let Some(operation) = &self.operation {
            operation.validate(self.hold_id)?;
        }
        Ok(())
    }
}

impl NotRunEntry {
    fn validate(&self) -> Result<(), SanitizationError> {
        validate_policy_token(&self.task_id, "entries.task_id")?;
        match &self.from {
            Some(NotRunFrom::Row(row)) => {
                validate_policy_token(row.code.as_str(), "entries.from.code")?;
            }
            Some(NotRunFrom::Gone(NotRunFromGone::Selected {
                dispatch_code: Some(code),
                ..
            })) => {
                validate_policy_token(code.as_str(), "entries.from.dispatch_code")?;
            }
            Some(NotRunFrom::Gone(_)) | None => {}
        }
        match &self.to {
            NotRunTo::Row(row) => row.validate(),
            NotRunTo::Gone(NotRunGone::Selected {
                decision_id,
                dispatch_code,
                ..
            }) => {
                validate_policy_token(decision_id, "entries.to.decision_id")?;
                if let Some(code) = dispatch_code {
                    validate_policy_token(code.as_str(), "entries.to.dispatch_code")?;
                }
                Ok(())
            }
            NotRunTo::Gone(NotRunGone::TaskAbsent {}) => Ok(()),
        }
    }
}

impl NotRunChange {
    fn validate(&self) -> Result<(), SanitizationError> {
        if self.entries.len() > NOT_RUN_CHANGE_MAX_ENTRIES {
            return Err(invalid("entries"));
        }
        // An empty change set carries a control change, a summary change or a hold's end.
        if self.entries.is_empty()
            && self.control.is_none()
            && self.summary.is_none()
            && self.player_hold_end.is_none()
        {
            return Err(invalid("entries"));
        }
        for entry in &self.entries {
            entry.validate()?;
        }
        if let Some(control) = &self.control {
            control.from.validate()?;
            control.to.validate()?;
        }
        if let Some(summary) = &self.summary {
            if let Some(from) = &summary.from {
                from.validate()?;
            }
            summary.to.validate()?;
        }
        if let Some(end) = &self.player_hold_end {
            end.validate()?;
        }
        Ok(())
    }
}

impl NotRunBaseline {
    /// `first_part`: the record's `base` is `null`, which only the first part writes.
    fn validate(&self, first_part: bool) -> Result<(), SanitizationError> {
        if self.part == 0 || self.part > self.parts || self.parts > NOT_RUN_BASELINE_MAX_PARTS {
            return Err(invalid("part"));
        }
        if (self.part == 1) != first_part {
            return Err(invalid("base"));
        }
        if self.prev_base.is_some_and(|base| base.seq == 0) {
            return Err(invalid("prev_base.seq"));
        }
        if self.rows.len() > NOT_RUN_BASELINE_PART_MAX_ITEMS
            || self.selected.len() > NOT_RUN_BASELINE_PART_MAX_ITEMS
        {
            return Err(invalid("rows"));
        }
        for row in &self.rows {
            row.validate()?;
        }
        for key in &self.selected {
            validate_policy_token(&key.task_id, "selected.task_id")?;
            validate_policy_token(&key.decision_id, "selected.decision_id")?;
            if let Some(code) = &key.dispatch_code {
                validate_policy_token(code.as_str(), "selected.dispatch_code")?;
            }
        }
        for task_id in &self.gone {
            validate_policy_token(task_id, "gone")?;
        }
        self.control.validate()?;
        self.summary.validate()
    }
}

impl NotRunRetired {
    fn validate(&self) -> Result<(), SanitizationError> {
        let faulted = self.reason == NotRunRetiredReason::RecorderFaulted;
        if faulted && self.gone.is_some() {
            return Err(invalid("gone"));
        }
        if faulted != self.check.is_some() {
            return Err(invalid("check"));
        }
        for task_id in self.gone.iter().flatten() {
            validate_policy_token(task_id, "gone")?;
        }
        Ok(())
    }
}

fn invalid(field: &'static str) -> SanitizationError {
    SanitizationError::new(SanitizationCode::InvalidPolicyPayload, field)
}

#[cfg(test)]
#[path = "not_run_tests.rs"]
mod tests;
