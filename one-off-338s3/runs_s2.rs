// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 S2 evidence on the fixture runtime
// (tests/support/c4_runtime.rs) with the actingctl.exe of the product commit's exact-SHA build
// (ONEOFF_ACTINGCTL): ac_run_pack's immediate handle and ac_get_run to the terminal state, the
// observer-only tier refusal, the ledger provenance in one correlation, ac_stop_run from a fresh
// MCP process, and a Runtime killed mid-run (Rc is not merged: R1 reports the run). The
// workflow copies this file to apps/actingctl/tests/oneoff_338s2.rs.

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    EventActor, EventQuery, EventSource, IdentifierIssuer, InstanceId, ProjectionProfile,
    RUNTIME_INFO_FILE, RuntimeInfo,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const ALIAS: &str = "neutral.instance";

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

fn actingctl() -> PathBuf {
    std::env::var_os("ONEOFF_ACTINGCTL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_actingctl")))
}

struct Runtime {
    child: Option<Child>,
    stop_path: PathBuf,
}

impl Runtime {
    fn spawn(root: &Path, instance_id: InstanceId, input_delay_ms: u64, label: &str) -> Self {
        let stop_path = root.join(format!("stop-runtime-{label}"));
        let log = File::create(root.join(format!("runtime-{label}.log"))).expect("runtime log");
        let child = Command::new(std::env::current_exe().expect("current test executable"))
            .args(["--exact", "c4_runtime_child_process", "--nocapture", "--test-threads=1"])
            .env("ACTINGCOMMAND_C4_TEST_CHILD", "1")
            .env("ACTINGCOMMAND_C4_TEST_ROOT", root)
            .env(
                "ACTINGCOMMAND_C4_TEST_INSTANCE",
                serde_json::to_string(&instance_id).expect("instance JSON"),
            )
            .env("ACTINGCOMMAND_C4_TEST_INSTANCE_ALIAS", ALIAS)
            .env("ACTINGCOMMAND_C4_TEST_STOP", &stop_path)
            .env("ACTINGCOMMAND_C4_TEST_INPUT_DELAY_MS", input_delay_ms.to_string())
            .stdout(Stdio::from(log.try_clone().expect("log clone")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn fixture Runtime");
        let pid = child.id();
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
                && info.pid() == pid
            {
                println!(
                    "S2|FIXTURE|runtime {label} ready owner_epoch={}",
                    serde_json::to_string(&info.owner_epoch()).expect("epoch")
                );
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(20), "runtime {label} not ready");
            thread::sleep(Duration::from_millis(10));
        }
        Self {
            child: Some(child),
            stop_path,
        }
    }

    fn kill(&mut self) {
        let mut child = self.child.take().expect("runtime child");
        child.kill().expect("kill runtime");
        let status = child.wait().expect("wait runtime");
        println!("S2|FIXTURE|runtime killed {status}");
    }

    fn stop_clean(&mut self) {
        fs::write(&self.stop_path, b"stop").expect("stop signal");
        let mut child = self.child.take().expect("runtime child");
        let started = Instant::now();
        while child.try_wait().expect("runtime state").is_none() {
            assert!(started.elapsed() < Duration::from_secs(10), "clean stop timed out");
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

struct Server {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    fn spawn(label: &str, args: &[&str]) -> Self {
        let mut child = Command::new(actingctl())
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mcp-serve");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut server = Self {
            label: label.to_owned(),
            child,
            stdin,
            lines,
            next_id: 1,
        };
        let answer = server.request(json!({
            "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "oneoff", "version": "0"}},
        }));
        assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn request(&mut self, mut message: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        message["jsonrpc"] = json!("2.0");
        message["id"] = json!(id);
        self.send(&message);
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("an answer");
            let answer: Value = serde_json::from_str(&line).expect("JSON");
            if answer.get("id") == Some(&json!(id)) {
                return answer;
            }
        }
    }

    fn tool(&mut self, name: &str, arguments: Value) -> (Value, Duration) {
        let started = Instant::now();
        let answer = self.request(json!({
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        let elapsed = started.elapsed();
        let structured = answer["result"]["structuredContent"].clone();
        println!(
            "S2|{}|{name} {arguments} -> {} ms: {}",
            self.label,
            elapsed.as_millis(),
            clip(&structured.to_string())
        );
        (structured, elapsed)
    }

    fn kill(mut self) {
        self.child.kill().expect("kill mcp-serve");
        let _ = self.child.wait();
        println!("S2|{}|mcp-serve process killed", self.label);
    }

    fn close(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("mcp-serve exit");
        assert!(status.success());
    }
}

fn clip(text: &str) -> String {
    const MAX: usize = 1200;
    if text.len() <= MAX {
        return text.to_owned();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} ...(+{} bytes)", &text[..end], text.len() - end)
}

fn ledger(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(root, EventActor::Cli, EventSource::Cli))
        .expect("ledger client")
}

/// The event types under one correlation, with each client.action's surface and control.
fn correlation_events(root: &Path, correlation: &str) -> Vec<String> {
    let query = EventQuery {
        correlation_id: Some(serde_json::from_value(json!(correlation)).expect("correlation")),
        ..EventQuery::default()
    };
    ledger(root)
        .query_events(query, ProjectionProfile::Forensic)
        .expect("correlation events")
        .iter()
        .map(|event| {
            let row = serde_json::to_value(event).expect("event JSON");
            let kind = row["event_type"].as_str().unwrap_or_default().to_owned();
            if kind == "client.action" {
                let text = row["payload"].to_string();
                let surface = text.contains("\"surface_id\":\"mcp\"");
                let control = ["ac_run_pack", "ac_stop_run"]
                    .into_iter()
                    .find(|control| text.contains(&format!("\"control_id\":\"{control}\"")));
                format!("client.action(surface_mcp={surface},control={control:?})")
            } else {
                kind
            }
        })
        .collect()
}

fn wait_terminal(server: &mut Server, handle: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let (run, _) = server.tool("ac_get_run", json!({"handle": handle, "wait_s": 20}));
        let state = run["result"]["state"].as_str().unwrap_or_default().to_owned();
        if !matches!(state.as_str(), "not_found" | "admitted" | "running")
            || run["result"]["job"]["phase"] == "failed"
        {
            return run;
        }
        assert!(Instant::now() < deadline, "run {handle} did not end");
    }
}

#[test]
fn oneoff_338s2_runs() {
    let root = TempDir::new().expect("tempdir");
    let state_root = root.path().to_str().expect("utf-8").to_owned();
    support::write_sealed_frame(&root.path().join("sealed.png"));
    let package = root.path().join("neutral-task.zip");
    let sha = write_neutral_contained_task_package(&package);
    let package_path = package.to_str().expect("utf-8").to_owned();
    let instance_id = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance")
        .transport();
    let mut runtime = Runtime::spawn(root.path(), instance_id, 0, "a");

    // Observer only: a write tool answers tier_not_enabled.
    let mut observer = Server::spawn("observer", &["mcp-serve", "--state-root", &state_root]);
    let (refused, _) = observer.tool(
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package_path, "package_ref": sha}),
    );
    assert_eq!(refused["error"]["code"], "tier_not_enabled");
    observer.close();

    // ac_run_pack answers at once with the handle; ac_get_run follows it to the end.
    let mut operator = Server::spawn(
        "operator",
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let (submitted, elapsed) = operator.tool(
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package_path, "package_ref": sha, "deadline_s": 120}),
    );
    assert_eq!(submitted["result"]["phase"], "submitting");
    println!("S2|RUN_PACK|answered in {} ms", elapsed.as_millis());
    let handle = submitted["result"]["handle"].as_str().expect("handle").to_owned();
    let correlation = submitted["result"]["correlation_id"].as_str().expect("correlation").to_owned();
    let run = wait_terminal(&mut operator, &handle);
    assert_eq!(run["result"]["state"], "succeeded");
    assert_eq!(run["result"]["job"]["phase"], "done");
    let events = correlation_events(root.path(), &correlation);
    println!("S2|PROVENANCE|correlation {correlation}: {events:?}");
    assert!(events.iter().any(|event| event == "governance.identity_declared"));
    assert!(events.iter().any(|event| event.starts_with("client.action(surface_mcp=true")));
    assert!(events.iter().any(|event| event.starts_with("task.")));
    operator.close();
    runtime.stop_clean();

    // ac_stop_run from a fresh MCP process, after the submitting process was killed.
    support::write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = Runtime::spawn(root.path(), instance_id, 4000, "b");
    let mut submitter = Server::spawn(
        "submitter",
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let (submitted, _) = submitter.tool(
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package_path, "package_ref": sha, "deadline_s": 120}),
    );
    let handle = submitted["result"]["handle"].as_str().expect("handle").to_owned();
    thread::sleep(Duration::from_millis(1500));
    submitter.kill();
    let mut stopper = Server::spawn(
        "stopper",
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let (stopped, _) = stopper.tool("ac_stop_run", json!({"handle": handle, "wait_s": 25}));
    println!("S2|STOP|fresh process -> {stopped}");
    assert_eq!(stopped["ok"], true);
    let run = wait_terminal(&mut stopper, &handle);
    println!("S2|STOP|run after stop: state {} terminal {}", run["result"]["state"], run["result"]["terminal"]);
    stopper.close();
    runtime.stop_clean();

    // The Runtime killed mid-run and started again; Rc is not merged, so R1 reports the run.
    support::write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = Runtime::spawn(root.path(), instance_id, 4000, "c");
    let mut server = Server::spawn(
        "restart",
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let (submitted, _) = server.tool(
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package_path, "package_ref": sha, "deadline_s": 120}),
    );
    let handle = submitted["result"]["handle"].as_str().expect("handle").to_owned();
    thread::sleep(Duration::from_millis(1500));
    runtime.kill();
    let _runtime = Runtime::spawn(root.path(), instance_id, 0, "d");
    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        let (run, _) = server.tool("ac_get_run", json!({"handle": handle, "wait_s": 5}));
        if run["result"]["job"]["phase"] != "submitting" {
            break run;
        }
        assert!(Instant::now() < deadline, "the submit job did not end");
    };
    println!(
        "S2|RESTART|job {} ; run state {}",
        run["result"]["job"], run["result"]["state"]
    );
    assert_eq!(run["result"]["job"]["phase"], "failed");
    assert_eq!(run["result"]["job"]["outcome"]["error"]["class"], "uncertain");
    assert_eq!(run["result"]["state"], "interrupted_unterminated");
    server.close();
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
    fs::write(path, &bytes).expect("write package");
    Sha256Hash::digest(&bytes).to_string()
}

/// Review fix F1: four ac_get_run waits on a pause job hold all four workers; cancelling them
/// frees the workers, so an ac_overview sent right after is answered at once. With
/// ONEOFF_EXPECT_FREED unset (the build before the fix) the time is only printed.
#[test]
fn oneoff_338s2_cancelled_waits_free_workers() {
    let label = std::env::var("ONEOFF_LABEL").unwrap_or_else(|_| "build".to_owned());
    let expect_freed = std::env::var_os("ONEOFF_EXPECT_FREED").is_some();
    let root = TempDir::new().expect("tempdir");
    let state_root = root.path().to_str().expect("utf-8").to_owned();
    support::write_sealed_frame(&root.path().join("sealed.png"));
    let package = root.path().join("neutral-task.zip");
    let sha = write_neutral_contained_task_package(&package);
    let package_path = package.to_str().expect("utf-8").to_owned();
    let instance_id = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance")
        .transport();
    let _runtime = Runtime::spawn(root.path(), instance_id, 90_000, "wait");
    let mut server = Server::spawn(
        &format!("cancel-{label}"),
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let (submitted, _) = server.tool(
        "ac_run_pack",
        json!({"instance": ALIAS, "package": package_path, "package_ref": sha, "deadline_s": 300}),
    );
    assert_eq!(submitted["result"]["phase"], "submitting");
    thread::sleep(Duration::from_millis(1500));
    // The instance pause drains the in-flight run, so its job outlives the call.
    let (paused, _) = server.tool("ac_pause", json!({"instance": ALIAS, "drain_timeout_s": 180}));
    let pause_handle = paused["result"]["handle"]
        .as_str()
        .expect("the pause is still draining, so the call answers with its handle")
        .to_owned();
    let first = server.next_id;
    for offset in 0..4 {
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": first + offset,
            "method": "tools/call",
            "params": {"name": "ac_get_run", "arguments": {"handle": pause_handle, "wait_s": 25}},
        }));
    }
    server.next_id += 4;
    thread::sleep(Duration::from_millis(1000));
    for offset in 0..4 {
        server.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": first + offset, "reason": "one-off"},
        }));
    }
    let overview_id = server.next_id;
    server.next_id += 1;
    let started = Instant::now();
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": overview_id,
        "method": "tools/call",
        "params": {"name": "ac_overview", "arguments": {}},
    }));
    let mut other_answers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let line = server
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("an answer to ac_overview");
        let answer: Value = serde_json::from_str(&line).expect("JSON");
        if answer.get("id") == Some(&json!(overview_id)) {
            assert_eq!(answer["result"]["structuredContent"]["ok"], true);
            break;
        }
        if answer.get("id").is_some() {
            other_answers.push(answer["id"].clone());
        }
    }
    let elapsed = started.elapsed();
    println!(
        "S2C|{label}|ac_overview after cancelling four ac_get_run waits answered in {} ms",
        elapsed.as_millis()
    );
    // Cancelled calls are never answered.
    let quiet_until = Instant::now() + Duration::from_secs(3);
    while let Ok(line) = server
        .lines
        .recv_timeout(quiet_until.saturating_duration_since(Instant::now()))
    {
        let answer: Value = serde_json::from_str(&line).expect("JSON");
        if answer.get("id").is_some() {
            other_answers.push(answer["id"].clone());
        }
    }
    println!("S2C|{label}|answers to the cancelled calls: {other_answers:?}");
    assert!(other_answers.is_empty());
    if expect_freed {
        assert!(elapsed < Duration::from_millis(2500), "the workers were not freed");
    }
    server.kill();
}
