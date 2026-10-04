// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted), Workflow #338 Rb evidence on the runtime-host test fixtures: the
//! read-only ResourceTargetView on the fixture catalog (contracts/scheduling/examples/catalog-a,
//! two instances) before and after a v2 policy and an inventory fact, with the ledger position
//! around the views, and the host's conditional resume (global and instance scope). Copied into
//! the runtime-host test module by the one-off workflow only; every printed line starts `RB|`.

use super::*;
use actingcommand_contract::{SchedulingPauseExpectation, SchedulingPauseScope};

fn rb_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("evidence JSON")
}

fn rb_ledger_position(client: &mut TestClient) -> u64 {
    let request = client.request(RuntimeOperation::QueryEvents {
        query: EventQuery::default(),
        profile: ProjectionProfile::Concise,
        page: RuntimeEventQueryPageRequest::new(1, None).expect("page request"),
    });
    let receipt = client.send(&request);
    match receipt.result() {
        Some(RuntimeResult::EventPage { page }) => page.snapshot_ledger_position(),
        other => panic!("event page expected: {other:?}"),
    }
}

fn rb_view(client: &mut TestClient, alias: &str, label: &str) -> RuntimeReceipt {
    let request = client.request(RuntimeOperation::ResourceTargetView {
        instance_alias: alias.to_owned(),
    });
    let receipt = client.send(&request);
    println!("RB|R5|VIEW|{label}|{alias}|{}", rb_json(&receipt));
    receipt
}

fn rb_refusal(receipt: &RuntimeReceipt) -> String {
    let error = receipt.error_projection();
    format!(
        "state={:?}|runtime_code={:?}|host_code={:?}",
        receipt.state(),
        error.map(|error| error.code),
        error.and_then(|error| error.host_code()),
    )
}

#[test]
fn oneoff_338rb_resource_target_view_on_the_fixture_catalog() {
    let root = TempDir::new().expect("state root");
    let host = RuntimeHost::start(
        config(&root).with_policy_inputs(PolicyInputSnapshot::new(
            pending_policy_facts(),
            pending_policy_resources(),
        )),
        Arc::new(FakeProvider::from_entries([
            (
                POLICY_INSTANCE_ALIAS.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
            (
                POLICY_INSTANCE_ALIAS_B.to_owned(),
                instance_id(),
                Arc::new(FakeState::default()),
            ),
        ])),
    )
    .expect("two-instance host");
    let mut client = TestClient::connect(&host);
    let no_catalog = rb_view(&mut client, POLICY_INSTANCE_ALIAS, "no_catalog");
    println!("RB|R5|NO_CATALOG|{}", rb_refusal(&no_catalog));
    host.activate_policy_catalog(&pending_policy_sources(1))
        .expect("catalog activation");

    let before = rb_ledger_position(&mut client);
    for alias in [POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B] {
        let receipt = rb_view(&mut client, alias, "no_policy");
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
        assert!(receipt.terminal().is_none());
    }
    let unknown = rb_view(&mut client, "instance-unknown", "unknown_alias");
    println!("RB|R5|UNKNOWN_ALIAS|{}", rb_refusal(&unknown));
    let after = rb_ledger_position(&mut client);
    println!(
        "RB|R5|LEDGER|views_without_policy|before={before}|after={after}|unchanged={}",
        before == after
    );
    assert_eq!(before, after);

    let now = unix_ms_now().expect("wall clock");
    let document = serde_json::json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": POLICY_INSTANCE_ALIAS,
        "valid_until_unix_ms": now + 3_600_000,
        "targets": [{
            "id": "target-a",
            "resource": "fixture-pool-a",
            "condition": {"kind": "at_least", "amount": 50},
            "scale": 10,
            "importance_milli": 500,
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    });
    let apply = client.agent_request(RuntimeOperation::ApplyResourceTargets {
        document_json: document.to_string(),
    });
    let applied = client.send(&apply);
    println!("RB|R5|APPLY|{}", rb_json(&applied));
    assert_eq!(applied.state(), RuntimeReceiptState::Completed);

    let before = rb_ledger_position(&mut client);
    let with_policy = rb_view(&mut client, POLICY_INSTANCE_ALIAS, "policy_no_inventory");
    let Some(RuntimeResult::ResourceTargetView { view }) = with_policy.result() else {
        panic!("view expected");
    };
    let active = view.active.as_ref().expect("active policy");
    let Some(RuntimeResult::ResourceTargetsApplied { applied: applied_result }) = applied.result()
    else {
        panic!("applied expected");
    };
    assert_eq!(active.version, applied_result.version);
    assert_eq!(active.event_id, applied_result.event_id);
    assert_eq!(active.policy_sha256, applied_result.policy_sha256);
    let after = rb_ledger_position(&mut client);
    println!(
        "RB|R5|LEDGER|view_with_policy|before={before}|after={after}|unchanged={}",
        before == after
    );
    assert_eq!(before, after);

    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        ContractFactValue::Integer(37),
        "snapshot:oneoff-338rb",
        Vec::new(),
    );
    let now = unix_ms_now().expect("wall clock");
    record.observed_at_unix_ms = now;
    record.expires_at_unix_ms = Some(now + 60_000);
    record.confidence_milli = 1_000;
    match host.publish_fact(record) {
        Ok(event_id) => println!("RB|R5|INVENTORY_PUBLISHED|{}", rb_json(&event_id)),
        Err(error) => println!(
            "RB|R5|INVENTORY_PUBLISH_FAILED|code={}|operation={}",
            error.code(),
            error.operation()
        ),
    }
    let before = rb_ledger_position(&mut client);
    for alias in [POLICY_INSTANCE_ALIAS, POLICY_INSTANCE_ALIAS_B] {
        let receipt = rb_view(&mut client, alias, "policy_and_inventory");
        assert_eq!(receipt.state(), RuntimeReceiptState::Completed);
    }
    let after = rb_ledger_position(&mut client);
    println!(
        "RB|R5|LEDGER|views_with_inventory|before={before}|after={after}|unchanged={}",
        before == after
    );
    assert_eq!(before, after);
    drop(client);
    host.close().expect("close host");
}

#[test]
fn oneoff_338rb_conditional_resume_on_the_host() {
    let root = TempDir::new().expect("state root");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("host");
    let mut client = TestClient::connect(&host);
    let epoch = host.runtime_info().owner_epoch();
    let other_epoch = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_owner_epoch()
        .expect("other epoch")
        .transport();
    for scope in [
        SchedulingPauseScope::Global,
        SchedulingPauseScope::Instance {
            instance_alias: POLICY_INSTANCE_ALIAS.to_owned(),
        },
    ] {
        let label = match &scope {
            SchedulingPauseScope::Global => "global",
            SchedulingPauseScope::Instance { .. } => "instance",
        };
        let before = rb_ledger_position(&mut client);
        let pause = client.request(RuntimeOperation::PauseScheduling {
            scope: scope.clone(),
            reason_code: "oneoff.rb".to_owned(),
            drain_timeout_ms: 1_000,
        });
        let paused = client.send(&pause);
        println!("RB|R4|{label}|PAUSE|{}", rb_json(&paused));
        let Some(RuntimeResult::SchedulingPaused { revision, .. }) = paused.result() else {
            println!("RB|R4|{label}|PAUSE_REFUSED|{}", rb_refusal(&paused));
            assert_eq!(label, "instance", "the global pause must succeed");
            continue;
        };
        let revision = *revision;
        let resume = |client: &TestClient, expected: Option<SchedulingPauseExpectation>| {
            client.request(RuntimeOperation::ResumeScheduling {
                scope: scope.clone(),
                expected,
            })
        };
        let wrong_revision = client.send(&resume(
            &client,
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision: revision + 1,
            }),
        ));
        println!(
            "RB|R4|{label}|REVISION_MISMATCH|expected_revision={}|{}",
            revision + 1,
            rb_refusal(&wrong_revision)
        );
        assert_eq!(
            wrong_revision
                .error_projection()
                .and_then(|error| error.host_code()),
            Some("scheduling_pause_revision_mismatch")
        );
        let wrong_epoch = client.send(&resume(
            &client,
            Some(SchedulingPauseExpectation {
                owner_epoch: other_epoch,
                revision,
            }),
        ));
        println!(
            "RB|R4|{label}|EPOCH_MISMATCH|{}",
            rb_refusal(&wrong_epoch)
        );
        assert_eq!(
            wrong_epoch
                .error_projection()
                .and_then(|error| error.host_code()),
            Some("scheduling_pause_owner_epoch_mismatch")
        );
        let matched = client.send(&resume(
            &client,
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision,
            }),
        ));
        println!("RB|R4|{label}|MATCH|{}", rb_json(&matched));
        assert_eq!(matched.state(), RuntimeReceiptState::Completed);
        let stale = client.send(&resume(
            &client,
            Some(SchedulingPauseExpectation {
                owner_epoch: epoch,
                revision,
            }),
        ));
        println!(
            "RB|R4|{label}|STALE_AFTER_RESUME|{}",
            rb_refusal(&stale)
        );
        let after = rb_ledger_position(&mut client);
        println!("RB|R4|{label}|LEDGER|before={before}|after={after}");
    }
    let unconditional_pause = client.send(&client.request(RuntimeOperation::PauseScheduling {
        scope: SchedulingPauseScope::Global,
        reason_code: "oneoff.rb".to_owned(),
        drain_timeout_ms: 1_000,
    }));
    assert_eq!(unconditional_pause.state(), RuntimeReceiptState::Completed);
    let unconditional = client.send(&client.request(RuntimeOperation::ResumeScheduling {
        scope: SchedulingPauseScope::Global,
        expected: None,
    }));
    println!("RB|R4|global|UNCONDITIONAL|{}", rb_json(&unconditional));
    assert_eq!(unconditional.state(), RuntimeReceiptState::Completed);
    drop(client);
    host.close().expect("close host");
}
