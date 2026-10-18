// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 L-3 (test plan v2.2 LED-I5): test-only read counters of the SQLite ledger
//! medium. While a counter is started on a thread, every row the medium reads on that thread is
//! counted by the table its statement reads (`ledger_events`, `ledger_links`, `ledger_artifacts`
//! or any other table or view), and so is every artifact material file the ledger asks its
//! verifier to open. A row of the three ledger tables read by a statement that selects
//! `sequence` first (and `ordinal` second, for an artifact row) carries its identity, so a row
//! read again is counted as repeated. Dropping the counter stops counting. Counting changes
//! nothing the medium reads or returns.
//!
//! The counters are per thread because an opening runs on the opening thread
//! (`contracts/ledger-store.md`: the constructor "runs on the opening thread"), before the writer
//! thread receives the store. The writer's command window (`LedgerWriterPeak`) therefore never
//! sees an opening, and these counters stay separate from it.

use actingcommand_contract::{ProjectedArtifactReference, VerifiedArtifactReference};
use rusqlite::types::Value;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::marker::PhantomData;

/// The rows of one ledger table a counter saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerTableReads {
    /// Rows the medium read.
    pub rows: u64,
    /// Rows whose identity had already been read since the counter started.
    pub repeated: u64,
}

/// What the SQLite medium read on one thread since its counter started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LedgerReadCounts {
    pub events: LedgerTableReads,
    pub links: LedgerTableReads,
    pub artifacts: LedgerTableReads,
    /// Rows of any other table or view (the meta row, the index table, the views).
    pub other_rows: u64,
    /// Artifact material files the ledger asked its verifier to open.
    pub material_files_opened: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Table {
    Events,
    Links,
    Artifacts,
    Other,
}

#[derive(Default)]
struct Counting {
    counts: LedgerReadCounts,
    /// The identities read: table, `sequence` and, for an artifact row, `ordinal`.
    seen: BTreeSet<(Table, i64, i64)>,
}

thread_local! {
    static COUNTING: RefCell<Option<Counting>> = const { RefCell::new(None) };
}

/// Counts the reads of the SQLite medium on the current thread until it is dropped.
#[must_use = "counting stops when the counter is dropped"]
pub struct LedgerReadCounter {
    // Bound to the thread that started it.
    _thread: PhantomData<*const ()>,
}

impl LedgerReadCounter {
    /// Starts counting on the current thread, from zero.
    pub fn start() -> Self {
        COUNTING.with(|counting| *counting.borrow_mut() = Some(Counting::default()));
        Self {
            _thread: PhantomData,
        }
    }

    /// What was counted since the counter started.
    pub fn counts(&self) -> LedgerReadCounts {
        COUNTING.with(|counting| {
            counting
                .borrow()
                .as_ref()
                .map(|counting| counting.counts)
                .unwrap_or_default()
        })
    }
}

impl Drop for LedgerReadCounter {
    fn drop(&mut self) {
        let _ = COUNTING.try_with(|counting| counting.borrow_mut().take());
    }
}

impl Counting {
    fn count_row(&mut self, sql: &str, row: &[Value]) {
        let table = table_of(sql);
        let reads = match table {
            Table::Events => &mut self.counts.events,
            Table::Links => &mut self.counts.links,
            Table::Artifacts => &mut self.counts.artifacts,
            Table::Other => {
                self.counts.other_rows += 1;
                return;
            }
        };
        reads.rows += 1;
        if let Some((sequence, ordinal)) = identity(table, sql, row)
            && !self.seen.insert((table, sequence, ordinal))
        {
            reads.repeated += 1;
        }
    }
}

/// The table named after the statement's first ` FROM `.
fn table_of(sql: &str) -> Table {
    let rest = sql
        .split_once(" FROM ")
        .map_or("", |(_, rest)| rest.trim_start());
    let end = rest
        .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .unwrap_or(rest.len());
    match &rest[..end] {
        "ledger_events" => Table::Events,
        "ledger_links" => Table::Links,
        "ledger_artifacts" => Table::Artifacts,
        _ => Table::Other,
    }
}

/// The row's `sequence` (and `ordinal`, for an artifact row) when the statement selects them
/// first; `None` when the row carries no identity.
fn identity(table: Table, sql: &str, row: &[Value]) -> Option<(i64, i64)> {
    let mut columns = sql.trim_start().strip_prefix("SELECT ")?.split([',', ' ']);
    if columns.next() != Some("sequence") {
        return None;
    }
    let Some(Value::Integer(sequence)) = row.first() else {
        return None;
    };
    if table != Table::Artifacts {
        return Some((*sequence, 0));
    }
    if columns.next() != Some("ordinal") {
        return None;
    }
    match row.get(1) {
        Some(Value::Integer(ordinal)) => Some((*sequence, *ordinal)),
        _ => None,
    }
}

/// Called by the SQLite medium for each row it read with `sql`.
pub(crate) fn row_read(sql: &str, row: &[Value]) {
    let _ = COUNTING.try_with(|counting| {
        if let Some(counting) = counting.borrow_mut().as_mut() {
            counting.count_row(sql, row);
        }
    });
}

fn material_opened() {
    let _ = COUNTING.try_with(|counting| {
        if let Some(counting) = counting.borrow_mut().as_mut() {
            counting.counts.material_files_opened += 1;
        }
    });
}

/// `verifier`, counting each material file it is asked to open.
pub(crate) fn counting_material<F>(
    verifier: &mut Option<F>,
) -> Option<impl FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>>
where
    F: FnMut(&ProjectedArtifactReference) -> Option<VerifiedArtifactReference>,
{
    verifier.as_mut().map(|verify| {
        move |reference: &ProjectedArtifactReference| {
            material_opened();
            verify(reference)
        }
    })
}

thread_local! {
    /// Workflow #411 A4: a test's action inside the next range read on this thread.
    static RANGE_HEAD_HOOK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Workflow #411 A4: runs `hook` once, on this thread, when the next range read
/// (`GlobalLedger::open_range`) has authenticated its head, inside its read transaction.
pub fn on_range_head(hook: impl FnOnce() + 'static) {
    RANGE_HEAD_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

/// Drops a hook that no range read ran; true when there was one.
pub fn clear_range_head() -> bool {
    RANGE_HEAD_HOOK.with(|slot| slot.borrow_mut().take().is_some())
}

/// Called by the range read once it has authenticated its head.
pub(crate) fn range_head_read() {
    let hook = RANGE_HEAD_HOOK
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten();
    if let Some(hook) = hook {
        hook();
    }
}
