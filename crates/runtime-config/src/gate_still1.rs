// SPDX-License-Identifier: AGPL-3.0-only

// STILL-1 (Workflow #407; coordinator brief rd-1031/check-speed/brief-slices.md, decision 1; design
// model-check.md §7, §8; register §1.249): `execution.capture.still_limit_ms` is off by default, 0
// or `30000..=1800000`, settable while the Runtime runs, and also per instance
// (`instance.<id>.execution.capture.still_limit_ms`), defaulting to the section's and overriding it
// where the instance names its own, from the file and through an instruction. A value outside the
// range is `config_value_invalid` (`out_of_range`) for the key that holds it: the key table refuses
// a value outside `0..=1800000` as it renders the file, and the whole-section check refuses a value
// between 0 and 30000, at a start, in `check-config` and in an instruction.

use super::*;
use crate::execution::CAPTURE_STILL_LIMIT_MS;
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

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
        channel: None,
    }
}

// The rows a start renders from a file of `capture` and, for instance A, its own still limit;
// instance B names none.
fn render(capture: Value, own_a: Option<u64>) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut a = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if let Some(own_a) = own_a {
        a["execution"] = json!({"capture": {"still_limit_ms": own_a}});
    }
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &defaults,
        &json!({"execution": {"capture": capture}, "instances": [a, b]}),
        &|_| None,
    )?;
    annotate_settable(&mut parameters)?;
    Ok(parameters)
}

fn from_file(capture: Value, own_a: Option<u64>) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(capture, own_a).expect("every key resolves"),
    })
    .expect("every key is in the key table")
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn assert_out_of_range(refused: &ConfigFailure, key: &str, case: &str) {
    assert_eq!(
        refused.failure_code(),
        ConfigCode::ConfigValueInvalid,
        "{case}"
    );
    assert_eq!(refused.config_key(), key, "{case}");
    assert_eq!(
        refused.problem(),
        Some(ConfigValueProblem::OutOfRange),
        "{case}"
    );
}

#[test]
fn still_limit_key_range_and_default() {
    let own_key = instance_key(INSTANCE_A, CAPTURE_STILL_LIMIT_MS);

    // The default: 0 from the defaults file, settable, for the section and for every instance.
    let parameters = render(json!({}), None).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    for key in [
        CAPTURE_STILL_LIMIT_MS.to_owned(),
        own_key.clone(),
        instance_key(INSTANCE_B, CAPTURE_STILL_LIMIT_MS),
    ] {
        let parameter = reported(&key);
        assert_eq!(parameter.value, FactScalar::DurationMs(0), "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    let defaults = from_file(json!({}), None);
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.still_limit_ms(INSTANCE_A), Ok(0));
    assert_eq!(EffectiveConfig::default().still_limit_ms(INSTANCE_A), Ok(0));

    // The table's bounds refuse a value as the file is rendered (a start, `check-config`).
    let refused = render(json!({"still_limit_ms": 1_800_001}), None).expect_err("above the range");
    assert_out_of_range(&refused, CAPTURE_STILL_LIMIT_MS, "the section above");
    let refused = render(json!({}), Some(1_800_001)).expect_err("an instance above the range");
    assert_out_of_range(&refused, &own_key, "an instance above");

    // Between 0 and 30 000 is refused by the whole-section check, for the section and for an
    // instance, from the file and through an instruction, naming the on range.
    for value in [1, 29_999] {
        let case = format!("{value} ms");
        let refused = from_file(json!({"still_limit_ms": value}), None)
            .check(&context())
            .expect_err(&case);
        assert_out_of_range(&refused, CAPTURE_STILL_LIMIT_MS, &case);
        assert!(
            refused
                .outcome()
                .value("range_min")
                .is_some_and(|min| *min == json!(30_000)),
            "{case}: {:?}",
            refused.outcome()
        );

        let refused = from_file(json!({}), Some(value))
            .check(&context())
            .expect_err(&case);
        assert_out_of_range(&refused, &own_key, &case);

        for key in [CAPTURE_STILL_LIMIT_MS, own_key.as_str()] {
            let refused = defaults
                .apply(&[set(key, json!(value))], &stamp(), &context())
                .expect_err(&case);
            assert_out_of_range(&refused, key, &case);
        }
    }

    // Each edge of the on range and 0 are taken, and the key in effect reads them.
    for value in [0, 30_000, 1_800_000] {
        let applied = defaults
            .apply(
                &[set(CAPTURE_STILL_LIMIT_MS, json!(value))],
                &stamp(),
                &context(),
            )
            .expect("a value within the range is set");
        assert_eq!(applied.config.still_limit_ms(INSTANCE_A), Ok(value));
        assert!(applied.sections.contains("execution"));
    }

    // The file: A names its own limit; the section's reaches B, and A's own 0 keeps A off.
    let own = from_file(json!({}), Some(60_000));
    own.check(&context()).expect("an instance's own key passes");
    assert_eq!(own.still_limit_ms(INSTANCE_A), Ok(60_000));
    assert_eq!(own.still_limit_ms(INSTANCE_B), Ok(0));
    let section = from_file(json!({"still_limit_ms": 120_000}), Some(0));
    section.check(&context()).expect("the section's key passes");
    assert_eq!(section.still_limit_ms(INSTANCE_A), Ok(0));
    assert_eq!(section.still_limit_ms(INSTANCE_B), Ok(120_000));

    // Through an instruction: A's own limit, and B keeps the section's.
    let changed = defaults
        .apply(&[set(&own_key, json!(60_000))], &stamp(), &context())
        .expect("the key is settable")
        .config;
    assert_eq!(changed.still_limit_ms(INSTANCE_A), Ok(60_000));
    assert_eq!(changed.still_limit_ms(INSTANCE_B), Ok(0));
}
