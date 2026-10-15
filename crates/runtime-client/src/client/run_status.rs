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
const RECENT_RUNS_BUDGET: Duration = Duration::from_secs(20);
/// The facts a run is listed by (Workflow #391 item 3): its main admission, its entry facts and
/// its terminal. A run without an entry check starts with its main admission, a run with one
/// records an entry fact once that check is done, and a run whose first check fails ends with
/// its terminal; each records only a few of them. Listing by them reads a few events per run,
/// however many other events its instance records.
const RUN_LISTING_EVENT_TYPES: [EventType; 5] = [
    EventType::TaskRequested,
    EventType::TaskEntryPreflight,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
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

/// One run a listing found: its identity and its first listing fact in the window.
#[derive(Debug, Clone, Copy)]
struct ListedRun {
    identity: TaskRunIdentity,
    sequence: u64,
}

/// The runs of one window, each once, oldest first by their first listing fact there, all read
/// at `snapshot`. `complete` is false when the deadline ended a read before its last page.
struct RunListing {
    runs: Vec<ListedRun>,
    snapshot: u64,
    complete: bool,
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

    /// The most recent `limit` (1-10) contained runs first recorded on `instance` since
    /// `since_unix_ms`, each in brief mode. The runs of the window are listed first and then
    /// read one by one (Workflow #391 item 3), so the instance's other events are never read.
    /// The whole read has a 20 s budget, checked between reads; when it ends the read early the
    /// result is `incomplete`. Read-only.
    pub fn recent_runs(
        &self,
        instance: InstanceId,
        since_unix_ms: u64,
        limit: usize,
    ) -> RuntimeClientResult<RecentContainedRuns> {
        if limit == 0 || limit > MAX_RECENT_RUNS {
            return Err(RuntimeClientError::fatal(
                ClientCode::RecentRunsLimitInvalid,
                RECENT_RUNS_OPERATION,
            ));
        }
        let deadline = Instant::now()
            .checked_add(RECENT_RUNS_BUDGET)
            .ok_or_else(|| {
                RuntimeClientError::fatal(
                    ClientCode::RecentRunsBudgetInvalid,
                    RECENT_RUNS_OPERATION,
                )
            })?;
        let listing = self.list_runs(
            &EventQuery {
                instance_id: Some(instance),
                from_timestamp_unix_ms: Some(since_unix_ms),
                ..EventQuery::default()
            },
            None,
            Some(deadline),
        )?;
        let mut incomplete = !listing.complete;
        let mut runs = Vec::with_capacity(limit);
        for listed in listing.runs.iter().rev() {
            if Instant::now() >= deadline {
                incomplete = true;
                break;
            }
            match self.read_contained_run_status(
                RunKey::RequestId(listed.identity.request_id),
                RunStatusMode::Brief,
                Some(listing.snapshot),
                Some(deadline),
            )? {
                Some(read) => {
                    // A later fact inside the window does not make an older run recent.
                    if read
                        .first_fact_timestamp_unix_ms
                        .is_some_and(|first| first >= since_unix_ms)
                    {
                        runs.push(read.status);
                        if runs.len() == limit {
                            break;
                        }
                    }
                }
                None => {
                    incomplete = true;
                    break;
                }
            }
        }
        Ok(RecentContainedRuns {
            runs,
            incomplete,
            snapshot_ledger_position: listing.snapshot,
        })
    }

    /// One page of the contained runs first recorded from `since_unix_ms` to `until_unix_ms`
    /// (Workflow #389 MCP-3, `actingctl runs` / `ac_runs`). The runs of the window are listed by
    /// run, as `recent_runs` lists them, and examined newest first by their order sequence: each
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

    /// The runs `window` holds a listing fact of (Workflow #391 item 3), one query per listing
    /// fact type, all at `snapshot` or else at the first query's; each run once, under the
    /// identity checks of a run's own read. A later fact inside the window does not list an
    /// older run first: the order is by each run's first listing fact in the window.
    fn list_runs(
        &self,
        window: &EventQuery,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
    ) -> RuntimeClientResult<RunListing> {
        let mut listing = RunListing {
            runs: Vec::new(),
            snapshot: 0,
            complete: true,
        };
        let mut snapshot = snapshot;
        let mut listed = BTreeMap::<RequestId, usize>::new();
        let mut runs_seen = BTreeSet::new();
        for event_type in RUN_LISTING_EVENT_TYPES {
            let read = self.read_status_pages(
                EventQuery {
                    event_type: Some(event_type),
                    ..window.clone()
                },
                snapshot,
                deadline,
                RECENT_RUNS_OPERATION,
            )?;
            snapshot = Some(read.snapshot);
            listing.snapshot = read.snapshot;
            for event in &read.events {
                let Some(identity) = run_fact_identity(event)? else {
                    continue;
                };
                match listed.get(&identity.request_id) {
                    Some(index) => {
                        let run = &mut listing.runs[*index];
                        if run.identity != identity {
                            return Err(status_error(ClientCode::RunStatusIdentityMismatch));
                        }
                        run.sequence = run.sequence.min(event.sequence);
                    }
                    None => {
                        if !runs_seen.insert(identity.run_id) {
                            return Err(status_error(ClientCode::RunStatusIdentityAmbiguous));
                        }
                        listed.insert(identity.request_id, listing.runs.len());
                        listing.runs.push(ListedRun {
                            identity,
                            sequence: event.sequence,
                        });
                    }
                }
            }
            if !read.complete {
                listing.complete = false;
                break;
            }
        }
        listing.runs.sort_by_key(|run| run.sequence);
        Ok(listing)
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

    /// `None` only when `deadline` ended a paged read before its last page.
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
            None if admitted_sequence.is_none() && !step_started => ContainedRunState::Running,
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
        for event_type in [EventType::RuntimeStarted] {
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

    /// One bounded page at `snapshot`.
    fn first_status_events(
        &self,
        query: EventQuery,
        limit: u16,
        snapshot: u64,
    ) -> RuntimeClientResult<Vec<ProjectedEvent>> {
        let request = RuntimeEventQueryPageRequest::new(limit, None)
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
            let mut request =
                RuntimeEventQueryPageRequest::new(MAX_RUNTIME_EVENT_QUERY_EVENTS, cursor.clone())
                    .map_err(|_| invalid(ClientCode::RunStatusPageRequestInvalid))?;
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
            if next.after_sequence() != last_sequence || next.after_sequence() <= previous_sequence
            {
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
