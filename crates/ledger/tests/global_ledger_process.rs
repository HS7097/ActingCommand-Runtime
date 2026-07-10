// SPDX-License-Identifier: AGPL-3.0-only

use std::cell::Cell;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use actingcommand_contract::{
    ClassifiedField, ClientActionKind, ClientPayload, ClientPayloadDraft, EventActor, EventDraft,
    EventLinks, EventOrigin, EventQuery, EventSeverity, EventSource, EventType, InputPayload,
    InputPayloadDraft, InputTransition, PayloadKind, ProjectionProfile, SanitizationError,
    SchedulerDecision, SchedulerPayload, SchedulerPayloadDraft,
};
use actingcommand_ledger::critical::{CriticalEventPlan, CriticalExecutionError, execute_critical};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, Sha256FieldRedactor};
use tempfile::TempDir;

const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_LEDGER_PROCESS_ROOT";
const CHILD_READY_ENV: &str = "ACTINGCOMMAND_LEDGER_PROCESS_READY";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

fn config(root: &Path, owner_id: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(root, owner_id)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn redactor() -> Sha256FieldRedactor {
    Sha256FieldRedactor::new(b"global-ledger-process-test-salt").expect("redactor")
}

fn client_event(
    event_id: &str,
    kind: ClientActionKind,
    source: EventSource,
    actor: EventActor,
    correlation_id: Option<&str>,
    fields: Vec<ClassifiedField>,
) -> actingcommand_contract::SanitizedEventDraft<ClientPayload> {
    client_draft(event_id, kind, source, actor, correlation_id, fields)
        .sanitize(&redactor())
        .expect("sanitize client event")
}

fn client_draft(
    event_id: &str,
    kind: ClientActionKind,
    source: EventSource,
    actor: EventActor,
    correlation_id: Option<&str>,
    fields: Vec<ClassifiedField>,
) -> EventDraft<ClientPayloadDraft> {
    EventDraft::new(
        event_id,
        1_752_147_200_000,
        kind.event_type(),
        EventSeverity::Info,
        EventOrigin::new(source, "process-test", actor).expect("origin"),
        EventLinks {
            correlation_id: correlation_id.map(str::to_string),
            ..EventLinks::default()
        },
        ClientPayloadDraft::new(kind, "process.acceptance", fields).expect("client payload"),
    )
}

struct FailingBoundaryRedactor;

impl actingcommand_contract::FieldRedactor for FailingBoundaryRedactor {
    fn fingerprint(&self, _field_name: &str, _value: &str) -> Result<String, SanitizationError> {
        Err(SanitizationError::redactor_failure())
    }
}

fn scheduler_event(
    event_id: &str,
    correlation_id: &str,
) -> actingcommand_contract::SanitizedEventDraft<SchedulerPayload> {
    let decision = SchedulerDecision::Admitted;
    EventDraft::new(
        event_id,
        1_752_147_200_000,
        decision.event_type(),
        EventSeverity::Info,
        EventOrigin::new(EventSource::Scheduler, "scheduler", EventActor::Scheduler)
            .expect("origin"),
        EventLinks {
            correlation_id: Some(correlation_id.to_string()),
            ..EventLinks::default()
        },
        SchedulerPayloadDraft::new(decision, "schedule.admit", vec![]).expect("scheduler payload"),
    )
    .sanitize(&redactor())
    .expect("sanitize scheduler event")
}

fn input_event(
    event_id: &str,
    transition: InputTransition,
    correlation_id: &str,
    action_id: &str,
) -> actingcommand_contract::SanitizedEventDraft<InputPayload> {
    EventDraft::new(
        event_id,
        1_752_147_200_000,
        transition.event_type(),
        EventSeverity::Info,
        EventOrigin::new(EventSource::Device, "device-proxy", EventActor::Runtime).expect("origin"),
        EventLinks {
            correlation_id: Some(correlation_id.to_string()),
            action_id: Some(action_id.to_string()),
            ..EventLinks::default()
        },
        InputPayloadDraft::new(transition, "input.tap", vec![]).expect("input payload"),
    )
    .sanitize(&redactor())
    .expect("sanitize input event")
}

#[test]
fn ledger_writer_process_child() {
    let (Ok(root), Ok(ready_path)) = (env::var(CHILD_ROOT_ENV), env::var(CHILD_READY_ENV)) else {
        return;
    };
    let ledger = GlobalLedger::open(config(Path::new(&root), "child-writer"))
        .expect("child opens global ledger");
    ledger
        .append(client_event(
            "child-cli-command",
            ClientActionKind::CliCommand,
            EventSource::Cli,
            EventActor::Cli,
            Some("process-recovery-correlation"),
            vec![],
        ))
        .expect("child appends durable event");
    fs::write(ready_path, "ready").expect("child writes ready marker");

    loop {
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn hard_killed_writer_releases_os_lock_and_records_recovery() {
    let temp = TempDir::new().expect("temp root");
    let ready_path = temp.path().join("child-ready");
    let executable = env::current_exe().expect("test executable");
    let mut child = Command::new(executable)
        .args(["--exact", "ledger_writer_process_child", "--nocapture"])
        .env(CHILD_ROOT_ENV, temp.path())
        .env(CHILD_READY_ENV, &ready_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ledger writer child");

    wait_for_ready(&mut child, &ready_path);
    child.kill().expect("hard-kill writer child");
    child.wait().expect("wait for killed writer child");

    let ledger = GlobalLedger::open(config(temp.path(), "recovery-parent"))
        .expect("reopen ledger after hard kill");
    let events = ledger
        .query(EventQuery::default())
        .expect("query recovered ledger");
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2],
        "recovery preserves contiguous sequence numbers"
    );
    assert_eq!(events[0].event_id, "child-cli-command");
    assert_eq!(events[1].event_type, EventType::LedgerRecovered);
    ledger.close().expect("close recovered ledger");
}

#[test]
fn five_sources_share_one_correlated_ledger() {
    let temp = TempDir::new().expect("temp root");
    let ledger = GlobalLedger::open(config(temp.path(), "correlation-writer")).expect("ledger");
    let correlation_id = "all-sources-correlation";
    let action_id = "all-sources-action";

    ledger
        .append(client_event(
            "cli-command",
            ClientActionKind::CliCommand,
            EventSource::Cli,
            EventActor::Cli,
            Some(correlation_id),
            vec![],
        ))
        .expect("append cli command");
    ledger
        .append(scheduler_event("scheduler-decision", correlation_id))
        .expect("append scheduler decision");
    let plan = CriticalEventPlan::new(
        input_event(
            "device-input-intent",
            InputTransition::Intent,
            correlation_id,
            action_id,
        )
        .erase()
        .expect("erase input intent"),
        input_event(
            "device-input-outcome",
            InputTransition::Committed,
            correlation_id,
            action_id,
        )
        .erase()
        .expect("erase input outcome"),
        input_event(
            "device-input-failure",
            InputTransition::Failed,
            correlation_id,
            action_id,
        )
        .erase()
        .expect("erase input failure"),
    )
    .expect("critical input plan");
    execute_critical(&ledger, plan, || Ok::<(), ()>(())).expect("record input outcome");
    ledger
        .append(client_event(
            "ui-action",
            ClientActionKind::UiAction,
            EventSource::Ui,
            EventActor::Ui,
            Some(correlation_id),
            vec![],
        ))
        .expect("append UI action");
    ledger
        .append(client_event(
            "lab-request",
            ClientActionKind::LabRequest,
            EventSource::Lab,
            EventActor::Lab,
            Some(correlation_id),
            vec![],
        ))
        .expect("append Lab request");

    let events = ledger
        .query(EventQuery {
            correlation_id: Some(correlation_id.to_string()),
            ..EventQuery::default()
        })
        .expect("query correlated events");
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "cli-command",
            "scheduler-decision",
            "device-input-intent",
            "device-input-outcome",
            "ui-action",
            "lab-request",
        ]
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.origin.source())
            .collect::<Vec<_>>(),
        vec![
            EventSource::Cli,
            EventSource::Scheduler,
            EventSource::Device,
            EventSource::Device,
            EventSource::Ui,
            EventSource::Lab,
        ]
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    ledger.close().expect("close correlated ledger");
}

#[test]
fn secret_injection_is_absent_from_files_queries_errors_and_projections() {
    const TOKEN: &str = "token-secret-7d141b7b";
    const ACCOUNT: &str = "account-secret-5a9c8f3e";
    const MACHINE_PATH: &str = r"C:\Users\process-secret\runtime-state";
    const ENDPOINT: &str = "https://process-secret.example.invalid:4876/api";
    const CORRELATION_ID: &str = "redaction-index-correlation";
    let sentinels = [TOKEN, ACCOUNT, MACHINE_PATH, ENDPOINT];

    let secret_fields = || {
        vec![
            ClassifiedField::secret_fingerprint("access-token", TOKEN).expect("token field"),
            ClassifiedField::secret_fingerprint("account-id", ACCOUNT).expect("account field"),
            ClassifiedField::secret_fingerprint("machine-path", MACHINE_PATH)
                .expect("machine path field"),
            ClassifiedField::secret_fingerprint("endpoint", ENDPOINT).expect("endpoint field"),
        ]
    };

    let temp = TempDir::new().expect("temp root");
    let ledger = GlobalLedger::open(config(temp.path(), "redaction-writer")).expect("ledger");
    let event = client_event(
        "secret-bearing-command",
        ClientActionKind::CliCommand,
        EventSource::Cli,
        EventActor::Cli,
        Some(CORRELATION_ID),
        secret_fields(),
    );
    ledger.append(event.clone()).expect("append redacted event");
    ledger.close().expect("close before recovered index query");

    let ledger = GlobalLedger::open(config(temp.path(), "redaction-query-writer"))
        .expect("reopen redaction ledger");
    let query = ledger
        .query(EventQuery {
            correlation_id: Some(CORRELATION_ID.to_string()),
            ..EventQuery::default()
        })
        .expect("query recovered correlation index");
    assert_eq!(query.len(), 1, "indexed query must retain the event");
    assert_eq!(query[0].event_id, "secret-bearing-command");

    let duplicate = ledger
        .append(event)
        .expect_err("duplicate append must surface an error");
    let projections = [
        (
            "CLI projection",
            ledger
                .project(
                    EventQuery {
                        correlation_id: Some(CORRELATION_ID.to_string()),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Cli,
                )
                .expect("CLI projection"),
        ),
        (
            "UI projection",
            ledger
                .project(
                    EventQuery {
                        correlation_id: Some(CORRELATION_ID.to_string()),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Ui,
                )
                .expect("UI projection"),
        ),
        (
            "Lab projection",
            ledger
                .project(
                    EventQuery {
                        correlation_id: Some(CORRELATION_ID.to_string()),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Lab,
                )
                .expect("Lab projection"),
        ),
    ];
    for (label, projection) in &projections {
        assert_eq!(projection.len(), 1, "{label} must retain the event");
        assert_eq!(projection[0].event_id, "secret-bearing-command");
        let projection_text = serde_json::to_string(projection).expect("serialize projection");
        assert_sentinels_absent(label, &projection_text, &sentinels);
    }
    let boundary_error = client_draft(
        "redaction-boundary-error",
        ClientActionKind::CliCommand,
        EventSource::Cli,
        EventActor::Cli,
        Some(CORRELATION_ID),
        secret_fields(),
    )
    .sanitize(&FailingBoundaryRedactor)
    .expect_err("redaction-boundary failure must surface");
    assert_eq!(boundary_error.code(), "redactor_failed");

    let query_text = serde_json::to_string(&query).expect("serialize query");
    let error_text = format!("{duplicate:?}{duplicate}{boundary_error:?}{boundary_error}");
    ledger.close().expect("close redaction ledger");
    let observed = [
        ("durable files", read_tree_text(temp.path())),
        ("indexed query", query_text),
        ("errors", error_text),
    ];
    for (surface, value) in observed {
        assert_sentinels_absent(surface, &value, &sentinels);
    }
}

#[test]
fn critical_append_failure_blocks_side_effect() {
    let temp = TempDir::new().expect("temp root");
    let ledger = GlobalLedger::open(config(temp.path(), "critical-writer")).expect("ledger");
    let intent = input_event(
        "duplicate-critical-intent",
        InputTransition::Intent,
        "critical-correlation",
        "critical-action",
    );
    ledger
        .append(intent.clone())
        .expect("seed duplicate critical intent");
    let plan = CriticalEventPlan::new(
        intent.erase().expect("erase intent"),
        input_event(
            "critical-success",
            InputTransition::Committed,
            "critical-correlation",
            "critical-action",
        )
        .erase()
        .expect("erase success"),
        input_event(
            "critical-failure",
            InputTransition::Failed,
            "critical-correlation",
            "critical-action",
        )
        .erase()
        .expect("erase failure"),
    )
    .expect("critical plan");
    let action_calls = Cell::new(0);

    let error = execute_critical(&ledger, plan, || {
        action_calls.set(action_calls.get() + 1);
        Ok::<(), ()>(())
    })
    .expect_err("intent append failure must block side effect");

    assert!(matches!(
        error,
        CriticalExecutionError::IntentAppend(ref source) if source.code() == "duplicate_event_id"
    ));
    assert_eq!(action_calls.get(), 0);
    ledger.close().expect("close critical ledger");
}

fn wait_for_ready(child: &mut std::process::Child, ready_path: &Path) {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if ready_path.exists() {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll writer child") {
            panic!("writer child exited before ready marker: {status}");
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed-out writer child");
            let _ = child.wait();
            panic!("writer child did not create ready marker before timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_tree_text(root: &Path) -> String {
    let mut text = String::new();
    for entry in fs::read_dir(root).expect("read ledger root") {
        let entry = entry.expect("read ledger entry");
        let path = entry.path();
        if path.is_dir() {
            text.push_str(&read_tree_text(&path));
        } else {
            text.push_str(&String::from_utf8_lossy(
                &fs::read(path).expect("read ledger file"),
            ));
        }
    }
    text
}

fn assert_sentinels_absent(surface: &str, value: &str, sentinels: &[&str]) {
    for sentinel in sentinels {
        assert!(
            !value.contains(sentinel),
            "{surface} must not disclose a redaction sentinel"
        );
    }
}
