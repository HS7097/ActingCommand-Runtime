// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-1 (0.12.84; register §1.526, the coordinator's rulings 1 and 3), actingd's half. The
// times are given (the driver's wall and monotonic milliseconds, the cycle's microseconds), so
// nothing reads a clock.
// - t1 (ruling 1): the daemon's line of a cycle prints every member, the ones it had and, after
//   `total_ms`, each stretch the host names beside them and `unattributed_ms`, the time up to the
//   capped record that none of the named parts covers; a line for several cycles sums them all.
// - t3 (ruling 3): a cycle longer than `clock_jump_threshold_ms`, and a look at the clock that is
//   merely late, both clocks agreeing, wake nothing; the wall clock set forward by more than the
//   threshold, or set back (behind the last look, or behind the monotonic clock by more than the
//   threshold), wakes the driver once, as a clock jump; a time the driver waited for wakes it as
//   an observation that is no jump.

use super::policy_cycle_line::PolicyCycleLines;
use super::*;
use actingcommand_runtime_host::{
    ClockSlotPreviewCost, PolicyCycleStretches, PolicyEvaluationCost, PolicyEvaluationExecution,
    PolicyEvaluationMeasurement,
};

/// The wall clock as the fixture's schedule starts.
const NOW: u64 = 1_800_000_000_000;

/// A cycle of production's shape on 11-26 (a long detection preview), its parts in microseconds:
/// they sum to 4 968 500, 1 500 less than the time up to the capped record.
fn measurement(seq: u64) -> PolicyEvaluationMeasurement {
    PolicyEvaluationMeasurement {
        sampled_at_monotonic_ms: 0,
        elapsed_micros: 8_000,
        requested_recompute: PolicyRecomputeKind::Full,
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
        projection_micros: 4_000,
        clock_slots: ClockSlotPreviewCost {
            micros: 90_000,
            pairs: 408,
            live_calls: 244,
            store_reads: 2_081,
        },
        held_observe_micros: 500,
        until_capped_micros: 4_970_000,
        stretches: PolicyCycleStretches {
            before_gates_micros: 1_000,
            gates_wait_micros: 2_000,
            gates_held_micros: 3_000,
            cycle_inputs_micros: 5_000,
            policy_lock_wait_micros: 6_000,
            evaluation_setup_micros: 7_000,
            no_progress_micros: 9_000,
            detection_preview_micros: 4_800_000,
            detection_preview_store_reads: 600,
            explanation_micros: 10_000,
            after_held_micros: 11_000,
            capped_record_micros: 12_000,
        },
    }
}

/// The whole cycle's time as the driver times it: 750 microseconds after the capped record.
const TOTAL: Duration = Duration::from_micros(4_970_750);

#[test]
fn cyc1_t1_the_line_prints_every_member_and_what_no_named_stretch_covers() {
    let mut lines = PolicyCycleLines::default();
    lines.note_wakes([(policy_cycle_line::CLOCK_JUMP_WAKE.to_owned(), 1)]);
    assert_eq!(
        lines
            .record(
                10_000,
                PolicyTrigger::ClockObserved { jumped: true },
                &measurement(7),
                TOTAL,
            )
            .as_deref(),
        Some(
            "actingd policy_cycle wakes=clock_jump:1 seq=7 first_seq=7 cycles=1 \
             triggers=clock_observed:1 projection_ms=4.000 evaluation_ms=8.000 \
             clock_slots_ms=90.000 clock_slot_pairs=408 clock_slot_live_calls=244 \
             clock_slot_store_reads=2081 held_ms=0.500 after_capped_ms=0.750 \
             total_ms=4970.750 before_gates_ms=1.000 gates_wait_ms=2.000 gates_held_ms=3.000 \
             cycle_inputs_ms=5.000 policy_lock_wait_ms=6.000 evaluation_setup_ms=7.000 \
             no_progress_ms=9.000 detection_preview_ms=4800.000 \
             detection_preview_store_reads=600 explanation_ms=10.000 after_held_ms=11.000 \
             capped_record_ms=12.000 unattributed_ms=1.500"
        ),
        "every member, the stretches after total_ms, and the 1.5 ms no stretch names"
    );

    // Two cycles within the second: one line for both, every stretch and count summed, and what
    // no stretch covers summed per cycle.
    assert_eq!(
        lines.record(10_400, PolicyTrigger::FactsChanged, &measurement(8), TOTAL),
        None,
        "kept"
    );
    assert_eq!(
        lines.record(10_800, PolicyTrigger::FactsChanged, &measurement(9), TOTAL),
        None,
        "kept"
    );
    assert_eq!(
        lines.flush(11_000).as_deref(),
        Some(
            "actingd policy_cycle wakes=none seq=9 first_seq=8 cycles=2 \
             triggers=facts_changed:2 projection_ms=8.000 evaluation_ms=16.000 \
             clock_slots_ms=180.000 clock_slot_pairs=816 clock_slot_live_calls=488 \
             clock_slot_store_reads=4162 held_ms=1.000 after_capped_ms=1.500 \
             total_ms=9941.500 before_gates_ms=2.000 gates_wait_ms=4.000 gates_held_ms=6.000 \
             cycle_inputs_ms=10.000 policy_lock_wait_ms=12.000 evaluation_setup_ms=14.000 \
             no_progress_ms=18.000 detection_preview_ms=9600.000 \
             detection_preview_store_reads=1200 explanation_ms=20.000 after_held_ms=22.000 \
             capped_record_ms=24.000 unattributed_ms=3.000"
        ),
        "one line for both"
    );

    // A measurement whose named parts exceed the time up to the capped record names nothing
    // uncovered.
    let mut over = measurement(10);
    over.until_capped_micros = 1_000;
    let line = lines
        .record(13_000, PolicyTrigger::Reconciliation, &over, TOTAL)
        .expect("a line a second after the last");
    assert!(
        line.ends_with(" capped_record_ms=12.000 unattributed_ms=0.000"),
        "{line}"
    );
}

/// A cycle the driver evaluated, naming no wake.
fn evaluated_cycle() -> PolicyCycle {
    PolicyCycle {
        directive: PolicyRecomputeDirective {
            kind: PolicyRecomputeKind::Full,
            reason: PolicyRecomputeReason::StartupOrRecovery,
            eligible_at_unix_ms: NOW,
        },
        evaluation: Some(actingcommand_policy::PolicyEvaluation {
            decisions: Vec::new(),
            next_wake_unix_ms: None,
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
        idle_wake_unix_ms: None,
    }
}

/// The schedule's work at `elapsed_ms` on its monotonic clock, the wall clock reading `wall_ms`
/// past `NOW`.
fn due(
    schedule: &mut PolicyDriverSchedule,
    wall_ms: u64,
    elapsed_ms: u64,
) -> Option<(PolicyTrigger, Vec<PolicyRecomputeWake>)> {
    schedule
        .take_due_work(NOW + wall_ms, elapsed_ms)
        .expect("the schedule's due work")
}

/// What the driver noted as having woken it, as the next evaluated cycle's line names it.
fn wakes_noted(schedule: &mut PolicyDriverSchedule, at_ms: u64) -> String {
    let line = schedule
        .lines
        .record(at_ms, PolicyTrigger::FactsChanged, &measurement(1), TOTAL)
        .expect("a line a second or more after the last");
    line.split(' ')
        .find_map(|member| member.strip_prefix("wakes="))
        .expect("the line's wakes")
        .to_owned()
}

#[test]
fn cyc1_t3_a_long_cycle_is_no_clock_jump_and_a_wall_clock_set_forward_or_back_wakes_once() {
    // The defaults' threshold, 5 000 ms (`PolicyCadence::default`); the reconciliation's interval
    // pushed out of the way.
    let cadence = PolicyCadence {
        reconciliation_interval_ms: 3_600_000,
        ..PolicyCadence::default()
    };
    assert_eq!(cadence.clock_jump_threshold_ms, 5_000);
    let mut schedule =
        PolicyDriverSchedule::new(cadence, &evaluated_cycle(), NOW).expect("the schedule");

    // Two cycles of 5.2 s back to back, as on production: both clocks moved by them.
    assert_eq!(due(&mut schedule, 5_200, 5_200), None, "a long cycle");
    assert_eq!(due(&mut schedule, 10_400, 10_400), None, "and another");
    assert_eq!(
        wakes_noted(&mut schedule, 10_400),
        "none",
        "nothing woke the driver"
    );

    // A look that is merely late: the driver slept five minutes, both clocks agree.
    assert_eq!(due(&mut schedule, 310_400, 310_400), None, "late, no jump");
    // Within the threshold the clocks may disagree.
    assert_eq!(
        due(&mut schedule, 315_400, 311_400),
        None,
        "4 s apart, no jump"
    );

    // The wall clock set forward by six seconds: one clock jump, then none.
    assert_eq!(
        due(&mut schedule, 321_500, 311_500),
        Some((PolicyTrigger::ClockObserved { jumped: true }, Vec::new())),
        "set forward"
    );
    assert_eq!(due(&mut schedule, 321_600, 311_600), None, "once");
    assert_eq!(
        wakes_noted(&mut schedule, 311_600),
        "clock_jump:1",
        "one clock jump woke the driver"
    );

    // The wall clock set back behind the last look: one clock jump, then none.
    assert_eq!(
        due(&mut schedule, 320_000, 311_700),
        Some((PolicyTrigger::ClockObserved { jumped: true }, Vec::new())),
        "set back"
    );
    assert_eq!(due(&mut schedule, 320_100, 311_800), None, "once");

    // Set back while it still moved forward: 2 s on the wall clock against 9 s of monotonic time.
    assert_eq!(
        due(&mut schedule, 322_000, 320_700),
        Some((PolicyTrigger::ClockObserved { jumped: true }, Vec::new())),
        "behind the monotonic clock by more than the threshold"
    );
    assert_eq!(due(&mut schedule, 322_100, 320_800), None, "once");
    assert_eq!(
        wakes_noted(&mut schedule, 320_800),
        "clock_jump:2",
        "two clock jumps woke the driver since the last line"
    );

    // A time the driver waited for: an observation, no jump.
    schedule.wake_no_later_than(NOW + 330_000);
    assert_eq!(due(&mut schedule, 329_999, 328_699), None, "not yet");
    assert_eq!(
        due(&mut schedule, 330_000, 328_700),
        Some((PolicyTrigger::ClockObserved { jumped: false }, Vec::new())),
        "the time came"
    );
}
