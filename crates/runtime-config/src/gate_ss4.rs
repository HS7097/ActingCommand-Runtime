// SPDX-License-Identifier: AGPL-3.0-only

// SS-4 (register §1.231; design `rd-1029/design-sch-state.md` §4.7 "形态"; coordinator work order
// `rd-1031/brief-ss4.md`, test 5): the switch `scheduling.no_progress.enabled`, default `true`,
// also per instance (`instance.<id>.scheduling.no_progress.enabled`) defaulting to the section's
// and overriding it where the instance names its own, from the file and through an instruction;
// both settable.

use super::*;
use crate::scheduling::NO_PROGRESS_ENABLED;
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

// The rows a start renders from a file of `scheduling` and, for instance A, its own `scheduling`
// member; instance B names none.
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

// Test 5 (the configuration): the guard is on by default for every instance; the file turns it
// off for one instance or for the section; an instruction turns it off for one instance and the
// section's change reaches only the instances that name none.
#[test]
fn ss4_t5_the_no_progress_switch_defaults_on_and_an_instance_overrides_it() {
    let parameters = render(json!({}), Value::Null);
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    let section = reported(NO_PROGRESS_ENABLED);
    assert_eq!(section.value, FactScalar::Boolean(true));
    assert_eq!(section.source, ConfigParameterSource::Default);
    assert!(section.settable);
    for instance in [INSTANCE_A, INSTANCE_B] {
        let own = reported(&instance_key(instance, NO_PROGRESS_ENABLED));
        assert_eq!(own.value, FactScalar::Boolean(true), "{instance}");
        assert_eq!(own.source, ConfigParameterSource::Default, "{instance}");
        assert!(own.settable, "{instance}");
    }
    let defaults = from_file(json!({}), Value::Null);
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.no_progress_enabled(INSTANCE_A), Ok(true));
    assert_eq!(
        defaults
            .scheduling()
            .map(|scheduling| scheduling.no_progress_enabled),
        Ok(true)
    );

    // The file: A turns it off for itself; the section's switch off reaches B, while A's own on
    // stays on.
    let own_off = from_file(json!({}), json!({"no_progress": {"enabled": false}}));
    own_off
        .check(&context())
        .expect("an instance's own key passes");
    assert_eq!(own_off.no_progress_enabled(INSTANCE_A), Ok(false));
    assert_eq!(own_off.no_progress_enabled(INSTANCE_B), Ok(true));
    let section_off = from_file(
        json!({"no_progress": {"enabled": false}}),
        json!({"no_progress": {"enabled": true}}),
    );
    section_off
        .check(&context())
        .expect("the section's key passes");
    assert_eq!(section_off.no_progress_enabled(INSTANCE_A), Ok(true));
    assert_eq!(section_off.no_progress_enabled(INSTANCE_B), Ok(false));

    // Through an instruction: A's own switch off; B keeps the section's.
    let changed = defaults
        .apply(
            &[ConfigSettingChange {
                key: instance_key(INSTANCE_A, NO_PROGRESS_ENABLED),
                value: Some(json!(false)),
            }],
            &stamp(),
            &context(),
        )
        .expect("the key is settable")
        .config;
    assert_eq!(changed.no_progress_enabled(INSTANCE_A), Ok(false));
    assert_eq!(changed.no_progress_enabled(INSTANCE_B), Ok(true));
}
