// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #361 R7, test row R7-T7 (M361 addendum of 2026-10-15; forced-compromises mapping
// S-08): the P-2a G9 day replay (`gate_p2a.rs`, fakes I2 and I3) of one task with a daily clock
// slot at 06:30 UTC, a daily limit of 1 and an activity window from 06:00 to 23:30. The slot of
// day 0 was served at its time. Admission holds the instance from 06:00 of day 1 until 05:00 of
// day 2, as an operator pause does. With `missed: skip_after` of two hours, day 1's occurrence
// is never dispatched and day 2's slot runs at 06:30 on day 2's budget. Without `missed` the
// same replay shows the displacement S-08 names: the owed run at day 2's window start takes the
// day's only allowance and day 2's own 06:30 is refused. Every expected value comes from the
// model row.

use serde_json::json;

use super::gate_p2a::{
    BUDGET_EXHAUSTED, FIRST, FakeAdmission, INSTANCE_A, Refusal, SEED, at, catalog_of_version,
    decision, facts, instance, last_reason, local_day, resources, task, task_state,
};
use crate::evaluate_with_eligibility;

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
// Midnight UTC of the replay's first day, and of the next.
const DAY_1: u64 = 400 * DAY_MS;
const DAY_2: u64 = DAY_1 + DAY_MS;
// The task's daily slot, 06:30, and the grace of its `missed` rule, two hours.
const SLOT: u64 = 6 * HOUR_MS + 30 * MINUTE_MS;
const GRACE_MS: u64 = 7_200_000;
// The pause: from 06:00 of day 1 until 05:00 of day 2.
const PAUSED_FROM: u64 = DAY_1 + 6 * HOUR_MS;
const PAUSED_UNTIL: u64 = DAY_2 + 5 * HOUR_MS;
// The activity window, 06:00 to 23:30 UTC.
const WINDOW_START_MINUTE: u16 = 360;
const WINDOW_END_MINUTE: u16 = 1_410;

// What two replayed days left: the dispatch times, the times the pair was refused by its
// budget, and the admissions of each day.
struct Replay {
    dispatched: Vec<u64>,
    budget_refused: Vec<u64>,
    admission: FakeAdmission,
}

// Replays day 1 and day 2 in minute steps; `grace_ms` gives the clock its `missed` rule.
fn replay(grace_ms: Option<u64>) -> Replay {
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
            "minutes_of_day": [SLOT / MINUTE_MS]
        }
    });
    if let Some(grace_ms) = grace_ms {
        clock["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    }
    let catalog = catalog_of_version(
        crate::SCHEDULING_SCHEMA_VERSION_V2,
        vec![task(FIRST, clock, 1, json!([]))],
        WINDOW_START_MINUTE,
        WINDOW_END_MINUTE,
    );
    let mut admission = FakeAdmission::default();
    admission.profiles = catalog.catalog().activity.profiles.clone();
    let instances = vec![instance(INSTANCE_A, "host.a")];
    let hosts = resources(&instances);
    let mut last_dispatched = Some(DAY_1 - DAY_MS + SLOT);
    let mut dispatched = Vec::new();
    let mut budget_refused = Vec::new();
    let mut now = DAY_1;
    while now < DAY_2 + DAY_MS {
        if (PAUSED_FROM..PAUSED_UNTIL).contains(&now) {
            admission.held.insert(INSTANCE_A.to_owned());
        } else {
            admission.held.remove(INSTANCE_A);
        }
        let evaluation = evaluate_with_eligibility::<Refusal>(
            &catalog,
            &facts(instances.clone(), vec![task_state(last_dispatched)]),
            &hosts,
            at(now),
            SEED,
            |intent| admission.preview(intent),
        )
        .unwrap_or_else(|error| panic!("at {now}: the replay evaluates: {error:?}"));
        if last_reason(decision(&evaluation, FIRST, INSTANCE_A)) == BUDGET_EXHAUSTED {
            budget_refused.push(now);
        }
        for intent in &evaluation.dispatch_intents {
            admission.admit(intent);
            last_dispatched = Some(now);
            dispatched.push(now);
        }
        now += MINUTE_MS;
    }
    Replay {
        dispatched,
        budget_refused,
        admission,
    }
}

#[test]
fn r7_t7_a_missed_clock_slot_with_skip_after_expires_and_never_displaces_the_next_day_s_slot() {
    let skipped = replay(Some(GRACE_MS));
    assert_eq!(
        skipped.dispatched,
        [DAY_2 + SLOT],
        "day 1's occurrence is never dispatched; day 2's slot runs at its own time"
    );
    assert_eq!(
        skipped.admission.used(FIRST, INSTANCE_A, local_day(DAY_1)),
        0
    );
    assert_eq!(
        skipped.admission.used(FIRST, INSTANCE_A, local_day(DAY_2)),
        1,
        "day 2's slot runs on day 2's budget"
    );
    assert!(
        skipped.budget_refused.is_empty(),
        "nothing is displaced: {:?}",
        skipped.budget_refused
    );

    let displaced = replay(None);
    assert_eq!(
        displaced.dispatched,
        [DAY_2 + u64::from(WINDOW_START_MINUTE) * MINUTE_MS],
        "without missed, day 1's occurrence is owed and runs at day 2's window start"
    );
    assert_eq!(
        displaced
            .admission
            .used(FIRST, INSTANCE_A, local_day(DAY_2)),
        1,
        "the owed run takes day 2's only allowance"
    );
    assert_eq!(
        displaced.budget_refused.first(),
        Some(&(DAY_2 + SLOT)),
        "day 2's own 06:30 is refused by the spent budget"
    );
}
