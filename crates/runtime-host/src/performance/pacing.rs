// SPDX-License-Identifier: AGPL-3.0-only

//! The shadow measurement (Workflow #385 P-m, frozen model PERF v1.4.2 §3, §4.1-§4.3, §4.6,
//! §10.3; coordinator ruling 7 of wave 13: measure and record only, no pacing action).
//!
//! - **Samples** (§3.2): the host timer probe's oversleep, and per instance one frame's
//!   acquisition (`capture_acquire_us`) and one input's backend response (`touch_response_us`,
//!   scored for a tap only unless `score_gestures`), handed over at the pipeline's fan-out point.
//! - **Scores** (§3.3): a sample scores 10 000 at or below its allowance `baseline ×
//!   allowance_milli / 1000 + slack_us`, else `10 000 × allowance / sample`; a signal's value is
//!   the mean score of its window, unknown below `min_window_samples`. `R_host` is the lowest
//!   known host signal, `R_emu(i)` the lowest known signal of instance i, each smoothed per
//!   evaluation (EWMA) and unknown once no sample came for `emu_stale_ms`; `R(i)` is the lower
//!   of the two that are known.
//! - **Baselines** (§3.5): per scope, signal and backend, the configured quantile of the calm
//!   samples (taken while the scope's stage is S0 without pressure), the initial value until
//!   `min_samples`, held within `[initial × clamp_min, initial × clamp_max]`; a held baseline is
//!   the warning `perf_baseline_clamped`. They stay in memory until AO-5 and AO-6.
//! - **Tracked value** (§4.3): `R_eff` starts at 10 000, follows a lower value after
//!   `degrade_sustain_ms` and a value `hysteresis_bp` higher after `recover_sustain_ms`; a clock
//!   jump only clears the timers; without any evidence a tick counts as a recovery to 10 000.
//! - **Curve and views** (§4.1, §3.4, §4.6): the curve read at `R_eff` (linear or step) with the
//!   stage-one floor of a pressure; the normal view is that reading, the learning view caps its
//!   stage at `learning_max_stage` with a full share, and stops an instance only past the
//!   emergency line (`learning_backstop_bp` held `pause_sustain_ms`); `allowed` is the ceiling of
//!   the shares' sum over the registered instances, the instances with the larger shares first.
//! - **Effect** (§10.3): `observe` gives S0 and a factor of 1 000; `pace` and `full` give the
//!   curve's factor. #385 P-k: a contained run reads its instance's factor and extra rounds in
//!   effect at each step ([`PacingShadow::effective_pacing`]); nothing here changes a dispatch or
//!   a deadline.
//! - **Records** (§12.1, §12.2): each change of a scope's stage is one `perf_stage_changed`
//!   outcome for `perf.pacing_changed`; `perf.summary` and the status read carry both views.

use crate::codes::{PacingCause, PacingCode, PacingSignal, ProbeKind};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    MAX_PERFORMANCE_PACING_INSTANCES, MAX_PERFORMANCE_STATUS_INSTANCES,
    MAX_PERFORMANCE_STATUS_WARNINGS, PerformanceInstancePacingViews, PerformanceMonitorHealth,
    PerformancePacingMode, PerformancePacingPhase, PerformancePacingStage,
    PerformancePacingSummary, PerformancePressureKind, PerformancePressureSeverity,
    PerformanceStageDwell, PerformanceViewStatus, RuntimePerformanceHostStatus,
    RuntimePerformanceInstanceStatus, RuntimePerformanceStatus,
};
use actingcommand_runtime_config::performance::PerformanceMeasurement;
use actingcommand_runtime_config::performance::pacing::{
    CurvePointSettings, Interpolation, PacingSettings,
};
use actingcommand_runtime_config::performance::responsiveness::{BaselineSettings, SignalSettings};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The highest responsiveness and the value `R_eff` starts at (§12.1).
const FULL_BP: u16 = 10_000;
/// A full dispatch share, in thousandths.
const FULL_SHARE_MILLI: u16 = 1_000;
/// A factor of one.
const UNIT_FACTOR_MILLI: u32 = 1_000;
const HOUR_MS: u64 = 3_600_000;
/// The hours of the status' recent dwell (§4.6 "近 24 h").
const RECENT_HOURS: u64 = 24;
/// The most samples kept between two evaluations, and in one signal's window.
const MAX_PENDING_SAMPLES: usize = 8_192;
const MAX_WINDOW_SAMPLES: usize = 8_192;

/// Where a sample comes from (§3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PacingSampleSource {
    /// A host probe's delay.
    Probe(ProbeKind),
    /// One frame's acquisition on an instance (`capture.completed`).
    CaptureAcquire { instance_alias: String },
    /// One input's backend response on an instance (`input.committed`); `tap` false for a
    /// gesture, whose hold and segments the response includes (§3.2, review L4).
    TouchResponse { instance_alias: String, tap: bool },
}

/// One measured delay, in microseconds, as it was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacingSample {
    pub(crate) observed_at_unix_ms: u64,
    pub(crate) source: PacingSampleSource,
    pub(crate) value_us: u64,
}

/// The backend kinds whose selection names a baseline (§3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PacingBackendRole {
    Capture,
    Touch,
}

/// The curve read at one value (§4.1), with the stage-one floor applied: a share of 0 is S4.
/// #385 P-k: `extra_retries` is the lower neighbour's, under both readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CurveReading {
    stage: PerformancePacingStage,
    factor_milli: u32,
    extra_retries: u32,
    share_milli: u16,
}

/// #385 P-k (PERF v1.4.2 §4.1, §4.4, §5.2, §5.4, §10.3): what each step of an instance's contained
/// runs is paced by, as the shadow measurement's last evaluation left it: the factor in effect
/// and the extra rounds in effect (0 under `observe`; the curve's, pressure floor included, under
/// `pace` and `full`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectivePacing {
    pub(crate) factor_milli: u32,
    pub(crate) extra_wait_rounds: u32,
}

impl EffectivePacing {
    /// The factor 1 000 and no extra round: `observe`, before the first evaluation, without
    /// counters and for an instance without a row.
    pub(crate) const UNPACED: Self = Self {
        factor_milli: UNIT_FACTOR_MILLI,
        extra_wait_rounds: 0,
    };

    /// What `step_pacing()` hands a step (§5.2, §5.4): the factor; the extra wait rounds; the
    /// observe-only retries, the configured base `sample_timeout_base` plus the rounds (§5.4 item
    /// 2); and the pre-input wait `pre_input_wait_base_ms × (f − 1000) / 1000`, zero at 1 000.
    pub(crate) fn step_pacing(
        self,
        sample_timeout_base: u32,
        pre_input_wait_base_ms: u64,
    ) -> actingcommand_execution_kernel::StepPacing {
        // base_ms × (f − 1000) / 1000 ms is exactly base_ms × (f − 1000) µs.
        let pre_input_wait =
            std::time::Duration::from_micros(pre_input_wait_base_ms.saturating_mul(u64::from(
                self.factor_milli.saturating_sub(UNIT_FACTOR_MILLI),
            )));
        actingcommand_execution_kernel::StepPacing::new(sample_timeout_base)
            .with_factor_milli(self.factor_milli)
            .with_extra_wait_rounds(self.extra_wait_rounds)
            .with_pre_input_wait(pre_input_wait)
    }
}

/// One view of a row: its stage and its share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowView {
    stage: PerformancePacingStage,
    share_milli: u16,
}

/// One row as an evaluation computed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowReading {
    reading: CurveReading,
    /// The curve's stage without the pressure floor, to tell a pressure's transition apart.
    curve_stage: PerformancePacingStage,
    learning: RowView,
    normal: RowView,
    effective_stage: PerformancePacingStage,
    effective_factor_milli: u32,
    /// #385 P-k: the extra rounds in effect (§4.4, §10.3): 0 under `observe`, the reading's
    /// under `pace` and `full`. Kept for the steps only; no status, summary or record carries it.
    effective_extra_rounds: u32,
}

/// One instance's row and whether each view lets it take a new run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstanceRow {
    alias: String,
    row: RowReading,
    learning_allowed: bool,
    normal_allowed: bool,
}

/// What one evaluation computed for every row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    host: RowReading,
    instances: Vec<InstanceRow>,
    learning_allowed: u32,
    normal_allowed: u32,
}

/// The time a view spent per stage in one hour, and how often it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HourBucket {
    hour: u64,
    dwell: PerformanceStageDwell,
    stops: u32,
}

/// One view's history (§4.6): the stage it was in at the last evaluation, whether it was
/// stopped, and its dwell and stops in all and per hour.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ViewTrack {
    stage: Option<PerformancePacingStage>,
    stopped: bool,
    total: PerformanceStageDwell,
    stops_total: u32,
    hours: VecDeque<HourBucket>,
}

impl ViewTrack {
    /// Attributes `elapsed_ms` to the stage of the last evaluation, then takes the new stage;
    /// entering a stop counts once.
    fn advance(
        &mut self,
        hour: u64,
        elapsed_ms: u64,
        stage: PerformancePacingStage,
        stopped: bool,
    ) {
        let entered_stop = stopped && !self.stopped;
        if self.hours.back().is_none_or(|bucket| bucket.hour != hour) {
            self.hours.push_back(HourBucket {
                hour,
                dwell: PerformanceStageDwell::default(),
                stops: 0,
            });
            while self
                .hours
                .front()
                .is_some_and(|bucket| bucket.hour.saturating_add(RECENT_HOURS) <= hour)
            {
                self.hours.pop_front();
            }
        }
        if let Some(bucket) = self.hours.back_mut() {
            if let Some(previous) = self.stage {
                bucket.dwell.add(previous, elapsed_ms);
            }
            if entered_stop {
                bucket.stops = bucket.stops.saturating_add(1);
            }
        }
        if let Some(previous) = self.stage {
            self.total.add(previous, elapsed_ms);
        }
        if entered_stop {
            self.stops_total = self.stops_total.saturating_add(1);
        }
        self.stage = Some(stage);
        self.stopped = stopped;
    }

    /// The status of this view now (`view` its stage and share), the last 24 hours up to `hour`
    /// included.
    fn status(&self, hour: u64, view: RowView) -> PerformanceViewStatus {
        let (dwell_24h, stops_24h) = self
            .hours
            .iter()
            .filter(|bucket| bucket.hour.saturating_add(RECENT_HOURS) > hour)
            .fold(
                (PerformanceStageDwell::default(), 0_u32),
                |(dwell, stops), bucket| {
                    (dwell.plus(bucket.dwell), stops.saturating_add(bucket.stops))
                },
            );
        PerformanceViewStatus {
            pacing_stage: view.stage,
            share_milli: view.share_milli,
            dwell_24h,
            dwell_total: self.total,
            stops_24h: stops_24h.min(self.stops_total),
            stops_total: self.stops_total,
        }
    }
}

/// A scope's tracked value and its record (§4.3).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tracker {
    r_eff: u16,
    down_since: Option<u64>,
    up_since: Option<u64>,
    backstop_since: Option<u64>,
    backstop_active: bool,
    /// The stage and curve stage recorded last; `None` before the first evaluation.
    stage: Option<PerformancePacingStage>,
    curve_stage: Option<PerformancePacingStage>,
    learning: ViewTrack,
    normal: ViewTrack,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            r_eff: FULL_BP,
            down_since: None,
            up_since: None,
            backstop_since: None,
            backstop_active: false,
            stage: None,
            curve_stage: None,
            learning: ViewTrack::default(),
            normal: ViewTrack::default(),
        }
    }
}

impl Tracker {
    /// One evaluation's input (§4.3): `None` when nothing is known, which counts as a recovery
    /// sample at 10 000 (the liveness exit of "Recovery without evidence"). Returns whether the
    /// input was that idle recovery.
    fn follow(
        &mut self,
        now: u64,
        input: Option<u16>,
        clock_jump: bool,
        pacing: &PacingSettings,
    ) -> bool {
        if clock_jump {
            self.down_since = None;
            self.up_since = None;
            return false;
        }
        let idle = input.is_none();
        let value = input.unwrap_or(FULL_BP);
        if !idle && value < self.r_eff {
            self.up_since = None;
            let since = *self.down_since.get_or_insert(now);
            if now.saturating_sub(since) >= pacing.degrade_sustain_ms {
                self.r_eff = value;
                self.down_since = None;
            }
        } else if value > self.r_eff
            && u32::from(value) >= u32::from(self.r_eff) + u32::from(pacing.hysteresis_bp)
        {
            self.down_since = None;
            let since = *self.up_since.get_or_insert(now);
            if now.saturating_sub(since) >= pacing.recover_sustain_ms {
                self.r_eff = value.saturating_sub(pacing.hysteresis_bp).max(self.r_eff);
                self.up_since = None;
            }
        } else {
            self.down_since = None;
            self.up_since = None;
        }
        idle
    }

    /// The learning view's emergency line (§4.5.2): held `pause_sustain_ms` at or below
    /// `learning_backstop_bp` (0: never).
    fn follow_backstop(&mut self, now: u64, backstop_bp: u16, sustain_ms: u64) {
        if backstop_bp > 0 && self.r_eff <= backstop_bp {
            let since = *self.backstop_since.get_or_insert(now);
            self.backstop_active = now.saturating_sub(since) >= sustain_ms;
        } else {
            self.backstop_since = None;
            self.backstop_active = false;
        }
    }
}

/// One baseline (§3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Baseline {
    initial_us: u64,
    calm: VecDeque<u64>,
    value_us: u64,
    /// The bound the learned value is held at, when it is held.
    held_at_us: Option<u64>,
}

impl Baseline {
    fn new(initial_us: u64) -> Self {
        Self {
            initial_us,
            calm: VecDeque::new(),
            value_us: initial_us,
            held_at_us: None,
        }
    }

    /// Takes one calm sample: the configured quantile of the last `window_samples` once there
    /// are `min_samples`, held within the clamp around the initial value.
    fn observe_calm(&mut self, value_us: u64, settings: &BaselineSettings) {
        self.calm.push_back(value_us);
        while self.calm.len() > settings.window_samples.max(1) {
            self.calm.pop_front();
        }
        if self.calm.len() < settings.min_samples.max(1) {
            self.value_us = self.initial_us;
            self.held_at_us = None;
            return;
        }
        let mut sorted = self.calm.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        let rank = (sorted.len() * usize::from(settings.quantile.min(100))).div_ceil(100);
        let learned = sorted
            .get(rank.saturating_sub(1))
            .or_else(|| sorted.first())
            .copied()
            .unwrap_or(self.initial_us);
        let floor = scaled(self.initial_us, settings.clamp_min_milli);
        let ceiling = scaled(self.initial_us, settings.clamp_max_milli).max(floor);
        if learned < floor {
            self.value_us = floor;
            self.held_at_us = Some(floor);
        } else if learned > ceiling {
            self.value_us = ceiling;
            self.held_at_us = Some(ceiling);
        } else {
            self.value_us = learned;
            self.held_at_us = None;
        }
    }
}

/// `value × milli / 1000`, without overflow.
fn scaled(value: u64, milli: u64) -> u64 {
    u64::try_from(u128::from(value) * u128::from(milli) / 1_000).unwrap_or(u64::MAX)
}

/// The score of one sample against `baseline_us` (§3.3), in basis points.
fn score(value_us: u64, baseline_us: u64, signal: &SignalSettings) -> u16 {
    let allowance = scaled(baseline_us, signal.allowance_milli).saturating_add(signal.slack_us);
    if value_us <= allowance {
        return FULL_BP;
    }
    let ratio = u128::from(allowance) * u128::from(FULL_BP) / u128::from(value_us.max(1));
    u16::try_from(ratio).unwrap_or(FULL_BP).min(FULL_BP)
}

/// One sample as a scope takes it: its signal, the backend word its baseline is kept for, its
/// time and value.
struct ScopedSample {
    signal: PacingSignal,
    backend: Option<String>,
    observed_at_unix_ms: u64,
    value_us: u64,
}

/// One scope's signals: their windows and their baselines per backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Scope {
    windows: BTreeMap<PacingSignal, VecDeque<(u64, u16)>>,
    baselines: BTreeMap<(PacingSignal, Option<String>), Baseline>,
    /// The scope's own smoothed responsiveness (`R_host` or `R_emu(i)`).
    smoothed: Option<u16>,
    last_sample_unix_ms: Option<u64>,
    tracker: Tracker,
}

impl Scope {
    /// Scores one sample and keeps it; a calm sample also teaches its baseline (§3.5).
    fn take(
        &mut self,
        sample: ScopedSample,
        settings: &SignalSettings,
        baseline_settings: &BaselineSettings,
        calm: bool,
    ) {
        let initial = settings.initial_baseline_us(sample.backend.as_deref());
        let baseline = self
            .baselines
            .entry((sample.signal, sample.backend))
            .or_insert_with(|| Baseline::new(initial));
        let window = self.windows.entry(sample.signal).or_default();
        window.push_back((
            sample.observed_at_unix_ms,
            score(sample.value_us, baseline.value_us, settings),
        ));
        while window.len() > MAX_WINDOW_SAMPLES {
            window.pop_front();
        }
        if calm {
            baseline.observe_calm(sample.value_us, baseline_settings);
        }
        self.last_sample_unix_ms = Some(
            self.last_sample_unix_ms
                .map_or(sample.observed_at_unix_ms, |last| {
                    last.max(sample.observed_at_unix_ms)
                }),
        );
    }

    /// The lowest known signal of the scope now (§3.3): each signal's mean score over its
    /// window, known from `min_samples` samples.
    fn raw(&mut self, now: u64, window_ms: u64, min_samples: usize) -> Option<u16> {
        let oldest = now.saturating_sub(window_ms);
        let mut lowest: Option<u16> = None;
        for window in self.windows.values_mut() {
            while window.front().is_some_and(|(at, _)| *at < oldest) {
                window.pop_front();
            }
            if window.is_empty() || window.len() < min_samples.max(1) {
                continue;
            }
            let sum = window
                .iter()
                .map(|(_, score)| u64::from(*score))
                .sum::<u64>();
            let count = u64::try_from(window.len()).unwrap_or(u64::MAX);
            let mean = u16::try_from(sum / count).unwrap_or(FULL_BP).min(FULL_BP);
            lowest = Some(lowest.map_or(mean, |value| value.min(mean)));
        }
        lowest
    }

    /// Smooths this evaluation's raw value (§3.3): a known value by the EWMA; an unknown one
    /// keeps the last value until no sample came for `stale_ms`.
    fn smooth(&mut self, now: u64, raw: Option<u16>, alpha_milli: u32, stale_ms: u64) {
        let fresh = self
            .last_sample_unix_ms
            .is_some_and(|last| now.saturating_sub(last) <= stale_ms);
        self.smoothed = match (raw, self.smoothed) {
            (Some(raw), None) => Some(raw),
            (Some(raw), Some(previous)) => {
                let alpha = u64::from(alpha_milli.clamp(1, 1_000));
                let mixed =
                    (alpha * u64::from(raw) + (1_000 - alpha) * u64::from(previous) + 500) / 1_000;
                Some(u16::try_from(mixed).unwrap_or(FULL_BP).min(FULL_BP))
            }
            (None, previous) => previous.filter(|_| fresh),
        };
    }

    /// The baselines held at a bound, as `perf_baseline_clamped` warnings.
    fn held_baselines(&self, instance_alias: Option<&str>) -> Vec<Outcome> {
        self.baselines
            .iter()
            .filter_map(|((signal, _), baseline)| {
                let limit = baseline.held_at_us?;
                let outcome = Outcome::new(PacingCode::BaselineClamped)
                    .signal_kind(*signal)
                    .baseline_us(baseline.value_us)
                    .limit_us(limit);
                Some(match instance_alias {
                    Some(alias) => outcome.instance_id(alias),
                    None => outcome,
                })
            })
            .collect()
    }
}

/// The backends an instance's last opens selected (§3.5 "键：(实例, 信号, 后端)").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct InstanceBackends {
    capture: Option<String>,
    touch: Option<String>,
}

/// The shadow measurement of one Runtime (§3, §4, §4.6). The host feeds it samples as they are
/// observed and evaluates it once per performance tick; it acts on nothing.
#[derive(Debug, Clone, Default)]
pub(crate) struct PacingShadow {
    pending: VecDeque<PacingSample>,
    host: Scope,
    instances: BTreeMap<String, Scope>,
    backends: BTreeMap<String, InstanceBackends>,
    probe_failures: BTreeMap<ProbeKind, Outcome>,
    pressure: Option<PerformancePressureSeverity>,
    last_evaluation_unix_ms: Option<u64>,
    /// The settings of the last evaluation and what it computed; `perf.summary` reads them.
    evaluated: Option<(PerformanceMeasurement, Snapshot)>,
}

impl PacingShadow {
    /// Takes one sample for the next evaluation; the oldest goes when too many wait.
    pub(crate) fn record(&mut self, sample: PacingSample) {
        self.pending.push_back(sample);
        while self.pending.len() > MAX_PENDING_SAMPLES {
            self.pending.pop_front();
        }
    }

    /// The backend an instance's open selected (`selected` as the backend open report names
    /// it); the instance's signals keep their baselines for that backend's word.
    pub(crate) fn note_backend(
        &mut self,
        instance_alias: &str,
        role: PacingBackendRole,
        selected: &str,
    ) {
        let word = backend_word(selected);
        let backends = self.backends.entry(instance_alias.to_owned()).or_default();
        match role {
            PacingBackendRole::Capture => backends.capture = Some(word),
            PacingBackendRole::Touch => backends.touch = Some(word),
        }
    }

    /// A host probe that could not run (`outcome`: `perf_probe_unavailable`): its signal stays
    /// unknown and the status carries the warning; every other signal goes on.
    pub(crate) fn probe_unavailable(&mut self, probe: ProbeKind, outcome: Outcome) {
        self.probe_failures.insert(probe, outcome);
    }

    /// Test builds only: the baseline `signal` of the host (`None`) or of an instance holds now,
    /// whatever backend it is kept for.
    #[cfg(test)]
    pub(crate) fn baseline_us(
        &self,
        instance_alias: Option<&str>,
        signal: PacingSignal,
    ) -> Option<u64> {
        let scope = match instance_alias {
            None => &self.host,
            Some(alias) => self.instances.get(alias)?,
        };
        scope
            .baselines
            .iter()
            .find(|((kept, _), _)| *kept == signal)
            .map(|(_, baseline)| baseline.value_us)
    }

    /// One evaluation at `now` (§3.3, §4.3, §4.6) over the registered `instances`, with the
    /// highest severity among the pressure floor's kinds in `pressure`. Returns the stage
    /// transitions as `perf_stage_changed` outcomes, the host's first, then the instances' by
    /// alias.
    pub(crate) fn evaluate(
        &mut self,
        now: u64,
        settings: &PerformanceMeasurement,
        instances: &[String],
        pressure: Option<PerformancePressureSeverity>,
    ) -> Vec<Outcome> {
        let pacing = &settings.pacing;
        let clock_jump = self
            .last_evaluation_unix_ms
            .is_some_and(|last| now < last || now.saturating_sub(last) > pacing.clock_jump_ms);
        let elapsed_ms = match self.last_evaluation_unix_ms {
            Some(last) if !clock_jump => now.saturating_sub(last),
            _ => 0,
        };
        self.last_evaluation_unix_ms = Some(now);
        self.pressure = pressure.filter(|severity| *severity >= pacing.pressure_floor.min_severity);
        let registered = instances.iter().cloned().collect::<BTreeSet<_>>();
        self.instances.retain(|alias, _| registered.contains(alias));
        self.backends.retain(|alias, _| registered.contains(alias));
        for alias in &registered {
            self.instances.entry(alias.clone()).or_default();
        }
        self.take_pending(settings);

        let responsiveness = &settings.responsiveness;
        let host_raw = self.host.raw(
            now,
            responsiveness.window_ms,
            responsiveness.min_window_samples,
        );
        self.host.smooth(
            now,
            host_raw,
            responsiveness.ewma_alpha_milli,
            responsiveness.emu_stale_ms,
        );
        let host_smoothed = self.host.smoothed;
        let host_idle = self
            .host
            .tracker
            .follow(now, host_smoothed, clock_jump, pacing);
        let mut idle = BTreeMap::new();
        for (alias, scope) in &mut self.instances {
            let raw = scope.raw(
                now,
                responsiveness.window_ms,
                responsiveness.min_window_samples,
            );
            scope.smooth(
                now,
                raw,
                responsiveness.ewma_alpha_milli,
                responsiveness.emu_stale_ms,
            );
            let input = match (host_smoothed, scope.smoothed) {
                (Some(host), Some(emulator)) => Some(host.min(emulator)),
                (host, emulator) => host.or(emulator),
            };
            idle.insert(
                alias.clone(),
                scope.tracker.follow(now, input, clock_jump, pacing),
            );
        }
        let backstop_bp = settings.learning.learning_backstop_bp;
        self.host
            .tracker
            .follow_backstop(now, backstop_bp, pacing.pause_sustain_ms);
        for scope in self.instances.values_mut() {
            scope
                .tracker
                .follow_backstop(now, backstop_bp, pacing.pause_sustain_ms);
        }

        let snapshot = self.snapshot(settings);
        let hour = now / HOUR_MS;
        let mut transitions = Vec::new();
        let host_row = snapshot.host;
        transitions.extend(transition(
            &mut self.host.tracker,
            &host_row,
            settings,
            host_idle,
            None,
            host_smoothed,
            None,
        ));
        // The host's view stops when it lets no instance take a new run.
        let any_instance = !snapshot.instances.is_empty();
        self.host.tracker.learning.advance(
            hour,
            elapsed_ms,
            host_row.learning.stage,
            any_instance && snapshot.learning_allowed == 0,
        );
        self.host.tracker.normal.advance(
            hour,
            elapsed_ms,
            host_row.normal.stage,
            any_instance && snapshot.normal_allowed == 0,
        );
        for entry in &snapshot.instances {
            let Some(scope) = self.instances.get_mut(&entry.alias) else {
                continue;
            };
            let emulator_smoothed = scope.smoothed;
            transitions.extend(transition(
                &mut scope.tracker,
                &entry.row,
                settings,
                idle.get(&entry.alias).copied().unwrap_or(false),
                Some(entry.alias.as_str()),
                host_smoothed,
                emulator_smoothed,
            ));
            scope.tracker.learning.advance(
                hour,
                elapsed_ms,
                entry.row.learning.stage,
                entry.row.learning.share_milli == 0,
            );
            scope.tracker.normal.advance(
                hour,
                elapsed_ms,
                entry.row.normal.stage,
                entry.row.normal.share_milli == 0,
            );
        }
        self.evaluated = Some((settings.clone(), snapshot));
        transitions
    }

    /// Scores the samples that arrived since the last evaluation, in their order.
    fn take_pending(&mut self, settings: &PerformanceMeasurement) {
        let responsiveness = &settings.responsiveness;
        let floor_free = self.pressure.is_none();
        while let Some(sample) = self.pending.pop_front() {
            let (signal, signal_settings, alias, backend) = match &sample.source {
                PacingSampleSource::Probe(ProbeKind::Timer) => (
                    PacingSignal::Timer,
                    &responsiveness.timer.signal,
                    None,
                    None,
                ),
                PacingSampleSource::Probe(ProbeKind::Message) => (
                    PacingSignal::Message,
                    &responsiveness.message.signal,
                    None,
                    None,
                ),
                PacingSampleSource::CaptureAcquire { instance_alias } => (
                    PacingSignal::CaptureAcquire,
                    &responsiveness.capture_acquire,
                    Some(instance_alias.as_str()),
                    self.backends
                        .get(instance_alias)
                        .and_then(|backends| backends.capture.clone()),
                ),
                PacingSampleSource::TouchResponse {
                    instance_alias,
                    tap,
                } => {
                    if !*tap && !responsiveness.score_gestures {
                        // A gesture's response includes its hold and segments: it stays in the
                        // context only, never scored (§3.2, review L4).
                        continue;
                    }
                    (
                        PacingSignal::TouchResponse,
                        &responsiveness.touch_response,
                        Some(instance_alias.as_str()),
                        self.backends
                            .get(instance_alias)
                            .and_then(|backends| backends.touch.clone()),
                    )
                }
            };
            if !signal_settings.enabled {
                continue;
            }
            let scope = match alias {
                None => &mut self.host,
                Some(alias) => match self.instances.get_mut(alias) {
                    Some(scope) => scope,
                    // An instance the registry no longer holds.
                    None => continue,
                },
            };
            // A calm sample (§3.5): its scope at S0 and no pressure of the floor.
            let calm = floor_free
                && scope
                    .tracker
                    .stage
                    .is_none_or(|stage| stage == PerformancePacingStage::S0);
            scope.take(
                ScopedSample {
                    signal,
                    backend,
                    observed_at_unix_ms: sample.observed_at_unix_ms,
                    value_us: sample.value_us,
                },
                signal_settings,
                &responsiveness.baseline,
                calm,
            );
        }
    }

    /// Every row's curve reading and views at the trackers' values (§4.1, §4.6).
    fn snapshot(&self, settings: &PerformanceMeasurement) -> Snapshot {
        let host = row_reading(
            settings,
            self.host.tracker.r_eff,
            self.host.tracker.backstop_active,
            self.pressure,
        );
        let mut instances = self
            .instances
            .iter()
            .map(|(alias, scope)| InstanceRow {
                alias: alias.clone(),
                row: row_reading(
                    settings,
                    scope.tracker.r_eff,
                    scope.tracker.backstop_active,
                    self.pressure,
                ),
                learning_allowed: false,
                normal_allowed: false,
            })
            .collect::<Vec<_>>();
        let (learning_allowed, learning_flags) = allowed(
            &instances
                .iter()
                .map(|entry| (entry.alias.as_str(), entry.row.learning.share_milli))
                .collect::<Vec<_>>(),
        );
        let (normal_allowed, normal_flags) = allowed(
            &instances
                .iter()
                .map(|entry| (entry.alias.as_str(), entry.row.normal.share_milli))
                .collect::<Vec<_>>(),
        );
        for ((entry, learning), normal) in
            instances.iter_mut().zip(learning_flags).zip(normal_flags)
        {
            entry.learning_allowed = learning;
            entry.normal_allowed = normal;
        }
        Snapshot {
            host,
            instances,
            learning_allowed,
            normal_allowed,
        }
    }

    /// #385 P-k (PERF v1.4.2 §4.6, §10.3): the factor in effect for `instance_alias` at the last
    /// evaluation: 1 000 under `observe`, the curve's factor (pressure floor included) under
    /// `pace` and `full`; 1 000 before the first evaluation and for an instance without a row.
    /// It clones nothing but the number. Test builds only: a step reads [`Self::effective_pacing`].
    #[cfg(test)]
    pub(crate) fn effective_factor_milli(&self, instance_alias: &str) -> u32 {
        self.effective_pacing(instance_alias).factor_milli
    }

    /// #385 P-k (PERF v1.4.2 §4.1, §4.4, §10.3): the factor and the extra rounds in effect for
    /// `instance_alias` at the last evaluation: 1 000 and 0 under `observe`; under `pace` and
    /// `full` the curve's factor and the lower neighbour's `extra_retries`, each at least the
    /// pressure floor's while a pressure of a floor kind at or above `min_severity` is active;
    /// [`EffectivePacing::UNPACED`] before the first evaluation and for an instance without a
    /// row. It clones nothing but the two numbers.
    pub(crate) fn effective_pacing(&self, instance_alias: &str) -> EffectivePacing {
        self.evaluated
            .as_ref()
            .and_then(|(_, snapshot)| {
                snapshot
                    .instances
                    .iter()
                    .find(|entry| entry.alias == instance_alias)
            })
            .map_or(EffectivePacing::UNPACED, |entry| EffectivePacing {
                factor_milli: entry.row.effective_factor_milli,
                extra_wait_rounds: entry.row.effective_extra_rounds,
            })
    }

    /// The pacing members of `perf.summary` (register P4) from the last evaluation; none before
    /// it.
    pub(crate) fn summary(&self) -> Option<PerformancePacingSummary> {
        let (settings, snapshot) = self.evaluated.as_ref()?;
        let host = &snapshot.host;
        let mode = settings.pacing.mode;
        Some(PerformancePacingSummary {
            responsiveness_bp: self.host.tracker.r_eff,
            host_responsiveness_bp: self.host.smoothed,
            emulator_responsiveness_bp: self
                .instances
                .values()
                .filter_map(|scope| scope.smoothed)
                .min(),
            pacing_stage: host.reading.stage,
            factor_milli: host.reading.factor_milli,
            dispatch_share_milli: host.normal.share_milli,
            pacing_mode: mode,
            learning: mode == PerformancePacingMode::Full && settings.learning_cap(),
            learning_view_stage: host.learning.stage,
            learning_view_share_milli: host.learning.share_milli,
            learning_view_allowed: snapshot.learning_allowed,
            normal_view_stage: host.normal.stage,
            normal_view_share_milli: host.normal.share_milli,
            normal_view_allowed: snapshot.normal_allowed,
            pacing_phase: phase(settings),
            learn_days: None,
            learn_active_ms: None,
            instances: snapshot
                .instances
                .iter()
                .take(MAX_PERFORMANCE_PACING_INSTANCES)
                .map(|entry| PerformanceInstancePacingViews {
                    instance_id: entry.alias.clone(),
                    learning_view_stage: entry.row.learning.stage,
                    learning_view_share_milli: entry.row.learning.share_milli,
                    learning_view_allowed: entry.learning_allowed,
                    normal_view_stage: entry.row.normal.stage,
                    normal_view_share_milli: entry.row.normal.share_milli,
                    normal_view_allowed: entry.normal_allowed,
                })
                .collect(),
        })
    }

    /// The status read (§4.6) at `now`: the views of the last evaluation, or before the first
    /// one those of every registered instance at its start under `settings`; `health` is the
    /// monitor's.
    pub(crate) fn status(
        &self,
        now: u64,
        settings: &PerformanceMeasurement,
        instances: &[String],
        health: PerformanceMonitorHealth,
    ) -> RuntimePerformanceStatus {
        let snapshot = match &self.evaluated {
            Some((_, snapshot)) => snapshot.clone(),
            None => {
                let mut shadow = self.clone();
                for alias in instances {
                    shadow.instances.entry(alias.clone()).or_default();
                }
                shadow.snapshot(settings)
            }
        };
        let hour = now / HOUR_MS;
        let host = &snapshot.host;
        let mode = settings.pacing.mode;
        let host_status = RuntimePerformanceHostStatus {
            host_responsiveness_bp: self.host.smoothed,
            responsiveness_bp: self.host.tracker.r_eff,
            pacing_stage: host.reading.stage,
            factor_milli: host.reading.factor_milli,
            learning_view: self.host.tracker.learning.status(hour, host.learning),
            learning_view_allowed: snapshot.learning_allowed,
            normal_view: self.host.tracker.normal.status(hour, host.normal),
            normal_view_allowed: snapshot.normal_allowed,
            effective_stage: host.effective_stage,
            effective_factor_milli: host.effective_factor_milli,
        };
        let empty = Scope::default();
        let rows = snapshot
            .instances
            .iter()
            .take(MAX_PERFORMANCE_STATUS_INSTANCES)
            .map(|entry| {
                let scope = self.instances.get(&entry.alias).unwrap_or(&empty);
                RuntimePerformanceInstanceStatus {
                    instance_alias: entry.alias.clone(),
                    emulator_responsiveness_bp: scope.smoothed,
                    responsiveness_bp: scope.tracker.r_eff,
                    pacing_stage: entry.row.reading.stage,
                    factor_milli: entry.row.reading.factor_milli,
                    learning_view: scope.tracker.learning.status(hour, entry.row.learning),
                    learning_view_allowed: entry.learning_allowed,
                    normal_view: scope.tracker.normal.status(hour, entry.row.normal),
                    normal_view_allowed: entry.normal_allowed,
                    effective_stage: entry.row.effective_stage,
                    effective_factor_milli: entry.row.effective_factor_milli,
                }
            })
            .collect::<Vec<_>>();
        let mut warnings = self.probe_failures.values().cloned().collect::<Vec<_>>();
        warnings.extend(self.host.held_baselines(None));
        for (alias, scope) in &self.instances {
            warnings.extend(scope.held_baselines(Some(alias.as_str())));
        }
        warnings.truncate(MAX_PERFORMANCE_STATUS_WARNINGS);
        RuntimePerformanceStatus {
            observed_at_unix_ms: now,
            pacing_mode: mode,
            pacing_phase: phase(settings),
            learning: mode == PerformancePacingMode::Full && settings.learning_cap(),
            monitor_health: health,
            host: host_status,
            instances: rows,
            warnings,
        }
    }
}

/// `full`'s phase (§4.6): the learning phase while the learning view caps; P-a ends it.
fn phase(settings: &PerformanceMeasurement) -> Option<PerformancePacingPhase> {
    (settings.pacing.mode == PerformancePacingMode::Full).then(|| {
        if settings.learning_cap() {
            PerformancePacingPhase::Learning
        } else {
            PerformancePacingPhase::Normal
        }
    })
}

/// The transition of one scope's stage at this evaluation (§12.1), as `perf_stage_changed`;
/// the first evaluation records its stage without one.
fn transition(
    tracker: &mut Tracker,
    row: &RowReading,
    settings: &PerformanceMeasurement,
    idle: bool,
    instance_alias: Option<&str>,
    host_smoothed: Option<u16>,
    emulator_smoothed: Option<u16>,
) -> Option<Outcome> {
    let previous = tracker.stage.replace(row.reading.stage);
    let previous_curve = tracker.curve_stage.replace(row.curve_stage);
    let from = previous?;
    if from == row.reading.stage {
        return None;
    }
    let cause = if previous_curve == Some(row.curve_stage) {
        PacingCause::Pressure
    } else if row.reading.stage < from {
        if idle {
            PacingCause::IdleRecovery
        } else {
            PacingCause::Recovery
        }
    } else {
        PacingCause::Responsiveness
    };
    let mode = settings.pacing.mode;
    let mut outcome = Outcome::new(PacingCode::StageChanged)
        .pacing_stage_from(from)
        .pacing_stage_to(row.reading.stage)
        .responsiveness_bp(tracker.r_eff)
        .factor_milli(row.reading.factor_milli)
        .pacing_cause(cause)
        .pacing_mode(mode);
    if let Some(alias) = instance_alias {
        outcome = outcome.instance_id(alias);
    }
    if let Some(host) = host_smoothed {
        outcome = outcome.host_responsiveness_bp(host);
    }
    if let Some(emulator) = emulator_smoothed {
        outcome = outcome.emulator_responsiveness_bp(emulator);
    }
    if mode == PerformancePacingMode::Full
        && settings.learning_cap()
        && row.learning.stage < row.normal.stage
    {
        outcome = outcome.pacing_learning_capped(true);
    }
    Some(outcome)
}

/// How many instances one view lets take a new run (§6.1, §6.2), and which: `allowed` is the
/// ceiling of the shares' sum, given to the larger shares first, then by alias.
fn allowed(shares: &[(&str, u16)]) -> (u32, Vec<bool>) {
    let sum = shares
        .iter()
        .map(|(_, share)| u64::from(*share))
        .sum::<u64>();
    let allowed = sum.div_ceil(u64::from(FULL_SHARE_MILLI));
    let mut order = (0..shares.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| {
        shares[*right]
            .1
            .cmp(&shares[*left].1)
            .then_with(|| shares[*left].0.cmp(shares[*right].0))
    });
    let mut flags = vec![false; shares.len()];
    for (rank, index) in order.into_iter().enumerate() {
        flags[index] = u64::try_from(rank).is_ok_and(|rank| rank < allowed);
    }
    (u32::try_from(allowed).unwrap_or(u32::MAX), flags)
}

/// One row's reading at `r_eff` (§4.1, §3.4, §4.5.2, §4.6, §10.3).
fn row_reading(
    settings: &PerformanceMeasurement,
    r_eff: u16,
    backstop_active: bool,
    pressure: Option<PerformancePressureSeverity>,
) -> RowReading {
    let pacing = &settings.pacing;
    let curve = read_curve(&pacing.curve, pacing.interpolate, r_eff);
    let mut reading = curve;
    if pressure.is_some() {
        // The stage-one floor (§3.4): a pressure never lowers the share. #385 P-k: nor the extra
        // rounds below the floor's.
        reading.stage = reading.stage.max(PerformancePacingStage::S1);
        reading.factor_milli = reading.factor_milli.max(pacing.pressure_floor.factor_milli);
        reading.extra_retries = reading
            .extra_retries
            .max(pacing.pressure_floor.extra_retries);
    }
    let normal = RowView {
        stage: reading.stage,
        share_milli: reading.share_milli,
    };
    let learning_max = settings.learning.learning_max_stage;
    let learning = if !settings.learning_cap() || learning_max == PerformancePacingStage::S4 {
        normal
    } else if backstop_active {
        RowView {
            stage: PerformancePacingStage::S4,
            share_milli: 0,
        }
    } else {
        RowView {
            stage: reading.stage.min(learning_max),
            share_milli: FULL_SHARE_MILLI,
        }
    };
    let (effective_stage, effective_factor_milli) = match pacing.mode {
        PerformancePacingMode::Observe => (PerformancePacingStage::S0, UNIT_FACTOR_MILLI),
        PerformancePacingMode::Pace => (reading.stage, reading.factor_milli),
        PerformancePacingMode::Full if settings.learning_cap() => {
            (learning.stage, reading.factor_milli)
        }
        PerformancePacingMode::Full => (reading.stage, reading.factor_milli),
    };
    // #385 P-k (§4.4, §4.5.2 "放慢不封顶", §10.3): the extra rounds follow the mode as the factor
    // does; the learning view caps neither.
    let effective_extra_rounds = match pacing.mode {
        PerformancePacingMode::Observe => 0,
        PerformancePacingMode::Pace | PerformancePacingMode::Full => reading.extra_retries,
    };
    RowReading {
        reading,
        curve_stage: curve.stage,
        learning,
        normal,
        effective_stage,
        effective_factor_milli,
        effective_extra_rounds,
    }
}

/// The curve read at `r` (§4.1): between the neighbours `p_k.r_bp > r >= p_{k+1}.r_bp` (the
/// first segment at 10 000) the stage and (#385 P-k) the extra retries are `p_{k+1}`'s under
/// both readings; the factor and the share lie between the two (`linear`) or are `p_{k+1}`'s
/// (`step`); a share of 0 is S4.
fn read_curve(curve: &[CurvePointSettings], interpolate: Interpolation, r: u16) -> CurveReading {
    let index = curve
        .windows(2)
        .position(|pair| pair[0].r_bp > r && r >= pair[1].r_bp)
        .unwrap_or(0);
    let (Some(higher), Some(lower)) = (curve.get(index), curve.get(index + 1)) else {
        // The configuration's check refuses a curve of fewer than two points.
        return CurveReading {
            stage: PerformancePacingStage::S0,
            factor_milli: UNIT_FACTOR_MILLI,
            extra_retries: 0,
            share_milli: FULL_SHARE_MILLI,
        };
    };
    let (factor_milli, share_milli) = match interpolate {
        Interpolation::Step => (lower.factor_milli, lower.share_milli),
        Interpolation::Linear => {
            let span = i64::from(higher.r_bp) - i64::from(lower.r_bp);
            let along = (i64::from(higher.r_bp) - i64::from(r)).clamp(0, span.max(0));
            let between = |high: i64, low: i64| {
                if span <= 0 {
                    low
                } else {
                    high + (low - high) * along / span
                }
            };
            (
                u32::try_from(between(
                    i64::from(higher.factor_milli),
                    i64::from(lower.factor_milli),
                ))
                .unwrap_or(lower.factor_milli),
                u16::try_from(between(
                    i64::from(higher.share_milli),
                    i64::from(lower.share_milli),
                ))
                .unwrap_or(lower.share_milli),
            )
        }
    };
    CurveReading {
        stage: if share_milli == 0 {
            PerformancePacingStage::S4
        } else {
            lower.stage
        },
        factor_milli: factor_milli.max(UNIT_FACTOR_MILLI),
        extra_retries: lower.extra_retries,
        share_milli: share_milli.min(FULL_SHARE_MILLI),
    }
}

/// The configuration word of a selected backend (§10.2 `initial_baseline_us` members): every
/// screencap reading of adb is `adb`; other names are their own word.
fn backend_word(selected: &str) -> String {
    if selected.starts_with("adb") {
        "adb".to_owned()
    } else {
        selected.to_owned()
    }
}

/// The highest severity among the active pressures whose kind the floor names (§3.4).
pub(crate) fn floor_pressure(
    active: impl IntoIterator<Item = (PerformancePressureKind, PerformancePressureSeverity)>,
    kinds: &[PerformancePressureKind],
) -> Option<PerformancePressureSeverity> {
    active
        .into_iter()
        .filter(|(kind, _)| kinds.contains(kind))
        .map(|(_, severity)| severity)
        .max()
}
