// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 S1 review fixes F1/F2/F3 on the fixture runtime
// (tests/support/c4_runtime.rs), driving the actingctl.exe of the product commit's exact-SHA
// build (ONEOFF_ACTINGCTL) with a stand-in actingd (ONEOFF_FAKE_ACTINGD) that prints a chosen
// `suspended` report. The workflow copies this file to apps/actingctl/tests/oneoff_338s1f.rs.

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{IdentifierIssuer, InstanceId, RUNTIME_INFO_FILE, RuntimeInfo};
use actingcommand_pack_containment::Sha256Hash;
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
const PAYLOAD_LIMIT: usize = 11 * 1024;

#[test]
fn c4_runtime_child_process() {
    support::run_child_if_requested();
}

fn actingctl() -> PathBuf {
    std::env::var_os("ONEOFF_ACTINGCTL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_actingctl")))
}

/// A fixture Runtime with its own stop file, so a second one can start on the same root.
struct Runtime {
    child: Option<Child>,
    stop_path: PathBuf,
    log_path: PathBuf,
}

impl Runtime {
    fn spawn(root: &Path, instance_id: InstanceId, label: &str) -> Self {
        let stop_path = root.join(format!("stop-runtime-{label}"));
        let log_path = root.join(format!("runtime-{label}.log"));
        let log = File::create(&log_path).expect("runtime log");
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
            .env("ACTINGCOMMAND_C4_TEST_INPUT_DELAY_MS", "0")
            .stdout(Stdio::from(log.try_clone().expect("log clone")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn fixture Runtime");
        let pid = child.id();
        let runtime = Self {
            child: Some(child),
            stop_path,
            log_path,
        };
        let started = Instant::now();
        loop {
            if let Ok(encoded) = fs::read(root.join(RUNTIME_INFO_FILE))
                && let Ok(info) = serde_json::from_slice::<RuntimeInfo>(&encoded)
                && info.validate().is_ok()
                && info.pid() == pid
            {
                println!(
                    "S1F|FIXTURE|runtime {label} ready pid={pid} owner_epoch={}",
                    serde_json::to_string(&info.owner_epoch()).expect("epoch JSON")
                );
                return runtime;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "Runtime {label} readiness timed out: {}",
                fs::read_to_string(&runtime.log_path).unwrap_or_default()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop_clean(&mut self) {
        fs::write(&self.stop_path, b"stop").expect("write stop signal");
        let mut child = self.child.take().expect("runtime child");
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait().expect("runtime state") {
                println!("S1F|FIXTURE|runtime stopped {status}");
                assert!(status.success());
                return;
            }
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
    fn spawn(label: &str, args: &[&str], report: &Path) -> Self {
        let mut child = Command::new(actingctl())
            .args(args)
            .env("ONEOFF_FAKE_REPORT", report)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn actingctl mcp-serve");
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
        let initialized = server.request(json!({
            "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "oneoff", "version": "0"}},
        }));
        assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");
        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write stdin");
        stdin.flush().expect("flush stdin");
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
            let answer: Value = serde_json::from_str(&line).expect("JSON line");
            if answer.get("id") == Some(&json!(id)) {
                return answer;
            }
        }
    }

    /// The envelope of one tool call, printed.
    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let answer = self.request(json!({
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        let structured = answer["result"]["structuredContent"].clone();
        let text_bytes = answer["result"]["content"][0]["text"]
            .as_str()
            .map_or(0, str::len);
        println!(
            "S1F|{}|{name} {arguments} -> payload {text_bytes} bytes, whole result {} bytes: {}",
            self.label,
            answer["result"].to_string().len(),
            clip(&structured.to_string())
        );
        structured
    }

    fn close(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("mcp-serve exit");
        assert!(status.success());
    }
}

fn clip(text: &str) -> String {
    const MAX: usize = 900;
    if text.len() <= MAX {
        return text.to_owned();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} ...(+{} bytes)", &text[..end], text.len() - end)
}

fn task_run(state_root: &str, package: &Path, sha: &str) -> Value {
    let output = Command::new(actingctl())
        .args([
            "task-run",
            "--state-root",
            state_root,
            "--instance",
            ALIAS,
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            sha,
        ])
        .output()
        .expect("actingctl task-run");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    println!(
        "S1F|FIXTURE|task-run exit {} -> {} {}",
        output.status,
        value["receipt"]["state"],
        value["receipt"]["result"]["kind"]
    );
    value
}

/// A `suspended` report for ALIAS whose nine rows take about `rows_bytes` together.
fn write_report(path: &Path, state_root: &Path, rows_bytes: usize) {
    let row_with = |kind: &str, index: usize, detail_len: usize| {
        json!({
            "task_id": format!("oneoff.{kind}.{index}"),
            "instance_id": ALIAS,
            "detail": "x".repeat(detail_len),
        })
    };
    // Each row plus its separating comma takes rows_bytes / 9.
    let empty_row = row_with("suspended", 0, 0).to_string().len() + 1;
    let detail_len = (rows_bytes / 9).saturating_sub(empty_row);
    let row = |kind: &str, index: usize| row_with(kind, index, detail_len);
    let report = json!({
        "schema_version": "actingcommand.actingd.suspended.v1",
        "status": "ok",
        "config_path": "oneoff",
        "state_root": state_root.display().to_string(),
        "through_sequence": 1,
        "daemon_started_at_unix_ms": null,
        "suspended": (0..3).map(|index| row("suspended", index)).collect::<Vec<_>>(),
        "lifted": (0..3).map(|index| row("lift", index)).collect::<Vec<_>>(),
        "repeating": (0..3).map(|index| row("repeating", index)).collect::<Vec<_>>(),
        "warnings": ["oneoff report warning"],
    });
    fs::write(path, report.to_string()).expect("write the fake report");
}

#[test]
fn oneoff_338s1f_review_fixes() {
    let root = TempDir::new().expect("tempdir");
    let state_root_path = root.path().to_path_buf();
    let state_root = state_root_path.to_str().expect("utf-8").to_owned();
    support::write_sealed_frame(&root.path().join("sealed.png"));
    let package = root.path().join("neutral-task.zip");
    let sha = write_neutral_contained_task_package(&package);
    let instance_id = *IdentifierIssuer::new()
        .expect("issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    // An install root whose runtime\actingcommand-actingd.exe is the stand-in.
    let install = TempDir::new().expect("tempdir");
    fs::create_dir_all(install.path().join("runtime")).expect("runtime dir");
    fs::copy(
        std::env::var_os("ONEOFF_FAKE_ACTINGD").expect("ONEOFF_FAKE_ACTINGD"),
        install.path().join("runtime").join("actingcommand-actingd.exe"),
    )
    .expect("copy the stand-in actingd");
    fs::write(
        install.path().join("actingd.config.json"),
        json!({"state_root": state_root}).to_string(),
    )
    .expect("config");
    let install_root = install.path().to_str().expect("utf-8").to_owned();
    let report = root.path().join("fake-report.json");
    write_report(&report, &state_root_path, 0);

    let mut runtime = Runtime::spawn(root.path(), instance_id, "first");
    task_run(&state_root, &package, &sha);
    // The frame now shows the terminal page, so these two runs cannot start from home.
    task_run(&state_root, &package, &sha);
    task_run(&state_root, &package, &sha);

    let mut server = Server::spawn(
        "fixes",
        &["mcp-serve", "--root", &install_root, "--state-root", &state_root],
        &report,
    );

    // (a) F1: restart between two ac_overview calls; an old cursor; then actingd stopped.
    let first = server.tool("ac_overview", json!({}));
    let first_epoch = first["result"]["daemon"]["owner_epoch"].clone();
    assert_eq!(first["result"]["daemon"]["online"], true);
    let page = server.tool("ac_events", json!({"limit": 2}));
    let cursor = page["result"]["next_cursor"].as_str().expect("next_cursor").to_owned();
    runtime.stop_clean();
    let mut runtime = Runtime::spawn(root.path(), instance_id, "second");
    let second = server.tool("ac_overview", json!({}));
    println!(
        "S1F|F1|restart: owner_epoch {first_epoch} -> {}; ok {}",
        second["result"]["daemon"]["owner_epoch"], second["ok"]
    );
    assert_eq!(second["ok"], true);
    assert_eq!(second["result"]["daemon"]["online"], true);
    assert_ne!(second["result"]["daemon"]["owner_epoch"], first_epoch);
    let stale = server.tool("ac_events", json!({"limit": 2, "cursor": cursor}));
    assert_eq!(stale["error"]["code"], "cursor_invalid");

    // (b) F2: report rows that fit alone, with runs that do not fit beside them.
    write_report(&report, &state_root_path, 0);
    let measured = server.tool("ac_diagnose", json!({"instance": ALIAS}));
    assert_eq!(measured["ok"], true);
    let runs = measured["result"]["runs"].as_array().cloned().unwrap_or_default();
    let run_bytes = runs.iter().map(|run| run.to_string().len()).collect::<Vec<_>>();
    let payload = json!({"ok": true, "result": measured["result"]}).to_string().len();
    let errors_bytes = measured["result"]["errors_page"]["events"].to_string().len();
    let base = payload - run_bytes.iter().sum::<usize>() - errors_bytes;
    println!("S1F|F2|measured: payload {payload}, base {base}, runs {run_bytes:?}, errors page {errors_bytes}");
    if run_bytes.is_empty() {
        println!("S1F|F2|no failed or open runs in the window: the run cut is not exercised");
    } else {
        let rows_bytes = PAYLOAD_LIMIT - base - 400;
        write_report(&report, &state_root_path, rows_bytes);
        let cut = server.tool("ac_diagnose", json!({"instance": ALIAS}));
        let kept_runs = cut["result"]["runs"].as_array().map_or(0, Vec::len);
        println!(
            "S1F|F2|over budget: runs {} -> {kept_runs}, runs_truncated {}, errors kept {}, errors truncated {}, suspended {} truncated {}, lift {} truncated {}, repeating {} truncated {}, report_warnings {}",
            runs.len(),
            cut["result"]["runs_truncated"],
            cut["result"]["errors_page"]["events"].as_array().map_or(0, Vec::len),
            cut["result"]["errors_page"]["truncated"],
            cut["result"]["suspended"].as_array().map_or(0, Vec::len),
            cut["result"]["suspended_truncated"],
            cut["result"]["lift"].as_array().map_or(0, Vec::len),
            cut["result"]["lift_truncated"],
            cut["result"]["repeating"].as_array().map_or(0, Vec::len),
            cut["result"]["repeating_truncated"],
            cut["result"]["report_warnings"],
        );
        assert_eq!(cut["ok"], true);
        assert_eq!(cut["result"]["runs_truncated"], true);
        assert!(kept_runs < runs.len());
        for list in ["suspended", "lift", "repeating"] {
            assert_eq!(cut["result"][list].as_array().map_or(0, Vec::len), 3, "{list}");
            assert!(cut["result"].get(format!("{list}_truncated")).is_none(), "{list}");
        }
        assert_eq!(cut["result"]["report_warnings"], json!(["oneoff report warning"]));
    }

    // (c) F3: the report read another state root.
    let elsewhere = TempDir::new().expect("tempdir");
    write_report(&report, elsewhere.path(), 600);
    let mismatch = server.tool("ac_diagnose", json!({"instance": ALIAS}));
    assert_eq!(mismatch["ok"], true);
    assert_eq!(
        mismatch["result"]["suspended_report"]["code"],
        "suspended_report_state_root_mismatch"
    );
    assert_eq!(mismatch["result"]["incomplete"], true);
    for list in ["suspended", "lift", "repeating", "report_warnings"] {
        assert_eq!(mismatch["result"][list], json!([]), "{list}");
    }

    // (a) F1: actingd stopped.
    runtime.stop_clean();
    let stopped = server.tool("ac_overview", json!({}));
    assert_eq!(stopped["ok"], true);
    assert_eq!(stopped["result"]["daemon"]["online"], false);
    assert_eq!(stopped["result"]["daemon"]["error"]["code"], "runtime_unavailable");
    assert!(stopped["result"].get("instances").is_none());
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
                "timeout_ms":3000,
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
