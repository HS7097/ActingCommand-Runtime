// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 DRILL-1 (forced compromise T-08: "CLI ↔ MCP option-two tools"), apps gate 63 and
// the parity row of apps gate 41: `actingctl ladder-drill --instance <selector> [--device]` and
// MCP `ac_ladder_drill` (`instance`, `device`) send the same `DrillRecoveryLadder` and answer the
// same object, the drill the Runtime admitted `{instance_alias, mode, ladder_request_id}`; the
// CLI line carries it beside the outcome `recovery_ladder_drill_queued` (Workflow #378 A5). The
// selector is the alias or the instance_id, resolved with one status read on either side; without
// `--device` (`device` false or absent) the drill is a plan. Both sides are the real binaries (the
// MCP server over pipes, fixture I5); the Runtime is a scripted one on loopback that records the
// drills it is asked for and admits each, so every call sees the same answer and no drill runs.
// `ac_ladder_drill` is an operator tool before U-3: an observer server neither lists nor runs it,
// and a `device` that is not a boolean is `arguments_invalid` before any request.

use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    IdentifierIssuer, InstanceId, OwnerEpoch, RUNTIME_INFO_FILE, RecoveryDrillMode,
    RecoveryLadderDrill, RequestId, RuntimeControlPlaneStatus, RuntimeInfo, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TerminalEvent,
};
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tempfile::TempDir;

const ALIAS: &str = "neutral.drill";
/// The largest request the scripted Runtime reads.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[test]
fn gate_actingctl_ladder_drill_and_ac_ladder_drill_send_the_same_drill_and_answer_the_same_object()
{
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path());
    let instance_id = json!(runtime.instance_id)
        .as_str()
        .expect("an instance_id is text")
        .to_owned();
    let ladder_request_id = json!(runtime.ladder_request_id);
    let mut mcp = McpServer::spawn(root.path());
    mcp.initialize();
    let mut id = 1;

    let cases: [(&[&str], Value, RecoveryDrillMode); 4] = [
        (
            &["ladder-drill", "--instance", ALIAS],
            json!({"instance": ALIAS}),
            RecoveryDrillMode::Plan,
        ),
        (
            &["ladder-drill", "--instance", ALIAS, "--device"],
            json!({"instance": ALIAS, "device": true}),
            RecoveryDrillMode::Device,
        ),
        (
            &["ladder-drill", "--instance", ALIAS],
            json!({"instance": ALIAS, "device": false}),
            RecoveryDrillMode::Plan,
        ),
        (
            &[
                "ladder-drill",
                "--instance",
                instance_id.as_str(),
                "--device",
            ],
            json!({"instance": instance_id.as_str(), "device": true}),
            RecoveryDrillMode::Device,
        ),
    ];
    for (words, arguments, mode) in cases {
        let (exited, mut line) = actingctl(root.path(), words);
        let outcome = line
            .as_object_mut()
            .and_then(|members| members.remove("outcome"));
        assert!(
            exited
                && outcome
                    .as_ref()
                    .is_some_and(|outcome| outcome["code"] == json!("recovery_ladder_drill_queued")),
            "actingctl {words:?} did not report recovery_ladder_drill_queued: {outcome:?} {line}"
        );
        id += 1;
        let answer = mcp.call_tool(id, "ac_ladder_drill", arguments.clone());
        let tool = accepted(&answer, "ac_ladder_drill").clone();
        let expected = json!({
            "instance_alias": ALIAS,
            "mode": mode.as_str(),
            "ladder_request_id": ladder_request_id,
        });
        assert_eq!(line, expected, "actingctl {words:?}");
        assert_eq!(tool, expected, "ac_ladder_drill {arguments}");
        assert_eq!(
            runtime.take_drills(),
            vec![(ALIAS.to_owned(), mode); 2],
            "actingctl {words:?} and ac_ladder_drill {arguments} each asked for one drill"
        );
    }

    // `device` is a boolean, as `--device` is a switch: anything else is refused before any
    // request reaches the Runtime.
    id += 1;
    let refused = mcp.call_tool(
        id,
        "ac_ladder_drill",
        json!({"instance": ALIAS, "device": "yes"}),
    );
    let outcome = tool_error(&refused, "usage", "arguments_invalid");
    assert_eq!(
        (
            &outcome["values"]["argument"],
            &outcome["values"]["argument_problem"]
        ),
        (&json!("device"), &json!("wrong_type")),
        "{refused}"
    );
    assert!(runtime.take_drills().is_empty());
    mcp.close_input();
    drop(mcp);

    // An observer server neither lists nor runs the drill (operator tier before U-3).
    let mut observer = McpServer::spawn_with_tier(root.path(), Some("observer"));
    observer.initialize();
    let listed = observer.request(2, "tools/list", json!({}));
    assert!(
        listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list answered no tools: {listed}"))
            .iter()
            .all(|tool| tool["name"] != json!("ac_ladder_drill")),
        "an observer server lists ac_ladder_drill: {listed}"
    );
    let refused = observer.call_tool(3, "ac_ladder_drill", json!({"instance": ALIAS}));
    let outcome = tool_error(&refused, "safety", "tier_not_enabled");
    assert_eq!(
        outcome["values"]["required_tier"],
        json!("operator"),
        "{refused}"
    );
    assert!(runtime.take_drills().is_empty());
    drop(observer);
    drop(runtime);
}

// The outcome of a tool error answer, after checking its class and code.
#[track_caller]
fn tool_error<'a>(answer: &'a Value, class: &str, code: &str) -> &'a Value {
    let result = &answer["result"];
    assert_eq!(result["isError"], json!(true), "not a tool error: {answer}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(
        (&error["class"], &error["outcome"]["code"]),
        (&json!(class), &json!(code)),
        "{answer}"
    );
    &error["outcome"]
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

// What the scripted Runtime answers with and what it was asked.
struct Script {
    owner_epoch: OwnerEpoch,
    instance_id: InstanceId,
    ladder_request_id: RequestId,
    issuer: IdentifierIssuer,
    drills: Mutex<Vec<(String, RecoveryDrillMode)>>,
}

// A Runtime on loopback with one instance, `ALIAS`, that admits every drill it is asked for
// under one ladder request id and records nothing else, until dropped.
struct ScriptedRuntime {
    address: SocketAddr,
    instance_id: InstanceId,
    ladder_request_id: RequestId,
    script: Arc<Script>,
    stopping: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
}

impl ScriptedRuntime {
    // Listens on loopback and writes `runtime-info.json` under `root` naming it.
    fn start(root: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the scripted Runtime");
        let address = listener.local_addr().expect("scripted Runtime address");
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let owner_epoch = *issuer.mint_owner_epoch().expect("owner epoch").transport();
        let instance_id = *issuer.mint_instance_id().expect("instance id").transport();
        let ladder_request_id = *issuer.mint_request_id().expect("request id").transport();
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
        let script = Arc::new(Script {
            owner_epoch,
            instance_id,
            ladder_request_id,
            issuer,
            drills: Mutex::new(Vec::new()),
        });
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = {
            let stopping = Arc::clone(&stopping);
            let script = Arc::clone(&script);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let stream = stream.expect("accept a client");
                    let script = Arc::clone(&script);
                    thread::spawn(move || serve(stream, &script));
                }
            })
        };
        Self {
            address,
            instance_id,
            ladder_request_id,
            script,
            stopping,
            acceptor: Some(acceptor),
        }
    }

    // The drills asked for since the last call, in order.
    fn take_drills(&self) -> Vec<(String, RecoveryDrillMode)> {
        std::mem::take(&mut *self.script.drills.lock().expect("the drills"))
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

// Answers one client's requests until it hangs up: the handshake, Status with the one instance,
// a write's provenance (the identity card and the client action) and the drill.
fn serve(mut stream: TcpStream, script: &Script) {
    while let Some(request) = next_request(&mut stream) {
        let (result, terminal) = match request.operation() {
            RuntimeOperation::Health => (
                RuntimeResult::Health {
                    owner_epoch: script.owner_epoch,
                },
                None,
            ),
            // Workflow #389 U-2: the handshake's BindChannel.
            RuntimeOperation::BindChannel { channel, via } => (
                RuntimeResult::ChannelBound {
                    channel: *channel,
                    via: *via,
                },
                None,
            ),
            RuntimeOperation::Status => (
                RuntimeResult::Status {
                    status: RuntimeControlPlaneStatus::new(
                        script.owner_epoch,
                        vec![
                            RuntimeInstanceStatus::new(
                                ALIAS,
                                script.instance_id,
                                false,
                                0,
                                false,
                                false,
                                false,
                            )
                            .expect("instance status"),
                        ],
                    )
                    .expect("scripted status"),
                },
                None,
            ),
            RuntimeOperation::DeclareGovernanceIdentity { .. } => {
                (RuntimeResult::GovernanceIdentityAccepted, None)
            }
            RuntimeOperation::RecordClientAction { .. } => (
                RuntimeResult::ClientActionRecorded,
                Some(TerminalEvent {
                    sequence: 1,
                    event_id: *script.issuer.mint_event_id().expect("event id").transport(),
                }),
            ),
            RuntimeOperation::DrillRecoveryLadder {
                instance_alias,
                mode,
            } => {
                script
                    .drills
                    .lock()
                    .expect("the drills")
                    .push((instance_alias.clone(), *mode));
                (
                    RuntimeResult::RecoveryLadderDrillQueued {
                        drill: RecoveryLadderDrill {
                            instance_alias: instance_alias.clone(),
                            mode: *mode,
                            ladder_request_id: script.ladder_request_id,
                        },
                    },
                    None,
                )
            }
            other => panic!("the scripted Runtime does not answer {other:?}"),
        };
        let receipt =
            RuntimeReceipt::success(&request, RuntimeReceiptState::Completed, terminal, result)
                .expect("scripted receipt");
        let body = serde_json::to_vec(&receipt).expect("encode the receipt");
        let length = u32::try_from(body.len()).expect("receipt length");
        stream
            .write_all(&length.to_be_bytes())
            .expect("write the receipt header");
        stream.write_all(&body).expect("write the receipt body");
        stream.flush().expect("flush the receipt");
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
        length > 0 && length <= MAX_FRAME_BYTES,
        "scripted Runtime request length {length}"
    );
    let mut body = vec![0_u8; length];
    stream
        .read_exact(&mut body)
        .expect("scripted Runtime request body");
    Some(serde_json::from_slice(&body).expect("decode a request"))
}
