// SPDX-License-Identifier: AGPL-3.0-only

use std::cell::Cell;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, DiagnosticCode, EffectDisposition, EventAction, EventActor,
    EventDraft, EventLinksDraft, EventOrigin, EventQuery, EventSeverity, EventSource, EventType,
    IdentifierIssuer, InputPayloadDraft, IssuedActionId, IssuedCorrelationId, IssuedEventId,
    OriginModule, ProjectionProfile, SanitizationError, SchedulerPayloadDraft, SecretField,
    SecretFingerprinter, Sha256Fingerprint,
};
use actingcommand_ledger::critical::{CriticalEventPlan, CriticalExecutionError, execute_critical};
use actingcommand_ledger::{GlobalLedger, GlobalLedgerConfig, Sha256SecretFingerprinter};
use tempfile::TempDir;

const CHILD_ROOT_ENV: &str = "ACTINGCOMMAND_LEDGER_PROCESS_ROOT";
const CHILD_READY_ENV: &str = "ACTINGCOMMAND_LEDGER_PROCESS_READY";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum ClientKind {
    UiAction,
    CliCommand,
    LabRequest,
}

#[derive(Clone, Copy)]
enum InputKind {
    Intent,
    Committed,
    Failed,
}

fn identifiers() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

fn event_id(_value: &str) -> IssuedEventId {
    identifiers().mint_event_id().expect("event id")
}

macro_rules! cached_identifier {
    ($function:ident, $type:ty, $mint:ident, $label:literal) => {
        fn $function(value: &str) -> $type {
            static IDS: OnceLock<Mutex<HashMap<String, $type>>> = OnceLock::new();
            let mut ids = IDS
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .expect(concat!($label, " registry"));
            *ids.entry(value.to_string())
                .or_insert_with(|| identifiers().$mint().expect($label))
        }
    };
}

cached_identifier!(
    correlation_id,
    IssuedCorrelationId,
    mint_correlation_id,
    "correlation id"
);
cached_identifier!(action_id, IssuedActionId, mint_action_id, "action id");

fn config(root: &Path, owner_id: &str) -> GlobalLedgerConfig {
    GlobalLedgerConfig::new(root, owner_id)
        .with_segment_max_bytes(16 * 1024)
        .with_ingress_capacity(8)
}

fn fingerprinter() -> Sha256SecretFingerprinter {
    Sha256SecretFingerprinter::new(b"global-ledger-process-test-salt").expect("fingerprinter")
}

fn client_event(
    event_id: &str,
    kind: ClientKind,
    source: EventSource,
    actor: EventActor,
    correlation_id: Option<&str>,
    audit: AuditInput,
) -> actingcommand_contract::SanitizedEventDraft {
    client_draft(event_id, kind, source, actor, correlation_id, audit)
        .sanitize(&fingerprinter())
        .expect("sanitize client event")
}

fn client_draft(
    event_id: &str,
    kind: ClientKind,
    source: EventSource,
    actor: EventActor,
    correlation_id: Option<&str>,
    audit: AuditInput,
) -> EventDraft {
    let payload = match kind {
        ClientKind::UiAction => {
            ClientPayloadDraft::ui_action(EventAction::ProcessAcceptance, audit)
        }
        ClientKind::CliCommand => {
            ClientPayloadDraft::cli_command(EventAction::ProcessAcceptance, audit)
        }
        ClientKind::LabRequest => {
            ClientPayloadDraft::lab_request(EventAction::ProcessAcceptance, audit)
        }
    };
    let mut links = EventLinksDraft::default();
    if let Some(value) = correlation_id {
        links = links.with_correlation_id(self::correlation_id(value));
    }
    EventDraft::new(
        self::event_id(event_id),
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(source, OriginModule::ProcessTest, actor),
        links,
        payload.into(),
    )
}

struct FailingBoundaryRedactor;

impl SecretFingerprinter for FailingBoundaryRedactor {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        Err(SanitizationError::fingerprinter_failure())
    }
}

fn scheduler_event(
    event_id: &str,
    correlation_id: &str,
) -> actingcommand_contract::SanitizedEventDraft {
    EventDraft::new(
        self::event_id(event_id),
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
        ),
        EventLinksDraft::default().with_correlation_id(self::correlation_id(correlation_id)),
        SchedulerPayloadDraft::admitted(EventAction::ScheduleAdmit, AuditInput::new()).into(),
    )
    .sanitize(&fingerprinter())
    .expect("sanitize scheduler event")
}

fn input_event(
    event_id: &str,
    transition: InputKind,
    correlation_id: &str,
    action_id: &str,
) -> actingcommand_contract::SanitizedEventDraft {
    let payload = match transition {
        InputKind::Intent => InputPayloadDraft::intent(EventAction::InputTap, AuditInput::new()),
        InputKind::Committed => InputPayloadDraft::committed(
            EventAction::InputTap,
            EffectDisposition::Performed,
            AuditInput::new(),
        ),
        InputKind::Failed => InputPayloadDraft::failed(
            EventAction::InputTap,
            DiagnosticCode::InputFailed,
            EffectDisposition::Indeterminate,
            AuditInput::new(),
        ),
    };
    EventDraft::new(
        self::event_id(event_id),
        1_752_147_200_000,
        EventSeverity::Info,
        EventOrigin::new(
            EventSource::Device,
            OriginModule::DeviceProxy,
            EventActor::Runtime,
        ),
        EventLinksDraft::default()
            .with_correlation_id(self::correlation_id(correlation_id))
            .with_action_id(self::action_id(action_id)),
        payload.into(),
    )
    .sanitize(&fingerprinter())
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
            ClientKind::CliCommand,
            EventSource::Cli,
            EventActor::Cli,
            Some("process-recovery-correlation"),
            AuditInput::new(),
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
            .map(|event| event.sequence())
            .collect::<Vec<_>>(),
        vec![1, 2],
        "recovery preserves contiguous sequence numbers"
    );
    assert_eq!(events[0].event_type(), EventType::CliCommand);
    assert_eq!(events[1].event_type(), EventType::LedgerRecovered);
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
            ClientKind::CliCommand,
            EventSource::Cli,
            EventActor::Cli,
            Some(correlation_id),
            AuditInput::new(),
        ))
        .expect("append cli command");
    ledger
        .append(scheduler_event("scheduler-decision", correlation_id))
        .expect("append scheduler decision");
    let plan = CriticalEventPlan::new(
        input_event(
            "device-input-intent",
            InputKind::Intent,
            correlation_id,
            action_id,
        ),
        input_event(
            "device-input-outcome",
            InputKind::Committed,
            correlation_id,
            action_id,
        ),
        input_event(
            "device-input-failure",
            InputKind::Failed,
            correlation_id,
            action_id,
        ),
    )
    .expect("critical input plan");
    execute_critical(&ledger, plan, || Ok::<(), ()>(())).expect("record input outcome");
    ledger
        .append(client_event(
            "ui-action",
            ClientKind::UiAction,
            EventSource::Ui,
            EventActor::Ui,
            Some(correlation_id),
            AuditInput::new(),
        ))
        .expect("append UI action");
    ledger
        .append(client_event(
            "lab-request",
            ClientKind::LabRequest,
            EventSource::Lab,
            EventActor::Lab,
            Some(correlation_id),
            AuditInput::new(),
        ))
        .expect("append Lab request");

    let events = ledger
        .query(EventQuery {
            correlation_id: Some(*self::correlation_id(correlation_id).transport()),
            ..EventQuery::default()
        })
        .expect("query correlated events");
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type())
            .collect::<Vec<_>>(),
        vec![
            EventType::CliCommand,
            EventType::SchedulerAdmitted,
            EventType::InputIntent,
            EventType::InputCommitted,
            EventType::UiAction,
            EventType::LabRequest,
        ]
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.origin().source())
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
            .map(|event| event.sequence())
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
        AuditInput::new()
            .with_authentication(TOKEN)
            .with_account(ACCOUNT)
            .with_machine_path(MACHINE_PATH)
            .with_device_endpoint(ENDPOINT)
    };

    let temp = TempDir::new().expect("temp root");
    let ledger = GlobalLedger::open(config(temp.path(), "redaction-writer")).expect("ledger");
    let event = client_event(
        "secret-bearing-command",
        ClientKind::CliCommand,
        EventSource::Cli,
        EventActor::Cli,
        Some(CORRELATION_ID),
        secret_fields(),
    );
    let expected_event_id = *event.event_id();
    ledger.append(event.clone()).expect("append redacted event");
    ledger.close().expect("close before recovered index query");

    let ledger = GlobalLedger::open(config(temp.path(), "redaction-query-writer"))
        .expect("reopen redaction ledger");
    let query = ledger
        .query(EventQuery {
            correlation_id: Some(*correlation_id(CORRELATION_ID).transport()),
            ..EventQuery::default()
        })
        .expect("query recovered correlation index");
    assert_eq!(query.len(), 1, "indexed query must retain the event");
    assert_eq!(query[0].event_id(), &expected_event_id);

    let duplicate = ledger
        .append(event)
        .expect_err("duplicate append must surface an error");
    let projections = [
        (
            "CLI projection",
            ledger
                .project(
                    EventQuery {
                        correlation_id: Some(*correlation_id(CORRELATION_ID).transport()),
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
                        correlation_id: Some(*correlation_id(CORRELATION_ID).transport()),
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
                        correlation_id: Some(*correlation_id(CORRELATION_ID).transport()),
                        ..EventQuery::default()
                    },
                    ProjectionProfile::Lab,
                )
                .expect("Lab projection"),
        ),
    ];
    for (label, projection) in &projections {
        assert_eq!(projection.len(), 1, "{label} must retain the event");
        assert_eq!(projection[0].event_id, expected_event_id);
        let projection_text = serde_json::to_string(projection).expect("serialize projection");
        assert_sentinels_absent(label, &projection_text, &sentinels);
    }
    let boundary_error = client_draft(
        "redaction-boundary-error",
        ClientKind::CliCommand,
        EventSource::Cli,
        EventActor::Cli,
        Some(CORRELATION_ID),
        secret_fields(),
    )
    .sanitize(&FailingBoundaryRedactor)
    .expect_err("redaction-boundary failure must surface");
    assert_eq!(boundary_error.code(), "fingerprinter_failed");

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
        InputKind::Intent,
        "critical-correlation",
        "critical-action",
    );
    ledger
        .append(intent.clone())
        .expect("seed duplicate critical intent");
    let plan = CriticalEventPlan::new(
        intent,
        input_event(
            "critical-success",
            InputKind::Committed,
            "critical-correlation",
            "critical-action",
        ),
        input_event(
            "critical-failure",
            InputKind::Failed,
            "critical-correlation",
            "critical-action",
        ),
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
