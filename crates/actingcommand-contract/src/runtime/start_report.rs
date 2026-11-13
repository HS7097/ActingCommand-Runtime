// SPDX-License-Identifier: AGPL-3.0-only

//! `RuntimeOperation::StartReport` (Workflow #421 INST-2; #421 deploy design §4): what the Runtime
//! knows about its own start, so the installer's post-start checks read no ledger and parse no
//! log: the ledger open's eight phases, their sum and the deadline, whether the open was slow and
//! read the owner history in full, whether this start loaded or seeded the tagged state, the
//! recovery's time, the pauses restored and dropped, the run-time instructions restored, the keys
//! not restored (dropped or superseded) and the keys restored over a default that changed, the
//! instances bound, the standard packs registered with their counts, and the dispatches admitted
//! since the start per instance. The read is answered from memory (a general read under
//! `runtime-client` 2, like `ScheduleExplanation`; only actingctl of the same build sends it); it
//! reads and records nothing in the ledger.
//!
//! #421 S2-IF (v0.12.21): an instruction kept over a default that changed
//! (`config_override_base_changed`) is restored: it counts in `instructions_restored` and is
//! named in `instructions_base_changed`, never in `instructions_dropped`, which names only the
//! keys not restored (`config_override_dropped`, `config_override_superseded`). The new list is
//! additive and absent when empty, so a report of an earlier build reads as one without it.
//!
//! Workflow #429 R-7a (register §1.233, `runtime-client` 4): `player_holds_restored`,
//! `player_holds_dropped` and `player_hold_not_durable` follow the same rule: absent when empty
//! (or `false`), so this build reads a report of an earlier build. R-7b fills them.

use super::{RuntimeContractError, RuntimeContractResult};
use crate::OwnerEpoch;
use crate::codes::RuntimeContractCode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The most rows one list of the answer holds (aliases, games, dropped or base-changed keys).
pub const MAX_START_REPORT_ROWS: usize = 256;

/// The phases of this start's ledger open, as its `ledger_opened` daemon line reports them
/// (Workflow #378 A6a; LN-8 the four of the writer open).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReportPhases {
    pub sql_read_ms: u64,
    pub verify_ms: u64,
    pub material_ms: u64,
    pub restore_ms: u64,
    pub meta_ms: u64,
    pub tail_ms: u64,
    pub index_ms: u64,
    pub import_ms: u64,
}

impl StartReportPhases {
    /// The sum of the eight phases.
    pub fn sum_ms(&self) -> u64 {
        [
            self.sql_read_ms,
            self.verify_ms,
            self.material_ms,
            self.restore_ms,
            self.meta_ms,
            self.tail_ms,
            self.index_ms,
            self.import_ms,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

/// This start's ledger open: its phases, their sum, the configured deadline, whether it took
/// longer than its warning line (`ledger_open_slow`), the reason the open read every owner
/// record of the history, when it did (#422 LN-7c O), and the code of the open's note about the
/// derived frame index, when it has one (#422 LN-7c R: the index is not complete yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReportOpen {
    pub phases: StartReportPhases,
    pub sum_ms: u64,
    pub deadline_ms: u64,
    pub slow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_read_full: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_note: Option<String>,
}

/// How this start restored the tagged state (Workflow #422 LN-4): loaded from its records, or
/// seeded once from the recoveries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartReportTaggedState {
    Load,
    Seed,
}

/// One standard-pack game root this Runtime registered, with its pack count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReportPacks {
    pub game: String,
    pub pack_count: u64,
}

/// The dispatches admitted on one instance since this Runtime started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartReportDispatches {
    pub instance_alias: String,
    pub count: u64,
}

/// The answer of `StartReport`. `tagged_state`, `recovery_ms` and `ready_at_unix_ms` are absent
/// until the start reached them; the lists are by name, each name once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeStartReport {
    pub owner_epoch: OwnerEpoch,
    pub started_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_at_unix_ms: Option<u64>,
    pub open: StartReportOpen,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagged_state: Option<StartReportTaggedState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_ms: Option<u64>,
    pub pauses_restored: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pauses_dropped: Vec<String>,
    /// The instructions restored, those kept over a changed default included.
    pub instructions_restored: u64,
    /// The keys not restored: dropped (`config_override_dropped`) or superseded by the file
    /// (`config_override_superseded`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions_dropped: Vec<String>,
    /// #421 S2-IF: the keys restored although the default they override changed
    /// (`config_override_base_changed`); informational.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions_base_changed: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances_bound: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub standard_packs: Vec<StartReportPacks>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispatches_since_start: Vec<StartReportDispatches>,
    /// Workflow #429 R-7a (register §1.233, R-7a §2.5; R-7b fills it): the aliases whose
    /// persisted player hold this start restored, `held`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub player_holds_restored: Vec<String>,
    /// Workflow #429 R-7a (R-7b fills it): the aliases of persisted player holds this start
    /// dropped because the alias is no longer registered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub player_holds_dropped: Vec<String>,
    /// Workflow #429 R-7a (R-7b fills it): the effective `ledger_observer.tag.workbench_patterns`
    /// does not match `host.player_hold`, so no hold can persist and every acquire is refused
    /// `player_hold_not_durable`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub player_hold_not_durable: bool,
}

impl RuntimeStartReport {
    /// The shape rules: the open's sum is its phases' sum; a ready time is not before the start;
    /// every list holds at most [`MAX_START_REPORT_ROWS`] names, each once (the aliases valid).
    pub fn validate(&self) -> RuntimeContractResult<()> {
        let invalid = || RuntimeContractError::new(RuntimeContractCode::InvalidStartReport);
        if self.open.sum_ms != self.open.phases.sum_ms()
            || self.started_at_unix_ms == 0
            || self
                .ready_at_unix_ms
                .is_some_and(|ready| ready < self.started_at_unix_ms)
        {
            return Err(invalid());
        }
        let lists_hold = names_once(self.pauses_dropped.iter().map(String::as_str), true)
            && names_once(self.instructions_dropped.iter().map(String::as_str), false)
            && names_once(
                self.instructions_base_changed.iter().map(String::as_str),
                false,
            )
            && names_once(self.instances_bound.iter().map(String::as_str), true)
            && names_once(
                self.standard_packs.iter().map(|row| row.game.as_str()),
                false,
            )
            && names_once(
                self.dispatches_since_start
                    .iter()
                    .map(|row| row.instance_alias.as_str()),
                true,
            )
            && names_once(self.player_holds_restored.iter().map(String::as_str), true)
            && names_once(self.player_holds_dropped.iter().map(String::as_str), true);
        if !lists_hold {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Whether `names` holds at most [`MAX_START_REPORT_ROWS`] names, none empty, each once, and
/// each a valid instance alias when `aliases` says so.
fn names_once<'a>(names: impl Iterator<Item = &'a str>, aliases: bool) -> bool {
    let mut seen = BTreeSet::new();
    for name in names {
        if name.is_empty()
            || !seen.insert(name)
            || (aliases && crate::validate_instance_alias(name).is_err())
        {
            return false;
        }
    }
    seen.len() <= MAX_START_REPORT_ROWS
}
