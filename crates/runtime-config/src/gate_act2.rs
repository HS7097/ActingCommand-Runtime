// SPDX-License-Identifier: AGPL-3.0-only

// ACT-1, ACT-2 (Workflow #407, register §1.250; design `rd-1031/check-speed/model-check.md` §1.7;
// coordinator brief `brief-slices.md`, decision 1): the `execution.action_gap` keys. Their defaults
// come from the defaults file and leave the behaviour off: `mode` `declared`, `kind` `fixed`,
// `fixed_ms` 0, `random_min_ms` and `random_max_ms` 0, `poll_ms` 50, `settle_frames` 1,
// `settle_change_max_milli` 500, `settled_max_stage` `s1`, `fallback_hold_ms` 3 600 000. Ranges:
// each gap 0..=60 000, `poll_ms` 0..=1 000, `settle_frames` 1..=8, `fallback_hold_ms`
// 0..=86 400 000; `random_min_ms` at most `random_max_ms`, each as it is in effect. Every key is
// settable and may be overridden per instance, in the file and by an instruction.

use super::*;
use crate::action_gap::{
    ActionGapConfig, ActionGapKind, ActionGapMode, FALLBACK_HOLD_MS, FIXED_MS, KIND, MODE, POLL_MS,
    RANDOM_MAX_MS, RANDOM_MIN_MS, SETTLE_FRAMES,
};
use actingcommand_contract::{
    ConfigParameter, ConfigSettingChange, PerformancePacingStage, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const INSTANCE_B: &str = "instance_fedcba9876543210fedcba9876543210";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};

const SHIPPED: ActionGapConfig = ActionGapConfig {
    mode: ActionGapMode::Declared,
    kind: ActionGapKind::Fixed,
    fixed_ms: 0,
    random_min_ms: 0,
    random_max_ms: 0,
    poll_ms: 50,
    settle_frames: 1,
    settle_change_max_milli: 500,
    settled_max_stage: PerformancePacingStage::S1,
    fallback_hold_ms: 3_600_000,
    // ACT-4 (register §1.254): the key its slice added, at its shipped default.
    settled_limit_ms: 2_000,
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

// The rows a start renders from a file of the section's `action_gap` member `section` and instance
// A's own `own_a`; instance B names none.
fn render(section: Value, own_a: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let a = json!({"instance_id": INSTANCE_A, "alias": "node.a",
                   "execution": {"action_gap": own_a}});
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &DefaultsFile::shipped()
            .expect("the shipped defaults file parses")
            .1,
        &json!({"execution": {"action_gap": section}, "instances": [a, b]}),
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
fn gap_keys_range_and_per_instance_override() {
    // The defaults: each from the defaults file, and no row of an instance's own.
    let defaults = from_file(json!({}), json!({}));
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.action_gap_section(), Ok(SHIPPED));
    assert_eq!(defaults.action_gap(INSTANCE_A), Ok(SHIPPED));
    assert_eq!(EffectiveConfig::default().action_gap_section(), Ok(SHIPPED));

    // Each bound in the file, as a start and `check-config` render it, and in an instruction.
    for (member, key, value) in [
        ("fixed_ms", FIXED_MS, json!(60_001)),
        ("random_min_ms", RANDOM_MIN_MS, json!(60_001)),
        ("random_max_ms", RANDOM_MAX_MS, json!(60_001)),
        ("poll_ms", POLL_MS, json!(1_001)),
        ("settle_frames", SETTLE_FRAMES, json!(0)),
        ("settle_frames", SETTLE_FRAMES, json!(9)),
        ("fallback_hold_ms", FALLBACK_HOLD_MS, json!(86_400_001)),
        ("mode", MODE, json!("fast")),
        ("kind", KIND, json!("always")),
    ] {
        let case = format!("{key} = {value}");
        let refused = render(json!({ member: value.clone() }), json!({})).expect_err(&case);
        assert_refused(&refused, key, ConfigValueProblem::OutOfRange, &case);
        let refused = defaults
            .apply(&[set(key, value)], &stamp(), &context())
            .expect_err(&case);
        assert_refused(&refused, key, ConfigValueProblem::OutOfRange, &case);
    }

    // The least random gap above the largest is refused, on the section and across an instance's
    // own key and the section's.
    let refused = defaults
        .apply(&[set(RANDOM_MIN_MS, json!(10))], &stamp(), &context())
        .expect_err("a least gap above the largest");
    assert_refused(
        &refused,
        RANDOM_MIN_MS,
        ConfigValueProblem::SectionCheck,
        "the section",
    );
    let own_min = instance_key(INSTANCE_A, RANDOM_MIN_MS);
    let refused = defaults
        .apply(&[set(&own_min, json!(10))], &stamp(), &context())
        .expect_err("an instance's least gap above the section's largest");
    assert_refused(
        &refused,
        &own_min,
        ConfigValueProblem::SectionCheck,
        "the instance",
    );

    // The file: A's own keys for A, the section's for B.
    let own = from_file(
        json!({"kind": "performance"}),
        json!({"mode": "settled", "fixed_ms": 40}),
    );
    own.check(&context()).expect("an instance's own keys pass");
    assert_eq!(
        own.action_gap(INSTANCE_A),
        Ok(ActionGapConfig {
            mode: ActionGapMode::Settled,
            kind: ActionGapKind::Performance,
            fixed_ms: 40,
            ..SHIPPED
        })
    );
    assert_eq!(
        own.action_gap(INSTANCE_B),
        Ok(ActionGapConfig {
            kind: ActionGapKind::Performance,
            ..SHIPPED
        })
    );

    // An instruction: A's own key for A only, and every key at the edge of its range is taken.
    let changed = defaults
        .apply(
            &[
                set(&instance_key(INSTANCE_A, MODE), json!("settled")),
                set(FIXED_MS, json!(60_000)),
                set(RANDOM_MAX_MS, json!(60_000)),
                set(RANDOM_MIN_MS, json!(60_000)),
                set(POLL_MS, json!(1_000)),
                set(SETTLE_FRAMES, json!(8)),
                set(FALLBACK_HOLD_MS, json!(86_400_000)),
            ],
            &stamp(),
            &context(),
        )
        .expect("each key is settable")
        .config;
    let section = ActionGapConfig {
        fixed_ms: 60_000,
        random_min_ms: 60_000,
        random_max_ms: 60_000,
        poll_ms: 1_000,
        settle_frames: 8,
        fallback_hold_ms: 86_400_000,
        ..SHIPPED
    };
    assert_eq!(
        changed.action_gap(INSTANCE_A),
        Ok(ActionGapConfig {
            mode: ActionGapMode::Settled,
            ..section
        })
    );
    assert_eq!(changed.action_gap(INSTANCE_B), Ok(section));
}
