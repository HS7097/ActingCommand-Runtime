// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.responsiveness.*` (Workflow #385 P-c, PERF §10.2): the responsiveness estimate's
//! window and smoothing, the host probes, the scored signals and their baselines. P-m reads them.

use super::{AT_START, Values, at_least, between, failure, integer_within, row, rule};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};
use serde_json::Value;

/// A signal's initial baseline per capture backend (an object with a `default` member).
pub const CAPTURE_ACQUIRE_INITIAL_BASELINE_US: &str =
    "performance.responsiveness.signals.capture_acquire.initial_baseline_us";
/// The touch response's initial baseline per backend (an object with a `default` member).
pub const TOUCH_RESPONSE_INITIAL_BASELINE_US: &str =
    "performance.responsiveness.signals.touch_response.initial_baseline_us";
pub const BASELINE_MIN_SAMPLES: &str = "performance.responsiveness.baseline.min_samples";
pub const BASELINE_WINDOW_SAMPLES: &str = "performance.responsiveness.baseline.window_samples";
pub const BASELINE_CLAMP_MIN_MILLI: &str = "performance.responsiveness.baseline.clamp_min_milli";
pub const BASELINE_CLAMP_MAX_MILLI: &str = "performance.responsiveness.baseline.clamp_max_milli";

/// The member every per-backend baseline object holds: the value of a backend it does not name.
const DEFAULT_BACKEND: &str = "default";

const READER: &str = "P-m";

const fn flag(name: &'static str) -> KeyRow {
    row(name, ValueType::Boolean, KeyRange::Any, AT_START, READER)
}

const fn integer(name: &'static str, range: KeyRange) -> KeyRow {
    row(name, ValueType::Integer, range, AT_START, READER)
}

const fn duration(name: &'static str, range: KeyRange) -> KeyRow {
    row(name, ValueType::DurationMs, range, AT_START, READER)
}

pub(crate) static ROWS: &[KeyRow] = &[
    duration("performance.responsiveness.window_ms", at_least(1)),
    integer("performance.responsiveness.min_window_samples", at_least(1)),
    integer(
        "performance.responsiveness.ewma_alpha_milli",
        between(1, 1_000),
    ),
    duration("performance.responsiveness.emu_stale_ms", at_least(1)),
    flag("performance.responsiveness.host_probe.timer.enabled"),
    duration(
        "performance.responsiveness.host_probe.timer.interval_ms",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.timer.burst",
        at_least(1),
    ),
    duration(
        "performance.responsiveness.host_probe.timer.period_ms",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.timer.initial_baseline_us",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.timer.allowance_milli",
        at_least(0),
    ),
    integer(
        "performance.responsiveness.host_probe.timer.slack_us",
        at_least(0),
    ),
    flag("performance.responsiveness.host_probe.message.enabled"),
    duration(
        "performance.responsiveness.host_probe.message.interval_ms",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.message.burst",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.message.initial_baseline_us",
        at_least(1),
    ),
    integer(
        "performance.responsiveness.host_probe.message.allowance_milli",
        at_least(0),
    ),
    integer(
        "performance.responsiveness.host_probe.message.slack_us",
        at_least(0),
    ),
    flag("performance.responsiveness.signals.capture_acquire.enabled"),
    row(
        CAPTURE_ACQUIRE_INITIAL_BASELINE_US,
        ValueType::Object,
        KeyRange::Any,
        AT_START,
        READER,
    ),
    integer(
        "performance.responsiveness.signals.capture_acquire.allowance_milli",
        at_least(0),
    ),
    integer(
        "performance.responsiveness.signals.capture_acquire.slack_us",
        at_least(0),
    ),
    flag("performance.responsiveness.signals.touch_response.enabled"),
    row(
        TOUCH_RESPONSE_INITIAL_BASELINE_US,
        ValueType::Object,
        KeyRange::Any,
        AT_START,
        READER,
    ),
    integer(
        "performance.responsiveness.signals.touch_response.allowance_milli",
        at_least(0),
    ),
    integer(
        "performance.responsiveness.signals.touch_response.slack_us",
        at_least(0),
    ),
    flag("performance.responsiveness.signals.touch_response.score_gestures"),
    flag("performance.responsiveness.signals.recognition.enabled"),
    flag("performance.responsiveness.signals.page_settle.record"),
    flag("performance.responsiveness.signals.page_settle.enabled"),
    flag("performance.responsiveness.emu_freeze_while_paused"),
    integer(BASELINE_MIN_SAMPLES, at_least(1)),
    integer(BASELINE_WINDOW_SAMPLES, at_least(1)),
    integer(
        "performance.responsiveness.baseline.quantile",
        between(0, 100),
    ),
    integer(BASELINE_CLAMP_MIN_MILLI, at_least(1)),
    integer(BASELINE_CLAMP_MAX_MILLI, at_least(1)),
    integer(
        "performance.responsiveness.baseline.persist_delta_milli",
        at_least(0),
    ),
    duration(
        "performance.responsiveness.baseline.persist_min_interval_ms",
        at_least(0),
    ),
];

/// §10.2, §3.5: each per-backend baseline names a `default` and positive values under backend
/// words; the baseline window holds at least its minimum; the clamp's floor is not above its
/// ceiling.
pub(super) fn check(values: &Values<'_>) -> Result<(), ConfigFailure> {
    for key in [
        CAPTURE_ACQUIRE_INITIAL_BASELINE_US,
        TOUCH_RESPONSE_INITIAL_BASELINE_US,
    ] {
        check_per_backend(key, &values.json(key)?)?;
    }
    rule(
        BASELINE_WINDOW_SAMPLES,
        values.integer(BASELINE_WINDOW_SAMPLES)? >= values.integer(BASELINE_MIN_SAMPLES)?,
    )?;
    rule(
        BASELINE_CLAMP_MAX_MILLI,
        values.integer(BASELINE_CLAMP_MAX_MILLI)? >= values.integer(BASELINE_CLAMP_MIN_MILLI)?,
    )
}

/// A per-backend object: backend words (`[a-z0-9_]`), each a positive number of microseconds,
/// `default` among them.
fn check_per_backend(key: &str, value: &Value) -> Result<(), ConfigFailure> {
    let members = value
        .as_object()
        .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
    for (backend, microseconds) in members {
        let word = !backend.is_empty()
            && backend
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if !word {
            return Err(failure(key, ConfigValueProblem::OutOfRange));
        }
        integer_within(key, Some(microseconds), 1, i64::MAX)?;
    }
    rule(key, members.contains_key(DEFAULT_BACKEND))
}
