// SPDX-License-Identifier: AGPL-3.0-only

// #486 ASLEEP-5 (0.12.83; register §1.524, the coordinator's rulings 2 and 4), actingd's half.
// - t7 (the work order's test 7): the daemon's line of each evaluated policy cycle. One line per
//   evaluated cycle with every member, what woke the driver first (its wakes with each cause's
//   number, then the cycle's number, its trigger's kind, and the milliseconds of the projection,
//   the evaluation, the clock slots' preview with its counts, the observation of the holds, what
//   follows the capped record and the whole cycle); cycles that follow one another faster than one
//   a second make one line a second at most, which names how many cycles it stands for and their
//   summed times and counts; fix round 1: the cycles still kept when the driver ends make their
//   line then. The times are given (the driver's monotonic milliseconds), so nothing reads a clock.
// - t2 (fix round 1, the work order's ruling 2 on the driver): the schedule's next wake after a
//   cycle is the earlier of the evaluation's wake and the host's time for the instances its idle
//   check left asleep, and none when neither names one.

use super::policy_cycle_line::{PolicyCycleLines, RUN_ENDED_WAKE};
use super::*;
use actingcommand_runtime_host::{
    ClockSlotPreviewCost, PolicyCycleStretches, PolicyEvaluationCost, PolicyEvaluationExecution,
    PolicyEvaluationMeasurement,
};

/// #503 CYC-1 (register §1.526, ruling 1): the members the line gained after `total_ms`, for a
/// measurement that names none of its other stretches; what is not covered up to the capped
/// record is none here, since this fixture's parts already exceed it. #503 CYC-2 (register
/// §1.528, ruling 1): and the slowest calls of the evaluation's setup, none here.
const NO_STRETCHES: &str = " before_gates_ms=0.000 gates_wait_ms=0.000 gates_held_ms=0.000 \
     cycle_inputs_ms=0.000 policy_lock_wait_ms=0.000 evaluation_setup_ms=0.000 \
     no_progress_ms=0.000 detection_preview_ms=0.000 detection_preview_store_reads=0 \
     explanation_ms=0.000 after_held_ms=0.000 capped_record_ms=0.000 unattributed_ms=0.000 \
     evaluation_setup_slowest=none";

/// A measurement of the cycle numbered `seq`: each time in microseconds as given.
fn measurement(seq: u64, scale: u64) -> PolicyEvaluationMeasurement {
    PolicyEvaluationMeasurement {
        sampled_at_monotonic_ms: 0,
        elapsed_micros: 4_000_000 * scale,
        requested_recompute: PolicyRecomputeKind::Incremental,
        execution: PolicyEvaluationExecution::FullCatalogScan,
        cost: PolicyEvaluationCost {
            catalog_records: 0,
            catalog_tasks: 0,
            fact_records: 0,
            outcome_records: 0,
            task_state_records: 0,
            instance_records: 0,
            resource_records: 0,
            task_instance_pairs: 0,
            work_units: 0,
        },
        cycle_seq: seq,
        projection_micros: 1_500 * scale,
        clock_slots: ClockSlotPreviewCost {
            micros: 2_000_250 * scale,
            pairs: 12 * scale,
            live_calls: 3 * scale,
            store_reads: 40 * scale,
        },
        held_observe_micros: 250 * scale,
        until_capped_micros: 4_900_000 * scale,
        stretches: PolicyCycleStretches::default(),
    }
}

#[test]
fn asleep5_t7_one_line_per_evaluated_cycle_with_every_member_and_one_a_second_at_most() {
    let mut lines = PolicyCycleLines::default();
    assert_eq!(lines.flush(5_000), None, "no cycle, no line");

    // One evaluated cycle: its line at once, every member present, its wakes first.
    lines.note_wakes([("lease.released".to_owned(), 2)]);
    lines.note_wakes([
        ("fact.published".to_owned(), 1),
        ("lease.released".to_owned(), 1),
    ]);
    // #503 CYC-1: each line ends with the members the line gained (`NO_STRETCHES`).
    assert_eq!(
        lines
            .record(
                10_000,
                PolicyTrigger::ResourcesChanged,
                &measurement(41, 1),
                Duration::from_micros(5_000_000),
            )
            .as_deref(),
        Some(
            format!(
                "actingd policy_cycle wakes=lease.released:3,fact.published:1 seq=41 \
                 first_seq=41 cycles=1 triggers=resources_changed:1 projection_ms=1.500 \
                 evaluation_ms=4000.000 clock_slots_ms=2000.250 clock_slot_pairs=12 \
                 clock_slot_live_calls=3 clock_slot_store_reads=40 held_ms=0.250 \
                 after_capped_ms=100.000 total_ms=5000.000{NO_STRETCHES}"
            )
            .as_str()
        ),
        "the cycle's line"
    );

    // Two more cycles within the second: kept, no line.
    lines.note_wakes([(RUN_ENDED_WAKE.to_owned(), 1)]);
    assert_eq!(
        lines.record(
            10_400,
            PolicyTrigger::FactsChanged,
            &measurement(42, 1),
            Duration::from_micros(5_000_000),
        ),
        None,
        "less than a second after the last line"
    );
    assert_eq!(
        lines.record(
            10_800,
            PolicyTrigger::ClockObserved { jumped: false },
            &measurement(43, 2),
            Duration::from_micros(10_000_000),
        ),
        None,
        "still within the second"
    );
    assert_eq!(lines.flush(10_999), None, "the second has not passed");

    // The second passed: one line for the two cycles kept, their times and counts summed and each
    // trigger's kind and each wake with its number.
    assert_eq!(
        lines.flush(11_000).as_deref(),
        Some(
            format!(
                "actingd policy_cycle wakes=run_ended:1 seq=43 first_seq=42 cycles=2 \
                 triggers=facts_changed:1,clock_observed:1 projection_ms=4.500 \
                 evaluation_ms=12000.000 clock_slots_ms=6000.750 clock_slot_pairs=36 \
                 clock_slot_live_calls=9 clock_slot_store_reads=120 held_ms=0.750 \
                 after_capped_ms=300.000 total_ms=15000.000{NO_STRETCHES}"
            )
            .as_str()
        ),
        "one line for both"
    );
    assert_eq!(lines.flush(12_500), None, "nothing kept, no line");

    // A cycle a second or more after the last line has its line at once; one with no wake noted
    // names none.
    assert_eq!(
        lines
            .record(
                13_000,
                PolicyTrigger::Reconciliation,
                &measurement(44, 1),
                Duration::from_micros(4_950_000),
            )
            .as_deref(),
        Some(
            format!(
                "actingd policy_cycle wakes=none seq=44 first_seq=44 cycles=1 \
                 triggers=reconciliation:1 projection_ms=1.500 evaluation_ms=4000.000 \
                 clock_slots_ms=2000.250 clock_slot_pairs=12 clock_slot_live_calls=3 \
                 clock_slot_store_reads=40 held_ms=0.250 after_capped_ms=50.000 \
                 total_ms=4950.000{NO_STRETCHES}"
            )
            .as_str()
        ),
        "its own line"
    );

    // Fix round 1: a cycle kept within the second is written as the driver ends, once.
    assert_eq!(
        lines.record(
            13_200,
            PolicyTrigger::FactsChanged,
            &measurement(45, 1),
            Duration::from_micros(5_000_000),
        ),
        None,
        "kept"
    );
    assert_eq!(
        lines.finish().as_deref(),
        Some(
            format!(
                "actingd policy_cycle wakes=none seq=45 first_seq=45 cycles=1 \
                 triggers=facts_changed:1 projection_ms=1.500 evaluation_ms=4000.000 \
                 clock_slots_ms=2000.250 clock_slot_pairs=12 clock_slot_live_calls=3 \
                 clock_slot_store_reads=40 held_ms=0.250 after_capped_ms=100.000 \
                 total_ms=5000.000{NO_STRETCHES}"
            )
            .as_str()
        ),
        "the line kept, as the driver ends"
    );
    assert_eq!(lines.finish(), None, "written once");
}

/// An evaluated cycle whose evaluation names `evaluation_wake` and the host `idle_wake`.
fn evaluated_cycle(evaluation_wake: Option<u64>, idle_wake: Option<u64>) -> PolicyCycle {
    PolicyCycle {
        directive: PolicyRecomputeDirective {
            kind: PolicyRecomputeKind::Incremental,
            reason: PolicyRecomputeReason::Event,
            eligible_at_unix_ms: NOW,
        },
        evaluation: Some(actingcommand_policy::PolicyEvaluation {
            decisions: Vec::new(),
            next_wake_unix_ms: evaluation_wake,
            preload_hint: None,
            dispatch_intents: Vec::new(),
            reason_chains: Vec::new(),
        }),
        pending_dispatch_intents: Vec::new(),
        detection_planning_signals: Vec::new(),
        clock_slot_signals: Vec::new(),
        task_held_signals: Vec::new(),
        measurement: None,
        fact_refresh_transitions: Vec::new(),
        idle_wake_unix_ms: idle_wake,
    }
}

/// The time of the fixture's cycles.
const NOW: u64 = 1_800_000_000_000;

#[test]
fn asleep5_t2_the_drivers_next_wake_is_the_earlier_of_the_evaluations_and_the_hosts() {
    let mut schedule =
        PolicyDriverSchedule::new(PolicyCadence::default(), &evaluated_cycle(None, None), NOW)
            .expect("the schedule");
    assert_eq!(schedule.next_wake_unix_ms, None, "neither names one");
    for (case, evaluation_wake, idle_wake, expected) in [
        (
            "the host's earlier",
            Some(NOW + 60_000),
            Some(NOW + 30_000),
            Some(NOW + 30_000),
        ),
        (
            "the evaluation's earlier",
            Some(NOW + 30_000),
            Some(NOW + 60_000),
            Some(NOW + 30_000),
        ),
        (
            "the host's alone",
            None,
            Some(NOW + 45_000),
            Some(NOW + 45_000),
        ),
        (
            "the evaluation's alone",
            Some(NOW + 50_000),
            None,
            Some(NOW + 50_000),
        ),
        ("neither", None, None, None),
    ] {
        schedule
            .apply_cycle(
                PolicyTrigger::FactsChanged,
                &evaluated_cycle(evaluation_wake, idle_wake),
                NOW,
            )
            .expect("apply the cycle");
        assert_eq!(schedule.next_wake_unix_ms, expected, "{case}");
    }
}
