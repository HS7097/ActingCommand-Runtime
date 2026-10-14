// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #398 RD-2 (model-398 v1.1 §4.5, §4.6; `contracts/v012-register.md` §1.52): every
// `device_link` key is parsed, checked and defaulted from the defaults file with the model's
// starting values, and each may change while the Runtime runs (§4.6 "运行中可改"). The section is
// checked whole: both wait lists are non-empty and the restart window holds at least the first
// restart wait. The defaults file excludes `device_link_lost` from the ladder (coordinator
// ruling of 2026-10-13 on model-398 §16 item 5).

use super::*;
use crate::device_link::{
    OFFLINE_ACTION, RESTORE_RETRY_MS, SETTLE_MS, WATCH_RESTART_BACKOFF_MS, WATCH_RESTART_LIMIT,
    WATCH_RESTART_WINDOW_MS,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, RecoveryTriggerStage, RuntimeConfigManifest,
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

// The rows a start renders from a file whose `device_link` section is `section`.
fn render(section: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut parameters = render_parameters(
        &mechanism_rows(),
        &[],
        &shipped(),
        &json!({ "device_link": section }),
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

// §4.6: every key of the section, defaulted from the defaults file (source `default`) with the
// model's starting value and settable; the section in effect reads the same values, passes its
// check, and is what a host started without a manifest reads.
#[test]
fn rd2_keys_every_device_link_key_has_its_model_default() {
    let parameters = render(json!({})).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    let scalars = [
        (WATCH_RESTART_LIMIT, FactScalar::Integer(5)),
        (WATCH_RESTART_WINDOW_MS, FactScalar::DurationMs(600_000)),
        (SETTLE_MS, FactScalar::DurationMs(3_000)),
        (OFFLINE_ACTION, FactScalar::String("wait".to_owned())),
    ];
    for (key, value) in &scalars {
        let parameter = reported(key);
        assert_eq!(&parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    for (key, value) in [
        (WATCH_RESTART_BACKOFF_MS, json!([1000, 5000, 30000])),
        (RESTORE_RETRY_MS, json!([5000, 15000, 60000])),
    ] {
        let parameter = reported(key);
        let FactScalar::String(text) = &parameter.value else {
            panic!("a list is reported as JSON text: {parameter:?}");
        };
        assert_eq!(
            serde_json::from_str::<Value>(text).expect("canonical JSON"),
            value,
            "{key}"
        );
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    assert_eq!(
        parameters
            .iter()
            .filter(|parameter| parameter.key.starts_with("device_link."))
            .count(),
        6
    );

    let config = from_file(json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    let section = config.device_link().expect("the section in effect");
    assert_eq!(section.watch_restart_backoff_ms, vec![1_000, 5_000, 30_000]);
    assert_eq!(section.watch_restart_limit, 5);
    assert_eq!(section.watch_restart_window_ms, 600_000);
    assert_eq!(section.settle_ms, 3_000);
    assert_eq!(section.offline_action, OfflineAction::Wait);
    assert_eq!(section.restore_retry_ms, vec![5_000, 15_000, 60_000]);
    // The last restart wait is used for every later restart.
    assert_eq!(section.restart_backoff_ms(0), 1_000);
    assert_eq!(section.restart_backoff_ms(2), 30_000);
    assert_eq!(section.restart_backoff_ms(9), 30_000);
    assert_eq!(EffectiveConfig::default().device_link(), Ok(section));

    // §4.5: the defaults file excludes `device_link_lost` from the ladder.
    let excluded = shipped()["recovery_ladder"]["exclude_codes"]
        .as_array()
        .expect("exclude_codes is a list")
        .clone();
    assert!(
        excluded.contains(&json!("device_link_lost")),
        "{excluded:?}"
    );
    // So under the defaults a run that ends with it starts no ladder (§12 RD-2-T4).
    let ladder = config
        .recovery_ladder(&context())
        .expect("the default ladder");
    assert_eq!(
        ladder.classify(RecoveryTriggerStage::Task, "device_link_lost"),
        TriggerVerdict::Excluded
    );

    // `kick_once` is the other offline action.
    let kick = from_file(json!({"offline_action": "kick_once", "settle_ms": 0}));
    kick.check(&context())
        .expect("kick_once and no settle wait");
    let section = kick.device_link().expect("the section in effect");
    assert_eq!(
        (section.offline_action, section.settle_ms),
        (OfflineAction::KickOnce, 0)
    );
}

// §4.6 "校验": a value of the wrong type or outside its range is refused as the file layer is
// rendered, a broken section rule by the check, both `config_value_invalid` with the key and why.
#[test]
fn rd2_keys_device_link_values_that_break_a_rule_are_refused() {
    let refusals = [
        (
            "no restart wait",
            json!({"watch": {"restart_backoff_ms": []}}),
            WATCH_RESTART_BACKOFF_MS,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a restart wait that is no number of milliseconds",
            json!({"watch": {"restart_backoff_ms": [1000, -1]}}),
            WATCH_RESTART_BACKOFF_MS,
            ConfigValueProblem::WrongType,
        ),
        (
            "no restart allowed",
            json!({"watch": {"restart_limit": 0}}),
            WATCH_RESTART_LIMIT,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "more than a hundred restarts",
            json!({"watch": {"restart_limit": 101}}),
            WATCH_RESTART_LIMIT,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "a window shorter than the first restart wait",
            json!({"watch": {"restart_window_ms": 999}}),
            WATCH_RESTART_WINDOW_MS,
            ConfigValueProblem::SectionCheck,
        ),
        (
            "a settle wait over a minute",
            json!({"settle_ms": 60_001}),
            SETTLE_MS,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "an offline action of neither word",
            json!({"offline_action": "kill_server"}),
            OFFLINE_ACTION,
            ConfigValueProblem::OutOfRange,
        ),
        (
            "no restore wait",
            json!({"restore_retry_ms": []}),
            RESTORE_RETRY_MS,
            ConfigValueProblem::SectionCheck,
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
