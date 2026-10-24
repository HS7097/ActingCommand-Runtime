// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a1 (SCH-M frozen model v1.0 §3.2, D-1 and D-6): the pure blackout and
//! day-boundary judgements, beside `activity_window_at`.
//!
//! - A blackout comes from exactly three sources: (a) the instance's own list
//!   (`instances[].scheduling.blackout`), (b) the machine-wide host list
//!   (`scheduling.blackout.host`) and (c) the active catalog's V2 timeline `maintenance` events
//!   whose scope matches the instance. Lists (a) and (b) hold `weekly` and `once` entries.
//! - The covering blackout at a time is the maximal union of the overlapping or adjacent intervals
//!   of all three sources that contains it; its end is the union's end, so a wake at that end never
//!   lands inside the next blackout. The reported id and source belong to the entry that covers
//!   the time and ends last.
//! - The day-boundary fit admits a run only if it ends, by its expected duration, before the end of
//!   the activity window it is admitted in.
//!
//! Nothing here knows a game, a time of day or a rule: every interval comes from the data the
//! caller passes in. Nothing here reads a clock.

use std::collections::BTreeMap;

use actingcommand_contract::outcome::BlackoutSource;

use super::{
    EvaluationTime, InstanceSnapshot, PolicyEvaluationError, PolicyEvaluationResult,
    activity_window_at, schedule_occurrences, scope_matches_instance,
};
use crate::{ActivityProfile, TimelineEvent, TimelineEventKind};

const DAY_MS: i128 = 86_400_000;
const MINUTE_MS: i128 = 60_000;
/// The most intervals one union is followed through, forward or backward. A computation bound,
/// not a threshold: a longer chain (only a blackout table that covers whole weeks without a gap
/// builds one) reports the end it reached, and the evaluation at that wake finds the rest.
const MAX_UNION_STEPS: usize = 1_024;
/// The local days a weekly entry's next start is searched over: one week and the day it began.
const WEEKLY_SEARCH_DAYS: i128 = 8;

/// When one blackout entry of a configuration list covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlackoutWindow {
    /// Every week: it starts on each of `weekdays` (1 Monday to 7 Sunday) at
    /// `start_minute_of_day` of the clock `utc_offset_minutes` from UTC and ends at
    /// `end_minute_of_day`; an end at or before the start crosses midnight.
    Weekly {
        weekdays: Vec<u8>,
        utc_offset_minutes: i16,
        start_minute_of_day: u16,
        end_minute_of_day: u16,
    },
    /// Once: `[from_unix_ms, until_unix_ms)`.
    Once {
        from_unix_ms: u64,
        until_unix_ms: u64,
    },
}

/// One blackout entry of a configuration list, as the configuration in effect holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackoutEntry {
    pub id: String,
    pub window: BlackoutWindow,
}

/// The three sources of blackouts (D-1).
#[derive(Debug, Clone, Copy)]
pub struct BlackoutSources<'a> {
    /// (b) The host entries: they cover every instance.
    pub host: &'a [BlackoutEntry],
    /// (a) Each instance's own entries, by the instance id the evaluation names it by
    /// (`InstanceSnapshot::instance_id`): they cover that instance only.
    pub instances: &'a BTreeMap<String, Vec<BlackoutEntry>>,
    /// (c) The active catalog's timeline events: the `maintenance` events whose scope matches the
    /// instance count, with the rules of a `timeline_active` window (explicit V2 validity; a
    /// zero duration covers nothing).
    pub catalog: &'a [TimelineEvent],
}

/// A blackout: the union of intervals that covers a time, with the entry that covers it and ends
/// last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blackout {
    /// The id of the entry that covers the time and ends last: a configuration entry's `id` or a
    /// catalog event's `id`.
    pub id: String,
    /// Where that entry comes from (vocabulary `blackout_source`).
    pub source: BlackoutSource,
    /// The union's start.
    pub start_unix_ms: u64,
    /// The union's end: the first instant no interval of the union covers.
    pub end_unix_ms: u64,
}

/// D-6: why a candidate does not fit the activity window it would be admitted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayBoundaryDeferral {
    /// The end of the activity window the candidate would be admitted in.
    pub window_end_unix_ms: u64,
    /// The time the candidate needs (its expected duration).
    pub required_ms: u64,
    /// The start of the profile's next activity window: the candidate's wake.
    pub next_eligible_unix_ms: u64,
}

/// One interval of one source: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interval<'a> {
    start: u64,
    end: u64,
    source: BlackoutSource,
    id: &'a str,
}

/// The blackout that covers `instance` at `time`, or none (D-1): the maximal union of the
/// overlapping or adjacent intervals of the three sources that contains `time.unix_ms`.
pub fn blackout_at(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
) -> PolicyEvaluationResult<Option<Blackout>> {
    let covering = covering(sources, instance, time, time.unix_ms)?;
    let Some(reported) = last_ending(&covering) else {
        return Ok(None);
    };
    let first_start = covering
        .iter()
        .map(|interval| interval.start)
        .min()
        .unwrap_or(reported.start);
    Ok(Some(Blackout {
        id: reported.id.to_owned(),
        source: reported.source,
        start_unix_ms: union_start(sources, instance, time, first_start)?,
        end_unix_ms: union_end(sources, instance, time, reported.end)?,
    }))
}

/// The next blackout that starts after `time` and at or before `bound_unix_ms`, or none (D-1,
/// D-4): the next union of intervals that begins after `time`. When a blackout covers `time`,
/// the next one begins after that blackout's end. Its id and source belong to the entry that
/// covers its start and ends last; its end is the union's end.
pub fn next_blackout_start(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
    bound_unix_ms: u64,
) -> PolicyEvaluationResult<Option<Blackout>> {
    let after = match blackout_at(sources, instance, time)? {
        Some(current) => current.end_unix_ms,
        None => time.unix_ms,
    };
    if after >= bound_unix_ms {
        return Ok(None);
    }
    let Some(start) = earliest_start_after(sources, instance, time, after)?
        .filter(|start| *start <= bound_unix_ms)
    else {
        return Ok(None);
    };
    let covering = covering(sources, instance, time, start)?;
    let Some(reported) = last_ending(&covering) else {
        return Err(PolicyEvaluationError::invalid(
            "a blackout start is covered by no interval",
        ));
    };
    Ok(Some(Blackout {
        id: reported.id.to_owned(),
        source: reported.source,
        start_unix_ms: start,
        end_unix_ms: union_end(sources, instance, time, reported.end)?,
    }))
}

/// D-6: whether a run that needs `required_ms` from `now_unix_ms` fits the activity window of
/// `profile` it would be admitted in. `None` when it ends at or before the window's end, or when
/// no window of the profile is open now (the activity window's own gate decides that); otherwise
/// the deferral, which wakes at the profile's next window start.
pub fn day_boundary_fit(
    profile: &ActivityProfile,
    now_unix_ms: u64,
    required_ms: u64,
) -> PolicyEvaluationResult<Option<DayBoundaryDeferral>> {
    let Some(window) = activity_window_at(profile, now_unix_ms)? else {
        return Ok(None);
    };
    let ends = now_unix_ms
        .checked_add(required_ms)
        .ok_or_else(|| PolicyEvaluationError::overflow("the day-boundary fit's end overflowed"))?;
    if ends <= window.until_unix_ms {
        return Ok(None);
    }
    let next_eligible_unix_ms =
        next_window_start(profile, window.until_unix_ms)?.ok_or_else(|| {
            PolicyEvaluationError::invalid("an activity profile has no next window start")
        })?;
    Ok(Some(DayBoundaryDeferral {
        window_end_unix_ms: window.until_unix_ms,
        required_ms,
        next_eligible_unix_ms,
    }))
}

/// The earliest start, at or after `at`, of a window occurrence of `profile`, with the window
/// rules of `activity_window_at` (a window opens on its weekday; `start == end` is the whole
/// local day).
fn next_window_start(profile: &ActivityProfile, at: u64) -> PolicyEvaluationResult<Option<u64>> {
    let mut earliest: Option<u64> = None;
    for window in &profile.windows {
        let offset = i128::from(window.utc_offset_minutes) * MINUTE_MS;
        let day = (i128::from(at) + offset).div_euclid(DAY_MS);
        let full_day = window.start_minute_of_day == window.end_minute_of_day;
        let opens = if full_day {
            0
        } else {
            i128::from(window.start_minute_of_day) * MINUTE_MS
        };
        for local_day in day..=day + WEEKLY_SEARCH_DAYS {
            if !window.weekdays.contains(&weekday(local_day)) {
                continue;
            }
            let start = local_day * DAY_MS + opens - offset;
            if start < i128::from(at) {
                continue;
            }
            let start = u64::try_from(start).map_err(|_| {
                PolicyEvaluationError::overflow("an activity window start overflowed")
            })?;
            earliest = Some(earliest.map_or(start, |known| known.min(start)));
            break;
        }
    }
    Ok(earliest)
}

/// The weekday (1 Monday to 7 Sunday) of a local day counted from the Unix epoch.
fn weekday(local_day: i128) -> u8 {
    // Day 0 (1970-01-01) is a Thursday, weekday 4. The value is within 1..=7.
    ((local_day + 3).rem_euclid(7) + 1) as u8
}

/// The interval reported for a time: of those that cover it, the one that ends last; of equal
/// ends, the first source in `blackout_source` order (host, instance, catalog), then the least
/// id.
fn last_ending<'a>(intervals: &[Interval<'a>]) -> Option<Interval<'a>> {
    intervals.iter().copied().max_by(|left, right| {
        left.end
            .cmp(&right.end)
            .then_with(|| right.source.cmp(&left.source))
            .then_with(|| right.id.cmp(left.id))
    })
}

/// The union's end from `end`: while an interval covers the end reached so far (it overlaps or
/// is adjacent), the end moves to the latest end of those intervals.
fn union_end(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
    mut end: u64,
) -> PolicyEvaluationResult<u64> {
    for _ in 0..MAX_UNION_STEPS {
        let further = covering(sources, instance, time, end)?
            .iter()
            .map(|interval| interval.end)
            .max();
        match further {
            Some(further) if further > end => end = further,
            _ => break,
        }
    }
    Ok(end)
}

/// The union's start from `start`: while an interval covers the instant before the start
/// reached so far, the start moves to the earliest start of those intervals.
fn union_start(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
    mut start: u64,
) -> PolicyEvaluationResult<u64> {
    for _ in 0..MAX_UNION_STEPS {
        let Some(before) = start.checked_sub(1) else {
            break;
        };
        let earlier = covering(sources, instance, time, before)?
            .iter()
            .map(|interval| interval.start)
            .min();
        match earlier {
            Some(earlier) if earlier < start => start = earlier,
            _ => break,
        }
    }
    Ok(start)
}

/// Every interval of the three sources that covers `at` (`start <= at < end`); of a catalog
/// event, its latest occurrence at or before `at`, which of its occurrences ends last.
fn covering<'a>(
    sources: &BlackoutSources<'a>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
    at: u64,
) -> PolicyEvaluationResult<Vec<Interval<'a>>> {
    let mut intervals = Vec::new();
    for (source, entry) in configured(sources, instance) {
        if let Some((start, end)) = entry_covering(&entry.window, at)? {
            intervals.push(Interval {
                start,
                end,
                source,
                id: &entry.id,
            });
        }
    }
    for event in maintenance(sources, instance) {
        if let Some((start, end)) =
            event_interval(event, time, at)?.filter(|(start, end)| *start <= at && at < *end)
        {
            intervals.push(Interval {
                start,
                end,
                source: BlackoutSource::Catalog,
                id: &event.id,
            });
        }
    }
    Ok(intervals)
}

/// The earliest interval start of the three sources that lies after `after`.
fn earliest_start_after(
    sources: &BlackoutSources<'_>,
    instance: &InstanceSnapshot,
    time: EvaluationTime,
    after: u64,
) -> PolicyEvaluationResult<Option<u64>> {
    let mut earliest: Option<u64> = None;
    let mut consider = |start: u64| {
        earliest = Some(earliest.map_or(start, |known| known.min(start)));
    };
    for (_, entry) in configured(sources, instance) {
        if let Some(start) = entry_start_after(&entry.window, after)? {
            consider(start);
        }
    }
    for event in maintenance(sources, instance) {
        if let Some(start) = event_start_after(event, time, after)? {
            consider(start);
        }
    }
    Ok(earliest)
}

/// The configuration entries that apply to `instance`: the host's, then its own.
fn configured<'a>(
    sources: &BlackoutSources<'a>,
    instance: &InstanceSnapshot,
) -> Vec<(BlackoutSource, &'a BlackoutEntry)> {
    let own: &'a [BlackoutEntry] = sources
        .instances
        .get(&instance.instance_id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    sources
        .host
        .iter()
        .map(|entry| (BlackoutSource::Host, entry))
        .chain(own.iter().map(|entry| (BlackoutSource::Instance, entry)))
        .collect()
}

/// The catalog's `maintenance` events whose scope matches `instance`.
fn maintenance<'a>(
    sources: &BlackoutSources<'a>,
    instance: &InstanceSnapshot,
) -> Vec<&'a TimelineEvent> {
    sources
        .catalog
        .iter()
        .filter(|event| {
            event.event_kind == TimelineEventKind::Maintenance
                && scope_matches_instance(&event.scope, instance)
        })
        .collect()
}

/// The interval of a configuration entry that covers `at` and ends last, if any.
fn entry_covering(window: &BlackoutWindow, at: u64) -> PolicyEvaluationResult<Option<(u64, u64)>> {
    match window {
        BlackoutWindow::Once {
            from_unix_ms,
            until_unix_ms,
        } => {
            Ok((*from_unix_ms <= at && at < *until_unix_ms)
                .then_some((*from_unix_ms, *until_unix_ms)))
        }
        BlackoutWindow::Weekly {
            weekdays,
            utc_offset_minutes,
            start_minute_of_day,
            end_minute_of_day,
        } => {
            let weekly = Weekly::new(
                *utc_offset_minutes,
                *start_minute_of_day,
                *end_minute_of_day,
            );
            let day = weekly.local_day(at);
            let mut found: Option<(u64, u64)> = None;
            // An occurrence lasts at most one day, so only the one that began on the day before
            // and the one of this day can cover `at`.
            for local_day in [day - 1, day] {
                if !weekdays.contains(&weekday(local_day)) {
                    continue;
                }
                let (start, end) = weekly.occurrence(local_day)?;
                if start <= at && at < end && found.is_none_or(|(_, known)| end > known) {
                    found = Some((start, end));
                }
            }
            Ok(found)
        }
    }
}

/// The first start of a configuration entry after `after`, if any.
fn entry_start_after(window: &BlackoutWindow, after: u64) -> PolicyEvaluationResult<Option<u64>> {
    match window {
        BlackoutWindow::Once { from_unix_ms, .. } => {
            Ok((*from_unix_ms > after).then_some(*from_unix_ms))
        }
        BlackoutWindow::Weekly {
            weekdays,
            utc_offset_minutes,
            start_minute_of_day,
            end_minute_of_day,
        } => {
            let weekly = Weekly::new(
                *utc_offset_minutes,
                *start_minute_of_day,
                *end_minute_of_day,
            );
            let day = weekly.local_day(after);
            for local_day in day..=day + WEEKLY_SEARCH_DAYS {
                if !weekdays.contains(&weekday(local_day)) {
                    continue;
                }
                let (start, _) = weekly.occurrence(local_day)?;
                if start > after {
                    return Ok(Some(start));
                }
            }
            Ok(None)
        }
    }
}

/// A weekly entry's clock and the length of one occurrence.
struct Weekly {
    offset_ms: i128,
    start_ms: i128,
    length_ms: i128,
}

impl Weekly {
    fn new(utc_offset_minutes: i16, start_minute_of_day: u16, end_minute_of_day: u16) -> Self {
        let start = i128::from(start_minute_of_day);
        let end = i128::from(end_minute_of_day);
        // An end at or before the start crosses midnight; an end equal to the start is one day.
        let length_ms = if end > start {
            (end - start) * MINUTE_MS
        } else {
            DAY_MS - (start - end) * MINUTE_MS
        };
        Self {
            offset_ms: i128::from(utc_offset_minutes) * MINUTE_MS,
            start_ms: start * MINUTE_MS,
            length_ms,
        }
    }

    fn local_day(&self, at: u64) -> i128 {
        (i128::from(at) + self.offset_ms).div_euclid(DAY_MS)
    }

    /// The occurrence that begins on `local_day`, in Unix milliseconds (a start before the epoch
    /// is clipped to it).
    fn occurrence(&self, local_day: i128) -> PolicyEvaluationResult<(u64, u64)> {
        let start = local_day * DAY_MS + self.start_ms - self.offset_ms;
        let end = start + self.length_ms;
        let unix = |value: i128| {
            u64::try_from(value.max(0))
                .map_err(|_| PolicyEvaluationError::overflow("a weekly blackout overflowed"))
        };
        Ok((unix(start)?, unix(end)?))
    }
}

/// `time` moved to the Unix instant `at`, both clocks by the same step, so a catalog clock reads
/// `at` on the same path as the evaluation's own clock. `None` when `at` lies before the
/// monotonic clock's origin, where a local clock has no occurrence.
fn moved(time: EvaluationTime, at: u64) -> Option<EvaluationTime> {
    let step = i128::from(at) - i128::from(time.unix_ms);
    let monotonic_ms = u64::try_from(i128::from(time.monotonic_ms) + step).ok()?;
    Some(EvaluationTime {
        unix_ms: at,
        monotonic_ms,
    })
}

/// A maintenance event's validity: explicit V2 validity is required, as for a `timeline_active`
/// window.
fn validity(event: &TimelineEvent) -> PolicyEvaluationResult<(u64, Option<u64>)> {
    let validity = event.validity.as_ref().ok_or_else(|| {
        PolicyEvaluationError::invalid(format!(
            "maintenance event '{}' requires explicit V2 validity",
            event.id
        ))
    })?;
    Ok((validity.from_unix_ms, validity.until_unix_ms.0))
}

/// The clipped interval of the occurrence `occurrence` of `event`, or none when the clip leaves
/// nothing.
fn clipped(
    event: &TimelineEvent,
    occurrence: u64,
    from: u64,
    until: Option<u64>,
) -> PolicyEvaluationResult<Option<(u64, u64)>> {
    let end = occurrence.checked_add(event.duration_ms).ok_or_else(|| {
        PolicyEvaluationError::overflow(format!(
            "maintenance event '{}' duration overflowed",
            event.id
        ))
    })?;
    let start = occurrence.max(from);
    let end = until.map_or(end, |until| end.min(until));
    Ok((start < end).then_some((start, end)))
}

/// The clipped interval of the latest occurrence of `event` at or before `at`, if any.
fn event_interval(
    event: &TimelineEvent,
    time: EvaluationTime,
    at: u64,
) -> PolicyEvaluationResult<Option<(u64, u64)>> {
    let (from, until) = validity(event)?;
    if event.duration_ms == 0 || until.is_some_and(|until| at >= until) {
        return Ok(None);
    }
    let Some(at_time) = moved(time, at) else {
        return Ok(None);
    };
    let (latest, _) = schedule_occurrences(&event.schedule, at_time)?;
    match latest {
        Some(occurrence) => clipped(event, occurrence, from, until),
        None => Ok(None),
    }
}

/// The first start of a clipped occurrence of `event` after `after`, if any.
fn event_start_after(
    event: &TimelineEvent,
    time: EvaluationTime,
    after: u64,
) -> PolicyEvaluationResult<Option<u64>> {
    let (from, until) = validity(event)?;
    if event.duration_ms == 0 {
        return Ok(None);
    }
    let Some(probe) = after.checked_add(1).map(|probe| probe.max(from)) else {
        return Ok(None);
    };
    if until.is_some_and(|until| probe >= until) {
        return Ok(None);
    }
    let Some(probe_time) = moved(time, probe) else {
        return Ok(None);
    };
    let (latest, next) = schedule_occurrences(&event.schedule, probe_time)?;
    // The latest occurrence at or before the probe, clipped by the validity, may still begin
    // after `after`: at the validity's start, or exactly at the probe.
    if let Some((start, _)) = latest
        .map(|occurrence| clipped(event, occurrence, from, until))
        .transpose()?
        .flatten()
        .filter(|(start, _)| *start > after)
    {
        return Ok(Some(start));
    }
    match next {
        Some(occurrence) => Ok(clipped(event, occurrence, from, until)?.map(|(start, _)| start)),
        None => Ok(None),
    }
}
