// SPDX-License-Identifier: AGPL-3.0-only

//! `performance.responsiveness.*` (Workflow #385 P-c, PERF §10.2): the responsiveness estimate's
//! window and smoothing, the host probes, the scored signals and their baselines. P-m reads them.

use super::{AT_START, Values, at_least, between, failure, integer_within, row, rule};
use crate::codes::ConfigValueProblem;
use crate::effective::ConfigFailure;
use crate::table::{KeyRange, KeyRow, ValueType};
use serde_json::Value;
use std::collections::BTreeMap;

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

/// Workflow #385 P-m (PERF §3.3, §10.2): one scored signal. Its sample scores 1 at or below the
/// allowance `baseline × allowance_milli / 1000 + slack_us`, else `allowance / sample`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalSettings {
    pub enabled: bool,
    /// The initial baseline per backend word; `default` is the one of a backend it does not
    /// name, or of a backend that is not known.
    pub initial_baseline_us: BTreeMap<String, u64>,
    pub allowance_milli: u64,
    pub slack_us: u64,
}

impl SignalSettings {
    /// The initial baseline of `backend` (`None`: not known), else the `default` member's.
    pub fn initial_baseline_us(&self, backend: Option<&str>) -> u64 {
        backend
            .and_then(|word| self.initial_baseline_us.get(word))
            .or_else(|| self.initial_baseline_us.get(DEFAULT_BACKEND))
            .copied()
            .unwrap_or(1)
    }
}

/// Workflow #385 P-m (PERF §3.2): a host probe's cadence and its signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProbeSettings {
    pub signal: SignalSettings,
    pub interval_ms: u64,
    pub burst: u32,
    /// The timer probe's sleep; the message probe has none.
    pub period_ms: Option<u64>,
}

/// Workflow #385 P-m (PERF §3.5): how a baseline is learned and bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselineSettings {
    pub min_samples: usize,
    pub window_samples: usize,
    /// The quantile of the calm samples, in percent (50: the median).
    pub quantile: u8,
    pub clamp_min_milli: u64,
    pub clamp_max_milli: u64,
}

/// Workflow #385 P-m: `performance.responsiveness.*` as the shadow measurement reads it.
/// `signals.recognition.enabled`, `signals.page_settle.*`, `emu_freeze_while_paused` and
/// `baseline.persist_*` are not read here: no recognition or page-settle score exists yet, a
/// paused instance comes with P-s, and a baseline stays in memory until AO-5 and AO-6 (§3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsivenessSettings {
    pub window_ms: u64,
    pub min_window_samples: usize,
    pub ewma_alpha_milli: u32,
    pub emu_stale_ms: u64,
    pub timer: HostProbeSettings,
    pub message: HostProbeSettings,
    pub capture_acquire: SignalSettings,
    pub touch_response: SignalSettings,
    /// Whether a gesture's touch response is scored too (§3.2: by default only a tap is).
    pub score_gestures: bool,
    pub baseline: BaselineSettings,
}

/// A host probe's signal: one initial baseline (no backend), its allowance and slack.
fn probe_signal(
    values: &Values<'_>,
    enabled: &'static str,
    initial: &'static str,
    allowance: &'static str,
    slack: &'static str,
) -> Result<SignalSettings, ConfigFailure> {
    Ok(SignalSettings {
        enabled: values.flag(enabled)?,
        initial_baseline_us: BTreeMap::from([(
            DEFAULT_BACKEND.to_owned(),
            values.unsigned::<u64>(initial)?,
        )]),
        allowance_milli: values.unsigned(allowance)?,
        slack_us: values.unsigned(slack)?,
    })
}

/// A device signal: its initial baseline per backend, its allowance and slack.
fn device_signal(
    values: &Values<'_>,
    enabled: &'static str,
    initial: &'static str,
    allowance: &'static str,
    slack: &'static str,
) -> Result<SignalSettings, ConfigFailure> {
    Ok(SignalSettings {
        enabled: values.flag(enabled)?,
        initial_baseline_us: per_backend(initial, &values.json(initial)?)?,
        allowance_milli: values.unsigned(allowance)?,
        slack_us: values.unsigned(slack)?,
    })
}

/// Workflow #385 P-m: the subsection in effect.
pub(super) fn read(values: &Values<'_>) -> Result<ResponsivenessSettings, ConfigFailure> {
    Ok(ResponsivenessSettings {
        window_ms: values.unsigned("performance.responsiveness.window_ms")?,
        min_window_samples: values.unsigned("performance.responsiveness.min_window_samples")?,
        ewma_alpha_milli: values.unsigned("performance.responsiveness.ewma_alpha_milli")?,
        emu_stale_ms: values.unsigned("performance.responsiveness.emu_stale_ms")?,
        timer: HostProbeSettings {
            signal: probe_signal(
                values,
                "performance.responsiveness.host_probe.timer.enabled",
                "performance.responsiveness.host_probe.timer.initial_baseline_us",
                "performance.responsiveness.host_probe.timer.allowance_milli",
                "performance.responsiveness.host_probe.timer.slack_us",
            )?,
            interval_ms: values
                .unsigned("performance.responsiveness.host_probe.timer.interval_ms")?,
            burst: values.unsigned("performance.responsiveness.host_probe.timer.burst")?,
            period_ms: Some(
                values.unsigned("performance.responsiveness.host_probe.timer.period_ms")?,
            ),
        },
        message: HostProbeSettings {
            signal: probe_signal(
                values,
                "performance.responsiveness.host_probe.message.enabled",
                "performance.responsiveness.host_probe.message.initial_baseline_us",
                "performance.responsiveness.host_probe.message.allowance_milli",
                "performance.responsiveness.host_probe.message.slack_us",
            )?,
            interval_ms: values
                .unsigned("performance.responsiveness.host_probe.message.interval_ms")?,
            burst: values.unsigned("performance.responsiveness.host_probe.message.burst")?,
            period_ms: None,
        },
        capture_acquire: device_signal(
            values,
            "performance.responsiveness.signals.capture_acquire.enabled",
            CAPTURE_ACQUIRE_INITIAL_BASELINE_US,
            "performance.responsiveness.signals.capture_acquire.allowance_milli",
            "performance.responsiveness.signals.capture_acquire.slack_us",
        )?,
        touch_response: device_signal(
            values,
            "performance.responsiveness.signals.touch_response.enabled",
            TOUCH_RESPONSE_INITIAL_BASELINE_US,
            "performance.responsiveness.signals.touch_response.allowance_milli",
            "performance.responsiveness.signals.touch_response.slack_us",
        )?,
        score_gestures: values
            .flag("performance.responsiveness.signals.touch_response.score_gestures")?,
        baseline: BaselineSettings {
            min_samples: values.unsigned(BASELINE_MIN_SAMPLES)?,
            window_samples: values.unsigned(BASELINE_WINDOW_SAMPLES)?,
            quantile: values.unsigned("performance.responsiveness.baseline.quantile")?,
            clamp_min_milli: values.unsigned(BASELINE_CLAMP_MIN_MILLI)?,
            clamp_max_milli: values.unsigned(BASELINE_CLAMP_MAX_MILLI)?,
        },
    })
}

/// A per-backend object as [`check_per_backend`] accepts it, read into its members.
fn per_backend(key: &str, value: &Value) -> Result<BTreeMap<String, u64>, ConfigFailure> {
    check_per_backend(key, value)?;
    let members = value
        .as_object()
        .ok_or_else(|| failure(key, ConfigValueProblem::WrongType))?;
    members
        .iter()
        .map(|(backend, microseconds)| {
            let microseconds = integer_within(key, Some(microseconds), 1, i64::MAX)?;
            u64::try_from(microseconds)
                .map(|microseconds| (backend.clone(), microseconds))
                .map_err(|_| failure(key, ConfigValueProblem::OutOfRange))
        })
        .collect()
}
