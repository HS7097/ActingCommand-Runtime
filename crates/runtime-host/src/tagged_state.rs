// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #422 LN-4 (LEAN-L model §3.2 step 5, §3.6; `contracts/ledger-observer.md` §11.5,
//! §11.6): whether a start loads the policy host's decision memory, the agent dispatcher and the
//! monitor registry from their tagged records, or seeds those records once from today's
//! recoveries. The seeding's marker is `runtime.lifecycle_observed` phase `tagged_state_enabled`;
//! the index names it in its `lifecycle` row `tagged_state_enabled`, so the start reads one row to
//! decide.

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use actingcommand_ledger::{GlobalLedger, GlobalLedgerError, LedgerIndexRecords, LedgerIndexRole};

/// The `lifecycle` key of the seeding's marker.
const TAGGED_STATE_ENABLED: &str = "tagged_state_enabled";

/// The `lifecycle` key of the latest start record (`runtime.started`, `runtime.takeover`).
const LAST_START: &str = "last_start";

/// How this start restores the tagged state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaggedStart {
    /// The marker is present, at `marker`: the records the index names are the state; no
    /// history is replayed for it. Every decision-memory change at or before the marker is in
    /// its value records (the seeding wrote them before the marker).
    Load { marker: u64 },
    /// No marker (a new state root, or the first start of the build that brings the load): the
    /// recoveries replay once, one value record per key is written, then the marker.
    Seed,
}

impl TaggedStart {
    pub(crate) fn read(ledger: &GlobalLedger) -> RuntimeHostResult<Self> {
        let marker = index_records(ledger, LedgerIndexRole::Lifecycle, &[TAGGED_STATE_ENABLED])?;
        Ok(match marker.records.first() {
            Some(record) => Self::Load {
                marker: record.event.sequence(),
            },
            None => Self::Seed,
        })
    }
}

/// Workflow #422 LN-4: whether the ledger holds a start record (`runtime.started` or
/// `runtime.takeover`, the `lifecycle` row `last_start`), read before this start writes its own.
/// Without one this is the root's first start: it has no earlier epoch to seed, so it writes no
/// marker, and the root's next start seeds what this epoch recorded.
pub(crate) fn earlier_start(ledger: &GlobalLedger) -> RuntimeHostResult<bool> {
    Ok(
        !index_records(ledger, LedgerIndexRole::Lifecycle, &[LAST_START])?
            .records
            .is_empty(),
    )
}

/// [`GlobalLedger::index_records`] with the host's error: the ledger's own code and operation,
/// relayed fatal (an index row that disagrees with its record is `ledger_tag_index_mismatch`).
pub(crate) fn index_records(
    ledger: &GlobalLedger,
    role: LedgerIndexRole,
    prefixes: &[&str],
) -> RuntimeHostResult<LedgerIndexRecords> {
    ledger
        .index_records(role, prefixes)
        .map_err(|error| index_read_error(&error))
}

fn index_read_error(error: &GlobalLedgerError) -> RuntimeHostError {
    let relayed = RuntimeHostError::relayed_fatal(
        error.code(),
        error.operation(),
        RuntimeErrorCode::LedgerFailure,
    );
    match error.detail() {
        Some(detail) => relayed.with_native_detail(detail.to_owned()),
        None => relayed,
    }
}
