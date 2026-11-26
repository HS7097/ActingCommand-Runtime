// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-2 (0.12.85; register §1.528, the coordinator's ruling 1), actingd's half. The times are
// given (the driver's monotonic milliseconds, the calls' microseconds), so nothing reads a clock.
// - t1: the daemon's line of a cycle ends with `evaluation_setup_slowest`, the three calls of the
//   evaluation's setup that took longest, `name:milliseconds` each, the slowest first; a line for
//   several cycles names the three slowest of the calls' sums, of two that took as long the earlier
//   in the cycle's order first.

use super::policy_cycle_line::PolicyCycleLines;
use super::*;
use actingcommand_runtime_host::{
    ClockSlotPreviewCost, PolicyCycleStretches, PolicyEvaluationCost, PolicyEvaluationExecution,
    PolicyEvaluationMeasurement, SetupCall, SetupCallTimes,
};

/// A cycle of the shape the line reads on production on 11-26 (3.6 s in the evaluation's setup),
/// its calls of the setup `calls`.
fn measurement(seq: u64, calls: SetupCallTimes) -> PolicyEvaluationMeasurement {
    PolicyEvaluationMeasurement {
        sampled_at_monotonic_ms: 0,
        elapsed_micros: 9_000,
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
        projection_micros: 190_000,
        clock_slots: ClockSlotPreviewCost::default(),
        held_observe_micros: 0,
        until_capped_micros: 3_900_000,
        stretches: PolicyCycleStretches {
            evaluation_setup_micros: 3_700_000,
            evaluation_setup_calls: calls,
            ..PolicyCycleStretches::default()
        },
    }
}

/// The whole cycle's time as the driver times it.
const TOTAL: Duration = Duration::from_micros(4_000_000);

/// The member the line ends with.
fn slowest_member(line: &str) -> &str {
    line.rsplit(' ')
        .next()
        .and_then(|member| member.strip_prefix("evaluation_setup_slowest="))
        .unwrap_or_else(|| panic!("the line ends with the slowest calls: {line}"))
}

#[test]
fn cyc2_t1_the_line_ends_with_the_three_slowest_calls_of_the_setup() {
    let mut one = SetupCallTimes::default();
    for (call, micros) in [
        (SetupCall::OutcomeKeys, 500),
        (SetupCall::Cadence, 500),
        (SetupCall::RemedyPlan, 100_000),
        (SetupCall::CatalogReads, 3_590_000),
        (SetupCall::RunSlots, 2_000),
    ] {
        one.add(call, micros);
    }
    let mut lines = PolicyCycleLines::default();
    let line = lines
        .record(
            10_000,
            PolicyTrigger::FactsChanged,
            &measurement(3, one),
            TOTAL,
        )
        .expect("the first line");
    assert_eq!(
        slowest_member(&line),
        "catalog_reads:3590.000,remedy_plan:100.000,run_slots:2.000",
        "{line}"
    );

    // Two cycles within a second: the calls summed; the three then share the third place's time,
    // and the earlier call in the cycle's order is named.
    let mut other = SetupCallTimes::default();
    for (call, micros) in [(SetupCall::OutcomeKeys, 1_500), (SetupCall::Cadence, 1_500)] {
        other.add(call, micros);
    }
    assert_eq!(
        lines.record(
            10_400,
            PolicyTrigger::FactsChanged,
            &measurement(4, one),
            TOTAL
        ),
        None,
        "kept"
    );
    assert_eq!(
        lines.record(
            10_800,
            PolicyTrigger::FactsChanged,
            &measurement(5, other),
            TOTAL
        ),
        None,
        "kept"
    );
    let line = lines.flush(11_000).expect("one line for both");
    assert_eq!(
        slowest_member(&line),
        "catalog_reads:3590.000,remedy_plan:100.000,outcome_keys:2.000",
        "{line}"
    );

    // A cycle whose setup no call took any time of names none.
    let line = lines
        .record(
            13_000,
            PolicyTrigger::Reconciliation,
            &measurement(6, SetupCallTimes::default()),
            TOTAL,
        )
        .expect("a line a second after the last");
    assert_eq!(slowest_member(&line), "none", "{line}");
}
