// SPDX-License-Identifier: AGPL-3.0-only

// #480 PG-2c (0.12.73; register §1.472; the coordinator's ruling 8 on board #480, who sends what):
// the rescue module's own client (`SystemHost`'s Runtime, `SystemRuntime`), whose instance pauses
// and global pause are the hold of a deploy, a rollback, a `down`, a placement and an instance's
// addition, sends every pause with `pause_mode` `quiet_or_lift`, set in code (`HOLD_PAUSE_MODE`),
// never `stand` and never a parameter. On the runtime client's scripted Runtime (loopback, a
// temporary state root), which answers each pause as paused; each pause on a connection of its own.

use super::host::{Host, SystemHost};
use actingcommand_contract::{
    RuntimeOperation, RuntimeRequest, RuntimeResult, SchedulingPauseMode, SchedulingPauseReason,
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
