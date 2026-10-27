// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #422 LN-2 (`contracts/ledger-observer.md` §11.5 steps 1–3): the tagged state a start
//! loads. It reads the `state` rows of the index table under the asked key prefixes and each
//! row's record by a point read, and checks that the row and its record agree: the record has the
//! row's event id, it carries the row's key, and its stored hash is the row's `value_sha256`. A
//! row that disagrees stops the read with the fatal `ledger_tag_index_mismatch`: the persisted
//! state breaks an invariant and would break it again at every start.

use super::history::HistorySelection;
use super::read_work::ReadWork;
use super::storage::{DurableStorage, EventStore};
use super::{GlobalLedgerError, GlobalLedgerResult};
use crate::PersistedEvent;
use crate::codes::{LedgerCode, LedgerLocation};
use actingcommand_contract::EventQuery;
use actingcommand_contract::outcome::Outcome;

/// One `state` row of the index table as stored (§11.1), with the stored hash of the record its
/// sequence names; `record_sha256` is `None` when no record has that sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StateIndexRow {
    pub(super) sequence: u64,
    pub(super) key: String,
    pub(super) event_id: String,
    pub(super) value_sha256: String,
    pub(super) record_sha256: Option<String>,
}

/// One tagged key and its latest record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerTaggedRecord {
    pub key: String,
    pub event: PersistedEvent,
}

/// The tagged keys under the asked prefixes with their latest records, sorted by (sequence,
/// key), as committed through `through_sequence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerTaggedState {
    pub through_sequence: u64,
    pub records: Vec<LedgerTaggedRecord>,
}

impl<B: DurableStorage> EventStore<B> {
    /// The tagged state under `prefixes`: index rows plus as many point reads of the store,
    /// whatever the ledger's length. Each row counts on `work`; a spent budget answers
    /// `ledger_read_budget_exceeded` (#411 A51). A store that keeps no index (the test-only
    /// segment store) cannot answer.
    pub(super) fn tagged_state(
        &self,
        prefixes: &[String],
        work: &ReadWork,
    ) -> GlobalLedgerResult<LedgerTaggedState> {
        let operation = LedgerLocation::LedgerIndexRows.location();
        let through_sequence = self.latest_sequence();
        let rows = self.backend.state_index_rows(prefixes).unwrap_or_else(|| {
            Err(GlobalLedgerError::fatal(
                LedgerCode::LedgerJointTransactionUnsupported.code(),
                operation,
            ))
        })?;
        let all = EventQuery::default();
        let selection = HistorySelection::of(&all);
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            if !work.visit() {
                return Err(ReadWork::exceeded(operation));
            }
            // LN-5 (LEAN-L §3.3): a point read of the store by the row's sequence, through the
            // sequence key, the record authenticated on its own; no other record is read. A
            // sequence past the head or without a record names no record.
            let read = match row.sequence.checked_sub(1) {
                Some(after) if row.sequence <= through_sequence => self.backend.history_page(
                    &selection,
                    (after, row.sequence),
                    1,
                    &self.retention,
                )?,
                _ => Vec::new(),
            };
            let Some(mut event) = read.into_iter().find(|event| agrees(&row, event)) else {
                return Err(mismatch(&row));
            };
            self.retention.annotate_event(&mut event);
            records.push(LedgerTaggedRecord {
                key: row.key,
                event,
            });
        }
        records.sort_by(|left, right| {
            left.event
                .sequence()
                .cmp(&right.event.sequence())
                .then_with(|| left.key.cmp(&right.key))
        });
        Ok(LedgerTaggedState {
            through_sequence,
            records,
        })
    }
}

/// The row names its record: same sequence and event id, the record carries the row's key, and
/// the record's stored hash is the row's value hash.
fn agrees(row: &StateIndexRow, event: &PersistedEvent) -> bool {
    event.sequence() == row.sequence
        && super::index::id_text(event.event_id()).as_deref() == Some(row.event_id.as_str())
        && super::index::names_key(event, &row.key)
        && row.record_sha256.as_deref() == Some(row.value_sha256.as_str())
}

/// `ledger_tag_index_mismatch` of one row (§16): `fact_key` is the row's key as a token, the
/// hashes are the index's (`expected_sha256`) and the record's (`actual_sha256`). A value that
/// would break its key's rule is left out; the raw row is in the detail.
fn mismatch(row: &StateIndexRow) -> GlobalLedgerError {
    let mut outcome = Outcome::new(LedgerCode::LedgerTagIndexMismatch)
        .operation(LedgerLocation::LedgerIndexRows)
        .fact_key(key_token(&row.key));
    if row.event_id.len() <= 256 && !row.event_id.chars().any(char::is_control) {
        outcome = outcome.event_id(row.event_id.clone());
    }
    if is_hash(&row.value_sha256) {
        outcome = outcome.expected_sha256(row.value_sha256.clone());
    }
    if let Some(actual) = row.record_sha256.as_deref().filter(|hash| is_hash(hash)) {
        outcome = outcome.actual_sha256(actual.to_owned());
    }
    let mut error = GlobalLedgerError::fatal(
        LedgerCode::LedgerTagIndexMismatch.code(),
        LedgerLocation::LedgerIndexRows.location(),
    )
    .with_detail(format!(
        "sequence={} key={:?} event_id={:?} value_sha256={:?} record_sha256={:?}",
        row.sequence, row.key, row.event_id, row.value_sha256, row.record_sha256
    ));
    error.values = Some(Box::new(outcome));
    error
}

/// The row key as a token of the key table (`[a-z0-9_.:-]`, 1 to 256 bytes): `/` becomes `:` and
/// any other byte outside the set `_`.
fn key_token(key: &str) -> String {
    let token = key
        .bytes()
        .take(256)
        .map(|byte| match byte {
            b'/' => ':',
            b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b':' | b'-' => char::from(byte),
            _ => '_',
        })
        .collect::<String>();
    if token.is_empty() {
        "_".to_owned()
    } else {
        token
    }
}

fn is_hash(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}
