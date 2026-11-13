// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #389 MCP-8 (MCP-0 command table v1.1 §2.3, §4.9): `actingctl overview` and MCP
// `ac_overview`. Both sides are the real binaries: the MCP server over pipes (fixture I5,
// `mcp_harness`) and the CLI as one process per call, on one c4 fake Runtime child
// (`tests/support/c4_runtime.rs`) with one instance, paused.
//
// The parity test: for the same arguments the CLI's stdout JSON without its `outcome` and the
// ac_overview result are the same object, apart from the start of the run window, which each
// read takes from its own clock. Every answer fits the output budget, so §2.2 leaves no other
// difference and nothing is marked truncated. Without `sections` every block is answered
// (daemon, instances, run_window, lab_tool, install, Workflow #385 P-m's performance, whose
// read time is its own like the run window's start, Workflow #389 MCP-9a's catalog,
// config_generation and watchdog, and Workflow #384 D2-S5's fact_refresh); with `sections` only
// those named and the daemon. The fake
// Runtime has no active policy catalog and no install root: catalog and watchdog are null and
// config_generation names neither generation. The instance is named by its alias and by its
// instance_id (D4). Workflow #378 A5: the
// CLI line carries `runtime_overview_reported` beside the overview.
//
// The refusal test: a section name §4.9 does not have is refused before any read, by the CLI as
// `actingctl_usage_invalid` naming `--sections`, by MCP as `arguments_invalid` naming `sections`,
// both with `argument_problem` `not_recognised`.
//
// The foreground test (Workflow #389 MCP-5b) and the activity test (Workflow #429 R-0b2) are
// described at their functions.

use crate::c4_runtime::{RuntimeChild, neutral_foreground, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    APPLICATION_FOREGROUND_FACT_KEY, EventActor, EventSource, FactValue, IdentifierIssuer,
    InputAction, InstanceActivity, InstanceActivityKind, OwnerEpoch, RUNTIME_FACT_SCHEMA_VERSION,
    RUNTIME_INFO_FILE, RuntimeControlPlaneStatus, RuntimeEventQueryPage, RuntimeFactScope,
    RuntimeFactSnapshot, RuntimeInfo, RuntimeInstanceStatus, RuntimeMonitorRegistryStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    RuntimeScheduleExplanation,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.overview";
// A block name §4.9 does not have.
const UNKNOWN_SECTION: &str = "neutral_block";
// The largest request the scripted Runtime reads.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[test]
fn gate_actingctl_overview_and_ac_overview_answer_the_same_overview_for_the_same_arguments() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    // An instance pause: the row carries it.
    let (exited, line) = actingctl(root.path(), &["pause", "--instance", ALIAS]);
    assert!(
        exited && line["outcome"]["code"] == json!("runtime_scheduling_paused"),
        "setup: actingctl pause: {line}"
    );
    let state_root = root.path().display().to_string();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    // Every block.
    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &["overview"], json!({}));
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from([
                "catalog",
                "config_generation",
                "daemon",
                "fact_refresh",
                "install",
                "instances",
                "lab_tool",
                "performance",
                "run_window",
                "watchdog",
            ]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
        assert_rows(side, answer);
        assert!(
            answer["run_window"]["since_unix_ms"].is_u64(),
            "{side}: no run window start: {answer}"
        );
        assert_eq!(
            (
                &answer["catalog"],
                &answer["config_generation"],
                &answer["watchdog"]
            ),
            (&Value::Null, &json!({}), &Value::Null),
            "{side}: {answer}"
        );
        assert_eq!(answer["lab_tool"], json!({"present": false}), "{side}");
        assert_eq!(
            answer["install"],
            json!({"state_root": state_root}),
            "{side}"
        );
    }

    // One instance by its instance_id, and two blocks.
    let instance_id = cli["instances"][0]["instance_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the overview names no instance_id: {cli}"))
        .to_owned();
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &[
            "overview",
            "--instance",
            &instance_id,
            "--sections",
            "instances,install",
        ],
        json!({"instance": instance_id, "sections": ["instances", "install"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from(["daemon", "install", "instances"]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
        assert_rows(side, answer);
        assert_eq!(
            answer["install"],
            json!({"state_root": state_root}),
            "{side}"
        );
    }

    // The instance by its alias, and the daemon alone with the run window.
    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["overview", "--instance", ALIAS, "--sections", "run_window"],
        json!({"instance": ALIAS, "sections": ["run_window"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            members(answer),
            BTreeSet::from(["daemon", "run_window"]),
            "{side}: {answer}"
        );
        assert_daemon(side, answer);
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_overview_and_ac_overview_refuse_an_unknown_section_naming_it() {
    let root = TempDir::new().expect("tempdir");
    let sections = format!("instances,{UNKNOWN_SECTION}");
    let (exited, line) = actingctl(root.path(), &["overview", "--sections", &sections]);
    assert!(!exited, "actingctl overview exited 0: {line}");
    let outcome = &line["outcome"];
    assert_eq!(
        (
            &outcome["code"],
            &outcome["values"]["argument"],
            &outcome["values"]["argument_problem"]
        ),
        (
            &json!("actingctl_usage_invalid"),
            &json!("--sections"),
            &json!("not_recognised")
        ),
        "actingctl overview: {line}"
    );

    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let answer = mcp.call_tool(
        2,
        "ac_overview",
        json!({"sections": ["instances", UNKNOWN_SECTION]}),
    );
    let error = &answer["result"]["structuredContent"]["error"];
    assert_eq!(
        (
            &answer["result"]["isError"],
            &error["class"],
            &error["outcome"]["code"],
            &error["outcome"]["values"]["argument"],
            &error["outcome"]["values"]["argument_problem"]
        ),
        (
            &json!(true),
            &json!("usage"),
            &json!("arguments_invalid"),
            &json!("sections"),
            &json!("not_recognised")
        ),
        "ac_overview: {answer}"
    );
    mcp.close_input();
    drop(mcp);
}

// Workflow #389 MCP-5b (MCP-0 command table v1.1 §2.3, §4.6, D10, ruling Q7): each instance row
// carries `foreground {package, observed_at_unix_ms}`, the instance program fact
// `application.foreground` the foreground gate records before a pointer input, so the package
// last seen, not a live read. Before any pointer input the instance has no such fact and its row
// no `foreground`, on both sides and without a warning; after one tap the row carries the
// package the fake device reports (`neutral_foreground`) and the time the fact was observed. For
// the same arguments the CLI's stdout JSON and the ac_overview result are the same object, apart
// from the start of the run window.
#[test]
fn gate_actingctl_overview_and_ac_overview_answer_the_same_foreground_the_instance_last_reported() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let words = ["overview", "--instance", ALIAS, "--sections", "instances"];
    let arguments = json!({"instance": ALIAS, "sections": ["instances"]});

    // No pointer input yet: no fact, no block.
    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &words, arguments.clone());
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert!(
            answer["instances"][0].get("foreground").is_none(),
            "{side}: a row carries a foreground no input has seen: {answer}"
        );
    }

    // One tap through the foreground gate.
    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a Runtime client");
    let token = holder.acquire_lease(ALIAS).expect("hold the instance");
    let tapped = holder.input(&token, InputAction::Tap { x: 1, y: 1 });
    holder.release_lease(&token).expect("release the instance");
    let instance_id = holder
        .status()
        .expect("read the Runtime status")
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("setup: the fake instance is not registered")
        .instance_id();
    let observed_at_unix_ms = holder
        .runtime_fact_snapshot()
        .expect("read the program facts")
        .records
        .iter()
        .find(|record| {
            record.key == APPLICATION_FOREGROUND_FACT_KEY
                && record.scope == RuntimeFactScope::Instance { instance_id }
        })
        .map(|record| record.observed_at_unix_ms)
        .unwrap_or_else(|| {
            panic!("setup: the tap left no foreground fact of the instance: {tapped:?}")
        });
    drop(holder);
    let package = neutral_foreground()
        .foreground
        .expect("the fake device reports a foreground package");

    let (cli, tool) = parity(&mut mcp, &mut id, root.path(), &words, arguments);
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            answer["instances"][0]["foreground"],
            json!({"package": package, "observed_at_unix_ms": observed_at_unix_ms}),
            "{side}: {answer}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// #411 A7 (register §1.98 A7-1; apps gate 72): each instance row carries backend_selfcheck, the
// last self-check of each backend entry the instance's program facts hold, {status,
// checked_at_unix_ms} verbatim from backend.selfcheck.<entry>.status and .checked_at_unix_ms.
// After one self-check of the fake instance (its preparation opens input and capture), the CLI's
// stdout JSON and the ac_overview result for the same arguments carry the same member, and it is
// the two entries' facts as the Runtime holds them.
#[test]
fn a7_cli_and_mcp_rows_carry_the_same_backend_selfcheck() {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;

    let holder = RuntimeClient::connect(RuntimeClientConfig::new(
        root.path(),
        actingcommand_runtime_client::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a Runtime client");
    let checked = holder.self_check_instance(ALIAS);
    assert!(checked.is_ok(), "setup: the self-check: {checked:?}");
    let instance_id = holder
        .status()
        .expect("read the Runtime status")
        .instances()
        .iter()
        .find(|instance| instance.instance_alias() == ALIAS)
        .expect("setup: the fake instance is not registered")
        .instance_id();
    let facts = holder
        .runtime_fact_snapshot()
        .expect("read the program facts");
    drop(holder);
    let fact = |key: String| {
        facts
            .records
            .iter()
            .find(|record| {
                record.key == key && record.scope == RuntimeFactScope::Instance { instance_id }
            })
            .map(|record| record.value.clone())
            .unwrap_or_else(|| panic!("setup: the self-check left no {key} fact"))
    };
    let entry = |name: &str| {
        let status = fact(format!("backend.selfcheck.{name}.status"));
        let checked_at = fact(format!("backend.selfcheck.{name}.checked_at_unix_ms"));
        let (FactValue::String(status), FactValue::Integer(checked_at)) = (status, checked_at)
        else {
            panic!("setup: the {name} self-check facts are not text and integer");
        };
        json!({"status": status, "checked_at_unix_ms": checked_at})
    };
    let expected = json!({"capture": entry("capture"), "input": entry("input")});

    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["overview", "--instance", ALIAS, "--sections", "instances"],
        json!({"instance": ALIAS, "sections": ["instances"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        assert_eq!(
            answer["instances"][0]["backend_selfcheck"], expected,
            "{side}: {answer}"
        );
    }

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

// Workflow #390 九处-1 (board #390 comment 6095942731, item 1; corrected ruling 4 of wave 18): the
// fake Runtime's instance is not discovery-bound, so no discovery or emulator control has read
// its emulator; actingctl status and the overview both show its emulator_state as unknown, never
// off.
#[test]
fn nine1_status_and_overview_show_unknown_while_the_runtime_holds_no_reading() {
    let root = TempDir::new().expect("tempdir");
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    runtime.wait_ready(root.path());
    let unknown = json!({"state": "unknown"});

    let (exited, status) = actingctl(root.path(), &["status"]);
    assert!(
        exited && status["outcome"]["code"] == json!("runtime_status_reported"),
        "actingctl status: {status}"
    );
    let instance = &status["instances"][0];
    assert_eq!(
        (&instance["instance_alias"], &instance["emulator_state"]),
        (&json!(ALIAS), &unknown),
        "actingctl status: {status}"
    );

    let (exited, overview) = actingctl(root.path(), &["overview", "--sections", "instances"]);
    assert!(exited, "actingctl overview: {overview}");
    let row = &overview["instances"][0];
    assert_eq!(
        (&row["alias"], &row["emulator_state"]),
        (&json!(ALIAS), &unknown),
        "actingctl overview: {overview}"
    );

    runtime.stop_clean();
}

// Workflow #429 R-0b2 (register §1.243): on a scripted Runtime whose Status answers one running
// instance and one idle one, and whose ledger is empty, the CLI's stdout JSON and the ac_overview
// result are the same rows: the running one carries the status' activity, the idle one no member.
// Each overview asked for one Status, and for nothing but the reads the rows had before.
#[test]
fn r0b2_cli_and_mcp_rows_carry_the_same_activity_from_one_status_read() {
    const RUNNING: &str = "neutral.running";
    const IDLE: &str = "neutral.idle";
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = |alias: &str| {
        RuntimeInstanceStatus::new(
            alias,
            *issuer.mint_instance_id().expect("instance id").transport(),
            false,
            0,
            false,
            false,
            false,
        )
        .expect("instance status")
    };
    let request_id = *issuer.mint_request_id().expect("request id").transport();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(
        root.path(),
        vec![
            instance(RUNNING).with_activity(Some(InstanceActivity {
                kind: InstanceActivityKind::PolicyRun,
                request_id: Some(request_id),
                package_id: Some("neutral.package".to_owned()),
                since_unix_ms: 1_700_000_000_000,
            })),
            instance(IDLE),
        ],
    );
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let asked_before = runtime.asked().len();

    let (cli, tool) = parity(
        &mut mcp,
        &mut id,
        root.path(),
        &["overview", "--sections", "instances"],
        json!({"sections": ["instances"]}),
    );
    for (side, answer) in [("CLI", &cli), ("ac_overview", &tool)] {
        let row = |alias: &str| {
            answer["instances"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["alias"] == json!(alias)))
                .unwrap_or_else(|| panic!("{side}: no row {alias}: {answer}"))
        };
        assert_eq!(
            row(RUNNING)["activity"],
            json!({
                "kind": "policy_run",
                "request_id": request_id,
                "package_id": "neutral.package",
                "since_unix_ms": 1_700_000_000_000_u64,
            }),
            "{side}: {answer}"
        );
        assert_eq!(row(IDLE).get("activity"), None, "{side}: {answer}");
    }

    // The CLI's reads, then the server's; the rows' run listings aside, whose pages are the same
    // with or without an activity.
    let one_overview = [
        "status",
        "runtime_fact_snapshot",
        "monitor_status",
        "schedule_explanation",
    ];
    assert_eq!(
        runtime.asked()[asked_before..]
            .iter()
            .copied()
            .filter(|read| *read != "query_events")
            .collect::<Vec<_>>(),
        [one_overview, one_overview].concat()
    );

    mcp.close_input();
    drop(mcp);
    drop(runtime);
}

// A Runtime on loopback that answers the reads of an overview's `instances` block with `instances`
// and otherwise nothing (an empty ledger, no monitor, no fact, no evaluation), lists those reads,
// and refuses every other request by panicking on its connection.
struct ScriptedRuntime {
    address: SocketAddr,
    stopping: Arc<AtomicBool>,
    asked: Arc<Mutex<Vec<&'static str>>>,
    acceptor: Option<JoinHandle<()>>,
}

impl ScriptedRuntime {
    // Listens on loopback and writes `runtime-info.json` under `root` naming it.
    fn start(root: &Path, instances: Vec<RuntimeInstanceStatus>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the scripted Runtime");
        let address = listener.local_addr().expect("scripted Runtime address");
        let owner_epoch = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("owner epoch")
            .transport();
        let info = RuntimeInfo::new(
            std::process::id(),
            address.ip().to_string(),
            address.port(),
            owner_epoch,
            1,
        )
        .expect("scripted Runtime info");
        fs::write(
            root.join(RUNTIME_INFO_FILE),
            serde_json::to_vec(&info).expect("encode the Runtime info"),
        )
        .expect("write the Runtime info");
        let status = Arc::new(
            RuntimeControlPlaneStatus::new(owner_epoch, instances).expect("scripted status"),
        );
        let stopping = Arc::new(AtomicBool::new(false));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            let asked = Arc::clone(&asked);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let stream = stream.expect("accept a client");
                    let status = Arc::clone(&status);
                    let asked = Arc::clone(&asked);
                    thread::spawn(move || serve(stream, owner_epoch, &status, &asked));
                }
            })
        };
        Self {
            address,
            stopping,
            asked,
            acceptor: Some(acceptor),
        }
    }

    // The reads asked for so far, in order (the handshake and health checks not counted).
    fn asked(&self) -> Vec<&'static str> {
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for ScriptedRuntime {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Wakes the acceptor, which then sees `stopping`.
        let _ = TcpStream::connect(self.address);
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

// Answers one client's requests until it hangs up.
fn serve(
    mut stream: TcpStream,
    owner_epoch: OwnerEpoch,
    status: &RuntimeControlPlaneStatus,
    asked: &Mutex<Vec<&'static str>>,
) {
    let record = |read: &'static str| {
        asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(read);
    };
    while let Some(request) = next_request(&mut stream) {
        let result = match request.operation() {
            RuntimeOperation::Health => RuntimeResult::Health { owner_epoch },
            // Workflow #434 LAB-6S2-B2 (adapted): the word echoed, as the Runtime echoes it
            // (actingctl and today's MCP server declare one, whose echo the client requires).
            RuntimeOperation::BindChannel {
                channel,
                via,
                initiator,
            } => RuntimeResult::ChannelBound {
                channel: *channel,
                via: *via,
                initiator: initiator
                    .as_ref()
                    .and_then(actingcommand_contract::DeclaredInitiator::initiator),
            },
            RuntimeOperation::Status => {
                record("status");
                RuntimeResult::Status {
                    status: status.clone(),
                }
            }
            RuntimeOperation::RuntimeFactSnapshot => {
                record("runtime_fact_snapshot");
                RuntimeResult::RuntimeFactSnapshot {
                    snapshot: RuntimeFactSnapshot {
                        schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
                        ledger_position: 1,
                        taken_at_unix_ms: 1,
                        snapshot_id: 1,
                        part: 1,
                        parts: 1,
                        records: Vec::new(),
                    },
                    config_version: None,
                }
            }
            RuntimeOperation::MonitorStatus => {
                record("monitor_status");
                RuntimeResult::MonitorStatus {
                    status: RuntimeMonitorRegistryStatus::new(owner_epoch, Vec::new())
                        .expect("scripted monitor status"),
                }
            }
            RuntimeOperation::ScheduleExplanation { .. } => {
                record("schedule_explanation");
                RuntimeResult::ScheduleExplanation {
                    explanation: Box::new(RuntimeScheduleExplanation {
                        evaluated_at_unix_ms: None,
                        catalog_hash: None,
                        instances: Vec::new(),
                    }),
                }
            }
            RuntimeOperation::QueryEvents { page, .. } => {
                record("query_events");
                RuntimeResult::EventPage {
                    page: RuntimeEventQueryPage::new(Vec::new(), 1, page.limit(), false, None)
                        .expect("scripted event page"),
                }
            }
            other => panic!("the scripted Runtime answers no {other:?}"),
        };
        let receipt =
            RuntimeReceipt::success(&request, RuntimeReceiptState::Completed, None, result)
                .expect("scripted receipt");
        let body = serde_json::to_vec(&receipt).expect("encode the receipt");
        let length = u32::try_from(body.len()).expect("receipt length");
        stream
            .write_all(&length.to_be_bytes())
            .and_then(|()| stream.write_all(&body))
            .and_then(|()| stream.flush())
            .expect("write the receipt");
    }
}

// The next request, or `None` once the client hung up.
fn next_request(stream: &mut TcpStream) -> Option<RuntimeRequest> {
    let mut header = [0_u8; 4];
    match stream.read_exact(&mut header) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
            ) =>
        {
            return None;
        }
        Err(error) => panic!("scripted Runtime request header: {error}"),
    }
    let length = u32::from_be_bytes(header) as usize;
    assert!(
        length > 0 && length <= MAX_REQUEST_BYTES,
        "scripted Runtime request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("scripted Runtime request body");
    Some(serde_json::from_slice(&body).expect("decode a request"))
}

// The CLI's answer and the ac_overview result for the same arguments, each accepted without
// warnings, and equal apart from the start of the run window.
fn parity(
    mcp: &mut McpServer,
    id: &mut u64,
    root: &Path,
    words: &[&str],
    arguments: Value,
) -> (Value, Value) {
    let (exited, mut line) = actingctl(root, words);
    let outcome = line
        .as_object_mut()
        .and_then(|members| members.remove("outcome"));
    assert!(
        exited
            && outcome
                .as_ref()
                .is_some_and(|outcome| outcome["code"] == json!("runtime_overview_reported")),
        "actingctl {words:?} did not report runtime_overview_reported: {outcome:?} {line}"
    );
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_overview", arguments.clone());
    let tool = accepted(&answer, "ac_overview").clone();
    assert!(
        answer["result"]["structuredContent"]
            .get("warnings")
            .is_none(),
        "ac_overview {arguments} warned: {answer}"
    );
    assert_eq!(
        without_window_start(&line),
        without_window_start(&tool),
        "actingctl {words:?} and ac_overview {arguments} answer differently:\nCLI {line}\nac_overview {tool}"
    );
    (line, tool)
}

// The daemon answers, with the owner epoch of its runtime-info.json.
#[track_caller]
fn assert_daemon(side: &str, answer: &Value) {
    let daemon = &answer["daemon"];
    assert_eq!(
        members(daemon),
        BTreeSet::from(["online", "owner_epoch"]),
        "{side}: {answer}"
    );
    assert_eq!(daemon["online"], json!(true), "{side}: {answer}");
}

// The one instance's row: its identity, lease_active, queued and its pause; no monitor and no
// run were set up, so neither is there. Workflow #390 九处-1: and its emulator_state, unknown,
// since the fake Runtime's instance is not discovery-bound and the Runtime holds no reading.
// #411 A7: and its backend_selfcheck, an object, since the program facts were read.
// #411 A42: and run_none_in_window, since the instance has no run in the window.
#[track_caller]
fn assert_rows(side: &str, answer: &Value) {
    let rows = answer["instances"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "{side}: {answer}");
    let row = &rows[0];
    assert_eq!(
        members(row),
        BTreeSet::from([
            "alias",
            "backend_selfcheck",
            "emulator_state",
            "instance_id",
            "lease_active",
            "pause",
            "queued",
            "run_none_in_window"
        ]),
        "{side}: {answer}"
    );
    assert!(
        row["alias"] == json!(ALIAS)
            && row["lease_active"].is_boolean()
            && row["queued"].is_u64()
            && row["emulator_state"] == json!({"state": "unknown"})
            && row["backend_selfcheck"].is_object(),
        "{side}: {answer}"
    );
    assert_eq!(
        members(&row["pause"]),
        BTreeSet::from(["pause_reason", "revision", "stage"]),
        "{side}: {answer}"
    );
}

// The answer without the start of the run window and the time of the performance read (Workflow
// #385 P-m): each read takes them from its own clock. #411 A42: so does each row's window with
// no run in it. #422 P29: and the performance read's ledger_read_pages, since each overview reads
// its own run block through the ledger's reader pool, so the second read counts more pages by
// design. #407 PERF-READ: and its memory, the process's counters and its owners' parts at the
// moment of each read.
fn without_window_start(answer: &Value) -> Value {
    let mut answer = answer.clone();
    if let Some(window) = answer.get_mut("run_window").and_then(Value::as_object_mut) {
        window.remove("since_unix_ms");
    }
    for row in answer
        .get_mut("instances")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(window) = row
            .get_mut("run_none_in_window")
            .and_then(Value::as_object_mut)
        {
            window.remove("since_unix_ms");
            window.remove("until_unix_ms");
        }
    }
    if let Some(performance) = answer.get_mut("performance").and_then(Value::as_object_mut) {
        performance.remove("observed_at_unix_ms");
        performance.remove("ledger_read_pages");
        performance.remove("memory");
    }
    answer
}

fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

// `actingctl <words> --state-root <root>`: whether it exited 0, and its one stdout line.
fn actingctl(root: &Path, words: &[&str]) -> (bool, Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_actingctl"))
        .args(words)
        .arg("--state-root")
        .arg(root)
        .env_remove("ACTINGCOMMAND_DETAIL")
        .output()
        .expect("run actingctl");
    let line = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "actingctl {words:?} printed no JSON line ({error}): {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), line)
}
