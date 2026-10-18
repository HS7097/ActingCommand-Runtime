// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #411 A6, part 2. A run's own status (`contained_run_status`, and each run that
//! `recent_runs` projects) read every record linked to the run: by its request, and, when a run
//! id is reversed to its request, by its run. A run's captures carry both links, so a long run
//! (minutes of capture waits) records more of them than one read keeps (16 384,
//! `MAX_RUN_SUMMARY_EVENTS`) and its status answered `run_status_event_limit_exceeded`. The
//! required state: both reads query only the record types the projection folds, so such a run
//! still projects its status, by its request and by its run alike. A scripted Runtime
//! (`test_support::ScriptedRuntime`) answers every event query from one in-memory ledger.

use super::{RejectProjectionSecrets, client_with_timeout};
use crate::test_support::{ScriptedConnection, ScriptedRuntime};
use crate::{ContainedRunProgress, ContainedRunState, RunKey, RunStatusMode};
use actingcommand_contract::{
    AuditInput, CapturePayloadDraft, EffectDisposition, EventAction, EventActor, EventDraft,
    EventLinksDraft, EventOrigin, EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer,
    InstanceId, OriginModule, ProjectedEvent, ProjectionPayload, RequestId, RunId, TaskOutcome,
    TaskPayloadDraft, TaskSemanticFact,
};
use std::time::Duration;
use tempfile::TempDir;

/// The window's start; every event of the ledger is recorded after it.
const SINCE_UNIX_MS: u64 = 1_800_000_000_000;
/// The run's captures: more records than one read keeps (16 384, `MAX_RUN_SUMMARY_EVENTS`).
const CAPTURES: usize = 16_385;
/// The page the run's one step waits on.
const PAGE: &str = "neutral/page_a";
const PACKAGE: &str = "neutral.main";
const TASK_LABEL: &str = "neutral.task";
const STEP: &str = "step_a";
/// One exchange carries a page of up to 256 events over loopback.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// One long run on one instance and its ledger.
struct LongRun {
    instance: InstanceId,
    request: RequestId,
    run: RunId,
    events: Vec<ProjectedEvent>,
}

/// A run whose one step waits on its page for minutes: admitted, its step started, then
/// `CAPTURES` captures linked to its request and its run, its step finished, succeeded.
fn long_run() -> LongRun {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = issuer.mint_instance_id().expect("instance id");
    let request = issuer.mint_request_id().expect("request id");
    let run = issuer.mint_run_id().expect("run id");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(request)
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(run);
    let mut events = Vec::with_capacity(CAPTURES + 4);
    let fact = |semantic: TaskSemanticFact| -> EventPayloadDraft {
        TaskPayloadDraft::semantic(semantic, AuditInput::new()).into()
    };
    let mut payloads: Vec<EventPayloadDraft> = vec![
        fact(TaskSemanticFact::PackageAdmitted {
            package_label: PACKAGE.to_owned(),
            task_label: TASK_LABEL.to_owned(),
            response_deadline_monotonic_ms: None,
        }),
        fact(TaskSemanticFact::StepStarted {
            step_index: 0,
            operation_label: STEP.to_owned(),
            from_page: PAGE.to_owned(),
            phase: None,
        }),
    ];
    payloads.extend((0..CAPTURES).map(|_| {
        CapturePayloadDraft::completed(
            EventAction::CaptureObserve,
            EffectDisposition::NotPerformed,
            16,
            9,
            AuditInput::new(),
        )
        .into()
    }));
    payloads.extend([
        fact(TaskSemanticFact::StepFinished {
            step_index: 0,
            operation_label: STEP.to_owned(),
            page_label: PAGE.to_owned(),
            phase: None,
        }),
        fact(TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some(PAGE.to_owned()),
            executed_steps: Some(1),
            outcome: None,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        }),
    ]);
    for payload in payloads {
        record(&issuer, &mut events, links.clone(), payload);
    }
    LongRun {
        instance: *instance.transport(),
        request: *request.transport(),
        run: *run.transport(),
        events,
    }
}

#[test]
fn a6_a_run_with_more_records_than_the_limit_still_projects_its_status() {
    let LongRun {
        instance,
        request,
        events,
        ..
    } = long_run();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), move |_| {
        vec![ScriptedConnection::new().ledger(events)]
    });
    let client = client_with_timeout(&root, IO_TIMEOUT);
    // MCP ac_get_run reads the run by its request.
    let status = client
        .contained_run_status(RunKey::RequestId(request), RunStatusMode::Full)
        .unwrap_or_else(|error| {
            panic!("the status of a run with {CAPTURES} captures, by its request: {error}")
        });
    assert_eq!(
        status.state,
        ContainedRunState::Succeeded,
        "the state of a run with {CAPTURES} captures"
    );
    assert_eq!(
        status.progress,
        Some(ContainedRunProgress {
            step_index: 0,
            operation_label: STEP.to_owned(),
            page: PAGE.to_owned(),
        }),
        "the last step of a run with {CAPTURES} captures"
    );
    // ac_overview reads the newest run of each instance, ac_diagnose its 10 most recent.
    for limit in [1, 10] {
        let recent = client
            .recent_runs(instance, SINCE_UNIX_MS, limit)
            .unwrap_or_else(|error| {
                panic!(
                    "the recent runs (limit {limit}) of an instance whose run has {CAPTURES} captures: {error}"
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
            vec![(Some(request), ContainedRunState::Succeeded)],
            "the recent runs (limit {limit}) are not the instance's one succeeded run"
        );
    }
    drop(client);
    runtime.finish();
}

#[test]
fn a6_the_run_id_reversal_reads_only_projected_types() {
    let LongRun {
        request,
        run,
        events,
        ..
    } = long_run();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), move |_| {
        vec![ScriptedConnection::new().ledger(events)]
    });
    let client = client_with_timeout(&root, IO_TIMEOUT);
    let by_request = client
        .contained_run_status(RunKey::RequestId(request), RunStatusMode::Full)
        .unwrap_or_else(|error| {
            panic!("the status of a run with {CAPTURES} captures, by its request: {error}")
        });
    // MCP ac_get_run and ac_stop_run find a run by its run id too.
    let by_run = client
        .contained_run_status(RunKey::RunId(run), RunStatusMode::Full)
        .unwrap_or_else(|error| {
            panic!("the status of a run with {CAPTURES} captures, by its run id: {error}")
        });
    assert_eq!(
        (by_run.run_id, by_run.state),
        (Some(run), ContainedRunState::Succeeded),
        "the run found by its run id"
    );
    assert_eq!(
        by_run, by_request,
        "the run found by its run id has another status than by its request"
    );
    drop(client);
    runtime.finish();
}

/// Records `payload` at the next sequence, at `SINCE_UNIX_MS + sequence`, as a Forensic read
/// projects it.
fn record(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) {
    let sequence = u64::try_from(events.len()).expect("ledger length") + 1;
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        SINCE_UNIX_MS + sequence,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        ),
        links,
        payload,
    )
    .sanitize(&RejectProjectionSecrets)
    .expect("sanitize a scripted ledger event");
    events.push(ProjectedEvent {
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
}
