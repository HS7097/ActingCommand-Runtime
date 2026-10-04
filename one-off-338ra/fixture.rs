// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 Ra R1/R2 evidence on the fixture runtime
// (tests/support/c4_runtime.rs). The one-off workflow copies this file to
// apps/actingctl/tests/oneoff_338ra.rs; nothing in the repository's product files changes.

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    ContainedTaskCancellationReason, ContainedTaskCancellationStatus, ContainedTaskRequest,
    EventActor, EventSource, IdentifierIssuer, InstanceId, RUNTIME_INFO_FILE, RequestId,
    RuntimeInfo, RuntimeReceipt, TaskOutcome,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{
    ContainedRunState, ContainedRunStatus, ContainedTaskResetOutcome, RunDispatch, RunKey,
    RunOrigin, RunStatusMode, RuntimeClient, RuntimeClientConfig, RuntimeFlowOutput,
};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const ALIAS: &str = "neutral.instance";
const IDENTITY_KEYS: &[&str] = &[
    "request_id",
    "correlation_id",
    "run_id",
    "task_id",
    "task_request_id",
    "event_id",
    "sequence",
    "response_deadline_monotonic_ms",
];

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

struct Runtime {
    child: Option<Child>,
    stop_path: PathBuf,
    log_path: PathBuf,
}

impl Runtime {
    fn spawn(root: &Path, instance_id: InstanceId, input_delay_ms: u64, label: &str) -> Self {
        let stop_path = root.join("stop-runtime");
        let log_path = root.join(format!("runtime-{label}.log"));
        let log = File::create(&log_path).expect("runtime log");
        let child = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "c4_runtime_child_process",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ACTINGCOMMAND_C4_TEST_CHILD", "1")
            .env("ACTINGCOMMAND_C4_TEST_ROOT", root)
            .env(
                "ACTINGCOMMAND_C4_TEST_INSTANCE",
                serde_json::to_string(&instance_id).expect("instance JSON"),
            )
            .env("ACTINGCOMMAND_C4_TEST_INSTANCE_ALIAS", ALIAS)
            .env("ACTINGCOMMAND_C4_TEST_STOP", &stop_path)
            .env(
                "ACTINGCOMMAND_C4_TEST_INPUT_DELAY_MS",
                input_delay_ms.to_string(),
            )
            .stdout(Stdio::from(log.try_clone().expect("runtime log clone")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn fixture Runtime");
        let mut runtime = Self {
            child: Some(child),
            stop_path,
            log_path,
        };
        runtime.wait_ready(root);
        runtime
    }

    fn wait_ready(&mut self, root: &Path) -> RuntimeInfo {
        let pid = self.child.as_ref().expect("runtime child").id();
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
                && info.pid() == pid
            {
                println!(
                    "RA|FIXTURE|runtime ready pid={pid} owner_epoch={}",
                    serde_json::to_string(&info.owner_epoch()).expect("epoch JSON")
                );
                return info;
            }
            if let Some(status) = self
                .child
                .as_mut()
                .expect("runtime child")
                .try_wait()
                .expect("runtime state")
            {
                panic!(
                    "Runtime exited before ready with {status}: {}",
                    fs::read_to_string(&self.log_path).unwrap_or_default()
                );
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "Runtime readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&mut self) {
        let mut child = self.child.take().expect("runtime child");
        child.kill().expect("kill fixture Runtime");
        let status = child.wait().expect("wait killed fixture Runtime");
        println!("RA|FIXTURE|runtime killed status={status}");
    }

    fn stop_clean(&mut self) {
        fs::write(&self.stop_path, b"stop").expect("write stop signal");
        let mut child = self.child.take().expect("runtime child");
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("runtime state") {
                assert!(
                    status.success(),
                    "Runtime clean stop failed: {}",
                    fs::read_to_string(&self.log_path).unwrap_or_default()
                );
                return;
            }
            if started.elapsed() >= Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Runtime clean stop timed out");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn connect(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect Runtime client")
}

fn mint_instance() -> InstanceId {
    *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport()
}

fn task_request(package: &Path, sha: &String) -> ContainedTaskRequest {
    ContainedTaskRequest::new(package.display().to_string(), sha).expect("contained task request")
}

macro_rules! to_json {
    ($value:expr) => {
        serde_json::to_string($value).expect("evidence JSON")
    };
}

fn print_status(label: &str, status: &ContainedRunStatus) {
    println!("RA|R1|{label}|{}", to_json!(status));
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn flatten(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                flatten(&format!("{prefix}.{key}"), child, out);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                flatten(&format!("{prefix}[{index}]"), child, out);
            }
        }
        leaf => out.push((prefix.to_owned(), leaf.clone())),
    }
}

fn compare_receipts(original: &RuntimeReceipt, split: &RuntimeReceipt) {
    let mut left = Vec::new();
    let mut right = Vec::new();
    flatten("receipt", &serde_json::to_value(original).expect("receipt"), &mut left);
    flatten("receipt", &serde_json::to_value(split).expect("receipt"), &mut right);
    let left_paths = left.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>();
    let right_paths = right.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>();
    println!(
        "RA|R2|RECEIPT_FIELDS|original={}|split={}|same_paths={}",
        left_paths.len(),
        right_paths.len(),
        left_paths == right_paths
    );
    assert_eq!(left_paths, right_paths, "receipt field sets differ");
    for ((path, original), (_, split)) in left.iter().zip(&right) {
        let identity = path
            .rsplit('.')
            .next()
            .is_some_and(|key| IDENTITY_KEYS.contains(&key));
        let equal = original == split;
        println!(
            "RA|R2|RECEIPT_FIELD|{path}|{}|original={original}|split={split}",
            if equal {
                "equal"
            } else if identity {
                "identity_differs"
            } else {
                "DIFFERS"
            }
        );
        assert!(equal || identity, "{path} differs outside identity fields");
    }
}

fn flow_event_types(output: &RuntimeFlowOutput) -> Vec<String> {
    output
        .events()
        .iter()
        .map(|event| to_json!(&event.event_type))
        .collect()
}

fn write_package(path: &Path) -> String {
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
                "resolution":{"width":16,"height":9},
                "entry_task_id":"task",
                "capture_interval_ms":1,
                "step_timeout_ms":50,
                "timeout_ms":30000,
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
                "coordinate_space":{"width":16,"height":9},
                "entry_page":"home",
                "target_page":"terminal",
                "operations":[{
                    "id":"open_terminal",
                    "from":"home",
                    "to":"terminal",
                    "click":{"kind":"point","x":1,"y":0},
                    "unguarded_trusted_coordinate":true
                }]
            }"#,
        ),
        (
            "resources/recognition/neutral.test.pack.json",
            br#"{
                "schema_version":"0.3",
                "game":"neutral",
                "server":"test",
                "coordinate_space":{"width":16,"height":9},
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

struct Fixture {
    root: TempDir,
    frame: PathBuf,
    package: PathBuf,
    sha: String,
    instance_id: InstanceId,
}

fn fixture() -> Fixture {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let sha = write_package(&package);
    Fixture {
        frame,
        package,
        sha,
        instance_id: mint_instance(),
        root,
    }
}

fn assert_manual_cli(status: &ContainedRunStatus) {
    assert_eq!(status.dispatch, RunDispatch::Manual);
    assert_eq!(status.origin, RunOrigin::Cli);
}

#[test]
fn oneoff_338ra_1_manual_success_and_prepare_submit() {
    let fixture = fixture();
    let root = fixture.root.path();
    let mut runtime = Runtime::spawn(root, fixture.instance_id, 0, "success");

    let original_client = connect(root);
    let original = original_client
        .run_contained_task(ALIAS, task_request(&fixture.package, &fixture.sha))
        .expect("original run_contained_task");
    println!("RA|R2|ORIGINAL_RECEIPT|{}", to_json!(original.receipt()));

    support::write_sealed_frame(&fixture.frame);
    thread::sleep(Duration::from_millis(500));
    let split_client = connect(root);
    let prepared = split_client
        .prepare_contained_task(ALIAS, task_request(&fixture.package, &fixture.sha))
        .expect("prepare_contained_task");
    let prepared_request_id = prepared.request_id();
    let prepared_correlation_id = prepared.correlation_id();
    println!(
        "RA|R2|PREPARED|request_id={}|correlation_id={}|holder={}|debug={prepared:?}",
        to_json!(&prepared_request_id),
        to_json!(&prepared_correlation_id),
        to_json!(&prepared.holder())
    );
    let before = connect(root)
        .contained_run_status(RunKey::RequestId(prepared_request_id), RunStatusMode::Full)
        .expect("status before submit");
    print_status("PREPARED_NOT_YET_SUBMITTED", &before);
    assert_eq!(before.state, ContainedRunState::NotFound);
    let split = split_client
        .submit_prepared(prepared)
        .expect("submit_prepared");
    println!("RA|R2|SPLIT_RECEIPT|{}", to_json!(split.receipt()));
    assert_eq!(split.receipt().request_id(), prepared_request_id);
    assert_eq!(split.receipt().correlation_id(), prepared_correlation_id);
    println!(
        "RA|R2|PREPARED_IDS_MATCH_RECEIPT|request_id={}|correlation_id={}",
        split.receipt().request_id() == prepared_request_id,
        split.receipt().correlation_id() == prepared_correlation_id
    );
    compare_receipts(original.receipt(), split.receipt());
    let original_types = flow_event_types(&original);
    let split_types = flow_event_types(&split);
    println!(
        "RA|R2|FLOW_EVENTS|original={}|split={}|same_types={}",
        original_types.len(),
        split_types.len(),
        original_types == split_types
    );
    println!("RA|R2|FLOW_EVENT_TYPES|original={}", original_types.join(","));
    println!("RA|R2|FLOW_EVENT_TYPES|split={}", split_types.join(","));

    let reader = connect(root);
    let mut run_id = None;
    for (label, request_id) in [
        ("original", original.receipt().request_id()),
        ("split", prepared_request_id),
    ] {
        for (mode_label, mode) in [("full", RunStatusMode::Full), ("brief", RunStatusMode::Brief)] {
            let status = reader
                .contained_run_status(RunKey::RequestId(request_id), mode)
                .expect("succeeded run status");
            print_status(&format!("SUCCEEDED|{label}|{mode_label}"), &status);
            assert_eq!(status.state, ContainedRunState::Succeeded);
            assert_manual_cli(&status);
            assert_eq!(status.instance_id, Some(fixture.instance_id));
            assert!(status.lease.is_some_and(|lease| lease.terminal.is_some()));
            assert_eq!(mode == RunStatusMode::Brief, status.progress.is_none());
            run_id = status.run_id;
        }
    }
    let by_run = reader
        .contained_run_status(RunKey::RunId(run_id.expect("run id")), RunStatusMode::Full)
        .expect("status by run id");
    print_status("SUCCEEDED|by_run_id|full", &by_run);
    assert_eq!(by_run.request_id, Some(prepared_request_id));

    let unknown: RequestId = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_request_id()
        .expect("request id")
        .transport();
    let missing = reader
        .contained_run_status(RunKey::RequestId(unknown), RunStatusMode::Brief)
        .expect("unknown request status");
    print_status("UNKNOWN_REQUEST|brief", &missing);
    assert_eq!(missing.state, ContainedRunState::NotFound);

    let recent = reader
        .recent_runs(fixture.instance_id, 0, 10)
        .expect("recent runs");
    println!("RA|R1|RECENT_RUNS|{}", to_json!(&recent));
    assert_eq!(recent.runs.len(), 2);
    assert!(!recent.incomplete);
    assert_eq!(recent.runs[0].request_id, Some(prepared_request_id));
    let one = reader
        .recent_runs(fixture.instance_id, 0, 1)
        .expect("recent runs limit 1");
    println!("RA|R1|RECENT_RUNS_LIMIT_1|{}", to_json!(&one));
    assert_eq!(one.runs.len(), 1);
    let refused = reader
        .recent_runs(fixture.instance_id, 0, 11)
        .expect_err("limit 11 refused");
    println!(
        "RA|R1|RECENT_RUNS_LIMIT_11|code={}|disposition={}",
        refused.code(),
        refused.disposition().as_str()
    );
    println!(
        "RA|FIXTURE|backend_events={}",
        support::backend_events(root).join(",")
    );
    drop((original_client, split_client, reader));
    runtime.stop_clean();
}

#[test]
fn oneoff_338ra_2_cancel_and_reset_from_another_connection() {
    let fixture = fixture();
    let root = fixture.root.path();
    let mut runtime = Runtime::spawn(root, fixture.instance_id, 4_000, "cancel");

    let mut submitter = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args([
            "task-run",
            "--state-root",
            root.to_str().expect("state root"),
            "--instance",
            ALIAS,
            "--package",
            fixture.package.to_str().expect("package path"),
            "--expected-sha256",
            &fixture.sha,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn actingctl task-run");
    wait_until(Duration::from_secs(15), || {
        support::backend_events(root)
            .iter()
            .any(|event| event == "tap_started")
    });
    let observer = connect(root);
    let recent = observer
        .recent_runs(fixture.instance_id, 0, 10)
        .expect("recent runs while running");
    println!("RA|R1|RECENT_RUNS_RUNNING|{}", to_json!(&recent));
    assert_eq!(recent.runs.len(), 1);
    assert_eq!(recent.runs[0].state, ContainedRunState::Running);
    let request_id = recent.runs[0].request_id.expect("running request id");
    let running = observer
        .contained_run_status(RunKey::RequestId(request_id), RunStatusMode::Full)
        .expect("running full status");
    print_status("RUNNING|full", &running);
    assert_eq!(running.state, ContainedRunState::Running);
    assert!(running.progress.is_some());
    assert_manual_cli(&running);

    submitter.kill().expect("kill actingctl submitter");
    let status = submitter.wait().expect("wait killed submitter");
    println!("RA|R2|SUBMITTER_KILLED|status={status}");

    let stopper = connect(root);
    let first = stopper
        .cancel_contained_task_and_reset(ALIAS, request_id, Duration::ZERO)
        .expect("cancel_contained_task_and_reset wait 0");
    println!(
        "RA|R2|CANCEL_AND_RESET|wait_ms=0|status={}|reset={:?}",
        to_json!(&first.status),
        first.reset
    );
    assert!(matches!(
        first.status,
        ContainedTaskCancellationStatus::Pending { .. }
    ));
    assert_eq!(first.reset, None);

    let started = Instant::now();
    let second = stopper
        .cancel_contained_task_and_reset(ALIAS, request_id, Duration::from_secs(20))
        .expect("cancel_contained_task_and_reset wait 20 s");
    println!(
        "RA|R2|CANCEL_AND_RESET|wait_ms=20000|elapsed_ms={}|status={}|reset={:?}",
        started.elapsed().as_millis(),
        to_json!(&second.status),
        second.reset
    );
    assert!(matches!(
        second.status,
        ContainedTaskCancellationStatus::Terminal {
            outcome: TaskOutcome::Cancelled,
            reason: Some(ContainedTaskCancellationReason::ClientRequested),
            ..
        }
    ));
    assert_eq!(second.reset, Some(ContainedTaskResetOutcome::Done));
    let events = support::backend_events(root);
    println!("RA|FIXTURE|backend_events={}", events.join(","));
    assert!(events.iter().any(|event| event == "reset"));

    let third = stopper
        .cancel_contained_task_and_reset(ALIAS, request_id, Duration::from_secs(5))
        .expect("cancel_contained_task_and_reset after terminal");
    println!(
        "RA|R2|CANCEL_AND_RESET|after_terminal|status={}|reset={:?}",
        to_json!(&third.status),
        third.reset
    );
    assert_eq!(third.reset, Some(ContainedTaskResetOutcome::NotNeeded));
    let once = stopper
        .cancel_contained_task(request_id)
        .expect("cancel_contained_task once");
    println!("RA|R2|CANCEL_ONCE|status={}", to_json!(&once));
    assert!(matches!(once, ContainedTaskCancellationStatus::Terminal { .. }));

    for (mode_label, mode) in [("full", RunStatusMode::Full), ("brief", RunStatusMode::Brief)] {
        let status = observer
            .contained_run_status(RunKey::RequestId(request_id), mode)
            .expect("cancelled run status");
        print_status(&format!("CANCELLED|{mode_label}"), &status);
        assert_eq!(status.state, ContainedRunState::Cancelled);
        assert_eq!(
            status
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.cancellation_reason),
            Some(ContainedTaskCancellationReason::ClientRequested)
        );
        assert_manual_cli(&status);
        let mut by_run = observer
            .contained_run_status(RunKey::RunId(status.run_id.expect("run id")), mode)
            .expect("cancelled status by run id");
        by_run.evidence.snapshot_ledger_position = status.evidence.snapshot_ledger_position;
        assert_eq!(by_run, status);
    }
    println!(
        "RA|R1|RECENT_RUNS_CANCELLED|{}",
        to_json!(
            &observer
                .recent_runs(fixture.instance_id, 0, 10)
                .expect("recent runs")
        )
    );
    drop((observer, stopper));
    runtime.stop_clean();
}

#[test]
fn oneoff_338ra_3_runtime_killed_mid_run() {
    let fixture = fixture();
    let root = fixture.root.path();
    let mut runtime = Runtime::spawn(root, fixture.instance_id, 4_000, "killed");

    let submitter = connect(root);
    let prepared = submitter
        .prepare_contained_task(ALIAS, task_request(&fixture.package, &fixture.sha))
        .expect("prepare_contained_task");
    let request_id = prepared.request_id();
    println!(
        "RA|R2|PREPARED|request_id={}|correlation_id={}",
        to_json!(&request_id),
        to_json!(&prepared.correlation_id())
    );
    let submission = thread::spawn(move || submitter.submit_prepared(prepared));
    wait_until(Duration::from_secs(15), || {
        support::backend_events(root)
            .iter()
            .any(|event| event == "tap_started")
    });
    let running = connect(root)
        .contained_run_status(RunKey::RequestId(request_id), RunStatusMode::Brief)
        .expect("running brief status");
    print_status("RUNNING|brief", &running);
    assert_eq!(running.state, ContainedRunState::Running);
    runtime.kill();
    let error = submission
        .join()
        .expect("submission thread")
        .expect_err("submission to a killed Runtime fails");
    println!(
        "RA|R6|KILLED_SUBMISSION|code={}|operation={}|disposition={}|{error}",
        error.code(),
        error.operation(),
        error.disposition().as_str()
    );

    let mut restarted = Runtime::spawn(root, fixture.instance_id, 0, "restarted");
    let reader = connect(root);
    for (mode_label, mode) in [("full", RunStatusMode::Full), ("brief", RunStatusMode::Brief)] {
        let status = reader
            .contained_run_status(RunKey::RequestId(request_id), mode)
            .expect("interrupted run status");
        print_status(&format!("INTERRUPTED|{mode_label}"), &status);
        assert_eq!(status.state, ContainedRunState::InterruptedUnterminated);
        assert!(status.evidence.restarted_after_admission);
        assert!(status.terminal.is_none());
        assert_manual_cli(&status);
    }
    println!(
        "RA|R1|RECENT_RUNS_INTERRUPTED|{}",
        to_json!(
            &reader
                .recent_runs(fixture.instance_id, 0, 10)
                .expect("recent runs")
        )
    );
    let error = reader
        .cancel_contained_task_and_reset(ALIAS, request_id, Duration::from_secs(2))
        .expect_err("an earlier epoch's open run needs recovery");
    println!(
        "RA|R2|CANCEL_AND_RESET|interrupted|code={}|operation={}|disposition={}",
        error.code(),
        error.operation(),
        error.disposition().as_str()
    );
    assert_eq!(error.code(), "runtime_contained_task_recovery_required");
    println!(
        "RA|FIXTURE|backend_events={}",
        support::backend_events(root).join(",")
    );
    drop(reader);
    restarted.stop_clean();
}
