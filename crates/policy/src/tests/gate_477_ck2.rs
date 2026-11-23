// SPDX-License-Identifier: AGPL-3.0-only

// #477 CK-2 (0.12.81; register §1.516; the coordinator's rulings of 11-23 and their Addendum 1, no
// words of the owner's behind them; contracts/scheduling/README.md, "Clock Sources", "Clock-slot
// records"): a clock occurrence stays owed when the latest dispatch that answered it never ran or
// its failure paused the pair. The Runtime says so in the pair's task snapshot (`clock_owed`); this
// gate is the evaluator's half, on the P-2a fixture (`gate_p2a`): one task on one instance, a daily
// UTC clock, an activity window open all day, no clock read.
// - an occurrence the latest dispatch left owed makes the clock true until a dispatch admitted
//   after it serves it (t1);
// - a later occurrence replaces an owed earlier one (t2);
// - with `missed`, an owed occurrence is due only inside its grace, as before (t3);
// - a clock under a `not` reads the same (t4);
// - a snapshot without the member serializes, and so hashes into the snapshot identity, byte for
//   byte as before (t5).

use serde_json::{Value, json};

use super::gate_p2a::{
    FIRST, INSTANCE_A, SEED, at, catalog_of_version, decision, facts, instance, last_reason,
    resources, task,
};
use crate::{
    ClockSlotView, CompiledCatalog, CompletedActivityWindow, PolicyCode, PolicyEvaluation,
    SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, TaskRuntimeSnapshot,
    TaskTerminalState, clock_slot_lapses, clock_slot_view, evaluate,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
// Midnight UTC of the day the scenarios run on.
const DAY_1: u64 = 400 * DAY_MS;
// The clock's occurrence that day, 07:00, and the second one of t2, 19:00.
const MORNING: u64 = DAY_1 + 7 * HOUR_MS;
const EVENING: u64 = DAY_1 + 19 * HOUR_MS;
// The dispatch that answered the morning occurrence, a minute after it.
const ANSWERED: u64 = MORNING + MINUTE_MS;
// The grace of t3's `missed` rule: two hours.
const GRACE_MS: u64 = 7_200_000;
const TRIGGER_FALSE: &str = "trigger_false";

// A UTC `daily` clock at the given times of the day, with `missed: skip_after` of `grace_ms`
// when one is given.
fn daily(times: &[u64], grace_ms: Option<u64>) -> Value {
    let minutes = times
        .iter()
        .map(|time| (time - DAY_1) / MINUTE_MS)
        .collect::<Vec<_>>();
    let mut clock = json!({
        "kind": "clock",
        "schedule": {
            "kind": "daily",
            "clock_source": {
                "kind": "server",
                "timezone_id": "etc/utc",
                "utc_offset_minutes": 0,
                "dst_offset_minutes": 0,
                "maintenance_drift_ms": 0
            },
            "minutes_of_day": minutes
        }
    });
    if let Some(grace_ms) = grace_ms {
        clock["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    }
    clock
}

// The P-2a catalog of `version` with the one task `FIRST` triggered by `trigger`, four runs a day,
// its activity window open all day.
fn catalog_with(version: &str, trigger: Value) -> CompiledCatalog {
    catalog_of_version(version, vec![task(FIRST, trigger, 4, json!([]))], 0, 0)
}

// The pair's snapshot after its latest dispatch at `dispatched`, which ended as `terminal`, with
// `clock_owed` as the Runtime projects it: `true` when that dispatch never ran or its failure
// paused the pair.
fn ran(dispatched: u64, terminal: TaskTerminalState, clock_owed: bool) -> TaskRuntimeSnapshot {
    TaskRuntimeSnapshot {
        task_id: FIRST.to_owned(),
        instance_id: INSTANCE_A.to_owned(),
        last_dispatched_unix_ms: Some(dispatched),
        eligible_since_unix_ms: None,
        terminal_state: Some(terminal),
        completed_window: None,
        last_duration_ms: None,
        failure_streak: None,
        expected_runtime_ms: None,
        expected_runtime_runs: None,
        clock_owed,
    }
}

// One evaluation of `catalog` at `now` with the pair's snapshot `state`.
fn evaluate_at(
    catalog: &CompiledCatalog,
    state: Option<TaskRuntimeSnapshot>,
    now: u64,
) -> PolicyEvaluation {
    let instances = vec![instance(INSTANCE_A, "host.a")];
    let hosts = resources(&instances);
    evaluate(
        catalog,
        &facts(instances, state.into_iter().collect()),
        &hosts,
        at(now),
        SEED,
    )
    .unwrap_or_else(|error| panic!("at {now}: the fixture evaluates: {error:?}"))
}

// Whether the evaluation dispatches the pair.
fn selected(evaluation: &PolicyEvaluation) -> bool {
    evaluation
        .dispatch_intents
        .iter()
        .any(|intent| intent.task_id == FIRST && intent.instance_id == INSTANCE_A)
}

// Test 1 (Addendum 1, A1 and test (2), the evaluator's half). The 07:00 occurrence was answered
// at 07:01. As on main (no member) that dispatch served it: at 07:10 the trigger is false. Left
// owed (the dispatch never ran, or its failure paused the pair) the clock is true at 07:10 and the
// pair is dispatched on the same occurrence. That run, admitted at 07:30, succeeded: the
// occurrence is served and the pair is due again at the next day's 07:00, not before.
#[test]
fn ck2_t1_an_occurrence_left_owed_is_due_until_a_dispatch_after_it_serves_it() {
    let catalog = catalog_with(SCHEDULING_SCHEMA_VERSION, daily(&[MORNING], None));
    let now = MORNING + 10 * MINUTE_MS;

    let served = evaluate_at(
        &catalog,
        Some(ran(ANSWERED, TaskTerminalState::Failed, false)),
        now,
    );
    assert!(!selected(&served), "{served:#?}");
    assert_eq!(
        last_reason(decision(&served, FIRST, INSTANCE_A)),
        TRIGGER_FALSE
    );

    let left = evaluate_at(
        &catalog,
        Some(ran(ANSWERED, TaskTerminalState::Failed, true)),
        now,
    );
    assert!(selected(&left), "the occurrence is owed: {left:#?}");

    let rerun = ran(
        MORNING + 30 * MINUTE_MS,
        TaskTerminalState::Succeeded,
        false,
    );
    for (at_time, due) in [
        (MORNING + 40 * MINUTE_MS, false),
        (MORNING + DAY_MS - MINUTE_MS, false),
        (MORNING + DAY_MS, true),
    ] {
        let evaluation = evaluate_at(&catalog, Some(rerun.clone()), at_time);
        assert_eq!(selected(&evaluation), due, "at {at_time}: {evaluation:#?}");
    }
}

// Test 7 of Addendum 1: a later occurrence replaces an owed earlier one. Clocks at 07:00 and
// 19:00; the 07:00 occurrence was answered at 07:01 and left owed. At 18:59 the due occurrence is
// 07:00 (none as on main, without the member); at 20:00 it is 19:00, the next tomorrow's 07:00,
// and the pair is dispatched once for it; the lapse pass reads as before, from the latest dispatch.
// One dispatch at 20:00 that succeeds serves 19:00: at 21:00 nothing is due.
#[test]
fn ck2_t2_a_later_occurrence_replaces_an_owed_earlier_one() {
    let catalog = catalog_with(SCHEDULING_SCHEMA_VERSION, daily(&[MORNING, EVENING], None));
    let spec = &catalog.catalog().tasks.tasks[0];
    let left = ran(ANSWERED, TaskTerminalState::Failed, true);
    let served = ran(ANSWERED, TaskTerminalState::Failed, false);
    let before_evening = EVENING - MINUTE_MS;
    let now = EVENING + HOUR_MS;

    assert_eq!(
        clock_slot_view(spec, Some(&left), at(before_evening)).expect("the clock slots read"),
        Some(ClockSlotView {
            due_occurrence: Some(MORNING),
            next_occurrence: Some(EVENING),
        })
    );
    assert_eq!(
        clock_slot_view(spec, Some(&served), at(before_evening)).expect("the clock slots read"),
        Some(ClockSlotView {
            due_occurrence: None,
            next_occurrence: Some(EVENING),
        })
    );
    assert_eq!(
        clock_slot_view(spec, Some(&left), at(now)).expect("the clock slots read"),
        Some(ClockSlotView {
            due_occurrence: Some(EVENING),
            next_occurrence: Some(MORNING + DAY_MS),
        })
    );
    let lapses = clock_slot_lapses(spec, Some(&left), at(now)).expect("the lapses read");
    assert!(lapses.is_empty(), "{lapses:?}");

    let evaluation = evaluate_at(&catalog, Some(left), now);
    assert_eq!(evaluation.dispatch_intents.len(), 1, "{evaluation:#?}");
    assert!(selected(&evaluation));
    let evening_run = ran(now, TaskTerminalState::Succeeded, false);
    assert!(!selected(&evaluate_at(
        &catalog,
        Some(evening_run),
        now + HOUR_MS
    )));
}

// Test 6 of Addendum 1: `missed` with `skip_after` (V2). The 07:00 occurrence, left owed by the
// dispatch at 07:01, is due until its grace of two hours ends: at 08:59 the pair is dispatched,
// at 09:00 the clock is false with `clock_occurrence_expired`, as for any unserved occurrence. As
// on main (no member) the occurrence was served and nothing is due at 08:59.
#[test]
fn ck2_t3_missed_bounds_an_owed_occurrence_by_its_grace_as_before() {
    let catalog = catalog_with(
        SCHEDULING_SCHEMA_VERSION_V2,
        daily(&[MORNING], Some(GRACE_MS)),
    );
    let left = ran(ANSWERED, TaskTerminalState::Failed, true);

    assert!(selected(&evaluate_at(
        &catalog,
        Some(left.clone()),
        MORNING + GRACE_MS - MINUTE_MS
    )));
    let expired = evaluate_at(&catalog, Some(left), MORNING + GRACE_MS);
    assert!(!selected(&expired), "{expired:#?}");
    let decided = decision(&expired, FIRST, INSTANCE_A);
    assert_eq!(last_reason(decided), TRIGGER_FALSE);
    assert!(
        decided
            .reasons
            .iter()
            .any(|reason| reason.outcome.is(PolicyCode::ClockOccurrenceExpired)),
        "{decided:#?}"
    );

    let served = evaluate_at(
        &catalog,
        Some(ran(ANSWERED, TaskTerminalState::Failed, false)),
        MORNING + GRACE_MS - MINUTE_MS,
    );
    assert!(!selected(&served), "{served:#?}");
}

// Test 10 of Addendum 1 (A6): a clock under a `not` reads the new meaning too. The trigger is
// `not(clock 07:00)`. The 07:00 occurrence answered at 07:01: as on main (no member) it is served,
// the clock false and the trigger true at 07:10; left owed by a failure that paused the pair, the
// clock is true and the trigger false.
#[test]
fn ck2_t4_a_clock_under_a_not_reads_an_owed_occurrence_as_due() {
    let catalog = catalog_with(
        SCHEDULING_SCHEMA_VERSION,
        json!({"kind": "not", "predicate": daily(&[MORNING], None)}),
    );
    let now = MORNING + 10 * MINUTE_MS;

    let served = evaluate_at(
        &catalog,
        Some(ran(ANSWERED, TaskTerminalState::Failed, false)),
        now,
    );
    assert!(selected(&served), "{served:#?}");

    let left = evaluate_at(
        &catalog,
        Some(ran(ANSWERED, TaskTerminalState::Failed, true)),
        now,
    );
    assert!(!selected(&left), "{left:#?}");
    assert_eq!(
        last_reason(decision(&left, FIRST, INSTANCE_A)),
        TRIGGER_FALSE
    );
}

// Test 8 of Addendum 1, the golden: a snapshot without the member, as the Runtime projects every
// pair of a ledger with no dispatch that never ran or paused its pair, serializes byte for byte as
// on main, so the snapshot identity hashed from it and every decision identity derived from it do
// not move, and main's bytes read back to it. The member, when present, is `true`.
#[test]
fn ck2_t5_a_snapshot_without_the_member_serializes_as_before() {
    let snapshot = TaskRuntimeSnapshot {
        completed_window: Some(CompletedActivityWindow {
            window_id: "fixture-window".to_owned(),
            completed_at_unix_ms: ANSWERED + MINUTE_MS,
        }),
        last_duration_ms: Some(60_000),
        failure_streak: Some(1),
        ..ran(ANSWERED, TaskTerminalState::Failed, false)
    };
    let main = format!(
        concat!(
            r#"{{"task_id":"task.first","instance_id":"instance-a","#,
            r#""last_dispatched_unix_ms":{},"eligible_since_unix_ms":null,"#,
            r#""terminal_state":"failed","completed_window":{{"window_id":"fixture-window","#,
            r#""completed_at_unix_ms":{}}},"last_duration_ms":60000,"failure_streak":1}}"#
        ),
        ANSWERED,
        ANSWERED + MINUTE_MS
    );
    assert_eq!(
        serde_json::to_string(&snapshot).expect("the snapshot serializes"),
        main
    );
    assert_eq!(
        serde_json::from_str::<TaskRuntimeSnapshot>(&main).expect("main's snapshot reads"),
        snapshot
    );

    let left = TaskRuntimeSnapshot {
        clock_owed: true,
        ..snapshot
    };
    let written = serde_json::to_value(&left).expect("the snapshot serializes");
    assert_eq!(written["clock_owed"], json!(true), "{written}");
    assert_eq!(
        serde_json::from_value::<TaskRuntimeSnapshot>(written).expect("it reads back"),
        left
    );
}
