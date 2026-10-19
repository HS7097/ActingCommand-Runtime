// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #406 T12 (host gate key 123; plan Q13; `contracts/v012-register.md` §1.136; brief T12
// requirement 1): the five `touch` keys default from the defaults file to the plan's values
// (sigma 250 ‰, 8 redraws, 4 candidates, 16 points per box, 1024 boxes), each is settable, and a
// value outside its range is `config_value_invalid` (`out_of_range`) for its key when the file
// layer is rendered (a start, `check-config`), in the whole-section check and in an instruction.

use super::*;
use crate::touch::{CANDIDATE_COUNT, MAX_BOXES, MAX_REDRAWS, POINTS_PER_BOX, SIGMA_RATIO_MILLI};
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

// The rows a start renders from a file whose `touch` section is `section`.
fn render(section: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let (_, defaults) = DefaultsFile::shipped().expect("the shipped defaults file parses");
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &defaults,
        &json!({ "touch": section }),
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

fn set(key: &str, value: i64) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(json!(value)),
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
fn t12_the_touch_section_parses_checks_and_defaults() {
    // The defaults: from the defaults file (source `default`), settable, and the section in
    // effect reads them, as a host started without a manifest does.
    let parameters = render(json!({})).expect("the defaults resolve");
    let defaults = [
        (SIGMA_RATIO_MILLI, 250),
        (MAX_REDRAWS, 8),
        (CANDIDATE_COUNT, 4),
        (POINTS_PER_BOX, 16),
        (MAX_BOXES, 1_024),
    ];
    for (key, value) in defaults {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"));
        assert_eq!(parameter.value, FactScalar::Integer(value), "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("touch."))
            .count(),
        defaults.len()
    );
    let expected = TouchConfig {
        sigma_ratio_milli: 250,
        max_redraws: 8,
        candidate_count: 4,
        points_per_box: 16,
        max_boxes: 1_024,
    };
    let config = from_file(json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    assert_eq!(config.touch(), Ok(expected));
    assert_eq!(EffectiveConfig::default().touch(), Ok(expected));

    // Each bound, in the file (rendered at a start and by `check-config`), in a manifest's value
    // (the whole-section check) and in an instruction.
    let bounds = [
        ("sampling", "sigma_ratio_milli", SIGMA_RATIO_MILLI, 0, 1_001),
        ("sampling", "max_redraws", MAX_REDRAWS, -1, 65),
        ("sampling", "candidate_count", CANDIDATE_COUNT, 0, 17),
        ("history", "points_per_box", POINTS_PER_BOX, 0, 257),
        ("history", "max_boxes", MAX_BOXES, 0, 65_537),
    ];
    for (member, name, key, below, above) in bounds {
        for value in [below, above] {
            let case = format!("{key} = {value}");
            let refused = render(json!({ member: { name: value } })).expect_err(&case);
            assert_out_of_range(&refused, key, &case);

            let refused = EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters: vec![ConfigParameter {
                    key: key.to_owned(),
                    value: FactScalar::Integer(value),
                    source: ConfigParameterSource::Explicit,
                    settable: true,
                }],
            })
            .expect("the key is in the key table")
            .check(&context())
            .expect_err(&case);
            assert_out_of_range(&refused, key, &case);

            let refused = config
                .apply(&[set(key, value)], &stamp(), &context())
                .expect_err(&case);
            assert_out_of_range(&refused, key, &case);
        }
    }

    // Every key is settable: one instruction changes all five within their bounds, and the
    // section in effect reads the instruction's values.
    let applied = config
        .apply(
            &[
                set(SIGMA_RATIO_MILLI, 1_000),
                set(MAX_REDRAWS, 0),
                set(CANDIDATE_COUNT, 1),
                set(POINTS_PER_BOX, 256),
                set(MAX_BOXES, 65_536),
            ],
            &stamp(),
            &context(),
        )
        .expect("values within their bounds are set");
    assert_eq!(
        applied.config.touch(),
        Ok(TouchConfig {
            sigma_ratio_milli: 1_000,
            max_redraws: 0,
            candidate_count: 1,
            points_per_box: 256,
            max_boxes: 65_536,
        })
    );
    assert!(applied.sections.contains("touch"));
}
