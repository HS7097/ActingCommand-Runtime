// SPDX-License-Identifier: AGPL-3.0-only

// IDLE-2 (Alice's words of 11-11: 「根据任务间隔时间自动选择是关闭模拟器还是关闭应用还是回到主页面」; the
// coordinator's rulings on Workflow #386, comment 6787682270): the value `auto` of `idle.action`
// and its two thresholds. Every expected value is the work order's: `idle.return_home_min_gap_ms`
// 600 000 and `idle.stop_emulator_min_gap_ms` 7 200 000, milliseconds `60000..=86400000`, each
// also per instance and settable; while the action in effect is `auto`, `return_home_min_gap_ms`
// ≤ `min_gap_ms` ≤ `stop_emulator_min_gap_ms` (`section_check` on the key that breaks it), and
// `auto` needs no `start_emulator`; a file that names none of it keeps today's behaviour.
// Neutral fixtures only.

use super::*;
use crate::idle::{
    ACTION, AutoGaps, IdleAction, IdleConfig, MIN_GAP_MS, RETURN_HOME_MIN_GAP_MS,
    STOP_EMULATOR_MIN_GAP_MS,
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

/// The work order's thresholds of `auto`.
const DEFAULT_GAPS: AutoGaps = AutoGaps {
    return_home_min_gap_ms: 600_000,
    stop_emulator_min_gap_ms: 7_200_000,
};

/// The section's keys in effect with `auto` and the defaults otherwise.
const AUTO_DEFAULTS: IdleConfig = IdleConfig {
    action: IdleAction::Auto(DEFAULT_GAPS),
    min_gap_ms: 1_800_000,
    close_app_wake_lead_ms: 300_000,
    stop_emulator_wake_lead_ms: 300_000,
};

// The two keys have the work order's defaults, for the section and every instance, all settable;
// `auto` parses with them (the section's or an instance's own), passes the check on an instance
// whose emulator the Runtime does not start, and an instruction of one instance's own threshold
// reaches that instance alone.
#[test]
fn idle2_auto_c1_the_word_and_the_two_keys_parse_with_their_defaults_and_per_instance_values() {
    let parameters = try_render(json!({}), Value::Null).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    for (key, value) in [
        (RETURN_HOME_MIN_GAP_MS, FactScalar::DurationMs(600_000)),
        (STOP_EMULATOR_MIN_GAP_MS, FactScalar::DurationMs(7_200_000)),
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

    // The section's `auto`: no instance names `start_emulator`, and the check passes.
    let auto = from_file(json!({"action": "auto"}), Value::Null);
    auto.check(&context())
        .expect("auto needs no configured emulator start");
    assert_eq!(auto.idle_section(), Ok(AUTO_DEFAULTS));
    assert_eq!(auto.idle(INSTANCE_A), Ok(AUTO_DEFAULTS));
    assert_eq!(
        auto.idle_section().map(|config| config.action.as_str()),
        Ok("auto")
    );

    // Instance A's own `auto` and thresholds; B keeps the section's action.
    let own = from_file(
        json!({}),
        json!({"idle": {
            "action": "auto",
            "return_home_min_gap_ms": 900_000,
            "stop_emulator_min_gap_ms": 3_600_000
        }}),
    );
    own.check(&context())
        .expect("an instance's own thresholds pass");
    let a = IdleConfig {
        action: IdleAction::Auto(AutoGaps {
            return_home_min_gap_ms: 900_000,
            stop_emulator_min_gap_ms: 3_600_000,
        }),
        ..AUTO_DEFAULTS
    };
    assert_eq!(own.idle(INSTANCE_A), Ok(a));
    assert_eq!(
        own.idle(INSTANCE_B).map(|config| config.action),
        Ok(IdleAction::CloseApp)
    );

    // Through an instruction: A's own threshold is A's alone.
    let changed = own
        .apply(
            &[change(
                &instance_key(INSTANCE_A, RETURN_HOME_MIN_GAP_MS),
                json!(1_200_000),
            )],
            &stamp(),
            &context(),
        )
        .expect("the threshold is settable")
        .config;
    assert_eq!(
        changed.idle(INSTANCE_A),
        Ok(IdleConfig {
            action: IdleAction::Auto(AutoGaps {
                return_home_min_gap_ms: 1_200_000,
                stop_emulator_min_gap_ms: 3_600_000,
            }),
            ..AUTO_DEFAULTS
        })
    );
    assert_eq!(
        changed.idle(INSTANCE_B).map(|config| config.action),
        Ok(IdleAction::CloseApp)
    );
}

// Each threshold outside `60000..=86400000` is refused where it enters; while the action in effect
// is `auto` (the section's or an instance's), a rest threshold above `min_gap_ms` or a stop
// threshold below it is `section_check` on that key, at start and through an instruction, and
// equal thresholds pass; another action's thresholds are not checked together.
#[test]
fn idle2_auto_c2_each_range_and_each_whole_section_refusal() {
    let invalid = ConfigCode::ConfigValueInvalid;
    for (key, member, value) in [
        (
            RETURN_HOME_MIN_GAP_MS,
            "return_home_min_gap_ms",
            json!(59_999),
        ),
        (
            RETURN_HOME_MIN_GAP_MS,
            "return_home_min_gap_ms",
            json!(86_400_001),
        ),
        (
            STOP_EMULATOR_MIN_GAP_MS,
            "stop_emulator_min_gap_ms",
            json!(59_999),
        ),
        (
            STOP_EMULATOR_MIN_GAP_MS,
            "stop_emulator_min_gap_ms",
            json!(86_400_001),
        ),
    ] {
        let mut idle = serde_json::Map::new();
        idle.insert(member.to_owned(), value.clone());
        let refused =
            try_render(Value::Object(idle), Value::Null).expect_err("a threshold out of range");
        assert_eq!(
            refusal(&refused),
            (invalid, key, Some(ConfigValueProblem::OutOfRange)),
            "{member} {value}"
        );
    }

    // The section's `auto`: a rest threshold above the gap, a stop threshold below it.
    let section_check = Some(ConfigValueProblem::SectionCheck);
    let above = from_file(
        json!({"action": "auto", "return_home_min_gap_ms": 1_800_001}),
        Value::Null,
    );
    let refused = above.check(&context()).expect_err("rest above the gap");
    assert_eq!(
        refusal(&refused),
        (invalid, RETURN_HOME_MIN_GAP_MS, section_check)
    );
    let below = from_file(
        json!({"action": "auto", "stop_emulator_min_gap_ms": 1_799_999}),
        Value::Null,
    );
    let refused = below.check(&context()).expect_err("stop below the gap");
    assert_eq!(
        refusal(&refused),
        (invalid, STOP_EMULATOR_MIN_GAP_MS, section_check)
    );
    // Equal thresholds pass.
    from_file(
        json!({
            "action": "auto",
            "return_home_min_gap_ms": 1_800_000,
            "stop_emulator_min_gap_ms": 1_800_000
        }),
        Value::Null,
    )
    .check(&context())
    .expect("equal thresholds keep the tiers' order");
    // Another action's thresholds are not checked together.
    from_file(
        json!({"action": "close_app", "return_home_min_gap_ms": 1_800_001}),
        Value::Null,
    )
    .check(&context())
    .expect("close_app reads no threshold of auto");

    // An instance's own `auto` with its stop threshold below its gap.
    let own_stop_key = instance_key(INSTANCE_A, STOP_EMULATOR_MIN_GAP_MS);
    let own = from_file(
        json!({}),
        json!({"idle": {"action": "auto", "stop_emulator_min_gap_ms": 600_000}}),
    );
    let refused = own
        .check(&context())
        .expect_err("the instance's stop threshold is below its gap");
    assert_eq!(
        refusal(&refused),
        (invalid, own_stop_key.as_str(), section_check)
    );

    // Through an instruction: under the section's `auto`, a gap above the stop threshold is
    // refused whole.
    let auto = from_file(json!({"action": "auto"}), Value::Null);
    let refused = auto
        .apply(
            &[change(MIN_GAP_MS, json!(7_200_001))],
            &stamp(),
            &context(),
        )
        .expect_err("the gap rises above the stop threshold");
    assert_eq!(refused.failure_code(), invalid);
    assert_eq!(refused.problem(), section_check);
    // And switching to `auto` is checked as well: the instance's own rest threshold above the gap.
    let ahead = from_file(
        json!({}),
        json!({"idle": {"return_home_min_gap_ms": 3_600_000}}),
    );
    ahead
        .check(&context())
        .expect("close_app reads no threshold of auto");
    let refused = ahead
        .apply(&[change(ACTION, json!("auto"))], &stamp(), &context())
        .expect_err("auto with the instance's rest threshold above its gap");
    assert_eq!(refused.failure_code(), invalid);
    assert_eq!(
        refused.config_key(),
        instance_key(INSTANCE_A, RETURN_HOME_MIN_GAP_MS).as_str()
    );
    assert_eq!(refused.problem(), section_check);
}

// A file that names neither `auto` nor the two keys renders their defaults and reads as today: the
// action and every IDLE-1 key in effect are the file's.
#[test]
fn idle2_auto_c3_a_file_without_them_keeps_today_s_behaviour() {
    let today = from_file(
        json!({"action": "stop_emulator", "min_gap_ms": 3_600_000}),
        json!({"start_emulator": true}),
    );
    today.check(&context()).expect_err(
        "instance B has no configured emulator start, as before IDLE-2 (stop_emulator needs it)",
    );
    let today = from_file(
        json!({"action": "close_app", "min_gap_ms": 3_600_000}),
        Value::Null,
    );
    today.check(&context()).expect("today's file passes");
    let expected = IdleConfig {
        action: IdleAction::CloseApp,
        min_gap_ms: 3_600_000,
        close_app_wake_lead_ms: 300_000,
        stop_emulator_wake_lead_ms: 300_000,
    };
    assert_eq!(today.idle_section(), Ok(expected));
    assert_eq!(today.idle(INSTANCE_A), Ok(expected));
    assert_eq!(today.idle(INSTANCE_B), Ok(expected));
}
