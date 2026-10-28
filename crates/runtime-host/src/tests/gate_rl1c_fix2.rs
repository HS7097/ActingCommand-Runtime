// SPDX-License-Identifier: AGPL-3.0-only

// ROUT-L RL-1c fix round 2 (#411; coordinator work order `rd-1027/brief-rl1c-fix2.md`; re-review
// `rd-1027/v01220-review/rl1c-fix1.md` M2 and L3): `actingd suspended`'s report.
//
// - L3, the deadline: a deadline that passes during the main read answers `ledger_read_incomplete`
//   with empty lists and `through_sequence` null. The deadline is judged by a fake read clock
//   (`LedgerReadClock`) that moves one step at each look and passes after a fixed number of
//   looks; no sleep, and the test reads no wall clock.
// - L3, a run: a run the budget leaves unread is shown with `step` null and its frames
//   `unavailable` (`ledger_read_budget_exceeded`); the budget is the read's most records, the
//   smallest one the main read completes within, so no clock is read.
// - M2: the report reads only the newest `config.subsystems` fact, not every
//   `runtime.fact_recorded`: on a ledger with many fact rows it completes within a budget far
//   below them, and its answer is the old full read's.
//
// The fixture is a closed state root whose ledger an offline writer extends (as #411 A24's gate
// does): two daemon starts' `config.subsystems` facts, optionally one paused scheduled run, and
// then many other workbench facts.

use super::clock_slot_report::ledger_events;
use super::gate_372k2::{slot_sources, slot_task};
use super::*;
use crate::SuspendedReadBudget;
use crate::failure_identity::{FailureIdentity, IdentityFrame, IdentityPackage};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CONFIG_SUBSYSTEMS_FACT_KEY, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, FactValue, PerformanceContext, PolicyDispatchEventData,
    PolicyExecutionEventData, PolicyFailureRecord, PolicyPayloadDraft, PolicyReasonRecord,
    RuntimeFactRecord, RuntimeFactScope, RuntimePayloadDraft, TaskPayloadDraft,
};
use actingcommand_ledger::{LedgerMaintenance, LedgerReadClock, Sha256SecretFingerprinter};
use actingcommand_runtime_database::MaintenanceLimits;
use serde_json::{Value, json};

/// The State seed of `config` and the offline writer's owner id: neutral fixture words.
const STATE_KEY: &[u8] = b"runtime-host-test-salt";
const WRITER: &str = "rl1c-fix2-offline-writer";
/// The paused pair's task: a neutral id outside the catalog.
pub(super) const PAUSED_TASK: &str = "fixture.rl1c-fix2.paused";
/// Other workbench facts after the latest daemon start: more than one newest-first statement.
pub(super) const LATER_FACTS: u64 = 300;
/// The latest daemon start, as its `config.subsystems` fact records it.
pub(super) const LATEST_START_UNIX_MS: u64 = POLICY_NOW_UNIX_MS + 2_000;

/// The offline writer of a closed state root and what it appends.
struct Appender {
    ledger: GlobalLedger,
    issuer: IdentifierIssuer,
    fingerprinter: Sha256SecretFingerprinter,
}

impl Appender {
    fn open(root: &TempDir) -> Self {
        let database = Arc::new(
            RuntimeStateStore::open_database(root.path(), STATE_KEY).expect("the state database"),
        );
        let limits = MaintenanceLimits::default();
        let ledger = LedgerMaintenance::acquire(
            root.path(),
            false,
            limits,
            limits.deadline().expect("deadline"),
        )
        .expect("the ledger's maintenance owner")
        .open_writer(database, WRITER.to_owned())
        .expect("the offline writer");
        Self {
            ledger,
            issuer: IdentifierIssuer::new().expect("identifier issuer"),
            fingerprinter: Sha256SecretFingerprinter::new(WRITER.as_bytes())
                .expect("fingerprinter"),
        }
    }

    fn append(
        &self,
        at_unix_ms: u64,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) {
        let draft = EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            at_unix_ms,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&self.fingerprinter)
        .expect("a fixture record");
        self.ledger.append(draft).expect("append a fixture record");
    }

    /// One workbench record of the Runtime scope.
    fn fact(&self, at_unix_ms: u64, key: &str, value: FactValue) {
        self.append(
            at_unix_ms,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::RuntimeFacts,
                EventActor::Runtime,
            ),
            EventLinksDraft::default(),
            RuntimePayloadDraft::fact_recorded(
                RuntimeFactRecord {
                    scope: RuntimeFactScope::Runtime,
                    key: key.to_owned(),
                    value,
                    observed_at_unix_ms: at_unix_ms,
                    source: OriginModule::Runtime,
                    ttl_ms: None,
                },
                AuditInput::new(),
            )
            .into(),
        );
    }

    /// One scheduled run of `PAUSED_TASK` that entered, started a step and failed: its dispatch
    /// intent, its rows, its `task.failed` terminal and the policy's record, which pauses the
    /// pair.
    fn paused_run(&self, at_unix_ms: u64) {
        let failure = Outcome::new(
            actingcommand_execution_kernel::ExecutionKernelCode::PageConfirmationFailed,
        );
        let links = EventLinksDraft::default()
            .with_instance_id(self.issuer.mint_instance_id().expect("instance id"))
            .with_request_id(self.issuer.mint_request_id().expect("request id"))
            .with_correlation_id(self.issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(self.issuer.mint_task_id().expect("task id"))
            .with_run_id(self.issuer.mint_run_id().expect("run id"));
        let policy = EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Policy,
            EventActor::Scheduler,
        );
        let runtime = EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        );
        let placeholder = |digit: char| format!("sha256:{}", digit.to_string().repeat(64));
        self.append(
            at_unix_ms,
            policy.clone(),
            links
                .clone()
                .with_action_id(self.issuer.mint_action_id().expect("action id")),
            PolicyPayloadDraft::dispatch_intent(
                PolicyDispatchEventData {
                    decision_id: "decision.rl1c-fix2.paused".to_owned(),
                    task_id: PAUSED_TASK.to_owned(),
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                    operation_id: "operation.rl1c-fix2".to_owned(),
                    procedure_binding_digest: placeholder('b'),
                    reason_chain_id: "reasons.rl1c-fix2".to_owned(),
                    reasons: vec![PolicyReasonRecord(Outcome::relayed("placement_selected"))],
                    catalog_hash: placeholder('c'),
                    catalog_version: 1,
                    input_ledger_position: 1,
                    fact_snapshot_id: "facts.rl1c-fix2".to_owned(),
                    approval_fact_ids: Vec::new(),
                    urgency_milli: 0,
                },
                AuditInput::new(),
            )
            .into(),
        );
        let facts = [
            TaskSemanticFact::EntryRecognition {
                phase: TaskEntryRecognitionPhase::Initial,
                required_pages: vec!["fixture/home".to_owned()],
                matched_page: Some("fixture/home".to_owned()),
                matched: true,
            },
            TaskSemanticFact::EntryRecoveryDecision { required: false },
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::Started,
                outcome: None,
            },
            TaskSemanticFact::StepStarted {
                step_index: 0,
                operation_label: "fixture_operation".to_owned(),
                from_page: "fixture/home".to_owned(),
                phase: None,
            },
            TaskSemanticFact::TerminalCommitted {
                task_result: TaskOutcome::Failure,
                final_page: Some("fixture/home".to_owned()),
                executed_steps: Some(1),
                outcome: Some(failure.clone()),
                failure_severe: false,
                scheduling_disposition: None,
                task_timing: None,
            },
        ];
        for (offset, fact) in (1..).zip(facts) {
            self.append(
                at_unix_ms + offset * 10,
                runtime.clone(),
                links.clone(),
                TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
            );
        }
        let identity = FailureIdentity {
            base: "page_confirmation_failed".to_owned(),
            key: "0".repeat(12),
            main: IdentityPackage {
                id_hash: "0a1b2c3d".to_owned(),
                hash: "0123456789ab".to_owned(),
            },
            layers: Vec::new(),
            frame: IdentityFrame::None,
        }
        .encode();
        self.append(
            at_unix_ms + 100,
            policy,
            links,
            PolicyPayloadDraft::execution_recorded(
                PolicyExecutionEventData {
                    decision_id: "decision.rl1c-fix2.paused".to_owned(),
                    task_id: PAUSED_TASK.to_owned(),
                    instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
                    observed_at_unix_ms: at_unix_ms + 100,
                    outcome: PolicyExecutionOutcome::Failed {
                        failure: PolicyFailureRecord {
                            outcome: failure,
                            failure_identity: Some(identity),
                            reported_success: false,
                            original_class: PolicyFailureClass::Severe,
                            effective_class: PolicyFailureClass::Severe,
                            consecutive_same_error: 1,
                            escalation_streak: 1,
                            performance_tax_exempt: false,
                            retry_attempt: 0,
                            disposition: PolicyFailureDisposition::PausedTask,
                            retry_at_unix_ms: None,
                            runtime_ms: 1_000,
                            sensitive: false,
                            perf_context: Box::new(PerformanceContext::unavailable(at_unix_ms)),
                            condition: None,
                        },
                    },
                },
                AuditInput::new(),
            )
            .into(),
        );
    }

    fn close(self) {
        self.ledger.close().expect("close the offline writer");
    }
}

/// A closed state root: a host's start and close, then an earlier and the latest daemon start's
/// `config.subsystems` facts, the paused run when `paused_run`, and `LATER_FACTS` other
/// workbench facts after them, the last of them the head.
pub(super) fn fixture(paused_run: bool) -> TempDir {
    let root = TempDir::new().expect("the state root");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("start the host");
    host.close().expect("close the host");
    let appender = Appender::open(&root);
    for at in [LATEST_START_UNIX_MS - 1_000, LATEST_START_UNIX_MS] {
        appender.fact(
            at,
            CONFIG_SUBSYSTEMS_FACT_KEY,
            FactValue::RecordList(Vec::new()),
        );
    }
    if paused_run {
        appender.paused_run(LATEST_START_UNIX_MS + 1_000);
    }
    for index in 0..LATER_FACTS {
        appender.fact(
            LATEST_START_UNIX_MS + 10_000 + index,
            "task.fixture-filler.attempts",
            FactValue::Integer(i64::try_from(index).expect("a count")),
        );
    }
    appender.close();
    root
}

/// `actingd suspended` on the closed root within `budget`.
pub(super) fn report_within(root: &TempDir, budget: SuspendedReadBudget) -> Value {
    let host = config(root);
    let sources = slot_sources(1, &[slot_task(&[600], 1)], None);
    crate::suspension_report_within(
        &SuspensionReportRequest {
            config_path: Path::new("actingd.config.json"),
            config_modified_unix_ms: None,
            host: &host,
            catalog: &sources,
        },
        budget,
    )
    .unwrap_or_else(|error| panic!("actingd suspended: {error}"))
}

/// A budget of `max_events` records and no deadline.
fn records(max_events: usize) -> SuspendedReadBudget {
    SuspendedReadBudget {
        max_events,
        ..SuspendedReadBudget::until(None)
    }
}

pub(super) fn warnings(report: &Value) -> Vec<String> {
    report["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("warnings is a list: {report:#}"))
        .iter()
        .map(|warning| warning.as_str().expect("a text warning").to_owned())
        .collect()
}

/// The smallest budget of records within which the report's main read completes (it answers
/// `through_sequence`): a run it shows may still be left unread there.
fn smallest_complete_budget(root: &TempDir) -> usize {
    let complete =
        |max_events: usize| !report_within(root, records(max_events))["through_sequence"].is_null();
    let (mut low, mut high) = (1_usize, 1_usize << 20);
    assert!(!complete(low), "one record holds no main read");
    assert!(complete(high), "a large budget holds the main read");
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if complete(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    high
}

/// The ledger's head sequence, and the latest daemon start as the old full read found it: every
/// `runtime.fact_recorded`, newest last, the last `config.subsystems` one.
pub(super) fn old_full_read(root: &TempDir) -> (u64, Option<u64>, usize) {
    let events = ledger_events(root);
    let head = events
        .iter()
        .map(PersistedEvent::sequence)
        .max()
        .expect("a ledger record");
    let facts = events
        .iter()
        .filter(|event| event.event_type() == EventType::RuntimeFactRecorded)
        .collect::<Vec<_>>();
    let started = facts.iter().rev().find_map(|event| match event.payload() {
        EventPayload::Runtime(actingcommand_contract::RuntimePayload::FactRecorded(payload))
            if payload.record().key == CONFIG_SUBSYSTEMS_FACT_KEY =>
        {
            Some(payload.record().observed_at_unix_ms)
        }
        _ => None,
    });
    (head, started, facts.len())
}

/// L3 (work order item 3, the deadline path): the deadline passes during the main read (the fake
/// read clock lets the read take `AHEAD` looks, and the same read takes more): the report answers
/// `ledger_read_incomplete` last, with every list empty and `through_sequence` null. With a
/// deadline that never passes the same read answers complete.
#[test]
fn rl1c_fix2_a_deadline_passing_during_the_main_read_answers_incomplete() {
    const AHEAD: u64 = 16;
    let root = fixture(true);
    let (head, _, _) = old_full_read(&root);

    let never = LedgerReadClock::arm(u64::MAX);
    let complete = report_within(&root, SuspendedReadBudget::until(Some(never.deadline())));
    let looks = never.looks();
    drop(never);
    assert_eq!(complete["through_sequence"], json!(head), "{complete:#}");
    assert!(
        looks > AHEAD,
        "the main read looks at its deadline more than {AHEAD} times: {looks}"
    );

    let clock = LedgerReadClock::arm(AHEAD);
    let report = report_within(&root, SuspendedReadBudget::until(Some(clock.deadline())));
    assert!(
        clock.looks() > AHEAD,
        "the read went on until its deadline passed: {} looks",
        clock.looks()
    );
    drop(clock);
    assert_eq!(
        warnings(&report).last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{report:#}"
    );
    for list in [
        "suspended",
        "lifted",
        "repeating",
        "clock_slots",
        "packs",
        "remedies",
    ] {
        assert_eq!(report[list], json!([]), "{list}: {report:#}");
    }
    assert_eq!(report["through_sequence"], Value::Null, "{report:#}");
}

/// L3 (work order item 3, the per-run path): within the smallest budget the main read completes
/// in, the paused pair's run is left unread: the pair is listed with `step` null and its frames'
/// comparison `unavailable` with `ledger_read_budget_exceeded`, and the report warns
/// `ledger_read_incomplete`. Without the bound the same pair has its step and no such comparison.
#[test]
fn rl1c_fix2_a_run_left_unread_has_no_step_and_unavailable_frames() {
    let root = fixture(true);
    let (head, _, _) = old_full_read(&root);
    let budget = smallest_complete_budget(&root);

    let bounded = report_within(&root, records(budget));
    assert_eq!(bounded["through_sequence"], json!(head), "{bounded:#}");
    let pair = &bounded["suspended"][0];
    assert_eq!(pair["task_id"], json!(PAUSED_TASK), "{bounded:#}");
    assert_eq!(pair["step"], Value::Null, "{bounded:#}");
    let comparison = &pair["frames"]["comparison"];
    assert_eq!(comparison["status"], json!("unavailable"), "{bounded:#}");
    assert_eq!(
        comparison["outcome"]["code"],
        json!("ledger_read_budget_exceeded"),
        "{bounded:#}"
    );
    assert_eq!(
        warnings(&bounded).last().map(String::as_str),
        Some("ledger_read_incomplete"),
        "{bounded:#}"
    );

    let whole = report_within(&root, SuspendedReadBudget::until(None));
    let pair = &whole["suspended"][0];
    assert_eq!(pair["task_id"], json!(PAUSED_TASK), "{whole:#}");
    assert!(
        !pair["step"].is_null(),
        "the run read has a step: {whole:#}"
    );
    assert_ne!(
        pair["frames"]["comparison"]["outcome"]["code"],
        json!("ledger_read_budget_exceeded"),
        "{whole:#}"
    );
    assert!(
        !warnings(&whole).contains(&"ledger_read_incomplete".to_owned()),
        "{whole:#}"
    );
}

/// M2 (work order item 2): with many `runtime.fact_recorded` rows after the latest daemon start,
/// the report completes within a budget smaller than those rows (the old selection held them
/// all), and it answers as without a bound; its `daemon_started_at_unix_ms` is the old full
/// read's, the newest `config.subsystems` fact's time.
#[test]
fn rl1c_fix2_the_report_reads_only_the_newest_config_fact() {
    let root = fixture(false);
    let (head, started, fact_rows) = old_full_read(&root);
    assert_eq!(started, Some(LATEST_START_UNIX_MS), "the old full read");
    let budget = smallest_complete_budget(&root);
    assert!(
        budget < fact_rows,
        "the report holds fewer records ({budget}) than the fact rows the old read held ({fact_rows})"
    );

    let bounded = report_within(&root, records(budget));
    assert_eq!(bounded["through_sequence"], json!(head), "{bounded:#}");
    assert_eq!(
        bounded["daemon_started_at_unix_ms"],
        json!(started),
        "{bounded:#}"
    );
    assert!(
        !warnings(&bounded)
            .iter()
            .any(|warning| warning == "daemon_start_unknown" || warning == "ledger_read_incomplete"),
        "{bounded:#}"
    );
    let whole = report_within(&root, SuspendedReadBudget::until(None));
    assert_eq!(
        bounded, whole,
        "the bounded report answers as the unbounded one"
    );
}
