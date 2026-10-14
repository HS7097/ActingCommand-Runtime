// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-6: the parity test of `actingctl run evidence` and MCP `ac_run_evidence`
//! (MCP-0 command table v1.1 §2.3, §4.7). Both sides are the real binaries: the MCP server over
//! pipes (fixture I5, `mcp_harness`) and the CLI as one process per call. The Runtime is a
//! scripted one on loopback that answers Health and event queries from one in-memory ledger and
//! records nothing, so every call reads the same snapshot. The ledger holds one contained run
//! started from the CLI: it recognises a frame, taps inside one box, recognises the next frame,
//! swipes between two boxes, ends on its terminal page, and its task diagnostic stream is sealed
//! last. For each argument set the CLI's stdout JSON and the ac_run_evidence result are the same
//! object (nothing is cut to the output budget, so §2.2 leaves them no presentation
//! difference), and that object is the one §4.7 gives for this run. Workflow #378 A5: the CLI
//! line carries the outcome `run_evidence_reported` beside the evidence; the evidence is the rest
//! of the line.

use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::page_projection::Rect;
use actingcommand_contract::{
    ArtifactKind, ArtifactPayloadDraft, ArtifactProducer, ArtifactRedactionState, AuditInput,
    ClientPayloadDraft, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, IdentifierIssuer, InputAction,
    InputSamplingEvidence, InputSamplingRegion, IssuedFrameId, OriginModule, OwnerEpoch,
    ProjectedArtifactReference, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RUNTIME_INFO_FILE, RecognizedTarget, RecognizedTargetRole, RetentionClass,
    RuntimeControlPlaneStatus, RuntimeEventQueryCursor, RuntimeEventQueryPage,
    RuntimeEventQueryPageRequest, RuntimeInfo, RuntimeOperation, RuntimeReceipt,
    RuntimeReceiptState, RuntimeRequest, RuntimeResult, SanitizationError, SecretField,
    SecretFingerprinter, Sha256Fingerprint, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

/// The ledger's first timestamp, 2100-01-01; one event a second from there on.
const START_UNIX_MS: u64 = 4_102_444_800_000;
const EVENT_SPACING_MS: u64 = 1_000;
const FRAME_WIDTH: u32 = 1280;
const FRAME_HEIGHT: u32 = 720;
/// The largest request the scripted Runtime reads.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[test]
fn gate_actingctl_run_evidence_and_ac_run_evidence_answer_the_same_steps_for_the_same_arguments() {
    let ledger = Ledger::build();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), Arc::clone(&ledger));
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let request_id = ledger.identity["request_id"].clone();
    let run_id = ledger.identity["run_id"].clone();

    // The whole run by its request_id (`--request-id` ↔ `handle`).
    let whole = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &[("request-id", request_id.clone())],
        json!({"handle": request_id}),
    );
    assert_eq!(
        whole,
        ledger.expected(&[0, 1], None),
        "the evidence of the whole run"
    );

    // The first step only: next_step names the step after it.
    let first = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &[("request-id", request_id.clone()), ("limit", json!(1))],
        json!({"handle": request_id, "limit": 1}),
    );
    assert_eq!(
        first,
        ledger.expected(&[0], Some(1)),
        "the first page of one step"
    );

    // The page after it, by the run's run_id: the same run, read from next_step.
    let second = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &[
            ("run-id", run_id.clone()),
            ("from-step", json!(1)),
            ("limit", json!(1)),
        ],
        json!({"run_id": run_id, "from_step": 1, "limit": 1}),
    );
    assert_eq!(
        second,
        ledger.expected(&[1], None),
        "the page from next_step"
    );

    mcp.close_input();
    drop(mcp);
    drop(runtime);
}

/// The CLI's stdout JSON for `flags`, after checking that it is the ac_run_evidence result for
/// `arguments` and that neither side warned.
#[track_caller]
fn parity(
    mcp: &mut McpServer,
    id: &mut u64,
    root: &Path,
    flags: &[(&str, Value)],
    arguments: Value,
) -> Value {
    let cli = cli_run_evidence(root, flags);
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_run_evidence", arguments.clone());
    let tool = accepted(&answer, "ac_run_evidence").clone();
    assert!(
        answer["result"]["structuredContent"]
            .get("warnings")
            .is_none(),
        "ac_run_evidence with {arguments} warned: {answer}"
    );
    assert_eq!(
        cli, tool,
        "with {arguments}, the CLI's stdout JSON and the ac_run_evidence result differ:\nCLI {cli}\nac_run_evidence {tool}"
    );
    cli
}

/// `actingctl run evidence --state-root <root>` with `flags`; its stdout JSON without the line's
/// `outcome`, which is `run_evidence_reported`.
#[track_caller]
fn cli_run_evidence(root: &Path, flags: &[(&str, Value)]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actingctl"));
    command.args(["run", "evidence", "--state-root"]).arg(root);
    for (name, value) in flags {
        command.arg(format!("--{name}"));
        command.arg(match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    }
    let output = command.output().expect("run actingctl run evidence");
    assert!(
        output.status.success(),
        "actingctl run evidence with {flags:?} failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl run evidence with {flags:?} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl run evidence with {flags:?} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("run_evidence_reported"),
        "actingctl run evidence with {flags:?}: {outcome}"
    );
    line
}

/// The scripted Runtime's ledger, and the rows §4.7 gives for its run.
struct Ledger {
    events: Vec<ProjectedEvent>,
    identity: Value,
    steps: Vec<Value>,
    final_: Value,
    overlay: Value,
}

impl Ledger {
    /// The §4.7 answer holding the steps at `positions`, with `next_step` when given.
    fn expected(&self, positions: &[usize], next_step: Option<usize>) -> Value {
        let mut expected = json!({
            "request_id": self.identity["request_id"],
            "run_id": self.identity["run_id"],
            "task_id": self.identity["task_id"],
            "steps": positions.iter().map(|position| self.steps[*position].clone()).collect::<Vec<_>>(),
            "final": self.final_,
            "overlay": self.overlay,
        });
        if let Some(next_step) = next_step {
            expected["next_step"] = json!(next_step);
        }
        expected
    }

    fn build() -> Arc<Self> {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance = issuer.mint_instance_id().expect("instance id");
        let correlation_id = issuer.mint_correlation_id().expect("correlation id");
        let task_id = issuer.mint_task_id().expect("task id");
        let run_id = issuer.mint_run_id().expect("run id");
        let links = EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(issuer.mint_request_id().expect("request id"))
            .with_correlation_id(correlation_id)
            .with_task_id(task_id)
            .with_run_id(run_id);
        let runtime = || {
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            )
        };
        let mut events = Vec::new();
        push(
            &issuer,
            &mut events,
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            links.clone(),
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        );
        let fact = |events: &mut Vec<ProjectedEvent>, links: EventLinksDraft, fact| {
            push(
                &issuer,
                events,
                runtime(),
                links,
                TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
            );
        };
        let frame_a = issuer.mint_frame_id().expect("frame id");
        let ref_a = frame(&issuer, &mut events, &links, frame_a);
        let home_button = Rect {
            x: 100,
            y: 600,
            width: 200,
            height: 80,
        };
        fact(
            &mut events,
            links.clone().with_frame_id(frame_a),
            recognition(
                "neutral/home",
                &[("neutral.home.button", true, Some(home_button))],
            ),
        );
        fact(
            &mut events,
            links.clone(),
            TaskSemanticFact::StepStarted {
                step_index: 0,
                operation_label: "open_list".to_owned(),
                from_page: "neutral/home".to_owned(),
                phase: None,
            },
        );
        // Step 0 taps inside the button's box, at a point drawn in it.
        let tap = InputAction::Tap { x: 180, y: 640 };
        push(
            &issuer,
            &mut events,
            runtime(),
            links.clone().with_frame_id(frame_a),
            TaskPayloadDraft::semantic_with_sampling(
                TaskSemanticFact::EffectIntent {
                    step_index: 0,
                    operation_label: "open_list".to_owned(),
                    action: tap,
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
        let frame_b = issuer.mint_frame_id().expect("frame id");
        let ref_b = frame(&issuer, &mut events, &links, frame_b);
        fact(
            &mut events,
            links.clone().with_frame_id(frame_b),
            recognition(
                "neutral/list",
                &[
                    ("neutral.list.title", true, None),
                    ("neutral.list.alert", false, None),
                ],
            ),
        );
        fact(
            &mut events,
            links.clone(),
            TaskSemanticFact::StepFinished {
                step_index: 0,
                operation_label: "open_list".to_owned(),
                page_label: "neutral/list".to_owned(),
                phase: None,
            },
        );
        fact(
            &mut events,
            links.clone(),
            TaskSemanticFact::StepStarted {
                step_index: 1,
                operation_label: "scroll_list".to_owned(),
                from_page: "neutral/list".to_owned(),
                phase: None,
            },
        );
        // Step 1 swipes from one box to another: two boxes, so no single box and no point.
        push(
            &issuer,
            &mut events,
            runtime(),
            links.clone().with_frame_id(frame_b),
            TaskPayloadDraft::semantic_with_sampling(
                TaskSemanticFact::EffectIntent {
                    step_index: 1,
                    operation_label: "scroll_list".to_owned(),
                    action: InputAction::Swipe {
                        x1: 640,
                        y1: 500,
                        x2: 640,
                        y2: 200,
                        duration_ms: 300,
                    },
                    frame_extent: None,
                },
                InputSamplingEvidence::new(
                    11,
                    vec![
                        InputSamplingRegion::new(600, 480, 80, 40).expect("swipe start box"),
                        InputSamplingRegion::new(600, 180, 80, 40).expect("swipe end box"),
                    ],
                )
                .expect("swipe sampling"),
                AuditInput::new(),
            )
            .into(),
        );
        let frame_c = issuer.mint_frame_id().expect("frame id");
        let ref_c = frame(&issuer, &mut events, &links, frame_c);
        fact(
            &mut events,
            links.clone(),
            TaskSemanticFact::StepFinished {
                step_index: 1,
                operation_label: "scroll_list".to_owned(),
                page_label: "neutral/terminal".to_owned(),
                phase: None,
            },
        );
        fact(
            &mut events,
            links.clone(),
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Success,
                final_page: Some("neutral/terminal".to_owned()),
                executed_steps: Some(2),
                outcome: None,
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
        );
        // The task diagnostic stream, sealed when the run ended.
        let overlay = artifact_event(
            &issuer,
            &mut events,
            links.clone(),
            ArtifactKind::DiagnosticJson,
            ArtifactProducer::ArtifactStore,
            ArtifactRedactionState::Pending,
            None,
        );
        let identity = json!(events.last().expect("the run's events").links);
        // §4.7 rows: a step's page is the page it started from; its targets are those of the
        // recognition of the frame its input was computed on; its frames are that frame and the
        // frames captured while it ran; its action is its first input.
        let steps = vec![
            json!({
                "step_index": 0,
                "page": "neutral/home",
                "targets": [{
                    "target": "neutral.home.button",
                    "matched": true,
                    "box": {"x": 100, "y": 600, "width": 200, "height": 80},
                }],
                "action": {
                    "kind": "tap",
                    "point": {"x": 180, "y": 640},
                    "rect": {"x": 100, "y": 600, "width": 200, "height": 80},
                },
                "frames": [{"ref": ref_a}, {"ref": ref_b}],
            }),
            json!({
                "step_index": 1,
                "page": "neutral/list",
                "targets": [
                    {"target": "neutral.list.title", "matched": true},
                    {"target": "neutral.list.alert", "matched": false},
                ],
                "action": {"kind": "swipe"},
                "frames": [{"ref": ref_b}, {"ref": ref_c}],
            }),
        ];
        Arc::new(Self {
            events,
            identity,
            steps,
            final_: json!({"page": "neutral/terminal", "frame_ref": ref_c}),
            overlay: json!({"ref": overlay}),
        })
    }
}

/// A recognition of one frame that matched `page`, with `targets` (id, passed, region).
fn recognition(page: &str, targets: &[(&str, bool, Option<Rect>)]) -> TaskSemanticFact {
    TaskSemanticFact::RecognitionCompleted {
        candidate_pages: vec![page.to_owned()],
        matched_page: Some(page.to_owned()),
        frame_width: FRAME_WIDTH,
        frame_height: FRAME_HEIGHT,
        targets: targets
            .iter()
            .map(|(target_id, passed, region)| RecognizedTarget {
                target_id: (*target_id).to_owned(),
                role: RecognizedTargetRole::Required,
                passed: *passed,
                region: *region,
            })
            .collect(),
    }
}

/// The verified capture frame `frame` of the run; its material ref.
fn frame(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    links: &EventLinksDraft,
    frame: IssuedFrameId,
) -> String {
    artifact_event(
        issuer,
        events,
        links.clone().with_frame_id(frame),
        ArtifactKind::CaptureFrame,
        ArtifactProducer::CaptureStore,
        ArtifactRedactionState::NotRequired,
        Some(frame),
    )
}

/// An `artifact.verified` event carrying one artifact; its material ref (`<seq>:<event_id>:
/// <artifact_id>`, §2.2).
fn artifact_event(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    links: EventLinksDraft,
    kind: ArtifactKind,
    producer: ArtifactProducer,
    redaction_state: ArtifactRedactionState,
    frame: Option<IssuedFrameId>,
) -> String {
    push(
        issuer,
        events,
        EventOrigin::new(
            EventSource::System,
            OriginModule::ArtifactStore,
            EventActor::System,
        ),
        links,
        ArtifactPayloadDraft::verified(AuditInput::new()).into(),
    );
    let event = events.last_mut().expect("the artifact event");
    let artifact = ProjectedArtifactReference {
        artifact_id: *issuer.mint_artifact_id().expect("artifact id").transport(),
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
    };
    let reference = format!(
        "{}:{}:{}",
        event.sequence,
        text_of(json!(event.event_id)),
        text_of(json!(artifact.artifact_id))
    );
    event.artifacts = vec![artifact];
    reference
}

fn text_of(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => other.to_string(),
    }
}

fn next_sequence(events: &[ProjectedEvent]) -> u64 {
    u64::try_from(events.len()).expect("ledger length") + 1
}

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the scripted ledger holds no secrets")
    }
}

/// Appends one event, as a Forensic read projects it, at the next sequence.
fn push(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) {
    let sequence = next_sequence(events);
    let sanitized = EventDraft::new(
        issuer.mint_event_id().expect("event id"),
        START_UNIX_MS + sequence * EVENT_SPACING_MS,
        EventSeverity::Info,
        origin,
        links,
        payload,
    )
    .sanitize(&NoSecrets)
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

/// A Runtime on loopback that answers every connection from one ledger until dropped.
struct ScriptedRuntime {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl ScriptedRuntime {
    /// Listens on loopback and writes `runtime-info.json` under `root` naming it.
    fn start(root: &Path, ledger: Arc<Ledger>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the scripted Runtime");
        let address = listener.local_addr().expect("scripted Runtime address");
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
        .expect("scripted Runtime info");
        fs::write(
            root.join(RUNTIME_INFO_FILE),
            serde_json::to_vec(&info).expect("encode the Runtime info"),
        )
        .expect("write the Runtime info");
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let stream = stream.expect("accept a client");
                    let ledger = Arc::clone(&ledger);
                    thread::spawn(move || serve(stream, owner_epoch, &ledger));
                }
            })
        };
        Self {
            address,
            stopping,
            acceptor: Some(acceptor),
        }
    }
}

impl Drop for ScriptedRuntime {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the acceptor, which then sees `stopping`.
        let _ = TcpStream::connect(self.address);
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

/// Answers one client's Health, Status and event queries until it hangs up.
fn serve(mut stream: TcpStream, owner_epoch: OwnerEpoch, ledger: &Ledger) {
    while let Some(request) = next_request(&mut stream) {
        let result = match request.operation() {
            RuntimeOperation::Health => RuntimeResult::Health { owner_epoch },
            // Workflow #389 U-2: the handshake's BindChannel.
            RuntimeOperation::BindChannel { channel, via } => RuntimeResult::ChannelBound {
                channel: *channel,
                via: *via,
            },
            // No instance is configured: the run's evidence names none.
            RuntimeOperation::Status => RuntimeResult::Status {
                status: RuntimeControlPlaneStatus::new(owner_epoch, Vec::new())
                    .expect("scripted status"),
            },
            RuntimeOperation::QueryEvents {
                query,
                profile,
                page,
            } => RuntimeResult::EventPage {
                page: event_page(&ledger.events, query, *profile, page),
            },
            _ => panic!("the scripted Runtime answers Health, Status and QueryEvents only"),
        };
        let receipt =
            RuntimeReceipt::success(&request, RuntimeReceiptState::Completed, None, result)
                .expect("scripted receipt");
        let body = serde_json::to_vec(&receipt).expect("encode the receipt");
        let length = u32::try_from(body.len()).expect("receipt length");
        stream
            .write_all(&length.to_be_bytes())
            .expect("write the receipt header");
        stream.write_all(&body).expect("write the receipt body");
        stream.flush().expect("flush the receipt");
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
        Err(error) => panic!("scripted Runtime request header: {error}"),
    }
    let length = u32::from_be_bytes(header) as usize;
    assert!(
        length > 0 && length <= MAX_FRAME_BYTES,
        "scripted Runtime request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("scripted Runtime request body");
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

/// Whether `event` matches `query`. This ledger serves the sequence bounds, one event type and
/// the run links; a query with any other filter is refused.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let unserved = EventQuery {
        from_sequence: None,
        to_sequence: None,
        event_type: None,
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
        .from_sequence
        .is_none_or(|from| event.sequence >= from)
        && query.to_sequence.is_none_or(|to| event.sequence <= to)
        && query
            .event_type
            .is_none_or(|event_type| event.event_type == event_type)
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
