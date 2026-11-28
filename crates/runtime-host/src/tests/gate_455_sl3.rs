// SPDX-License-Identifier: AGPL-3.0-only

// #455 SL3 (0.12.104; register §1.559; the coordinator's rulings of 11-29 after the owner's words of
// 11-28; coordinator work order `rd-1123/brief-455-sl3.md`; contracts/scheduling/README.md, "A pool
// whose spending a line governs"), the host's half:
// - h1: a line stored through the stored document (`ApplyResourceTargets`, v2) on the fixture's
//   pool that writes `spend_line`, a package reading of the pool published: the judged spender is
//   deferred `policy_benefit_nonpositive`, its pressing in `spend_line_pressed`; a higher reading
//   published, it is dispatched and pressed no more;
// - h2: on an instance whose idle action can close it, a pressed spender whose last reason is
//   `policy_benefit_nonpositive` is not due: the instance takes its action and is not woken for it;
//   a pressed spender that still passes and is selected is due, as any selected decision.
//
// The keys and pools are made up; times are fixed Unix milliseconds on a manual Runtime clock, and
// nothing sleeps.

use super::*;
use crate::PolicyTrigger;
use crate::application_update::IdleUpdateHold;
use crate::idle_action::{IdleEnd, IdlePlan, IdlePlanner, IdleStep, IdleView};
use crate::schedule_explanation::{
    IdleGates, IdleHeldAhead, IdleHeldBack, IdleSleepers, ScheduleExplanationTable,
};
use actingcommand_contract::outcome::Outcome;
use actingcommand_policy::{
    DecisionReason, EligibilityState, PolicyCode, PolicyEvaluation, SchedulingDecisionState,
    TaskDecision,
};
use actingcommand_runtime_config::{IdleAction, IdleConfig};
use serde_json::{Value, json};

const ALIAS: &str = POLICY_INSTANCE_ALIAS;
/// catalog-a's task, here a judged spender of the currency.
const TASK: &str = "fixture.observe";
/// The pool that writes `spend_line` (6000 milli of 100 units), read from [`CURRENCY_KEY`].
const CURRENCY: &str = "pool.currency";
const CURRENCY_KEY: &str = "resource.currency";
/// The spender's fixed value and its consumption of the currency a run.
const VALUE: i64 = 500;
const CONSUMED: u64 = 120;
/// The line stored on the currency.
const LINE: u64 = 400;
/// How long a reading stays valid: within the freshness policy `stored_fact` gives (one second to
/// two minutes), so the record is not refused `fact_observation_invalid`.
const VALID_MS: u64 = 60_000;
const MINUTE_MS: u64 = 60_000;
/// The idle plan's instance and instant (h2).
const NODE: &str = "node-a";
const AT: u64 = 1_799_971_200_000;
/// `idle.close_app_wake_lead_ms` and `idle.stop_emulator_wake_lead_ms`'s defaults.
const LEAD_MS: u64 = 300_000;
/// `close_app` with the production gap and leads.
const CLOSE: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: LEAD_MS,
    stop_emulator_wake_lead_ms: LEAD_MS,
};

// ---- h1: the host

/// catalog-a declared V2 (its events valid from 0): its task judged with the fixed value [`VALUE`],
/// due at every evaluation, consuming [`CONSUMED`] of [`CURRENCY`] a run and producing nothing; its
/// one pool the currency, which writes `spend_line` (6000 milli of 100 units, no income floor) and
/// no valuation, read from [`CURRENCY_KEY`] through the ledger; its profiles without goals.
fn line_sources() -> CatalogSources {
    let mut sources = policy_sources(1);
    for source in [
        &mut sources.tasks,
        &mut sources.pools,
        &mut sources.activity,
        &mut sources.timeline,
    ] {
        let mut document: Value =
            serde_json::from_slice(&source.bytes).expect("the fixture's JSON");
        document["schema_version"] = json!(actingcommand_policy::SCHEDULING_SCHEMA_VERSION_V2);
        if let Some(events) = document.get_mut("events").and_then(Value::as_array_mut) {
            for event in events {
                event["validity"] = json!({"from_unix_ms": 0, "until_unix_ms": null});
            }
        }
        if document.get("tasks").is_some() {
            let task = &mut document["tasks"][0];
            task["trigger"] = json!({
                "kind": "clock",
                "schedule": {"kind": "interval", "clock_source": {"kind": "local"}, "every_ms": 1,
                             "anchor_ms": 0}
            });
            task["consumes"] = json!([{
                "pool_id": CURRENCY,
                "direction": "consume",
                "amount": CONSUMED,
                "observation_source": "scan_verified",
                "confidence_milli": 1000
            }]);
            task["produces"] = json!([]);
            task["benefit_judged"] = json!(true);
            task["benefit_fixed_value_milli"] = json!(VALUE);
        }
        if document.get("pools").is_some() {
            document["pools"] = json!([{
                "id": CURRENCY,
                "scope": {"kind": "instance", "instance_id": ALIAS},
                "capacity": 100_000,
                "projection": {"kind": "none"},
                "observation": {"kind": "fact", "fact_key": CURRENCY_KEY},
                "value_source": {"kind": "ledger_fact", "minimum_confidence_milli": 500},
                "group_delay": {"minimum_delay_ms": 60000, "maximum_delay_ms": 300000},
                "spend_line": {"price_milli": 6000, "scale": 100, "income_floor_milli_per_day": 0}
            }]);
        }
        if let Some(profiles) = document.get_mut("profiles").and_then(Value::as_array_mut) {
            for profile in profiles {
                profile["goals"] = json!([]);
            }
        }
        source.bytes = serde_json::to_vec_pretty(&document).expect("the fixture's bytes");
    }
    sources
}

/// A host of the policy instance on `clock` with [`line_sources`] active, compiled first by the
/// compiler the staging runs, so a refusal names its issues.
fn start(root: &TempDir, clock: &Arc<ManualRuntimeClock>) -> RuntimeHost {
    let sources = line_sources();
    if let Err(failure) = compile_catalog(&sources) {
        panic!(
            "the fixture's catalog compiles: {:?}",
            failure.diagnostics()
        );
    }
    let host = RuntimeHost::start(
        config(root).with_runtime_clock(Arc::<ManualRuntimeClock>::clone(clock)),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("the runtime host");
    host.activate_policy_catalog(&sources)
        .expect("the catalog, configured directly");
    host
}

/// One evaluated cycle at `unix_ms`, the Runtime clock moved there and the host's capacity
/// sampled, over the fixture's facts, no pool snapshot configured.
fn evaluate_at(host: &RuntimeHost, clock: &ManualRuntimeClock, unix_ms: u64) -> PolicyEvaluation {
    clock.set_unix_ms(unix_ms);
    host.capacity_sampler_for_test().expect("capacity owner")().expect("capacity sample");
    let mut resources = policy_resources();
    resources.pools.clear();
    host.evaluate_policy_cycle_with_test_inputs(
        &policy_facts(),
        &resources,
        EvaluationTime {
            unix_ms,
            monotonic_ms: unix_ms,
        },
        7,
        PolicyTrigger::Recovery,
    )
    .expect("a policy cycle")
    .evaluation
    .expect("an evaluated cycle")
}

fn decision(evaluation: &PolicyEvaluation) -> &TaskDecision {
    evaluation
        .decisions
        .iter()
        .find(|entry| entry.task_id == TASK && entry.instance_id.as_deref() == Some(ALIAS))
        .unwrap_or_else(|| panic!("{TASK}'s decision: {evaluation:#?}"))
}

fn intent(evaluation: &PolicyEvaluation) -> Option<&DispatchIntent> {
    evaluation
        .dispatch_intents
        .iter()
        .find(|dispatch| dispatch.task_id == TASK && dispatch.instance_id == ALIAS)
}

/// The v2 document of the line of [`LINE`] on the currency for the instance, valid for a day.
fn line_document() -> Value {
    json!({
        "schema_version": "actingcommand.resource-targets.v2",
        "instance": ALIAS,
        "valid_until_unix_ms": POLICY_NOW_UNIX_MS + 86_400_000,
        "targets": [{
            "id": "line",
            "resource": CURRENCY,
            "condition": {"kind": "at_least", "amount": LINE},
            "apply": {"mode": "adjust", "weight": "score_stage"}
        }]
    })
}

/// A package reading of the currency of `value` as the Runtime publishes it (detector
/// `resource_reading:<task>/<id>`, no input invalidation), observed at `observed_at` under the
/// snapshot `snapshot`, valid for [`VALID_MS`].
fn reading(value: i64, observed_at: u64, snapshot: &str) -> FactRecord {
    let mut record = stored_fact(
        FactScope::Instance {
            instance_id: ALIAS.to_owned(),
        },
        CURRENCY_KEY,
        ContractFactValue::Integer(value),
        snapshot,
        Vec::new(),
    );
    record.observed_at_unix_ms = observed_at;
    record.expires_at_unix_ms = Some(observed_at + VALID_MS);
    record.confidence_milli = 990;
    record.source_detector = format!(
        "{}task/k2",
        actingcommand_contract::RESOURCE_READING_DETECTOR_PREFIX
    );
    record
}

// h1 (the work order's host test 1; rulings 1 to 4). The line of 400 is stored through
// `ApplyResourceTargets` (a v2 document) and taken. The currency read at 450: the spender's run of
// 120 leaves 330, 70 below the line, 70 units at 70 % of the full price of 6000 milli of 100 units,
// `P = 6000 · 70 · 70 ÷ 100² = 2940`: `B = 500 − 2940`, not dispatched, its last reason
// `policy_benefit_nonpositive` with `benefit_milli` −2440, and `spend_line_pressed` with the pool,
// the line, the reading, the consumption, the shortfall and `P`. The currency read at 600: the run
// leaves 480, above the line, so it is dispatched and carries no pressing.
#[test]
fn sl3_h1_a_stored_line_presses_the_spender_until_a_higher_reading() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(POLICY_NOW_UNIX_MS, 0));
    let host = start(&root, &clock);
    evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS);

    clock.set_unix_ms(POLICY_NOW_UNIX_MS + 1_000);
    let mut client = TestClient::connect(&host);
    let request = client.request(RuntimeOperation::ApplyResourceTargets {
        document_json: line_document().to_string(),
        expected_version: None,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "the line is stored: {receipt:?}"
    );
    drop(client);

    clock.set_unix_ms(POLICY_NOW_UNIX_MS + 2_000);
    host.publish_fact(reading(
        450,
        POLICY_NOW_UNIX_MS + 2_000,
        "run:fixture/frame:455-sl3/k2",
    ))
    .expect("the reading is published");
    let held = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS + 3_000);
    assert!(intent(&held).is_none(), "{held:#?}");
    let spend = decision(&held);
    let deciding = spend
        .reasons
        .last()
        .unwrap_or_else(|| panic!("a reason: {spend:#?}"));
    assert_eq!(
        deciding.outcome.code().as_str(),
        "policy_benefit_nonpositive",
        "{spend:#?}"
    );
    assert_eq!(
        deciding.outcome.value("benefit_milli"),
        Some(&json!(-2_440))
    );
    let pressed = spend
        .reasons
        .iter()
        .find(|given| given.outcome.is(PolicyCode::SpendLinePressed))
        .unwrap_or_else(|| panic!("the pressing: {spend:#?}"));
    for (key, value) in [
        ("pool_id", json!(CURRENCY)),
        ("line_at_least", json!(LINE)),
        ("pool_reading", json!(450)),
        ("consumption", json!(CONSUMED)),
        ("line_shortfall", json!(70)),
        ("pressing_milli", json!(2_940)),
    ] {
        assert_eq!(
            pressed.outcome.value(key),
            Some(&value),
            "{key}: {pressed:?}"
        );
    }

    clock.set_unix_ms(POLICY_NOW_UNIX_MS + 4_000);
    host.publish_fact(reading(
        600,
        POLICY_NOW_UNIX_MS + 4_000,
        "run:fixture/frame:455-sl3/k2-higher",
    ))
    .expect("the higher reading is published");
    let released = evaluate_at(&host, &clock, POLICY_NOW_UNIX_MS + 5_000);
    assert!(intent(&released).is_some(), "{released:#?}");
    assert!(
        decision(&released)
            .reasons
            .iter()
            .all(|given| !given.outcome.is(PolicyCode::SpendLinePressed)),
        "{released:#?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host");
}

// ---- h2: the idle plan

fn reason_of(outcome: Outcome) -> DecisionReason {
    let detail = format!("fixture: {}", outcome.code().as_str());
    DecisionReason { outcome, detail }
}

/// The spender's decision on [`NODE`] in `state` as the benefit gate leaves it pressed:
/// `benefit_fixed_value`, `spend_line_pressed` (h1's numbers), then `last`; no wake of its own.
fn pressed_spender(state: SchedulingDecisionState, last: Outcome) -> TaskDecision {
    TaskDecision {
        task_id: "task.spend".to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility: EligibilityState::True,
        state,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![
            reason_of(
                Outcome::new(PolicyCode::BenefitFixedValue)
                    .benefit_milli(-2_440_i64)
                    .fixed_value_milli(VALUE)
                    .expected_duration_ms(60_000_u64),
            ),
            reason_of(
                Outcome::new(PolicyCode::SpendLinePressed)
                    .pool_id(CURRENCY)
                    .line_at_least(400_i64)
                    .pool_reading(450_i64)
                    .consumption(120_i64)
                    .line_shortfall(70_i64)
                    .pressing_milli(2_940_i64),
            ),
            reason_of(last),
        ],
        next_wake_unix_ms: None,
    }
}

/// A task of the instance whose trigger is false, naming no time.
fn idle_task() -> TaskDecision {
    TaskDecision {
        task_id: "task.later".to_owned(),
        instance_id: Some(NODE.to_owned()),
        eligibility: EligibilityState::False,
        state: SchedulingDecisionState::Blocked,
        rank: None,
        detection_suggestions: Vec::new(),
        reasons: vec![reason_of(Outcome::new(PolicyCode::TriggerFalse))],
        next_wake_unix_ms: None,
    }
}

/// The idle plan of [`NODE`], an instance whose idle action can put it to sleep, from an
/// evaluation of `decisions` at `at` that read no regenerating pool.
fn plan_of(decisions: Vec<TaskDecision>, at: u64) -> IdlePlan {
    let steps = BTreeSet::new();
    let instances = BTreeSet::from([NODE.to_owned()]);
    let mut table = ScheduleExplanationTable::default();
    table.record_stopping(
        &PolicyEvaluation {
            decisions,
            next_wake_unix_ms: None,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        },
        "sha256:455-sl3",
        at,
        &|_: &str, _: &str| -> Option<(u64, String)> { None },
        IdleGates {
            held_back: &IdleHeldBack::new(),
            held_ahead: &IdleHeldAhead::new(),
            update_hold: IdleUpdateHold::default(),
        },
        IdleSleepers {
            instances: &instances,
            pool_steps: &steps,
        },
    );
    table.idle_plan(NODE).expect("an evaluated plan")
}

/// The instance free, its emulator running, both actions able to bring it back.
fn free() -> IdleView {
    IdleView {
        emulator_on: true,
        app_wakeable: true,
        emulator_wakeable: true,
        ..IdleView::default()
    }
}

// h2 (the work order's host test 2; ruling 4). The pressed spender deferred
// `policy_benefit_nonpositive`, beside a task whose trigger is false: the plan is not due and names
// no wake, so the awake instance takes its action (`close_app`) and, closed, is not woken for it ten
// minutes later. The same spender pressed but positive and selected (its last reason
// `rank_assigned`) is due.
#[test]
fn sl3_h2_an_instance_that_can_sleep_is_not_woken_for_a_pressed_spender() {
    let later = AT + 10 * MINUTE_MS;
    let held = || {
        vec![
            pressed_spender(
                SchedulingDecisionState::Deferred,
                Outcome::new(PolicyCode::PolicyBenefitNonpositive)
                    .benefit_milli(-2_440_i64)
                    .expected_duration_ms(60_000_u64),
            ),
            idle_task(),
        ]
    };
    let plan = plan_of(held(), AT);
    assert!(!plan.due_now, "{plan:?}");
    assert_eq!(plan.next_wake_unix_ms, None, "{plan:?}");
    let mut planner = IdlePlanner::default();
    assert_eq!(
        planner.check(NODE, &CLOSE, free(), Some(plan), AT),
        Some(IdleStep::Act {
            action: IdleAction::CloseApp,
            next_due_unix_ms: None,
            wake_at_unix_ms: None,
        }),
        "the action"
    );
    planner.finished(NODE, IdleEnd::Done);
    let plan = plan_of(held(), later);
    assert_eq!(
        planner.check(NODE, &CLOSE, free(), Some(plan), later),
        None,
        "not woken"
    );

    let selected = plan_of(
        vec![pressed_spender(
            SchedulingDecisionState::Selected,
            Outcome::new(PolicyCode::RankAssigned),
        )],
        AT,
    );
    assert!(selected.due_now, "{selected:?}");
}
