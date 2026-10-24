// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #386 SR-4a1 (SCH-M frozen model v1.0 §3.2, D-1 and D-6): the pure blackout and
//! day-boundary judgements, beside `activity_window_at`. RED STUB: the types only; every
//! function answers none.
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

use super::{EvaluationTime, InstanceSnapshot, PolicyEvaluationResult};
use crate::{ActivityProfile, TimelineEvent};

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

/// SR-4a1 red stub: no source is read.
pub fn blackout_at(
    _sources: &BlackoutSources<'_>,
    _instance: &InstanceSnapshot,
    _time: EvaluationTime,
) -> PolicyEvaluationResult<Option<Blackout>> {
    Ok(None)
}

/// SR-4a1 red stub: no source is read.
pub fn next_blackout_start(
    _sources: &BlackoutSources<'_>,
    _instance: &InstanceSnapshot,
    _time: EvaluationTime,
    _bound_unix_ms: u64,
) -> PolicyEvaluationResult<Option<Blackout>> {
    Ok(None)
}

/// SR-4a1 red stub: no window is read.
pub fn day_boundary_fit(
    _profile: &ActivityProfile,
    _now_unix_ms: u64,
    _required_ms: u64,
) -> PolicyEvaluationResult<Option<DayBoundaryDeferral>> {
    Ok(None)
}
