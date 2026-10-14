// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #385 P-c, test rows Tc-1, Tc-2, Tc-4 and Tc-5 (frozen model PERF v1.4.2 §14). Every
// expected value comes from the model: §10.1 to §10.4 (the keys and their defaults), §4.2 (the
// default curve), §4.1 (the curve's rules), §4.6 and §10.4 (`mode`), §10.4 (the calibration's
// rules) and §11 (`perf_config_invalid` naming the key). Two spellings follow the register
// (`contracts/v012-register.md` §5): the pressure streaks keep their file members
// `performance.pressure_start_samples` / `_end_samples`, and the stage boundaries are
// `bounds.s1` to `bounds.s4` (a configuration key is lowercase).

use super::*;
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

/// The rows a start renders from a file whose `performance` section is `section`.
fn rendered(section: Value) -> Vec<actingcommand_contract::ConfigParameter> {
    render_parameters(
        &mechanism_rows(),
        &[],
        &shipped(),
        &json!({ "performance": section }),
        &|_| None,
    )
    .expect("every key resolves")
}

/// The configuration a start reads from a file whose `performance` section is `section`.
fn from_file(section: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: rendered(section),
    })
    .expect("every key is in the key table")
}

/// The default curve of §4.2: `[r_bp, stage, factor_milli, extra_retries, share_milli]`.
fn model_curve() -> Value {
    let point = |r_bp: i64, stage: &str, factor: i64, retries: i64, share: i64| {
        json!({"r_bp": r_bp, "stage": stage, "factor_milli": factor,
               "extra_retries": retries, "share_milli": share})
    };
    json!([
        point(10_000, "S0", 1_000, 0, 1_000),
        point(9_000, "S0", 1_000, 0, 1_000),
        point(7_500, "S1", 1_500, 1, 1_000),
        point(6_000, "S2", 2_500, 2, 1_000),
        point(4_500, "S3", 3_250, 3, 500),
        point(3_000, "S3", 4_000, 3, 0),
        point(0, "S3", 4_000, 3, 0),
    ])
}

/// Every key of PERF §10 with the default the model gives it, by subsection.
fn model_defaults() -> [Vec<(&'static str, Value)>; 4] {
    let monitor = vec![
        ("performance.monitor.sample_interval_ms", json!(2_000)),
        ("performance.monitor.context_window_ms", json!(30_000)),
        ("performance.monitor.summary_interval_ms", json!(60_000)),
        ("performance.monitor.max_pipeline_samples", json!(4_096)),
        ("performance.monitor.process_coverage_min_bp", json!(9_900)),
        ("performance.monitor.max_system_samples", json!(64)),
        ("performance.monitor.max_consecutive_failures", json!(3)),
        ("performance.monitor.control_freshness_multiplier", json!(2)),
        ("performance.monitor.stutter_frame_gap_ms", json!(1_000)),
        ("performance.load.heavy_threshold_milli", json!(1_500)),
        ("performance.pressure_start_samples", json!(3)),
        ("performance.pressure_end_samples", json!(3)),
        (
            "performance.pressure.thresholds",
            json!({"cpu": [8_500, 7_000], "ram": [9_000, 8_000], "gpu": [9_000, 7_500],
                   "disk_queue_milli": [4_000, 2_000], "disk_latency_us": [50_000, 25_000],
                   "process_cpu_bp": 4_000, "process_io_bps": 67_108_864}),
        ),
        (
            "performance.pressure.severity",
            json!({"critical_bp": 9_700, "elevated_bp": 9_200,
                   "third_party_critical": [3, 8_000], "third_party_elevated": [2, 6_000]}),
        ),
        (
            "performance.pressure.gpu_aggregate",
            json!("max_engine_type"),
        ),
    ];
    let responsiveness = vec![
        ("performance.responsiveness.window_ms", json!(30_000)),
        ("performance.responsiveness.min_window_samples", json!(3)),
        ("performance.responsiveness.ewma_alpha_milli", json!(300)),
        ("performance.responsiveness.emu_stale_ms", json!(120_000)),
        (
            "performance.responsiveness.host_probe.timer.enabled",
            json!(true),
        ),
        (
            "performance.responsiveness.host_probe.timer.interval_ms",
            json!(1_000),
        ),
        (
            "performance.responsiveness.host_probe.timer.burst",
            json!(5),
        ),
        (
            "performance.responsiveness.host_probe.timer.period_ms",
            json!(15),
        ),
        (
            "performance.responsiveness.host_probe.timer.initial_baseline_us",
            json!(16_000),
        ),
        (
            "performance.responsiveness.host_probe.timer.allowance_milli",
            json!(2_000),
        ),
        (
            "performance.responsiveness.host_probe.timer.slack_us",
            json!(10_000),
        ),
        (
            "performance.responsiveness.host_probe.message.enabled",
            json!(true),
        ),
        (
            "performance.responsiveness.host_probe.message.interval_ms",
            json!(1_000),
        ),
        (
            "performance.responsiveness.host_probe.message.burst",
            json!(5),
        ),
        (
            "performance.responsiveness.host_probe.message.initial_baseline_us",
            json!(1_000),
        ),
        (
            "performance.responsiveness.host_probe.message.allowance_milli",
            json!(3_000),
        ),
        (
            "performance.responsiveness.host_probe.message.slack_us",
            json!(20_000),
        ),
        (
            "performance.responsiveness.signals.capture_acquire.enabled",
            json!(true),
        ),
        (
            "performance.responsiveness.signals.capture_acquire.initial_baseline_us",
            json!({"default": 15_000, "nemu_ipc": 15_000, "adb": 400_000}),
        ),
        (
            "performance.responsiveness.signals.capture_acquire.allowance_milli",
            json!(3_000),
        ),
        (
            "performance.responsiveness.signals.capture_acquire.slack_us",
            json!(10_000),
        ),
        (
            "performance.responsiveness.signals.touch_response.enabled",
            json!(true),
        ),
        (
            "performance.responsiveness.signals.touch_response.initial_baseline_us",
            json!({"default": 50_000}),
        ),
        (
            "performance.responsiveness.signals.touch_response.allowance_milli",
            json!(3_000),
        ),
        (
            "performance.responsiveness.signals.touch_response.slack_us",
            json!(30_000),
        ),
        (
            "performance.responsiveness.signals.touch_response.score_gestures",
            json!(false),
        ),
        (
            "performance.responsiveness.signals.recognition.enabled",
            json!(false),
        ),
        (
            "performance.responsiveness.signals.page_settle.record",
            json!(true),
        ),
        (
            "performance.responsiveness.signals.page_settle.enabled",
            json!(false),
        ),
        (
            "performance.responsiveness.emu_freeze_while_paused",
            json!(true),
        ),
        ("performance.responsiveness.baseline.min_samples", json!(50)),
        (
            "performance.responsiveness.baseline.window_samples",
            json!(500),
        ),
        ("performance.responsiveness.baseline.quantile", json!(50)),
        (
            "performance.responsiveness.baseline.clamp_min_milli",
            json!(250),
        ),
        (
            "performance.responsiveness.baseline.clamp_max_milli",
            json!(2_000),
        ),
        (
            "performance.responsiveness.baseline.persist_delta_milli",
            json!(100),
        ),
        (
            "performance.responsiveness.baseline.persist_min_interval_ms",
            json!(600_000),
        ),
    ];
    let pacing = vec![
        ("performance.pacing.mode", json!("observe")),
        ("performance.pacing.interpolate", json!("linear")),
        ("performance.pacing.curve", model_curve()),
        ("performance.pacing.max_factor_milli", json!(5_000)),
        ("performance.pacing.hysteresis_bp", json!(500)),
        ("performance.pacing.degrade_sustain_ms", json!(4_000)),
        ("performance.pacing.recover_sustain_ms", json!(30_000)),
        ("performance.pacing.pause_sustain_ms", json!(60_000)),
        ("performance.pacing.resume_sustain_ms", json!(60_000)),
        ("performance.pacing.pause_cooldown_ms", json!(10_000)),
        ("performance.pacing.pre_input_wait_base_ms", json!(300)),
        ("performance.pacing.inter_run_gap_base_ms", json!(0)),
        (
            "performance.pacing.pressure_floor.kinds",
            json!([
                "cpu",
                "ram",
                "gpu",
                "disk_io",
                "third_party",
                "fullscreen_contention",
                "ledger_backlog"
            ]),
        ),
        (
            "performance.pacing.pressure_floor.min_severity",
            json!("elevated"),
        ),
        (
            "performance.pacing.pressure_floor.factor_milli",
            json!(1_250),
        ),
        ("performance.pacing.pressure_floor.extra_retries", json!(1)),
        ("performance.pacing.tax_exempt_from_stage", json!("S1")),
        ("performance.pacing.heavy_dispatch_limit.normal", json!(2)),
        ("performance.pacing.heavy_dispatch_limit.paused", json!(1)),
        ("performance.pacing.clock_jump_ms", json!(30_000)),
        ("performance.pacing.hold.max_ms", json!(300_000)),
        ("performance.pacing.hold.poll_ms", json!(250)),
        (
            "performance.pacing.hold.probe_capture_interval_ms",
            json!(5_000),
        ),
        ("performance.pacing.hold.on_partial_pause", json!(false)),
        ("performance.pacing.pause_probe.interval_ms", json!(30_000)),
        ("performance.pacing.pause_probe.frames", json!(3)),
    ];
    let calibration = vec![
        ("performance.calibration.apply", json!("auto")),
        ("performance.calibration.band_bp", json!(500)),
        (
            "performance.calibration.trouble_codes",
            json!([
                "contained_task_page_unknown",
                "page_confirmation_failed",
                "recognition_sample_deadline_exceeded"
            ]),
        ),
        ("performance.calibration.learn_min_days", json!(3)),
        (
            "performance.calibration.learn_min_active_ms",
            json!(43_200_000),
        ),
        ("performance.calibration.learning_max_stage", json!("S2")),
        ("performance.calibration.learning_backstop_bp", json!(2_000)),
        ("performance.calibration.interval_ms", json!(86_400_000)),
        ("performance.calibration.min_state_steps", json!(200)),
        ("performance.calibration.tolerance.s0", json!(10)),
        ("performance.calibration.tolerance.s1", json!(50)),
        ("performance.calibration.tolerance.s2", json!(150)),
        ("performance.calibration.tolerance.stop", json!(400)),
        ("performance.calibration.bounds.s1", json!([7_000, 9_700])),
        ("performance.calibration.bounds.s2", json!([5_000, 8_500])),
        ("performance.calibration.bounds.s3", json!([3_000, 7_000])),
        ("performance.calibration.bounds.s4", json!([1_000, 5_000])),
        ("performance.calibration.max_shift_bp", json!(500)),
        ("performance.calibration.pressure_max_stage", json!("S2")),
    ];
    [monitor, responsiveness, pacing, calibration]
}

#[test]
fn tc_1_every_performance_key_is_in_the_defaults_file_and_config_parameters_with_its_source() {
    let defaults = shipped();
    let model = model_defaults();
    // §10.1 to §10.4, one table each.
    assert_eq!(
        model.iter().map(Vec::len).collect::<Vec<_>>(),
        [15, 37, 26, 19]
    );
    let model = model.into_iter().flatten().collect::<Vec<_>>();
    // The key table has exactly the model's keys in the section.
    let table_keys = rows()
        .filter_map(|row| match row.name {
            KeyName::Exact(key) if key.starts_with("performance.") => Some(key),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let model_keys = model.iter().map(|(key, _)| *key).collect::<BTreeSet<_>>();
    assert_eq!(table_keys, model_keys);

    // A configuration that names no performance key: every key in effect is the defaults
    // file's, which is the model's, with the source `default`.
    let parameters = rendered(json!({}));
    for (key, expected) in &model {
        assert_eq!(
            member(&defaults, key),
            Some(expected),
            "{key} in the defaults file"
        );
        let (row, _) = find_row(key).expect("the key is in the key table");
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == *key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(
            scalar_json(row.value_type, &parameter.value).as_ref(),
            Some(expected),
            "{key}"
        );
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
    }

    // A configuration that names two keys: those two are `explicit` with the file's values,
    // every other key stays the default.
    let parameters = rendered(json!({
        "pacing": {"hold": {"max_ms": 600_000}},
        "pressure_start_samples": 5,
    }));
    for (key, expected) in &model {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == *key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        let (row, _) = find_row(key).expect("the key is in the key table");
        let (value, source) = match *key {
            "performance.pacing.hold.max_ms" => (json!(600_000), ConfigParameterSource::Explicit),
            "performance.pressure_start_samples" => (json!(5), ConfigParameterSource::Explicit),
            _ => (expected.clone(), ConfigParameterSource::Default),
        };
        assert_eq!(
            scalar_json(row.value_type, &parameter.value),
            Some(value),
            "{key}"
        );
        assert_eq!(parameter.source, source, "{key}");
    }
}

/// The configuration's `perf_config_invalid` at start: its code and the key it names.
fn refused(section: Value) -> (ConfigCode, String) {
    let failure = from_file(section)
        .check(&context())
        .expect_err("the section breaks a rule");
    (failure.failure_code(), failure.config_key().to_owned())
}

/// The heartbeat bound and device bounds the other sections' checks read: inputs of the check.
fn context() -> CheckContext {
    CheckContext::new(
        5_000,
        DeviceBounds {
            application_stop_ms: 60_000,
            emulator_stop_ms: 200_000,
            emulator_start_ms: 190_000,
        },
    )
}

/// The default curve with `point`'s member `name` set to `value`.
fn curve_with(point: usize, name: &str, value: Value) -> Value {
    let mut curve = model_curve();
    curve[point][name] = value;
    json!({ "pacing": { "curve": curve } })
}

#[test]
fn tc_2_a_curve_that_breaks_a_rule_is_refused_naming_the_curve() {
    let curve = "performance.pacing.curve";
    for (case, section) in [
        // Points 9 000 and 10 000: `r_bp` does not fall strictly.
        (
            "points that do not fall",
            curve_with(1, "r_bp", json!(10_000)),
        ),
        (
            "a factor below one",
            curve_with(2, "factor_milli", json!(900)),
        ),
        // Points 6 000 (share 1 000) and 4 500 (share 1 000 here) then 3 000 (share 0).
        (
            "a full share straight to none",
            curve_with(4, "share_milli", json!(1_000)),
        ),
        // Point 4 500 names S1 after point 6 000's S2.
        (
            "a stage that steps back",
            curve_with(4, "stage", json!("S1")),
        ),
    ] {
        assert_eq!(
            refused(section),
            (ConfigCode::PerfConfigInvalid, curve.to_owned()),
            "{case}"
        );
    }
}

#[test]
fn tc_4_a_configuration_without_mode_takes_the_release_default_observe() {
    // The file names other performance keys, never `mode`.
    let manifest = from_file(json!({"pacing": {"hold": {"max_ms": 600_000}}})).manifest();
    let mode = manifest
        .parameters
        .iter()
        .find(|parameter| parameter.key == "performance.pacing.mode")
        .expect("mode is in effect");
    assert_eq!(mode.value, FactScalar::String("observe".to_owned()));
    assert_eq!(mode.source, ConfigParameterSource::Default);
    // The release's default is `observe` (§4.6: every release until Alice approves another).
    assert_eq!(
        member(&shipped(), "performance.pacing.mode"),
        Some(&json!("observe"))
    );
    // The example configuration an installation starts from names no performance key, so a
    // user configuration carries `mode` only when a person writes it.
    let example = serde_json::from_str::<Value>(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../distribution/windows/actingd.config.example.json"
    )))
    .expect("the example configuration parses");
    assert_eq!(example.get("performance"), None);
}

#[test]
fn tc_5_calibration_values_that_break_a_rule_are_refused_naming_the_key() {
    for (case, section, config_key) in [
        (
            "a tolerance that does not rise",
            json!({"calibration": {"tolerance": {"s1": 10}}}),
            "performance.calibration.tolerance.s1",
        ),
        (
            "a boundary whose minimum is not below its maximum",
            json!({"calibration": {"bounds": {"s2": [8_500, 5_000]}}}),
            "performance.calibration.bounds.s2",
        ),
        (
            "a deeper stage's boundary above the stage before it",
            json!({"calibration": {"bounds": {"s3": [6_000, 9_000]}}}),
            "performance.calibration.bounds.s3",
        ),
        (
            "a pressure-only state allowed to S3",
            json!({"calibration": {"pressure_max_stage": "S3"}}),
            "performance.calibration.pressure_max_stage",
        ),
    ] {
        assert_eq!(
            refused(section),
            (ConfigCode::PerfConfigInvalid, config_key.to_owned()),
            "{case}"
        );
    }
}

/// Coordinator ruling (2026-10-14) on the curve's length: at most 8 points, the largest count
/// whose `config.parameters` text stays within 1 024 bytes for every curve the other rules allow.
/// The longest curve of 8 points is accepted with its text within 1 024 bytes (the longest of 9
/// would pass it); 9 points are refused, at start as `perf_config_invalid` and through an
/// instruction as `config_value_invalid`, both `out_of_range` and naming the curve.
#[test]
fn a_curve_of_eight_points_is_accepted_and_nine_are_refused_by_file_and_by_instruction() {
    const CURVE: &str = "performance.pacing.curve";
    const LONGEST: i64 = i64::MAX;
    // The longest text the other rules allow: `r_bp` 10 000 first, four digits in between and 0
    // last; `extra_retries` and `factor_milli` at their largest; every share full.
    let curve = |points: i64| {
        Value::Array(
            (0..points)
                .map(|at| {
                    let r_bp = match at {
                        0 => 10_000,
                        _ if at + 1 == points => 0,
                        _ => 9_999 - at,
                    };
                    json!({"r_bp": r_bp, "stage": "S0", "factor_milli": LONGEST,
                           "extra_retries": LONGEST, "share_milli": 1_000})
                })
                .collect(),
        )
    };
    assert!(canonical_json(&curve(9)).len() > 1_024);
    let change = |curve: Value| ConfigSettingChange {
        key: CURVE.to_owned(),
        value: Some(curve),
    };
    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
    };

    // By file.
    let accepted = from_file(json!({"pacing": {"max_factor_milli": LONGEST, "curve": curve(8)}}));
    accepted
        .check(&context())
        .expect("eight points are accepted");
    let Some(FactScalar::String(text)) = accepted.value(CURVE) else {
        panic!("the curve is reported as text");
    };
    assert!(text.len() <= 1_024, "{} bytes", text.len());
    let refused = from_file(json!({"pacing": {"max_factor_milli": LONGEST, "curve": curve(9)}}))
        .check(&context())
        .expect_err("nine points are refused");
    assert_eq!(
        (
            refused.failure_code(),
            refused.config_key(),
            refused.problem()
        ),
        (
            ConfigCode::PerfConfigInvalid,
            CURVE,
            Some(ConfigValueProblem::OutOfRange)
        )
    );

    // By instruction: the curve is settable.
    let running = from_file(json!({"pacing": {"max_factor_milli": LONGEST}}));
    let applied = running
        .apply(&[change(curve(8))], &stamp, &context())
        .expect("eight points by instruction");
    assert_eq!(applied.config.value(CURVE), accepted.value(CURVE));
    let refused = running
        .apply(&[change(curve(9))], &stamp, &context())
        .expect_err("nine points by instruction are refused");
    assert_eq!(
        (
            refused.failure_code(),
            refused.config_key(),
            refused.problem()
        ),
        (
            ConfigCode::ConfigValueInvalid,
            CURVE,
            Some(ConfigValueProblem::OutOfRange)
        )
    );
}
