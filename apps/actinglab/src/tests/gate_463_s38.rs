// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-9 (register §1.444; contracts/package-variables.md, "Runs without a dispatch";
// contracts/runtime-interface.md, "The variable values of a contained run"; coordinator work order
// rd-1116/brief-463-s38-run-variables.md): the row `run start` takes `--variable <name>=<json>`,
// repeatable, read by the one parser of `--variable` (`lab_ops::core_ops::parse_variables`, which
// the scan replay and `standard-pack trial` read with), and sends the values in the run's request
// (`ContainedTaskRequest.variables`, `runtime-client` 11). Each test drives the runtime client's
// scripted Runtime on loopback in a temporary state root, one connection per line as the CLI
// module connects, and writes down every frame of every connection in the order the product sends
// them:
// - `s39_two_values_reach_the_runs_request`: a line with two values sends the status read and the
//   run, whose request names both by name with their JSON values; a line without `--variable`
//   sends a request without the member, as before.
// - `s39_a_repeated_name_and_a_right_side_that_is_no_json_are_refused_by_the_shared_parser`: a
//   name given twice is `repeated`, a right side that is no JSON `not_recognised`, each
//   `actingctl_usage_invalid` naming `--variable` after the handshake and before any read: no
//   status read and no run; the shared parser answers the same word for the same words.
// No device, no clock read, no sleep: a temporary state root and a made-up package path.

use super::*;
use crate::cli_module;
use actingcommand_contract::{
    ContainedTaskRequest, OwnerEpoch, RuntimeControlPlaneStatus, RuntimeErrorCode,
    RuntimeErrorProjection, RuntimeInstanceStatus, RuntimeOperation, RuntimeReceiptState,
    RuntimeRequest, RuntimeResult,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::codes::ArgumentProblem;
use actingcommand_lab_ops::core_ops::parse_variables;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use std::collections::BTreeMap;

/// The scripted Runtime's one instance.
const ALIAS: &str = "neutral.vars";
/// The requests of every connection's handshake.
const HANDSHAKE: [&str; 2] = ["health", "bind_channel"];

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
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

/// The handshake, then `then`.
fn frames(then: &[&str]) -> Vec<String> {
    HANDSHAKE
        .iter()
        .chain(then)
        .map(|operation| (*operation).to_owned())
        .collect()
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

/// A refusal of the Runtime: denied, `lease_busy`; the run is not what these tests are about.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// The run's request as the connection `record` received it.
fn run_request(record: &ScriptedConnectionRecord) -> ContainedTaskRequest {
    record
        .requests
        .iter()
        .find_map(|sent| match sent.operation() {
            RuntimeOperation::RunContainedTask { request, .. } => Some(request.clone()),
            _ => None,
        })
        .expect("the run's request")
}

/// The line `exit` printed.
fn line_of(exit: &Exit) -> Value {
    exit.line.clone().expect("a JSON line")
}

/// A package path the CLI module can make absolute (it reads nothing of it), in `temp`.
fn package_in(temp: &TempDir) -> String {
    let package = temp.path().join("neutral.pack");
    fs::write(&package, b"neutral package").unwrap();
    package.to_str().expect("a UTF-8 path").to_owned()
}

#[test]
fn s39_two_values_reach_the_runs_request() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let package = package_in(&temp);
    let runtime = ScriptedRuntime::start(&state, |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(denied),
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(denied),
        ]
    });
    let _state_root = use_runtime_state_root(&state);

    let with = cli_module::run_line(&words_of(&[
        "run",
        "start",
        ALIAS,
        package.as_str(),
        "--variable",
        "floor=20",
        "--variable",
        "pick=\"item_a\"",
    ]));
    let without = cli_module::run_line(&words_of(&["run", "start", ALIAS, package.as_str()]));
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "{record:?}");
    assert_eq!(record.connections.len(), 2, "{record:?}");
    for connection in &record.connections {
        assert_eq!(
            operations(connection),
            frames(&["status", "run_contained_task"])
        );
    }

    // The two values, by name, with their JSON values; the Runtime's refusal is the line's.
    let sent = run_request(&record.connections[0]);
    assert_eq!(
        sent.variables(),
        &BTreeMap::from([
            ("floor".to_owned(), json!(20)),
            ("pick".to_owned(), json!("item_a")),
        ])
    );
    assert_eq!(
        json!(sent)["variables"],
        json!({"floor": 20, "pick": "item_a"})
    );
    let printed = line_of(&with);
    assert_eq!(printed["outcome"]["code"], "lease_busy", "{printed}");
    assert_ne!(with.code, 0, "{printed}");

    // Without `--variable` the request names no `variables`, as before.
    let sent = run_request(&record.connections[1]);
    assert!(sent.variables().is_empty());
    assert!(json!(sent).get("variables").is_none());
    assert_eq!(line_of(&without)["outcome"]["code"], "lease_busy");
}

#[test]
fn s39_a_repeated_name_and_a_right_side_that_is_no_json_are_refused_by_the_shared_parser() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let package = package_in(&temp);
    let cases: [(&[&str], ArgumentProblem); 2] = [
        (&["floor=20", "floor=30"][..], ArgumentProblem::Repeated),
        (&["floor=twenty"][..], ArgumentProblem::NotRecognised),
    ];
    let runtime = ScriptedRuntime::start(&state, |_| {
        vec![ScriptedConnection::new(), ScriptedConnection::new()]
    });
    let _state_root = use_runtime_state_root(&state);

    let mut printed = Vec::new();
    for (variables, _) in &cases {
        let mut line = vec!["run", "start", ALIAS, package.as_str()];
        for variable in *variables {
            line.extend(["--variable", *variable]);
        }
        printed.push(cli_module::run_line(&words_of(&line)));
    }
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "{record:?}");
    assert_eq!(record.connections.len(), cases.len(), "{record:?}");

    for ((variables, problem), (exit, connection)) in cases
        .iter()
        .zip(printed.iter().zip(&record.connections))
    {
        // Refused after the handshake, before the status read and the run.
        assert_eq!(operations(connection), frames(&[]), "{variables:?}");
        let line = line_of(exit);
        assert_ne!(exit.code, 0, "{variables:?}: {line}");
        assert_eq!(
            line["outcome"]["code"], "actingctl_usage_invalid",
            "{variables:?}: {line}"
        );
        assert_eq!(
            line["outcome"]["values"]["argument"], "--variable",
            "{variables:?}: {line}"
        );
        assert_eq!(
            line["outcome"]["values"]["argument_problem"],
            problem.as_str(),
            "{variables:?}: {line}"
        );
        // The shared parser, which the scan replay reads its `--variable` with, answers the
        // same word for the same words.
        let refusal = parse_variables("replay", variables.iter().copied())
            .expect_err("the shared parser refuses them");
        assert_eq!(refusal.problem, *problem, "{variables:?}");
    }
}
