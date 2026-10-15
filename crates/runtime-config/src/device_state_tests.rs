// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-5b (model-398 v1.1 §7.8; `contracts/v012-register.md` §1.59): every
// `device_state` key is parsed, checked and defaulted from the defaults file with the model's
// starting values (`in_use_gate` off: the handset is a dedicated one, Alice 2026-10-13 on §16
// item 2), and each may change while the Runtime runs. The section is checked whole: the fact
// TTL holds at least one sampling interval and the thermal resume level is below the hold level.

use super::*;
use crate::device_state::{
    BATTERY_MIN_PERCENT, FACT_TTL_MS, IN_USE_BACKOFF_MS, IN_USE_GATE, KEEPALIVE_MARGIN_MS,
    KEYGUARD_INSECURE, SAMPLE_INTERVAL_MS, THERMAL_HOLD_AT, THERMAL_RESUME_BELOW, WAKE_SETTLE_MS,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RuntimeConfigManifest,
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

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

// The rows a start renders from a file whose `device_state` section is `section`.
fn render(section: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &shipped(),
        &json!({ "device_state": section }),
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

// §7.8: every key of the section, defaulted from the defaults file (source `default`) with the
// model's starting value and settable; the section in effect reads the same values, passes its
// check, and is what a host started without a manifest reads.
#[test]
fn rd5b_keys_every_device_state_key_has_its_model_default() {
    let parameters = render(json!({})).expect("the defaults resolve");
    let expected = [
        (SAMPLE_INTERVAL_MS, FactScalar::DurationMs(60_000)),
        (FACT_TTL_MS, FactScalar::DurationMs(180_000)),
        (WAKE_SETTLE_MS, FactScalar::DurationMs(1_000)),
        (KEEPALIVE_MARGIN_MS, FactScalar::DurationMs(60_000)),
        (KEYGUARD_INSECURE, FactScalar::String("dismiss".to_owned())),
        (BATTERY_MIN_PERCENT, FactScalar::Integer(15)),
        (THERMAL_HOLD_AT, FactScalar::String("severe".to_owned())),
        (
            THERMAL_RESUME_BELOW,
            FactScalar::String("moderate".to_owned()),
        ),
        (IN_USE_GATE, FactScalar::Boolean(false)),
        (IN_USE_BACKOFF_MS, FactScalar::DurationMs(300_000)),
    ];
    for (key, value) in &expected {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == *key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(&parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("device_state."))
            .count(),
        expected.len()
    );

    let config = from_file(json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    let section = config.device_state().expect("the section in effect");
    assert_eq!(
        section,
        DeviceStateConfig {
            sample_interval_ms: 60_000,
            fact_ttl_ms: 180_000,
            wake_settle_ms: 1_000,
            keepalive_margin_ms: 60_000,
            keyguard_insecure: KeyguardInsecureAction::Dismiss,
            battery_min_percent: 15,
            thermal_hold_at: ThermalLevel::Severe,
            thermal_resume_below: ThermalLevel::Moderate,
            in_use_gate: false,
            in_use_backoff_ms: 300_000,
        }
    );
    assert_eq!(EffectiveConfig::default().device_state(), Ok(section));

    // The other words of the two word keys, and a gate that is on.
    let other = from_file(json!({
        "keyguard_insecure": "wait",
        "thermal_hold_at": "critical",
        "thermal_resume_below": "light",
        "in_use_gate": true
    }));
    other.check(&context()).expect("other words pass");
    let section = other.device_state().expect("the section in effect");
    assert_eq!(
        (
            section.keyguard_insecure,
            section.thermal_hold_at,
            section.thermal_resume_below,
            section.in_use_gate
        ),
        (
            KeyguardInsecureAction::Wait,
            ThermalLevel::Critical,
            ThermalLevel::Light,
            true
        )
    );
}

// §7.8 "校验": a value of the wrong type or outside its range is refused as the file layer is
// rendered, a broken section rule by the check, both `config_value_invalid` with the key and why.
#[test]
fn rd5b_keys_device_state_values_that_break_a_rule_are_refused() {
    let refusals = [
        (
            "a sampling interval under five seconds",
            json!({"sample_interval_ms": 4_999}),
            SAMPLE_INTERVAL_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a sampling interval over an hour",
            json!({"sample_interval_ms": 3_600_001}),
            SAMPLE_INTERVAL_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a fact TTL shorter than the sampling interval",
            json!({"fact_ttl_ms": 59_999}),
            FACT_TTL_MS,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a settle wait under a tenth of a second",
            json!({"wake_settle_ms": 99}),
            WAKE_SETTLE_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a settle wait over ten seconds",
            json!({"wake_settle_ms": 10_001}),
            WAKE_SETTLE_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a keyguard action of neither word",
            json!({"keyguard_insecure": "enter_pin"}),
            KEYGUARD_INSECURE,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a battery level over a hundred percent",
            json!({"battery_min_percent": 101}),
            BATTERY_MIN_PERCENT,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a thermal level outside the thermal words",
            json!({"thermal_hold_at": "hot"}),
            THERMAL_HOLD_AT,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a resume level not below the hold level",
            json!({"thermal_hold_at": "moderate", "thermal_resume_below": "moderate"}),
            THERMAL_RESUME_BELOW,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "an in-use gate that is no boolean",
            json!({"in_use_gate": "on"}),
            IN_USE_GATE,
            ConfigValueProblem::WrongType,
        ),
        (
            "an in-use backoff under ten seconds",
            json!({"in_use_backoff_ms": 9_999}),
            IN_USE_BACKOFF_MS,
            ConfigValueProblem::OutOfRange,
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
}
