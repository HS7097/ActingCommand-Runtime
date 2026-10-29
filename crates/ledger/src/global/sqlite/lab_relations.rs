// SPDX-License-Identifier: AGPL-3.0-only

//! #422 RB-1 (0.12.27; production v0.12.26 after the machine restart of 2026-10-29 05:23,
//! investigation `rd-1029/inv-reboot/rb.md`): a reader-pool page no longer reads the Lab
//! relations of the whole history.
//!
//! Before, every page that needed its Lab relations (the Lab view before its selection; any other
//! page at its first own row that names a request, a correlation or a run, IDLE-OV M1) read them
//! from sequence 1 (`read_lab_relations`): every Lab anchor, then the run of every link row that
//! names one of their requests or correlations, mostly random reads. With a cold page cache that
//! read outlasted the page's `ledger_observer.read_budget_ms`, so the page stopped before that row
//! and the next page was refused; the run listing then always failed. Its work also grew with the
//! Lab history, cold or warm.
//!
//! - **Built once, in the background.** When the reader pool starts, a thread of its own builds a
//!   [`LabRelationIndex`] through the head the writer published then (its anchor), on a
//!   read-only connection of its own, outside every page's budget, within the open's deadline
//!   (`ledger_observer.open_timeout_ms`, in whole seconds as the open counts it). The index keeps
//!   each related id with the sequence at which it became Lab-related (`since`).
//! - **Pages read the records after it.** A page at a snapshot at or before the index's end takes
//!   the ids whose `since` is at or before its snapshot: exactly what `read_lab_relations` reads at
//!   that snapshot, also for a listing pinned to an older snapshot. A page past the index's end
//!   reads only the records after it ([`LabRelationIndex::extend_through`]), in spans that start
//!   at one sequence and double up to `ledger_observer.history_scan_rows`, under the page's own
//!   budget, as a page's history statements do; the spans it completed extend the shared index
//!   for every later page (never backwards), also when the budget cut the page. A cut there ends
//!   the page as a cut of the relations always did (IDLE-OV M1).
//! - **Before the build ends, or when it failed,** pages read the whole history, as before. A
//!   failed build is said once as a warning of the reader pool: its deadline
//!   `ledger_read_budget_exceeded` of `build_lab_relations`, any other failure with its own code
//!   at `build_lab_relations`; it is never fatal, and is not tried again before the next start.
//!
//! No configuration key, outcome code, ledger format or table is added: the index is memory only,
//! and grows with the Lab-related ids, not with the history.

use super::{
    LAB_ANCHOR_SOURCE, LAB_ANCHOR_TYPE, LabRelations, SqlRow, decode, encode, failure,
    id_from_text, read_lab_relations, read_rows_bound, sql_error, views,
};
use crate::codes::{LedgerCode, LedgerLocation};
use crate::global::read_work::ReadWork;
use crate::global::{GlobalLedgerError, GlobalLedgerResult};
use actingcommand_contract::outcome::IoKind;
use actingcommand_contract::{CorrelationId, RequestId, RunId};
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, TransactionBehavior};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// The statement of the requests or correlations (`{column}`) the Lab anchors after `?4` through
/// `?3` name, each with the first such anchor's sequence: the anchors of `read_lab_relations`
/// (by source `?1` or type `?2`, through the view indexes), within a range of sequences.
fn anchored_statement(column: &str) -> String {
    format!(
        "SELECT xl.{column},MIN(x.sequence) FROM ledger_events x JOIN ledger_links xl ON xl.sequence=x.sequence WHERE x.origin_source=?1 AND x.sequence>?4 AND x.sequence<=?3 AND xl.{column} IS NOT NULL GROUP BY xl.{column} UNION SELECT xl.{column},MIN(x.sequence) FROM ledger_events x JOIN ledger_links xl ON xl.sequence=x.sequence WHERE x.event_type=?2 AND x.sequence>?4 AND x.sequence<=?3 AND xl.{column} IS NOT NULL GROUP BY xl.{column}"
    )
}

/// The statement of the runs of the link rows through `?1` that name one of the requests `?2` or
/// one of the correlations `?3` (through the link indexes, as `read_lab_relations`'s run
/// statement), each run with the request and correlation of those rows and the first row's
/// sequence.
const RUNS_NAMING: &str = "SELECT k.run_id,k.request_id,k.correlation_id,MIN(k.sequence) FROM ledger_links k WHERE k.request_id IN (SELECT value FROM json_each(?2)) AND k.run_id IS NOT NULL AND k.sequence<=?1 GROUP BY k.run_id,k.request_id,k.correlation_id UNION SELECT k.run_id,k.request_id,k.correlation_id,MIN(k.sequence) FROM ledger_links k WHERE k.correlation_id IN (SELECT value FROM json_each(?3)) AND k.run_id IS NOT NULL AND k.sequence<=?1 GROUP BY k.run_id,k.request_id,k.correlation_id";

/// The statement of the runs of the link rows after `?1` through `?2` (the link table's own key)
/// that name a run and a request or a correlation, each with that request and correlation and
/// the first row's sequence.
const RUNS_BETWEEN: &str = "SELECT k.run_id,k.request_id,k.correlation_id,MIN(k.sequence) FROM ledger_links k WHERE k.sequence>?1 AND k.sequence<=?2 AND k.run_id IS NOT NULL AND (k.request_id IS NOT NULL OR k.correlation_id IS NOT NULL) GROUP BY k.run_id,k.request_id,k.correlation_id";

/// #422 RB-1: the Lab relations of the committed records through `through`, each id with the
/// sequence at which it became Lab-related (`since`): a request or a correlation at the first Lab
/// anchor that names it; a run at the first record that names it together with a request or a
/// correlation related by then (the later of that record and that relation). The relations at a
/// snapshot at or before `through` are the ids whose `since` is at or before it, which are
/// `read_lab_relations`'s at that snapshot: a record at or before the snapshot names the run, and
/// a request or correlation it names is anchored at or before the snapshot.
#[derive(Clone, Debug, Default)]
pub(in crate::global) struct LabRelationIndex {
    through: u64,
    requests: BTreeMap<RequestId, u64>,
    correlations: BTreeMap<CorrelationId, u64>,
    runs: BTreeMap<RunId, u64>,
}

impl LabRelationIndex {
    /// The Lab relations at `snapshot`, at or before the index's end (`through`).
    pub(in crate::global) fn at(&self, snapshot: u64) -> LabRelations {
        debug_assert!(snapshot <= self.through, "{snapshot} > {}", self.through);
        fn since_by<K: Copy + Ord>(ids: &BTreeMap<K, u64>, snapshot: u64) -> BTreeSet<K> {
            ids.iter()
                .filter(|(_, since)| **since <= snapshot)
                .map(|(id, _)| *id)
                .collect()
        }
        LabRelations {
            requests: since_by(&self.requests, snapshot),
            correlations: since_by(&self.correlations, snapshot),
            runs: since_by(&self.runs, snapshot),
        }
    }

    /// The index through `end` (after its end, `through`): it reads the records after its end
    /// only.
    /// 1. The Lab anchors in the range give the requests and correlations they name first.
    /// 2. Those new ones give the runs of their link rows through `end`, older ones included (a
    ///    new anchor may name an old request).
    /// 3. The link rows in the range give their runs when they name a request or a correlation
    ///    related by then (skipped while the index holds none: the build, whose new ones are
    ///    every one).
    ///
    /// The index changes only when all of it was read: a statement that fails or is interrupted
    /// leaves it as it was.
    pub(in crate::global) fn extend_through(
        &mut self,
        connection: &Connection,
        end: u64,
    ) -> GlobalLedgerResult<()> {
        if end <= self.through {
            return Ok(());
        }
        let after = self.through;
        let new_requests = anchored::<RequestId>(connection, "request_id", (after, end))?
            .into_iter()
            .filter(|(id, _)| !self.requests.contains_key(id))
            .collect::<BTreeMap<_, _>>();
        let new_correlations =
            anchored::<CorrelationId>(connection, "correlation_id", (after, end))?
                .into_iter()
                .filter(|(id, _)| !self.correlations.contains_key(id))
                .collect::<BTreeMap<_, _>>();
        let mut linked = Vec::new();
        if !new_requests.is_empty() || !new_correlations.is_empty() {
            linked.extend(run_rows(
                connection,
                RUNS_NAMING,
                &[
                    SqlValue::Integer(encode(end)),
                    key_list(new_requests.keys())?,
                    key_list(new_correlations.keys())?,
                ],
            )?);
        }
        if !self.requests.is_empty() || !self.correlations.is_empty() {
            linked.extend(run_rows(
                connection,
                RUNS_BETWEEN,
                &[
                    SqlValue::Integer(encode(after)),
                    SqlValue::Integer(encode(end)),
                ],
            )?);
        }
        let since_of = |request: Option<RequestId>, correlation: Option<CorrelationId>| {
            let request = request.and_then(|id| {
                self.requests
                    .get(&id)
                    .or_else(|| new_requests.get(&id))
                    .copied()
            });
            let correlation = correlation.and_then(|id| {
                self.correlations
                    .get(&id)
                    .or_else(|| new_correlations.get(&id))
                    .copied()
            });
            match (request, correlation) {
                (Some(request), Some(correlation)) => Some(request.min(correlation)),
                (request, correlation) => request.or(correlation),
            }
        };
        let runs = linked
            .into_iter()
            .filter_map(|(run, request, correlation, first)| {
                since_of(request, correlation).map(|since| (run, first.max(since)))
            })
            .collect::<Vec<_>>();
        self.requests.extend(new_requests);
        self.correlations.extend(new_correlations);
        for (run, since) in runs {
            self.runs
                .entry(run)
                .and_modify(|known| *known = (*known).min(since))
                .or_insert(since);
        }
        self.through = end;
        Ok(())
    }
}

/// The ids of `column` the Lab anchors after `range.0` through `range.1` name, each with its first
/// anchor's sequence.
fn anchored<T: serde::de::DeserializeOwned + Ord>(
    connection: &Connection,
    column: &str,
    (after, through): (u64, u64),
) -> GlobalLedgerResult<BTreeMap<T, u64>> {
    let rows = read_rows_bound(
        connection,
        &anchored_statement(column),
        &[
            SqlValue::Text(views::key(LAB_ANCHOR_SOURCE)?),
            SqlValue::Text(views::key(LAB_ANCHOR_TYPE)?),
            SqlValue::Integer(encode(through)),
            SqlValue::Integer(encode(after)),
        ],
        None,
        &mut 0,
        false,
    )?;
    let mut ids = BTreeMap::new();
    for row in rows {
        let (id, first) = id_and_sequence(row)?;
        ids.entry(id_from_text::<T>(&id)?)
            .and_modify(|known: &mut u64| *known = (*known).min(first))
            .or_insert(first);
    }
    Ok(ids)
}

/// One run row: the run, the request and the correlation its link rows name, and the first such
/// row's sequence.
type RunRow = (RunId, Option<RequestId>, Option<CorrelationId>, u64);

fn run_rows(
    connection: &Connection,
    statement: &str,
    parameters: &[SqlValue],
) -> GlobalLedgerResult<Vec<RunRow>> {
    let optional = |value: SqlValue| -> GlobalLedgerResult<Option<String>> {
        match value {
            SqlValue::Null => Ok(None),
            SqlValue::Text(text) => Ok(Some(text)),
            _ => Err(invalid_row()),
        }
    };
    read_rows_bound(connection, statement, parameters, None, &mut 0, false)?
        .into_iter()
        .map(|row| -> GlobalLedgerResult<RunRow> {
            let [run, request, correlation, first] =
                <[SqlValue; 4]>::try_from(row).map_err(|_| invalid_row())?;
            let (SqlValue::Text(run), SqlValue::Integer(first)) = (run, first) else {
                return Err(invalid_row());
            };
            Ok((
                id_from_text::<RunId>(&run)?,
                optional(request)?
                    .map(|text| id_from_text::<RequestId>(&text))
                    .transpose()?,
                optional(correlation)?
                    .map(|text| id_from_text::<CorrelationId>(&text))
                    .transpose()?,
                decode(first),
            ))
        })
        .collect()
}

/// The id's text and the sequence of a row of an id and a sequence.
fn id_and_sequence(row: SqlRow) -> GlobalLedgerResult<(String, u64)> {
    match <[SqlValue; 2]>::try_from(row) {
        Ok([SqlValue::Text(id), SqlValue::Integer(first)]) => Ok((id, decode(first))),
        _ => Err(invalid_row()),
    }
}

fn invalid_row() -> GlobalLedgerError {
    failure(
        LedgerCode::InvalidEventLink.code(),
        LedgerLocation::ReadLedgerView.location(),
    )
}

/// The stored keys of `ids` as one JSON array for `json_each`.
fn key_list<'a, T: serde::Serialize + 'a>(
    ids: impl Iterator<Item = &'a T>,
) -> GlobalLedgerResult<SqlValue> {
    let keys = ids
        .map(views::key)
        .collect::<GlobalLedgerResult<Vec<_>>>()?;
    serde_json::to_string(&keys)
        .map(SqlValue::Text)
        .map_err(|error| {
            GlobalLedgerError::json(
                LedgerCode::EventSerializationFailed.code(),
                LedgerLocation::EncodeLedgerQuery.location(),
                &error,
            )
        })
}

/// #422 RB-1: the Lab relations of one reader pool: its build's state, shared by its readers.
pub(in crate::global) struct LabRelationCell {
    /// The most sequences one span of a page's extension reads
    /// (`ledger_observer.history_scan_rows`).
    scan_rows: u64,
    state: Mutex<LabIndexState>,
    /// Tests: told when the build ends.
    settled: Condvar,
}

enum LabIndexState {
    /// The build runs: pages read the whole history's relations.
    Building,
    /// The index, as far as the pages have extended it.
    Built(Arc<LabRelationIndex>),
    /// No index (the build failed, or none runs): pages read the whole history's relations.
    Unbuilt,
}

impl LabRelationCell {
    /// The cell of a pool whose build starts now (`building`) or that builds none.
    pub(in crate::global) fn new(scan_rows: usize, building: bool) -> Self {
        Self {
            scan_rows: u64::try_from(scan_rows).unwrap_or(u64::MAX).max(1),
            state: Mutex::new(if building {
                LabIndexState::Building
            } else {
                LabIndexState::Unbuilt
            }),
            settled: Condvar::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, LabIndexState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The index, once built.
    fn index(&self) -> Option<Arc<LabRelationIndex>> {
        match &*self.state() {
            LabIndexState::Built(index) => Some(Arc::clone(index)),
            LabIndexState::Building | LabIndexState::Unbuilt => None,
        }
    }

    /// The build ended: with its index, or (`None`) without one.
    pub(in crate::global) fn settle(&self, built: Option<LabRelationIndex>) {
        *self.state() = match built {
            Some(index) => LabIndexState::Built(Arc::new(index)),
            None => LabIndexState::Unbuilt,
        };
        self.settled.notify_all();
    }

    /// Keeps `index` when it reaches further than the index kept: the index only moves forward.
    fn offer(&self, index: &Arc<LabRelationIndex>) {
        let mut state = self.state();
        if let LabIndexState::Built(kept) = &*state
            && kept.through < index.through
        {
            *state = LabIndexState::Built(Arc::clone(index));
        }
    }

    /// `base` extended through `to`, in spans of one sequence, then each twice the one before up
    /// to `scan_rows`. The spans read are kept for every page ([`Self::offer`]), also when a later
    /// span failed, whose failure is then the answer.
    fn extend(
        &self,
        connection: &Connection,
        base: &Arc<LabRelationIndex>,
        to: u64,
    ) -> GlobalLedgerResult<Arc<LabRelationIndex>> {
        let mut index = LabRelationIndex::clone(base);
        let mut span = 1_u64;
        let mut read = Ok(());
        while index.through < to {
            let end = index.through.saturating_add(span).min(to);
            read = index.extend_through(connection, end);
            if read.is_err() {
                break;
            }
            span = span.saturating_mul(2).min(self.scan_rows);
        }
        let index = Arc::new(index);
        if index.through > base.through {
            self.offer(&index);
        }
        read.map(|()| index)
    }

    /// Tests: waits at most `wait` for the build to end; `Some(Some(through))` once it built its
    /// index (the index's end now), `Some(None)` when it ended without one or none runs, `None`
    /// while it still runs.
    #[cfg(test)]
    pub(in crate::global) fn settled_for_test(
        &self,
        wait: std::time::Duration,
    ) -> Option<Option<u64>> {
        let (state, _) = self
            .settled
            .wait_timeout_while(self.state(), wait, |state| {
                matches!(state, LabIndexState::Building)
            })
            .unwrap_or_else(PoisonError::into_inner);
        match &*state {
            LabIndexState::Building => None,
            LabIndexState::Built(index) => Some(Some(index.through)),
            LabIndexState::Unbuilt => Some(None),
        }
    }
}

/// #422 RB-1: the Lab relations through `snapshot` of a page on `connection`, whose statements
/// run under the page's budget: from `cell`'s index once it is built (extended through the
/// snapshot when it ends before it), otherwise from the whole history (`read_lab_relations`), as
/// before the index. A writer's page has no cell.
pub(in crate::global) fn relations_at(
    connection: &Connection,
    snapshot: u64,
    cell: Option<&LabRelationCell>,
) -> GlobalLedgerResult<LabRelations> {
    let Some((cell, index)) = cell.and_then(|cell| cell.index().map(|index| (cell, index))) else {
        return read_lab_relations(connection, snapshot, &LabRelations::default());
    };
    if snapshot <= index.through {
        return Ok(index.at(snapshot));
    }
    Ok(cell.extend(connection, &index, snapshot)?.at(snapshot))
}

/// #422 RB-1: builds the index through `through` (the head the writer published when the pool
/// started) on `connection`, a read-only connection of the build's own, in one read transaction,
/// before `deadline` (the open's deadline taken afresh). Its statements run under a progress
/// handler that looks every `check_vm_steps` SQLite steps and interrupts the statement once the
/// deadline has passed or `stopping` says the pool closes.
///
/// A deadline that passes is `ledger_read_budget_exceeded` of `build_lab_relations`; a close
/// that stops it is the interrupted statement's SQL failure, which the pool does not say.
pub(in crate::global) fn build(
    connection: &mut Connection,
    through: u64,
    deadline: Instant,
    check_vm_steps: u32,
    stopping: impl Fn() -> bool + Send + 'static,
) -> GlobalLedgerResult<LabRelationIndex> {
    let operation = LedgerLocation::BuildLabRelations.location();
    let exceeded = || {
        ReadWork::exceeded(operation).with_detail(format!(
            "the Lab relations through {through} were not read before the open's deadline"
        ))
    };
    if Instant::now() >= deadline {
        return Err(exceeded());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|error| sql_error(error, LedgerLocation::BeginSqliteSnapshot.location()))?;
    transaction
        .progress_handler(
            i32::try_from(check_vm_steps).unwrap_or(i32::MAX),
            Some(move || stopping() || Instant::now() >= deadline),
        )
        .map_err(|error| sql_error(error, operation))?;
    let mut index = LabRelationIndex::default();
    let built = index.extend_through(&transaction, through);
    let removed = transaction
        .progress_handler(0, None::<fn() -> bool>)
        .map_err(|error| sql_error(error, operation));
    let closed = transaction
        .commit()
        .map_err(|error| sql_error(error, LedgerLocation::CloseSqliteSnapshot.location()));
    match built.and(removed) {
        Ok(()) => closed.map(|()| index),
        Err(error)
            if error.code == LedgerCode::LedgerSqliteFailed.as_str()
                && error.write_failure == Some(IoKind::Interrupted)
                && Instant::now() >= deadline =>
        {
            Err(exceeded().with_close_result(closed))
        }
        Err(error) => Err(error.with_close_result(closed)),
    }
}
