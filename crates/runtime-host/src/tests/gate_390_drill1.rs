// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #390 DRILL-1 (forced compromise T-08; coordinator ruling 9 of wave 13), host gate 88:
// a person's drill of one physical instance's stuck-recovery ladder (`DrillRecoveryLadder`). The
// drill is recorded like a ladder a trigger started (its plan, one `recovery_rung_finished` per
// rung and its end) and every record of its own is tagged `drill`. It never acts on the device
// unless the request says so (`mode: device`); then it climbs under the same key, admission and
// cool-down as a real ladder. The instance here is discovery-bound to the fake emulator and has
// no startup package, so the plan skips return home and the application restart for their own
// reasons and leaves the emulator restart pending (contracts/emulator-control.md, "Drills").

use super::instance_workers::{phases, wait_until};
use super::*;
use crate::codes::HostCode;
use actingcommand_contract::codes::RuntimeContractCode;
use actingcommand_contract::{
    EmulatorInstanceAction, InstanceStuckRecovery, RecoveryDrillMode, RecoveryLadderDrill,
    RecoveryLadderOutcome, RecoveryLadderSuppression, RecoveryRung, RecoveryRungOutcome,
    RecoveryRungPlan, RecoveryRungSkipReason, RecoveryRungState, RecoveryTriggerStage,
    RecoveryTriggerStart, RequestId, RuntimeLifecyclePhase, RuntimePayload,
};

const ALIAS: &str = "drill.instance";

// A started host whose one physical instance `ALIAS` is discovery-bound to the fake emulator,
// with opens that pass a preparation's self-check and no startup package. `stuck_recovery`
// sets the instance's own switch; without it the section's `enabled` (true) applies.
fn drill_host(
    root: &TempDir,
    state: &Arc<FakeState>,
    instance: InstanceId,
    stuck_recovery: Option<bool>,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let mut host_config = config(root);
    if let Some(enabled) = stuck_recovery {
        host_config = host_config.with_stuck_recovery(BTreeMap::from([(
            ALIAS.to_owned(),
            InstanceStuckRecovery {
                enabled,
                cooldown_secs: 600,
            },
        )]));
    }
    let host = RuntimeHost::start(
        host_config,
        Arc::new(FakeProvider::one(ALIAS, instance, Arc::clone(state))),
    )
    .expect("runtime host with the fake emulator");
    bind_fake_emulator(&host, ALIAS, instance, state);
    host
}

// `actingctl ladder-drill` of `ALIAS` in `mode`: the request and its receipt.
fn drill(
    host: &RuntimeHost,
    mode: RecoveryDrillMode,
    connection: u64,
) -> (RuntimeRequest, RuntimeReceipt) {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: ALIAS.to_owned(),
            mode,
        },
    );
    let receipt = host
        .process_request_for_test(&request, ConnectionId::new(connection).expect("connection"))
        .expect("drill receipt");
    (request, receipt)
}

// The drill a completed receipt admitted.
#[track_caller]
fn admitted_drill(receipt: &RuntimeReceipt, mode: RecoveryDrillMode) -> RecoveryLadderDrill {
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_drill_recovery_ladder_completed",
        "{receipt:?}"
    );
    let Some(RuntimeResult::RecoveryLadderDrillQueued { drill }) = receipt.result() else {
        panic!("a drill's receipt names the drill: {receipt:?}");
    };
    assert_eq!(
        (drill.instance_alias.as_str(), drill.mode),
        (ALIAS, mode),
        "{receipt:?}"
    );
    drill.clone()
}

// The ladder phases recorded under `request_id`, each with its `drill` tag and severity, in
// ledger order.
fn drill_phases(
    host: &RuntimeHost,
    request_id: RequestId,
) -> Vec<(bool, EventSeverity, RuntimeLifecyclePhase)> {
    let events = host
        .query_persisted_events_for_test(EventQuery {
            request_id: Some(request_id),
            ..EventQuery::default()
        })
        .expect("the drill's records");
    events
        .iter()
        .filter_map(|event| match event.payload() {
            EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) => {
                Some((value.drill(), event.severity(), value.phase()))
            }
            _ => None,
        })
        .collect()
}

// Waits until the drill's `recovery_ladder_finished` is recorded and its key released.
fn wait_for_drill_end(host: &RuntimeHost, request_id: RequestId) {
    wait_until("the drill's end and its release", || {
        drill_phases(host, request_id).iter().any(|(_, _, phase)| {
            matches!(phase, RuntimeLifecyclePhase::RecoveryLadderFinished { .. })
        }) && host
            .instance_claims_for_test(ALIAS)
            .expect("instance claims")
            .0
            .is_none()
    });
}

fn skipped(rung: RecoveryRung, reason: RecoveryRungSkipReason) -> RecoveryRungPlan {
    RecoveryRungPlan {
        rung,
        state: RecoveryRungState::Skipped,
        reason: Some(reason),
    }
}

// The plan every drill of this instance records: return home and the application restart
// skipped for their own reasons, the emulator restart pending.
fn expected_plan() -> Vec<RecoveryRungPlan> {
    vec![
        skipped(
            RecoveryRung::ReturnHome,
            RecoveryRungSkipReason::NoRecoveryPackage,
        ),
        skipped(
            RecoveryRung::ApplicationRestart,
            RecoveryRungSkipReason::NoStartupPackage,
        ),
        RecoveryRungPlan {
            rung: RecoveryRung::EmulatorRestart,
            state: RecoveryRungState::Pending,
            reason: None,
        },
    ]
}

// Checks the drill's `recovery_ladder_started`: its trigger is a drill's and its plan is the
// instance's.
#[track_caller]
fn assert_drill_started(phase: &RuntimeLifecyclePhase) {
    let RuntimeLifecyclePhase::RecoveryLadderStarted { trigger, rungs, .. } = phase else {
        panic!("the drill starts with recovery_ladder_started: {phase:?}");
    };
    assert_eq!(trigger.stage, RecoveryTriggerStage::Drill, "{phase:?}");
    assert_eq!(
        trigger.failure_code.as_str(),
        HostCode::RecoveryLadderDrillRequested.as_str()
    );
    assert!(
        trigger.run_id.is_none()
            && trigger.task_id.is_none()
            && trigger.preparation.is_none()
            && trigger.rule.is_none()
            && trigger.matched.is_none(),
        "a drill's trigger names no run, task, preparation, rule or match: {phase:?}"
    );
    assert_eq!(
        trigger.start,
        RecoveryTriggerStart::Immediately,
        "{phase:?}"
    );
    assert_eq!(rungs, &expected_plan());
}

// Each finished rung with its result and the code of its outcome.
fn rungs_finished(
    phases: &[(bool, EventSeverity, RuntimeLifecyclePhase)],
) -> Vec<(RecoveryRung, RecoveryRungOutcome, Option<String>)> {
    phases
        .iter()
        .filter_map(|(_, _, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryRungFinished {
                rung,
                rung_result,
                outcome,
                ..
            } => Some((
                *rung,
                *rung_result,
                outcome
                    .as_ref()
                    .map(|outcome| outcome.code().as_str().to_owned()),
            )),
            _ => None,
        })
        .collect()
}

// Whether `phase` is one of a ladder's own phases, the ones a drill tags.
fn ladder_phase(phase: &RuntimeLifecyclePhase) -> bool {
    matches!(
        phase,
        RuntimeLifecyclePhase::RecoveryLadderStarted { .. }
            | RuntimeLifecyclePhase::RecoveryRungFinished { .. }
            | RuntimeLifecyclePhase::RecoveryInstanceStopped { .. }
            | RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
            | RuntimeLifecyclePhase::RecoveryLadderFinished { .. }
            | RuntimeLifecyclePhase::RecoveryLadderSuppressed { .. }
    )
}

fn emulator_actions(state: &FakeState) -> Vec<EmulatorInstanceAction> {
    state
        .emulator_actions
        .lock()
        .expect("fake emulator actions lock")
        .clone()
}

#[test]
fn drill1_a_plan_drill_is_recorded_like_a_ladder_tagged_drill_and_never_acts_on_the_device() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = drill_host(&root, &state, instance_id(), None);

    let (_, receipt) = drill(&host, RecoveryDrillMode::Plan, 401);
    let admitted = admitted_drill(&receipt, RecoveryDrillMode::Plan);
    wait_for_drill_end(&host, admitted.ladder_request_id);

    let phases = drill_phases(&host, admitted.ladder_request_id);
    // Only the ladder's own phases, each tagged drill and at info: no backend open, no
    // preparation, no Stop or Start.
    assert!(
        phases.iter().all(|(drill, severity, phase)| *drill
            && *severity == EventSeverity::Info
            && ladder_phase(phase)),
        "every record of the drill is a ladder phase tagged drill, at info: {phases:?}"
    );
    assert_drill_started(&phases.first().expect("the drill's records").2);
    assert_eq!(
        rungs_finished(&phases),
        vec![
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedNoRecoveryPackage
                        .as_str()
                        .to_owned()
                )
            ),
            (
                RecoveryRung::ApplicationRestart,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedNoStartupPackage
                        .as_str()
                        .to_owned()
                )
            ),
            (
                RecoveryRung::EmulatorRestart,
                RecoveryRungOutcome::Skipped,
                Some(HostCode::RecoveryRungSkippedDrillPlan.as_str().to_owned())
            ),
        ]
    );
    assert_eq!(
        phases.last().map(|(_, _, phase)| phase.clone()),
        Some(RuntimeLifecyclePhase::RecoveryLadderFinished {
            ladder_result: RecoveryLadderOutcome::Planned,
            rungs_tried: 0,
            outcome: None,
        })
    );
    assert!(
        !phases.iter().any(|(_, _, phase)| matches!(
            phase,
            RuntimeLifecyclePhase::RecoveryInstanceStopped { .. }
                | RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
                | RuntimeLifecyclePhase::InstancePreparationFinished { .. }
        )),
        "a plan drill stops nothing and prepares nothing: {phases:?}"
    );
    assert!(
        emulator_actions(&state).is_empty(),
        "a plan drill never drives the emulator"
    );
    // No record outside the drill's own is tagged.
    let tagged = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events")
        .iter()
        .filter(|event| {
            matches!(
                event.payload(),
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)) if value.drill()
            )
        })
        .count();
    assert_eq!(tagged, phases.len());
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn drill1_a_device_drill_climbs_its_rungs_on_the_instance_like_a_real_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = drill_host(&root, &state, instance_id(), None);

    let (_, receipt) = drill(&host, RecoveryDrillMode::Device, 402);
    let admitted = admitted_drill(&receipt, RecoveryDrillMode::Device);
    wait_for_drill_end(&host, admitted.ladder_request_id);

    let phases = drill_phases(&host, admitted.ladder_request_id);
    let own = phases
        .iter()
        .filter(|(_, _, phase)| ladder_phase(phase))
        .collect::<Vec<_>>();
    assert!(
        own.iter().all(|(drill, _, _)| *drill),
        "every ladder phase of the drill is tagged drill: {phases:?}"
    );
    assert!(
        phases
            .iter()
            .all(|(drill, _, phase)| ladder_phase(phase) || !*drill),
        "no other record is tagged drill: {phases:?}"
    );
    assert_drill_started(&own.first().expect("the drill's records").2);
    let rungs = rungs_finished(&phases);
    assert_eq!(
        rungs,
        vec![
            (
                RecoveryRung::ReturnHome,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedNoRecoveryPackage
                        .as_str()
                        .to_owned()
                )
            ),
            (
                RecoveryRung::ApplicationRestart,
                RecoveryRungOutcome::Skipped,
                Some(
                    HostCode::RecoveryRungSkippedNoStartupPackage
                        .as_str()
                        .to_owned()
                )
            ),
            (
                RecoveryRung::EmulatorRestart,
                RecoveryRungOutcome::EnvironmentReady,
                None
            ),
        ],
        "{phases:?}"
    );
    assert_eq!(
        own.iter()
            .filter(|(_, _, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryInstanceStopped { .. }
            ))
            .count(),
        1,
        "one confirmed Stop: {phases:?}"
    );
    assert_eq!(
        own.iter()
            .filter(|(_, _, phase)| matches!(
                phase,
                RuntimeLifecyclePhase::RecoveryEnvironmentReady { .. }
            ))
            .count(),
        1,
        "one ready environment: {phases:?}"
    );
    assert_eq!(
        own.last().map(|(_, _, phase)| phase.clone()),
        Some(RuntimeLifecyclePhase::RecoveryLadderFinished {
            ladder_result: RecoveryLadderOutcome::EnvironmentReady,
            rungs_tried: 1,
            outcome: None,
        })
    );
    // The rung's preparation is the ladder's own (stage recovery_preparation), not tagged.
    assert!(
        phases.iter().any(|(drill, _, phase)| !*drill
            && matches!(
                phase,
                RuntimeLifecyclePhase::InstancePreparationFinished {
                    stage: RecoveryTriggerStage::RecoveryPreparation,
                    ..
                }
            )),
        "{phases:?}"
    );
    assert_eq!(
        emulator_actions(&state),
        vec![EmulatorInstanceAction::Stop, EmulatorInstanceAction::Start],
        "the device drill drove the emulator's Stop and Start"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn drill1_a_plan_drill_leaves_the_cool_down_and_a_device_drill_opens_it_as_a_real_ladder() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = drill_host(&root, &state, instance_id(), None);

    // Two plan drills in a row: the first opened no cool-down window.
    for connection in [403, 404] {
        let (_, receipt) = drill(&host, RecoveryDrillMode::Plan, connection);
        let admitted = admitted_drill(&receipt, RecoveryDrillMode::Plan);
        wait_for_drill_end(&host, admitted.ladder_request_id);
    }
    // A device drill got the key (the default `cooldown_opens_on = admitted`): the window is
    // open, and the drills `trigger_defaults.respect_cooldown` holds are refused.
    let (_, receipt) = drill(&host, RecoveryDrillMode::Device, 405);
    let admitted = admitted_drill(&receipt, RecoveryDrillMode::Device);
    wait_for_drill_end(&host, admitted.ladder_request_id);

    let (request, refused) = drill(&host, RecoveryDrillMode::Plan, 406);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        HostCode::RecoveryLadderDrillCoolingDown.as_str(),
        "{refused:?}"
    );
    let next_eligible_unix_ms = refused
        .outcome()
        .value("next_eligible_unix_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("the refusal names the window's end: {refused:?}"));
    // The refused drill recorded its suppression, tagged drill, and the command's rejection.
    let refused_records = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        })
        .expect("the refused drill's records");
    let suppressions = phases(&refused_records)
        .into_iter()
        .filter_map(|(event, phase)| match (event.payload(), phase) {
            (
                EventPayload::Runtime(RuntimePayload::LifecycleObserved(value)),
                RuntimeLifecyclePhase::RecoveryLadderSuppressed {
                    reason,
                    until_unix_ms,
                    rule,
                },
            ) => Some((value.drill(), reason, until_unix_ms, rule)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        suppressions,
        vec![(
            true,
            RecoveryLadderSuppression::Cooldown,
            next_eligible_unix_ms,
            None
        )]
    );
    assert_eq!(
        refused_records
            .iter()
            .filter(|event| event.event_type() == EventType::CommandRejected)
            .count(),
        1
    );
    assert!(
        !phases(&refused_records)
            .iter()
            .any(|(_, phase)| matches!(phase, RuntimeLifecyclePhase::RecoveryLadderStarted { .. })),
        "a refused drill starts no ladder"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}

#[test]
fn drill1_a_drill_is_refused_when_stuck_recovery_is_off_or_it_comes_from_an_agent() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = drill_host(&root, &state, instance_id(), Some(false));

    let (request, refused) = drill(&host, RecoveryDrillMode::Device, 407);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        HostCode::RecoveryLadderDrillDisabled.as_str(),
        "{refused:?}"
    );
    let records = host
        .query_persisted_events_for_test(EventQuery {
            correlation_id: Some(request.correlation_id()),
            ..EventQuery::default()
        })
        .expect("the refused drill's records");
    assert!(
        phases(&records).is_empty(),
        "no ladder record for a drill of an instance whose ladder is off"
    );
    assert_eq!(
        records
            .iter()
            .filter(|event| event.event_type() == EventType::CommandRejected)
            .count(),
        1
    );
    assert!(emulator_actions(&state).is_empty());

    // Only a person (Ui) or the operator (Cli) may ask for a drill: no agent or scheduler path.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let agent = RuntimeRequest::new(
        ids.mint_request_id().expect("request id"),
        ids.mint_correlation_id().expect("correlation id"),
        None,
        EventActor::Agent,
        EventSource::Adapter,
        unix_ms_now().expect("wall clock"),
        RuntimeOperation::DrillRecoveryLadder {
            instance_alias: ALIAS.to_owned(),
            mode: RecoveryDrillMode::Plan,
        },
    );
    assert_eq!(
        agent.err().map(|error| error.code().as_str()),
        Some(RuntimeContractCode::InvalidEmulatorControlOrigin.as_str())
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");
}
