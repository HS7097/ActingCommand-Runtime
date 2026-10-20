// SPDX-License-Identifier: AGPL-3.0-only

//! #411 A47 (0.12.8; found in #411 A46): when the `application_restart` rung's
//! `application.stop` fails, the ladder keeps its own key and climbs to its next configured rung,
//! `emulator_restart`, exactly as the configured rungs say. Before, the stop's failed lease end
//! (`application_control`'s backend-failure cleanup) released the ladder's key, so the climb ended
//! at its next holding check with `recovery_ladder_key_lost` and never tried the emulator restart
//! that a hung Android needs. The A46 fence the failed stop makes does not stop the climb: the
//! instance waits for a person only when the ladder ends with the fence still held. Requirements
//! of the slice:
//!
//! - T1: the stop fails; the climb reaches `emulator_restart` on the ladder's own key (its Stop
//!   and Start run, its readiness passes, and that preparation lifts the fence), no rung meets
//!   `recovery_ladder_key_lost`, and the ladder releases its key once, at its end;
//! - T2: the stop fails and `emulator_restart` fails too (the Start's readiness wait times out);
//!   the climb is exhausted at Error, the instance stays fenced with the stop's failure as the
//!   cause and waits for a person, and the host is still up.
//!
//! The fixture is the ladder fixture on an instance with emulator control and a discovery
//! binding (`emulator_ladder_fixture`), whose application backend fails every command with a
//! fatal device error and no unconfirmed resource. Existing tests missed this because no ladder
//! test's application stop failed. Every wait returns as soon as what it waits for is recorded;
//! the bound only stops a broken run.

use super::gate_403::{fence_records, status_fence};
use super::instance_workers::{
    LadderRun, all_events, emulator_ladder_fixture, ladder_end_code, ladder_finished,
    ladder_releases, phases, rung_outcomes, wait_for_ladder_end,
};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    EmulatorInstanceAction, RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome,
    RuntimeLifecyclePhase,
};

/// The emulator ladder fixture whose instance's application backend fails every command; its
/// scheduled run has failed and its ladder has ended.
fn climbed(prepare: impl FnOnce(&FakeState)) -> LadderRun {
    let run = emulator_ladder_fixture(0, false);
    run.state.fail_application.store(true, Ordering::Release);
    prepare(&run.state);
    run.fail_scheduled_run();
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    run
}

/// The fixture instance's emulator actions, in order.
fn emulator_actions(state: &FakeState) -> Vec<EmulatorInstanceAction> {
    state
        .emulator_actions
        .lock()
        .expect("fake emulator actions")
        .clone()
}

/// The `application_restart` rung failed with the A46 fence its failed stop made.
fn assert_stop_failed(rungs: &[(RecoveryRung, RecoveryRungOutcome, Option<String>)]) {
    assert!(
        rungs.contains(&(
            RecoveryRung::ApplicationRestart,
            RecoveryRungOutcome::Failed,
            Some(HostCode::InstanceResourcesFenced.as_str().to_owned()),
        )),
        "the application stop failed and fenced the instance: {rungs:?}"
    );
}

/// T1: the stop fails and the climb goes on to `emulator_restart` on the ladder's own key: no
/// rung meets `recovery_ladder_key_lost`; the emulator rung runs its Stop and Start and its
/// readiness passes (`recovery_environment_ready`), and that passing preparation lifts the A46
/// fence; the ladder releases its key once, after its end; the failure stays recorded with its
/// code; the host is up.
#[test]
fn a47_t1_a_failed_application_stop_keeps_the_ladder_key_and_the_climb_reaches_emulator_restart() {
    // The emulator rung's readiness preparation passes its self-check.
    let run = climbed(|state| state.observed_open.store(true, Ordering::Release));
    let events = all_events(&run.host);
    let rungs = rung_outcomes(&events);

    assert!(
        rungs
            .iter()
            .all(|(_, _, code)| code.as_deref() != Some(HostCode::RecoveryLadderKeyLost.as_str())),
        "the ladder lost its key during the climb: {rungs:?}"
    );
    assert_stop_failed(&rungs);
    assert!(
        rungs
            .iter()
            .any(|(rung, outcome, _)| *rung == RecoveryRung::EmulatorRestart
                && *outcome != RecoveryRungOutcome::Skipped),
        "the climb reached emulator_restart: {rungs:?}"
    );
    assert_eq!(
        emulator_actions(&run.state),
        [EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start],
        "the emulator rung ran its Stop and Start"
    );
    assert!(
        phases(&events).iter().any(|(_, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
        )),
        "the emulator rung's readiness passed"
    );
    assert!(
        phases(&events)
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::ResourceFenceLifted { .. })),
        "the emulator rung's preparation lifted the fence"
    );
    assert_eq!(status_fence(&run.host, POLICY_INSTANCE_ALIAS), None);
    let (finished, _, _) = ladder_finished(&events).expect("the ladder's end");
    let releases = ladder_releases(&events, run.context.lease_token().lease_id());
    let [release] = releases.as_slice() else {
        panic!("the ladder releases its key once: {releases:#?}");
    };
    assert!(release.sequence() > finished.sequence());
    assert!(
        fence_records(&events)
            .iter()
            .any(|(_, outcome)| outcome.codes().iter().any(
                |code| code.as_str() == SharedCode::ApplicationBackendOperationFailed.as_str()
            )),
        "the stop's failure stays recorded with its code"
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

/// T2: the stop fails and `emulator_restart` fails too (its Start's readiness wait times out).
/// The climb ran every configured rung and is exhausted at Error; the instance stays fenced with
/// the stop's `application_backend_operation_failed` as the cause and waits for a person; the
/// host is still up.
#[test]
fn a47_t2_a_failed_emulator_restart_after_the_stop_exhausts_the_climb_and_the_fence_awaits_a_person()
 {
    let run = climbed(|state| state.fail_emulator_start.store(true, Ordering::Release));
    let events = all_events(&run.host);
    let rungs = rung_outcomes(&events);

    assert_eq!(
        rungs.last(),
        Some(&(
            RecoveryRung::EmulatorRestart,
            RecoveryRungOutcome::Failed,
            Some("emulator_control_wait_timeout".to_owned())
        )),
        "the emulator rung ran and failed on its own: {rungs:?}"
    );
    assert_stop_failed(&rungs);
    let (finished, outcome, _) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(outcome, RecoveryLadderOutcome::Exhausted);
    assert_eq!(ladder_end_code(finished), Some("recovery_ladder_exhausted"));
    assert_eq!(finished.severity(), EventSeverity::Error);
    let fence = status_fence(&run.host, POLICY_INSTANCE_ALIAS).expect("the instance stays fenced");
    assert!(fence.awaiting_person, "{fence:?}");
    assert_eq!(
        fence.cause.as_str(),
        SharedCode::ApplicationBackendOperationFailed.as_str()
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    assert!(!run.host.is_shutdown_requested().expect("shutdown state"));
    run.host.close().expect("close host");
}
