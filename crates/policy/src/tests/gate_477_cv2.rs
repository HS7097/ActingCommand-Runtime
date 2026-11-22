// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #477 CV-2 (register §1.508; contracts/standard-pack-schedule.md, "`requires`";
// coordinator work order `rd-1122/brief-477-cv2.md`, its rulings 1, 2 and 6 and its tests 1, 2, 3,
// 5 and 7): a `schedule.json` that names `catalog_business` in `requires` writes none of its rules'
// business members, and the one composer takes each rule's business part from the root's version 2
// `pack-catalog.json`: the per-rule part where the package's entry holds one for the rule, else the
// entry's own part, else none.
// - a switched root composes to the bytes of the same root whose rules write the members: an
//   entry's own part, a per-rule part, the own part beside it, a rule whose package has no entry
//   (1);
// - each refusal of ruling 1 has its pointer and its word: a rule that writes one of the six
//   members, a version 1 catalog, none (2);
// - a compiler diagnostic on an effect that came from the catalog names the catalog's entry and
//   member, where the rule's root names the rule's (3);
// - a `model` origin in the catalog is reported as one written in a rule was (5);
// - a root that does not name the subsystem keeps its golden bytes and does not read the catalog
//   (7).
// A made-up game; every package, pool, rule and key is a placeholder. No clock is read.

use std::collections::BTreeMap;

use actingcommand_contract::{
    STANDARD_PACK_CATALOG_FILE, STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem,
    StandardPackRootBytes, StandardPackRootIssue, read_standard_pack_root,
};
use serde_json::{Map, Value, json};

use crate::{
    BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM, StandardPackBinding,
    StandardPackComposeFailure, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, compose_standard_pack_rules,
};

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const INSTANCE: &str = "fixture-instance-a";
/// The package, and its one rule, whose entry holds its own part.
const OWN: &str = "pack.own";
/// The package of two rules: the first has a per-rule part, the second takes the entry's own.
const SHARED: &str = "pack.shared";
const SHARED_A: &str = "pack.shared.a";
const SHARED_B: &str = "pack.shared.b";
/// The package, and its one rule, the catalog lists no entry for.
const BARE: &str = "pack.bare";
const POINTS: &str = "pool.points";
const VERSION_2: &str = "actingcommand.pack-catalog.v2";

/// One business part as the test writes it: the effects, the fixed value (its milli and whether the
/// model wrote it) and the limited mark.
#[derive(Debug, Clone, Default)]
struct Part {
    consumes: Vec<Value>,
    produces: Vec<Value>,
    value: Option<(u64, bool)>,
    limited: bool,
}

/// One effect on [`POINTS`].
fn effect(direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": POINTS,
        "direction": direction,
        "amount": amount,
        "observation_source": "self_reported",
        "confidence_milli": 1000
    })
}

/// [`OWN`]'s part: it consumes 6 points on a value the model wrote and is limited.
fn own_part() -> Part {
    Part {
        consumes: vec![effect("consume", 6)],
        value: Some((12_000, true)),
        limited: true,
        ..Part::default()
    }
}

/// [`SHARED_A`]'s per-rule part: it consumes 3 points on a value the pack wrote.
fn shared_a_part() -> Part {
    Part {
        consumes: vec![effect("consume", 3)],
        value: Some((5_000, false)),
        ..Part::default()
    }
}

/// [`SHARED`]'s own part, which stands for [`SHARED_B`]: it produces 2 points.
fn shared_own_part() -> Part {
    Part {
        produces: vec![effect("produce", 2)],
        ..Part::default()
    }
}

/// A `schedule.json` rule `id` running `package` on an hourly clock, without a business member.
fn bare_rule(id: &str, package: &str) -> Map<String, Value> {
    json!({
        "id": id,
        "package_id": package,
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": 3_600_000,
                "anchor_ms": 0
            }
        },
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86_400_000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
    .as_object()
    .cloned()
    .expect("a rule object")
}

/// The rule `id` of `package` writing `part` itself, after its other members, as a rule writes
/// it: its effects, its fixed value with its origin, its limited mark.
fn writing(id: &str, package: &str, part: &Part) -> Value {
    let mut rule = bare_rule(id, package);
    rule.insert("consumes".to_owned(), Value::Array(part.consumes.clone()));
    rule.insert("produces".to_owned(), Value::Array(part.produces.clone()));
    if let Some((milli, model)) = part.value {
        rule.insert("fixed_value_milli".to_owned(), json!(milli));
        if model {
            rule.insert("fixed_value_origin".to_owned(), json!("model"));
        }
    }
    if part.limited {
        rule.insert("limited".to_owned(), json!(true));
    }
    Value::Object(rule)
}

/// `part` as a version 2 catalog writes it: `consumes`, `produces`, `value` and `limited`.
fn catalog_members(part: &Part) -> Map<String, Value> {
    let mut members = Map::new();
    members.insert("consumes".to_owned(), Value::Array(part.consumes.clone()));
    members.insert("produces".to_owned(), Value::Array(part.produces.clone()));
    if let Some((milli, model)) = part.value {
        let value = if model {
            json!({"milli": milli, "origin": "model"})
        } else {
            json!({"milli": milli})
        };
        members.insert("value".to_owned(), value);
    }
    if part.limited {
        members.insert("limited".to_owned(), json!(true));
    }
    members
}

/// The root's four rules ([`OWN`], [`SHARED_A`], [`SHARED_B`], [`BARE`]): written without a business
/// member when `switched`, else each writing the part that stands for it in [`catalog`].
fn rules(switched: bool, own: &Part, shared_a: &Part, shared_own: &Part) -> Vec<Value> {
    let none = Part::default();
    [
        (OWN, OWN, own),
        (SHARED_A, SHARED, shared_a),
        (SHARED_B, SHARED, shared_own),
        (BARE, BARE, &none),
    ]
    .into_iter()
    .map(|(id, package, part)| {
        if switched {
            Value::Object(bare_rule(id, package))
        } else {
            writing(id, package, part)
        }
    })
    .collect()
}

/// The version 2 catalog of the root: [`OWN`]'s entry with its own part (`/packs/0`), [`SHARED`]'s
/// with its own part and [`SHARED_A`]'s per-rule part (`/packs/1`, `/packs/1/rules/0`); no entry of
/// [`BARE`].
fn catalog(own: &Part, shared_a: &Part, shared_own: &Part) -> Vec<u8> {
    let mut own_entry = Map::new();
    own_entry.insert("package_id".to_owned(), json!(OWN));
    own_entry.extend(catalog_members(own));
    let mut rule_part = Map::new();
    rule_part.insert("rule_id".to_owned(), json!(SHARED_A));
    rule_part.extend(catalog_members(shared_a));
    let mut shared_entry = Map::new();
    shared_entry.insert("package_id".to_owned(), json!(SHARED));
    shared_entry.extend(catalog_members(shared_own));
    shared_entry.insert("rules".to_owned(), json!([rule_part]));
    serde_json::to_vec(&json!({
        "schema_version": VERSION_2,
        "game": GAME,
        "packs": [own_entry, shared_entry]
    }))
    .expect("catalog bytes")
}

/// A pool of constant capacity 120 with a valuation block, without a scope.
fn pool() -> Value {
    json!({
        "id": POINTS,
        "capacity": 120,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": "resource.points"},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Points", "unit": "point", "scale": 1, "base_weight_milli": 1000}
    })
}

/// A `schedule.json` of `rules` and [`pool`], naming `requires`.
fn schedule(rules: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [pool()],
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// The `requires` of the root before it is switched, and after.
const WRITTEN: [&str; 1] = [BENEFIT_FIXED_VALUE_SUBSYSTEM];
const TAKEN: [&str; 2] = [BENEFIT_FIXED_VALUE_SUBSYSTEM, CATALOG_BUSINESS_SUBSYSTEM];

/// Composes `schedule` for one instance of the game, whose registry holds `packages`, with the
/// root's `catalog` bytes when given and no Runtime manifest.
fn compose_with(
    packages: &[&str],
    schedule: &Value,
    catalog: Option<&[u8]>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let bundle = json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packages
            .iter()
            .map(|package_id| json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            }))
            .collect::<Vec<_>>()
    });
    let bundle = serde_json::to_vec(&bundle).expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let irreversible = super::readable_root(&root);
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog,
        },
    )]);
    let instances = BTreeMap::from([(
        INSTANCE.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems: None,
        catalog_version: 1,
    })
}

/// [`compose_with`] of the fixture's three packages.
fn compose(
    schedule: &Value,
    catalog: Option<&[u8]>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    compose_with(&[OWN, SHARED, BARE], schedule, catalog)
}

/// The first refusal of composing `schedule` with `catalog`, as the composer names it.
fn first_issue(schedule: &Value, catalog: Option<&[u8]>) -> StandardPackRootIssue {
    let failure = compose(schedule, catalog).expect_err("the root is refused");
    let first = failure.issues().first().expect("an issue");
    assert_eq!(first.game_id.as_deref(), Some(GAME), "{failure}");
    first.issue.clone()
}

/// Test 1 (rulings 1 and 2): the root switched (its rules writing no business member, `requires`
/// naming `catalog_business`) with its version 2 catalog composes to the bytes the root composes to
/// whose rules write the members themselves, after their other members: the four documents, the
/// compiled catalog and its hash, the rules digest, the manifest, the coverage and who wrote each
/// value. [`OWN`] takes its entry's own part, [`SHARED_A`] its per-rule part, [`SHARED_B`] its
/// entry's own part beside that, and [`BARE`], whose package has no entry, none.
#[test]
fn cv2_t1_a_switched_root_composes_to_the_bytes_of_its_rules() {
    let (own, shared_a, shared_own) = (own_part(), shared_a_part(), shared_own_part());
    let written = schedule(rules(false, &own, &shared_a, &shared_own), &WRITTEN);
    let switched = schedule(rules(true, &own, &shared_a, &shared_own), &TAKEN);
    let catalog = catalog(&own, &shared_a, &shared_own);
    let before =
        compose(&written, None).unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let after = compose(&switched, Some(&catalog))
        .unwrap_or_else(|failure| panic!("the catalog's parts compose: {failure}"));
    let (rules_wrote, catalog_gave) = (before.sources(), after.sources());
    for (document, left, right) in [
        ("tasks", &rules_wrote.tasks, &catalog_gave.tasks),
        ("pools", &rules_wrote.pools, &catalog_gave.pools),
        ("activity", &rules_wrote.activity, &catalog_gave.activity),
        ("timeline", &rules_wrote.timeline, &catalog_gave.timeline),
    ] {
        assert_eq!(
            String::from_utf8_lossy(&left.bytes),
            String::from_utf8_lossy(&right.bytes),
            "{document}"
        );
    }
    assert_eq!(before.compiled(), after.compiled());
    assert_eq!(
        before.compiled().catalog_hash(),
        after.compiled().catalog_hash()
    );
    assert_eq!(before.rules_digest(), after.rules_digest());
    assert_eq!(before.manifest(), after.manifest());
    assert_eq!(before.benefit_coverage(), after.benefit_coverage());
    assert_eq!(before.benefit_provenance(), after.benefit_provenance());
    // Fix round 1 (the independent review): and what the composer derives beside them.
    assert_eq!(before.benefit_followers(), after.benefit_followers());
    assert_eq!(before.benefit_pending(), after.benefit_pending());
    assert_eq!(before.consumption_facts(), after.consumption_facts());

    // What the catalog gave each rule, as the composed tasks hold it.
    let tasks = &after.compiled().catalog().tasks.tasks;
    let task = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id == id)
            .unwrap_or_else(|| panic!("the task {id}: {tasks:?}"))
    };
    assert_eq!(task(OWN).consumes.len(), 1);
    assert_eq!(task(OWN).benefit_fixed_value_milli, Some(12_000));
    assert_eq!(task(OWN).benefit_limited, Some(true));
    assert_eq!(task(SHARED_A).consumes.len(), 1);
    assert_eq!(task(SHARED_A).benefit_fixed_value_milli, Some(5_000));
    assert_eq!(task(SHARED_A).benefit_limited, None);
    assert_eq!(task(SHARED_B).produces.len(), 1);
    assert!(task(SHARED_B).consumes.is_empty());
    assert_eq!(task(SHARED_B).benefit_fixed_value_milli, None);
    assert!(task(BARE).consumes.is_empty() && task(BARE).produces.is_empty());
    assert_eq!(task(BARE).benefit_fixed_value_milli, None);
}

/// Test 1, fix round 1 (the independent review): rules that write their effects in the middle, right
/// after `feedback_stop` (where `standard-pack adopt` writes them), their value and mark after the
/// other members: the switched root, whose rules take the part at their end, composes to the same
/// compiled catalog, hash, rules digest, manifest, coverage, provenance, followers, pending rules
/// and consumption facts, and the same tasks document member for member, in another member order
/// (the sources keep the order they are given; nothing decides by it).
#[test]
fn cv2_t1_effects_written_in_the_middle_of_a_rule_compose_the_same() {
    let (own, shared_a, shared_own) = (own_part(), shared_a_part(), shared_own_part());
    let none = Part::default();
    let middle = |id: &str, package: &str, part: &Part| {
        let mut rule = Map::new();
        for (name, value) in bare_rule(id, package) {
            let after_stop = name == "feedback_stop";
            rule.insert(name, value);
            if after_stop {
                rule.insert("consumes".to_owned(), Value::Array(part.consumes.clone()));
                rule.insert("produces".to_owned(), Value::Array(part.produces.clone()));
            }
        }
        if let Some((milli, model)) = part.value {
            rule.insert("fixed_value_milli".to_owned(), json!(milli));
            if model {
                rule.insert("fixed_value_origin".to_owned(), json!("model"));
            }
        }
        if part.limited {
            rule.insert("limited".to_owned(), json!(true));
        }
        Value::Object(rule)
    };
    let written = schedule(
        vec![
            middle(OWN, OWN, &own),
            middle(SHARED_A, SHARED, &shared_a),
            middle(SHARED_B, SHARED, &shared_own),
            middle(BARE, BARE, &none),
        ],
        &WRITTEN,
    );
    let switched = schedule(rules(true, &own, &shared_a, &shared_own), &TAKEN);
    let catalog = catalog(&own, &shared_a, &shared_own);
    let before =
        compose(&written, None).unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let after = compose(&switched, Some(&catalog))
        .unwrap_or_else(|failure| panic!("the catalog's parts compose: {failure}"));
    assert_eq!(before.compiled(), after.compiled());
    assert_eq!(
        before.compiled().catalog_hash(),
        after.compiled().catalog_hash()
    );
    assert_eq!(before.rules_digest(), after.rules_digest());
    assert_eq!(before.manifest(), after.manifest());
    assert_eq!(before.benefit_coverage(), after.benefit_coverage());
    assert_eq!(before.benefit_provenance(), after.benefit_provenance());
    assert_eq!(before.benefit_followers(), after.benefit_followers());
    assert_eq!(before.benefit_pending(), after.benefit_pending());
    assert_eq!(before.consumption_facts(), after.consumption_facts());
    let tasks = |composition: &StandardPackComposition| {
        serde_json::from_slice::<Value>(&composition.sources().tasks.bytes).expect("tasks JSON")
    };
    assert_eq!(tasks(&before), tasks(&after), "member for member");
}

/// Test 2 (ruling 1): a switched rule that writes one of the six members is refused at it in
/// `schedule.json`, `catalog_business_member`, one case each (the origin with its value is named
/// at the origin, the value alone at the value); a switched root whose catalog is of version 1 is
/// refused `catalog_version_1` at its `schema_version`, and one without a catalog
/// `catalog_missing`, the whole document, both in `pack-catalog.json`.
#[test]
fn cv2_t2_each_refusal_has_its_pointer_and_word() {
    let (own, shared_a, shared_own) = (own_part(), shared_a_part(), shared_own_part());
    let catalog = catalog(&own, &shared_a, &shared_own);
    let cases = [
        ("consumes", json!({"consumes": []})),
        ("produces", json!({"produces": []})),
        ("limited", json!({"limited": true})),
        ("realization", json!({"realization": true})),
        (
            "fixed_value_origin",
            json!({"fixed_value_milli": 1000, "fixed_value_origin": "model"}),
        ),
        ("fixed_value_milli", json!({"fixed_value_milli": 1000})),
    ];
    for (member, members) in cases {
        let mut switched = rules(true, &own, &shared_a, &shared_own);
        switched[0]
            .as_object_mut()
            .expect("a rule object")
            .extend(members.as_object().cloned().expect("the members"));
        assert_eq!(
            first_issue(&schedule(switched, &TAKEN), Some(&catalog)),
            StandardPackRootIssue {
                file: STANDARD_PACK_SCHEDULE_FILE,
                problem: StandardPackProblem::CatalogBusinessMember,
                pointer: format!("/rules/0/{member}"),
            },
            "{member}"
        );
    }

    let switched = schedule(rules(true, &own, &shared_a, &shared_own), &TAKEN);
    let version_1 = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.pack-catalog.v1",
        "game": GAME,
        "packs": []
    }))
    .expect("catalog bytes");
    assert_eq!(
        first_issue(&switched, Some(&version_1)),
        StandardPackRootIssue {
            file: STANDARD_PACK_CATALOG_FILE,
            problem: StandardPackProblem::CatalogVersion1,
            pointer: "/schema_version".to_owned(),
        }
    );
    assert_eq!(
        first_issue(&switched, None),
        StandardPackRootIssue {
            file: STANDARD_PACK_CATALOG_FILE,
            problem: StandardPackProblem::CatalogMissing,
            pointer: String::new(),
        }
    );
}

/// Test 3 (ruling 2): an effect above its pool's constant capacity is the compiler's
/// `effect_incompatible`. Written in the rules, it names the rule's effect in `schedule.json`;
/// taken from the catalog, the same diagnostics, one for one, name the effect of the part that
/// gave it: [`SHARED_A`]'s per-rule part's consumption (`/packs/1/rules/0/consumes/0`) and
/// [`SHARED`]'s own part's production, which [`SHARED_B`] took (`/packs/1/produces/0`).
#[test]
fn cv2_t3_a_compiler_diagnostic_on_a_catalog_effect_names_the_catalog() {
    let own = own_part();
    let shared_a = Part {
        consumes: vec![effect("consume", 500)],
        ..shared_a_part()
    };
    let shared_own = Part {
        produces: vec![effect("produce", 500)],
        ..shared_own_part()
    };
    let written = compose(
        &schedule(rules(false, &own, &shared_a, &shared_own), &WRITTEN),
        None,
    )
    .expect_err("the rules are refused");
    let taken = compose(
        &schedule(rules(true, &own, &shared_a, &shared_own), &TAKEN),
        Some(&catalog(&own, &shared_a, &shared_own)),
    )
    .expect_err("the parts are refused");
    // Each refusal: its file, its pointer, and whether the compiler's code is `effect_incompatible`.
    let places = |failure: &StandardPackComposeFailure| {
        failure
            .issues()
            .iter()
            .map(|issue| {
                assert_eq!(issue.issue.problem, StandardPackProblem::ScheduleInvalid);
                (
                    issue.issue.file,
                    issue.issue.pointer.clone(),
                    issue.detail.starts_with("effect_incompatible at /tasks/"),
                )
            })
            .collect::<Vec<_>>()
    };
    let written = places(&written);
    let taken = places(&taken);
    assert_eq!(taken.len(), written.len(), "{written:?} {taken:?}");
    for (file, pointer) in [
        (STANDARD_PACK_SCHEDULE_FILE, "/rules/1/consumes/0"),
        (STANDARD_PACK_SCHEDULE_FILE, "/rules/2/produces/0"),
    ] {
        assert!(
            written.contains(&(file, pointer.to_owned(), true)),
            "{written:?}"
        );
    }
    for (file, pointer) in [
        (STANDARD_PACK_CATALOG_FILE, "/packs/1/rules/0/consumes/0"),
        (STANDARD_PACK_CATALOG_FILE, "/packs/1/produces/0"),
    ] {
        assert!(
            taken.contains(&(file, pointer.to_owned(), true)),
            "{taken:?}"
        );
    }
}

/// Test 5 (ruling 6): the provenance of a switched root names each fixed value under the origin
/// its part's `value.origin` says, as the rule's `fixed_value_origin` said: [`OWN`]'s, `model`,
/// the model's; [`SHARED_A`]'s, without an origin, the pack's; the pool's valuation block the
/// pack's, as before.
#[test]
fn cv2_t5_a_model_origin_in_the_catalog_is_reported_as_a_rule_s() {
    let (own, shared_a, shared_own) = (own_part(), shared_a_part(), shared_own_part());
    let written = compose(
        &schedule(rules(false, &own, &shared_a, &shared_own), &WRITTEN),
        None,
    )
    .unwrap_or_else(|failure| panic!("the rules compose: {failure}"));
    let taken = compose(
        &schedule(rules(true, &own, &shared_a, &shared_own), &TAKEN),
        Some(&catalog(&own, &shared_a, &shared_own)),
    )
    .unwrap_or_else(|failure| panic!("the catalog's parts compose: {failure}"));
    assert_eq!(taken.benefit_provenance(), written.benefit_provenance());
    let [(game, provenance)] = taken.benefit_provenance() else {
        panic!("one game's provenance: {:?}", taken.benefit_provenance());
    };
    assert_eq!(game, GAME);
    assert_eq!(provenance.fixed_value.model, [OWN]);
    assert_eq!(provenance.fixed_value.pack, [SHARED_A]);
    assert_eq!(provenance.valuation.pack, [POINTS]);
    assert!(provenance.valuation.model.is_empty());
}

/// Test 7 (must not change): the neutral example catalog `catalog-a`'s task as a rule of a root
/// that does not name `catalog_business` composes to its golden tasks document, each member in its
/// place (the composer's members first, the rule's in its order, a scope at the end of the fact
/// node, `instance_overrides` and `benefit_judged` last); and the same root holding a catalog,
/// even one no reader takes, composes byte for byte the same: the file is not read for it.
#[test]
fn cv2_t7_a_root_that_is_not_switched_keeps_its_golden_bytes() {
    let parse = |text: &str| -> Value { serde_json::from_str(text).expect("example JSON") };
    let tasks = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
    ));
    let pools = parse(include_str!(
        "../../../../contracts/scheduling/examples/catalog-a/pools.json"
    ));
    let mut rule = tasks["tasks"][0].as_object().cloned().expect("the task");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        rule.shift_remove(member);
    }
    if let Some(Value::Object(fact)) = rule
        .get_mut("trigger")
        .and_then(|trigger| trigger.get_mut("predicates"))
        .and_then(|predicates| predicates.get_mut(1))
    {
        fact.shift_remove("scope");
    }
    rule.insert("yield_points".to_owned(), json!([]));
    rule.insert("package_id".to_owned(), json!("fixture.observe"));
    let mut fixture_pool = pools["pools"][0].as_object().cloned().expect("the pool");
    fixture_pool.shift_remove("scope");
    let plain = json!({
        "schema_version": "actingcommand.schedule.v1",
        "rules": [rule],
        "pools": [fixture_pool]
    });
    let composed = compose_with(&["fixture.observe"], &plain, None)
        .unwrap_or_else(|failure| panic!("the example composes: {failure}"));
    let golden = json!({
        "schema_version": "actingcommand.scheduling.v2",
        "catalog": {"catalog_id": "standard-packs", "catalog_version": 1, "approval_refs": []},
        "tasks": [{
            "id": "fixture.observe",
            "scope": {"kind": "instance", "instance_id": INSTANCE},
            "entrypoint": {"operation_id": "operation.package.run"},
            "procedure_ref": "package.fixture.observe",
            "priority": 100,
            "trigger": {
                "kind": "any",
                "predicates": [
                    {
                        "kind": "clock",
                        "schedule": {
                            "kind": "interval",
                            "clock_source": {"kind": "local"},
                            "every_ms": 3_600_000,
                            "anchor_ms": 0
                        }
                    },
                    {
                        "kind": "fact",
                        "fact_key": "resource.primary",
                        "comparison": "less_than",
                        "value": {"type": "integer", "value": 20},
                        "max_age_ms": 900_000,
                        "scope": {"kind": "instance", "instance_id": INSTANCE}
                    }
                ]
            },
            "feedback_stop": {
                "kind": "outcome",
                "task_id": "fixture.observe",
                "outcome_key": "completed",
                "comparison": "eq",
                "value": {"type": "boolean", "value": true}
            },
            "consumes": [],
            "produces": [{
                "pool_id": "fixture-pool-a",
                "direction": "produce",
                "amount": 1,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }],
            "on_failure": {
                "action": "continue",
                "retry_limit": 2,
                "retry_backoff_ms": 30000,
                "escalation_threshold": 3
            },
            "sensitive": false,
            "next_run_clamp_ms": 86_400_000,
            "yield_points": [],
            "expected_duration_ms": 60000,
            "cooldown_ms": 1000,
            "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
            "loop_budget": {
                "daily_limit": 24,
                "window_iteration_limit": 4,
                "max_runtime_ms": 300_000
            },
            "strategic_weight_milli": 1000,
            "instance_overrides": [],
            "benefit_judged": true
        }]
    });
    assert_eq!(
        String::from_utf8_lossy(&composed.sources().tasks.bytes),
        String::from_utf8_lossy(&serde_json::to_vec_pretty(&golden).expect("golden bytes"))
    );

    let with_catalog = compose_with(
        &["fixture.observe"],
        &plain,
        Some(b"not a catalog".as_slice()),
    )
    .unwrap_or_else(|failure| panic!("the catalog is not read: {failure}"));
    let (alone, beside) = (composed.sources(), with_catalog.sources());
    for (left, right) in [
        (&alone.tasks, &beside.tasks),
        (&alone.pools, &beside.pools),
        (&alone.activity, &beside.activity),
        (&alone.timeline, &beside.timeline),
    ] {
        assert_eq!(left.bytes, right.bytes);
    }
    assert_eq!(composed.compiled(), with_catalog.compiled());
    assert_eq!(composed.rules_digest(), with_catalog.rules_digest());
}
