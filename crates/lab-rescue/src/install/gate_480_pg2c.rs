// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2c (0.12.73; register §1.472; the coordinator's ruling 8 on board #480, who sends what):
// the rescue module's own client (`SystemHost`'s Runtime, `SystemRuntime`), whose instance pauses
// and global pause are the hold of a deploy, a rollback, a `down`, a placement and an instance's
// addition, sends every pause with `pause_mode` `quiet_or_lift`, set in code (`HOLD_PAUSE_MODE`),
// never `stand` and never a parameter. On the runtime client's scripted Runtime (loopback, a
// temporary state root), which answers each pause as paused; each pause on a connection of its own.

use super::host::{Host, SystemHost};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    InstancePauseStage, PauseObstacle, RuntimeOperation, RuntimeReceipt, RuntimeRequest,
    RuntimeResult, SchedulingPauseMode, SchedulingPauseReason,
};
use actingcommand_lab_ops::initiator::Initiator;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use tempfile::TempDir;

/// The instance the hold pauses.
const ALIAS: &str = "neutral.a";

/// The scripted answer to a pause: paused, the scope it asked for.
fn paused(request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::PauseScheduling { scope, .. } = request.operation() else {
        panic!("a pause: {:?}", request.operation());
    };
    ScriptedAnswer::completed(
        request,
        RuntimeResult::SchedulingPaused {
            scope: scope.clone(),
            revision: 1,
            drained: None,
        },
    )
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
        RuntimeResult::SchedulingPaused {
            scope: scope.clone(),
            revision: 1,
            drained: None,
        },
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
/// cause did not get a quiet instance: the hold's pause fails, `runtime_result_unexpected` with the
/// receipt's outcome (its cause `scheduling_pause_standing`) behind it, as every other failure of
/// the hold's pause; nothing is taken as drained.
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

    assert_eq!(
        failure.code().as_str(),
        "runtime_result_unexpected",
        "{failure:?}"
    );
    assert!(
        failure
            .codes()
            .iter()
            .any(|code| code.as_str() == "scheduling_pause_standing"),
        "the receipt's cause is behind it: {failure:?}"
    );
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "a connection beyond the one");
}
