// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-1b (`contracts/agent-wake.md`; register §1.246; design rd-1031/agt/model-agt-v2.md
// sections 2, 5.4, 6.2, 7 and 9): the worker of the Runtime's messages to registered channels, over
// a real ledger with a fake clock and a fake transport. No worker thread runs: each test runs the
// worker's iteration itself, so nothing here sleeps or reads a wall clock. The names are made up.

use super::*;
use crate::agent_wake::rows::{WaitReason, WakeRow, never_a_candidate};
use actingcommand_agent_forward::{
    AgentForward, DeferReason, DeliveryProblem, DeliveryProof, FakeForward, ForwardMessage,
    ForwardOutcome, IntentFailed, SendLimits, fake_registry_text,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AgentDeliveryState, AgentWakeQuiet, AgentWakeTestQueued, AgentWakeWorkerState,
    ConfigSettingChange, EventQuery, EventType, FactScalar, FactValue, OriginModule,
    RuntimeConfigManifest, RuntimeFactRecord, RuntimeFactScope, RuntimeReceipt,
    RuntimeReceiptState,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};
use std::sync::OnceLock;

const NOW: u64 = 1_793_000_000_000;
const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
const CHANNEL: &str = "总览";
const FAILED_CODE: &str = "agent_wake_delivery_failed";

// A whole section: a limit that fits a test's long temporary path, a short retry and give-up, no
// gap, no quiet, no rule.
fn section() -> Value {
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
        "rules": [],
    })
}

fn with(mut section: Value, path: &str, value: Value) -> Value {
    let pointer = format!("/{}", path.replace('.', "/"));
    *section.pointer_mut(&pointer).expect("the member exists") = value;
    section
}

fn manifest(section: Value) -> RuntimeConfigManifest {
    let (_, defaults) = DefaultsFile::shipped().expect("the defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "agent_wake": section }),
        &|_| None,
    )
    .expect("the sections resolve");
    annotate_settable(&mut parameters).expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

fn write_registry(root: &Path, channels: &[&str]) {
    let folder = root.join("agent-forward");
    fs::create_dir_all(&folder).expect("the registry's folder");
    fs::write(folder.join("channels.json"), fake_registry_text(channels)).expect("the registry");
}

struct Bed {
    root: TempDir,
    clock: Arc<ManualRuntimeClock>,
    ids: IdentifierIssuer,
}

impl Bed {
    fn new(channels: &[&str]) -> Self {
        let root = TempDir::new().expect("tempdir");
        write_registry(root.path(), channels);
        Self {
            root,
            clock: Arc::new(ManualRuntimeClock::new(NOW, 0)),
            ids: IdentifierIssuer::new().expect("identifier issuer"),
        }
    }

    fn start(&self, section: Value, forward: Arc<dyn AgentForward>) -> RuntimeHost {
        let clock: Arc<dyn RuntimeClock> = self.clock.clone();
        RuntimeHost::start(
            config(&self.root)
                .with_runtime_clock(clock)
                .with_config_manifest(manifest(section))
                .with_agent_forward_for_test(forward),
            Arc::new(FakeProvider::one(
                "node.a",
                instance_id(),
                Arc::new(FakeState::default()),
            )),
        )
        .expect("runtime host")
    }

    fn receipt(&self, host: &RuntimeHost, operation: RuntimeOperation) -> RuntimeReceipt {
        host.process_request_for_test(
            &runtime_request(&self.ids, operation),
            ConnectionId::new(1).expect("connection"),
        )
        .expect("a receipt")
    }

    fn ask(&self, host: &RuntimeHost, to: &str, alias: Option<&str>) -> RuntimeReceipt {
        self.receipt(
            host,
            RuntimeOperation::AgentWakeTest {
                to: to.to_owned(),
                instance_alias: alias.map(str::to_owned),
            },
        )
    }

    fn queued(&self, host: &RuntimeHost, to: &str) -> AgentWakeTestQueued {
        let receipt = self.ask(host, to, None);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        let Some(RuntimeResult::AgentWakeTestQueued { queued }) = receipt.result() else {
            panic!("a queued test: {receipt:?}");
        };
        queued.clone()
    }

    fn status(&self, host: &RuntimeHost) -> Option<actingcommand_contract::AgentWakeStatus> {
        let receipt = self.receipt(host, RuntimeOperation::Status);
        let Some(RuntimeResult::Status { status }) = receipt.result() else {
            panic!("a status: {receipt:?}");
        };
        status.agent_wake().cloned()
    }

    fn folder(&self) -> PathBuf {
        self.root.path().join("agent-wakes")
    }
}

fn step(host: &RuntimeHost) {
    host.agent_wake_step_for_test().expect("an iteration");
}

fn rows(host: &RuntimeHost) -> Vec<WakeRow> {
    host.agent_wake_rows_for_test().expect("the rows")
}

fn state(host: &RuntimeHost) -> AgentDeliveryState {
    rows(host).last().expect("a row").state
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

fn deferred() -> ForwardOutcome {
    ForwardOutcome::Deferred {
        reason: DeferReason::TargetOffline,
        detail: "neutral detail".to_owned(),
    }
}

fn delivered() -> ForwardOutcome {
    ForwardOutcome::Delivered {
        proof: DeliveryProof::Written,
        detail: "neutral detail".to_owned(),
    }
}

// A transport that dies right after its intent, as a crash between `sending` and the result does.
struct DiesAfterIntent;

impl AgentForward for DiesAfterIntent {
    fn send(
        &self,
        _channel: &str,
        _message: &ForwardMessage,
        _limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        before_write().expect("the intent commits");
        panic!("a transport that dies after its intent");
    }
}

struct AlwaysDies;

impl AgentForward for AlwaysDies {
    fn send(
        &self,
        _channel: &str,
        _message: &ForwardMessage,
        _limits: &SendLimits<'_>,
        _before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        panic!("a transport that always dies");
    }
}

// A send during which the Runtime is asked to stop, before or after its intent.
struct StopsDuringSend {
    fatal: OnceLock<FatalState>,
    after_intent: bool,
}

impl AgentForward for StopsDuringSend {
    fn send(
        &self,
        _channel: &str,
        _message: &ForwardMessage,
        limits: &SendLimits<'_>,
        before_write: &mut dyn FnMut() -> Result<(), IntentFailed>,
    ) -> ForwardOutcome {
        let fatal = self.fatal.get().expect("the host's fatal state");
        if self.after_intent {
            before_write().expect("the intent commits");
            fatal.request_shutdown();
            assert!((limits.stopped)(), "the send sees the stop");
            return ForwardOutcome::Unknown {
                problem: DeliveryProblem::WriteTimedOut,
                detail: "cancelled".to_owned(),
            };
        }
        fatal.request_shutdown();
        assert!(before_write().is_err(), "no new `sending` after stop");
        ForwardOutcome::Failed {
            problem: DeliveryProblem::WriteFailed,
            detail: "no write".to_owned(),
        }
    }
}

#[test]
fn no_section_starts_no_worker_and_writes_no_file_or_row() {
    let bed = Bed::new(&[CHANNEL]);
    let clock: Arc<dyn RuntimeClock> = bed.clock.clone();
    let host = RuntimeHost::start(
        config(&bed.root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(
            "node.a",
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    assert!(!host.agent_wake_thread_runs_for_test().expect("the thread"));
    assert_eq!(bed.status(&host), None);
    let receipt = bed.ask(&host, CHANNEL, None);
    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), "agent_wake_disabled");
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    assert_eq!(host.agent_wake_fact_for_test().expect("the fact"), None);
    assert!(!bed.folder().exists());
}

#[test]
fn test_wake_writes_body_then_row_then_sends_once() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    let queued = bed.queued(&host, CHANNEL);
    assert_eq!(queued.channel, CHANNEL);
    assert_eq!(queued.quiet, None);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    let row = &kept[0];
    assert_eq!(row.wake_id, queued.wake_id);
    assert_eq!(row.state, AgentDeliveryState::Delivered);
    assert_eq!(row.proof.as_deref(), Some("written"));
    assert!(row.test);
    assert_eq!(row.kind, "delivery_test");
    // The body is there, of its schema, and its digest is the row's.
    let file = row.body_file.clone().expect("the body's name");
    assert_eq!(file, format!("w{NOW}.json"));
    let text = fs::read_to_string(bed.folder().join(&file)).expect("the body");
    let body: Value = serde_json::from_str(&text).expect("the body is JSON");
    assert_eq!(body["schema"], json!("actingcommand.agent-wake-body.v1"));
    assert_eq!(body["test"], json!(true));
    assert!(body.get("trigger").is_none());
    assert_eq!(
        row.body_sha256.as_deref(),
        Some(format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
    );
    // Exactly the message of the format, once, with the row's count.
    let path = format!(
        "{}{file}",
        actingcommand_runtime_config::agent_wake::body_folder_text(bed.root.path(), "agent-wakes")
    );
    let sends = forward.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].channel, CHANNEL);
    assert_eq!(
        sends[0].message.text,
        format!("ActingCommand to {CHANNEL}\n发送测试 正文 {path}")
    );
    assert_eq!(
        row.message_chars,
        u64::try_from(sends[0].message.text.chars().count()).ok()
    );
    assert!(sends[0].wrote);
    // The detail file holds what a person reads.
    let detail: Value = serde_json::from_str(
        &fs::read_to_string(bed.folder().join(format!("w{NOW}.delivery.json")))
            .expect("the detail file"),
    )
    .expect("the detail is JSON");
    assert_eq!(detail["state"], json!("delivered"));
    assert_eq!(detail["detail"], json!("neutral detail"));
    // Nothing is sent again.
    bed.clock.advance(120_000);
    step(&host);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn intent_is_committed_before_the_forward_writes() {
    let bed = Bed::new(&[CHANNEL]);
    let host = bed.start(section(), Arc::new(DiesAfterIntent));
    bed.queued(&host, CHANNEL);
    step(&host);
    // The transport never got past its intent: what it committed is durable in the fact.
    let record = host
        .agent_wake_fact_for_test()
        .expect("the fact")
        .expect("a record");
    let FactValue::RecordList(recorded) = record.value else {
        panic!("a record list");
    };
    assert_eq!(
        recorded[0].get("state"),
        Some(&FactScalar::String("sending".to_owned()))
    );
    // And the fake's order: a write only ever follows a committed intent.
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    let send = &forward.sends()[0];
    assert!(send.intent_committed && send.wrote);
}

#[test]
fn a_refused_intent_means_no_write() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    host.agent_wake_refuse_intent_for_test().expect("the hook");
    step(&host);
    let send = &forward.sends()[0];
    assert!(!send.intent_committed && !send.wrote);
    // No write happened: the row stays as it was, and the failure is said once.
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert_eq!(
        value(&said[0].0, "agent_delivery_problem"),
        Some("intent_refused")
    );
    assert!(host.fatal_error().expect("health").is_none());
}

#[test]
fn restart_turns_sending_into_unknown_and_never_resends() {
    let bed = Bed::new(&[CHANNEL]);
    let host = bed.start(section(), Arc::new(DiesAfterIntent));
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Sending);
    host.close().expect("close");

    bed.clock.advance(1_000);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    assert_eq!(state(&host), AgentDeliveryState::Unknown);
    assert_eq!(
        rows(&host)[0].problem.as_deref(),
        Some("interrupted_by_stop")
    );
    let said = failures(&host, FAILED_CODE)
        .into_iter()
        .filter(|(outcome, _)| value(outcome, "agent_delivery_state") == Some("unknown"))
        .count();
    assert_eq!(said, 1);
    bed.clock.advance(120_000);
    step(&host);
    assert_eq!(forward.sends().len(), 0, "never resent");
}

#[test]
fn restart_keeps_prepared_and_deferred_and_tries_them() {
    let bed = Bed::new(&[CHANNEL]);
    // Prepared: a hold keeps it from any send.
    let held = with(
        section(),
        "quiet.hold_until_unix_ms",
        json!(NOW + 10 * DAY_MS),
    );
    let host = bed.start(held, Arc::new(FakeForward::new([])));
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    host.close().expect("close");

    bed.clock.advance(1_000);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 1);

    // Deferred: tried at once after the restart, its wait forgotten.
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Deferred);
    host.close().expect("close");
    bed.clock.advance(1_000);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    assert_eq!(state(&host), AgentDeliveryState::Deferred);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn deferred_retries_then_expires_loudly() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([deferred(), deferred()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Deferred);
    assert_eq!(forward.sends().len(), 1);
    // Not before its retry.
    step(&host);
    assert_eq!(forward.sends().len(), 1);
    bed.clock.advance(1_000);
    step(&host);
    assert_eq!(forward.sends().len(), 2);
    assert_eq!(
        failures(&host, FAILED_CODE).len(),
        0,
        "no noise until give-up"
    );
    // Past its give-up: expired, loud once, and no send.
    bed.clock.advance(60_000);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Expired);
    assert_eq!(forward.sends().len(), 2);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert_eq!(value(&said[0].0, "agent_delivery_state"), Some("expired"));
    bed.clock.advance(60_000);
    step(&host);
    assert_eq!(failures(&host, FAILED_CODE).len(), 1);
}

#[test]
fn a_definite_failure_is_loud_and_not_retried() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Failed {
        problem: DeliveryProblem::Rejected,
        detail: "neutral refusal".to_owned(),
    }]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Failed);
    assert_eq!(rows(&host)[0].problem.as_deref(), Some("rejected"));
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert_eq!(
        value(&said[0].0, "agent_delivery_problem"),
        Some("rejected")
    );
    bed.clock.advance(120_000);
    step(&host);
    assert_eq!(forward.sends().len(), 1, "not retried");
    assert_eq!(failures(&host, FAILED_CODE).len(), 1);
}

#[test]
fn unknown_is_loud_and_not_retried() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Unknown {
        problem: DeliveryProblem::WriteTimedOut,
        detail: "neutral timeout".to_owned(),
    }]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Unknown);
    assert!(forward.sends()[0].wrote);
    assert_eq!(failures(&host, FAILED_CODE).len(), 1);
    bed.clock.advance(120_000);
    step(&host);
    assert_eq!(forward.sends().len(), 1, "never resent");
    assert_eq!(failures(&host, FAILED_CODE).len(), 1);
}

#[test]
fn a_row_writes_at_most_four_records() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([
        deferred(),
        ForwardOutcome::Deferred {
            reason: DeferReason::TargetAwaitingInput,
            detail: "another reason".to_owned(),
        },
        deferred(),
        delivered(),
    ]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    for _ in 0..4 {
        step(&host);
        bed.clock.advance(1_000);
    }
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 4);
    // `prepared`, the first `deferred`, `sending` and the result.
    assert_eq!(host.agent_wake_rows_recorded_for_test().expect("count"), 4);
}

#[test]
fn the_ledger_holds_names_digest_and_counts_never_the_text() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    let record = host
        .agent_wake_fact_for_test()
        .expect("the fact")
        .expect("a record");
    let written = serde_json::to_string(&record).expect("the record");
    // Names, a digest and counts.
    let row = &rows(&host)[0];
    for held in [
        row.channel.as_str(),
        row.body_file.as_deref().expect("a name"),
        row.body_sha256.as_deref().expect("a digest"),
    ] {
        assert!(written.contains(held), "{held}");
    }
    assert!(written.contains("message_chars"));
    // Never the message, the body or an address.
    let message = forward.sends()[0].message.text.clone();
    let body = fs::read_to_string(bed.folder().join(format!("w{NOW}.json"))).expect("the body");
    assert!(!written.contains(&message));
    assert!(!written.contains(&body));
    for held_back in [
        "ActingCommand to",
        "正文",
        "agent_wake_not_authorization",
        "00000000-0000-4000-8000-000000000001",
        "neutral detail",
    ] {
        assert!(!written.contains(held_back), "{held_back}");
    }
}

#[test]
fn failure_values_pass_the_key_table_with_a_longest_chinese_channel() {
    let longest = "频".repeat(16);
    let bed = Bed::new(&[longest.as_str()]);
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Failed {
        problem: DeliveryProblem::OwnerMismatch,
        detail: "客户端的话不进账本".to_owned(),
    }]));
    let host = bed.start(section(), forward.clone());
    let receipt = bed.ask(&host, &longest, Some("node.a"));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    step(&host);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    let outcome = &said[0].0;
    assert_eq!(value(outcome, "agent_channel"), Some(longest.as_str()));
    assert_eq!(value(outcome, "alias"), Some("node.a"));
    assert_eq!(value(outcome, "agent_delivery_state"), Some("failed"));
    assert_eq!(
        value(outcome, "agent_delivery_problem"),
        Some("owner_mismatch")
    );
    assert_eq!(
        value(outcome, "file").map(str::to_owned),
        Some(format!("w{NOW}.json"))
    );
    // A value that broke the key table would be a fatal ledger error.
    assert!(host.fatal_error().expect("health").is_none());
    assert!(!outcome.to_string().contains("客户端的话"));
}

#[test]
fn a_delivery_failure_counts_nothing_toward_direction_one() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Failed {
        problem: DeliveryProblem::Rejected,
        detail: "neutral".to_owned(),
    }]));
    let host = bed.start(section(), forward);
    // The wake names an instance; its failure still has no instance link, and direction ①'s count
    // returns at once without one.
    let receipt = bed.ask(&host, CHANNEL, Some("node.a"));
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    step(&host);
    let said = failures(&host, FAILED_CODE);
    assert_eq!(said.len(), 1);
    assert!(!said[0].1, "no instance link");
    assert!(host.fatal_error().expect("health").is_none());
    let status = bed.receipt(&host, RuntimeOperation::Status);
    let Some(RuntimeResult::Status { status }) = status.result() else {
        panic!("a status");
    };
    assert!(
        status
            .instances()
            .iter()
            .all(|instance| { instance.pause().is_none() && instance.resource_fence().is_none() })
    );
}

#[test]
fn a_delivery_failure_never_becomes_a_candidate() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([ForwardOutcome::Failed {
        problem: DeliveryProblem::Rejected,
        detail: "neutral".to_owned(),
    }]));
    let host = bed.start(section(), forward);
    bed.queued(&host, CHANNEL);
    step(&host);
    let said = failures(&host, FAILED_CODE);
    assert!(never_a_candidate(said[0].0.code().as_str()));
    assert!(never_a_candidate("agent_budget_exhausted"));
    assert!(!never_a_candidate("neutral_code"));
    // Nothing more comes of it.
    bed.clock.advance(120_000);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
}

#[test]
fn unreadable_row_shape_is_loud_and_the_start_goes_on() {
    let bed = Bed::new(&[CHANNEL]);
    let host = bed.start(section(), Arc::new(FakeForward::new([])));
    host.agent_wake_record_fact_for_test(RuntimeFactRecord {
        scope: RuntimeFactScope::Runtime,
        key: "host.agent_wake".to_owned(),
        value: FactValue::RecordList(vec![BTreeMap::from([(
            "neutral".to_owned(),
            FactScalar::Boolean(true),
        )])]),
        observed_at_unix_ms: NOW,
        source: OriginModule::Runtime,
        ttl_ms: None,
    })
    .expect("the record");
    host.close().expect("close");

    bed.clock.advance(1_000);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    assert_eq!(failures(&host, "invalid_runtime_state_fact").len(), 1);
    assert_eq!(rows(&host), Vec::new());
    assert!(host.fatal_error().expect("health").is_none());
    // The next write replaces the record.
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    let record = host
        .agent_wake_fact_for_test()
        .expect("the fact")
        .expect("a record");
    let FactValue::RecordList(recorded) = record.value else {
        panic!("a record list");
    };
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].contains_key("wake_id"));
}

#[test]
fn a_worker_panic_backs_off_and_stays_bounded() {
    let bed = Bed::new(&[CHANNEL]);
    let host = bed.start(section(), Arc::new(AlwaysDies));
    bed.queued(&host, CHANNEL);
    let said = |count: usize| {
        let said = failures(&host, FAILED_CODE);
        assert_eq!(said.len(), count);
        assert!(said.iter().all(|(outcome, _)| {
            value(outcome, "agent_delivery_problem") == Some("worker_panicked")
        }));
    };
    step(&host);
    said(1);
    assert_eq!(
        bed.status(&host).expect("a status").worker,
        AgentWakeWorkerState::BackingOff
    );
    // Within the backoff: no iteration, no record.
    step(&host);
    bed.clock.advance(999);
    step(&host);
    said(1);
    // The backoff doubles, up to its longest.
    bed.clock.advance(1);
    step(&host);
    said(2);
    bed.clock.advance(1_000);
    step(&host);
    said(2);
    bed.clock.advance(1_000);
    step(&host);
    said(3);
    bed.clock.advance(4_000);
    step(&host);
    said(4);
    bed.clock.advance(3_999);
    step(&host);
    said(4);
    assert!(host.fatal_error().expect("health").is_none());
}

#[test]
fn quiet_window_holds_sends_and_keeps_rows() {
    let bed = Bed::new(&[CHANNEL]);
    let quiet = with(
        section(),
        "quiet.windows",
        json!([{
            "kind": "once", "id": "neutral",
            "from_unix_ms": NOW - 1, "until_unix_ms": NOW + 10_000,
        }]),
    );
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(quiet, forward.clone());
    let queued = bed.queued(&host, CHANNEL);
    assert_eq!(queued.quiet, Some(AgentWakeQuiet::Window));
    step(&host);
    // A row and a body, no send, and no record per tick.
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    assert!(bed.folder().join(format!("w{NOW}.json")).is_file());
    assert_eq!(forward.sends().len(), 0);
    assert_eq!(
        bed.status(&host).expect("a status").quiet,
        Some(AgentWakeQuiet::Window)
    );
    let recorded = host.agent_wake_rows_recorded_for_test().expect("count");
    step(&host);
    assert_eq!(
        host.agent_wake_rows_recorded_for_test().expect("count"),
        recorded
    );
    bed.clock.advance(10_000);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 1);
    assert_eq!(bed.status(&host).expect("a status").quiet, None);
}

#[test]
fn hold_until_holds_sends_until_its_time() {
    let bed = Bed::new(&[CHANNEL]);
    let held = with(section(), "quiet.hold_until_unix_ms", json!(NOW + 5_000));
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(held, forward.clone());
    let queued = bed.queued(&host, CHANNEL);
    assert_eq!(queued.quiet, Some(AgentWakeQuiet::Hold));
    step(&host);
    bed.clock.advance(4_999);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    assert_eq!(forward.sends().len(), 0);
    bed.clock.advance(1);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn disabled_drops_candidates_and_holds_open_rows() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([deferred(), delivered()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Deferred);

    let receipt = bed.receipt(
        &host,
        RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key: "agent_wake.enabled".to_owned(),
                value: Some(json!(false)),
            }],
            expected_version: None,
        },
    );
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    // The request is refused, a candidate is dropped and counted, the open row waits.
    let refused = bed.ask(&host, CHANNEL, None);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(refused.outcome().code().as_str(), "agent_wake_disabled");
    assert_eq!(
        host.agent_wake_offer_for_test("agent_wake_neutral", CHANNEL)
            .expect("the queue"),
        Some(true)
    );
    bed.clock.advance(120_000);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1, "no row for a dropped candidate");
    assert_eq!(kept[0].state, AgentDeliveryState::Deferred);
    assert_eq!(kept[0].reason, Some(WaitReason::Disabled));
    assert_eq!(forward.sends().len(), 1);
    let status = bed.status(&host).expect("a status");
    assert!(!status.enabled);
    assert_eq!(status.open, 1);
    assert_eq!(status.suppressed_today, 2);
    assert_eq!(failures(&host, FAILED_CODE).len(), 0, "off is no failure");
}

#[test]
fn give_up_is_read_only_when_a_send_is_due() {
    let bed = Bed::new(&[CHANNEL]);
    let held = with(section(), "quiet.hold_until_unix_ms", json!(NOW + 120_000));
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(held, forward.clone());
    bed.queued(&host, CHANNEL);
    step(&host);
    // Far past its give-up, but quiet: no expiry and no noise.
    bed.clock.advance(90_000);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    assert_eq!(failures(&host, FAILED_CODE).len(), 0);
    // When quiet ends it is expired, loud once, and no message goes to anyone.
    bed.clock.advance(30_000);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Expired);
    assert_eq!(failures(&host, FAILED_CODE).len(), 1);
    assert_eq!(forward.sends().len(), 0);
}

#[test]
fn rows_for_one_channel_go_oldest_first_with_the_gap() {
    let bed = Bed::new(&[CHANNEL]);
    let gapped = with(section(), "delivery.channel_gap_ms", json!(5_000));
    let forward = Arc::new(FakeForward::new([delivered(), delivered()]));
    let host = bed.start(gapped, forward.clone());
    let older = bed.queued(&host, CHANNEL);
    let younger = bed.queued(&host, CHANNEL);
    step(&host);
    let delivered_ids = || {
        rows(&host)
            .into_iter()
            .filter(|row| row.state == AgentDeliveryState::Delivered)
            .map(|row| row.wake_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(delivered_ids(), vec![older.wake_id.clone()]);
    step(&host);
    bed.clock.advance(4_999);
    step(&host);
    assert_eq!(forward.sends().len(), 1, "within the gap");
    bed.clock.advance(1);
    step(&host);
    assert_eq!(forward.sends().len(), 2);
    assert_eq!(delivered_ids(), vec![older.wake_id, younger.wake_id]);
    // Every message is different: a collision takes the next file name.
    let sends = forward.sends();
    assert!(sends[1].message.text.contains(&format!("w{NOW}-2.json")));
}

#[test]
fn an_unknown_channel_fails_at_once() {
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    // The door refuses a name the readable registry does not list.
    let refused = bed.ask(&host, "别的频道", None);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        "agent_wake_channel_unknown"
    );
    // A channel removed after its wake was made ends the row at once, with no wait.
    bed.queued(&host, CHANNEL);
    write_registry(bed.root.path(), &["别的频道"]);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Failed);
    assert_eq!(rows(&host)[0].problem.as_deref(), Some("channel_unknown"));
    assert_eq!(failures(&host, "agent_wake_channel_unknown").len(), 1);
    assert_eq!(forward.sends().len(), 0);
}

#[test]
fn an_unreadable_registry_never_stops_a_start() {
    let bed = Bed::new(&[CHANNEL]);
    fs::write(
        bed.root.path().join("agent-forward").join("channels.json"),
        "not a registry",
    )
    .expect("a broken registry");
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    assert!(host.fatal_error().expect("health").is_none());
    // The test is taken and its row waits, with the reason.
    bed.queued(&host, CHANNEL);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept[0].state, AgentDeliveryState::Deferred);
    assert_eq!(
        kept[0].reason.map(WaitReason::as_str),
        Some("registry_unreadable")
    );
    assert_eq!(forward.sends().len(), 0);
    assert!(bed.status(&host).expect("a status").attention);
    // When the file is good again it goes out.
    write_registry(bed.root.path(), &[CHANNEL]);
    bed.clock.advance(1_000);
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Delivered);
    assert!(!bed.status(&host).expect("a status").attention);
}

#[test]
fn retention_removes_by_age_and_count_and_keeps_open_rows() {
    let bed = Bed::new(&[CHANNEL]);
    let section = with(section(), "body.retention_days", json!(1));
    let section = with(section, "body.max_files", json!(8));
    let section = with(
        section,
        "quiet.hold_until_unix_ms",
        json!(NOW + 10 * DAY_MS),
    );
    let host = bed.start(section, Arc::new(FakeForward::new([])));
    bed.queued(&host, CHANNEL);
    step(&host);
    let open = format!("w{NOW}.json");
    assert!(bed.folder().join(&open).is_file());

    let now = NOW + 2 * DAY_MS + 60 * 60 * 1_000;
    bed.clock.set_unix_ms(now);
    let old = NOW - DAY_MS;
    for name in [
        format!("w{old}.json"),
        format!("w{old}.delivery.json"),
        "note.txt".to_owned(),
    ] {
        fs::write(bed.folder().join(name), "{}").expect("a file");
    }
    for age in 1..=10_u64 {
        fs::write(bed.folder().join(format!("w{}.json", now - age)), "{}").expect("a file");
    }
    step(&host);
    // By age, then by count (8), oldest first; the open row's file stays though it is old, and a
    // file of no wake's name is not the sweep's.
    assert!(bed.folder().join(&open).is_file());
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    assert!(!bed.folder().join(format!("w{old}.json")).exists());
    assert!(!bed.folder().join(format!("w{old}.delivery.json")).exists());
    assert!(bed.folder().join("note.txt").is_file());
    for age in 8..=10_u64 {
        assert!(!bed.folder().join(format!("w{}.json", now - age)).exists());
    }
    for age in 1..=7_u64 {
        assert!(bed.folder().join(format!("w{}.json", now - age)).is_file());
    }
}

#[test]
fn shutdown_after_intent_is_unknown_and_before_intent_stays() {
    for after_intent in [true, false] {
        let bed = Bed::new(&[CHANNEL]);
        let forward = Arc::new(StopsDuringSend {
            fatal: OnceLock::new(),
            after_intent,
        });
        let host = bed.start(section(), forward.clone());
        let fatal = host.fatal_state_for_test().expect("the fatal state");
        assert!(forward.fatal.set(fatal).is_ok());
        bed.queued(&host, CHANNEL);
        step(&host);
        if after_intent {
            assert_eq!(state(&host), AgentDeliveryState::Unknown);
            assert_eq!(rows(&host)[0].problem.as_deref(), Some("write_timed_out"));
        } else {
            assert_eq!(state(&host), AgentDeliveryState::Prepared);
            assert_eq!(failures(&host, FAILED_CODE).len(), 0);
        }
        host.close().expect("close");
    }
    // Asked to stop before a send starts: no send at all.
    let bed = Bed::new(&[CHANNEL]);
    let forward = Arc::new(FakeForward::new([delivered()]));
    let host = bed.start(section(), forward.clone());
    bed.queued(&host, CHANNEL);
    host.agent_wake_request_shutdown_for_test().expect("stop");
    step(&host);
    assert_eq!(state(&host), AgentDeliveryState::Prepared);
    assert_eq!(forward.sends().len(), 0);
    host.close().expect("close");
}

// Left to this slice by AGT-1a: the section check has no instance list.
#[test]
fn a_rule_naming_an_unknown_alias_is_refused_while_running() {
    let bed = Bed::new(&[CHANNEL]);
    let host = bed.start(section(), Arc::new(FakeForward::new([])));
    let rules = |alias: &str| RuntimeOperation::ConfigureSettings {
        changes: vec![ConfigSettingChange {
            key: "agent_wake.rules".to_owned(),
            value: Some(json!([{
                "id": "neutral", "on": {"kinds": ["delivery_test"]}, "instances": [alias],
                "to": CHANNEL, "cooldown_ms": 0, "daily_cap": 1,
            }])),
        }],
        expected_version: None,
    };
    let refused = bed.receipt(&host, rules("node.other"));
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(refused.outcome().code().as_str(), "config_value_invalid");
    assert_eq!(
        value(refused.outcome(), "config_value_problem"),
        Some("instance_unknown")
    );
    let taken = bed.receipt(&host, rules("node.a"));
    assert_eq!(taken.state(), RuntimeReceiptState::Completed, "{taken:?}");
}

#[test]
fn agent_wake_test_takes_only_person_origins() {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let refused = |actor: EventActor, source: EventSource| {
        let request = RuntimeRequest::new(
            ids.mint_request_id().expect("request id"),
            ids.mint_correlation_id().expect("correlation id"),
            None,
            actor,
            source,
            NOW,
            RuntimeOperation::AgentWakeTest {
                to: CHANNEL.to_owned(),
                instance_alias: None,
            },
        );
        match request {
            Ok(request) => request.validate().err().map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        }
    };
    assert_eq!(refused(EventActor::User, EventSource::Ui), None);
    assert_eq!(refused(EventActor::Cli, EventSource::Cli), None);
    assert!(refused(EventActor::Agent, EventSource::Adapter).is_some());
    let lab = refused(EventActor::Lab, EventSource::Lab).expect("the Lab is refused");
    assert!(lab.contains("invalid_agent_wake_test_origin"), "{lab}");
    // It is an instruction of no instance: its failures can fence nothing.
    let operation = RuntimeOperation::AgentWakeTest {
        to: CHANNEL.to_owned(),
        instance_alias: Some("node.a".to_owned()),
    };
    assert_eq!(operation.instance_alias(), None);
}
