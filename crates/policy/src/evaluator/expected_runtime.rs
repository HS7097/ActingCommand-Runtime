// SPDX-License-Identifier: AGPL-3.0-only

//! SCH-M slice ED-1 (Workflow #405; SCH-M frozen model v1.0 §3.4 D-12, D-15): the expected duration E of a
//! task package on an instance, Alice 10-22: 「每个任务执行所需的时间可以动态的从账本读，近5次该任务包的
//! 平均时间加5分钟」.
//!
//! - E = min(⌈mean(S)⌉ + `margin_ms`, M): S is the latest min(`runs`, k) of the k samples stored
//!   for the pair, M the task's `loop_budget.max_runtime_ms`.
//! - With no sample (k = 0) or with the switch off, E is the declared value D.
//!
//! The host (slice ED-2) computes E from the instance's `health.run_duration` record with this
//! function and hands it to the evaluation as `TaskRuntimeSnapshot::expected_runtime_ms`; the
//! evaluation then uses E for the candidate's cost, the dispatch intent's duration (hence the
//! budget reservations and forward planning) and the production estimate. A pair whose snapshot
//! carries no E is evaluated exactly as before.

use super::{PolicyEvaluationError, PolicyEvaluationResult, TaskRuntimeSnapshot};
use crate::TaskSpec;

/// The expected-duration settings of one instance as they are in effect
/// (`instance.<id>.scheduling.expected_runtime.*`, else `scheduling.expected_runtime.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedRuntimeSettings {
    /// Off: E is always the declared value, and the snapshot carries none.
    pub enabled: bool,
    /// The most samples, newest first, that the mean reads: `1..=16`.
    pub runs: u8,
    /// What is added to the mean, in milliseconds.
    pub margin_ms: u64,
}

/// Where an expected duration comes from (`ac_schedule`'s `expected_runtime_source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedRuntimeSource {
    /// The declared value D: no sample, or the switch is off.
    Declared,
    /// The mean of this many samples plus the margin.
    Runs(u8),
}

/// An expected duration and where it comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedRuntime {
    pub expected_ms: u64,
    pub source: ExpectedRuntimeSource,
}

impl ExpectedRuntime {
    /// The members of `TaskRuntimeSnapshot` that carry it, `expected_runtime_ms` and
    /// `expected_runtime_runs`: both absent for the declared value, so the evaluation input is
    /// today's, byte for byte.
    pub fn snapshot_members(&self) -> (Option<u64>, Option<u8>) {
        match self.source {
            ExpectedRuntimeSource::Declared => (None, None),
            ExpectedRuntimeSource::Runs(runs) => (Some(self.expected_ms), Some(runs)),
        }
    }
}

/// SCH-M §3.4 D-12: E for a pair from its samples, newest first (`d1` first), its declared value
/// `declared_ms` and its task's `max_runtime_ms`:
/// - the switch off, or no sample: `declared_ms`;
/// - otherwise the mean of the newest min(`runs`, k) samples, rounded up, plus `margin_ms`, at
///   most `max_runtime_ms`.
///
/// `runs` outside `1..=16` or an overflowing sum is `policy_evaluation_input_invalid` or
/// `policy_evaluation_numeric_overflow`: the configuration check keeps both out.
pub fn expected_runtime(
    samples_newest_first: &[u64],
    settings: ExpectedRuntimeSettings,
    declared_ms: u64,
    max_runtime_ms: u64,
) -> PolicyEvaluationResult<ExpectedRuntime> {
    if !(1..=MAX_EXPECTED_RUNTIME_RUNS).contains(&settings.runs) {
        return Err(PolicyEvaluationError::invalid(
            "expected-runtime runs must be within 1..=16",
        ));
    }
    let used = samples_newest_first.len().min(usize::from(settings.runs));
    if !settings.enabled || used == 0 {
        return Ok(ExpectedRuntime {
            expected_ms: declared_ms,
            source: ExpectedRuntimeSource::Declared,
        });
    }
    let sum = samples_newest_first[..used]
        .iter()
        .map(|sample| u128::from(*sample))
        .sum::<u128>();
    let count = used as u128;
    let mean = sum.div_ceil(count);
    let expected = u64::try_from(mean)
        .ok()
        .and_then(|mean| mean.checked_add(settings.margin_ms))
        .ok_or_else(|| PolicyEvaluationError::overflow("an expected duration overflowed"))?;
    Ok(ExpectedRuntime {
        expected_ms: expected.min(max_runtime_ms),
        // `used` is at most `runs`, which is at most 16.
        source: ExpectedRuntimeSource::Runs(used as u8),
    })
}

/// The most samples a pair's mean reads (the range of `scheduling.expected_runtime.runs`).
pub const MAX_EXPECTED_RUNTIME_RUNS: u8 = 16;

/// SCH-M D-15: the E a pair's snapshot carries, at most the task's `max_runtime_ms` (the
/// admission's own bound); none when the snapshot carries none, and the declared value applies.
pub(crate) fn snapshot_expected_runtime(
    task: &TaskSpec,
    state: Option<&TaskRuntimeSnapshot>,
) -> Option<u64> {
    state
        .and_then(|state| state.expected_runtime_ms)
        .map(|expected| expected.min(task.loop_budget.max_runtime_ms))
}
