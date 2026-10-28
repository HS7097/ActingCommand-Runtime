// SPDX-License-Identifier: AGPL-3.0-only

//! P32-A (#422 6468995196; design rd-1028/design-p32.md §1.5 and §2 A-1, A-2; 0.12 wave 2). On
//! v0.12.24 one busy instance's run listing took about 25 s and answered incomplete. The
//! required state, on a scripted Runtime that answers every event query from one in-memory
//! ledger, the time standing still (a deadline already reached or one no test reaches):
//! - A-1: a run's Runtime restart, lease and dispatch intent reads go on past a page the
//!   Runtime's read budget cut (`has_more`, no row, its cursor past the position its read
//!   reached); before, that page was the answer and read as "none". A deadline reached when
//!   such a page comes back makes the read answer incomplete (`window_run_status` and
//!   `window_run_catalog_task_id` answer `None`, as for a cut status read), never "none"; with
//!   the deadline unreached, or without one (`contained_run_status`), the read goes on to the
//!   record the cut page did not reach.
//! - A-2: the run listing asks its pages without payloads (`Concise`) and finds each fact's run
//!   from its links; on one ledger it lists the same runs, field by field (identity, order,
//!   settled, opened in the window, snapshot, complete), as the listing that asked `Forensic`
//!   and read each fact's payload, with and without an instance, from before the first run and
//!   from inside the runs. The Runtime's own task records of the listing types that link no
//!   instance (a Lab debug run's) are no run in either.

use super::{
    RejectProjectionSecrets, client_with_timeout, scripted_runtime, unreached_deadline,
    write_scripted_result,
};
use crate::ipc::DEFAULT_RUNTIME_MAX_FRAME_BYTES;
use crate::{ContainedRunState, RunKey, RunStatusMode, WindowRun};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, ContainedTaskLeaseTerminal,
    EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, EventType, IdentifierIssuer,
    InstanceId, IssuedInstanceId, LeasePayloadDraft, LedgerMaterialReadState, LedgerPageLimit,
    LedgerReadScope, LedgerReadSource, OriginModule, OwnerEpoch, PolicyDispatchEventData,
    PolicyPayloadDraft, PolicyReasonRecord, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RequestId, RuntimeErrorCode, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeOperation, RuntimePayloadDraft, RuntimeRequest,
    RuntimeResult, TaskEntryRecognitionPhase, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The ledger's first record is recorded at this time plus one, each later one a millisecond
/// after the one before.
const RECORDED_FROM_UNIX_MS: u64 = 1_800_000_000_000;
/// One exchange carries a page of up to 256 events over loopback.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const PACKAGE: &str = "neutral.package";
const TASK_LABEL: &str = "neutral.task";
const HOME: &str = "neutral/home";
const STEP: &str = "neutral.step";
/// The policy task the scheduler dispatched the cut-read run for: an opaque id.
const CATALOG_TASK: &str = "neutral.routine.collect";
/// The record types a run is listed by (`RUN_LISTING_EVENT_TYPES`).
const LISTING_TYPES: [EventType; 5] = [
    EventType::TaskRequested,
    EventType::TaskEntryPreflight,
    EventType::TaskCompleted,
    EventType::TaskFailed,
    EventType::TaskCancelled,
];

/// A deadline already reached when the client first checks it: `unreached_deadline` less the
/// hour it grants (the form of actingctl's `gate_422_idle_ov::spent_deadline`).
fn reached_deadline() -> Instant {
    unreached_deadline()
        .checked_sub(Duration::from_secs(60 * 60))
        .expect("the deadline unreached_deadline grants, less its hour")
}

// ---- The scripted ledger ---------------------------------------------------------------------

/// One run's links: its own (instance, request, correlation, task, run and lease), its
/// request's alone, its lease's alone, and its dispatch intent's (the run's instance,
/// correlation, task and run under the admission's request and action).
#[derive(Clone)]
struct RunLinks {
    request: RequestId,
    run: EventLinksDraft,
    request_only: EventLinksDraft,
    lease_only: EventLinksDraft,
    intent: EventLinksDraft,
}

/// The ledger's records in sequence order, as a Forensic read projects them.
struct Recorder {
    issuer: IdentifierIssuer,
    events: Vec<ProjectedEvent>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            issuer: IdentifierIssuer::new().expect("identifier issuer"),
            events: Vec::new(),
        }
    }

    fn next_sequence(&self) -> u64 {
        u64::try_from(self.events.len()).expect("ledger length") + 1
    }

    /// Records one event at the next sequence; its sequence.
    fn record(
        &mut self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> u64 {
        let sequence = self.next_sequence();
        let sanitized = EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            RECORDED_FROM_UNIX_MS + sequence,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&RejectProjectionSecrets)
        .expect("sanitize a scripted ledger event");
        self.events.push(ProjectedEvent {
            views: Vec::new(),
            artifact_evictions: Vec::new(),
            schema_version: sanitized.schema_version().to_owned(),
            sequence,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_owned(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts: Vec::new(),
        });
        sequence
    }

    fn mint_run(&self, instance: IssuedInstanceId) -> RunLinks {
        let issuer = &self.issuer;
        let request = issuer.mint_request_id().expect("request id");
        let correlation = issuer.mint_correlation_id().expect("correlation id");
        let task = issuer.mint_task_id().expect("task id");
        let run = issuer.mint_run_id().expect("run id");
        let lease = issuer.mint_lease_id().expect("lease id");
        RunLinks {
            request: *request.transport(),
            run: EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(request)
                .with_correlation_id(correlation)
                .with_task_id(task)
                .with_run_id(run)
                .with_lease_id(lease),
            request_only: EventLinksDraft::default().with_request_id(request),
            lease_only: EventLinksDraft::default().with_lease_id(lease),
            intent: EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(issuer.mint_request_id().expect("admission request id"))
                .with_correlation_id(correlation)
                .with_task_id(task)
                .with_run_id(run)
                .with_action_id(issuer.mint_action_id().expect("intent action id")),
        }
    }

    fn cli_command(&mut self, run: &RunLinks) -> u64 {
        self.record(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            run.request_only.clone(),
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )
    }

    fn fact(&mut self, run: &RunLinks, fact: TaskSemanticFact) -> u64 {
        self.record(
            runtime_origin(),
            run.run.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
    }

    fn entry_fact(&mut self, run: &RunLinks) -> u64 {
        self.fact(
            run,
            TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::Initial,
                required_pages: vec![HOME.to_owned()],
                matched_page: Some(HOME.to_owned()),
                matched: true,
            },
        )
    }

    fn admitted(&mut self, run: &RunLinks) -> u64 {
        self.fact(
            run,
            TaskSemanticFact::PackageAdmitted {
                package_label: PACKAGE.to_owned(),
                task_label: TASK_LABEL.to_owned(),
                response_deadline_monotonic_ms: None,
            },
        )
    }

    fn step_started(&mut self, run: &RunLinks) -> u64 {
        self.fact(
            run,
            TaskSemanticFact::StepStarted {
                step_index: 0,
                operation_label: STEP.to_owned(),
                from_page: HOME.to_owned(),
                phase: None,
            },
        )
    }

    fn terminal(&mut self, run: &RunLinks, task_result: TaskOutcome) -> u64 {
        let outcome = match task_result {
            TaskOutcome::Success => None,
            TaskOutcome::Failure => Some(Outcome::new(SharedCode::ContainedTaskPageUnknown)),
            TaskOutcome::Cancelled => Some(Outcome::new(RuntimeErrorCode::ContainedTaskCancelled)),
        };
        self.fact(
            run,
            TaskSemanticFact::TerminalCommitted {
                task_result,
                final_page: Some(HOME.to_owned()),
                executed_steps: Some(1),
                outcome,
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
        )
    }

    fn runtime_started(&mut self) -> u64 {
        self.record(
            runtime_origin(),
            EventLinksDraft::default(),
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into(),
        )
    }
}

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

// ---- The scripted Runtime --------------------------------------------------------------------

/// One QueryEvents page the client asked for.
#[derive(Debug, Clone)]
struct Asked {
    query: EventQuery,
    profile: ProjectionProfile,
    first: bool,
}

/// Answers Health, BindChannel and every event query of one client from `events` until it hangs
/// up. The first page of a query for one of the `cut` event types is a page the Runtime's read
/// budget cut before the query's first record: no row, more to read, its cursor at the record
/// before that one. A Concise or Cli page leaves each payload out.
fn serve_ledger(
    listener: TcpListener,
    owner_epoch: OwnerEpoch,
    events: &[ProjectedEvent],
    cut: &[EventType],
    asked: &Mutex<Vec<Asked>>,
) {
    let (mut stream, _) = listener.accept().expect("accept the client");
    while let Some(request) = next_request(&mut stream) {
        let result = match request.operation() {
            RuntimeOperation::Health => RuntimeResult::Health { owner_epoch },
            RuntimeOperation::BindChannel { channel, via } => RuntimeResult::ChannelBound {
                channel: *channel,
                via: *via,
            },
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => {
                asked
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Asked {
                        query: query.clone(),
                        profile: *profile,
                        first: page.cursor().is_none(),
                    });
                RuntimeResult::EventPage {
                    page: event_page(events, query, *profile, page, cut),
                }
            }
            _ => panic!("the scripted ledger answers Health, BindChannel and QueryEvents only"),
        };
        write_scripted_result(&mut stream, &request, result);
    }
}

/// The next request, or `None` once the client hung up.
fn next_request(stream: &mut TcpStream) -> Option<RuntimeRequest> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
            ) =>
        {
            return None;
        }
        Err(error) => panic!("scripted ledger request header: {error}"),
    }
    let length = u32::from_be_bytes(header) as usize;
    assert!(
        length > 0 && length <= DEFAULT_RUNTIME_MAX_FRAME_BYTES,
        "scripted ledger request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("scripted ledger request body");
    Some(serde_json::from_slice(&body).expect("decode a request"))
}

/// One page of the events `query` matches after the request's cursor, oldest first, at the
/// ledger's one snapshot (its last sequence), each as `profile` projects it; the first page of
/// a `cut` type's query cut by the read budget before its first match.
fn event_page(
    events: &[ProjectedEvent],
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
    cut: &[EventType],
) -> RuntimeEventQueryPage {
    assert_served(query);
    let snapshot = events.last().map_or(0, |event| event.sequence);
    if let Some(position) = request.snapshot_position() {
        assert_eq!(position, snapshot, "a read at another snapshot");
    }
    if request.cursor().is_none() && query.event_type.is_some_and(|kind| cut.contains(&kind)) {
        let first = events
            .iter()
            .find(|event| matches(query, event))
            .map(|event| event.sequence)
            .expect("a cut query matches a record");
        let scanned = first - 1;
        assert!(scanned > 0, "a cut query's first match follows a record");
        let cursor = RuntimeEventQueryCursor::new(snapshot, scanned, query, profile)
            .expect("the cut page's cursor");
        return RuntimeEventQueryPage::projected(
            Vec::new(),
            snapshot,
            request.limit(),
            true,
            Some(cursor),
            LedgerReadScope {
                source: LedgerReadSource::Runtime,
                material_read: LedgerMaterialReadState::NotRequested,
                scanned_through_position: scanned,
                read_complete: false,
                limits: vec![LedgerPageLimit::ReadBudget],
            },
            Vec::new(),
        )
        .expect("a page the read budget cut");
    }
    let after = request
        .cursor()
        .map_or(0, RuntimeEventQueryCursor::after_sequence);
    let mut matching = events
        .iter()
        .filter(|event| event.sequence > after && matches(query, event));
    let rows = matching
        .by_ref()
        .take(usize::from(request.limit()))
        .map(|event| project(event, profile))
        .collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next_cursor = rows.last().filter(|_| has_more).map(|last| {
        RuntimeEventQueryCursor::new(snapshot, last.sequence, query, profile).expect("next cursor")
    });
    RuntimeEventQueryPage::new(rows, snapshot, request.limit(), has_more, next_cursor)
        .expect("event page")
}

/// The record as `profile` projects it: Concise and Cli leave the payload out.
fn project(event: &ProjectedEvent, profile: ProjectionProfile) -> ProjectedEvent {
    let mut projected = event.clone();
    if matches!(profile, ProjectionProfile::Concise | ProjectionProfile::Cli) {
        projected.payload = ProjectionPayload::Omitted;
    }
    projected
}

/// This ledger serves the time and sequence bounds, one event type or a set of them, the
/// instance and the run links; a query with any other filter is refused.
fn assert_served(query: &EventQuery) {
    let unserved = EventQuery {
        from_timestamp_unix_ms: None,
        to_timestamp_unix_ms: None,
        from_sequence: None,
        to_sequence: None,
        event_type: None,
        event_types: Vec::new(),
        instance_id: None,
        request_id: None,
        correlation_id: None,
        task_id: None,
        run_id: None,
        lease_id: None,
        ..query.clone()
    };
    assert_eq!(
        unserved,
        EventQuery::default(),
        "the scripted ledger serves no such filter"
    );
}

/// Whether `event` matches `query`'s served filters; the upper time bound is exclusive.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let links = &event.links;
    query
        .from_timestamp_unix_ms
        .is_none_or(|from| event.timestamp_unix_ms >= from)
        && query
            .to_timestamp_unix_ms
            .is_none_or(|to| event.timestamp_unix_ms < to)
        && query
            .from_sequence
            .is_none_or(|from| event.sequence >= from)
        && query.to_sequence.is_none_or(|to| event.sequence <= to)
        && query
            .event_type
            .is_none_or(|event_type| event.event_type == event_type)
        && (query.event_types.is_empty() || query.event_types.contains(&event.event_type))
        && query
            .instance_id
            .is_none_or(|id| links.instance_id() == Some(&id))
        && query
            .request_id
            .is_none_or(|id| links.request_id() == Some(&id))
        && query
            .correlation_id
            .is_none_or(|id| links.correlation_id() == Some(&id))
        && query.task_id.is_none_or(|id| links.task_id() == Some(&id))
        && query.run_id.is_none_or(|id| links.run_id() == Some(&id))
        && query
            .lease_id
            .is_none_or(|id| links.lease_id() == Some(&id))
}

/// The scripted Runtime of one ledger, a client of it, and the pages the client asked for.
struct Scripted {
    _root: TempDir,
    server: std::thread::JoinHandle<()>,
    client: crate::RuntimeClient,
    asked: Arc<Mutex<Vec<Asked>>>,
}

impl Scripted {
    /// The scripted Runtime of `events`, whose first pages of the `cut` types the read budget
    /// cuts, and a client of it.
    fn start(events: Vec<ProjectedEvent>, cut: Vec<EventType>) -> Self {
        let root = TempDir::new().expect("tempdir");
        let asked = Arc::new(Mutex::new(Vec::new()));
        let served = Arc::clone(&asked);
        let server = scripted_runtime(&root, move |listener, owner_epoch| {
            serve_ledger(listener, owner_epoch, &events, &cut, &served);
        });
        let client = client_with_timeout(&root, IO_TIMEOUT);
        Self {
            _root: root,
            server,
            client,
            asked,
        }
    }

    /// How many pages the client asked for so far.
    fn asked_count(&self) -> usize {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// The pages the client asked for, from the `from`-th on.
    fn asked_since(&self, from: usize) -> Vec<Asked> {
        self.asked.lock().unwrap_or_else(PoisonError::into_inner)[from..].to_vec()
    }

    /// The client hangs up and the scripted Runtime ends.
    fn finish(self) {
        drop(self.client);
        self.server.join().expect("scripted ledger");
    }
}

// ---- A-1: a cut status read answers incomplete, not "none" -----------------------------------

/// A scheduled run whose Runtime restarted after its admission, whose lease a record that does
/// not link its request released, after an unrelated Runtime start before it: the ledger, the
/// run's instance and request.
fn interrupted_scheduled_run() -> (Vec<ProjectedEvent>, InstanceId, RequestId) {
    let mut ledger = Recorder::new();
    let instance = ledger.issuer.mint_instance_id().expect("instance id");
    ledger.runtime_started();
    let run = ledger.mint_run(instance);
    // The dispatch intent links the run's correlation, task and run under the admission's
    // request.
    ledger.record(
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        ),
        run.intent.clone(),
        PolicyPayloadDraft::dispatch_intent(dispatch_intent(), AuditInput::new()).into(),
    );
    ledger.record(
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
        ),
        run.request_only.clone(),
        CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
    );
    ledger.entry_fact(&run);
    ledger.admitted(&run);
    ledger.step_started(&run);
    ledger.runtime_started();
    let lease_only = run.lease_only.clone();
    ledger.record(
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
        ),
        lease_only,
        LeasePayloadDraft::released(
            EventAction::LeaseRelease,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
        .into(),
    );
    (ledger.events, *instance.transport(), run.request)
}

/// The scheduler's dispatch intent for `CATALOG_TASK`. Only its task_id is read; the other
/// members hold neutral values the contract accepts.
fn dispatch_intent() -> PolicyDispatchEventData {
    let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
    PolicyDispatchEventData {
        decision_id: "decision.p32a.scheduled".to_owned(),
        task_id: CATALOG_TASK.to_owned(),
        instance_id: "neutral.p32a.alias".to_owned(),
        operation_id: "operation.p32a.scheduled".to_owned(),
        procedure_binding_digest: placeholder('b'),
        reason_chain_id: "reasons.p32a.scheduled".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: placeholder('c'),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "facts.p32a.scheduled".to_owned(),
        approval_fact_ids: Vec::new(),
        urgency_milli: 0,
    }
}

/// The one run the instance's window lists, and the listing's snapshot.
fn listed_run(client: &crate::RuntimeClient, instance: InstanceId) -> (WindowRun, u64) {
    let listing = client
        .list_window_runs(Some(instance), RECORDED_FROM_UNIX_MS, unreached_deadline())
        .expect("the window's listing");
    assert!(listing.complete, "the listing is complete: {listing:?}");
    let [run] = listing.runs.as_slice() else {
        panic!("one run in the window: {listing:?}");
    };
    (*run, listing.snapshot_ledger_position)
}

/// The pages of `event_type` among `asked`: whether each was a first page.
fn pages_of(asked: &[Asked], event_type: EventType) -> Vec<bool> {
    asked
        .iter()
        .filter(|page| page.query.event_type == Some(event_type))
        .map(|page| page.first)
        .collect()
}

#[test]
fn p32_a1_a_restart_read_the_budget_cut_is_incomplete_at_the_deadline_and_read_on_before_it() {
    let (events, instance, request) = interrupted_scheduled_run();
    let scripted = Scripted::start(events, vec![EventType::RuntimeStarted]);
    let client = &scripted.client;
    let (run, snapshot) = listed_run(client, instance);
    assert_eq!(run.identity.request_id, request);

    // At the deadline the cut restart read leaves the run unread: incomplete, not "running".
    let from = scripted.asked_count();
    let cut = client
        .window_run_status(&run, snapshot, reached_deadline())
        .expect("the status read at the deadline");
    assert!(
        cut.is_none(),
        "a restart read the budget cut, at the deadline, is incomplete: {cut:?}"
    );
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::RuntimeStarted),
        vec![true],
        "the cut page, then no page after the deadline"
    );

    // Before the deadline the read goes on past the cut page to the restart.
    let from = scripted.asked_count();
    let read = client
        .window_run_status(&run, snapshot, unreached_deadline())
        .expect("the status read")
        .expect("a status read before the deadline");
    assert_eq!(
        read.status.state,
        ContainedRunState::InterruptedUnterminated,
        "the restart the cut page did not reach: {read:?}"
    );
    assert!(read.status.evidence.restarted_after_admission, "{read:?}");
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::RuntimeStarted),
        vec![true, false, true, false],
        "since the run's first fact and since its admission: the cut page, then the page after it"
    );

    // Without a deadline (`ac_get_run`) the read goes on as well.
    let status = client
        .contained_run_status(RunKey::RequestId(request), RunStatusMode::Brief)
        .expect("the run's status");
    assert_eq!(status.state, ContainedRunState::InterruptedUnterminated);
    scripted.finish();
}

#[test]
fn p32_a1_a_lease_read_the_budget_cut_is_incomplete_at_the_deadline_and_read_on_before_it() {
    let (events, instance, request) = interrupted_scheduled_run();
    let scripted = Scripted::start(events, vec![EventType::LeaseReleased]);
    let client = &scripted.client;
    let (run, snapshot) = listed_run(client, instance);

    // At the deadline the cut lease read leaves the run unread, not "no lease terminal".
    let from = scripted.asked_count();
    let cut = client
        .window_run_status(&run, snapshot, reached_deadline())
        .expect("the status read at the deadline");
    assert!(
        cut.is_none(),
        "a lease read the budget cut, at the deadline, is incomplete: {cut:?}"
    );
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::LeaseReleased),
        vec![true],
        "the cut page, then no page after the deadline"
    );

    // Before the deadline the read goes on past the cut page to the release.
    let from = scripted.asked_count();
    let read = client
        .window_run_status(&run, snapshot, unreached_deadline())
        .expect("the status read")
        .expect("a status read before the deadline");
    assert_eq!(
        read.status.lease.and_then(|lease| lease.terminal),
        Some(ContainedTaskLeaseTerminal::Released),
        "the release the cut page did not reach: {read:?}"
    );
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::LeaseReleased),
        vec![true, false],
        "the cut page, then the page after it"
    );

    // Without a deadline (`ac_get_run`) the read goes on as well.
    let status = client
        .contained_run_status(RunKey::RequestId(request), RunStatusMode::Brief)
        .expect("the run's status");
    assert_eq!(
        status.lease.and_then(|lease| lease.terminal),
        Some(ContainedTaskLeaseTerminal::Released)
    );
    scripted.finish();
}

#[test]
fn p32_a1_a_dispatch_intent_read_the_budget_cut_is_incomplete_at_the_deadline_and_read_on() {
    let (events, instance, _) = interrupted_scheduled_run();
    let scripted = Scripted::start(events, vec![EventType::PolicyDispatchIntent]);
    let client = &scripted.client;
    let (run, snapshot) = listed_run(client, instance);

    // At the deadline the cut intent read is incomplete, not "dispatched by hand".
    let from = scripted.asked_count();
    let cut = client
        .window_run_catalog_task_id(&run, snapshot, reached_deadline())
        .expect("the intent read at the deadline");
    assert!(
        cut.is_none(),
        "an intent read the budget cut, at the deadline, is incomplete: {cut:?}"
    );
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::PolicyDispatchIntent),
        vec![true],
        "the cut page, then no page after the deadline"
    );

    // Before the deadline the read goes on past the cut page to the intent.
    let from = scripted.asked_count();
    let read = client
        .window_run_catalog_task_id(&run, snapshot, unreached_deadline())
        .expect("the intent read");
    assert_eq!(
        read,
        Some(Some(CATALOG_TASK.to_owned())),
        "the intent the cut page did not reach"
    );
    assert_eq!(
        pages_of(&scripted.asked_since(from), EventType::PolicyDispatchIntent),
        vec![true, false],
        "the cut page, then the page after it"
    );
    scripted.finish();
}

// ---- A-2: the listing without payloads lists what the listing with them listed ---------------

/// Two instances' runs one after another: succeeded, failed, cancelled, ended before its
/// admission, open, and one with two terminals; between them a Lab debug run's own
/// `task.requested` and `task.completed`, which link its request, correlation, task and run but
/// no instance, and records of other types. The ledger, the two instances, and the time inside
/// the runs from which the window holds the cancelled run's terminal but not its admission.
fn listing_ledger() -> (Vec<ProjectedEvent>, [InstanceId; 2], u64) {
    let mut ledger = Recorder::new();
    let a = ledger.issuer.mint_instance_id().expect("instance id");
    let b = ledger.issuer.mint_instance_id().expect("instance id");
    ledger.runtime_started();

    let succeeded = ledger.mint_run(a);
    ledger.cli_command(&succeeded);
    ledger.entry_fact(&succeeded);
    ledger.entry_fact(&succeeded);
    ledger.admitted(&succeeded);
    ledger.step_started(&succeeded);
    ledger.terminal(&succeeded, TaskOutcome::Success);

    let failed = ledger.mint_run(b);
    ledger.cli_command(&failed);
    ledger.entry_fact(&failed);
    ledger.admitted(&failed);
    ledger.step_started(&failed);
    ledger.terminal(&failed, TaskOutcome::Failure);

    // A Lab debug run: the Runtime's own records, no semantic fact and no instance.
    let lab_links = EventLinksDraft::default()
        .with_request_id(ledger.issuer.mint_request_id().expect("request id"))
        .with_correlation_id(ledger.issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(ledger.issuer.mint_task_id().expect("task id"))
        .with_run_id(ledger.issuer.mint_run_id().expect("run id"));
    ledger.record(
        runtime_origin(),
        lab_links.clone(),
        TaskPayloadDraft::requested(EventAction::RuntimeDebugPackage, AuditInput::new()).into(),
    );
    ledger.record(
        runtime_origin(),
        lab_links,
        TaskPayloadDraft::completed(
            EventAction::ArtifactExport,
            EffectDisposition::Performed,
            AuditInput::new(),
        )
        .into(),
    );

    let cancelled = ledger.mint_run(a);
    ledger.cli_command(&cancelled);
    ledger.entry_fact(&cancelled);
    let admitted = ledger.admitted(&cancelled);
    ledger.step_started(&cancelled);
    ledger.terminal(&cancelled, TaskOutcome::Cancelled);

    let before_admission = ledger.mint_run(b);
    ledger.cli_command(&before_admission);
    ledger.entry_fact(&before_admission);
    ledger.entry_fact(&before_admission);
    ledger.terminal(&before_admission, TaskOutcome::Failure);

    let twice = ledger.mint_run(b);
    ledger.cli_command(&twice);
    ledger.admitted(&twice);
    ledger.terminal(&twice, TaskOutcome::Success);
    ledger.terminal(&twice, TaskOutcome::Success);

    let open = ledger.mint_run(a);
    ledger.cli_command(&open);
    ledger.entry_fact(&open);
    ledger.admitted(&open);
    ledger.step_started(&open);
    ledger.runtime_started();

    let inside = RECORDED_FROM_UNIX_MS + admitted + 1;
    (ledger.events, [*a.transport(), *b.transport()], inside)
}

#[test]
fn p32_a2_the_listing_without_payloads_lists_the_runs_of_the_listing_with_them_field_by_field() {
    let (events, [a, b], inside) = listing_ledger();
    let scripted = Scripted::start(events, Vec::new());
    let client = &scripted.client;
    for (window, instance, since, runs) in [
        (
            "every instance, the whole history",
            None,
            RECORDED_FROM_UNIX_MS,
            6,
        ),
        (
            "instance A, the whole history",
            Some(a),
            RECORDED_FROM_UNIX_MS,
            3,
        ),
        (
            "instance B, the whole history",
            Some(b),
            RECORDED_FROM_UNIX_MS,
            3,
        ),
        ("every instance, from inside the runs", None, inside, 4),
        ("instance A, from inside the runs", Some(a), inside, 2),
    ] {
        let from = scripted.asked_count();
        let listed = client
            .list_window_runs(instance, since, unreached_deadline())
            .unwrap_or_else(|error| panic!("{window}: the listing: {error}"));
        let without = scripted.asked_since(from);
        let from = scripted.asked_count();
        let before = client
            .list_window_runs_from_payloads(instance, since, unreached_deadline())
            .unwrap_or_else(|error| panic!("{window}: the listing from payloads: {error}"));
        let with = scripted.asked_since(from);

        assert_eq!(listed, before, "{window}: the listings differ");
        assert!(listed.complete, "{window}: {listed:?}");
        assert_eq!(listed.runs.len(), runs, "{window}: {listed:?}");
        // One page per listing type: the listing asks its pages without payloads, the one
        // before A-2 asked them with.
        for (pages, profile) in [
            (&without, ProjectionProfile::Concise),
            (&with, ProjectionProfile::Forensic),
        ] {
            assert_eq!(
                pages
                    .iter()
                    .filter_map(|page| page.query.event_type)
                    .collect::<Vec<_>>(),
                LISTING_TYPES.to_vec(),
                "{window}: one page per listing type"
            );
            assert!(
                pages.iter().all(|page| page.profile == profile),
                "{window}: every page asked {profile:?}: {pages:?}"
            );
        }
    }

    // The listing shows what each run's facts in the window prove.
    let listing = client
        .list_window_runs(None, RECORDED_FROM_UNIX_MS, unreached_deadline())
        .expect("the listing");
    assert_eq!(
        listing
            .runs
            .iter()
            .map(|run| (run.settled, run.opened_in_window))
            .collect::<Vec<_>>(),
        vec![
            // Newest first: the open run, the one with two terminals, the one that ended
            // before its admission, the cancelled, the failed and the succeeded run.
            (None, true),
            (None, true),
            (None, false),
            (Some(ContainedRunState::Cancelled), true),
            (None, true),
            (Some(ContainedRunState::Succeeded), true),
        ],
        "{listing:?}"
    );
    let inside_listing = client
        .list_window_runs(Some(a), inside, unreached_deadline())
        .expect("the listing from inside the runs");
    assert_eq!(
        inside_listing
            .runs
            .iter()
            .map(|run| (run.settled, run.opened_in_window))
            .collect::<Vec<_>>(),
        vec![(None, true), (Some(ContainedRunState::Cancelled), false)],
        "the cancelled run is listed by its terminal alone: {inside_listing:?}"
    );
    scripted.finish();
}
