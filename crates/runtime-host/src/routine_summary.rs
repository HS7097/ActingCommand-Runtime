// SPDX-License-Identifier: AGPL-3.0-only

//! The routine summary's memory (Workflow #429 R-12; register §1.264;
//! `contracts/runtime-interface.md`, "Workbench request kinds", the row `RoutineSummary`): the
//! health statistics of the contained-task runs of this owner epoch, per instance, bucket and
//! task. The host counts each policy run and direct run of an instance as it settles it, by the
//! task its run record names; `RoutineSummary` and the daily summary of the Runtime's messages
//! answer from here. Memory only: this holds no ledger handle and reads or writes nothing but
//! itself, so a restart begins the memory, its revisions and every instance's coverage again.
//! Pure: the host hands the settle time and the host's local offset at an instant.
//!
//! - **Counting.** A run that succeeded is a success; one that failed counts under its outcome
//!   code; one that ended `contained_task_cancelled` (a stop, `StopInstanceRun`,
//!   `CancelContainedTask`, a drain's cancel) is an interruption, never a failure. A run counts
//!   in the buckets of its settle time. Health statistics only, never a count of runs done or
//!   allowed (Alice's ruling 100).
//! - **Buckets.** An hour bucket is one hour of the host's local clock, a day bucket one calendar
//!   day of it, each opened from the host's local offset at the run that opens it: a day bucket
//!   runs from that day's local midnight to the next, each by the offset then, so a day across a
//!   daylight saving change lasts 23 or 25 hours. A bucket never overlaps another of its
//!   instance and width.
//! - **Bounds.** Per instance at most the last [`KEPT_HOURS`] hour buckets and [`KEPT_DAYS`] day
//!   buckets: an older one is dropped, and the instance's coverage of that width then begins at
//!   the oldest bucket kept; before a drop it begins at the epoch's start.
//! - **Revisions.** One counter per owner epoch, raised by every counted run; a bucket carries the
//!   revision of its last change, and a read after a revision answers only the buckets changed
//!   after it (none for a revision above the current one).

use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    OwnerEpoch, RoutineSummaryBucket, RoutineSummaryFailure, RoutineSummaryGranularity,
    RoutineSummaryInstance, RoutineSummaryPage, RoutineSummaryTask, RuntimeErrorCode,
};
use std::collections::BTreeMap;

/// One hour in milliseconds.
const HOUR_MS: u64 = 60 * 60 * 1_000;
/// One day of 24 hours in milliseconds.
const DAY_MS: u64 = 24 * HOUR_MS;
/// The hour buckets kept per instance: the contract's bound (R-12), not a tunable.
pub(crate) const KEPT_HOURS: usize = 48;
/// The day buckets kept per instance: the contract's bound (R-12), not a tunable.
pub(crate) const KEPT_DAYS: usize = 14;

/// How a run ended, as the summary counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunEnd {
    Succeeded,
    /// It failed: the outcome code of its terminal.
    Failed(CodeStr),
    /// It ended `contained_task_cancelled`: neither a success nor a failure of its task.
    Interrupted,
}

impl RunEnd {
    /// A run whose terminal carries the failure `code`, or none for a success: an interruption
    /// when the code is `contained_task_cancelled` (also a scheduled run's, whose terminal is a
    /// failure that carries that code), otherwise a failure under it.
    pub(crate) fn of_terminal(code: Option<CodeStr>) -> Self {
        match code {
            None => Self::Succeeded,
            Some(code) if code == RuntimeErrorCode::ContainedTaskCancelled.code() => {
                Self::Interrupted
            }
            Some(code) => Self::Failed(code),
        }
    }
}

/// One counted run: the instance it ran on, the task its run record names, how it ended and
/// when the host settled it.
pub(crate) struct SettledRun<'a> {
    pub(crate) instance_alias: &'a str,
    pub(crate) task_id: &'a str,
    pub(crate) end: RunEnd,
    pub(crate) settled_at_unix_ms: u64,
}

/// One task's health in one bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TaskHealth {
    successes: u32,
    failures: BTreeMap<CodeStr, u32>,
    interruptions: u32,
}

impl TaskHealth {
    fn count(&mut self, end: &RunEnd) {
        match end {
            RunEnd::Succeeded => self.successes = self.successes.saturating_add(1),
            RunEnd::Failed(code) => {
                let failures = self.failures.entry(code.clone()).or_insert(0);
                *failures = failures.saturating_add(1);
            }
            RunEnd::Interrupted => self.interruptions = self.interruptions.saturating_add(1),
        }
    }
}

/// One bucket, `[start_unix_ms, end_unix_ms)`, its tasks by id.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bucket {
    start_unix_ms: u64,
    end_unix_ms: u64,
    revision: u64,
    tasks: BTreeMap<String, TaskHealth>,
}

impl Bucket {
    fn holds(&self, unix_ms: u64) -> bool {
        self.start_unix_ms <= unix_ms && unix_ms < self.end_unix_ms
    }

    /// The bucket as the page carries it: its tasks by id, each task's failures by code.
    fn answer(&self) -> RoutineSummaryBucket {
        RoutineSummaryBucket {
            start_unix_ms: self.start_unix_ms,
            end_unix_ms: self.end_unix_ms,
            revision: self.revision,
            tasks: self
                .tasks
                .iter()
                .map(|(task_id, health)| RoutineSummaryTask {
                    task_id: task_id.clone(),
                    successes: health.successes,
                    failures: health
                        .failures
                        .iter()
                        .map(|(code, count)| RoutineSummaryFailure {
                            code: code.clone(),
                            count: *count,
                        })
                        .collect(),
                    interruptions: health.interruptions,
                })
                .collect(),
        }
    }
}

/// One instance's buckets of one width, oldest first, at most the width's bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Series {
    buckets: Vec<Bucket>,
    /// Whether a bucket was dropped: the coverage then begins at the oldest bucket kept.
    dropped: bool,
}

/// Where one run goes in one series: the bounds a new bucket would take, the bound of the
/// series and the revision of this change.
struct Placement<'a> {
    at_unix_ms: u64,
    bounds: (u64, u64),
    kept: usize,
    revision: u64,
    task_id: &'a str,
}

impl Series {
    /// When this series' statistics are complete from: the epoch's start until a bucket was
    /// dropped, then the start of the oldest bucket kept (never before the epoch's start).
    fn coverage_from(&self, started_at_unix_ms: u64) -> u64 {
        match self.buckets.first() {
            Some(oldest) if self.dropped => oldest.start_unix_ms.max(started_at_unix_ms),
            _ => started_at_unix_ms,
        }
    }

    /// Counts `end` in the bucket that holds the run's instant, opened with the placement's
    /// bounds when none does (kept clear of its neighbours: an offset change inside an hour, or
    /// a clock that went back), then drops the oldest buckets past the bound.
    fn count(&mut self, placement: &Placement<'_>, end: &RunEnd) {
        let at = placement.at_unix_ms;
        let index = match self.buckets.iter().position(|bucket| bucket.holds(at)) {
            Some(index) => index,
            None => {
                let after = self
                    .buckets
                    .partition_point(|bucket| bucket.start_unix_ms <= at);
                let (mut start_unix_ms, mut end_unix_ms) = placement.bounds;
                if let Some(previous) = after
                    .checked_sub(1)
                    .and_then(|before| self.buckets.get(before))
                {
                    start_unix_ms = start_unix_ms.max(previous.end_unix_ms);
                }
                if let Some(next) = self.buckets.get(after) {
                    end_unix_ms = end_unix_ms.min(next.start_unix_ms);
                }
                self.buckets.insert(
                    after,
                    Bucket {
                        start_unix_ms,
                        end_unix_ms,
                        revision: placement.revision,
                        tasks: BTreeMap::new(),
                    },
                );
                after
            }
        };
        if let Some(bucket) = self.buckets.get_mut(index) {
            bucket.revision = placement.revision;
            bucket
                .tasks
                .entry(placement.task_id.to_owned())
                .or_default()
                .count(end);
        }
        if self.buckets.len() > placement.kept {
            let excess = self.buckets.len() - placement.kept;
            self.buckets = self.buckets.split_off(excess);
            self.dropped = true;
        }
    }
}

/// One instance's hour and day buckets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InstanceSeries {
    hours: Series,
    days: Series,
}

/// The routine summary of one owner epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoutineSummaryMemory {
    /// When this epoch's memory began: every instance's coverage until a bucket is dropped.
    started_at_unix_ms: u64,
    /// The summary's revision now, raised by every counted run.
    revision: u64,
    /// Each instance's buckets, by alias.
    instances: BTreeMap<String, InstanceSeries>,
}

impl RoutineSummaryMemory {
    /// The memory of an epoch that began at `started_at_unix_ms`: no bucket, revision 0.
    pub(crate) const fn new(started_at_unix_ms: u64) -> Self {
        Self {
            started_at_unix_ms,
            revision: 0,
            instances: BTreeMap::new(),
        }
    }

    /// Counts `run` in its instance's hour and day buckets of its settle time: one change, so
    /// the revision is raised once and both buckets carry it. `offset_ms` is the host's local
    /// offset from UTC, in milliseconds east, at an instant.
    pub(crate) fn count(&mut self, run: &SettledRun<'_>, offset_ms: &dyn Fn(u64) -> i64) {
        self.revision = self.revision.saturating_add(1);
        let revision = self.revision;
        let at_unix_ms = run.settled_at_unix_ms;
        let series = self
            .instances
            .entry(run.instance_alias.to_owned())
            .or_default();
        series.hours.count(
            &Placement {
                at_unix_ms,
                bounds: hour_bounds(at_unix_ms, offset_ms),
                kept: KEPT_HOURS,
                revision,
                task_id: run.task_id,
            },
            &run.end,
        );
        series.days.count(
            &Placement {
                at_unix_ms,
                bounds: day_bounds(at_unix_ms, offset_ms),
                kept: KEPT_DAYS,
                revision,
                task_id: run.task_id,
            },
            &run.end,
        );
    }

    /// What `RoutineSummary` answers for `aliases`, in their order: each instance's coverage of
    /// `granularity` and its buckets of that width, oldest first, with `since_revision` only those
    /// changed after it. An instance with no counted run has the epoch's start and no bucket.
    pub(crate) fn page(
        &self,
        owner_epoch: OwnerEpoch,
        aliases: &[String],
        granularity: RoutineSummaryGranularity,
        since_revision: Option<u64>,
    ) -> RoutineSummaryPage {
        let instances = aliases
            .iter()
            .map(|alias| {
                let series = self.instances.get(alias).map(|series| match granularity {
                    RoutineSummaryGranularity::Hour => &series.hours,
                    RoutineSummaryGranularity::Day => &series.days,
                });
                RoutineSummaryInstance {
                    instance_alias: alias.clone(),
                    coverage_from_unix_ms: series.map_or(self.started_at_unix_ms, |series| {
                        series.coverage_from(self.started_at_unix_ms)
                    }),
                    buckets: series
                        .map(|series| {
                            series
                                .buckets
                                .iter()
                                .filter(|bucket| {
                                    since_revision.is_none_or(|since| bucket.revision > since)
                                })
                                .map(Bucket::answer)
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect();
        RoutineSummaryPage {
            owner_epoch,
            granularity,
            revision: self.revision,
            instances,
        }
    }
}

/// Which `width_ms` of a clock `offset_ms` east of UTC holds `unix_ms`, counted from the epoch.
fn index_of(unix_ms: u64, offset_ms: i64, width_ms: u64) -> i128 {
    (i128::from(unix_ms) + i128::from(offset_ms)).div_euclid(i128::from(width_ms))
}

/// The instant where the `index`th `width_ms` of a clock `offset_ms` east of UTC begins.
fn instant(index: i128, width_ms: u64, offset_ms: i64) -> u64 {
    u64::try_from(index * i128::from(width_ms) - i128::from(offset_ms)).unwrap_or(0)
}

/// The hour of the host's local clock that holds `at_unix_ms`, by the offset there.
fn hour_bounds(at_unix_ms: u64, offset_ms: &dyn Fn(u64) -> i64) -> (u64, u64) {
    let offset = offset_ms(at_unix_ms);
    let start = instant(index_of(at_unix_ms, offset, HOUR_MS), HOUR_MS, offset);
    (start, start.saturating_add(HOUR_MS))
}

/// The calendar day of the host's local clock that holds `at_unix_ms`: from its local midnight
/// to the next, each by the offset at that midnight (23 or 25 hours across a daylight saving
/// change). A day those midnights do not frame (an offset change at midnight itself) is the 24
/// hours from its midnight by the offset at `at_unix_ms`.
fn day_bounds(at_unix_ms: u64, offset_ms: &dyn Fn(u64) -> i64) -> (u64, u64) {
    let offset = offset_ms(at_unix_ms);
    let day = index_of(at_unix_ms, offset, DAY_MS);
    let midnight = |day: i128| instant(day, DAY_MS, offset_ms(instant(day, DAY_MS, offset)));
    let (start, end) = (midnight(day), midnight(day + 1));
    if start <= at_unix_ms && at_unix_ms < end {
        (start, end)
    } else {
        let start = instant(day, DAY_MS, offset);
        (start, start.saturating_add(DAY_MS))
    }
}

#[cfg(test)]
mod tests {
    // Workflow #429 R-12 (register §1.264): the memory on its own, with the settle times and the
    // host's local offset given (no clock, no host, no ledger). The host's wiring is
    // `tests::gate_429_r12`'s. The task ids and codes are made up.

    use super::*;
    use actingcommand_contract::IdentifierIssuer;

    const MINUTE_MS: u64 = 60_000;
    /// 2026-10-26 00:00 UTC, a whole day.
    const DAY_ZERO: u64 = 20_752 * DAY_MS;
    const ALIAS: &str = "r12.instance.a";

    fn epoch() -> OwnerEpoch {
        *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_owner_epoch()
            .expect("an epoch")
            .transport()
    }

    /// A failure code the catalog registers.
    fn deadline_code() -> CodeStr {
        RuntimeErrorCode::ContainedTaskDeadlineExceeded
            .code()
            .into()
    }

    fn run(task_id: &str, end: RunEnd, settled_at_unix_ms: u64) -> SettledRun<'_> {
        SettledRun {
            instance_alias: ALIAS,
            task_id,
            end,
            settled_at_unix_ms,
        }
    }

    fn page(
        memory: &RoutineSummaryMemory,
        granularity: RoutineSummaryGranularity,
        since_revision: Option<u64>,
    ) -> RoutineSummaryPage {
        memory.page(epoch(), &[ALIAS.to_owned()], granularity, since_revision)
    }

    /// The `[start, end)` of each bucket of the one instance's page.
    fn spans(page: &RoutineSummaryPage) -> Vec<(u64, u64)> {
        page.instances[0]
            .buckets
            .iter()
            .map(|bucket| (bucket.start_unix_ms, bucket.end_unix_ms))
            .collect()
    }

    // The hour and the day of the host's local clock hold each run: by a whole-hour offset, by a
    // half-hour one (whose hours begin at half past in UTC), and across both daylight saving
    // changes, where the day runs from local midnight to local midnight, 23 and 25 hours, and the
    // hours on both sides of the change stay one hour each and apart.
    #[test]
    fn hour_and_day_buckets_follow_the_hosts_local_clock() {
        let east_nine = |_: u64| 9 * 60 * MINUTE_MS as i64;
        let mut memory = RoutineSummaryMemory::new(DAY_ZERO);
        // 16:33 UTC is 01:33 of the next local day nine hours east.
        let at = DAY_ZERO + 16 * 60 * MINUTE_MS + 33 * MINUTE_MS;
        memory.count(&run("task.a", RunEnd::Succeeded, at), &east_nine);
        let local_midnight = DAY_ZERO + DAY_MS - 9 * 60 * MINUTE_MS;
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Hour, None)),
            [(
                DAY_ZERO + 16 * 60 * MINUTE_MS,
                DAY_ZERO + 17 * 60 * MINUTE_MS
            )]
        );
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Day, None)),
            [(local_midnight, local_midnight + DAY_MS)]
        );

        // Five and a half hours east: the hours begin at half past in UTC.
        let east_five_half = |_: u64| 330 * MINUTE_MS as i64;
        let mut memory = RoutineSummaryMemory::new(DAY_ZERO);
        let at = DAY_ZERO + 10 * 60 * MINUTE_MS + 5 * MINUTE_MS;
        memory.count(&run("task.a", RunEnd::Succeeded, at), &east_five_half);
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Hour, None)),
            [(
                DAY_ZERO + 9 * 60 * MINUTE_MS + 30 * MINUTE_MS,
                DAY_ZERO + 10 * 60 * MINUTE_MS + 30 * MINUTE_MS
            )]
        );
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Day, None)),
            [(
                DAY_ZERO - 330 * MINUTE_MS,
                DAY_ZERO + DAY_MS - 330 * MINUTE_MS
            )]
        );

        // Spring forward: one hour east until 01:00 UTC (02:00 local), two hours east after.
        let change = DAY_ZERO + 60 * MINUTE_MS;
        let spring = move |unix_ms: u64| {
            if unix_ms < change {
                60 * MINUTE_MS as i64
            } else {
                120 * MINUTE_MS as i64
            }
        };
        let mut memory = RoutineSummaryMemory::new(DAY_ZERO - DAY_MS);
        memory.count(
            &run("task.a", RunEnd::Succeeded, change - 30 * MINUTE_MS),
            &spring,
        );
        memory.count(
            &run("task.a", RunEnd::Succeeded, change + 30 * MINUTE_MS),
            &spring,
        );
        let day = page(&memory, RoutineSummaryGranularity::Day, None);
        assert_eq!(
            spans(&day),
            [(
                DAY_ZERO - 60 * MINUTE_MS,
                DAY_ZERO + DAY_MS - 120 * MINUTE_MS
            )]
        );
        let (start, end) = spans(&day)[0];
        assert_eq!(end - start, 23 * 60 * MINUTE_MS, "a 23-hour day");
        assert_eq!(day.instances[0].buckets[0].tasks[0].successes, 2);
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Hour, None)),
            [
                (DAY_ZERO, DAY_ZERO + 60 * MINUTE_MS),
                (DAY_ZERO + 60 * MINUTE_MS, DAY_ZERO + 120 * MINUTE_MS),
            ]
        );

        // Fall back: two hours east until 01:00 UTC (03:00 local), one hour east after.
        let fall = move |unix_ms: u64| {
            if unix_ms < change {
                120 * MINUTE_MS as i64
            } else {
                60 * MINUTE_MS as i64
            }
        };
        let mut memory = RoutineSummaryMemory::new(DAY_ZERO - DAY_MS);
        memory.count(
            &run("task.a", RunEnd::Succeeded, change - 30 * MINUTE_MS),
            &fall,
        );
        memory.count(
            &run("task.a", RunEnd::Succeeded, change + 30 * MINUTE_MS),
            &fall,
        );
        let day = page(&memory, RoutineSummaryGranularity::Day, None);
        assert_eq!(
            spans(&day),
            [(
                DAY_ZERO - 120 * MINUTE_MS,
                DAY_ZERO + DAY_MS - 60 * MINUTE_MS
            )]
        );
        let (start, end) = spans(&day)[0];
        assert_eq!(end - start, 25 * 60 * MINUTE_MS, "a 25-hour day");
        assert_eq!(
            spans(&page(&memory, RoutineSummaryGranularity::Hour, None)),
            [
                (DAY_ZERO, DAY_ZERO + 60 * MINUTE_MS),
                (DAY_ZERO + 60 * MINUTE_MS, DAY_ZERO + 120 * MINUTE_MS),
            ]
        );
    }

    // Each counted run raises the revision once and marks the buckets it changed; a read after
    // a revision answers only the buckets changed after it, none after the current one or above
    // it, and the page always says the current revision and the epoch it belongs to.
    #[test]
    fn since_revision_answers_only_the_buckets_changed_after_it() {
        let utc = |_: u64| 0_i64;
        let mut memory = RoutineSummaryMemory::new(DAY_ZERO);
        let first_hour = DAY_ZERO + 5 * MINUTE_MS;
        let second_hour = DAY_ZERO + 65 * MINUTE_MS;
        memory.count(&run("task.a", RunEnd::Succeeded, first_hour), &utc);
        memory.count(
            &run("task.b", RunEnd::Failed(deadline_code()), second_hour),
            &utc,
        );
        let epoch = epoch();
        let all = memory.page(
            epoch,
            &[ALIAS.to_owned()],
            RoutineSummaryGranularity::Hour,
            None,
        );
        assert_eq!(all.owner_epoch, epoch);
        assert_eq!(all.revision, 2);
        let revisions = |page: &RoutineSummaryPage| {
            page.instances[0]
                .buckets
                .iter()
                .map(|bucket| (bucket.start_unix_ms, bucket.revision))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            revisions(&all),
            [(DAY_ZERO, 1), (DAY_ZERO + 60 * MINUTE_MS, 2)]
        );
        let after_one = page(&memory, RoutineSummaryGranularity::Hour, Some(1));
        assert_eq!(revisions(&after_one), [(DAY_ZERO + 60 * MINUTE_MS, 2)]);
        assert_eq!(after_one.revision, 2);

        // The first hour changes again: only it is newer than 2.
        memory.count(&run("task.a", RunEnd::Interrupted, first_hour + 1), &utc);
        let after_two = page(&memory, RoutineSummaryGranularity::Hour, Some(2));
        assert_eq!(revisions(&after_two), [(DAY_ZERO, 3)]);
        assert_eq!(after_two.revision, 3);
        let task = &after_two.instances[0].buckets[0].tasks[0];
        assert_eq!((task.successes, task.interruptions), (1, 1));
        // The day bucket changed with every run.
        let day = page(&memory, RoutineSummaryGranularity::Day, Some(2));
        assert_eq!(day.instances[0].buckets.len(), 1);
        assert_eq!(day.instances[0].buckets[0].revision, 3);

        // Nothing changed after the current revision, nor after one above it; the instance's
        // coverage is still answered.
        for cursor in [3, 99] {
            let none = page(&memory, RoutineSummaryGranularity::Hour, Some(cursor));
            assert_eq!(none.revision, 3);
            assert_eq!(none.instances.len(), 1);
            assert!(none.instances[0].buckets.is_empty(), "after {cursor}");
            assert_eq!(none.instances[0].coverage_from_unix_ms, DAY_ZERO);
        }
        // An instance the read names with no run: its coverage and no bucket.
        let other = memory.page(
            epoch,
            &["r12.instance.b".to_owned()],
            RoutineSummaryGranularity::Hour,
            Some(0),
        );
        assert_eq!(other.instances[0].instance_alias, "r12.instance.b");
        assert!(other.instances[0].buckets.is_empty());
        assert_eq!(other.instances[0].coverage_from_unix_ms, DAY_ZERO);
    }

    // At most the last 48 hour buckets and 14 day buckets are kept: the 49th hour drops the
    // first, and the instance's hour coverage moves from the epoch's start to the start of the
    // oldest hour kept; the 15th day does the same for the days. Until a drop the coverage is the
    // epoch's start.
    #[test]
    fn old_buckets_are_dropped_and_coverage_moves_forward() {
        let utc = |_: u64| 0_i64;
        let started = DAY_ZERO + 20 * MINUTE_MS;
        let mut memory = RoutineSummaryMemory::new(started);
        for hour in 0..48 {
            memory.count(
                &run("task.a", RunEnd::Succeeded, started + hour * 60 * MINUTE_MS),
                &utc,
            );
        }
        let hours = page(&memory, RoutineSummaryGranularity::Hour, None);
        assert_eq!(hours.instances[0].buckets.len(), KEPT_HOURS);
        assert_eq!(hours.instances[0].coverage_from_unix_ms, started);
        assert_eq!(hours.instances[0].buckets[0].start_unix_ms, DAY_ZERO);

        memory.count(
            &run("task.a", RunEnd::Succeeded, started + 48 * 60 * MINUTE_MS),
            &utc,
        );
        let hours = page(&memory, RoutineSummaryGranularity::Hour, None);
        assert_eq!(hours.instances[0].buckets.len(), KEPT_HOURS);
        let oldest = DAY_ZERO + 60 * MINUTE_MS;
        assert_eq!(hours.instances[0].buckets[0].start_unix_ms, oldest);
        assert_eq!(hours.instances[0].coverage_from_unix_ms, oldest);
        // The days are within their bound: their coverage stays the epoch's start.
        let days = page(&memory, RoutineSummaryGranularity::Day, None);
        assert_eq!(days.instances[0].buckets.len(), 3);
        assert_eq!(days.instances[0].coverage_from_unix_ms, started);

        let mut memory = RoutineSummaryMemory::new(started);
        for day in 0..15 {
            memory.count(
                &run("task.a", RunEnd::Succeeded, started + day * DAY_MS),
                &utc,
            );
        }
        let days = page(&memory, RoutineSummaryGranularity::Day, None);
        assert_eq!(days.instances[0].buckets.len(), KEPT_DAYS);
        assert_eq!(
            days.instances[0].buckets[0].start_unix_ms,
            DAY_ZERO + DAY_MS
        );
        assert_eq!(days.instances[0].coverage_from_unix_ms, DAY_ZERO + DAY_MS);
        assert_eq!(
            days.instances[0].buckets[KEPT_DAYS - 1].start_unix_ms,
            DAY_ZERO + 14 * DAY_MS
        );
    }
}
