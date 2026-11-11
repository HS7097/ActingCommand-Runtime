// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G1 (register §1.287, "Slice LAB-5G1"; coordinator work order
// rd-1108/arch-013/brief-lab5g1.md): the code that serves the rows the CLI module serves itself
// moved into `lab_ops::core_ops` and takes its connection from its caller. For each row below,
// the moved code on a connection the caller keeps answers the line the CLI module prints for the
// same words (a connection made for the call, as the program connects), with the same exit code,
// and the kept connection carries the requests the CLI module's own connection carries; no other
// connection is made.
// - `lab5g1_three_ways_serve_on_a_kept_connection`: one row of each way the moved code serves a
//   row: `codes` (the words of an offline command of `lab-ops`, no Runtime: the kept connection
//   carries nothing past its handshake), `events` (the words of a command of `lab-ops` that reads
//   the Runtime, `events::dispatch_on`) and `resume` (the parameters read by name on a client).
// - `lab5g1_debugging_rows_serve_on_a_kept_connection`: `run start` and `reset` (each refused by
//   the Runtime), `run stop --cancel-only` and `drill`.
// Fakes only: the runtime client's scripted Runtime on loopback in a temporary state root, one
// script per connection; no device, no clock, no sleep.

use super::*;
use crate::cli_module::{self, words};
use actingcommand_contract::{
    ContainedTaskCancellationStatus, IdentifierIssuer, InstanceId, OwnerEpoch, RecoveryDrillMode,
    RecoveryLadderDrill, RuntimeControlPlaneStatus, RuntimeErrorCode, RuntimeErrorProjection,
    RuntimeInstanceStatus, RuntimeOperation, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    SchedulingPauseScope,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::core_ops::{self, Caller};
use actingcommand_lab_ops::front::{self, Connection};
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};

/// The scripted Runtime's one instance.
const ALIAS: &str = "neutral.kept";
/// The requests of every connection's handshake.
const HANDSHAKE: [&str; 2] = ["health", "bind_channel"];

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The script of one connection, for the Runtime's owner epoch.
type Script<'a> = &'a dyn Fn(OwnerEpoch) -> ScriptedConnection;

/// One row's two answers and what each connection received.
struct Served {
    /// The CLI module's: `run_line`, on a connection made for the call.
    module: Exit,
    /// The moved code's, on the connection the test keeps.
    kept: Exit,
    /// The operation of each request, per scripted connection in order: the kept one first.
    requests: Vec<Vec<String>>,
}

/// `line` through the CLI module and through the moved code on a kept connection to one scripted
/// Runtime, which answers each connection with `script` (the kept one first; the module's when
/// `module_connects`).
fn served_both_ways(line: &[&str], script: Script<'_>, module_connects: bool) -> Served {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let runtime = ScriptedRuntime::start(&state, |owner_epoch| {
        let mut scripts = vec![script(owner_epoch)];
        if module_connects {
            scripts.push(script(owner_epoch));
        }
        scripts
    });
    let _state_root = use_runtime_state_root(&state);
    let kept = RuntimeClient::connect(front::client_config(&state)).expect("the kept connection");
    let module = cli_module::run_line(&words_of(line));
    let table = cli_module::table().expect("the operation table");
    let call = words::parse(table, &words_of(line)).unwrap_or_else(|outcome| {
        panic!("{line:?} is refused: {outcome:?}");
    });
    let operation = table.operation(&call.name).expect("the call's row");
    assert!(core_ops::serves(operation), "{line:?}");
    let kept_answer = core_ops::answer(
        operation,
        &call.params,
        &Caller {
            connection: Connection::Kept(&kept),
            ..cli_module::core_ops::caller()
        },
    );
    drop(kept);
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "{line:?}: a connection beyond the kept one and the module's"
    );
    Served {
        module,
        kept: kept_answer,
        requests: record.connections.iter().map(operations).collect(),
    }
}

/// The operation of each request a connection received.
fn operations(record: &ScriptedConnectionRecord) -> Vec<String> {
    record
        .requests
        .iter()
        .map(|request| {
            json!(request.operation())["operation"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// The exit code and the line, the request and correlation ids each run issues masked.
fn printed(exit: &Exit) -> (u8, Value) {
    let mut line = exit.line.clone().expect("a JSON line");
    mask_issued(&mut line);
    (exit.code, line)
}

fn mask_issued(value: &mut Value) {
    match value {
        Value::Object(members) => {
            for (name, member) in members.iter_mut() {
                if matches!(name.as_str(), "request_id" | "correlation_id") && member.is_string() {
                    *member = json!("<issued>");
                } else {
                    mask_issued(member);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_issued),
        _ => {}
    }
}

/// `served`'s two lines are one line, of the outcome `code`, and both connections received the
/// handshake and then `then`.
fn assert_same_line(line: &[&str], served: &Served, code: &str, then: &[&str]) {
    let module = printed(&served.module);
    assert_eq!(printed(&served.kept), module, "{line:?}");
    assert_eq!(
        module.1["outcome"]["code"],
        json!(code),
        "{line:?}: {}",
        module.1
    );
    let expected = HANDSHAKE
        .iter()
        .chain(then)
        .map(|operation| (*operation).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        served.requests,
        vec![expected.clone(), expected],
        "{line:?}"
    );
}

/// The scripted status: the one instance [`ALIAS`].
fn status(
    owner_epoch: OwnerEpoch,
    instance_id: InstanceId,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let instance =
            RuntimeInstanceStatus::new(ALIAS, instance_id, false, 0, false, false, false)
                .expect("instance status");
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, vec![instance]).expect("scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

/// A refusal of the Runtime: denied, `lease_busy`.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

#[test]
fn lab5g1_three_ways_serve_on_a_kept_connection() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }

    // The words of an offline command of `lab-ops`: no Runtime is read, so the module connects
    // to none and the kept connection carries nothing past its handshake.
    let line = ["codes", "actingctl_usage_invalid"];
    let served = served_both_ways(&line, &|_| ScriptedConnection::new(), false);
    let module = printed(&served.module);
    assert_eq!(printed(&served.kept), module, "{line:?}");
    assert_eq!(module.1["outcome"]["code"], json!("outcome_codes_reported"));
    assert_eq!(served.requests, vec![HANDSHAKE.map(str::to_owned).to_vec()]);

    // The words of a command of `lab-ops` that reads the Runtime: `events` on its connection.
    let line = ["events"];
    let served = served_both_ways(
        &line,
        &|_| ScriptedConnection::new().ledger(Vec::new()),
        true,
    );
    assert_same_line(&line, &served, "runtime_events_reported", &["query_events"]);

    // The parameters read by name on a client: `resume`, every pause.
    let line = ["resume"];
    let served = served_both_ways(
        &line,
        &|_| {
            ScriptedConnection::new().request(|request| {
                ScriptedAnswer::completed(
                    request,
                    RuntimeResult::SchedulingResumed {
                        scope: SchedulingPauseScope::Global,
                        revision: 2,
                        selfcheck: None,
                    },
                )
            })
        },
        true,
    );
    assert_same_line(
        &line,
        &served,
        "runtime_scheduling_resumed",
        &["resume_scheduling"],
    );
}

#[test]
fn lab5g1_debugging_rows_serve_on_a_kept_connection() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance_id = *issuer.mint_instance_id().expect("instance id").transport();
    let ladder_request_id = *issuer.mint_request_id().expect("request id").transport();
    let run_request_id = *issuer.mint_request_id().expect("request id").transport();
    let run_request = json!(run_request_id)
        .as_str()
        .expect("a request id is text")
        .to_owned();
    let temp = TempDir::new().unwrap();
    let package = temp.path().join("neutral.pack");
    fs::write(&package, b"neutral package").unwrap();
    let package = package.to_str().expect("a UTF-8 path").to_owned();

    // `run start`: the instance resolved, then the run the Runtime refuses.
    let line = ["run", "start", ALIAS, package.as_str()];
    let served = served_both_ways(
        &line,
        &|owner_epoch| {
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(denied)
        },
        true,
    );
    assert_same_line(
        &line,
        &served,
        "lease_busy",
        &["status", "run_contained_task"],
    );

    // `run stop --cancel-only`: the run's cancellation alone.
    let line = ["run", "stop", run_request.as_str(), "--cancel-only"];
    let served = served_both_ways(
        &line,
        &|_| {
            ScriptedConnection::new().request(|request| {
                let RuntimeOperation::CancelContainedTask { task_request_id } = request.operation()
                else {
                    return denied(request);
                };
                ScriptedAnswer::completed(
                    request,
                    RuntimeResult::ContainedTaskCancellation {
                        task_request_id: *task_request_id,
                        status: ContainedTaskCancellationStatus::Pending {
                            deadline_monotonic_ms: 5_000,
                        },
                    },
                )
            })
        },
        true,
    );
    assert_same_line(
        &line,
        &served,
        "lab_operation_completed",
        &["cancel_contained_task"],
    );

    // `reset`: the instance resolved, then the reset the Runtime refuses.
    let line = ["reset", ALIAS];
    let served = served_both_ways(
        &line,
        &|owner_epoch| {
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(denied)
        },
        true,
    );
    assert_same_line(&line, &served, "lease_busy", &["status", "safe_reset"]);

    // `drill`: the instance resolved, then the plan the Runtime queues.
    let line = ["drill", ALIAS];
    let served = served_both_ways(
        &line,
        &|owner_epoch| {
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(move |request| {
                    ScriptedAnswer::completed(
                        request,
                        RuntimeResult::RecoveryLadderDrillQueued {
                            drill: RecoveryLadderDrill {
                                instance_alias: ALIAS.to_owned(),
                                mode: RecoveryDrillMode::Plan,
                                ladder_request_id,
                            },
                        },
                    )
                })
        },
        true,
    );
    assert_same_line(
        &line,
        &served,
        "recovery_ladder_drill_queued",
        &["status", "drill_recovery_ladder"],
    );
}
