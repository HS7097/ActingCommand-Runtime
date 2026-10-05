// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #338 S2 evidence on the runtime-host fixtures: a host with
//! the fixture catalog (contracts/scheduling/examples/catalog-a, two instances) and a governance
//! policy whose allowed_clients refuses `actingctl-mcp`, driven by the actingctl.exe of the
//! product commit's exact-SHA build (ONEOFF_ACTINGCTL): ac_resources_list / ac_targets_get,
//! ac_targets_set and clearing with empty targets, the refused identity card as a warning,
//! ac_pause / external resume / ac_pause / ac_resume refused for a stale revision and a stale
//! epoch, and the provenance per correlation. Copied into the runtime-host test module by the
//! one-off workflow only; every printed line starts `S2H|`.

use super::*;
use actingcommand_contract::{EventActor, EventQuery, EventSource, EventType, ProjectionProfile};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::collections::BTreeSet;
use std::io::{BufRead as _, BufReader, Write as _};
use std::sync::mpsc::{self as s2_mpsc, Receiver as S2Receiver};

struct S2Server {
    label: String,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    lines: S2Receiver<String>,
    next_id: u64,
}

impl S2Server {
    fn spawn(label: &str, args: &[&str]) -> Self {
        let actingctl = std::env::var_os("ONEOFF_ACTINGCTL").expect("ONEOFF_ACTINGCTL");
        let mut child = std::process::Command::new(actingctl)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn mcp-serve");
        let stdout = child.stdout.take().expect("stdout");
        let (sender, lines) = s2_mpsc::channel();
        std::thread::spawn(move || {
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
        let answer = server.request(serde_json::json!({
            "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "oneoff", "version": "0"}},
        }));
        assert_eq!(answer["result"]["protocolVersion"], "2025-11-25");
        server.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, message: &serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{message}").expect("write");
        stdin.flush().expect("flush");
    }

    fn request(&mut self, mut message: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        message["jsonrpc"] = serde_json::json!("2.0");
        message["id"] = serde_json::json!(id);
        self.send(&message);
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("an answer");
            let answer: serde_json::Value = serde_json::from_str(&line).expect("JSON");
            if answer.get("id") == Some(&serde_json::json!(id)) {
                return answer;
            }
        }
    }

    fn tool(&mut self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let answer = self.request(serde_json::json!({
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }));
        let structured = answer["result"]["structuredContent"].clone();
        let text = structured.to_string();
        let shown = if text.len() > 1500 {
            let mut end = 1500;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{} ...(+{} bytes)", &text[..end], text.len() - end)
        } else {
            text
        };
        println!("S2H|{}|{name} {arguments} -> {shown}", self.label);
        structured
    }

    fn close(mut self) {
        drop(self.stdin.take());
        let status = self.child.wait().expect("mcp-serve exit");
        assert!(status.success());
    }
}

fn s2_error_code(envelope: &serde_json::Value) -> String {
    format!(
        "code={} host_code={}",
        envelope["error"]["code"], envelope["error"]["details"]["host_failure"]["code"]
    )
}

#[test]
fn oneoff_338s2_operator_tools_on_the_fixture_catalog() {
    let root = TempDir::new().expect("state root");
    let host = RuntimeHost::start(
        config(&root)
            .with_policy_inputs(PolicyInputSnapshot::new(
                pending_policy_facts(),
                pending_policy_resources(),
            ))
            .with_governance_policy(crate::GovernancePolicy {
                allowed_clients: Some(BTreeSet::from(["oneoff-other-client".to_owned()])),
            }),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("two-instance host");
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("catalog activation");
    let state_root = root.path().to_str().expect("utf-8").to_owned();

    // Observer only: the R5 reads answer, a write tool is refused.
    let mut observer = S2Server::spawn("observer", &["mcp-serve", "--state-root", &state_root]);
    let listed = observer.tool(
        "ac_resources_list",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS}),
    );
    assert_eq!(listed["ok"], true);
    assert!(listed["result"]["targetable"].is_array());
    let refused = observer.tool(
        "ac_targets_set",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS, "targets": []}),
    );
    assert_eq!(refused["error"]["code"], "tier_not_enabled");
    observer.close();

    let mut server = S2Server::spawn(
        "operator",
        &["mcp-serve", "--state-root", &state_root, "--tier", "operator"],
    );
    let before = server.tool(
        "ac_targets_get",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS}),
    );
    assert_eq!(before["ok"], true);
    let set = server.tool(
        "ac_targets_set",
        serde_json::json!({
            "instance": POLICY_INSTANCE_ALIAS,
            "valid_days": 1,
            "targets": [{
                "id": "target-a",
                "resource": "fixture-pool-a",
                "condition": {"kind": "at_least", "amount": 50},
                "scale": 10,
                "importance_milli": 500,
                "apply": {"mode": "adjust", "weight": "score_stage"}
            }],
        }),
    );
    assert_eq!(set["ok"], true);
    let active = server.tool(
        "ac_targets_get",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS}),
    );
    assert!(active["result"]["active"].is_object());
    let cleared = server.tool(
        "ac_targets_set",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS, "targets": []}),
    );
    println!("S2H|CLEAR|empty targets -> ok {}", cleared["ok"]);
    let after_clear = server.tool(
        "ac_targets_get",
        serde_json::json!({"instance": POLICY_INSTANCE_ALIAS}),
    );
    println!("S2H|CLEAR|active after clearing -> {}", after_clear["result"]["active"]);

    // The refused identity card is a warning; the pause still goes ahead.
    let first = server.tool("ac_pause", serde_json::json!({}));
    assert_eq!(first["ok"], true);
    println!("S2H|CARD|warnings {}", first["warnings"]);
    assert!(
        first["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.iter().any(|warning| {
                warning["code"] == "governance_client_not_allowed"
                    && warning["details"]["card_refusal"]["details"]["host_failure"]["code"]
                        == "governance_client_not_allowed"
            }))
    );
    let epoch = first["result"]["owner_epoch"].as_str().expect("owner_epoch").to_owned();
    let first_revision = first["result"]["paused"]["revision"]
        .as_u64()
        .or_else(|| first["result"]["paused"]["scheduling_paused"]["revision"].as_u64())
        .expect("revision");
    // An external resume, then a new pause: the first revision is stale.
    let external = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("external client");
    let resumed = external
        .resume_scheduling(actingcommand_contract::SchedulingPauseScope::Global)
        .expect("external resume");
    println!("S2H|PAUSE|external resume -> {}", serde_json::to_string(&resumed).expect("json"));
    let second = server.tool("ac_pause", serde_json::json!({}));
    let second_revision = second["result"]["paused"]["revision"]
        .as_u64()
        .or_else(|| second["result"]["paused"]["scheduling_paused"]["revision"].as_u64())
        .expect("revision");
    println!("S2H|PAUSE|revisions first {first_revision} second {second_revision} epoch {epoch}");
    let stale_revision = server.tool(
        "ac_resume",
        serde_json::json!({"expected_owner_epoch": epoch, "expected_revision": first_revision}),
    );
    println!("S2H|PAUSE|stale revision -> {}", s2_error_code(&stale_revision));
    assert_eq!(stale_revision["ok"], false);
    let stale_epoch = server.tool(
        "ac_resume",
        serde_json::json!({
            "expected_owner_epoch": "epoch_00000000000000000000000000000001",
            "expected_revision": second_revision,
        }),
    );
    println!("S2H|PAUSE|stale epoch -> {}", s2_error_code(&stale_epoch));
    assert_eq!(stale_epoch["ok"], false);
    let lifted = server.tool(
        "ac_resume",
        serde_json::json!({"expected_owner_epoch": epoch, "expected_revision": second_revision}),
    );
    assert_eq!(lifted["ok"], true);
    server.close();

    // Provenance: every MCP client.action and what else shares its correlation.
    let actions = external
        .query_events(
            EventQuery {
                event_type: Some(EventType::ClientAction),
                ..EventQuery::default()
            },
            ProjectionProfile::Forensic,
        )
        .expect("client.action events");
    for action in &actions {
        let correlation = action.links.correlation_id().copied();
        let together = external
            .query_events(
                EventQuery {
                    correlation_id: correlation,
                    ..EventQuery::default()
                },
                ProjectionProfile::Concise,
            )
            .expect("correlation events");
        let payload = serde_json::to_string(&action.payload).expect("payload");
        println!(
            "S2H|PROVENANCE|client.action surface_mcp={} payload_has_control={:?} actor={:?} source={:?} correlation_events={:?}",
            payload.contains("\"surface_id\":\"mcp\""),
            ["ac_targets_set", "ac_pause", "ac_resume"]
                .into_iter()
                .find(|control| payload.contains(control)),
            action.origin.actor(),
            action.origin.source(),
            together
                .iter()
                .map(|event| serde_json::to_string(&event.event_type).expect("type"))
                .collect::<Vec<_>>()
        );
    }
    assert!(!actions.is_empty());
    host.close().expect("host close");
}
