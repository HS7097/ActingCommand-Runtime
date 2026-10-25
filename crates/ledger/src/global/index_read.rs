// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #422 LN-4 (`contracts/ledger-observer.md` §11.5 steps 1 to 3, §11.6 X1 and X2): the
//! start's read of the index table. One writer command reads the rows of one role whose key starts
//! with one of the prefixes, point-reads the record each row names by its sequence, and checks
//! that the row names that record: the same event id, a key the record carries in that role, and
//! the record's hash as the row's `value_sha256`. A row that disagrees is
//! `ledger_tag_index_mismatch` (fatal): an invariant of persisted state is broken and would recur
//! at every start. The read volume is the rows plus as many point reads, whatever the ledger's
//! length.

use super::read_work::ReadWork;
use super::storage::{DurableStorage, EventStore};
use super::{GlobalLedger, GlobalLedgerError, GlobalLedgerResult, WriterCommand};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::EventQuery;

/// A role of `ledger_index` the start reads (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LedgerIndexRole {
    /// The tagged keys; each row's record is the key's current value.
    State,
    /// The committed intents without a result (X1); the key is the intent's event id.
    Open,
    /// `last_start`, `last_close`, `last_fatal` and `tagged_state_enabled` (X2, LN-4).
    Lifecycle,
}

impl LedgerIndexRole {
    /// The value of the table's `role` column.
    pub(super) const fn column(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Open => "open",
            Self::Lifecycle => "lifecycle",
        }
    }
}

/// One index row and the record it names, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerIndexRecord {
    pub key: String,
    pub event: PersistedEvent,
}

/// The checked rows of one read in (sequence, key) order, and the ledger's latest sequence in
/// the same writer command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerIndexRecords {
    pub through_sequence: u64,
    pub records: Vec<LedgerIndexRecord>,
}

/// One row as the store holds it, with the hash of the ledger row its sequence names.
pub(super) struct StoredIndexRow {
    pub(super) sequence: u64,
    pub(super) key: String,
    pub(super) event_id: String,
    pub(super) value_sha256: String,
    pub(super) record_sha256: Option<String>,
}

impl<B: DurableStorage> EventStore<B> {
    /// The checked records of the rows of `role` whose key starts with one of `prefixes`. A
    /// store that keeps no index answers `ledger_selection_query_unsupported`.
    pub(super) fn index_records(
        &self,
        role: LedgerIndexRole,
        prefixes: &[String],
        work: &ReadWork,
    ) -> GlobalLedgerResult<LedgerIndexRecords> {
        let operation = LedgerLocation::LedgerIndexRows.location();
        let rows = self.backend.index_rows(role, prefixes).unwrap_or_else(|| {
            Err(GlobalLedgerError::request(
                LedgerCode::SelectionQueryUnsupported.code(),
                operation,
            ))
        })?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let event = self
                .query_within(
                    &EventQuery {
                        from_sequence: Some(row.sequence),
                        to_sequence: Some(row.sequence),
                        ..EventQuery::default()
                    },
                    work,
                    operation,
                )?
                .into_iter()
                .next();
            match event {
                Some(event)
                    if super::index::id_text(event.event_id()).as_deref()
                        == Some(row.event_id.as_str())
                        && row.record_sha256.as_deref() == Some(row.value_sha256.as_str())
                        && super::index::row_names_record(role, &row.key, &event) =>
                {
                    records.push(LedgerIndexRecord {
                        key: row.key,
                        event,
                    });
                }
                _ => return Err(index_mismatch(role, &row)),
            }
        }
        records.sort_by(|left, right| {
            (left.event.sequence(), left.key.as_str())
                .cmp(&(right.event.sequence(), right.key.as_str()))
        });
        Ok(LedgerIndexRecords {
            through_sequence: self.latest_sequence(),
            records,
        })
    }
}

fn index_mismatch(role: LedgerIndexRole, row: &StoredIndexRow) -> GlobalLedgerError {
    GlobalLedgerError::fatal(
        LedgerCode::LedgerTagIndexMismatch.code(),
        LedgerLocation::LedgerIndexRows.location(),
    )
    .with_detail(format!(
        "role={} key={} sequence={} event_id={} value_sha256={} record_sha256={}",
        role.column(),
        row.key,
        row.sequence,
        row.event_id,
        row.value_sha256,
        row.record_sha256.as_deref().unwrap_or("-"),
    ))
}

impl GlobalLedger {
    /// Workflow #422 LN-4 (`contracts/ledger-observer.md` §11.5 steps 1 to 3): the records the
    /// index rows of `role` name whose key starts with one of `prefixes`, each checked against
    /// its row, in (sequence, key) order, with the latest sequence of the same writer command.
    /// The writer commits everything captured before it first, as for every read.
    pub fn index_records(
        &self,
        role: LedgerIndexRole,
        prefixes: &[&str],
    ) -> GlobalLedgerResult<LedgerIndexRecords> {
        let operation = LedgerLocation::LedgerIndexRows.location();
        let (response, receiver) = std::sync::mpsc::sync_channel(1);
        let sender = self.sender.as_ref().ok_or_else(|| {
            GlobalLedgerError::fatal(LedgerCode::WriterUnavailable.code(), operation)
        })?;
        super::send_command(
            sender,
            WriterCommand::IndexRecords {
                role,
                prefixes: prefixes.iter().map(|prefix| (*prefix).to_owned()).collect(),
                response,
            },
            operation,
        )?;
        super::receive_response(receiver, operation, self.reply_timeout)?
    }
}
