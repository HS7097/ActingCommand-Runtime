// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-0 (register §1.213; contracts/runtime-interface.md, "Workbench request
// kinds"): the player workbench's request kinds have their wire shapes and no behaviour yet.
// Each is refused `not_implemented` (Denied) naming its operation, as LoadPack is before U-3,
// and has no effect: nothing is appended to the ledger and no instance's status changes.
// Workflow #429 R-4 (register §1.239, since 0.12.36) gives `ConfigureSettings.expected_version`
// its behaviour, so that case left this test (`gate_387_d1b`, `r4_t*`); so does `RoutineSummary`
// with R-12 (register §1.264, since 0.12.38; `gate_429_r12`), `StopInstanceRun` with R-6
// (register §1.302; `gate_429_r6`), and `EditPackSchedule` with R-8 (register §1.308;
// `gate_429_r8`), which also gives R-0c's `PackScheduleView` its behaviour, so this file's test of
// that read left with it; and `ApplyResourceTargets.expected_version` with Workflow #455 T1-R
// (register §1.351, since 0.12.60; `gate_455_t1r`).

use super::gate_383_ao4q::ledger_head;
use super::*;
use actingcommand_contract::{ResourceTargetEdit, RuntimeInstanceStatus};
use serde_json::json;

const ALIAS: &str = "node.a";

fn instances(reader: &mut TestClient) -> Vec<RuntimeInstanceStatus> {
    let request = reader.request(RuntimeOperation::Status);
    let receipt = reader.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("the status: {receipt:?}");
    };
    status.instances().to_vec()
}

#[test]
fn r0_each_workbench_kind_without_behaviour_answers_not_implemented_with_no_effect() {
    let root = TempDir::new().expect("tempdir");
    let host = host_with_state(&root, ALIAS, Arc::new(FakeState::default()));
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);
    let holder_id = *client.ids.mint_holder_id().expect("holder id").transport();

    let status_before = instances(&mut reader);
    assert!(
        status_before
            .iter()
            .all(|instance| instance.activity().is_none()),
        "no kind of activity is filled before its slice: {status_before:?}"
    );
    let head_before = ledger_head(&mut reader);

    let cases = [
        (
            client.request(RuntimeOperation::RunInstanceTask {
                instance_alias: ALIAS.to_owned(),
                package_id: "package.example".to_owned(),
                holder_id,
                response_deadline_ms: None,
            }),
            "run_instance_task",
        ),
        (
            client.request(RuntimeOperation::EditResourceTarget {
                instance_alias: ALIAS.to_owned(),
                expected_version: 1,
                op: ResourceTargetEdit::Delete {
                    id: "target.a".to_owned(),
                },
            }),
            "edit_resource_target",
        ),
    ];
    for (request, operation) in cases {
        let receipt = client.send(&request);
        assert_eq!(receipt.state(), RuntimeReceiptState::Denied, "{operation}");
        assert_eq!(
            receipt.outcome().code().as_str(),
            "not_implemented",
            "{operation}"
        );
        assert_eq!(
            receipt.outcome().value("operation"),
            Some(&json!(operation)),
            "{operation}"
        );
        assert!(receipt.result().is_none(), "{operation}");
        assert!(receipt.terminal().is_none(), "{operation}");
    }

    assert_eq!(ledger_head(&mut reader), head_before);
    assert_eq!(instances(&mut reader), status_before);
    drop(client);
    drop(reader);
    host.close().expect("close host");
}
