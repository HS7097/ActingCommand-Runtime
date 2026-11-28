// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL3 (0.12.104; register §1.559; the owner's words of 11-28 that the spending of a currency
// saved for something is pressed below its line, the harder the farther below, the rulings 1 to 9
// the coordinator's of 11-29; coordinator work order `rd-1123/brief-455-sl3.md`;
// contracts/scheduling/README.md, "A pool whose spending a line governs"): a judged candidate whose
// run would leave a pool below its line in effect, `a = r − c < L`, is charged
// `P = price_milli · min(d, scale) · min(c, d) ÷ scale²`, `d = L − a`, and carries
// `spend_line_pressed`; the codes that decide are the ones of before.
// - at or above the line, with a line past its own lifetime and with a line on another pool, the
//   evaluation equals, whole, the one without the stored line (t1);
// - a little below: `P` pinned, the candidate passes; a twin of lower value shows the item (t2);
// - far below: `f = 1` from `d ≥ scale` on, every unit at the full price (t3);
// - two spenders of one pool: the one with the least return for each unit stops first (t4);
// - the owner's two cases in neutral numbers (t5);
// - a line in effect and no reading: unknown, the reading asked for; no line: not read (t6);
// - a fixed value is `V − P`, a production `W·r − P` (t7);
// - a producer of the pool, one that also consumes it included, is never pressed (t8);
// - two such pools on one task, each its own `P` (t9);
// - a dispatch after the reading lowers the amount the line reads (t10).
//
// The fixture: one instance, the window open all day, judged tasks on an hourly clock; the currency
// (`pool.currency`, no valuation, capacity 100 000, `spend_line` 100 000 milli of 100 units: 1000
// milli a unit at the full price, reached 100 units below the line), a second such pool
// (`pool.second`, capacity 10 000, 50 000 milli of 50 units) and a valued pool (`pool.neutral`,
// 1000 milli a unit at the step 1) read at 5000, every reading a minute before NOW. The line is a
// stored v3 goal `{at_least}`, 400 on the currency unless a test says otherwise. Names are made up;
// no clock is read.

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{Initiator, RESOURCE_TARGETS_FACT_KEY, RuntimeChannel};
use serde_json::{Value, json};

use super::selection_catalog::{DAY_MS, HOUR_MS, NOW, fact, instance};
use crate::{
    CandidateEligibility, CappedMemory, CatalogDocumentSource, CatalogSources, ChainHandoverInputs,
    CompiledCatalog, DecisionReason, DispatchIntent, EvaluationFacts, EvaluationResources,
    EvaluationTime, FactRefreshInputs, FactRefreshSettings, FactValue, HostResourceSnapshot,
    InstanceHolds, InstancePoolLines, LeftOutFacts, NoProgressGuards, ObservedFact, OwedReruns,
    PolicyCode, PolicyEvaluation, PolicyEvaluationError, RemedyPlan, RunSlots,
    SCHEDULING_SCHEMA_VERSION_V2, ScopeSelector, TargetSetter, TaskDecision, TaskRuntimeSnapshot,
    TaskTerminalState, check_resource_targets, compile_catalog, evaluate_with_left_out_facts,
    parse_resource_targets, project_fact_pools,
};

const A: &str = "instance-a";
const GAME: &str = "game-a";
const OPERATION: &str = "operation.run";
const MINUTE_MS: u64 = 60_000;
/// The currency: no valuation, `spend_line` 100 000 milli of 100 units.
const CURRENCY: &str = "pool.currency";
const CURRENCY_KEY: &str = "resource.currency";
/// A second such pool: `spend_line` 50 000 milli of 50 units.
const SECOND: &str = "pool.second";
const SECOND_KEY: &str = "resource.second";
/// A valued pool, 1000 milli a unit at the step 1, read at 5000.
const VALUED: &str = "pool.neutral";
const VALUED_KEY: &str = "resource.neutral";
/// The line on the currency, unless a test says otherwise.
const LINE: u64 = 400;
/// The declared duration of every task, `E` without a snapshot.
const E: u64 = 60_000;

// t1 (rulings 3 and 6; the work order's first stop condition). The spenders: `task_spend` (`V`
// 50 000) and `task_lone` (no fixed value) consume 100 of the currency, read at 500; `task_earn`
// (`V` 800) produces 50 of it, `task_neutral` 2 of the valued pool. Without a record `task_spend`
// is dispatched. A line of 400 (each run leaves exactly 400), a line of 100, a line of 450 past its
// own lifetime and a line on the second pool, which no task consumes: each evaluation equals, whole,
// the one without the record.
#[test]
fn sl3_t1_at_or_above_the_line_and_without_a_line_in_effect_nothing_changes() {
    let world = || {
        World::new(
            vec![
                spender("task_spend", 50_000, 100),
                judged("task_lone", json!([consume(CURRENCY, 100)]), json!([])),
                fixed(
                    judged("task_earn", json!([]), json!([produce(CURRENCY, 50)])),
                    800,
                ),
                judged("task_neutral", json!([]), json!([produce(VALUED, 2)])),
            ],
            Some(500),
        )
    };
    let without = world().evaluate();
    assert_eq!(
        dispatched(&without).first(),
        Some(&"task_spend"),
        "{without:#?}"
    );
    let mut lapsing = line_target("lapsed", CURRENCY, 450);
    lapsing["valid_until_unix_ms"] = json!(NOW - MINUTE_MS);
    for (label, lined) in [
        (
            "at the line",
            world().with_lines(&[("line", CURRENCY, LINE)]),
        ),
        ("above it", world().with_lines(&[("line", CURRENCY, 100)])),
        (
            "past its own lifetime",
            world().with_document(&v3(vec![lapsing])),
        ),
        (
            "on another pool",
            world().with_lines(&[("line", SECOND, 100)]),
        ),
    ] {
        assert_eq!(lined.evaluate(), without, "{label}");
    }
}

// t2 (rulings 2, 3 and 4). `task_spend` (`V` 50 000) consumes 110 at 500 below a line of 400: `a`
// 390, `d` 10, 10 units below the line at a tenth of the full price, `P = 100 000 · 10 · 10 ÷
// 100² = 1000`. It still passes and is dispatched, `benefit_fixed_value` with `B` 49 000 first and
// `spend_line_pressed` next, its values and text pinned. Its twin `task_thin` (`V` 1000) is charged
// the same and is `policy_benefit_nonpositive` with `B` 0 and the item `pool.currency:-1000`.
#[test]
fn sl3_t2_a_spend_that_crosses_the_line_by_a_little_is_charged_and_still_passes() {
    let evaluation = World::new(
        vec![
            spender("task_spend", 50_000, 110),
            spender("task_thin", 1_000, 110),
        ],
        Some(500),
    )
    .with_lines(&[("line", CURRENCY, LINE)])
    .evaluate();
    assert_eq!(dispatched(&evaluation), ["task_spend"], "{evaluation:#?}");
    let spend = decision(&evaluation, "task_spend");
    assert_eq!(code_at(spend, 0), "benefit_fixed_value", "{spend:#?}");
    assert_eq!(code_at(spend, 1), "spend_line_pressed", "{spend:#?}");
    let fixed_value = &spend.reasons[0].outcome;
    assert_eq!(fixed_value.value("benefit_milli"), Some(&json!(49_000)));
    assert_eq!(fixed_value.value("fixed_value_milli"), Some(&json!(50_000)));
    let pressed = &spend.reasons[1];
    assert_eq!(
        pressed.detail,
        "pool_id=pool.currency line_at_least=400 pool_reading=500 consumption=110 \
         line_shortfall=10 pressing_milli=1000 pressing=1000/1"
    );
    assert_pressed(pressed, CURRENCY, (LINE, 500, 110, 10, 1000));

    let thin = decision(&evaluation, "task_thin");
    assert_pressed(&thin.reasons[1], CURRENCY, (LINE, 500, 110, 10, 1000));
    assert_eq!(
        last(thin).detail,
        "benefit_milli=0 expected_duration_ms=60000 items=pool.currency:-1000",
        "{thin:#?}"
    );
}

// t3 (ruling 3). The same spender (`V` 50 000, 110 a run) below the line of 400: read at 411 the
// shortfall is 99, 99 units at 99 % of the full price, `P` 98 010; at 410 it is 100, the full price
// on 100 units, 100 000; at 250 (260) and at 5 (505) every one of its 110 units is below the line at
// the full price, 110 000. Each is `policy_benefit_nonpositive` with `B = 50 000 − P`, its last
// reason, and the item `pool.currency:-<P>`.
#[test]
fn sl3_t3_far_below_the_line_each_unit_is_charged_at_the_full_price() {
    for (reading, shortfall, pressing) in [
        (411, 99, 98_010),
        (410, 100, 100_000),
        (250, 260, 110_000),
        (5, 505, 110_000),
    ] {
        let evaluation = World::new(vec![spender("task_spend", 50_000, 110)], Some(reading))
            .with_lines(&[("line", CURRENCY, LINE)])
            .evaluate();
        assert!(dispatched(&evaluation).is_empty(), "{reading}");
        let spend = decision(&evaluation, "task_spend");
        assert_pressed(
            reason(spend, PolicyCode::SpendLinePressed),
            CURRENCY,
            (LINE, reading, 110, shortfall, pressing),
        );
        let benefit = 50_000 - pressing;
        assert_eq!(
            last(spend).detail,
            format!(
                "benefit_milli={benefit} expected_duration_ms=60000 items=pool.currency:-{pressing}"
            ),
            "{reading}"
        );
        assert_eq!(
            code_at(spend, spend.reasons.len() - 1),
            "policy_benefit_nonpositive"
        );
    }
}

// t4 (ruling 3, the order the owner asked for). `task_low` (`V` 5000) and `task_high` (`V` 50 000)
// each consume 10 a run: 500 and 5000 of value for each unit. Read at 250 below a line of 400 each
// is charged its 10 units at the full price, 10 000: `task_low` is `policy_benefit_nonpositive`
// with `B` −5000, its last reason, `task_high` is dispatched with `B` 40 000, both carrying
// `spend_line_pressed`. Read at 405 each is charged 5 units at 5 % of the full price, 250, and
// neither is `policy_benefit_nonpositive`.
#[test]
fn sl3_t4_the_spender_with_the_least_return_for_each_unit_stops_first() {
    let world = |reading| {
        World::new(
            vec![
                spender("task_low", 5_000, 10),
                spender("task_high", 50_000, 10),
            ],
            Some(reading),
        )
        .with_lines(&[("line", CURRENCY, LINE)])
        .evaluate()
    };
    let far = world(250);
    assert_eq!(dispatched(&far), ["task_high"], "{far:#?}");
    let low = decision(&far, "task_low");
    assert_pressed(&low.reasons[1], CURRENCY, (LINE, 250, 10, 160, 10_000));
    assert_eq!(
        code_at(low, low.reasons.len() - 1),
        "policy_benefit_nonpositive"
    );
    assert_eq!(
        last(low).outcome.value("benefit_milli"),
        Some(&json!(-5_000))
    );
    let high = decision(&far, "task_high");
    assert_pressed(&high.reasons[1], CURRENCY, (LINE, 250, 10, 160, 10_000));
    assert_eq!(
        high.reasons[0].outcome.value("benefit_milli"),
        Some(&json!(40_000))
    );

    let near = world(405);
    for task_id in ["task_low", "task_high"] {
        let entry = decision(&near, task_id);
        assert_pressed(&entry.reasons[1], CURRENCY, (LINE, 405, 10, 5, 250));
        assert!(
            entry
                .reasons
                .iter()
                .all(|given| !given.outcome.is(PolicyCode::PolicyBenefitNonpositive)),
            "{task_id}: {entry:#?}"
        );
    }
}

// t5 (ruling 3, the owner's two cases in neutral numbers). A line of 400 and an amount of 500: a
// spend of 100 leaves 400 and the evaluation equals, whole, the one without the line; a spend of 120
// leaves 380 and is pressed on 20 units at a fifth of the full price, `P` 4000 (`B` 46 000). An
// amount of 300: a spend of 1 leaves 299, `d` 101, the one unit at the full price, `P` 1000.
#[test]
fn sl3_t5_the_owners_two_cases_in_neutral_numbers() {
    let alone =
        |consumed, reading| World::new(vec![spender("task_spend", 50_000, consumed)], reading);
    assert_eq!(
        alone(100, Some(500))
            .with_lines(&[("line", CURRENCY, LINE)])
            .evaluate(),
        alone(100, Some(500)).evaluate()
    );

    let more = alone(120, Some(500))
        .with_lines(&[("line", CURRENCY, LINE)])
        .evaluate();
    let spend = decision(&more, "task_spend");
    assert_pressed(&spend.reasons[1], CURRENCY, (LINE, 500, 120, 20, 4_000));
    assert_eq!(
        spend.reasons[0].outcome.value("benefit_milli"),
        Some(&json!(46_000))
    );

    let one = alone(1, Some(300))
        .with_lines(&[("line", CURRENCY, LINE)])
        .evaluate();
    let spend = decision(&one, "task_spend");
    assert_pressed(&spend.reasons[1], CURRENCY, (LINE, 300, 1, 101, 1_000));
    assert_eq!(
        spend.reasons[0].outcome.value("benefit_milli"),
        Some(&json!(49_000))
    );
}

// t6 (ruling 5). The currency not read and a line of 400 in effect: `task_spend` is `benefit_unknown`
// with `reading_unknown`, the pool, its key and `fact_missing`, as a regenerating pool's missing
// reading is; the key reaches the fact refresh (here no reader is bound: `fact_refresh_unbound` with
// the key); nothing is dispatched, no `benefit_fixed_value`, no `spend_line_pressed`. Without the
// line the pool is not read: `task_spend` is dispatched with `B` 50 000.
#[test]
fn sl3_t6_with_a_line_and_no_reading_the_benefit_is_unknown_and_the_reading_asked_for() {
    let unread = || World::new(vec![spender("task_spend", 50_000, 110)], None);
    let lined = unread().with_lines(&[("line", CURRENCY, LINE)]).evaluate();
    assert!(dispatched(&lined).is_empty(), "{lined:#?}");
    let spend = decision(&lined, "task_spend");
    let unknown = &reason(spend, PolicyCode::BenefitUnknown).outcome;
    assert_eq!(
        unknown.value("benefit_gap"),
        Some(&json!("reading_unknown"))
    );
    assert_eq!(unknown.value("pool_id"), Some(&json!(CURRENCY)));
    assert_eq!(unknown.value("fact_key"), Some(&json!(CURRENCY_KEY)));
    assert_eq!(
        unknown.value("unknown_reason"),
        Some(&json!("fact_missing"))
    );
    assert_eq!(
        reason(spend, PolicyCode::FactRefreshUnbound)
            .outcome
            .value("fact_key"),
        Some(&json!(CURRENCY_KEY))
    );
    assert!(
        spend.reasons.iter().all(|given| {
            !given.outcome.is(PolicyCode::BenefitFixedValue)
                && !given.outcome.is(PolicyCode::SpendLinePressed)
        }),
        "{spend:#?}"
    );

    let unlined = unread().evaluate();
    assert_eq!(dispatched(&unlined), ["task_spend"], "{unlined:#?}");
    let spend = decision(&unlined, "task_spend");
    assert_eq!(
        reason(spend, PolicyCode::BenefitFixedValue)
            .outcome
            .value("benefit_milli"),
        Some(&json!(50_000))
    );
    assert!(
        spend
            .reasons
            .iter()
            .all(|given| !given.outcome.is(PolicyCode::BenefitUnknown)),
        "{spend:#?}"
    );
}

// t7 (ruling 3). Read at 450 below a line of 400, a run of 110 leaves 340: `d` 60, 60 units at 60 %
// of the full price, `P` 36 000. `task_fixed` (`V` 40 000) is `B = 40 000 − 36 000 = 4000`,
// dispatched; `task_made` (no fixed value, producing 2 of the valued pool, 2000) is `B = 2000 −
// 36 000`, `policy_benefit_nonpositive` with the items `pool.neutral:+2000,pool.currency:-36000`,
// `spend_line_pressed` its first reason.
#[test]
fn sl3_t7_a_fixed_value_spender_is_v_less_p_and_a_producer_its_production_less_p() {
    let evaluation = World::new(
        vec![
            spender("task_fixed", 40_000, 110),
            judged(
                "task_made",
                json!([consume(CURRENCY, 110)]),
                json!([produce(VALUED, 2)]),
            ),
        ],
        Some(450),
    )
    .with_lines(&[("line", CURRENCY, LINE)])
    .evaluate();
    assert_eq!(dispatched(&evaluation), ["task_fixed"], "{evaluation:#?}");
    let fixed_one = decision(&evaluation, "task_fixed");
    assert_eq!(code_at(fixed_one, 0), "benefit_fixed_value");
    assert_eq!(
        fixed_one.reasons[0].outcome.value("benefit_milli"),
        Some(&json!(4_000))
    );
    assert_eq!(
        fixed_one.reasons[0].outcome.value("fixed_value_milli"),
        Some(&json!(40_000))
    );
    assert_pressed(
        &fixed_one.reasons[1],
        CURRENCY,
        (LINE, 450, 110, 60, 36_000),
    );

    let made = decision(&evaluation, "task_made");
    assert_pressed(&made.reasons[0], CURRENCY, (LINE, 450, 110, 60, 36_000));
    assert_eq!(
        last(made).detail,
        "benefit_milli=-34000 expected_duration_ms=60000 \
         items=pool.neutral:+2000,pool.currency:-36000",
        "{made:#?}"
    );
}

// t8 (ruling 6). `task_earn` (`V` 800) produces 50 of the currency; `task_swap` (`V` 5000) produces
// 20 of it and consumes 120. Read at 100, far below a line of 400: neither is pressed, and the
// evaluation equals, whole, the one without the line.
#[test]
fn sl3_t8_a_producer_of_the_pool_is_never_pressed() {
    let world = || {
        World::new(
            vec![
                fixed(
                    judged("task_earn", json!([]), json!([produce(CURRENCY, 50)])),
                    800,
                ),
                fixed(
                    judged(
                        "task_swap",
                        json!([consume(CURRENCY, 120)]),
                        json!([produce(CURRENCY, 20)]),
                    ),
                    5_000,
                ),
            ],
            Some(100),
        )
    };
    let lined = world().with_lines(&[("line", CURRENCY, LINE)]).evaluate();
    assert!(
        lined
            .decisions
            .iter()
            .flat_map(|entry| &entry.reasons)
            .all(|given| !given.outcome.is(PolicyCode::SpendLinePressed)),
        "{lined:#?}"
    );
    assert_eq!(lined, world().evaluate());
}

// t9 (ruling 3, several pools). `task_both` (`V` 2500) consumes 110 of the currency, read at 500
// below a line of 400 (`d` 10, `P` 1000), and 20 of the second pool, read at 110 below a line of 100
// (`d` 10, a fifth of its full price on 10 units, `P = 50 000 · 10 · 10 ÷ 50² = 2000`):
// `benefit_fixed_value` with `B` −500, then one `spend_line_pressed` for each pool in the order its
// consumption is declared, and `policy_benefit_nonpositive` with both items.
#[test]
fn sl3_t9_two_pools_a_line_governs_on_one_task_are_each_pressed() {
    let evaluation = World::new(
        vec![fixed(
            judged(
                "task_both",
                json!([consume(CURRENCY, 110), consume(SECOND, 20)]),
                json!([]),
            ),
            2_500,
        )],
        Some(500),
    )
    .with_second(110)
    .with_lines(&[("line", CURRENCY, LINE), ("second_line", SECOND, 100)])
    .evaluate();
    let both = decision(&evaluation, "task_both");
    assert_eq!(code_at(both, 0), "benefit_fixed_value", "{both:#?}");
    assert_eq!(
        both.reasons[0].outcome.value("benefit_milli"),
        Some(&json!(-500))
    );
    assert_pressed(&both.reasons[1], CURRENCY, (LINE, 500, 110, 10, 1_000));
    assert_pressed(&both.reasons[2], SECOND, (100, 110, 20, 10, 2_000));
    assert_eq!(
        both.reasons[2].detail,
        "pool_id=pool.second line_at_least=100 pool_reading=110 consumption=20 \
         line_shortfall=10 pressing_milli=2000 pressing=2000/1"
    );
    assert_eq!(
        last(both).detail,
        "benefit_milli=-500 expected_duration_ms=60000 \
         items=pool.currency:-1000,pool.second:-2000",
        "{both:#?}"
    );
}

// t10 (ruling 2: the amount is the one the lines read). The currency read at 500 a minute before
// NOW, and `task_other` (50 a run) dispatched half a minute before NOW, after the reading: the amount
// is 450, so `task_spend`'s run of 110 leaves 340, `d` 60, `P` 36 000. Without that dispatch the
// amount is 500, `d` 10, `P` 1000.
#[test]
fn sl3_t10_a_dispatch_after_the_reading_lowers_the_amount_the_line_reads() {
    let world = || {
        World::new(
            vec![
                spender("task_spend", 50_000, 110),
                spender("task_other", 1_000, 50),
            ],
            Some(500),
        )
        .with_lines(&[("line", CURRENCY, LINE)])
    };
    let after = world()
        .with_snapshot(TaskRuntimeSnapshot {
            task_id: "task_other".to_owned(),
            instance_id: A.to_owned(),
            last_dispatched_unix_ms: Some(NOW - MINUTE_MS / 2),
            eligible_since_unix_ms: None,
            terminal_state: Some(TaskTerminalState::Succeeded),
            completed_window: None,
            last_duration_ms: None,
            failure_streak: None,
            expected_runtime_ms: None,
            expected_runtime_runs: None,
            clock_owed: false,
        })
        .evaluate();
    let spend = decision(&after, "task_spend");
    assert_pressed(
        reason(spend, PolicyCode::SpendLinePressed),
        CURRENCY,
        (LINE, 450, 110, 60, 36_000),
    );
    let before = world().evaluate();
    let spend = decision(&before, "task_spend");
    assert_pressed(
        reason(spend, PolicyCode::SpendLinePressed),
        CURRENCY,
        (LINE, 500, 110, 10, 1_000),
    );
}

// ---- The world a test drives ----

/// The catalog, the readings, the instance's stored goal record and its task snapshots.
struct World {
    catalog: CompiledCatalog,
    /// The currency's reading a minute before NOW; `None`: not read.
    currency: Option<i64>,
    /// The second pool's reading a minute before NOW; `None`: not read.
    second: Option<i64>,
    /// The instance's `session.resource_targets` record; `None` without one.
    record: Option<ObservedFact>,
    /// The task snapshots the evaluation is given.
    snapshots: Vec<TaskRuntimeSnapshot>,
}

impl World {
    /// The catalog of `task_list` with the currency read at `currency`, no record, no snapshot.
    fn new(task_list: Vec<Value>, currency: Option<i64>) -> Self {
        Self {
            catalog: catalog(task_list),
            currency,
            second: None,
            record: None,
            snapshots: Vec::new(),
        }
    }

    /// The same world with the second pool read at `reading`.
    fn with_second(mut self, reading: i64) -> Self {
        self.second = Some(reading);
        self
    }

    /// The same world whose stored record is the v3 document of `lines`, each `(id, pool,
    /// at_least)`.
    fn with_lines(self, lines: &[(&str, &str, u64)]) -> Self {
        let targets = lines
            .iter()
            .map(|(id, pool, at_least)| line_target(id, pool, *at_least))
            .collect();
        self.with_document(&v3(targets))
    }

    /// The same world whose stored record is the one `document` writes.
    fn with_document(mut self, document: &Value) -> Self {
        self.record = Some(goal_record(&self.catalog, document));
        self
    }

    /// The same world with the task snapshot `snapshot`.
    fn with_snapshot(mut self, snapshot: TaskRuntimeSnapshot) -> Self {
        self.snapshots.push(snapshot);
        self
    }

    /// The readings, the record and the snapshots.
    fn facts(&self) -> EvaluationFacts {
        let mut facts = bare_facts();
        let read = |key: &str, value: i64| fact(A, key, value, NOW - MINUTE_MS, None);
        facts.facts.push(read(VALUED_KEY, 5_000));
        facts
            .facts
            .extend(self.currency.map(|value| read(CURRENCY_KEY, value)));
        facts
            .facts
            .extend(self.second.map(|value| read(SECOND_KEY, value)));
        facts.facts.extend(self.record.clone());
        facts.tasks.clone_from(&self.snapshots);
        facts
    }

    /// One round at NOW by the Runtime's entry, with a fresh memory, every candidate admitted.
    fn evaluate(&self) -> PolicyEvaluation {
        let facts = self.facts();
        let resource_snapshot = resources(&self.catalog, &facts);
        let mut memory = CappedMemory::default();
        evaluate_with_left_out_facts::<PolicyEvaluationError>(
            &self.catalog,
            &facts,
            &resource_snapshot,
            time(NOW),
            11,
            &BTreeSet::new(),
            &refresh_inputs(),
            &RemedyPlan::default(),
            &ChainHandoverInputs::default(),
            &RunSlots::default(),
            &OwedReruns::default(),
            &InstanceHolds::default(),
            &NoProgressGuards::default(),
            &mut memory,
            &InstancePoolLines::default(),
            &LeftOutFacts::default(),
            |_, _, _| Ok(false),
            |_: &DispatchIntent| Ok(CandidateEligibility::Eligible),
        )
        .unwrap_or_else(|error| panic!("the round evaluates: {error}"))
    }
}

/// The instance with no fact.
fn bare_facts() -> EvaluationFacts {
    EvaluationFacts {
        ledger_position: 7,
        fact_snapshot_id: "snapshot:fixture-455-sl3".to_owned(),
        facts: Vec::new(),
        outcomes: Vec::new(),
        tasks: Vec::new(),
        instances: vec![instance(A, &[])],
        priority_offsets: Vec::new(),
    }
}

fn time(at: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms: at,
        monotonic_ms: at,
    }
}

fn resources(compiled: &CompiledCatalog, facts: &EvaluationFacts) -> EvaluationResources {
    project_fact_pools(
        compiled,
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

// ---- The goal documents ----

/// The line `id` of `at_least` on `pool`: a target that carries nothing but its amount, without a
/// lifetime.
fn line_target(id: &str, pool: &str, at_least: u64) -> Value {
    json!({
        "id": id,
        "resource": pool,
        "condition": {"kind": "at_least", "amount": at_least},
        "apply": {"mode": "adjust", "weight": "score_stage"}
    })
}

/// The v3 document of `targets` for the instance.
fn v3(targets: Vec<Value>) -> Value {
    json!({
        "schema_version": "actingcommand.resource-targets.v3",
        "instance": A,
        "targets": targets
    })
}

/// The record `document` writes, checked against `compiled` and stored as the formal entry stores
/// it for a Lab connection, an hour before NOW.
fn goal_record(compiled: &CompiledCatalog, document: &Value) -> ObservedFact {
    let parsed = parse_resource_targets(&serde_json::to_vec(document).expect("document bytes"))
        .unwrap_or_else(|error| panic!("the goal parses: {error:?}"));
    let set_at = NOW - HOUR_MS;
    let accepted = check_resource_targets(&parsed, compiled, &bare_facts(), time(set_at))
        .unwrap_or_else(|error| panic!("the goal is accepted: {error:?}"));
    let composed = accepted
        .compose(
            None,
            Some(TargetSetter {
                set_unix_ms: set_at,
                channel: RuntimeChannel::Lab,
                initiator: Some(Initiator::LabCli),
            }),
        )
        .unwrap_or_else(|error| panic!("the goal composes: {error:?}"))
        .expect("with a setter every document composes");
    ObservedFact {
        scope: ScopeSelector::Instance {
            instance_id: A.to_owned(),
        },
        fact_key: RESOURCE_TARGETS_FACT_KEY.to_owned(),
        value: FactValue::RecordList(
            serde_json::from_value(serde_json::to_value(&composed.rows).expect("rows"))
                .expect("record list"),
        ),
        observed_at_unix_ms: set_at,
        expires_at_unix_ms: composed.expires_at_unix_ms,
        confidence_milli: 1000,
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
        .find(|entry| entry.task_id == task_id && entry.instance_id.as_deref() == Some(A))
        .unwrap_or_else(|| panic!("a decision for {task_id} on {A}: {evaluation:#?}"))
}

fn reason(entry: &TaskDecision, code: PolicyCode) -> &DecisionReason {
    entry
        .reasons
        .iter()
        .find(|given| given.outcome.is(code))
        .unwrap_or_else(|| panic!("a {} reason on {entry:#?}", code.as_str()))
}

/// The decision's last reason.
fn last(entry: &TaskDecision) -> &DecisionReason {
    entry
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason on {entry:#?}"))
}

/// The code of the decision's reason at `index`.
fn code_at(entry: &TaskDecision, index: usize) -> &str {
    entry
        .reasons
        .get(index)
        .unwrap_or_else(|| panic!("a reason at {index} on {entry:#?}"))
        .outcome
        .code()
        .as_str()
}

/// The values of a `spend_line_pressed`: `(line_at_least, pool_reading, consumption,
/// line_shortfall, pressing_milli)`.
type Pressed = (u64, i64, u64, u64, i64);

/// `given` is `spend_line_pressed` on `pool` with the values `expected`.
#[track_caller]
fn assert_pressed(given: &DecisionReason, pool: &str, expected: Pressed) {
    let (line, reading, consumed, shortfall, pressing) = expected;
    assert!(
        given.outcome.is(PolicyCode::SpendLinePressed),
        "spend_line_pressed: {given:#?}"
    );
    let outcome = &given.outcome;
    assert_eq!(outcome.value("pool_id"), Some(&json!(pool)), "{given:#?}");
    assert_eq!(
        outcome.value("line_at_least"),
        Some(&json!(line)),
        "{given:#?}"
    );
    assert_eq!(
        outcome.value("pool_reading"),
        Some(&json!(reading)),
        "{given:#?}"
    );
    assert_eq!(
        outcome.value("consumption"),
        Some(&json!(consumed)),
        "{given:#?}"
    );
    assert_eq!(
        outcome.value("line_shortfall"),
        Some(&json!(shortfall)),
        "{given:#?}"
    );
    assert_eq!(
        outcome.value("pressing_milli"),
        Some(&json!(pressing)),
        "{given:#?}"
    );
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

fn consume(pool: &str, amount: u64) -> Value {
    effect(pool, "consume", amount)
}

fn produce(pool: &str, amount: u64) -> Value {
    effect(pool, "produce", amount)
}

/// A judged task of the game on an hourly clock with `consumes` and `produces`, of priority 100.
fn judged(id: &str, consumes: Value, produces: Value) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "game", "game_id": GAME},
        "entrypoint": {"operation_id": OPERATION},
        "procedure_ref": format!("procedure.{id}"),
        "priority": 100,
        "trigger": {
            "kind": "clock",
            "schedule": {
                "kind": "interval",
                "clock_source": {"kind": "local"},
                "every_ms": HOUR_MS,
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
        "expected_duration_ms": E,
        "cooldown_ms": 1000,
        "load_profile": {"kind": "weighted", "cpu_milli": 200, "gpu_milli": 100, "io_milli": 300},
        "strategic_weight_milli": 1000,
        "instance_overrides": [],
        "benefit_judged": true
    })
}

/// `task` with the fixed value `value`.
fn fixed(mut task: Value, value: i64) -> Value {
    task["benefit_fixed_value_milli"] = json!(value);
    task
}

/// A judged task of the fixed value `value` consuming `consumed` of the currency a run.
fn spender(id: &str, value: i64, consumed: u64) -> Value {
    fixed(
        judged(id, json!([consume(CURRENCY, consumed)]), json!([])),
        value,
    )
}

/// A pool of the instance that does not regenerate, read from `resource.<the id without
/// "pool.">`, of constant `capacity`, without a valuation.
fn declared_pool(id: &str, capacity: u64) -> Value {
    json!({
        "id": id,
        "scope": {"kind": "instance", "instance_id": A},
        "capacity": capacity,
        "projection": {"kind": "none"},
        "observation": {"kind": "fact", "fact_key": format!("resource.{}", &id["pool.".len()..])},
        "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
        "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000}
    })
}

/// The currency and the second pool, each writing its `spend_line`, and the valued pool.
fn pools() -> Value {
    let mut currency = declared_pool(CURRENCY, 100_000);
    currency["spend_line"] =
        json!({"price_milli": 100_000, "scale": 100, "income_floor_milli_per_day": 0});
    let mut second = declared_pool(SECOND, 10_000);
    second["spend_line"] =
        json!({"price_milli": 50_000, "scale": 50, "income_floor_milli_per_day": 0});
    let mut valued = declared_pool(VALUED, 10_000);
    valued["valuation"] =
        json!({"name": "Fixture", "unit": "unit", "scale": 1, "base_weight_milli": 1000});
    json!([currency, second, valued])
}

fn catalog(task_list: Vec<Value>) -> CompiledCatalog {
    let header = json!({
        "catalog_id": "fixture.455-sl3",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-455-sl3"]
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
    let source = |name: &str, declared: &Value| {
        CatalogDocumentSource::new(
            format!("memory://fixture-455-sl3/{name}.json"),
            serde_json::to_vec(declared).expect("fixture document bytes"),
        )
    };
    compile_catalog(&CatalogSources {
        tasks: source("tasks", &document("tasks", Value::Array(task_list))),
        pools: source("pools", &document("pools", pools())),
        activity: source("activity", &document("profiles", json!([profile]))),
        timeline: source("timeline", &document("events", json!([]))),
        selection: None,
    })
    .unwrap_or_else(|failure| panic!("the catalog compiles: {:?}", failure.diagnostics()))
}
