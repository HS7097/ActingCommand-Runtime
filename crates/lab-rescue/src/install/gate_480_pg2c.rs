// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2c (0.12.73; register §1.472; the coordinator's ruling 8 on board #480, who sends what):
// the rescue module's own client (`SystemHost`'s Runtime, `SystemRuntime`), whose instance pauses
// and global pause are the hold of a deploy, a rollback, a `down`, a placement and an instance's
// addition, sends every pause with `pause_mode` `quiet_or_lift`, set in code (`HOLD_PAUSE_MODE`),
// never `stand` and never a parameter. On the runtime client's scripted Runtime (loopback, a
// temporary state root), which answers each pause as paused; each pause on a connection of its own.

use super::fixture::FakeHost;
use super::hold::{self, PauseIntent};
use super::host::{Host, SystemHost};
use super::runtime::PauseFailed;
use crate::codes::InstallLocation;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    InstancePauseStage, PauseObstacle, RuntimeOperation, RuntimeReceipt, RuntimeRequest,
    RuntimeResult, SchedulingDrainSummary, SchedulingPauseMode, SchedulingPauseReason,
    SchedulingPauseScope,
};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use serde_json::json;
use std::collections::BTreeMap;
use tempfile::TempDir;

/// The instance the hold pauses.
const ALIAS: &str = "neutral.a";

/// The result a Runtime answers a pause of `scope` with, as the host builds it: an instance pause
/// carries the runs it drained (none here), a global pause none (fix round 2: the contract refuses a
/// receipt of an instance pause without them, `invalid_scheduling_pause_receipt`).
fn paused_result(scope: &SchedulingPauseScope) -> RuntimeResult {
    RuntimeResult::SchedulingPaused {
        scope: scope.clone(),
        revision: 1,
        drained: matches!(scope, SchedulingPauseScope::Instance { .. }).then_some(
            SchedulingDrainSummary {
                finished: 0,
                cancelled: 0,
            },
        ),
    }
}

/// The scripted answer to a pause: paused, the scope it asked for.
fn paused(request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::PauseScheduling { scope, .. } = request.operation() else {
        panic!("a pause: {:?}", request.operation());
    };
    ScriptedAnswer::completed(request, paused_result(scope))
}

#[test]
fn pg2c_every_pause_of_the_rescue_modules_hold_is_quiet_or_lift() {
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(temp.path(), |_| {
        vec![
            ScriptedConnection::new().request(paused),
            ScriptedConnection::new().request(paused),
        ]
    });
    let host = SystemHost::of(Some(Initiator::LabRescue));
    let reason = SchedulingPauseReason::new("lab-deploy-1").expect("a pause reason");

    host.runtime()
        .pause(temp.path(), ALIAS, &reason, 60_000)
        .expect("the instance pause of the hold");
    host.runtime()
        .pause_global(temp.path(), &reason, 60_000)
        .expect("the global pause of the hold");

    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the two");
    let modes = record
        .connections
        .iter()
        .flat_map(|connection| connection.requests.iter())
        .filter_map(|request| match request.operation() {
            RuntimeOperation::PauseScheduling { pause_mode, .. } => Some(*pause_mode),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            SchedulingPauseMode::QuietOrLift,
            SchedulingPauseMode::QuietOrLift
        ]
    );
}

/// Fix round 1 (item 9): a completed pause of `request`'s instance whose receipt carries
/// `scheduling_pause_standing` (`paused`, `lease`) under its success code, as a Runtime answers a
/// person's pause that is not yet quiet, and never a tool's hold.
fn standing(request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::PauseScheduling { scope, .. } = request.operation() else {
        panic!("a pause: {:?}", request.operation());
    };
    let receipt = RuntimeReceipt::scheduling_paused(
        request,
        paused_result(scope),
        Some(
            Outcome::relayed("scheduling_pause_standing")
                .pause_stage(InstancePauseStage::Paused)
                .in_the_way([PauseObstacle::Lease]),
        ),
    )
    .expect("the scripted receipt");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// #480 PG-2c fix round 1 (item 9): a hold's instance pause answered Completed with an informing
/// cause did not get a quiet instance: the hold's pause fails, as every other failure of the hold's
/// pause; nothing is taken as drained. Fix round 2 (item 3): its failure is its own code,
/// `scheduling_hold_not_quiet`, whose values are what the receipt said (the cause's code, its stage
/// and what was in the way) and nothing else, and it is told apart as a pause that stands.
#[test]
fn pg2c_fix1_a_holds_pause_answered_with_a_cause_fails() {
    let temp = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(temp.path(), |_| {
        vec![ScriptedConnection::new().request(standing)]
    });
    let host = SystemHost::of(Some(Initiator::LabRescue));
    let reason = SchedulingPauseReason::new("lab-deploy-1").expect("a pause reason");

    let failure = host
        .runtime()
        .pause(temp.path(), ALIAS, &reason, 60_000)
        .expect_err("a pause that did not leave its instance quiet");

    let PauseFailed::Stands(failure) = failure else {
        panic!("a pause that stands: {failure:?}");
    };
    assert_eq!(
        failure.code().as_str(),
        "scheduling_hold_not_quiet",
        "{failure:?}"
    );
    assert_eq!(
        serde_json::Value::Object(failure.values().clone()),
        json!({
            "receipt_cause": "scheduling_pause_standing",
            "pause_stage": "paused",
            "in_the_way": ["lease"],
        }),
        "{failure:?}"
    );
    assert!(failure.causes().is_empty(), "{failure:?}");
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
}

/// #480 PG-2c fix round 2 (item 2), through the hold's pause step (`hold::run_pause`) on the fake
/// Runtime: of two instances paused at once, the Runtime answers `neutral.a`'s pause before the
/// instance is quiet (it stands, `fixture::answered_unquiet`) and pauses `neutral.b`. The step fails
/// at once with the true cause: `runtime_stop_failed` for `neutral.a`, caused by
/// `scheduling_hold_not_quiet` with what the receipt said; `neutral.a`'s pause is not sent again
/// (sent again it would meet `scheduling_already_paused`), and `neutral.b`'s drain is recorded.
#[test]
fn pg2c_fix2_a_pause_answered_unquiet_fails_the_pause_step_at_once_with_its_cause() {
    let temp = TempDir::new().expect("tempdir");
    let host = FakeHost::new(&temp.path().join("work"));
    host.runtime.boot(&["neutral.a", "neutral.b"]);
    *host.runtime.unquiet_pause.borrow_mut() = vec!["neutral.a".to_owned()];
    let reason = SchedulingPauseReason::new("lab-down-1").expect("a pause reason");
    let intent = PauseIntent {
        reason: reason.clone(),
        instances: vec!["neutral.a".to_owned(), "neutral.b".to_owned()],
        known: BTreeMap::new(),
        drain_timeout_ms: 60_000,
    };

    let Err(refused) = hold::run_pause(
        &host,
        temp.path(),
        InstallLocation::Stop,
        &intent,
        &BTreeMap::new(),
        BTreeMap::new(),
        60_000,
        true,
    ) else {
        panic!("the pause step fails");
    };

    let refusal = &refused.refusal;
    assert_eq!(
        refusal.code().as_str(),
        "install_command_refused",
        "{refusal:?}"
    );
    assert_eq!(
        refusal.value("install_refusal"),
        Some(&json!("runtime_stop_failed")),
        "{refusal:?}"
    );
    assert_eq!(
        refusal.value("alias"),
        Some(&json!("neutral.a")),
        "{refusal:?}"
    );
    let [cause] = refusal.causes() else {
        panic!("one cause: {refusal:?}");
    };
    assert_eq!(
        cause.outcome().code().as_str(),
        "scheduling_hold_not_quiet",
        "{refusal:?}"
    );
    assert_eq!(
        cause.outcome().value("receipt_cause"),
        Some(&json!("scheduling_pause_standing")),
        "{refusal:?}"
    );
    let pauses_of_a = host
        .runtime
        .calls
        .borrow()
        .iter()
        .filter(|call| call.starts_with("pause neutral.a "))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        pauses_of_a,
        [format!("pause neutral.a {reason} not quiet")],
        "not sent again"
    );
    assert!(refused.drained.contains_key("neutral.b"), "the drain of b");
    assert_eq!(
        host.runtime.pause_of("neutral.a").as_deref(),
        Some(reason.as_str()),
        "the pause answered before it was quiet stands"
    );
}
