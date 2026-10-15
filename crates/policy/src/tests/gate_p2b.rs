// SPDX-License-Identifier: AGPL-3.0-only

// P-2b gate tests of the policy crate: the second half of test plan v2.2 row P-2
// (`rd-1009/tests/test-ci-plan.md:163`; `inventory-policy.md` keys 20, 33 and 34, gaps G11, G12
// and G13, builders I4; `req-basis-cp.md` P-2 G11, G12 and G13). Every expected value comes from
// the cited requirement, not from what the code does today.
//
// - G11 (key 20): the PR-time check of one scheduling declaration, `validate_catalog_declaration`,
//   refuses each of the four documents with the code and at the field `compile_catalog` refuses it
//   with at parse level (contracts/resource-declarations.md: `scheduling/{tasks,pools,activity,
//   timeline}.json` are checked by "the production scheduling document parser and schema-version
//   check"); compile may add cross-document refusals of its own. Limited to the four documents:
//   the PR-time coverage of the selection document and of resource targets is not written for
//   now (coordinator-rulings-c-items.md:20).
// - G12 (key 33): the resource-target document builder (I4) for v1 and v2. Realistic mistakes meet
//   the contract's rejection reason at its field (contracts/resource-targets.md, the v1 and v2
//   tables and "The first failing check is reported"). The `shortfall_linear` term is 0 when the
//   target is met and never falls as the shortfall grows, and the other instance decides as
//   without the policy (contracts/resource-targets.md, "Evaluation" and "Resource weights (v2)").
// - G13 (key 34): a selection document enters the catalog hash, and the evaluation ranks by its
//   score (contracts/scheduling/README.md, "Canonical Serialization And Hash" step 3 and
//   "Score-Assisted Priority"). That a catalog without one keeps its existing hash is the frozen
//   V1 identity `neutral_catalog_compiles_to_stable_hash_and_dry_run` holds (key 19), which key 34
//   lists too. The Unknown (abort) sub-row is P-2c, written with D2 S1 (gate_d2_s1.rs).

use std::collections::BTreeMap;

use actingcommand_contract::{
    FactScalar as ContractFactScalar, RESOURCE_TARGETS_FACT_KEY, ResourceTargetsRejectionReason,
};
use serde_json::{Value, json};

use super::selection_catalog as fixture;
use crate::evaluator::tests::{
    NOW, base_facts, base_resources, decision_for, due_clock, false_fact, two_task_catalog,
};
use crate::{
    CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources, CheckedResourceTargets,
    CompiledCatalog, EvaluationFacts, EvaluationTime, FactScalar, FactValue, InstanceSnapshot,
    ObservedFact, PolicyEvaluation, ResourceTargetsError, SchedulingDocumentKind, ScopeSelector,
    check_resource_targets, compile_catalog, evaluate, parse_resource_targets,
    validate_catalog_declaration,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;

// The four documents the PR-time check covers.
const KINDS: [SchedulingDocumentKind; 4] = [
    SchedulingDocumentKind::Tasks,
    SchedulingDocumentKind::Pools,
    SchedulingDocumentKind::Activity,
    SchedulingDocumentKind::Timeline,
];

// G12: the catalog-a fixture (`contracts/scheduling/examples/catalog-a`) as the evaluator tests
// extend it. Instance A holds the policy; B is the other instance on the same server.
const INSTANCE_A: &str = "fixture-instance-a";
const INSTANCE_B: &str = "fixture-instance-b";
const SERVER: &str = "fixture-server-a";
const PRIMARY: &str = "fixture.observe";
// The task the target names; one run of it produces one unit of the pool.
const NAMED: &str = "fixture.observe-secondary";
// A task whose scope is instance B only.
const ELSEWHERE: &str = "fixture.observe-elsewhere";
// A task an instance override disables on instance A.
const DISABLED: &str = "fixture.observe-disabled";
// The pool of instance A and its inventory fact.
const POOL: &str = "fixture-pool-a";
const INVENTORY: &str = "resource.primary";
// The target: at least 50 units, step S = 1 and importance I = 1000, so both versions score
// 1000 per missing unit: v1 `s = min(floor(g * I * u / (S * U)), 1e6)` with `u = U`, and v2
// `R = min(floor(r * W / Q), 1e6)` with `W = Γ = min(floor(I * g / S), 1e6)`, `Q = S` (the
// pool declares no valuation) and `r = 1`.
const AT_LEAST: u64 = 50;
const SCALE: u64 = 1;
const IMPORTANCE: u64 = 1_000;
const SEED: u64 = 5;

// G13: two tasks on one instance; the lower priority scores far more than the priority gap
// weighs (an effective 1000 weighs one priority level).
const LEADING: &str = "task.leading";
const TRAILING: &str = "task.trailing";
const LEADING_PRIORITY: i16 = 100;
const TRAILING_PRIORITY: i16 = 90;
const TRAILING_SCORE: i64 = 50_000;
const SELECTION_INSTANCE: &str = "instance-a";

// P-2 G11 (key 20): every example catalog, the v2 timeline among them, passes the PR-time check
// document by document, and compiles.
#[test]
fn gate_p2b_g11_every_example_document_passes_the_pr_time_check_and_its_catalog_compiles() {
    for (name, sources) in [
        ("catalog-a", catalog_a()),
        ("h1-neutral-activity", h1_neutral_activity()),
        ("catalog-a as v2", as_v2(catalog_a())),
    ] {
        for kind in KINDS {
            validate_catalog_declaration(document(&sources, kind), kind).unwrap_or_else(
                |diagnostic| panic!("{name}: the {kind:?} document passes: {diagnostic:?}"),
            );
        }
        compile_catalog(&sources).unwrap_or_else(|failure| {
            panic!("{name}: the catalog compiles: {:?}", failure.diagnostics())
        });
    }
}

// P-2 G11 (key 20): a realistic mistake in one document is refused by the PR-time check with the
// code the requirement names, the other three documents pass, and the compiler refuses the
// catalog with that code, in that document, at that field.
#[test]
fn gate_p2b_g11_a_document_the_pr_time_check_refuses_is_refused_by_compile_with_its_code() {
    use CatalogDiagnosticCode as Code;
    use SchedulingDocumentKind as Kind;
    let rows = [
        (
            "an unknown task field",
            Kind::Tasks,
            Code::UnknownField,
            with_value(catalog_a(), Kind::Tasks, |tasks| {
                tasks["tasks"][0]["unexpected"] = json!(true);
            }),
        ),
        (
            "a repeated key",
            Kind::Tasks,
            Code::DuplicateKey,
            with_text(
                catalog_a(),
                Kind::Tasks,
                "\"priority\": 100,",
                "\"priority\": 100, \"priority\": 100,",
            ),
        ),
        (
            "a pool without its capacity",
            Kind::Pools,
            Code::MissingRequiredField,
            with_value(catalog_a(), Kind::Pools, |pools| {
                pools["pools"][0]
                    .as_object_mut()
                    .expect("the pool object")
                    .remove("capacity");
            }),
        ),
        (
            "a doubled comma",
            Kind::Pools,
            Code::InvalidJson,
            with_text(
                catalog_a(),
                Kind::Pools,
                "\"capacity\": 120,",
                "\"capacity\": 120,,",
            ),
        ),
        (
            "a budget written as text",
            Kind::Activity,
            Code::TypeMismatch,
            with_value(catalog_a(), Kind::Activity, |activity| {
                activity["profiles"][0]["daily_budget"] = json!("24");
            }),
        ),
        (
            "a legacy schema version",
            Kind::Timeline,
            Code::UnsupportedSchemaVersion,
            with_value(catalog_a(), Kind::Timeline, |timeline| {
                timeline["schema_version"] = json!("actingcommand.scheduling.v0");
            }),
        ),
        (
            "an unknown field of a v2 timeline event",
            Kind::Timeline,
            Code::UnknownField,
            with_value(as_v2(catalog_a()), Kind::Timeline, |timeline| {
                timeline["events"][0]["unexpected"] = json!(true);
            }),
        ),
    ];
    for (name, kind, code, sources) in rows {
        let refused = validate_catalog_declaration(document(&sources, kind), kind)
            .expect_err("the PR-time check refuses the document");
        assert_eq!(
            (refused.code, refused.source.document),
            (code, kind),
            "{name}: {refused:?}"
        );
        for other in KINDS.into_iter().filter(|other| *other != kind) {
            validate_catalog_declaration(document(&sources, other), other).unwrap_or_else(
                |diagnostic| {
                    panic!("{name}: the untouched {other:?} document passes: {diagnostic:?}")
                },
            );
        }
        let failure = compile_catalog(&sources).expect_err("the compiler refuses the catalog");
        assert!(
            failure.diagnostics().iter().any(|diagnostic| {
                diagnostic.code == refused.code
                    && diagnostic.source.document == kind
                    && diagnostic.json_path == refused.json_path
            }),
            "{name}: the compiler refuses with {:?} at {:?} in the {kind:?} document; it gave {:?}",
            refused.code,
            refused.json_path,
            failure.diagnostics()
        );
    }
}

// P-2 G11 (key 20): a reference across documents is the compiler's own refusal: each document
// passes the PR-time check on its own, and the catalog is refused as a dangling reference.
#[test]
fn gate_p2b_g11_a_dangling_pool_passes_each_document_check_and_compile_refuses_it() {
    let sources = with_value(catalog_a(), SchedulingDocumentKind::Tasks, |tasks| {
        tasks["tasks"][0]["produces"][0]["pool_id"] = json!("fixture-pool-missing");
    });
    for kind in KINDS {
        validate_catalog_declaration(document(&sources, kind), kind)
            .unwrap_or_else(|diagnostic| panic!("the {kind:?} document passes: {diagnostic:?}"));
    }
    let failure = compile_catalog(&sources).expect_err("the compiler refuses the catalog");
    assert!(
        failure.diagnostics().iter().any(|diagnostic| {
            diagnostic.code == CatalogDiagnosticCode::DanglingReference
                && diagnostic.source.document == SchedulingDocumentKind::Tasks
        }),
        "{:?}",
        failure.diagnostics()
    );
}

// P-2 G12 (key 33): with the I4 builder, each realistic mistake of a v1 and of a v2 document is
// refused with its reason at its field (contracts/resource-targets.md:49-82 and the v2 table): a
// version no reader takes, an instance the projection does not have, a lifetime already over, a
// repeated target id, a pool the catalog does not have, a task it does not have, a task whose
// scope leaves the instance out, and a task an instance override disables there.
#[test]
fn gate_p2b_g12_each_realistic_mistake_of_either_version_is_refused_with_its_reason_at_its_field() {
    use ResourceTargetsRejectionReason as Reason;
    let catalog = g12_catalog();
    let facts = g12_facts();
    let rows: [(&str, fn(&mut TargetsDocument), Reason, &str); 8] = [
        (
            "a version no reader takes",
            |document| document.schema_version = Some("actingcommand.resource-targets.v3"),
            Reason::UnsupportedSchemaVersion,
            "/schema_version",
        ),
        (
            "an instance the projection does not have",
            |document| document.instance = "fixture-instance-z",
            Reason::UnknownInstance,
            "/instance",
        ),
        (
            "a lifetime that ended a minute ago",
            |document| document.valid_until_unix_ms = Some(NOW - MINUTE_MS),
            Reason::ValidityOutOfRange,
            "/valid_until_unix_ms",
        ),
        (
            "a target id given twice",
            |document| {
                document.targets.push(TargetEntry {
                    id: "primary-floor",
                    resource: POOL,
                    tasks: vec![PRIMARY],
                });
            },
            Reason::DuplicateId,
            "/targets/1/id",
        ),
        (
            "a pool the catalog does not have",
            |document| document.targets[0].resource = "fixture-pool-missing",
            Reason::UnknownResource,
            "/targets/0/resource",
        ),
        (
            "a task the catalog does not have",
            |document| document.targets[0].tasks = vec!["fixture.observe-missing"],
            Reason::UnknownTask,
            "/targets/0/tasks/0",
        ),
        (
            "a task of another instance",
            |document| document.targets[0].tasks = vec![ELSEWHERE],
            Reason::TaskOutOfScope,
            "/targets/0/tasks/0",
        ),
        (
            "a task disabled on the instance",
            |document| document.targets[0].tasks = vec![DISABLED],
            Reason::TaskDisabled,
            "/targets/0/tasks/0",
        ),
    ];
    for version in [Version::V1, Version::V2] {
        judge(&TargetsDocument::new(version), &catalog, &facts).unwrap_or_else(|error| {
            panic!("{version:?}: the builder's document applies: {error:?}")
        });
        for (name, mistake, reason, field_path) in rows {
            let mut document = TargetsDocument::new(version);
            mistake(&mut document);
            let rejection = match judge(&document, &catalog, &facts) {
                Err(ResourceTargetsError::Rejected(rejection)) => rejection,
                other => panic!("{version:?}, {name}: refused, not {other:?}"),
            };
            assert_eq!(
                (rejection.reason, rejection.field_path.as_str()),
                (reason, field_path),
                "{version:?}, {name}: {rejection:?}"
            );
        }
    }
}

// P-2 G12 (key 33): a stored v1 or v2 policy of instance A scores its named candidate 0 when the
// target is met and never less as the shortfall grows (1000 per missing unit with this target),
// and instance B decides exactly as without A's policy.
#[test]
fn gate_p2b_g12_a_met_target_scores_nothing_more_shortfall_never_less_and_other_instances_are_unaffected()
 {
    let catalog = g12_catalog();
    let facts = g12_facts();
    for version in [Version::V1, Version::V2] {
        let policy = stored_policy(&TargetsDocument::new(version), &catalog, &facts);
        let mut previous: Option<i64> = None;
        for gap in [0_u64, 1, 2, 5, 20] {
            let current = i64::try_from(AT_LEAST - gap).expect("the inventory");
            let scored = evaluate_at(&catalog, &facts, current, Some(&policy));
            let effective = decision_for(&scored, NAMED, INSTANCE_A)
                .rank
                .as_ref()
                .expect("the named candidate's rank")
                .effective_milli;
            let expected = i64::try_from(gap * IMPORTANCE / SCALE).expect("the target score");
            assert_eq!(effective, expected, "{version:?}, gap {gap}");
            if let Some(previous) = previous {
                assert!(
                    effective >= previous,
                    "{version:?}, gap {gap}: {effective} < {previous}"
                );
            }
            previous = Some(effective);
            assert_eq!(
                instance_b(&scored),
                instance_b(&evaluate_at(&catalog, &facts, current, None)),
                "{version:?}, gap {gap}: instance B decides as without A's policy"
            );
        }
    }
}

// P-2 G13 (key 34): the selection document is part of the catalog identity: a catalog with it
// hashes apart from the same four documents without it, the same document hashes alike, and a
// changed score table is a new identity.
#[test]
fn gate_p2b_g13_a_selection_document_enters_the_catalog_hash() {
    let tasks = [(LEADING, LEADING_PRIORITY), (TRAILING, TRAILING_PRIORITY)];
    let without = fixture::catalog(&tasks, None);
    let with = fixture::catalog(&tasks, Some(&score_table(TRAILING_SCORE)));
    let again = fixture::catalog(&tasks, Some(&score_table(TRAILING_SCORE)));
    let changed = fixture::catalog(&tasks, Some(&score_table(TRAILING_SCORE - 10_000)));

    assert!(without.selection_policy().is_none());
    assert!(with.selection_policy().is_some());
    assert_ne!(with.catalog_hash(), without.catalog_hash());
    assert_eq!(with.catalog_hash(), again.catalog_hash());
    assert_ne!(changed.catalog_hash(), with.catalog_hash());
    assert_ne!(changed.catalog_hash(), without.catalog_hash());
}

// P-2 G13 (key 34): the evaluation ranks by the selection score. Without the document the
// higher priority task is selected; with it the lower priority task, whose score outweighs the
// priority gap, is selected, and each candidate's effective value is its score.
#[test]
fn gate_p2b_g13_the_evaluation_ranks_by_the_selection_score() {
    let tasks = [(LEADING, LEADING_PRIORITY), (TRAILING, TRAILING_PRIORITY)];
    let instances = [fixture::instance(SELECTION_INSTANCE, &[])];

    let unscored =
        fixture::evaluate_fixture(&fixture::catalog(&tasks, None), &instances, Vec::new())
            .unwrap_or_else(|error| panic!("the catalog without a document evaluates: {error:?}"));
    assert_eq!(selected(&unscored), [LEADING]);

    let scored = fixture::evaluate_fixture(
        &fixture::catalog(&tasks, Some(&score_table(TRAILING_SCORE))),
        &instances,
        Vec::new(),
    )
    .unwrap_or_else(|error| panic!("the catalog with a document evaluates: {error:?}"));
    assert_eq!(selected(&scored), [TRAILING]);
    for (task_id, score) in [(LEADING, 0), (TRAILING, TRAILING_SCORE)] {
        let rank = decision_for(&scored, task_id, SELECTION_INSTANCE)
            .rank
            .as_ref()
            .unwrap_or_else(|| panic!("{task_id} is ranked"));
        assert_eq!(rank.effective_milli, score, "{task_id}");
    }
    let total = |task_id: &str| {
        decision_for(&scored, task_id, SELECTION_INSTANCE)
            .rank
            .as_ref()
            .map(|rank| rank.total_score)
    };
    assert!(
        total(TRAILING) > total(LEADING),
        "the scored task ranks first: {:?} against {:?}",
        total(TRAILING),
        total(LEADING)
    );
}

// --- G11: the example catalogs and their mistakes ---

fn example(
    name: &str,
    tasks: &[u8],
    pools: &[u8],
    activity: &[u8],
    timeline: &[u8],
) -> CatalogSources {
    let source = |document: &str, bytes: &[u8]| {
        CatalogDocumentSource::new(format!("memory://{name}/{document}.json"), bytes.to_vec())
    };
    CatalogSources {
        tasks: source("tasks", tasks),
        pools: source("pools", pools),
        activity: source("activity", activity),
        timeline: source("timeline", timeline),
        selection: None,
    }
}

fn catalog_a() -> CatalogSources {
    example(
        "catalog-a",
        include_bytes!("../../../../contracts/scheduling/examples/catalog-a/tasks.json"),
        include_bytes!("../../../../contracts/scheduling/examples/catalog-a/pools.json"),
        include_bytes!("../../../../contracts/scheduling/examples/catalog-a/activity.json"),
        include_bytes!("../../../../contracts/scheduling/examples/catalog-a/timeline.json"),
    )
}

fn h1_neutral_activity() -> CatalogSources {
    example(
        "h1-neutral-activity",
        include_bytes!("../../../../contracts/scheduling/examples/h1-neutral-activity/tasks.json"),
        include_bytes!("../../../../contracts/scheduling/examples/h1-neutral-activity/pools.json"),
        include_bytes!(
            "../../../../contracts/scheduling/examples/h1-neutral-activity/activity.json"
        ),
        include_bytes!(
            "../../../../contracts/scheduling/examples/h1-neutral-activity/timeline.json"
        ),
    )
}

// The same catalog in schema v2: every document's version, and each timeline event's validity.
fn as_v2(mut sources: CatalogSources) -> CatalogSources {
    for kind in KINDS {
        sources = with_value(sources, kind, |document| {
            document["schema_version"] = json!(crate::SCHEDULING_SCHEMA_VERSION_V2);
            if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
                for event in events {
                    event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
                }
            }
        });
    }
    sources
}

fn document(sources: &CatalogSources, kind: SchedulingDocumentKind) -> &CatalogDocumentSource {
    match kind {
        SchedulingDocumentKind::Tasks => &sources.tasks,
        SchedulingDocumentKind::Pools => &sources.pools,
        SchedulingDocumentKind::Activity => &sources.activity,
        SchedulingDocumentKind::Timeline => &sources.timeline,
        SchedulingDocumentKind::Selection | SchedulingDocumentKind::ResourceTargets => {
            panic!("{kind:?} is not one of the four scheduling documents")
        }
    }
}

fn document_mut(
    sources: &mut CatalogSources,
    kind: SchedulingDocumentKind,
) -> &mut CatalogDocumentSource {
    match kind {
        SchedulingDocumentKind::Tasks => &mut sources.tasks,
        SchedulingDocumentKind::Pools => &mut sources.pools,
        SchedulingDocumentKind::Activity => &mut sources.activity,
        SchedulingDocumentKind::Timeline => &mut sources.timeline,
        SchedulingDocumentKind::Selection | SchedulingDocumentKind::ResourceTargets => {
            panic!("{kind:?} is not one of the four scheduling documents")
        }
    }
}

// The catalog with the `kind` document changed as JSON.
fn with_value(
    mut sources: CatalogSources,
    kind: SchedulingDocumentKind,
    change: impl FnOnce(&mut Value),
) -> CatalogSources {
    let source = document_mut(&mut sources, kind);
    let mut value: Value = serde_json::from_slice(&source.bytes).expect("the example JSON");
    change(&mut value);
    source.bytes = serde_json::to_vec_pretty(&value).expect("the document bytes");
    sources
}

// The catalog with the first `from` of the `kind` document's text replaced by `to`.
fn with_text(
    mut sources: CatalogSources,
    kind: SchedulingDocumentKind,
    from: &str,
    to: &str,
) -> CatalogSources {
    let source = document_mut(&mut sources, kind);
    let text = String::from_utf8(source.bytes.clone()).expect("the example text");
    assert!(text.contains(from), "{from:?} is in the {kind:?} example");
    source.bytes = text.replacen(from, to, 1).into_bytes();
    sources
}

// --- G12: the I4 resource-target builder and its catalog ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    V1,
    V2,
}

// I4: one resource-target document. Every target is an `adjust` target of at least AT_LEAST
// units that states its step, importance and rule, so the v1 and v2 documents differ only in
// their version.
#[derive(Debug, Clone)]
struct TargetsDocument {
    version: Version,
    // A version other than the builder's own.
    schema_version: Option<&'static str>,
    instance: &'static str,
    valid_until_unix_ms: Option<u64>,
    targets: Vec<TargetEntry>,
}

#[derive(Debug, Clone)]
struct TargetEntry {
    id: &'static str,
    resource: &'static str,
    tasks: Vec<&'static str>,
}

impl TargetsDocument {
    // Instance A keeps at least AT_LEAST units of its pool, produced by the named task, for the
    // next hour.
    fn new(version: Version) -> Self {
        Self {
            version,
            schema_version: None,
            instance: INSTANCE_A,
            valid_until_unix_ms: Some(NOW + HOUR_MS),
            targets: vec![TargetEntry {
                id: "primary-floor",
                resource: POOL,
                tasks: vec![NAMED],
            }],
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let version = self.schema_version.unwrap_or(match self.version {
            Version::V1 => crate::RESOURCE_TARGETS_SCHEMA_VERSION,
            Version::V2 => crate::RESOURCE_TARGETS_SCHEMA_VERSION_V2,
        });
        let targets = self
            .targets
            .iter()
            .map(|target| {
                json!({
                    "id": target.id,
                    "resource": target.resource,
                    "condition": {"kind": "at_least", "amount": AT_LEAST},
                    "scale": SCALE,
                    "importance_milli": IMPORTANCE,
                    "rule": "shortfall_linear",
                    "apply": {"mode": "adjust", "weight": "score_stage"},
                    "tasks": target.tasks,
                })
            })
            .collect::<Vec<_>>();
        let mut document = json!({
            "schema_version": version,
            "instance": self.instance,
            "targets": targets,
        });
        if let Some(valid_until) = self.valid_until_unix_ms {
            document["valid_until_unix_ms"] = json!(valid_until);
        }
        serde_json::to_vec(&document).expect("the document bytes")
    }
}

// The formal entry's parse and check, at NOW.
fn judge(
    document: &TargetsDocument,
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
) -> Result<CheckedResourceTargets, ResourceTargetsError> {
    let parsed = parse_resource_targets(&document.bytes())?;
    check_resource_targets(&parsed, catalog, facts, evaluation_time())
}

// Catalog-a with its two due tasks on server A (priority 100 and 0), a task of instance B only
// and a task an override disables on instance A.
fn g12_catalog() -> CompiledCatalog {
    two_task_catalog(|tasks| {
        for task in tasks.iter_mut() {
            task["scope"] = json!({"kind": "server", "server_id": SERVER});
            task["trigger"] = due_clock();
            task["feedback_stop"] = false_fact();
        }
        tasks[0]["priority"] = json!(100);
        tasks[1]["priority"] = json!(0);
        let mut elsewhere = tasks[1].clone();
        elsewhere["id"] = json!(ELSEWHERE);
        elsewhere["procedure_ref"] = json!("procedure.observe-elsewhere");
        elsewhere["scope"] = json!({"kind": "instance", "instance_id": INSTANCE_B});
        let mut disabled = tasks[1].clone();
        disabled["id"] = json!(DISABLED);
        disabled["procedure_ref"] = json!("procedure.observe-disabled");
        disabled["instance_overrides"] = json!([{
            "instance_id": INSTANCE_A,
            "enabled": false,
            "priority": null,
            "strategic_weight_milli": null,
            "load_profile": null
        }]);
        tasks.push(elsewhere);
        tasks.push(disabled);
    })
}

// The evaluator fixture's projection with instance B on the same server and host.
fn g12_facts() -> EvaluationFacts {
    let mut facts = base_facts();
    facts.instances.push(InstanceSnapshot {
        instance_id: INSTANCE_B.to_owned(),
        server_id: SERVER.to_owned(),
        game_id: "fixture-game-a".to_owned(),
        host_id: "fixture-host-a".to_owned(),
        available: true,
        capability_operation_ids: vec!["operation.observe".to_owned()],
        preferred_task_ids: Vec::new(),
    });
    facts
}

// The policy as the formal entry stores it: the checked rows as instance A's record list.
fn stored_policy(
    document: &TargetsDocument,
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
) -> ObservedFact {
    let checked = judge(document, catalog, facts)
        .unwrap_or_else(|error| panic!("{:?}: the document applies: {error:?}", document.version));
    let rows: Vec<BTreeMap<String, FactScalar>> = checked
        .rows
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|(field, value)| (field, scalar(value)))
                .collect()
        })
        .collect();
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: INSTANCE_A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(rows),
        observed_at_unix_ms: NOW,
        expires_at_unix_ms: document.valid_until_unix_ms,
        confidence_milli: 1_000,
    }
}

fn scalar(value: ContractFactScalar) -> FactScalar {
    match value {
        ContractFactScalar::Boolean(value) => FactScalar::Boolean(value),
        ContractFactScalar::Integer(value) => FactScalar::Integer(value),
        ContractFactScalar::String(value) => FactScalar::String(value),
        ContractFactScalar::TimestampMs(value) => FactScalar::TimestampMs(value),
        ContractFactScalar::DurationMs(value) => FactScalar::DurationMs(value),
    }
}

// One evaluation at NOW with instance A's inventory at `current` and, when given, its policy.
fn evaluate_at(
    catalog: &CompiledCatalog,
    facts: &EvaluationFacts,
    current: i64,
    policy: Option<&ObservedFact>,
) -> PolicyEvaluation {
    let mut facts = facts.clone();
    facts.facts.push(ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: INSTANCE_A.to_owned(),
        },
        fact_key: INVENTORY.to_owned(),
        value: FactValue::Integer(current),
        observed_at_unix_ms: NOW,
        expires_at_unix_ms: None,
        confidence_milli: 1_000,
    });
    facts.facts.extend(policy.cloned());
    evaluate(catalog, &facts, &base_resources(), evaluation_time(), SEED)
        .unwrap_or_else(|error| panic!("the round evaluates: {error:?}"))
}

// Everything instance B decided: its decisions, its intents and their reason chains.
fn instance_b(evaluation: &PolicyEvaluation) -> Vec<u8> {
    let intents = evaluation
        .dispatch_intents
        .iter()
        .filter(|intent| intent.instance_id == INSTANCE_B)
        .collect::<Vec<_>>();
    let chains = evaluation
        .reason_chains
        .iter()
        .filter(|chain| {
            intents
                .iter()
                .any(|intent| intent.decision_id == chain.decision_id)
        })
        .collect::<Vec<_>>();
    let decisions = evaluation
        .decisions
        .iter()
        .filter(|decision| decision.instance_id.as_deref() == Some(INSTANCE_B))
        .collect::<Vec<_>>();
    serde_json::to_vec(&(decisions, intents, chains)).expect("instance B's bytes")
}

fn evaluation_time() -> EvaluationTime {
    EvaluationTime {
        unix_ms: NOW,
        monotonic_ms: NOW,
    }
}

// --- G13: the score table ---

// A selection document whose only term reads the task priority through a table: the trailing
// task's priority scores TRAILING_SCORE (or `trailing_score`), every other priority 0.
fn score_table(trailing_score: i64) -> Value {
    fixture::selection(
        vec![fixture::integer_field("task.priority")],
        Vec::new(),
        Vec::new(),
        vec![json!({
            "term_id": "term-priority-table",
            "value": fixture::field_ref("task.priority"),
            "transform": {
                "kind": "lookup",
                "entries": [{
                    "key": {"type": "integer", "value": TRAILING_PRIORITY},
                    "value_milli": trailing_score
                }],
                "default_milli": 0
            },
            "weight_milli": 1000,
            "on_unknown": fixture::drop_candidate()
        })],
    )
}

// The task ids the round dispatches, in intent order.
fn selected(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}
