// SPDX-License-Identifier: AGPL-3.0-only

// LOAD-3 (Workflow #407, register §1.260; design `rd-1031/check-speed/model-loading.md` §3.6 and
// §7; coordinator brief `brief-load-3.md`): `execution.loading.mode`, `off`, `shadow` or `on`. Its
// default comes from the defaults file and is `off`. It is settable, read at each step, and may be
// overridden per instance in the file and by an instruction; an instance that names none takes the
// section's. Another word, or a value of another type, is refused as `config_value_invalid` for
// the key that holds it.

use super::*;
use crate::execution::{LOADING_MODE, LoadingModeConfig};
use actingcommand_contract::{ConfigParameter, ConfigSettingChange, RuntimeConfigManifest};
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

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

// The rows a start renders from a file of the section's `loading` member `section` and instance
// A's own `own_a`; instance B names none.
fn render(section: Value, own_a: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let a = json!({"instance_id": INSTANCE_A, "alias": "node.a",
                   "execution": {"loading": own_a}});
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &DefaultsFile::shipped()
            .expect("the shipped defaults file parses")
            .1,
        &json!({"execution": {"loading": section}, "instances": [a, b]}),
        &|_| None,
    )
}

fn from_file(section: Value, own_a: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(section, own_a).expect("every key resolves"),
    })
    .expect("every key is in the key table")
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
fn loading_mode_key_default_off_and_per_instance_override() {
    // The default: `off` from the defaults file, for the section and for an instance that names
    // none, which reports no row of its own.
    let defaults = from_file(json!({}), json!({}));
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.loading_mode_section(), Ok(LoadingModeConfig::Off));
    assert_eq!(
        defaults.loading_mode(INSTANCE_A),
        Ok(LoadingModeConfig::Off)
    );
    assert_eq!(
        EffectiveConfig::default().loading_mode_section(),
        Ok(LoadingModeConfig::Off)
    );
    let rows = render(json!({}), json!({})).expect("the defaults render");
    assert_eq!(
        rows.iter()
            .filter(|row| row.key.ends_with(LOADING_MODE))
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        [LOADING_MODE],
        "one row, the section's"
    );

    // Another word and another type, in the file as a start renders it and in an instruction, on
    // the section's key and on an instance's.
    let own_a = instance_key(INSTANCE_A, LOADING_MODE);
    for (value, problem) in [
        (json!("always"), ConfigValueProblem::OutOfRange),
        (json!(1), ConfigValueProblem::WrongType),
    ] {
        let case = format!("{LOADING_MODE} = {value}");
        let refused = render(json!({"mode": value.clone()}), json!({})).expect_err(&case);
        assert_eq!(refused.config_key(), LOADING_MODE, "{case}");
        assert_eq!(
            refused.failure_code(),
            ConfigCode::ConfigValueInvalid,
            "{case}"
        );
        let refused = defaults
            .apply(&[set(LOADING_MODE, value.clone())], &stamp(), &context())
            .expect_err(&case);
        assert_refused(&refused, LOADING_MODE, problem, &case);
        let refused = defaults
            .apply(&[set(&own_a, value.clone())], &stamp(), &context())
            .expect_err(&case);
        assert_refused(&refused, &own_a, problem, &case);
    }

    // The file: A's own mode for A, the section's for B.
    let own = from_file(json!({"mode": "shadow"}), json!({"mode": "on"}));
    own.check(&context())
        .expect("an instance's own mode passes");
    assert_eq!(own.loading_mode(INSTANCE_A), Ok(LoadingModeConfig::On));
    assert_eq!(own.loading_mode(INSTANCE_B), Ok(LoadingModeConfig::Shadow));
    assert_eq!(own.loading_mode_section(), Ok(LoadingModeConfig::Shadow));

    // An instruction while the Runtime runs: each word is taken, A's own for A only.
    for (word, mode) in [
        ("off", LoadingModeConfig::Off),
        ("shadow", LoadingModeConfig::Shadow),
        ("on", LoadingModeConfig::On),
    ] {
        let changed = defaults
            .apply(&[set(&own_a, json!(word))], &stamp(), &context())
            .expect("an instance's mode is settable")
            .config;
        assert_eq!(changed.loading_mode(INSTANCE_A), Ok(mode), "{word}");
        assert_eq!(
            changed.loading_mode(INSTANCE_B),
            Ok(LoadingModeConfig::Off),
            "{word}: B keeps the section's"
        );
        let changed = defaults
            .apply(&[set(LOADING_MODE, json!(word))], &stamp(), &context())
            .expect("the section's mode is settable")
            .config;
        assert_eq!(changed.loading_mode_section(), Ok(mode), "{word}");
        assert_eq!(changed.loading_mode(INSTANCE_B), Ok(mode), "{word}");
    }
}
