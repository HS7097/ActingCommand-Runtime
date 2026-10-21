// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A35-S5 (design `rd-1021/train-0129/a35-design.md` §5.1 S5; coordinator decision Q7 of
//! 2026-10-21): a recovery ladder that holds instead of climbing is visible. `Status` (and so
//! `actingctl status`) and the instance's `ac_overview` row carry `ladder_busy_hold {kind,
//! held_ms, limit_ms}` while the ladder's hold waits: what it waits for, how long it has held and
//! its limit. The values are the ladder's own, in memory: the ladder sets them when its hold
//! begins to wait and removes them when the wait ends, however it ends. Nothing is recorded or
//! persisted, and no configuration is read for it.

use super::*;
use crate::codes::HostLocation;
use actingcommand_contract::{InstanceLadderBusyHold, LadderBusyHoldKind};

/// One holding ladder as the status shows it.
#[derive(Clone, Copy)]
pub(super) struct LadderBusyHoldState {
    kind: LadderBusyHoldKind,
    /// When the climb's first hold of this kind began, on the Runtime clock (monotonic
    /// milliseconds, HOST-I2).
    since_ms: u64,
    /// The configured limit of the kind's holds in one climb.
    limit_ms: u64,
}

impl LadderBusyHoldState {
    pub(super) const fn new(kind: LadderBusyHoldKind, since_ms: u64, limit_ms: u64) -> Self {
        Self {
            kind,
            since_ms,
            limit_ms,
        }
    }

    /// The status member at `now_ms` on the Runtime clock.
    const fn status(self, now_ms: u64) -> InstanceLadderBusyHold {
        InstanceLadderBusyHold {
            kind: self.kind,
            held_ms: now_ms.saturating_sub(self.since_ms),
            limit_ms: self.limit_ms,
        }
    }
}

impl HostShared {
    /// A ladder's hold waits (`wait`) while the status shows it as `hold` for `instance_id`. The
    /// hold is removed when `wait` returns, whatever it returns; a failure to remove it is the
    /// answer when `wait` itself succeeded.
    pub(super) fn show_ladder_busy_hold<T>(
        &self,
        instance_id: InstanceId,
        hold: LadderBusyHoldState,
        wait: impl FnOnce() -> RuntimeHostResult<T>,
    ) -> RuntimeHostResult<T> {
        lock(&self.ladder_busy_holds, HostLocation::RunRecoveryLadder)?.insert(instance_id, hold);
        let waited = wait();
        let removed = lock(&self.ladder_busy_holds, HostLocation::RunRecoveryLadder)
            .map(|mut holds| holds.remove(&instance_id));
        let waited = waited?;
        removed?;
        Ok(waited)
    }

    /// The hold of every ladder whose hold waits, at `now_ms` on the Runtime clock, by instance
    /// (the status read).
    pub(super) fn ladder_busy_hold_statuses(
        &self,
        now_ms: u64,
    ) -> RuntimeHostResult<BTreeMap<InstanceId, InstanceLadderBusyHold>> {
        Ok(lock(
            &self.ladder_busy_holds,
            HostLocation::ProjectRuntimeControlPlaneStatus,
        )?
        .iter()
        .map(|(instance_id, hold)| (*instance_id, hold.status(now_ms)))
        .collect())
    }
}
