// SPDX-License-Identifier: AGPL-3.0-only

// One-off (to be reverted): Workflow #338 S1 scripted MCP client evidence on the fixture
// runtime (tests/support/c4_runtime.rs). The one-off workflow copies this file to
// apps/actingctl/tests/oneoff_338s1.rs and points ONEOFF_ACTINGCTL / ONEOFF_ACTINGD at the
// actingctl.exe / actingcommand-actingd.exe of the product commit's Windows exact-SHA build
// artifact; nothing in the product files changes.

#[path = "../../../tests/support/c4_runtime.rs"]
mod support;

use actingcommand_contract::{
    EventActor, EventQuery, EventSource, EventType, IdentifierIssuer, ProjectionProfile,
};
use actingcommand_pack_containment::Sha256Hash;
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use zip::{ZipWriter, write::FileOptions};

const MODERN: &str = "2026-07-28";
const RESULT_LIMIT: usize = 24 * 1024;
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

struct Server {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    largest_call_result: usize,
    call_results: usize,
}

impl Server {
    fn spawn(label: &str, args: &[&str]) -> Self {
        let mut child = Command::new(actingctl())
            .args(args)
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
        println!("S1|{label}|spawn actingctl {}", args.join(" "));
        Self {
            label: label.to_owned(),
            child,
            stdin,
            lines,
            largest_call_result: 0,
            call_results: 0,
        }
    }

    fn send_raw(&mut self, text: &str) {
        println!("S1|{}|>> {}", self.label, clip(text));
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(text.as_bytes()).expect("write stdin");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush stdin");
    }

    fn send(&mut self, message: &Value) {
        self.send_raw(&message.to_string());
    }

    fn next(&mut self, timeout: Duration) -> Option<Value> {
        let line = self.lines.recv_timeout(timeout).ok()?;
        println!("S1|{}|<< {}", self.label, clip(&line));
        Some(serde_json::from_str(&line).expect("stdout line is one JSON message"))
    }

    /// The answer with this id; notifications in between are printed and skipped.
    fn answer(&mut self, id: &Value, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self
                .next(left)
                .unwrap_or_else(|| panic!("{}: no answer to {id}", self.label));
            if message.get("id") == Some(id) {
                if let Some(result) = message.get("result")
                    && result.get("content").is_some()
                {
                    let size = result.to_string().len();
                    self.largest_call_result = self.largest_call_result.max(size);
                    self.call_results += 1;
                }
                return message;
            }
        }
    }

    fn call(&mut self, message: Value) -> (Value, Duration) {
        let id = message["id"].clone();
        let started = Instant::now();
        self.send(&message);
        let answer = self.answer(&id, Duration::from_secs(40));
        (answer, started.elapsed())
    }

    fn tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        let (answer, elapsed) = self.call(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        println!("S1|{}|{name} answered in {} ms", self.label, elapsed.as_millis());
        answer
    }

    fn close(mut self) {
        drop(self.stdin.take());
        let started = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("child state") {
                break status;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "{}: mcp-serve did not exit after stdin EOF",
                self.label
            );
            thread::sleep(Duration::from_millis(20));
        };
        println!(
            "S1|{}|stdin EOF -> exit {status} after {} ms; {} tools/call results, largest {} bytes (limit {RESULT_LIMIT})",
            self.label,
            started.elapsed().as_millis(),
            self.call_results,
            self.largest_call_result,
        );
        assert!(status.success());
        assert!(self.largest_call_result <= RESULT_LIMIT);
    }
}

fn clip(text: &str) -> String {
    const MAX: usize = 700;
    if text.len() <= MAX {
        return text.to_owned();
    }
    let mut end = MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} ...(+{} bytes)", &text[..end], text.len() - end)
}

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "oneoff-338s1", "version": "0"},
    })
}

fn envelope(answer: &Value) -> Value {
    let result = &answer["result"];
    if let Some(structured) = result.get("structuredContent") {
        let text = result["content"][0]["text"].as_str().expect("text content");
        assert_eq!(
            &serde_json::from_str::<Value>(text).expect("text is JSON"),
            structured,
            "the text content repeats structuredContent"
        );
        return structured.clone();
    }
    let text = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(text).expect("text content is the envelope")
}

fn initialize(server: &mut Server, id: u64, version: &str) -> Value {
    // Warm-up ping first, so the timing below is the initialize round trip, not process start.
    let (_, ping) = server.call(json!({"jsonrpc":"2.0","id":format!("warm-{id}"),"method":"ping"}));
    let (answer, elapsed) = server.call(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "oneoff-338s1", "version": "0"},
        },
    }));
    println!(
        "S1|{}|TIMING first answer after spawn (ping) {} ms; initialize({version}) {} ms",
        server.label,
        ping.as_millis(),
        elapsed.as_millis()
    );
    assert!(elapsed < Duration::from_millis(200));
    server.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    answer
}

fn client_action_count(root: &Path) -> usize {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect for the client.action count")
    .query_events(
        EventQuery {
            event_type: Some(EventType::ClientAction),
            ..EventQuery::default()
        },
        ProjectionProfile::Concise,
    )
    .expect("client.action query")
    .len()
}

fn mint_request_id() -> String {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let request = issuer.mint_request_id().expect("request id");
    serde_json::to_value(request.transport())
        .expect("request id JSON")
        .as_str()
        .expect("string")
        .to_owned()
}

fn mint_run_id() -> String {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let run = issuer.mint_run_id().expect("run id");
    serde_json::to_value(run.transport())
        .expect("run id JSON")
        .as_str()
        .expect("string")
        .to_owned()
}

#[test]
fn oneoff_338s1_a_offline_lazy_startup() {
    let empty = TempDir::new().expect("tempdir");
    let state_root = empty.path().to_str().expect("utf-8").to_owned();

    let mut server = Server::spawn("offline", &["mcp-serve", "--state-root", &state_root]);
    let answer = initialize(&mut server, 1, "2025-11-25");
    assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
    let overview = envelope(&server.tool(2, "ac_overview", json!({})));
    println!("S1|offline|ac_overview -> {}", overview);
    assert_eq!(overview["ok"], true);
    assert_eq!(overview["result"]["daemon"]["online"], false);
    assert_eq!(overview["result"]["daemon"]["error"]["code"], "runtime_unavailable");
    assert!(overview["result"].get("instances").is_none());
    for (id, name) in [(3, "ac_events"), (4, "ac_get_run")] {
        let arguments = if name == "ac_get_run" {
            json!({"handle": mint_request_id()})
        } else {
            json!({})
        };
        let answer = server.tool(id, name, arguments);
        assert_eq!(answer["result"]["isError"], true);
        let error = envelope(&answer);
        println!(
            "S1|offline|{name} -> class {} code {}",
            error["error"]["class"], error["error"]["code"]
        );
        assert_eq!(error["error"]["code"], "runtime_unavailable");
    }
    server.close();

    // Modern era, offline: server/discover needs no Runtime either.
    let mut server = Server::spawn("modern-offline", &["mcp-serve", "--state-root", &state_root]);
    let (warm, warm_elapsed) = server.call(json!({"jsonrpc":"2.0","id":"warm","method":"tools/list","params":{"_meta": modern_meta()}}));
    assert_eq!(warm["result"]["resultType"], "complete");
    let (answer, elapsed) = server.call(json!({"jsonrpc":"2.0","id":"discover","method":"server/discover","params":{"_meta": modern_meta()}}));
    println!(
        "S1|modern-offline|TIMING first answer after spawn {} ms; server/discover {} ms",
        warm_elapsed.as_millis(),
        elapsed.as_millis()
    );
    assert!(elapsed < Duration::from_millis(200));
    assert_eq!(
        answer["result"]["supportedVersions"],
        json!([MODERN, "2025-11-25", "2025-06-18", "2025-03-26"])
    );
    assert_eq!(answer["result"]["ttlMs"], 0);
    assert_eq!(answer["result"]["cacheScope"], "private");
    server.close();

    // The install and state roots are located at the first tool call, not at startup: a
    // malformed actingd.config.json does not stop initialize, the first call reports it.
    let fake_root = TempDir::new().expect("tempdir");
    fs::write(fake_root.path().join("actingd.config.json"), b"{ not json").expect("config");
    let root = fake_root.path().to_str().expect("utf-8").to_owned();
    let mut server = Server::spawn("lazy-location", &["mcp-serve", "--root", &root]);
    initialize(&mut server, 1, "2025-06-18");
    let overview = envelope(&server.tool(2, "ac_overview", json!({})));
    println!("S1|lazy-location|ac_overview -> {}", overview);
    assert_eq!(
        overview["result"]["daemon"]["error"]["code"],
        "install_state_root_unresolved"
    );
    server.close();
}

#[test]
fn oneoff_338s1_b_sessions_on_the_fixture_runtime() {
    let root = TempDir::new().expect("tempdir");
    let frame = root.path().join("sealed.png");
    support::write_sealed_frame(&frame);
    let package = root.path().join("neutral-task.zip");
    let package_sha = write_neutral_contained_task_package(&package);
    let mut runtime =
        support::RuntimeChild::spawn_for_instance(root.path(), "c4_runtime_child_process", ALIAS);
    runtime.wait_ready(root.path());
    let state_root = root.path().to_str().expect("utf-8").to_owned();

    // One manual run through the CLI, so the runs and events tools have something to read.
    let task_run = Command::new(actingctl())
        .args([
            "task-run",
            "--state-root",
            &state_root,
            "--instance",
            ALIAS,
            "--package",
            package.to_str().expect("package path"),
            "--expected-sha256",
            &package_sha,
        ])
        .output()
        .expect("actingctl task-run");
    assert!(
        task_run.status.success(),
        "task-run: {}",
        String::from_utf8_lossy(&task_run.stderr)
    );
    let task_run: Value = serde_json::from_slice(&task_run.stdout).expect("task-run JSON");
    let handle = task_run["receipt"]["request_id"]
        .as_str()
        .expect("receipt request_id")
        .to_owned();
    println!(
        "S1|fixture|actingctl task-run -> {} request_id {handle}",
        task_run["receipt"]["result"]["kind"]
    );
    let actions_before = client_action_count(root.path());
    println!("S1|ledger|client.action events before the MCP sessions: {actions_before}");

    for version in ["2025-11-25", "2025-06-18", "2025-03-26"] {
        let label = format!("legacy-{version}");
        let mut server = Server::spawn(&label, &["mcp-serve", "--state-root", &state_root]);
        let answer = initialize(&mut server, 1, version);
        assert_eq!(answer["result"]["protocolVersion"], version);
        assert_eq!(
            answer["result"]["capabilities"],
            json!({"tools": {"listChanged": false}})
        );
        assert_eq!(answer["result"]["serverInfo"]["name"], "actingctl-mcp");
        assert!(
            answer["result"]["instructions"]
                .as_str()
                .is_some_and(|text| text.starts_with("ActingCommand local control"))
        );
        let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        let tools = answer["result"]["tools"].as_array().expect("tools");
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        let structured_era = version != "2025-03-26";
        println!(
            "S1|{label}|tools/list {names:?}; title/outputSchema present: {}",
            tools
                .iter()
                .all(|tool| tool.get("title").is_some() && tool.get("outputSchema").is_some())
        );
        for tool in tools {
            assert_eq!(tool.get("title").is_some(), structured_era);
            assert_eq!(tool.get("outputSchema").is_some(), structured_era);
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
        }
        let answer = server.tool(3, "ac_overview", json!({}));
        assert_eq!(answer["result"].get("structuredContent").is_some(), structured_era);
        let overview = envelope(&answer);
        assert_eq!(overview["ok"], true);
        assert_eq!(overview["result"]["daemon"]["online"], true);
        assert_eq!(overview["result"]["instances"][0]["alias"], ALIAS);
        assert_eq!(overview["result"]["instances"][0]["run"]["state"], "succeeded");
        let answer = server.tool(4, "ac_events", json!({"limit": 5}));
        let page = envelope(&answer);
        assert_eq!(page["ok"], true);
        let cursor = page["result"]["next_cursor"]
            .as_str()
            .expect("a page of 5 has a next cursor")
            .to_owned();
        let next = envelope(&server.tool(5, "ac_events", json!({"limit": 5, "cursor": cursor})));
        assert_eq!(next["ok"], true);
        assert!(
            next["result"]["events"][0]["seq"].as_u64()
                > page["result"]["events"][4]["seq"].as_u64()
        );
        let refused = envelope(&server.tool(
            6,
            "ac_events",
            json!({"limit": 5, "view": "errors", "cursor": cursor}),
        ));
        assert_eq!(refused["error"]["code"], "cursor_invalid");
        let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":7,"method":"ping"}));
        assert_eq!(answer["result"], json!({}));
        if version == "2025-03-26" {
            server.send_raw(r#"[{"jsonrpc":"2.0","id":8,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"ac_events","arguments":{"limit":1}}}]"#);
            let batch = server.next(Duration::from_secs(20)).expect("batch answer");
            assert_eq!(batch.as_array().map(Vec::len), Some(2));
        } else {
            server.send_raw(r#"[{"jsonrpc":"2.0","id":8,"method":"ping"}]"#);
            let refused = server.next(Duration::from_secs(10)).expect("batch refusal");
            assert_eq!(refused["error"]["code"], -32600);
        }
        // notifications/cancelled: a waiting ac_get_run is answered with nothing; progress
        // arrives every 5 s while it waits.
        let waiting = mint_request_id();
        server.send(&json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "_meta": {"progressToken": "wait-10"},
                "name": "ac_get_run",
                "arguments": {"handle": waiting, "wait_s": 25},
            },
        }));
        let progress = server.next(Duration::from_secs(8)).expect("a progress notification");
        assert_eq!(progress["method"], "notifications/progress");
        assert_eq!(progress["params"]["progressToken"], "wait-10");
        server.send(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10,"reason":"one-off"}}));
        let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":11,"method":"ping"}));
        assert_eq!(answer["result"], json!({}));
        let quiet_until = Instant::now() + Duration::from_secs(22);
        while let Some(message) = server.next(quiet_until.saturating_duration_since(Instant::now())) {
            assert_ne!(message.get("id"), Some(&json!(10)), "a cancelled call was answered");
        }
        println!("S1|{label}|cancelled call 10 got no answer within 22 s");
        server.close();
    }

    // Runs: ac_get_run by handle, by an unknown run_id, and with a wait that ends at once.
    let mut server = Server::spawn("runs", &["mcp-serve", "--state-root", &state_root]);
    initialize(&mut server, 1, "2025-11-25");
    let run = envelope(&server.tool(2, "ac_get_run", json!({"handle": handle, "wait_s": 5})));
    assert_eq!(run["result"]["schema_version"], "actingcommand.run-status.v1");
    assert_eq!(run["result"]["state"], "succeeded");
    assert_eq!(run["result"]["dispatch"], "manual");
    assert_eq!(run["result"]["origin"], "cli");
    let unknown = envelope(&server.tool(3, "ac_get_run", json!({"run_id": mint_run_id()})));
    assert_eq!(unknown["result"]["state"], "not_found");
    assert_eq!(unknown["result"]["request_id"], Value::Null);
    let neither = server.tool(4, "ac_get_run", json!({}));
    assert_eq!(envelope(&neither)["error"]["code"], "arguments_invalid");
    let diagnosis = envelope(&server.tool(5, "ac_diagnose", json!({"instance": ALIAS})));
    assert_eq!(diagnosis["ok"], true);
    assert!(diagnosis["result"]["window"]["since_unix_ms"].as_u64().is_some());
    assert_eq!(diagnosis["result"]["suspended_report"]["code"], "install_root_unresolved");
    assert_eq!(diagnosis["result"]["incomplete"], true);
    let too_old = envelope(&server.tool(6, "ac_diagnose", json!({"instance": ALIAS, "since_unix_ms": 1})));
    assert_eq!(too_old["error"]["code"], "since_out_of_range");
    let unknown_instance = envelope(&server.tool(7, "ac_diagnose", json!({"instance": "no.such.instance"})));
    assert_eq!(unknown_instance["error"]["code"], "instance_unknown");
    let tier = envelope(&server.tool(8, "ac_events", json!({"limit": 100})));
    assert_eq!(tier["ok"], true);
    println!(
        "S1|runs|ac_events limit 100 -> {} rows, truncated {}",
        tier["result"]["events"].as_array().map_or(0, Vec::len),
        tier["result"]["truncated"]
    );
    server.close();

    // ac_diagnose with an install root: the read-only actingd suspended subprocess runs and
    // its own failure document is passed on (this fixture has no policy configuration).
    if let Some(actingd) = std::env::var_os("ONEOFF_ACTINGD") {
        let install = TempDir::new().expect("tempdir");
        fs::create_dir_all(install.path().join("runtime")).expect("runtime dir");
        fs::copy(&actingd, install.path().join("runtime").join("actingcommand-actingd.exe"))
            .expect("copy actingd");
        fs::write(
            install.path().join("actingd.config.json"),
            serde_json::to_vec(&json!({"state_root": state_root})).expect("config JSON"),
        )
        .expect("write config");
        let install_root = install.path().to_str().expect("utf-8").to_owned();
        let mut server = Server::spawn(
            "suspended",
            &["mcp-serve", "--root", &install_root, "--state-root", &state_root],
        );
        initialize(&mut server, 1, "2025-11-25");
        let diagnosis = envelope(&server.tool(2, "ac_diagnose", json!({"instance": ALIAS})));
        println!(
            "S1|suspended|suspended_report -> {}",
            diagnosis["result"]["suspended_report"]
        );
        assert_eq!(diagnosis["ok"], true);
        assert!(diagnosis["result"]["suspended_report"]["status"].is_string());
        server.close();
    }

    // Materials: export the PNG of a capture; text mode refuses it.
    let mut server = Server::spawn("materials", &["mcp-serve", "--state-root", &state_root]);
    initialize(&mut server, 1, "2025-11-25");
    let page = envelope(&server.tool(2, "ac_events", json!({"limit": 100})));
    let materials = page["result"]["events"]
        .as_array()
        .expect("events")
        .iter()
        .flat_map(|event| event["materials"].as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    println!("S1|materials|{} materials on the first page", materials.len());
    match materials.iter().find(|material| material["media_type"] == "image/png") {
        Some(png) => {
            let export = envelope(&server.tool(3, "ac_material", json!({"ref": png["ref"], "mode": "export"})));
            assert_eq!(export["ok"], true);
            let path = PathBuf::from(export["result"]["path"].as_str().expect("path"));
            assert_eq!(
                fs::metadata(&path).expect("exported file").len(),
                export["result"]["size"].as_u64().expect("size")
            );
            let refused = envelope(&server.tool(4, "ac_material", json!({"ref": png["ref"], "mode": "text"})));
            assert_eq!(refused["error"]["code"], "material_not_text");
        }
        None => println!("S1|materials|no PNG material on the first page: export not exercised"),
    }
    match materials.iter().find(|material| {
        material["media_type"] == "application/json" || material["media_type"] == "text/plain"
    }) {
        Some(text) => {
            let read = envelope(&server.tool(5, "ac_material", json!({"ref": text["ref"], "mode": "text", "max_bytes": 8192})));
            assert_eq!(read["ok"], true);
        }
        None => println!("S1|materials|no text or JSON material on the first page: text mode not exercised"),
    }
    server.close();

    // Modern era on the same Runtime, then the error paths.
    let mut server = Server::spawn("modern", &["mcp-serve", "--state-root", &state_root]);
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta": modern_meta()}}));
    assert_eq!(answer["result"]["resultType"], "complete");
    assert_eq!(
        answer["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "actingctl-mcp"
    );
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta": modern_meta()}}));
    assert_eq!(answer["result"]["resultType"], "complete");
    assert_eq!(answer["result"]["ttlMs"], 0);
    assert_eq!(answer["result"]["cacheScope"], "private");
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"_meta": modern_meta(),"name":"ac_overview","arguments":{}}}));
    assert_eq!(answer["result"]["resultType"], "complete");
    assert_eq!(answer["result"]["structuredContent"]["ok"], true);
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":4,"method":"ping","params":{"_meta": modern_meta()}}));
    assert_eq!(answer["error"]["code"], -32601);
    server.send_raw("{not json");
    assert_eq!(server.next(Duration::from_secs(5)).expect("parse error")["error"]["code"], -32700);
    server.send_raw(r#"{"jsonrpc":"2.0","id":10}"#);
    assert_eq!(server.next(Duration::from_secs(5)).expect("invalid request")["error"]["code"], -32600);
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":11,"method":"resources/list","params":{"_meta": modern_meta()}}));
    assert_eq!(answer["error"]["code"], -32601);
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"_meta": modern_meta(),"name":"ac_nonexistent","arguments":{}}}));
    assert_eq!(answer["error"]["code"], -32602);
    let mut unsupported = modern_meta();
    unsupported["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":13,"method":"tools/list","params":{"_meta": unsupported}}));
    assert_eq!(answer["error"]["code"], -32022);
    assert_eq!(
        answer["error"]["data"]["supported"],
        json!([MODERN, "2025-11-25", "2025-06-18", "2025-03-26"])
    );
    let mut legacy_in_meta = modern_meta();
    legacy_in_meta["io.modelcontextprotocol/protocolVersion"] = json!("2025-11-25");
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":14,"method":"tools/list","params":{"_meta": legacy_in_meta}}));
    assert_eq!(answer["error"]["code"], -32022);
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":15,"method":"tools/list"}));
    assert_eq!(answer["error"]["code"], -32602);
    let mut no_capabilities = modern_meta();
    no_capabilities
        .as_object_mut()
        .expect("meta")
        .remove("io.modelcontextprotocol/clientCapabilities");
    let (answer, _) = server.call(json!({"jsonrpc":"2.0","id":16,"method":"tools/list","params":{"_meta": no_capabilities}}));
    assert_eq!(answer["error"]["code"], -32602);
    server.send_raw(r#"[{"jsonrpc":"2.0","id":17,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}]"#);
    assert_eq!(server.next(Duration::from_secs(5)).expect("batch refusal")["error"]["code"], -32600);
    server.close();

    let mut server = Server::spawn("unknown-legacy-version", &["mcp-serve", "--state-root", &state_root]);
    let answer = initialize(&mut server, 1, "2024-11-05");
    assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
    server.close();

    let actions_after = client_action_count(root.path());
    println!("S1|ledger|client.action events after the MCP sessions: {actions_after}");
    assert_eq!(actions_before, actions_after);
    runtime.stop_clean();
}

#[test]
fn oneoff_338s1_c_list_tools_and_mcp_config() {
    for args in [
        &["mcp-serve", "--list-tools", "--format", "markdown"][..],
        &["mcp-config", "--client", "claude"][..],
        &["mcp-config", "--client", "codex"][..],
        &["mcp-config", "--client", "codex", "--tier", "observer,operator"][..],
    ] {
        let output = Command::new(actingctl()).args(args).output().expect("run actingctl");
        assert!(output.status.success(), "{args:?}");
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            println!("S1|{}|{line}", args.join(" "));
        }
    }
    for args in [
        &["mcp-serve", "--list-tools", "--tier", "operator"][..],
        &["mcp-config"][..],
    ] {
        let output = Command::new(actingctl()).args(args).output().expect("run actingctl");
        println!(
            "S1|{}|exit {} stderr {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        assert!(!output.status.success());
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
