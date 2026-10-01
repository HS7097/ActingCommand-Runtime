// SPDX-License-Identifier: AGPL-3.0-only

//! One-off (to be reverted): Workflow #335 S2b B2-B7 on the PR head. Inputs are the rd5/s1c
//! bench catalog and actingd policy inputs, copied unchanged into `one-off-335s2b/`, and the
//! catalogs the model's examples derive from it (section 3, examples A-D; section 8 bench v3).

use std::path::PathBuf;

use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactScalar, FactValue, ObservedFact, PolicyEvaluation, PriorityOffset,
    PriorityOffsetOrigin, ResourceTargetsError, ScopeSelector, TaskDecision,
    check_resource_targets, compile_catalog, evaluate, parse_resource_targets,
};
use serde_json::{Value, json};

const NOW: u64 = 1_790_900_000_000;
const HOUR: u64 = 3_600_000;
const DAY: u64 = 86_400_000;
const SEED: u64 = 7;
const V2: &str = "actingcommand.resource-targets.v2";
const CAFE: &str = "cafe_income";
const NOTICE: &str = "notice_home";

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("ONE_OFF_335S2B_DIR").expect("ONE_OFF_335S2B_DIR"))
}

fn json_file(path: &str) -> Value {
    let raw =
        std::fs::read(dir().join(path)).unwrap_or_else(|error| panic!("read {path}: {error}"));
    serde_json::from_slice(&raw).unwrap_or_else(|error| panic!("parse {path}: {error}"))
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("encode")
}

/// The four bench documents, editable.
#[derive(Clone)]
struct Catalog {
    tasks: Value,
    pools: Value,
    activity: Value,
    timeline: Value,
}

impl Catalog {
    fn bench() -> Self {
        Self {
            tasks: json_file("s1c/tasks.json"),
            pools: json_file("s1c/pools.json"),
            activity: json_file("s1c/activity.json"),
            timeline: json_file("s1c/timeline.json"),
        }
    }

    /// The planned bench v3: catalog_version 3, approval `-v3`, credits Q 1000, B 100, G 100.
    fn v3() -> Self {
        let mut catalog = Self::bench();
        for document in [
            &mut catalog.tasks,
            &mut catalog.pools,
            &mut catalog.activity,
            &mut catalog.timeline,
        ] {
            let approval = document["catalog"]["approval_refs"][0]
                .as_str()
                .expect("approval")
                .replace("-v2", "-v3");
            document["catalog"]["catalog_version"] = json!(3);
            document["catalog"]["approval_refs"] = json!([approval]);
        }
        catalog.valuation(
            0,
            json!({
                "name": "Credits", "unit": "credit", "scale": 1000, "base_weight_milli": 100,
                "gap": {"rule": "shortfall_linear", "weight_milli": 100}
            }),
        );
        catalog
    }

    fn valuation(&mut self, pool: usize, valuation: Value) {
        self.pools["pools"][pool]["valuation"] = valuation;
    }

    fn without_valuation(&mut self, pool: usize) {
        self.pools["pools"][pool]
            .as_object_mut()
            .expect("pool")
            .remove("valuation");
    }

    fn without_gap(&mut self, pool: usize) {
        self.pools["pools"][pool]["valuation"]
            .as_object_mut()
            .expect("valuation")
            .remove("gap");
    }

    fn credits_pool(&self) -> String {
        self.pools["pools"][0]["id"]
            .as_str()
            .expect("pool")
            .to_owned()
    }

    /// Adds a pool beside credits: same scope and projection, its own fact key.
    fn add_pool(&mut self, suffix: &str, valuation: Option<Value>) -> String {
        let mut pool = self.pools["pools"][0].clone();
        let id = self
            .credits_pool()
            .replace(".credits", &format!(".{suffix}"));
        pool["id"] = json!(id);
        pool["observation"]["fact_key"] = json!(format!("resource.{suffix}"));
        let object = pool.as_object_mut().expect("pool");
        object.remove("valuation");
        if let Some(valuation) = valuation {
            object.insert("valuation".to_owned(), valuation);
        }
        self.pools["pools"]
            .as_array_mut()
            .expect("pools")
            .push(pool);
        id
    }

    fn task_mut(&mut self, id: &str) -> &mut Value {
        self.tasks["tasks"]
            .as_array_mut()
            .expect("tasks")
            .iter_mut()
            .find(|task| task["id"] == id)
            .expect("task")
    }

    /// Adds a task cloned from cafe_income under another id.
    fn add_task(&mut self, id: &str) {
        let mut task = self.task_mut(CAFE).clone();
        task["id"] = json!(id);
        self.tasks["tasks"]
            .as_array_mut()
            .expect("tasks")
            .push(task);
    }

    fn produces(&mut self, task: &str, effects: &[(&str, u64)]) {
        self.task_mut(task)["produces"] = Value::Array(
            effects
                .iter()
                .map(|(pool, amount)| {
                    json!({"pool_id": pool, "direction": "produce", "amount": amount,
                        "observation_source": "inferred", "confidence_milli": 1000})
                })
                .collect(),
        );
    }

    /// Scopes every task, pool and activity profile to the bench server (two instances).
    fn server_scoped(&mut self, server: &str) {
        let scope = json!({"kind": "server", "server_id": server});
        for task in self.tasks["tasks"].as_array_mut().expect("tasks") {
            task["scope"] = scope.clone();
        }
        for pool in self.pools["pools"].as_array_mut().expect("pools") {
            pool["scope"] = scope.clone();
        }
        for profile in self.activity["profiles"].as_array_mut().expect("profiles") {
            profile["scope"] = scope.clone();
        }
    }

    fn compile(&self) -> CompiledCatalog {
        compile_catalog(&CatalogSources {
            tasks: CatalogDocumentSource::new("memory://tasks.json", bytes(&self.tasks)),
            pools: CatalogDocumentSource::new("memory://pools.json", bytes(&self.pools)),
            activity: CatalogDocumentSource::new("memory://activity.json", bytes(&self.activity)),
            timeline: CatalogDocumentSource::new("memory://timeline.json", bytes(&self.timeline)),
            selection: None,
        })
        .unwrap_or_else(|failure| panic!("compile: {:?}", failure.diagnostics()))
    }
}

fn instance_id() -> String {
    json_file("s1c/pools.json")["pools"][0]["scope"]["instance_id"]
        .as_str()
        .expect("instance")
        .to_owned()
}

fn second_instance_id() -> String {
    format!("{}-second", instance_id())
}

fn server_id() -> String {
    json_file("inputs.json")["facts"]["instances"][0]["server_id"]
        .as_str()
        .expect("server")
        .to_owned()
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

fn two_instance_facts() -> EvaluationFacts {
    let mut facts = base_facts();
    let mut second = facts.instances[0].clone();
    second.instance_id = second_instance_id();
    facts.instances.push(second);
    facts
}

fn resources(cpu: u16) -> EvaluationResources {
    let mut resources: EvaluationResources =
        serde_json::from_value(json_file("inputs.json")["resources"].clone()).expect("resources");
    resources.hosts[0].cpu_available_milli = cpu;
    resources
}

fn scope(instance: &str) -> ScopeSelector {
    ScopeSelector::Instance {
        instance_id: instance.to_owned(),
    }
}

fn inventory_fact(
    instance: &str,
    key: &str,
    current: i64,
    observed_at: u64,
    expires_at: u64,
) -> ObservedFact {
    ObservedFact {
        scope: scope(instance),
        fact_key: key.to_owned(),
        value: FactValue::Integer(current),
        observed_at_unix_ms: observed_at,
        expires_at_unix_ms: Some(expires_at),
        confidence_milli: 1_000,
    }
}

fn credits(instance: &str, current: i64) -> ObservedFact {
    inventory_fact(
        instance,
        "resource.credits",
        current,
        NOW - 60_000,
        NOW - 60_000 + HOUR,
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

/// A v2 document for `instance` with the given targets.
fn v2(instance: &str, valid_until: Option<u64>, targets: Value) -> Vec<u8> {
    let mut document = json!({"schema_version": V2, "instance": instance, "targets": targets});
    if let Some(valid_until) = valid_until {
        document["valid_until_unix_ms"] = json!(valid_until);
    }
    bytes(&document)
}

fn target(id: &str, resource: &str, at_least: u64, apply: Value) -> Value {
    json!({"id": id, "resource": resource, "condition": {"kind": "at_least", "amount": at_least},
        "apply": apply})
}

fn adjust() -> Value {
    json!({"mode": "adjust", "weight": "score_stage"})
}

fn overriding(manual_offset: Option<&str>) -> Value {
    let mut apply = json!({"mode": "override", "weight": "score_stage"});
    if let Some(manual_offset) = manual_offset {
        apply["manual_offset"] = json!(manual_offset);
    }
    apply
}

/// The stored record exactly as the formal entry checks, encodes and publishes it.
fn policy_fact(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    instance: &str,
    document: &[u8],
    applied_at: u64,
) -> ObservedFact {
    let parsed = parse_resource_targets(document).unwrap_or_else(|error| panic!("{error:?}"));
    let checked = check_resource_targets(&parsed, catalog, facts, time(applied_at))
        .unwrap_or_else(|error| panic!("{error:?}"));
    ObservedFact {
        scope: scope(instance),
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
        expires_at_unix_ms: parsed.valid_until_unix_ms(),
        confidence_milli: 1_000,
    }
}

fn offset(task: &str, instance: &str, offset_milli: i32) -> PriorityOffset {
    PriorityOffset {
        task_id: task.to_owned(),
        instance_id: Some(instance.to_owned()),
        offset_milli,
        origin: PriorityOffsetOrigin::Agent,
        observed_at_unix_ms: NOW - 120_000,
    }
}

fn run(catalog: &CompiledCatalog, facts: &EvaluationFacts, cpu: u16) -> PolicyEvaluation {
    evaluate(catalog, facts, &resources(cpu), time(NOW), SEED)
        .unwrap_or_else(|error| panic!("{error:?}"))
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task: &str, instance: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| {
            decision.task_id == task && decision.instance_id.as_deref() == Some(instance)
        })
        .unwrap_or_else(|| panic!("decision {task}@{instance}"))
}

fn detail(evaluation: &PolicyEvaluation, task: &str, instance: &str, code: &str) -> String {
    decision(evaluation, task, instance)
        .reasons
        .iter()
        .find(|reason| reason.code == code)
        .map_or_else(|| "-".to_owned(), |reason| reason.detail.clone())
}

fn effective(evaluation: &PolicyEvaluation, task: &str, instance: &str) -> String {
    decision(evaluation, task, instance)
        .rank
        .as_ref()
        .map_or_else(
            || "none".to_owned(),
            |rank| rank.effective_milli.to_string(),
        )
}

fn total(evaluation: &PolicyEvaluation, task: &str, instance: &str) -> String {
    decision(evaluation, task, instance)
        .rank
        .as_ref()
        .map_or_else(|| "none".to_owned(), |rank| rank.total_score.to_string())
}

fn winner(evaluation: &PolicyEvaluation, instance: &str) -> String {
    evaluation
        .dispatch_intents
        .iter()
        .find(|intent| intent.instance_id == instance)
        .map_or_else(|| "none".to_owned(), |intent| intent.task_id.clone())
}

/// Every reason of the winner's chain whose code is one of `codes`, as `code: detail`.
fn chain_reasons(evaluation: &PolicyEvaluation, instance: &str, codes: &[&str]) -> Vec<String> {
    let task = winner(evaluation, instance);
    decision(evaluation, &task, instance)
        .reasons
        .iter()
        .filter(|reason| codes.iter().any(|code| reason.code.starts_with(code)))
        .map(|reason| format!("{}: {}", reason.code, reason.detail))
        .collect()
}

fn instance_bytes(evaluation: &PolicyEvaluation, instance: &str, with_reasons: bool) -> Vec<u8> {
    let decisions = evaluation
        .decisions
        .iter()
        .filter(|decision| decision.instance_id.as_deref() == Some(instance))
        .map(|decision| {
            let mut decision = decision.clone();
            if !with_reasons {
                decision.reasons.clear();
            }
            decision
        })
        .collect::<Vec<_>>();
    let intents = evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == instance)
        .collect::<Vec<_>>();
    let chains = evaluation
        .reason_chains
        .iter()
        .filter(|chain| {
            with_reasons
                && intents
                    .iter()
                    .any(|intent| intent.decision_id == chain.decision_id)
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&(decisions, intents, chains)).expect("bytes")
}

/// B2 (ruling 1): an enabled instance leaves the other instance's decisions alone; with a
/// short host budget only `host_budget_deferred` may differ; withdrawn or expired, the enabled
/// instance decides as without a policy apart from its reasons.
#[test]
fn b2_other_instances_and_disabled_policy() {
    let (first, second) = (instance_id(), second_instance_id());
    let mut source = Catalog::v3();
    source.server_scoped(&server_id());
    let catalog = source.compile();
    let pool = source.credits_pool();
    let facts = two_instance_facts();
    let with = |policy: Option<ObservedFact>| {
        let mut facts = facts.clone();
        facts.facts = vec![credits(&first, 0), credits(&second, 0)];
        facts.facts.extend(policy);
        facts
    };
    let active = policy_fact(
        &catalog,
        &facts,
        &first,
        &v2(
            &first,
            Some(NOW + DAY),
            json!([target("credits-floor", &pool, 100_000, adjust())]),
        ),
        NOW - 120_000,
    );
    for (group, cpu) in [("budget_ample", 1_000), ("budget_short", 100)] {
        let enabled = run(&catalog, &with(Some(active.clone())), cpu);
        let none = run(&catalog, &with(None), cpu);
        println!(
            "B2|{group}|winner_first={} winner_second={}|without_policy: winner_first={} winner_second={}",
            winner(&enabled, &first),
            winner(&enabled, &second),
            winner(&none, &first),
            winner(&none, &second)
        );
        let identical =
            instance_bytes(&enabled, &second, true) == instance_bytes(&none, &second, true);
        println!("B2|{group}|second_instance_bytes_identical={identical}");
        for task in [NOTICE, CAFE] {
            let (with_policy, without) = (
                decision(&enabled, task, &second),
                decision(&none, task, &second),
            );
            let codes = |decision: &TaskDecision| {
                decision
                    .reasons
                    .iter()
                    .map(|reason| reason.code.clone())
                    .collect::<Vec<_>>()
            };
            let strip = |decision: &TaskDecision| {
                decision
                    .reasons
                    .iter()
                    .filter(|reason| reason.code != "host_budget_deferred")
                    .cloned()
                    .collect::<Vec<_>>()
            };
            println!(
                "B2|{group}|second:{task}|rank_equal={}|reasons_equal_but_host_budget_deferred={}|state={:?}/{:?}|codes_with_policy={:?}|codes_without={:?}",
                with_policy.rank == without.rank,
                strip(with_policy) == strip(without),
                with_policy.state,
                without.state,
                codes(with_policy),
                codes(without)
            );
        }
    }
    let withdrawn = policy_fact(
        &catalog,
        &facts,
        &first,
        &v2(&first, None, json!([])),
        NOW - 60_000,
    );
    let expired = policy_fact(
        &catalog,
        &facts,
        &first,
        &v2(
            &first,
            Some(NOW - DAY),
            json!([target("credits-floor", &pool, 100_000, adjust())]),
        ),
        NOW - 2 * DAY,
    );
    let none = run(&catalog, &with(None), 1_000);
    for (case, policy) in [("withdrawn", withdrawn), ("expired", expired)] {
        let evaluation = run(&catalog, &with(Some(policy)), 1_000);
        println!(
            "B2|{case}|first_bytes_identical_with_reasons={}|first_bytes_identical_without_reasons={}|decision_record={}|expired_reason_on_cafe={}",
            instance_bytes(&evaluation, &first, true) == instance_bytes(&none, &first, true),
            instance_bytes(&evaluation, &first, false) == instance_bytes(&none, &first, false),
            chain_reasons(&evaluation, &first, &["decision_record"]).join(" | "),
            detail(&evaluation, CAFE, &first, "resource_target_policy_expired")
        );
    }
}

fn verdict(
    outcome: Result<actingcommand_policy::CheckedResourceTargets, ResourceTargetsError>,
) -> String {
    match outcome {
        Ok(checked) => format!(
            "accepted|{}|rows={}|conditions={}",
            checked.policy_sha256,
            serde_json::to_string(&checked.rows).expect("rows"),
            serde_json::to_string(&checked.conditions).expect("conditions")
        ),
        Err(ResourceTargetsError::Rejected(rejection)) => format!(
            "{}|{}|{}:{}",
            serde_json::to_string(&rejection.reason).expect("reason"),
            rejection.field_path,
            rejection.line,
            rejection.column
        ),
        Err(other) => format!("error|{other:?}"),
    }
}

/// B3: v2 rejections an agent realistically makes; legal documents, their rows and identity.
#[test]
fn b3_v2_rejections_rows_and_identity() {
    let instance = instance_id();
    let v3 = Catalog::v3();
    let pool = v3.credits_pool();
    let mut unproduced = Catalog::v3();
    let gems = unproduced.add_pool(
        "gems",
        Some(json!({"name": "Gems", "unit": "gem", "scale": 10, "base_weight_milli": 500})),
    );
    let mut no_gap = Catalog::v3();
    no_gap.without_gap(0);
    let catalogs = [
        ("x", Catalog::bench().compile()),
        ("v3", v3.compile()),
        ("v3_plus_unproduced_pool", unproduced.compile()),
        ("v3_without_gap", no_gap.compile()),
    ];
    let catalog = |name: &str| {
        &catalogs
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .expect("catalog")
            .1
    };
    let mut facts = base_facts();
    facts.facts = vec![credits(&instance, 60_000)];
    let valid = Some(NOW + DAY);
    let floor = |apply: Value| target("credits-floor", &pool, 100_000, apply);
    let with = |mut value: Value, key: &str, field: Value| {
        value[key] = field;
        value
    };
    let cafe = json!([CAFE]);
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "scale_missing_without_valuation",
            "x",
            v2(&instance, valid, json!([floor(adjust())])),
        ),
        (
            "importance_missing_without_gap",
            "v3_without_gap",
            v2(&instance, valid, json!([floor(adjust())])),
        ),
        (
            "manual_offset_in_adjust",
            "v3",
            v2(
                &instance,
                valid,
                json!([floor(with(adjust(), "manual_offset", json!("keep")))]),
            ),
        ),
        (
            "manual_offset_unknown_value",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(overriding(Some("drop"))), "tasks", cafe.clone())]),
            ),
        ),
        (
            "override_without_tasks",
            "v3",
            v2(&instance, valid, json!([floor(overriding(None))])),
        ),
        (
            "rule_linear",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "rule", json!("linear"))]),
            ),
        ),
        (
            "duplicate_resource",
            "v3",
            v2(
                &instance,
                valid,
                json!([
                    floor(adjust()),
                    target("credits-reserve", &pool, 200_000, adjust())
                ]),
            ),
        ),
        (
            "tasks_empty",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "tasks", json!([]))]),
            ),
        ),
        (
            "unknown_task",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "tasks", json!(["cafe_income_daily"]))]),
            ),
        ),
        (
            "listed_task_not_producing",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "tasks", json!([NOTICE]))]),
            ),
        ),
        (
            "no_producing_task_for_default_scope",
            "v3_plus_unproduced_pool",
            v2(
                &instance,
                valid,
                json!([with(
                    target("gems-floor", &gems, 1_000, adjust()),
                    "importance_milli",
                    json!(100)
                )]),
            ),
        ),
        (
            "scale_zero",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "scale", json!(0))]),
            ),
        ),
        (
            "importance_over_bound",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "importance_milli", json!(1_000_001))]),
            ),
        ),
        (
            "null_scale",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "scale", Value::Null)]),
            ),
        ),
        (
            "missing_valid_until",
            "v3",
            v2(&instance, None, json!([floor(adjust())])),
        ),
        (
            "stale_valid_until",
            "v3",
            v2(&instance, Some(NOW - HOUR), json!([floor(adjust())])),
        ),
        (
            "unknown_field_weight_milli",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(adjust()), "weight_milli", json!(100))]),
            ),
        ),
        (
            "unknown_instance",
            "v3",
            v2("OtherInstance", valid, json!([floor(adjust())])),
        ),
        (
            "valid_adjust",
            "v3",
            v2(&instance, valid, json!([floor(adjust())])),
        ),
        (
            "valid_override_keep_default",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(floor(overriding(None)), "tasks", cafe.clone())]),
            ),
        ),
        (
            "valid_override_supersede",
            "v3",
            v2(
                &instance,
                valid,
                json!([with(
                    floor(overriding(Some("supersede"))),
                    "tasks",
                    cafe.clone()
                )]),
            ),
        ),
        (
            "valid_every_optional_field",
            "v3",
            v2(
                &instance,
                valid,
                json!([{"id": "credits-floor", "resource": pool, "condition": {"kind": "at_least", "amount": 100000},
                    "scale": 1000, "importance_milli": 100, "rule": "shortfall_linear",
                    "apply": {"mode": "override", "weight": "score_stage", "manual_offset": "keep"},
                    "tasks": [CAFE]}]),
            ),
        ),
        (
            "valid_own_terms_without_valuation",
            "x",
            v2(
                &instance,
                valid,
                json!([with(
                    with(floor(adjust()), "scale", json!(1000)),
                    "importance_milli",
                    json!(100)
                )]),
            ),
        ),
        ("valid_withdrawal", "v3", v2(&instance, None, json!([]))),
    ];
    for (case, catalog_name, document) in &cases {
        let outcome = parse_resource_targets(document).and_then(|parsed| {
            check_resource_targets(&parsed, catalog(*catalog_name), &facts, time(NOW))
        });
        println!("B3|{case}|catalog={catalog_name}|{}", verdict(outcome));
    }
    // Identity: a resubmission, a reordered and minified copy, and the v1 twin.
    let (_, _, adjust_document) = cases
        .iter()
        .find(|(case, _, _)| *case == "valid_adjust")
        .expect("valid_adjust");
    let check = |document: &[u8]| {
        let parsed = parse_resource_targets(document).expect("parse");
        check_resource_targets(&parsed, catalog("v3"), &facts, time(NOW)).expect("check")
    };
    let first = check(adjust_document.as_slice());
    let again = check(adjust_document.as_slice());
    let value: Value = serde_json::from_slice(adjust_document).expect("json");
    let minified = serde_json::to_vec(&value).expect("minified");
    let reordered = check(minified.as_slice());
    let mut v1_twin = json_file("docs/base.json");
    v1_twin["valid_until_unix_ms"] = json!(NOW + DAY);
    let v1 = check(bytes(&v1_twin).as_slice());
    let mut v2_twin = v1_twin.clone();
    v2_twin["schema_version"] = json!(V2);
    let v2_same_fields = check(bytes(&v2_twin).as_slice());
    println!(
        "B3|identity|resubmission_same={}|minified_same={}|v1={}|v2_same_fields={}|v1_ne_v2={}",
        first == again,
        first.policy_sha256 == reordered.policy_sha256,
        v1.policy_sha256,
        v2_same_fields.policy_sha256,
        v1.policy_sha256 != v2_same_fields.policy_sha256
    );
    let limits = first
        .rows
        .iter()
        .map(|row| row.len())
        .max()
        .unwrap_or_default();
    println!(
        "B3|rows|valid_adjust_rows={}|widest_row_fields={limits}",
        first.rows.len()
    );
}

/// One evaluation on the bench instance with credits at `current`, the given policy document
/// and optional offsets.
fn one(
    catalog: &CompiledCatalog,
    current: Option<i64>,
    document: Option<&[u8]>,
    offsets: &[(&str, i32)],
) -> PolicyEvaluation {
    let instance = instance_id();
    let mut facts = base_facts();
    facts.facts = current
        .map(|current| credits(&instance, current))
        .into_iter()
        .collect();
    if let Some(document) = document {
        let policy = policy_fact(catalog, &facts, &instance, document, NOW - 120_000);
        facts.facts.push(policy);
    }
    facts.priority_offsets = offsets
        .iter()
        .map(|(task, offset_milli)| offset(task, &instance, *offset_milli))
        .collect();
    run(catalog, &facts, 1_000)
}

/// B4: every number of examples A-D, the flip boundaries included.
#[test]
fn b4_examples_a_to_d() {
    let instance = instance_id();
    let valid = Some(NOW + DAY);
    // Example A: bench v3 (Q = S = 1000, B = 100, G = 100), T = 100000, cafe r = 1000.
    let v3 = Catalog::v3();
    let pool = v3.credits_pool();
    let catalog = v3.compile();
    let adjust_a = v2(
        &instance,
        valid,
        json!([target("credits-floor", &pool, 100_000, adjust())]),
    );
    for current in [0, 50_990, 51_000, 51_001, 60_000, 100_000] {
        let evaluation = one(&catalog, Some(current), Some(adjust_a.as_slice()), &[]);
        println!(
            "B4|A|c={current}|winner={}|effective cafe={} notice={}|total cafe={} notice={}|resource_targets={}|resource_weights={}|rank_breakdown={}",
            winner(&evaluation, &instance),
            effective(&evaluation, CAFE, &instance),
            effective(&evaluation, NOTICE, &instance),
            total(&evaluation, CAFE, &instance),
            total(&evaluation, NOTICE, &instance),
            detail(&evaluation, CAFE, &instance, "resource_targets"),
            detail(&evaluation, CAFE, &instance, "resource_weights"),
            chain_reasons(&evaluation, &instance, &["rank_breakdown"]).join(" | ")
        );
    }
    // Example B: Q = S = 10000, B = 20, G = 10, T = 5e6; cafe r = 300000, mail r = 50000.
    let mut source = Catalog::bench();
    source.valuation(
        0,
        json!({
            "name": "Credits", "unit": "credit", "scale": 10000, "base_weight_milli": 20,
            "gap": {"rule": "shortfall_linear", "weight_milli": 10}
        }),
    );
    source.add_task("mail_rewards");
    source.produces(CAFE, &[(pool.as_str(), 300_000)]);
    source.produces("mail_rewards", &[(pool.as_str(), 50_000)]);
    let catalog = source.compile();
    let adjust_b = v2(
        &instance,
        valid,
        json!([target("credits-floor", &pool, 5_000_000, adjust())]),
    );
    for current in [1_000_000, 4_853_000, 4_853_001, 4_900_000, 5_000_000] {
        let evaluation = one(&catalog, Some(current), Some(adjust_b.as_slice()), &[]);
        println!(
            "B4|B|c={current}|winner={}|effective cafe={} mail={} notice={}|cafe resource_weights={}|mail resource_weights={}",
            winner(&evaluation, &instance),
            effective(&evaluation, CAFE, &instance),
            effective(&evaluation, "mail_rewards", &instance),
            effective(&evaluation, NOTICE, &instance),
            detail(&evaluation, CAFE, &instance, "resource_weights"),
            detail(&evaluation, "mail_rewards", &instance, "resource_weights")
        );
    }
    // Example C: example B plus a second resource (Q = 10, B = 500, no gap, no target);
    // daily_rewards produces 20000 credits and 20 of it.
    let gems = source.add_pool(
        "gems",
        Some(json!({"name": "Gems", "unit": "gem", "scale": 10, "base_weight_milli": 500})),
    );
    source.add_task("daily_rewards");
    source.produces(
        "daily_rewards",
        &[(pool.as_str(), 20_000), (gems.as_str(), 20)],
    );
    let catalog = source.compile();
    for current in [1_000_000, 5_000_000] {
        let evaluation = one(&catalog, Some(current), Some(adjust_b.as_slice()), &[]);
        println!(
            "B4|C|c={current}|winner={}|effective daily={} cafe={}|daily resource_weights={}|cafe resource_weights={}",
            winner(&evaluation, &instance),
            effective(&evaluation, "daily_rewards", &instance),
            effective(&evaluation, CAFE, &instance),
            detail(&evaluation, "daily_rewards", &instance, "resource_weights"),
            detail(&evaluation, CAFE, &instance, "resource_weights")
        );
    }
    // Example D: bench v3, c = 0, cafe offset -300000.
    let catalog = Catalog::v3().compile();
    let floor = |apply: Value| {
        let mut target = target("credits-floor", &pool, 100_000, apply);
        target["tasks"] = json!([CAFE]);
        target
    };
    let mut v1_override = json_file("docs/override.json");
    v1_override["valid_until_unix_ms"] = json!(NOW + DAY);
    let documents = [
        ("v2_adjust", adjust_a.clone()),
        (
            "v2_override_default_keep",
            v2(&instance, valid, json!([floor(overriding(None))])),
        ),
        (
            "v2_override_supersede",
            v2(
                &instance,
                valid,
                json!([floor(overriding(Some("supersede")))]),
            ),
        ),
        ("v1_override_bench_document", bytes(&v1_override)),
    ];
    for (case, document) in &documents {
        let evaluation = one(
            &catalog,
            Some(0),
            Some(document.as_slice()),
            &[(CAFE, -300_000)],
        );
        println!(
            "B4|D|{case}|winner={}|effective cafe={} notice={}|override={}|scored={}",
            winner(&evaluation, &instance),
            effective(&evaluation, CAFE, &instance),
            effective(&evaluation, NOTICE, &instance),
            detail(
                &evaluation,
                CAFE,
                &instance,
                "resource_target_override:credits-floor"
            ),
            detail(&evaluation, CAFE, &instance, "scored")
        );
    }
}

/// B5: the override switch (example D) and its release at g = 0, base weight kept.
#[test]
fn b5_override_switch_and_release() {
    let instance = instance_id();
    let valid = Some(NOW + DAY);
    let v3 = Catalog::v3();
    let pool = v3.credits_pool();
    let catalog = v3.compile();
    let floor = |apply: Value| {
        let mut target = target("credits-floor", &pool, 100_000, apply);
        target["tasks"] = json!([CAFE]);
        target
    };
    for (case, apply) in [
        ("keep", overriding(None)),
        ("supersede", overriding(Some("supersede"))),
    ] {
        let document = v2(&instance, valid, json!([floor(apply)]));
        for current in [0, 100_000] {
            let evaluation = one(
                &catalog,
                Some(current),
                Some(document.as_slice()),
                &[(CAFE, -300_000)],
            );
            let codes = decision(&evaluation, CAFE, &instance)
                .reasons
                .iter()
                .map(|reason| reason.code.clone())
                .collect::<Vec<_>>();
            println!(
                "B5|{case}|c={current}|winner={}|effective cafe={}|scored={}|resource_targets={}|resource_weights={}|codes={codes:?}",
                winner(&evaluation, &instance),
                effective(&evaluation, CAFE, &instance),
                detail(&evaluation, CAFE, &instance, "scored"),
                detail(&evaluation, CAFE, &instance, "resource_targets"),
                detail(&evaluation, CAFE, &instance, "resource_weights")
            );
        }
    }
}

/// B6: freshness and wake of a candidate a v2 policy moves.
#[test]
fn b6_freshness_and_wake() {
    let instance = instance_id();
    let v3 = Catalog::v3();
    let pool = v3.credits_pool();
    let catalog = v3.compile();
    for (case, current, credits_expiry, valid_until) in [
        ("inventory_lapses_first", 0, NOW + 30 * 60_000, NOW + DAY),
        ("policy_lapses_first", 0, NOW + 2 * HOUR, NOW + HOUR),
        // Lapses within the 40 s before the next trigger occurrence, so the wake is visible.
        ("inventory_lapses_in_20s", 0, NOW + 20_000, NOW + DAY),
        ("policy_lapses_in_30s", 0, NOW + HOUR, NOW + 30_000),
        (
            "base_weight_only_satisfied",
            100_000,
            NOW + 30 * 60_000,
            NOW + HOUR,
        ),
    ] {
        let mut facts = base_facts();
        facts.facts = vec![inventory_fact(
            &instance,
            "resource.credits",
            current,
            NOW - 60_000,
            credits_expiry,
        )];
        let document = v2(
            &instance,
            Some(valid_until),
            json!([target("credits-floor", &pool, 100_000, adjust())]),
        );
        let policy = policy_fact(&catalog, &facts, &instance, &document, NOW - 120_000);
        facts.facts.push(policy);
        let mut blocking = facts.clone();
        // notice_home cooled down, so the moved candidate is dispatched and its intent shown.
        blocking.tasks =
            vec![serde_json::from_value(json!({
            "task_id": NOTICE, "instance_id": instance, "last_dispatched_unix_ms": NOW - 1_000,
            "eligible_since_unix_ms": null, "terminal_state": null
        }))
        .expect("task state")];
        let evaluation = run(&catalog, &blocking, 1_000);
        let intent = evaluation
            .dispatch_intents
            .iter()
            .find(|intent| intent.task_id == CAFE)
            .expect("cafe intent");
        println!(
            "B6|{case}|credits_expiry={credits_expiry}|valid_until={valid_until}|facts_fresh_until={:?}|next_wake={:?}|resource_weights={}",
            intent.prerequisites.facts_fresh_until_unix_ms,
            evaluation.next_wake_unix_ms,
            detail(&evaluation, CAFE, &instance, "resource_weights")
        );
    }
}

/// B7: reason formats and bounds; a task producing six resources; unresolved targets.
#[test]
fn b7_reasons() {
    let instance = instance_id();
    let valid = Some(NOW + DAY);
    let mut source = Catalog::v3();
    let pool = source.credits_pool();
    let mut produced = vec![(pool.clone(), 1_000)];
    for (suffix, scale, base) in [
        ("gems", 10, 500),
        ("energy", 10, 200),
        ("tickets", 1, 1_000),
        ("keys", 1, 800),
        ("tokens", 100, 50),
    ] {
        let id = source.add_pool(
            suffix,
            Some(
                json!({"name": suffix, "unit": "unit", "scale": scale, "base_weight_milli": base,
                "gap": {"rule": "shortfall_linear", "weight_milli": 100}}),
            ),
        );
        produced.push((id, 20));
    }
    let effects = produced
        .iter()
        .map(|(pool, amount)| (pool.as_str(), *amount))
        .collect::<Vec<_>>();
    source.produces(CAFE, &effects);
    let catalog = source.compile();
    let mut override_target = target(
        "tickets-push",
        &produced[3].0,
        40,
        overriding(Some("supersede")),
    );
    override_target["tasks"] = json!([CAFE]);
    let targets = Value::Array(vec![
        target("credits-floor", &produced[0].0, 100_000, adjust()),
        target("gems-floor", &produced[1].0, 3_000, adjust()),
        target("energy-floor", &produced[2].0, 500, adjust()),
        override_target,
    ]);
    let mut facts = base_facts();
    facts.facts = vec![
        credits(&instance, 0),
        inventory_fact(&instance, "resource.gems", 120, NOW - 60_000, NOW + HOUR),
        inventory_fact(&instance, "resource.tickets", 3, NOW - 60_000, NOW + HOUR),
    ];
    let policy = policy_fact(
        &catalog,
        &facts,
        &instance,
        &v2(&instance, valid, targets),
        NOW - 120_000,
    );
    facts.facts.push(policy.clone());
    facts.priority_offsets = vec![offset(CAFE, &instance, -2_000)];
    let evaluation = run(&catalog, &facts, 1_000);
    let longest = evaluation
        .decisions
        .iter()
        .flat_map(|decision| decision.reasons.iter())
        .map(|reason| reason.detail.len())
        .max()
        .unwrap_or_default();
    println!(
        "B7|six_resources|winner={}|longest_detail_bytes={longest}",
        winner(&evaluation, &instance)
    );
    for reason in &decision(&evaluation, CAFE, &instance).reasons {
        println!(
            "B7|six_resources|cafe|{}|bytes={}|{}",
            reason.code,
            reason.detail.len(),
            reason.detail
        );
    }
    for reason in &decision(&evaluation, NOTICE, &instance).reasons {
        if reason.code.starts_with("resource") || reason.code == "scored" {
            println!("B7|six_resources|notice|{}|{}", reason.code, reason.detail);
        }
    }
    // Unresolved: a policy relying on the declared terms meets a catalog without them.
    let adjust_doc = v2(
        &instance,
        valid,
        json!([target("credits-floor", &pool, 100_000, adjust())]),
    );
    let v3 = Catalog::v3().compile();
    let mut without_gap = Catalog::v3();
    without_gap.without_gap(0);
    let mut without_valuation = Catalog::v3();
    without_valuation.without_valuation(0);
    for (case, catalog) in [
        ("gap_removed", without_gap.compile()),
        ("valuation_removed", without_valuation.compile()),
    ] {
        let mut facts = base_facts();
        facts.facts = vec![credits(&instance, 0)];
        let policy = policy_fact(&v3, &facts, &instance, &adjust_doc, NOW - 120_000);
        facts.facts.push(policy);
        let evaluation = run(&catalog, &facts, 1_000);
        println!(
            "B7|{case}|winner={}|effective cafe={}|resource_targets={}|resource_weights={}|decision_record={}",
            winner(&evaluation, &instance),
            effective(&evaluation, CAFE, &instance),
            detail(&evaluation, CAFE, &instance, "resource_targets"),
            detail(&evaluation, CAFE, &instance, "resource_weights"),
            chain_reasons(&evaluation, &instance, &["decision_record"]).join(" | ")
        );
    }
    let _ = policy;
}

/// Amendment 5925475858: a stored v2 target without tasks that lapses at catalog level is
/// reported on every candidate of the instance; a producing task held back this round is not.
#[test]
fn amendment_catalog_level_lapse() {
    let instance = instance_id();
    let v3 = Catalog::v3();
    let pool = v3.credits_pool();
    let stored_on = v3.compile();
    let document = v2(
        &instance,
        Some(NOW + DAY),
        json!([target("credits-floor", &pool, 100_000, adjust())]),
    );
    let mut facts = base_facts();
    facts.facts = vec![credits(&instance, 0)];
    let policy = policy_fact(&stored_on, &facts, &instance, &document, NOW - 120_000);
    facts.facts.push(policy);
    // (a) a catalog update renames the pool (the task now produces the new id).
    let mut renamed = Catalog::v3();
    let renamed_id = format!("{pool}-renamed");
    renamed.pools["pools"][0]["id"] = json!(renamed_id);
    renamed.produces(CAFE, &[(renamed_id.as_str(), 1_000)]);
    // (b) a catalog update leaves no task producing the pool.
    let mut unproduced = Catalog::v3();
    unproduced.produces(CAFE, &[]);
    // (c) the unchanged catalog; the producing task just ran and is cooling down this round.
    let mut cooling = facts.clone();
    cooling.tasks = vec![
        serde_json::from_value(json!({
            "task_id": CAFE, "instance_id": instance, "last_dispatched_unix_ms": NOW - 1_000,
            "eligible_since_unix_ms": null, "terminal_state": null
        }))
        .expect("task state"),
    ];
    for (case, catalog, facts) in [
        ("a_pool_renamed", renamed.compile(), facts.clone()),
        ("b_no_producing_task", unproduced.compile(), facts.clone()),
        (
            "c_producer_held_back_this_round",
            Catalog::v3().compile(),
            cooling,
        ),
    ] {
        let evaluation = run(&catalog, &facts, 1_000);
        println!(
            "LAPSE|{case}|winner={}|decision_record={}",
            winner(&evaluation, &instance),
            chain_reasons(&evaluation, &instance, &["decision_record"]).join(" | ")
        );
        for task in [NOTICE, CAFE] {
            let entry = decision(&evaluation, task, &instance);
            println!(
                "LAPSE|{case}|{task}|state={:?}|last={}|effective={}|resource_target_tasks_unevaluable={}|resource_codes={:?}",
                entry.state,
                entry
                    .reasons
                    .last()
                    .map_or("none", |reason| reason.code.as_str()),
                effective(&evaluation, task, &instance),
                detail(
                    &evaluation,
                    task,
                    &instance,
                    "resource_target_tasks_unevaluable"
                ),
                entry
                    .reasons
                    .iter()
                    .filter(|reason| reason.code.starts_with("resource"))
                    .map(|reason| reason.code.clone())
                    .collect::<Vec<_>>()
            );
        }
    }
}
