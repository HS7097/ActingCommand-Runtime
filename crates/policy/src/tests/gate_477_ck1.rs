// SPDX-License-Identifier: AGPL-3.0-only

// #477 CK-1 (0.12.83; register §1.510; the coordinator's rulings 1 to 8 of 11-23, no words of the
// owner's behind them; contracts/scheduling/README.md, "Clock Sources"; contracts/standard-pack-
// schedule.md, "`requires`"): a clock that occurs once a month, minute M of day D (1 to 28) of
// every month of the Gregorian calendar, in its source's local time, and in every other respect a
// weekly clock. On the P-2a fixture (one task, one instance, the activity window open all day), no
// clock read; every expected instant is the calendar's, written out in UTC beside it.
// - the occurrence across a month's end, a year's end, February of a leap and of a common year,
//   the century rule, a source ahead of and one behind UTC, and a revealed source with daylight
//   saving and drift, which reaches its local time as the weekly kind does (t1); a `local` source
//   is refused as for the weekly kind (t1);
// - day 28 is taken, days 0 and 29 and minute 1440 refused with the weekly kind's diagnostic (t2);
// - owed from its occurrence until a dispatch serves it, and after CK-2's two cases (t3);
// - the next wake is the next month's occurrence, a `missed` grace bounding it (t4);
// - a `schedule.json` that writes one names `clock_monthly` in `requires` (t5);
// - a timeline event with a monthly schedule (t6);
// - a catalogue without the kind keeps its golden identities (t7).

use std::collections::{BTreeMap, BTreeSet};

use actingcommand_contract::{
    STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem, StandardPackRootBytes,
    read_standard_pack_root,
};
use serde_json::{Value, json};

use super::gate_p2a::{
    FIRST, INSTANCE_A, SEED, at, decision, facts, instance, last_reason, resources, task,
};
use crate::{
    CLOCK_MONTHLY_SUBSYSTEM, CatalogDiagnosticCode, CatalogDocumentSource, CatalogSources,
    ClockSchedule, ClockSlotView, CompiledCatalog, EligibilityState, PolicyCode, PolicyEvaluation,
    PredicateSpec, SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2, StandardPackBinding,
    StandardPackComposeFailure, StandardPackComposeInput, StandardPackComposition,
    StandardPackGameFiles, TaskRuntimeSnapshot, TaskTerminalState, TimelineAvailability,
    TimelineQueryContext, clock_slot_view, compile_catalog, compose_standard_pack_rules, evaluate,
    inspect_timeline,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
const WEEK_MS: u64 = 7 * DAY_MS;
const GAME: &str = "game-a";
const SERVER: &str = "server-a";
const PACKAGE: &str = "fixture.monthly";
const EVENT: &str = "fixture.monthly-window";
const TRIGGER_FALSE: &str = "trigger_false";
// The grace of t4's `missed` rule: two hours.
const GRACE_MS: u64 = 7_200_000;

// t1, UTC.
const JAN_28_2026_2300: u64 = 1_769_641_200_000; // 2026-01-28T23:00Z
const FEB_28_2026_2300: u64 = 1_772_319_600_000; // 2026-02-28T23:00Z
const MAR_28_2026_2300: u64 = 1_774_738_800_000; // 2026-03-28T23:00Z
const MAR_01_2026: u64 = 1_772_323_200_000; // 2026-03-01T00:00Z
const DEC_01_2026: u64 = 1_796_083_200_000; // 2026-12-01T00:00Z
const JAN_01_2027: u64 = 1_798_761_600_000; // 2027-01-01T00:00Z
const FEB_01_2027: u64 = 1_801_440_000_000; // 2027-02-01T00:00Z
const FEB_01_2024: u64 = 1_706_745_600_000; // 2024-02-01T00:00Z
const FEB_28_2024: u64 = 1_709_078_400_000; // 2024-02-28T00:00Z
const MAR_01_2024: u64 = 1_709_251_200_000; // 2024-03-01T00:00Z
const MAR_28_2024: u64 = 1_711_584_000_000; // 2024-03-28T00:00Z
const FEB_01_2025: u64 = 1_738_368_000_000; // 2025-02-01T00:00Z
const MAR_01_2025: u64 = 1_740_787_200_000; // 2025-03-01T00:00Z
const FEB_01_2000: u64 = 949_363_200_000; // 2000-02-01T00:00Z
const MAR_01_2000: u64 = 951_868_800_000; // 2000-03-01T00:00Z
const FEB_01_2100: u64 = 4_105_123_200_000; // 2100-02-01T00:00Z
const FEB_28_2100: u64 = 4_107_456_000_000; // 2100-02-28T00:00Z
const MAR_01_2100: u64 = 4_107_542_400_000; // 2100-03-01T00:00Z
const MAR_28_2100: u64 = 4_109_875_200_000; // 2100-03-28T00:00Z
// t1 and t6, a server clock nine hours ahead of UTC, day 1 at minute 240 (04:00 local): the
// UTC day before, 19:00, the last day of the month before in UTC.
const NOV_01_0400_AHEAD: u64 = 1_793_473_200_000; // 2026-10-31T19:00Z
const DEC_01_0400_AHEAD: u64 = 1_796_065_200_000; // 2026-11-30T19:00Z
const JAN_01_0400_AHEAD: u64 = 1_798_743_600_000; // 2026-12-31T19:00Z
// t1, a server clock ten hours behind UTC, day 28 at minute 1380 (23:00 local): 09:00 UTC of the
// day after, March 1 for February.
const JAN_28_2300_BEHIND: u64 = 1_769_677_200_000; // 2026-01-29T09:00Z
const FEB_28_2300_BEHIND: u64 = 1_772_355_600_000; // 2026-03-01T09:00Z
const MAR_28_2300_BEHIND: u64 = 1_774_774_800_000; // 2026-03-29T09:00Z
// t1, a revealed clock of UTC offset 480, daylight saving 60 and drift 120 000 ms, day 15 at
// minute 600 (10:00 local, a Tuesday in December 2026): 01:02 UTC.
const NOV_15_1000_REVEALED: u64 = 1_794_704_520_000; // 2026-11-15T01:02Z
const DEC_15_1000_REVEALED: u64 = 1_797_296_520_000; // 2026-12-15T01:02Z
const JAN_15_1000_REVEALED: u64 = 1_799_974_920_000; // 2027-01-15T01:02Z
// t3 and t4: day 1 at 07:00 UTC, a Tuesday in December 2026, which the weekly clock on Tuesday
// at 07:00 UTC shares; the monthly clock's next occurrence.
const OCCURRENCE: u64 = 1_796_108_400_000; // 2026-12-01T07:00Z
const NEXT_MONTH: u64 = 1_798_786_800_000; // 2027-01-01T07:00Z
// The dispatch that answered the occurrence, a minute after it.
const ANSWERED: u64 = OCCURRENCE + MINUTE_MS;
// A dispatch a week earlier, which served the previous occurrence of either clock.
const EARLIER: u64 = OCCURRENCE - WEEK_MS + MINUTE_MS;

// The neutral example catalog's golden identities (as in `gate_ord1`).
const V1_EXAMPLE_HASH: &str =
    "sha256:24dc8c0b4d63c03ad77ebe8a57e91186070ee0d379f17a3374060207cc3f257c";
const V2_EXAMPLE_HASH: &str =
    "sha256:baf684c3f5864fb105ed351c2553831e7af1d76b165bd0a9233db7b26a725047";

// t1: across a month's end, a year's end, the four Februaries and the century rule.
#[test]
fn ck1_t1_occurrences_across_a_months_end_a_years_end_februaries_and_centuries() {
    // Day 28 at 23:00: before, at and after February's occurrence, across the end of February.
    let end_of_month = || monthly(utc(), 28, 1_380);
    assert_eq!(
        occurrences(end_of_month(), FEB_28_2026_2300 - 1),
        (Some(JAN_28_2026_2300), Some(FEB_28_2026_2300))
    );
    assert_eq!(
        occurrences(end_of_month(), FEB_28_2026_2300),
        (Some(FEB_28_2026_2300), Some(MAR_28_2026_2300))
    );
    assert_eq!(
        occurrences(end_of_month(), MAR_01_2026),
        (Some(FEB_28_2026_2300), Some(MAR_28_2026_2300))
    );
    // Day 1 at 00:00 across the end of 2026.
    assert_eq!(
        occurrences(monthly(utc(), 1, 0), JAN_01_2027 - 1),
        (Some(DEC_01_2026), Some(JAN_01_2027))
    );
    assert_eq!(
        occurrences(monthly(utc(), 1, 0), JAN_01_2027),
        (Some(JAN_01_2027), Some(FEB_01_2027))
    );
    // Day 1 at noon of February's last day: February has 29 days in 2024 and 2000 (a century
    // divisible by 400), 28 in 2025 and 2100 (a century that is not).
    for (february, march, days) in [
        (FEB_01_2024, MAR_01_2024, 29),
        (FEB_01_2025, MAR_01_2025, 28),
        (FEB_01_2000, MAR_01_2000, 29),
        (FEB_01_2100, MAR_01_2100, 28),
    ] {
        let now = march - 12 * HOUR_MS;
        assert_eq!(
            occurrences(monthly(utc(), 1, 0), now),
            (Some(february), Some(march)),
            "at {now}"
        );
        assert_eq!(march - february, days * DAY_MS, "at {now}");
    }
    // Day 28: February 29 of 2024 follows its occurrence in February; 2100 has no February 29,
    // so March 1 follows its occurrence by one day.
    assert_eq!(
        occurrences(monthly(utc(), 28, 0), MAR_01_2024 - 12 * HOUR_MS),
        (Some(FEB_28_2024), Some(MAR_28_2024))
    );
    assert_eq!(
        occurrences(monthly(utc(), 28, 0), MAR_01_2100 + 12 * HOUR_MS),
        (Some(FEB_28_2100), Some(MAR_28_2100))
    );
    assert_eq!(MAR_01_2100 - FEB_28_2100, DAY_MS);
}

// t1: the local time of a source ahead of UTC, of one behind it, and of a revealed source with
// daylight saving and drift, reached by the same offsets as the weekly kind's.
#[test]
fn ck1_t1_a_source_ahead_of_or_behind_utc_reaches_its_local_day_as_the_weekly_kind() {
    // Nine hours ahead: day 1 at 04:00 local is 19:00 UTC of the day before.
    let ahead = || monthly(server(540), 1, 240);
    assert_eq!(
        occurrences(ahead(), DEC_01_0400_AHEAD - 1),
        (Some(NOV_01_0400_AHEAD), Some(DEC_01_0400_AHEAD))
    );
    assert_eq!(
        occurrences(ahead(), DEC_01_0400_AHEAD),
        (Some(DEC_01_0400_AHEAD), Some(JAN_01_0400_AHEAD))
    );
    assert_eq!(
        DEC_01_0400_AHEAD / DAY_MS,
        DEC_01_2026 / DAY_MS - 1,
        "the occurrence's UTC day is the day before its local day"
    );
    // Ten hours behind: day 28 at 23:00 local is 09:00 UTC of the day after.
    let behind = || monthly(server(-600), 28, 1_380);
    assert_eq!(
        occurrences(behind(), FEB_28_2300_BEHIND - 1),
        (Some(JAN_28_2300_BEHIND), Some(FEB_28_2300_BEHIND))
    );
    assert_eq!(
        occurrences(behind(), FEB_28_2300_BEHIND),
        (Some(FEB_28_2300_BEHIND), Some(MAR_28_2300_BEHIND))
    );
    // Revealed, 480 + 60 minutes ahead with 120 000 ms of drift: the monthly occurrence on a
    // Tuesday is the weekly clock's on Tuesday at the same minute.
    let after = DEC_15_1000_REVEALED + HOUR_MS;
    assert_eq!(
        occurrences(monthly(revealed(), 15, 600), DEC_15_1000_REVEALED - 1),
        (Some(NOV_15_1000_REVEALED), Some(DEC_15_1000_REVEALED))
    );
    assert_eq!(
        occurrences(monthly(revealed(), 15, 600), after),
        (Some(DEC_15_1000_REVEALED), Some(JAN_15_1000_REVEALED))
    );
    assert_eq!(
        occurrences(weekly(revealed(), 2, 600), after),
        (
            Some(DEC_15_1000_REVEALED),
            Some(DEC_15_1000_REVEALED + WEEK_MS)
        )
    );
}

// t1: a `local` source, the host's monotonic coordinate, names no wall-clock day: the monthly kind
// is refused with it as the weekly kind is.
#[test]
fn ck1_t1_a_local_source_is_refused_as_for_the_weekly_kind() {
    let local = || json!({"kind": "local"});
    let weekly_refused = refusals(clock(weekly(local(), 2, 420)));
    assert!(
        weekly_refused.contains(&outside_bounds()),
        "{weekly_refused:?}"
    );
    assert_eq!(refusals(clock(monthly(local(), 1, 420))), weekly_refused);
}

// t2: day 28 and minute 1439 are taken, in V1 and V2; day 0, day 29 and minute 1440 are refused
// with the diagnostic an out-of-range `weekday` or `minute_of_day` of the weekly kind gets.
#[test]
fn ck1_t2_day_28_is_taken_and_days_0_and_29_and_minute_1440_are_refused_as_the_weekly_kind() {
    for version in [SCHEDULING_SCHEMA_VERSION, SCHEDULING_SCHEMA_VERSION_V2] {
        let taken = catalog_with(version, clock(monthly(utc(), 28, 1_439)), Vec::new());
        assert!(
            matches!(
                &taken.catalog().tasks.tasks[0].trigger,
                PredicateSpec::Clock {
                    schedule: ClockSchedule::Monthly {
                        day_of_month: 28,
                        minute_of_day: 1_439,
                        ..
                    },
                    missed: None,
                }
            ),
            "{version}"
        );
    }
    for (weekday, minute_of_day) in [(0, 0), (8, 0), (2, 1_440)] {
        let weekly_refused = refusals(clock(weekly(utc(), weekday, minute_of_day)));
        assert_eq!(
            weekly_refused,
            [outside_bounds()],
            "{weekday}, {minute_of_day}"
        );
    }
    for (day_of_month, minute_of_day) in [(0, 0), (29, 0), (28, 1_440)] {
        assert_eq!(
            refusals(clock(monthly(utc(), day_of_month, minute_of_day))),
            [outside_bounds()],
            "{day_of_month}, {minute_of_day}"
        );
    }
}

// t3: owed from its occurrence until a dispatch serves it, as the weekly clock that shares the
// occurrence: before it nothing is due; from it the pair is dispatched (an hour and six days
// later too); a dispatch admitted after it serves it, succeeded or failed; CK-2's two cases (a
// dispatch that settled as interrupted, and one whose failure paused the pair, both projected by
// the Runtime as a failed snapshot with `clock_owed`) leave it owed, and the run after them serves
// it, until the kind's next occurrence.
#[test]
fn ck1_t3_an_occurrence_is_owed_until_a_dispatch_serves_it_and_after_ck2_s_two_cases() {
    for (kind, schedule, next) in kinds() {
        let catalog = catalog_with(SCHEDULING_SCHEMA_VERSION, clock(schedule), Vec::new());
        let earlier = ran(EARLIER, TaskTerminalState::Succeeded, false);
        assert!(
            !selected(&evaluate_at(
                &catalog,
                Some(earlier.clone()),
                OCCURRENCE - 1
            )),
            "{kind}: nothing is due before the occurrence"
        );
        for now in [OCCURRENCE, OCCURRENCE + HOUR_MS, OCCURRENCE + 6 * DAY_MS] {
            assert!(
                selected(&evaluate_at(&catalog, Some(earlier.clone()), now)),
                "{kind}: owed at {now}"
            );
        }
        for terminal in [TaskTerminalState::Succeeded, TaskTerminalState::Failed] {
            let served = evaluate_at(
                &catalog,
                Some(ran(ANSWERED, terminal, false)),
                OCCURRENCE + 10 * MINUTE_MS,
            );
            assert!(!selected(&served), "{kind}, {terminal:?}: {served:#?}");
            assert_eq!(
                last_reason(decision(&served, FIRST, INSTANCE_A)),
                TRIGGER_FALSE,
                "{kind}, {terminal:?}"
            );
        }
        let left = ran(ANSWERED, TaskTerminalState::Failed, true);
        assert!(
            selected(&evaluate_at(
                &catalog,
                Some(left),
                OCCURRENCE + 10 * MINUTE_MS
            )),
            "{kind}: left owed"
        );
        let rerun = ran(
            OCCURRENCE + 30 * MINUTE_MS,
            TaskTerminalState::Succeeded,
            false,
        );
        for (now, due) in [
            (OCCURRENCE + 40 * MINUTE_MS, false),
            (next - 1, false),
            (next, true),
        ] {
            assert_eq!(
                selected(&evaluate_at(&catalog, Some(rerun.clone()), now)),
                due,
                "{kind} at {now}"
            );
        }
    }
}

// t4: once the occurrence is served the pair's wake and the round's are the next occurrence, the
// next month's (the weekly clock's next week's); `clock_slot_view` says the same. With `missed`
// of two hours (V2) a due occurrence wakes at its grace's end, and from then the clock is false
// with `clock_occurrence_expired`, waking at the next occurrence; an occurrence CK-2 left owed is
// due only inside the grace.
#[test]
fn ck1_t4_the_next_wake_is_the_next_months_occurrence_and_a_grace_bounds_it() {
    for (kind, schedule, next) in kinds() {
        let catalog = catalog_with(
            SCHEDULING_SCHEMA_VERSION,
            clock(schedule.clone()),
            Vec::new(),
        );
        let answered = ran(ANSWERED, TaskTerminalState::Succeeded, false);
        let now = OCCURRENCE + 10 * MINUTE_MS;
        let served = evaluate_at(&catalog, Some(answered.clone()), now);
        let pair = decision(&served, FIRST, INSTANCE_A);
        assert_eq!(last_reason(pair), TRIGGER_FALSE, "{kind}");
        assert_eq!(pair.next_wake_unix_ms, Some(next), "{kind}");
        assert_eq!(served.next_wake_unix_ms, Some(next), "{kind}");
        assert_eq!(
            clock_slot_view(&catalog.catalog().tasks.tasks[0], Some(&answered), at(now))
                .unwrap_or_else(|error| panic!("{kind}: the clock reads: {error}")),
            Some(ClockSlotView {
                due_occurrence: None,
                next_occurrence: Some(next),
            }),
            "{kind}"
        );

        let missed = catalog_with(
            SCHEDULING_SCHEMA_VERSION_V2,
            with_grace(clock(schedule), GRACE_MS),
            Vec::new(),
        );
        let earlier = ran(EARLIER, TaskTerminalState::Succeeded, false);
        let due = evaluate_at(&missed, Some(earlier.clone()), OCCURRENCE + MINUTE_MS);
        assert!(selected(&due), "{kind}: {due:#?}");
        assert_eq!(
            decision(&due, FIRST, INSTANCE_A).next_wake_unix_ms,
            Some(OCCURRENCE + GRACE_MS),
            "{kind}"
        );
        let expired = evaluate_at(&missed, Some(earlier), OCCURRENCE + GRACE_MS);
        assert!(!selected(&expired), "{kind}: {expired:#?}");
        let lapsed = decision(&expired, FIRST, INSTANCE_A);
        assert_eq!(last_reason(lapsed), TRIGGER_FALSE, "{kind}");
        assert!(
            lapsed
                .reasons
                .iter()
                .any(|reason| reason.outcome.is(PolicyCode::ClockOccurrenceExpired)),
            "{kind}: {lapsed:#?}"
        );
        assert_eq!(lapsed.next_wake_unix_ms, Some(next), "{kind}");

        let left = ran(ANSWERED, TaskTerminalState::Failed, true);
        assert!(
            selected(&evaluate_at(
                &missed,
                Some(left.clone()),
                OCCURRENCE + GRACE_MS - MINUTE_MS
            )),
            "{kind}: owed inside its grace"
        );
        assert!(
            !selected(&evaluate_at(&missed, Some(left), OCCURRENCE + GRACE_MS)),
            "{kind}: not after it"
        );
    }
}

// t5: a `schedule.json` whose rule's clock (here under an `any`, beside a weekly clock) or whose
// timeline event's schedule is monthly names `clock_monthly` in `requires`. Without it the file is
// refused `schedule_invalid` at the first such `schedule`, rules before events, by the composer
// with no manifest as by a Runtime's; a weekly clock alone needs nothing, as before. With it the
// file composes and the composed task and event hold the monthly clock. A Runtime whose manifest
// does not name the subsystem (one of 0.12.82) refuses the file at its `requires` entry
// (`schedule_requires_missing`); one that names it composes it.
#[test]
fn ck1_t5_a_monthly_clock_needs_clock_monthly_in_requires() {
    let either = || {
        json!({
            "kind": "any",
            "predicates": [clock(weekly(utc(), 2, 420)), clock(monthly(utc(), 1, 420))]
        })
    };
    assert_eq!(
        refused(compose_file(&schedule_file(either(), None, &[]), None)),
        (
            StandardPackProblem::ScheduleInvalid,
            "/rules/0/trigger/predicates/1/schedule".to_owned()
        )
    );
    assert_eq!(
        refused(compose_file(
            &schedule_file(
                clock(weekly(utc(), 2, 420)),
                Some(monthly(utc(), 1, 420)),
                &[]
            ),
            None
        )),
        (
            StandardPackProblem::ScheduleInvalid,
            "/timeline/0/schedule".to_owned()
        )
    );
    let weekly_only = compose_file(
        &schedule_file(
            clock(weekly(utc(), 2, 420)),
            Some(weekly(utc(), 2, 420)),
            &[],
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("a weekly clock requires nothing: {failure}"));
    assert!(weekly_only.games()[0].requires.is_empty());

    let named = schedule_file(
        either(),
        Some(monthly(utc(), 1, 420)),
        &[CLOCK_MONTHLY_SUBSYSTEM],
    );
    let composed = compose_file(&named, None)
        .unwrap_or_else(|failure| panic!("the file names the subsystem: {failure}"));
    assert_eq!(composed.games()[0].requires, [CLOCK_MONTHLY_SUBSYSTEM]);
    let catalog = composed.compiled().catalog();
    let PredicateSpec::Any { predicates } = &catalog.tasks.tasks[0].trigger else {
        panic!("the composed trigger: {:?}", catalog.tasks.tasks[0].trigger);
    };
    assert!(
        matches!(
            &predicates[1],
            PredicateSpec::Clock {
                schedule: ClockSchedule::Monthly {
                    day_of_month: 1,
                    minute_of_day: 420,
                    ..
                },
                ..
            }
        ),
        "{predicates:?}"
    );
    assert!(
        matches!(
            catalog.timeline.events[0].schedule,
            ClockSchedule::Monthly {
                day_of_month: 1,
                minute_of_day: 420,
                ..
            }
        ),
        "{:?}",
        catalog.timeline.events
    );

    let older = BTreeSet::from(["standard_pack_schedule".to_owned()]);
    assert_eq!(
        refused(compose_file(&named, Some(&older))),
        (
            StandardPackProblem::ScheduleRequiresMissing,
            "/requires/0".to_owned()
        )
    );
    let this_build = BTreeSet::from([
        "standard_pack_schedule".to_owned(),
        CLOCK_MONTHLY_SUBSYSTEM.to_owned(),
    ]);
    let accepted = compose_file(&named, Some(&this_build))
        .unwrap_or_else(|failure| panic!("a Runtime that names it composes it: {failure}"));
    assert_eq!(accepted.games()[0].requires, [CLOCK_MONTHLY_SUBSYSTEM]);
}

// t6: a V2 timeline event whose schedule is monthly (nine hours ahead of UTC, day 1 at 04:00 local,
// two days long): closed before its occurrence and waking at it; open from it for its two days,
// waking at their end; closed after them until the next month's occurrence. A task whose trigger
// is the event's window is dispatched inside it and not before it, the round waking at the
// occurrence.
#[test]
fn ck1_t6_a_timeline_event_with_a_monthly_schedule() {
    let event = json!({
        "id": EVENT,
        "scope": {"kind": "game", "game_id": GAME},
        "event_kind": "activity",
        "schedule": monthly(server(540), 1, 240),
        "duration_ms": 2 * DAY_MS,
        "invalidates_fact_prefixes": [],
        "validity": {"from_unix_ms": 0, "until_unix_ms": null}
    });
    let catalog = catalog_with(
        SCHEDULING_SCHEMA_VERSION_V2,
        json!({"kind": "timeline_active", "event_id": EVENT}),
        vec![event],
    );
    let context = TimelineQueryContext {
        instance_id: INSTANCE_A.to_owned(),
        server_id: SERVER.to_owned(),
        game_id: GAME.to_owned(),
    };
    let availability = |now: u64| {
        inspect_timeline(&catalog, at(now), &context, &[EVENT.to_owned()])
            .unwrap_or_else(|error| panic!("at {now}: the timeline reads: {error}"))
            .events[0]
            .availability
            .clone()
    };
    let window_end = DEC_01_0400_AHEAD + 2 * DAY_MS;
    assert_eq!(
        availability(DEC_01_0400_AHEAD - 1),
        TimelineAvailability {
            state: EligibilityState::False,
            active_interval: None,
            next_wake_unix_ms: Some(DEC_01_0400_AHEAD),
        }
    );
    assert_eq!(
        availability(DEC_01_0400_AHEAD),
        TimelineAvailability {
            state: EligibilityState::True,
            active_interval: Some((DEC_01_0400_AHEAD, window_end)),
            next_wake_unix_ms: Some(window_end),
        }
    );
    assert_eq!(
        availability(window_end),
        TimelineAvailability {
            state: EligibilityState::False,
            active_interval: None,
            next_wake_unix_ms: Some(JAN_01_0400_AHEAD),
        }
    );

    let before = evaluate_at(&catalog, None, DEC_01_0400_AHEAD - 1);
    assert!(!selected(&before), "{before:#?}");
    assert_eq!(before.next_wake_unix_ms, Some(DEC_01_0400_AHEAD));
    assert!(selected(&evaluate_at(
        &catalog,
        None,
        DEC_01_0400_AHEAD + HOUR_MS
    )));
}

// t7: a catalogue without the kind is unchanged: the neutral example catalog keeps its V1 and V2
// golden identities, and its documents name no monthly clock.
#[test]
fn ck1_t7_a_catalogue_without_the_kind_keeps_its_golden_hashes() {
    for (version, golden) in [
        (SCHEDULING_SCHEMA_VERSION, V1_EXAMPLE_HASH),
        (SCHEDULING_SCHEMA_VERSION_V2, V2_EXAMPLE_HASH),
    ] {
        let example = compile_catalog(&example_sources(version))
            .unwrap_or_else(|failure| panic!("{version}: {:?}", failure.diagnostics()));
        assert_eq!(example.catalog_hash(), golden, "{version}");
        // The bundle is no serializable type; its four documents are, one by one.
        let bundle = example.catalog();
        let text = serde_json::to_string(&(
            &bundle.tasks,
            &bundle.pools,
            &bundle.activity,
            &bundle.timeline,
        ))
        .expect("the documents serialize");
        assert!(!text.contains("monthly"), "{version}: {text}");
    }
}

// ---- Fixtures

fn utc() -> Value {
    server(0)
}

// A server clock source `offset_minutes` ahead of UTC, without daylight saving or drift.
fn server(offset_minutes: i16) -> Value {
    json!({
        "kind": "server",
        "timezone_id": "fixed/offset",
        "utc_offset_minutes": offset_minutes,
        "dst_offset_minutes": 0,
        "maintenance_drift_ms": 0
    })
}

// A revealed clock source 480 minutes ahead of UTC with 60 of daylight saving and 120 000 ms of
// maintenance drift.
fn revealed() -> Value {
    json!({
        "kind": "reveal",
        "reveal_source": "fixture.reveal-a",
        "timezone_id": "fixed/plus-eight",
        "utc_offset_minutes": 480,
        "dst_offset_minutes": 60,
        "maintenance_drift_ms": 120_000
    })
}

fn monthly(source: Value, day_of_month: u8, minute_of_day: u16) -> Value {
    json!({
        "kind": "monthly",
        "clock_source": source,
        "day_of_month": day_of_month,
        "minute_of_day": minute_of_day
    })
}

fn weekly(source: Value, weekday: u8, minute_of_day: u16) -> Value {
    json!({
        "kind": "weekly",
        "clock_source": source,
        "weekday": weekday,
        "minute_of_day": minute_of_day
    })
}

fn clock(schedule: Value) -> Value {
    json!({"kind": "clock", "schedule": schedule})
}

// `predicate`, a clock, with `missed: skip_after` of `grace_ms`.
fn with_grace(mut predicate: Value, grace_ms: u64) -> Value {
    predicate["missed"] = json!({"kind": "skip_after", "grace_ms": grace_ms});
    predicate
}

// The two kinds t3 and t4 compare, each with its next occurrence after [`OCCURRENCE`], which
// both share: the monthly clock on day 1 and the weekly clock on Tuesday, at 07:00 UTC.
fn kinds() -> [(&'static str, Value, u64); 2] {
    [
        ("monthly", monthly(utc(), 1, 420), NEXT_MONTH),
        ("weekly", weekly(utc(), 2, 420), OCCURRENCE + WEEK_MS),
    ]
}

// The diagnostic an out-of-range clock gets: `predicate_uncomputable` at the clock.
fn outside_bounds() -> (CatalogDiagnosticCode, String, String) {
    (
        CatalogDiagnosticCode::PredicateUncomputable,
        "/tasks/0/trigger".to_owned(),
        "clock schedule is outside the frozen V1 bounds".to_owned(),
    )
}

// One activity profile of the game, open all day on every weekday (as P-2a's).
fn activity_profile() -> Value {
    json!({
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
    })
}

// The documents of a catalog of `version` with P-2a's one task `FIRST` triggered by `trigger`
// (four runs a day), the activity profile and `events`.
fn catalog_sources(version: &str, trigger: Value, events: Vec<Value>) -> CatalogSources {
    let header = json!({
        "catalog_id": "fixture.ck1",
        "catalog_version": 1,
        "approval_refs": ["approval:fixture-ck1"]
    });
    let document = |member: &str, entries: Value| {
        let mut document = json!({"schema_version": version, "catalog": header});
        document[member] = entries;
        document
    };
    CatalogSources {
        tasks: document_source(
            "tasks",
            &document("tasks", json!([task(FIRST, trigger, 4, json!([]))])),
        ),
        pools: document_source("pools", &document("pools", json!([]))),
        activity: document_source(
            "activity",
            &document("profiles", json!([activity_profile()])),
        ),
        timeline: document_source("timeline", &document("events", Value::Array(events))),
        selection: None,
    }
}

fn document_source(name: &str, document: &Value) -> CatalogDocumentSource {
    CatalogDocumentSource::new(
        format!("memory://fixture-ck1/{name}.json"),
        serde_json::to_vec(document).expect("fixture document bytes"),
    )
}

fn catalog_with(version: &str, trigger: Value, events: Vec<Value>) -> CompiledCatalog {
    compile_catalog(&catalog_sources(version, trigger, events)).unwrap_or_else(|failure| {
        panic!(
            "{version}: the fixture catalog compiles: {:?}",
            failure.diagnostics()
        )
    })
}

// What the compiler refuses of a V2 catalog whose task's trigger is `trigger`: each diagnostic's
// code, pointer and text.
fn refusals(trigger: Value) -> Vec<(CatalogDiagnosticCode, String, String)> {
    compile_catalog(&catalog_sources(
        SCHEDULING_SCHEMA_VERSION_V2,
        trigger,
        Vec::new(),
    ))
    .expect_err("the clock is refused")
    .diagnostics()
    .iter()
    .map(|diagnostic| {
        (
            diagnostic.code,
            diagnostic.json_path.clone(),
            diagnostic.reason.clone(),
        )
    })
    .collect()
}

// The latest occurrence of `schedule` at or before `now` and its next one, as `clock_slot_view`
// reads them for a pair never dispatched, of a task whose trigger is that clock alone.
fn occurrences(schedule: Value, now: u64) -> (Option<u64>, Option<u64>) {
    let catalog = catalog_with(SCHEDULING_SCHEMA_VERSION_V2, clock(schedule), Vec::new());
    let view = clock_slot_view(&catalog.catalog().tasks.tasks[0], None, at(now))
        .unwrap_or_else(|error| panic!("at {now}: the clock reads: {error}"))
        .unwrap_or_else(|| panic!("at {now}: the clock counts"));
    (view.due_occurrence, view.next_occurrence)
}

// The pair's snapshot after its latest dispatch at `dispatched`, which ended as `terminal`, with
// `clock_owed` as the Runtime projects it (as CK-2's gate writes it).
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

// A `schedule.json` rule running [`PACKAGE`]: P-2a's task without scope, entrypoint, procedure
// and instance overrides, with `package_id`, triggered by `trigger`.
fn schedule_rule(trigger: Value) -> Value {
    let mut rule = task(PACKAGE, trigger, 4, json!([]));
    let members = rule.as_object_mut().expect("a task object");
    for member in ["scope", "entrypoint", "procedure_ref", "instance_overrides"] {
        members.shift_remove(member);
    }
    members.insert("package_id".to_owned(), json!(PACKAGE));
    rule
}

// The game root's `schedule.json`: the one rule triggered by `trigger`, a day-long activity event
// on `event_schedule` when one is given, and `requires`.
fn schedule_file(trigger: Value, event_schedule: Option<Value>, requires: &[&str]) -> Value {
    let timeline = event_schedule.map_or_else(Vec::new, |schedule| {
        vec![json!({
            "id": EVENT,
            "event_kind": "activity",
            "schedule": schedule,
            "duration_ms": DAY_MS,
            "invalidates_fact_prefixes": [],
            "validity": {"from_unix_ms": 0, "until_unix_ms": null}
        })]
    });
    json!({
        "schema_version": "actingcommand.schedule.v1",
        "requires": requires,
        "rules": [schedule_rule(trigger)],
        "timeline": timeline
    })
}

// Composes `file` in the fixture game root (its bundle lists the one package) for the one bound
// instance; with `subsystems`, a Runtime manifest checks `requires`.
fn compose_file(
    file: &Value,
    subsystems: Option<&BTreeSet<String>>,
) -> Result<StandardPackComposition, StandardPackComposeFailure> {
    let bundle = serde_json::to_vec(&json!({
        "schema_version": "actingcommand.bundle.v4",
        "game": GAME,
        "packs": [{
            "package_id": PACKAGE,
            "server": SERVER,
            "entry_task_id": "task",
            "path": format!("packs/{PACKAGE}"),
            "file_count": 1,
            "byte_count": 64
        }]
    }))
    .expect("bundle bytes");
    let root = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle.as_slice()),
            ..StandardPackRootBytes::default()
        },
        GAME,
    )
    .unwrap_or_else(|issue| panic!("the root reads: {issue}"))
    .expect("the root holds bundle.json");
    let bytes = serde_json::to_vec(file).expect("schedule bytes");
    // Workflow #455 G-R2s: every package of the root reads, none irreversible.
    let irreversible = super::readable_root(&root);
    let games = BTreeMap::from([(
        GAME.to_owned(),
        StandardPackGameFiles {
            schedule: &bytes,
            root: &root,
            maintenance: &[],
            irreversible: &irreversible,
            catalog: None,
        },
    )]);
    let instances = BTreeMap::from([(
        INSTANCE_A.to_owned(),
        StandardPackBinding {
            game_id: GAME.to_owned(),
            server_id: SERVER.to_owned(),
        },
    )]);
    compose_standard_pack_rules(&StandardPackComposeInput {
        instances: &instances,
        games: &games,
        subsystems,
        catalog_version: 1,
    })
}

// The one refusal of a composition that must fail: its problem and pointer in the game's file.
fn refused(
    composed: Result<StandardPackComposition, StandardPackComposeFailure>,
) -> (StandardPackProblem, String) {
    let failure = match composed {
        Ok(_) => panic!("the schedule is refused"),
        Err(failure) => failure,
    };
    assert_eq!(failure.issues().len(), 1, "one refusal: {failure}");
    let issue = &failure.issues()[0];
    assert_eq!(issue.game_id.as_deref(), Some(GAME));
    assert_eq!(issue.issue.file, STANDARD_PACK_SCHEDULE_FILE);
    (issue.issue.problem, issue.issue.pointer.clone())
}

// The neutral example catalog `catalog-a` declared as `version` (V2: every timeline event valid
// from 0, as its V2 golden was computed), as `gate_ord1` reads it.
fn example_sources(version: &str) -> CatalogSources {
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
    let [tasks, pools, activity, timeline] = documents;
    CatalogSources {
        tasks: document_source("example-tasks", &tasks),
        pools: document_source("example-pools", &pools),
        activity: document_source("example-activity", &activity),
        timeline: document_source("example-timeline", &timeline),
        selection: None,
    }
}
