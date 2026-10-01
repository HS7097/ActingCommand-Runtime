// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted): Workflow #335 S2b B1 "v1 unchanged". Built from the PR head and
//! from its exact merge-base in the same CI run; it uses only the v1 API present in both, and
//! every `B1` line must be equal between the two builds. Inputs are the rd5/s1c bench catalog,
//! its v1 documents and its actingd policy inputs, copied unchanged into `one-off-335s2b/`.

use std::path::PathBuf;

use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactScalar, FactValue, ObservedFact, PolicyEvaluation, PriorityOffset,
    PriorityOffsetOrigin, ResourceTargetsError, ScopeSelector, check_resource_targets,
    compile_catalog, evaluate, parse_resource_targets,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// 2026-10-02 09:13:20 JST: after the bench documents' own lifetimes.
const NOW: u64 = 1_790_900_000_000;
const HOUR: u64 = 3_600_000;
const DAY: u64 = 86_400_000;
const SEED: u64 = 7;

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("ONE_OFF_335S2B_DIR").expect("ONE_OFF_335S2B_DIR"))
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(dir().join(path)).unwrap_or_else(|error| panic!("read {path}: {error}"))
}

fn json_file(path: &str) -> Value {
    serde_json::from_slice(&read(path)).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("encode")
}

fn compile(
    tasks: Vec<u8>,
    pools: Vec<u8>,
    activity: Vec<u8>,
    timeline: Vec<u8>,
) -> CompiledCatalog {
    compile_catalog(&CatalogSources {
        tasks: CatalogDocumentSource::new("memory://tasks.json", tasks),
        pools: CatalogDocumentSource::new("memory://pools.json", pools),
        activity: CatalogDocumentSource::new("memory://activity.json", activity),
        timeline: CatalogDocumentSource::new("memory://timeline.json", timeline),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("compile: {:?}", failure.diagnostics()))
}

/// The bench catalog X, byte for byte.
fn catalog_x() -> CompiledCatalog {
    compile(
        read("s1c/tasks.json"),
        read("s1c/pools.json"),
        read("s1c/activity.json"),
        read("s1c/timeline.json"),
    )
}

/// X + valuation: the planned bench v3 catalog (catalog_version 3, approval id `-v3`, the
/// credits pool declaring Q 1000, B 100, G 100).
fn catalog_x_valuation() -> CompiledCatalog {
    let bump = |mut document: Value| {
        let approval = document["catalog"]["approval_refs"][0]
            .as_str()
            .expect("approval")
            .replace("-v2", "-v3");
        document["catalog"]["catalog_version"] = json!(3);
        document["catalog"]["approval_refs"] = json!([approval]);
        document
    };
    let mut pools = bump(json_file("s1c/pools.json"));
    pools["pools"][0]["valuation"] = json!({
        "name": "Credits", "unit": "credit", "scale": 1000, "base_weight_milli": 100,
        "gap": {"rule": "shortfall_linear", "weight_milli": 100}
    });
    compile(
        bytes(&bump(json_file("s1c/tasks.json"))),
        bytes(&pools),
        bytes(&bump(json_file("s1c/activity.json"))),
        bytes(&bump(json_file("s1c/timeline.json"))),
    )
}

struct Bench {
    instance: String,
    server: String,
    fact_key: String,
}

fn bench() -> Bench {
    let pools = json_file("s1c/pools.json");
    let inputs = json_file("inputs.json");
    Bench {
        instance: pools["pools"][0]["scope"]["instance_id"]
            .as_str()
            .expect("instance")
            .to_owned(),
        server: inputs["facts"]["instances"][0]["server_id"]
            .as_str()
            .expect("server")
            .to_owned(),
        fact_key: pools["pools"][0]["observation"]["fact_key"]
            .as_str()
            .expect("fact key")
            .to_owned(),
    }
}

fn time(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: unix_ms,
    }
}

fn base_facts() -> EvaluationFacts {
    let mut facts: EvaluationFacts =
        serde_json::from_value(json_file("inputs.json")["facts"].clone()).expect("facts");
    facts.fact_snapshot_id = "snapshot:one-off-335s2b".to_owned();
    facts.ledger_position = 41;
    facts
}

fn resources() -> EvaluationResources {
    serde_json::from_value(json_file("inputs.json")["resources"].clone()).expect("resources")
}

fn instance_scope(bench: &Bench) -> ScopeSelector {
    ScopeSelector::Instance {
        instance_id: bench.instance.clone(),
    }
}

/// An inventory observation as a detector publishes it (TTL one hour by default).
fn inventory(
    bench: &Bench,
    value: FactValue,
    observed_at: u64,
    expires_at: u64,
    confidence: u16,
) -> ObservedFact {
    ObservedFact {
        scope: instance_scope(bench),
        fact_key: bench.fact_key.clone(),
        value,
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: Some(expires_at),
        confidence_milli: confidence,
    }
}

fn credits(bench: &Bench, current: i64) -> ObservedFact {
    inventory(
        bench,
        FactValue::Integer(current),
        NOW - 60_000,
        NOW - 60_000 + HOUR,
        1_000,
    )
}

fn scalar(value: actingcommand_contract::FactScalar) -> FactScalar {
    match value {
        actingcommand_contract::FactScalar::Boolean(value) => FactScalar::Boolean(value),
        actingcommand_contract::FactScalar::Integer(value) => FactScalar::Integer(value),
        actingcommand_contract::FactScalar::String(value) => FactScalar::String(value),
        actingcommand_contract::FactScalar::TimestampMs(value) => FactScalar::TimestampMs(value),
        actingcommand_contract::FactScalar::DurationMs(value) => FactScalar::DurationMs(value),
    }
}

/// A bench document with its lifetime set (`None` removes it), as bytes.
fn redated(path: &str, valid_until: Option<u64>) -> Vec<u8> {
    let mut document = json_file(path);
    match valid_until {
        Some(valid_until) => document["valid_until_unix_ms"] = json!(valid_until),
        None => {
            document
                .as_object_mut()
                .expect("object")
                .remove("valid_until_unix_ms");
        }
    }
    bytes(&document)
}

/// The stored record exactly as the formal entry checks, encodes and publishes it.
fn policy_fact(
    catalog: &CompiledCatalog,
    document: &[u8],
    applied_at: u64,
    scope: ScopeSelector,
) -> ObservedFact {
    let parsed = parse_resource_targets(document).expect("parse");
    let checked =
        check_resource_targets(&parsed, catalog, &base_facts(), time(applied_at)).expect("check");
    ObservedFact {
        scope,
        fact_key: actingcommand_contract::RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            checked
                .rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(key, value)| (key, scalar(value)))
                        .collect()
                })
                .collect(),
        ),
        observed_at_unix_ms: applied_at,
        expires_at_unix_ms: parsed.document().valid_until_unix_ms,
        confidence_milli: 1_000,
    }
}

fn offset(task: &str, bench: &Bench, offset_milli: i32) -> PriorityOffset {
    PriorityOffset {
        task_id: task.to_owned(),
        instance_id: Some(bench.instance.clone()),
        offset_milli,
        origin: PriorityOffsetOrigin::Agent,
        observed_at_unix_ms: NOW - 120_000,
    }
}

fn run(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> PolicyEvaluation {
    evaluate(catalog, facts, &resources(), time(NOW), SEED).expect("evaluate")
}

fn digest(value: &impl serde::Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serialize"))
    )
}

fn summary(evaluation: &PolicyEvaluation) -> String {
    let winner = evaluation
        .dispatch_intents
        .first()
        .map_or("none", |intent| intent.task_id.as_str());
    let effective = |task: &str| {
        evaluation
            .decisions
            .iter()
            .find(|decision| decision.task_id == task)
            .and_then(|decision| decision.rank.as_ref())
            .map_or_else(
                || "none".to_owned(),
                |rank| rank.effective_milli.to_string(),
            )
    };
    format!(
        "winner={winner} effective(cafe_income)={} effective(notice_home)={}",
        effective("cafe_income"),
        effective("notice_home")
    )
}

/// The B1 input groups: every v1 policy state the evaluator distinguishes, on catalog X.
fn b1_inputs(catalog: &CompiledCatalog) -> Vec<(&'static str, EvaluationFacts)> {
    let bench = bench();
    let base = redated("docs/base.json", Some(NOW + DAY));
    let overriding = redated("docs/override.json", Some(NOW + DAY));
    let with = |facts: Vec<ObservedFact>, offsets: Vec<PriorityOffset>| {
        let mut all = base_facts();
        all.facts = facts;
        all.priority_offsets = offsets;
        all
    };
    let adjust = || policy_fact(catalog, &base, NOW - 120_000, instance_scope(&bench));
    let override_policy =
        || policy_fact(catalog, &overriding, NOW - 120_000, instance_scope(&bench));
    let mut unreadable = adjust();
    if let FactValue::RecordList(rows) = &mut unreadable.value {
        // A header written by a later build this one does not know.
        rows[0].insert(
            "schema_version".to_owned(),
            FactScalar::String("actingcommand.resource-targets.v3".to_owned()),
        );
    }
    let mut server_record = adjust();
    server_record.scope = ScopeSelector::Server {
        server_id: bench.server.clone(),
    };
    let withdrawal = policy_fact(
        catalog,
        &read("docs/withdraw.json"),
        NOW - 120_000,
        instance_scope(&bench),
    );
    vec![
        ("none_c0", with(vec![credits(&bench, 0)], vec![])),
        ("none_no_inventory", with(vec![], vec![])),
        (
            "v1_adjust_c0",
            with(vec![credits(&bench, 0), adjust()], vec![]),
        ),
        (
            "v1_adjust_c9000",
            with(vec![credits(&bench, 9_000), adjust()], vec![]),
        ),
        (
            "v1_adjust_c9500",
            with(vec![credits(&bench, 9_500), adjust()], vec![]),
        ),
        (
            "v1_adjust_offset_minus_300000",
            with(
                vec![credits(&bench, 0), adjust()],
                vec![offset("cafe_income", &bench, -300_000)],
            ),
        ),
        (
            "v1_override_offset_minus_300000",
            with(
                vec![credits(&bench, 0), override_policy()],
                vec![offset("cafe_income", &bench, -300_000)],
            ),
        ),
        (
            "v1_satisfied_adjust_c10000",
            with(vec![credits(&bench, 10_000), adjust()], vec![]),
        ),
        (
            "v1_satisfied_override_offset_c12000",
            with(
                vec![credits(&bench, 12_000), override_policy()],
                vec![offset("cafe_income", &bench, -300_000)],
            ),
        ),
        ("v1_pending_missing", with(vec![adjust()], vec![])),
        (
            "v1_pending_expired",
            with(
                vec![
                    inventory(
                        &bench,
                        FactValue::Integer(0),
                        NOW - 2 * HOUR,
                        NOW - HOUR,
                        1_000,
                    ),
                    adjust(),
                ],
                vec![],
            ),
        ),
        (
            "v1_pending_low_confidence",
            with(
                vec![
                    inventory(&bench, FactValue::Integer(0), NOW - 60_000, NOW + HOUR, 0),
                    adjust(),
                ],
                vec![],
            ),
        ),
        (
            "v1_pending_invalid_value",
            with(
                vec![
                    inventory(
                        &bench,
                        FactValue::String("unreadable".to_owned()),
                        NOW - 60_000,
                        NOW + HOUR,
                        1_000,
                    ),
                    adjust(),
                ],
                vec![],
            ),
        ),
        (
            "v1_expired_policy",
            with(
                vec![
                    credits(&bench, 0),
                    policy_fact(
                        catalog,
                        &redated("docs/base.json", Some(NOW - DAY)),
                        NOW - 2 * DAY,
                        instance_scope(&bench),
                    ),
                ],
                vec![],
            ),
        ),
        (
            "unreadable_unknown_version",
            with(vec![credits(&bench, 0), unreadable], vec![]),
        ),
        (
            "ignored_server_record",
            with(vec![credits(&bench, 0), server_record], vec![]),
        ),
        (
            "v1_withdrawal",
            with(vec![credits(&bench, 0), withdrawal], vec![]),
        ),
    ]
}

/// B1 part 1: `PolicyEvaluation` bytes of every v1 input group on catalog X.
#[test]
fn b1_evaluations() {
    let catalog = catalog_x();
    println!("B1|catalog_x|{}", catalog.catalog_hash());
    for (case, facts) in b1_inputs(&catalog) {
        let evaluation = run(&catalog, &facts);
        println!(
            "B1|{case}|sha256={}|{}",
            digest(&evaluation),
            summary(&evaluation)
        );
    }
}

/// B1 part 2: the v1 rejection matrix, (reason, path, line, column) per realistic mistake.
#[test]
fn b1_v1_rejections() {
    let catalog = catalog_x();
    let bench = bench();
    let mut facts = base_facts();
    facts.facts = vec![credits(&bench, 0)];
    let valid = NOW + DAY;
    let edit = |change: &dyn Fn(&mut Value)| {
        let mut document = json_file("docs/base.json");
        document["valid_until_unix_ms"] = json!(valid);
        change(&mut document);
        bytes(&document)
    };
    let text = |raw: &str| raw.as_bytes().to_vec();
    let base_text = String::from_utf8(edit(&|_| {})).expect("utf-8");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("bad_syntax_bench_file", read("docs/bad-syntax.json")),
        (
            "bad_unknown_task_redated",
            redated("docs/bad-unknown-task.json", Some(valid)),
        ),
        (
            "bad_unmapped_task_redated",
            redated("docs/bad-unmapped-task.json", Some(valid)),
        ),
        ("stale_bench_base", read("docs/base.json")),
        (
            "unknown_instance",
            edit(&|document| document["instance"] = json!("OtherInstance")),
        ),
        ("missing_valid_until", redated("docs/base.json", None)),
        (
            "withdrawal_with_valid_until",
            redated("docs/withdraw.json", Some(valid)),
        ),
        (
            "valid_until_beyond_one_year",
            edit(&|document| document["valid_until_unix_ms"] = json!(NOW + 400 * DAY)),
        ),
        (
            "duplicate_target_id",
            edit(&|document| {
                let target = document["targets"][0].clone();
                document["targets"]
                    .as_array_mut()
                    .expect("targets")
                    .push(target);
            }),
        ),
        (
            "duplicate_task",
            edit(&|document| {
                let mut target = document["targets"][0].clone();
                target["id"] = json!("credits-floor-2");
                document["targets"]
                    .as_array_mut()
                    .expect("targets")
                    .push(target);
            }),
        ),
        (
            "amount_zero",
            edit(&|document| document["targets"][0]["condition"]["amount"] = json!(0)),
        ),
        (
            "scale_zero",
            edit(&|document| document["targets"][0]["scale"] = json!(0)),
        ),
        (
            "importance_over_bound",
            edit(&|document| document["targets"][0]["importance_milli"] = json!(1_000_001)),
        ),
        (
            "tasks_empty",
            edit(&|document| document["targets"][0]["tasks"] = json!([])),
        ),
        (
            "rule_linear",
            edit(&|document| document["targets"][0]["rule"] = json!("linear")),
        ),
        (
            "mode_replace",
            edit(&|document| document["targets"][0]["apply"]["mode"] = json!("replace")),
        ),
        (
            "unknown_field_note",
            edit(&|document| document["targets"][0]["note"] = json!("keep credits up")),
        ),
        (
            "duplicate_key_scale",
            text(&base_text.replacen(
                "\"scale\": 10000,",
                "\"scale\": 10000,\n      \"scale\": 20000,",
                1,
            )),
        ),
        (
            "float_amount",
            text(&base_text.replacen("\"amount\": 10000", "\"amount\": 10000.5", 1)),
        ),
        (
            "null_scale",
            edit(&|document| document["targets"][0]["scale"] = Value::Null),
        ),
        (
            "unsupported_schema_version",
            edit(&|document| {
                document["schema_version"] = json!("actingcommand.resource-targets.v3")
            }),
        ),
        (
            "v1_label_with_manual_offset",
            edit(&|document| document["targets"][0]["apply"]["manual_offset"] = json!("supersede")),
        ),
        (
            "v1_label_without_scale",
            edit(&|document| {
                document["targets"][0]
                    .as_object_mut()
                    .expect("target")
                    .remove("scale");
            }),
        ),
        (
            "unknown_resource",
            edit(&|document| document["targets"][0]["resource"] = json!("credits")),
        ),
        ("valid_adjust", edit(&|_| {})),
        ("valid_override", redated("docs/override.json", Some(valid))),
        ("valid_withdrawal", read("docs/withdraw.json")),
    ];
    for (case, document) in cases {
        let outcome = parse_resource_targets(&document)
            .and_then(|parsed| check_resource_targets(&parsed, &catalog, &facts, time(NOW)));
        match outcome {
            Ok(checked) => println!(
                "B1R|{case}|accepted|{}|rows={}|conditions={}",
                checked.policy_sha256,
                serde_json::to_string(&checked.rows).expect("rows"),
                serde_json::to_string(&checked.conditions).expect("conditions")
            ),
            Err(ResourceTargetsError::Rejected(rejection)) => println!(
                "B1R|{case}|{}|{}|{}|{}",
                serde_json::to_string(&rejection.reason).expect("reason"),
                rejection.field_path,
                rejection.line,
                rejection.column
            ),
            Err(other) => println!("B1R|{case}|error|{other:?}"),
        }
    }
}

/// B1 part 3: the same v1 inputs on X and on X + valuation decide alike; intents differ only in
/// decision_id, reason_chain_id, catalog_hash, catalog_version and approval_refs.
#[test]
fn b1_valuation_is_not_read_for_v1() {
    let x = catalog_x();
    let valued = catalog_x_valuation();
    println!(
        "B1X|catalogs|x={}|x_valuation={}",
        x.catalog_hash(),
        valued.catalog_hash()
    );
    let x_inputs = b1_inputs(&x);
    let valued_inputs = b1_inputs(&valued);
    for ((case, x_facts), (_, valued_facts)) in x_inputs.into_iter().zip(valued_inputs) {
        let left = serde_json::to_value(run(&x, &x_facts)).expect("x");
        let right = serde_json::to_value(run(&valued, &valued_facts)).expect("valued");
        let decisions = |evaluation: &Value| {
            let decisions = evaluation["decisions"]
                .as_array()
                .expect("decisions")
                .iter()
                .map(|decision| {
                    json!([
                        decision["task_id"],
                        decision["instance_id"],
                        decision["state"],
                        decision["rank"],
                        decision["reasons"]
                    ])
                })
                .collect::<Vec<_>>();
            json!([decisions, evaluation["next_wake_unix_ms"]])
        };
        let intents = |evaluation: &Value| {
            let mut intents = evaluation["dispatch_intents"].clone();
            for intent in intents.as_array_mut().expect("intents") {
                let intent = intent.as_object_mut().expect("intent");
                for field in [
                    "decision_id",
                    "reason_chain_id",
                    "catalog_hash",
                    "catalog_version",
                    "approval_refs",
                ] {
                    intent.remove(field);
                }
            }
            intents
        };
        let chains = |evaluation: &Value| {
            let mut chains = evaluation["reason_chains"].clone();
            for chain in chains.as_array_mut().expect("chains") {
                let chain = chain.as_object_mut().expect("chain");
                chain.remove("id");
                chain.remove("decision_id");
            }
            chains
        };
        let verdict = if decisions(&left) == decisions(&right)
            && intents(&left) == intents(&right)
            && chains(&left) == chains(&right)
        {
            "equal"
        } else {
            "DIFFERENT"
        };
        println!(
            "B1X|{case}|{verdict}|decisions_sha256={}",
            digest(&decisions(&left))
        );
        assert_eq!(verdict, "equal", "{case}");
    }
}
