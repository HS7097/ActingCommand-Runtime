// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #421 MCP-ENTRY, test rows E-1 to E-11, and fix round 1's E-13, E-14, E-16 and E-18
//! to E-23: the entry's logic with a fake child and a fake clock (no process, no sleep).

use crate::logic::{ClientKind, Core, EntryConfig, Host, PlanEnd, Stamp};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

#[derive(Default)]
struct FakeHost {
    now: u64,
    stamp: Stamp,
    busy: bool,
    fail_spawn: bool,
    next_pid: u32,
    spawned: Vec<(u64, Vec<String>)>,
    sent: Vec<(u64, Value)>,
    stopped: Vec<u64>,
    plans: Vec<Option<String>>,
    client: Vec<Value>,
    status: Value,
}

impl Host for FakeHost {
    fn now_ms(&self) -> u64 {
        self.now
    }

    fn stamp(&self) -> Stamp {
        self.stamp.clone()
    }

    fn busy(&mut self) -> bool {
        self.busy
    }

    fn spawn(&mut self, id: u64, command: &[String]) -> io::Result<u32> {
        if self.fail_spawn {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        self.next_pid += 1;
        self.spawned.push((id, command.to_vec()));
        Ok(1000 + self.next_pid)
    }

    fn send(&mut self, id: u64, line: &[u8]) -> bool {
        let message = serde_json::from_slice(line).expect("the entry sends JSON");
        self.sent.push((id, message));
        !self.stopped.contains(&id)
    }

    fn stop(&mut self, id: u64) {
        self.stopped.push(id);
    }

    fn start_plan(&mut self, current_build: Option<&str>, _timeout_ms: u64) {
        self.plans.push(current_build.map(str::to_owned));
    }

    fn to_client(&mut self, line: &[u8]) {
        self.client
            .push(serde_json::from_slice(line).expect("the entry answers JSON"));
    }

    fn write_status(&mut self, status: &Value) {
        self.status = status.clone();
    }

    fn note(&mut self, _text: &str) {}
}

const ARGS: [&str; 3] = ["mcp-serve", "--tier", "observer,operator"];
const OLD: &str = "1111111111111111111111111111111111111111";
const NEW: &str = "2222222222222222222222222222222222222222";

fn entry() -> Core<FakeHost> {
    let host = FakeHost {
        now: 1_000_000,
        stamp: Stamp {
            files: vec![Some((10, 1))],
        },
        ..FakeHost::default()
    };
    let mut entry = Core::new(
        host,
        EntryConfig {
            default_exe: "ROOT/runtime/actingctl.exe".to_owned(),
            child_args: ARGS.iter().map(|word| (*word).to_owned()).collect(),
            client_kind: Some(ClientKind::Codex),
            pid: 77,
        },
    );
    entry.start();
    // The first plan names the first child's build.
    assert_eq!(entry.host.plans, vec![None]);
    entry.plan_done(PlanEnd::Answered(plan(false, OLD)));
    entry
}

static TUNABLES: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
    json!({"poll_ms": 1_000, "settle_ms": 5_000, "verify_timeout_ms": 30_000,
        "drain_max_ms": 60_000, "refused_retry_max_ms": 12_000})
});

fn plan(switch: bool, commit: &str) -> Value {
    json!({
        "switch": switch,
        "reason": if switch { "build_changed" } else { "same_build" },
        "build": {"commit": commit, "manifest_sha256": "a".repeat(64)},
        "child": ["ROOT/runtime/actingctl.exe", "mcp-serve", "--tier", "observer,operator"],
        "tunables": TUNABLES.clone(),
        "entry_protocol": 1,
        "outcome": {},
    })
}

fn line(value: &Value) -> Vec<u8> {
    value.to_string().into_bytes()
}

fn client(entry: &mut Core<FakeHost>, message: &Value) {
    entry.client_line(&line(message));
}

fn child(entry: &mut Core<FakeHost>, id: u64, message: &Value) {
    entry.child_line(id, &line(message));
}

/// The messages child `id` received, in order.
fn received(entry: &Core<FakeHost>, id: u64) -> Vec<Value> {
    entry
        .host
        .sent
        .iter()
        .filter(|(child, _)| *child == id)
        .map(|(_, message)| message.clone())
        .collect()
}

/// Answers every request of the entry's own that child `id` has not answered yet: initialize
/// with `version`, tools/list, server/discover, and entryStatus with `status`.
fn serve_internal(entry: &mut Core<FakeHost>, id: u64, version: &str, status: &Value) {
    let mut answered = 0;
    loop {
        let pending = received(entry, id)
            .into_iter()
            .filter(|message| {
                message.get("method").is_some()
                    && message
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|key| key.starts_with("actingcommand-entry:"))
            })
            .collect::<Vec<_>>();
        if pending.len() == answered {
            return;
        }
        let request = pending[answered].clone();
        answered += 1;
        let result = match request["method"].as_str() {
            Some("initialize") => {
                json!({"protocolVersion": version, "capabilities": {"tools": {"listChanged": false}}})
            }
            Some("tools/list") => json!({"tools": [{"name": format!("tool_{id}")}]}),
            Some("server/discover") => {
                json!({"supportedVersions": [version], "capabilities": {"tools": {}}})
            }
            _ => status.clone(),
        };
        child(
            entry,
            id,
            &json!({"jsonrpc": "2.0", "id": request["id"], "result": result}),
        );
    }
}

/// A legacy session on child 1: initialize answered with 2025-11-25, then initialized.
fn legacy_session(entry: &mut Core<FakeHost>) {
    client(
        entry,
        &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "codex-mcp-client", "version": "9.9"}}}),
    );
    child(
        entry,
        1,
        &json!({"jsonrpc": "2.0", "id": 0, "result": {"protocolVersion": "2025-11-25",
            "capabilities": {"tools": {"listChanged": false}}, "serverInfo": {"name": "actingctl-mcp"}}}),
    );
    client(
        entry,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
}

/// The install changes and settles; the plan names the new build.
fn plan_switch(entry: &mut Core<FakeHost>) {
    entry.host.stamp = Stamp {
        files: vec![Some((20, 2))],
    };
    entry.tick();
    entry.host.now += 5_000;
    entry.tick();
    assert_eq!(
        entry.host.plans.last(),
        Some(&Some(OLD.to_owned())),
        "a settled change asks the plan with the current build"
    );
    entry.plan_done(PlanEnd::Answered(plan(true, NEW)));
}

fn methods(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| message.get("method").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

fn list_changed(entry: &Core<FakeHost>) -> Vec<Value> {
    entry
        .host
        .client
        .iter()
        .filter(|message| message["method"] == "notifications/tools/list_changed")
        .cloned()
        .collect()
}

#[test]
fn e1_the_entry_pipes_both_ways_and_keeps_ids() {
    let mut entry = entry();
    assert_eq!(entry.host.spawned.len(), 1);
    assert_eq!(
        entry.host.spawned[0].1,
        vec![
            "ROOT/runtime/actingctl.exe",
            "mcp-serve",
            "--tier",
            "observer,operator"
        ],
        "the child takes the arguments after -- unchanged"
    );
    legacy_session(&mut entry);
    let init = &entry.host.client[0];
    assert_eq!(init["id"], 0);
    assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);

    let call = json!({"jsonrpc": "2.0", "id": "call-7", "method": "tools/call",
        "params": {"name": "ac_overview", "arguments": {}}});
    client(&mut entry, &call);
    assert_eq!(received(&entry, 1).last(), Some(&call));
    let progress = json!({"jsonrpc": "2.0", "method": "notifications/progress",
        "params": {"progressToken": 1, "progress": 1}});
    child(&mut entry, 1, &progress);
    let answer = json!({"jsonrpc": "2.0", "id": "call-7", "result": {"content": []}});
    child(&mut entry, 1, &answer);
    let tail = &entry.host.client[entry.host.client.len() - 2..];
    assert_eq!(tail, &[progress, answer]);
}

#[test]
fn e2_the_handshake_is_replayed_and_a_changed_protocol_version_is_refused() {
    let mut entry = entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    assert_eq!(
        entry.host.spawned.len(),
        2,
        "no call ever ran: the candidate starts"
    );
    let replay = received(&entry, 2);
    assert_eq!(replay[0]["method"], "initialize");
    assert_eq!(
        replay[0]["params"]["clientInfo"]["name"],
        "codex-mcp-client"
    );
    assert_eq!(replay[0]["params"]["protocolVersion"], "2025-11-25");
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(
        methods(&received(&entry, 2)),
        vec!["initialize", "notifications/initialized", "tools/list"]
    );
    assert_eq!(entry.current_child(), Some(2));

    // The next change: a candidate that negotiates another version is refused.
    entry.host.stamp = Stamp {
        files: vec![Some((30, 3))],
    };
    entry.tick();
    entry.host.now += 5_000;
    entry.tick();
    entry.plan_done(PlanEnd::Answered(plan(true, OLD)));
    serve_internal(&mut entry, 3, "2025-06-18", &json!({}));
    assert_eq!(entry.current_child(), Some(2));
    assert!(entry.host.stopped.contains(&3));
    assert_eq!(
        entry.host.status["last_candidate_failure"]["problem"],
        "protocol_version_changed"
    );
}

#[test]
fn e3_a_switch_happens_only_between_calls() {
    let mut entry = entry();
    legacy_session(&mut entry);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "ac_overview"}}),
    );
    plan_switch(&mut entry);
    assert_eq!(
        entry.host.spawned.len(),
        1,
        "no candidate while a call is in flight"
    );
    assert_eq!(entry.host.status["deferred"]["kind"], "calls_in_flight");
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": 5, "result": {"content": []}}),
    );
    // The call is answered: the child is asked for its handles, then the candidate starts.
    serve_internal(
        &mut entry,
        1,
        "2025-11-25",
        &json!({"outstanding_handles": 0, "kinds": [], "calls_in_flight": 0}),
    );
    assert_eq!(entry.host.spawned.len(), 2);
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(entry.current_child(), Some(2));
}

#[test]
fn e4_a_failed_candidate_keeps_the_old_child_and_is_reported() {
    let mut entry = entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    let request = received(&entry, 2)[0].clone();
    child(
        &mut entry,
        2,
        &json!({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32603, "message": "x"}}),
    );
    assert_eq!(entry.current_child(), Some(1));
    assert!(entry.host.stopped.contains(&2));
    assert_eq!(
        entry.host.status["last_candidate_failure"]["problem"],
        "initialize_failed"
    );
    assert_eq!(entry.host.status["child"]["build"], OLD);
    let call = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"});
    client(&mut entry, &call);
    assert_eq!(received(&entry, 1).last(), Some(&call));

    // The same install is not tried again right away (fix round 1, review L2: once more after
    // settle_ms, E-20).
    entry.host.now += 4_000;
    entry.tick();
    assert_eq!(entry.host.plans.len(), 2);
}

#[test]
fn e5_a_child_that_dies_mid_call_gives_outcome_unknown_and_is_never_resent() {
    let mut entry = entry();
    legacy_session(&mut entry);
    let call = json!({"jsonrpc": "2.0", "id": 41, "method": "tools/call",
        "params": {"name": "ac_run_pack", "arguments": {}}});
    client(&mut entry, &call);
    entry.child_exited(1);
    let error = entry.host.client.last().expect("an answer").clone();
    assert_eq!(error["id"], 41);
    assert_eq!(error["error"]["message"], "mcp_entry_outcome_unknown");
    assert_eq!(
        error["error"]["data"]["inspect_with"],
        json!(["ac_get_run", "ac_overview"])
    );
    assert_eq!(entry.current_child(), None);

    let next = json!({"jsonrpc": "2.0", "id": 42, "method": "tools/list"});
    client(&mut entry, &next);
    assert_eq!(
        entry.host.spawned.len(),
        2,
        "the next call starts a fresh child"
    );
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    let sent = received(&entry, 2);
    assert_eq!(
        methods(&sent),
        vec!["initialize", "notifications/initialized", "tools/list"]
    );
    assert_eq!(sent.last(), Some(&next));
    assert!(
        entry
            .host
            .sent
            .iter()
            .filter(|(_, message)| message["id"] == 41)
            .count()
            == 1,
        "the call in flight was sent once only"
    );
}

#[test]
fn e6_outstanding_handles_defer_the_switch_until_they_reach_zero() {
    let mut entry = entry();
    legacy_session(&mut entry);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "ac_lab_do"}}),
    );
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": 5, "result": {"content": []}}),
    );
    plan_switch(&mut entry);
    serve_internal(
        &mut entry,
        1,
        "2025-11-25",
        &json!({"outstanding_handles": 2, "kinds": ["lab"], "calls_in_flight": 0}),
    );
    assert_eq!(entry.host.spawned.len(), 1);
    assert_eq!(entry.host.status["deferred"]["kind"], "handles");
    assert_eq!(entry.host.status["deferred"]["outstanding_handles"], 2);

    // Past drain_max_ms it still waits, and says so.
    entry.host.now += 61_000;
    entry.tick();
    serve_internal(
        &mut entry,
        1,
        "2025-11-25",
        &json!({"outstanding_handles": 1, "kinds": ["lab"], "calls_in_flight": 0}),
    );
    assert_eq!(entry.host.spawned.len(), 1);
    assert_eq!(entry.host.status["deferred"]["past_drain_max"], true);

    entry.tick();
    serve_internal(
        &mut entry,
        1,
        "2025-11-25",
        &json!({"outstanding_handles": 0, "kinds": [], "calls_in_flight": 0}),
    );
    assert_eq!(entry.host.spawned.len(), 2);
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(entry.current_child(), Some(2));
}

#[test]
fn e7_a_child_without_entry_status_counts_as_zero() {
    let mut entry = entry();
    legacy_session(&mut entry);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "ac_overview"}}),
    );
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": 5, "result": {"content": []}}),
    );
    plan_switch(&mut entry);
    let query = received(&entry, 1).last().expect("a query").clone();
    assert_eq!(query["method"], "actingcommand/entryStatus");
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": query["id"],
            "error": {"code": -32601, "message": "mcp_method_unknown"}}),
    );
    assert_eq!(entry.host.spawned.len(), 2);
}

#[test]
fn e8_no_plan_while_an_install_operation_is_under_way() {
    let mut entry = entry();
    entry.host.busy = true;
    entry.host.stamp = Stamp {
        files: vec![Some((20, 2))],
    };
    entry.tick();
    entry.host.now += 5_000;
    entry.tick();
    entry.host.now += 5_000;
    entry.tick();
    assert_eq!(entry.host.plans.len(), 1, "only the first plan ran");
    assert_eq!(entry.host.status["deferred"]["kind"], "install_busy");
    entry.host.busy = false;
    entry.tick();
    assert_eq!(entry.host.plans.len(), 2);
}

#[test]
fn e9_the_legacy_notice_is_sent_and_the_refetch_recorded() {
    let mut entry = entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(
        list_changed(&entry),
        vec![json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})]
    );
    assert_eq!(entry.host.status["notice"]["tools_list_after"], false);
    assert_eq!(entry.host.status["refresh"]["needed"], true);
    assert_eq!(entry.host.status["refresh"]["entry"], "codex_reload");
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 8, "method": "tools/list"}),
    );
    assert_eq!(entry.host.status["notice"]["tools_list_after"], true);
    assert_eq!(entry.host.status["refresh"]["needed"], false);
    assert_eq!(received(&entry, 2).last().expect("forwarded")["id"], 8);
}

#[test]
fn e10_the_2026_07_28_notice_goes_through_subscriptions_listen() {
    let mut entry = entry();
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "dsh", "version": "2"},
    });
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": "listen-1", "method": "subscriptions/listen",
            "params": {"_meta": meta, "notifications": {"toolsListChanged": true}}}),
    );
    let ack = entry
        .host
        .client
        .last()
        .expect("an acknowledgement")
        .clone();
    assert_eq!(ack["method"], "notifications/subscriptions/acknowledged");
    assert_eq!(
        ack["params"]["_meta"]["io.modelcontextprotocol/subscriptionId"],
        "listen-1"
    );
    assert_eq!(
        ack["params"]["notifications"],
        json!({"toolsListChanged": true})
    );
    assert!(
        received(&entry, 1).is_empty(),
        "the entry owns the listen stream"
    );
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": "d1", "method": "server/discover", "params": {"_meta": meta}}),
    );
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": "d1", "result": {"supportedVersions": ["2026-07-28"],
            "capabilities": {"tools": {}}}}),
    );
    assert_eq!(
        entry.host.client.last().expect("discover")["result"]["capabilities"]["tools"]["listChanged"],
        true
    );

    plan_switch(&mut entry);
    serve_internal(&mut entry, 2, "2026-07-28", &json!({}));
    let replay = received(&entry, 2);
    assert_eq!(methods(&replay), vec!["server/discover", "tools/list"]);
    assert_eq!(replay[1]["params"]["_meta"], meta);
    assert_eq!(entry.current_child(), Some(2));
    assert_eq!(
        list_changed(&entry),
        vec![
            json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed",
            "params": {"_meta": {"io.modelcontextprotocol/subscriptionId": "listen-1"}}})
        ]
    );
    assert_eq!(entry.host.status["notice"]["legacy"], false);
    assert_eq!(entry.host.status["notice"]["listen_subscriptions"], 1);
}

#[test]
fn e11_the_status_file_says_what_the_entry_runs() {
    let mut entry = entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    let status = entry.host.status.clone();
    assert_eq!(
        status["schema_version"],
        "actingcommand.mcp-entry-status.v1"
    );
    assert_eq!(status["entry_generation"], 1);
    assert_eq!(status["entry_protocol"], 1);
    assert_eq!(status["pid"], 77);
    assert_eq!(status["client"]["configured"], "codex");
    assert_eq!(status["client"]["name"], "codex-mcp-client");
    assert_eq!(status["client"]["version"], "9.9");
    assert_eq!(status["protocol"]["generation"], "legacy");
    assert_eq!(status["protocol"]["version"], "2025-11-25");
    assert_eq!(status["child"]["pid"], 1002);
    assert_eq!(status["child"]["build"], NEW);
    assert_eq!(status["child"]["since_unix_ms"], 1_005_000);
    assert_eq!(status["previous_children"][0]["pid"], 1001);
    assert_eq!(status["previous_children"][0]["build"], OLD);
    assert_eq!(status["previous_children"][0]["ended"], "switched");
    assert_eq!(status["last_switch"]["from_build"], OLD);
    assert_eq!(status["last_switch"]["to_build"], NEW);
    assert_eq!(status["deferred"], Value::Null);
    assert_eq!(status["notice"]["sent_at_unix_ms"], 1_005_000);
    assert_eq!(status["refresh"]["entry"], "codex_reload");
    assert_eq!(status["tunables"]["poll_ms"], 1_000);
}

// ---- fix round 1 (rd-1027/brief-mcp-entry-fix1.md, review rd-1027/mcp-entry/review.md) ------

fn config() -> EntryConfig {
    EntryConfig {
        default_exe: "ROOT/runtime/actingctl.exe".to_owned(),
        child_args: ARGS.iter().map(|word| (*word).to_owned()).collect(),
        client_kind: Some(ClientKind::Codex),
        pid: 77,
    }
}

fn status_query_of(entry: &Core<FakeHost>, id: u64) -> Value {
    let query = received(entry, id).last().expect("a query").clone();
    assert_eq!(query["method"], "actingcommand/entryStatus");
    query
}

/// A tools/call on child 1, answered.
fn answered_call(entry: &mut Core<FakeHost>, id: u64, tool: &str) {
    client(
        entry,
        &json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": tool}}),
    );
    child(
        entry,
        1,
        &json!({"jsonrpc": "2.0", "id": id, "result": {"content": []}}),
    );
}

/// The candidate child `id` refuses the replayed initialize.
fn refuse_candidate(entry: &mut Core<FakeHost>, id: u64) {
    let request = received(entry, id)[0].clone();
    child(
        entry,
        id,
        &json!({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32603, "message": "x"}}),
    );
}

/// E-13 (review H1): the child started while an install was under way; the deferred first
/// plan names the new build, which the child may not run: it is a switch, not the child's build.
#[test]
fn e13_an_initial_plan_after_the_install_changed_under_the_child_is_a_switch() {
    let host = FakeHost {
        now: 1_000_000,
        stamp: Stamp {
            files: vec![Some((10, 1))],
        },
        busy: true,
        ..FakeHost::default()
    };
    let mut entry = Core::new(host, config());
    entry.start();
    assert!(
        entry.host.plans.is_empty(),
        "no plan while the install is busy"
    );
    entry.host.stamp = Stamp {
        files: vec![Some((20, 2))],
    };
    entry.host.busy = false;
    entry.tick();
    assert_eq!(entry.host.plans, vec![None], "the first plan");
    entry.plan_done(PlanEnd::Answered(plan(false, NEW)));
    assert_eq!(
        entry.host.status["child"]["build"],
        Value::Null,
        "the installed build is not taken as the running child's"
    );
    assert_eq!(entry.host.spawned.len(), 2, "a candidate starts");
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(entry.current_child(), Some(2));
    assert_eq!(entry.host.status["child"]["build"], NEW);
    assert_eq!(entry.host.status["last_switch"]["to_build"], NEW);
}

/// E-14 (review M1): `install\current.json` alone does not hold the entry back (the plan
/// decides); a held writer lock still does.
#[test]
fn e14_an_unfinished_operation_file_alone_does_not_hold_the_entry_back() {
    let root = tempfile::TempDir::new().expect("tempdir");
    let install = root.path().join("install");
    std::fs::create_dir_all(&install).expect("the install directory");
    std::fs::write(install.join("current.json"), b"{}").expect("current.json");
    let (events, _receiver) = std::sync::mpsc::channel();
    let mut host = crate::RealHost {
        root: root.path().to_path_buf(),
        child_args: Vec::new(),
        events,
        children: HashMap::new(),
        status_path: root.path().join("entry-1.json"),
        stdout: io::stdout(),
    };
    assert!(!host.busy(), "current.json alone is the plan's to judge");
    let lock = std::fs::File::create(install.join("writer.lock")).expect("writer.lock");
    lock.lock().expect("the lock is taken");
    assert!(host.busy(), "a held writer lock still holds the plan back");
}

/// E-16 (review M2): the status names the outstanding handles the child reported, so a
/// person can collect a stuck one.
#[test]
fn e16_the_status_names_the_outstanding_handles() {
    let mut entry = entry();
    legacy_session(&mut entry);
    answered_call(&mut entry, 5, "ac_lab_do");
    plan_switch(&mut entry);
    serve_internal(
        &mut entry,
        1,
        "2025-11-25",
        &json!({"outstanding_handles": 1, "kinds": ["lab"], "handles": ["lab_job_7"], "calls_in_flight": 0}),
    );
    assert_eq!(entry.host.status["deferred"]["kind"], "handles");
    assert_eq!(
        entry.host.status["deferred"]["handles"],
        json!(["lab_job_7"])
    );
}

/// E-18 (review M4): consecutive refusals double the wait before the next plan up to
/// `refused_retry_max_ms`, a refusal's tunables are taken, and an answered plan resets it.
#[test]
fn e18_consecutive_refusals_back_off_and_carry_the_tunables() {
    let mut entry = entry();
    let refusal = json!({
        "tunables": {"poll_ms": 1_000, "settle_ms": 4_000, "verify_timeout_ms": 30_000,
            "drain_max_ms": 60_000, "refused_retry_max_ms": 10_000},
        "outcome": {"code": "install_command_refused"},
    });
    entry.host.stamp = Stamp {
        files: vec![Some((20, 2))],
    };
    entry.tick();
    entry.host.now += 5_000;
    entry.tick();
    assert_eq!(entry.host.plans.len(), 2);
    entry.plan_done(PlanEnd::Answered(refusal.clone()));
    assert_eq!(
        entry.host.status["tunables"]["refused_retry_max_ms"],
        10_000
    );
    assert_eq!(entry.host.status["plan_refusals"]["consecutive"], 1);
    // Waits of 4 s, 8 s, then the 10 s cap.
    for (wait, plans) in [(4_000, 3), (8_000, 4), (10_000, 5)] {
        entry.host.now += wait - 1;
        entry.tick();
        assert_eq!(entry.host.plans.len(), plans - 1, "not before {wait} ms");
        entry.host.now += 1;
        entry.tick();
        assert_eq!(entry.host.plans.len(), plans, "after {wait} ms");
        if plans < 5 {
            entry.plan_done(PlanEnd::Answered(refusal.clone()));
        }
    }
    entry.plan_done(PlanEnd::Answered(plan(true, NEW)));
    assert_eq!(entry.host.status["plan_refusals"]["consecutive"], 0);
    assert_eq!(entry.host.spawned.len(), 2, "the switch goes on");
}

/// E-19 (review L1): a stopped child is ended only once its grace passed; one never stopped is
/// ended at once when the entry ends.
#[test]
fn e19_a_stopped_child_is_ended_only_past_its_grace() {
    let grace = Duration::from_secs(5);
    let stopped = Instant::now();
    assert!(!crate::end_due(
        Some(stopped),
        stopped + Duration::from_millis(4_999),
        grace
    ));
    assert!(crate::end_due(Some(stopped), stopped + grace, grace));
    assert!(crate::end_due(None, stopped, grace));
}

/// E-20 (review L2): a refused candidate is tried once more after `settle_ms`, then not again
/// until the install changes.
#[test]
fn e20_a_rejected_candidate_is_tried_once_more_after_settle() {
    let mut entry = entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    refuse_candidate(&mut entry, 2);
    assert_eq!(entry.current_child(), Some(1));
    entry.host.now += 5_000;
    entry.tick();
    assert_eq!(entry.host.plans.len(), 3, "one more plan after settle_ms");
    assert_eq!(entry.host.plans[2], Some(OLD.to_owned()));
    entry.plan_done(PlanEnd::Answered(plan(true, NEW)));
    assert_eq!(entry.host.spawned.len(), 3, "a second candidate");
    refuse_candidate(&mut entry, 3);
    assert_eq!(entry.current_child(), Some(1));
    entry.host.now += 60_000;
    entry.tick();
    assert_eq!(
        entry.host.plans.len(),
        3,
        "no third try for the same install"
    );
    assert_eq!(entry.host.spawned.len(), 3);
}

/// E-21 (review L3): the child died; the first plan after the fresh child names another build
/// than its predecessor's, and the client gets the change notice. Re-review N1: also when both
/// candidates for the install's build were refused before the child died.
#[test]
fn e21_a_fresh_child_of_another_build_sends_the_notice() {
    let mut entry = entry();
    legacy_session(&mut entry);
    entry.child_exited(1);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 42, "method": "tools/list"}),
    );
    serve_internal(&mut entry, 2, "2025-11-25", &json!({}));
    assert_eq!(entry.current_child(), Some(2));
    assert!(list_changed(&entry).is_empty());
    entry.tick();
    assert_eq!(entry.host.plans.last(), Some(&None), "its build is asked");
    entry.plan_done(PlanEnd::Answered(plan(false, NEW)));
    assert_eq!(entry.host.status["child"]["build"], NEW);
    assert_eq!(
        list_changed(&entry),
        vec![json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})]
    );

    // Re-review N1: two refused candidates for the new build, then the child exits.
    let mut entry = self::entry();
    legacy_session(&mut entry);
    plan_switch(&mut entry);
    refuse_candidate(&mut entry, 2);
    entry.host.now += 5_000;
    entry.tick();
    entry.plan_done(PlanEnd::Answered(plan(true, NEW)));
    refuse_candidate(&mut entry, 3);
    assert_eq!(entry.host.plans.len(), 3);
    assert_eq!(entry.current_child(), Some(1));
    entry.child_exited(1);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 42, "method": "tools/list"}),
    );
    serve_internal(&mut entry, 4, "2025-11-25", &json!({}));
    assert_eq!(entry.current_child(), Some(4));
    entry.tick();
    assert_eq!(
        entry.host.plans.len(),
        4,
        "the fresh child's build is asked although its stamp was refused"
    );
    assert_eq!(entry.host.plans.last(), Some(&None));
    entry.plan_done(PlanEnd::Answered(plan(false, NEW)));
    assert_eq!(entry.host.status["child"]["build"], NEW);
    assert_eq!(
        list_changed(&entry),
        vec![json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})]
    );
}

/// E-22 (review L5): the status drops an answered call at once.
#[test]
fn e22_the_status_drops_an_answered_call_at_once() {
    let mut entry = entry();
    legacy_session(&mut entry);
    client(
        &mut entry,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "ac_overview"}}),
    );
    assert_eq!(entry.host.status["calls_in_flight"], 1);
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": 5, "result": {"content": []}}),
    );
    assert_eq!(entry.host.status["calls_in_flight"], 0);
}

/// E-23 (review L7): a `0` answered after another call reached the child does not count; the
/// entry asks again.
#[test]
fn e23_a_zero_count_is_ignored_after_a_call_reached_the_child() {
    let mut entry = entry();
    legacy_session(&mut entry);
    answered_call(&mut entry, 5, "ac_overview");
    plan_switch(&mut entry);
    let query = status_query_of(&entry, 1);
    answered_call(&mut entry, 6, "ac_lab_do");
    let zero = json!({"outstanding_handles": 0, "kinds": [], "handles": [], "calls_in_flight": 0});
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": query["id"], "result": zero}),
    );
    assert_eq!(entry.host.spawned.len(), 1, "the stale 0 does not count");
    let again = status_query_of(&entry, 1);
    assert_ne!(again["id"], query["id"], "the entry asks again");
    child(
        &mut entry,
        1,
        &json!({"jsonrpc": "2.0", "id": again["id"], "result": zero}),
    );
    assert_eq!(entry.host.spawned.len(), 2);
}
