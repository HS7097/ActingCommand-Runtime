// SPDX-License-Identifier: AGPL-3.0-only

// #486 ASLEEP-5 (0.12.83; register §1.524, the coordinator's ruling 4): the daemon's line of each
// evaluated policy cycle, the work order's test 7. One line per evaluated cycle with every member
// (the cycle's number, its trigger's kind, what woke the driver with each cause's number, and the
// milliseconds of the projection, the evaluation, the clock slots' preview with its counts, the
// observation of the holds, what follows the capped record and the whole cycle); cycles that
// follow one another faster than one a second make one line a second at most, which names how
// many cycles it stands for and their summed times and counts. The times are given, so nothing
// reads a clock.

use super::policy_cycle_line::{PolicyCycleLines, RUN_ENDED_WAKE};
use super::*;
use actingcommand_runtime_host::{
    ClockSlotPreviewCost, PolicyEvaluationCost, PolicyEvaluationExecution,
    PolicyEvaluationMeasurement,
};

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
    }
}

#[test]
fn asleep5_t7_one_line_per_evaluated_cycle_with_every_member_and_one_a_second_at_most() {
    let mut lines = PolicyCycleLines::default();
    assert_eq!(lines.flush(5_000), None, "no cycle, no line");

    // One evaluated cycle: its line at once, every member present.
    lines.note_wakes([("lease.released".to_owned(), 2)]);
    lines.note_wakes([
        ("fact.published".to_owned(), 1),
        ("lease.released".to_owned(), 1),
    ]);
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
            "actingd policy_cycle seq=41 first_seq=41 cycles=1 triggers=resources_changed:1 \
             wakes=lease.released:3,fact.published:1 projection_ms=1.500 \
             evaluation_ms=4000.000 clock_slots_ms=2000.250 clock_slot_pairs=12 \
             clock_slot_live_calls=3 clock_slot_store_reads=40 held_ms=0.250 \
             after_capped_ms=100.000 total_ms=5000.000"
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
            PolicyTrigger::ClockObserved {
                previous_unix_ms: 10_000,
            },
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
            "actingd policy_cycle seq=43 first_seq=42 cycles=2 \
             triggers=facts_changed:1,clock_observed:1 wakes=run_ended:1 projection_ms=4.500 \
             evaluation_ms=12000.000 clock_slots_ms=6000.750 clock_slot_pairs=36 \
             clock_slot_live_calls=9 clock_slot_store_reads=120 held_ms=0.750 \
             after_capped_ms=300.000 total_ms=15000.000"
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
            "actingd policy_cycle seq=44 first_seq=44 cycles=1 triggers=reconciliation:1 \
             wakes=none projection_ms=1.500 evaluation_ms=4000.000 clock_slots_ms=2000.250 \
             clock_slot_pairs=12 clock_slot_live_calls=3 clock_slot_store_reads=40 \
             held_ms=0.250 after_capped_ms=50.000 total_ms=4950.000"
        ),
        "its own line"
    );
}
