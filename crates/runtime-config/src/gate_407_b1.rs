// SPDX-License-Identifier: AGPL-3.0-only

// PERF-B1 (Workflow #407 B1; host gate key 411; coordinator brief rd-1024/brief-perf-b1-capture.md,
// test row (d); requirements R2, R3): the kernel's capture keys are configuration keys of the key
// table. `execution.capture.max_interval_ms`, the largest capture interval a package may declare,
// defaults to 60 000 ms (R2); `execution.capture.backoff_enabled`, the back-off's switch, to on
// (R3); `execution.capture.idle_ceiling_ms`, the back-off's ceiling, to 5 000 ms (R3). Each comes
// from the defaults file and is settable. A value outside its bounds is `config_value_invalid`
// (`out_of_range`) for its key when the file layer is rendered (a start, `check-config`), in the
// whole-section check and in an instruction: the largest declarable interval is 5 000 to
// 1 800 000 ms (at least the bound every package was held to before, at most the longest wait a
// step may declare), the idle ceiling 50 to 60 000 ms; a switch that is no boolean is
// `wrong_type`. The kernel's refusal of a package above the ceiling is exec key 184.

use super::*;
use crate::execution::{CAPTURE_BACKOFF_ENABLED, CAPTURE_IDLE_CEILING_MS, CAPTURE_MAX_INTERVAL_MS};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

// The rows a start renders from a file whose `execution.capture` member is `capture`.
fn render(capture: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "execution": { "capture": capture } }),
        &|_| None,
    )?;
    annotate_settable(&mut parameters)?;
    Ok(parameters)
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn assert_refused(refused: &ConfigFailure, key: &str, problem: ConfigValueProblem, case: &str) {
    assert_eq!(
        refused.failure_code(),
        ConfigCode::ConfigValueInvalid,
        "{case}"
    );
    assert_eq!(refused.config_key(), key, "{case}");
    assert_eq!(refused.problem(), Some(problem), "{case}");
}

#[test]
fn b1_d_the_capture_keys_default_from_the_file_and_their_bounds_are_checked() {
    // The defaults: from the defaults file (source `default`), settable, and the keys in effect
    // read them, as a host started without a manifest does.
    let parameters = render(json!({})).expect("the defaults resolve");
    let defaults = [
        (CAPTURE_MAX_INTERVAL_MS, FactScalar::DurationMs(60_000)),
        (CAPTURE_BACKOFF_ENABLED, FactScalar::Boolean(true)),
        (CAPTURE_IDLE_CEILING_MS, FactScalar::DurationMs(5_000)),
    ];
    for (key, value) in &defaults {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == *key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(parameter.value, *value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("execution."))
            .count(),
        defaults.len()
    );
    let expected = CaptureCadenceConfig {
        max_interval_ms: 60_000,
        backoff_enabled: true,
        idle_ceiling_ms: 5_000,
    };
    let config = EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    })
    .expect("every key is in the key table");
    config
        .check(&context())
        .expect("the defaults pass the check");
    assert_eq!(config.capture_cadence(), Ok(expected));
    assert_eq!(EffectiveConfig::default().capture_cadence(), Ok(expected));

    // Each bound, in the file (rendered at a start and by `check-config`), in a manifest's value
    // (the whole-section check) and in an instruction.
    let bounds = [
        ("max_interval_ms", CAPTURE_MAX_INTERVAL_MS, 4_999, 1_800_001),
        ("idle_ceiling_ms", CAPTURE_IDLE_CEILING_MS, 49, 60_001),
    ];
    for (member, key, below, above) in bounds {
        for value in [below, above] {
            let case = format!("{key} = {value}");
            let refused = render(json!({ member: value })).expect_err(&case);
            assert_refused(&refused, key, ConfigValueProblem::OutOfRange, &case);

            let refused = EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters: vec![ConfigParameter {
                    key: key.to_owned(),
                    value: FactScalar::DurationMs(value),
                    source: ConfigParameterSource::Explicit,
                    settable: true,
                }],
            })
            .expect("the key is in the key table")
            .check(&context())
            .expect_err(&case);
            assert_refused(&refused, key, ConfigValueProblem::OutOfRange, &case);

            let refused = config
                .apply(&[set(key, json!(value))], &stamp(), &context())
                .expect_err(&case);
            assert_refused(&refused, key, ConfigValueProblem::OutOfRange, &case);
        }
    }
    let refused = render(json!({"backoff_enabled": "on"})).expect_err("a switch of text");
    assert_refused(
        &refused,
        CAPTURE_BACKOFF_ENABLED,
        ConfigValueProblem::WrongType,
        "a switch of text",
    );

    // Every key is settable: instructions at each bound's edge are taken, and the keys in effect
    // read them.
    for (max_interval_ms, backoff_enabled, idle_ceiling_ms) in
        [(5_000, false, 50), (1_800_000, true, 60_000)]
    {
        let applied = config
            .apply(
                &[
                    set(CAPTURE_MAX_INTERVAL_MS, json!(max_interval_ms)),
                    set(CAPTURE_BACKOFF_ENABLED, json!(backoff_enabled)),
                    set(CAPTURE_IDLE_CEILING_MS, json!(idle_ceiling_ms)),
                ],
                &stamp(),
                &context(),
            )
            .expect("values within their bounds are set");
        assert_eq!(
            applied.config.capture_cadence(),
            Ok(CaptureCadenceConfig {
                max_interval_ms,
                backoff_enabled,
                idle_ceiling_ms,
            })
        );
        assert!(applied.sections.contains("execution"));
    }
}
