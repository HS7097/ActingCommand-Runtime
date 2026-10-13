// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #391 item 3 (MCP-391c; FCM T-07; MCP-0 command table §4.23). After many Lab
//! operations on one instance, `ac_overview` read that instance's newest run as incomplete and
//! `ac_diagnose` failed, both with `run_status_event_limit_exceeded`: `recent_runs` read every
//! event the instance recorded in the window. The required state: an instance's recent runs
//! are read by run, so however many other events the instance records in the window, its runs
//! come back, complete. A scripted Runtime answers Health and every event query from one
//! in-memory ledger.

use super::{
    RejectProjectionSecrets, client_with_timeout, scripted_runtime, write_scripted_result,
};
use crate::ContainedRunState;
use crate::ipc::DEFAULT_RUNTIME_MAX_FRAME_BYTES;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    AuditInput, CapturePayloadDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventQuery, EventSeverity, EventSource,
    IdentifierIssuer, OriginModule, OwnerEpoch, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeOperation, RuntimeRequest, RuntimeResult,
    TaskEntryRecognitionPhase, TaskEntryTargetDisposition, TaskOutcome, TaskPayloadDraft,
    TaskSemanticFact,
};
use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use tempfile::TempDir;

/// The instance's Lab captures in the window: more events than one read of the instance's
/// window may keep (16 384, `MAX_RUN_SUMMARY_EVENTS`).
const LAB_CAPTURES: u64 = 16_385;
/// The window's start; every event of the ledger is recorded after it.
const SINCE_UNIX_MS: u64 = 1_800_000_000_000;
/// The entry page the run did not find.
const ENTRY_PAGE: &str = "neutral/entry";
/// One exchange carries a page of up to 256 events over loopback.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn gate_recent_runs_of_an_instance_with_more_lab_events_than_one_read_keeps_come_back_complete() {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = issuer.mint_instance_id().expect("instance id");
    let request = issuer.mint_request_id().expect("request id");
    let run_links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"));
    let failure = SharedCode::ContainedTaskHomeEntryNotMatched.as_str();
    // One run whose entry page did not match ends failed before its package is admitted; then
    // the Lab works on the same instance.
    let run_facts = [
        // Workflow #373 NAV-3b: the entry's pages as a set, and no matched page.
        TaskSemanticFact::EntryRecognition {
            phase: TaskEntryRecognitionPhase::Initial,
            required_pages: vec![ENTRY_PAGE.to_owned()],
            matched_page: None,
            matched: false,
        },
        TaskSemanticFact::EntryTargetDisposition {
            disposition: TaskEntryTargetDisposition::FailClosed,
            failure_code: Some(failure.to_owned()),
        },
        TaskSemanticFact::TerminalCommitted {
            outcome: TaskOutcome::Failure,
            final_page: None,
            executed_steps: Some(0),
            failure_code: Some(failure.to_owned()),
            scheduling_disposition: None,
            task_timing: None,
        },
    ];
    let runtime_origin = EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    );
    let lab_origin = EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab);
    let lab_links = EventLinksDraft::default().with_instance_id(instance);
    let mut events = Vec::new();
    for fact in run_facts {
        let sequence = next_sequence(&events);
        events.push(projected(
            &issuer,
            sequence,
            runtime_origin.clone(),
            run_links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        ));
    }
    for _ in 0..LAB_CAPTURES {
        let sequence = next_sequence(&events);
        events.push(projected(
            &issuer,
            sequence,
            lab_origin.clone(),
            lab_links.clone(),
            CapturePayloadDraft::completed(
                EventAction::CaptureObserve,
                EffectDisposition::NotPerformed,
                16,
                9,
                AuditInput::new(),
            )
            .into(),
        ));
    }

    let root = TempDir::new().expect("tempdir");
    let server = scripted_runtime(&root, move |listener, owner_epoch| {
        serve_ledger(listener, owner_epoch, &events);
    });
    let client = client_with_timeout(&root, IO_TIMEOUT);
    // ac_overview reads the newest run of each instance, ac_diagnose its 10 most recent.
    for limit in [1, 10] {
        let recent = client
            .recent_runs(*instance.transport(), SINCE_UNIX_MS, limit)
            .unwrap_or_else(|error| {
                panic!(
                    "the recent runs (limit {limit}) of an instance with {LAB_CAPTURES} Lab captures in the window: {error}"
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
            vec![(Some(*request.transport()), ContainedRunState::Failed)],
            "the recent runs (limit {limit}) are not the instance's one failed run"
        );
    }
    drop(client);
    server.join().expect("scripted ledger");
}

fn next_sequence(events: &[ProjectedEvent]) -> u64 {
    u64::try_from(events.len()).expect("ledger length") + 1
}

/// One event at `sequence`, recorded at `SINCE_UNIX_MS + sequence`, as a Forensic read projects
/// it.
fn projected(
    issuer: &IdentifierIssuer,
    sequence: u64,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> ProjectedEvent {
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        SINCE_UNIX_MS + sequence,
        EventSeverity::Info,
        origin,
        links,
        payload,
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
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => RuntimeResult::EventPage {
                page: event_page(events, query, *profile, page),
            },
            _ => panic!("the scripted ledger answers Health and QueryEvents only"),
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

/// Whether `event` matches `query`. This ledger serves the time and sequence bounds, one event
/// type and the run links; a query with any other filter is refused.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let unserved = EventQuery {
        from_timestamp_unix_ms: None,
        to_timestamp_unix_ms: None,
        from_sequence: None,
        to_sequence: None,
        event_type: None,
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
