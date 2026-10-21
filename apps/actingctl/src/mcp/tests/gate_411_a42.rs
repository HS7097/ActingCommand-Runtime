// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A42 (0.12.9 train; frozen by the coordinator 10-20 23:2x). On the production host a
//! nine-minute run's `ac_run_evidence` answered `runtime_event_query_response_too_large`, with
//! from_step and limit alike, and `ac_overview` answered `call_budget_exhausted` and
//! `run_incomplete` for two instances that had no run in 24 h (only Lab activity), so "no run"
//! could not be told from "not read". The required states:
//! - T1: a long run's evidence read page after page: every page fits the output budget and holds
//!   at least one step, a step that alone is over it is answered with its heavy part referenced
//!   (and says which), each QueryEvents answer stays under the Runtime's response limit, and the
//!   cursor walks every step once, in order.
//! - T2: `detail` `summary`: one page holds every step's summary of a run of realistic length,
//!   without its targets and frames, with its kind, timing, frame reference and the run's
//!   failure code on the step the run ended in.
//! - T3: an instance with no run in the window: its overview row says so with the window's
//!   bounds, the read finished within the budget, and it read no more of a long history before
//!   the window than of a short one.
//!
//! The Runtime is a fake on loopback that answers Health, BindChannel, Status and QueryEvents
//! from one in-memory ledger, as the ledger answers them: each page is cut to the response limit
//! (`MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES`, a lone record over it answering
//! `runtime_event_query_response_too_large`), and the records a query examines are those of the
//! smallest index it names, between its sequence bounds (the ledger's own index read,
//! `candidates_from`); a time bound selects nothing by itself.

use super::super::jobs::Jobs;
use super::super::run_evidence as tool;
use super::super::runtime::RuntimeAccess;
use super::super::tools::{self, ToolContext};
use super::gate_mcpt1::unreached_deadline;
use crate::overview::{self, RunWindow, Warning};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::page_projection::Rect;
use actingcommand_contract::{
    ArtifactKind, ArtifactPayloadDraft, ArtifactProducer, ArtifactRedactionState, AuditInput,
    ClientPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, IdentifierIssuer, InputAction,
    InputSamplingEvidence, InputSamplingRegion, InstanceId, IssuedFrameId, IssuedInstanceId,
    MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES, OriginModule, OwnerEpoch, ProjectedArtifactReference,
    ProjectedEvent, ProjectionPayload, ProjectionProfile, RUNTIME_INFO_FILE, RecognizedTarget,
    RecognizedTargetRole, RetentionClass, RuntimeControlPlaneStatus, RuntimeEventQueryCursor,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeInfo, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint, TaskOutcome,
    TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_client::{RuntimeChannel, RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

/// The ledger's first timestamp, 2100-01-01; one record a second from there on.
const START_UNIX_MS: u64 = 4_102_444_800_000;
const RECORD_SPACING_MS: u64 = 1_000;
const FRAME_WIDTH: u32 = 1280;
const FRAME_HEIGHT: u32 = 720;
/// The largest request the fake Runtime reads.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// T1: a long run, one of whose steps waited on one page and captured a frame every second
/// for about two and a half minutes.
const LONG_RUN_STEPS: u32 = 30;
const WAITING_STEP: u32 = 7;
const WAITING_FRAMES: usize = 160;
/// T2: a run of realistic length: about nine minutes at a step every nine seconds.
const REALISTIC_STEPS: u32 = 60;
/// T2: the failure the run ended with, in its last step.
const RUN_FAILURE: &str = "contained_task_step_unconfirmed";

// ---- T1 -------------------------------------------------------------------------------------

#[test]
fn a42_t1_a_long_runs_evidence_pages_stay_under_the_limit_and_the_cursor_walks_every_step() {
    let run = RunLedger::build(LONG_RUN_STEPS, Some(WAITING_STEP), Ending::Succeeded);
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), run.events.clone());

    let mut walked = Vec::new();
    let mut from_step = 0_u64;
    let mut waiting_row = None;
    for page_number in 0..=LONG_RUN_STEPS {
        let page = call_evidence(
            root.path(),
            json!({"handle": run.request_id, "from_step": from_step}),
        );
        // Every page fits the output budget and answers at least one step, so the cursor
        // always moves on.
        assert!(
            tools::fits_success(&page),
            "page {page_number} (from_step {from_step}) is over the output budget: {} bytes",
            page.to_string().len()
        );
        let steps = page["steps"].as_array().cloned().unwrap_or_default();
        assert!(
            !steps.is_empty(),
            "page {page_number} (from_step {from_step}) answers no step: {page}"
        );
        for step in &steps {
            walked.push(step["step_index"].as_u64().expect("a step_index"));
            if step["step_index"] == json!(WAITING_STEP) {
                waiting_row = Some(step.clone());
            }
        }
        match page.get("next_step") {
            Some(next) => {
                let next = next.as_u64().expect("next_step is a position");
                assert_eq!(
                    next,
                    from_step + steps.len() as u64,
                    "next_step continues after the last step answered: {page}"
                );
                from_step = next;
            }
            None => break,
        }
    }
    assert_eq!(
        walked,
        (0..u64::from(LONG_RUN_STEPS)).collect::<Vec<_>>(),
        "the cursor walks every step once, in order"
    );

    // The waiting step alone is over the output budget: its frames are referenced, not
    // inlined, and the row says which part it left out, how many there are and where they are.
    let waiting = waiting_row.expect("the waiting step was answered");
    assert_eq!(waiting["omitted"], json!(["frames"]), "{waiting}");
    assert_eq!(
        waiting["frames_total"],
        json!(WAITING_FRAMES + 2),
        "{waiting}"
    );
    assert!(waiting.get("frames").is_none(), "{waiting}");
    let range = &waiting["ledger_range"];
    assert!(
        matches!(
            (range["after_sequence"].as_u64(), range["through_sequence"].as_u64()),
            (Some(after), Some(through)) if after < through
        ),
        "{waiting}"
    );

    // Each QueryEvents answer stayed under the Runtime's response limit.
    let stats = runtime.stats();
    assert_eq!(stats.refused_too_large, 0, "a query answered too large");
    assert!(
        stats.largest_page_bytes <= MAX_RUNTIME_EVENT_QUERY_RESPONSE_BYTES,
        "a page of {} bytes",
        stats.largest_page_bytes
    );
    drop(runtime);
}

// ---- T2 -------------------------------------------------------------------------------------

#[test]
fn a42_t2_one_summary_page_holds_every_step_of_a_run_of_realistic_length() {
    let run = RunLedger::build(REALISTIC_STEPS, None, Ending::FailedInLastStep);
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), run.events.clone());

    let page = call_evidence(
        root.path(),
        json!({"handle": run.request_id, "detail": "summary"}),
    );
    assert!(
        tools::fits_success(&page),
        "the summary page is over the budget"
    );
    assert!(
        page.get("truncated").is_none() && page.get("next_step").is_none(),
        "one page holds the whole run: {page}"
    );
    let steps = page["steps"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        steps
            .iter()
            .map(|step| step["step_index"].as_u64())
            .collect::<Vec<_>>(),
        (0..u64::from(REALISTIC_STEPS))
            .map(Some)
            .collect::<Vec<_>>(),
        "every step's summary, in order"
    );
    for step in &steps {
        // A summary carries no heavy payload.
        assert!(
            step.get("targets").is_none() && step.get("frames").is_none(),
            "a summary with payloads: {step}"
        );
        // Index, kind, timing, page and frame references.
        assert_eq!(step["kind"], json!("tap"), "{step}");
        assert!(step["page"].is_string(), "{step}");
        assert!(
            step["started_unix_ms"]
                .as_u64()
                .is_some_and(|started| started >= START_UNIX_MS),
            "{step}"
        );
        assert_eq!(step["frames_total"], json!(2), "{step}");
        assert!(step["frame_sequence"].is_u64(), "{step}");
    }
    let (last, finished) = steps.split_last().expect("steps");
    for step in finished {
        assert!(
            step["duration_ms"].is_u64() && step.get("finished").is_none(),
            "{step}"
        );
    }
    // The run ended in its last step: that step never finished and carries the run's failure.
    assert_eq!(last["finished"], json!(false), "{last}");
    assert_eq!(last["failure_code"], json!(RUN_FAILURE), "{last}");
    drop(runtime);
}

// ---- T3 -------------------------------------------------------------------------------------

/// T3: the run window, and the idle instance's history before it.
const WINDOW_START_UNIX_MS: u64 = START_UNIX_MS + 1_000_000_000;
const WINDOW_END_UNIX_MS: u64 = WINDOW_START_UNIX_MS + 24 * 60 * 60 * 1000;
const SHORT_HISTORY_RUNS: usize = 2_000;
const LONG_HISTORY_RUNS: usize = 8_000;
/// In the window: the idle instance's Lab activity, and another instance's runs.
const LAB_REQUESTS: usize = 2_000;
const OTHER_INSTANCE_RUNS: usize = 100;

#[test]
fn a42_t3_an_instance_without_a_run_in_the_window_answers_none_in_window_within_budget() {
    let mut examined = Vec::new();
    for history_runs in [SHORT_HISTORY_RUNS, LONG_HISTORY_RUNS] {
        let (instance, events) = idle_instance_ledger(history_runs);
        let root = TempDir::new().expect("tempdir");
        let runtime = FakeRuntime::start(root.path(), events);
        let client = RuntimeClient::connect(RuntimeClientConfig::new(
            root.path(),
            RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        ))
        .expect("connect to the fake Runtime");
        let status =
            RuntimeInstanceStatus::new("neutral.idle", instance, false, 0, false, false, false)
                .expect("instance status");
        let mut row = json!({});
        let (mut warnings, mut incomplete) = (Vec::new(), false);
        overview::run_block(
            &client,
            &status,
            RunWindow {
                since_unix_ms: WINDOW_START_UNIX_MS,
                until_unix_ms: WINDOW_END_UNIX_MS,
            },
            unreached_deadline(),
            &mut row,
            &mut warnings,
            &mut incomplete,
        );
        // "None in window", typed, with the window's bounds; the read finished.
        assert_eq!(
            row,
            json!({"run_none_in_window": {
                "since_unix_ms": WINDOW_START_UNIX_MS,
                "until_unix_ms": WINDOW_END_UNIX_MS,
            }}),
            "history of {history_runs} runs"
        );
        assert!(
            !incomplete && warnings.is_empty(),
            "history of {history_runs} runs: incomplete {incomplete}, {} warnings: {:?}",
            warnings.len(),
            warnings
                .iter()
                .map(|warning| match warning {
                    Warning::Outcome(outcome) => outcome.to_string(),
                    Warning::Runtime(error) => error.outcome().to_string(),
                })
                .collect::<Vec<_>>()
        );
        drop(client);
        examined.push(runtime.stats().examined);
        drop(runtime);
    }
    // Bounded by the window, never a scan of the history before it: a four times longer
    // history costs not one record more.
    assert_eq!(
        examined[1], examined[0],
        "records examined with a history of {LONG_HISTORY_RUNS} runs and of {SHORT_HISTORY_RUNS}"
    );
}

/// An instance that ran `history_runs` runs before the window and none in it, where it has only
/// Lab activity while another instance runs; its instance_id and the ledger.
fn idle_instance_ledger(history_runs: usize) -> (InstanceId, Vec<ProjectedEvent>) {
    let mut ledger = Recorder::new(WINDOW_START_UNIX_MS - 2 * (history_runs as u64) * 1_000);
    let idle = ledger.issuer.mint_instance_id().expect("instance id");
    let other = ledger.issuer.mint_instance_id().expect("instance id");
    for _ in 0..history_runs {
        ledger.listed_run(idle);
    }
    ledger.at(WINDOW_START_UNIX_MS);
    for index in 0..LAB_REQUESTS {
        if index % (LAB_REQUESTS / OTHER_INSTANCE_RUNS) == 0 {
            ledger.listed_run(other);
        }
        let links = EventLinksDraft::default()
            .with_instance_id(idle)
            .with_request_id(ledger.issuer.mint_request_id().expect("request id"))
            .with_correlation_id(ledger.issuer.mint_correlation_id().expect("correlation id"));
        ledger.push(
            EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
            links,
            ClientPayloadDraft::lab_request(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
    }
    (*idle.transport(), ledger.events)
}

// ---- The runs of T1 and T2 ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Succeeded,
    /// The last step never finished; the run failed with `RUN_FAILURE`.
    FailedInLastStep,
}

/// One contained run started from the CLI: each step recognises a frame, starts, taps inside
/// a box, captures the next frame and finishes; the waiting step also captures
/// `WAITING_FRAMES` frames while it waits. Then the terminal and the sealed diagnostic stream.
struct RunLedger {
    events: Vec<ProjectedEvent>,
    request_id: Value,
}

impl RunLedger {
    fn build(steps: u32, waiting: Option<u32>, ending: Ending) -> Self {
        let mut ledger = Recorder::new(START_UNIX_MS);
        let issuer = &ledger.issuer;
        let request = issuer.mint_request_id().expect("request id");
        let links = EventLinksDraft::default()
            .with_instance_id(issuer.mint_instance_id().expect("instance id"))
            .with_request_id(request)
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(issuer.mint_task_id().expect("task id"))
            .with_run_id(issuer.mint_run_id().expect("run id"));
        ledger.push(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            links.clone(),
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        let button = Rect {
            x: 100,
            y: 600,
            width: 200,
            height: 80,
        };
        for index in 0..steps {
            let page = format!("neutral/page_{:02}", index % 12);
            let operation = format!("operation_{:02}", index % 12);
            let seen = ledger.frame(&links);
            ledger.fact(
                links.clone().with_frame_id(seen),
                TaskSemanticFact::RecognitionCompleted {
                    candidate_pages: vec![page.clone()],
                    matched_page: Some(page.clone()),
                    frame_width: FRAME_WIDTH,
                    frame_height: FRAME_HEIGHT,
                    targets: vec![
                        RecognizedTarget {
                            target_id: "neutral.target.button".to_owned(),
                            role: RecognizedTargetRole::Required,
                            passed: true,
                            region: Some(button),
                        },
                        RecognizedTarget {
                            target_id: "neutral.target.badge".to_owned(),
                            role: RecognizedTargetRole::Required,
                            passed: false,
                            region: None,
                        },
                    ],
                },
            );
            ledger.fact(
                links.clone(),
                TaskSemanticFact::StepStarted {
                    step_index: index,
                    operation_label: operation.clone(),
                    from_page: page.clone(),
                    phase: None,
                },
            );
            ledger.push(
                runtime_origin(),
                links.clone().with_frame_id(seen),
                TaskPayloadDraft::semantic_with_sampling(
                    TaskSemanticFact::EffectIntent {
                        step_index: index,
                        operation_label: operation.clone(),
                        action: InputAction::Tap { x: 180, y: 640 },
                        frame_extent: None,
                    },
                    InputSamplingEvidence::new(
                        7,
                        vec![InputSamplingRegion::new(100, 600, 200, 80).expect("tap box")],
                    )
                    .expect("tap sampling"),
                    AuditInput::new(),
                )
                .into(),
            );
            if waiting == Some(index) {
                for _ in 0..WAITING_FRAMES {
                    ledger.frame(&links);
                }
            }
            ledger.frame(&links);
            let last = index + 1 == steps;
            if !(last && ending == Ending::FailedInLastStep) {
                ledger.fact(
                    links.clone(),
                    TaskSemanticFact::StepFinished {
                        step_index: index,
                        operation_label: operation,
                        page_label: page,
                        phase: None,
                    },
                );
            }
        }
        let terminal = match ending {
            Ending::Succeeded => TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Success,
                final_page: Some("neutral/page_done".to_owned()),
                executed_steps: Some(steps),
                outcome: None,
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
            Ending::FailedInLastStep => TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                final_page: None,
                executed_steps: Some(steps - 1),
                outcome: Some(Outcome::relayed(RUN_FAILURE)),
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
        };
        ledger.fact(links.clone(), terminal);
        ledger.artifact(
            links,
            ArtifactKind::DiagnosticJson,
            ArtifactProducer::ArtifactStore,
            ArtifactRedactionState::Pending,
            None,
        );
        Self {
            events: ledger.events,
            request_id: json!(request.transport()),
        }
    }
}

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

/// One `ac_run_evidence` call with `arguments`, answered by the fake Runtime of `root`; its
/// result.
fn call_evidence(root: &Path, arguments: Value) -> Value {
    let runtime = RuntimeAccess::new(None, Some(root.to_path_buf()));
    let cancelled = AtomicBool::new(false);
    let jobs = Jobs::new();
    let context = ToolContext {
        runtime: &runtime,
        cancelled: &cancelled,
        deadline: unreached_deadline(),
        session: 1,
        jobs: &jobs,
    };
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };
    match tool::run(&context, &arguments) {
        Ok(success) => success.result,
        Err(error) => panic!("ac_run_evidence {arguments:?} failed: {}", error.to_value()),
    }
}

// ---- The ledger the fake Runtime serves -----------------------------------------------------

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the fake ledger holds no secrets")
    }
}

/// Records events one after another, as a Forensic read projects them.
struct Recorder {
    issuer: IdentifierIssuer,
    events: Vec<ProjectedEvent>,
    /// The next record's time.
    next_unix_ms: u64,
}

impl Recorder {
    fn new(start_unix_ms: u64) -> Self {
        Self {
            issuer: IdentifierIssuer::new().expect("identifier issuer"),
            events: Vec::new(),
            next_unix_ms: start_unix_ms,
        }
    }

    /// The next record is recorded at `unix_ms`.
    fn at(&mut self, unix_ms: u64) {
        assert!(
            unix_ms >= self.next_unix_ms,
            "the ledger's time goes forward"
        );
        self.next_unix_ms = unix_ms;
    }

    fn push(&mut self, origin: EventOrigin, links: EventLinksDraft, payload: EventPayloadDraft) {
        let sequence = u64::try_from(self.events.len()).expect("ledger length") + 1;
        let sanitized = EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            self.next_unix_ms,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&NoSecrets)
        .expect("sanitize a fake ledger event");
        self.next_unix_ms += RECORD_SPACING_MS;
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
    }

    fn fact(&mut self, links: EventLinksDraft, fact: TaskSemanticFact) {
        self.push(
            runtime_origin(),
            links,
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        );
    }

    /// One run of `instance` as its listing facts record it: its main admission and its
    /// terminal.
    fn listed_run(&mut self, instance: IssuedInstanceId) {
        let issuer = &self.issuer;
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(issuer.mint_request_id().expect("request id"))
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(issuer.mint_task_id().expect("task id"))
            .with_run_id(issuer.mint_run_id().expect("run id"));
        self.fact(
            links.clone(),
            TaskSemanticFact::PackageAdmitted {
                package_label: "neutral.package".to_owned(),
                task_label: "neutral.task".to_owned(),
                response_deadline_monotonic_ms: None,
            },
        );
        self.fact(
            links,
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Success,
                final_page: Some("neutral/page_done".to_owned()),
                executed_steps: Some(1),
                outcome: None,
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
        );
    }

    /// A verified capture frame of the run; its frame.
    fn frame(&mut self, links: &EventLinksDraft) -> IssuedFrameId {
        let frame = self.issuer.mint_frame_id().expect("frame id");
        self.artifact(
            links.clone().with_frame_id(frame),
            ArtifactKind::CaptureFrame,
            ArtifactProducer::CaptureStore,
            ArtifactRedactionState::NotRequired,
            Some(frame),
        );
        frame
    }

    /// An `artifact.verified` record carrying one artifact.
    fn artifact(
        &mut self,
        links: EventLinksDraft,
        kind: ArtifactKind,
        producer: ArtifactProducer,
        redaction_state: ArtifactRedactionState,
        frame: Option<IssuedFrameId>,
    ) {
        self.push(
            EventOrigin::new(
                EventSource::System,
                OriginModule::ArtifactStore,
                EventActor::System,
            ),
            links,
            ArtifactPayloadDraft::verified(AuditInput::new()).into(),
        );
        let artifact_id = *self
            .issuer
            .mint_artifact_id()
            .expect("artifact id")
            .transport();
        let event = self.events.last_mut().expect("the artifact event");
        event.artifacts = vec![ProjectedArtifactReference {
            artifact_id,
            kind,
            run_id: event.links.run_id().copied(),
            frame_id: frame.map(|frame| *frame.transport()),
            correlation_id: event.links.correlation_id().copied(),
            object_key: None,
            media_type: kind.media_type(),
            byte_count: 4096,
            sha256: format!("sha256:{:064x}", event.sequence),
            created_at_unix_ms: event.timestamp_unix_ms,
            producer,
            retention_class: RetentionClass::DebugFull,
            redaction_state,
        }];
    }
}

// ---- The fake Runtime -----------------------------------------------------------------------

/// What the fake ledger answered.
#[derive(Clone, Copy, Debug, Default)]
struct Stats {
    /// Records the queries examined (the ledger's index read).
    examined: u64,
    /// The largest page answered, encoded.
    largest_page_bytes: usize,
    /// Queries answered `runtime_event_query_response_too_large`.
    refused_too_large: u64,
}

/// A Runtime on loopback that answers every connection from one ledger until dropped.
struct FakeRuntime {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    stats: Arc<Mutex<Stats>>,
    acceptor: Option<JoinHandle<()>>,
}

impl FakeRuntime {
    /// Listens on loopback and writes `runtime-info.json` under `root` naming it.
    fn start(root: &Path, events: Vec<ProjectedEvent>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake Runtime");
        let address = listener.local_addr().expect("fake Runtime address");
        let owner_epoch = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport();
        let info = RuntimeInfo::new(
            std::process::id(),
            address.ip().to_string(),
            address.port(),
            owner_epoch,
            1,
        )
        .expect("fake Runtime info");
        fs::write(
            root.join(RUNTIME_INFO_FILE),
            serde_json::to_vec(&info).expect("encode the Runtime info"),
        )
        .expect("write the Runtime info");
        let stopping = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(Mutex::new(Stats::default()));
        let ledger = Arc::new(Ledger::new(events));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            let stats = Arc::clone(&stats);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let stream = stream.expect("accept a client");
                    let ledger = Arc::clone(&ledger);
                    let stats = Arc::clone(&stats);
                    thread::spawn(move || serve(stream, owner_epoch, &ledger, &stats));
                }
            })
        };
        Self {
            address,
            stopping,
            stats,
            acceptor: Some(acceptor),
        }
    }

    fn stats(&self) -> Stats {
        *self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for FakeRuntime {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the acceptor, which then sees `stopping`.
        let _ = TcpStream::connect(self.address);
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

/// Answers one client's Health, BindChannel, Status and event queries until it hangs up.
fn serve(mut stream: TcpStream, owner_epoch: OwnerEpoch, ledger: &Ledger, stats: &Mutex<Stats>) {
    while let Some(request) = next_request(&mut stream) {
        let receipt = match request.operation() {
            RuntimeOperation::Health => completed(&request, RuntimeResult::Health { owner_epoch }),
            RuntimeOperation::BindChannel { channel, via } => completed(
                &request,
                RuntimeResult::ChannelBound {
                    channel: *channel,
                    via: *via,
                },
            ),
            RuntimeOperation::Status => completed(
                &request,
                RuntimeResult::Status {
                    status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                        .expect("fake status"),
                },
            ),
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => match ledger.page(query, *profile, page, stats) {
                Some(page) => completed(&request, RuntimeResult::EventPage { page }),
                None => RuntimeReceipt::failure(
                    &request,
                    RuntimeReceiptState::Denied,
                    None,
                    Outcome::new(SharedCode::RuntimeEventQueryResponseTooLarge),
                )
                .expect("fake failure receipt"),
            },
            other => panic!("the fake Runtime answers no {other:?}"),
        };
        let body = serde_json::to_vec(&receipt).expect("encode the receipt");
        let length = u32::try_from(body.len()).expect("receipt length");
        stream
            .write_all(&length.to_be_bytes())
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .expect("write the receipt");
    }
}

fn completed(request: &RuntimeRequest, result: RuntimeResult) -> RuntimeReceipt {
    RuntimeReceipt::success(request, RuntimeReceiptState::Completed, None, result)
        .expect("fake receipt")
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
        Err(error) => panic!("fake Runtime request header: {error}"),
    }
    let length = u32::from_be_bytes(header) as usize;
    assert!(
        length > 0 && length <= MAX_REQUEST_BYTES,
        "fake Runtime request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("fake Runtime request body");
    Some(serde_json::from_slice(&body).expect("decode a request"))
}

/// The fake ledger: its records in sequence order, at one snapshot (the last sequence).
struct Ledger {
    events: Vec<ProjectedEvent>,
    /// How many records each indexed value has, for picking the smallest index.
    counts: BTreeMap<String, u64>,
}

impl Ledger {
    fn new(events: Vec<ProjectedEvent>) -> Self {
        let mut counts = BTreeMap::new();
        for event in &events {
            for key in index_keys(event) {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
        Self { events, counts }
    }

    /// One page of the records `query` selects after the request's cursor, cut to the response
    /// limit as the ledger cuts it; `None` when a lone record is over it.
    fn page(
        &self,
        query: &EventQuery,
        profile: ProjectionProfile,
        request: &RuntimeEventQueryPageRequest,
        stats: &Mutex<Stats>,
    ) -> Option<RuntimeEventQueryPage> {
        assert_served(query);
        let snapshot = self.events.last().map_or(0, |event| event.sequence);
        if let Some(position) = request.snapshot_position() {
            assert_eq!(position, snapshot, "a read at another snapshot");
        }
        let after = request
            .cursor()
            .map_or(0, RuntimeEventQueryCursor::after_sequence);
        let from = query.from_sequence.unwrap_or(0).max(after + 1);
        let through = query.to_sequence.unwrap_or(u64::MAX).min(snapshot);
        // The ledger's index read: the records of the smallest index the query names, between
        // its sequence bounds.
        let named = named_keys(query);
        let smallest = named
            .iter()
            .min_by_key(|key| self.counts.get(*key).copied().unwrap_or(0));
        let wanted = usize::from(request.limit()) + 1;
        let mut matched = Vec::new();
        let mut examined = 0_u64;
        if smallest.is_none_or(|key| self.counts.contains_key(key)) {
            for event in self
                .events
                .iter()
                .filter(|event| event.sequence >= from && event.sequence <= through)
                .filter(|event| smallest.is_none_or(|key| index_keys(event).contains(key)))
            {
                examined += 1;
                if matches(query, event) {
                    matched.push(project(event, profile));
                    if matched.len() == wanted {
                        break;
                    }
                }
            }
        }
        let mut has_more = matched.len() == wanted;
        matched.truncate(usize::from(request.limit()));
        let mut stats = stats.lock().unwrap_or_else(PoisonError::into_inner);
        stats.examined += examined;
        loop {
            let next_cursor = matched.last().filter(|_| has_more).map(|last| {
                RuntimeEventQueryCursor::new(snapshot, last.sequence, query, profile)
                    .expect("next cursor")
            });
            match RuntimeEventQueryPage::new(
                matched.clone(),
                snapshot,
                request.limit(),
                has_more,
                next_cursor,
            ) {
                Ok(page) => {
                    let bytes = serde_json::to_vec(&page).expect("encode the page").len();
                    stats.largest_page_bytes = stats.largest_page_bytes.max(bytes);
                    return Some(page);
                }
                Err(error)
                    if error.code().as_str()
                        == SharedCode::RuntimeEventQueryResponseTooLarge.as_str() =>
                {
                    if matched.len() > 1 {
                        matched.pop();
                        has_more = true;
                    } else {
                        stats.refused_too_large += 1;
                        return None;
                    }
                }
                Err(error) => panic!("fake event page: {error:?}"),
            }
        }
    }
}

/// The record as `profile` projects it: Concise and Cli leave the payload out.
fn project(event: &ProjectedEvent, profile: ProjectionProfile) -> ProjectedEvent {
    let mut projected = event.clone();
    if matches!(profile, ProjectionProfile::Concise | ProjectionProfile::Cli) {
        projected.payload = ProjectionPayload::Omitted;
    }
    projected
}

/// The index keys of one record: its event type and each of its links.
fn index_keys(event: &ProjectedEvent) -> Vec<String> {
    let links = &event.links;
    let mut keys = vec![format!("type:{}", json!(event.event_type))];
    keys.extend(
        links
            .instance_id()
            .map(|id| format!("instance:{}", json!(id))),
    );
    keys.extend(
        links
            .request_id()
            .map(|id| format!("request:{}", json!(id))),
    );
    keys.extend(
        links
            .correlation_id()
            .map(|id| format!("correlation:{}", json!(id))),
    );
    keys.extend(links.task_id().map(|id| format!("task:{}", json!(id))));
    keys.extend(links.run_id().map(|id| format!("run:{}", json!(id))));
    keys.extend(links.lease_id().map(|id| format!("lease:{}", json!(id))));
    keys
}

/// The index keys `query` names: one event type and its links (a set of event types and the
/// time bounds are filters, not indexes).
fn named_keys(query: &EventQuery) -> Vec<String> {
    let mut keys = Vec::new();
    keys.extend(query.event_type.map(|kind| format!("type:{}", json!(kind))));
    keys.extend(
        query
            .instance_id
            .map(|id| format!("instance:{}", json!(id))),
    );
    keys.extend(query.request_id.map(|id| format!("request:{}", json!(id))));
    keys.extend(
        query
            .correlation_id
            .map(|id| format!("correlation:{}", json!(id))),
    );
    keys.extend(query.task_id.map(|id| format!("task:{}", json!(id))));
    keys.extend(query.run_id.map(|id| format!("run:{}", json!(id))));
    keys.extend(query.lease_id.map(|id| format!("lease:{}", json!(id))));
    keys
}

/// This ledger serves the time and sequence bounds, one event type or a set of them, the
/// instance and the run links; a query with any other filter fails the test.
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
        "the fake ledger serves no such filter"
    );
}

/// Whether `event` matches every filter of `query`; the upper time bound is exclusive, as in
/// the contract.
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
