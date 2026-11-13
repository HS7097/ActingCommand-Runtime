// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 G-R2a gate tests, policy half (0.12.60; register §1.357; coordinator work order
// `rd-1114/brief-455-gr2a-v3-store.md`; contracts/resource-targets.md, "Document
// `actingcommand.resource-targets.v3`", "Storage" and "Resource weights (v2)"): the goal
// document's value store. A v3 record's targets weigh as a v2 record's; a target whose own
// lifetime has passed weighs nothing and says what an expired v2 policy says; the v3 reader
// refuses each mistake at its field; a record at its bounds is 241 rows and is evaluated; v1 and
// v2 documents write and weigh as before; a version the reader does not know runs the instance on
// base scheduling. The catalog, the v1 and v2 documents, the readings and the round are
// gate_ss1's (tasks on one instance, due at NOW; pool A: step 10, base 500, gap weight 100; pool
// B: step 1, base 7, no gap). Every expected value comes from the contract.

use std::collections::BTreeMap;

use actingcommand_contract::{
    FactScalar as ContractFactScalar, Initiator, RESOURCE_TARGETS_FACT_KEY,
    ResourceTargetCondition, ResourceTargetConditionState, ResourceTargetPendingReason,
    ResourceTargetsRejectionReason as Reason, RuntimeChannel,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::gate_ss1::{
    A, POOL_A, POOL_A_KEY, POOL_B_KEY, TASK_A, TASK_B, V1, V2, catalog, decision, inputs, items,
    pool, reading, reason_detail, round, task, task_a, task_b, term, time, v1_target, v2_target,
};
use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, instance};
use crate::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, ComposedResourceTargets,
    EvaluationResources, FactValue, HostResourceSnapshot, ObservedFact, PolicyEvaluation,
    ResourceTargetsError, ScopeSelector, TargetSetter, check_resource_targets, compile_catalog,
    evaluate, parse_resource_targets, project_fact_pools,
};

const V3: &str = "actingcommand.resource-targets.v3";
const EXPIRED: &str = "resource_target_policy_expired";
const TARGETS: &str = "resource_targets";

/// Who sets the rows the documents of these tests write: a Lab connection that declared
/// `Lab-CLI`, at `at`.
fn setter(at: u64) -> TargetSetter {
    TargetSetter {
        set_unix_ms: at,
        channel: RuntimeChannel::Lab,
        initiator: Some(Initiator::LabCli),
    }
}

/// The record the formal entry composes for `document` over the stored rows `stored`, the
/// document checked against `catalog` at `at`, as `ApplyResourceTargets` does with a request that
/// names its channel.
fn compose(
    catalog: &CompiledCatalog,
    document: &Value,
    stored: Option<&[BTreeMap<String, ContractFactScalar>]>,
    at: u64,
) -> Result<ComposedResourceTargets, ResourceTargetsError> {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))?;
    check_resource_targets(&parsed, catalog, &inputs(Vec::new(), Vec::new()), time(at))?
        .compose(stored, Some(setter(at)))
        .map(|composed| composed.expect("with a setter every document composes"))
}

/// Instance A's `session.resource_targets` record of `composed`, observed at `at`, with the
/// record's own expiry.
fn record(composed: &ComposedResourceTargets, at: u64) -> ObservedFact {
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&composed.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: at,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1000,
    }
}

/// gate_ss1's round, at `at` instead of NOW.
fn round_at(catalog: &CompiledCatalog, facts: Vec<ObservedFact>, at: u64) -> PolicyEvaluation {
    let facts = inputs(facts, Vec::new());
    let resources = project_fact_pools(
        catalog,
        &facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: instance(A, &[]).host_id,
                cpu_available_milli: 1_000,
                gpu_available_milli: 1_000,
                io_available_milli: 1_000,
                host_responsiveness_basis_points: 10_000,
                third_party_pressure_basis_points: 0,
                heavy_dispatch_limit: 1,
                active_heavy_dispatches: 0,
                heavy_threshold_milli: crate::DEFAULT_HEAVY_THRESHOLD_MILLI,
            }],
        },
    );
    evaluate(catalog, &facts, &resources, time(at), 11)
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
}

/// `goal-a`: pool A, at least 100, its step and importance the pool's valuation.
fn goal_a() -> Value {
    json!({
        "id": "goal-a",
        "resource": POOL_A,
        "condition": {"kind": "at_least", "amount": 100},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// `goal-b`: pool B, at least 50, with an importance of its own (pool B declares no gap).
fn goal_b() -> Value {
    json!({
        "id": "goal-b",
        "resource": "pool_b",
        "condition": {"kind": "at_least", "amount": 50},
        "importance_milli": 300,
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// `goal-b` as an override of task B that supersedes its offset.
fn goal_b_override() -> Value {
    let mut target = goal_b();
    target["apply"] =
        json!({"mode": "override", "weight": "score_stage", "manual_offset": "supersede"});
    target["tasks"] = json!([TASK_B]);
    target
}

/// `target` with its own lifetime.
fn lasting(mut target: Value, valid_until: u64) -> Value {
    target["valid_until_unix_ms"] = json!(valid_until);
    target
}

/// A v3 document of instance A with `targets` and no `entries`.
fn v3_document(targets: Vec<Value>) -> Value {
    json!({"schema_version": V3, "instance": A, "targets": targets})
}

/// Each candidate's own wake (never serialized), by task.
fn wakes(evaluation: &PolicyEvaluation) -> Vec<String> {
    evaluation
        .decisions
        .iter()
        .map(|decision| {
            format!(
                "{}@{:?}={:?}",
                decision.task_id, decision.instance_id, decision.next_wake_unix_ms
            )
        })
        .collect()
}

/// The earlier of `wake` and `lapse`.
fn earliest(wake: Option<u64>, lapse: u64) -> Option<u64> {
    Some(wake.map_or(lapse, |wake| wake.min(lapse)))
}

fn text(value: &str) -> ContractFactScalar {
    ContractFactScalar::String(value.to_owned())
}

fn integer(value: u64) -> ContractFactScalar {
    ContractFactScalar::Integer(i64::try_from(value).expect("a fixture integer"))
}

fn row(fields: &[(&str, ContractFactScalar)]) -> BTreeMap<String, ContractFactScalar> {
    fields
        .iter()
        .map(|(field, value)| ((*field).to_owned(), value.clone()))
        .collect()
}

/// `sha256:<hex>` of a canonical text written out by hand.
fn identity(canonical: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

// v3 equals v2. The same two targets (goal-a, and goal-b as an override of task B) as a v2 record
// and as a v3 record whose targets each carry the v2 document's lifetime: while the lifetime runs
// and after it has passed, every candidate's decision, score, reasons, published scheduling facts
// (`policy_dispatch_parameters`), dispatch intent and reason chain and the evaluation's next wake
// are equal (the whole evaluation, as JSON), and so is each decision's own wake; the one
// difference is the record's identity, which the reasons name (`policy=<sha>`), so it is replaced
// before comparing. The records do weigh: task A's term is base plus shortfall (20 × (500 + 500)
// / 10) while active and the base 1000 after expiry.
#[test]
fn gr2a_t1_a_v3_record_with_a_v2_records_targets_weighs_byte_for_byte_as_the_v2_record() {
    let catalog = catalog(&[task_a(), task_b()]);
    let checked_at = NOW - HOUR_MS;
    for (label, valid_until, task_a_term) in
        [("active", NOW + DAY_MS, 2000), ("expired", NOW - 1, 1000)]
    {
        let v2 = compose(
            &catalog,
            &json!({
                "schema_version": V2,
                "instance": A,
                "valid_until_unix_ms": valid_until,
                "targets": [goal_a(), goal_b_override()]
            }),
            None,
            checked_at,
        )
        .unwrap_or_else(|error| panic!("{label}: the v2 document applies: {error:?}"));
        let v3 = compose(
            &catalog,
            &v3_document(vec![
                lasting(goal_a(), valid_until),
                lasting(goal_b_override(), valid_until),
            ]),
            None,
            checked_at,
        )
        .unwrap_or_else(|error| panic!("{label}: the v3 document applies: {error:?}"));
        assert_ne!(v2.policy_sha256, v3.policy_sha256, "{label}: two records");
        let evaluate_with = |composed: &ComposedResourceTargets| {
            round(
                &catalog,
                vec![
                    reading(POOL_A_KEY, 50),
                    reading(POOL_B_KEY, 10),
                    record(composed, checked_at),
                ],
                Vec::new(),
            )
        };
        let (under_v2, under_v3) = (evaluate_with(&v2), evaluate_with(&v3));
        let whole = |evaluation: &PolicyEvaluation| {
            serde_json::to_string(evaluation).expect("the evaluation encodes")
        };
        assert_eq!(
            whole(&under_v3).replace(&v3.policy_sha256, &v2.policy_sha256),
            whole(&under_v2),
            "{label}: the v3 record weighs as the v2 record"
        );
        assert_eq!(wakes(&under_v3), wakes(&under_v2), "{label}: each wake");
        assert_eq!(term(&under_v3, TASK_A), task_a_term, "{label}");
    }
}

// Per-target validity. goal-a lasts until `first`, goal-b until `second`, later. Before `first`
// both weigh and every candidate, and the evaluation, wakes at `first` + 1, the earlier expiry (or
// earlier, when it would without a goal). After `first` goal-a weighs nothing: task A keeps only
// the base weight of pool A, names no goal-a item, and, as task B does, carries
// `resource_target_policy_expired` with goal-a's lifetime, the detail an expired v2 record of
// goal-a alone gives with its own identity; goal-b weighs as before (its item and pool B's weight
// unchanged) and the candidates wake at `second` + 1.
#[test]
fn gr2a_t2_a_lapsed_target_weighs_nothing_and_says_expired_while_the_other_weighs_on() {
    let catalog = catalog(&[task_a(), task_b()]);
    let (first, second) = (NOW + 30 * 60_000, NOW + 2 * HOUR_MS);
    let readings = || vec![reading(POOL_A_KEY, 50), reading(POOL_B_KEY, 10)];
    let v3 = compose(
        &catalog,
        &v3_document(vec![lasting(goal_a(), first), lasting(goal_b(), second)]),
        None,
        NOW,
    )
    .unwrap_or_else(|error| panic!("the v3 document applies: {error:?}"));
    let with_record = |composed: &ComposedResourceTargets, at: u64| {
        let mut facts = readings();
        facts.push(record(composed, NOW));
        round_at(&catalog, facts, at)
    };

    let before = with_record(&v3, NOW);
    let baseline = round_at(&catalog, readings(), NOW);
    assert_eq!(
        reason_detail(&before, TASK_A, TARGETS),
        "goal-a@pool_a:applied current=50 at_least=100 gap=50 step=10 importance=100 gap_weight=500"
    );
    let both = reason_detail(&before, TASK_B, TARGETS).to_owned();
    assert!(
        both.starts_with("goal-a@pool_a:applied ") && both.contains("; goal-b@pool_b:applied "),
        "{both}"
    );
    for task_id in [TASK_A, TASK_B] {
        assert_eq!(
            decision(&before, task_id).next_wake_unix_ms,
            earliest(decision(&baseline, task_id).next_wake_unix_ms, first + 1),
            "{task_id} before {first}"
        );
    }
    assert_eq!(
        before.next_wake_unix_ms,
        earliest(baseline.next_wake_unix_ms, first + 1)
    );

    let after = with_record(&v3, first + 1);
    let baseline = round_at(&catalog, readings(), first + 1);
    let expired = format!(
        "policy={} applied_at={NOW} valid_until={first}",
        v3.policy_sha256
    );
    for task_id in [TASK_A, TASK_B] {
        assert_eq!(
            reason_detail(&after, task_id, EXPIRED),
            expired,
            "{task_id}"
        );
    }
    assert_eq!(
        items(&after, TASK_A),
        "pool_a:r=20,per=10,base=500,gap=0,effective=500,term=1000"
    );
    assert!(
        decision(&after, TASK_A)
            .reasons
            .iter()
            .all(|reason| reason.outcome.code().as_str() != TARGETS),
        "{:#?}",
        decision(&after, TASK_A)
    );
    let v2 = compose(
        &catalog,
        &json!({
            "schema_version": V2,
            "instance": A,
            "valid_until_unix_ms": first,
            "targets": [goal_a()]
        }),
        None,
        NOW,
    )
    .unwrap_or_else(|error| panic!("the v2 document applies: {error:?}"));
    let under_v2 = with_record(&v2, first + 1);
    assert_eq!(
        reason_detail(&under_v2, TASK_A, EXPIRED).replace(&v2.policy_sha256, &v3.policy_sha256),
        expired,
        "what an expired v2 policy says"
    );
    let goal_b_item = |detail: &str| {
        detail
            .split("; ")
            .find(|item| item.starts_with("goal-b@"))
            .map(str::to_owned)
    };
    assert_eq!(
        Some(reason_detail(&after, TASK_B, TARGETS).to_owned()),
        goal_b_item(&both),
        "goal-b weighs as before, and alone"
    );
    let pool_b_item = |items: &str| {
        items
            .split(';')
            .find(|item| item.starts_with("pool_b:"))
            .map(str::to_owned)
    };
    assert_eq!(
        pool_b_item(items(&after, TASK_B)),
        pool_b_item(items(&before, TASK_B))
    );
    assert!(pool_b_item(items(&after, TASK_B)).is_some_and(|item| !item.contains(",gap=0,")));
    for task_id in [TASK_A, TASK_B] {
        assert_eq!(
            decision(&after, task_id).next_wake_unix_ms,
            earliest(decision(&baseline, task_id).next_wake_unix_ms, second + 1),
            "{task_id} after {first}"
        );
    }
}

// The v3 reader's refusals, each with its reason at its field: an unknown member (written first
// in its target, where the declaration parser places it: the target), a 97th entry, a name given
// twice, a name out of form, a value whose canonical text is 1025 bytes (one of 1024 is taken),
// `by_unix_ms` (not a member of a v3 target in this slice: `unknown_field` at itself, the
// coordinator's ruling of 11-14 on #1702), a document-level `valid_until_unix_ms`, and a v1
// document over a v3 record.
#[test]
fn gr2a_t3_the_v3_reader_refuses_each_mistake_at_its_field() {
    let catalog = catalog(&[task_a(), task_b()]);
    let entries = |count: usize| {
        (0..count)
            .map(|index| json!({"name": format!("entry_{index}"), "value": index}))
            .collect::<Vec<_>>()
    };
    let document = |targets: Value, entries: Vec<Value>| json!({"schema_version": V3, "instance": A, "targets": targets, "entries": entries});
    let target = || json!([lasting(goal_a(), NOW + DAY_MS)]);
    // An entry whose value's canonical text, a JSON string, is `bytes` bytes long.
    let long = |bytes: usize| vec![json!({"name": "long", "value": "a".repeat(bytes - 2)})];
    let mut dated = lasting(goal_a(), NOW + DAY_MS);
    dated["by_unix_ms"] = json!(NOW + HOUR_MS);
    let mut unknown = json!({"unexpected": 1});
    for (field, value) in goal_a().as_object().expect("a target object") {
        unknown[field] = value.clone();
    }
    let mut lifetime = document(target(), entries(1));
    lifetime["valid_until_unix_ms"] = json!(NOW + DAY_MS);

    let accepted = compose(&catalog, &document(target(), long(1024)), None, NOW)
        .unwrap_or_else(|error| panic!("a value of 1024 bytes applies: {error:?}"));
    let rows: [(&str, Value, Reason, &str); 7] = [
        (
            "an unknown member",
            document(json!([unknown]), entries(1)),
            Reason::UnknownField,
            "/targets/0",
        ),
        (
            "a 97th entry",
            document(target(), entries(97)),
            Reason::OutOfRange,
            "/entries",
        ),
        (
            "a name given twice",
            document(
                target(),
                vec![
                    json!({"name": "route", "value": "a"}),
                    json!({"name": "route", "value": "b"}),
                ],
            ),
            Reason::DuplicateId,
            "/entries/1/name",
        ),
        (
            "a name out of form",
            document(target(), vec![json!({"name": "Route", "value": "a"})]),
            Reason::InvalidValue,
            "/entries/0/name",
        ),
        (
            "a value of 1025 bytes",
            document(target(), long(1025)),
            Reason::OutOfRange,
            "/entries/0/value",
        ),
        (
            "a deadline, not a member of a v3 target in this slice",
            document(json!([dated]), entries(1)),
            Reason::UnknownField,
            "/targets/0/by_unix_ms",
        ),
        (
            "a lifetime of the document",
            lifetime,
            Reason::UnknownField,
            "/valid_until_unix_ms",
        ),
    ];
    for (name, document, reason, field_path) in rows {
        match compose(&catalog, &document, None, NOW) {
            Err(ResourceTargetsError::Rejected(rejection)) => assert_eq!(
                (rejection.reason, rejection.field_path.as_str()),
                (reason, field_path),
                "{name}: {rejection:?}"
            ),
            other => panic!("{name}: refused, not {other:?}"),
        }
    }
    match compose(
        &catalog,
        &v1_target(&[TASK_A]),
        Some(accepted.rows.as_slice()),
        NOW,
    ) {
        Err(ResourceTargetsError::Rejected(rejection)) => assert_eq!(
            (rejection.reason, rejection.field_path.as_str()),
            (Reason::UnsupportedSchemaVersion, "/schema_version"),
            "{rejection:?}"
        ),
        other => panic!("a v1 document over a v3 record: refused, not {other:?}"),
    }
}

// The bound: 16 targets on 16 pools, each naming 8 of the 128 tasks that produce its pool, and 96
// entries make a record of 1 + 16 + 128 + 96 = 241 rows, inside the 256 the scheduler's input
// takes; the evaluation takes it and reads every target (each pool's first task names its goal).
#[test]
fn gr2a_t4_a_record_of_16_targets_128_task_rows_and_96_entries_is_241_rows_and_is_evaluated() {
    let pools = (0..16)
        .map(|index| format!("pool_{index:02}"))
        .collect::<Vec<_>>();
    let mut tasks = Vec::new();
    let mut targets = Vec::new();
    for (index, pool_id) in pools.iter().enumerate() {
        let named = (0..8)
            .map(|task| format!("{pool_id}_task_{task}"))
            .collect::<Vec<_>>();
        for task_id in &named {
            tasks.push(task(task_id, 100, &[(pool_id.as_str(), 10)]));
        }
        targets.push(lasting(
            json!({
                "id": format!("goal-{index:02}"),
                "resource": pool_id,
                "condition": {"kind": "at_least", "amount": 100},
                "apply": {"mode": "adjust", "weight": "score_stage"},
                "tasks": named
            }),
            NOW + DAY_MS,
        ));
    }
    let catalog = bound_catalog(&pools, tasks);
    let entries = (0..96)
        .map(|index| json!({"name": format!("entry_{index:02}"), "value": index}))
        .collect::<Vec<_>>();
    let document =
        json!({"schema_version": V3, "instance": A, "targets": targets, "entries": entries});
    let composed = compose(&catalog, &document, None, NOW)
        .unwrap_or_else(|error| panic!("the document at its bounds applies: {error:?}"));
    assert_eq!(composed.rows.len(), 241);
    let count = |kind: &str| {
        composed
            .rows
            .iter()
            .filter(|row| row.get("row") == Some(&text(kind)))
            .count()
    };
    assert_eq!(
        [
            count("policy"),
            count("target"),
            count("task"),
            count("entry")
        ],
        [1, 16, 128, 96]
    );
    let evaluation = round_at(&catalog, vec![record(&composed, NOW)], NOW);
    for (index, pool_id) in pools.iter().enumerate() {
        let detail = reason_detail(&evaluation, &format!("{pool_id}_task_0"), TARGETS);
        assert!(
            detail.starts_with(&format!("goal-{index:02}@{pool_id}:")),
            "{detail}"
        );
    }
}

/// gate_ss1's catalog with `pools` in place of its two, each valued as pool A, and `tasks`.
fn bound_catalog(pools: &[String], tasks: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.goal-store-bound",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-goal-store-bound"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": crate::SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let pools = pools
        .iter()
        .map(|pool_id| {
            pool(
                pool_id,
                &format!("resource.{pool_id}"),
                json!({"name": "Material", "unit": "piece", "scale": 10, "base_weight_milli": 500,
                    "gap": {"rule": "shortfall_linear", "weight_milli": 100}}),
            )
        })
        .collect::<Vec<_>>();
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": "game-a"},
        "windows": [{
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "utc_offset_minutes": 0,
            "start_minute_of_day": 0,
            "end_minute_of_day": 0
        }],
        "daily_budget": 24,
        "max_window_iterations": 4,
        "session_max_ms": 7200000,
        "detection_budget": {
            "window_dispatch_limit": 4,
            "window_runtime_ms": 300000,
            "expected_duration_ms": 60000
        },
        "minimum_interval_ms": 60000,
        "maximum_interval_ms": 300000,
        "seed_source": "ledger",
        "resample_policy": "same_round_stable",
        "importance_milli": 1000,
        "goals": []
    });
    let source = |name: &str, document: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture/{name}.json"),
            serde_json::to_vec(document).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", Value::Array(pools))),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the bound catalog compiles: {:?}", failure.diagnostics()))
}

// v1 and v2 records untouched. gate_ss1's v2 document (goal-a, no tasks) and v1 document (goal-a
// naming tasks A and B), applied over no record and over a record of the other version (or another
// v2 policy), write exactly the rows, identity, lifetime and conditions the contract gives them,
// pinned here byte for byte from their canonical texts written out by hand; under a reading of 50
// they weigh as gate_ss1 pins: the v2 record gives task A the term 2000, the v1 record task A the
// effective 3333 + 1000.
#[test]
fn gr2a_t5_v1_and_v2_documents_write_the_bytes_they_wrote_before_and_weigh_as_before() {
    let catalog = catalog(&[task_a(), task_b()]);
    let checked_at = NOW - HOUR_MS;
    let valid_until = NOW + DAY_MS;
    let v2_identity = identity(&format!(
        r#"{{"instance":"{A}","schema_version":"{V2}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":100,"kind":"at_least"}},"id":"goal-a","resource":"{POOL_A}"}}],"valid_until_unix_ms":{valid_until}}}"#
    ));
    let v1_identity = identity(&format!(
        r#"{{"instance":"{A}","schema_version":"{V1}","targets":[{{"apply":{{"mode":"adjust","weight":"score_stage"}},"condition":{{"amount":100,"kind":"at_least"}},"id":"goal-a","importance_milli":1000,"resource":"{POOL_A}","rule":"shortfall_linear","scale":10,"tasks":["{TASK_A}","{TASK_B}"]}}],"valid_until_unix_ms":{valid_until}}}"#
    ));
    let header = |version: &str, identity: &str| {
        row(&[
            ("row", text("policy")),
            ("schema_version", text(version)),
            ("instance", text(A)),
            ("policy_sha256", text(identity)),
        ])
    };
    let condition = vec![ResourceTargetCondition {
        target_id: "goal-a".to_owned(),
        resource: POOL_A.to_owned(),
        fact_key: POOL_A_KEY.to_owned(),
        state: ResourceTargetConditionState::AwaitingObservation {
            reason: ResourceTargetPendingReason::Missing,
        },
    }];
    let v2_written = ComposedResourceTargets {
        policy_sha256: v2_identity.clone(),
        rows: vec![
            header(V2, &v2_identity),
            row(&[
                ("row", text("target")),
                ("id", text("goal-a")),
                ("resource", text(POOL_A)),
                ("at_least", integer(100)),
                ("mode", text("adjust")),
                ("weight", text("score_stage")),
            ]),
        ],
        expires_at_unix_ms: Some(valid_until),
        valid_until_unix_ms: Some(valid_until),
        conditions: condition.clone(),
    };
    let v1_written = ComposedResourceTargets {
        policy_sha256: v1_identity.clone(),
        rows: vec![
            header(V1, &v1_identity),
            row(&[
                ("row", text("target")),
                ("id", text("goal-a")),
                ("resource", text(POOL_A)),
                ("at_least", integer(100)),
                ("scale", integer(10)),
                ("importance_milli", integer(1000)),
                ("rule", text("shortfall_linear")),
                ("mode", text("adjust")),
                ("weight", text("score_stage")),
            ]),
            row(&[
                ("row", text("task")),
                ("target", text("goal-a")),
                ("task", text(TASK_A)),
            ]),
            row(&[
                ("row", text("task")),
                ("target", text("goal-a")),
                ("task", text(TASK_B)),
            ]),
        ],
        expires_at_unix_ms: Some(valid_until),
        valid_until_unix_ms: Some(valid_until),
        conditions: condition,
    };
    let v2 = v2_target(None);
    let v1 = v1_target(&[TASK_A, TASK_B]);
    let other_v2 = {
        let mut document = v2_target(None);
        document["targets"][0]["condition"]["amount"] = json!(200);
        document
    };
    let other_v2_rows = compose(&catalog, &other_v2, None, checked_at)
        .expect("another v2 document applies")
        .rows;
    for (label, document, stored, written) in [
        ("v2 over none", &v2, None, &v2_written),
        (
            "v2 over v1",
            &v2,
            Some(v1_written.rows.as_slice()),
            &v2_written,
        ),
        (
            "v2 over v2",
            &v2,
            Some(other_v2_rows.as_slice()),
            &v2_written,
        ),
        ("v1 over none", &v1, None, &v1_written),
        (
            "v1 over v2",
            &v1,
            Some(v2_written.rows.as_slice()),
            &v1_written,
        ),
    ] {
        let composed = compose(&catalog, document, stored, checked_at)
            .unwrap_or_else(|error| panic!("{label}: applies: {error:?}"));
        assert_eq!(&composed, written, "{label}");
    }
    let weighed = |written: &ComposedResourceTargets| {
        round(
            &catalog,
            vec![reading(POOL_A_KEY, 50), record(written, checked_at)],
            Vec::new(),
        )
    };
    assert_eq!(term(&weighed(&v2_written), TASK_A), 2000);
    let under_v1 = weighed(&v1_written);
    assert_eq!(
        decision(&under_v1, TASK_A)
            .rank
            .as_ref()
            .map(|rank| rank.effective_milli),
        Some(3333 + 1000)
    );
}

// An older Runtime. A build before G-R2a reads a v3 record as it reads any version it does not
// know: its v1 reader takes every header that is not v2 and refuses it as
// `unsupported_schema_version`. That build cannot run here; this build reads a version it does not
// know the same way (the record of a v3 document with its header's version rewritten to v4): the
// record is unreadable, the instance runs base scheduling and says so, and the round evaluates.
#[test]
fn gr2a_t6_a_record_of_a_version_the_reader_does_not_know_runs_on_base_scheduling() {
    let catalog = catalog(&[task_a(), task_b()]);
    let checked_at = NOW - HOUR_MS;
    let v3 = compose(
        &catalog,
        &v3_document(vec![lasting(goal_a(), NOW + DAY_MS)]),
        None,
        checked_at,
    )
    .unwrap_or_else(|error| panic!("the v3 document applies: {error:?}"));
    let mut rows = v3.rows.clone();
    rows[0].insert(
        "schema_version".to_owned(),
        text("actingcommand.resource-targets.v4"),
    );
    let unknown = ComposedResourceTargets { rows, ..v3 };
    let evaluation = round(
        &catalog,
        vec![reading(POOL_A_KEY, 50), record(&unknown, checked_at)],
        Vec::new(),
    );
    let detail = reason_detail(&evaluation, TASK_A, "resource_target_policy_unreadable");
    assert!(
        detail.contains("unsupported_schema_version") && detail.ends_with("runs base scheduling"),
        "{detail}"
    );
    assert_eq!(term(&evaluation, TASK_A), 1000, "the base weight alone");
}

// The receipt of a v3 document (the coordinator's ruling of 11-14 on #1702): it never names a
// lifetime or conditions, both left out together, whatever its targets carry: every target with a
// lifetime, a target without one, over no record and over a v3 record; the record still holds
// each target's own lifetime, which the read answers.
#[test]
fn gr2a_t7_a_v3_documents_receipt_names_no_lifetime_and_no_condition() {
    let catalog = catalog(&[task_a(), task_b()]);
    let lasting_targets = v3_document(vec![
        lasting(goal_a(), NOW + DAY_MS),
        lasting(goal_b(), NOW + HOUR_MS),
    ]);
    let over_none = compose(&catalog, &lasting_targets, None, NOW)
        .unwrap_or_else(|error| panic!("the v3 document applies: {error:?}"));
    let over_v3 = compose(
        &catalog,
        &v3_document(vec![goal_a(), lasting(goal_b(), NOW + HOUR_MS)]),
        Some(over_none.rows.as_slice()),
        NOW,
    )
    .unwrap_or_else(|error| panic!("the v3 document applies over a v3 record: {error:?}"));
    for (label, composed) in [("over none", &over_none), ("over v3", &over_v3)] {
        assert_eq!(
            (composed.valid_until_unix_ms, composed.conditions.len()),
            (None, 0),
            "{label}"
        );
    }
    assert_eq!(
        over_none.rows[1].get("valid_until_unix_ms"),
        Some(&integer(NOW + DAY_MS)),
        "the record keeps the target's own lifetime"
    );
}

// The receipt of a v2 document over a v3 record (the coordinator's ruling of 11-14 on #1702): the
// lifetime and the conditions it answers over no record, for the same document checked at the
// same time; the record it writes is the v3 record that keeps the stored entry. With
// `targets: []` over a v3 record holding an entry, likewise: no lifetime, no condition.
#[test]
fn gr2a_t8_a_v2_documents_receipt_over_a_v3_record_is_the_one_it_answers_over_none() {
    let catalog = catalog(&[task_a(), task_b()]);
    let stored = compose(
        &catalog,
        &json!({
            "schema_version": V3,
            "instance": A,
            "targets": [lasting(goal_a(), NOW + DAY_MS)],
            "entries": [{"name": "route", "value": "a"}]
        }),
        None,
        NOW,
    )
    .unwrap_or_else(|error| panic!("the v3 document applies: {error:?}"))
    .rows;
    for (label, document) in [
        (
            "targets",
            json!({
                "schema_version": V2,
                "instance": A,
                "valid_until_unix_ms": NOW + HOUR_MS,
                "targets": [goal_a(), goal_b()]
            }),
        ),
        (
            "targets: []",
            json!({"schema_version": V2, "instance": A, "targets": []}),
        ),
    ] {
        let over_none = compose(&catalog, &document, None, NOW)
            .unwrap_or_else(|error| panic!("{label}: applies over none: {error:?}"));
        let over_v3 = compose(&catalog, &document, Some(stored.as_slice()), NOW)
            .unwrap_or_else(|error| panic!("{label}: applies over v3: {error:?}"));
        assert_eq!(
            (&over_v3.valid_until_unix_ms, &over_v3.conditions),
            (&over_none.valid_until_unix_ms, &over_none.conditions),
            "{label}"
        );
        assert_eq!(
            over_v3.rows[0].get("schema_version"),
            Some(&text(V3)),
            "{label}: a v3 record"
        );
        assert_eq!(
            over_v3.rows[0].get("policy_sha256"),
            Some(&text(&over_v3.policy_sha256)),
            "{label}: the receipt's identity names the v3 record written"
        );
        assert!(
            over_v3
                .rows
                .iter()
                .any(|row| row.get("name") == Some(&text("route"))),
            "{label}: the stored entry kept"
        );
    }
}
