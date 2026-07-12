// SPDX-License-Identifier: AGPL-3.0-only

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    EventActor, EventPayload, EventQuery, EventSource, EventType, ProjectionPayload,
    ProjectionProfile, TaskOutcome, TaskPayload, TaskSemanticFact,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::Value;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

#[test]
fn actingctl_observe_and_reset_leave_runtime_alive_and_share_projection_shape() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());

    let observe = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "observe",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "ak.cn",
        ])
        .output()
        .expect("run actingctl observe");
    assert!(
        observe.status.success(),
        "actingctl observe failed: {}",
        String::from_utf8_lossy(&observe.stderr)
    );
    let observe_json: Value = serde_json::from_slice(&observe.stdout).expect("observe JSON");
    assert_eq!(
        observe_json["receipt"]["result"]["kind"],
        "readonly_observation_completed"
    );
    assert!(observe_json["events"].as_array().is_some_and(|events| {
        events
            .iter()
            .any(|event| event["event_type"] == "recognition.completed")
    }));
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture"]
    );
    runtime.assert_alive();

    let reset = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "reset",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "ak.cn",
        ])
        .output()
        .expect("run actingctl reset");
    assert!(
        reset.status.success(),
        "actingctl reset failed: {}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let reset_json: Value = serde_json::from_slice(&reset.stdout).expect("reset JSON");
    assert_eq!(
        reset_json["receipt"]["result"]["kind"],
        "safe_reset_completed"
    );
    assert!(reset_json["events"].is_array());
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "reset"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "reset",
            "capture_close",
            "close"
        ]
    );
}

#[test]
fn actingctl_status_monitor_and_stream_are_runtime_backed() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let mut runtime = support::RuntimeChild::spawn(root.path(), "c4_runtime_child_process");
    runtime.wait_ready(root.path());
    let binary = env!("CARGO_BIN_EXE_actingctl");
    let state_root = root.path().to_str().expect("state root");

    let status = run_json(binary, ["status", "--state-root", state_root]);
    assert_eq!(status["instances"][0]["instance_alias"], "ak.cn");

    let monitor_status = run_json(binary, ["monitor-status", "--state-root", state_root]);
    assert_eq!(monitor_status["instances"][0]["instance_alias"], "ak.cn");
    assert!(monitor_status["instances"][0].get("policy").is_none());

    let configured = run_json(
        binary,
        [
            "monitor-set",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
            "--interval-ms",
            "60000",
            "--expect",
            "home",
        ],
    );
    assert_eq!(configured["instance_alias"], "ak.cn");
    assert_eq!(configured["policy"]["expected_page"], "home");

    let cleared = run_json(
        binary,
        [
            "monitor-clear",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
        ],
    );
    assert_eq!(cleared["instance_alias"], "ak.cn");
    assert!(cleared.get("policy").is_none());

    let stream = run_json(
        binary,
        [
            "stream",
            "--state-root",
            state_root,
            "--instance",
            "ak.cn",
            "--max-frames",
            "2",
            "--interval-ms",
            "1",
        ],
    );
    assert_eq!(
        stream["receipt"]["result"]["kind"],
        "capture_sequence_completed"
    );
    assert_eq!(
        stream["receipt"]["result"]["sequence"]["observations"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    runtime.assert_alive();
    runtime.stop_clean();
}

#[test]
fn actingctl_runs_neutral_contained_task_without_lab_and_runtime_survives_client_exit() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let expected_sha256 = write_neutral_contained_task_package(&package);
    let mut runtime = support::RuntimeChild::spawn_for_instance(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
    );
    runtime.wait_ready(root.path());

    let output = run_json(
        env!("CARGO_BIN_EXE_actingctl"),
        [
            "task-run",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "neutral.instance",
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ],
    );

    assert_eq!(
        output["receipt"]["result"]["kind"],
        "contained_task_completed"
    );
    assert_eq!(
        output["receipt"]["result"]["final_page"],
        "neutral/terminal"
    );
    assert_eq!(output["receipt"]["result"]["executed_steps"], 1);
    assert_eq!(
        support::backend_events(root.path()),
        ["capture_open", "capture", "open", "tap", "capture"]
    );
    runtime.assert_alive();
    runtime.stop_clean();
    assert_eq!(
        support::backend_events(root.path()),
        [
            "capture_open",
            "capture",
            "open",
            "tap",
            "capture",
            "capture_close",
            "close"
        ]
    );
}

#[test]
fn runtime_finishes_and_rebuilds_contained_task_after_client_is_killed() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let expected_sha256 = write_neutral_contained_task_package(&package);
    let mut runtime = support::RuntimeChild::spawn_for_instance_with_input_delay(
        root.path(),
        "c4_runtime_child_process",
        "neutral.instance",
        Duration::from_millis(500),
    );
    runtime.wait_ready(root.path());

    let mut client_process = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "task-run",
            "--state-root",
            root.path().to_str().expect("state root"),
            "--instance",
            "neutral.instance",
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &expected_sha256,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn actingctl task-run");
    wait_until(Duration::from_secs(5), || {
        support::backend_events(root.path())
            .iter()
            .any(|event| event == "tap_started")
    });
    client_process.kill().expect("kill actingctl client");
    let status = client_process.wait().expect("wait killed actingctl client");
    assert!(!status.success(), "killed client unexpectedly succeeded");

    wait_until(Duration::from_secs(5), || {
        let events = support::backend_events(root.path());
        events.iter().any(|event| event == "tap")
            && events.iter().filter(|event| *event == "capture").count() == 2
    });
    runtime.assert_alive();

    let ledger_client = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect fresh Runtime client");
    let events = wait_for_terminal_events(&ledger_client);
    let facts = events
        .iter()
        .filter_map(|event| match &event.payload {
            ProjectionPayload::Full(EventPayload::Task(TaskPayload::Semantic(payload))) => {
                Some(payload.fact())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for required in [
        "package_admitted",
        "run_started",
        "evidence_indexed",
        "recognition_started",
        "recognition_completed",
        "step_started",
        "effect_intent",
        "effect_completed",
        "step_finished",
        "finalizing",
        "terminal_committed",
    ] {
        assert!(
            facts.iter().any(|fact| task_fact_kind(fact) == required),
            "missing Runtime semantic fact {required}"
        );
    }
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::TerminalCommitted {
                outcome: TaskOutcome::Success,
                final_page: Some(page),
                executed_steps: 1,
                failure_code: None,
            } if page == "neutral/terminal"
        )),
        "unexpected Runtime semantic facts after client kill: {facts:#?}"
    );
    let sequence = |event_type| {
        events
            .iter()
            .find(|event| event.event_type == event_type)
            .map(|event| event.sequence)
            .unwrap_or_else(|| panic!("missing {event_type:?}"))
    };
    assert!(sequence(EventType::TaskEffectIntent) < sequence(EventType::InputIntent));
    assert!(sequence(EventType::InputIntent) < sequence(EventType::InputCommitted));
    assert!(sequence(EventType::InputCommitted) < sequence(EventType::TaskEffectCompleted));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == EventType::TaskCompleted)
            .count(),
        1
    );

    drop(ledger_client);
    runtime.assert_alive();
    runtime.stop_clean();
}

fn wait_for_terminal_events(client: &RuntimeClient) -> Vec<actingcommand_contract::ProjectedEvent> {
    let started = Instant::now();
    loop {
        let events = client
            .query_events(EventQuery::default(), ProjectionProfile::Forensic)
            .expect("query Runtime ledger after client kill");
        if events.iter().any(|event| {
            matches!(
                event.event_type,
                EventType::TaskCompleted | EventType::TaskFailed | EventType::TaskCancelled
            )
        }) {
            return events;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Runtime task terminal did not become durable after client kill"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn task_fact_kind(fact: &TaskSemanticFact) -> &'static str {
    match fact {
        TaskSemanticFact::PackageAdmitted { .. } => "package_admitted",
        TaskSemanticFact::RunStarted => "run_started",
        TaskSemanticFact::EvidenceIndexed { .. } => "evidence_indexed",
        TaskSemanticFact::RecognitionStarted { .. } => "recognition_started",
        TaskSemanticFact::RecognitionCompleted { .. } => "recognition_completed",
        TaskSemanticFact::StepStarted { .. } => "step_started",
        TaskSemanticFact::EffectIntent { .. } => "effect_intent",
        TaskSemanticFact::EffectCompleted { .. } => "effect_completed",
        TaskSemanticFact::StepFinished { .. } => "step_finished",
        TaskSemanticFact::Finalizing { .. } => "finalizing",
        TaskSemanticFact::TerminalCommitted { .. } => "terminal_committed",
        TaskSemanticFact::TerminalRejected { .. } => "terminal_rejected",
    }
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn write_neutral_contained_task_package(path: &Path) -> String {
    let cursor = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(cursor);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let files: &[(&str, &[u8])] = &[
        (
            "control.json",
            br#"{
                "schema_version":"Lab-1y.control.v1",
                "package_id":"neutral.semantic.task",
                "execution_mode":"navigable_route",
                "game":"neutral",
                "server":"test",
                "resolution":{"width":2,"height":1},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":1000,
                "max_steps":2
            }"#,
        ),
        (
            "resources/manifest.json",
            br#"{"schema_version":"0.3","entry_task_id":"task"}"#,
        ),
        (
            "resources/operations/task/task.json",
            br#"{
                "schema_version":"0.6",
                "task_id":"task",
                "game":"neutral",
                "server_scope":["test"],
                "coordinate_space":{"width":2,"height":1},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0}
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":2,"height":1},
                "defaults":{"color_max_distance":0.0},
                "targets":[
                    {"type":"color","id":"page/home","region":{"x":0,"y":0,"width":1,"height":1},"expected":[255,0,0]},
                    {"type":"color","id":"page/terminal","region":{"x":0,"y":0,"width":1,"height":1},"expected":[0,0,255]}
                ]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pages.json",
            br#"{
                "schema_version":"0.3",
                "pages":[
                    {"id":"neutral/home","required":["page/home"],"optional":[],"forbidden":[]},
                    {"id":"neutral/terminal","required":["page/terminal"],"optional":[],"forbidden":[]}
                ]
            }"#,
        ),
    ];
    for (entry, contents) in files {
        zip.start_file(*entry, options).expect("zip entry");
        zip.write_all(contents).expect("zip contents");
    }
    let bytes = zip.finish().expect("finish zip").into_inner();
    fs::write(path, &bytes).expect("write neutral contained package");
    Sha256Hash::digest(&bytes).to_string()
}

fn run_json<const N: usize>(binary: &str, arguments: [&str; N]) -> Value {
    let output = Command::new(binary)
        .args(arguments)
        .output()
        .expect("run actingctl");
    assert!(
        output.status.success(),
        "actingctl failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("actingctl JSON")
}
