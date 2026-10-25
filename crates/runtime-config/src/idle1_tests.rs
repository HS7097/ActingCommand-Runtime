// SPDX-License-Identifier: AGPL-3.0-only

// IDLE-1 (Alice's ruling 41 of 10-24 and her ruling of 10-25: 「这是两个功能，也就是说一个功能下的两种选项，
// 我们可以选择是关闭模拟器还是只关应用程序」): the `idle` section. Every expected value is the work order's:
// `idle.action` `close_app` (one of `none`, `close_app`, `stop_emulator`), `idle.min_gap_ms`
// 1 800 000, the wake leads 60 000 (`close_app`) and 300 000 (`stop_emulator`); each also per
// instance, defaulting to the section's; every key settable; each lead shorter than the gap; and
// `stop_emulator` only on an instance whose emulator the Runtime starts. Neutral fixtures only.

use super::*;
use crate::idle::{
    ACTION, CLOSE_APP_WAKE_LEAD_MS, IdleAction, IdleConfig, MIN_GAP_MS, STOP_EMULATOR_WAKE_LEAD_MS,
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

// The rows a start renders from a file of `idle` and, for instance A, the members `own_a` adds to
// its entry (B names none).
fn try_render(idle: Value, own_a: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut a = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if let Value::Object(members) = own_a {
        for (name, member) in members {
            a[name] = member;
        }
    }
    let b = json!({"instance_id": INSTANCE_B, "alias": "node.b"});
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned(), INSTANCE_B.to_owned()],
        &DefaultsFile::shipped()
            .expect("the shipped defaults file parses")
            .1,
        &json!({"idle": idle, "instances": [a, b]}),
        &|_| None,
    )
}

fn from_file(idle: Value, own_a: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: try_render(idle, own_a).expect("every key resolves"),
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

fn change(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn refusal(failure: &ConfigFailure) -> (ConfigCode, &str, Option<ConfigValueProblem>) {
    (
        failure.failure_code(),
        failure.config_key(),
        failure.problem(),
    )
}

const SECTION_DEFAULTS: IdleConfig = IdleConfig {
    action: IdleAction::CloseApp,
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: 60_000,
    stop_emulator_wake_lead_ms: 300_000,
};

// The section's keys have the work order's defaults and every instance's keys default to them,
// all settable; an instance's own key overrides the section's for that instance only, from the
// file and through an instruction, and a change of the section's key reaches every instance that
// names none.
#[test]
fn idle1_c1_defaults_and_per_instance_keys() {
    let parameters = try_render(json!({}), Value::Null).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    for (key, value) in [
        (ACTION, FactScalar::String("close_app".to_owned())),
        (MIN_GAP_MS, FactScalar::DurationMs(1_800_000)),
        (CLOSE_APP_WAKE_LEAD_MS, FactScalar::DurationMs(60_000)),
        (STOP_EMULATOR_WAKE_LEAD_MS, FactScalar::DurationMs(300_000)),
    ] {
        let parameter = reported(key);
        assert_eq!(parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
        for instance in [INSTANCE_A, INSTANCE_B] {
            let own = reported(&instance_key(instance, key));
            assert_eq!(own.value, value, "{instance} {key}");
            assert_eq!(
                own.source,
                ConfigParameterSource::Default,
                "{instance} {key}"
            );
            assert!(own.settable, "{instance} {key}");
        }
    }
    let defaults = from_file(json!({}), Value::Null);
    defaults.check(&context()).expect("the defaults pass");
    assert_eq!(defaults.idle_section(), Ok(SECTION_DEFAULTS));
    assert_eq!(defaults.idle(INSTANCE_A), Ok(SECTION_DEFAULTS));

    // Instance A names its own action and gap; B names none.
    let own = from_file(
        json!({}),
        json!({"idle": {"action": "none", "min_gap_ms": 3_600_000}}),
    );
    own.check(&context()).expect("an instance's own keys pass");
    let a = IdleConfig {
        action: IdleAction::None,
        min_gap_ms: 3_600_000,
        ..SECTION_DEFAULTS
    };
    assert_eq!(own.idle(INSTANCE_A), Ok(a));
    assert_eq!(own.idle(INSTANCE_B), Ok(SECTION_DEFAULTS));

    // Through instructions: the section's lead reaches B only; A's own lead is A's alone.
    let changed = own
        .apply(
            &[
                change(CLOSE_APP_WAKE_LEAD_MS, json!(120_000)),
                change(
                    &instance_key(INSTANCE_A, STOP_EMULATOR_WAKE_LEAD_MS),
                    json!(600_000),
                ),
            ],
            &stamp(),
            &context(),
        )
        .expect("both keys are settable")
        .config;
    assert_eq!(
        changed.idle(INSTANCE_A),
        Ok(IdleConfig {
            close_app_wake_lead_ms: 120_000,
            stop_emulator_wake_lead_ms: 600_000,
            ..a
        })
    );
    assert_eq!(
        changed.idle(INSTANCE_B),
        Ok(IdleConfig {
            close_app_wake_lead_ms: 120_000,
            ..SECTION_DEFAULTS
        })
    );
}

// A value of the wrong kind or out of range is refused where it enters; a wake lead not shorter
// than the gap (the section's or an instance's) is refused as `section_check`, at start and
// through an instruction; `stop_emulator` is refused on an instance whose emulator the Runtime
// does not start and passes on one it does.
#[test]
fn idle1_c2_bad_values_are_refused() {
    let invalid = ConfigCode::ConfigValueInvalid;
    // An unknown action and a gap below its range never render.
    let unknown = try_render(json!({"action": "sleep"}), Value::Null).expect_err("unknown action");
    assert_eq!(
        refusal(&unknown),
        (invalid, ACTION, Some(ConfigValueProblem::OutOfRange))
    );
    let short = try_render(json!({"min_gap_ms": 1_000}), Value::Null).expect_err("gap too short");
    assert_eq!(
        refusal(&short),
        (invalid, MIN_GAP_MS, Some(ConfigValueProblem::OutOfRange))
    );
    let wrong = try_render(json!({"action": true}), Value::Null).expect_err("no token");
    assert_eq!(
        refusal(&wrong),
        (invalid, ACTION, Some(ConfigValueProblem::WrongType))
    );

    // A lead as long as the gap: the section's at start, an instance's in its own check.
    let section = from_file(
        json!({"min_gap_ms": 300_000, "stop_emulator_wake_lead_ms": 300_000}),
        Value::Null,
    );
    let refused = section.check(&context()).expect_err("lead equals the gap");
    assert_eq!(
        refusal(&refused),
        (
            invalid,
            STOP_EMULATOR_WAKE_LEAD_MS,
            Some(ConfigValueProblem::SectionCheck)
        )
    );
    let own_lead_key = instance_key(INSTANCE_A, CLOSE_APP_WAKE_LEAD_MS);
    let own = from_file(
        json!({}),
        json!({"idle": {"min_gap_ms": 120_000, "close_app_wake_lead_ms": 120_000}}),
    );
    let refused = own
        .check(&context())
        .expect_err("the instance's lead equals its gap");
    assert_eq!(
        refusal(&refused),
        (
            invalid,
            own_lead_key.as_str(),
            Some(ConfigValueProblem::SectionCheck)
        )
    );

    // Through an instruction: a section gap below the default stop lead is refused whole.
    let defaults = from_file(json!({}), Value::Null);
    let refused = defaults
        .apply(&[change(MIN_GAP_MS, json!(240_000))], &stamp(), &context())
        .expect_err("the gap falls below the stop lead");
    assert_eq!(refused.failure_code(), invalid);
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));

    // `stop_emulator` needs the Runtime's start of the instance's emulator.
    let own_action_key = instance_key(INSTANCE_A, ACTION);
    let unstarted = from_file(json!({}), json!({"idle": {"action": "stop_emulator"}}));
    let refused = unstarted
        .check(&context())
        .expect_err("no configured emulator start");
    assert_eq!(
        refusal(&refused),
        (
            invalid,
            own_action_key.as_str(),
            Some(ConfigValueProblem::SectionCheck)
        )
    );
    let started = from_file(
        json!({}),
        json!({"idle": {"action": "stop_emulator"}, "start_emulator": true}),
    );
    started
        .check(&context())
        .expect("the configured start brings the emulator back");
    assert_eq!(
        started.idle(INSTANCE_A).map(|config| config.action),
        Ok(IdleAction::StopEmulator)
    );
    // The same through an instruction of the section's action: B has no configured start.
    let refused = started
        .apply(
            &[change(ACTION, json!("stop_emulator"))],
            &stamp(),
            &context(),
        )
        .expect_err("instance B has no configured emulator start");
    assert_eq!(refused.failure_code(), invalid);
    assert_eq!(
        refused.config_key(),
        instance_key(INSTANCE_B, ACTION).as_str()
    );
}
