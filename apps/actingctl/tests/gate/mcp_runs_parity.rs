// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-3: the parity test of `actingctl runs` and MCP `ac_runs` (MCP-0 command
//! table v1.1 §2.3, §4.3). Both sides are the real binaries: the MCP server over pipes (fixture
//! I5, `mcp_harness`) and the CLI as one process per call. The Runtime is a scripted one on
//! loopback that answers Health, Status and event queries from one in-memory ledger and records
//! nothing, so every call reads the same snapshot (a real Runtime records each Status it
//! answers, and its run facts come only from running a task package). The ledger holds a failed
//! manual run on one instance, Lab captures on that instance, then a succeeded scheduled run on
//! another. For each argument set the CLI's stdout JSON and the ac_runs result are the same
//! object: nothing is cut to the output budget, so §2.2 leaves them no presentation difference.
//! That object is the one §4.3 gives for this ledger, with the coordinator's ruling on it: a
//! scheduled run's row carries its policy task id as `catalog_task_id`, a manual run's has none,
//! and `task_prefix` matches `catalog_task_id`. Workflow #378 A5: the CLI line carries the
//! outcome `run_list_reported` beside the run list; the run list is the rest of the line.

use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CapturePayloadDraft, ClientPayloadDraft, CommandPayloadDraft, EffectDisposition,
    EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventQuery, EventSeverity, EventSource, IdentifierIssuer, IssuedInstanceId, OriginModule,
    OwnerEpoch, PolicyDispatchEventData, PolicyPayloadDraft, PolicyReasonRecord, ProjectedEvent,
    ProjectionPayload, ProjectionProfile, RUNTIME_INFO_FILE, RuntimeControlPlaneStatus,
    RuntimeEventQueryCursor, RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeInfo,
    RuntimeInstanceStatus, RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
    TaskEntryRecognitionPhase, TaskEntryTargetDisposition, TaskOutcome, TaskPayloadDraft,
    TaskSemanticFact,
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

/// The window's start, 2100-01-01: never further back than the 7-day bound of §4.3.
const SINCE_UNIX_MS: u64 = 4_102_444_800_000;
/// The ledger records one event a second from the window's start on.
const EVENT_SPACING_MS: u64 = 1_000;
const ALIAS_A: &str = "neutral.runs.a";
const ALIAS_B: &str = "neutral.runs.b";
/// The policy task the scheduler dispatched the run on instance B for: an opaque id from the
/// resource side.
const POLICY_TASK_B: &str = "neutral.routine.collect";
/// A prefix of `POLICY_TASK_B`; no Runtime task_id (`task_<hex>`) starts with it.
const POLICY_TASK_PREFIX: &str = "neutral.routine.";
/// The Lab captures on instance A between the two runs.
const LAB_CAPTURES: usize = 10;
/// The largest request the scripted Runtime reads.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[test]
fn gate_actingctl_runs_and_ac_runs_answer_the_same_run_list_for_the_same_arguments() {
    let ledger = Ledger::build();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), Arc::clone(&ledger));
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let window = json!({"since_unix_ms": SINCE_UNIX_MS});
    let snapshot = json!(ledger.snapshot());

    // Every run of the window, newest first.
    let all = parity(
        &mut mcp,
        &mut id,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS}),
    );
    assert_eq!(
        all,
        json!({
            "runs": [ledger.succeeded_row.clone(), ledger.failed_row.clone()],
            "window": window,
            "snapshot_ledger_position": snapshot,
        }),
        "the run list of the whole window"
    );

    // One instance, named by its alias.
    let one = parity(
        &mut mcp,
        &mut id,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS, "instance": ALIAS_A}),
    );
    assert_eq!(
        one,
        json!({
            "runs": [ledger.failed_row.clone()],
            "window": window,
            "snapshot_ledger_position": snapshot,
        }),
        "the run list of instance {ALIAS_A}"
    );

    // Pages of one run: the first page's cursor gives the second on either side.
    let first = parity(
        &mut mcp,
        &mut id,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS, "limit": 1}),
    );
    let cursor = first["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a first page of one run of two has no next_cursor: {first}"))
        .to_owned();
    assert_eq!(
        first,
        json!({
            "runs": [ledger.succeeded_row.clone()],
            "window": window,
            "next_cursor": cursor,
            "snapshot_ledger_position": snapshot,
        }),
        "the first page of one run"
    );
    let second = parity(
        &mut mcp,
        &mut id,
        root.path(),
        json!({"limit": 1, "cursor": cursor}),
    );
    assert_eq!(
        second,
        json!({
            "runs": [ledger.failed_row.clone()],
            "window": window,
            "snapshot_ledger_position": snapshot,
        }),
        "the page after the first page's next_cursor"
    );

    // A prefix of the policy task id: the scheduled run only; the manual run has no policy task
    // id and never matches.
    let prefixed = parity(
        &mut mcp,
        &mut id,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS, "task_prefix": POLICY_TASK_PREFIX}),
    );
    assert_eq!(
        prefixed,
        json!({
            "runs": [ledger.succeeded_row.clone()],
            "window": window,
            "snapshot_ledger_position": snapshot,
        }),
        "the run list of the policy task prefix {POLICY_TASK_PREFIX}"
    );

    mcp.close_input();
    drop(mcp);
    drop(runtime);
}

/// The CLI's stdout JSON for `arguments`, each given as its flag (R2), after checking that it
/// is the ac_runs result for the same arguments and that neither side warned.
#[track_caller]
fn parity(mcp: &mut McpServer, id: &mut u64, root: &Path, arguments: Value) -> Value {
    let cli = cli_runs(root, &arguments);
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_runs", arguments.clone());
    let tool = accepted(&answer, "ac_runs").clone();
    assert!(
        answer["result"]["structuredContent"]
            .get("warnings")
            .is_none(),
        "ac_runs with {arguments} warned: {answer}"
    );
    assert_eq!(
        cli, tool,
        "with {arguments}, the CLI's stdout JSON and the ac_runs result differ:\nCLI {cli}\nac_runs {tool}"
    );
    cli
}

/// `actingctl runs --state-root <root>` with `arguments` as flags; its stdout JSON without the
/// line's `outcome`, which is `run_list_reported`.
#[track_caller]
fn cli_runs(root: &Path, arguments: &Value) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actingctl"));
    command.arg("runs").arg("--state-root").arg(root);
    for (name, value) in arguments.as_object().expect("arguments are an object") {
        command.arg(format!("--{}", name.replace('_', "-")));
        command.arg(match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    }
    let output = command.output().expect("run actingctl runs");
    assert!(
        output.status.success(),
        "actingctl runs with {arguments} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl runs with {arguments} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl runs with {arguments} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("run_list_reported"),
        "actingctl runs with {arguments}: {outcome}"
    );
    line
}

/// The scripted Runtime's ledger and status, and the two rows §4.3 gives for its runs.
struct Ledger {
    events: Vec<ProjectedEvent>,
    instances: Vec<RuntimeInstanceStatus>,
    failed_row: Value,
    succeeded_row: Value,
}

impl Ledger {
    fn build() -> Arc<Self> {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance_a = issuer.mint_instance_id().expect("instance id");
        let instance_b = issuer.mint_instance_id().expect("instance id");
        let failure = SharedCode::ContainedTaskHomeEntryNotMatched.as_str();
        let mut events = Vec::new();
        // On instance A, a run started from the CLI whose entry page did not match fails before
        // its package is admitted; then the Lab works on A.
        let failed = record_run(
            &issuer,
            &mut events,
            instance_a,
            Dispatched::Cli,
            vec![
                // Workflow #373 NAV-3b: the entry's pages as a set; none matched.
                TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::Initial,
                    required_pages: vec!["neutral/entry".to_owned()],
                    matched_page: None,
                    matched: false,
                },
                TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::FailClosed,
                    outcome: Some(Outcome::new(SharedCode::ContainedTaskHomeEntryNotMatched)),
                },
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Failure,
                    final_page: None,
                    executed_steps: Some(0),
                    outcome: Some(Outcome::new(SharedCode::ContainedTaskHomeEntryNotMatched)),
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
            ],
        );
        for _ in 0..LAB_CAPTURES {
            push(
                &issuer,
                &mut events,
                EventOrigin::new(EventSource::Lab, OriginModule::Actinglab, EventActor::Lab),
                EventLinksDraft::default().with_instance_id(instance_a),
                CapturePayloadDraft::completed(
                    EventAction::CaptureObserve,
                    EffectDisposition::NotPerformed,
                    16,
                    9,
                    AuditInput::new(),
                )
                .into(),
            );
        }
        // On instance B, a run the scheduler dispatched for one policy task: its entry page
        // matched, it takes one step and succeeds.
        let succeeded = record_run(
            &issuer,
            &mut events,
            instance_b,
            Dispatched::Scheduler {
                alias: ALIAS_B,
                catalog_task_id: POLICY_TASK_B,
            },
            vec![
                // Workflow #373 NAV-3b: the entry's pages as a set; its one page matched.
                TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::Initial,
                    required_pages: vec!["neutral/home".to_owned()],
                    matched_page: Some("neutral/home".to_owned()),
                    matched: true,
                },
                TaskSemanticFact::EntryRecoveryDecision { required: false },
                TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::Started,
                    outcome: None,
                },
                TaskSemanticFact::StepStarted {
                    step_index: 0,
                    operation_label: "open_terminal".to_owned(),
                    from_page: "neutral/home".to_owned(),
                    phase: None,
                },
                TaskSemanticFact::StepFinished {
                    step_index: 0,
                    operation_label: "open_terminal".to_owned(),
                    page_label: "neutral/terminal".to_owned(),
                    phase: None,
                },
                TaskSemanticFact::TerminalCommitted {
                    task_result: TaskOutcome::Success,
                    final_page: Some("neutral/terminal".to_owned()),
                    executed_steps: Some(1),
                    outcome: None,
                    failure_severe: false,
                    scheduling_disposition: None,
                    task_timing: None,
                },
            ],
        );
        let instances = vec![
            RuntimeInstanceStatus::new(
                ALIAS_A,
                *instance_a.transport(),
                false,
                0,
                false,
                false,
                false,
            )
            .expect("instance A status"),
            RuntimeInstanceStatus::new(
                ALIAS_B,
                *instance_b.transport(),
                false,
                0,
                false,
                false,
                false,
            )
            .expect("instance B status"),
        ];
        // §4.3 rows: the CLI run is manual from the CLI and has no policy task id; the
        // scheduler's run is scheduled from the scheduler and carries its policy task id
        // (ruling). admitted_at is the run's first fact and ended_at its terminal.
        let mut failed_row = failed.row(ALIAS_A, "manual", "cli", "failed", "failure");
        failed_row["failure_code"] = json!(failure);
        failed_row["executed_steps"] = json!(0);
        let mut succeeded_row =
            succeeded.row(ALIAS_B, "scheduled", "scheduler", "succeeded", "success");
        succeeded_row["catalog_task_id"] = json!(POLICY_TASK_B);
        succeeded_row["final_page"] = json!("neutral/terminal");
        succeeded_row["executed_steps"] = json!(1);
        Arc::new(Self {
            events,
            instances,
            failed_row,
            succeeded_row,
        })
    }

    fn snapshot(&self) -> u64 {
        self.events.last().map_or(0, |event| event.sequence)
    }
}

/// One recorded run: its links and the sequences of its first and last fact.
struct RecordedRun {
    links: Value,
    first: u64,
    last: u64,
}

impl RecordedRun {
    /// The row §4.3 gives for this run, without its policy task id and the members of its
    /// terminal's detail.
    fn row(
        &self,
        alias: &str,
        dispatch: &str,
        origin: &str,
        state: &str,
        task_result: &str,
    ) -> Value {
        let (first, last) = (timestamp(self.first), timestamp(self.last));
        json!({
            "request_id": self.links["request_id"],
            "run_id": self.links["run_id"],
            "task_id": self.links["task_id"],
            "alias": alias,
            "instance_id": self.links["instance_id"],
            "dispatch": dispatch,
            "origin": origin,
            "state": state,
            "admitted_at_unix_ms": first,
            "ended_at_unix_ms": last,
            "duration_ms": last - first,
            "task_result": task_result,
        })
    }
}

/// How a run was dispatched, as the events before its facts record it.
enum Dispatched {
    /// By hand: its request opens with the CLI's command.
    Cli,
    /// By the scheduler for one policy task: the dispatch intent, under the admission request
    /// and linked to the run, then the scheduler's command under the run's own request.
    Scheduler {
        alias: &'static str,
        catalog_task_id: &'static str,
    },
}

/// Appends one run on `instance`, the events of its dispatch and then its facts, and returns it.
fn record_run(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    instance: IssuedInstanceId,
    dispatched: Dispatched,
    facts: Vec<TaskSemanticFact>,
) -> RecordedRun {
    let correlation_id = issuer.mint_correlation_id().expect("correlation id");
    let task_id = issuer.mint_task_id().expect("task id");
    let run_id = issuer.mint_run_id().expect("run id");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(correlation_id)
        .with_task_id(task_id)
        .with_run_id(run_id);
    match dispatched {
        Dispatched::Cli => push(
            issuer,
            events,
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            links.clone(),
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        ),
        Dispatched::Scheduler {
            alias,
            catalog_task_id,
        } => {
            let admission = EventLinksDraft::default()
                .with_instance_id(instance)
                .with_request_id(issuer.mint_request_id().expect("admission request id"))
                .with_correlation_id(correlation_id)
                .with_task_id(task_id)
                .with_run_id(run_id)
                .with_action_id(issuer.mint_action_id().expect("intent action id"));
            push(
                issuer,
                events,
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Policy,
                    EventActor::Scheduler,
                ),
                admission,
                PolicyPayloadDraft::dispatch_intent(
                    dispatch_intent(alias, catalog_task_id),
                    AuditInput::new(),
                )
                .into(),
            );
            push(
                issuer,
                events,
                EventOrigin::new(
                    EventSource::Scheduler,
                    OriginModule::Scheduler,
                    EventActor::Scheduler,
                ),
                links.clone(),
                CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new())
                    .into(),
            );
        }
    }
    let first = next_sequence(events);
    for fact in facts {
        push(
            issuer,
            events,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        );
    }
    let last = next_sequence(events) - 1;
    RecordedRun {
        links: json!(events.last().expect("the run's facts").links),
        first,
        last,
    }
}

/// The scheduler's dispatch intent for `catalog_task_id` on `alias`. Only its task_id is read;
/// the other members hold neutral values the contract accepts.
fn dispatch_intent(alias: &str, catalog_task_id: &str) -> PolicyDispatchEventData {
    let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
    PolicyDispatchEventData {
        decision_id: "decision.runs.scheduled".to_owned(),
        task_id: catalog_task_id.to_owned(),
        instance_id: alias.to_owned(),
        operation_id: "operation.runs.scheduled".to_owned(),
        package_digest: placeholder('a').into(),
        procedure_binding_digest: placeholder('b'),
        reason_chain_id: "reasons.runs.scheduled".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: placeholder('c'),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "facts.runs.scheduled".to_owned(),
        approval_fact_ids: Vec::new(),
        urgency_milli: 0,
    }
}

fn next_sequence(events: &[ProjectedEvent]) -> u64 {
    u64::try_from(events.len()).expect("ledger length") + 1
}

fn timestamp(sequence: u64) -> u64 {
    SINCE_UNIX_MS + sequence * EVENT_SPACING_MS
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
        timestamp(sequence),
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
            RuntimeOperation::Status => RuntimeResult::Status {
                status: RuntimeControlPlaneStatus::new(owner_epoch, ledger.instances.clone())
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
