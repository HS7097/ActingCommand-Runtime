// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-0 (register §1.213; contracts/runtime-interface.md, "Workbench request
// kinds"): the player workbench's request kinds have their wire shapes and no behaviour yet.
// Each, and a ConfigureSettings or ApplyResourceTargets that names its new version member, is
// refused `not_implemented` (Denied) naming its operation, as LoadPack is before U-3, and has no
// effect: nothing is appended to the ledger and no instance's status changes.

use super::gate_383_ao4q::ledger_head;
use super::*;
use actingcommand_contract::{
    ConfigSettingChange, FactScope, PackScheduleEditRow, ResourceTargetEdit,
    RoutineSummaryGranularity, RuntimeInstanceStatus,
};
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
    let run = *client
        .ids
        .mint_request_id()
        .expect("request id")
        .transport();

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
            client.request(RuntimeOperation::StopInstanceRun {
                instance_alias: ALIAS.to_owned(),
                request_id: run,
                wait_ms: 1_000,
            }),
            "stop_instance_run",
        ),
        (
            client.request(RuntimeOperation::EditPackSchedule {
                scope: FactScope::Instance {
                    instance_id: ALIAS.to_owned(),
                },
                package_id: "package.example".to_owned(),
                rows: vec![PackScheduleEditRow {
                    from_unix_ms: 1,
                    until_unix_ms: None,
                    schedulable: false,
                }],
                expected_version: None,
            }),
            "edit_pack_schedule",
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
        (
            client.request(RuntimeOperation::RoutineSummary {
                instance: Some(ALIAS.to_owned()),
                granularity: RoutineSummaryGranularity::Hour,
                since_revision: None,
            }),
            "routine_summary",
        ),
        (
            client.request(RuntimeOperation::ConfigureSettings {
                changes: vec![ConfigSettingChange {
                    key: "scheduling.parallel.enabled".to_owned(),
                    value: Some(json!(true)),
                }],
                expected_version: Some(1),
            }),
            "configure_settings",
        ),
        (
            client.agent_request(RuntimeOperation::ApplyResourceTargets {
                document_json: "{}".to_owned(),
                expected_version: Some(1),
            }),
            "apply_resource_targets",
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
