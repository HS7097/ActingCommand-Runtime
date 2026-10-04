// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 Rc evidence on the fixture runtime
// (tests/support/c4_runtime.rs): a direct run whose Runtime child is killed mid-run, the start
// settlement, the resubmission of the original request, a further restart, and a run two epochs
// back. The one-off workflow copies this file to apps/actingctl/tests/oneoff_338rc.rs; nothing in
// the repository's product files changes. Every printed line starts with `RC|`.

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    ContainedTaskRequest, EventActor, EventPayload, EventQuery, EventSource, EventType,
    IdentifierIssuer, InstanceId, ProjectedEvent, ProjectionPayload, ProjectionProfile,
    RUNTIME_INFO_FILE, RequestId, RuntimeInfo, RuntimeOperation, RuntimeReceipt, RuntimeRequest,
    TaskPayload, TaskSemanticFact,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{
    ContainedRunState, RunKey, RunStatusMode, RuntimeClient, RuntimeClientConfig,
};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const ALIAS: &str = "neutral.instance";
const OTHER_ALIAS: &str = "other.instance";

macro_rules! to_json {
    ($value:expr) => {
        serde_json::to_string($value).expect("evidence JSON")
    };
}

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

struct Runtime {
    child: Option<Child>,
    stop_path: PathBuf,
    log_path: PathBuf,
    info: Option<RuntimeInfo>,
}

impl Runtime {
    fn spawn(
        root: &Path,
        alias: &str,
        instance_id: InstanceId,
        input_delay_ms: u64,
        label: &str,
    ) -> Self {
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
            .env("ACTINGCOMMAND_C4_TEST_INSTANCE_ALIAS", alias)
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
            info: None,
        };
        runtime.wait_ready(root, label);
        runtime
    }

    fn wait_ready(&mut self, root: &Path, label: &str) {
        let pid = self.child.as_ref().expect("runtime child").id();
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
                && info.pid() == pid
            {
                println!(
                    "RC|FIXTURE|{label}|runtime ready pid={pid} owner_epoch={}",
                    to_json!(&info.owner_epoch())
                );
                self.info = Some(info);
                return;
            }
            if let Some(status) = self
                .child
                .as_mut()
                .expect("runtime child")
                .try_wait()
                .expect("runtime state")
            {
                panic!(
                    "Runtime {label} exited before ready with {status}: {}",
                    fs::read_to_string(&self.log_path).unwrap_or_default()
                );
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "Runtime {label} readiness timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn info(&self) -> &RuntimeInfo {
        self.info.as_ref().expect("runtime info")
    }

    fn kill(&mut self) {
        let mut child = self.child.take().expect("runtime child");
        child.kill().expect("kill fixture Runtime");
        let status = child.wait().expect("wait killed fixture Runtime");
        println!("RC|FIXTURE|runtime killed status={status}");
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
                break;
            }
            if started.elapsed() >= Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Runtime clean stop timed out");
            }
            thread::sleep(Duration::from_millis(10));
        }
        // The stop signal must not stop the next runtime of this root.
        let moved = self.stop_path.with_extension(format!(
            "used-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::rename(&self.stop_path, moved).expect("retire stop signal");
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

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let started = Instant::now();
    while !predicate() {
        assert!(started.elapsed() < timeout, "condition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn raw_exchange(info: &RuntimeInfo, request: &RuntimeRequest) -> std::io::Result<RuntimeReceipt> {
    let mut stream = TcpStream::connect(info.socket_addr().expect("Runtime socket"))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let body = serde_json::to_vec(request).expect("serialize Runtime request");
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)?;
    stream.flush()?;
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    let receipt = serde_json::from_slice::<RuntimeReceipt>(&body).expect("decode Runtime receipt");
    receipt.validate().expect("validate Runtime receipt");
    Ok(receipt)
}

fn original_request(alias: &str, package: &Path, sha: &String) -> RuntimeRequest {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Cli,
        EventSource::Cli,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis() as u64,
        RuntimeOperation::run_contained_task(
            alias,
            ids.mint_holder_id().expect("holder id"),
            ContainedTaskRequest::new(package.display().to_string(), sha)
                .expect("contained task request"),
        ),
    )
    .expect("Runtime request")
}

fn request_events(client: &RuntimeClient, request_id: RequestId) -> Vec<ProjectedEvent> {
    client
        .query_events(
            EventQuery {
                request_id: Some(request_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("request events")
}

fn lease_events(client: &RuntimeClient, lease_id: actingcommand_contract::LeaseId) -> Vec<ProjectedEvent> {
    client
        .query_events(
            EventQuery {
                lease_id: Some(lease_id),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("lease events")
}

/// One line per task terminal, terminal intent and lease terminal of the request.
fn print_settlement(label: &str, client: &RuntimeClient, request_id: RequestId) -> (usize, usize) {
    let events = request_events(client, request_id);
    let mut terminals = 0;
    for event in &events {
        if let ProjectionPayload::Full(payload) = &event.payload
            && let EventPayload::Task(TaskPayload::Semantic(semantic)) = payload.as_ref()
            && matches!(
                semantic.fact(),
                TaskSemanticFact::Finalizing { .. } | TaskSemanticFact::TerminalCommitted { .. }
            )
        {
            if matches!(semantic.fact(), TaskSemanticFact::TerminalCommitted { .. }) {
                terminals += 1;
            }
            println!(
                "RC|LEDGER|{label}|seq={}|type={}|origin={}/{}|fact={}|links={}",
                event.sequence,
                to_json!(&event.event_type),
                to_json!(&event.origin.source()),
                to_json!(&event.origin.actor()),
                to_json!(semantic.fact()),
                to_json!(&event.links)
            );
        }
    }
    let lease_id = events
        .iter()
        .find_map(|event| event.links.lease_id().copied())
        .expect("run lease");
    let lease_terminals = lease_events(client, lease_id)
        .into_iter()
        .filter(|event| {
            matches!(
                event.event_type,
                EventType::LeaseReleased | EventType::LeaseExpired
            )
        })
        .collect::<Vec<_>>();
    for event in &lease_terminals {
        println!(
            "RC|LEDGER|{label}|seq={}|type={}|origin={}/{}|links={}",
            event.sequence,
            to_json!(&event.event_type),
            to_json!(&event.origin.source()),
            to_json!(&event.origin.actor()),
            to_json!(&event.links)
        );
    }
    println!(
        "RC|LEDGER|{label}|request_events={}|task_terminals={terminals}|lease_terminals={}",
        events.len(),
        lease_terminals.len()
    );
    (terminals, lease_terminals.len())
}

fn status(label: &str, client: &RuntimeClient, request_id: RequestId) -> ContainedRunState {
    let mut state = None;
    for (mode_label, mode) in [("full", RunStatusMode::Full), ("brief", RunStatusMode::Brief)] {
        let status = client
            .contained_run_status(RunKey::RequestId(request_id), mode)
            .expect("run status");
        println!("RC|R1|{label}|{mode_label}|{}", to_json!(&status));
        state = Some(status.state);
    }
    state.expect("run state")
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

/// Starts the original request on a thread and kills the Runtime once its input began.
fn run_and_kill(
    root: &Path,
    runtime: &mut Runtime,
    request: &RuntimeRequest,
) -> thread::JoinHandle<std::io::Result<RuntimeReceipt>> {
    let info = runtime.info().clone();
    let sent = request.clone();
    let submission = thread::spawn(move || raw_exchange(&info, &sent));
    wait_until(Duration::from_secs(15), || {
        support::backend_events(root)
            .iter()
            .filter(|event| *event == "tap_started")
            .count()
            > 0
    });
    runtime.kill();
    submission
}

#[test]
fn oneoff_338rc_previous_epoch_direct_run_is_settled_once() {
    let root = TempDir::new().expect("tempdir");
    let root = root.path();
    let frame = root.join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.join("neutral-task.zip");
    let sha = write_package(&package);
    let instance_id = mint_instance();

    let mut first = Runtime::spawn(root, ALIAS, instance_id, 4_000, "epoch1");
    let request = original_request(ALIAS, &package, &sha);
    let request_id = request.request_id();
    println!("RC|FIXTURE|original request_id={}", to_json!(&request_id));
    let submission = run_and_kill(root, &mut first, &request);
    match submission.join().expect("submission thread") {
        Ok(receipt) => panic!("killed run answered: {}", to_json!(&receipt)),
        Err(error) => println!("RC|FIXTURE|original submission failed: {error}"),
    }

    let mut second = Runtime::spawn(root, ALIAS, instance_id, 0, "epoch2");
    let client = connect(root);
    let (terminals, lease_terminals) = print_settlement("after_first_restart", &client, request_id);
    assert_eq!((terminals, lease_terminals), (1, 1));
    let state = status("after_first_restart", &client, request_id);
    assert_eq!(state, ContainedRunState::Cancelled);

    let replay = raw_exchange(second.info(), &request).expect("resubmission receipt");
    println!("RC|RESUBMIT|{}", to_json!(&replay));
    let (terminals, lease_terminals) = print_settlement("after_resubmission", &client, request_id);
    assert_eq!((terminals, lease_terminals), (1, 1));
    let replay_again = raw_exchange(second.info(), &request).expect("second resubmission receipt");
    println!(
        "RC|RESUBMIT|second_equals_first={}",
        replay_again == replay
    );
    assert_eq!(replay_again, replay);
    drop(client);
    second.stop_clean();

    let mut third = Runtime::spawn(root, ALIAS, instance_id, 0, "epoch3");
    let client = connect(root);
    let (terminals, lease_terminals) = print_settlement("after_second_restart", &client, request_id);
    assert_eq!((terminals, lease_terminals), (1, 1));
    let state = status("after_second_restart", &client, request_id);
    assert_eq!(state, ContainedRunState::Cancelled);
    drop(client);
    third.stop_clean();
}

#[test]
fn oneoff_338rc_run_two_epochs_back_stays_open() {
    let root = TempDir::new().expect("tempdir");
    let root = root.path();
    let frame = root.join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.join("neutral-task.zip");
    let sha = write_package(&package);
    let instance_id = mint_instance();

    let mut first = Runtime::spawn(root, ALIAS, instance_id, 4_000, "epoch1");
    let request = original_request(ALIAS, &package, &sha);
    let request_id = request.request_id();
    println!("RC|TWO_EPOCHS|original request_id={}", to_json!(&request_id));
    let submission = run_and_kill(root, &mut first, &request);
    let _ = submission.join().expect("submission thread");

    // Epoch 2 registers another instance only: the run's instance is not registered, so this
    // start leaves the run open. Epoch 3 registers it again; the run is now two epochs back.
    let mut second = Runtime::spawn(root, OTHER_ALIAS, mint_instance(), 0, "epoch2-other");
    let client = connect(root);
    let (terminals, _) = print_settlement("two_epochs|after_epoch2", &client, request_id);
    assert_eq!(terminals, 0);
    let state = status("two_epochs|after_epoch2", &client, request_id);
    println!("RC|TWO_EPOCHS|after_epoch2|state={}", to_json!(&state));
    drop(client);
    second.stop_clean();

    let mut third = Runtime::spawn(root, ALIAS, instance_id, 0, "epoch3");
    let client = connect(root);
    let (terminals, _) = print_settlement("two_epochs|after_epoch3", &client, request_id);
    assert_eq!(terminals, 0);
    let state = status("two_epochs|after_epoch3", &client, request_id);
    assert_eq!(state, ContainedRunState::InterruptedUnterminated);
    drop(client);
    third.stop_clean();
}
