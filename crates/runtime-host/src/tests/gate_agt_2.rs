// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-2 (`contracts/agent-wake.md`, "Triggers, rules and guards"; register §1.253;
// design rd-1031/agt/model-agt-v2.md sections 2, 4, 5.3, 7.3 and 10): the tap, the first triggers,
// the rules and the guards, over a real ledger with a fake clock, a fixed local zone and a fake
// transport. The records go through the append path the Runtime's own writers use, in the shapes
// they commit. No worker thread runs: each test runs the worker's iteration itself, so nothing
// here sleeps or reads a wall clock. The names are made up.

use super::*;
use crate::agent_wake::guards::DAY_MS;
use crate::agent_wake::rows::{WakeRow, never_a_candidate};
use crate::codes::HostCode;
use crate::failure_identity::identifier_text;
use crate::host::WakeRecordForTest;
use actingcommand_agent_forward::{
    AgentForward, DeliveryProblem, DeliveryProof, FakeForward, ForwardOutcome, fake_registry_text,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{CodeStr, Outcome};
use actingcommand_contract::{
    AgentDeliveryState, AgentWakeStatus, ConfigSettingChange, EventQuery, EventType,
    RuntimeConfigManifest, RuntimeInstanceStatus, RuntimeReceipt, RuntimeReceiptState,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_ledger::PersistedEvent;
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

/// 07:33:20 UTC.
const NOW: u64 = 1_793_000_000_000;
const HOUR_MS: u64 = 60 * 60 * 1_000;
const CHANNEL: &str = "总览";
const ALIAS: &str = "node.a";
const RUN_CODE: &str = "capture_backend_operation_failed";
const BUDGET_CODE: &str = "agent_budget_exhausted";
const FAILED_CODE: &str = "agent_wake_delivery_failed";

fn rule(id: &str, kind: &str, codes: &[&str], cooldown_ms: u64, daily_cap: u64) -> Value {
    json!({
        "id": id, "on": {"kinds": [kind], "codes": codes}, "instances": "*", "to": CHANNEL,
        "cooldown_ms": cooldown_ms, "daily_cap": daily_cap,
    })
}

// A whole section: no coalescing, no cool-down unless a rule says one, no gap, no quiet.
fn section(rules: Value) -> Value {
    json!({
        "enabled": true,
        "summary": {"max_chars": 2048},
        "body": {"dir": "agent-wakes", "max_bytes": 65536, "retention_days": 14, "max_files": 512},
        "delivery": {
            "queue_capacity": 8, "coalesce_ms": 0, "send_deadline_ms": 1000,
            "retry_interval_ms": 1000, "give_up_ms": 60000, "channel_gap_ms": 0,
            "history_rows": 32, "panic_backoff_ms": 1000, "panic_backoff_max_ms": 4000,
        },
        "quiet": {"windows": [], "hold_until_unix_ms": 0},
        "budget": {"daily_cap": 24},
        "rules": rules,
    })
}

fn with(mut section: Value, path: &str, value: Value) -> Value {
    let pointer = format!("/{}", path.replace('.', "/"));
    *section.pointer_mut(&pointer).expect("the member exists") = value;
    section
}

// The section, with the recovery ladder off so that no ladder of the host's own starts beside
// the records the tests commit.
fn manifest(section: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "agent_wake": section, "recovery_ladder": {"enabled": false} }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn delivered() -> ForwardOutcome {
    ForwardOutcome::Delivered {
        proof: DeliveryProof::Written,
        detail: "neutral detail".to_owned(),
    }
}

fn delivering(count: usize) -> Arc<FakeForward> {
    Arc::new(FakeForward::new((0..count).map(|_| delivered())))
}

fn east_nine() -> i64 {
    i64::try_from(9 * HOUR_MS).expect("an offset")
}

struct Bed {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    ids: IdentifierIssuer,
}

impl Bed {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let folder = root.path().join("agent-forward");
        fs::create_dir_all(&folder).expect("the registry's folder");
        fs::write(folder.join("channels.json"), fake_registry_text(&[CHANNEL]))
            .expect("the registry");
        Self {
            root,
            clock: Arc::new(ManualRuntimeClock::new(NOW, 0)),
            ids: IdentifierIssuer::new().expect("identifier issuer"),
        }
    }

    // A host whose local zone is `offset_ms` east of UTC.
    fn start(&self, section: Value, forward: Arc<dyn AgentForward>, offset_ms: i64) -> RuntimeHost {
        let clock: Arc<dyn RuntimeClock> = self.clock.clone();
        let host = RuntimeHost::start(
            config(&self.root)
                .with_runtime_clock(clock)
                .with_config_manifest(manifest(section))
                .with_agent_forward_for_test(forward),
            Arc::new(FakeProvider::one(
                ALIAS,
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host");
        host.agent_wake_set_local_offset_for_test(offset_ms)
            .expect("the local zone");
        host
    }

    fn receipt(&self, host: &RuntimeHost, operation: RuntimeOperation) -> RuntimeReceipt {
        host.process_request_for_test(
            &runtime_request(&self.ids, operation),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("a receipt")
    }

    fn status(&self, host: &RuntimeHost) -> AgentWakeStatus {
        let receipt = self.receipt(host, RuntimeOperation::Status);
        let Some(RuntimeResult::Status { status }) = receipt.result() else {
            panic!("a status: {receipt:?}");
        };
        status.agent_wake().cloned().expect("the member")
    }

    fn instance(&self, host: &RuntimeHost) -> RuntimeInstanceStatus {
        let receipt = self.receipt(host, RuntimeOperation::Status);
        let Some(RuntimeResult::Status { status }) = receipt.result() else {
            panic!("a status: {receipt:?}");
        };
        status
            .instances()
            .iter()
            .find(|instance| instance.instance_alias() == ALIAS)
            .cloned()
            .expect("the instance")
    }

    fn body(&self, row: &WakeRow) -> Value {
        let file = row.body_file.as_deref().expect("the body's name");
        let text =
            fs::read_to_string(self.root.path().join("agent-wakes").join(file)).expect("the body");
        serde_json::from_str(&text).expect("the body is JSON")
    }
}

fn append(host: &RuntimeHost, record: WakeRecordForTest) -> PersistedEvent {
    host.agent_wake_append_for_test(ALIAS, record)
        .expect("the record is committed")
}

fn step(host: &RuntimeHost) {
    host.agent_wake_step_for_test().expect("an iteration");
}

fn rows(host: &RuntimeHost) -> Vec<WakeRow> {
    host.agent_wake_rows_for_test().expect("the rows")
}

fn queued(host: &RuntimeHost) -> Option<usize> {
    host.agent_wake_queued_for_test().expect("the queue")
}

fn row_code(row: &WakeRow) -> Option<&str> {
    row.code.as_ref().map(CodeStr::as_str)
}

// The failure of a run, with a code a rule lists.
fn run_outcome() -> Outcome {
    Outcome::new(SharedCode::CaptureBackendOperationFailed)
}

// What a fence's own record and the failure of work that met it carry.
fn fenced_outcome() -> Outcome {
    Outcome::new(HostCode::InstanceResourcesFenced).instance_id(identifier_text(&instance_id()))
}

// The `runtime.failed` records with the code `spelled`, with whether they name an instance.
fn failures(host: &RuntimeHost, spelled: &str) -> Vec<(Outcome, bool)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFailed),
        ..EventQuery::default()
    })
    .expect("the failure records")
    .iter()
    .filter_map(|event| {
        event
            .payload()
            .outcome()
            .filter(|outcome| outcome.code().as_str() == spelled)
            .map(|outcome| (outcome.clone(), event.links().instance_id().is_some()))
    })
    .collect()
}

fn value<'a>(outcome: &'a Outcome, key: &str) -> Option<&'a str> {
    outcome.values().get(key).and_then(Value::as_str)
}

#[test]
fn each_first_kind_wakes_with_its_code_and_instance() {
    let bed = Bed::new();
    let forward = delivering(3);
    let rules = json!([
        rule("ladder", "ladder_exhausted", &[], 0, 8),
        rule("fence", "instance_fenced", &[], 0, 8),
        rule("runs", "run_failed", &[RUN_CODE], 0, 8),
    ]);
    let host = bed.start(section(rules), forward.clone(), 0);
    let first = [
        (
            WakeRecordForTest::LadderExhausted,
            "ladder_exhausted",
            "ladder",
            HostCode::RecoveryLadderExhausted.as_str(),
            "阶梯用尽",
        ),
        (
            WakeRecordForTest::FenceAwaitingPerson,
            "instance_fenced",
            "fence",
            HostCode::InstanceResourcesFenceAwaitingPerson.as_str(),
            "实例隔离",
        ),
        (
            WakeRecordForTest::RunFailed(run_outcome()),
            "run_failed",
            "runs",
            RUN_CODE,
            "运行失败",
        ),
    ];
    for (index, (record, kind, rule_id, spelled, word)) in first.into_iter().enumerate() {
        let committed = append(&host, record);
        step(&host);
        let kept = rows(&host);
        assert_eq!(kept.len(), index + 1, "{kind}");
        let row = &kept[index];
        assert_eq!(row.kind, kind);
        assert_eq!(row.rule.as_deref(), Some(rule_id));
        assert!(!row.test);
        assert_eq!(row.instance_alias.as_deref(), Some(ALIAS));
        assert_eq!(row_code(row), Some(spelled));
        assert_eq!(
            row.trigger_event_id.as_deref(),
            Some(identifier_text(committed.event_id()).as_str())
        );
        assert_eq!(row.state, AgentDeliveryState::Delivered, "{kind}");
        // The message names the kind's word and the instance.
        let sends = forward.sends();
        assert_eq!(sends.len(), index + 1);
        let text = &sends[index].message.text;
        let line_two = format!("ActingCommand to {CHANNEL}\n{word} {ALIAS} 正文 ");
        assert!(text.starts_with(&line_two), "{text}");
        // The body carries the rule, the instance and the trigger with its code.
        let body = bed.body(row);
        assert_eq!(body["kind"], json!(kind));
        assert_eq!(body["rule"], json!(rule_id));
        assert_eq!(body["instance_alias"], json!(ALIAS));
        assert_eq!(body["test"], json!(false));
        assert_eq!(body["trigger"]["code"], json!(spelled));
        assert_eq!(body["trigger"]["sequence"], json!(committed.sequence()));
        bed.clock.advance(1_000);
    }
    // A failed run's body starts its reads from the run's handle.
    let body = bed.body(&rows(&host)[2]);
    assert_eq!(body["next_reads"][0]["tool"], json!("ac_get_run"));
    assert_eq!(body["next_reads"][1]["tool"], json!("ac_events"));
    assert_eq!(body["next_reads"][1]["args"]["instance"], json!(ALIAS));
}

#[test]
fn fence_wakes_only_when_it_waits_for_a_person() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("fence", "instance_fenced", &[], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    // The fence's own record: the instance is fenced, and a ladder may still bring it back.
    append(&host, WakeRecordForTest::RuntimeFailed(fenced_outcome()));
    assert_eq!(queued(&host), Some(0), "no candidate");
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    // The fence waits for a person: one wake.
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, "instance_fenced");
    assert_eq!(
        row_code(&kept[0]),
        Some(HostCode::InstanceResourcesFenceAwaitingPerson.as_str())
    );
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn a_fence_met_by_other_work_does_not_wake() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("fence", "instance_fenced", &[], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    // The work that met the fence ends with the fence's code: a run's failed terminal and a
    // request's failure record. Neither waits for a person.
    append(&host, WakeRecordForTest::RunFailed(fenced_outcome()));
    append(&host, WakeRecordForTest::RuntimeFailed(fenced_outcome()));
    step(&host);
    bed.clock.advance(HOUR_MS);
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    assert_eq!(forward.sends().len(), 0);
    assert_eq!(
        bed.status(&host).suppressed_today,
        0,
        "no rule's candidate, so nothing was held"
    );
}

#[test]
fn run_failed_needs_a_listed_code() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("runs", "run_failed", &[RUN_CODE], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    // A failed run of a code the rule does not list wakes nothing.
    append(
        &host,
        WakeRecordForTest::RunFailed(Outcome::new(SharedCode::InputBackendOperationFailed)),
    );
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    // One of the listed code does.
    append(&host, WakeRecordForTest::RunFailed(run_outcome()));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    assert_eq!(row_code(&kept[0]), Some(RUN_CODE));
    // No list, no wake: a rule on failed runs that lists no code is refused.
    let refused = bed.receipt(
        &host,
        RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key: "agent_wake.rules".to_owned(),
                value: Some(json!([{
                    "id": "bare", "on": {"kinds": ["run_failed"]}, "instances": "*",
                    "to": CHANNEL, "cooldown_ms": 0, "daily_cap": 1,
                }])),
            }],
            expected_version: None,
        },
    );
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        value(refused.outcome(), "config_value_problem"),
        Some("match_missing")
    );
}

#[test]
fn interrupted_and_drill_ladders_do_not_wake() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("ladder", "ladder_exhausted", &[], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    for record in [
        WakeRecordForTest::LadderInterrupted,
        WakeRecordForTest::LadderDrilled,
        WakeRecordForTest::LadderPlanned,
    ] {
        append(&host, record);
        assert_eq!(queued(&host), Some(0), "no candidate");
        step(&host);
    }
    assert_eq!(rows(&host), Vec::new());
    assert_eq!(forward.sends().len(), 0);
    // A ladder a trigger started that ended exhausted does wake.
    append(&host, WakeRecordForTest::LadderExhausted);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
}

#[test]
fn other_writers_of_the_record_type_do_not_wake() {
    let bed = Bed::new();
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Failed {
        problem: DeliveryProblem::Rejected,
        detail: "neutral refusal".to_owned(),
    }]));
    let rules = json!([
        rule("fence", "instance_fenced", &[], 0, 8),
        rule("runs", "run_failed", &[RUN_CODE], 0, 8),
    ]);
    let host = bed.start(section(rules), forward.clone(), 0);
    // A `task.failed` of a writer that commits no terminal (the Lab's, the evidence export's), and
    // `runtime.failed` records with the listed code and with the fence's code.
    let other = append(&host, WakeRecordForTest::OtherTaskFailed);
    assert_eq!(other.event_type(), EventType::TaskFailed);
    append(&host, WakeRecordForTest::RuntimeFailed(run_outcome()));
    append(&host, WakeRecordForTest::RuntimeFailed(fenced_outcome()));
    assert_eq!(queued(&host), Some(0), "no candidate");
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    // The run's own terminal wakes. Its delivery fails, and that failure's record wakes nothing.
    append(&host, WakeRecordForTest::RunFailed(run_outcome()));
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(rows(&host)[0].state, AgentDeliveryState::Failed);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert!(never_a_candidate(said[0].0.code().as_str()));
    assert_eq!(queued(&host), Some(0));
    bed.clock.advance(HOUR_MS);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn ladder_end_and_person_wait_make_one_wake_with_the_rest_related() {
    let bed = Bed::new();
    let forward = delivering(2);
    let rules = json!([
        rule("ladder", "ladder_exhausted", &[], 0, 8),
        rule("fence", "instance_fenced", &[], 0, 8),
    ]);
    let host = bed.start(
        with(section(rules), "delivery.coalesce_ms", json!(3_000)),
        forward.clone(),
        0,
    );
    let ladder = append(&host, WakeRecordForTest::LadderExhausted);
    step(&host);
    assert_eq!(rows(&host), Vec::new(), "the burst gathers");
    // The fence's wait for a person follows the ladder's end within the window.
    bed.clock.advance(1_000);
    let fence = append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    bed.clock.advance(1_999);
    step(&host);
    assert_eq!(rows(&host), Vec::new(), "within coalesce_ms of the first");
    bed.clock.advance(1);
    step(&host);
    // One wake for the channel, of the stronger kind; the other is related.
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, "ladder_exhausted");
    assert_eq!(
        kept[0].trigger_event_id.as_deref(),
        Some(identifier_text(ladder.event_id()).as_str())
    );
    let body = bed.body(&kept[0]);
    assert_eq!(
        body["related"],
        json!([{
            "kind": "instance_fenced",
            "event_id": identifier_text(fence.event_id()),
            "code": HostCode::InstanceResourcesFenceAwaitingPerson.as_str(),
        }])
    );
    assert_eq!(
        body["next_reads"][0]["args"]["after_sequence"],
        json!(ladder.sequence() - 1)
    );
    assert_eq!(forward.sends().len(), 1);
    bed.clock.advance(HOUR_MS);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
}

#[test]
fn cooldown_and_daily_cap_survive_a_restart() {
    let bed = Bed::new();
    let rules = json!([rule("fence", "instance_fenced", &[], HOUR_MS, 2)]);
    let host = bed.start(section(rules.clone()), delivering(1), 0);
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    host.close().expect("close");

    // Within the cool-down, after a restart: held.
    bed.clock.advance(60_000);
    let host = bed.start(section(rules.clone()), delivering(1), 0);
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(
        rows(&host).len(),
        1,
        "the cool-down came back with the rows"
    );
    assert_eq!(bed.status(&host).suppressed_today, 1);
    // Past it: the second wake fills the rule's cap.
    bed.clock.advance(HOUR_MS);
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 2);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 1);
    host.close().expect("close");

    // After another restart, past the cool-down, the day cap still holds.
    bed.clock.advance(HOUR_MS + 1);
    let host = bed.start(section(rules), delivering(1), 0);
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 2, "the day cap came back with the rows");
    assert_eq!(failures(&host, BUDGET_CODE).len(), 1, "said once that day");
    let status = bed.status(&host);
    assert_eq!(status.suppressed_today, 1);
    assert!(status.attention);
}

#[test]
fn budget_exhausted_is_recorded_once_a_day_and_stops_wakes() {
    let bed = Bed::new();
    let rules = json!([
        rule("fence", "instance_fenced", &[], 0, 2),
        rule("runs", "run_failed", &[RUN_CODE], 0, 2),
    ]);
    let host = bed.start(
        with(section(rules), "budget.daily_cap", json!(3)),
        delivering(8),
        0,
    );
    let wake = |record: WakeRecordForTest| {
        append(&host, record);
        step(&host);
        bed.clock.advance(1_000);
    };
    // The rule's second wake fills its cap and says so, once.
    wake(WakeRecordForTest::FenceAwaitingPerson);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 0);
    wake(WakeRecordForTest::FenceAwaitingPerson);
    let said = failures(&host, BUDGET_CODE);
    assert_eq!(said.len(), 1);
    let (outcome, linked) = &said[0];
    assert_eq!(value(outcome, "config_key"), Some("agent_wake.rules"));
    assert_eq!(value(outcome, "label"), Some("fence"));
    assert_eq!(value(outcome, "agent_channel"), Some(CHANNEL));
    assert_eq!(outcome.values().get("limit_count"), Some(&json!(2)));
    assert!(!linked, "no instance link");
    assert!(never_a_candidate(outcome.code().as_str()));
    assert!(bed.status(&host).attention);
    // The rule's wakes stop for the day; nothing more is said.
    wake(WakeRecordForTest::FenceAwaitingPerson);
    assert_eq!(rows(&host).len(), 2);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 1);
    // Another rule's wake fills the budget of all rules and says so, once.
    wake(WakeRecordForTest::RunFailed(run_outcome()));
    assert_eq!(rows(&host).len(), 3);
    let said = failures(&host, BUDGET_CODE);
    assert_eq!(said.len(), 2);
    assert_eq!(
        value(&said[1].0, "config_key"),
        Some("agent_wake.budget.daily_cap")
    );
    assert_eq!(value(&said[1].0, "label"), None);
    assert_eq!(said[1].0.values().get("limit_count"), Some(&json!(3)));
    // Every wake stops for the day.
    wake(WakeRecordForTest::RunFailed(run_outcome()));
    wake(WakeRecordForTest::FenceAwaitingPerson);
    assert_eq!(rows(&host).len(), 3);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 2);
    let status = bed.status(&host);
    assert_eq!(status.suppressed_today, 3);
    assert!(status.attention);
    assert!(host.fatal_error().expect("health").is_none());
}

#[test]
fn a_full_queue_never_blocks_the_append_path() {
    let bed = Bed::new();
    let forward = delivering(2);
    let rules = json!([rule("fence", "instance_fenced", &[], 0, 8)]);
    let host = bed.start(
        with(section(rules), "delivery.queue_capacity", json!(1)),
        forward.clone(),
        0,
    );
    // Three records with room for one: every append commits, and the tap drops two and counts
    // them.
    for _ in 0..3 {
        append(&host, WakeRecordForTest::FenceAwaitingPerson);
    }
    assert_eq!(queued(&host), Some(1));
    assert!(host.fatal_error().expect("health").is_none());
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert_eq!(
        value(&said[0].0, "agent_delivery_problem"),
        Some("wake_queue_full")
    );
    assert_eq!(bed.status(&host).suppressed_today, 2);
}

#[test]
fn the_queue_takes_records_from_before_the_thread_starts() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("ladder", "ladder_exhausted", &[], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    // The queue is built with the shared state; no worker thread runs.
    assert!(!host.agent_wake_thread_runs_for_test().expect("the thread"));
    assert_eq!(queued(&host), Some(0));
    append(&host, WakeRecordForTest::LadderExhausted);
    assert_eq!(queued(&host), Some(1), "it waits for the thread");
    assert_eq!(rows(&host), Vec::new());
    // The worker's first iteration takes what waited.
    step(&host);
    assert_eq!(queued(&host), Some(0));
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn a_scheduling_pause_stops_no_wake_and_a_wake_changes_no_scheduling() {
    let bed = Bed::new();
    let forward = delivering(1);
    let rules = json!([rule("fence", "instance_fenced", &[], 0, 8)]);
    let host = bed.start(section(rules), forward.clone(), 0);
    let paused = bed.receipt(
        &host,
        RuntimeOperation::PauseScheduling {
            scope: SchedulingPauseScope::Instance {
                instance_alias: ALIAS.to_owned(),
            },
            pause_reason: SchedulingPauseReason::new("neutral_pause").expect("pause reason"),
            drain_timeout_ms: 1_000,
        },
    );
    assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
    let before = bed.instance(&host);
    assert!(before.pause().is_some());
    // The paused instance's wake is made and sent.
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].state, AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 1);
    // And it touched no pause, lease or queue.
    let after = bed.instance(&host);
    assert_eq!(after.pause(), before.pause());
    assert_eq!(after.resource_fence(), before.resource_fence());
    assert!(!after.lease_active());
    assert_eq!(after.queued_request_count(), before.queued_request_count());
}

#[test]
fn the_daily_cap_rolls_over_at_the_local_day() {
    // NOW is 16:33:20 nine hours east of UTC: local midnight comes 7 h 26 min 40 s later, at
    // 15:00 UTC of the same UTC day.
    let to_local_midnight_ms = DAY_MS - (NOW + 9 * HOUR_MS) % DAY_MS;
    assert_eq!(to_local_midnight_ms, 26_800_000);
    assert_eq!(
        (NOW + to_local_midnight_ms) / DAY_MS,
        NOW / DAY_MS,
        "the same UTC day"
    );
    let bed = Bed::new();
    let rules = json!([rule("fence", "instance_fenced", &[], 0, 1)]);
    let host = bed.start(section(rules), delivering(4), east_nine());
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(
        failures(&host, BUDGET_CODE).len(),
        1,
        "one wake fills the cap"
    );
    // Later the same local day: held.
    bed.clock.advance(to_local_midnight_ms - 1);
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(bed.status(&host).suppressed_today, 1);
    // At local midnight, still the same UTC day, the cap is new: one more wake, said again.
    bed.clock.advance(1);
    let status = bed.status(&host);
    assert_eq!(status.suppressed_today, 0, "a new day's count");
    assert!(!status.attention, "nothing is used up on the new day");
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 2);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 2);
    // And held again for the rest of that day.
    append(&host, WakeRecordForTest::FenceAwaitingPerson);
    step(&host);
    assert_eq!(rows(&host).len(), 2);
    assert_eq!(failures(&host, BUDGET_CODE).len(), 2);
}
