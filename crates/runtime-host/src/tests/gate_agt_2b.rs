// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #432 AGT-2b (`contracts/agent-wake.md`, "The daily summary"; register §1.263; design
// rd-1031/agt/model-agt-v2.md sections 4, 5.3 and 10): a `daily_summary` rule's summary, over a
// real ledger with a fake clock, a fixed host zone and a fake transport. No worker thread runs:
// each test runs the worker's iteration itself, so nothing here sleeps or reads a wall clock. The
// names are made up.

use super::*;
use crate::agent_wake::rows::WakeRow;
use crate::host::WakeRecordForTest;
use actingcommand_agent_forward::{
    AgentForward, DeliveryProblem, DeliveryProof, FakeForward, ForwardOutcome, fake_registry_text,
};
use actingcommand_contract::{
    AgentDeliveryState, AgentWakeQuiet, AgentWakeStatus, ConfigSettingChange,
    RoutineSummaryGranularity, RuntimeConfigManifest, RuntimeReceipt, RuntimeReceiptState,
};
use actingcommand_runtime_config::{
    DefaultsFile, annotate_settable, mechanism_rows, render_parameters,
};
use serde_json::{Value, json};

/// 2026-10-26 07:33:20 UTC, a Monday: 16:33:20 nine hours east of UTC.
const NOW: u64 = 1_793_000_000_000;
const MINUTE_MS: u64 = 60_000;
const DAY_MS: u64 = 24 * 60 * MINUTE_MS;
/// The summary rules' zone: nine hours east of UTC.
const EAST_NINE_MS: u64 = 9 * 60 * MINUTE_MS;
const CHANNEL: &str = "总览";
const ALIAS: &str = "node.a";

/// The instant of `minute` on the local day `days` after NOW's, nine hours east of UTC.
fn at(days: u64, minute: u64) -> u64 {
    let midnight = (NOW + EAST_NINE_MS) / DAY_MS * DAY_MS - EAST_NINE_MS;
    midnight + days * DAY_MS + minute * MINUTE_MS
}

fn summary_rule(minute: u64) -> Value {
    json!({
        "id": "daily",
        "on": {"kinds": ["daily_summary"], "at_minute_of_day": minute, "utc_offset_minutes": 540},
        "instances": "*", "to": CHANNEL, "cooldown_ms": 0, "daily_cap": 1,
    })
}

fn fence_rule() -> Value {
    json!({
        "id": "fence", "on": {"kinds": ["instance_fenced"]}, "instances": "*", "to": CHANNEL,
        "cooldown_ms": 0, "daily_cap": 8,
    })
}

// A whole section: no coalescing, no gap, no quiet.
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

// The section, with the recovery ladder off so that no ladder of the host's own starts.
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

    // A host whose own zone is UTC, so the rule's day is its own offset's.
    fn start(&self, section: Value, forward: Arc<dyn AgentForward>) -> RuntimeHost {
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
        host.agent_wake_set_local_offset_for_test(0)
            .expect("the host's zone");
        host
    }

    // The clock moves forward to `instant`.
    fn move_to(&self, instant: u64) {
        let now = self.clock.sample().expect("the clock").unix_ms;
        assert!(instant >= now, "the clock only moves forward");
        self.clock.advance(instant - now);
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

    fn body_path(&self, row: &WakeRow) -> String {
        let file = row.body_file.as_deref().expect("the body's name");
        format!(
            "{}{file}",
            actingcommand_runtime_config::agent_wake::body_folder_text(
                self.root.path(),
                "agent-wakes"
            )
        )
    }

    fn body(&self, row: &WakeRow) -> Value {
        let file = row.body_file.as_deref().expect("the body's name");
        let text =
            fs::read_to_string(self.root.path().join("agent-wakes").join(file)).expect("the body");
        serde_json::from_str(&text).expect("the body is JSON")
    }
}

fn step(host: &RuntimeHost) {
    host.agent_wake_step_for_test().expect("an iteration");
}

fn rows(host: &RuntimeHost) -> Vec<WakeRow> {
    host.agent_wake_rows_for_test().expect("the rows")
}

#[test]
fn daily_summary_fires_once_at_its_local_minute() {
    let bed = Bed::new();
    let forward = delivering(2);
    let host = bed.start(section(json!([summary_rule(21 * 60)])), forward.clone());
    // Before 21:00 of its own zone: nothing.
    step(&host);
    bed.move_to(at(0, 21 * 60) - 1);
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    assert_eq!(forward.sends().len(), 0);

    // At 21:00 there, 12:00 in the host's own zone: one summary, sent.
    bed.move_to(at(0, 21 * 60));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    let row = &kept[0];
    assert_eq!(row.kind, "daily_summary");
    assert_eq!(row.rule.as_deref(), Some("daily"));
    assert!(!row.test);
    assert_eq!(row.instance_alias, None);
    assert_eq!(row.code, None);
    assert_eq!(row.created_at_unix_ms, at(0, 21 * 60));
    assert_eq!(row.state, AgentDeliveryState::Delivered);
    // The line names the kind word and the rule's local date, within the limit.
    let sends = forward.sends();
    assert_eq!(sends.len(), 1);
    let text = &sends[0].message.text;
    assert_eq!(
        text,
        &format!(
            "ActingCommand to {CHANNEL}\n今日汇总 2026-10-26 正文 {}",
            bed.body_path(row)
        )
    );
    let chars = u64::try_from(text.chars().count()).expect("a count");
    assert_eq!(row.message_chars, Some(chars));
    assert!(chars <= 2048);
    assert!(!row.alias_cut);
    // A cap of one that its summary fills is no cap used up.
    assert!(!bed.status(&host).attention);

    // The rest of that day, and the next day before its minute: nothing more.
    bed.move_to(at(1, 0) - 1);
    step(&host);
    bed.move_to(at(1, 21 * 60) - 1);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(forward.sends().len(), 1);

    // The next day's minute: the next day's summary.
    bed.move_to(at(1, 21 * 60));
    step(&host);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[1].created_at_unix_ms, at(1, 21 * 60));
    assert_eq!(bed.body(&kept[1])["summary"]["date"], json!("2026-10-27"));
    let sends = forward.sends();
    assert_eq!(sends.len(), 2);
    assert!(
        sends[1].message.text.starts_with(&format!(
            "ActingCommand to {CHANNEL}\n今日汇总 2026-10-27 正文 "
        )),
        "{}",
        sends[1].message.text
    );
}

#[test]
fn a_missed_summary_is_sent_once_after_start_the_same_day() {
    let bed = Bed::new();
    // 16:00 of its zone: NOW, 16:33:20, is past it, and no Runtime ran at it.
    let rules = json!([summary_rule(16 * 60)]);
    let host = bed.start(section(rules.clone()), delivering(1));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1, "sent after the start");
    assert_eq!(kept[0].state, AgentDeliveryState::Delivered);
    assert_eq!(bed.body(&kept[0])["summary"]["date"], json!("2026-10-26"));
    step(&host);
    assert_eq!(rows(&host).len(), 1, "once");
    host.close().expect("close");

    // A restart later the same day sends none: the rows hold that day's.
    bed.move_to(at(0, 20 * 60));
    let forward = delivering(1);
    let host = bed.start(section(rules.clone()), forward.clone());
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(forward.sends().len(), 0);
    host.close().expect("close");

    // No Runtime the whole next day: a start the day after sends that day's only, never the
    // missed day's.
    bed.move_to(at(2, 18 * 60));
    let forward = delivering(2);
    let host = bed.start(section(rules), forward.clone());
    step(&host);
    bed.move_to(at(3, 0) - 1);
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 2);
    assert_eq!(bed.body(&kept[1])["summary"]["date"], json!("2026-10-28"));
    let sends = forward.sends();
    assert_eq!(sends.len(), 1);
    assert!(
        sends[0]
            .message
            .text
            .contains("\n今日汇总 2026-10-28 正文 "),
        "{}",
        sends[0].message.text
    );
}

#[test]
fn a_summary_inside_a_quiet_window_waits_for_its_end() {
    let bed = Bed::new();
    let forward = delivering(1);
    // Quiet every day from 20:00 to 22:00 of the rule's zone; the summary is due at 21:00.
    let mut quiet = section(json!([summary_rule(21 * 60)]));
    quiet["quiet"]["windows"] = json!([{
        "id": "evening", "weekdays": [1, 2, 3, 4, 5, 6, 7], "utc_offset_minutes": 540,
        "start_minute_of_day": 20 * 60, "end_minute_of_day": 22 * 60,
    }]);
    let host = bed.start(quiet, forward.clone());
    bed.move_to(at(0, 21 * 60));
    step(&host);
    assert_eq!(rows(&host), Vec::new(), "it waits");
    assert_eq!(bed.status(&host).quiet, Some(AgentWakeQuiet::Window));
    bed.move_to(at(0, 22 * 60) - 1);
    step(&host);
    assert_eq!(rows(&host), Vec::new());
    assert_eq!(forward.sends().len(), 0);

    // At the window's end: that day's summary, sent once.
    bed.move_to(at(0, 22 * 60));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].created_at_unix_ms, at(0, 22 * 60));
    assert_eq!(kept[0].state, AgentDeliveryState::Delivered);
    assert_eq!(bed.body(&kept[0])["summary"]["date"], json!("2026-10-26"));
    assert_eq!(bed.status(&host).quiet, None);
    bed.move_to(at(1, 0) - 1);
    step(&host);
    assert_eq!(rows(&host).len(), 1);
    assert_eq!(forward.sends().len(), 1);
}

#[test]
fn a_day_whose_summary_could_not_be_sent_is_named_in_the_next() {
    let bed = Bed::new();
    let forward = delivering(2);
    let host = bed.start(section(json!([summary_rule(21 * 60)])), forward.clone());
    bed.move_to(at(0, 21 * 60));
    step(&host);
    let first = bed.body(&rows(&host)[0]);
    assert_eq!(first["summary"]["date"], json!("2026-10-26"));
    assert_eq!(first["summary"]["previous"], Value::Null);
    assert_eq!(first["summary"]["skipped"], Value::Null);

    // A hold from before the next day's minute until after that day's end, as a person sets it.
    let held = bed.receipt(
        &host,
        RuntimeOperation::ConfigureSettings {
            changes: vec![ConfigSettingChange {
                key: "agent_wake.quiet.hold_until_unix_ms".to_owned(),
                value: Some(json!(at(2, 60))),
            }],
            expected_version: None,
        },
    );
    assert_eq!(held.state(), RuntimeReceiptState::Completed, "{held:?}");
    bed.move_to(at(1, 21 * 60));
    step(&host);
    assert_eq!(bed.status(&host).quiet, Some(AgentWakeQuiet::Hold));
    bed.move_to(at(2, 0) - 1);
    step(&host);
    assert_eq!(rows(&host).len(), 1, "the day ended first");
    // The hold ends the day after: never the day before's summary.
    bed.move_to(at(2, 60));
    step(&host);
    assert_eq!(bed.status(&host).quiet, None);
    assert_eq!(rows(&host).len(), 1);

    // That day's summary names the day that had none, after the one before it.
    bed.move_to(at(2, 21 * 60));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 2);
    let summary = &bed.body(&kept[1])["summary"];
    assert_eq!(summary["date"], json!("2026-10-28"));
    assert_eq!(
        summary["previous"],
        json!({"date": "2026-10-26", "state": "delivered"})
    );
    assert_eq!(
        summary["skipped"],
        json!({"from": "2026-10-27", "through": "2026-10-27", "days": 1})
    );
    assert_eq!(forward.sends().len(), 2);
}

#[test]
fn the_summary_body_holds_the_routine_summary_and_the_days_wakes() {
    let bed = Bed::new();
    let forward = Arc::new(FakeForward::new([
        delivered(),
        ForwardOutcome::Failed {
            problem: DeliveryProblem::Rejected,
            detail: "neutral refusal".to_owned(),
        },
        delivered(),
        delivered(),
    ]));
    let host = bed.start(
        section(json!([fence_rule(), summary_rule(21 * 60)])),
        forward.clone(),
    );
    // A wake of the day before, which the summary leaves out.
    host.agent_wake_append_for_test(ALIAS, WakeRecordForTest::FenceAwaitingPerson)
        .expect("the record is committed");
    step(&host);
    // Two of the day: one refused, one delivered.
    bed.move_to(at(1, 10 * 60));
    for _ in 0..2 {
        host.agent_wake_append_for_test(ALIAS, WakeRecordForTest::FenceAwaitingPerson)
            .expect("the record is committed");
        step(&host);
        bed.clock.advance(1_000);
    }
    bed.move_to(at(1, 21 * 60));
    step(&host);
    let kept = rows(&host);
    assert_eq!(kept.len(), 4);
    assert_eq!(kept[1].state, AgentDeliveryState::Failed);
    let row = &kept[3];
    assert_eq!(row.kind, "daily_summary");
    assert_eq!(row.state, AgentDeliveryState::Delivered);

    let body = bed.body(row);
    assert_eq!(body["schema"], json!("actingcommand.agent-wake-body.v1"));
    assert_eq!(body["kind"], json!("daily_summary"));
    assert_eq!(body["rule"], json!("daily"));
    assert_eq!(body["channel"], json!(CHANNEL));
    assert_eq!(body["test"], json!(false));
    assert_eq!(body["notice"], json!("agent_wake_not_authorization"));
    assert!(body.get("instance_alias").is_none());
    assert!(body.get("trigger").is_none());
    let summary = &body["summary"];
    assert_eq!(summary["date"], json!("2026-10-27"));
    assert_eq!(summary["at_minute_of_day"], json!(21 * 60));
    assert_eq!(summary["utc_offset_minutes"], json!(540));
    // The day's wakes, by kind and the state each is in.
    assert_eq!(
        summary["wakes"],
        json!([
            {"kind": "instance_fenced", "state": "delivered", "count": 1},
            {"kind": "instance_fenced", "state": "failed", "count": 1},
        ])
    );
    // The Runtime's epoch and start, as its start report says.
    let reported = bed.receipt(&host, RuntimeOperation::StartReport);
    let Some(RuntimeResult::StartReport { report }) = reported.result() else {
        panic!("a start report: {reported:?}");
    };
    assert_eq!(
        summary["runtime"]["owner_epoch"],
        serde_json::to_value(report.owner_epoch).expect("the epoch")
    );
    assert_eq!(
        summary["runtime"]["started_at_unix_ms"],
        json!(report.started_at_unix_ms)
    );
    // Each instance of the rule, with what `RoutineSummary` answers for it.
    let instances = summary["instances"].as_array().expect("a list");
    assert_eq!(instances.len(), 1);
    assert_eq!(instances[0]["instance_alias"], json!(ALIAS));
    let answered = bed.receipt(
        &host,
        RuntimeOperation::RoutineSummary {
            instance: Some(ALIAS.to_owned()),
            granularity: RoutineSummaryGranularity::Day,
            since_revision: None,
        },
    );
    match answered.result() {
        Some(RuntimeResult::RoutineSummary { summary: page }) => {
            assert_eq!(
                instances[0]["routine_summary_page"],
                serde_json::to_value(page).expect("the page")
            );
        }
        _ => {
            assert_eq!(
                answered.state(),
                RuntimeReceiptState::Denied,
                "{answered:?}"
            );
            let refused = &instances[0]["routine_summary_refused"];
            assert_eq!(refused["code"], json!(answered.outcome().code().as_str()));
            assert_eq!(refused["detail"], json!("codes"));
            assert!(instances[0].get("routine_summary_page").is_none());
        }
    }
}
