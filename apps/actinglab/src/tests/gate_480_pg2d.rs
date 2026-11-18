// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2d (0.12.74; register §1.481; coordinator work order
// `rd-1118/brief-480-pg2d-pause-records-resume-cause.md`, item 3 and its test 4): on the runtime
// client's scripted Runtime (loopback, a temporary state root), the Lab's `resume` row answers the
// receipt's cause: a resume of a person's pause that was not quiet yet, answered by the Runtime
// with `scheduling_resume_without_reconnect` under its success code, answers
// `runtime_scheduling_resumed` caused by that outcome, exit 0, on the command line's form and on
// the form MCP's `ac_resume` sends (`expect`); an ordinary resume answers no cause. The row's
// parameters are those it had. No clock is read.

use super::*;
use crate::cli_module::{self, words};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    IdentifierIssuer, OwnerEpoch, RuntimeControlPlaneStatus, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeRequest, RuntimeResult, SchedulingPauseScope,
    SchedulingResumeCaptureCheck, SchedulingResumeSelfCheck, SchedulingResumeTouchCheck,
};
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::core_ops::{self, Caller};
use actingcommand_lab_ops::front::{Connection, Front};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_lab_ops::table::{ParamForm, ParamType};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use serde_json::{Value, json};

/// The scripted Runtime's one instance.
const ALIAS: &str = "neutral.a";

/// The status the row reads to resolve the instance it names: the one instance [`ALIAS`].
fn status(
    owner_epoch: OwnerEpoch,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let instance_id = *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_instance_id()
            .expect("instance id")
            .transport();
        let instance =
            RuntimeInstanceStatus::new(ALIAS, instance_id, false, 0, false, false, false)
                .expect("instance status");
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, vec![instance]).expect("scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

/// The result a Runtime answers a resume of `scope` with: an instance resume carries a self-check
/// (here that of a resume that ran no reconnect: both sides not ok, no values, no failure code), a
/// global resume none.
fn resumed(scope: &SchedulingPauseScope) -> RuntimeResult {
    let selfcheck = matches!(scope, SchedulingPauseScope::Instance { .. }).then_some(
        SchedulingResumeSelfCheck {
            capture: SchedulingResumeCaptureCheck {
                ok: false,
                width: None,
                height: None,
                capture_backend: None,
            },
            touch: SchedulingResumeTouchCheck {
                ok: false,
                backend: None,
                max_x: None,
                max_y: None,
                invasive: false,
            },
            failure_code: None,
        },
    );
    RuntimeResult::SchedulingResumed {
        scope: scope.clone(),
        revision: 2,
        selfcheck,
    }
}

/// The completed receipt of `request`, a resume; with `without_reconnect` its outcome carries
/// `scheduling_resume_without_reconnect` under its success code, as a Runtime of 0.12.73 answers a
/// resume of a person's pause that was not quiet yet.
fn answered(request: &RuntimeRequest, without_reconnect: bool) -> ScriptedAnswer {
    let RuntimeOperation::ResumeScheduling { scope, .. } = request.operation() else {
        panic!("a resume: {:?}", request.operation());
    };
    let cause = without_reconnect.then(|| Outcome::relayed("scheduling_resume_without_reconnect"));
    let receipt = RuntimeReceipt::scheduling_resumed(request, resumed(scope), cause)
        .expect("the scripted receipt");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// The row's answer to the words `line`, on a connection of its own.
fn answer(line: &[String]) -> Exit {
    let table = cli_module::table().expect("the operation table");
    let call = words::parse(table, line).expect("the line");
    let operation = table.operation(&call.name).expect("the row resume");
    core_ops::answer(
        operation,
        &call.params,
        &Caller {
            connection: Connection::Connect(Front::Actinglab(Initiator::LabCli)),
            ..cli_module::core_ops::caller()
        },
    )
}

/// The codes of the causes under the line's outcome, depth first.
fn cause_codes(line: &Value) -> Vec<String> {
    let mut codes = Vec::new();
    let mut causes = line["outcome"]["causes"].as_array().cloned();
    while let Some(links) = causes {
        let Some(first) = links.first() else {
            break;
        };
        codes.push(first["code"].as_str().unwrap_or_default().to_owned());
        causes = first["causes"].as_array().cloned();
    }
    codes
}

#[test]
fn pg2d_the_resume_row_answers_the_receipts_cause() {
    let _guard = env_lock();
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let mut epoch = None;
    let runtime = ScriptedRuntime::start(&state, |owner_epoch| {
        epoch = Some(owner_epoch);
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch))
                .request(|request| answered(request, true)),
            ScriptedConnection::new()
                .request(status(owner_epoch))
                .request(|request| answered(request, true)),
            ScriptedConnection::new().request(|request| answered(request, false)),
        ]
    });
    let _state_root = use_runtime_state_root(&state);
    let expect = json!({
        "owner_epoch": epoch.expect("the scripted Runtime's owner epoch"),
        "revision": 1,
    })
    .to_string();
    let words_of = |line: &[&str]| {
        line.iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };

    // The command line's form, and the form `ac_resume` sends (the pause it read as `expect`): a
    // resume the Runtime answered with `scheduling_resume_without_reconnect`.
    for line in [
        words_of(&["resume", ALIAS]),
        words_of(&["resume", ALIAS, "--expect", expect.as_str()]),
    ] {
        let exit = answer(&line);
        let printed: Value = exit.line.expect("the row's line");
        assert_eq!(exit.code, 0, "{line:?}: {printed}");
        assert_eq!(
            printed["outcome"]["code"],
            json!("runtime_scheduling_resumed"),
            "{line:?}: {printed}"
        );
        assert_eq!(
            cause_codes(&printed),
            [
                "request_resume_scheduling_completed",
                "scheduling_resume_without_reconnect"
            ],
            "{line:?}: {printed}"
        );
    }

    // An ordinary resume (here of every instance): no cause.
    let exit = answer(&words_of(&["resume"]));
    let printed: Value = exit.line.expect("the row's line");
    assert_eq!(exit.code, 0, "{printed}");
    assert_eq!(
        printed["outcome"]["code"],
        json!("runtime_scheduling_resumed"),
        "{printed}"
    );
    assert!(cause_codes(&printed).is_empty(), "{printed}");

    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the three"
    );
    assert_eq!(record.connections.len(), 3);
}

// The row's parameters are those it had: `instance` (positional, an instance, not required) and
// `expect` (an option, JSON, not required but in MCP), nothing more.
#[test]
fn pg2d_the_resume_row_keeps_its_parameters() {
    let table = cli_module::table().expect("the operation table");
    let operation = table.operation("resume").expect("the row resume");
    let params = operation
        .params
        .iter()
        .map(|param| {
            (
                param.name.as_str(),
                param.form,
                param.value_type.clone(),
                param.required,
                param.repeat,
                param.default.clone(),
                param.required_in.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        params,
        [
            (
                "instance",
                ParamForm::Positional,
                ParamType::Instance,
                false,
                false,
                None,
                None
            ),
            (
                "expect",
                ParamForm::Option,
                ParamType::Json,
                false,
                false,
                None,
                Some(vec!["mcp".to_owned()])
            ),
        ]
    );
}
