// SPDX-License-Identifier: AGPL-3.0-only

// SCH-M slice ED-1 (frozen model v1.0 §3.4 "Configuration", §6 row ED-1; Alice 10-22 ruling 39
// preface: 「这个是按实例指定的…随时可以通过配置文件调」): the expected-duration keys. Every expected
// value is the model's table: `scheduling.expected_runtime.enabled` `true`, `.runs` 5,
// `.margin_ms` 300 000, each also per instance (`instance.<id>.scheduling.expected_runtime.*`)
// defaulting to the section's and overriding it where the instance names its own;
// `scheduling.fact_refresh.expected_duration_ms` 60 000; every key settable.

use super::*;
use crate::scheduling::{
    EXPECTED_RUNTIME_ENABLED, EXPECTED_RUNTIME_MARGIN_MS, EXPECTED_RUNTIME_RUNS,
    ExpectedRuntimeConfig, FACT_REFRESH_EXPECTED_DURATION_MS,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

// The rows a start renders from a file of `scheduling` and, for instances A and B, their own
// `scheduling` members.
fn render(scheduling: Value, own_a: Value) -> Vec<ConfigParameter> {
    let mut a = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if !own_a.is_null() {
        a["scheduling"] = own_a;
    }
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &DefaultsFile::shipped()
            .expect("the shipped defaults file parses")
            .1,
        &json!({"scheduling": scheduling, "instances": [a, b]}),
        &|_| None,
    )
    .expect("every key resolves")
}

fn from_file(scheduling: Value, own_a: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(scheduling, own_a),
    })
    .expect("every key is in the key table")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

// §3.4 "Configuration": the section's keys have the model's defaults and every instance's keys
// default to them; an instance's own key overrides the section's for that instance only, from
// the file and through an instruction, and a change of the section's key reaches every
// instance that names none.
#[test]
fn ed1_c1_per_instance_keys_override_the_section() {
    let parameters = render(json!({}), Value::Null);
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    for (key, value) in [
        (EXPECTED_RUNTIME_ENABLED, FactScalar::Boolean(true)),
        (EXPECTED_RUNTIME_RUNS, FactScalar::Integer(5)),
        (EXPECTED_RUNTIME_MARGIN_MS, FactScalar::DurationMs(300_000)),
        (
            FACT_REFRESH_EXPECTED_DURATION_MS,
            FactScalar::DurationMs(60_000),
        ),
    ] {
        let parameter = reported(key);
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    for member in [
        EXPECTED_RUNTIME_ENABLED,
        EXPECTED_RUNTIME_RUNS,
        EXPECTED_RUNTIME_MARGIN_MS,
    ] {
        for instance in [INSTANCE_A, INSTANCE_B] {
            let parameter = reported(&instance_key(instance, member));
            assert_eq!(parameter.value, reported(member).value, "{member}");
            assert_eq!(parameter.source, ConfigParameterSource::Default, "{member}");
            assert!(parameter.settable, "{member}");
        }
    }
    let section = ExpectedRuntimeConfig {
        enabled: true,
        runs: 5,
        margin_ms: 300_000,
    };
    let defaults = from_file(json!({}), Value::Null);
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.expected_runtime(INSTANCE_A), Ok(section));
    assert_eq!(
        defaults
            .scheduling()
            .map(|scheduling| scheduling.expected_runtime),
        Ok(section)
    );
    assert_eq!(
        defaults
            .fact_refresh()
            .map(|fact_refresh| fact_refresh.expected_duration_ms),
        Ok(60_000)
    );

    // Instance A names its own runs and margin; B names none.
    let own = from_file(
        json!({}),
        json!({"expected_runtime": {"runs": 3, "margin_ms": 0}}),
    );
    own.check(&context()).expect("an instance's own keys pass");
    let a = ExpectedRuntimeConfig {
        enabled: true,
        runs: 3,
        margin_ms: 0,
    };
    assert_eq!(own.expected_runtime(INSTANCE_A), Ok(a));
    assert_eq!(own.expected_runtime(INSTANCE_B), Ok(section));

    // Through instructions: the section's runs reach B only; A's own switch turns E off for A.
    let changed = own
        .apply(
            &[
                ConfigSettingChange {
                    key: EXPECTED_RUNTIME_RUNS.to_owned(),
                    value: Some(json!(8)),
                },
                ConfigSettingChange {
                    key: instance_key(INSTANCE_A, EXPECTED_RUNTIME_ENABLED),
                    value: Some(json!(false)),
                },
            ],
            &stamp(),
            &context(),
        )
        .expect("both keys are settable")
        .config;
    assert_eq!(
        changed.expected_runtime(INSTANCE_A),
        Ok(ExpectedRuntimeConfig {
            enabled: false,
            ..a
        })
    );
    assert_eq!(
        changed.expected_runtime(INSTANCE_B),
        Ok(ExpectedRuntimeConfig { runs: 8, ..section })
    );
}
