// SPDX-License-Identifier: AGPL-3.0-only

//! P32 B-1 (#422 6473873443; design rd-1028/design-p32.md §2 B, §3 B-1; since 0.12.27): the
//! request and the result of `QueryRuns`, the run listing the Runtime answers from the run
//! summary it keeps in memory (B-2), one row per run with its brief status as
//! [`crate::RunFold`] folds it, read at one ledger snapshot.
//!
//! Defined in 0.12.27 and served from the release that carries P32 B-3: no operation carries
//! them in 0.12.27 (no client sends `QueryRuns`, no Runtime answers it), and B-3 adds the
//! operation with its runtime interface revision row. Both validate on their own; a refusal
//! names the member it refuses ([`QueryRunsInvalid::member`]) under one code,
//! `invalid_query_runs`.

use super::RuntimeContractError;
use crate::codes::RuntimeContractCode;
use crate::outcome::Code;
use crate::{ContainedRunState, InstanceId, RunBriefStatus, RunDispatch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

/// The most rows one `QueryRuns` page answers (the run list's limit, Workflow #389 MCP-3).
pub const MAX_QUERY_RUNS_ROWS: u16 = 100;
/// The longest `task_prefix`, in bytes.
pub const MAX_QUERY_RUNS_TASK_PREFIX_BYTES: usize = 256;

/// A `QueryRuns` request or result the contract refuses, and the member it refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryRunsInvalid {
    member: &'static str,
}

impl QueryRunsInvalid {
    const fn at(member: &'static str) -> Self {
        Self { member }
    }

    /// The member refused, by its name in the request or result (`rows.order_sequence` for a
    /// member of a row).
    pub const fn member(self) -> &'static str {
        self.member
    }

    pub const fn code(self) -> Code {
        RuntimeContractCode::InvalidQueryRuns.code()
    }
}

impl fmt::Display for QueryRunsInvalid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} refused at {}", self.code(), self.member)
    }
}

impl Error for QueryRunsInvalid {}

impl From<QueryRunsInvalid> for RuntimeContractError {
    fn from(invalid: QueryRunsInvalid) -> Self {
        Self::new(invalid.code())
    }
}

/// Where a page of runs starts: the snapshot the listing's first page was read at, and the
/// order sequence of the last run the page before it examined (the next page holds runs
/// ordered before it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsCursor {
    pub snapshot_ledger_position: u64,
    pub before_sequence: u64,
}

impl QueryRunsCursor {
    fn validate(&self, member: &'static str) -> Result<(), QueryRunsInvalid> {
        if self.snapshot_ledger_position == 0
            || self.before_sequence == 0
            || self.before_sequence > self.snapshot_ledger_position
        {
            return Err(QueryRunsInvalid::at(member));
        }
        Ok(())
    }
}

/// `QueryRuns`: the runs first recorded from `since_unix_ms` to `until_unix_ms`, on
/// `instance_id` or on every instance, newest first by each run's first listing fact in the
/// window, kept by `states`, `dispatch` and `task_prefix` (an unset filter keeps every run;
/// `task_prefix` keeps the scheduled runs whose policy task id starts with it), at most `limit`
/// of them; `cursor` continues an earlier page at that page's snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<InstanceId>,
    pub since_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<QueryRunsCursor>,
    pub limit: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<ContainedRunState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<RunDispatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_prefix: Option<String>,
}

impl QueryRunsRequest {
    /// Refuses, by the member: a `limit` of 0 or above [`MAX_QUERY_RUNS_ROWS`]; an
    /// `until_unix_ms` before `since_unix_ms`; a `cursor` at snapshot 0, before sequence 0 or
    /// before a sequence past its snapshot; a state twice or `not_found` among `states` (a
    /// listed run has facts); an empty `task_prefix` or one longer than
    /// [`MAX_QUERY_RUNS_TASK_PREFIX_BYTES`].
    pub fn validate(&self) -> Result<(), QueryRunsInvalid> {
        if self.limit == 0 || self.limit > MAX_QUERY_RUNS_ROWS {
            return Err(QueryRunsInvalid::at("limit"));
        }
        if self
            .until_unix_ms
            .is_some_and(|until| until < self.since_unix_ms)
        {
            return Err(QueryRunsInvalid::at("until_unix_ms"));
        }
        if let Some(cursor) = &self.cursor {
            cursor.validate("cursor")?;
        }
        let mut states = BTreeSet::new();
        for state in &self.states {
            if *state == ContainedRunState::NotFound || !states.insert(state_rank(*state)) {
                return Err(QueryRunsInvalid::at("states"));
            }
        }
        if self.task_prefix.as_ref().is_some_and(|prefix| {
            prefix.is_empty() || prefix.len() > MAX_QUERY_RUNS_TASK_PREFIX_BYTES
        }) {
            return Err(QueryRunsInvalid::at("task_prefix"));
        }
        Ok(())
    }
}

/// How far back the Runtime's run summary answers completely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryRunsCoverageState {
    /// Built: every run with a fact from `from_unix_ms` on is in it.
    Complete,
    /// Being built from the newest record back: every run with a fact from `from_unix_ms` on
    /// is in it so far, older ones not yet.
    Building,
    /// Not built (its build failed): the page holds no row, and the reader reads the ledger.
    Unavailable,
}

/// The coverage a page was answered under: a request whose window starts before
/// `from_unix_ms`, or any request under `unavailable`, is not answered completely by the
/// summary, and its reader says so or reads the ledger; the page never pretends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsCoverage {
    pub from_unix_ms: u64,
    pub state: QueryRunsCoverageState,
}

/// One run of a `QueryRuns` page: its brief status at the page's snapshot, the policy task id
/// of a scheduled run, its order and its two times, and what its listing facts in the window
/// alone show (`settled`, `opened_in_window`, as runtime-client's `WindowRun`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsRow {
    pub status: RunBriefStatus,
    /// The `task_id` of a scheduled run's `policy.dispatch_intent`: an opaque string from the
    /// resource side. Absent for a manual run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_task_id: Option<String>,
    /// The sequence of the run's first listing fact in the window: what the page orders by.
    pub order_sequence: u64,
    /// When the run recorded its first fact.
    pub first_fact_unix_ms: u64,
    /// When the run recorded its terminal, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_unix_ms: Option<u64>,
    /// `succeeded` or `cancelled` when the window holds the run's one terminal and it is
    /// `task.completed` or `task.cancelled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<ContainedRunState>,
    /// Whether the window holds the run's main admission (`task.requested`).
    pub opened_in_window: bool,
}

/// The answer of `QueryRuns`: its rows newest first, the snapshot they were read at, where the
/// next page starts (absent once every run of the window was examined) and the coverage they
/// were answered under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRunsPage {
    pub rows: Vec<QueryRunsRow>,
    pub snapshot_ledger_position: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<QueryRunsCursor>,
    pub coverage: QueryRunsCoverage,
}

impl QueryRunsPage {
    /// Refuses, by the member: a snapshot of 0; more than [`MAX_QUERY_RUNS_ROWS`] rows, or any
    /// row under `unavailable` coverage; rows not newest first by order sequence, or one ordered
    /// at 0 or past the snapshot; a run twice; a row read at another snapshot; a row whose state
    /// is `not_found`; a terminal time without a terminal or a terminal without its time; a
    /// first fact recorded after the terminal; `settled` other than `succeeded` or `cancelled`,
    /// or other than the row's state; a policy task id on a run not dispatched by the scheduler,
    /// or an empty one; a `next` at another snapshot, past its last row, or invalid as a cursor.
    pub fn validate(&self) -> Result<(), QueryRunsInvalid> {
        let snapshot = self.snapshot_ledger_position;
        if snapshot == 0 {
            return Err(QueryRunsInvalid::at("snapshot_ledger_position"));
        }
        if self.rows.len() > usize::from(MAX_QUERY_RUNS_ROWS) {
            return Err(QueryRunsInvalid::at("rows"));
        }
        if self.coverage.state == QueryRunsCoverageState::Unavailable && !self.rows.is_empty() {
            return Err(QueryRunsInvalid::at("coverage"));
        }
        let mut requests = BTreeSet::new();
        let mut before = u64::MAX;
        for row in &self.rows {
            if row.order_sequence == 0
                || row.order_sequence > snapshot
                || row.order_sequence >= before
            {
                return Err(QueryRunsInvalid::at("rows.order_sequence"));
            }
            before = row.order_sequence;
            if !requests.insert(row.status.request_id) {
                return Err(QueryRunsInvalid::at("rows.status.request_id"));
            }
            if row.status.evidence.snapshot_ledger_position != snapshot {
                return Err(QueryRunsInvalid::at("rows.status.evidence"));
            }
            if row.status.state == ContainedRunState::NotFound {
                return Err(QueryRunsInvalid::at("rows.status.state"));
            }
            match (&row.status.terminal, row.terminal_unix_ms) {
                (Some(_), Some(terminal)) if terminal < row.first_fact_unix_ms => {
                    return Err(QueryRunsInvalid::at("rows.terminal_unix_ms"));
                }
                (Some(_), Some(_)) | (None, None) => {}
                _ => return Err(QueryRunsInvalid::at("rows.terminal_unix_ms")),
            }
            if let Some(settled) = row.settled
                && (!matches!(
                    settled,
                    ContainedRunState::Succeeded | ContainedRunState::Cancelled
                ) || settled != row.status.state)
            {
                return Err(QueryRunsInvalid::at("rows.settled"));
            }
            if let Some(task) = &row.catalog_task_id
                && (task.is_empty() || row.status.dispatch != RunDispatch::Scheduled)
            {
                return Err(QueryRunsInvalid::at("rows.catalog_task_id"));
            }
        }
        // The next page holds runs ordered before its cursor: at or before the last row, never
        // a row again.
        if let Some(next) = &self.next {
            next.validate("next")?;
            if next.snapshot_ledger_position != snapshot || next.before_sequence > before {
                return Err(QueryRunsInvalid::at("next"));
            }
        }
        Ok(())
    }
}

/// One number per state, for a set of states.
const fn state_rank(state: ContainedRunState) -> u8 {
    match state {
        ContainedRunState::NotFound => 0,
        ContainedRunState::Admitted => 1,
        ContainedRunState::Running => 2,
        ContainedRunState::Succeeded => 3,
        ContainedRunState::Failed => 4,
        ContainedRunState::Cancelled => 5,
        ContainedRunState::InterruptedUnterminated => 6,
    }
}
