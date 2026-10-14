// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #383 AO-1 (`contracts/ledger-observer.md` §15; `contracts/v012-register.md` §5): every
// `ledger_observer` key is parsed, checked and defaulted in v0.12.0, the keys only 0.12.x slices
// read included. The defaults are the contract's §15 table; whether an instruction may change a
// key is AO-1's decision on §15's proposal (watermark, batch and write-path values are read when
// the ledger opens; the probes' own periods and thresholds when they run); the rules are §7's
// configuration check and §9.1's one wait per attempt.

use super::*;
use crate::ledger_observer::{
    BUFFER_BYTES, BUFFER_HYSTERESIS_RATIO, BUFFER_RECORDS, BUFFER_TIER1_RATIO, BUFFER_TIER2_RATIO,
    BUFFER_TIER3_RATIO, DUMP_MAX_BYTES, FLUSH_ATTEMPT_STALL_MS, FLUSH_BATCH_RECORDS,
    FLUSH_BATCH_RECORDS_BUSY, FLUSH_LAG_WARN_MS, HEALTH_PROBE_INTERVAL_MS, PROBE_INTERVAL_MS,
    PROBE_SAMPLE_KEYS, REQUEST_INDEX_RECORDS, SPILL_BATCH_BYTES, STATUS_GAP_WINDOW_MS,
    TAG_REOBSERVE_PREFIXES, TAG_WORKBENCH_PATTERNS, WRITE_ATTEMPTS, WRITE_RETRY_BACKOFF_MS,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

// §15 `tag.workbench_patterns`.
const WORKBENCH_PATTERNS: [&str; 10] = [
    "task.*.last_duration_ms",
    "task.*.last_outcome",
    "task.*.failure_streak",
    "task.*.completed_at_unix_ms",
    "host.scheduling_pause",
    "host.install_transition",
    "host.install_transition.pauses",
    "host.perf_baseline.*",
    "host.perf_calibration.*",
    "host.config_overrides",
];
// §15 `tag.reobserve_prefixes`.
const REOBSERVE_PREFIXES: [&str; 10] = [
    "device.",
    "backend.",
    "application.",
    "lease.",
    "provider.",
    "config.",
    "task.page",
    "task.game",
    "task.server",
    "host.pack_registry.",
];

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

// The rows a start renders from a file whose `ledger_observer` section is `section`.
fn render(section: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &shipped(),
        &json!({ "ledger_observer": section }),
        &|_| None,
    )?;
    annotate_settable(&mut parameters)?;
    Ok(parameters)
}

fn from_file(section: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(section).expect("every key resolves"),
    })
    .expect("every key is in the key table")
}

fn json_text(value: &FactScalar) -> Value {
    match value {
        FactScalar::String(text) => serde_json::from_str(text).expect("canonical JSON"),
        other => panic!("a list or decimal is reported as JSON text: {other:?}"),
    }
}

// §15: every key of the section, defaulted from the defaults file (source `default`), with
// AO-1's settable decision; the section in effect reads the same values and passes its check.
#[test]
fn ao1_keys_every_ledger_observer_key_has_its_contract_default() {
    let parameters = render(json!({})).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    // (key, default, settable while the Runtime runs).
    let scalars = [
        (BUFFER_BYTES, FactScalar::Integer(67_108_864), false),
        (BUFFER_RECORDS, FactScalar::Integer(20_480), false),
        (SPILL_BATCH_BYTES, FactScalar::Integer(4_194_304), false),
        (DUMP_MAX_BYTES, FactScalar::Integer(1_073_741_824), false),
        (FLUSH_BATCH_RECORDS, FactScalar::Integer(256), false),
        (FLUSH_BATCH_RECORDS_BUSY, FactScalar::Integer(2_048), false),
        (WRITE_ATTEMPTS, FactScalar::Integer(5), false),
        (
            FLUSH_ATTEMPT_STALL_MS,
            FactScalar::DurationMs(120_000),
            false,
        ),
        (FLUSH_LAG_WARN_MS, FactScalar::DurationMs(5_000), true),
        (
            HEALTH_PROBE_INTERVAL_MS,
            FactScalar::DurationMs(1_000),
            false,
        ),
        (PROBE_INTERVAL_MS, FactScalar::DurationMs(60_000), true),
        (PROBE_SAMPLE_KEYS, FactScalar::Integer(1_024), true),
        (REQUEST_INDEX_RECORDS, FactScalar::Integer(4_096), false),
        (
            STATUS_GAP_WINDOW_MS,
            FactScalar::DurationMs(86_400_000),
            true,
        ),
    ];
    for (key, value, settable) in &scalars {
        let parameter = reported(key);
        assert_eq!(&parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert_eq!(parameter.settable, *settable, "{key}");
    }
    let texts = [
        (BUFFER_TIER1_RATIO, json!(0.60)),
        (BUFFER_TIER2_RATIO, json!(0.75)),
        (BUFFER_TIER3_RATIO, json!(0.90)),
        (BUFFER_HYSTERESIS_RATIO, json!(0.10)),
        (WRITE_RETRY_BACKOFF_MS, json!([0, 100, 250, 500, 1000])),
        (TAG_WORKBENCH_PATTERNS, json!(WORKBENCH_PATTERNS)),
        (TAG_REOBSERVE_PREFIXES, json!(REOBSERVE_PREFIXES)),
    ];
    for (key, value) in &texts {
        let parameter = reported(key);
        assert_eq!(&json_text(&parameter.value), value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(!parameter.settable, "{key}");
    }
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("ledger_observer."))
            .count(),
        21
    );

    let config = from_file(json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    let section = config.ledger_observer().expect("the section in effect");
    assert_eq!(section.write_attempts, 5);
    assert_eq!(section.write_retry_backoff_ms, vec![0, 100, 250, 500, 1000]);
    assert_eq!(section.flush_attempt_stall_ms, 120_000);
    assert_eq!(section.health_probe_interval_ms, 1_000);
    assert_eq!(section.dump_max_bytes, 1_073_741_824);
    assert_eq!(section.tag_workbench_patterns, WORKBENCH_PATTERNS);
    assert_eq!(section.tag_reobserve_prefixes, REOBSERVE_PREFIXES);
    // A host started without a manifest reads the same section from the defaults file.
    assert_eq!(EffectiveConfig::default().ledger_observer(), Ok(section));
}

// §7 "configuration check" and §9.1: a value of the wrong type or outside its range is refused
// as the file layer is rendered, a broken section rule by the check a start and check-config
// make, both `config_value_invalid` with the key and why. Through an instruction a key read when
// the ledger opens is `config_key_not_settable`, and a probe value outside its range is refused
// while the configuration in effect stays.
#[test]
fn ao1_keys_ledger_observer_values_that_break_a_rule_are_refused() {
    let refusals = [
        (
            "tier 1 at tier 2",
            json!({"buffer": {"tier1_ratio": 0.75}}),
            BUFFER_TIER1_RATIO,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "tier 2 above tier 3",
            json!({"buffer": {"tier2_ratio": 0.95}}),
            BUFFER_TIER2_RATIO,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "tier 3 above the budget",
            json!({"buffer": {"tier3_ratio": 1.5}}),
            BUFFER_TIER3_RATIO,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "no hysteresis",
            json!({"buffer": {"hysteresis_ratio": 0.0}}),
            BUFFER_HYSTERESIS_RATIO,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a ratio as text",
            json!({"buffer": {"tier1_ratio": "0.6"}}),
            BUFFER_TIER1_RATIO,
            ConfigValueProblem::WrongType,
        ),
        (
            "a spill batch larger than the gap between tiers 2 and 3",
            json!({"spill_batch_bytes": 16_777_216}),
            SPILL_BATCH_BYTES,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "no write attempt",
            json!({"write_attempts": 0}),
            WRITE_ATTEMPTS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "fewer waits than attempts",
            json!({"write_attempts": 2, "write_retry_backoff_ms": [0]}),
            WRITE_RETRY_BACKOFF_MS,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a wait that is no number of milliseconds",
            json!({"write_retry_backoff_ms": [0, 100, 250, 500, -1]}),
            WRITE_RETRY_BACKOFF_MS,
            ConfigValueProblem::WrongType,
        ),
        (
            "a stall limit of zero",
            json!({"flush_attempt_stall_ms": 0}),
            FLUSH_ATTEMPT_STALL_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "an empty tag pattern",
            json!({"tag": {"workbench_patterns": [""]}}),
            TAG_WORKBENCH_PATTERNS,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a prefix that is no text",
            json!({"tag": {"reobserve_prefixes": [1]}}),
            TAG_REOBSERVE_PREFIXES,
            ConfigValueProblem::WrongType,
        ),
    ];
    for (case, section, config_key, problem) in refusals {
        let refused = match render(section) {
            Err(refused) => refused,
            Ok(parameters) => EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters,
            })
            .expect("every key is in the key table")
            .check(&context())
            .expect_err(case),
        };
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        assert_eq!(refused.config_key(), config_key, "{case}");
        assert_eq!(refused.problem(), Some(problem), "{case}");
    }

    // Two attempts with two waits is a whole policy.
    let two = from_file(json!({"write_attempts": 2, "write_retry_backoff_ms": [0, 50]}));
    two.check(&context()).expect("two attempts, two waits");
    let section = two.ledger_observer().expect("the section in effect");
    assert_eq!(
        (section.write_attempts, section.write_retry_backoff_ms),
        (2, vec![0, 50])
    );

    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
    };
    let running = from_file(json!({}));
    let change = |key: &str, value: Value| ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    };
    let refused = running
        .apply(&[change(WRITE_ATTEMPTS, json!(3))], &stamp, &context())
        .expect_err("a write-path key is read when the ledger opens");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigKeyNotSettable);
    assert_eq!(refused.config_key(), WRITE_ATTEMPTS);
    let refused = running
        .apply(&[change(PROBE_INTERVAL_MS, json!(0))], &stamp, &context())
        .expect_err("a probe period of zero");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), PROBE_INTERVAL_MS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));
    assert_eq!(
        running.ledger_observer(),
        from_file(json!({})).ledger_observer()
    );
}
