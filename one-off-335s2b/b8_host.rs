// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted): Workflow #335 S2b B8, the host path on the PR head. The neutral
//! fixture catalog with a valuation on its pool; a v2 policy through the formal entry; the
//! inventory published; one policy cycle; the inventory changed before admission; a restart and
//! the same document again.

use super::*;

fn one_off_document() -> String {
    serde_json::json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": POLICY_INSTANCE_ALIAS,
        "valid_until_unix_ms": POLICY_NOW_UNIX_MS + 86_400_000,
        "targets": [{
            "id": "primary-floor",
            "resource": "fixture-pool-a",
            "condition": {"kind": "at_least", "amount": 100},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    })
    .to_string()
}

fn one_off_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    let mut pools: serde_json::Value =
        serde_json::from_slice(&sources.pools.bytes).expect("fixture pools");
    pools["pools"][0]["valuation"] = serde_json::json!({
        "name": "Primary", "unit": "unit", "scale": 1, "base_weight_milli": 100,
        "gap": {"rule": "shortfall_linear", "weight_milli": 1000}
    });
    sources.pools.bytes = serde_json::to_vec_pretty(&pools).expect("pools bytes");
    sources
}

fn one_off_inventory(current: i64, observed_at: u64, snapshot: &str) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        },
        "resource.primary",
        ContractFactValue::Integer(current),
        snapshot,
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + 60_000);
    record
}

fn one_off_apply(client: &mut TestClient) -> RuntimeReceipt {
    let request = client.agent_request(RuntimeOperation::ApplyResourceTargets {
        document_json: one_off_document(),
    });
    client.send(&request)
}

fn one_off_applied(receipt: &RuntimeReceipt) -> String {
    match receipt.result() {
        Some(RuntimeResult::ResourceTargetsApplied { applied }) => {
            serde_json::to_string(applied).expect("applied JSON")
        }
        other => panic!("not applied: state={:?} result={other:?}", receipt.state()),
    }
}

#[test]
fn one_off_335s2b_b8_formal_entry_cycle_admission_restart() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let registered = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS,
        POLICY_NOW_UNIX_MS,
    ));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            registered,
            Arc::clone(&state),
        )),
    )
    .expect("runtime host");
    host.activate_policy_catalog(&one_off_sources())
        .expect("activate catalog with valuation");
    let mut client = TestClient::connect(&host);

    // 1. The formal entry stores the v2 policy.
    let first = one_off_apply(&mut client);
    println!(
        "B8|apply|state={:?}|applied={}",
        first.state(),
        one_off_applied(&first)
    );

    // 2. The inventory: 40 of at least 100.
    clock.advance(1_000);
    host.publish_fact(one_off_inventory(
        40,
        POLICY_NOW_UNIX_MS + 1_000,
        "snapshot:primary-40",
    ))
    .expect("publish inventory");
    clock.advance(1_000);
    let evaluated_at = POLICY_NOW_UNIX_MS + 2_000;
    let cycle = host
        .evaluate_policy_cycle_with_test_inputs(
            &policy_facts(),
            &policy_resources(),
            EvaluationTime {
                unix_ms: evaluated_at,
                monotonic_ms: evaluated_at,
            },
            7,
            PolicyTrigger::FactsChanged,
        )
        .expect("policy cycle");
    let evaluation = cycle.evaluation.expect("policy evaluation");
    let intent = evaluation
        .dispatch_intents
        .first()
        .unwrap_or_else(|| panic!("dispatch intent: {evaluation:#?}"))
        .clone();
    let chain = evaluation
        .reason_chains
        .iter()
        .find(|chain| chain.id == intent.reason_chain_id)
        .expect("reason chain")
        .clone();
    println!(
        "B8|intent|task={}|fact_snapshot_id={}|facts_fresh_until={:?}|next_wake={:?}",
        intent.task_id,
        intent.fact_snapshot_id,
        intent.prerequisites.facts_fresh_until_unix_ms,
        evaluation.next_wake_unix_ms
    );
    for reason in &chain.reasons {
        println!("B8|chain|{}|{}", reason.code, reason.detail);
    }

    // 3. The inventory changes before admission: the intent is stale.
    record_policy_approval(&host, &intent);
    clock.advance(1_000);
    host.publish_fact(one_off_inventory(
        55,
        POLICY_NOW_UNIX_MS + 3_000,
        "snapshot:primary-55",
    ))
    .expect("publish changed inventory");
    let admission = host.admit_policy_dispatch(&intent, &chain, &policy_context(&host, &intent));
    match admission {
        Ok(_) => println!("B8|admission_after_inventory_change|admitted"),
        Err(error) => println!(
            "B8|admission_after_inventory_change|code={}|operation={}",
            error.code(),
            error.operation()
        ),
    }
    drop(client);
    host.close().expect("close host");

    // 4. A restart, then the same document again.
    let clock = Arc::new(ManualRuntimeClock::new(
        POLICY_NOW_UNIX_MS + 10_000,
        POLICY_NOW_UNIX_MS + 10_000,
    ));
    let reopened = RuntimeHost::start(
        config(&root).with_runtime_clock(clock),
        Arc::new(FakeProvider::one(POLICY_INSTANCE_ALIAS, registered, state)),
    )
    .expect("reopened runtime host");
    let mut client = TestClient::connect(&reopened);
    let again = one_off_apply(&mut client);
    println!(
        "B8|reapply_after_restart|state={:?}|applied={}",
        again.state(),
        one_off_applied(&again)
    );
    assert!(reopened.fatal_error().expect("health").is_none());
    drop(client);
    reopened.close().expect("close reopened host");
}
