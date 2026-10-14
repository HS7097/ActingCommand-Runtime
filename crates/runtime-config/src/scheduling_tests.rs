// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #386 SR-1 (frozen scheduling model v1.4.2 §9; `contracts/v012-register.md` §5): the
// `scheduling` keys are parsed, checked and defaulted in v0.12.0, and Workflow #405's random
// task delay (the coordinator's order of 2026-10-14) per instance as well. The defaults are the
// model's §9 table and the order's values; the rules are the model's (§5.1 b host blackout
// entries, §4.1 and §7.2 code lists) and the order's (`0 <= min_ms <= max_ms <= 86400000`,
// checked on the pair in effect).

use super::*;
use crate::recovery_ladder::{Listed, listed};
use crate::scheduling::{
    BLACKOUT_HARD_QUIET_MS, BLACKOUT_HOST, BLACKOUT_QUIET_MARGIN_MS,
    BLACKOUT_STOP_IN_FLIGHT_AT_START, COST_COMMIT_MARGIN_MS, COST_COMMIT_MAX_STAGE,
    COST_COMMIT_PROMOTE, FEATURE_LOCK_CODES, FEATURE_LOCK_LOCKED_VALID_FOR_MS,
    NOT_RUN_DIAGNOSE_ROWS, RANDOM_DELAY_ENABLED, RANDOM_DELAY_MAX_MS, RANDOM_DELAY_MIN_MS,
    RERUN_CONDITION_CODES, RERUN_EXPIRES, RERUN_MAX_PER_OCCURRENCE, RERUN_PROMOTE,
    RERUN_REFUND_COUNTS, RERUN_RESUME_SETTLE_MS, RERUN_RESUME_STAGE, SAMPLE_TIMEOUT_RETRIES,
};
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, ConfigSettingChange, FactScalar, RuntimeConfigManifest,
};
use serde_json::{Value, json};

const INSTANCE_A: &str = "instance_0123456789abcdef0123456789abcdef";
const DEVICE: DeviceBounds = DeviceBounds {
    application_stop_ms: 60_000,
    emulator_stop_ms: 200_000,
    emulator_start_ms: 190_000,
};
// SCHED §9 `scheduling.rerun.condition_codes`: the eight codes that end a run by a condition.
const CONDITION_CODES: [&str; 8] = [
    "recognition_sample_budget_insufficient",
    "candidate_sample_budget_insufficient",
    "recognition_sample_deadline_exceeded",
    "candidate_sample_deadline_exceeded",
    "blackout_run_stopped",
    "perf_run_yielded",
    "perf_hold_expired",
    "cost_commit_time_insufficient",
];

fn context() -> CheckContext {
    CheckContext::new(5_000, DEVICE)
}

fn shipped() -> Value {
    DefaultsFile::shipped()
        .expect("the shipped defaults file parses")
        .1
}

// The rows a start renders from a file of `scheduling` and, for instance A, `instance`.
fn render(scheduling: Value, instance: Value) -> Result<Vec<ConfigParameter>, ConfigFailure> {
    let mut entry = json!({"instance_id": INSTANCE_A, "alias": "node.a"});
    if let Value::Object(members) = instance {
        entry
            .as_object_mut()
            .expect("an instance entry")
            .extend(members);
    }
    render_parameters(
        &mechanism_rows(),
        &[INSTANCE_A.to_owned()],
        &shipped(),
        &json!({"scheduling": scheduling, "instances": [entry]}),
        &|_| None,
    )
}

fn from_file(scheduling: Value, instance: Value) -> EffectiveConfig {
    EffectiveConfig::from_manifest(&RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters: render(scheduling, instance).expect("every key resolves"),
    })
    .expect("every key is in the key table")
}

fn stamp() -> InstructionStamp {
    InstructionStamp {
        set_unix_ms: 1,
        set_in_owner_epoch: "epoch".to_owned(),
        request_id: "request".to_owned(),
    }
}

fn set(key: &str, value: Value) -> ConfigSettingChange {
    ConfigSettingChange {
        key: key.to_owned(),
        value: Some(value),
    }
}

fn list(value: &FactScalar) -> Value {
    match value {
        FactScalar::String(text) => serde_json::from_str(text).expect("a list's canonical JSON"),
        other => panic!("a list is reported as JSON text: {other:?}"),
    }
}

// SCHED §9 and the #405 order: every key of the section, defaulted from the defaults file
// (source `default`), settable while the Runtime runs; an instance's random delay defaults to
// the section's. The rerun codes are the model's eight as far as the catalog lists them
// (`perf_run_yielded` and `perf_hold_expired` arrive with performance slices P-s and P-h).
#[test]
fn sr1_c1_every_scheduling_key_has_its_model_default() {
    let parameters = render(json!({}), json!({})).expect("the defaults resolve");
    let reported = |key: &str| {
        parameters
            .iter()
            .find(|parameter| parameter.key == key)
            .unwrap_or_else(|| panic!("no parameter {key}"))
    };
    let expected = [
        (SAMPLE_TIMEOUT_RETRIES, FactScalar::Integer(2)),
        (RERUN_MAX_PER_OCCURRENCE, FactScalar::Integer(2)),
        (RERUN_RESUME_STAGE, FactScalar::String("s2".to_owned())),
        (RERUN_RESUME_SETTLE_MS, FactScalar::DurationMs(60_000)),
        (RERUN_PROMOTE, FactScalar::Boolean(true)),
        (RERUN_REFUND_COUNTS, FactScalar::Boolean(true)),
        (RERUN_EXPIRES, FactScalar::String("window_end".to_owned())),
        (COST_COMMIT_PROMOTE, FactScalar::Boolean(true)),
        (COST_COMMIT_MAX_STAGE, FactScalar::String("s2".to_owned())),
        (COST_COMMIT_MARGIN_MS, FactScalar::DurationMs(60_000)),
        (BLACKOUT_HOST, FactScalar::String("[]".to_owned())),
        (BLACKOUT_QUIET_MARGIN_MS, FactScalar::DurationMs(60_000)),
        (BLACKOUT_HARD_QUIET_MS, FactScalar::DurationMs(0)),
        (BLACKOUT_STOP_IN_FLIGHT_AT_START, FactScalar::Boolean(true)),
        (
            FEATURE_LOCK_LOCKED_VALID_FOR_MS,
            FactScalar::DurationMs(86_400_000),
        ),
        (NOT_RUN_DIAGNOSE_ROWS, FactScalar::Integer(10)),
        (RANDOM_DELAY_ENABLED, FactScalar::Boolean(false)),
        (RANDOM_DELAY_MIN_MS, FactScalar::DurationMs(60_000)),
        (RANDOM_DELAY_MAX_MS, FactScalar::DurationMs(3_600_000)),
    ];
    for (key, value) in &expected {
        let parameter = reported(key);
        assert_eq!(&parameter.value, value, "{key}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{key}");
        assert!(parameter.settable, "{key}");
    }
    let listed_codes = CONDITION_CODES
        .iter()
        .filter(|code| matches!(listed(code), Listed::Active))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        list(&reported(RERUN_CONDITION_CODES).value),
        json!(listed_codes)
    );
    assert_eq!(
        list(&reported(FEATURE_LOCK_CODES).value),
        json!(["contained_task_feature_locked"])
    );
    for member in [
        RANDOM_DELAY_ENABLED,
        RANDOM_DELAY_MIN_MS,
        RANDOM_DELAY_MAX_MS,
    ] {
        let parameter = reported(&instance_key(INSTANCE_A, member));
        assert_eq!(parameter.value, reported(member).value, "{member}");
        assert_eq!(parameter.source, ConfigParameterSource::Default, "{member}");
        assert!(parameter.settable, "{member}");
    }

    let config = from_file(json!({}), json!({}));
    config
        .check(&context())
        .expect("the defaults pass the check");
    assert_eq!(config.sample_timeout_retries(), Ok(2));
    let section = config.scheduling().expect("the section in effect");
    assert_eq!(section.sample_timeout_retries, 2);
    assert!(section.blackout.host.is_empty());
    assert_eq!(
        from_file(
            json!({"recognition": {"sample_timeout_retries": 0}}),
            json!({})
        )
        .sample_timeout_retries(),
        Ok(0)
    );
}

// One refused file: the `scheduling` members, the code, the key, why, and for a host blackout
// entry the member pointer and the entry's id.
struct Refusal {
    case: &'static str,
    scheduling: Value,
    code: ConfigCode,
    config_key: &'static str,
    problem: Option<ConfigValueProblem>,
    blackout: Option<(&'static str, Option<&'static str>)>,
}

fn night(weekdays: Value) -> Value {
    json!({"id": "night", "weekdays": weekdays, "utc_offset_minutes": 540,
           "start_minute_of_day": 1380, "end_minute_of_day": 360})
}

fn refusals() -> Vec<Refusal> {
    let value = |case, scheduling, config_key, problem| Refusal {
        case,
        scheduling,
        code: ConfigCode::ConfigValueInvalid,
        config_key,
        problem: Some(problem),
        blackout: None,
    };
    let entry = |case, host, field, id| Refusal {
        case,
        scheduling: json!({"blackout": {"host": host}}),
        code: ConfigCode::BlackoutConfigInvalid,
        config_key: BLACKOUT_HOST,
        problem: None,
        blackout: Some((field, id)),
    };
    vec![
        value(
            "a negative retry count",
            json!({"recognition": {"sample_timeout_retries": -1}}),
            SAMPLE_TIMEOUT_RETRIES,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a retry count as text",
            json!({"recognition": {"sample_timeout_retries": "2"}}),
            SAMPLE_TIMEOUT_RETRIES,
            ConfigValueProblem::WrongType,
        ),
        value(
            "a stage outside the vocabulary",
            json!({"rerun": {"resume_stage": "s5"}}),
            RERUN_RESUME_STAGE,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "an expiry other than the window end",
            json!({"rerun": {"expires": "never"}}),
            RERUN_EXPIRES,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a condition code the catalog does not list",
            json!({"rerun": {"condition_codes": ["neutral_unlisted_failure"]}}),
            RERUN_CONDITION_CODES,
            ConfigValueProblem::CodeUnknown,
        ),
        value(
            "a retired condition code",
            json!({"rerun": {"condition_codes": ["recognition_sample_deadline_or_permission_lost"]}}),
            RERUN_CONDITION_CODES,
            ConfigValueProblem::CodeRetired,
        ),
        value(
            "a feature lock code that is no text",
            json!({"feature_lock": {"codes": [1]}}),
            FEATURE_LOCK_CODES,
            ConfigValueProblem::WrongType,
        ),
        value(
            "a random delay longer than a day",
            json!({"random_delay": {"max_ms": 86_400_001}}),
            RANDOM_DELAY_MAX_MS,
            ConfigValueProblem::OutOfRange,
        ),
        value(
            "a shortest random delay above the longest",
            json!({"random_delay": {"min_ms": 7_200_000}}),
            RANDOM_DELAY_MIN_MS,
            ConfigValueProblem::SectionCheck,
        ),
        entry(
            "a weekday outside 1 to 7",
            json!([night(json!([0]))]),
            "/scheduling/blackout/host/0/weekdays",
            Some("night"),
        ),
        entry(
            "a weekday twice",
            json!([night(json!([1, 1]))]),
            "/scheduling/blackout/host/0/weekdays",
            Some("night"),
        ),
        entry(
            "two entries with one id",
            json!([night(json!([1])), night(json!([4]))]),
            "/scheduling/blackout/host/1/id",
            Some("night"),
        ),
        entry(
            "a start minute past the day",
            json!([{"id": "late", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 1440, "end_minute_of_day": 0}]),
            "/scheduling/blackout/host/0/start_minute_of_day",
            Some("late"),
        ),
        entry(
            "an offset beyond fourteen hours",
            json!([{"id": "far", "weekdays": [1], "utc_offset_minutes": 900,
                    "start_minute_of_day": 0, "end_minute_of_day": 60}]),
            "/scheduling/blackout/host/0/utc_offset_minutes",
            Some("far"),
        ),
        entry(
            "an entry without its end",
            json!([{"id": "open", "weekdays": [1], "utc_offset_minutes": 0,
                    "start_minute_of_day": 0}]),
            "/scheduling/blackout/host/0",
            Some("open"),
        ),
        entry(
            "an entry without an id",
            json!([{"weekdays": [1], "utc_offset_minutes": 0, "start_minute_of_day": 0,
                    "end_minute_of_day": 60}]),
            "/scheduling/blackout/host/0",
            None,
        ),
    ]
}

// SCHED §5.1 b, §4.1, §7.2 and the #405 order: a value of the wrong type or outside its range is
// refused as the file layer is rendered, a broken section rule by the check that a start and
// check-config make; a host blackout entry's rule names the member and the entry. Through an
// instruction the same values are `config_value_invalid` and the configuration in effect stays.
#[test]
fn sr1_c2_scheduling_values_that_break_a_rule_are_refused() {
    for row in refusals() {
        let refused = match render(row.scheduling.clone(), json!({})) {
            Err(refused) => refused,
            Ok(parameters) => EffectiveConfig::from_manifest(&RuntimeConfigManifest {
                subsystems: Vec::new(),
                parameters,
            })
            .expect("every key is in the key table")
            .check(&context())
            .expect_err(row.case),
        };
        assert_eq!(refused.failure_code(), row.code, "{}", row.case);
        assert_eq!(refused.config_key(), row.config_key, "{}", row.case);
        assert_eq!(refused.problem(), row.problem, "{}", row.case);
        assert_eq!(
            refused.field().zip(Some(refused.blackout_id())),
            row.blackout,
            "{}",
            row.case
        );
    }

    // Through an instruction: a broken host blackout entry and a reversed pair.
    let running = from_file(json!({}), json!({}));
    for (changes, config_key) in [
        (
            vec![set(BLACKOUT_HOST, json!([night(json!([0]))]))],
            BLACKOUT_HOST,
        ),
        (
            vec![set(RANDOM_DELAY_MIN_MS, json!(5_000_000))],
            RANDOM_DELAY_MIN_MS,
        ),
    ] {
        let refused = running
            .apply(&changes, &stamp(), &context())
            .expect_err(config_key);
        assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
        assert_eq!(refused.config_key(), config_key);
        assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
        assert_eq!(refused.field(), None);
    }
    assert_eq!(
        running.scheduling(),
        from_file(json!({}), json!({})).scheduling()
    );
}

// The #405 order: an instance's random delay is its own `min_ms`, `max_ms` and `enabled`, or the
// section's where it names none; `min_ms` is at most `max_ms` as the pair is in effect, so a
// value that sets one side alone is checked against the other, at start and through an
// instruction; every member is settable while the Runtime runs.
#[test]
fn sr1_c3_an_instance_random_delay_is_checked_as_the_pair_in_effect() {
    let own = |members: Value| json!({"scheduling": {"random_delay": members}});
    let key = |member| instance_key(INSTANCE_A, member);

    // Its own shortest delay above the section's longest.
    let refused = from_file(json!({}), own(json!({"min_ms": 4_000_000})))
        .check(&context())
        .expect_err("a shortest delay above the longest in effect");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));

    // Its own shortest delay within the section's longest.
    let running = from_file(json!({}), own(json!({"min_ms": 120_000})));
    running
        .check(&context())
        .expect("a shortest delay below the longest in effect");
    assert_eq!(
        running.value(&key(RANDOM_DELAY_MIN_MS)),
        Some(&FactScalar::DurationMs(120_000))
    );
    assert_eq!(
        running.value(&key(RANDOM_DELAY_MAX_MS)),
        Some(&FactScalar::DurationMs(3_600_000))
    );

    // An instruction on one side is checked against the other side in effect.
    let refused = running
        .apply(
            &[set(&key(RANDOM_DELAY_MAX_MS), json!(60_000))],
            &stamp(),
            &context(),
        )
        .expect_err("a longest delay below the instance's shortest");
    assert_eq!(refused.failure_code(), ConfigCode::ConfigValueInvalid);
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));
    assert_eq!(refused.problem(), Some(ConfigValueProblem::SectionCheck));
    let refused = running
        .apply(
            &[set(RANDOM_DELAY_MAX_MS, json!(100_000))],
            &stamp(),
            &context(),
        )
        .expect_err("the section's longest below the instance's shortest");
    assert_eq!(refused.config_key(), key(RANDOM_DELAY_MIN_MS));

    let applied = running
        .apply(
            &[set(&key(RANDOM_DELAY_ENABLED), json!(true))],
            &stamp(),
            &context(),
        )
        .expect("enabling the instance's delay");
    assert_eq!(
        applied.config.value(&key(RANDOM_DELAY_ENABLED)),
        Some(&FactScalar::Boolean(true))
    );
    assert_eq!(
        applied.settings[0].source,
        ConfigParameterSource::Instruction
    );
}
