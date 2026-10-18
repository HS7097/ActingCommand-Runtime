// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A6. On busy instances `ac_overview` answered `run_status_event_limit_exceeded`
//! during `recent_runs` (operation `recent_runs`, the instance rows `run_incomplete`), and no
//! newest run: the listing of the window's runs (Workflow #391 item 3) read every entry fact
//! of the instance's window into one read, and a day of runs whose entry gates navigate records
//! more entry facts than one read keeps (16 384, `MAX_RUN_SUMMARY_EVENTS`). The required state:
//! the listing keeps none of the facts it lists by, so the instance's newest runs come back
//! complete, newest first. A scripted Runtime answers Health and every event query from one
//! in-memory ledger.

use super::{
    RejectProjectionSecrets, client_with_timeout, scripted_runtime, write_scripted_result,
};
use crate::ContainedRunState;
use crate::ipc::DEFAULT_RUNTIME_MAX_FRAME_BYTES;
use actingcommand_contract::{
    AuditInput, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity,
    EventSource, IdentifierIssuer, InstanceId, OriginModule, OwnerEpoch, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, RequestId, RuntimeEventQueryCursor,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeOperation, RuntimeRequest,
    RuntimeResult, TaskEntryRecognitionPhase, TaskEntryTargetDisposition, TaskOutcome,
    TaskPayloadDraft, TaskSemanticFact,
};
use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use tempfile::TempDir;

/// The window's start; every event of the ledger is recorded after it.
const SINCE_UNIX_MS: u64 = 1_800_000_000_000;
/// The run's entry page, which its entry gate reaches over `HOP_PAGES`.
const ENTRY_PAGE: &str = "neutral/entry_page";
/// The pages of the gate's navigation hops, one edge package each.
const HOP_PAGES: [&str; 4] = [
    "neutral/page_a",
    "neutral/page_b",
    "neutral/page_c",
    "neutral/home",
];
const EDGE_PACKAGE: &str = "neutral.edge";
const MAIN_PACKAGE: &str = "neutral.main";
const TASK_LABEL: &str = "neutral.task";
const STEP: &str = "step_a";
/// Each run's entry facts (`task.entry_preflight`): its first recognition and recovery
/// decision, three per hop (the edge's admission and completion, the hop page's check), its
/// recognition after the hops and its disposition.
const ENTRY_FACTS_PER_RUN: usize = 2 + 3 * HOP_PAGES.len() + 2;
/// The window's runs: together they record more entry facts than one read keeps.
const RUNS: usize = 16_384 / ENTRY_FACTS_PER_RUN + 1;
/// One exchange carries a page of up to 256 events over loopback.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// A day of one instance: `RUNS` runs one after another, each navigating to its entry page,
/// then admitted, one step, succeeded. The instance and each run's request, oldest first, and
/// the ledger.
fn busy_window() -> (InstanceId, Vec<RequestId>, Vec<ProjectedEvent>) {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = issuer.mint_instance_id().expect("instance id");
    let origin = EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    );
    let mut requests = Vec::with_capacity(RUNS);
    let mut events = Vec::new();
    for _ in 0..RUNS {
        let request = issuer.mint_request_id().expect("request id");
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(request)
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(issuer.mint_task_id().expect("task id"))
            .with_run_id(issuer.mint_run_id().expect("run id"));
        for fact in run_facts() {
            let sequence = next_sequence(&events);
            events.push(projected(
                &issuer,
                sequence,
                origin.clone(),
                links.clone(),
                fact,
            ));
        }
        requests.push(*request.transport());
    }
    (*instance.transport(), requests, events)
}

/// One run's facts in the order its Runtime records them.
fn run_facts() -> Vec<TaskSemanticFact> {
    let mut facts = vec![
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_pages: vec![ENTRY_PAGE.to_owned()],
            matched_page: None,
            matched: false,
        },
        TaskSemanticFact::EntryRecoveryDecision { required: true },
    ];
    for page in HOP_PAGES {
        facts.extend([
            TaskSemanticFact::EntryRecoveryPackageAdmitted {
                package_label: EDGE_PACKAGE.to_owned(),
                role: None,
            },
            TaskSemanticFact::EntryRecoveryCompleted {
                package_label: EDGE_PACKAGE.to_owned(),
                final_page: page.to_owned(),
                executed_steps: 1,
                role: None,
            },
            TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::NavigationTarget,
                required_pages: vec![page.to_owned()],
                matched_page: Some(page.to_owned()),
                matched: true,
            },
        ]);
    }
    facts.extend([
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::PostRecovery,
            required_pages: vec![ENTRY_PAGE.to_owned()],
            matched_page: Some(ENTRY_PAGE.to_owned()),
            matched: true,
        },
        TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::Started,
            outcome: None,
        },
        TaskSemanticFact::PackageAdmitted {
            package_label: MAIN_PACKAGE.to_owned(),
            task_label: TASK_LABEL.to_owned(),
            response_deadline_monotonic_ms: None,
        },
        TaskSemanticFact::StepStarted {
            step_index: 0,
            operation_label: STEP.to_owned(),
            from_page: ENTRY_PAGE.to_owned(),
            phase: None,
        },
        TaskSemanticFact::StepFinished {
            step_index: 0,
            operation_label: STEP.to_owned(),
            page_label: ENTRY_PAGE.to_owned(),
            phase: None,
        },
        TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some(ENTRY_PAGE.to_owned()),
            executed_steps: Some(1),
            outcome: None,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        },
    ]);
    facts
}

#[test]
fn a6_recent_runs_of_an_instance_with_more_entry_facts_than_one_read_keeps_come_back_complete() {
    let (instance, requests, events) = busy_window();
    // The newest runs first.
    let newest = requests
        .iter()
        .rev()
        .map(|request| (Some(*request), ContainedRunState::Succeeded))
        .collect::<Vec<_>>();
    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        serve_ledger(listener, owner_epoch, &events);
    });
    let client = client_with_timeout(&root, IO_TIMEOUT);
    // ac_overview reads the newest run of each instance, ac_diagnose its 10 most recent.
    for limit in [1, 10] {
        let recent = client
            .recent_runs(instance, SINCE_UNIX_MS, limit)
            .unwrap_or_else(|error| {
                panic!(
                    "the recent runs (limit {limit}) of an instance with {RUNS} runs in the window: {error}"
                )
            });
        assert!(
            !recent.incomplete,
            "the recent runs (limit {limit}) are incomplete"
        );
        assert_eq!(
            recent
                .runs
                .iter()
                .map(|run| (run.request_id, run.state))
                .collect::<Vec<_>>(),
            newest[..limit].to_vec(),
            "the recent runs (limit {limit}) are not the instance's newest runs, newest first"
        );
    }
    drop(client);
    server.join().expect("scripted ledger");
}

fn next_sequence(events: &[ProjectedEvent]) -> u64 {
    u64::try_from(events.len()).expect("ledger length") + 1
}

/// The fact at `sequence`, recorded at `SINCE_UNIX_MS + sequence`, as a Forensic read projects
/// it.
fn projected(
    issuer: &IdentifierIssuer,
    sequence: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    fact: TaskSemanticFact,
) -> ProjectedEvent {
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        SINCE_UNIX_MS + sequence,
        EventSeverity::Info,
        origin,
        links,
        TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
    )
    .sanitize(&RejectProjectionSecrets)
    .expect("sanitize a scripted ledger event");
    ProjectedEvent {
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
    }
}

/// Answers Health and every event query of one client from `events` until it hangs up.
fn serve_ledger(listener: TcpListener, owner_epoch: OwnerEpoch, events: &[ProjectedEvent]) {
    let (mut stream, _) = listener.accept().expect("accept the client");
    while let Some(request) = next_request(&mut stream) {
        let result = match request.operation() {
            RuntimeOperation::Health => RuntimeResult::Health { owner_epoch },
            // Workflow #389 U-2: the handshake's BindChannel.
            RuntimeOperation::BindChannel { channel, via } => RuntimeResult::ChannelBound {
                channel: *channel,
                via: *via,
            },
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => RuntimeResult::EventPage {
                page: event_page(events, query, *profile, page),
            },
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
/// ledger's one snapshot (its last sequence).
fn event_page(
    events: &[ProjectedEvent],
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
) -> RuntimeEventQueryPage {
    assert_served(query);
    let snapshot = events.last().map_or(0, |event| event.sequence);
    if let Some(position) = request.snapshot_position() {
        assert_eq!(position, snapshot, "a read at another snapshot");
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
        .cloned()
        .collect::<Vec<_>>();
    let has_more = matching.next().is_some();
    let next_cursor = rows.last().filter(|_| has_more).map(|last| {
        RuntimeEventQueryCursor::new(snapshot, last.sequence, query, profile).expect("next cursor")
    });
    RuntimeEventQueryPage::new(rows, snapshot, request.limit(), has_more, next_cursor)
        .expect("event page")
}

/// This ledger serves the time and sequence bounds, one event type or a set of them and the
/// run links; a query with any other filter is refused.
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

/// Whether `event` matches `query`'s served filters.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let links = &event.links;
    query
        .from_timestamp_unix_ms
        .is_none_or(|from| event.timestamp_unix_ms >= from)
        && query
            .to_timestamp_unix_ms
            .is_none_or(|to| event.timestamp_unix_ms <= to)
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
