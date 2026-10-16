// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S4 (option 丙; host gate 81), model M-PKG v1.1 §4.5 S4-T6 and decision D7:
// a prerequisite package is named by its `package_id` and found by its path, and the package at
// that path must declare that `package_id` in its control. A package that declares another one
// is refused with `contained_task_prerequisite_mismatch` before any input: as a layer of a
// `linear_steps` package's chain, as the return-home package of a page-graph entry, and (train 16
// rulings, PKG-S4 item 3) as the return-home package of the recovery ladder's first rung.
// Workflow #373 NAV-3a (M-373 §4.3): the `package_id` and the path are a standard-pack
// registry's entry, no configuration entry.

use super::gate_373_nav3a::{
    AWAY, LEAD_STEPS, MAIN, MENU, OPEN, Pack, RETURN_HOME, StandardRoot, route, screens,
};
use super::gate_e4::exec_i6::{self, I6Pack, RESOLUTION, ROOT};
use super::instance_workers::{
    admit_ladder_run, ladder_end_code, ladder_finished, ladder_setup, phases, rung_outcomes,
    wait_for_ladder_end,
};
use super::*;
use actingcommand_contract::{
    RecoveryLadderOutcome, RecoveryRung, RecoveryRungOutcome, RuntimeLifecyclePhase, RuntimePayload,
};
use serde_json::json;

const MISMATCH: &str = "contained_task_prerequisite_mismatch";
// The `package_id` the package in the return-home entry's directory declares instead of the
// entry's own.
const OTHER: &str = "neutral.test.other";

// The entry facts of the run `events` belong to.
fn entry_facts(events: &[ProjectedEvent]) -> Vec<&TaskSemanticFact> {
    events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEntryPreflight)
        .filter_map(projected_task_semantic_fact)
        .collect()
}

// S4-T6, chain: the root's prerequisite package `neutral.i6.lead` is listed at a directory whose
// package declares `neutral.i6.other`; the run of the root is denied with the mismatch, with no
// input.
#[test]
fn s4_t6_a_chain_layer_whose_control_names_another_package_id_is_refused() {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let temp = TempDir::new().expect("tempdir");
    let packs_root = temp.path().join("packs");
    let mut registry = StandardRoot::new(temp.path());
    exec_i6::write_pack(
        &packs_root.join(ROOT),
        &I6Pack {
            package_id: ROOT,
            prerequisite: Some("neutral.i6.lead"),
            resolution: RESOLUTION,
        },
    );
    exec_i6::write_pack(
        &registry.list("neutral.i6.lead"),
        &I6Pack {
            package_id: "neutral.i6.other",
            prerequisite: None,
            resolution: RESOLUTION,
        },
    );
    registry.write_bundle();
    let host = RuntimeHost::start(
        config(&temp).with_standard_packs_root(Some(registry.root.clone())),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let request = client.request(RuntimeOperation::run_contained_task(
        "neutral.instance",
        client.ids.mint_holder_id().expect("holder"),
        ContainedTaskRequest::new(packs_root.join(ROOT).display().to_string())
            .expect("task request"),
    ));

    let receipt = client.send(&request);

    assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{receipt:?}");
    assert_eq!(receipt.outcome().code().as_str(), MISMATCH, "{receipt:?}");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    drop(client);
    host.close().expect("close runtime host");
}

// S4-T6, return home: a page-graph run whose request binds no recovery package takes the
// return-home package the registry names for its game and server, `neutral.test.return_home`;
// the package in that entry's directory declares `neutral.test.other`, so the entry fails closed
// with the mismatch, before the package is admitted as the run's recovery and before any input.
#[test]
fn s4_t6_a_configured_return_home_package_whose_control_names_another_package_id_is_refused() {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    screens(&state, vec![AWAY, MENU]);
    let temp = TempDir::new().expect("tempdir");
    let mut registry = StandardRoot::new(temp.path());
    route("neutral.test.other", "away", "menu", LEAD_STEPS, None)
        .write(&registry.list(RETURN_HOME));
    registry.return_home(RETURN_HOME);
    registry.write_bundle();
    let target = temp.path().join("target");
    Pack {
        package_id: MAIN,
        mode: "navigable_route",
        prerequisite: None,
        entry: json!(["menu"]),
        target: "done",
        operations: OPEN,
    }
    .write(&target);
    let host = RuntimeHost::start(
        config(&temp).with_standard_packs_root(Some(registry.root.clone())),
        Arc::new(FakeProvider::one(
            "neutral.instance",
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let correlation = client.ids.mint_correlation_id().expect("correlation");
    let correlation_id = *correlation.transport();
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(target.display().to_string()).expect("task request"),
        ),
    );

    let receipt = client.send(&request);

    assert_eq!(receipt.state(), RuntimeReceiptState::Failed, "{receipt:?}");
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    let facts = entry_facts(&events);
    assert!(
        facts.iter().any(|fact| matches!(
            fact,
            TaskSemanticFact::EntryTargetDisposition {
                disposition: TaskEntryTargetDisposition::FailClosed,
                outcome: Some(outcome),
            } if outcome.code().as_str() == MISMATCH
        )),
        "{facts:?}"
    );
    assert!(
        !facts
            .iter()
            .any(|fact| matches!(fact, TaskSemanticFact::EntryRecoveryPackageAdmitted { .. })),
        "{facts:?}"
    );
    drop(client);
    host.close().expect("close runtime host");
}

// S4-T6, the ladder's first rung (train 16 rulings, PKG-S4 item 3; PR #826 deviation 3): a
// scheduled run that binds no recovery package fails (its captures show no page) and starts the
// recovery ladder, whose first rung takes the return-home package the registry names for the
// failed package's game and server, `neutral.test.return_home` (M-373 §4.3); the package in that
// entry's directory declares `neutral.test.other`. The package found at the registry's entry must
// declare that entry's `package_id`, or it is refused before it runs with
// `contained_task_prerequisite_mismatch` (contracts/linear-steps.md, "Ladder and page-graph home
// entry"; contracts/emulator-control.md, "Rungs": an admission refusal keeps its code, and the
// rung's failure is recorded as `runtime.failed` with category `recovery_ladder`). So the rung is
// `failed` with that code and names no run, no run admits the package under either id, and
// nothing is input (the fixture's other runs see no page either, so they input nothing). The rung
// failed on its own, so the climb goes on (M387 §4.1: a rung's success ends it, every rung failed
// or skipped ends it `exhausted`; §4.4: rungs that failed on their own end it at Error;
// contracts/emulator-control.md, `rungs_tried` counts the rungs executed, not skipped): the
// fixture's application restart runs next and fails, its emulator restart is skipped (no
// emulator control), and the ladder ends `exhausted` with two rungs tried.
#[test]
fn gate_s4_t6_a_return_home_rung_whose_control_names_another_package_id_fails_and_climbs_on() {
    let temp = TempDir::new().expect("tempdir");
    let (host_config, _) = ladder_setup(temp.path());
    let request = ContainedTaskRequest::new(
        temp.path()
            .join("physical-scheduled-task.zip")
            .to_string_lossy()
            .into_owned(),
    )
    .expect("scheduled request without a recovery binding");
    let mut registry = StandardRoot::new(temp.path());
    route(OTHER, "away", "menu", LEAD_STEPS, None).write(&registry.list(RETURN_HOME));
    registry.return_home(RETURN_HOME);
    registry.write_bundle();
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let host = RuntimeHost::start(
        host_config.with_standard_packs_root(Some(registry.root.clone())),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    let context = admit_ladder_run(&host, &request);
    state.unknown_capture.store(true, Ordering::Release);

    host.run_scheduled_contained_task(&context, &request)
        .expect_err("the scheduled run fails");
    wait_for_ladder_end(&host, POLICY_INSTANCE_ALIAS);
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("ledger events");
    host.close().expect("close runtime host");

    let rungs = rung_outcomes(&events);
    assert_eq!(
        rungs.first(),
        Some(&(
            RecoveryRung::ReturnHome,
            RecoveryRungOutcome::Failed,
            Some(MISMATCH.to_owned())
        )),
        "{rungs:?}"
    );
    let first_rung = phases(&events)
        .into_iter()
        .find_map(|(_, phase)| match phase {
            RuntimeLifecyclePhase::RecoveryRungFinished { rung, run_id, .. } => {
                Some((rung, run_id))
            }
            _ => None,
        });
    assert_eq!(
        first_rung,
        Some((RecoveryRung::ReturnHome, None)),
        "the refused rung names no run"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Runtime(RuntimePayload::Failed(record))
                if record.detail().is_some_and(|detail| detail.category() == "recovery_ladder")
                    && record.outcome().code().as_str() == MISMATCH
        )),
        "the rung's refusal is recorded as a recovery_ladder failure"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event.payload(),
            EventPayload::Task(TaskPayload::Semantic(payload))
                if matches!(
                    payload.fact(),
                    TaskSemanticFact::PackageAdmitted { package_label, .. }
                        if package_label == RETURN_HOME || package_label == OTHER
                )
        )),
        "no run admitted the package of the return-home entry"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 0);
    assert!(
        matches!(
            rungs.get(1),
            Some((RecoveryRung::ApplicationRestart, result, _))
                if *result != RecoveryRungOutcome::Skipped
        ),
        "the climb goes on to the application restart: {rungs:?}"
    );
    let (end, ladder_result, rungs_tried) = ladder_finished(&events).expect("the ladder's end");
    assert_eq!(ladder_result, RecoveryLadderOutcome::Exhausted);
    assert_eq!(rungs_tried, 2);
    assert_eq!(ladder_end_code(end), Some("recovery_ladder_exhausted"));
    assert_eq!(end.severity(), EventSeverity::Error);
}
