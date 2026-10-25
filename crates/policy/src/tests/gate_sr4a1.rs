// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-4a1 gate tests (SCH-M frozen model v1.0 §3.2 D-1, §6 row SR-4a1): the pure
// blackout judgement over its three sources. A neutral host: instances `instance-a` (game
// `game-a`) and `instance-b` (game `game-b`), clocks at UTC, the week of Monday 2026-10-19. Every
// expected value comes from the model's D-1:
// - a `weekly` entry starts on its weekdays at its start minute and an end at or before the start
//   crosses midnight; a `once` entry is `[from_unix_ms, until_unix_ms)`; a catalog `maintenance`
//   event covers `[occurrence, occurrence + duration_ms)` within its validity, for the instances
//   its scope matches;
// - the covering blackout is the maximal union of the overlapping or adjacent intervals of the
//   three sources, its end the union's end; its id and source are those of the entry that covers
//   the time and ends last;
// - a host entry covers every instance, an instance entry only its own instance.

use std::collections::BTreeMap;

use actingcommand_contract::outcome::BlackoutSource;

use crate::{
    Blackout, BlackoutEntry, BlackoutSources, BlackoutWindow, ClockSchedule, ClockSource,
    EvaluationTime, InstanceSnapshot, RequiredNullable, ScopeSelector, TimelineEvent,
    TimelineEventKind, TimelineValidity, blackout_at, next_blackout_start,
};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 86_400_000;
// 2026-10-19T00:00:00Z, a Monday (weekday 1).
const MONDAY: u64 = 1_792_368_000_000;
const TUESDAY: u64 = MONDAY + DAY_MS;
const WEDNESDAY: u64 = MONDAY + 2 * DAY_MS;
const THURSDAY: u64 = MONDAY + 3 * DAY_MS;
const INSTANCE_A: &str = "instance-a";
const INSTANCE_B: &str = "instance-b";

fn instance(instance_id: &str, game_id: &str) -> InstanceSnapshot {
    InstanceSnapshot {
        instance_id: instance_id.to_owned(),
        server_id: format!("server-{game_id}"),
        game_id: game_id.to_owned(),
        host_id: "host-a".to_owned(),
        available: true,
        capability_operation_ids: Vec::new(),
        preferred_task_ids: Vec::new(),
    }
}

fn instance_a() -> InstanceSnapshot {
    instance(INSTANCE_A, "game-a")
}

fn instance_b() -> InstanceSnapshot {
    instance(INSTANCE_B, "game-b")
}

fn at(unix_ms: u64) -> EvaluationTime {
    EvaluationTime {
        unix_ms,
        monotonic_ms: 1_000_000,
    }
}

fn weekly(id: &str, weekdays: &[u8], start_minute: u16, end_minute: u16) -> BlackoutEntry {
    BlackoutEntry {
        id: id.to_owned(),
        window: BlackoutWindow::Weekly {
            weekdays: weekdays.to_vec(),
            utc_offset_minutes: 0,
            start_minute_of_day: start_minute,
            end_minute_of_day: end_minute,
        },
    }
}

fn once(id: &str, from_unix_ms: u64, until_unix_ms: u64) -> BlackoutEntry {
    BlackoutEntry {
        id: id.to_owned(),
        window: BlackoutWindow::Once {
            from_unix_ms,
            until_unix_ms,
        },
    }
}

fn utc() -> ClockSource {
    ClockSource::Server {
        timezone_id: "fixed/utc".to_owned(),
        utc_offset_minutes: 0,
        dst_offset_minutes: 0,
        maintenance_drift_ms: 0,
    }
}

fn maintenance(
    id: &str,
    scope: ScopeSelector,
    schedule: ClockSchedule,
    duration_ms: u64,
) -> TimelineEvent {
    TimelineEvent {
        id: id.to_owned(),
        scope,
        event_kind: TimelineEventKind::Maintenance,
        schedule,
        duration_ms,
        invalidates_fact_prefixes: Vec::new(),
        validity: Some(TimelineValidity {
            from_unix_ms: 0,
            until_unix_ms: RequiredNullable(None),
        }),
    }
}

fn game_a() -> ScopeSelector {
    ScopeSelector::Game {
        game_id: "game-a".to_owned(),
    }
}

fn own(instance_id: &str, entries: Vec<BlackoutEntry>) -> BTreeMap<String, Vec<BlackoutEntry>> {
    BTreeMap::from([(instance_id.to_owned(), entries)])
}

fn blackout(id: &str, source: BlackoutSource, start: u64, end: u64) -> Option<Blackout> {
    Some(Blackout {
        id: id.to_owned(),
        source,
        start_unix_ms: start,
        end_unix_ms: end,
    })
}

fn covering(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    unix_ms: u64,
) -> Option<Blackout> {
    blackout_at(sources, instance, at(unix_ms)).expect("the judgement reads its sources")
}

fn next(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    unix_ms: u64,
    bound: u64,
) -> Option<Blackout> {
    next_blackout_start(sources, instance, at(unix_ms), bound)
        .expect("the search reads its sources")
}

// D-1: each kind of interval covers on its own, half-open: a weekly host entry that crosses
// midnight, a once entry of the instance, a catalog maintenance event of the instance's game.
#[test]
fn sr4a1_t1_weekly_once_and_catalog_intervals_each_cover() {
    let a = instance_a();
    let empty = BTreeMap::new();

    // A weekly host entry: Mondays 23:00 to 01:00 the next day.
    let host = [weekly("night", &[1], 23 * 60, 60)];
    let sources = BlackoutSources {
        host: &host,
        instances: &empty,
        catalog: &[],
    };
    let night = blackout(
        "night",
        BlackoutSource::Host,
        MONDAY + 23 * HOUR_MS,
        TUESDAY + HOUR_MS,
    );
    assert_eq!(covering(&sources, &a, MONDAY + 23 * HOUR_MS), night);
    assert_eq!(covering(&sources, &a, TUESDAY + 30 * MINUTE_MS), night);
    assert_eq!(covering(&sources, &a, MONDAY + 23 * HOUR_MS - 1), None);
    assert_eq!(covering(&sources, &a, TUESDAY + HOUR_MS), None);
    // It starts on Mondays only.
    assert_eq!(
        covering(&sources, &a, TUESDAY + 23 * HOUR_MS + 30 * MINUTE_MS),
        None
    );
    assert_eq!(
        next(&sources, &a, MONDAY + 12 * HOUR_MS, MONDAY + 23 * HOUR_MS),
        night
    );
    assert_eq!(
        next(
            &sources,
            &a,
            MONDAY + 12 * HOUR_MS,
            MONDAY + 23 * HOUR_MS - 1
        ),
        None
    );

    // A once entry of instance A: Wednesday 10:00 to 12:00.
    let instances = own(
        INSTANCE_A,
        vec![once(
            "absence",
            WEDNESDAY + 10 * HOUR_MS,
            WEDNESDAY + 12 * HOUR_MS,
        )],
    );
    let sources = BlackoutSources {
        host: &[],
        instances: &instances,
        catalog: &[],
    };
    let absence = blackout(
        "absence",
        BlackoutSource::Instance,
        WEDNESDAY + 10 * HOUR_MS,
        WEDNESDAY + 12 * HOUR_MS,
    );
    assert_eq!(covering(&sources, &a, WEDNESDAY + 10 * HOUR_MS), absence);
    assert_eq!(covering(&sources, &a, WEDNESDAY + 11 * HOUR_MS), absence);
    assert_eq!(covering(&sources, &a, WEDNESDAY + 10 * HOUR_MS - 1), None);
    assert_eq!(covering(&sources, &a, WEDNESDAY + 12 * HOUR_MS), None);
    assert_eq!(next(&sources, &a, WEDNESDAY, THURSDAY), absence);

    // A catalog maintenance event of game A: Thursdays 02:00, three hours.
    let catalog = [maintenance(
        "upkeep",
        game_a(),
        ClockSchedule::Weekly {
            clock_source: utc(),
            weekday: 4,
            minute_of_day: 2 * 60,
        },
        3 * HOUR_MS,
    )];
    let sources = BlackoutSources {
        host: &[],
        instances: &empty,
        catalog: &catalog,
    };
    let upkeep = blackout(
        "upkeep",
        BlackoutSource::Catalog,
        THURSDAY + 2 * HOUR_MS,
        THURSDAY + 5 * HOUR_MS,
    );
    assert_eq!(covering(&sources, &a, THURSDAY + 2 * HOUR_MS), upkeep);
    assert_eq!(covering(&sources, &a, THURSDAY + 3 * HOUR_MS), upkeep);
    assert_eq!(covering(&sources, &a, THURSDAY + 2 * HOUR_MS - 1), None);
    assert_eq!(covering(&sources, &a, THURSDAY + 5 * HOUR_MS), None);
    assert_eq!(
        next(&sources, &a, WEDNESDAY, THURSDAY + 2 * HOUR_MS),
        upkeep
    );
}

// D-1: overlapping and adjacent intervals of the three sources form one blackout whose end is
// the union's end, so the wake never lands inside the next interval; the reported entry is the
// one that covers the time and ends last.
#[test]
fn sr4a1_t2_overlapping_and_adjacent_intervals_wake_at_the_union_end() {
    let a = instance_a();
    // Host: Mondays 20:00 to 22:00. Instance A: Monday 21:00 to 23:00 (overlaps). Catalog: one
    // hour from Monday 23:00 (adjacent to the instance entry's end).
    let host = [weekly("evening", &[1], 20 * 60, 22 * 60)];
    let instances = own(
        INSTANCE_A,
        vec![once("late", MONDAY + 21 * HOUR_MS, MONDAY + 23 * HOUR_MS)],
    );
    let catalog = [maintenance(
        "after",
        game_a(),
        ClockSchedule::At {
            clock_source: utc(),
            at_ms: MONDAY + 23 * HOUR_MS,
        },
        HOUR_MS,
    )];
    let sources = BlackoutSources {
        host: &host,
        instances: &instances,
        catalog: &catalog,
    };
    let start = MONDAY + 20 * HOUR_MS;
    let end = TUESDAY;

    assert_eq!(
        covering(&sources, &a, MONDAY + 20 * HOUR_MS + 30 * MINUTE_MS),
        blackout("evening", BlackoutSource::Host, start, end)
    );
    // Two entries cover 21:30; the instance entry ends last.
    assert_eq!(
        covering(&sources, &a, MONDAY + 21 * HOUR_MS + 30 * MINUTE_MS),
        blackout("late", BlackoutSource::Instance, start, end)
    );
    assert_eq!(
        covering(&sources, &a, MONDAY + 23 * HOUR_MS),
        blackout("after", BlackoutSource::Catalog, start, end)
    );
    assert_eq!(covering(&sources, &a, end), None);
    // The next blackout seen from before it starts at the host entry and ends at the union's
    // end.
    assert_eq!(
        next(&sources, &a, MONDAY + 19 * HOUR_MS, MONDAY + 20 * HOUR_MS),
        blackout("evening", BlackoutSource::Host, start, end)
    );
}

// D-1: the host list, the instance's own list and the catalog's maintenance events all cover one
// instance, each reported with its own source; the next blackout steps through them in order.
#[test]
fn sr4a1_t3_host_instance_and_catalog_entries_cover_one_instance() {
    let a = instance_a();
    let host = [weekly("restart", &[2], 4 * 60, 5 * 60)];
    let instances = own(
        INSTANCE_A,
        vec![once(
            "quiet",
            TUESDAY + 10 * HOUR_MS,
            TUESDAY + 11 * HOUR_MS,
        )],
    );
    let catalog = [maintenance(
        "upkeep",
        game_a(),
        ClockSchedule::At {
            clock_source: utc(),
            at_ms: TUESDAY + 15 * HOUR_MS,
        },
        HOUR_MS,
    )];
    let sources = BlackoutSources {
        host: &host,
        instances: &instances,
        catalog: &catalog,
    };
    let restart = blackout(
        "restart",
        BlackoutSource::Host,
        TUESDAY + 4 * HOUR_MS,
        TUESDAY + 5 * HOUR_MS,
    );
    let quiet = blackout(
        "quiet",
        BlackoutSource::Instance,
        TUESDAY + 10 * HOUR_MS,
        TUESDAY + 11 * HOUR_MS,
    );
    let upkeep = blackout(
        "upkeep",
        BlackoutSource::Catalog,
        TUESDAY + 15 * HOUR_MS,
        TUESDAY + 16 * HOUR_MS,
    );
    let half = 30 * MINUTE_MS;
    assert_eq!(
        covering(&sources, &a, TUESDAY + 4 * HOUR_MS + half),
        restart
    );
    assert_eq!(covering(&sources, &a, TUESDAY + 10 * HOUR_MS + half), quiet);
    assert_eq!(
        covering(&sources, &a, TUESDAY + 15 * HOUR_MS + half),
        upkeep
    );
    assert_eq!(covering(&sources, &a, TUESDAY + 12 * HOUR_MS), None);

    let bound = WEDNESDAY - 1;
    assert_eq!(next(&sources, &a, TUESDAY, bound), restart);
    assert_eq!(next(&sources, &a, TUESDAY + 5 * HOUR_MS, bound), quiet);
    assert_eq!(next(&sources, &a, TUESDAY + 11 * HOUR_MS, bound), upkeep);
    assert_eq!(next(&sources, &a, TUESDAY + 16 * HOUR_MS, bound), None);
}

// D-1: an instance's own entry covers that instance only, and a catalog event covers only the
// instances its scope matches; a host entry covers every instance.
#[test]
fn sr4a1_t4_an_instance_entry_covers_only_its_instance() {
    let (a, b) = (instance_a(), instance_b());
    let host = [weekly("restart", &[2], 4 * 60, 5 * 60)];
    let instances = own(
        INSTANCE_A,
        vec![once(
            "quiet",
            TUESDAY + 10 * HOUR_MS,
            TUESDAY + 11 * HOUR_MS,
        )],
    );
    let catalog = [maintenance(
        "upkeep",
        game_a(),
        ClockSchedule::At {
            clock_source: utc(),
            at_ms: TUESDAY + 15 * HOUR_MS,
        },
        HOUR_MS,
    )];
    let sources = BlackoutSources {
        host: &host,
        instances: &instances,
        catalog: &catalog,
    };
    let half = 30 * MINUTE_MS;

    let restart = blackout(
        "restart",
        BlackoutSource::Host,
        TUESDAY + 4 * HOUR_MS,
        TUESDAY + 5 * HOUR_MS,
    );
    assert_eq!(
        covering(&sources, &b, TUESDAY + 4 * HOUR_MS + half),
        restart
    );
    assert_eq!(covering(&sources, &b, TUESDAY + 10 * HOUR_MS + half), None);
    assert_eq!(covering(&sources, &b, TUESDAY + 15 * HOUR_MS + half), None);
    assert_eq!(
        next(&sources, &b, TUESDAY + 5 * HOUR_MS, WEDNESDAY - 1),
        None
    );

    // Instance A, under the same sources, is covered by its own entry.
    assert_eq!(
        covering(&sources, &a, TUESDAY + 10 * HOUR_MS + half),
        blackout(
            "quiet",
            BlackoutSource::Instance,
            TUESDAY + 10 * HOUR_MS,
            TUESDAY + 11 * HOUR_MS,
        )
    );
}
