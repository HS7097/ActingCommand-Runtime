// SPDX-License-Identifier: AGPL-3.0-only

// #428 COV-1 (register §1.299; coordinator work order `rd-1111/brief-cov1.md`, the coordinator's
// ruling of 11-11, not Alice's words; contracts/standard-pack-schedule.md, "Coverage"):
// `actinglab standard-pack check` and `package bundle --schedule` count a judged rule that declares
// no value and, by its trigger alone, only runs in its chain head's run window as a *follower*
// (`benefit_coverage.follower`), not as undeclared and with no `standard_pack_benefit_undeclared`,
// and list it with the complete rule it rides with (`benefit_followers`). Every other rule that
// declares no value stays undeclared with its warning, and an answer for a file without followers
// is the one of before. Neutral fixtures only: game `game-a`, packages on server `server-a`.

use crate::StandardPackCheckResponse;
use crate::standard_pack_schedule::{ScheduleReport, readable_root, report_schedule};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    BundleMaintenance, MaintenanceUse, StandardPackRoot, StandardPackRootBytes,
    read_standard_pack_root,
};
use actingcommand_policy::{BENEFIT_FIXED_VALUE_SUBSYSTEM, BenefitCoverage};
use serde_json::{Value, json};
use std::path::Path;

const GAME: &str = "game-a";
const SERVER: &str = "server-a";
/// A pool with a valuation.
const VALUED: &str = "pool.valued";
/// A pool without a valuation.
const BARE: &str = "pool.bare";
/// The complete rule a chain rides with.
const HEAD: &str = "fixture.head";

// A head valued by its effects and one rule whose whole trigger is an edge to it (a
// `dependency_completed` accepting `succeeded` alone, with `since_own_run: true`): the rule is a
// follower, counted apart, warned about by nothing, and listed with its head.
#[test]
fn cov1_t1_a_rule_riding_with_a_valued_head_is_a_follower_and_not_warned_about() {
    let rules = [
        rule(
            HEAD,
            clock(),
            Vec::new(),
            vec![effect(VALUED, "produce", 1)],
        ),
        rule("fixture.follower", edge(HEAD), Vec::new(), Vec::new()),
    ];

    let report = check(&rules, &[], &[]);
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 1,
            follower: 1,
            ..BenefitCoverage::default()
        }
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        serde_json::to_value(&report.benefit_followers).expect("the followers"),
        json!([{"task_id": "fixture.follower", "rides_with": HEAD}])
    );
    assert_eq!(
        serde_json::to_string(&report.benefit_coverage).expect("the coverage"),
        r#"{"complete":1,"pending":0,"undeclared":0,"maintenance_declared":0,"realization":0,"fixed_value":0,"follower":1}"#
    );
}

// A head with a fixed value, a rule that rides with it through an edge under `all`, and a rule
// before both in the file that rides with that follower: both are followers of the head, listed
// in file order.
#[test]
fn cov1_t2_a_follower_of_a_follower_rides_with_the_valued_head() {
    let mut head = rule(HEAD, clock(), Vec::new(), Vec::new());
    head["fixed_value_milli"] = json!(12_000);
    let rules = [
        rule(
            "fixture.second",
            all(vec![edge("fixture.first")]),
            Vec::new(),
            Vec::new(),
        ),
        head,
        rule(
            "fixture.first",
            all(vec![clock(), edge(HEAD)]),
            Vec::new(),
            Vec::new(),
        ),
    ];

    let report = check(&rules, &[BENEFIT_FIXED_VALUE_SUBSYSTEM], &[]);
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 1,
            fixed_value: 1,
            follower: 2,
            ..BenefitCoverage::default()
        }
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(
        serde_json::to_value(&report.benefit_followers).expect("the followers"),
        json!([
            {"task_id": "fixture.second", "rides_with": HEAD},
            {"task_id": "fixture.first", "rides_with": HEAD}
        ])
    );
}

// Each way of not being a follower: a level link (no `since_own_run`), an edge under `any`, an
// edge that also accepts `failed`, an edge to a rule that declares itself maintenance, an edge to
// a rule whose package is maintenance-role, an edge to the valued head beside a link to that
// unmarked rule, an edge to a rule that is itself undeclared, and an edge to the valued head from
// a rule that consumes a pool without a valuation. Each stays undeclared with its warning, in
// file order, and nothing is listed.
#[test]
fn cov1_t3_each_way_of_not_being_a_follower_stays_undeclared_with_its_warning() {
    let mut maintenance = rule("fixture.maintenance", clock(), Vec::new(), Vec::new());
    maintenance["maintenance"] = json!(true);
    let rules = [
        rule(
            HEAD,
            clock(),
            Vec::new(),
            vec![effect(VALUED, "produce", 1)],
        ),
        rule(
            "fixture.level",
            after(HEAD, &["succeeded"], None),
            Vec::new(),
            Vec::new(),
        ),
        rule(
            "fixture.alternative",
            json!({"kind": "any", "predicates": [edge(HEAD), clock()]}),
            Vec::new(),
            Vec::new(),
        ),
        rule(
            "fixture.failed_too",
            after(HEAD, &["succeeded", "failed"], Some(true)),
            Vec::new(),
            Vec::new(),
        ),
        maintenance,
        rule(
            "fixture.after_maintenance",
            edge("fixture.maintenance"),
            Vec::new(),
            Vec::new(),
        ),
        rule("fixture.role", clock(), Vec::new(), Vec::new()),
        rule(
            "fixture.after_role",
            edge("fixture.role"),
            Vec::new(),
            Vec::new(),
        ),
        rule(
            "fixture.beside_role",
            all(vec![
                edge(HEAD),
                after("fixture.role", &["succeeded"], None),
            ]),
            Vec::new(),
            Vec::new(),
        ),
        rule("fixture.undeclared", clock(), Vec::new(), Vec::new()),
        rule(
            "fixture.after_undeclared",
            edge("fixture.undeclared"),
            Vec::new(),
            Vec::new(),
        ),
        rule(
            "fixture.unvalued",
            edge(HEAD),
            vec![effect(BARE, "consume", 1)],
            Vec::new(),
        ),
    ];
    // `fixture.role` is maintenance-role by a bundle maintenance entry: unmarked, not judged.
    let role = [BundleMaintenance {
        package_id: "fixture.role".to_owned(),
        server: SERVER.to_owned(),
        uses: vec![MaintenanceUse::ReturnHome],
    }];

    let report = check(&rules, &[], &role);
    assert_eq!(
        report.benefit_coverage,
        BenefitCoverage {
            complete: 1,
            undeclared: 9,
            maintenance_declared: 1,
            ..BenefitCoverage::default()
        }
    );
    let found = report
        .warnings
        .iter()
        .map(|warning| {
            assert!(
                warning.is(SharedCode::StandardPackBenefitUndeclared),
                "{warning:?}"
            );
            assert_eq!(warning.value("game"), Some(&json!(GAME)), "{warning:?}");
            (
                warning.value("task_id").cloned(),
                warning.value("pool_id").cloned(),
            )
        })
        .collect::<Vec<_>>();
    let expected = [
        ("fixture.level", None),
        ("fixture.alternative", None),
        ("fixture.failed_too", None),
        ("fixture.after_maintenance", None),
        ("fixture.after_role", None),
        ("fixture.beside_role", None),
        ("fixture.undeclared", None),
        ("fixture.after_undeclared", None),
        ("fixture.unvalued", Some(BARE)),
    ]
    .map(|(rule_id, pool_id)| (Some(json!(rule_id)), pool_id.map(|pool| json!(pool))));
    assert_eq!(found, expected);
    assert!(
        report.benefit_followers.is_empty(),
        "{:?}",
        report.benefit_followers
    );
}

// A file whose rules require no other rule: the coverage serializes as before COV-1, without
// `follower`, and the check's answer carries no `benefit_followers`.
#[test]
fn cov1_t4_a_file_without_followers_answers_as_before() {
    let rules = [
        rule(
            HEAD,
            clock(),
            Vec::new(),
            vec![effect(VALUED, "produce", 1)],
        ),
        rule("fixture.none", clock(), Vec::new(), Vec::new()),
    ];

    let report = check(&rules, &[], &[]);
    assert_eq!(
        serde_json::to_string(&report.benefit_coverage).expect("the coverage"),
        r#"{"complete":1,"pending":0,"undeclared":1,"maintenance_declared":0,"realization":0,"fixed_value":0}"#
    );
    assert!(
        report.benefit_followers.is_empty(),
        "{:?}",
        report.benefit_followers
    );
    let answer = serde_json::to_value(StandardPackCheckResponse {
        status: "checked".to_owned(),
        root: "root".to_owned(),
        game: GAME.to_owned(),
        root_files: Vec::new(),
        pack_count: rules.len(),
        warnings: report.warnings,
        benefit_coverage: Some(report.benefit_coverage),
        benefit_provenance: Some(report.benefit_provenance),
        benefit_followers: report.benefit_followers,
        // #428 SCHED-C1f1: the answer's struct gained this member.
        consumption_facts: Some(report.consumption_facts),
        // Workflow #444 NOTICES-3: the answer's struct gained this member.
        notices: None,
        // Workflow #453 S8: the answer's struct gained this member.
        models: Vec::new(),
    })
    .expect("the answer");
    assert!(answer.get("benefit_followers").is_none(), "{answer}");
    assert!(
        answer["benefit_coverage"].get("follower").is_none(),
        "{answer}"
    );
}

/// The report of `schedule.json` with `rules`, `requires` and the valued and bare pools, against
/// a root with a package per rule and the bundle's `maintenance` entries.
fn check(rules: &[Value], requires: &[&str], maintenance: &[BundleMaintenance]) -> ScheduleReport {
    let packages = rules
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package id"))
        .collect::<Vec<_>>();
    let schedule = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": [pool(VALUED, true), pool(BARE, false)],
        "activity": [],
        "timeline": [],
        "held": []
    }))
    .expect("schedule bytes");
    let game_root = root(&packages);
    report_schedule(
        GAME,
        &game_root,
        maintenance,
        &schedule,
        Path::new("schedule.json"),
        // Workflow #455 G-R2s: every package of the root reads, none irreversible.
        &readable_root(&game_root),
    )
    .unwrap_or_else(|error| panic!("the schedule is checked: {error}"))
}

/// The game root: a bundle index v4 with a package per rule.
fn root(packages: &[&str]) -> StandardPackRoot {
    let packs = packages
        .iter()
        .map(|package_id| {
            json!({
                "package_id": package_id,
                "server": SERVER,
                "entry_task_id": "task",
                "path": format!("packs/{package_id}"),
                "file_count": 1,
                "byte_count": 64
            })
        })
        .collect::<Vec<_>>();
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": packs
    }))
    .expect("bundle bytes");
    read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json")
}

/// An hourly clock.
fn clock() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": 3600000,
            "anchor_ms": 0
        }
    })
}

/// `dependency_completed` of `task_id` accepting `states`, with `since_own_run` when given.
fn after(task_id: &str, states: &[&str], since_own_run: Option<bool>) -> Value {
    let mut node = json!({
        "kind": "dependency_completed",
        "task_id": task_id,
        "terminal_states": states
    });
    if let Some(since_own_run) = since_own_run {
        node["since_own_run"] = json!(since_own_run);
    }
    node
}

/// An edge: `dependency_completed` of `task_id` accepting `succeeded` alone, `since_own_run: true`.
fn edge(task_id: &str) -> Value {
    after(task_id, &["succeeded"], Some(true))
}

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

/// A pool of constant capacity 1000 read from `resource.<the id without "pool.">`; with `valued`,
/// valued 1000 milli a unit.
fn pool(id: &str, valued: bool) -> Value {
    let mut pool = json!({
        "id": id,
        "capacity": 1000,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    });
    if valued {
        pool["valuation"] =
            json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    }
    pool
}

/// A `schedule.json` rule running the package of its own id on `trigger`.
fn rule(id: &str, trigger: Value, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "package_id": id,
        "priority": 100,
        "trigger": trigger,
        "feedback_stop": {
            "kind": "outcome",
            "task_id": id,
            "outcome_key": "completed",
            "comparison": "eq",
            "value": {"type": "boolean", "value": true}
        },
        "consumes": consumes,
        "produces": produces,
        "on_failure": {
            "action": "continue",
            "retry_limit": 2,
            "retry_backoff_ms": 30000,
            "escalation_threshold": 3
        },
        "sensitive": false,
        "next_run_clamp_ms": 86400000,
        "yield_points": [],
        "expected_duration_ms": 60000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "light"},
        "strategic_weight_milli": 1000
    })
}
