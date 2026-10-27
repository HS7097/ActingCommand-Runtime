// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #421 MCP-ENTRY, test rows E-1 to E-11: the entry's logic with a fake child and a
//! fake clock (no process, no sleep).

use crate::logic::{ClientKind, Core, EntryConfig, Host, PlanEnd, Stamp};
use serde_json::{Value, json};
use std::io;

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

fn plan(switch: bool, commit: &str) -> Value {
    json!({
        "switch": switch,
        "reason": if switch { "build_changed" } else { "same_build" },
        "build": {"commit": commit, "manifest_sha256": "a".repeat(64)},
        "child": ["ROOT/runtime/actingctl.exe", "mcp-serve", "--tier", "observer,operator"],
        "tunables": {"poll_ms": 1_000, "settle_ms": 5_000, "verify_timeout_ms": 30_000, "drain_max_ms": 60_000},
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

    // The same install is not tried again until it changes.
    entry.host.now += 60_000;
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
