// SPDX-License-Identifier: AGPL-3.0-only

// #428 SCHED-C1c (register §1.299; the coordinator's rulings of 11-11, points 10 to 12, not Alice's
// words; coordinator work order `rd-1111/brief-sched-c1c.md`; contracts/scheduling/README.md,
// "Pool lines" with "The trigger line" and "A pool its lines cannot spend", "Capped spending order"
// with "Old triggers on a capped pool"; contracts/standard-pack-schedule.md, "A redundant
// trigger"):
// - a pool's optional `trigger_line` `T`: a capped episode begins at `r ≥ T` instead of at the
//   capacity and is spent at `r ≤ S`, `r < T`; its order, rounds and end are SS-2h's;
// - the member's refusals at its pointer, and `lines_invalid` with `trigger_line` at evaluation;
// - a catalog without the member keeps its golden hashes and carries no `trigger_line`;
// - a valued pool whose lines no consumer's run fits under from the capacity: the warning
//   `pool_lines_unspendable`, at compile time where the compiler can tell and as a decision reason;
// - the composer carries the member, requires `pool_trigger_line`, and hints at an old capacity
//   threshold on a valued pool with lines (`standard_pack_line_trigger_redundant`).
//
// The catalog is neutral and V2, as SS-2h's and SCHED-C1a's: tasks on one instance, the capped
// pool `pool.neutral` read from `resource.neutral`, its capacity read from
// `resource.neutral.capacity` (180), valued 1000 milli a unit (Q = 1), no regeneration, its stop
// line the capacity less 40 (140) and its hard line the capacity less 76 (104) unless a test says
// otherwise; `pool.out` valued 1000 milli a unit. A run is simulated: the pool is read again half a
// minute after the dispatch, and the next round is a minute after it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogCompileFailure, CatalogDiagnosticCode,
    CatalogDocumentSource, CatalogSources, ChainHandoverInputs, CompiledCatalog, DecisionReason,
    DiagnosticSeverity, DispatchIntent, EvaluationFacts, EvaluationResources, EvaluationTime,
    FACT_REFERENCE_SUBSYSTEM, FactRefreshInputs, FactRefreshSettings, HostResourceSnapshot,
    InstanceHolds, NoProgressGuards, OwedReruns, POOL_FACT_REFERENCE_SUBSYSTEM,
    POOL_LINES_SUBSYSTEM, POOL_TRIGGER_LINE_SUBSYSTEM, PolicyCode, PolicyEvaluation,
    PolicyEvaluationError, PoolLine, RemedyPlan, RunSlots, SCHEDULING_SCHEMA_VERSION,
    SCHEDULING_SCHEMA_VERSION_V2, SchedulingDecisionState, StandardPackBinding,
    StandardPackCompileWarning, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TaskDecision, TaskRuntimeSnapshot, TaskTerminalState, compile_catalog,
    compose_standard_pack_rules, evaluate_with_capped_spending, project_fact_pools,
};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    BundleIndex, StandardPackProblem, StandardPackRootBytes, read_standard_pack_root,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The capped pool, read from `READING_KEY`.
const POOL: &str = "pool.neutral";
const READING_KEY: &str = "resource.neutral";
/// The capacity fact of the capped pool, in its scope.
const CAPACITY_KEY: &str = "resource.neutral.capacity";
/// A fact of the pool's scope a hard line reads in test 4.
const HARD_KEY: &str = "resource.neutral.hard";
/// A fact of the pool's scope a trigger line reads in test 2.
const TRIGGER_KEY: &str = "resource.neutral.trigger";
/// The produced pool, 1000 milli a unit.
const OUT: &str = "pool.out";
/// The neutral example catalog's golden identities, computed before SS-2h (as in `gate_ss2h`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// Test 1, the trigger line. With `trigger_line` 20 below the cap (160), read at 175 between it and
// the cap, a capped episode begins: A (30 units, `V ÷ c` 1000) and B (25 units, 800) are frozen A
// then B and planned 175 → 145 → 120. A runs as the head (`capped_spend` episode 1, round 1, rank
// 1, `trigger_line` 160) while B waits (`capped_not_head`); B then runs as rank 2 of the same
// round, and at 120 (at or below the stop line, below the trigger line) the episode ends and
// nothing runs.
// Read at 160 an episode begins too (`r ≥ T`); at 159 none does, and the same pool without the
// member begins none at 175: A is judged at full price (B = 0, within its rounding error) and B is
// not positive, so nothing runs.
#[test]
fn sched_c1c_t1_an_episode_starts_at_the_trigger_line() {
    let consumers = || vec![judged("task_a", 30, 30), judged("task_b", 25, 20)];
    let mut world = World::new(catalog(consumers(), pools(Some(line(-20)))), 175);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    let head = spend(&first, "task_a");
    assert_eq!(head.value("capped_episode"), Some(&json!(1)));
    assert_eq!(head.value("capped_round"), Some(&json!(1)));
    assert_eq!(head.value("capped_rank"), Some(&json!(1)));
    assert_eq!(head.value("pool_capacity"), Some(&json!(180)));
    assert_eq!(head.value("stop_line"), Some(&json!(140)));
    assert_eq!(head.value("trigger_line"), Some(&json!(160)));
    assert_eq!(head.value("pool_reading"), Some(&json!(175)));
    assert_eq!(
        hold(decision(&first, "task_b")).as_deref(),
        Some("capped_not_head")
    );
    world.run(&first, 30);
    let second = world.evaluate();
    assert_eq!(dispatched(&second), ["task_b"], "{second:#?}");
    let next = spend(&second, "task_b");
    assert_eq!(next.value("capped_rank"), Some(&json!(2)), "frozen");
    assert_eq!(next.value("capped_round"), Some(&json!(1)));
    assert_eq!(next.value("trigger_line"), Some(&json!(160)));
    world.run(&second, 25);
    let third = world.evaluate();
    assert!(dispatched(&third).is_empty(), "{third:#?}");
    assert_eq!(world.memory.episode(A, POOL), None, "spent at 120");

    let mut at = World::new(catalog(consumers(), pools(Some(line(-20)))), 160);
    let evaluation = at.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    assert_eq!(
        spend(&evaluation, "task_a").value("pool_reading"),
        Some(&json!(160))
    );
    assert_eq!(at.memory.episode(A, POOL), Some((1, 1)));

    let mut below = World::new(catalog(consumers(), pools(Some(line(-20)))), 159);
    let evaluation = below.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    assert!(below.memory.is_empty());

    let mut plain = World::new(catalog(consumers(), pools(None)), 175);
    let evaluation = plain.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    assert!(plain.memory.is_empty());
    let full_price = decision(&evaluation, "task_a");
    assert!(
        last(full_price)
            .outcome
            .is(PolicyCode::BenefitWithinTolerance),
        "{full_price:#?}"
    );
    let negative = decision(&evaluation, "task_b");
    assert!(
        last(negative)
            .outcome
            .is(PolicyCode::PolicyBenefitNonpositive),
        "{negative:#?}"
    );
}

// Test 2, the member's refusals, each at its pointer, as `pool_lines_invalid` at `trigger_line`:
// written without both other lines (alone, and beside the stop line only, which is refused at
// `stop_line` as well); below the stop line (two constants, 130 under 140; two references to the
// capacity, −50 under −40) and below the hard line (100 under 104); above the capacity (a constant
// 200 of a constant 180; a reference to the capacity +10). Each form's own errors: a constant
// above 2^53−1 is `limit_exceeded` at the member; a reference whose key is no identifier
// `type_mismatch` at `trigger_line/fact_key`, whose offset is outside the safe range
// `limit_exceeded` at `trigger_line/offset`, that names a scope `unknown_field` at
// `trigger_line/scope`. A V1 document that names it is `unknown_field` at the member. At the stop
// line and at the capacity it compiles and is carried as written. At evaluation a trigger line
// read below the stop line (its own fact, 130) holds a consumer that would pass with
// `lines_invalid`, which carries `trigger_line`.
#[test]
fn sched_c1c_t2_the_member_is_refused_at_its_pointer() {
    const MEMBER: &str = "/pools/1/trigger_line";
    let constant = |trigger: Option<Value>| {
        let mut pool = valued(POOL, 1000, 180);
        pool["stop_line"] = json!(140);
        pool["hard_line"] = json!(104);
        if let Some(trigger) = trigger {
            pool["trigger_line"] = trigger;
        }
        pool
    };
    let refused = |case: &str, pool: Value, code: CatalogDiagnosticCode, path: &str| {
        let failure =
            compile_v2(vec![judged("task_a", 30, 30)], json!([out_pool(), pool])).expect_err(case);
        assert!(
            refused_at(&failure, code, path),
            "{case}: {:?}",
            failure.diagnostics()
        );
    };
    let invalid = CatalogDiagnosticCode::PoolLinesInvalid;
    let mut alone = valued(POOL, 1000, 180);
    alone["trigger_line"] = json!(160);
    refused("alone", alone, invalid, MEMBER);
    let mut beside_stop = valued(POOL, 1000, 180);
    beside_stop["stop_line"] = json!(140);
    beside_stop["trigger_line"] = json!(160);
    refused("beside the stop line", beside_stop.clone(), invalid, MEMBER);
    refused(
        "beside the stop line, the stop line",
        beside_stop,
        invalid,
        "/pools/1/stop_line",
    );
    for (case, pool) in [
        (
            "two constants below the stop line",
            constant(Some(json!(130))),
        ),
        ("a constant below the hard line", constant(Some(json!(100)))),
        ("a constant above the capacity", constant(Some(json!(200)))),
        (
            "a reference below the stop line",
            capped_pool(Some(line(-50))),
        ),
        (
            "a reference above the capacity",
            capped_pool(Some(line(10))),
        ),
    ] {
        refused(case, pool, invalid, MEMBER);
    }
    refused(
        "a constant above 2^53 - 1",
        constant(Some(json!(9_007_199_254_740_992_u64))),
        CatalogDiagnosticCode::LimitExceeded,
        MEMBER,
    );
    let unnamed = json!({"type": "fact", "fact_key": "Not An Identifier", "offset": 0});
    refused(
        "a key that is no identifier",
        capped_pool(Some(unnamed)),
        CatalogDiagnosticCode::TypeMismatch,
        "/pools/1/trigger_line/fact_key",
    );
    let far =
        json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": 9_007_199_254_740_992_i64});
    refused(
        "an offset outside the safe range",
        capped_pool(Some(far)),
        CatalogDiagnosticCode::LimitExceeded,
        "/pools/1/trigger_line/offset",
    );
    let scoped = json!({
        "type": "fact",
        "fact_key": CAPACITY_KEY,
        "offset": -20,
        "scope": {"kind": "instance", "instance_id": A}
    });
    refused(
        "a scope",
        capped_pool(Some(scoped)),
        CatalogDiagnosticCode::UnknownField,
        "/pools/1/trigger_line/scope",
    );
    let failure = compile_catalog(&example(SCHEDULING_SCHEMA_VERSION, |_, pools| {
        pools["pools"][0]["trigger_line"] = json!(10);
    }))
    .expect_err("V1");
    assert!(
        refused_at(
            &failure,
            CatalogDiagnosticCode::UnknownField,
            "/pools/0/trigger_line"
        ),
        "{:?}",
        failure.diagnostics()
    );

    for (pool, written) in [
        (constant(Some(json!(140))), json!(140)),
        (constant(Some(json!(180))), json!(180)),
        (capped_pool(Some(line(0))), line(0)),
    ] {
        let compiled = catalog(vec![judged("task_a", 30, 30)], json!([out_pool(), pool]));
        let carried = serde_json::to_value(&compiled.catalog().pools.pools).expect("pools");
        assert_eq!(carried[1]["trigger_line"], written, "{written}");
        assert!(carried[0].get("trigger_line").is_none(), "{written}");
    }
    let compiled = catalog(
        vec![judged("task_a", 30, 30)],
        json!([out_pool(), constant(Some(json!(150)))]),
    );
    assert_eq!(
        compiled.catalog().pools.pools[1].trigger_line,
        Some(PoolLine::Constant(150))
    );

    let own_fact = json!({"type": "fact", "fact_key": TRIGGER_KEY, "offset": 0});
    let mut world = World::new(
        catalog(vec![judged("task_p", 10, 30)], pools(Some(own_fact))),
        150,
    );
    world.extra.insert(TRIGGER_KEY.to_owned(), 130);
    let evaluation = world.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    let held = decision(&evaluation, "task_p");
    assert_eq!(hold(held).as_deref(), Some("lines_invalid"));
    let outcome = &last(held).outcome;
    assert_eq!(outcome.value("trigger_line"), Some(&json!(130)));
    assert_eq!(outcome.value("stop_line"), Some(&json!(140)));
    assert_eq!(outcome.value("hard_line"), Some(&json!(104)));
    assert_eq!(outcome.value("pool_capacity"), Some(&json!(180)));
}

// Test 3, identity. The neutral example catalog keeps its V1 and V2 golden identities; a valued
// pool with both lines and no trigger line serializes no `trigger_line`, its capped episode begins
// at the cap (180), and its `capped_spend` carries no `trigger_line`.
#[test]
fn sched_c1c_t3_without_the_member_nothing_changes() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example(version, |_, _| {}))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
    }
    let plain = catalog(vec![judged("task_a", 30, 30)], pools(None));
    let text = serde_json::to_string(&plain.catalog().pools.pools).expect("pools");
    assert!(!text.contains("trigger_line"), "{text}");
    let mut world = World::new(plain, 180);
    let evaluation = world.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    let head = reason(decision(&evaluation, "task_a"), PolicyCode::CappedSpend);
    assert_eq!(head.outcome.value("pool_reading"), Some(&json!(180)));
    assert_eq!(head.outcome.value("trigger_line"), None);
    assert!(!head.detail.contains("trigger_line"), "{}", head.detail);
}

// Test 4, a pool its lines cannot spend. Its capacity read at 180, its stop line 10 below the cap
// (170) and its hard line read from its own fact (`resource.neutral.hard`); A (30 units, `V ÷ c`
// 1000) and B (25 units, 800) are its judged consumers. Compiled with constant lines against a
// constant capacity (180, 170, 160), or with a hard line referencing the capacity's fact (−20 of
// it), where 180 − 160 = 20 is below both runs, the catalog compiles with the warning
// `pool_lines_unspendable` at the pool's `hard_line`, its values in its text; a composed file's
// warning is placed in its game's `schedule.json` (`/pools/1/hard_line`) and recorded so in the
// warnings fact. With a third consumer C of 20 units, or a constant hard line against a capacity
// read from a fact, none. At evaluation: with the hard line at 104 an episode begins at 250 and A
// runs as its head; the hard line read at 160 then leaves no run that fits from the cap, so at 220
// the episode waits (still episode 1, round 1) and A and B are deferred with
// `pool_lines_unspendable` {`pool_id`, `pool_capacity` 180, `stop_line` 170, `hard_line` 160,
// `least_consumption` 25} as their last reason; read at 104 again, B runs as rank 2 of the same
// round. Begun at 250 with the hard line at 160, no episode begins; with C one does (20 ≤ 20).
#[test]
fn sched_c1c_t4_a_pool_its_lines_cannot_spend_warns() {
    let two = || vec![judged("task_a", 30, 30), judged("task_b", 25, 20)];
    let three = || {
        let mut tasks = two();
        tasks.push(judged("task_c", 20, 20));
        tasks
    };
    let constants = || {
        let mut pool = valued(POOL, 1000, 180);
        pool["stop_line"] = json!(170);
        pool["hard_line"] = json!(160);
        pool
    };
    let referenced = || {
        let mut pool = capped_pool(None);
        pool["stop_line"] = line(-10);
        pool["hard_line"] = line(-20);
        pool
    };
    for (case, pool) in [("constants", constants()), ("references", referenced())] {
        let compiled = catalog(two(), json!([out_pool(), pool.clone()]));
        let found = unspendable(&compiled);
        let [warning] = found.as_slice() else {
            panic!("{case}: one warning: {:?}", compiled.warnings());
        };
        assert_eq!(warning.json_path, "/pools/1/hard_line", "{case}");
        assert_eq!(warning.severity, DiagnosticSeverity::Warning, "{case}");
        let text = &warning.reason;
        assert!(
            text.contains("pool_id=pool.neutral capacity_less_hard_line=20 least_consumption=25"),
            "{case}: {text}"
        );
        let fits = catalog(three(), json!([out_pool(), pool]));
        assert!(unspendable(&fits).is_empty(), "{case}");
    }
    let mut untold = capped_pool(None);
    untold["stop_line"] = json!(170);
    untold["hard_line"] = json!(160);
    let compiled = catalog(two(), json!([out_pool(), untold]));
    assert!(unspendable(&compiled).is_empty());

    let mut tight = schedule_pool(POOL);
    tight["stop_line"] = json!(230);
    tight["hard_line"] = json!(220);
    let tight_file = schedule(
        vec![schedule_rule("rule.a", hourly(), 30)],
        vec![schedule_pool(OUT), tight],
        &[POOL_LINES_SUBSYSTEM],
    );
    let composed =
        compose(&tight_file).unwrap_or_else(|failure| panic!("the tight file composes: {failure}"));
    let placed = StandardPackCompileWarning {
        game_id: Some(GAME.to_owned()),
        code: CatalogDiagnosticCode::PoolLinesUnspendable,
        pointer: "/pools/1/hard_line".to_owned(),
    };
    assert_eq!(composed.compile_warnings(), [placed.clone()]);
    let recorded = placed.to_contract();
    let code = PolicyCode::PoolLinesUnspendable.as_str();
    assert_eq!(recorded.warning, code);
    assert_eq!(recorded.game.as_deref(), Some(GAME));
    assert_eq!(recorded.pointer, "/pools/1/hard_line");

    let read_hard = || {
        let mut pool = capped_pool(None);
        pool["stop_line"] = line(-10);
        pool["hard_line"] = json!({"type": "fact", "fact_key": HARD_KEY, "offset": 0});
        json!([out_pool(), pool])
    };
    let mut world = World::new(catalog(two(), read_hard()), 250);
    world.extra.insert(HARD_KEY.to_owned(), 104);
    let first = world.evaluate();
    assert_eq!(dispatched(&first), ["task_a"], "{first:#?}");
    assert_eq!(
        spend(&first, "task_a").value("capped_rank"),
        Some(&json!(1))
    );
    world.run(&first, 30);
    world.extra.insert(HARD_KEY.to_owned(), 160);
    let waiting = world.evaluate();
    assert!(dispatched(&waiting).is_empty(), "{waiting:#?}");
    assert_eq!(world.memory.episode(A, POOL), Some((1, 1)), "it waits");
    for task_id in ["task_a", "task_b"] {
        let held = decision(&waiting, task_id);
        assert_eq!(held.state, SchedulingDecisionState::Deferred, "{task_id}");
        let warning = &last(held).outcome;
        assert!(
            warning.is(PolicyCode::PoolLinesUnspendable),
            "{task_id}: {held:#?}"
        );
        assert_eq!(warning.value("pool_id"), Some(&json!(POOL)), "{task_id}");
        assert_eq!(warning.value("pool_capacity"), Some(&json!(180)));
        assert_eq!(warning.value("stop_line"), Some(&json!(170)));
        assert_eq!(warning.value("hard_line"), Some(&json!(160)));
        assert_eq!(warning.value("least_consumption"), Some(&json!(25)));
    }
    world.extra.insert(HARD_KEY.to_owned(), 104);
    world.now += MINUTE_MS;
    let resumed = world.evaluate();
    assert_eq!(dispatched(&resumed), ["task_b"], "{resumed:#?}");
    let next = spend(&resumed, "task_b");
    assert_eq!(next.value("capped_episode"), Some(&json!(1)));
    assert_eq!(next.value("capped_round"), Some(&json!(1)));
    assert_eq!(next.value("capped_rank"), Some(&json!(2)));

    let mut never = World::new(catalog(two(), read_hard()), 250);
    never.extra.insert(HARD_KEY.to_owned(), 160);
    let evaluation = never.evaluate();
    assert!(dispatched(&evaluation).is_empty(), "{evaluation:#?}");
    assert!(never.memory.is_empty());
    assert!(
        last(decision(&evaluation, "task_b"))
            .outcome
            .is(PolicyCode::PoolLinesUnspendable),
        "{evaluation:#?}"
    );

    let mut fits = World::new(catalog(three(), read_hard()), 250);
    fits.extra.insert(HARD_KEY.to_owned(), 160);
    let evaluation = fits.evaluate();
    assert_eq!(dispatched(&evaluation), ["task_a"], "{evaluation:#?}");
    assert_eq!(fits.memory.episode(A, POOL), Some((1, 1)));
    assert!(
        evaluation.decisions.iter().all(|decision| {
            !decision
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::PoolLinesUnspendable))
        }),
        "{evaluation:#?}"
    );
}

// Test 5, the composer. A `schedule.json` pool's `trigger_line` (220 of a constant capacity of
// 240, beside 200 and 160) passes through to the composed pool as written and compiles into the
// pool's trigger line; a file that writes it without `pool_trigger_line` in `requires` is refused,
// `schedule_invalid` at the first pool that writes it, and one below its stop line (190) is refused
// at the same pointer, the compiler's `pool_lines_invalid` mapped back. The hint: of the judged
// rules consuming a valued pool with lines whose capacity is read from its fact, the one whose
// trigger requires through `all` the pool's projection at least the capacity less 20
// (`greater_than_or_equal`) and the one requiring under a nested `all` a fact on its reading key
// above it (`greater_than`) are one `standard_pack_line_trigger_redundant` each {`game`, `task_id`,
// `pool_id`}, in file order; a threshold under `any`, one compared `less_than_or_equal` and a
// constant threshold give none, and neither do the same rules on a pool without a valuation. The
// hint is none of the composition's own warnings.
#[test]
fn sched_c1c_t5_the_composer_carries_the_member_and_hints_at_old_triggers() {
    let lined = |trigger: u64| {
        let mut pool = schedule_pool(POOL);
        pool["stop_line"] = json!(200);
        pool["hard_line"] = json!(160);
        pool["trigger_line"] = json!(trigger);
        pool
    };
    let file = |trigger: u64, requires: &[&str]| {
        schedule(
            vec![schedule_rule("rule.a", hourly(), 10)],
            vec![schedule_pool(OUT), lined(trigger)],
            requires,
        )
    };
    let both = [POOL_LINES_SUBSYSTEM, POOL_TRIGGER_LINE_SUBSYSTEM];
    let composed =
        compose(&file(220, &both)).unwrap_or_else(|failure| panic!("the file composes: {failure}"));
    let document = |bytes: &[u8]| serde_json::from_slice::<Value>(bytes).expect("a document");
    let carried = document(&composed.sources().pools.bytes);
    assert_eq!(carried["pools"][1]["trigger_line"], json!(220));
    assert!(carried["pools"][0].get("trigger_line").is_none());
    let compiled = composed.compiled().catalog();
    assert_eq!(
        compiled.pools.pools[1].trigger_line,
        Some(PoolLine::Constant(220))
    );
    let failure = compose(&file(220, &[POOL_LINES_SUBSYSTEM])).expect_err("requires");
    let issue = &failure.issues()[0].issue;
    assert_eq!(issue.problem, StandardPackProblem::ScheduleInvalid);
    assert_eq!(issue.pointer, "/pools/1/trigger_line");
    let failure = compose(&file(190, &both)).expect_err("below the stop line");
    assert!(
        failure.issues().iter().any(|refused| {
            refused.issue.problem == StandardPackProblem::ScheduleInvalid
                && refused.issue.pointer == "/pools/1/trigger_line"
        }),
        "{failure}"
    );

    let threshold = |comparison: &str, value: Value| {
        json!({
            "kind": "resource_projection",
            "pool_id": POOL,
            "comparison": comparison,
            "value": value
        })
    };
    let near_cap = || json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": -20});
    let reading_above = json!({
        "kind": "fact",
        "fact_key": READING_KEY,
        "comparison": "greater_than",
        "value": near_cap(),
        "max_age_ms": DAY_MS
    });
    let at_least = threshold("greater_than_or_equal", near_cap());
    let at_most = threshold("less_than_or_equal", near_cap());
    let constant = threshold("greater_than_or_equal", json!(200));
    let nested = all(vec![all(vec![hourly(), reading_above])]);
    let rules = || {
        vec![
            schedule_rule("rule.a", all(vec![hourly(), at_least.clone()]), 10),
            schedule_rule("rule.b", nested.clone(), 10),
            schedule_rule("rule.c", any(vec![hourly(), at_least.clone()]), 10),
            schedule_rule("rule.d", all(vec![hourly(), at_most.clone()]), 10),
            schedule_rule("rule.e", all(vec![hourly(), constant.clone()]), 10),
        ]
    };
    let capped = || {
        let mut pool = schedule_pool(POOL);
        pool["capacity"] = line(0);
        pool["stop_line"] = line(-40);
        pool["hard_line"] = line(-76);
        pool
    };
    let requires = [
        FACT_REFERENCE_SUBSYSTEM,
        POOL_FACT_REFERENCE_SUBSYSTEM,
        POOL_LINES_SUBSYSTEM,
    ];
    let hinted_file = schedule(rules(), vec![schedule_pool(OUT), capped()], &requires);
    let hinted = compose(&hinted_file)
        .unwrap_or_else(|failure| panic!("the hinted file composes: {failure}"));
    let hints = hinted.redundant_trigger_warnings();
    let found = hints
        .iter()
        .map(|hint| {
            assert!(
                hint.is(SharedCode::StandardPackLineTriggerRedundant),
                "{hint:?}"
            );
            assert_eq!(hint.value("game"), Some(&json!(GAME)), "{hint:?}");
            let task_id = hint.value("task_id").cloned();
            (task_id, hint.value("pool_id").cloned())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found,
        [
            (Some(json!("rule.a")), Some(json!(POOL))),
            (Some(json!("rule.b")), Some(json!(POOL))),
        ]
    );
    let hinted_twice = hinted
        .warnings()
        .iter()
        .any(|warning| warning.is(SharedCode::StandardPackLineTriggerRedundant));
    assert!(!hinted_twice, "{:?}", hinted.warnings());

    let mut unvalued = capped();
    unvalued
        .as_object_mut()
        .expect("a pool object")
        .shift_remove("valuation");
    let plain_file = schedule(rules(), vec![schedule_pool(OUT), unvalued], &requires);
    let plain = compose(&plain_file)
        .unwrap_or_else(|failure| panic!("the unvalued file composes: {failure}"));
    assert!(plain.redundant_trigger_warnings().is_empty());
}

// ---- The world a test drives ----

/// The catalog, the Runtime's memory of the capped episodes, the capped pool's reading, other facts
/// of its scope and the tasks' snapshots.
struct World {
    catalog: CompiledCatalog,
    memory: CappedMemory,
    reading: i64,
    read_at: u64,
    now: u64,
    extra: BTreeMap<String, i64>,
    states: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The world at NOW, the capped pool read at `reading` a minute before.
    fn new(catalog: CompiledCatalog, reading: i64) -> Self {
        Self {
            catalog,
            memory: CappedMemory::default(),
            reading,
            read_at: NOW - MINUTE_MS,
            now: NOW,
            extra: BTreeMap::new(),
            states: Vec::new(),
        }
    }

    fn facts(&self) -> EvaluationFacts {
        let mut facts = vec![
            fact(A, READING_KEY, self.reading, self.read_at, None),
            fact(A, CAPACITY_KEY, 180, NOW - HOUR_MS, None),
        ];
        for (fact_key, value) in &self.extra {
            facts.push(fact(A, fact_key, *value, NOW - HOUR_MS, None));
        }
        EvaluationFacts {
            ledger_position: 7,
            fact_snapshot_id: "snapshot:fixture-sched-c1c".to_owned(),
            facts,
            outcomes: Vec::new(),
            tasks: self.states.clone(),
            instances: vec![instance(A, &[])],
            priority_offsets: Vec::new(),
        }
    }

    /// One round by the Runtime's entry, with its memory, every candidate admitted.
    fn evaluate(&mut self) -> PolicyEvaluation {
        let facts = self.facts();
        let resources = resources(&self.catalog, &facts);
        evaluate_with_capped_spending::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resources,
            time(self.now),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut self.memory,
            |_, _, _| Ok(false),
            |_: &DispatchIntent| -> Result<CandidateEligibility, PolicyEvaluationError> {
                Ok(CandidateEligibility::Eligible)
            },
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }

    /// Runs the round's one dispatch: it succeeds having consumed `spent`; the pool is read half a
    /// minute after the dispatch and the next round is a minute after it.
    fn run(&mut self, evaluation: &PolicyEvaluation, spent: i64) {
        let [intent] = evaluation.dispatch_intents.as_slice() else {
            panic!("one dispatch: {evaluation:#?}");
        };
        self.states.retain(|state| state.task_id != intent.task_id);
        self.states.push(TaskRuntimeSnapshot {
            task_id: intent.task_id.clone(),
            instance_id: A.to_owned(),
            last_dispatched_unix_ms: Some(self.now),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
        });
        self.reading -= spent;
        self.read_at = self.now + 30_000;
        self.now += MINUTE_MS;
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(catalog: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        catalog,
        facts,
        &EvaluationResources {
            pools: Vec::new(),
            hosts: vec![HostResourceSnapshot {
                host_id: format!("host.{A}"),
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
    )
}

/// The fact refresh off: no reader is bound.
fn refresh_inputs() -> FactRefreshInputs {
    FactRefreshInputs {
        settings: FactRefreshSettings {
            enabled: false,
            min_interval_ms: 300_000,
            daily_limit: 96,
            failure_backoff_ms: 600_000,
            max_consecutive_failures: 3,
            expected_duration_ms: crate::FACT_REFRESH_EXPECTED_DURATION_MS,
        },
        readers: Vec::new(),
        instance_facts_loaded_at_unix_ms: Some(NOW - DAY_MS),
        instance_holds: BTreeMap::new(),
        idle_closed: BTreeMap::new(),
    }
}

// ---- Assertions ----

fn dispatched(evaluation: &PolicyEvaluation) -> Vec<&str> {
    evaluation
        .dispatch_intents
        .iter()
        .map(|intent| intent.task_id.as_str())
        .collect()
}

fn decision<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|decision| decision.task_id == task_id && decision.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(decision: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    decision
        .reasons
        .iter()
        .find(|reason| reason.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {decision:#?}", code.as_str()))
}

fn last(decision: &TaskDecision) -> &DecisionReason {
    decision
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {decision:#?}"))
}

/// The `pool_line_hold` of the decision's last reason, when it is `pool_line_held`.
fn hold(decision: &TaskDecision) -> Option<String> {
    let last = last(decision);
    if !last.outcome.is(PolicyCode::PoolLineHeld) {
        return None;
    }
    last.outcome
        .value("pool_line_hold")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The `capped_spend` reason of `task_id`'s dispatch.
fn spend<'a>(evaluation: &'a PolicyEvaluation, task_id: &str) -> &'a Outcome {
    &reason(decision(evaluation, task_id), PolicyCode::CappedSpend).outcome
}

fn refused_at(failure: &CatalogCompileFailure, code: CatalogDiagnosticCode, path: &str) -> bool {
    failure
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.code == code && diagnostic.json_path == path)
}

/// The compiled catalog's `pool_lines_unspendable` warnings.
fn unspendable(compiled: &CompiledCatalog) -> Vec<crate::CatalogDiagnostic> {
    compiled
        .warnings()
        .iter()
        .filter(|warning| warning.code == CatalogDiagnosticCode::PoolLinesUnspendable)
        .cloned()
        .collect()
}

// ---- The catalog ----

fn effect(pool: &str, direction: &str, amount: u64) -> Value {
    json!({
        "pool_id": pool,
        "direction": direction,
        "amount": amount,
        "observation_source": "scan_verified",
        "confidence_milli": 1000
    })
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
}

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", amount)
}

/// Due while the capped pool holds `amount`.
fn enough(amount: u64) -> Value {
    json!({
        "kind": "resource_projection",
        "pool_id": POOL,
        "comparison": "greater_than_or_equal",
        "value": amount
    })
}

fn hourly() -> Value {
    json!({
        "kind": "clock",
        "schedule": {
            "kind": "interval",
            "clock_source": {"kind": "local"},
            "every_ms": HOUR_MS,
            "anchor_ms": 0
        }
    })
}

fn all(predicates: Vec<Value>) -> Value {
    json!({"kind": "all", "predicates": predicates})
}

fn any(predicates: Vec<Value>) -> Value {
    json!({"kind": "any", "predicates": predicates})
}

/// A task of the game on its hourly clock, of `priority`, unmarked.
fn task_body(id: &str, priority: i16, consumes: Vec<Value>, produces: Vec<Value>) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": priority,
        "trigger": hourly(),
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
        "next_run_clamp_ms": DAY_MS,
        "yield_points": [],
        "expected_duration_ms": 60_000,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": []
    })
}

/// A judged task: `consumed` units of the capped pool a run, due while the pool holds them, and
/// `produced` units of `pool.out`.
fn judged(id: &str, consumed: u64, produced: u64) -> Value {
    let mut task = task_body(
        id,
        100,
        vec![consume(POOL, consumed)],
        vec![produce(OUT, produced)],
    );
    task["benefit_judged"] = json!(true);
    task["trigger"] = enough(consumed);
    task
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, weighed `weight` milli a unit.
fn valued(id: &str, weight: u32, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
        "valuation": {"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": weight}
    })
}

fn out_pool() -> Value {
    valued(OUT, 1000, 1_000_000)
}

/// The capacity fact plus `offset`.
fn line(offset: i64) -> Value {
    json!({"type": "fact", "fact_key": CAPACITY_KEY, "offset": offset})
}

/// The capped pool: its capacity read from its fact, its stop line 40 below the cap and its hard
/// line 76 below it, and `trigger` its trigger line when given.
fn capped_pool(trigger: Option<Value>) -> Value {
    let mut pool = valued(POOL, 1000, 1);
    pool["capacity"] = line(0);
    pool["stop_line"] = line(-40);
    pool["hard_line"] = line(-76);
    if let Some(trigger) = trigger {
        pool["trigger_line"] = trigger;
    }
    pool
}

/// The produced pool and the capped pool ([`capped_pool`]).
fn pools(trigger: Option<Value>) -> Value {
    json!([out_pool(), capped_pool(trigger)])
}

fn catalog(tasks: Vec<Value>, pools: Value) -> CompiledCatalog {
    compile_v2(tasks, pools).unwrap_or_else(|failure| {
        panic!("the fixture catalog compiles: {:?}", failure.diagnostics())
    })
}

fn compile_v2(tasks: Vec<Value>, pools: Value) -> Result<CompiledCatalog, CatalogCompileFailure> {
    let header = json!({
        "catalog_id": "fixture.sched-c1c",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-sched-c1c"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({
            "schema_version": SCHEDULING_SCHEMA_VERSION_V2,
            "catalog": header,
        });
        document[member] = entries;
        document
    };
    let profile = json!({
        "id": "fixture-activity",
        "scope": {"kind": "game", "game_id": GAME},
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
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(tasks))),
        pools: source("pools", &document("pools", pools)),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
}

/// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
/// from 0, as its V2 golden was computed), its tasks and pools documents changed by `edit`.
fn example(version: &str, edit: impl FnOnce(&mut Value, &mut Value)) -> CatalogSources {
    let parse = |bytes: &[u8]| -> Value { serde_json::from_slice(bytes).expect("example JSON") };
    let mut documents = [
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/tasks.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/pools.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/activity.json"
        )),
        parse(include_bytes!(
            "../../../../contracts/scheduling/examples/catalog-a/timeline.json"
        )),
    ];
    for declared in &mut documents {
        declared["schema_version"] = json!(version);
        if version == SCHEDULING_SCHEMA_VERSION_V2
            && let Some(events) = declared.get_mut("events").and_then(Value::as_array_mut)
        {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
    }
    let [mut tasks, mut pools, activity, timeline] = documents;
    edit(&mut tasks, &mut pools);
    CatalogSources {
        tasks: source("example-tasks", &tasks),
        pools: source("example-pools", &pools),
        activity: source("example-activity", &activity),
        timeline: source("example-timeline", &timeline),
        selection: None,
    }
}

fn source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-sched-c1c/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

// ---- The standard-pack fixture of tests 4 and 5 ----

/// A `schedule.json` rule running the package of its own id on `trigger`, consuming `consumed` of
/// the capped pool and producing one `pool.out`.
fn schedule_rule(id: &str, trigger: Value, consumed: u64) -> Value {
    let mut rule = task_body(
        id,
        100,
        vec![consume(POOL, consumed)],
        vec![produce(OUT, 1)],
    );
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(id));
    members.insert("trigger".to_owned(), trigger);
    rule
}

/// A `schedule.json` pool: `id` of constant capacity 240, valued, without a scope.
fn schedule_pool(id: &str) -> Value {
    let mut pool = valued(id, 1000, 240);
    pool.as_object_mut()
        .expect("a pool object")
        .shift_remove("scope");
    pool
}

/// `schedule.json` with these rules and pools, naming `requires`.
fn schedule(rules: Vec<Value>, pools: Vec<Value>, requires: &[&str]) -> Value {
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": rules,
        "pools": pools,
        "activity": [],
        "timeline": [],
        "held": []
    })
}

/// Composes `schedule` for one instance of the game, whose registry holds a package per rule and
/// `rule.maint`, a return-home maintenance package; no Runtime manifest checks `requires`.
fn compose(schedule: &Value) -> Result<StandardPackComposition, crate::StandardPackComposeFailure> {
    let packages = schedule["rules"]
        .as_array()
        .expect("the rules")
        .iter()
        .map(|rule| rule["package_id"].as_str().expect("a package").to_owned())
        .chain(["rule.maint".to_owned()])
        .collect::<BTreeSet<_>>();
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
            .collect::<Vec<_>>(),
        "maintenance": [{"package_id": "rule.maint", "server": SERVER, "uses": ["return_home"]}]
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
    let maintenance = serde_json::from_slice::<BundleIndex>(&bundle)
        .expect("a bundle index")
        .maintenance()
        .to_vec();
    let schedule = serde_json::to_vec(schedule).expect("schedule bytes");
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &schedule,
            root: &root,
            maintenance: &maintenance,
        },
    )]);
    let instances = BTreeMap::from([(
        A.to_owned(),
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
