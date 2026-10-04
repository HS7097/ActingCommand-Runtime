// SPDX-License-Identifier: AGPL-3.0-only

//! Generic contained-run status (Workflow #338 R1): a client projection of one manual or
//! scheduled contained run read from its ledger events. It is read-only and never persisted.

use super::*;
use actingcommand_contract::{
    ContainedTaskLeaseTerminal, EventLinks, InstanceId, LeaseId, PackageRef,
};
use std::collections::VecDeque;

const RUN_STATUS_SCHEMA: &str = "actingcommand.run-status.v1";
const RUN_STATUS_OPERATION: &str = "contained_run_status";
const RECENT_RUNS_OPERATION: &str = "recent_runs";
const MAX_RECENT_RUNS: usize = 10;
const RECENT_RUNS_BUDGET: Duration = Duration::from_secs(20);

/// The step-level task events a brief read leaves out (`EventQuery::exclude_event_types`).
const BRIEF_EXCLUDED_EVENT_TYPES: [EventType; 10] = [
    EventType::TaskStepStarted,
    EventType::TaskStepFinished,
    EventType::TaskEffectIntent,
    EventType::TaskEffectCompleted,
    EventType::TaskRecognitionStarted,
    EventType::TaskRecognitionCompleted,
    EventType::TaskSelectionEvaluated,
    EventType::TaskEvidenceIndexed,
    EventType::TaskGeometryObserved,
    EventType::TaskEntryPreflight,
];

/// Selects one contained run by the request that submitted it or by its run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKey {
    RequestId(RequestId),
    RunId(RunId),
}

/// `Full` reads every event of the run; `Brief` leaves the step-level events out and returns no
/// progress.
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
    /// Admitted before a later Runtime start or takeover and never terminated; uncertain.
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
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_package_ref"
    )]
    pub package_ref: Option<PackageRef>,
    #[serde(serialize_with = "serialize_package_refs")]
    pub recovery_packages: Vec<PackageRef>,
    pub state: ContainedRunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ContainedRunTerminal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease: Option<ContainedRunLease>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ContainedRunProgress>,
    pub evidence: ContainedRunEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContainedRunTerminal {
    pub outcome: TaskOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
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

struct StatusPages {
    events: Vec<ProjectedEvent>,
    snapshot: u64,
    complete: bool,
}

impl RuntimeClient {
    /// Projects one contained run, manual or scheduled, from its ledger events (Workflow #338
    /// R1). A run admitted before a later Runtime start or takeover without a terminal is
    /// `interrupted_unterminated`. Read-only.
    pub fn contained_run_status(
        &self,
        key: RunKey,
        mode: RunStatusMode,
    ) -> RuntimeClientResult<ContainedRunStatus> {
        self.read_contained_run_status(key, mode, None, None)?
            .ok_or_else(|| status_error("run_status_read_incomplete"))
    }

    /// The most recent `limit` (1-10) contained runs admitted on `instance` since
    /// `since_unix_ms`, each in brief mode. The whole read has a 20 s budget, checked between
    /// reads; when it ends the read early the result is `incomplete`. Read-only.
    pub fn recent_runs(
        &self,
        instance: InstanceId,
        since_unix_ms: u64,
        limit: usize,
    ) -> RuntimeClientResult<RecentContainedRuns> {
        if limit == 0 || limit > MAX_RECENT_RUNS {
            return Err(RuntimeClientError::fatal(
                "recent_runs_limit_invalid",
                RECENT_RUNS_OPERATION,
            ));
        }
        let deadline = Instant::now()
            .checked_add(RECENT_RUNS_BUDGET)
            .ok_or_else(|| {
                RuntimeClientError::fatal("recent_runs_budget_invalid", RECENT_RUNS_OPERATION)
            })?;
        let listing = self.read_status_pages(
            EventQuery {
                instance_id: Some(instance),
                event_type: Some(EventType::TaskRequested),
                from_timestamp_unix_ms: Some(since_unix_ms),
                ..EventQuery::default()
            },
            None,
            Some(deadline),
            RECENT_RUNS_OPERATION,
        )?;
        let mut incomplete = !listing.complete;
        let mut recent = VecDeque::with_capacity(limit);
        for event in &listing.events {
            if package_admitted(event)?.is_none() {
                continue;
            }
            if recent.len() == limit {
                recent.pop_front();
            }
            recent.push_back(admitted_request_id(event)?);
        }
        let mut runs = Vec::with_capacity(recent.len());
        for request_id in recent.into_iter().rev() {
            if Instant::now() >= deadline {
                incomplete = true;
                break;
            }
            match self.read_contained_run_status(
                RunKey::RequestId(request_id),
                RunStatusMode::Brief,
                Some(listing.snapshot),
                Some(deadline),
            )? {
                Some(status) => runs.push(status),
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

    /// `None` only when `deadline` ended a paged read before its last page.
    fn read_contained_run_status(
        &self,
        key: RunKey,
        mode: RunStatusMode,
        snapshot: Option<u64>,
        deadline: Option<Instant>,
    ) -> RuntimeClientResult<Option<ContainedRunStatus>> {
        let (request_id, snapshot) = match key {
            RunKey::RequestId(request_id) => (request_id, snapshot),
            RunKey::RunId(run_id) => {
                let read = self.read_status_pages(
                    EventQuery {
                        run_id: Some(run_id),
                        event_type: Some(EventType::TaskRequested),
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
                    if package_admitted(event)?.is_some() {
                        let request_id = admitted_request_id(event)?;
                        if !request_ids.contains(&request_id) {
                            request_ids.push(request_id);
                        }
                    }
                }
                match request_ids.as_slice() {
                    [] => {
                        return Ok(Some(ContainedRunStatus::not_found(
                            None,
                            Some(run_id),
                            read.snapshot,
                        )));
                    }
                    [request_id] => (*request_id, Some(read.snapshot)),
                    _ => return Err(status_error("run_status_identity_ambiguous")),
                }
            }
        };
        let mut query = EventQuery {
            request_id: Some(request_id),
            ..EventQuery::default()
        };
        if mode == RunStatusMode::Brief {
            query.exclude_event_types = BRIEF_EXCLUDED_EVENT_TYPES.to_vec();
        }
        let read = self.read_status_pages(query, snapshot, deadline, RUN_STATUS_OPERATION)?;
        if !read.complete {
            return Ok(None);
        }
        let snapshot = read.snapshot;
        let events = read.events;
        if events.is_empty() {
            return Ok(Some(ContainedRunStatus::not_found(
                Some(request_id),
                None,
                snapshot,
            )));
        }

        let mut origin = None;
        let mut admitted = None;
        let mut terminal = None;
        let mut recovery_packages = Vec::new();
        let mut progress = None;
        let mut step_started = false;
        for event in &events {
            if let Some(found) = request_origin(event)
                && origin.replace(found).is_some()
            {
                return Err(status_error("run_status_origin_inconsistent"));
            }
            let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(event)? else {
                continue;
            };
            match payload.fact() {
                TaskSemanticFact::PackageAdmitted { package_sha256, .. } => {
                    let found = (
                        event.sequence,
                        package_sha256.clone(),
                        event.links.lease_id().copied(),
                    );
                    if admitted.replace(found).is_some() {
                        return Err(status_error("run_status_package_fact_count_invalid"));
                    }
                }
                TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } => {
                    recovery_packages.push(package_sha256.clone());
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
                    outcome,
                    final_page,
                    executed_steps,
                    failure_code,
                    ..
                } => {
                    let found = ContainedRunTerminal {
                        outcome: *outcome,
                        failure_code: failure_code.clone(),
                        final_page: final_page.clone(),
                        executed_steps: *executed_steps,
                        cancellation_reason: (*outcome == TaskOutcome::Cancelled).then_some(
                            ContainedTaskCancellationReason::from_failure_code(
                                failure_code.as_deref(),
                            ),
                        ),
                        sequence: event.sequence,
                        event_id: event.event_id,
                    };
                    if terminal.replace(found).is_some() {
                        return Err(status_error("run_status_terminal_state_inconsistent"));
                    }
                }
                _ => {}
            }
        }

        let run_id = single_link(&events, EventLinks::run_id)?;
        if let RunKey::RunId(expected) = key
            && run_id != Some(expected)
        {
            return Err(status_error("run_status_identity_mismatch"));
        }
        let task_id = single_link(&events, EventLinks::task_id)?;
        let instance_id = single_link(&events, EventLinks::instance_id)?;
        let correlation_id = single_link(&events, EventLinks::correlation_id)?;
        let admitted_sequence = admitted.as_ref().map(|(sequence, _, _)| *sequence);
        let restarted_after_admission = match admitted_sequence {
            Some(sequence) => self.runtime_restarted_since(sequence, snapshot)?,
            None => false,
        };
        let open = terminal.is_none() && admitted_sequence.is_some();
        if mode == RunStatusMode::Brief {
            step_started = open
                && !self
                    .first_status_events(
                        EventQuery {
                            request_id: Some(request_id),
                            event_type: Some(EventType::TaskStepStarted),
                            ..EventQuery::default()
                        },
                        1,
                        snapshot,
                    )?
                    .is_empty();
            let entry = self.read_status_pages(
                EventQuery {
                    request_id: Some(request_id),
                    event_type: Some(EventType::TaskEntryPreflight),
                    ..EventQuery::default()
                },
                Some(snapshot),
                deadline,
                RUN_STATUS_OPERATION,
            )?;
            if !entry.complete {
                return Ok(None);
            }
            for event in &entry.events {
                if let EventPayload::Task(TaskPayload::Semantic(payload)) = full_payload(event)?
                    && let TaskSemanticFact::EntryRecoveryPackageAdmitted { package_sha256 } =
                        payload.fact()
                {
                    recovery_packages.push(package_sha256.clone());
                }
            }
        }
        let lease = match admitted.as_ref().and_then(|(_, _, lease_id)| *lease_id) {
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
            None if restarted_after_admission => ContainedRunState::InterruptedUnterminated,
            None if step_started => ContainedRunState::Running,
            None => ContainedRunState::Admitted,
        };
        let (dispatch, origin) = origin.unwrap_or((RunDispatch::Unknown, RunOrigin::Unknown));
        Ok(Some(ContainedRunStatus {
            schema_version: RUN_STATUS_SCHEMA,
            request_id: Some(request_id),
            correlation_id,
            run_id,
            task_id,
            instance_id,
            dispatch,
            origin,
            package_ref: admitted.map(|(_, package, _)| package),
            recovery_packages,
            state,
            terminal,
            lease,
            progress,
            evidence: ContainedRunEvidence {
                snapshot_ledger_position: snapshot,
                admitted_sequence,
                restarted_after_admission,
            },
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
            _ => Err(status_error("run_status_lease_terminal_inconsistent")),
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
            .map_err(|_| status_error("run_status_page_request_invalid"))?;
        let page = self.query_event_page(query, ProjectionProfile::Forensic, request)?;
        if page.snapshot_ledger_position() != snapshot {
            return Err(status_error("run_status_snapshot_changed"));
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
        operation: &'static str,
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
                    .map_err(|_| invalid("run_status_page_request_invalid"))?;
            if cursor.is_none()
                && let Some(snapshot) = snapshot
            {
                request = request
                    .at_snapshot(snapshot)
                    .map_err(|_| invalid("run_status_page_request_invalid"))?;
            }
            let page =
                self.query_event_page(query.clone(), ProjectionProfile::Forensic, request)?;
            let page_position = page.snapshot_ledger_position();
            if position.is_some_and(|expected| expected != page_position) {
                return Err(invalid("run_status_snapshot_changed"));
            }
            position = Some(page_position);
            for event in page.events() {
                if event.sequence <= last_sequence || !event_ids.insert(event.event_id) {
                    return Err(invalid("run_status_pagination_invalid"));
                }
                if events.len() == MAX_RUN_SUMMARY_EVENTS {
                    return Err(invalid("run_status_event_limit_exceeded"));
                }
                let event_bytes = serde_json::to_vec(event)
                    .map_err(|_| invalid("run_status_event_encode_failed"))?;
                resident_bytes = resident_bytes
                    .checked_add(event_bytes.len())
                    .filter(|total| *total <= MAX_RUN_SUMMARY_RESIDENT_BYTES)
                    .ok_or_else(|| invalid("run_status_resident_limit_exceeded"))?;
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
                return Err(invalid("run_status_event_limit_exceeded"));
            }
            let next = page
                .next_cursor()
                .cloned()
                .ok_or_else(|| invalid("run_status_pagination_invalid"))?;
            let previous_sequence = cursor.as_ref().map_or(0, |value| value.after_sequence());
            if next.after_sequence() != last_sequence || next.after_sequence() <= previous_sequence
            {
                return Err(invalid("run_status_pagination_invalid"));
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
        Err(invalid("run_status_page_limit_exceeded"))
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
            package_ref: None,
            recovery_packages: Vec::new(),
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

const fn status_error(code: &'static str) -> RuntimeClientError {
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

/// Only a `PackageAdmitted` payload admits a run; a `TaskPayload::Requested` does not.
fn package_admitted(event: &ProjectedEvent) -> RuntimeClientResult<Option<&PackageRef>> {
    if event.event_type != EventType::TaskRequested {
        return Ok(None);
    }
    match full_payload(event)? {
        EventPayload::Task(TaskPayload::Semantic(payload)) => match payload.fact() {
            TaskSemanticFact::PackageAdmitted { package_sha256, .. } => Ok(Some(package_sha256)),
            _ => Ok(None),
        },
        _ => Ok(None),
    }
}

fn admitted_request_id(event: &ProjectedEvent) -> RuntimeClientResult<RequestId> {
    event
        .links
        .request_id()
        .copied()
        .ok_or_else(|| status_error("run_status_identity_missing"))
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
                return Err(status_error("run_status_identity_mismatch"));
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

fn serialize_package_ref<S: serde::Serializer>(
    value: &Option<PackageRef>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    value
        .as_ref()
        .map(PackageRef::prefixed_wire_value)
        .serialize(serializer)
}

fn serialize_package_refs<S: serde::Serializer>(
    values: &[PackageRef],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(values.iter().map(PackageRef::prefixed_wire_value))
}
