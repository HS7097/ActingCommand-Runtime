// SPDX-License-Identifier: AGPL-3.0-only

//! Generic contained-run status (Workflow #338 R1): a client projection of one manual or
//! scheduled contained run read from its ledger events. It is read-only and never persisted.

use super::*;
use crate::codes::ClientCode;
use actingcommand_contract::{
    ContainedTaskLeaseTerminal, EventLinks, InstanceId, LeaseId, TaskRunIdentity,
};

const RUN_STATUS_SCHEMA: &str = "actingcommand.run-status.v1";
const RUN_STATUS_OPERATION: Location = ClientLocation::ContainedRunStatus.location();
const RECENT_RUNS_OPERATION: Location = ClientLocation::RecentRuns.location();
const MAX_RECENT_RUNS: usize = 10;
/// The most runs one page of a run list holds (Workflow #389 MCP-3).
const MAX_RUN_LIST: usize = 100;
/// The facts a run is listed by (Workflow #391 item 3): its main admission, its entry facts and
/// its terminal. A run without an entry check starts with its main admission, a run with one
/// records an entry fact once that check is done, and a run whose first check fails ends with
/// its terminal. Listing by them reads none of the instance's other events. A run records one
/// main admission and one terminal, but its entry gate's navigation and its shared interrupts
/// record several entry facts (Workflow #373 NAV-5, NAV-6), so a busy window holds more entry
/// facts than one read keeps (Workflow #411 A6): the listing keeps none of them.
const RUN_LISTING_EVENT_TYPES: [EventType; 5] = [
    EventType::TaskRequested,
    EventType::TaskEntryPreflight,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
];
/// #422 IDLE-OV (H1): the listing facts a run records one of each, its main admission and its
/// terminal. `latest_runs` lists these over its whole window and the entry facts, which a busy
/// window holds thousands of, only from its newest runs on.
const RUN_ADMISSION_AND_TERMINAL_TYPES: [EventType; 4] = [
    EventType::TaskRequested,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
];
/// The most pages one listing type is read in (Workflow #411 A6), each of up to 256 records:
/// the listing keeps no record, so it ends at its deadline or at its last page, never at a
/// record count.
const MAX_RUN_LISTING_PAGES: usize = 1_024;
/// The most runs one listing holds (Workflow #411 A6): before it, each listing type was read
/// into at most `MAX_RUN_SUMMARY_EVENTS` records, so every listing that fitted then fits.
const MAX_LISTED_RUNS: usize = RUN_LISTING_EVENT_TYPES.len() * MAX_RUN_SUMMARY_EVENTS;
/// The record types a run's own status reads (Workflow #411 A6), by its request and when a run
/// id is reversed to its request: the request's origin (`request_origin`), the run lease's own
/// release or expiry (`contained_run_lease_terminal`) and the run's semantic facts except its
/// per-frame and per-action ones (evidence, recognition, selection, effect), which the
/// projection never folds and which a long run records thousands of. The facts kept are the
/// ones the projection folds (main admission, entry facts, steps, terminal) and every one that
/// can be a run's first fact, whose sequence and time classify an interruption and filter a
/// window: a run without an entry check starts with its main admission, a run with one with
/// the geometry observation of its first frame (recorded before that frame's evidence), and a
/// run that ends before its first frame with its terminal intent.
const RUN_STATUS_EVENT_TYPES: [EventType; 17] = [
    EventType::CliCommand,
    EventType::UiAction,
    EventType::LabRequest,
    EventType::CommandReceived,
    EventType::TaskRequested,
    EventType::TaskStarted,
    EventType::TaskGeometryObserved,
    EventType::TaskEntryPreflight,
    EventType::TaskStepStarted,
    EventType::TaskStepFinished,
    EventType::TaskTerminalIntent,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
    EventType::TaskTerminalRejected,
    EventType::LeaseReleased,
    EventType::LeaseExpired,
];

/// Selects one contained run by the request that submitted it or by its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKey {
    RequestId(RequestId),
    RunId(RunId),
}

/// Both modes read the same bounded run facts; `Brief` returns no step progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatusMode {
    Full,
    Brief,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDispatch {
    Manual,
    Scheduled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOrigin {
    Cli,
    Ui,
    Lab,
    Scheduler,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainedRunState {
    NotFound,
    Admitted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// A run fact precedes a later Runtime start or takeover without a terminal; uncertain.
    InterruptedUnterminated,
}

/// `actingcommand.run-status.v1`: one contained run as its ledger events show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainedRunStatus {
    pub schema_version: &'static str,
    pub request_id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<InstanceId>,
    pub dispatch: RunDispatch,
    pub origin: RunOrigin,
    /// Workflow #336 PKG-S2 (option 丙): the admitted package's `package_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_label: Option<String>,
    /// The `package_id` of each prerequisite or recovery package the run admitted, in order.
    pub recovery_package_labels: Vec<String>,
    pub state: ContainedRunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ContainedRunTerminal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<ContainedRunLease>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ContainedRunProgress>,
    pub evidence: ContainedRunEvidence,
}

/// A run's terminal as `actingcommand.run-status.v1` shows it. `failure_code` is the code of the
/// terminal's committed outcome (ledger revision 3). A relayed spelling the catalog does not
/// register was committed as `outcome_relay_unregistered`, and that is the code this member
/// reads; the original spelling (a token) stays in that outcome's `raw_code` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainedRunTerminal {
    pub outcome: TaskOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<actingcommand_contract::outcome::CodeStr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executed_steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<ContainedTaskCancellationReason>,
    pub sequence: u64,
    pub event_id: EventId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContainedRunLease {
    pub lease_id: LeaseId,
    pub terminal: Option<ContainedTaskLeaseTerminal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainedRunProgress {
    pub step_index: u32,
    pub operation_label: String,
    pub page: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContainedRunEvidence {
    pub snapshot_ledger_position: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admitted_sequence: Option<u64>,
    pub restarted_after_admission: bool,
}

/// The most recent runs of one instance, newest first, all read at one ledger snapshot.
/// `incomplete` is set when the time budget ended the read before every run was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecentContainedRuns {
    pub runs: Vec<ContainedRunStatus>,
    pub incomplete: bool,
    pub snapshot_ledger_position: u64,
}

/// What a run list keeps (Workflow #389 MCP-3, `actingctl runs` / `ac_runs`); an unset member
/// keeps every run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunListFilter {
    /// Runs on this instance only.
    pub instance_id: Option<InstanceId>,
    /// Scheduled runs whose policy task id (`catalog_task_id`) starts with this text; a manual
    /// run has none and never matches.
    pub task_prefix: Option<String>,
    pub dispatch: Option<RunDispatch>,
    /// Runs in one of these states; empty keeps every state.
    pub states: Vec<ContainedRunState>,
}

/// Where a page of a run list starts: the snapshot its first page was read at and, on a later
/// page, the order sequence of the last run the page before it examined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunListPosition {
    pub snapshot_ledger_position: u64,
    pub before_sequence: u64,
}

/// One run of a run list: its brief run status, the policy task id of a scheduled run, its order
/// and its two times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedContainedRun {
    pub status: ContainedRunStatus,
    /// The policy task a scheduled run was dispatched for: the `task_id` of its
    /// `policy.dispatch_intent`, named `catalog_task_id` where it stands beside the run's own
    /// task_id (`SchedulingOutcomeIdentity`). An opaque string from the resource side. Absent
    /// for a manual run.
    pub catalog_task_id: Option<String>,
    /// What the list orders by, newest first: the sequence of the run's first listing fact in
    /// the window (its main admission, or its entry check when it has one).
    pub order_sequence: u64,
    /// When the run recorded its first fact.
    pub first_fact_unix_ms: u64,
    /// When the run recorded its terminal, once it has one.
    pub terminal_unix_ms: Option<u64>,
}

/// One page of a run list, newest first, read at one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainedRunPage {
    pub runs: Vec<ListedContainedRun>,
    /// Where the next page starts; absent once every listed run was examined.
    pub next: Option<RunListPosition>,
    /// The deadline ended the page before it held `limit` runs and before every listed run was
    /// examined; `next` continues after the last run it examined.
    pub incomplete: bool,
    pub snapshot_ledger_position: u64,
}

struct StatusPages {
    events: Vec<ProjectedEvent>,
    snapshot: u64,
    complete: bool,
}

/// How one listing query ended (Workflow #411 A6): the snapshot it was read at, and whether it
/// reached its last page before its deadline.
struct ListingPages {
    snapshot: u64,
    complete: bool,
}

/// One run a listing found: its identity and its first listing fact in the window; and (#422
/// QR-2b) whether the window holds its main admission, and the terminal facts the window holds
/// of it.
#[derive(Debug, Clone, Copy)]
struct ListedRun {
    identity: TaskRunIdentity,
    sequence: u64,
    opened: bool,
    terminal: Option<EventType>,
    terminals: u32,
}

impl ListedRun {
    fn first(identity: TaskRunIdentity, event: &ProjectedEvent) -> Self {
        let mut run = Self {
            identity,
            sequence: event.sequence,
            opened: false,
            terminal: None,
            terminals: 0,
        };
        run.note(event.event_type);
        run
    }

    /// #422 QR-2b: one more listing fact of the run, by its type.
    fn note(&mut self, event_type: EventType) {
        match event_type {
            EventType::TaskRequested => self.opened = true,
            EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled => {
                self.terminal = Some(event_type);
                self.terminals = self.terminals.saturating_add(1);
            }
            _ => {}
        }
    }

    /// #422 QR-2b: the state the listing alone proves: succeeded or cancelled when it holds the
    /// run's one terminal and that terminal is `task.completed` or `task.cancelled` (a
    /// terminal's record type is its `task_result`'s, so the run's status read would answer
    /// that state). A failed terminal, no terminal or more than one terminal proves nothing.
    fn settled(&self) -> Option<ContainedRunState> {
        match (self.terminals, self.terminal) {
            (1, Some(EventType::TaskCompleted)) => Some(ContainedRunState::Succeeded),
            (1, Some(EventType::TaskCancelled)) => Some(ContainedRunState::Cancelled),
            _ => None,
        }
    }
}

/// #422 QR-2b: one run of a window's run listing (`list_window_runs`), as the listing alone
/// shows it, before any read of the run's own facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRun {
    pub identity: TaskRunIdentity,
    /// The sequence of its first listing fact in the window: what the listing orders by.
    pub order_sequence: u64,
    /// `Succeeded` or `Cancelled` when the window holds the run's one terminal and it is
    /// `task.completed` or `task.cancelled`: its status read would answer that state, so a
    /// reader that only needs to know that it is neither failed nor open need not read it. A
    /// settled run loses the identity checks of its own read (the listing checks its identity
    /// against its other listing facts only). `None` for a failed run, an open one, or one with
    /// more than one terminal (its status read refuses it).
    pub settled: Option<ContainedRunState>,
    /// Whether the window holds the run's main admission (`task.requested`), not only its
    /// entry facts or its terminal. A run listed without its admission began before the window
    /// (an entry fact can be an interrupt it handled mid-run), or ended before its admission
    /// (its entry check failed, or it ended before its first frame); only its status read
    /// tells which. A run with its admission in the window began in it, as its status read
    /// would find, except that a run with an entry check records its first frame's geometry
    /// (its first fact) and its entry check's facts before its admission, so a run whose entry
    /// check began before the window's start and whose admission falls in the window is
    /// counted in it.
    pub opened_in_window: bool,
}

/// #422 QR-2b: the runs of one window, each once, newest first by their first listing fact in
/// the window, all read at one snapshot. `complete` is false when the deadline ended a listing
/// read before its last page; a partial listing may hold an older run and miss a newer one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRunListing {
    pub runs: Vec<WindowRun>,
    pub snapshot_ledger_position: u64,
    pub complete: bool,
}

/// #422 QR-2b: one listed run's brief status, read at its listing's snapshot, and when the run
/// recorded its first fact (a run first recorded before the window is not one of its runs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRunStatus {
    pub status: ContainedRunStatus,
    pub first_fact_unix_ms: Option<u64>,
}

/// The runs of one window, each once, oldest first by their first listing fact there, all read
/// at `snapshot`. `complete` is false when the deadline ended a read before its last page.
struct RunListing {
    runs: Vec<ListedRun>,
    snapshot: u64,
    complete: bool,
}

/// A run listing while its listing queries are folded into it (Workflow #411 A6; #422 IDLE-OV
/// H1): its runs, each once with its first listing fact read so far, at the snapshot of its
/// first query.
struct RunListingFold {
    runs: Vec<ListedRun>,
    listed: BTreeMap<RequestId, usize>,
    runs_seen: BTreeSet<RunId>,
    snapshot: Option<u64>,
    complete: bool,
}

impl RunListingFold {
    fn at(snapshot: Option<u64>) -> Self {
        Self {
            runs: Vec::new(),
            listed: BTreeMap::new(),
            runs_seen: BTreeSet::new(),
            snapshot,
            complete: true,
        }
    }

    /// The first listing fact read so far of the `nth` newest run by that fact (1: the
    /// newest); `None` when fewer runs are listed.
    fn nth_newest_first_sequence(&self, nth: usize) -> Option<u64> {
        let mut sequences = self.runs.iter().map(|run| run.sequence).collect::<Vec<_>>();
        sequences.sort_unstable();
        sequences
            .len()
            .checked_sub(nth)
            .and_then(|index| sequences.get(index).copied())
    }

    /// One listing fact: a run it names the first time is listed, a later fact of a listed
    /// run can only move its first listing fact earlier.
    fn fold(&mut self, event: &ProjectedEvent) -> RuntimeClientResult<()> {
        let Some(identity) = run_fact_identity(event)? else {
            return Ok(());
        };
        match self.listed.get(&identity.request_id) {
            Some(index) => {
                let run = &mut self.runs[*index];
                if run.identity != identity {
                    return Err(status_error(ClientCode::RunStatusIdentityMismatch));
                }
                run.sequence = run.sequence.min(event.sequence);
                run.note(event.event_type);
            }
            None => {
                if !self.runs_seen.insert(identity.run_id) {
                    return Err(status_error(ClientCode::RunStatusIdentityAmbiguous));
                }
                if self.runs.len() == MAX_LISTED_RUNS {
                    return Err(RuntimeClientError::fatal(
                        ClientCode::RunStatusResidentLimitExceeded,
                        RECENT_RUNS_OPERATION,
                    ));
                }
                self.listed.insert(identity.request_id, self.runs.len());
                self.runs.push(ListedRun::first(identity, event));
            }
        }
        Ok(())
    }

    /// The listing, oldest first by each run's first listing fact.
    fn finish(mut self) -> RunListing {
        self.runs.sort_by_key(|run| run.sequence);
        RunListing {
            runs: self.runs,
            snapshot: self.snapshot.unwrap_or(0),
            complete: self.complete,
        }
    }
}

struct ReadRunStatus {
    status: ContainedRunStatus,
    first_fact_timestamp_unix_ms: Option<u64>,
    terminal_timestamp_unix_ms: Option<u64>,
}

impl RuntimeClient {
    /// Projects one contained run, manual or scheduled, from its ledger events (Workflow #338
    /// R1). A run with a fact before a later Runtime start or takeover without a terminal is
    /// `interrupted_unterminated`. Read-only.
    pub fn contained_run_status(
        &self,
        key: RunKey,
        mode: RunStatusMode,
    ) -> RuntimeClientResult<ContainedRunStatus> {
        self.read_contained_run_status(key, mode, None, None)?
            .map(|read| read.status)
            .ok_or_else(|| status_error(ClientCode::RunStatusReadIncomplete))
    }

    /// The newest `limit` (1-10) contained runs first recorded on `instance` since
    /// `since_unix_ms`, newest first, each in brief mode (Workflow #411 A42; #422 IDLE-OV).
    /// The window is listed forward, by run, on the instance, each listing query a first page
    /// that normally starts at the window's first record (LN-WS R3):
    /// - its main admissions and terminals over the whole window, a few facts per run;
    /// - then its entry facts (H1), which a busy instance records thousands of a day, only from
    ///   the first listing fact of the `limit`-th newest run found (for the overview, the
    ///   newest), or from the window's start when fewer were found. Runs on one instance do not
    ///   overlap: a run records its facts under its lease, the scheduler holds one lease per
    ///   instance, and an entry fact is recorded only before the run's end phase. So every
    ///   entry fact of a newer run, also of one still in its entry phase, lies after that fact.
    ///   The one fact recorded after a lease ended is a terminal the sweep's expiry found
    ///   (#411 A64); a run without a main admission that ends so is listed at that terminal.
    ///
    /// An instance without a run in the window so normally costs one page per listing type,
    /// however many records of other types the window holds; a busy one its admissions and
    /// terminals and the entry facts of its newest runs, not those of the whole window. A first
    /// page whose window lookup the Runtime's read budget cut (the budget was spent waiting for
    /// the shared connection) starts at the query's first sequence instead (sequence 1 without
    /// `from_sequence`) and pages through the history before the window on the type's index,
    /// its rows still filtered to the window. Its runs are then read newest first. An instance
    /// without a run since `since_unix_ms` answers no run, complete. A listing `deadline` ended
    /// is `incomplete` with no run (it may miss a newer run), never "no run"; `deadline` is
    /// checked between reads, and reaching it ends the read `incomplete`. Read-only.
    pub fn latest_runs(
        &self,
        instance: InstanceId,
        since_unix_ms: u64,
        limit: usize,
        deadline: Instant,
    ) -> RuntimeClientResult<RecentContainedRuns> {
        if limit == 0 || limit > MAX_RECENT_RUNS {
            return Err(RuntimeClientError::fatal(
                ClientCode::RecentRunsLimitInvalid,
                RECENT_RUNS_OPERATION,
            ));
        }
        let window = EventQuery {
            instance_id: Some(instance),
            from_timestamp_unix_ms: Some(since_unix_ms),
            ..EventQuery::default()
        };
        let mut fold = RunListingFold::at(None);
        self.fold_run_listing(
            &mut fold,
            &RUN_ADMISSION_AND_TERMINAL_TYPES,
            &window,
            Some(deadline),
        )?;
        // H1: the entry facts from the `limit`-th newest run found on; a run they list besides
        // came after it. An incomplete fold reads no further.
        let entries = EventQuery {
            from_sequence: fold.nth_newest_first_sequence(limit),
            ..window
        };
        self.fold_run_listing(
            &mut fold,
            &[EventType::TaskEntryPreflight],
            &entries,
            Some(deadline),
        )?;
        let listing = fold.finish();
        let mut latest = RecentContainedRuns {
            runs: Vec::with_capacity(limit),
            incomplete: !listing.complete,
            snapshot_ledger_position: listing.snapshot,
        };
        // A partial listing may hold an older run and miss a newer one.
        if latest.incomplete {
            return Ok(latest);
        }
        for listed in listing.runs.iter().rev() {
            if Instant::now() >= deadline {
                latest.incomplete = true;
                return Ok(latest);
            }
            let Some(status) = self.read_contained_run_status(
                RunKey::RequestId(listed.identity.request_id),
                RunStatusMode::Brief,
                Some(listing.snapshot),
                Some(deadline),
            )?
            else {
                latest.incomplete = true;
                return Ok(latest);
            };
            // A later fact inside the window does not make an older run recent.
            if status
                .first_fact_timestamp_unix_ms
                .is_some_and(|first| first >= since_unix_ms)
            {
                latest.runs.push(status.status);
                if latest.runs.len() == limit {
                    return Ok(latest);
                }
            }
        }
        Ok(latest)
    }

    /// One page of the contained runs first recorded from `since_unix_ms` to `until_unix_ms`
    /// (Workflow #389 MCP-3, `actingctl runs` / `ac_runs`). The runs of the window are listed by
    /// run (Workflow #391 item 3), and examined newest first by their order sequence: each
    /// is read in brief mode and kept when `filter` keeps it, until `limit` (1-100) are kept.
    /// A scheduled run also gets its policy task id, from its dispatch intent; `task_prefix`
    /// matches that id, so a run without one is not kept and not read. `from` continues an
    /// earlier page at that page's snapshot. `deadline` is checked between runs; reaching it ends
    /// the page `incomplete`. Read-only.
    pub fn list_contained_runs(
        &self,
        since_unix_ms: u64,
        until_unix_ms: Option<u64>,
        filter: &RunListFilter,
        from: Option<RunListPosition>,
        limit: usize,
        deadline: Instant,
    ) -> RuntimeClientResult<ContainedRunPage> {
        if limit == 0 || limit > MAX_RUN_LIST {
            return Err(RuntimeClientError::fatal(
                ClientCode::RecentRunsLimitInvalid,
                RECENT_RUNS_OPERATION,
            ));
        }
        let listing = self.list_runs(
            &EventQuery {
                instance_id: filter.instance_id,
                from_timestamp_unix_ms: Some(since_unix_ms),
                to_timestamp_unix_ms: until_unix_ms,
                ..EventQuery::default()
            },
            from.map(|position| position.snapshot_ledger_position),
            Some(deadline),
        )?;
        let before = from.map_or(u64::MAX, |position| position.before_sequence);
        let mut page = ContainedRunPage {
            runs: Vec::new(),
            next: None,
            incomplete: false,
            snapshot_ledger_position: listing.snapshot,
        };
        let position = |before_sequence| RunListPosition {
            snapshot_ledger_position: listing.snapshot,
            before_sequence,
        };
        // A partial listing may miss a run between two it holds: no run is examined, and the
        // next page starts where this one did.
        if !listing.complete {
            page.incomplete = true;
            page.next = Some(position(before));
            return Ok(page);
        }
        let mut examined = before;
        for listed in listing
            .runs
            .iter()
            .rev()
            .filter(|listed| listed.sequence < before)
        {
            if page.runs.len() == limit {
                page.next = Some(position(examined));
                break;
            }
            if Instant::now() >= deadline {
                page.incomplete = true;
                page.next = Some(position(examined));
                break;
            }
            let identity = listed.identity;
            // Workflow #389 MCP-3 ruling: task_prefix matches the policy task id, which only a
            // scheduled run has. A run without one, or whose one does not start with the
            // prefix, is not read.
            let mut catalog_task_id = None;
            if let Some(prefix) = &filter.task_prefix {
                match self.dispatched_catalog_task_id(identity, listing.snapshot)? {
                    Some(found) if found.starts_with(prefix.as_str()) => {
                        catalog_task_id = Some(found);
                    }
                    _ => {
                        examined = listed.sequence;
                        continue;
                    }
                }
            }
            let Some(read) = self.read_contained_run_status(
                RunKey::RequestId(identity.request_id),
                RunStatusMode::Brief,
                Some(listing.snapshot),
                Some(deadline),
            )?
            else {
                page.incomplete = true;
                page.next = Some(position(examined));
                break;
            };
            examined = listed.sequence;
            // A run first recorded before the window is not one of its runs.
            let Some(first_fact_unix_ms) = read
                .first_fact_timestamp_unix_ms
                .filter(|first| *first >= since_unix_ms)
            else {
                continue;
            };
            if filter
                .dispatch
                .is_some_and(|dispatch| read.status.dispatch != dispatch)
                || (!filter.states.is_empty() && !filter.states.contains(&read.status.state))
            {
                continue;
            }
            // Only a scheduled run carries a policy task id; a manual one never matches a
            // task_prefix.
            let catalog_task_id = match (read.status.dispatch, catalog_task_id) {
                (RunDispatch::Scheduled, Some(found)) => Some(found),
                (RunDispatch::Scheduled, None) => {
                    self.dispatched_catalog_task_id(identity, listing.snapshot)?
                }
                _ if filter.task_prefix.is_some() => continue,
                _ => None,
            };
            page.runs.push(ListedContainedRun {
                status: read.status,
                catalog_task_id,
                order_sequence: listed.sequence,
                first_fact_unix_ms,
                terminal_unix_ms: read.terminal_timestamp_unix_ms,
            });
        }
        Ok(page)
    }

    /// #422 QR-2b: the contained runs with a listing fact since `since_unix_ms`, on `instance`
    /// or on every instance, listed once as `list_contained_runs` lists them (each listing
    /// type one query, its pages without run recovery, all at one snapshot), newest first, and
    /// none of them read. Each run says what the listing alone shows of it (`WindowRun`): a
    /// reader reads with `window_run_status` and `window_run_catalog_task_id`, at the listing's
    /// snapshot, only the runs it needs, so one listing serves every part of a reader that
    /// needs the window's runs. `deadline` is checked between pages; reaching it ends the
    /// listing `complete: false`. Read-only.
    pub fn list_window_runs(
        &self,
        instance: Option<InstanceId>,
        since_unix_ms: u64,
        deadline: Instant,
    ) -> RuntimeClientResult<WindowRunListing> {
        let listing = self.list_runs(
            &EventQuery {
                instance_id: instance,
                from_timestamp_unix_ms: Some(since_unix_ms),
                ..EventQuery::default()
            },
            None,
            Some(deadline),
        )?;
        Ok(WindowRunListing {
            runs: listing
                .runs
                .iter()
                .rev()
                .map(|run| WindowRun {
                    identity: run.identity,
                    order_sequence: run.sequence,
                    settled: run.settled(),
                    opened_in_window: run.opened,
                })
                .collect(),
            snapshot_ledger_position: listing.snapshot,
            complete: listing.complete,
        })
    }

    /// #422 QR-2b: the brief status of one run of a `list_window_runs` listing, read at that
    /// listing's `snapshot` as `list_contained_runs` reads it; `None` when `deadline`, checked
    /// between pages, ended the read. Read-only.
    pub fn window_run_status(
        &self,
        run: &WindowRun,
        snapshot: u64,
        deadline: Instant,
    ) -> RuntimeClientResult<Option<WindowRunStatus>> {
        Ok(self
            .read_contained_run_status(
                RunKey::RequestId(run.identity.request_id),
                RunStatusMode::Brief,
                Some(snapshot),
                Some(deadline),
            )?
            .map(|read| WindowRunStatus {
                status: read.status,
                first_fact_unix_ms: read.first_fact_timestamp_unix_ms,
            }))
    }

    /// #422 QR-2b: the policy task id of one run of a `list_window_runs` listing (the `task_id`
    /// of its `policy.dispatch_intent`, as `list_contained_runs` reads it for a scheduled run),
    /// one bounded page at the listing's `snapshot`; `None` for a run dispatched by hand.
    /// Read-only.
    pub fn window_run_catalog_task_id(
        &self,
        run: &WindowRun,
        snapshot: u64,
    ) -> RuntimeClientResult<Option<String>> {
        self.dispatched_catalog_task_id(run.identity, snapshot)
    }

    /// The runs `window` holds a listing fact of (Workflow #391 item 3), one query per listing
    /// fact type, all at `snapshot` or else at the first query's; each run once, under the
    /// identity checks of a run's own read. A later fact inside the window does not list an
    /// older run first: the order is by each run's first listing fact in the window. Each
    /// page's facts are folded into the runs as the page arrives and none is kept (Workflow
    /// #411 A6), so however many listing facts the window holds, the listing holds its runs
    /// only, at most `MAX_LISTED_RUNS` of them.
    fn list_runs(
        &self,
        window: &EventQuery,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
    ) -> RuntimeClientResult<RunListing> {
        let mut fold = RunListingFold::at(snapshot);
        self.fold_run_listing(&mut fold, &RUN_LISTING_EVENT_TYPES, window, deadline)?;
        Ok(fold.finish())
    }

    /// Folds the listing facts of `event_types` in `window` into `fold`, one query per type at
    /// the fold's snapshot (or else the first query's, which the fold keeps). A query its
    /// deadline ended leaves the fold incomplete and reads no further type; an incomplete fold
    /// reads nothing.
    fn fold_run_listing(
        &self,
        fold: &mut RunListingFold,
        event_types: &[EventType],
        window: &EventQuery,
        deadline: Option<Instant>,
    ) -> RuntimeClientResult<()> {
        for event_type in event_types {
            if !fold.complete {
                break;
            }
            let read = self.visit_listing_pages(
                EventQuery {
                    event_type: Some(*event_type),
                    ..window.clone()
                },
                fold.snapshot,
                deadline,
                |event| fold.fold(event),
            )?;
            fold.snapshot = Some(read.snapshot);
            fold.complete = read.complete;
        }
        Ok(())
    }

    /// Every page of one listing query on one snapshot (Workflow #411 A6), with the cursor,
    /// snapshot and order checks of `read_status_pages`. Each record is handed to
    /// `visit` as its page arrives and is not kept, so the read is bounded by its deadline
    /// (reaching it ends the read with `complete: false`) and by `MAX_RUN_LISTING_PAGES`, not by
    /// a record count.
    fn visit_listing_pages(
        &self,
        query: EventQuery,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
        mut visit: impl FnMut(&ProjectedEvent) -> RuntimeClientResult<()>,
    ) -> RuntimeClientResult<ListingPages> {
        let invalid = |code| RuntimeClientError::fatal(code, RECENT_RUNS_OPERATION);
        let mut event_ids = BTreeSet::<EventId>::new();
        let mut cursor = None;
        let mut position = snapshot;
        let mut last_sequence = 0_u64;
        for _ in 0..MAX_RUN_LISTING_PAGES {
            // #422 QR-2: the listing reads no run recovery, so its pages ask for none.
            let mut request =
                RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, cursor.clone())
                    .map_err(|_| invalid(ClientCode::RunStatusPageRequestInvalid))?
                    .without_run_recovery();
            if cursor.is_none()
                && let Some(snapshot) = snapshot
            {
                request = request
                    .at_snapshot(snapshot)
                    .map_err(|_| invalid(ClientCode::RunStatusPageRequestInvalid))?;
            }
            let page =
                self.query_event_page(query.clone(), ProjectionProfile::Forensic, request)?;
            let page_position = page.snapshot_ledger_position();
            if position.is_some_and(|expected| expected != page_position) {
                return Err(invalid(ClientCode::RunStatusSnapshotChanged));
            }
            position = Some(page_position);
            for event in page.events() {
                if event.sequence <= last_sequence || !event_ids.insert(event.event_id) {
                    return Err(invalid(ClientCode::RunStatusPaginationInvalid));
                }
                last_sequence = event.sequence;
                visit(event)?;
            }
            if !page.has_more() {
                return Ok(ListingPages {
                    snapshot: page_position,
                    complete: true,
                });
            }
            let next = page
                .next_cursor()
                .cloned()
                .ok_or_else(|| invalid(ClientCode::RunStatusPaginationInvalid))?;
            let previous_sequence = cursor.as_ref().map_or(0, |value| value.after_sequence());
            // #411 A51: a page the Runtime's read budget cut goes on after the position its
            // read reached, at or after its last row.
            if next.after_sequence() < last_sequence || next.after_sequence() <= previous_sequence {
                return Err(invalid(ClientCode::RunStatusPaginationInvalid));
            }
            cursor = Some(next);
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(ListingPages {
                    snapshot: page_position,
                    complete: false,
                });
            }
        }
        Err(invalid(ClientCode::RunStatusPageLimitExceeded))
    }

    /// The policy task id of the run `identity` names (Workflow #389 MCP-3 ruling): the `task_id`
    /// of its one `policy.dispatch_intent`. The intent is linked to the run and its task but
    /// recorded under the admission request, not the run's own request, so it is read with one
    /// bounded query by run at `snapshot`. A run dispatched by hand has none. The id is an
    /// opaque string from the resource side and is never parsed.
    fn dispatched_catalog_task_id(
        &self,
        identity: TaskRunIdentity,
        snapshot: u64,
    ) -> RuntimeClientResult<Option<String>> {
        let query = EventQuery {
            run_id: Some(identity.run_id),
            event_type: Some(EventType::PolicyDispatchIntent),
            ..EventQuery::default()
        };
        let intents = self.first_status_events(query, 2, snapshot)?;
        let intent = match intents.as_slice() {
            [] => return Ok(None),
            [intent] => intent,
            _ => return Err(status_error(ClientCode::RunStatusIdentityAmbiguous)),
        };
        if intent.links.task_id() != Some(&identity.task_id) {
            return Err(status_error(ClientCode::RunStatusIdentityMismatch));
        }
        match full_payload(intent)? {
            EventPayload::Policy(PolicyPayload::DispatchIntent(payload)) => {
                Ok(Some(payload.task_id().to_owned()))
            }
            _ => Err(status_error(ClientCode::RunStatusIdentityMissing)),
        }
    }

    /// `None` only when `deadline` ended a paged read before its last page. Both reads, by run
    /// and by request, read `RUN_STATUS_EVENT_TYPES` only (Workflow #411 A6).
    fn read_contained_run_status(
        &self,
        key: RunKey,
        mode: RunStatusMode,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
    ) -> RuntimeClientResult<Option<ReadRunStatus>> {
        let (request_id, snapshot) = match key {
            RunKey::RequestId(request_id) => (request_id, snapshot),
            RunKey::RunId(run_id) => {
                let read = self.read_status_pages(
                    EventQuery {
                        run_id: Some(run_id),
                        event_types: RUN_STATUS_EVENT_TYPES.to_vec(),
                        ..EventQuery::default()
                    },
                    snapshot,
                    deadline,
                    RUN_STATUS_OPERATION,
                )?;
                if !read.complete {
                    return Ok(None);
                }
                let mut request_ids = Vec::new();
                for event in &read.events {
                    if let Some(identity) = run_fact_identity(event)? {
                        let request_id = identity.request_id;
                        if !request_ids.contains(&request_id) {
                            request_ids.push(request_id);
                        }
                    }
                }
                match request_ids.as_slice() {
                    [] => {
                        return Ok(Some(ReadRunStatus {
                            status: ContainedRunStatus::not_found(
                                None,
                                Some(run_id),
                                read.snapshot,
                            ),
                            first_fact_timestamp_unix_ms: None,
                            terminal_timestamp_unix_ms: None,
                        }));
                    }
                    [request_id] => (*request_id, Some(read.snapshot)),
                    _ => return Err(status_error(ClientCode::RunStatusIdentityAmbiguous)),
                }
            }
        };
        let query = EventQuery {
            request_id: Some(request_id),
            event_types: RUN_STATUS_EVENT_TYPES.to_vec(),
            ..EventQuery::default()
        };
        let read = self.read_status_pages(query, snapshot, deadline, RUN_STATUS_OPERATION)?;
        if !read.complete {
            return Ok(None);
        }
        let snapshot = read.snapshot;
        let events = read.events;
        if events.is_empty() {
            return Ok(Some(ReadRunStatus {
                status: ContainedRunStatus::not_found(Some(request_id), None, snapshot),
                first_fact_timestamp_unix_ms: None,
                terminal_timestamp_unix_ms: None,
            }));
        }

        let mut origin = None;
        let mut admitted = None;
        let mut terminal = None;
        let mut recovery_package_labels = Vec::new();
        let mut progress = None;
        let mut step_started = false;
        let mut first_fact = None;
        let mut first_fact_timestamp_unix_ms = None;
        let mut terminal_timestamp_unix_ms = None;
        for event in &events {
            if let Some(found) = request_origin(event)
                && origin.replace(found).is_some()
            {
                return Err(status_error(ClientCode::RunStatusOriginInconsistent));
            }
            let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(event)? else {
                continue;
            };
            let identity = run_fact_identity(event)?
                .ok_or_else(|| status_error(ClientCode::RunStatusIdentityMissing))?;
            match first_fact {
                Some((_, previous)) if previous != identity => {
                    return Err(status_error(ClientCode::RunStatusIdentityMismatch));
                }
                None => {
                    first_fact = Some((event.sequence, identity));
                    first_fact_timestamp_unix_ms = Some(event.timestamp_unix_ms);
                }
                _ => {}
            }
            match payload.fact() {
                TaskSemanticFact::PackageAdmitted { package_label, .. } => {
                    let found = (
                        event.sequence,
                        package_label.clone(),
                        event.links.lease_id().copied(),
                    );
                    if admitted.replace(found).is_some() {
                        return Err(status_error(ClientCode::RunStatusPackageFactCountInvalid));
                    }
                }
                TaskSemanticFact::EntryRecoveryPackageAdmitted { package_label, .. } => {
                    recovery_package_labels.push(package_label.clone());
                }
                TaskSemanticFact::StepStarted {
                    step_index,
                    operation_label,
                    from_page,
                    ..
                } => {
                    step_started = true;
                    progress = Some(ContainedRunProgress {
                        step_index: *step_index,
                        operation_label: operation_label.clone(),
                        page: from_page.clone(),
                    });
                }
                TaskSemanticFact::StepFinished {
                    step_index,
                    operation_label,
                    page_label,
                    ..
                } => {
                    progress = Some(ContainedRunProgress {
                        step_index: *step_index,
                        operation_label: operation_label.clone(),
                        page: page_label.clone(),
                    });
                }
                TaskSemanticFact::TerminalCommitted {
                    task_result,
                    final_page,
                    executed_steps,
                    outcome,
                    ..
                } => {
                    let failure_code = outcome.as_ref().map(|outcome| outcome.code().clone());
                    let found = ContainedRunTerminal {
                        outcome: *task_result,
                        final_page: final_page.clone(),
                        executed_steps: *executed_steps,
                        cancellation_reason: (*task_result == TaskOutcome::Cancelled).then_some(
                            ContainedTaskCancellationReason::from_failure_code(
                                failure_code.as_ref(),
                            ),
                        ),
                        failure_code,
                        sequence: event.sequence,
                        event_id: event.event_id,
                    };
                    if terminal.replace(found).is_some() {
                        return Err(status_error(ClientCode::RunStatusTerminalStateInconsistent));
                    }
                    terminal_timestamp_unix_ms = Some(event.timestamp_unix_ms);
                }
                _ => {}
            }
        }

        if let Some((_, identity)) = first_fact
            && (identity.request_id != request_id
                || events
                    .iter()
                    .any(|event| !identity.matches_links(&event.links)))
        {
            return Err(status_error(ClientCode::RunStatusIdentityMismatch));
        }
        let run_id = single_link(&events, EventLinks::run_id)?;
        if let RunKey::RunId(expected) = key
            && run_id != Some(expected)
        {
            return Err(status_error(ClientCode::RunStatusIdentityMismatch));
        }
        let task_id = single_link(&events, EventLinks::task_id)?;
        let instance_id = single_link(&events, EventLinks::instance_id)?;
        let correlation_id = single_link(&events, EventLinks::correlation_id)?;
        let admitted_sequence = admitted.as_ref().map(|(sequence, _, _)| *sequence);
        let restarted_after_first_fact = match first_fact {
            Some((sequence, _)) => self.runtime_restarted_since(sequence, snapshot)?,
            None => false,
        };
        let restarted_after_admission = match admitted_sequence {
            Some(sequence) if restarted_after_first_fact => {
                self.runtime_restarted_since(sequence, snapshot)?
            }
            Some(_) => false,
            None => false,
        };
        let open = terminal.is_none() && first_fact.is_some();
        let lease = match single_link(&events, EventLinks::lease_id)? {
            Some(lease_id) => Some(ContainedRunLease {
                lease_id,
                terminal: self.contained_run_lease_terminal(lease_id, &events, snapshot)?,
            }),
            None => None,
        };
        let state = match &terminal {
            Some(terminal) => match terminal.outcome {
                TaskOutcome::Success => ContainedRunState::Succeeded,
                TaskOutcome::Failure => ContainedRunState::Failed,
                TaskOutcome::Cancelled => ContainedRunState::Cancelled,
            },
            None if !open => ContainedRunState::NotFound,
            None if restarted_after_first_fact => ContainedRunState::InterruptedUnterminated,
            None if step_started || admitted_sequence.is_none() => ContainedRunState::Running,
            None => ContainedRunState::Admitted,
        };
        let (dispatch, origin) = origin.unwrap_or((RunDispatch::Unknown, RunOrigin::Unknown));
        Ok(Some(ReadRunStatus {
            status: ContainedRunStatus {
                schema_version: RUN_STATUS_SCHEMA,
                request_id: Some(request_id),
                correlation_id,
                run_id,
                task_id,
                instance_id,
                dispatch,
                origin,
                package_label: admitted.map(|(_, package, _)| package),
                recovery_package_labels,
                state,
                terminal,
                lease,
                progress: if mode == RunStatusMode::Full {
                    progress
                } else {
                    None
                },
                evidence: ContainedRunEvidence {
                    snapshot_ledger_position: snapshot,
                    admitted_sequence,
                    restarted_after_admission,
                },
            },
            first_fact_timestamp_unix_ms,
            terminal_timestamp_unix_ms,
        }))
    }

    /// Any Runtime start or takeover at or after `sequence`, at `snapshot`. Neither event links
    /// a request and the event origin carries no epoch, so each is one targeted query.
    fn runtime_restarted_since(&self, sequence: u64, snapshot: u64) -> RuntimeClientResult<bool> {
        for event_type in [EventType::RuntimeStarted, EventType::RuntimeTakeover] {
            let query = EventQuery {
                event_type: Some(event_type),
                from_sequence: Some(sequence),
                ..EventQuery::default()
            };
            if !self.first_status_events(query, 1, snapshot)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The run lease's terminal: the request's own release when it has one, else the lease's
    /// release or expiry (an expiry or a connection cleanup does not link the request).
    fn contained_run_lease_terminal(
        &self,
        lease_id: LeaseId,
        request_events: &[ProjectedEvent],
        snapshot: u64,
    ) -> RuntimeClientResult<Option<ContainedTaskLeaseTerminal>> {
        let mut terminals = request_events
            .iter()
            .filter(|event| is_lease_terminal(event, lease_id))
            .cloned()
            .collect::<Vec<_>>();
        if terminals.is_empty() {
            for event_type in [EventType::LeaseReleased, EventType::LeaseExpired] {
                let query = EventQuery {
                    lease_id: Some(lease_id),
                    event_type: Some(event_type),
                    ..EventQuery::default()
                };
                terminals.extend(self.first_status_events(query, 2, snapshot)?);
            }
        }
        match terminals.as_slice() {
            [] => Ok(None),
            [event] if event.event_type == EventType::LeaseExpired => {
                Ok(Some(ContainedTaskLeaseTerminal::Expired))
            }
            [_] => Ok(Some(ContainedTaskLeaseTerminal::Released)),
            _ => Err(status_error(ClientCode::RunStatusLeaseTerminalInconsistent)),
        }
    }

    /// One bounded page at `snapshot`, without run recovery (#422 QR-2: its rows are all it
    /// reads).
    fn first_status_events(
        &self,
        query: EventQuery,
        limit: u16,
        snapshot: u64,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        let request = RuntimeEventQueryPageRequest::new(limit, None)
            .map(RuntimeEventQueryPageRequest::without_run_recovery)
            .and_then(|request| request.at_snapshot(snapshot))
            .map_err(|_| status_error(ClientCode::RunStatusPageRequestInvalid))?;
        let page = self.query_event_page(query, ProjectionProfile::Forensic, request)?;
        if page.snapshot_ledger_position() != snapshot {
            return Err(status_error(ClientCode::RunStatusSnapshotChanged));
        }
        Ok(page.events().to_vec())
    }

    /// Every page of one Forensic query on one snapshot, with `summarize_run`'s cursor, snapshot
    /// and limit checks. A `deadline` is checked between pages; reaching it ends the read with
    /// `complete: false`.
    fn read_status_pages(
        &self,
        query: EventQuery,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
        operation: Location,
    ) -> RuntimeClientResult<StatusPages> {
        let invalid = |code| RuntimeClientError::fatal(code, operation);
        let mut events = Vec::new();
        let mut event_ids = BTreeSet::<EventId>::new();
        let mut cursor = None;
        let mut position = snapshot;
        let mut last_sequence = 0_u64;
        let mut resident_bytes = 0_usize;
        for _ in 0..MAX_RUN_SUMMARY_PAGES {
            // #422 QR-2: a run's status reads its rows only, so its pages ask for no run
            // recovery.
            let mut request =
                RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, cursor.clone())
                    .map_err(|_| invalid(ClientCode::RunStatusPageRequestInvalid))?
                    .without_run_recovery();
            if cursor.is_none()
                && let Some(snapshot) = snapshot
            {
                request = request
                    .at_snapshot(snapshot)
                    .map_err(|_| invalid(ClientCode::RunStatusPageRequestInvalid))?;
            }
            let page =
                self.query_event_page(query.clone(), ProjectionProfile::Forensic, request)?;
            let page_position = page.snapshot_ledger_position();
            if position.is_some_and(|expected| expected != page_position) {
                return Err(invalid(ClientCode::RunStatusSnapshotChanged));
            }
            position = Some(page_position);
            for event in page.events() {
                if event.sequence <= last_sequence || !event_ids.insert(event.event_id) {
                    return Err(invalid(ClientCode::RunStatusPaginationInvalid));
                }
                if events.len() == MAX_RUN_SUMMARY_EVENTS {
                    return Err(invalid(ClientCode::RunStatusEventLimitExceeded));
                }
                let event_bytes = serde_json::to_vec(event)
                    .map_err(|_| invalid(ClientCode::RunStatusEventEncodeFailed))?;
                resident_bytes = resident_bytes
                    .checked_add(event_bytes.len())
                    .filter(|total| *total <= MAX_RUN_SUMMARY_RESIDENT_BYTES)
                    .ok_or_else(|| invalid(ClientCode::RunStatusResidentLimitExceeded))?;
                last_sequence = event.sequence;
                events.push(event.clone());
            }
            if !page.has_more() {
                return Ok(StatusPages {
                    events,
                    snapshot: page_position,
                    complete: true,
                });
            }
            if events.len() == MAX_RUN_SUMMARY_EVENTS {
                return Err(invalid(ClientCode::RunStatusEventLimitExceeded));
            }
            let next = page
                .next_cursor()
                .cloned()
                .ok_or_else(|| invalid(ClientCode::RunStatusPaginationInvalid))?;
            let previous_sequence = cursor.as_ref().map_or(0, |value| value.after_sequence());
            // #411 A51: a page the Runtime's read budget cut goes on after the position its
            // read reached, at or after its last row.
            if next.after_sequence() < last_sequence || next.after_sequence() <= previous_sequence {
                return Err(invalid(ClientCode::RunStatusPaginationInvalid));
            }
            cursor = Some(next);
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok(StatusPages {
                    events,
                    snapshot: page_position,
                    complete: false,
                });
            }
        }
        Err(invalid(ClientCode::RunStatusPageLimitExceeded))
    }
}

impl ContainedRunStatus {
    fn not_found(request_id: Option<RequestId>, run_id: Option<RunId>, snapshot: u64) -> Self {
        Self {
            schema_version: RUN_STATUS_SCHEMA,
            request_id,
            correlation_id: None,
            run_id,
            task_id: None,
            instance_id: None,
            dispatch: RunDispatch::Unknown,
            origin: RunOrigin::Unknown,
            package_label: None,
            recovery_package_labels: Vec::new(),
            state: ContainedRunState::NotFound,
            terminal: None,
            lease: None,
            progress: None,
            evidence: ContainedRunEvidence {
                snapshot_ledger_position: snapshot,
                admitted_sequence: None,
                restarted_after_admission: false,
            },
        }
    }
}

fn status_error(code: impl Into<Code>) -> RuntimeClientError {
    RuntimeClientError::fatal(code, RUN_STATUS_OPERATION)
}

/// The request's origin event: a client command is manual, the scheduler's receipt scheduled.
fn request_origin(event: &ProjectedEvent) -> Option<(RunDispatch, RunOrigin)> {
    match event.event_type {
        EventType::CliCommand => Some((RunDispatch::Manual, RunOrigin::Cli)),
        EventType::UiAction => Some((RunDispatch::Manual, RunOrigin::Ui)),
        EventType::LabRequest => Some((RunDispatch::Manual, RunOrigin::Lab)),
        EventType::CommandReceived if event.origin.source() == EventSource::Scheduler => {
            Some((RunDispatch::Scheduled, RunOrigin::Scheduler))
        }
        _ => None,
    }
}

fn run_fact_identity(event: &ProjectedEvent) -> RuntimeClientResult<Option<TaskRunIdentity>> {
    let payload = full_payload(event)?;
    if !matches!(payload, EventPayload::Task(TaskPayload::Semantic(_))) {
        return Ok(None);
    }
    TaskRunIdentity::from_semantic_event(payload, &event.links)
        .map(Some)
        .ok_or_else(|| status_error(ClientCode::RunStatusIdentityMissing))
}

/// The one value of a link over the run's events; two different values fail.
fn single_link<T: Copy + PartialEq>(
    events: &[ProjectedEvent],
    link: fn(&EventLinks) -> Option<&T>,
) -> RuntimeClientResult<Option<T>> {
    let mut found = None;
    for value in events.iter().filter_map(|event| link(&event.links)) {
        match found {
            Some(existing) if existing != *value => {
                return Err(status_error(ClientCode::RunStatusIdentityMismatch));
            }
            _ => found = Some(*value),
        }
    }
    Ok(found)
}

fn is_lease_terminal(event: &ProjectedEvent, lease_id: LeaseId) -> bool {
    matches!(
        event.event_type,
        EventType::LeaseReleased | EventType::LeaseExpired
    ) && event.links.lease_id() == Some(&lease_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Workflow #336 PKG-S2 (M-PKG v1.1 §4.3 S2-T3): run status names the run's packages by
    // `package_id` and carries no package digest.
    #[test]
    fn s2_t3_run_status_names_its_packages_by_label_without_a_digest() {
        let mut status = ContainedRunStatus::not_found(None, None, 1);
        status.package_label = Some("neutral.main".to_owned());
        status.recovery_package_labels =
            vec!["neutral.outer".to_owned(), "neutral.inner".to_owned()];
        let value = serde_json::to_value(&status).expect("run status JSON");
        assert_eq!(value["package_label"], "neutral.main");
        assert_eq!(
            value["recovery_package_labels"],
            serde_json::json!(["neutral.outer", "neutral.inner"])
        );
        for member in [
            "package_ref",
            "recovery_packages",
            "package_sha256",
            "package_digest",
        ] {
            assert!(value.get(member).is_none(), "{member}");
        }
    }
}
