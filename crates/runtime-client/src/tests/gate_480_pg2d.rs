// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2d (0.12.74; register §1.481; coordinator work order
// `rd-1118/brief-480-pg2d-pause-records-resume-cause.md`, item 3; `contracts/scheduling-pause.md`,
// "Resume"): the resume calls return the receipt's outcome beside the result, in the shape of
// `pause_scheduling_as` (`SchedulingResumeAnswer`), with no wire change. Over `ScriptedRuntime`, one
// connection each, no sleep and no device:
// - `resume_scheduling_as` and `resume_scheduling_expected_as` answer a resume of a person's pause
//   that was not quiet yet with `scheduling_resume_without_reconnect` as the success code's one
//   cause, and an ordinary resume with no cause;
// - `resume_scheduling` and `resume_scheduling_expected` answer the result alone, as before, the
//   cause or not.

use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedRuntime};
use crate::{RuntimeClient, RuntimeClientConfig, SchedulingResumeAnswer};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    EventActor, EventSource, OwnerEpoch, RuntimeOperation, RuntimeReceipt, RuntimeRequest,
    RuntimeResult, SchedulingPauseExpectation, SchedulingPauseScope, SchedulingResumeCaptureCheck,
    SchedulingResumeSelfCheck, SchedulingResumeTouchCheck,
};
use std::path::Path;
use tempfile::TempDir;

const INSTANCE: &str = "neutral.a";
/// The success code of a completed resume.
const RESUMED: &str = "request_resume_scheduling_completed";
/// The cause of a resume that ran no reconnect.
const WITHOUT_RECONNECT: &str = "scheduling_resume_without_reconnect";

fn connect(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        state_root,
        crate::RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("runtime client")
}

fn scope() -> SchedulingPauseScope {
    SchedulingPauseScope::Instance {
        instance_alias: INSTANCE.to_owned(),
    }
}

fn expectation(owner_epoch: Option<OwnerEpoch>) -> SchedulingPauseExpectation {
    SchedulingPauseExpectation {
        owner_epoch: owner_epoch.expect("the scripted Runtime's owner epoch"),
        revision: 1,
    }
}

/// The result of an instance resume of `scope`, its self-check that of a resume that ran no
/// reconnect, as the host builds it: both sides not ok, no values, no failure code.
fn resumed(scope: &SchedulingPauseScope) -> RuntimeResult {
    RuntimeResult::SchedulingResumed {
        scope: scope.clone(),
        revision: 2,
        selfcheck: Some(SchedulingResumeSelfCheck {
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
        }),
    }
}

/// The completed receipt of `request`, a resume; with `without_reconnect` its outcome carries
/// `scheduling_resume_without_reconnect` under the success code, as the Runtime answers a resume of
/// a person's pause that was not quiet yet (`RuntimeReceipt::scheduling_resumed`).
fn answered(request: &RuntimeRequest, without_reconnect: bool) -> ScriptedAnswer {
    let RuntimeOperation::ResumeScheduling { scope, .. } = request.operation() else {
        panic!("a resume: {:?}", request.operation());
    };
    let cause = without_reconnect.then(|| Outcome::relayed(WITHOUT_RECONNECT));
    let receipt = RuntimeReceipt::scheduling_resumed(request, resumed(scope), cause)
        .expect("the scripted receipt");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// The codes of the causes under `answer`'s success code.
#[track_caller]
fn causes(answer: &SchedulingResumeAnswer) -> Vec<String> {
    assert_eq!(answer.outcome.code().as_str(), RESUMED, "{answer:?}");
    answer
        .outcome
        .causes()
        .iter()
        .map(|link| link.outcome().code().as_str().to_owned())
        .collect()
}

#[test]
fn pg2d_the_new_resume_calls_answer_the_receipts_cause() {
    let root = TempDir::new().expect("tempdir");
    let mut epoch = None;
    let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
        epoch = Some(owner_epoch);
        vec![
            ScriptedConnection::new()
                .request(|request| answered(request, true))
                .request(|request| answered(request, true))
                .request(|request| answered(request, false))
                .request(|request| answered(request, false)),
        ]
    });
    let client = connect(root.path());

    let unquiet = client
        .resume_scheduling_as(scope())
        .expect("the resume of a pause not yet quiet");
    let unquiet_expected = client
        .resume_scheduling_expected_as(scope(), expectation(epoch))
        .expect("the conditional resume of a pause not yet quiet");
    let ordinary = client
        .resume_scheduling_as(scope())
        .expect("an ordinary resume");
    let ordinary_expected = client
        .resume_scheduling_expected_as(scope(), expectation(epoch))
        .expect("an ordinary conditional resume");
    drop(client);

    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    for answer in [&unquiet, &unquiet_expected] {
        assert_eq!(answer.result, resumed(&scope()), "{answer:?}");
        assert_eq!(causes(answer), [WITHOUT_RECONNECT], "{answer:?}");
    }
    for answer in [&ordinary, &ordinary_expected] {
        assert_eq!(answer.result, resumed(&scope()), "{answer:?}");
        assert!(causes(answer).is_empty(), "{answer:?}");
    }
}

#[test]
fn pg2d_the_existing_resume_calls_answer_the_result_alone() {
    let root = TempDir::new().expect("tempdir");
    let mut epoch = None;
    let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
        epoch = Some(owner_epoch);
        vec![
            ScriptedConnection::new()
                .request(|request| answered(request, true))
                .request(|request| answered(request, true))
                .request(|request| answered(request, false)),
        ]
    });
    let client = connect(root.path());

    let unquiet = client
        .resume_scheduling(scope())
        .expect("the resume of a pause not yet quiet");
    let unquiet_expected = client
        .resume_scheduling_expected(scope(), expectation(epoch))
        .expect("the conditional resume of a pause not yet quiet");
    let ordinary = client
        .resume_scheduling(scope())
        .expect("an ordinary resume");
    drop(client);

    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
    for result in [&unquiet, &unquiet_expected, &ordinary] {
        assert_eq!(*result, resumed(&scope()));
    }
}
