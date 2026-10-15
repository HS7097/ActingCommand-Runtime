// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S4 (option 丙; host gate 81), model M-PKG v1.1 §4.5 S4-T6 and decision D7:
// a configured prerequisite package is named by its `package_id` and found by its path, and the
// package at that path must declare that `package_id` in its control. A package that declares
// another one is refused with `contained_task_prerequisite_mismatch` before any input: as a
// layer of a `linear_steps` package's chain, and as the configured return-home package of a
// page-graph entry.

use super::gate_e4::exec_i6::{self, I6Pack, RESOLUTION, ROOT};
use super::*;
use actingcommand_contract::ContainedTaskRecoveryBinding;

const MISMATCH: &str = "contained_task_prerequisite_mismatch";

// The entry facts of the run `events` belong to.
fn entry_facts(events: &[ProjectedEvent]) -> Vec<&TaskSemanticFact> {
    events
        .iter()
        .filter(|event| event.event_type == EventType::TaskEntryPreflight)
        .filter_map(projected_task_semantic_fact)
        .collect()
}

// S4-T6, chain: the root's prerequisite package `neutral.i6.lead` is configured at a path whose
// package declares `neutral.i6.other`; the run of the root is denied with the mismatch, with no
// input.
#[test]
fn s4_t6_a_chain_layer_whose_control_names_another_package_id_is_refused() {
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    let temp = TempDir::new().expect("tempdir");
    let packs_root = temp.path().join("packs");
    exec_i6::write_pack(
        &packs_root.join(ROOT),
        &I6Pack {
            package_id: ROOT,
            prerequisite: Some("neutral.i6.lead"),
            resolution: RESOLUTION,
        },
    );
    exec_i6::write_pack(
        &packs_root.join("neutral.i6.lead"),
        &I6Pack {
            package_id: "neutral.i6.other",
            prerequisite: None,
            resolution: RESOLUTION,
        },
    );
    let binding =
        ContainedTaskRecoveryBinding::new(packs_root.join("neutral.i6.lead").display().to_string())
            .expect("prerequisite binding");
    let host = RuntimeHost::start(
        config(&temp)
            .with_prerequisite_packages(BTreeMap::from([("neutral.i6.lead".to_owned(), binding)])),
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

// S4-T6, configured return home: a page-graph run whose request binds no recovery package takes
// the return-home package configured for its game and server, `fixture01.return-home`; the
// package at that entry's path declares `fixture01.other`, so the entry fails closed with the
// mismatch, before the package is admitted as the run's recovery and before any input.
#[test]
fn s4_t6_a_configured_return_home_package_whose_control_names_another_package_id_is_refused() {
    let root = TempDir::new().expect("tempdir");
    let target =
        explicit_home_contained_task_package(true, "fixture01.target", [0, 0, 255], [255, 0, 0]);
    let recovery =
        explicit_home_contained_task_package(true, "fixture01.other", [0, 0, 255], [255, 0, 0]);
    let target_path = root.path().join("target.zip");
    let recovery_path = root.path().join("return-home.zip");
    fs::write(&target_path, &target).expect("write target package");
    fs::write(&recovery_path, &recovery).expect("write return-home package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let binding = ContainedTaskRecoveryBinding::new(recovery_path.display().to_string())
        .expect("return-home binding");
    let host = RuntimeHost::start(
        config(&root)
            .with_prerequisite_packages(BTreeMap::from([(
                "fixture01.return-home".to_owned(),
                binding,
            )]))
            .with_return_home_packages(BTreeMap::from([(
                ("fixture01".to_owned(), "test".to_owned()),
                "fixture01.return-home".to_owned(),
            )])),
        Arc::new(FakeProvider::one(
            "fixture01.instance",
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
            "fixture01.instance",
            client.ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(target_path.display().to_string()).expect("task request"),
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
