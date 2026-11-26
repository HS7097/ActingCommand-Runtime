// SPDX-License-Identifier: AGPL-3.0-only

// #503 CYC-1 (0.12.84; register §1.526, the coordinator's rulings 1 and 2), the host's half, on
// the detection fixture (one instance, a task whose trigger fact is unknown, a detection budget of
// two runs a window) with given evaluation times and no clock read.
// - h1 (ruling 1): a delay put into each stretch of a cycle (the two older hooks for what precedes
//   the gates and for the cycle's other inputs; the lap clock's test seams for the rest) shows in
//   that stretch's member of the measurement and in no other, and what no named part covers up to
//   the capped record stays small.
// - h2 (ruling 2): with nothing changed, a second and a third cycle ask the store nothing; the
//   reservation is recorded once; a new snapshot's reservation counts on the usage this host
//   recorded for its window, and asks only its own id; the quota exhausted, its record once, and
//   the cycle after it asks nothing and hands the recorder the same record.

use super::*;
use crate::policy_host::{
    CycleStretch, advance_lap_clock_for_test, plan_cycle_stretch_delay_for_test,
};
use crate::{PolicyCycle, PolicyEvaluationMeasurement};
use std::time::Duration;

/// The delay put into one stretch: longer than any stretch of these cycles takes by itself.
const DELAY: Duration = Duration::from_secs(600);
const DELAY_MICROS: u64 = 600_000_000;
/// The most the parts no member names may take in these cycles.
const UNATTRIBUTED_BOUND_MICROS: u64 = 1_000_000;

/// Each part of the host's cycle up to its capped record, as the daemon's line names it.
fn cycle_parts(measurement: &PolicyEvaluationMeasurement) -> [(&'static str, u64); 15] {
    let stretches = measurement.stretches;
    [
        ("before_gates", stretches.before_gates_micros),
        ("gates_wait", stretches.gates_wait_micros),
        ("gates_held", stretches.gates_held_micros),
        ("projection", measurement.projection_micros),
        ("cycle_inputs", stretches.cycle_inputs_micros),
        ("policy_lock_wait", stretches.policy_lock_wait_micros),
        ("evaluation_setup", stretches.evaluation_setup_micros),
        ("evaluation", measurement.elapsed_micros),
        ("no_progress", stretches.no_progress_micros),
        ("detection_preview", stretches.detection_preview_micros),
        ("clock_slots", measurement.clock_slots.micros),
        ("explanation", stretches.explanation_micros),
        ("held", measurement.held_observe_micros),
        ("after_held", stretches.after_held_micros),
        ("capped_record", stretches.capped_record_micros),
    ]
}

fn detection_host(root: &TempDir) -> RuntimeHost {
    let host = RuntimeHost::start(
        config(root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("detection runtime host");
    host.activate_policy_catalog(&detection_policy_sources(1))
        .expect("activate detection catalog");
    host
}

/// The `index`th evaluated cycle, two seconds after the one before, on the facts of `snapshot`.
fn detection_cycle(host: &RuntimeHost, index: u64, snapshot: &str) -> PolicyCycle {
    let now_unix_ms = POLICY_NOW_UNIX_MS + index * 2_000;
    host.evaluate_policy_cycle_with_test_inputs(
        &detection_policy_facts(false, snapshot),
        &policy_resources(),
        EvaluationTime {
            unix_ms: now_unix_ms,
            monotonic_ms: now_unix_ms,
        },
        index + 1,
        PolicyTrigger::FactsChanged,
    )
    .expect("an evaluated detection cycle")
}

/// How a stretch of the next cycle gets its delay: an older hook of the host, or a seam of the lap
/// clock.
enum PutDelay {
    Hook(fn(&RuntimeHost)),
    Seam(CycleStretch),
}

fn delay_before_gates(host: &RuntimeHost) {
    host.run_before_policy_inputs_read_for_test(Box::new(|| advance_lap_clock_for_test(DELAY)))
        .expect("the hook before the gates");
}

fn delay_cycle_inputs(host: &RuntimeHost) {
    host.run_between_policy_sample_and_validation_for_test(Box::new(|| {
        advance_lap_clock_for_test(DELAY);
    }))
    .expect("the hook after the cycle's inputs");
}

#[test]
fn cyc1_h1_a_delay_put_into_each_stretch_shows_in_its_member_and_in_no_other() {
    let root = TempDir::new().expect("tempdir");
    let host = detection_host(&root);
    // Every stretch the line gained, in the cycle's order.
    let cases = [
        ("before_gates", PutDelay::Hook(delay_before_gates)),
        ("gates_wait", PutDelay::Seam(CycleStretch::GatesWait)),
        ("gates_held", PutDelay::Seam(CycleStretch::GatesHeld)),
        ("cycle_inputs", PutDelay::Hook(delay_cycle_inputs)),
        (
            "policy_lock_wait",
            PutDelay::Seam(CycleStretch::PolicyLockWait),
        ),
        (
            "evaluation_setup",
            PutDelay::Seam(CycleStretch::EvaluationSetup),
        ),
        ("no_progress", PutDelay::Seam(CycleStretch::NoProgress)),
        (
            "detection_preview",
            PutDelay::Seam(CycleStretch::DetectionPreview),
        ),
        ("explanation", PutDelay::Seam(CycleStretch::Explanation)),
        ("after_held", PutDelay::Seam(CycleStretch::AfterHeld)),
        ("capped_record", PutDelay::Seam(CycleStretch::CappedRecord)),
    ];

    for (index, (member, put_delay)) in cases.into_iter().enumerate() {
        match put_delay {
            PutDelay::Hook(hook) => hook(&host),
            PutDelay::Seam(stretch) => plan_cycle_stretch_delay_for_test(stretch, DELAY),
        }
        let cycle = detection_cycle(
            &host,
            u64::try_from(index).expect("a small index"),
            "snapshot:cyc1-stretches",
        );
        let measurement = cycle
            .measurement
            .expect("an evaluated cycle carries its measurement");
        let parts = cycle_parts(&measurement);
        for (name, micros) in parts {
            if name == member {
                assert!(
                    micros >= DELAY_MICROS,
                    "{member}: the delay shows in its own member ({micros} us)"
                );
            } else {
                assert!(
                    micros < DELAY_MICROS,
                    "{member}: the delay does not show in {name} ({micros} us)"
                );
            }
        }
        let named = parts.iter().map(|(_, micros)| micros).sum::<u64>();
        assert!(
            measurement.until_capped_micros >= named,
            "{member}: the named parts lie within the time up to the capped record"
        );
        assert!(
            measurement.until_capped_micros - named < UNATTRIBUTED_BOUND_MICROS,
            "{member}: what no member names stays small ({} us)",
            measurement.until_capped_micros - named
        );
    }
    host.close().expect("close the runtime");
}

#[test]
fn cyc1_h2_with_nothing_changed_the_detection_preview_asks_the_store_nothing_again() {
    let root = TempDir::new().expect("tempdir");
    let host = detection_host(&root);
    let reads = |cycle: &PolicyCycle| {
        cycle
            .measurement
            .expect("an evaluated cycle carries its measurement")
            .stretches
            .detection_preview_store_reads
    };

    // The first cycle: the reservation and the window, each asked once; the window holds no usage
    // and is not asked again for the reservation's own id.
    let first = detection_cycle(&host, 1, "snapshot:cyc1-a");
    assert_eq!(first.detection_planning_signals.len(), 1);
    assert_eq!(
        first.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionReserved
    );
    assert_eq!(
        first.detection_planning_signals[0]
            .detection_budget
            .as_ref()
            .expect("detection budget")
            .dispatch_used,
        1
    );
    assert_eq!(reads(&first), 2, "the reservation's id and its window");

    // Nothing changed: the second and the third cycle reserve nothing and ask the store nothing.
    for index in 2..=3 {
        let again = detection_cycle(&host, index, "snapshot:cyc1-a");
        assert!(
            again.detection_planning_signals.is_empty(),
            "cycle {index}: the reservation is recorded already"
        );
        assert_eq!(reads(&again), 0, "cycle {index}: nothing asked again");
    }

    // A new snapshot: its reservation counts on the usage this host recorded for the window, read
    // as before, and only its own id is asked.
    let next = detection_cycle(&host, 4, "snapshot:cyc1-b");
    assert_eq!(next.detection_planning_signals.len(), 1);
    let budget = next.detection_planning_signals[0]
        .detection_budget
        .as_ref()
        .expect("detection budget");
    assert_eq!(
        next.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionReserved
    );
    assert_eq!(budget.dispatch_used, 2, "the first reservation counted");
    assert_eq!(budget.runtime_reserved_ms, 20_000);
    assert_eq!(reads(&next), 1, "the new reservation's id alone");

    // The budget spent: the exhausted record, asked once; with nothing changed the next cycle asks
    // nothing and hands the recorder the same record, which records nothing new.
    let exhausted = detection_cycle(&host, 5, "snapshot:cyc1-c");
    assert_eq!(exhausted.detection_planning_signals.len(), 1);
    assert_eq!(
        exhausted.detection_planning_signals[0].kind,
        PolicyPlanningSignalKind::DetectionQuotaExhausted
    );
    assert_eq!(
        reads(&exhausted),
        2,
        "the reservation's id and the exhausted id"
    );
    let same = detection_cycle(&host, 6, "snapshot:cyc1-c");
    assert_eq!(
        same.detection_planning_signals, exhausted.detection_planning_signals,
        "the same exhausted record"
    );
    assert_eq!(reads(&same), 0, "nothing asked again");

    // Each record once: two reservations and one exhaustion.
    let mut client = TestClient::connect(&host);
    let signals = projected_events(
        &mut client,
        EventQuery {
            event_type: Some(EventType::PolicyPlanningSignalObserved),
            ..EventQuery::default()
        },
    );
    assert_eq!(signals.len(), 3, "each planning signal recorded once");
    drop(client);
    host.close().expect("close the runtime");
}
