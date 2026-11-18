// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2c (0.12.73; register §1.472; the coordinator's ruling 8 on board #480, who sends what):
// on the runtime client's scripted Runtime (loopback, a temporary state root), the Lab's `pause`
// row, a person's or an agent's pause, sends `pause_mode` `stand`, and when the receipt's outcome
// carries a cause (a pause that stands and is not yet quiet) its answer carries that outcome as its
// cause, exit 0; a recording's hold (`record start`'s pause, `lab_ops::scheduling`'s
// `pause_for_recording`) sends `quiet_or_lift`, set in code. No clock is read.

use super::*;
use crate::cli_module::{self, words};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    RuntimeOperation, RuntimeReceipt, RuntimeRequest, RuntimeResult, SchedulingPauseMode,
    SchedulingPauseReason, SchedulingPauseScope,
};
use actingcommand_lab_ops::core_ops::{self, Caller};
use actingcommand_lab_ops::front::{Connection, Front};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedRuntime,
};
use serde_json::{Value, json};

/// The mode of the one pause a connection asked for.
fn pause_mode_asked(record: &ScriptedConnectionRecord) -> SchedulingPauseMode {
    record
        .requests
        .iter()
        .find_map(|request| match request.operation() {
            RuntimeOperation::PauseScheduling { pause_mode, .. } => Some(*pause_mode),
            _ => None,
        })
        .expect("a PauseScheduling")
}

/// A completed pause of `request`'s scope whose receipt carries `cause` under its success code, as
/// a Runtime of 0.12.73 answers a person's pause that stands.
fn paused_with_cause(request: &RuntimeRequest, cause: Outcome) -> ScriptedAnswer {
    let RuntimeOperation::PauseScheduling { scope, .. } = request.operation() else {
        panic!("a pause: {:?}", request.operation());
    };
    let receipt = RuntimeReceipt::scheduling_paused(
        request,
        RuntimeResult::SchedulingPaused {
            scope: scope.clone(),
            revision: 3,
            drained: None,
        },
        Some(cause),
    )
    .expect("the scripted receipt");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// The cause a Runtime of 0.12.73 gives a person's pause that answered before its instance was
/// quiet, as the client receives it.
fn standing() -> Outcome {
    serde_json::from_value(json!({
        "code": "scheduling_pause_standing",
        "category": "info",
        "values": {"pause_stage": "paused", "in_the_way": ["lease"]},
    }))
    .expect("a received outcome")
}

#[test]
fn pg2c_the_pause_row_sends_stand_and_answers_the_receipts_cause() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let runtime = ScriptedRuntime::start(&state, |_| {
        vec![ScriptedConnection::new().request(|request| paused_with_cause(request, standing()))]
    });
    let _state_root = use_runtime_state_root(&state);
    let table = cli_module::table().expect("the operation table");
    let call = words::parse(table, &["pause".to_owned()]).expect("the line pause");
    let operation = table.operation(&call.name).expect("the row pause");

    let exit = core_ops::answer(
        operation,
        &call.params,
        &Caller {
            connection: Connection::Connect(Front::Actinglab(Initiator::LabCli)),
            ..cli_module::core_ops::caller()
        },
    );

    let line: Value = exit.line.expect("the row's line");
    assert_eq!(
        line["outcome"]["code"],
        json!("runtime_scheduling_paused"),
        "{line}"
    );
    assert_eq!(
        line["outcome"]["causes"][0]["code"],
        json!("request_pause_scheduling_completed"),
        "{line}"
    );
    assert_eq!(
        line["outcome"]["causes"][0]["causes"][0]["code"],
        json!("scheduling_pause_standing"),
        "{line}"
    );
    assert_eq!(exit.code, 0, "{line}");
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    let [connection] = &record.connections[..] else {
        panic!("one connection");
    };
    assert_eq!(pause_mode_asked(connection), SchedulingPauseMode::Stand);
}

#[test]
fn pg2c_a_recordings_hold_sends_quiet_or_lift() {
    let temp = TempDir::new().unwrap();
    let runtime = ScriptedRuntime::start(temp.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            ScriptedAnswer::completed(
                request,
                RuntimeResult::SchedulingPaused {
                    scope: SchedulingPauseScope::Instance {
                        instance_alias: "neutral.a".to_owned(),
                    },
                    revision: 1,
                    drained: None,
                },
            )
        })]
    });
    let client = RuntimeClient::connect(lab_client_config_via(temp.path(), None)).expect("connect");

    actingcommand_lab_ops::scheduling::pause_for_recording(
        &client,
        "neutral.a",
        SchedulingPauseReason::new("lab-record-1").expect("the hold's reason"),
    )
    .expect("the hold's pause");
    drop(client);

    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    let [connection] = &record.connections[..] else {
        panic!("one connection");
    };
    assert_eq!(
        pause_mode_asked(connection),
        SchedulingPauseMode::QuietOrLift
    );
}
