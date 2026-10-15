// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #389 MCP-7 (MCP-0 command table v1.1 §2.3, §4.8): `actingctl diagnose` and MCP
//! `ac_diagnose`. Both sides are the real binaries: the MCP server over pipes (fixture I5,
//! `mcp_harness`) and the CLI as one process per call. The Runtime is a scripted one on loopback
//! that answers Health, Status and event queries from one in-memory ledger and records nothing,
//! so every call reads the same snapshot.
//!
//! The parity test: for the same parameters, with and without an instance, the CLI's stdout JSON
//! equals the ac_diagnose result (nothing is cut to the output budget, so §2.2 leaves them no
//! presentation difference), and both carry the two sections §4.8 adds: `runs_grouped`, the
//! failed and open runs grouped by (policy task, failure code, final page), and
//! `failures_grouped`, the `runtime.failed`, `command.rejected` and paused-task records grouped
//! by (type, policy task, code), each the most frequent first. A code is its record's outcome,
//! a relayed spelling the catalog does not register read back as the policy reads it (A3
//! ruling 7). The policy task is named `catalog_task_id`, as the MCP-3 ruling names it beside a
//! run's own task_id.
//!
//! The generation test: an ac_diagnose of a server started on an install root runs the
//! `actingd suspended` of the generation selected at the call, not the one selected when the
//! server started (the stale-source report of 10-13, as the train 4 ruling on MCP-11).
//!
//! Workflow #378 A5: the CLI line carries the outcome `runtime_diagnosis_reported` beside the
//! diagnosis, which is the rest of the line; a report status that is unavailable or failed
//! carries its `outcome`; a tool error is `{class, outcome}`.

use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, DiagnosticCode, DiagnosticDetailDraft,
    EffectDisposition, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventQuery, EventSeverity, EventSource, IdentifierIssuer, IssuedInstanceId,
    LedgerView, OriginModule, OwnerEpoch, PerformanceContext, PolicyDispatchEventData,
    PolicyExecutionEventData, PolicyExecutionOutcome, PolicyFailureClass, PolicyFailureDisposition,
    PolicyFailureRecord, PolicyPayloadDraft, PolicyReasonRecord, ProjectedEvent, ProjectionPayload,
    ProjectionProfile, RUNTIME_INFO_FILE, RuntimeControlPlaneStatus, RuntimeEventQueryCursor,
    RuntimeEventQueryPage, RuntimeEventQueryPageRequest, RuntimeInfo, RuntimeInstanceStatus,
    RuntimeOperation, RuntimePayloadDraft, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest,
    RuntimeResult, SanitizationError, SecretField, SecretFingerprinter, Sha256Fingerprint,
    TaskEntryRecognitionPhase, TaskEntryTargetDisposition, TaskOutcome, TaskPayloadDraft,
    TaskSemanticFact,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

/// The window's start, 2100-01-01: never further back than the 7-day bound of §4.8.
const SINCE_UNIX_MS: u64 = 4_102_444_800_000;
/// The ledger records one event a second from the window's start on.
const EVENT_SPACING_MS: u64 = 1_000;
const ALIAS_A: &str = "neutral.diag.a";
const ALIAS_B: &str = "neutral.diag.b";
/// The policy task the scheduler dispatched the failing runs for: an opaque id from the
/// resource side.
const POLICY_TASK: &str = "neutral.routine.collect";
/// The policy task of the run that succeeded.
const OTHER_POLICY_TASK: &str = "neutral.routine.other";
const FINAL_PAGE: &str = "neutral/terminal";
/// The failure of the runs that failed on `FINAL_PAGE`.
const PAGE_UNKNOWN: SharedCode = SharedCode::ContainedTaskPageUnknown;
/// The failure of the run whose entry page did not match.
const ENTRY_NOT_MATCHED: SharedCode = SharedCode::ContainedTaskHomeEntryNotMatched;
/// Spellings no catalog registers: their records keep them as `raw_code` of
/// `outcome_relay_unregistered`.
const RELAYED_RUN_CODE: &str = "neutral_unlisted_step_failure";
const RELAYED_RUNTIME_CODE: &str = "neutral_unlisted_runtime_failure";
/// The largest request the scripted Runtime reads.
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const ACTINGD: &str = "actingcommand-actingd.exe";

#[test]
fn gate_actingctl_diagnose_and_ac_diagnose_answer_the_same_diagnosis_with_its_groups() {
    let ledger = Ledger::build();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), Arc::clone(&ledger));
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();

    // Every instance: no instance argument.
    let every = parity(
        &mut mcp,
        2,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS}),
    );
    assert_eq!(
        every.get("instance"),
        None,
        "a diagnosis of every instance names no instance: {every}"
    );
    assert_eq!(
        every["runs_grouped"], ledger.every_runs_grouped,
        "runs_grouped of every instance"
    );
    assert_eq!(
        every["failures_grouped"], ledger.every_failures_grouped,
        "failures_grouped of every instance"
    );

    // One instance, named by its alias: as before, with the two groups of that instance only.
    let one = parity(
        &mut mcp,
        3,
        root.path(),
        json!({"since_unix_ms": SINCE_UNIX_MS, "instance": ALIAS_A}),
    );
    assert_eq!(
        one["instance"],
        json!({"alias": ALIAS_A, "instance_id": ledger.instance_a}),
        "the diagnosed instance"
    );
    assert_eq!(
        one["runs_grouped"], ledger.a_runs_grouped,
        "runs_grouped of instance {ALIAS_A}"
    );
    assert_eq!(
        one["failures_grouped"], ledger.a_failures_grouped,
        "failures_grouped of instance {ALIAS_A}"
    );

    mcp.close_input();
    drop(mcp);
    drop(runtime);
}

/// The CLI's stdout JSON for `arguments`, each given as its flag (R2), after checking that it
/// is the ac_diagnose result for the same arguments, that nothing was cut and that MCP warned
/// nothing.
#[track_caller]
fn parity(mcp: &mut McpServer, id: u64, root: &Path, arguments: Value) -> Value {
    let cli = cli_diagnose(root, &arguments);
    let answer = mcp.call_tool(id, "ac_diagnose", arguments.clone());
    let tool = accepted(&answer, "ac_diagnose").clone();
    assert!(
        answer["result"]["structuredContent"]
            .get("warnings")
            .is_none(),
        "ac_diagnose with {arguments} warned: {answer}"
    );
    let cut = tool
        .as_object()
        .map(|members| {
            members
                .keys()
                .filter(|key| key.ends_with("_truncated"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(
        cut.is_empty(),
        "setup: ac_diagnose with {arguments} was cut to the output budget ({cut:?}): {tool}"
    );
    assert_eq!(
        cli, tool,
        "with {arguments}, the CLI's stdout JSON and the ac_diagnose result differ:\nCLI {cli}\nac_diagnose {tool}"
    );
    cli
}

/// `actingctl diagnose --state-root <root>` with `arguments` as flags; its stdout JSON without
/// the line's `outcome`, which is `runtime_diagnosis_reported`.
#[track_caller]
fn cli_diagnose(root: &Path, arguments: &Value) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_actingctl"));
    command.arg("diagnose").arg("--state-root").arg(root);
    for (name, value) in arguments.as_object().expect("arguments are an object") {
        command.arg(format!("--{}", name.replace('_', "-")));
        command.arg(match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        });
    }
    let output = command.output().expect("run actingctl diagnose");
    assert!(
        output.status.success(),
        "actingctl diagnose with {arguments} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut line: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl diagnose with {arguments} printed no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    let Some(outcome) = outcome else {
        panic!("actingctl diagnose with {arguments} printed no outcome: {line}")
    };
    assert_eq!(
        outcome["code"],
        json!("runtime_diagnosis_reported"),
        "actingctl diagnose with {arguments}: {outcome}"
    );
    line
}

/// The scripted Runtime's ledger and status, and the groups §4.8 gives for them.
struct Ledger {
    events: Vec<ProjectedEvent>,
    instances: Vec<RuntimeInstanceStatus>,
    instance_a: Value,
    every_runs_grouped: Value,
    every_failures_grouped: Value,
    a_runs_grouped: Value,
    a_failures_grouped: Value,
}

impl Ledger {
    fn build() -> Arc<Self> {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let instance_a = issuer.mint_instance_id().expect("instance id");
        let instance_b = issuer.mint_instance_id().expect("instance id");
        let mut events = Vec::new();

        // Two scheduled runs of one policy task, on A and on B, fail alike on the same page.
        // The policy lets the first one continue and pauses the task after the second.
        let first = record_run(
            &issuer,
            &mut events,
            instance_a,
            Dispatched::Scheduler(ALIAS_A, POLICY_TASK),
            started_then_failed(Some(Outcome::new(PAGE_UNKNOWN)), Some(FINAL_PAGE)),
        );
        push(
            &issuer,
            &mut events,
            policy_origin(),
            first.links.clone(),
            policy_execution(
                ALIAS_A,
                POLICY_TASK,
                PolicyExecutionOutcome::Failed {
                    failure: policy_failure(PAGE_UNKNOWN, false),
                },
            ),
        );
        let second = record_run(
            &issuer,
            &mut events,
            instance_b,
            Dispatched::Scheduler(ALIAS_B, POLICY_TASK),
            started_then_failed(Some(Outcome::new(PAGE_UNKNOWN)), Some(FINAL_PAGE)),
        );
        let paused = push(
            &issuer,
            &mut events,
            policy_origin(),
            second.links.clone(),
            policy_execution(
                ALIAS_B,
                POLICY_TASK,
                PolicyExecutionOutcome::Failed {
                    failure: policy_failure(PAGE_UNKNOWN, true),
                },
            ),
        );
        // A run started by hand on A whose entry page did not match.
        let manual = record_run(
            &issuer,
            &mut events,
            instance_a,
            Dispatched::Cli,
            vec![
                TaskSemanticFact::EntryRecognition {
                    phase: TaskEntryRecognitionPhase::Initial,
                    required_pages: vec!["neutral/entry".to_owned()],
                    matched_page: None,
                    matched: false,
                },
                TaskSemanticFact::EntryTargetDisposition {
                    disposition: TaskEntryTargetDisposition::FailClosed,
                    outcome: Some(Outcome::new(ENTRY_NOT_MATCHED)),
                },
                terminal(
                    TaskOutcome::Failure,
                    None,
                    0,
                    Some(Outcome::new(ENTRY_NOT_MATCHED)),
                ),
            ],
        );
        // A scheduled run on A that failed with a spelling the catalog does not register.
        let relayed = record_run(
            &issuer,
            &mut events,
            instance_a,
            Dispatched::Scheduler(ALIAS_A, POLICY_TASK),
            started_then_failed(Some(Outcome::relayed(RELAYED_RUN_CODE)), Some(FINAL_PAGE)),
        );
        // A run started by hand on B, still running.
        let open = record_run(
            &issuer,
            &mut events,
            instance_b,
            Dispatched::Cli,
            started_facts(),
        );
        // A scheduled run of another policy task that succeeded: in no group.
        let mut succeeded_facts = started_facts();
        succeeded_facts.push(terminal(TaskOutcome::Success, Some(FINAL_PAGE), 1, None));
        record_run(
            &issuer,
            &mut events,
            instance_a,
            Dispatched::Scheduler(ALIAS_A, OTHER_POLICY_TASK),
            succeeded_facts,
        );
        // The non-run failures: one Runtime failure on A, one refused request on A and one on B.
        let runtime_failed = push(
            &issuer,
            &mut events,
            runtime_origin(),
            EventLinksDraft::default().with_instance_id(instance_a),
            RuntimePayloadDraft::failed(
                DiagnosticCode::RuntimeDiagnostic,
                EffectDisposition::Indeterminate,
                Outcome::relayed(RELAYED_RUNTIME_CODE),
                DiagnosticDetailDraft::new(
                    "runtime_lifecycle",
                    actingcommand_runtime_host::HostLocation::RuntimeLifecycleSessionClose
                        .location(),
                    "runtime_host",
                    "close_execution_session",
                ),
                AuditInput::new(),
            )
            .into(),
        );
        let rejected_a = push_rejected(&issuer, &mut events, instance_a);
        let rejected_b = push_rejected(&issuer, &mut events, instance_b);

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

        // §4.8: a group per (policy task, failure code, final page) with its count, the
        // sequences of its oldest and newest run, the newest run's request_id and the instances
        // by alias; the most frequent first, the most recent first among equals. A manual run
        // has no policy task; an open run has no failure code or final page. The relayed
        // spelling is read back from the terminal's outcome.
        let relayed_group = json!({
            "catalog_task_id": POLICY_TASK,
            "failure_code": RELAYED_RUN_CODE,
            "final_page": FINAL_PAGE,
            "count": 1,
            "first_seq": relayed.first,
            "last_seq": relayed.first,
            "latest_request_id": relayed.request_id(),
            "instances": [ALIAS_A],
        });
        let manual_group = json!({
            "failure_code": ENTRY_NOT_MATCHED.as_str(),
            "count": 1,
            "first_seq": manual.first,
            "last_seq": manual.first,
            "latest_request_id": manual.request_id(),
            "instances": [ALIAS_A],
        });
        let open_group = json!({
            "count": 1,
            "first_seq": open.first,
            "last_seq": open.first,
            "latest_request_id": open.request_id(),
            "instances": [ALIAS_B],
        });
        let every_runs_grouped = json!([
            page_unknown_group(&[&first, &second], &[ALIAS_A, ALIAS_B]),
            open_group,
            relayed_group.clone(),
            manual_group.clone(),
        ]);
        let a_runs_grouped = json!([
            relayed_group,
            manual_group,
            page_unknown_group(&[&first], &[ALIAS_A]),
        ]);

        // §4.8: a group per (type, policy task, code) with its count and sequences; only the
        // policy execution that paused its task counts, and only it names a policy task.
        let rejected = DiagnosticCode::CommandRejected.code().as_str();
        let runtime_group = json!({
            "type": "runtime.failed",
            "code": RELAYED_RUNTIME_CODE,
            "count": 1,
            "first_seq": runtime_failed,
            "last_seq": runtime_failed,
        });
        let every_failures_grouped = json!([
            {
                "type": "command.rejected",
                "code": rejected,
                "count": 2,
                "first_seq": rejected_a,
                "last_seq": rejected_b,
            },
            runtime_group.clone(),
            {
                "type": "policy.execution_recorded",
                "catalog_task_id": POLICY_TASK,
                "code": PAGE_UNKNOWN.as_str(),
                "count": 1,
                "first_seq": paused,
                "last_seq": paused,
            },
        ]);
        let a_failures_grouped = json!([
            {
                "type": "command.rejected",
                "code": rejected,
                "count": 1,
                "first_seq": rejected_a,
                "last_seq": rejected_a,
            },
            runtime_group,
        ]);
        Arc::new(Self {
            events,
            instances,
            instance_a: json!(instance_a.transport()),
            every_runs_grouped,
            every_failures_grouped,
            a_runs_grouped,
            a_failures_grouped,
        })
    }

    fn snapshot(&self) -> u64 {
        self.events.last().map_or(0, |event| event.sequence)
    }
}

/// An entry page that matched and a first step started.
fn started_facts() -> Vec<TaskSemanticFact> {
    vec![
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
    ]
}

/// `started_facts`, then a failed terminal with this outcome on `final_page`.
fn started_then_failed(
    outcome: Option<Outcome>,
    final_page: Option<&str>,
) -> Vec<TaskSemanticFact> {
    let mut facts = started_facts();
    facts.push(terminal(TaskOutcome::Failure, final_page, 1, outcome));
    facts
}

fn terminal(
    task_result: TaskOutcome,
    final_page: Option<&str>,
    executed_steps: u32,
    outcome: Option<Outcome>,
) -> TaskSemanticFact {
    TaskSemanticFact::TerminalCommitted {
        task_result,
        final_page: final_page.map(str::to_owned),
        executed_steps: Some(executed_steps),
        outcome,
        failure_severe: false,
        scheduling_disposition: None,
        task_timing: None,
    }
}

/// The policy's record of how a scheduled run of `task` on `alias` ended.
fn policy_execution(alias: &str, task: &str, outcome: PolicyExecutionOutcome) -> EventPayloadDraft {
    PolicyPayloadDraft::execution_recorded(
        PolicyExecutionEventData {
            decision_id: "decision.diag.scheduled".to_owned(),
            task_id: task.to_owned(),
            instance_id: alias.to_owned(),
            observed_at_unix_ms: SINCE_UNIX_MS,
            outcome,
        },
        AuditInput::new(),
    )
    .into()
}

/// A failure the policy recorded: recoverable and continuing, or severe and pausing the task.
fn policy_failure(failure: SharedCode, paused: bool) -> PolicyFailureRecord {
    let class = if paused {
        PolicyFailureClass::Severe
    } else {
        PolicyFailureClass::Recoverable
    };
    PolicyFailureRecord {
        outcome: Outcome::new(failure),
        failure_identity: None,
        reported_success: false,
        original_class: class,
        effective_class: class,
        consecutive_same_error: 1,
        escalation_streak: 1,
        performance_tax_exempt: false,
        retry_attempt: 0,
        disposition: if paused {
            PolicyFailureDisposition::PausedTask
        } else {
            PolicyFailureDisposition::Continue
        },
        retry_at_unix_ms: None,
        runtime_ms: 1_000,
        sensitive: false,
        perf_context: Box::new(PerformanceContext::unavailable(SINCE_UNIX_MS)),
        condition: None,
    }
}

/// A refused request on `instance`; returns its sequence.
fn push_rejected(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    instance: IssuedInstanceId,
) -> u64 {
    push(
        issuer,
        events,
        runtime_origin(),
        EventLinksDraft::default()
            .with_instance_id(instance)
            .with_request_id(issuer.mint_request_id().expect("request id"))
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id")),
        CommandPayloadDraft::rejected(
            EventAction::RuntimeTaskRun,
            DiagnosticCode::CommandRejected,
            EffectDisposition::NotPerformed,
            AuditInput::new(),
        )
        .into(),
    )
}

fn runtime_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Runtime,
        OriginModule::Runtime,
        EventActor::Runtime,
    )
}

fn policy_origin() -> EventOrigin {
    EventOrigin::new(
        EventSource::Scheduler,
        OriginModule::Policy,
        EventActor::Scheduler,
    )
}

/// One recorded run: its links and the sequence of its first fact, which is its first listing
/// fact (its entry check).
struct RecordedRun {
    links: EventLinksDraft,
    projected: Value,
    first: u64,
}

impl RecordedRun {
    fn request_id(&self) -> Value {
        self.projected["request_id"].clone()
    }
}

/// How a run was dispatched, as the events before its facts record it.
enum Dispatched {
    /// By hand: its request opens with the CLI's command.
    Cli,
    /// By the scheduler for one policy task on one alias: the dispatch intent, under the
    /// admission request and linked to the run, then the scheduler's command under the run's own
    /// request.
    Scheduler(&'static str, &'static str),
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
        Dispatched::Cli => {
            push(
                issuer,
                events,
                EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
                links.clone(),
                ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new())
                    .into(),
            );
        }
        Dispatched::Scheduler(alias, catalog_task_id) => {
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
                policy_origin(),
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
            runtime_origin(),
            links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        );
    }
    RecordedRun {
        links,
        projected: json!(events.last().expect("the run's facts").links),
        first,
    }
}

/// The scheduler's dispatch intent for `catalog_task_id` on `alias`. Only its task_id is read;
/// the other members hold neutral values the contract accepts.
fn dispatch_intent(alias: &str, catalog_task_id: &str) -> PolicyDispatchEventData {
    let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
    PolicyDispatchEventData {
        decision_id: "decision.diag.scheduled".to_owned(),
        task_id: catalog_task_id.to_owned(),
        instance_id: alias.to_owned(),
        operation_id: "operation.diag.scheduled".to_owned(),
        procedure_binding_digest: placeholder('b'),
        reason_chain_id: "reasons.diag.scheduled".to_owned(),
        reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
        catalog_hash: placeholder('c'),
        catalog_version: 1,
        input_ledger_position: 1,
        fact_snapshot_id: "facts.diag.scheduled".to_owned(),
        approval_fact_ids: Vec::new(),
        urgency_milli: 0,
    }
}

fn next_sequence(events: &[ProjectedEvent]) -> u64 {
    u64::try_from(events.len()).expect("ledger length") + 1
}

/// The `runs_grouped` group of the runs that failed with `PAGE_UNKNOWN` on `FINAL_PAGE`, oldest
/// first, on these instances.
fn page_unknown_group(runs: &[&RecordedRun], instances: &[&str]) -> Value {
    json!({
        "catalog_task_id": POLICY_TASK,
        "failure_code": PAGE_UNKNOWN.as_str(),
        "final_page": FINAL_PAGE,
        "count": runs.len(),
        "first_seq": runs.first().expect("a run").first,
        "last_seq": runs.last().expect("a run").first,
        "latest_request_id": runs.last().expect("a run").request_id(),
        "instances": instances,
    })
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

/// Appends one event, as a Forensic read projects it, at the next sequence; returns that
/// sequence. Its severity is the one sanitizing derives from its outcome (ledger revision 3).
fn push(
    issuer: &IdentifierIssuer,
    events: &mut Vec<ProjectedEvent>,
    origin: EventOrigin,
    links: EventLinksDraft,
    payload: EventPayloadDraft,
) -> u64 {
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
    sequence
}

/// A Runtime on loopback that answers every connection from one ledger until dropped.
struct ScriptedRuntime {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl ScriptedRuntime {
    /// Listens on loopback and writes `runtime-info.json` under `state_root` naming it.
    fn start(state_root: &Path, ledger: Arc<Ledger>) -> Self {
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
            state_root.join(RUNTIME_INFO_FILE),
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
                page: event_page(ledger, query, *profile, page),
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
    ledger: &Ledger,
    query: &EventQuery,
    profile: ProjectionProfile,
    request: &RuntimeEventQueryPageRequest,
) -> RuntimeEventQueryPage {
    let snapshot = ledger.snapshot();
    if let Some(position) = request.snapshot_position() {
        assert_eq!(position, snapshot, "a read at another snapshot");
    }
    let after = request
        .cursor()
        .map_or(0, RuntimeEventQueryCursor::after_sequence);
    let mut matching = ledger
        .events
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

/// Whether `event` matches `query`. This ledger serves the errors view (warning and above), the
/// time and sequence bounds, one event type and the run links; a query with any other filter
/// is refused.
fn matches(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let unserved = EventQuery {
        view: None,
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
    let view = match query.view.as_ref() {
        None => true,
        Some(LedgerView::Errors) => event.severity >= EventSeverity::Warning,
        Some(other) => panic!("the scripted ledger serves no view {other:?}"),
    };
    let links = &event.links;
    view && query
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

/// The generation test (Workflow #389 MCP-7 and its scope extension): an install root with slots A
/// and B, generation 1 selected on slot A. The MCP server is the install's own actingctl in slot A,
/// started as the fixed entry `<root>\runtime\actingctl.exe` starts it: with the install root and
/// the selection of its start inherited, which it holds for its whole life. The programs it runs
/// are stand-ins, actingctl itself, which refuses their arguments as usage: it prints its one
/// result line, the refusal `{outcome}`, and exits 1 (Workflow #378 A5, model §7.3):
/// `<slot>\runtime\actingcommand-actingd.exe` (actingd suspended, actingd check-config) and
/// `<root>\tools\actinglab.exe` (the Lab tools). A stand-in that
/// ran leaves its exit code 1 in the answer: ac_diagnose's report status is `failed` with
/// `suspended_report_failed` and `exit_code`, ac_binding_draft's check_config has `exit_code`. A
/// program that is not there leaves no exit code (a spawn failure).
///
/// A deploy then selects generation 2 on slot B, and the actingd stand-in now stands only in
/// slot B. Each tool answers the generation selected now: ac_diagnose and ac_binding_draft's
/// check-config run slot B's program (with the start generation they would find none in slot A
/// and answer a spawn failure), and ac_packs names generation 2's configuration. When the install
/// root then has no selection at all, ac_packs and ac_binding_draft answer the CLI's
/// `install_selection_unavailable`: each reads `install\active.json` at the call (Workflow #336
/// PKG-S4: ac_run_pack runs no package digest child any more and reads no selection). The
/// actinglab children
/// differ only in the selection handed to them, which a stand-in cannot report; they take it from
/// the same per-call reading.
#[cfg(windows)]
#[test]
fn gate_tools_of_a_server_the_fixed_entry_started_answer_the_generation_selected_now() {
    use actingcommand_contract::{INSTALL_ROOT_ENV, INSTALL_SELECTION_ENV};

    let temp = TempDir::new().expect("tempdir");
    let root = temp.path().join("install");
    let state = root.join("state");
    let package = temp.path().join("neutral-package");
    for directory in [
        state.clone(),
        package.clone(),
        root.join("A").join("runtime"),
        root.join("B").join("runtime"),
        root.join("tools"),
        root.join("install").join("generations").join("1"),
        root.join("install").join("generations").join("2"),
    ] {
        fs::create_dir_all(&directory).expect("create fixture directory");
    }
    for slot in ["A", "B"] {
        fs::write(root.join("install").join(format!("slot-{slot}.lock")), b"")
            .expect("write slot lock");
    }
    let actingctl = env!("CARGO_BIN_EXE_actingctl");
    let server = root.join("A").join("runtime").join("actingctl.exe");
    fs::copy(actingctl, &server).expect("place the server");
    fs::copy(actingctl, root.join("tools").join("actinglab.exe"))
        .expect("place the actinglab stand-in");
    let stand_in = |slot: &str| root.join(slot).join("runtime").join(ACTINGD);
    fs::copy(actingctl, stand_in("A")).expect("place the actingd stand-in");
    let config_path =
        |generation: u64| format!("install/generations/{generation}/actingd.config.json");
    // Writes the slot's members and the generation's configuration, selects them in
    // `install\active.json` and returns the selection.
    let select = |slot: &str, generation: u64| -> String {
        let members = format!(r#"{{"fixture":"diagnose-generation","slot":"{slot}"}}"#);
        fs::write(root.join(slot).join("MEMBERS.json"), &members).expect("write members");
        let config =
            json!({"state_root": state, "instances": [], "generation": generation}).to_string();
        fs::write(root.join(config_path(generation)), &config).expect("write the configuration");
        let selection = json!({
            "schema_version": "actingcommand.install-selection.v1",
            "slot": slot,
            "generation": generation,
            "members": {
                "path": format!("{slot}/MEMBERS.json"),
                "sha256": format!("{:x}", Sha256::digest(members.as_bytes())),
            },
            "config": {
                "path": config_path(generation),
                "sha256": format!("{:x}", Sha256::digest(config.as_bytes())),
            },
        })
        .to_string();
        fs::write(root.join("install").join("active.json"), &selection)
            .expect("write the selection");
        selection
    };
    let started_with = select("A", 1);
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = issuer.mint_instance_id().expect("instance id");
    let runtime = ScriptedRuntime::start(
        &state,
        Arc::new(Ledger {
            events: Vec::new(),
            instances: vec![
                RuntimeInstanceStatus::new(
                    ALIAS_A,
                    *instance.transport(),
                    false,
                    0,
                    false,
                    false,
                    false,
                )
                .expect("instance status"),
            ],
            instance_a: Value::Null,
            every_runs_grouped: Value::Null,
            every_failures_grouped: Value::Null,
            a_runs_grouped: Value::Null,
            a_failures_grouped: Value::Null,
        }),
    );
    let mut mcp = McpServer::spawn_program(
        &server,
        &["--tier", "observer,operator,author"],
        &[
            (INSTALL_ROOT_ENV, root.to_str().expect("UTF-8 fixture root")),
            (INSTALL_SELECTION_ENV, started_with.as_str()),
        ],
    );
    mcp.initialize();
    let mut id = 1;
    let mut call = |mcp: &mut McpServer, tool: &str, arguments: Value| {
        id += 1;
        mcp.call_tool(id, tool, arguments)
    };
    let report = |answer: &Value| accepted(answer, "ac_diagnose")["suspended_report"].clone();
    let packs_config = |answer: &Value| {
        accepted(answer, "ac_packs")["source"]["config"]
            .as_str()
            .unwrap_or_else(|| panic!("ac_packs named no configuration: {answer}"))
            .replace('\\', "/")
    };
    let record_stop = json!({
        "lab": {
            "binding_example": {
                "scheduled_execution": {"package_path": package.display().to_string()},
            },
            "prerequisite_entry_example": {},
            "catalog_on_failure_example": {},
            "binding_requires": [],
        },
    });

    // Generation 1: slot A's actingd runs, and ac_packs names generation 1's configuration.
    let before = report(&call(&mut mcp, "ac_diagnose", json!({})));
    assert_eq!(
        (
            before["outcome"]["code"].clone(),
            before["exit_code"].clone()
        ),
        (json!("suspended_report_failed"), json!(1)),
        "the actingd stand-in in slot A ran: {before}"
    );
    let config = packs_config(&call(&mut mcp, "ac_packs", json!({})));
    assert!(
        config.ends_with(&config_path(1)),
        "ac_packs read {config}, not generation 1"
    );

    // A deploy selects generation 2 on slot B; the actingd stand-in now stands only there.
    fs::rename(stand_in("A"), stand_in("B")).expect("move the actingd stand-in to slot B");
    select("B", 2);
    let after = report(&call(&mut mcp, "ac_diagnose", json!({})));
    assert_eq!(
        (after["outcome"]["code"].clone(), after["exit_code"].clone()),
        (json!("suspended_report_failed"), json!(1)),
        "ac_diagnose ran actingd suspended of the generation it started with, not of the one selected now: {after}"
    );
    let config = packs_config(&call(&mut mcp, "ac_packs", json!({})));
    assert!(
        config.ends_with(&config_path(2)),
        "ac_packs read {config}, not generation 2 selected now"
    );
    let answer = call(
        &mut mcp,
        "ac_binding_draft",
        json!({"record_stop": record_stop.clone()}),
    );
    let check_config = accepted(&answer, "ac_binding_draft")["check_config"].clone();
    assert_eq!(
        check_config["exit_code"],
        json!(1),
        "ac_binding_draft ran actingd check-config of the generation it started with, not of the one selected now: {check_config}"
    );

    // The install root no longer has a selection: each tool reads that at the call and answers
    // the CLI's code.
    fs::remove_file(root.join("install").join("active.json")).expect("remove the selection");
    for (tool, arguments) in [
        ("ac_packs", json!({})),
        ("ac_binding_draft", json!({"record_stop": record_stop})),
    ] {
        let answer = call(&mut mcp, tool, arguments);
        assert_eq!(
            answer["result"]["structuredContent"]["error"]["outcome"]["code"],
            json!("install_selection_unavailable"),
            "{tool} without a selection: {answer}"
        );
    }

    mcp.close_input();
    drop(mcp);
    drop(runtime);
}
