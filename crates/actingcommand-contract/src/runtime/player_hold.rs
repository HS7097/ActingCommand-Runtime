// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #429 R-7a (register §1.233, since 0.12.35; `runtime-client` 4): the player hold's
//! frozen contract ([contracts/player-hold.md](../../../../contracts/player-hold.md); design
//! `rd-1031/design-r7a-hold-v3.1.md`). A player hold is one instance's own state, never a
//! scheduling pause: while a person plays on the device the Runtime touches it not at all.
//!
//! This module holds the shapes and the pure functions every counter shares:
//! - the two requests' members and their two results (`PlayerHoldAcquired`,
//!   `PlayerHoldReleased`), the status members `player_hold`, `player_hold_operation` and
//!   `control`, and their shape self-checks;
//! - the one order of the control gates ([`ControlGates::evaluate`], R-7a §2.6) and the status
//!   functions built on it ([`RuntimeControlPlaneStatus::instance_gates`],
//!   [`RuntimeControlPlaneStatus::hold_ended_with_pause`],
//!   [`RuntimeControlPlaneStatus::with_derived_control`]);
//! - the settlement of a lost receipt ([`settle_player_hold`], the first-match table of §3.3.3);
//! - the receipt waits and the receipt-read deadlines of the two requests.
//!
//! R-7a's Runtime answers both requests `not_implemented` and fills none of the new members;
//! R-7b adds the behaviour.

use super::{
    CONNECTION_PREPARATION_CLAIM_WAIT_MS, CONNECTION_PREPARATION_WAIT_MS,
    MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS, MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS,
    RuntimeContractError, RuntimeContractResult, RuntimeControlPlaneStatus, RuntimeInstanceStatus,
    SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS, SchedulingDrainSummary, SchedulingResumeSelfCheck,
    StartupPackageWithheld, validate_instance_alias,
};
use crate::codes::RuntimeContractCode;
use crate::outcome::CodeStr;
use crate::{ChannelVia, OwnerEpoch, RequestId, RuntimeChannel};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The client's allowance for the device-session close of a hand-back (R-7a §3.1): added to the
/// acquire's receipt wait and its receipt-read deadline only. No source bounds the close; R-7b
/// measures it on a fake device and sets the value. Changing it changes no wire shape.
pub const PLAYER_HOLD_CLOSE_ALLOWANCE_MS: u64 = 30_000;

/// Closed bounds of `AcquirePlayerHold.drain_timeout_ms`, those of an instance pause's drain.
pub const MIN_PLAYER_HOLD_DRAIN_TIMEOUT_MS: u64 = MIN_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS;
pub const MAX_PLAYER_HOLD_DRAIN_TIMEOUT_MS: u64 = MAX_SCHEDULING_PAUSE_DRAIN_TIMEOUT_MS;

/// The receipt-read deadline of an `AcquirePlayerHold` (R-7a §3.1): the drain timeout, the
/// checkpoint grace and the close allowance, the receipt wait without its IO margin. `None` on
/// overflow.
pub fn player_hold_acquire_receipt_deadline(drain_timeout_ms: u64) -> Option<Duration> {
    drain_timeout_ms
        .checked_add(SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS)
        .and_then(|ms| ms.checked_add(PLAYER_HOLD_CLOSE_ALLOWANCE_MS))
        .map(Duration::from_millis)
}

/// The receipt wait of an `AcquirePlayerHold` (R-7a §3.1), the socket read timeout:
/// `drain_timeout_ms + SCHEDULING_PAUSE_CHECKPOINT_GRACE_MS + PLAYER_HOLD_CLOSE_ALLOWANCE_MS +
/// io`. `None` on overflow.
pub fn player_hold_acquire_receipt_wait(drain_timeout_ms: u64, io: Duration) -> Option<Duration> {
    player_hold_acquire_receipt_deadline(drain_timeout_ms)?.checked_add(io)
}

/// The receipt-read deadline of a `ReleasePlayerHold` (R-7a §3.2): the reconnect claim's wait
/// and the reconnect's bound, 33 000 ms, the receipt wait without its IO margin.
pub const fn player_hold_release_receipt_deadline() -> Duration {
    Duration::from_millis(CONNECTION_PREPARATION_CLAIM_WAIT_MS + CONNECTION_PREPARATION_WAIT_MS)
}

/// The receipt wait of a `ReleasePlayerHold` (R-7a §3.2), the socket read timeout:
/// `CONNECTION_PREPARATION_CLAIM_WAIT_MS + CONNECTION_PREPARATION_WAIT_MS + io`, as an instance
/// resume. `None` on overflow.
pub fn player_hold_release_receipt_wait(io: Duration) -> Option<Duration> {
    player_hold_release_receipt_deadline().checked_add(io)
}

crate::outcome_vocabulary! {
    /// Where a player hold stands (vocabulary `player_hold_phase`, R-7a §2.2). The first three
    /// are the acquire's: the gate is closed and queued operator and client entries end, the
    /// in-flight runs drain, the device is handed back and its session closed. `held`: the
    /// Runtime touches the device not at all. `releasing`: the release takes its reconnect
    /// claim and lifts the hold.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldPhase: "player_hold_phase" {
        Requesting => "requesting",
        Draining => "draining",
        HandingBack => "handing_back",
        Held => "held",
        Releasing => "releasing",
    }
}

impl PlayerHoldPhase {
    /// One of the acquire's three phases, which carry the two deadlines.
    pub const fn is_acquiring(self) -> bool {
        matches!(self, Self::Requesting | Self::Draining | Self::HandingBack)
    }
}

crate::outcome_vocabulary! {
    /// What a release did next (vocabulary `player_hold_next`, R-7a §3.2): reconnected the device,
    /// or touched nothing because a pause holds the instance. With both pauses in effect it is
    /// `instance_paused`, the first by R-7a §2.6.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldNext: "player_hold_next" {
        Reconnected => "reconnected",
        InstancePaused => "instance_paused",
        SchedulingPaused => "scheduling_paused",
    }
}

impl PlayerHoldNext {
    /// The pause `startup_package_withheld` names when this is a pause.
    const fn withheld(self) -> Option<StartupPackageWithheld> {
        match self {
            Self::Reconnected => None,
            Self::InstancePaused => Some(StartupPackageWithheld::InstancePaused),
            Self::SchedulingPaused => Some(StartupPackageWithheld::SchedulingPaused),
        }
    }

    /// The token of `startup_package_withheld` in this vocabulary (its two values are this
    /// vocabulary's two pauses).
    pub const fn of_withheld(withheld: StartupPackageWithheld) -> Self {
        match withheld {
            StartupPackageWithheld::InstancePaused => Self::InstancePaused,
            StartupPackageWithheld::SchedulingPaused => Self::SchedulingPaused,
        }
    }
}

crate::outcome_vocabulary! {
    /// One control gate of an instance (vocabulary `instance_gate`, R-7a §2.6 O-1), in the one
    /// order of the control gates: the first in effect decides the instance's effective code.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum InstanceGate: "instance_gate" {
        /// 1 (1a): the instance's player hold, in any phase.
        PlayerHold => "player_hold",
        /// 2 (1b): the instance's own scheduling pause, in any stage.
        InstancePause => "instance_pause",
        /// 3 (1c): the global scheduling pause.
        GlobalPause => "global_pause",
        /// 4 (1d): the instance's #403 resource fence, in any state.
        ResourceFence => "resource_fence",
    }
}

crate::outcome_vocabulary! {
    /// The effective code of a control gate (vocabulary `control_gate_code`, R-7a §2.6 O-1): each
    /// token is the spelling of the runtime-host code the gate defers a dispatch with
    /// (`dispatch_player_held`, `dispatch_paused_instance`, `dispatch_paused_global`,
    /// `dispatch_resource_fenced`). The contract derives it; the host emits the code itself.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum ControlGateCode: "control_gate_code" {
        DispatchPlayerHeld => "dispatch_player_held",
        DispatchPausedInstance => "dispatch_paused_instance",
        DispatchPausedGlobal => "dispatch_paused_global",
        DispatchResourceFenced => "dispatch_resource_fenced",
    }
}

impl InstanceGate {
    /// The gate's effective code (R-7a §2.6 O-1).
    pub const fn code(self) -> ControlGateCode {
        match self {
            Self::PlayerHold => ControlGateCode::DispatchPlayerHeld,
            Self::InstancePause => ControlGateCode::DispatchPausedInstance,
            Self::GlobalPause => ControlGateCode::DispatchPausedGlobal,
            Self::ResourceFence => ControlGateCode::DispatchResourceFenced,
        }
    }
}

crate::outcome_vocabulary! {
    /// The kind of a player hold operation (vocabulary `player_hold_operation_kind`).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldOperationKind: "player_hold_operation_kind" {
        Acquire => "acquire",
        Release => "release",
    }
}

crate::outcome_vocabulary! {
    /// The state of a player hold operation (vocabulary `player_hold_operation_state`).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldOperationState: "player_hold_operation_state" {
        Running => "running",
        Completed => "completed",
        Failed => "failed",
    }
}

crate::outcome_vocabulary! {
    /// Whether a restored hold had finished its hand-back before the Runtime stopped (vocabulary
    /// `player_hold_restored_from`, R-7a §2.4): its persisted row had `reached_held_unix_ms`.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldRestoredFrom: "player_hold_restored_from" {
        Acquiring => "acquiring",
        Held => "held",
    }
}

crate::outcome_vocabulary! {
    /// What a release did with the instance's startup package (vocabulary
    /// `player_hold_startup_package`, R-7a §3.2).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldStartupPackage: "player_hold_startup_package" {
        /// A new startup package claim was queued (behind the pause, when a pause holds it).
        Scheduled => "scheduled",
        /// `skip` cancelled a queued startup package claim and ended its W2-B row.
        Cancelled => "cancelled",
        /// Nothing was queued or cancelled.
        None => "none",
    }
}

crate::outcome_vocabulary! {
    /// Who asked for a player hold operation (vocabulary `player_hold_source_actor`): the
    /// request's envelope, one of the source gate's three (R-7a §3.1 step 2).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldSourceActor: "player_hold_source_actor" {
        /// `(User, Ui)`.
        User => "user",
        /// `(Cli, Cli)`: actingctl, and MCP on the channel `mcp`.
        Cli => "cli",
        /// `(Lab, Lab)`.
        Lab => "lab",
    }
}

crate::outcome_vocabulary! {
    /// Why a client settles a player hold request from `Status` (vocabulary
    /// `player_hold_settlement_trigger`, R-7a §3.3.2).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldSettlementTrigger: "player_hold_settlement_trigger" {
        /// The connection ended before the receipt header of the frame this call wrote.
        Dropped => "dropped",
        /// The receipt-read deadline passed (`runtime_receipt_timeout`).
        TimedOut => "timed_out",
        /// Any other failure after the frame was sent, or the process that sent it is gone: the
        /// most cautious trigger, the one a stored attempt carries.
        Reopened => "reopened",
    }
}

crate::outcome_vocabulary! {
    /// A settlement's verdict (vocabulary `player_hold_settlement_verdict`, R-7a §3.3.3).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldSettlementVerdict: "player_hold_settlement_verdict" {
        Settled => "settled",
        Unconfirmed => "unconfirmed",
        Unsupported => "unsupported",
    }
}

crate::outcome_vocabulary! {
    /// What a settled request did (vocabulary `player_hold_settlement_outcome`, R-7a §3.3.3).
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldSettlementOutcome: "player_hold_settlement_outcome" {
        Completed => "completed",
        Failed => "failed",
        NotPerformed => "not_performed",
        EndedByOther => "ended_by_other",
        HoldEnded => "hold_ended",
    }
}

crate::outcome_vocabulary! {
    /// What a settlement saw when it could not confirm (vocabulary
    /// `player_hold_unconfirmed_observed`, R-7a §3.3.3). The first three are not final: read
    /// `Status` again later and settle again.
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum PlayerHoldUnconfirmedObserved: "player_hold_unconfirmed_observed" {
        InProgress => "in_progress",
        InProgressOther => "in_progress_other",
        StatusUnreadable => "status_unreadable",
        Superseded => "superseded",
        EpochChanged => "epoch_changed",
        InstanceNotListed => "instance_not_listed",
        Inconsistent => "inconsistent",
        NotRegistered => "not_registered",
    }
}

impl PlayerHoldUnconfirmedObserved {
    /// Whether a later read can still settle it (R-7a §3.3.3 "final").
    pub const fn is_final(self) -> bool {
        !matches!(
            self,
            Self::InProgress | Self::InProgressOther | Self::StatusUnreadable
        )
    }
}

crate::outcome_vocabulary! {
    /// The row of the settlement table that decided (vocabulary `player_hold_settlement_rule`,
    /// R-7a §3.3.3), for tests and diagnosis.
    #[derive(Serialize, Deserialize)]
    pub enum PlayerHoldSettlementRule: "player_hold_settlement_rule" {
        P1 => "P1",
        P2 => "P2",
        A1 => "A1",
        A2 => "A2",
        A3 => "A3",
        A4 => "A4",
        A5 => "A5",
        A6 => "A6",
        A7 => "A7",
        A8 => "A8",
        A9 => "A9",
        A10 => "A10",
        A11 => "A11",
        A12 => "A12",
        A13 => "A13",
        R1 => "R1",
        R2 => "R2",
        R3 => "R3",
        R4 => "R4",
        R5 => "R5",
        R6 => "R6",
        R7 => "R7",
        R8 => "R8",
        R9 => "R9",
        R10 => "R10",
        R11 => "R11",
    }
}

/// What a release asks for the instance's startup package (R-7a §3.2): `run` (absent means
/// `run`) queues a new startup package claim, `skip` cancels a queued one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerHoldStartupChoice {
    Run,
    Skip,
}

/// Who began or ended a player hold operation (R-7a §2.4): the envelope's actor, and the channel
/// of the connection the request came on (always present: a request that reaches the handler
/// has a channel), with `via` only on the Lab channel. The channel only says who; it never
/// decides anything (Alice's ruling 135).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldSource {
    pub actor: PlayerHoldSourceActor,
    pub channel: RuntimeChannel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<ChannelVia>,
}

impl PlayerHoldSource {
    /// `via` only on the Lab channel (`invalid_player_hold_state`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.via.is_some() && self.channel != RuntimeChannel::Lab {
            return Err(invalid_state());
        }
        Ok(())
    }
}

/// One instance's player hold as `Status` reports it (`RuntimeInstanceStatus.player_hold`,
/// R-7a §3.4). The two deadlines stand in the acquire's three phases only, `close_started_unix_ms`
/// only in `handing_back` once the close began; `reached_held_unix_ms` once the hand-back
/// finished (a hold restored after a stop during its acquire has none); the two `restored_*`
/// only on a hold restored at a start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldState {
    /// The `request_id` of the `AcquirePlayerHold` that set it.
    pub hold_id: RequestId,
    /// The instance's hold slot revision in this owner epoch: one more at each set and each lift.
    pub revision: u64,
    pub phase: PlayerHoldPhase,
    pub since_unix_ms: u64,
    pub source: PlayerHoldSource,
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
    pub restored_from_phase: Option<PlayerHoldRestoredFrom>,
}

impl PlayerHoldState {
    /// The hold's own shape (R-7a §3.3.1, `invalid_player_hold_state`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.source.validate()?;
        let acquiring = self.phase.is_acquiring();
        let deadlines = [self.drain_deadline_unix_ms, self.grace_deadline_unix_ms];
        let restored_acquiring =
            self.restored_from_phase == Some(PlayerHoldRestoredFrom::Acquiring);
        let holds = self.revision != 0
            && self.since_unix_ms != 0
            // The two deadlines: both in the acquire's phases, neither after.
            && if acquiring {
                deadlines.iter().all(Option::is_some)
            } else {
                deadlines.iter().all(Option::is_none)
            }
            && (self.close_started_unix_ms.is_none() || self.phase == PlayerHoldPhase::HandingBack)
            // `reached_held_unix_ms`: never while acquiring; on `held` and `releasing` unless the
            // hold was restored from a stop during its acquire, which never has it.
            && if acquiring || restored_acquiring {
                self.reached_held_unix_ms.is_none()
            } else {
                self.reached_held_unix_ms.is_some()
            }
            && self.restored_from_owner_epoch.is_some() == self.restored_from_phase.is_some();
        if !holds {
            return Err(invalid_state());
        }
        Ok(())
    }
}

/// The hold a `ReleasePlayerHold` may end (R-7a §3.2): the owner epoch, the hold and the slot
/// revision the caller saw. The Runtime compares the three under the hold table's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldExpectation {
    pub owner_epoch: OwnerEpoch,
    pub hold_id: RequestId,
    pub revision: u64,
}

impl PlayerHoldExpectation {
    /// A revision of 0 names no hold (`invalid_player_hold_expectation`); a `hold_id` of the
    /// wrong form does not decode.
    pub fn validate(&self) -> RuntimeContractResult<()> {
        if self.revision == 0 {
            return Err(RuntimeContractError::new(
                RuntimeContractCode::InvalidPlayerHoldExpectation,
            ));
        }
        Ok(())
    }
}

/// The revisions of the pauses in effect when a release ended a hold (R-7a §2.1): at least one,
/// present only when the release's `next` is a pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldPausesAtEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_revision: Option<u64>,
}

impl PlayerHoldPausesAtEnd {
    fn holds(&self) -> bool {
        (self.instance_revision.is_some() || self.global_revision.is_some())
            && self.instance_revision != Some(0)
            && self.global_revision != Some(0)
    }
}

/// Whether a release's answer has the shape its `next` asks for (R-7a §3.2, "the result's shape
/// self-check"): `reconnected` carries `selfcheck` and neither `pauses_at_end` nor
/// `startup_package_withheld`; `instance_paused` carries `pauses_at_end.instance_revision`;
/// `scheduling_paused` carries `pauses_at_end.global_revision` and no `instance_revision`; a
/// pause carries no `selfcheck`; `startup_package_withheld` only beside `startup_package:
/// scheduled`, and equal to `next`.
fn release_shape_holds(
    next: PlayerHoldNext,
    selfcheck: Option<&SchedulingResumeSelfCheck>,
    startup_package: PlayerHoldStartupPackage,
    startup_package_withheld: Option<StartupPackageWithheld>,
    pauses_at_end: Option<&PlayerHoldPausesAtEnd>,
) -> bool {
    let by_next = match next {
        PlayerHoldNext::Reconnected => {
            selfcheck.is_some() && pauses_at_end.is_none() && startup_package_withheld.is_none()
        }
        PlayerHoldNext::InstancePaused => {
            selfcheck.is_none() && pauses_at_end.is_some_and(|p| p.instance_revision.is_some())
        }
        PlayerHoldNext::SchedulingPaused => {
            selfcheck.is_none()
                && pauses_at_end
                    .is_some_and(|p| p.global_revision.is_some() && p.instance_revision.is_none())
        }
    };
    let withheld = startup_package_withheld.is_none_or(|withheld| {
        startup_package == PlayerHoldStartupPackage::Scheduled && next.withheld() == Some(withheld)
    });
    by_next
        && withheld
        && pauses_at_end.is_none_or(PlayerHoldPausesAtEnd::holds)
        && selfcheck.is_none_or(|selfcheck| selfcheck.validate().is_ok())
}

/// The result of a player hold operation in its record (R-7a §3.3.1): an acquire's `revision`
/// (the hold's) and `drained`; a release's `revision` (the slot's after the lift), `next`,
/// `selfcheck`, `startup_package`, `startup_package_withheld` and `pauses_at_end`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldOperationResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drained: Option<SchedulingDrainSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<PlayerHoldNext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selfcheck: Option<SchedulingResumeSelfCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_package: Option<PlayerHoldStartupPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_package_withheld: Option<StartupPackageWithheld>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pauses_at_end: Option<PlayerHoldPausesAtEnd>,
}

impl PlayerHoldOperationResult {
    /// No release member: what an acquire's result may not carry.
    fn has_release_members(&self) -> bool {
        self.next.is_some()
            || self.selfcheck.is_some()
            || self.startup_package.is_some()
            || self.startup_package_withheld.is_some()
            || self.pauses_at_end.is_some()
    }
}

/// One instance's last accepted player hold operation in this owner epoch, from memory only
/// (`RuntimeInstanceStatus.player_hold_operation`, R-7a §3.3.1). Accepting an operation records
/// it before any state change, persistence or receipt; an entry refusal neither writes nor
/// replaces it; only the next accepted operation replaces it, and a `running` one ends only by
/// its own final state. It stays after the hold is gone, until the next owner epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldOperation {
    pub request_id: RequestId,
    pub kind: PlayerHoldOperationKind,
    pub state: PlayerHoldOperationState,
    /// While `running`: an acquire's is the hold's phase now, a release's `releasing`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PlayerHoldPhase>,
    /// An acquire's is its own `request_id`; a release's is its `expected.hold_id`.
    pub hold_id: RequestId,
    pub source: PlayerHoldSource,
    pub since_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_unix_ms: Option<u64>,
    /// The failure's code, when `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeStr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_phase: Option<PlayerHoldPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<PlayerHoldOperationResult>,
}

impl PlayerHoldOperation {
    /// The record's shape (R-7a §3.3.1, `invalid_player_hold_state`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.source.validate()?;
        let by_state = match self.state {
            PlayerHoldOperationState::Running => {
                self.ended_unix_ms.is_none()
                    && self.code.is_none()
                    && self.failed_phase.is_none()
                    && self.phase.is_some_and(|phase| match self.kind {
                        PlayerHoldOperationKind::Acquire => phase.is_acquiring(),
                        PlayerHoldOperationKind::Release => phase == PlayerHoldPhase::Releasing,
                    })
            }
            PlayerHoldOperationState::Failed => {
                self.ended_unix_ms.is_some() && self.code.is_some() && self.failed_phase.is_some()
            }
            PlayerHoldOperationState::Completed => {
                self.ended_unix_ms.is_some()
                    && self.result.is_some()
                    && self.code.is_none()
                    && self.failed_phase.is_none()
            }
        };
        let by_kind = match (self.kind, &self.result) {
            (_, None) => true,
            (PlayerHoldOperationKind::Acquire, Some(result)) => !result.has_release_members(),
            (PlayerHoldOperationKind::Release, Some(result)) => {
                match (self.state, result.next, result.startup_package) {
                    (PlayerHoldOperationState::Completed, Some(next), Some(startup_package)) => {
                        release_shape_holds(
                            next,
                            result.selfcheck.as_ref(),
                            startup_package,
                            result.startup_package_withheld,
                            result.pauses_at_end.as_ref(),
                        )
                    }
                    (PlayerHoldOperationState::Completed, _, _) => false,
                    // A release that did not complete carries no release result.
                    (_, _, _) => !result.has_release_members(),
                }
            }
        };
        let identity =
            self.kind == PlayerHoldOperationKind::Release || self.hold_id == self.request_id;
        if !(by_state && by_kind && identity && self.since_unix_ms != 0) {
            return Err(invalid_state());
        }
        Ok(())
    }
}

/// `AcquirePlayerHold`'s answer once the hold is `held` (R-7a §3.1): the hold, and what the
/// drain did with the instance's in-flight runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldAcquired {
    pub hold: PlayerHoldState,
    pub drained: SchedulingDrainSummary,
}

impl PlayerHoldAcquired {
    /// The hold's own shape, in phase `held` (`invalid_player_hold_receipt`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        self.hold.validate()?;
        if self.hold.phase != PlayerHoldPhase::Held {
            return Err(invalid_receipt());
        }
        Ok(())
    }
}

/// `ReleasePlayerHold`'s answer (R-7a §3.2). `revision` is the slot's after the lift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldReleased {
    pub instance_alias: String,
    pub hold_id: RequestId,
    pub revision: u64,
    pub next: PlayerHoldNext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selfcheck: Option<SchedulingResumeSelfCheck>,
    pub startup_package: PlayerHoldStartupPackage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_package_withheld: Option<StartupPackageWithheld>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pauses_at_end: Option<PlayerHoldPausesAtEnd>,
}

impl PlayerHoldReleased {
    /// The shape self-check of R-7a §3.2 (`invalid_player_hold_receipt`).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        validate_instance_alias(&self.instance_alias)?;
        if self.revision == 0
            || !release_shape_holds(
                self.next,
                self.selfcheck.as_ref(),
                self.startup_package,
                self.startup_package_withheld,
                self.pauses_at_end.as_ref(),
            )
        {
            return Err(invalid_receipt());
        }
        Ok(())
    }
}

/// The four inputs of one control decision (R-7a §2.6 O-3), read at one moment (O-4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlInputs {
    pub player_hold: bool,
    pub instance_pause: bool,
    pub global_pause: bool,
    pub resource_fence: bool,
}

/// The control gates in effect on one instance, in the one order of R-7a §2.6 O-1: `dispatch`
/// holds automatic dispatch and routine claims, `device` every device operation (only the player
/// hold). [`ControlGates::evaluate`] is the only place the order is written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlGates {
    pub dispatch: Vec<InstanceGate>,
    pub device: Vec<InstanceGate>,
}

impl ControlGates {
    /// The gates the inputs put in effect, in order (R-7a §2.6 O-1, O-3).
    pub fn evaluate(inputs: ControlInputs) -> Self {
        let dispatch = [
            (inputs.player_hold, InstanceGate::PlayerHold),
            (inputs.instance_pause, InstanceGate::InstancePause),
            (inputs.global_pause, InstanceGate::GlobalPause),
            (inputs.resource_fence, InstanceGate::ResourceFence),
        ]
        .into_iter()
        .filter_map(|(active, gate)| active.then_some(gate))
        .collect();
        let device = if inputs.player_hold {
            vec![InstanceGate::PlayerHold]
        } else {
            Vec::new()
        };
        Self { dispatch, device }
    }

    /// The effective code: the first dispatch gate's (R-7a §2.6 O-1); `None` with no gate.
    pub fn code(&self) -> Option<ControlGateCode> {
        self.dispatch.first().map(|gate| gate.code())
    }
}

/// An instance's control gates as `Status` reports them (`RuntimeInstanceStatus.control`, R-7a
/// §2.6 O-7): only [`RuntimeControlPlaneStatus::with_derived_control`] fills it, from the same
/// status's members; absent with no control gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceControl {
    pub code: ControlGateCode,
    pub dispatch: Vec<InstanceGate>,
    pub device: Vec<InstanceGate>,
}

impl InstanceControl {
    /// The member for `gates`; `None` with no control gate.
    pub fn from_gates(gates: &ControlGates) -> Option<Self> {
        Some(Self {
            code: gates.code()?,
            dispatch: gates.dispatch.clone(),
            device: gates.device.clone(),
        })
    }
}

impl RuntimeInstanceStatus {
    /// R-7a: this status with the instance's player hold (R-7b fills it).
    pub fn with_player_hold(
        mut self,
        player_hold: Option<PlayerHoldState>,
    ) -> RuntimeContractResult<Self> {
        self.player_hold = player_hold;
        self.validate()?;
        Ok(self)
    }

    pub const fn player_hold(&self) -> Option<&PlayerHoldState> {
        self.player_hold.as_ref()
    }

    /// R-7a: this status with the instance's last accepted player hold operation of this epoch
    /// (R-7b fills it).
    pub fn with_player_hold_operation(
        mut self,
        operation: Option<PlayerHoldOperation>,
    ) -> RuntimeContractResult<Self> {
        self.player_hold_operation = operation;
        self.validate()?;
        Ok(self)
    }

    pub const fn player_hold_operation(&self) -> Option<&PlayerHoldOperation> {
        self.player_hold_operation.as_ref()
    }

    /// The control gates as the status carries them. Readers call
    /// [`RuntimeControlPlaneStatus::instance_gates`] instead, which reads the members (O-7 4).
    pub const fn control(&self) -> Option<&InstanceControl> {
        self.control.as_ref()
    }

    /// The four inputs of this instance's control gates (O-3), the global pause given.
    fn control_inputs(&self, global_pause: bool) -> ControlInputs {
        ControlInputs {
            player_hold: self.player_hold.is_some(),
            instance_pause: self.pause.is_some(),
            global_pause,
            resource_fence: self.resource_fence.is_some(),
        }
    }

    /// The player hold members' shapes (part of [`RuntimeInstanceStatus::validate`]).
    pub(super) fn validate_player_hold(&self) -> RuntimeContractResult<()> {
        if let Some(hold) = &self.player_hold {
            hold.validate()?;
        }
        if let Some(operation) = &self.player_hold_operation {
            operation.validate()?;
        }
        Ok(())
    }
}

impl RuntimeControlPlaneStatus {
    /// One instance by its alias.
    pub fn instance(&self, alias: &str) -> Option<&RuntimeInstanceStatus> {
        self.instances
            .iter()
            .find(|instance| instance.instance_alias == alias)
    }

    /// R-7a §2.6 O-7 2: fills every instance's `control` from the same status's four members
    /// (the instance's player hold, its pause, the global pause, its fence), absent with no
    /// gate, then validates. The last step of building a status: call it after `new`,
    /// `with_source`, `with_scheduling_pause` and each instance's members. Calling it again
    /// changes nothing.
    ///
    /// How it remembers (the coordinator's decision 3 on §8.4): a flag that is not part of the
    /// wire and not part of equality (`control_derived`); `with_scheduling_pause`, the one method
    /// that changes a member afterwards, derives again when the flag is set or any instance
    /// carries a `control` (a status read in), so the order of the calls never poisons.
    pub fn with_derived_control(mut self) -> RuntimeContractResult<Self> {
        self.derive_control();
        self.control_derived = true;
        self.validate()?;
        Ok(self)
    }

    /// Fills `control` again where `with_derived_control` was called or a `control` is present.
    pub(super) fn derive_control_again(&mut self) {
        if self.control_derived
            || self
                .instances
                .iter()
                .any(|instance| instance.control.is_some())
        {
            self.derive_control();
        }
    }

    fn derive_control(&mut self) {
        let global_pause = self.scheduling_pause.is_some();
        for instance in &mut self.instances {
            instance.control = InstanceControl::from_gates(&ControlGates::evaluate(
                instance.control_inputs(global_pause),
            ));
        }
    }

    /// R-7a §2.6 O-7 5: a `control` that is present equals what the members derive and holds a
    /// dispatch gate (`invalid_runtime_instance_control`); an absent one is not checked (an R-7a
    /// or older Runtime fills none).
    pub(super) fn validate_control(&self) -> RuntimeContractResult<()> {
        let global_pause = self.scheduling_pause.is_some();
        for instance in &self.instances {
            let Some(control) = &instance.control else {
                continue;
            };
            let derived = InstanceControl::from_gates(&ControlGates::evaluate(
                instance.control_inputs(global_pause),
            ));
            if control.dispatch.is_empty() || derived.as_ref() != Some(control) {
                return Err(RuntimeContractError::new(
                    RuntimeContractCode::InvalidRuntimeInstanceControl,
                ));
            }
        }
        Ok(())
    }

    /// R-7a §2.1: the control gates of instance `alias`, always from the status's members
    /// through [`ControlGates::evaluate`], never from `control` itself (an R-7a Runtime fills
    /// none). `None` when the status does not list the instance.
    pub fn instance_gates(&self, alias: &str) -> Option<ControlGates> {
        let instance = self.instance(alias)?;
        Some(ControlGates::evaluate(
            instance.control_inputs(self.scheduling_pause.is_some()),
        ))
    }

    /// R-7a §2.1: "the hold ended, automatic scheduling stays paused": the instance's record of
    /// this epoch is a completed release whose `next` is a pause, and a pause it saw at the end
    /// is still the same one (its revision unchanged).
    pub fn hold_ended_with_pause(&self, alias: &str) -> bool {
        let Some(instance) = self.instance(alias) else {
            return false;
        };
        let Some(operation) = instance.player_hold_operation.as_ref() else {
            return false;
        };
        let Some(result) = operation
            .result
            .as_ref()
            .filter(|_| {
                operation.kind == PlayerHoldOperationKind::Release
                    && operation.state == PlayerHoldOperationState::Completed
            })
            .filter(|result| {
                matches!(
                    result.next,
                    Some(PlayerHoldNext::InstancePaused | PlayerHoldNext::SchedulingPaused)
                )
            })
        else {
            return false;
        };
        let Some(pauses) = result.pauses_at_end else {
            return false;
        };
        let instance_same = pauses
            .instance_revision
            .is_some_and(|revision| instance.pause.as_ref().map(|p| p.revision) == Some(revision));
        let global_same = pauses.global_revision.is_some_and(|revision| {
            self.scheduling_pause.as_ref().map(|p| p.revision) == Some(revision)
        });
        instance_same || global_same
    }
}

/// One player hold request a client sent, as it settles it (R-7a §3.3.2): every member public
/// and serializable, so a console stores the attempt before it submits and settles it after it
/// reopened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldAttempt {
    pub kind: PlayerHoldOperationKind,
    /// The request's `request_id` (R).
    pub request_id: RequestId,
    /// The owner epoch it was sent in (E0).
    pub sent_in_owner_epoch: OwnerEpoch,
    /// The hold it is about (H): an acquire's own `request_id`, a release's `expected.hold_id`.
    pub expected_hold_id: RequestId,
    pub trigger: PlayerHoldSettlementTrigger,
}

/// Who ended, or is ending, a hold (R-7a §3.3.3 A6, A12, R7): a release record's source and
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldEndedBy {
    pub source: PlayerHoldSource,
    pub state: PlayerHoldOperationState,
}

/// What one instance shows now (R-7a §3.3.2): its hold, its record, its control gates by the
/// members and whether the hold ended under a pause still in effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldView {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<PlayerHoldState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<PlayerHoldOperation>,
    pub gates: ControlGates,
    pub hold_ended_with_pause: bool,
}

/// The decision of one row of the settlement table (R-7a §3.3.3). `settlement_outcome` (the
/// design's `outcome`, renamed because `outcome` is the envelope's name) when settled,
/// `observed` when unconfirmed; `final` says whether reading again can change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerHoldDecision {
    pub rule: PlayerHoldSettlementRule,
    pub verdict: PlayerHoldSettlementVerdict,
    #[serde(rename = "final")]
    pub is_final: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_outcome: Option<PlayerHoldSettlementOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<PlayerHoldUnconfirmedObserved>,
    /// A11, A12: the hold was restored from the persisted row at a start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restored: bool,
    /// Absent only for P1 without an attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<PlayerHoldAttempt>,
    /// E1, the owner epoch of the status read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_owner_epoch: Option<OwnerEpoch>,
    /// The instance's hold now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<PlayerHoldState>,
    /// The instance's record now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<PlayerHoldOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_by: Option<PlayerHoldEndedBy>,
}

/// What [`settle_player_hold`] gives (R-7a §3.3.2): the instance now, without an attempt, or the
/// decision of the settlement table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerHoldSettlement {
    Current(Box<PlayerHoldView>),
    Decided(Box<PlayerHoldDecision>),
}

impl PlayerHoldSettlement {
    /// The decision, when there is one.
    pub fn decision(&self) -> Option<&PlayerHoldDecision> {
        match self {
            Self::Current(_) => None,
            Self::Decided(decision) => Some(decision),
        }
    }
}

/// Settles a player hold request whose receipt never arrived, or describes the instance now
/// (R-7a §3.3.2, §3.3.3). With `attempt` `None` it describes the instance (`Current`), or answers
/// P1 / P2. With an attempt it walks the table in its written order: P rows first, then by the
/// owner epoch, top to bottom, the first match deciding. It reads once and never resends.
///
/// Call it with `Some(attempt)` only inside the method, after `submit` returned a failure, or
/// once the process that submitted is gone; while a submit in the same process has not
/// returned, call it with `None` (it would settle a request still on its way as not
/// registered).
pub fn settle_player_hold(
    attempt: Option<&PlayerHoldAttempt>,
    alias: &str,
    status: Option<&RuntimeControlPlaneStatus>,
) -> PlayerHoldSettlement {
    use PlayerHoldSettlementOutcome as Outcome;
    use PlayerHoldSettlementRule as Rule;
    use PlayerHoldUnconfirmedObserved as Observed;
    let Some(status) = status else {
        return decided(
            Rule::P1,
            Verdict::Unconfirmed(Observed::StatusUnreadable),
            attempt,
            None,
            None,
        );
    };
    let Some(instance) = status.instance(alias) else {
        let mut decision = decision(
            Rule::P2,
            Verdict::Unconfirmed(Observed::InstanceNotListed),
            attempt,
        );
        decision.status_owner_epoch = Some(status.owner_epoch());
        return PlayerHoldSettlement::Decided(Box::new(decision));
    };
    let hold = instance.player_hold.as_ref();
    let record = instance.player_hold_operation.as_ref();
    let Some(attempt) = attempt else {
        return PlayerHoldSettlement::Current(Box::new(PlayerHoldView {
            alias: alias.to_owned(),
            hold: hold.cloned(),
            operation: record.cloned(),
            gates: status.instance_gates(alias).unwrap_or_default(),
            hold_ended_with_pause: status.hold_ended_with_pause(alias),
        }));
    };

    let same_epoch = status.owner_epoch() == attempt.sent_in_owner_epoch;
    let own = record.filter(|record| record.request_id == attempt.request_id);
    let dropped = attempt.trigger == PlayerHoldSettlementTrigger::Dropped;
    let revision_four = status.instances().iter().any(|instance| {
        instance.player_hold.is_some()
            || instance.player_hold_operation.is_some()
            || instance.control.is_some()
    });
    let held_by = |hold_id: RequestId| hold.filter(|hold| hold.hold_id == hold_id);
    let release_of = |hold_id: RequestId| {
        record.filter(|record| {
            record.kind == PlayerHoldOperationKind::Release && record.hold_id == hold_id
        })
    };
    let own_state = own.map(|record| record.state);
    let r = attempt.request_id;
    let h = attempt.expected_hold_id;

    let (rule, verdict, restored, ended_by) = match (attempt.kind, same_epoch) {
        (PlayerHoldOperationKind::Acquire, true) => match own_state {
            Some(PlayerHoldOperationState::Running) => (
                Rule::A1,
                Verdict::Unconfirmed(Observed::InProgress),
                false,
                None,
            ),
            Some(PlayerHoldOperationState::Completed) => {
                (Rule::A2, Verdict::Settled(Outcome::Completed), false, None)
            }
            Some(PlayerHoldOperationState::Failed) => {
                (Rule::A3, Verdict::Settled(Outcome::Failed), false, None)
            }
            None => match (held_by(r), release_of(r)) {
                (Some(hold), _)
                    if matches!(
                        hold.phase,
                        PlayerHoldPhase::Held | PlayerHoldPhase::Releasing
                    ) =>
                {
                    (Rule::A4, Verdict::Settled(Outcome::Completed), false, None)
                }
                (Some(_), _) => (
                    Rule::A5,
                    Verdict::Unconfirmed(Observed::Inconsistent),
                    false,
                    None,
                ),
                (None, Some(release)) => (
                    Rule::A6,
                    Verdict::Settled(Outcome::Completed),
                    false,
                    Some(ended(release)),
                ),
                (None, None) if record.is_none() && dropped && !revision_four => {
                    (Rule::A7, Verdict::Unsupported, false, None)
                }
                (None, None) if record.is_none() && dropped => (
                    Rule::A8,
                    Verdict::Settled(Outcome::NotPerformed),
                    false,
                    None,
                ),
                (None, None) if record.is_none() => (
                    Rule::A9,
                    Verdict::Unconfirmed(Observed::NotRegistered),
                    false,
                    None,
                ),
                (None, None) => (
                    Rule::A10,
                    Verdict::Unconfirmed(Observed::Superseded),
                    false,
                    None,
                ),
            },
        },
        (PlayerHoldOperationKind::Acquire, false) => match (held_by(r), release_of(r)) {
            (Some(_), _) => (Rule::A11, Verdict::Settled(Outcome::Completed), true, None),
            (None, Some(release)) => (
                Rule::A12,
                Verdict::Settled(Outcome::Completed),
                true,
                Some(ended(release)),
            ),
            (None, None) => (
                Rule::A13,
                Verdict::Unconfirmed(Observed::EpochChanged),
                false,
                None,
            ),
        },
        (PlayerHoldOperationKind::Release, true) => match own_state {
            Some(PlayerHoldOperationState::Running) => (
                Rule::R1,
                Verdict::Unconfirmed(Observed::InProgress),
                false,
                None,
            ),
            Some(PlayerHoldOperationState::Completed) => {
                (Rule::R2, Verdict::Settled(Outcome::Completed), false, None)
            }
            Some(PlayerHoldOperationState::Failed) => {
                (Rule::R3, Verdict::Settled(Outcome::Failed), false, None)
            }
            None => match (held_by(h), release_of(h)) {
                (Some(hold), _) if hold.phase == PlayerHoldPhase::Releasing => (
                    Rule::R4,
                    Verdict::Unconfirmed(Observed::InProgressOther),
                    false,
                    None,
                ),
                (Some(_), _) if dropped => (
                    Rule::R5,
                    Verdict::Settled(Outcome::NotPerformed),
                    false,
                    None,
                ),
                (Some(_), _) => (
                    Rule::R6,
                    Verdict::Unconfirmed(Observed::NotRegistered),
                    false,
                    None,
                ),
                (None, Some(release)) => (
                    Rule::R7,
                    Verdict::Settled(Outcome::EndedByOther),
                    false,
                    Some(ended(release)),
                ),
                (None, None) if record.is_none() && dropped && !revision_four => {
                    (Rule::R8, Verdict::Unsupported, false, None)
                }
                (None, None) => (Rule::R9, Verdict::Settled(Outcome::HoldEnded), false, None),
            },
        },
        (PlayerHoldOperationKind::Release, false) => match held_by(h) {
            Some(_) => (
                Rule::R10,
                Verdict::Settled(Outcome::NotPerformed),
                false,
                None,
            ),
            None => (Rule::R11, Verdict::Settled(Outcome::HoldEnded), false, None),
        },
    };
    let mut decision = decision(rule, verdict, Some(attempt));
    decision.restored = restored;
    decision.ended_by = ended_by;
    decision.status_owner_epoch = Some(status.owner_epoch());
    decision.hold = hold.cloned();
    decision.operation = record.cloned();
    PlayerHoldSettlement::Decided(Box::new(decision))
}

/// A verdict with its one value.
#[derive(Clone, Copy)]
enum Verdict {
    Settled(PlayerHoldSettlementOutcome),
    Unconfirmed(PlayerHoldUnconfirmedObserved),
    Unsupported,
}

fn decision(
    rule: PlayerHoldSettlementRule,
    verdict: Verdict,
    attempt: Option<&PlayerHoldAttempt>,
) -> PlayerHoldDecision {
    let (verdict, is_final, settlement_outcome, observed) = match verdict {
        Verdict::Settled(outcome) => (
            PlayerHoldSettlementVerdict::Settled,
            true,
            Some(outcome),
            None,
        ),
        Verdict::Unconfirmed(observed) => (
            PlayerHoldSettlementVerdict::Unconfirmed,
            observed.is_final(),
            None,
            Some(observed),
        ),
        Verdict::Unsupported => (PlayerHoldSettlementVerdict::Unsupported, true, None, None),
    };
    PlayerHoldDecision {
        rule,
        verdict,
        is_final,
        settlement_outcome,
        observed,
        restored: false,
        attempt: attempt.cloned(),
        status_owner_epoch: None,
        hold: None,
        operation: None,
        ended_by: None,
    }
}

fn decided(
    rule: PlayerHoldSettlementRule,
    verdict: Verdict,
    attempt: Option<&PlayerHoldAttempt>,
    hold: Option<&PlayerHoldState>,
    operation: Option<&PlayerHoldOperation>,
) -> PlayerHoldSettlement {
    let mut decision = decision(rule, verdict, attempt);
    decision.hold = hold.cloned();
    decision.operation = operation.cloned();
    PlayerHoldSettlement::Decided(Box::new(decision))
}

fn ended(release: &PlayerHoldOperation) -> PlayerHoldEndedBy {
    PlayerHoldEndedBy {
        source: release.source,
        state: release.state,
    }
}

fn invalid_state() -> RuntimeContractError {
    RuntimeContractError::new(RuntimeContractCode::InvalidPlayerHoldState)
}

fn invalid_receipt() -> RuntimeContractError {
    RuntimeContractError::new(RuntimeContractCode::InvalidPlayerHoldReceipt)
}

#[cfg(test)]
#[path = "player_hold_tests.rs"]
mod tests;
