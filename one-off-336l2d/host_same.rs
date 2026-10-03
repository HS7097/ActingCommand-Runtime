// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #336 L2d evidence: the existing page-graph home entry
//! preflight with a request-bound recovery package (`task-run --recovery-package`), run on the
//! fake physical provider with the same packages and the same host configuration on the L2c tip
//! 89e57ae3 and on the product. Both hosts have `return_home_packages` configured for both games
//! used, except in the scenario without any binding, which shows the unconfigured path. Every
//! printed line starts with `SAME|`; the one-off evidence script compares the two runs after
//! removing identifiers, digests and timings.

use super::*;

struct SameCase {
    target: Vec<u8>,
    recovery: Option<Vec<u8>>,
    unused_binding: bool,
    transition_after_input: bool,
    transition_after_capture: usize,
    return_home_configured: bool,
}

/// The same host configuration on both trees (both have the L2c `return_home_packages`): a
/// configured return-home package for both games the packages use, whose binding names no
/// existing package, so a run that used it instead of the request binding would differ.
fn same_config(root: &TempDir, return_home_configured: bool) -> RuntimeHostConfig {
    if !return_home_configured {
        return config(root);
    }
    let mapped = "fixture01.l2d.return-home".to_owned();
    config(root)
        .with_prerequisite_packages(BTreeMap::from([(
            mapped.clone(),
            ContainedTaskRecoveryBinding::new(
                root.path().join("mapped-return-home.zip").display().to_string(),
                "8".repeat(64),
            )
            .expect("mapped binding"),
        )]))
        .with_return_home_packages(BTreeMap::from([
            (("fixture01".to_owned(), "test".to_owned()), mapped.clone()),
            (("neutral".to_owned(), "test".to_owned()), mapped),
        ]))
}

fn same_run(scenario: &str, case: SameCase) {
    let root = TempDir::new().expect("tempdir");
    let target_path = root.path().join("target.zip");
    fs::write(&target_path, &case.target).expect("write target package");
    let state = Arc::new(FakeState::default());
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(case.transition_after_input, Ordering::Release);
    state
        .transition_capture_after_capture
        .store(case.transition_after_capture, Ordering::Release);
    let host = RuntimeHost::start(
        same_config(&root, case.return_home_configured),
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
    let mut task_request = ContainedTaskRequest::new(
        target_path.display().to_string(),
        format!("{:x}", Sha256::digest(&case.target)),
    )
    .expect("target request");
    if let Some(recovery) = &case.recovery {
        let recovery_path = root.path().join("return-home.zip");
        fs::write(&recovery_path, recovery).expect("write recovery package");
        task_request = task_request
            .with_recovery(
                ContainedTaskRecoveryBinding::new(
                    recovery_path.display().to_string(),
                    format!("{:x}", Sha256::digest(recovery)),
                )
                .expect("binding"),
            )
            .expect("request with recovery");
    } else if case.unused_binding {
        task_request = task_request
            .with_recovery(
                ContainedTaskRecoveryBinding::new(
                    root.path().join("unused.zip").display().to_string(),
                    "9".repeat(64),
                )
                .expect("binding"),
            )
            .expect("request with unused binding");
    }
    let request = client.request_with_correlation(
        correlation,
        RuntimeOperation::run_contained_task(
            "fixture01.instance",
            client.ids.mint_holder_id().expect("holder"),
            task_request,
        ),
    );
    let receipt = client.send(&request);
    println!(
        "SAME|{scenario}|receipt|{}",
        serde_json::to_string(&receipt).expect("receipt JSON")
    );
    println!(
        "SAME|{scenario}|counts|input_count={} capture_count={} return_home_configured={}",
        state.input_count.load(Ordering::Acquire),
        state.capture_count.load(Ordering::Acquire),
        case.return_home_configured
    );
    let events = projected_events(
        &mut client,
        EventQuery {
            correlation_id: Some(correlation_id),
            ..EventQuery::default()
        },
    );
    for event in &events {
        let fact = projected_task_semantic_fact(event)
            .map(|fact| serde_json::to_value(fact).expect("fact JSON"))
            .unwrap_or(serde_json::Value::Null);
        println!(
            "SAME|{scenario}|event|{}",
            serde_json::json!({"event_type": format!("{:?}", event.event_type), "fact": fact})
        );
    }
    drop(client);
    host.close().expect("close host");
}

#[test]
fn oneoff_336l2d_request_bound_home_entry_unchanged() {
    let home = |id: &str, home: [u8; 3], other: [u8; 3]| {
        explicit_home_contained_task_package(true, id, home, other)
    };
    same_run(
        "bound recovery then target",
        SameCase {
            target: home("fixture01.target", [0, 0, 255], [255, 0, 0]),
            recovery: Some(home("fixture01.return-home", [0, 0, 255], [255, 0, 0])),
            unused_binding: false,
            transition_after_input: true,
            transition_after_capture: 0,
            return_home_configured: true,
        },
    );
    same_run(
        "recovery failure",
        SameCase {
            target: home("fixture01.target", [0, 0, 255], [255, 0, 0]),
            recovery: Some(home("fixture01.return-home", [255, 255, 0], [0, 0, 255])),
            unused_binding: false,
            transition_after_input: true,
            transition_after_capture: 0,
            return_home_configured: true,
        },
    );
    same_run(
        "persistent non-home",
        SameCase {
            target: home("fixture01.target", [255, 255, 0], [255, 0, 0]),
            recovery: Some(home("fixture01.return-home", [0, 0, 255], [255, 0, 0])),
            unused_binding: false,
            transition_after_input: true,
            transition_after_capture: 0,
            return_home_configured: true,
        },
    );
    same_run(
        "already home",
        SameCase {
            target: home("fixture01.target", [0, 0, 255], [255, 0, 0]),
            recovery: None,
            unused_binding: false,
            transition_after_input: false,
            transition_after_capture: 1,
            return_home_configured: true,
        },
    );
    same_run(
        "non-home start ignores binding",
        SameCase {
            target: neutral_non_home_start_contained_task_package(true),
            recovery: None,
            unused_binding: true,
            transition_after_input: true,
            transition_after_capture: 0,
            return_home_configured: true,
        },
    );
    same_run(
        "binding missing, nothing configured",
        SameCase {
            target: home("fixture01.target", [0, 0, 255], [255, 0, 0]),
            recovery: None,
            unused_binding: false,
            transition_after_input: false,
            transition_after_capture: 0,
            return_home_configured: false,
        },
    );
}
