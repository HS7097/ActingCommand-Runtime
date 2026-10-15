// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #385 P-m, the shadow measurement (frozen model PERF v1.4.2 §3, §4, §4.6, §10.3; test
// rows Tm-1 to Tm-5, Tm-7, Tm-8, Tm-11, Tm-12 and Tc-3 of §14). Each expected value is the
// model's: a sample scores 10 000 at or below `baseline × allowance_milli / 1000 + slack_us`,
// else `10 000 × allowance / sample`; a signal is the mean score of its window; the tracked
// value follows a lower one after `degrade_sustain_ms`; the curve is read at it (§4.1); the
// learning view caps the stage at `learning_max_stage` with a full share (§4.5.2); `observe`
// gives S0 and a factor of 1 000 (§10.3). The settings are the shipped defaults (§10), the
// curve §4.2's. Times are fixed Unix milliseconds; nothing here sleeps or reads a clock.
//
// With the shipped defaults the allowance of a frame acquisition on a backend the
// configuration does not name is 15 000 × 3 + 10 000 = 55 000 us, of a tap 50 000 × 3 +
// 30 000 = 180 000 us, of the timer probe 16 000 × 2 + 10 000 = 42 000 us; the baseline clamp
// is [initial × 0.25, initial × 2]; fewer than 50 calm samples keep the initial baseline.
//
// The status read itself (Tm-12) runs through a host started without a performance monitor:
// nothing else writes, so the ledger's committed head shows that the read appended nothing.

use super::gate_383_ao4q::ledger_head;
use super::*;
use crate::codes::{PacingCode, PacingSignal, ProbeKind};
use crate::performance::{PacingSample, PacingSampleSource, PacingShadow};
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    PerformancePacingMode, PerformancePacingStage, PerformancePressureSeverity,
    RuntimePerformanceStatus,
};
use actingcommand_runtime_config::EffectiveConfig;
use actingcommand_runtime_config::performance::PerformanceMeasurement;
use actingcommand_runtime_config::performance::pacing::CurvePointSettings;
use serde_json::json;

const T0: u64 = 1_760_000_000_000;
// One degrade sustain (§4.3 default 4 s) after T0.
const SUSTAINED: u64 = T0 + 4_000;
const A: &str = "neutral.instance.a";
const B: &str = "neutral.instance.b";
const C: &str = "neutral.instance.c";
// A frame acquisition ten times the initial baseline: 55 000 / 150 000 scores 3 666.
const SLOW_CAPTURE_US: u64 = 150_000;
const SLOW_CAPTURE_BP: u16 = 3_666;
// A frame acquisition within the allowance: it scores 10 000.
const QUICK_CAPTURE_US: u64 = 9_000;

fn settings() -> PerformanceMeasurement {
    EffectiveConfig::default()
        .performance_measurement()
        .expect("the shipped performance section")
}

fn aliases(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn capture(alias: &str, at: u64, value_us: u64) -> PacingSample {
    PacingSample {
        observed_at_unix_ms: at,
        source: PacingSampleSource::CaptureAcquire {
            instance_alias: alias.to_owned(),
        },
        value_us,
    }
}

fn touch(alias: &str, at: u64, value_us: u64, tap: bool) -> PacingSample {
    PacingSample {
        observed_at_unix_ms: at,
        source: PacingSampleSource::TouchResponse {
            instance_alias: alias.to_owned(),
            tap,
        },
        value_us,
    }
}

fn timer(at: u64, value_us: u64) -> PacingSample {
    PacingSample {
        observed_at_unix_ms: at,
        source: PacingSampleSource::Probe(ProbeKind::Timer),
        value_us,
    }
}

// `count` samples of `value_us` for `alias`, one per millisecond before T0.
fn captures(shadow: &mut PacingShadow, alias: &str, count: u64, value_us: u64) {
    for index in 0..count {
        shadow.record(capture(alias, T0 - 1_000 + index, value_us));
    }
}

// Two evaluations, one degrade sustain apart: the tracked values have followed the samples.
fn settle(
    shadow: &mut PacingShadow,
    settings: &PerformanceMeasurement,
    instances: &[String],
) -> Vec<Outcome> {
    let mut transitions = shadow.evaluate(T0, settings, instances, None);
    transitions.extend(shadow.evaluate(SUSTAINED, settings, instances, None));
    transitions
}

fn read_status(
    shadow: &PacingShadow,
    settings: &PerformanceMeasurement,
    instances: &[String],
) -> RuntimePerformanceStatus {
    let status = shadow.status(
        SUSTAINED,
        settings,
        instances,
        PerformanceMonitorHealth::Healthy,
    );
    status.validate().expect("a valid status");
    status
}

fn row<'a>(
    status: &'a RuntimePerformanceStatus,
    alias: &str,
) -> &'a actingcommand_contract::RuntimePerformanceInstanceStatus {
    status
        .instances
        .iter()
        .find(|row| row.instance_alias == alias)
        .unwrap_or_else(|| panic!("no row of {alias}: {status:?}"))
}

fn outcome_json(outcome: &Outcome) -> serde_json::Value {
    serde_json::to_value(outcome).expect("outcome json")
}

// Tm-1 (§3.3): instance A's frames take ten times the baseline, B's are quick. R(A) falls to
// A's own score; B stays at 10 000; A's value never reaches B.
#[test]
fn tm_1_a_slow_instance_lowers_only_its_own_responsiveness() {
    let settings = settings();
    let instances = aliases(&[A, B]);
    let mut shadow = PacingShadow::default();
    captures(&mut shadow, A, 10, SLOW_CAPTURE_US);
    captures(&mut shadow, B, 10, QUICK_CAPTURE_US);
    settle(&mut shadow, &settings, &instances);
    let status = read_status(&shadow, &settings, &instances);
    let (a, b) = (row(&status, A), row(&status, B));
    assert_eq!(
        (a.emulator_responsiveness_bp, a.responsiveness_bp),
        (Some(SLOW_CAPTURE_BP), SLOW_CAPTURE_BP)
    );
    assert_eq!(
        (b.emulator_responsiveness_bp, b.responsiveness_bp),
        (Some(10_000), 10_000)
    );
    assert_eq!(b.pacing_stage, PerformancePacingStage::S0);
}

// Tm-2 (§3.3): twenty samples in the 30 s window, one of them ten times late: the window's mean
// stays above 9 500 and no stage changes.
#[test]
fn tm_2_one_late_sample_among_twenty_changes_no_stage() {
    let settings = settings();
    let instances = aliases(&[A]);
    let mut shadow = PacingShadow::default();
    captures(&mut shadow, A, 19, QUICK_CAPTURE_US);
    shadow.record(capture(A, T0 - 1, SLOW_CAPTURE_US));
    let mut transitions = Vec::new();
    for at in [T0, T0 + 2_000, SUSTAINED, T0 + 6_000] {
        transitions.extend(shadow.evaluate(at, &settings, &instances, None));
    }
    assert!(transitions.is_empty(), "{transitions:?}");
    let status = read_status(&shadow, &settings, &instances);
    let a = row(&status, A);
    // (19 × 10 000 + 3 666) / 20.
    assert_eq!(a.responsiveness_bp, 9_683);
    assert!(a.responsiveness_bp >= 9_500);
    assert_eq!(a.pacing_stage, PerformancePacingStage::S0);
}

// Tm-3 (§3.5): before 50 calm samples the baseline is the initial value; then the median of the
// calm samples; past the clamp it is held at the bound, which the status reports once as
// perf_baseline_clamped.
#[test]
fn tm_3_a_baseline_is_initial_then_the_median_and_held_at_its_clamp() {
    let settings = settings();
    let instances = aliases(&[]);
    let mut shadow = PacingShadow::default();
    for index in 0..49 {
        shadow.record(timer(T0 - 1_000 + index, 30_000));
    }
    shadow.evaluate(T0, &settings, &instances, None);
    assert_eq!(shadow.baseline_us(None, PacingSignal::Timer), Some(16_000));
    shadow.record(timer(T0 + 1, 30_000));
    shadow.evaluate(T0 + 2_000, &settings, &instances, None);
    assert_eq!(shadow.baseline_us(None, PacingSignal::Timer), Some(30_000));
    for index in 0..60 {
        shadow.record(timer(T0 + 2_001 + index, 100_000));
    }
    shadow.evaluate(SUSTAINED, &settings, &instances, None);
    // The median of 50 × 30 000 and 60 × 100 000 is 100 000, above 16 000 × 2.
    assert_eq!(shadow.baseline_us(None, PacingSignal::Timer), Some(32_000));
    let status = read_status(&shadow, &settings, &instances);
    assert_eq!(status.warnings.len(), 1, "{:?}", status.warnings);
    let warning = outcome_json(&status.warnings[0]);
    assert_eq!(warning["code"], json!("perf_baseline_clamped"));
    assert_eq!(warning["category"], json!("warning"));
    assert_eq!(warning["values"]["signal_kind"], json!("timer"));
    assert_eq!(warning["values"]["baseline_us"], json!(32_000));
    assert_eq!(warning["values"]["limit_us"], json!(32_000));
    assert!(warning["values"].get("instance_id").is_none(), "{warning}");
}

// Tm-4 (§3.5): only samples taken at S0 without pressure teach a baseline: none under a
// pressure of the floor, none while the stage is S1; the same samples at S0 without pressure do.
#[test]
fn tm_4_only_calm_samples_teach_a_baseline() {
    let settings = settings();
    let instances = aliases(&[]);

    // Under a pressure (the floor holds S1).
    let mut pressured = PacingShadow::default();
    pressured.evaluate(
        T0,
        &settings,
        &instances,
        Some(PerformancePressureSeverity::Elevated),
    );
    for index in 0..60 {
        pressured.record(timer(T0 + 1 + index, 30_000));
    }
    pressured.evaluate(
        T0 + 2_000,
        &settings,
        &instances,
        Some(PerformancePressureSeverity::Elevated),
    );
    assert_eq!(
        pressured.baseline_us(None, PacingSignal::Timer),
        Some(16_000)
    );
    // The pressure ended: back at S0, the same samples teach it.
    pressured.evaluate(SUSTAINED, &settings, &instances, None);
    for index in 0..60 {
        pressured.record(timer(SUSTAINED + 1 + index, 30_000));
    }
    pressured.evaluate(T0 + 6_000, &settings, &instances, None);
    assert_eq!(
        pressured.baseline_us(None, PacingSignal::Timer),
        Some(30_000)
    );

    // At S1 by responsiveness: ten slow samples (42 000 / 50 000 scores 8 400) move the stage.
    let mut slow = PacingShadow::default();
    for index in 0..10 {
        slow.record(timer(T0 - 1_000 + index, 50_000));
    }
    settle(&mut slow, &settings, &instances);
    let status = read_status(&slow, &settings, &instances);
    assert_eq!(status.host.responsiveness_bp, 8_400);
    assert_eq!(status.host.pacing_stage, PerformancePacingStage::S1);
    for index in 0..60 {
        slow.record(timer(SUSTAINED + 1 + index, 30_000));
    }
    slow.evaluate(T0 + 6_000, &settings, &instances, None);
    assert_eq!(slow.baseline_us(None, PacingSignal::Timer), Some(16_000));
}

// Tm-5 (§3.2, §3.3): the timer probe's oversleeps score by their allowance; a probe that cannot
// run leaves its signal unknown with one perf_probe_unavailable, and every other signal goes on.
#[test]
fn tm_5_timer_oversleeps_score_by_the_formula_and_a_missing_probe_leaves_only_its_signal_unknown() {
    let settings = settings();
    let instances = aliases(&[A]);
    let mut probed = PacingShadow::default();
    // Scores 10 000, 10 000, 10 000, 5 000 and 2 500: a mean of 7 500.
    for (index, value_us) in [10_000, 20_000, 42_000, 84_000, 168_000]
        .into_iter()
        .enumerate()
    {
        probed.record(timer(T0 - 100 + index as u64, value_us));
    }
    probed.evaluate(T0, &settings, &instances, None);
    let status = read_status(&probed, &settings, &instances);
    assert_eq!(status.host.host_responsiveness_bp, Some(7_500));
    assert!(status.warnings.is_empty(), "{:?}", status.warnings);

    let mut unprobed = PacingShadow::default();
    unprobed.probe_unavailable(
        ProbeKind::Timer,
        Outcome::new(PacingCode::PerfProbeUnavailable).probe_kind(ProbeKind::Timer),
    );
    captures(&mut unprobed, A, 10, SLOW_CAPTURE_US);
    settle(&mut unprobed, &settings, &instances);
    let status = read_status(&unprobed, &settings, &instances);
    assert_eq!(status.host.host_responsiveness_bp, None);
    assert_eq!(status.warnings.len(), 1, "{:?}", status.warnings);
    let warning = outcome_json(&status.warnings[0]);
    assert_eq!(warning["code"], json!("perf_probe_unavailable"));
    assert_eq!(warning["values"]["probe_kind"], json!("timer"));
    assert_eq!(row(&status, A).responsiveness_bp, SLOW_CAPTURE_BP);
}

// Tm-7 (§10.3, v1.3 #12): under observe the stage that takes effect is S0 with a factor of 1 000
// in every row, whatever the curve gives; the stage transition is recorded all the same.
#[test]
fn tm_7_observe_takes_nothing_into_effect_and_records_the_transition() {
    let settings = settings();
    assert_eq!(settings.pacing.mode, PerformancePacingMode::Observe);
    let instances = aliases(&[A]);
    let mut shadow = PacingShadow::default();
    captures(&mut shadow, A, 10, SLOW_CAPTURE_US);
    let transitions = settle(&mut shadow, &settings, &instances);
    assert_eq!(transitions.len(), 1, "{transitions:?}");
    let transition = outcome_json(&transitions[0]);
    assert_eq!(transition["code"], json!("perf_stage_changed"));
    assert_eq!(transition["category"], json!("info"));
    let values = &transition["values"];
    assert_eq!(values["pacing_stage_from"], json!("s0"));
    assert_eq!(values["pacing_stage_to"], json!("s3"));
    assert_eq!(values["responsiveness_bp"], json!(SLOW_CAPTURE_BP));
    // Between (4 500, 3 250) and (3 000, 4 000): 3 250 + 750 × 834 / 1 500.
    assert_eq!(values["factor_milli"], json!(3_667));
    assert_eq!(values["pacing_cause"], json!("responsiveness"));
    assert_eq!(values["pacing_mode"], json!("observe"));
    assert_eq!(values["instance_id"], json!(A));
    assert_eq!(values["emulator_responsiveness_bp"], json!(SLOW_CAPTURE_BP));
    let status = read_status(&shadow, &settings, &instances);
    assert_eq!(row(&status, A).pacing_stage, PerformancePacingStage::S3);
    for (effective_stage, effective_factor_milli) in [
        (
            status.host.effective_stage,
            status.host.effective_factor_milli,
        ),
        (
            row(&status, A).effective_stage,
            row(&status, A).effective_factor_milli,
        ),
    ] {
        assert_eq!(
            (effective_stage, effective_factor_milli),
            (PerformancePacingStage::S0, 1_000)
        );
    }
}

// Tm-8 (§3.2, review L4): a 650 ms drag's touch response never lowers R_emu; a tap of the same
// response does (180 000 / 650 000 scores 2 769).
#[test]
fn tm_8_a_drag_s_touch_response_is_not_scored_and_a_tap_s_is() {
    let settings = settings();
    let instances = aliases(&[A]);
    for (tap, expected) in [(false, (None, 10_000)), (true, (Some(2_769), 2_769))] {
        let mut shadow = PacingShadow::default();
        for index in 0..10 {
            shadow.record(touch(A, T0 - 1_000 + index, 650_000, tap));
        }
        settle(&mut shadow, &settings, &instances);
        let status = read_status(&shadow, &settings, &instances);
        let a = row(&status, A);
        assert_eq!(
            (a.emulator_responsiveness_bp, a.responsiveness_bp),
            expected,
            "tap {tap}"
        );
    }
}

// Tm-11 (§4.6): under observe with §4.2's curve, three instances at 2 900, 4 800 and 10 000:
// the learning view gives S2, S2, S0, every share full and allowed 3; the normal view S4, S3,
// S0, shares 0, 0.6 and 1 and allowed 2; nothing takes effect; perf.summary carries the same.
#[test]
fn tm_11_the_learning_and_the_normal_view_side_by_side() {
    let settings = settings();
    let instances = aliases(&[A, B, C]);
    let mut shadow = PacingShadow::default();
    // 550 000 000 / 189 655 = 2 900 and 550 000 000 / 114 583 = 4 800 (basis points).
    captures(&mut shadow, A, 10, 189_655);
    captures(&mut shadow, B, 10, 114_583);
    captures(&mut shadow, C, 10, QUICK_CAPTURE_US);
    settle(&mut shadow, &settings, &instances);
    let status = read_status(&shadow, &settings, &instances);
    let views = |alias: &str| {
        let row = row(&status, alias);
        (
            row.responsiveness_bp,
            (
                row.learning_view.pacing_stage,
                row.learning_view.share_milli,
                row.learning_view_allowed,
            ),
            (
                row.normal_view.pacing_stage,
                row.normal_view.share_milli,
                row.normal_view_allowed,
            ),
            (row.effective_stage, row.effective_factor_milli),
        )
    };
    use PerformancePacingStage::{S0, S2, S3, S4};
    assert_eq!(
        views(A),
        (2_900, (S2, 1_000, true), (S4, 0, false), (S0, 1_000))
    );
    assert_eq!(
        views(B),
        (4_800, (S2, 1_000, true), (S3, 600, true), (S0, 1_000))
    );
    assert_eq!(
        views(C),
        (10_000, (S0, 1_000, true), (S0, 1_000, true), (S0, 1_000))
    );
    assert_eq!(
        (
            status.host.learning_view_allowed,
            status.host.normal_view_allowed
        ),
        (3, 2)
    );

    let summary = shadow.summary().expect("the pacing members");
    summary.validate().expect("valid pacing members");
    assert_eq!(
        (summary.learning_view_allowed, summary.normal_view_allowed),
        (3, 2)
    );
    assert_eq!(summary.pacing_mode, PerformancePacingMode::Observe);
    assert_eq!(summary.pacing_phase, None);
    for (alias, learning, normal) in [
        (A, (S2, 1_000, true), (S4, 0, false)),
        (B, (S2, 1_000, true), (S3, 600, true)),
        (C, (S0, 1_000, true), (S0, 1_000, true)),
    ] {
        let row = summary
            .instances
            .iter()
            .find(|row| row.instance_id == alias)
            .unwrap_or_else(|| panic!("no summary row of {alias}"));
        assert_eq!(
            (
                row.learning_view_stage,
                row.learning_view_share_milli,
                row.learning_view_allowed
            ),
            learning,
            "{alias}"
        );
        assert_eq!(
            (
                row.normal_view_stage,
                row.normal_view_share_milli,
                row.normal_view_allowed
            ),
            normal,
            "{alias}"
        );
    }
}

// Tc-3 (§4.1, §10.3: the configuration decides): the same samples under two curves give each
// curve's stage and factor. At 8 500 §4.2's curve gives S1 and 1 000 + 500 × 500 / 1 500; a
// curve whose S2 starts at 8 000 gives S2 and 1 000 + 1 000 × 1 300 / 1 800.
#[test]
fn tc_3_two_curves_give_each_its_own_stage_and_factor() {
    let shipped = settings();
    let mut steeper = settings();
    let point = |r_bp, stage, factor_milli, extra_retries, share_milli| CurvePointSettings {
        r_bp,
        stage,
        factor_milli,
        extra_retries,
        share_milli,
    };
    steeper.pacing.curve = vec![
        point(10_000, PerformancePacingStage::S0, 1_000, 0, 1_000),
        point(9_800, PerformancePacingStage::S0, 1_000, 0, 1_000),
        point(8_000, PerformancePacingStage::S2, 2_000, 2, 1_000),
        point(5_000, PerformancePacingStage::S3, 3_000, 3, 500),
        point(0, PerformancePacingStage::S3, 3_000, 3, 0),
    ];
    let instances = aliases(&[A]);
    for (settings, stage, factor_milli) in [
        (&shipped, PerformancePacingStage::S1, 1_166),
        (&steeper, PerformancePacingStage::S2, 1_722),
    ] {
        let mut shadow = PacingShadow::default();
        // 550 000 000 / 64 705 = 8 500 (basis points).
        captures(&mut shadow, A, 10, 64_705);
        settle(&mut shadow, settings, &instances);
        let status = read_status(&shadow, settings, &instances);
        let a = row(&status, A);
        assert_eq!(a.responsiveness_bp, 8_500);
        assert_eq!((a.pacing_stage, a.factor_milli), (stage, factor_milli));
    }
}

// Tm-12 (§4.6), the Runtime's side: PerformanceStatus answers from memory, one row per
// registered instance with the mode in effect (observe) and nothing taking effect, appends no
// ledger record and leaves no record of its request.
#[test]
fn tm_12_the_status_read_answers_from_memory_and_appends_nothing() {
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(
            POLICY_INSTANCE_ALIAS,
            instance_id(),
            Arc::new(FakeState::default()),
        )),
    )
    .expect("runtime host");
    let mut client = TestClient::connect(&host);
    let mut reader = TestClient::connect(&host);
    let head = ledger_head(&mut reader);
    let request = client.request(RuntimeOperation::PerformanceStatus);
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let Some(RuntimeResult::PerformanceStatus { status }) = receipt.result() else {
        panic!("expected a performance status, got {:?}", receipt.result());
    };
    assert_eq!(status.pacing_mode, PerformancePacingMode::Observe);
    assert_eq!(status.pacing_phase, None);
    // This host runs no performance monitor.
    assert_eq!(status.monitor_health, PerformanceMonitorHealth::Unavailable);
    assert_eq!(
        (
            status.host.effective_stage,
            status.host.effective_factor_milli
        ),
        (PerformancePacingStage::S0, 1_000)
    );
    let rows = status
        .instances
        .iter()
        .map(|row| {
            (
                row.instance_alias.as_str(),
                row.effective_stage,
                row.effective_factor_milli,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![(POLICY_INSTANCE_ALIAS, PerformancePacingStage::S0, 1_000)]
    );
    assert_eq!(ledger_head(&mut reader), head, "the read appended a record");
    assert!(
        projected_events(
            &mut reader,
            EventQuery {
                request_id: Some(request.request_id()),
                ..EventQuery::default()
            },
        )
        .is_empty(),
        "the read left a record of its request"
    );
    drop(reader);
    drop(client);
    host.close().expect("close host");
}
