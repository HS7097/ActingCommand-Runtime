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
    FLUSH_BATCH_RECORDS_BUSY, FLUSH_LAG_WARN_MS, HEALTH_PROBE_INTERVAL_MS, OPEN_TIMEOUT_MS,
    OPEN_WARN_RATIO_MILLI, PROBE_INTERVAL_MS, PROBE_SAMPLE_KEYS, READ_BUDGET_MS,
    REQUEST_INDEX_RECORDS, SCRUB_CHUNK_ROWS, SCRUB_PASS_INTERVAL_MS, SCRUB_ROWS_PER_SECOND,
    SPILL_BATCH_BYTES, STATUS_GAP_WINDOW_MS, TAG_REOBSERVE_PREFIXES, TAG_WORKBENCH_PATTERNS,
    WRITE_ATTEMPTS, WRITE_RETRY_BACKOFF_MS, WRITER_REPLY_TIMEOUT_MS,
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
const WORKBENCH_PATTERNS: [&str; 11] = [
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
    "host.contained_settlement",
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
    // The 21 keys of §15, #411 A51's `read_budget_ms` and `close_timeout_ms`, P0-LO's
    // `open_timeout_ms` and `open_warn_ratio_milli`, LN-5's `history_page_rows`, LN-TUNE's
    // `writer_reply_timeout_ms`, LN-8's `open_verify_tail_records`, LN-9's three scrub keys and
    // LN-WS's `read_check_vm_steps` and `history_scan_rows`.
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("ledger_observer."))
            .count(),
        33
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
        channel: None,
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

// P0-LO (brief model items 1, 3, 4): a file that names neither key, a revision-5 section, is
// accepted and the open's deadline is 600 000 ms and its warning ratio 500 from the defaults
// file; an explicit in-range value is taken; neither key may be changed while running.
#[test]
fn p0_lo_the_open_deadline_and_its_warning_ratio_default_and_take_an_explicit_value() {
    // A revision-5 section and one that names both keys are both members the file accepts.
    serde_json::from_value::<LedgerObserverFile>(json!({"close_timeout_ms": 60_000}))
        .expect("a section that names neither key");
    serde_json::from_value::<LedgerObserverFile>(
        json!({"open_timeout_ms": 120_000, "open_warn_ratio_milli": 800}),
    )
    .expect("a section that names both keys");

    fn reported(parameters: &[ConfigParameter], key: &str) -> ConfigParameter {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .cloned()
            .unwrap_or_else(|| panic!("no parameter {key}"))
    }
    let parameters = render(json!({})).expect("a file that names neither key resolves");
    for (key, value) in [
        (OPEN_TIMEOUT_MS, FactScalar::DurationMs(600_000)),
        (OPEN_WARN_RATIO_MILLI, FactScalar::Integer(500)),
    ] {
        let parameter = reported(&parameters, key);
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(!parameter.settable, "{key}");
    }
    let defaulted = from_file(json!({}));
    defaulted
        .check(&context())
        .expect("the defaults pass the check");
    let section = defaulted.ledger_observer().expect("the section in effect");
    assert_eq!(
        (section.open_timeout_ms, section.open_warn_ratio_milli),
        (600_000, 500)
    );
    // A host started without a manifest reads the same values from the defaults file.
    assert_eq!(EffectiveConfig::default().ledger_observer(), Ok(section));

    let explicit = json!({"open_timeout_ms": 120_000, "open_warn_ratio_milli": 800});
    let parameters = render(explicit.clone()).expect("in-range values resolve");
    for (key, value) in [
        (OPEN_TIMEOUT_MS, FactScalar::DurationMs(120_000)),
        (OPEN_WARN_RATIO_MILLI, FactScalar::Integer(800)),
    ] {
        let parameter = reported(&parameters, key);
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Explicit, "{key}");
    }
    let configured = from_file(explicit);
    configured
        .check(&context())
        .expect("in-range values pass the check");
    let section = configured.ledger_observer().expect("the section in effect");
    assert_eq!(
        (section.open_timeout_ms, section.open_warn_ratio_milli),
        (120_000, 800)
    );

    // Start-time only: an instruction is refused for each key and the values in effect stay.
    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    };
    for (key, value) in [
        (OPEN_TIMEOUT_MS, json!(300_000)),
        (OPEN_WARN_RATIO_MILLI, json!(700)),
    ] {
        let refused = configured
            .apply(
                &[ConfigSettingChange {
                    key: key.to_owned(),
                    value: Some(value),
                }],
                &stamp,
                &context(),
            )
            .expect_err("the key is read when the ledger opens");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigKeyNotSettable,
            "{key}"
        );
        assert_eq!(refused.config_key(), key);
    }
    assert_eq!(configured.ledger_observer(), Ok(section));
}

// P0-LO (brief model items 1, 3): a deadline below 10 000 ms or above 600 000 ms and a warning
// ratio below 100 or above 1 000 are refused as `config_value_invalid` (`out_of_range`) for
// their key.
#[test]
fn p0_lo_open_values_outside_their_ranges_are_refused() {
    for (section, config_key) in [
        (json!({"open_timeout_ms": 9_999}), OPEN_TIMEOUT_MS),
        (json!({"open_timeout_ms": 600_001}), OPEN_TIMEOUT_MS),
        (json!({"open_warn_ratio_milli": 99}), OPEN_WARN_RATIO_MILLI),
        (
            json!({"open_warn_ratio_milli": 1_001}),
            OPEN_WARN_RATIO_MILLI,
        ),
    ] {
        let case = section.to_string();
        let refused = render(section).expect_err(&case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        assert_eq!(refused.config_key(), config_key, "{case}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::OutOfRange),
            "{case}"
        );
    }
}

// #422 LN-TUNE (brief rd-1026/brief-ln-tune.md item 1): `writer_reply_timeout_ms` is read from
// the defaults file (10 000 ms) and from an explicit value, is read when the ledger opens (an
// instruction is refused), and a value below 1 000 ms or above 600 000 ms is refused as
// `config_value_invalid` (`out_of_range`); a read budget at the reply limit in effect breaks the
// section's rule that a read stops before its caller stops waiting.
#[test]
fn ln_tune_the_writer_reply_timeout_is_read_and_refuses_an_out_of_range_value() {
    let defaulted = from_file(json!({}));
    defaulted
        .check(&context())
        .expect("the defaults pass the check");
    let section = defaulted.ledger_observer().expect("the section in effect");
    assert_eq!(section.writer_reply_timeout_ms, 10_000);
    let parameter = render(json!({}))
        .expect("the defaults resolve")
        .into_iter()
        .find(|parameter| parameter.key == WRITER_REPLY_TIMEOUT_MS)
        .expect("the key is reported");
    assert_eq!(parameter.value, FactScalar::DurationMs(10_000));
    assert_eq!(parameter.source, ConfigParameterSource::Default);
    assert!(!parameter.settable);

    let configured = from_file(json!({"writer_reply_timeout_ms": 30_000}));
    configured
        .check(&context())
        .expect("an in-range value passes the check");
    let section = configured.ledger_observer().expect("the section in effect");
    assert_eq!(section.writer_reply_timeout_ms, 30_000);
    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    };
    let refused = configured
        .apply(
            &[ConfigSettingChange {
                key: WRITER_REPLY_TIMEOUT_MS.to_owned(),
                value: Some(json!(60_000)),
            }],
            &stamp,
            &context(),
        )
        .expect_err("the key is read when the ledger opens");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigKeyNotSettable);
    assert_eq!(configured.ledger_observer(), Ok(section));

    for value in [999, 600_001] {
        let case = format!("writer_reply_timeout_ms {value}");
        let refused = render(json!({"writer_reply_timeout_ms": value})).expect_err(&case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        assert_eq!(refused.config_key(), WRITER_REPLY_TIMEOUT_MS, "{case}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::OutOfRange),
            "{case}"
        );
    }

    let refused = from_file(json!({"writer_reply_timeout_ms": 1_000, "read_budget_ms": 1_000}))
        .check(&context())
        .expect_err("a read budget at the reply limit");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), READ_BUDGET_MS);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
}

// #422 LN-9 (LEAN-L §3.7, §6; Alice's ruling 57): the scrub's three keys are read from the defaults
// file (625 rows a second, chunks of 256 rows, a pass every 2 400 000 ms) and from explicit values;
// the rate is settable (an instruction in its range takes effect, one out of it is refused and the
// rate in effect stays), the chunk and the pass interval are read when the ledger opens; a value
// outside its range is `config_value_invalid` (`out_of_range`).
#[test]
fn ln9_the_scrub_keys_default_take_explicit_values_and_refuse_out_of_range_values() {
    let defaulted = from_file(json!({}));
    defaulted
        .check(&context())
        .expect("the defaults pass the check");
    let section = defaulted.ledger_observer().expect("the section in effect");
    assert_eq!(
        (
            section.scrub_rows_per_second,
            section.scrub_chunk_rows,
            section.scrub_pass_interval_ms
        ),
        (625, 256, 2_400_000)
    );
    let parameters = render(json!({})).expect("the defaults resolve");
    for (key, value, settable) in [
        (SCRUB_ROWS_PER_SECOND, FactScalar::Integer(625), true),
        (SCRUB_CHUNK_ROWS, FactScalar::Integer(256), false),
        (
            SCRUB_PASS_INTERVAL_MS,
            FactScalar::DurationMs(2_400_000),
            false,
        ),
    ] {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert_eq!(parameter.settable, settable, "{key}");
    }

    let configured = from_file(json!({
        "scrub_rows_per_second": 1_000,
        "scrub_chunk_rows": 512,
        "scrub_pass_interval_ms": 3_600_000
    }));
    configured
        .check(&context())
        .expect("in-range values pass the check");
    let section = configured.ledger_observer().expect("the section in effect");
    assert_eq!(
        (
            section.scrub_rows_per_second,
            section.scrub_chunk_rows,
            section.scrub_pass_interval_ms
        ),
        (1_000, 512, 3_600_000)
    );

    let stamp = InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    };
    let change = |key: &str, value: Value| ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    };
    let applied = configured
        .apply(
            &[change(SCRUB_ROWS_PER_SECOND, json!(250))],
            &stamp,
            &context(),
        )
        .expect("the rate is settable");
    assert_eq!(
        applied
            .config
            .ledger_observer()
            .expect("the section in effect")
            .scrub_rows_per_second,
        250
    );
    let refused = configured
        .apply(
            &[change(SCRUB_ROWS_PER_SECOND, json!(100_001))],
            &stamp,
            &context(),
        )
        .expect_err("a rate out of range");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::OutOfRange));
    for (key, value) in [
        (SCRUB_CHUNK_ROWS, json!(128)),
        (SCRUB_PASS_INTERVAL_MS, json!(600_000)),
    ] {
        let refused = configured
            .apply(&[change(key, value)], &stamp, &context())
            .expect_err("the key is read when the ledger opens");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigKeyNotSettable,
            "{key}"
        );
    }
    assert_eq!(configured.ledger_observer(), Ok(section));

    for (section, config_key) in [
        (json!({"scrub_rows_per_second": 0}), SCRUB_ROWS_PER_SECOND),
        (
            json!({"scrub_rows_per_second": 100_001}),
            SCRUB_ROWS_PER_SECOND,
        ),
        (json!({"scrub_chunk_rows": 0}), SCRUB_CHUNK_ROWS),
        (json!({"scrub_chunk_rows": 4_097}), SCRUB_CHUNK_ROWS),
        (
            json!({"scrub_pass_interval_ms": 59_999}),
            SCRUB_PASS_INTERVAL_MS,
        ),
        (
            json!({"scrub_pass_interval_ms": 86_400_001}),
            SCRUB_PASS_INTERVAL_MS,
        ),
    ] {
        let case = section.to_string();
        let refused = render(section).expect_err(&case);
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        assert_eq!(refused.config_key(), config_key, "{case}");
        assert_eq!(
            refused.problem(),
            Some(ConfigValueProblem::OutOfRange),
            "{case}"
        );
    }
}
