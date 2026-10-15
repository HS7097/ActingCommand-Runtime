// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #336 PKG-S2 (option 丙), model M-PKG v1.1 §4.3 (host gate 68): a package is named by
// its `package_id`. A reading's source names that id, not the package's content (S2-T4), and a
// replayed request is the same package when its package, admitted again, has the recorded id,
// whatever changed in place (S2-T6).

use super::*;

// S2-T4: the same package changed in one file keeps its readings' `resource_bundle_hash`, the
// SHA-256 of `package:<package_id>`; another package has another one.
#[test]
fn s2_t4_a_reading_source_names_the_package_id_whatever_its_content() {
    let package_id = "fixture01.readings";
    let admitted_label = |bytes: &[u8]| {
        PreparedContainedTask::load(
            "fixture01.instance",
            bytes,
            &actingcommand_contract::PackageRef::from("0".repeat(64)),
        )
        .expect("the package is admitted")
        .package_label()
        .to_owned()
    };
    let before = explicit_home_contained_task_package(true, package_id, [0, 0, 255], [255, 0, 0]);
    let after = explicit_home_contained_task_package(true, package_id, [0, 0, 254], [255, 0, 0]);
    assert_ne!(before, after, "one file of the package changed");

    let source = crate::host::resource_reading_bundle_hash(&admitted_label(&before));
    assert_eq!(
        source,
        crate::host::resource_reading_bundle_hash(&admitted_label(&after))
    );
    assert_eq!(
        source,
        format!(
            "{:x}",
            Sha256::digest(format!("package:{package_id}").as_bytes())
        )
    );
    let other = explicit_home_contained_task_package(
        true,
        "fixture01.other-readings",
        [0, 0, 255],
        [255, 0, 0],
    );
    assert_ne!(
        source,
        crate::host::resource_reading_bundle_hash(&admitted_label(&other))
    );
}

// S2-T6: a request sent again after its package was changed in place is answered from its
// ledger as before; a request whose path now holds another package is refused
// `contained_task_request_package_reused`.
#[test]
fn s2_t6_a_replay_continues_after_an_in_place_change_and_refuses_another_package() {
    let root = TempDir::new().expect("tempdir");
    let package = root.path().join("neutral-task.zip");
    let bytes = neutral_contained_task_package(true);
    fs::write(&package, &bytes).expect("write package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    let host = host_with_state(&root, "neutral.instance", Arc::clone(&state));
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let request = runtime_request(
        &ids,
        RuntimeOperation::run_contained_task(
            "neutral.instance",
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package.display().to_string()).expect("task request"),
        ),
    );
    let connection = ConnectionId::new(93).expect("connection");
    let first = host
        .process_request_for_test(&request, connection)
        .expect("first contained task");

    // The same package (`neutral.semantic.task`), one file changed in place.
    let changed = neutral_contained_task_package(false);
    assert_ne!(changed, bytes);
    fs::write(&package, &changed).expect("change the package in place");
    let replayed = host
        .process_request_for_test(&request, connection)
        .expect("replayed contained task");
    assert_eq!(replayed, first);
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);

    // Another package at the same path.
    fs::write(
        &package,
        explicit_home_contained_task_package(true, "fixture01.other", [0, 0, 255], [255, 0, 0]),
    )
    .expect("replace the package");
    let refused = host
        .process_request_for_test(&request, connection)
        .expect("the replay is answered");
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(
        refused.outcome().code().as_str(),
        "contained_task_request_package_reused"
    );
    assert_eq!(state.input_count.load(Ordering::Acquire), 1);
    host.close().expect("close host");
}
